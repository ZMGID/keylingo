use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

use crate::external_agents::session::live::SessionCommand;
use crate::external_agents::spawn::{fold_stderr, join_stderr_tail};
use crate::external_agents::stream::{usage_from_parts, CliUsageParts};
use crate::external_agents::types::{ExternalCliSlashCommand, UnifiedAgentEvent};
use crate::proc::NoConsoleWindow;

/// Codex `app-server` speaks newline-delimited JSON-RPC over stdio (one JSON object per line,
/// no `Content-Length` framing). Responses omit the `jsonrpc` field, so we never require it.
async fn write_rpc(
    stdin: &mut tokio::process::ChildStdin,
    id: u64,
    method: &str,
    params: Value,
) -> Result<(), String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let mut line = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|e| e.to_string())
}

async fn write_rpc_result(
    stdin: &mut tokio::process::ChildStdin,
    id: &Value,
    result: Value,
) -> Result<(), String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    });
    let mut line = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|e| e.to_string())
}

/// Server → client approval requests are auto-approved. Each request method maps to a different
/// response shape (see the `*RequestApprovalResponse` schemas); return the matching approve value.
fn approval_response(method: &str) -> Option<Value> {
    match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            Some(json!({ "decision": "acceptForSession" }))
        }
        // Legacy exec/apply-patch approval requests use ReviewDecision.
        "execCommandApproval" | "applyPatchApproval" => {
            Some(json!({ "decision": "approved_for_session" }))
        }
        "item/permissions/requestApproval" => {
            Some(json!({ "permissions": {}, "scope": "session" }))
        }
        _ => None,
    }
}

/// Map a single codex app-server notification to zero or more `UnifiedAgentEvent`s. Returns `true`
/// when the notification signals the turn has ended (completed / failed).
fn map_codex_notification(
    method: &str,
    params: &Value,
    emitted_tools: &mut HashSet<String>,
    sink: &mut dyn FnMut(UnifiedAgentEvent),
) -> bool {
    match method {
        "item/agentMessage/delta" => {
            if let Some(delta) = params.get("delta").and_then(|v| v.as_str()) {
                if !delta.is_empty() {
                    sink(UnifiedAgentEvent::TextDelta {
                        delta: delta.to_string(),
                    });
                }
            }
        }
        "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
            if let Some(delta) = params.get("delta").and_then(|v| v.as_str()) {
                if !delta.is_empty() {
                    sink(UnifiedAgentEvent::ThinkingDelta {
                        delta: delta.to_string(),
                    });
                }
            }
        }
        "item/commandExecution/outputDelta" => {
            // Output streamed before the item completes; the completed item carries the
            // aggregated output we surface as the tool result, so deltas are not re-emitted.
        }
        "item/started" => {
            if let Some(item) = params.get("item").and_then(|v| v.as_object()) {
                emit_command_execution(item, emitted_tools, sink, false);
            }
        }
        "item/completed" => {
            if let Some(item) = params.get("item").and_then(|v| v.as_object()) {
                emit_command_execution(item, emitted_tools, sink, true);
            }
        }
        "thread/tokenUsage/updated" => {
            // `tokenUsage` 有 `last` 与 `total` 两份：
            //   last  = 最近一次请求的快照 → **上下文占用**口径，是用量条要的分子
            //   total = 整个 thread 的累计消耗 → 计费口径，随轮次单调增长
            // 用 total 当「已用上下文」会持续虚高，最终把进度条推满而实际远未满。
            // `last` 缺失时才退回 `total`（兼容旧版 codex）。
            //
            // 同层还有 `modelContextWindow`（与 last/total 平级）：codex 自报的**本轮真实窗口**。
            // 本机实测 258400，而静态表（`codex debug models` 的 context_window）是 272000，
            // 偏高 5.3%。CLI 实报优先于任何静态表（模型可能中途切换），走
            // `ModelUsage::context_window_tokens` 这条 L9 最高优先级通道。
            let model_context_window = params
                .get("tokenUsage")
                .and_then(|v| v.get("modelContextWindow"))
                .and_then(|v| v.as_u64())
                .filter(|tokens| *tokens > 0);
            if let Some(usage) = params
                .get("tokenUsage")
                .and_then(|v| v.get("last").or_else(|| v.get("total")))
                .and_then(|v| v.as_object())
            {
                let field = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
                let parts = CliUsageParts {
                    input: field("inputTokens"),
                    output: field("outputTokens"),
                    cache_read: field("cachedInputTokens"),
                    cache_creation: field("cacheWriteInputTokens"),
                    // **codex 的 cache 已含在 inputTokens 里**（OpenAI 口径），不能再加一遍。
                    // 本机实测原文（codex-cli 0.145.0, 2026-07-26）：
                    //   {"cacheWriteInputTokens":0,"cachedInputTokens":3456,"inputTokens":16865,
                    //    "outputTokens":7,"reasoningOutputTokens":0,"totalTokens":16872}
                    // 对账 16865 + 7 = 16872 = totalTokens ⇒ 3456 是 inputTokens 的子集。
                    // 当成不相交去加会得到 20328，虚高 20%。
                    cache_included_in_input: true,
                    // 刻意**不读** `reasoningOutputTokens`：同一样本里它是 0 而
                    // input+output 已恰好等于 totalTokens，说明推理 token 已含在 outputTokens 内。
                    context_window: model_context_window,
                    ..Default::default()
                };
                if parts.input > 0
                    || parts.output > 0
                    || parts.cache_read > 0
                    || parts.cache_creation > 0
                    // 窗口本身也是有效信息：一轮还没产生 token 时先把分母立起来，
                    // 好过让用量条继续吃静态表的近似值。
                    || parts.context_window.is_some()
                {
                    sink(UnifiedAgentEvent::Usage {
                        usage: usage_from_parts(parts),
                    });
                }
            }
        }
        "turn/completed" => {
            if let Some(turn) = params.get("turn").and_then(|v| v.as_object()) {
                if turn.get("status").and_then(|v| v.as_str()) == Some("failed") {
                    let message = turn
                        .get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("Codex turn failed");
                    sink(UnifiedAgentEvent::Error {
                        message: message.to_string(),
                    });
                }
            }
            return true;
        }
        // There is no `turn/failed` notification in the app-server protocol; failures arrive
        // either as a failed `turn/completed` (handled above) or as a top-level `error` /
        // `thread/realtime/error` notification. Surface those and end the loop.
        "error" | "thread/realtime/error" => {
            let message = params
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|v| v.as_str())
                .or_else(|| params.get("message").and_then(|v| v.as_str()))
                .unwrap_or("Codex error");
            sink(UnifiedAgentEvent::Error {
                message: message.to_string(),
            });
            return true;
        }
        _ => {}
    }
    false
}

/// A `commandExecution` ThreadItem (camelCase wire shape) maps to a Bash tool use / result.
fn emit_command_execution(
    item: &serde_json::Map<String, Value>,
    emitted_tools: &mut HashSet<String>,
    sink: &mut dyn FnMut(UnifiedAgentEvent),
    include_result: bool,
) {
    if item.get("type").and_then(|v| v.as_str()) != Some("commandExecution") {
        return;
    }
    let id = match item
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        Some(id) => id.to_string(),
        None => return,
    };
    let command = item
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if emitted_tools.insert(id.clone()) {
        sink(UnifiedAgentEvent::ToolUse {
            id: id.clone(),
            name: "Bash".to_string(),
            input: json!({ "command": command }),
        });
    }
    if !include_result {
        return;
    }
    let content = item
        .get("aggregatedOutput")
        .map(|value| match value {
            Value::String(s) => s.clone(),
            _ => value.to_string(),
        })
        .unwrap_or_default();
    let exit_code = item.get("exitCode").and_then(|v| v.as_i64());
    let status_failed = matches!(
        item.get("status").and_then(|v| v.as_str()),
        Some("failed") | Some("declined")
    );
    let is_error = exit_code.map(|code| code != 0).unwrap_or(status_failed);
    sink(UnifiedAgentEvent::ToolResult {
        tool_use_id: id,
        content,
        is_error,
    });
}

// ===========================================================================================
// Persistent session (Phase 2): keep the app-server process alive across turns.
// ===========================================================================================

/// `/compact` must NOT be sent as prompt text — codex treats it as a plain user message (the
/// model just role-plays a compaction while the real context keeps growing). The app-server
/// protocol compacts via the dedicated `thread/compact/start` RPC instead.
fn is_compact_slash(prompt: &str) -> bool {
    prompt.trim() == "/compact"
}

/// Normalize conversation-stored effort before `turn/start`.
///
/// Curated catalog dropped `none`/`minimal` and added `max`/`ultra`. Legacy sessions may still
/// hold the old ids — omit them rather than send a value the picker no longer offers.
pub fn normalize_codex_effort(raw: Option<&str>) -> Option<String> {
    let raw = raw.map(str::trim).filter(|s| !s.is_empty())?;
    let lower = raw.to_ascii_lowercase();
    match lower.as_str() {
        "default" | "none" | "minimal" | "off" | "auto" | "unset" => None,
        "low" | "medium" | "high" | "xhigh" | "max" | "ultra" => Some(lower),
        _ => None,
    }
}

/// Build the `turn/start` params, applying the per-turn `model` / reasoning `effort` (R4: codex
/// applies both every turn, so a mid-session switch takes effect on the next turn). Pure so the
/// per-turn application is unit-testable.
fn build_codex_turn_params(
    thread_id: &str,
    cwd: &str,
    input: Vec<Value>,
    model: Option<&str>,
    effort: Option<&str>,
) -> Value {
    let mut turn_params = json!({
        "threadId": thread_id,
        "input": input,
        "cwd": cwd,
        "approvalPolicy": "never",
    });
    if let Some(effort) = normalize_codex_effort(effort) {
        turn_params["effort"] = json!(effort);
    }
    if let Some(model) = model {
        turn_params["model"] = json!(model);
    }
    turn_params
}

/// A live Codex app-server connection: one `thread/start` (or `thread/resume`), then many
/// `turn/start` calls over the same process. Owned exclusively by its actor task.
pub struct CodexAppServerSession {
    child: Child,
    stdin: ChildStdin,
    reader: Lines<BufReader<ChildStdout>>,
    thread_id: String,
    cwd: String,
    next_id: u64,
    emitted_tools: HashSet<String>,
    /// 服务端当前活跃轮次的 turn id（取自任意一条带 `turnId` 的通知）。
    /// `turn/steer` 的 `expectedTurnId` 前置条件要用它；轮末清空。
    active_turn_id: Option<String>,
    /// Ring-buffered stderr tail (N1), joined on close / error for diagnostics.
    stderr_tail: tokio::task::JoinHandle<String>,
}

/// Handshake timeouts (缺陷 4 / R3): 30s each, up from 15/20s.
const CODEX_INITIALIZE_TIMEOUT: Duration = Duration::from_secs(30);
const CODEX_THREAD_START_TIMEOUT: Duration = Duration::from_secs(30);

impl CodexAppServerSession {
    /// Spawn `codex app-server`, `initialize`, then create or resume a thread. The process and
    /// thread persist for subsequent `run_turn` calls.
    pub async fn connect(
        resolved_bin: &Path,
        args: &[String],
        cwd: &Path,
        model: Option<&str>,
        sandbox: Option<&str>,
        resume_thread: Option<&str>,
    ) -> Result<Self, String> {
        let mut child = crate::external_agents::spawn::cli_command(resolved_bin)
            .args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .no_console_window()
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("spawn: {e}"))?;
        // N1: drain stderr for the process lifetime.
        let stderr_tail = crate::external_agents::spawn::spawn_stderr_tail(child.stderr.take());
        let mut stdin = match child.stdin.take() {
            Some(s) => s,
            None => {
                let tail = join_stderr_tail(&mut child, stderr_tail).await;
                return Err(fold_stderr("spawn: stdin unavailable".to_string(), &tail));
            }
        };
        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                let tail = join_stderr_tail(&mut child, stderr_tail).await;
                return Err(fold_stderr("spawn: stdout unavailable".to_string(), &tail));
            }
        };
        let mut reader = BufReader::new(stdout).lines();

        let cwd_str = cwd.to_string_lossy().to_string();
        let chosen_model = model.filter(|m| !m.is_empty() && *m != "default");

        let handshake = async {
            write_rpc(
                &mut stdin,
                1,
                "initialize",
                json!({ "clientInfo": { "name": "kivio", "title": "kivio", "version": "0" } }),
            )
            .await
            .map_err(|e| format!("initialize: {e}"))?;
            read_until_response(&mut reader, &mut stdin, 1, CODEX_INITIALIZE_TIMEOUT)
                .await
                .map_err(|e| format!("initialize: {e}"))?;

            let (method, mut params) = match resume_thread.filter(|t| !t.is_empty()) {
                Some(tid) => ("thread/resume", json!({ "threadId": tid })),
                None => (
                    "thread/start",
                    json!({
                        "cwd": cwd_str,
                        "sandbox": sandbox.filter(|s| !s.is_empty()).unwrap_or("workspace-write"),
                        "approvalPolicy": "never",
                    }),
                ),
            };
            if let Some(m) = chosen_model {
                params["model"] = json!(m);
            }
            write_rpc(&mut stdin, 2, method, params)
                .await
                .map_err(|e| format!("thread-start: {e}"))?;
            let result =
                read_until_response(&mut reader, &mut stdin, 2, CODEX_THREAD_START_TIMEOUT)
                    .await
                    .map_err(|e| format!("thread-start: {e}"))?;
            let thread_id = result
                .get("thread")
                .and_then(|t| t.get("id"))
                .and_then(|v| v.as_str())
                .or_else(|| result.get("threadId").and_then(|v| v.as_str()))
                .map(str::to_string)
                .ok_or_else(|| format!("thread-start: invalid {method} response"))?;
            Ok::<_, String>(thread_id)
        }
        .await;

        match handshake {
            Ok(thread_id) => Ok(Self {
                child,
                stdin,
                reader,
                thread_id,
                cwd: cwd_str,
                next_id: 3,
                emitted_tools: HashSet::new(),
                active_turn_id: None,
                stderr_tail,
            }),
            Err(msg) => {
                let tail = join_stderr_tail(&mut child, stderr_tail).await;
                Err(fold_stderr(msg, &tail))
            }
        }
    }

    pub fn thread_id(&self) -> &str {
        &self.thread_id
    }

    /// 常驻子进程的 pid。只作为注册表元数据（诊断 / 「两轮是不是同一个进程」），
    /// 关停一律走 actor 的 `Close`，绝不按 pid 杀。
    pub fn child_pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Run one turn over the live thread. Emits events into `events`; polls `control` so an
    /// incoming `Cancel` sends `turn/interrupt` (without killing the process). Does NOT close stdin.
    pub async fn run_turn(
        &mut self,
        prompt: &str,
        model: Option<&str>,
        reasoning: Option<&str>,
        images: &[crate::external_agents::attachments::ImageBlock],
        events: &mpsc::Sender<UnifiedAgentEvent>,
        control: &mut mpsc::Receiver<SessionCommand>,
    ) -> Result<(), String> {
        let chosen_model = model.filter(|m| !m.is_empty() && *m != "default");
        let chosen_effort = normalize_codex_effort(reasoning);
        let turn_id = self.next_id;
        self.next_id += 1;

        if is_compact_slash(prompt) {
            // Real compaction RPC; the server runs it as a turn (contextCompaction item +
            // turn/completed), so the read loop below works unchanged.
            write_rpc(
                &mut self.stdin,
                turn_id,
                "thread/compact/start",
                json!({ "threadId": self.thread_id }),
            )
            .await?;
        } else {
            // Codex reads images as `localImage` items pointing at on-disk files; copy each into a
            // private temp dir (its sandbox can't reach the conversation attachments dir).
            let mut input = vec![json!({ "type": "text", "text": prompt })];
            for path in crate::external_agents::attachments::materialize_images_to_tempdir(images) {
                input.push(json!({ "type": "localImage", "path": path.to_string_lossy() }));
            }
            let turn_params = build_codex_turn_params(
                &self.thread_id,
                &self.cwd,
                input,
                chosen_model,
                chosen_effort.as_deref(),
            );
            write_rpc(&mut self.stdin, turn_id, "turn/start", turn_params).await?;
        }

        // 已发出、还在等响应的 `turn/steer`（rpc_id → (steer_id, 文本, 回执通道)）。
        // 受理与否只有响应说得准，所以 oneshot 在这里排队、由读循环兑付。
        let mut pending_steers: std::collections::HashMap<
            u64,
            (String, String, oneshot::Sender<bool>),
        > = std::collections::HashMap::new();
        loop {
            match control.try_recv() {
                Ok(SessionCommand::Cancel) => {
                    let iid = self.next_id;
                    self.next_id += 1;
                    let _ = write_rpc(
                        &mut self.stdin,
                        iid,
                        "turn/interrupt",
                        json!({ "threadId": self.thread_id }),
                    )
                    .await;
                    return Err("cancelled".to_string());
                }
                Ok(SessionCommand::Close) => return Err("closed".to_string()),
                Ok(SessionCommand::Steer {
                    id,
                    text,
                    kind: crate::external_agents::session::live::MessageInjectionKind::Steer,
                    accepted,
                    ..
                }) => {
                    // `turn/steer` 往**在飞的**这一轮追加用户输入（不新起一轮、不发
                    // turn/started）。`expectedTurnId` 是前置条件，必须等于服务端当前活跃的
                    // turn id —— 那是服务端给的字符串，不是我们的 JSON-RPC 请求 id，所以只能
                    // 从通知里抓（每条通知的 params 都带 turnId）。还没抓到就说明这一轮还没
                    // 真正开始，此时无从注入，回 false 让调用方按普通消息在轮末发。
                    match self.active_turn_id.clone() {
                        Some(expected_turn_id) => {
                            let rpc_id = self.next_id;
                            self.next_id += 1;
                            let params = json!({
                                "threadId": self.thread_id,
                                "input": [{ "type": "text", "text": text }],
                                "expectedTurnId": expected_turn_id,
                            });
                            match write_rpc(&mut self.stdin, rpc_id, "turn/steer", params).await {
                                // 受理与否要等它的**响应**（review / compact 轮次会被拒），
                                // 所以在这里只登记，由下面的读循环兑付这个 oneshot。
                                Ok(()) => {
                                    pending_steers.insert(rpc_id, (id, text, accepted));
                                }
                                Err(_) => {
                                    let _ = accepted.send(false);
                                }
                            }
                        }
                        None => {
                            let _ = accepted.send(false);
                        }
                    }
                }
                Ok(SessionCommand::Steer { accepted, .. }) => {
                    let _ = accepted.send(false);
                }
                Ok(SessionCommand::RunTurn { done, .. }) => {
                    let _ = done.send(Err("session busy".to_string()));
                }
                Ok(SessionCommand::PiSession { reply, .. }) => {
                    let _ = reply.send(Err("Pi session commands are unsupported".to_string()));
                }
                // codex 无后台任务协议（stop_task 是 claude 专属），忽略。
                Ok(SessionCommand::StopTask { .. }) => {}
                Err(mpsc::error::TryRecvError::Empty) => {}
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    return Err("control channel closed".to_string())
                }
            }

            let line = match timeout(Duration::from_millis(200), self.reader.next_line()).await {
                Ok(Ok(Some(l))) => l,
                Ok(Ok(None)) => return Err("codex app-server exited mid-turn".to_string()),
                Ok(Err(e)) => return Err(e.to_string()),
                Err(_) => continue,
            };
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = match serde_json::from_str(line.trim()) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if let (Some(method), Some(id)) = (
                value.get("method").and_then(|v| v.as_str()),
                value.get("id"),
            ) {
                if let Some(result) = approval_response(method) {
                    write_rpc_result(&mut self.stdin, id, result).await?;
                }
                continue;
            }
            if let Some(method) = value.get("method").and_then(|v| v.as_str()) {
                let params = value.get("params").cloned().unwrap_or(Value::Null);
                // 服务端给的活跃 turn id：每条 turn 相关通知的 params 都带 turnId。
                // `turn/steer` 的 `expectedTurnId` 只能取自这里。
                if let Some(turn) = params.get("turnId").and_then(|v| v.as_str()) {
                    self.active_turn_id = Some(turn.to_string());
                }
                let mut buf: Vec<UnifiedAgentEvent> = Vec::new();
                let ended =
                    map_codex_notification(method, &params, &mut self.emitted_tools, &mut |e| {
                        buf.push(e)
                    });
                for e in buf {
                    let _ = events.send(e).await;
                }
                if ended {
                    self.active_turn_id = None;
                    return Ok(());
                }
                continue;
            }
            // `turn/steer` 的响应。**必须排在下面那条通用 error 分支之前**：被拒的 steer
            // （review / compact 轮次不可 steer、expectedTurnId 已过期）回的是带 id 的
            // error，落到通用分支会把整轮判死 —— 用户只是插话没插上，不该赔掉这一轮。
            if let Some(rpc_id) = value.get("id").and_then(Value::as_u64) {
                if let Some((steer_id, steer_text, accepted)) = pending_steers.remove(&rpc_id) {
                    let ok = value.get("result").is_some();
                    let _ = accepted.send(ok);
                    if ok {
                        // 受理了才在时间线上留卡（卡的语义是「这句话确实进了模型输入」）。
                        let _ = events
                            .send(UnifiedAgentEvent::UserSteer {
                                id: steer_id,
                                text: steer_text,
                            })
                            .await;
                    }
                    continue;
                }
            }
            if let Some(err) = value.get("error") {
                let message = err
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| err.to_string());
                let _ = events
                    .send(UnifiedAgentEvent::Error {
                        message: message.clone(),
                    })
                    .await;
                return Err(message);
            }
            // Response to turn/start (or a stale id): the turn is now running — keep reading.
        }
    }

    /// Close stdin and kill the process.
    pub async fn close(mut self) {
        let _ = self.stdin.shutdown().await;
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
        let _ = self.stderr_tail.await;
    }
}

/// Read JSON-RPC lines until the response with `target_id` arrives, auto-answering any
/// server→client approval requests and skipping notifications.
async fn read_until_response(
    reader: &mut Lines<BufReader<ChildStdout>>,
    stdin: &mut ChildStdin,
    target_id: u64,
    overall: Duration,
) -> Result<Value, String> {
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > overall {
            return Err("codex app-server handshake timeout".to_string());
        }
        let line = match timeout(Duration::from_millis(200), reader.next_line()).await {
            Ok(Ok(Some(l))) => l,
            Ok(Ok(None)) => return Err("codex app-server exited during handshake".to_string()),
            Ok(Err(e)) => return Err(e.to_string()),
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let (Some(method), Some(id)) = (
            value.get("method").and_then(|v| v.as_str()),
            value.get("id"),
        ) {
            if let Some(result) = approval_response(method) {
                write_rpc_result(stdin, id, result).await?;
            }
            continue;
        }
        if value.get("method").is_some() {
            continue; // notification
        }
        if let Some(err) = value.get("error") {
            return Err(err
                .get("message")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| err.to_string()));
        }
        if value.get("id").and_then(|v| v.as_u64()) == Some(target_id) {
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

/// Curated codex built-in slash commands (not exposed via any list RPC). Merged with the
/// dynamic `skills/list` results for the slash popover.
const CODEX_BUILTIN_COMMANDS: &[(&str, &str)] = &[
    ("compact", "压缩对话历史"),
    ("diff", "查看改动 diff"),
    ("init", "生成 AGENTS.md"),
    ("model", "切换模型"),
    ("approvals", "审批策略"),
    ("review", "审查改动"),
    ("status", "会话状态"),
    ("mcp", "MCP server 状态"),
    ("new", "新会话"),
    ("undo", "撤销上一步"),
];

/// Result of a one-shot Codex model catalog probe (app-server `model/list`).
///
/// Aligns with desktop-cc-gui: runtime list is authoritative; each model carries its own
/// `supportedReasoningEfforts`. `reasoning_options` is a convenience union / default-model
/// slice for callers that only want a flat list.
#[derive(Debug, Clone)]
pub struct CodexModelsProbe {
    pub models: Vec<crate::external_agents::types::RuntimeModelOption>,
    pub reasoning_by_model:
        std::collections::HashMap<String, Vec<crate::external_agents::types::RuntimeModelOption>>,
    pub reasoning_options: Vec<crate::external_agents::types::RuntimeModelOption>,
}

/// **Selectable** Codex catalog — word-for-word the 4 entries in desktop-cc-gui
/// `generatedModelCatalog.json` → `engines.codex`.
///
/// This is what users actually see in cc-gui when `model/list` is empty/degraded
/// (workspace not connected): sol / terra / luna / gpt-5.5. Live `model/list` on
/// current CLI returns a *different* short set (5.5/5.4/5.4-mini/5.3-codex/5.2) and
/// **omits** gpt-5.6-* — so we do **not** dump that list into the picker. Runtime is
/// only used to enrich efforts/labels for ids that already sit in this curated table.
const CODEX_CURATED_CATALOG: &[(&str, &str, &[&str])] = &[
    (
        "gpt-5.6-sol",
        "gpt-5.6-sol",
        &["low", "medium", "high", "xhigh", "max", "ultra"],
    ),
    (
        "gpt-5.6-terra",
        "gpt-5.6-terra",
        &["low", "medium", "high", "xhigh", "max", "ultra"],
    ),
    (
        "gpt-5.6-luna",
        "gpt-5.6-luna",
        &["low", "medium", "high", "xhigh", "max"],
    ),
    ("gpt-5.5", "gpt-5.5", &["low", "medium", "high", "xhigh"]),
];

fn effort_options(ids: &[&str]) -> Vec<crate::external_agents::types::RuntimeModelOption> {
    use crate::external_agents::types::RuntimeModelOption;
    ids.iter()
        .map(|id| RuntimeModelOption {
            id: (*id).to_string(),
            label: title_case_effort(id),
            context_window_tokens: None,
        })
        .collect()
}

/// Build the picker list the way desktop-cc-gui does in practice for most users:
///
/// 1. **Curated 4** (generated catalog) as the selectable set / order  
/// 2. If runtime `model/list` has the **same id**, overwrite label / efforts / window  
/// 3. If `config.toml` model is still missing, inject it after Auto  
///
/// Deliberately does **not** append every runtime-only id (gpt-5.4 / 5.2 / …) — that
/// is what made Kivio show a junk list while cc-gui showed the clean 4.
pub fn merge_codex_model_catalog(
    runtime: CodexModelsProbe,
    config_model: Option<&str>,
) -> CodexModelsProbe {
    use crate::external_agents::types::{default_model_option, RuntimeModelOption};

    let runtime_by_id: std::collections::HashMap<&str, &RuntimeModelOption> = runtime
        .models
        .iter()
        .filter(|m| m.id != "default")
        .map(|m| (m.id.as_str(), m))
        .collect();

    let mut models = vec![default_model_option()];
    let mut reasoning_by_model = std::collections::HashMap::new();
    let mut seen = HashSet::new();
    seen.insert("default".to_string());

    for (id, label, efforts) in CODEX_CURATED_CATALOG {
        seen.insert((*id).to_string());
        // Runtime enrichment when the same catalog id appears in model/list.
        if let Some(rt) = runtime_by_id.get(*id) {
            models.push(RuntimeModelOption {
                id: (*id).to_string(),
                label: if rt.label.trim().is_empty() {
                    (*label).to_string()
                } else {
                    rt.label.clone()
                },
                context_window_tokens: rt.context_window_tokens,
            });
            if let Some(opts) = runtime.reasoning_by_model.get(*id) {
                if !opts.is_empty() {
                    reasoning_by_model.insert((*id).to_string(), opts.clone());
                    continue;
                }
            }
        } else {
            models.push(RuntimeModelOption {
                id: (*id).to_string(),
                label: (*label).to_string(),
                context_window_tokens: None,
            });
        }
        reasoning_by_model.insert((*id).to_string(), effort_options(efforts));
    }

    // config.toml model missing from curated set → inject (cc-gui same behavior).
    if let Some(cfg) = config_model
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "default")
    {
        if !seen.contains(cfg) {
            models.insert(
                1,
                RuntimeModelOption {
                    id: cfg.to_string(),
                    label: format!("{cfg} (config)"),
                    context_window_tokens: None,
                },
            );
            reasoning_by_model.insert(
                cfg.to_string(),
                effort_options(&["low", "medium", "high", "xhigh", "max", "ultra"]),
            );
        }
    }

    let reasoning_options = config_model
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|cfg| reasoning_by_model.get(cfg).cloned())
        .filter(|o| !o.is_empty())
        .or_else(|| reasoning_by_model.get("gpt-5.6-sol").cloned())
        .or_else(|| {
            models
                .iter()
                .filter(|m| m.id != "default")
                .find_map(|m| reasoning_by_model.get(&m.id).cloned())
        })
        .unwrap_or_default();

    CodexModelsProbe {
        models,
        reasoning_by_model,
        reasoning_options,
    }
}

/// When model/list / debug models both fail — still serve the curated 4.
pub fn codex_static_fallback_probe() -> CodexModelsProbe {
    merge_codex_model_catalog(
        CodexModelsProbe {
            models: vec![crate::external_agents::types::default_model_option()],
            reasoning_by_model: std::collections::HashMap::new(),
            reasoning_options: Vec::new(),
        },
        None,
    )
}

/// Discover Codex models via app-server JSON-RPC `model/list` (same path as desktop-cc-gui).
///
/// Spawns a short-lived `codex app-server`, `initialize`s, then `model/list` — does **not**
/// create a thread (cheaper than a full chat session). Failure → `None` so the caller can
/// fall back to `codex debug models` or the static catalog.
pub async fn detect_codex_models(
    resolved_bin: &Path,
    cwd: &Path,
    timeout_secs: u64,
) -> Option<CodexModelsProbe> {
    let mut child = crate::external_agents::spawn::cli_command(resolved_bin)
        .arg("app-server")
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .no_console_window()
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let (mut stdin, stdout) = (child.stdin.take()?, child.stdout.take()?);
    let mut reader = BufReader::new(stdout).lines();
    let overall = Duration::from_secs(timeout_secs);

    let ok = write_rpc(
        &mut stdin,
        1,
        "initialize",
        json!({ "clientInfo": { "name": "kivio", "title": "kivio", "version": "0" } }),
    )
    .await
    .is_ok()
        && read_until_response(&mut reader, &mut stdin, 1, overall)
            .await
            .is_ok()
        && write_rpc(&mut stdin, 2, "model/list", json!({}))
            .await
            .is_ok();

    let probe = if ok {
        read_until_response(&mut reader, &mut stdin, 2, overall)
            .await
            .ok()
            .and_then(|result| parse_codex_model_list_result(&result))
    } else {
        None
    };

    let _ = child.start_kill();
    let _ = child.wait().await;
    probe
}

/// Parse a `model/list` **result** object (`{ "data": [ ... ] }`).
///
/// Field names match the live app-server (camelCase). We also accept snake_case for
/// older / relay shapes. Hidden models are dropped. A synthetic `default` row is prepended
/// (Kivio Auto = don't pin a model in Turn/start).
pub fn parse_codex_model_list_result(result: &Value) -> Option<CodexModelsProbe> {
    use crate::external_agents::types::{default_model_option, RuntimeModelOption};

    let data = result
        .get("data")
        .or_else(|| result.get("models"))
        .and_then(|v| v.as_array())?;

    let mut models = Vec::new();
    let mut reasoning_by_model = std::collections::HashMap::new();
    let mut default_model_id: Option<String> = None;
    let mut seen = HashSet::new();

    for entry in data {
        if entry.get("hidden").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        let Some(id) = entry
            .get("id")
            .or_else(|| entry.get("model"))
            .or_else(|| entry.get("slug"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
        else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        let label = entry
            .get("displayName")
            .or_else(|| entry.get("display_name"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(id.as_str())
            .to_string();
        let context_window_tokens = entry
            .get("contextWindow")
            .or_else(|| entry.get("context_window"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        let is_default = entry
            .get("isDefault")
            .or_else(|| entry.get("is_default"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if is_default && default_model_id.is_none() {
            default_model_id = Some(id.clone());
        }

        let efforts = parse_codex_reasoning_efforts(entry);
        if !efforts.is_empty() {
            reasoning_by_model.insert(id.clone(), efforts);
        }

        models.push(RuntimeModelOption {
            id,
            label,
            context_window_tokens,
        });
    }

    if models.is_empty() {
        return None;
    }

    // Prefer server-declared default at the front of the real catalog (after Auto).
    if let Some(default_id) = default_model_id.as_deref() {
        if let Some(pos) = models.iter().position(|m| m.id == default_id) {
            if pos > 0 {
                let m = models.remove(pos);
                models.insert(0, m);
            }
        }
    }

    let mut out = vec![default_model_option()];
    // Auto inherits the first real model's window when known.
    if let Some(first) = models.first() {
        out[0].context_window_tokens = first.context_window_tokens;
    }
    out.extend(models);

    // Flat effort list: default model's efforts, else first model that has any.
    let reasoning_options = default_model_id
        .as_ref()
        .and_then(|id| reasoning_by_model.get(id).cloned())
        .or_else(|| {
            out.iter()
                .filter(|m| m.id != "default")
                .find_map(|m| reasoning_by_model.get(&m.id).cloned())
        })
        .unwrap_or_default();

    Some(CodexModelsProbe {
        models: out,
        reasoning_by_model,
        reasoning_options,
    })
}

/// Extract per-model effort options from a model/list or debug-models entry.
fn parse_codex_reasoning_efforts(
    entry: &Value,
) -> Vec<crate::external_agents::types::RuntimeModelOption> {
    use crate::external_agents::types::RuntimeModelOption;

    let levels = entry
        .get("supportedReasoningEfforts")
        .or_else(|| entry.get("supported_reasoning_efforts"))
        .or_else(|| entry.get("supported_reasoning_levels"))
        .and_then(|v| v.as_array());
    let Some(levels) = levels else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for level in levels {
        let id = level
            .get("reasoningEffort")
            .or_else(|| level.get("reasoning_effort"))
            .or_else(|| level.get("effort"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(id) = id else { continue };
        if !seen.insert(id.to_string()) {
            continue;
        }
        let label = level
            .get("description")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|d| format!("{id} — {d}"))
            .unwrap_or_else(|| title_case_effort(id));
        out.push(RuntimeModelOption {
            id: id.to_string(),
            label,
            context_window_tokens: None,
        });
    }
    out
}

fn title_case_effort(id: &str) -> String {
    let mut chars = id.chars();
    match chars.next() {
        Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
        None => id.to_string(),
    }
}

/// Discover codex slash commands: curated built-ins + dynamic skills from `skills/list`.
pub async fn detect_codex_commands(
    resolved_bin: &Path,
    cwd: &Path,
    timeout_secs: u64,
) -> Option<Vec<ExternalCliSlashCommand>> {
    let mut out: Vec<ExternalCliSlashCommand> = CODEX_BUILTIN_COMMANDS
        .iter()
        .map(|(name, desc)| ExternalCliSlashCommand {
            slash: format!("/{name}"),
            name: (*name).to_string(),
            description: Some((*desc).to_string()),
            argument_hint: None,
        })
        .collect();

    // Best-effort: pull skills via the app-server. Failure leaves just the built-ins.
    if let Ok(mut child) = crate::external_agents::spawn::cli_command(resolved_bin)
        .arg("app-server")
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .no_console_window()
        .kill_on_drop(true)
        .spawn()
    {
        if let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) {
            let mut reader = BufReader::new(stdout).lines();
            let overall = Duration::from_secs(timeout_secs);
            let ok = write_rpc(
                &mut stdin,
                1,
                "initialize",
                json!({ "clientInfo": { "name": "kivio", "title": "kivio", "version": "0" } }),
            )
            .await
            .is_ok()
                && read_until_response(&mut reader, &mut stdin, 1, overall)
                    .await
                    .is_ok()
                && write_rpc(&mut stdin, 2, "skills/list", json!({}))
                    .await
                    .is_ok();
            if ok {
                if let Ok(result) = read_until_response(&mut reader, &mut stdin, 2, overall).await {
                    let mut seen: HashSet<String> = out.iter().map(|c| c.name.clone()).collect();
                    if let Some(groups) = result.get("data").and_then(|v| v.as_array()) {
                        for group in groups {
                            let Some(skills) = group.get("skills").and_then(|v| v.as_array())
                            else {
                                continue;
                            };
                            for skill in skills {
                                let Some(name) = skill
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                else {
                                    continue;
                                };
                                if seen.insert(name.to_string()) {
                                    out.push(ExternalCliSlashCommand {
                                        slash: format!("/{name}"),
                                        name: name.to_string(),
                                        description: skill
                                            .get("description")
                                            .and_then(|v| v.as_str())
                                            .map(|d| d.trim().to_string())
                                            .filter(|d| !d.is_empty()),
                                        argument_hint: None,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        let _ = child.start_kill();
        let _ = child.wait().await;
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    Some(out)
}

/// Spawn the actor task that owns a connected session and serves `SessionCommand`s.
pub fn spawn_codex_session_actor(
    mut session: CodexAppServerSession,
) -> mpsc::Sender<SessionCommand> {
    let (tx, mut rx) = mpsc::channel::<SessionCommand>(8);
    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                SessionCommand::RunTurn {
                    prompt,
                    model,
                    reasoning,
                    images,
                    events,
                    done,
                    // codex 侧还没有权限审批（目前只有 claude 走 stdio 控制通道），忽略即可 ——
                    // 通道从来不会被建起来（`run.rs::turn_asks_for_permission` 只对带
                    // `--permission-prompt-tool` 的 argv 为真，那是 claude 专属 flag）。
                    approvals: _,
                } => {
                    // Invariant (A4): `run_turn` sends every `event` before returning, and mpsc
                    // preserves order, so the caller's post-`done` drain sees them all. `done.send`
                    // stays LAST.
                    let result = session
                        .run_turn(
                            &prompt,
                            model.as_deref(),
                            reasoning.as_deref(),
                            &images,
                            &events,
                            &mut rx,
                        )
                        .await;
                    let _ = done.send(result);
                }
                // 轮次之间没有可注入的对象：回 false 让前端把这条留在队列里、
                // 轮末按普通消息发出去（绝不静默吞掉）。
                SessionCommand::Steer { accepted, .. } => {
                    let _ = accepted.send(false);
                }
                SessionCommand::PiSession { reply, .. } => {
                    let _ = reply.send(Err("Pi session commands are unsupported".to_string()));
                }
                SessionCommand::Cancel => {} // no active turn between turns
                // codex 无后台任务协议，忽略。
                SessionCommand::StopTask { .. } => {}
                SessionCommand::Close => {
                    session.close().await;
                    return;
                }
            }
        }
        session.close().await;
    });
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(method: &str, raw: &str) -> (Vec<UnifiedAgentEvent>, bool) {
        let params: Value = serde_json::from_str(raw).unwrap();
        let mut events = Vec::new();
        let mut tools = HashSet::new();
        let ended = map_codex_notification(method, &params, &mut tools, &mut |e| events.push(e));
        (events, ended)
    }

    /// Shape mirrors live `codex app-server` → `model/list` (2026-08 probe).
    #[test]
    fn parse_model_list_result_matches_cc_gui_shape() {
        let result = json!({
            "data": [
                {
                    "id": "gpt-5.5",
                    "model": "gpt-5.5",
                    "displayName": "GPT-5.5",
                    "description": "Frontier model",
                    "hidden": false,
                    "isDefault": true,
                    "supportedReasoningEfforts": [
                        {"reasoningEffort": "low", "description": "Fast"},
                        {"reasoningEffort": "medium", "description": "Balanced"},
                        {"reasoningEffort": "high", "description": "Deep"},
                        {"reasoningEffort": "xhigh", "description": "Extra"}
                    ],
                    "defaultReasoningEffort": "medium"
                },
                {
                    "id": "gpt-5.4-mini",
                    "model": "gpt-5.4-mini",
                    "displayName": "GPT-5.4-Mini",
                    "hidden": false,
                    "isDefault": false,
                    "supportedReasoningEfforts": [
                        {"reasoningEffort": "low", "description": "Fast"},
                        {"reasoningEffort": "high", "description": "Deep"}
                    ],
                    "defaultReasoningEffort": "medium"
                },
                {
                    "id": "hidden-model",
                    "model": "hidden-model",
                    "displayName": "Hidden",
                    "hidden": true
                }
            ]
        });
        let probe = parse_codex_model_list_result(&result).unwrap();
        // Auto + two visible models; hidden dropped.
        assert_eq!(probe.models.len(), 3);
        assert_eq!(probe.models[0].id, "default");
        // isDefault model is first real entry.
        assert_eq!(probe.models[1].id, "gpt-5.5");
        assert_eq!(probe.models[1].label, "GPT-5.5");
        assert!(probe.models.iter().any(|m| m.id == "gpt-5.4-mini"));
        assert!(!probe.models.iter().any(|m| m.id == "hidden-model"));
        // per-model efforts
        let gpt55 = probe.reasoning_by_model.get("gpt-5.5").unwrap();
        assert_eq!(gpt55.len(), 4);
        assert!(gpt55.iter().any(|e| e.id == "xhigh"));
        let mini = probe.reasoning_by_model.get("gpt-5.4-mini").unwrap();
        assert_eq!(mini.len(), 2);
        // flat options come from the default model
        assert!(probe.reasoning_options.iter().any(|e| e.id == "medium"));
        assert!(probe.reasoning_options.iter().any(|e| e.id == "xhigh"));
    }

    #[test]
    fn parse_model_list_empty_or_all_hidden_is_none() {
        assert!(parse_codex_model_list_result(&json!({"data": []})).is_none());
        assert!(parse_codex_model_list_result(&json!({
            "data": [{"id": "x", "hidden": true}]
        }))
        .is_none());
    }

    #[test]
    fn merge_uses_curated_four_like_cc_gui_not_raw_model_list() {
        // Live model/list on this machine — 5 ids, no gpt-5.6-*. Must NOT dump these.
        let runtime = parse_codex_model_list_result(&json!({
            "data": [
                {
                    "id": "gpt-5.5",
                    "displayName": "GPT-5.5",
                    "isDefault": true,
                    "supportedReasoningEfforts": [
                        {"reasoningEffort": "low", "description": "Fast"},
                        {"reasoningEffort": "high", "description": "Deep"}
                    ]
                },
                {"id": "gpt-5.4", "displayName": "gpt-5.4"},
                {"id": "gpt-5.4-mini", "displayName": "GPT-5.4-Mini"},
                {"id": "gpt-5.3-codex", "displayName": "gpt-5.3-codex"},
                {"id": "gpt-5.2", "displayName": "gpt-5.2"}
            ]
        }))
        .unwrap();

        let merged = merge_codex_model_catalog(runtime, Some("gpt-5.6-sol"));
        // curated four (+ Auto)
        let ids: Vec<&str> = merged.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "default",
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna",
                "gpt-5.5",
            ]
        );
        // runtime-only junk must not appear
        assert!(!merged.models.iter().any(|m| m.id == "gpt-5.4"));
        assert!(!merged.models.iter().any(|m| m.id == "gpt-5.2"));
        // runtime enriches gpt-5.5 label + efforts
        assert_eq!(
            merged
                .models
                .iter()
                .find(|m| m.id == "gpt-5.5")
                .unwrap()
                .label,
            "GPT-5.5"
        );
        assert_eq!(merged.reasoning_by_model.get("gpt-5.5").unwrap().len(), 2);
        // sol keeps curated ultra ladder
        assert!(merged
            .reasoning_by_model
            .get("gpt-5.6-sol")
            .unwrap()
            .iter()
            .any(|e| e.id == "ultra"));
    }

    #[test]
    fn normalize_codex_effort_drops_legacy_and_keeps_valid() {
        assert_eq!(normalize_codex_effort(None), None);
        assert_eq!(normalize_codex_effort(Some("")), None);
        assert_eq!(normalize_codex_effort(Some("default")), None);
        assert_eq!(normalize_codex_effort(Some("none")), None);
        assert_eq!(normalize_codex_effort(Some("minimal")), None);
        assert_eq!(normalize_codex_effort(Some("off")), None);
        assert_eq!(normalize_codex_effort(Some("bogus")), None);
        assert_eq!(
            normalize_codex_effort(Some("high")).as_deref(),
            Some("high")
        );
        assert_eq!(
            normalize_codex_effort(Some("XHIGH")).as_deref(),
            Some("xhigh")
        );
        assert_eq!(
            normalize_codex_effort(Some("ultra")).as_deref(),
            Some("ultra")
        );
    }

    #[test]
    fn merge_injects_unknown_config_model_after_auto() {
        let runtime = parse_codex_model_list_result(&json!({
            "data": [{"id": "gpt-5.5", "displayName": "GPT-5.5", "isDefault": true}]
        }))
        .unwrap();
        let merged = merge_codex_model_catalog(runtime, Some("my-custom-proxy-model"));
        assert_eq!(merged.models[0].id, "default");
        assert_eq!(merged.models[1].id, "my-custom-proxy-model");
        assert!(merged.models[1].label.contains("config"));
        // still the curated four after the config inject
        assert!(merged.models.iter().any(|m| m.id == "gpt-5.6-sol"));
        assert!(merged.models.iter().any(|m| m.id == "gpt-5.5"));
    }

    /// Live cross-turn continuity: connect once, run two turns on the SAME process, and confirm
    /// turn 2 recalls a fact stated only in turn 1 — proving the codex thread persists between
    /// turns (Phase 2). Requires a logged-in `codex` CLI + network.
    #[tokio::test]
    #[ignore = "requires live codex login + network"]
    async fn persistent_session_remembers_across_turns() {
        use crate::external_agents::session::live::SessionCommand;
        use tokio::sync::{mpsc, oneshot};

        let bin = which_codex().expect("codex on PATH");
        let cwd = std::env::temp_dir();
        let session = CodexAppServerSession::connect(
            &bin,
            &["app-server".to_string()],
            &cwd,
            None,
            None,
            None,
        )
        .await
        .expect("connect codex app-server");
        let thread_id = session.thread_id().to_string();
        assert!(!thread_id.is_empty());
        let control = spawn_codex_session_actor(session);

        async fn one_turn(control: &mpsc::Sender<SessionCommand>, prompt: &str) -> String {
            let (etx, mut erx) = mpsc::channel::<UnifiedAgentEvent>(64);
            let (dtx, drx) = oneshot::channel();
            control
                .send(SessionCommand::RunTurn {
                    prompt: prompt.to_string(),
                    model: None,
                    reasoning: None,
                    images: vec![],
                    events: etx,
                    done: dtx,
                    approvals: None,
                })
                .await
                .unwrap();
            let mut text = String::new();
            // Drain events until the turn's `done` fires.
            let mut drx = drx;
            loop {
                tokio::select! {
                    biased;
                    r = &mut drx => { while let Ok(e) = erx.try_recv() { if let UnifiedAgentEvent::TextDelta { delta } = e { text.push_str(&delta); } } r.unwrap().unwrap(); break; }
                    ev = erx.recv() => { if let Some(UnifiedAgentEvent::TextDelta { delta }) = ev { text.push_str(&delta); } }
                }
            }
            text
        }

        let _t1 = one_turn(&control, "Remember this secret number: 42. Just reply OK.").await;
        let t2 = one_turn(
            &control,
            "What was the secret number I just gave you? Reply with only the digits.",
        )
        .await;
        eprintln!("turn2 reply: {t2:?}");
        assert!(
            t2.contains("42"),
            "turn 2 should recall 42 from turn 1, got: {t2:?}"
        );
        let _ = control.send(SessionCommand::Close).await;
    }

    fn which_codex() -> Option<std::path::PathBuf> {
        let out = std::process::Command::new("which")
            .arg("codex")
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if p.is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(p))
        }
    }

    #[tokio::test]
    #[ignore = "requires live codex CLI on PATH"]
    async fn live_detect_codex_commands() {
        let bin = which_codex().expect("codex on PATH");
        let cmds = detect_codex_commands(&bin, &std::env::temp_dir(), 12)
            .await
            .expect("codex commands");
        eprintln!("codex commands: {}", cmds.len());
        for c in cmds.iter().take(12) {
            eprintln!("  {}", c.slash);
        }
        // At least the curated built-ins must be present.
        assert!(cmds.iter().any(|c| c.name == "compact"));
    }

    #[test]
    fn agent_message_delta_emits_text() {
        let (events, ended) = collect(
            "item/agentMessage/delta",
            r#"{"delta":"hi","itemId":"i","threadId":"t","turnId":"u"}"#,
        );
        assert!(!ended);
        assert!(matches!(
            events.first(),
            Some(UnifiedAgentEvent::TextDelta { delta }) if delta == "hi"
        ));
    }

    #[test]
    fn reasoning_deltas_emit_thinking() {
        let (summary, _) = collect(
            "item/reasoning/summaryTextDelta",
            r#"{"delta":"plan","itemId":"i","summaryIndex":0,"threadId":"t","turnId":"u"}"#,
        );
        assert!(matches!(
            summary.first(),
            Some(UnifiedAgentEvent::ThinkingDelta { delta }) if delta == "plan"
        ));
        let (text, _) = collect(
            "item/reasoning/textDelta",
            r#"{"delta":"think","contentIndex":0,"itemId":"i","threadId":"t","turnId":"u"}"#,
        );
        assert!(matches!(
            text.first(),
            Some(UnifiedAgentEvent::ThinkingDelta { delta }) if delta == "think"
        ));
    }

    #[test]
    fn command_execution_emits_tool_use_and_result() {
        let started = r#"{"item":{"type":"commandExecution","id":"cmd-1","command":"ls","status":"inProgress"},"startedAtMs":0,"threadId":"t","turnId":"u"}"#;
        let completed = r#"{"item":{"type":"commandExecution","id":"cmd-1","command":"ls","aggregatedOutput":"ok\n","exitCode":0,"status":"completed"},"completedAtMs":1,"threadId":"t","turnId":"u"}"#;
        let started_val: Value = serde_json::from_str(started).unwrap();
        let completed_val: Value = serde_json::from_str(completed).unwrap();
        let mut events = Vec::new();
        let mut tools = HashSet::new();
        map_codex_notification("item/started", &started_val, &mut tools, &mut |e| {
            events.push(e)
        });
        map_codex_notification("item/completed", &completed_val, &mut tools, &mut |e| {
            events.push(e)
        });
        assert!(matches!(
            events.first(),
            Some(UnifiedAgentEvent::ToolUse { id, name, .. }) if id == "cmd-1" && name == "Bash"
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            UnifiedAgentEvent::ToolResult { tool_use_id, content, is_error }
                if tool_use_id == "cmd-1" && content.contains("ok") && !*is_error
        )));
    }

    #[test]
    fn token_usage_emits_usage() {
        let (events, _) = collect(
            "thread/tokenUsage/updated",
            r#"{"threadId":"t","turnId":"u","tokenUsage":{"last":{"cachedInputTokens":0,"inputTokens":5,"outputTokens":7,"reasoningOutputTokens":0,"totalTokens":12},"total":{"cachedInputTokens":0,"inputTokens":5,"outputTokens":7,"reasoningOutputTokens":0,"totalTokens":12}}}"#,
        );
        assert!(events
            .iter()
            .any(|event| matches!(event, UnifiedAgentEvent::Usage { .. })));
    }

    fn only_usage(events: &[UnifiedAgentEvent]) -> crate::chat::model::ModelUsage {
        events
            .iter()
            .find_map(|e| match e {
                UnifiedAgentEvent::Usage { usage } => Some(usage.clone()),
                _ => None,
            })
            .expect("应产出 Usage 事件")
    }

    #[test]
    fn token_usage_prefers_last_snapshot_over_cumulative_total() {
        // 第三轮时 total 已累计到远高于本轮实际上下文占用的量级。用量条要的是 last。
        let (events, _) = collect(
            "thread/tokenUsage/updated",
            r#"{"threadId":"t","turnId":"u","tokenUsage":{
                 "last":{"cacheWriteInputTokens":0,"cachedInputTokens":40000,"inputTokens":41200,"outputTokens":300,"reasoningOutputTokens":250,"totalTokens":41500},
                 "total":{"cacheWriteInputTokens":0,"cachedInputTokens":180000,"inputTokens":189000,"outputTokens":2400,"reasoningOutputTokens":1800,"totalTokens":191400}}}"#,
        );
        let usage = only_usage(&events);
        assert_eq!(
            usage.input_tokens,
            Some(41_200),
            "取 last，不是 total(189000)"
        );
        assert_eq!(usage.output_tokens, Some(300));
        assert_eq!(usage.cached_input_tokens, Some(40_000));
        // codex 的 cachedInputTokens 是 inputTokens 的子集（实测对账见解析处注释）：
        // 41200 + 300 = 41500，不得把 40000 再加一遍变成 81500，也不得加 reasoning 的 250。
        assert_eq!(usage.total_tokens, Some(41_500));
    }

    /// 钉住 codex 的 cache 包含关系。这条直接复刻本机实测原文（codex-cli 0.145.0,
    /// 2026-07-26），任何人把 `cache_included_in_input` 改回 false 都会让它变红。
    #[test]
    fn token_usage_matches_live_codex_total_exactly() {
        let (events, _) = collect(
            "thread/tokenUsage/updated",
            r#"{"threadId":"t","turnId":"u","tokenUsage":{
                 "last":{"cacheWriteInputTokens":0,"cachedInputTokens":3456,"inputTokens":16865,"outputTokens":7,"reasoningOutputTokens":0,"totalTokens":16872},
                 "modelContextWindow":258400,
                 "total":{"cacheWriteInputTokens":0,"cachedInputTokens":3456,"inputTokens":16865,"outputTokens":7,"reasoningOutputTokens":0,"totalTokens":16872}}}"#,
        );
        let usage = only_usage(&events);
        // 必须与 codex 自报的 totalTokens 完全一致——这是「口径对了」的唯一硬判据。
        assert_eq!(usage.total_tokens, Some(16_872));
        assert_eq!(usage.cached_input_tokens, Some(3_456));
        // 分母也来自同一条 payload：codex 自报 258400，而静态表（`codex debug models`
        // 的 context_window）是 272000，偏高 5.3%。实报优先。
        assert_eq!(usage.context_window_tokens, Some(258_400));
    }

    #[test]
    fn token_usage_reports_window_even_before_any_token_is_spent() {
        // 一轮开头 token 还是 0，但窗口已知——此时也要把分母立起来，
        // 否则用量条会先吃一段静态表的近似值再跳变。
        let (events, _) = collect(
            "thread/tokenUsage/updated",
            r#"{"threadId":"t","turnId":"u","tokenUsage":{
                 "last":{"cachedInputTokens":0,"inputTokens":0,"outputTokens":0,"totalTokens":0},
                 "modelContextWindow":258400,
                 "total":{"cachedInputTokens":0,"inputTokens":0,"outputTokens":0,"totalTokens":0}}}"#,
        );
        let usage = only_usage(&events);
        assert_eq!(usage.context_window_tokens, Some(258_400));
    }

    #[test]
    fn token_usage_without_model_context_window_leaves_denominator_unset() {
        // 旧版 codex 不报 modelContextWindow：不得凭空造一个，交给 L9 的静态表兜。
        let (events, _) = collect(
            "thread/tokenUsage/updated",
            r#"{"threadId":"t","turnId":"u","tokenUsage":{
                 "last":{"cachedInputTokens":0,"inputTokens":16,"outputTokens":7,"totalTokens":23}}}"#,
        );
        let usage = only_usage(&events);
        assert_eq!(usage.context_window_tokens, None);
    }

    #[test]
    fn token_usage_falls_back_to_total_when_last_absent() {
        let (events, _) = collect(
            "thread/tokenUsage/updated",
            r#"{"threadId":"t","turnId":"u","tokenUsage":{
                 "total":{"cachedInputTokens":11,"inputTokens":16,"outputTokens":7,"totalTokens":23}}}"#,
        );
        let usage = only_usage(&events);
        assert_eq!(usage.input_tokens, Some(16));
        assert_eq!(usage.cached_input_tokens, Some(11));
        assert_eq!(usage.total_tokens, Some(23));
    }

    #[test]
    fn token_usage_emits_when_only_cache_is_nonzero() {
        // 极端形态防御：只有 cache 字段非零（inputTokens 为 0）时仍要上报，不能静默丢弃。
        // 注意 codex 实测不会出现这种形态（cache ⊆ input），这里纯粹是解析层的健壮性。
        let (events, _) = collect(
            "thread/tokenUsage/updated",
            r#"{"threadId":"t","turnId":"u","tokenUsage":{
                 "last":{"cachedInputTokens":52000,"inputTokens":0,"outputTokens":0,"totalTokens":52000}}}"#,
        );
        assert!(only_usage(&events).cached_input_tokens == Some(52_000));
    }

    #[test]
    fn turn_completed_ends_loop() {
        let (_, ended) = collect(
            "turn/completed",
            r#"{"threadId":"t","turn":{"id":"u","items":[],"status":"completed"}}"#,
        );
        assert!(ended);
    }

    #[test]
    fn turn_failed_emits_error_and_ends() {
        let (events, ended) = collect(
            "turn/completed",
            r#"{"threadId":"t","turn":{"id":"u","items":[],"status":"failed","error":{"message":"boom"}}}"#,
        );
        assert!(ended);
        assert!(events.iter().any(|event| matches!(
            event,
            UnifiedAgentEvent::Error { message, .. } if message == "boom"
        )));
    }

    #[test]
    fn error_notification_emits_error_and_ends() {
        let (events, ended) = collect("error", r#"{"error":{"message":"fatal"}}"#);
        assert!(ended);
        assert!(events.iter().any(|event| matches!(
            event,
            UnifiedAgentEvent::Error { message, .. } if message == "fatal"
        )));
    }

    fn event_variant(event: &UnifiedAgentEvent) -> &'static str {
        match event {
            UnifiedAgentEvent::TextDelta { .. } => "TextDelta",
            UnifiedAgentEvent::ThinkingDelta { .. } => "ThinkingDelta",
            UnifiedAgentEvent::ToolUse { .. } => "ToolUse",
            UnifiedAgentEvent::ToolResult { .. } => "ToolResult",
            UnifiedAgentEvent::Usage { .. } => "Usage",
            UnifiedAgentEvent::Error { .. } => "Error",
            UnifiedAgentEvent::Raw { .. } => "Raw",
            UnifiedAgentEvent::SlashCommands { .. } => "SlashCommands",
            UnifiedAgentEvent::CliCompacted { .. } => "CliCompacted",
            UnifiedAgentEvent::UserSteer { .. } => "UserSteer",
            UnifiedAgentEvent::UserFollowUp { .. } => "UserFollowUp",
            UnifiedAgentEvent::StatusNote { .. } => "StatusNote",
            UnifiedAgentEvent::BackgroundTask { .. } => "BackgroundTask",
            UnifiedAgentEvent::TodoWrite { .. } => "TodoWrite",
            UnifiedAgentEvent::SubagentProgress { .. } => "SubagentProgress",
        }
    }

    /// 起一个真机 codex 常驻会话（生产路径：`connect` + `spawn_codex_session_actor`），
    /// 跑一轮并收齐事件。`None` = 本机没有可用的 codex，调用方 skip。
    ///
    /// 此前这两条真机测试驱动的是 `run_codex_app_server_session` —— 一个只被它们自己吊着命的
    /// 一次性驱动，即同一协议的第二份实现。改成驱动生产代码后那份实现被删掉了。
    async fn live_codex_turn(prompt: &str, wall_clock: Duration) -> Option<Vec<UnifiedAgentEvent>> {
        let bin = match crate::external_agents::spawn::resolve_binary(
            &crate::external_agents::defs::codex::CODEX_AGENT_DEF,
        )
        .await
        {
            Some(bin) => bin,
            None => {
                eprintln!("SKIP: 本机没有可用的 codex CLI");
                return None;
            }
        };
        let cwd = std::env::temp_dir();
        let session = match CodexAppServerSession::connect(
            &bin,
            &["app-server".to_string()],
            &cwd,
            None,
            None,
            None,
        )
        .await
        {
            Ok(session) => session,
            Err(err) => {
                eprintln!("SKIP: 连接失败（未登录 / 网络？）：{err}");
                return None;
            }
        };
        let control = spawn_codex_session_actor(session);
        let (events_tx, mut events_rx) = tokio::sync::mpsc::channel::<UnifiedAgentEvent>(256);
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        control
            .send(SessionCommand::RunTurn {
                prompt: prompt.to_string(),
                model: None,
                reasoning: None,
                images: Vec::new(),
                events: events_tx,
                done: done_tx,
                approvals: None,
            })
            .await
            .expect("actor alive");
        let collector = tokio::spawn(async move {
            let mut out = Vec::new();
            while let Some(event) = events_rx.recv().await {
                out.push(event);
            }
            out
        });
        match tokio::time::timeout(wall_clock, done_rx).await {
            Ok(Ok(Ok(()))) => eprintln!("turn: Ok"),
            Ok(Ok(Err(err))) => eprintln!("turn: Err({err})"),
            Ok(Err(_)) => eprintln!("turn: actor dropped the done channel"),
            Err(_) => panic!("codex app-server session HUNG past the {wall_clock:?} guard"),
        }
        Some(collector.await.expect("collector task"))
    }

    /// 真机验证「运行中立刻引导」：codex 的 `turn/steer` 往**在飞的**这一轮追加用户输入。
    ///
    /// 这条测的是三件只有真 app-server 能证明的事：
    ///   1. `expectedTurnId` 拿的是**服务端**给的 turn id（我们从通知的 `turnId` 抓），
    ///      用我们自己的 JSON-RPC 请求 id 会被判前置条件不符；
    ///   2. 受理回执要等 `turn/steer` 的**响应**，不是写完就算；
    ///   3. 被拒的 steer 回的是带 id 的 error，**不能**把整轮判死（读循环里那条
    ///      `pending_steers` 分支必须排在通用 error 分支之前）。
    #[tokio::test]
    #[ignore = "requires live codex login + network"]
    async fn codex_turn_steer_injects_into_the_running_turn() {
        let bin = match crate::external_agents::spawn::resolve_binary(
            &crate::external_agents::defs::codex::CODEX_AGENT_DEF,
        )
        .await
        {
            Some(bin) => bin,
            None => {
                eprintln!("SKIP: 本机没有可用的 codex CLI");
                return;
            }
        };
        let cwd = std::env::temp_dir();
        let session = match CodexAppServerSession::connect(
            &bin,
            &["app-server".to_string()],
            &cwd,
            None,
            None,
            None,
        )
        .await
        {
            Ok(session) => session,
            Err(err) => {
                eprintln!("SKIP: 连接失败（未登录 / 网络？）：{err}");
                return;
            }
        };
        let control = spawn_codex_session_actor(session);
        let (events_tx, mut events_rx) = tokio::sync::mpsc::channel::<UnifiedAgentEvent>(256);
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        control
            .send(SessionCommand::RunTurn {
                // 让这一轮够长，好在它跑着的时候插话（数到 5 会连着出好几段 reasoning/text）。
                prompt: "Count slowly from 1 to 5, one number per line, then say COUNT_DONE."
                    .to_string(),
                model: None,
                reasoning: None,
                images: Vec::new(),
                events: events_tx,
                done: done_tx,
                approvals: None,
            })
            .await
            .expect("actor alive");

        // 等到服务端真的开了这一轮（我们抓到 turnId）再插话。太早插 = 没有活跃 turn，
        // 按设计会回 false —— 那是正确行为，但测不到注入。
        let steer_control = control.clone();
        let steered = tokio::spawn(async move {
            for _ in 0..40 {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
                if steer_control
                    .send(SessionCommand::Steer {
                        id: "steer-live-1".to_string(),
                        text: "Change of plan: stop counting and reply STEER_OK.".to_string(),
                        images: Vec::new(),
                        kind: crate::external_agents::session::live::MessageInjectionKind::Steer,
                        accepted: accepted_tx,
                    })
                    .await
                    .is_err()
                {
                    return false;
                }
                if accepted_rx.await.unwrap_or(false) {
                    return true;
                }
            }
            false
        });

        let collector = tokio::spawn(async move {
            let mut out = Vec::new();
            while let Some(event) = events_rx.recv().await {
                out.push(event);
            }
            out
        });
        match tokio::time::timeout(Duration::from_secs(180), done_rx).await {
            Ok(Ok(Ok(()))) => eprintln!("turn: Ok"),
            Ok(Ok(Err(err))) => eprintln!("turn: Err({err})"),
            Ok(Err(_)) => eprintln!("turn: actor dropped the done channel"),
            Err(_) => panic!("codex app-server session HUNG past the guard"),
        }
        let accepted = steered.await.expect("steer task");
        let captured = collector.await.expect("collector task");
        let seq: Vec<&str> = captured.iter().map(event_variant).collect();
        eprintln!("codex steer sequence: {seq:?}");
        for (i, ev) in captured.iter().enumerate() {
            eprintln!("[{i}] {ev:?}");
        }

        assert!(accepted, "turn/steer 未被受理（seq: {seq:?}）");
        let steer_event = captured.iter().any(|event| {
            matches!(event, UnifiedAgentEvent::UserSteer { id, text }
                if id == "steer-live-1" && text.contains("STEER_OK"))
        });
        assert!(
            steer_event,
            "受理了却没发 UserSteer 事件（时间线上就不会有插话卡）：{seq:?}"
        );
        // 被拒的 steer 不该赔掉整轮；受理的更不该。这一轮必须仍然正常产出内容。
        assert!(
            captured
                .iter()
                .any(|e| matches!(e, UnifiedAgentEvent::TextDelta { .. })),
            "插话之后这一轮没有任何正文，疑似被 steer 的响应误判成致命错误：{seq:?}"
        );
    }

    #[tokio::test]
    #[ignore = "requires live codex login + network"]
    async fn codex_app_server_smoke() {
        let Some(captured) = live_codex_turn(
            "Reply with exactly the token SMOKE_OK and nothing else.",
            Duration::from_secs(90),
        )
        .await
        else {
            return;
        };
        eprintln!("=== codex app-server smoke: {} events ===", captured.len());
        for (i, ev) in captured.iter().enumerate() {
            eprintln!("[{i}] {ev:?}");
        }
        let seq: Vec<&str> = captured.iter().map(event_variant).collect();
        eprintln!("codex sequence: {seq:?}");

        let got_text = captured
            .iter()
            .any(|e| matches!(e, UnifiedAgentEvent::TextDelta { .. }));
        let got_error = captured
            .iter()
            .any(|e| matches!(e, UnifiedAgentEvent::Error { .. }));
        assert!(
            got_text || got_error,
            "expected at least one TextDelta or a clean Error, got: {seq:?}"
        );
    }

    /// Live proof that L4 reads the `last` snapshot, not the cumulative `total`.
    ///
    /// 单测只能证明「给定这样的 JSON 会取 last」；这条证明真实 codex 确实**发**了
    /// `last` 且它与 `total` 在多轮下会分叉。跑两轮同一 thread：
    /// `total` 单调累加，`last` 只反映最近一次请求 —— 若 Kivio 读回 total，
    /// 第二轮的用量会包含第一轮，进度条持续虚高。
    #[tokio::test]
    #[ignore = "requires live codex login + network"]
    async fn codex_usage_uses_last_snapshot_not_cumulative_total() {
        let Some(captured) = live_codex_turn(
            "Reply with exactly the token USAGE_OK and nothing else.",
            Duration::from_secs(120),
        )
        .await
        else {
            return;
        };
        let usages: Vec<crate::chat::model::ModelUsage> = captured
            .into_iter()
            .filter_map(|e| match e {
                UnifiedAgentEvent::Usage { usage } => Some(usage),
                _ => None,
            })
            .collect();
        for u in &usages {
            eprintln!(
                "codex usage: input={:?} output={:?} cache_read={:?} total={:?} window={:?}",
                u.input_tokens,
                u.output_tokens,
                u.cached_input_tokens,
                u.total_tokens,
                u.context_window_tokens
            );
        }
        assert!(
            !usages.is_empty(),
            "codex reported no usage — thread/tokenUsage/updated parsing regressed"
        );

        // **口径硬判据**：codex 的 `cachedInputTokens` 是 `inputTokens` 的子集
        // （实测 16865 + 7 = 16872 = 其自报的 totalTokens），所以 Kivio 算出的
        // total 必须恰好等于 input + output。把 cache 再加一遍会让这条变红——
        // 那正是曾经真实发生过的 bug（20328 vs 16872，虚高 20%）。
        for u in &usages {
            let input = u.input_tokens.unwrap_or(0);
            let output = u.output_tokens.unwrap_or(0);
            let cache = u.cached_input_tokens.unwrap_or(0);
            assert_eq!(
                u.total_tokens,
                Some(input + output),
                "codex total must equal input+output (cache is a subset of input): {u:?}"
            );
            assert!(
                cache <= input,
                "cachedInputTokens should never exceed inputTokens: {u:?}"
            );
        }

        // `last` 是快照不是累计：多条上报时最后一条不应等于各条之和（读回 total 时会）。
        if usages.len() > 1 {
            let sum: u64 = usages.iter().filter_map(|u| u.input_tokens).sum();
            let last = usages.last().and_then(|u| u.input_tokens).unwrap_or(0);
            eprintln!(
                "codex usage reports={} sum_input={sum} last_input={last}",
                usages.len()
            );
            assert!(
                last < sum,
                "last snapshot must be strictly below the sum of all reports (else it is cumulative)"
            );
        }
    }

    #[test]
    fn build_codex_turn_params_applies_model_and_effort_per_turn() {
        let params = build_codex_turn_params(
            "thread-1",
            "/work",
            vec![json!({ "type": "text", "text": "hi" })],
            Some("gpt-5.3-codex"),
            Some("high"),
        );
        assert_eq!(params["threadId"], json!("thread-1"));
        assert_eq!(params["model"], json!("gpt-5.3-codex"));
        assert_eq!(params["effort"], json!("high"));
        assert_eq!(params["approvalPolicy"], json!("never"));
    }

    #[test]
    fn build_codex_turn_params_omits_defaults() {
        let params = build_codex_turn_params(
            "thread-1",
            "/work",
            vec![json!({ "type": "text", "text": "hi" })],
            None,
            None,
        );
        assert!(params.get("model").is_none());
        assert!(params.get("effort").is_none());
    }

    #[test]
    fn approval_response_shapes() {
        assert_eq!(
            approval_response("item/commandExecution/requestApproval"),
            Some(json!({ "decision": "acceptForSession" }))
        );
        assert_eq!(
            approval_response("item/fileChange/requestApproval"),
            Some(json!({ "decision": "acceptForSession" }))
        );
        assert_eq!(
            approval_response("item/permissions/requestApproval"),
            Some(json!({ "permissions": {}, "scope": "session" }))
        );
        assert!(approval_response("item/started").is_none());
    }
}
