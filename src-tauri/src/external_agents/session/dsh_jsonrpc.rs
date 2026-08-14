//! DeepSeek Harness SDK JSON-RPC persistent session.
//!
//! dsh 没有「一条命令直接出流式 JSON」的模式，它只能 boot profile。本模块拉起
//! `dsh --profile kivio`（profile 由 `dsh_profile.rs` 维护），再驱动
//! Kivio 自带的 resumable JSON-RPC bridge（profile 由 `dsh_profile.rs` 维护）。
//!
//! # 线协议（0.1.0-rc.6 实测）
//!
//! 客户端请求：
//! - `initialize { cwd, provider, model, maxTokens? }`
//! - `session/open { sessionId, resume }`
//! - `session/prompt { sessionId, contentBlocks }`
//! - `session/cancel { sessionId }`
//! - `shutdown`
//!
//! 服务端把完整的持久会话日志广播成 `session.event`，另有 `session.status`。一轮工具循环
//! 会产生多个 step，所以 `assistant/chunk.finish` **不是轮终点**；真正终点是匹配 session 的
//! `turn/end`，随后 `session.status: idle` 表示整台 agent 静止。
//!
//! 服务端广播**运行时里的所有 session**（包含子代理），必须按 `params.sessionId` 过滤，
//! 否则子代理正文会串进父气泡。Kivio bridge 直接调用 dsh 公共的 `agents.resume()` 与
//! `agent.cancel()`，所以进程重建和用户停止都不会再丢失原生会话。

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, Mutex};
use tokio::time::timeout;
use uuid::Uuid;

use crate::chat::model::ModelUsage;
use crate::external_agents::session::live::{SessionCommand, CANCELLED_SESSION_LOST};
use crate::external_agents::types::UnifiedAgentEvent;
use crate::proc::NoConsoleWindow;

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(45);
const READ_POLL: Duration = Duration::from_millis(200);
const CANCEL_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
const DEFAULT_PROVIDER: &str = "deepseek-official";
const DEFAULT_MODEL: &str = "deepseek-v4-flash";

// reasoningEffort 只能写进共享的 profiles/kivio patch。锁必须覆盖「写 patch → spawn →
// initialize」整个窗口；只锁文件写入仍会让并发启动的另一轮在进程读配置前把值换掉。
static DSH_PROFILE_BOOT_LOCK: Mutex<()> = Mutex::const_new(());

/// 运行中的 dsh 连接：一个进程可承载同一个 session 的多轮 prompt。
pub struct DshJsonRpcSession {
    child: Child,
    stdin: ChildStdin,
    reader: Lines<BufReader<ChildStdout>>,
    stderr_tail: tokio::task::JoinHandle<String>,
    session_id: String,
    resumed: bool,
    next_id: u64,
    /// `initialize` 时实际固定给 agent 的 route/model。现有 SDK 没有 session 级换模型方法；
    /// 调用方的启动指纹应在变化时换进程，这里再做一道 fail-loud 防线。
    route: ModelRoute,
    /// 最近一条 `request/context.contextWindow`，附到后续 usage 上作为权威分母。
    context_window: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelRoute {
    provider: String,
    model: String,
}

impl DshJsonRpcSession {
    /// 生成 Kivio profile、拉起 dsh，并完成 `initialize` 握手。
    pub async fn connect(
        resolved_bin: &Path,
        args: &[String],
        cwd: &Path,
        resume_session_id: Option<&str>,
        model: Option<&str>,
        reasoning: Option<&str>,
        sandbox: Option<&str>,
    ) -> Result<Self, String> {
        let _profile_boot_guard = DSH_PROFILE_BOOT_LOCK.lock().await;
        crate::external_agents::dsh_profile::ensure_profile_ready(resolved_bin, reasoning).await?;

        let route = resolve_model_route_for_turn(model)?;
        let session_id = resume_session_id
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("kivio-{}", Uuid::new_v4()));
        let wants_resume = resume_session_id.is_some_and(|id| !id.trim().is_empty());
        let mut command = crate::external_agents::spawn::cli_command(resolved_bin);
        command
            .args(args)
            .current_dir(cwd)
            .env("DSH_TELEMETRY_DISABLED", "1")
            .env("DSH_PERMISSION_MODE", normalize_sandbox(sandbox))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .no_console_window()
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|e| format!("spawn dsh: {e}"))?;
        let stderr_tail = crate::external_agents::spawn::spawn_stderr_tail(child.stderr.take());
        let mut stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                let tail =
                    crate::external_agents::spawn::join_stderr_tail(&mut child, stderr_tail).await;
                return Err(crate::external_agents::spawn::fold_stderr(
                    "spawn dsh: stdin unavailable".to_string(),
                    &tail,
                ));
            }
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                let tail =
                    crate::external_agents::spawn::join_stderr_tail(&mut child, stderr_tail).await;
                return Err(crate::external_agents::spawn::fold_stderr(
                    "spawn dsh: stdout unavailable".to_string(),
                    &tail,
                ));
            }
        };
        let mut reader = BufReader::new(stdout).lines();

        let handshake = async {
            write_rpc(
                &mut stdin,
                1,
                "initialize",
                json!({
                    "cwd": cwd.to_string_lossy(),
                    "provider": route.provider,
                    "model": route.model,
                }),
            )
            .await
            .map_err(|e| format!("dsh initialize: {e}"))?;
            let result = read_until_response(&mut reader, 1, INITIALIZE_TIMEOUT)
                .await
                .map_err(|e| format!("dsh initialize: {e}"))?;
            let name = result
                .get("serverInfo")
                .and_then(|v| v.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let resumable = result
                .get("capabilities")
                .and_then(|value| value.get("resume"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let cancellable = result
                .get("capabilities")
                .and_then(|value| value.get("cancel"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if name != "kivio-dsh-sdk-runtime" || !resumable || !cancellable {
                return Err(format!(
                    "dsh initialize: Kivio resumable bridge unavailable (server={name:?})"
                ));
            }

            write_rpc(
                &mut stdin,
                2,
                "session/open",
                json!({ "sessionId": session_id, "resume": wants_resume }),
            )
            .await
            .map_err(|e| format!("dsh session/open: {e}"))?;
            let opened = read_until_response(&mut reader, 2, INITIALIZE_TIMEOUT)
                .await
                .map_err(|e| format!("dsh session/open: {e}"))?;
            let resumed = opened
                .get("resumed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if resumed != wants_resume {
                return Err(format!(
                    "dsh session/open: resume mismatch (requested={wants_resume}, actual={resumed})"
                ));
            }
            Ok::<bool, String>(resumed)
        }
        .await;

        match handshake {
            Ok(resumed) => Ok(Self {
                child,
                stdin,
                reader,
                stderr_tail,
                session_id,
                resumed,
                next_id: 3,
                route,
                context_window: None,
            }),
            Err(message) => {
                let tail =
                    crate::external_agents::spawn::join_stderr_tail(&mut child, stderr_tail).await;
                Err(crate::external_agents::spawn::fold_stderr(message, &tail))
            }
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn resumed(&self) -> bool {
        self.resumed
    }

    pub fn child_pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// 在同一个 live agent 上执行一轮，直到匹配 session 的 `turn/end` + `status:idle`。
    pub async fn run_turn(
        &mut self,
        prompt: &str,
        model: Option<&str>,
        events: &mpsc::Sender<UnifiedAgentEvent>,
        control: &mut mpsc::Receiver<SessionCommand>,
    ) -> Result<(), String> {
        let requested_route = resolve_model_route_for_turn(model)?;
        if requested_route != self.route {
            // 防止未来有人把 dsh 从启动指纹里删掉后，UI 显示新模型、实际 agent 仍跑旧模型。
            return Err(crate::external_agents::session::acp::NEEDS_RECONNECT.to_string());
        }

        let prompt_id = self.next_id;
        self.next_id += 1;
        write_rpc(
            &mut self.stdin,
            prompt_id,
            "session/prompt",
            json!({
                "sessionId": self.session_id,
                "contentBlocks": [{ "type": "text", "text": prompt }],
            }),
        )
        .await?;

        let mut started = false;
        let mut prompt_acknowledged = false;
        let mut terminal: Option<Result<(), String>> = None;
        let mut cancel_requested = false;
        let mut cancel_id: Option<u64> = None;
        let mut cancel_started: Option<std::time::Instant> = None;

        loop {
            match control.try_recv() {
                Ok(SessionCommand::Cancel) => cancel_requested = true,
                Ok(SessionCommand::Close) => return Err("closed".to_string()),
                Ok(SessionCommand::Steer { accepted, .. }) => {
                    let _ = accepted.send(false);
                }
                Ok(SessionCommand::RunTurn { done, .. }) => {
                    let _ = done.send(Err("session busy".to_string()));
                }
                Ok(SessionCommand::StopTask { .. }) => {}
                Err(mpsc::error::TryRecvError::Empty) => {}
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    return Err("control channel closed".to_string());
                }
            }

            // The transport dispatches JSON-RPC lines concurrently. Cancelling before the prompt ACK
            // can hit an idle agent, return success, and then let the prompt start afterwards.
            if cancel_requested && prompt_acknowledged && cancel_id.is_none() {
                let id = self.next_id;
                self.next_id += 1;
                if write_rpc(
                    &mut self.stdin,
                    id,
                    "session/cancel",
                    json!({ "sessionId": self.session_id }),
                )
                .await
                .is_err()
                {
                    crate::external_agents::spawn::kill_agent_process_tree(&mut self.child);
                    let _ = self.child.wait().await;
                    return Err(CANCELLED_SESSION_LOST.to_string());
                }
                cancel_id = Some(id);
                cancel_started = Some(std::time::Instant::now());
            }

            let line = match timeout(READ_POLL, self.reader.next_line()).await {
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) => return Err("dsh exited mid-turn".to_string()),
                Ok(Err(err)) => return Err(format!("read dsh: {err}")),
                Err(_) => {
                    if cancel_started.is_some_and(|started| started.elapsed() >= CANCEL_TIMEOUT) {
                        crate::external_agents::spawn::kill_agent_process_tree(&mut self.child);
                        let _ = self.child.wait().await;
                        return Err(CANCELLED_SESSION_LOST.to_string());
                    }
                    continue;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = match serde_json::from_str(line.trim()) {
                Ok(value) => value,
                Err(_) => {
                    let _ = events.send(UnifiedAgentEvent::Raw { line }).await;
                    continue;
                }
            };

            if cancel_id.is_some() && value.get("id").and_then(Value::as_u64) == cancel_id {
                if rpc_error_message(&value).is_some() {
                    crate::external_agents::spawn::kill_agent_process_tree(&mut self.child);
                    let _ = self.child.wait().await;
                    return Err(CANCELLED_SESSION_LOST.to_string());
                }
                return Err("cancelled".to_string());
            }

            if value.get("id").and_then(Value::as_u64) == Some(prompt_id) {
                if let Some(error) = rpc_error_message(&value) {
                    return Err(error);
                }
                prompt_acknowledged = true;
                // `session/prompt` 的 result 只是入队回执（messageId），不是轮终点。
                continue;
            }

            let Some(method) = value.get("method").and_then(Value::as_str) else {
                continue;
            };
            let params = value.get("params").unwrap_or(&Value::Null);
            if params.get("sessionId").and_then(Value::as_str) != Some(self.session_id.as_str()) {
                // 这条协议广播 runtime 中每个 session（子代理也在里面）。严格隔离父会话。
                continue;
            }

            match method {
                "session.status" => match params.get("status").and_then(Value::as_str) {
                    Some("running") => started = true,
                    Some("idle") if started && terminal.is_some() && cancel_id.is_none() => {
                        return terminal.take().expect("checked above");
                    }
                    _ => {}
                },
                "session.event" => {
                    let Some(event) = params.get("event") else {
                        continue;
                    };
                    let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
                    let data = event.get("data").unwrap_or(&Value::Null);
                    if event_type == "turn/start" {
                        started = true;
                    }
                    let mut mapped = Vec::new();
                    if let Some(result) = map_session_event(
                        event_type,
                        data,
                        &mut self.context_window,
                        &mut |event| mapped.push(event),
                    ) {
                        terminal = Some(result);
                    }
                    for event in mapped {
                        let _ = events.send(event).await;
                    }
                }
                _ => {}
            }

            // 防御：服务端必须先确认 prompt 入队。若未来协议漏掉回执但仍跑完一轮，不能把
            // 另一个 producer 的 idle 当成本轮成功；现版实测 ack 总在 step/start 前后到达。
            if terminal.is_some() && !prompt_acknowledged {
                continue;
            }
        }
    }

    /// 优先走协议 shutdown，再有界等待；没退出就杀进程组。
    pub async fn close(mut self) {
        let id = self.next_id;
        let _ = write_rpc(&mut self.stdin, id, "shutdown", Value::Null).await;
        let _ = self.stdin.shutdown().await;
        if timeout(SHUTDOWN_GRACE, self.child.wait()).await.is_err() {
            crate::external_agents::spawn::kill_agent_process_tree(&mut self.child);
            let _ = self.child.wait().await;
        }
        let _ = self.stderr_tail.await;
    }
}

/// actor 与其他常驻协议同契约：所有 event 先入队，`done` 最后发。
pub fn spawn_dsh_session_actor(mut session: DshJsonRpcSession) -> mpsc::Sender<SessionCommand> {
    let (tx, mut rx) = mpsc::channel::<SessionCommand>(8);
    tokio::spawn(async move {
        while let Some(command) = rx.recv().await {
            match command {
                SessionCommand::RunTurn {
                    prompt,
                    model,
                    reasoning: _,
                    images: _,
                    events,
                    done,
                    approvals: _,
                } => {
                    let result = session
                        .run_turn(&prompt, model.as_deref(), &events, &mut rx)
                        .await;
                    let _ = done.send(result);
                }
                SessionCommand::Steer { accepted, .. } => {
                    let _ = accepted.send(false);
                }
                SessionCommand::Cancel => {}
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

async fn write_rpc(
    stdin: &mut ChildStdin,
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
        .map_err(|e| format!("write dsh: {e}"))?;
    stdin.flush().await.map_err(|e| format!("flush dsh: {e}"))
}

async fn read_until_response(
    reader: &mut Lines<BufReader<ChildStdout>>,
    target_id: u64,
    overall: Duration,
) -> Result<Value, String> {
    let started = std::time::Instant::now();
    loop {
        if started.elapsed() > overall {
            return Err(format!("handshake timeout after {}s", overall.as_secs()));
        }
        let line = match timeout(READ_POLL, reader.next_line()).await {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => return Err("dsh exited during handshake".to_string()),
            Ok(Err(err)) => return Err(format!("read dsh handshake: {err}")),
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(line.trim()) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if value.get("id").and_then(Value::as_u64) != Some(target_id) {
            continue;
        }
        if let Some(error) = rpc_error_message(&value) {
            return Err(error);
        }
        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
    }
}

fn rpc_error_message(value: &Value) -> Option<String> {
    let error = value.get("error")?;
    Some(
        error
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| error.to_string()),
    )
}

pub fn is_missing_session_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("dsh session/open:")
        && (lower.contains("session \"") && lower.contains("\" not found"))
}

fn resolve_model_route_for_turn(selected: Option<&str>) -> Result<ModelRoute, String> {
    let explicit = selected
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "default");
    if let Some(explicit) = explicit {
        return Ok(resolve_model_route(Some(explicit)));
    }
    if let Some((provider, model)) =
        crate::external_agents::dsh_profile::active_provider_default_route()?
    {
        return Ok(ModelRoute { provider, model });
    }
    Ok(resolve_model_route(None))
}

/// 外部模型值：默认 DeepSeek 路由用裸 model id；用户配置的 pi-ai 路由由 detection 编成
/// `provider:model`。用冒号而不是斜杠 —— 模型 id 自己可以含 `/`。
fn resolve_model_route(selected: Option<&str>) -> ModelRoute {
    let selected = selected
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "default")
        .unwrap_or(DEFAULT_MODEL);
    match selected.split_once(':') {
        Some((provider, model)) if !provider.trim().is_empty() && !model.trim().is_empty() => {
            ModelRoute {
                provider: provider.trim().to_string(),
                model: model.trim().to_string(),
            }
        }
        _ => ModelRoute {
            provider: DEFAULT_PROVIDER.to_string(),
            model: selected.to_string(),
        },
    }
}

fn normalize_sandbox(sandbox: Option<&str>) -> &'static str {
    match sandbox.map(str::trim) {
        Some("read-only") => "read-only",
        Some("danger-full-access") => "danger-full-access",
        _ => "workspace-write",
    }
}

/// 映射一条 dsh SessionEvent；返回 Some 表示看到了匹配轮次的终态。
fn map_session_event(
    event_type: &str,
    data: &Value,
    context_window: &mut Option<u64>,
    sink: &mut dyn FnMut(UnifiedAgentEvent),
) -> Option<Result<(), String>> {
    match event_type {
        "request/context" => {
            if let Some(window) = data.get("contextWindow").and_then(Value::as_u64) {
                *context_window = Some(window);
            }
        }
        "assistant/chunk" => {
            let chunk = data.get("chunk")?;
            match chunk.get("type").and_then(Value::as_str).unwrap_or("") {
                "text-delta" => {
                    if let Some(delta) = chunk.get("text").and_then(Value::as_str) {
                        if !delta.is_empty() {
                            sink(UnifiedAgentEvent::TextDelta {
                                delta: delta.to_string(),
                            });
                        }
                    }
                }
                "reasoning-delta" => {
                    if let Some(delta) = chunk.get("text").and_then(Value::as_str) {
                        if !delta.is_empty() {
                            sink(UnifiedAgentEvent::ThinkingDelta {
                                delta: delta.to_string(),
                            });
                        }
                    }
                }
                "usage" => {
                    if let Some(usage) = parse_usage(chunk.get("usage"), *context_window) {
                        sink(UnifiedAgentEvent::Usage { usage });
                    }
                }
                // block-start / block-end / tool-call-delta / finish 都有更权威的独立事件或
                // 轮终点。尤其 tool-call-delta 是一个字符一个字符地来，不能拿它造工具卡。
                _ => {}
            }
        }
        "tool/call" => {
            let id = data
                .get("callId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let name = data
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if !id.is_empty() && !name.is_empty() {
                let raw = data.get("arguments").and_then(Value::as_str).unwrap_or("");
                let input = serde_json::from_str(raw).unwrap_or_else(|_| {
                    if raw.is_empty() {
                        Value::Null
                    } else {
                        json!({ "raw": raw })
                    }
                });
                sink(UnifiedAgentEvent::ToolUse { id, name, input });
            }
        }
        "tool/result" => {
            map_tool_results(data, sink);
        }
        "turn/end" => {
            let reason = data.get("reason").unwrap_or(&Value::Null);
            let kind = reason
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("error");
            return Some(match kind {
                "completed" | "max-tokens" => Ok(()),
                "aborted" => Err("dsh turn aborted".to_string()),
                "blocked" => Err("dsh turn blocked".to_string()),
                "interrupted" => Err("dsh turn interrupted".to_string()),
                "error" => {
                    let message = reason
                        .get("error")
                        .and_then(|error| error.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("dsh turn failed")
                        .to_string();
                    sink(UnifiedAgentEvent::Error {
                        message: message.clone(),
                    });
                    Err(message)
                }
                other => Err(format!("dsh turn ended: {other}")),
            });
        }
        _ => {}
    }
    None
}

fn parse_usage(value: Option<&Value>, context_window: Option<u64>) -> Option<ModelUsage> {
    let usage = value?;
    let input = usage
        .get("inputTokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output = usage
        .get("outputTokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read = usage
        .get("cacheReadTokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_write = usage
        .get("cacheWriteTokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning = usage
        .get("reasoningTokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    // dsh 的 TokenUsage 已把 cache hit 从 inputTokens 里减掉（translate.ts::mapUsage）。
    // outputTokens 是 provider completion_tokens，已含 reasoning；total 不再加 reasoning。
    Some(ModelUsage {
        input_tokens: Some(input),
        output_tokens: Some(output),
        total_tokens: Some(
            input
                .saturating_add(output)
                .saturating_add(cache_read)
                .saturating_add(cache_write),
        ),
        cached_input_tokens: (cache_read > 0).then_some(cache_read),
        cache_creation_input_tokens: (cache_write > 0).then_some(cache_write),
        reasoning_tokens: (reasoning > 0).then_some(reasoning),
        context_window_tokens: context_window,
    })
}

fn map_tool_results(data: &Value, sink: &mut dyn FnMut(UnifiedAgentEvent)) {
    let blocks = data
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array);
    let Some(blocks) = blocks else {
        return;
    };
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("tool-result") {
            continue;
        }
        let tool_use_id = block
            .get("toolCallId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if tool_use_id.is_empty() {
            continue;
        }
        let content = block
            .get("content")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| match item.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            item.get("text").and_then(Value::as_str).map(str::to_string)
                        }
                        Some("image") => Some("[image attachment]".to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let is_error = block
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| data.get("error").is_some());
        sink(UnifiedAgentEvent::ToolResult {
            tool_use_id,
            content,
            is_error,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::external_agents::dsh_profile::KIVIO_PROFILE;

    use super::*;

    #[test]
    fn parses_default_and_provider_qualified_models() {
        assert_eq!(
            resolve_model_route(None),
            ModelRoute {
                provider: "deepseek-official".into(),
                model: "deepseek-v4-flash".into(),
            }
        );
        assert_eq!(
            resolve_model_route(Some("xiaobai:gpt-5.6-luna")),
            ModelRoute {
                provider: "xiaobai".into(),
                model: "gpt-5.6-luna".into(),
            }
        );
        // 模型 id 自己可以含 `/`，冒号才是 route 分隔符。
        assert_eq!(
            resolve_model_route(Some("relay:vendor/model-x")),
            ModelRoute {
                provider: "relay".into(),
                model: "vendor/model-x".into(),
            }
        );
    }

    #[test]
    fn classifies_only_missing_resume_targets_for_fresh_fallback() {
        assert!(is_missing_session_error(
            "dsh session/open: session \"kivio-old\" not found"
        ));
        assert!(!is_missing_session_error(
            "dsh session/open: persisted log checksum mismatch"
        ));
        assert!(!is_missing_session_error("dsh initialize: auth failed"));
    }

    #[test]
    fn sandbox_defaults_to_workspace_write() {
        assert_eq!(normalize_sandbox(None), "workspace-write");
        assert_eq!(normalize_sandbox(Some("default")), "workspace-write");
        assert_eq!(normalize_sandbox(Some("read-only")), "read-only");
        assert_eq!(
            normalize_sandbox(Some("danger-full-access")),
            "danger-full-access"
        );
    }

    #[test]
    fn maps_text_reasoning_context_and_usage_without_double_counting_reasoning() {
        let mut emitted = Vec::new();
        let mut window = None;
        map_session_event(
            "request/context",
            &json!({ "contextWindow": 1_000_000 }),
            &mut window,
            &mut |event| emitted.push(event),
        );
        map_session_event(
            "assistant/chunk",
            &json!({ "chunk": { "type": "text-delta", "text": "OK" } }),
            &mut window,
            &mut |event| emitted.push(event),
        );
        map_session_event(
            "assistant/chunk",
            &json!({ "chunk": { "type": "reasoning-delta", "text": "Think" } }),
            &mut window,
            &mut |event| emitted.push(event),
        );
        map_session_event(
            "assistant/chunk",
            &json!({ "chunk": { "type": "usage", "usage": {
                "inputTokens": 166,
                "outputTokens": 184,
                "cacheReadTokens": 1152,
                "reasoningTokens": 49
            } } }),
            &mut window,
            &mut |event| emitted.push(event),
        );
        assert!(matches!(
            &emitted[0],
            UnifiedAgentEvent::TextDelta { delta } if delta == "OK"
        ));
        assert!(matches!(
            &emitted[1],
            UnifiedAgentEvent::ThinkingDelta { delta } if delta == "Think"
        ));
        let UnifiedAgentEvent::Usage { usage } = &emitted[2] else {
            panic!("expected usage")
        };
        assert_eq!(usage.input_tokens, Some(166));
        assert_eq!(usage.output_tokens, Some(184));
        assert_eq!(usage.cached_input_tokens, Some(1152));
        assert_eq!(usage.reasoning_tokens, Some(49));
        // reasoning 是 output 的子集，不能再加一次：166 + 184 + 1152 = 1502。
        assert_eq!(usage.total_tokens, Some(1502));
        assert_eq!(usage.context_window_tokens, Some(1_000_000));
    }

    #[test]
    fn maps_complete_tool_call_and_result() {
        let mut emitted = Vec::new();
        let mut window = None;
        map_session_event(
            "tool/call",
            &json!({
                "callId": "call_1",
                "name": "bash",
                "arguments": "{\"command\":\"echo ok\"}"
            }),
            &mut window,
            &mut |event| emitted.push(event),
        );
        map_session_event(
            "tool/result",
            &json!({ "message": { "content": [{
                "type": "tool-result",
                "toolCallId": "call_1",
                "content": [{ "type": "text", "text": "ok\n" }],
                "isError": false
            }] } }),
            &mut window,
            &mut |event| emitted.push(event),
        );
        assert!(matches!(
            &emitted[0],
            UnifiedAgentEvent::ToolUse { id, name, input }
                if id == "call_1" && name == "bash" && input["command"] == "echo ok"
        ));
        assert!(matches!(
            &emitted[1],
            UnifiedAgentEvent::ToolResult { tool_use_id, content, is_error }
                if tool_use_id == "call_1" && content == "ok\n" && !is_error
        ));
    }

    #[test]
    fn turn_error_is_both_emitted_and_terminal() {
        let mut emitted = Vec::new();
        let mut window = None;
        let result = map_session_event(
            "turn/end",
            &json!({ "reason": { "kind": "error", "error": {
                "message": "missing credential", "code": "MISSING_CREDENTIAL"
            } } }),
            &mut window,
            &mut |event| emitted.push(event),
        )
        .expect("turn/end must terminate");
        assert_eq!(result, Err("missing credential".to_string()));
        assert!(matches!(
            &emitted[0],
            UnifiedAgentEvent::Error { message } if message == "missing credential"
        ));
    }

    #[test]
    fn ignores_incomplete_tool_call_deltas() {
        let mut emitted = Vec::new();
        let mut window = None;
        map_session_event(
            "assistant/chunk",
            &json!({ "chunk": {
                "type": "tool-call-delta",
                "id": "call_1",
                "name": "bash",
                "argumentsDelta": "{\"command\""
            } }),
            &mut window,
            &mut |event| emitted.push(event),
        );
        assert!(emitted.is_empty());
    }

    /// 真机协议门：显式 `DSH_E2E=1` 才跑，避免普通测试消耗用户额度。
    #[tokio::test]
    #[ignore = "requires installed/authenticated dsh; run with DSH_E2E=1"]
    async fn live_dsh_emits_tool_text_and_usage() {
        assert_eq!(std::env::var("DSH_E2E").as_deref(), Ok("1"));
        let bin = std::env::var_os("DSH_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                directories::BaseDirs::new()
                    .expect("home")
                    .home_dir()
                    .join(".local/bin/dsh")
            });
        assert!(bin.is_file(), "dsh binary missing: {}", bin.display());
        let cwd = std::env::current_dir().expect("cwd");
        let args = vec!["--profile".to_string(), KIVIO_PROFILE.to_string()];
        let mut session = DshJsonRpcSession::connect(
            &bin,
            &args,
            &cwd,
            None,
            Some("deepseek-v4-flash"),
            Some("off"),
            Some("read-only"),
        )
        .await
        .expect("connect live dsh");
        let (event_tx, mut event_rx) = mpsc::channel(4096);
        let (_control_tx, mut control_rx) = mpsc::channel(4);
        session
            .run_turn(
                "必须调用 bash 工具执行 `printf KIVIO_DSH_E2E`，然后只回答命令输出。",
                Some("deepseek-v4-flash"),
                &event_tx,
                &mut control_rx,
            )
            .await
            .expect("live dsh turn");
        drop(event_tx);
        let mut events = Vec::new();
        while let Some(event) = event_rx.recv().await {
            events.push(event);
        }
        assert!(
            events
                .iter()
                .any(|event| matches!(event, UnifiedAgentEvent::ToolUse { .. })),
            "missing tool event: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, UnifiedAgentEvent::TextDelta { .. })),
            "missing text delta: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, UnifiedAgentEvent::Usage { .. })),
            "missing usage: {events:?}"
        );
        session.close().await;
    }

    #[tokio::test]
    #[ignore = "requires installed/authenticated dsh; run with DSH_E2E=1"]
    async fn live_dsh_keeps_multi_turn_context_and_streams_reasoning() {
        assert_eq!(std::env::var("DSH_E2E").as_deref(), Ok("1"));
        let bin = std::env::var_os("DSH_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                directories::BaseDirs::new()
                    .expect("home")
                    .home_dir()
                    .join(".local/bin/dsh")
            });
        let cwd = std::env::current_dir().expect("cwd");
        let args = vec!["--profile".to_string(), KIVIO_PROFILE.to_string()];
        let mut session = DshJsonRpcSession::connect(
            &bin,
            &args,
            &cwd,
            None,
            Some("deepseek-v4-flash"),
            Some("high"),
            Some("read-only"),
        )
        .await
        .expect("connect live dsh");
        let original_id = session.session_id().to_string();
        let (event_tx, mut event_rx) = mpsc::channel(4096);
        let (_control_tx, mut control_rx) = mpsc::channel(4);
        session
            .run_turn(
                "记住验证码 KIVIO-7429。只回答 ACK。",
                Some("deepseek-v4-flash"),
                &event_tx,
                &mut control_rx,
            )
            .await
            .expect("first dsh turn");
        session.close().await;

        let mut session = DshJsonRpcSession::connect(
            &bin,
            &args,
            &cwd,
            Some(&original_id),
            Some("deepseek-v4-flash"),
            Some("high"),
            Some("read-only"),
        )
        .await
        .expect("resume live dsh");
        assert!(session.resumed(), "bridge created instead of resuming");
        session
            .run_turn(
                "上一轮的验证码是什么？只回答验证码。",
                Some("deepseek-v4-flash"),
                &event_tx,
                &mut control_rx,
            )
            .await
            .expect("second dsh turn after process restart");
        assert_eq!(session.session_id(), original_id);
        drop(event_tx);
        let mut text = String::new();
        let mut saw_reasoning = false;
        while let Some(event) = event_rx.recv().await {
            match event {
                UnifiedAgentEvent::TextDelta { delta } => text.push_str(&delta),
                UnifiedAgentEvent::ThinkingDelta { delta } if !delta.is_empty() => {
                    saw_reasoning = true;
                }
                _ => {}
            }
        }
        assert!(
            text.contains("KIVIO-7429"),
            "second turn lost context: {text:?}"
        );
        assert!(saw_reasoning, "high effort emitted no reasoning delta");
        session.close().await;
    }

    #[tokio::test]
    #[ignore = "requires installed dsh; run with DSH_E2E=1"]
    async fn live_dsh_cancel_preserves_resumable_session() {
        assert_eq!(std::env::var("DSH_E2E").as_deref(), Ok("1"));
        let bin = std::env::var_os("DSH_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                directories::BaseDirs::new()
                    .expect("home")
                    .home_dir()
                    .join(".local/bin/dsh")
            });
        let cwd = std::env::current_dir().expect("cwd");
        let args = vec!["--profile".to_string(), KIVIO_PROFILE.to_string()];
        let mut session = DshJsonRpcSession::connect(
            &bin,
            &args,
            &cwd,
            None,
            Some("deepseek-v4-flash"),
            Some("off"),
            Some("read-only"),
        )
        .await
        .expect("connect live dsh");
        let (event_tx, _event_rx) = mpsc::channel(4096);
        let (control_tx, mut control_rx) = mpsc::channel(1);
        // Queue cancel before run_turn reads the prompt ACK. The client must defer the cancel RPC
        // until the prompt is durably enqueued, otherwise the bridge can cancel an idle agent.
        control_tx
            .send(SessionCommand::Cancel)
            .await
            .expect("queue early cancel");
        let error = session
            .run_turn(
                "写一篇很长的文章。",
                Some("deepseek-v4-flash"),
                &event_tx,
                &mut control_rx,
            )
            .await
            .expect_err("cancel must stop the active dsh turn");
        assert_eq!(error, "cancelled");
        assert!(session.child.try_wait().expect("child status").is_none());
        session
            .run_turn(
                "只回答 READY。",
                Some("deepseek-v4-flash"),
                &event_tx,
                &mut control_rx,
            )
            .await
            .expect("session should remain usable after cancel");
        session.close().await;
    }
}
