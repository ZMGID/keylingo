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
//! - `session/command { sessionId, line }`（bridge：`ctx.commands.execute`，不进模型）
//! - `session/cancel { sessionId }`
//! - `shutdown`
//!
//! 服务端 → 客户端请求（官方传输层支持，官方 server 自己不发；Kivio bridge 补这一条）：
//! - `session/ask { sessionId, questions }` → `{ answers: [{ id, selected, custom? }] }`
//!   这是 `ctx.userQuestions` 的跨进程出口：preset 里的 `ask_user_question` 会停在
//!   `UserQuestionProvider.ask()`，bridge 把官方问题形状原样转给宿主，等 Kivio 已有的
//!   问用户卡片作答后再把官方 `AskUserQuestionAnswer` 回给工具。
//!
//! 服务端把完整的持久会话日志广播成 `session.event`，另有 `session.status`。一轮工具循环
//! 会产生多个 step，所以 `assistant/chunk.finish` **不是轮终点**；真正终点是匹配 session 的
//! `turn/end`，随后 `session.status: idle` 表示整台 agent 静止。
//!
//! 服务端广播**运行时里的所有 session**（包含子代理）。父会话的正文/工具卡按
//! `params.sessionId` 过滤；子会话的 `tool/call` / 正文折成 `SubagentProgress`，挂到
//! 父工具卡上，不进父气泡。`subagent.started` / `subagent.finished` 没有 `sessionId`，
//! 改按 `parentSessionId` 认父会话。父轮结束后 `tool-jobs` 会对空闲父会话
//! `followup` 开一轮汇报（和官方 web 同一条路）；空闲读要攒这轮 `TextDelta`，
//! `turn/end` 时落成一条助手消息，不能只留任务边沿。Kivio bridge 直接调用 dsh
//! 公共的 `agents.resume()` 与 `agent.cancel()`，所以进程重建和用户停止都不会再丢失原生会话。

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, Mutex};
use tokio::time::timeout;
use uuid::Uuid;

use crate::chat::model::ModelUsage;
use crate::external_agents::prompt::is_cli_slash_input;
use crate::external_agents::session::live::{
    ApprovalAsk, ApprovalBridge, ApprovalDecision, SessionCommand, CANCELLED_SESSION_LOST,
};
use crate::external_agents::types::UnifiedAgentEvent;
use crate::proc::NoConsoleWindow;

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(45);
const READ_POLL: Duration = Duration::from_millis(200);
const CANCEL_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
const MAX_RECOVERABLE_READ_ERRORS: u32 = 32;
const CHILD_PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(350);
const CHILD_PREVIEW_MAX_CHARS: usize = 1200;
const CHILD_STEP_MAX_CHARS: usize = 160;
const CHILD_STEPS_MAX: usize = 24;
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
    /// 跨轮保留：父轮压缩/后台 call 配对，以及轮间空闲读的 leftover。
    map_state: MapSessionState,
    /// 子会话进度。官方 subagent 默认后台，父轮结束后孩子还在跑。
    child_progress: HashMap<String, ChildProgress>,
    /// 轮间父会话唤醒轮：`tool-jobs` 对空闲 owner `followup` 之后的正文。
    idle_wake: IdleWakeState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelRoute {
    provider: String,
    model: String,
}

/// `map_session_event` 跨事件的少量状态：压缩 summary→end 配对、后台 tool/call→result。
#[derive(Default)]
struct MapSessionState {
    context_window: Option<u64>,
    compact: Option<PendingCompact>,
    background_calls: HashMap<String, PendingBackground>,
}

#[derive(Default)]
struct PendingCompact {
    trigger: String,
    dropped_tokens: Option<u64>,
}

struct PendingBackground {
    name: String,
    description: Option<String>,
}

#[derive(Default)]
struct ChildProgress {
    steps: Vec<String>,
    preview: String,
    last_emit: Option<Instant>,
}

/// 轮间空闲读副作用：任务边沿 / 子代理进度 / 唤醒轮正文。会话层没有 `AppHandle`。
pub enum DshIdleEffect {
    Event(UnifiedAgentEvent),
    Wake {
        text: String,
        model: Option<String>,
        usage: Option<ModelUsage>,
    },
}

pub type DshIdleSink = std::sync::Arc<dyn Fn(DshIdleEffect) + Send + Sync>;

enum DshIdlePump {
    Quiet,
    Hiccup,
    Dead,
    Reply(String),
    Event(UnifiedAgentEvent),
    Wake {
        text: String,
        usage: Option<ModelUsage>,
    },
}

#[derive(Default)]
struct IdleWakeState {
    collecting: bool,
    text: String,
    usage: Option<ModelUsage>,
}

enum IdleParentFold {
    Quiet,
    Event(UnifiedAgentEvent),
    Wake {
        text: String,
        usage: Option<ModelUsage>,
    },
}

enum ReadStep {
    Line(String),
    Idle,
    Eof,
    Fatal,
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
        preset: Option<&str>,
    ) -> Result<Self, String> {
        let _profile_boot_guard = DSH_PROFILE_BOOT_LOCK.lock().await;
        if crate::external_agents::overrides::active_provider("dsh").is_none()
            && !crate::external_agents::dsh_plugins::official_deepseek_key_ready()
        {
            return Err("missing credential".to_string());
        }
        crate::external_agents::dsh_profile::ensure_profile_ready(resolved_bin, reasoning, preset)
            .await?;

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
            .env(
                "DSH_AGENT_PRESET",
                crate::external_agents::dsh_profile::normalize_agent_preset(preset),
            )
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
                map_state: MapSessionState::default(),
                child_progress: HashMap::new(),
                idle_wake: IdleWakeState::default(),
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
        mut approvals: Option<&mut ApprovalBridge>,
    ) -> Result<(), String> {
        let requested_route = resolve_model_route_for_turn(model)?;
        if requested_route != self.route {
            // 防止未来有人把 dsh 从启动指纹里删掉后，UI 显示新模型、实际 agent 仍跑旧模型。
            return Err(crate::external_agents::session::acp::NEEDS_RECONNECT.to_string());
        }

        self.idle_wake = IdleWakeState::default();
        let prompt_id = self.next_id;
        self.next_id += 1;
        let is_slash = is_cli_slash_input(prompt);
        let (method, params) = turn_rpc(&self.session_id, prompt);
        write_rpc(&mut self.stdin, prompt_id, method, params).await?;

        let mut started = false;
        let mut prompt_acknowledged = false;
        let mut terminal: Option<Result<(), String>> = None;
        self.map_state.context_window = self.context_window;
        let mut cancel_requested = false;
        let mut cancel_id: Option<u64> = None;
        let mut cancel_started: Option<std::time::Instant> = None;
        let mut pending_asks: std::collections::HashMap<String, Value> =
            std::collections::HashMap::new();
        let mut last_user_question_call: Option<(String, String)> = None;

        loop {
            if let Some(bridge) = approvals.as_deref_mut() {
                while let Ok(decision) = bridge.decisions.try_recv() {
                    settle_session_ask(&mut self.stdin, &mut pending_asks, decision).await?;
                }
            }

            match control.try_recv() {
                Ok(SessionCommand::Cancel) => cancel_requested = true,
                Ok(SessionCommand::Close) => {
                    reject_pending_asks(
                        &mut self.stdin,
                        &mut pending_asks,
                        "ASK_ABORTED",
                        "ask_user_question was aborted before the user answered",
                    )
                    .await;
                    return Err("closed".to_string());
                }
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
            if cancel_requested && !pending_asks.is_empty() {
                reject_pending_asks(
                    &mut self.stdin,
                    &mut pending_asks,
                    "ASK_ABORTED",
                    "ask_user_question was aborted before the user answered",
                )
                .await;
            }
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
                    self.context_window = self.map_state.context_window;
                    return Err(error);
                }
                prompt_acknowledged = true;
                if is_slash {
                    // `session/command` 没有 turn/end；回执就是轮终点。compaction/*
                    // 等 session.event 在 execute() 返回前已经写过，本循环前面几轮已映射。
                    let result = value.get("result").unwrap_or(&Value::Null);
                    if let Some(text) = result
                        .get("text")
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                    {
                        let _ = events
                            .send(UnifiedAgentEvent::TextDelta {
                                delta: text.to_string(),
                            })
                            .await;
                    }
                    self.context_window = self.map_state.context_window;
                    return match command_result_error(result) {
                        Some(message) => Err(message),
                        None => Ok(()),
                    };
                }
                // `session/prompt` 的 result 只是入队回执（messageId），不是轮终点。
                continue;
            }

            if is_incoming_rpc_request(&value) {
                handle_incoming_request(
                    &mut self.stdin,
                    &self.session_id,
                    &value,
                    approvals.as_deref_mut(),
                    &mut pending_asks,
                    last_user_question_call.as_ref(),
                )
                .await?;
                continue;
            }

            let Some(method) = value.get("method").and_then(Value::as_str) else {
                continue;
            };
            let params = value.get("params").unwrap_or(&Value::Null);
            if matches!(method, "subagent.started" | "subagent.finished") {
                // 官方通知只有 parentSessionId / childSessionId，没有 sessionId。
                if params.get("parentSessionId").and_then(Value::as_str)
                    != Some(self.session_id.as_str())
                {
                    continue;
                }
                if method == "subagent.finished" {
                    if let Some(child_id) = params.get("childSessionId").and_then(Value::as_str) {
                        self.child_progress.remove(child_id);
                    }
                }
                if let Some(event) = map_subagent_notification(method, params) {
                    let _ = events.send(event).await;
                }
                continue;
            }
            if params.get("sessionId").and_then(Value::as_str) != Some(self.session_id.as_str()) {
                // 子会话：折成进度，不进父气泡，也不结束父轮。
                if method == "session.event" {
                    if let Some(event) = fold_child_session_params(params, &mut self.child_progress)
                    {
                        let _ = events.send(event).await;
                    }
                }
                continue;
            }

            match method {
                "session.status" => match params.get("status").and_then(Value::as_str) {
                    Some("running") => started = true,
                    Some("idle") if started && terminal.is_some() && cancel_id.is_none() => {
                        reject_pending_asks(
                            &mut self.stdin,
                            &mut pending_asks,
                            "ASK_ABORTED",
                            "ask_user_question was aborted before the user answered",
                        )
                        .await;
                        self.context_window = self.map_state.context_window;
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
                        &mut self.map_state,
                        &mut |event| mapped.push(event),
                    ) {
                        terminal = Some(result);
                    }
                    self.context_window = self.map_state.context_window;
                    for event in mapped {
                        if let UnifiedAgentEvent::ToolUse { id, name, .. } = &event {
                            if is_user_question_tool(name) {
                                last_user_question_call = Some((id.clone(), name.clone()));
                            }
                        }
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

    async fn next_line(&mut self) -> ReadStep {
        match timeout(READ_POLL, self.reader.next_line()).await {
            Ok(Ok(Some(line))) => ReadStep::Line(line),
            Ok(Ok(None)) => ReadStep::Eof,
            Ok(Err(_)) => ReadStep::Fatal,
            Err(_) => ReadStep::Idle,
        }
    }

    async fn write_control_line(&mut self, line: &str) {
        let _ = self.stdin.write_all(line.as_bytes()).await;
        let _ = self.stdin.flush().await;
    }

    /// 轮间空闲读一步：只读 + 分类，不写。写回（fail-closed 的 ask 回复）交给 actor。
    async fn read_idle_frame(&mut self) -> DshIdlePump {
        let line = match self.next_line().await {
            ReadStep::Line(line) => line,
            ReadStep::Idle => return DshIdlePump::Quiet,
            ReadStep::Eof => return DshIdlePump::Dead,
            ReadStep::Fatal => return DshIdlePump::Hiccup,
        };
        if line.trim().is_empty() {
            return DshIdlePump::Quiet;
        }
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            return DshIdlePump::Quiet;
        };
        self.classify_idle_value(&value)
    }

    fn classify_idle_value(&mut self, value: &Value) -> DshIdlePump {
        if is_incoming_rpc_request(value) {
            let Some(id) = rpc_id(value) else {
                return DshIdlePump::Quiet;
            };
            return DshIdlePump::Reply(rpc_error_line(
                &id,
                -32000,
                "no user-questions provider is registered",
                Some(json!({ "code": "NO_PROVIDER" })),
            ));
        }
        let Some(method) = value.get("method").and_then(Value::as_str) else {
            return DshIdlePump::Quiet;
        };
        let params = value.get("params").unwrap_or(&Value::Null);
        if matches!(method, "subagent.started" | "subagent.finished") {
            if params.get("parentSessionId").and_then(Value::as_str) != Some(self.session_id.as_str())
            {
                return DshIdlePump::Quiet;
            }
            if method == "subagent.finished" {
                if let Some(child_id) = params.get("childSessionId").and_then(Value::as_str) {
                    self.child_progress.remove(child_id);
                }
            }
            return map_subagent_notification(method, params)
                .map(DshIdlePump::Event)
                .unwrap_or(DshIdlePump::Quiet);
        }
        if method != "session.event" {
            return DshIdlePump::Quiet;
        }
        if params.get("sessionId").and_then(Value::as_str) != Some(self.session_id.as_str()) {
            return fold_child_session_params(params, &mut self.child_progress)
                .map(DshIdlePump::Event)
                .unwrap_or(DshIdlePump::Quiet);
        }
        let Some(event) = params.get("event") else {
            return DshIdlePump::Quiet;
        };
        let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
        let data = event.get("data").unwrap_or(&Value::Null);
        let folded = fold_idle_parent_session(
            event_type,
            data,
            &mut self.map_state,
            &mut self.idle_wake,
        );
        self.context_window = self.map_state.context_window;
        match folded {
            IdleParentFold::Quiet => DshIdlePump::Quiet,
            IdleParentFold::Event(event) => DshIdlePump::Event(event),
            IdleParentFold::Wake { text, usage } => DshIdlePump::Wake { text, usage },
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
pub fn spawn_dsh_session_actor(session: DshJsonRpcSession) -> mpsc::Sender<SessionCommand> {
    spawn_dsh_session_actor_with_sink(session, None)
}

/// 带轮间空闲读的 actor。官方 subagent 默认后台，父轮 `turn/end` 之后孩子还在往
/// stdout 写 `session.event`；不读的话工具卡永远停在「运行中…」。
pub fn spawn_dsh_session_actor_with_sink(
    mut session: DshJsonRpcSession,
    sink: Option<DshIdleSink>,
) -> mpsc::Sender<SessionCommand> {
    enum ActorStep {
        Cmd(Option<SessionCommand>),
        Idle(DshIdlePump),
    }
    let (tx, mut rx) = mpsc::channel::<SessionCommand>(8);
    tokio::spawn(async move {
        let mut idle_dead = false;
        let mut idle_hiccups = 0u32;
        loop {
            let step = if idle_dead {
                ActorStep::Cmd(rx.recv().await)
            } else {
                tokio::select! {
                    cmd = rx.recv() => ActorStep::Cmd(cmd),
                    pump = session.read_idle_frame() => ActorStep::Idle(pump),
                }
            };
            let cmd = match step {
                ActorStep::Idle(pump) => {
                    match pump {
                        DshIdlePump::Quiet => idle_hiccups = 0,
                        DshIdlePump::Hiccup => {
                            idle_hiccups += 1;
                            if idle_hiccups >= MAX_RECOVERABLE_READ_ERRORS {
                                idle_dead = true;
                            }
                        }
                        DshIdlePump::Dead => idle_dead = true,
                        DshIdlePump::Reply(line) => {
                            idle_hiccups = 0;
                            session.write_control_line(&line).await;
                        }
                        DshIdlePump::Event(event) => {
                            idle_hiccups = 0;
                            if let Some(sink) = sink.as_ref() {
                                sink(DshIdleEffect::Event(event));
                            }
                        }
                        DshIdlePump::Wake { text, usage } => {
                            idle_hiccups = 0;
                            if let Some(sink) = sink.as_ref() {
                                sink(DshIdleEffect::Wake {
                                    text,
                                    model: Some(session.route.model.clone()),
                                    usage,
                                });
                            }
                        }
                    }
                    continue;
                }
                ActorStep::Cmd(cmd) => cmd,
            };
            let Some(command) = cmd else {
                session.close().await;
                return;
            };
            match command {
                SessionCommand::RunTurn {
                    prompt,
                    model,
                    reasoning: _,
                    images: _,
                    events,
                    done,
                    mut approvals,
                } => {
                    let result = session
                        .run_turn(
                            &prompt,
                            model.as_deref(),
                            &events,
                            &mut rx,
                            approvals.as_mut(),
                        )
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
    });
    tx
}

const SESSION_ASK_METHOD: &str = "session/ask";

pub fn is_user_question_tool(name: &str) -> bool {
    crate::external_agents::ask_user::matches_tool("dsh", name)
}

fn is_incoming_rpc_request(value: &Value) -> bool {
    value.get("method").and_then(Value::as_str).is_some() && rpc_id(value).is_some()
}

fn rpc_id(value: &Value) -> Option<Value> {
    match value.get("id") {
        Some(id) if id.is_string() || id.is_number() => Some(id.clone()),
        _ => None,
    }
}

fn rpc_id_key(id: &Value) -> Option<String> {
    match id {
        Value::String(value) if !value.is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

async fn handle_incoming_request(
    stdin: &mut ChildStdin,
    session_id: &str,
    value: &Value,
    approvals: Option<&mut ApprovalBridge>,
    pending_asks: &mut std::collections::HashMap<String, Value>,
    last_user_question_call: Option<&(String, String)>,
) -> Result<(), String> {
    let Some(id) = rpc_id(value) else {
        return Ok(());
    };
    let method = value.get("method").and_then(Value::as_str).unwrap_or("");
    if method != SESSION_ASK_METHOD {
        return write_rpc_error(
            stdin,
            &id,
            -32601,
            &format!("method not found: {method}"),
            None,
        )
        .await;
    }
    let params = value.get("params").unwrap_or(&Value::Null);
    if params.get("sessionId").and_then(Value::as_str) != Some(session_id) {
        return write_rpc_error(
            stdin,
            &id,
            -32602,
            "session/ask sessionId does not match the open session",
            Some(json!({ "code": "ASK_MISSING_AGENT" })),
        )
        .await;
    }
    let Some(questions) = params.get("questions").filter(|value| {
        value
            .as_array()
            .is_some_and(|questions| !questions.is_empty())
    }) else {
        return write_rpc_error(
            stdin,
            &id,
            -32602,
            "ask_user_question requires at least one question",
            Some(json!({ "code": "EMPTY_QUESTIONS" })),
        )
        .await;
    };
    let Some(bridge) = approvals else {
        return write_rpc_error(
            stdin,
            &id,
            -32000,
            "no user-questions provider is registered",
            Some(json!({ "code": "NO_PROVIDER" })),
        )
        .await;
    };
    let Some(request_id) = rpc_id_key(&id) else {
        return write_rpc_error(
            stdin,
            &id,
            -32602,
            "session/ask is missing a request id",
            None,
        )
        .await;
    };
    let (tool_call_id, tool_name) = last_user_question_call
        .cloned()
        .unwrap_or_else(|| (request_id.clone(), "ask_user_question".to_string()));
    pending_asks.insert(request_id.clone(), id.clone());
    if bridge
        .requests
        .send(ApprovalAsk {
            request_id: request_id.clone(),
            tool_call_id,
            tool_name,
            input: json!({ "questions": questions }),
            requires_user_interaction: true,
        })
        .await
        .is_err()
    {
        pending_asks.remove(&request_id);
        return write_rpc_error(
            stdin,
            &id,
            -32000,
            "ask_user_question was aborted before the user answered",
            Some(json!({ "code": "ASK_ABORTED" })),
        )
        .await;
    }
    Ok(())
}

async fn settle_session_ask(
    stdin: &mut ChildStdin,
    pending_asks: &mut std::collections::HashMap<String, Value>,
    decision: ApprovalDecision,
) -> Result<(), String> {
    let Some(id) = pending_asks.remove(&decision.request_id) else {
        return Ok(());
    };
    if decision.approved {
        if let Some(result) = decision.updated_input {
            return write_rpc_result(stdin, &id, result).await;
        }
        return write_rpc_error(
            stdin,
            &id,
            -32000,
            "ask_user_question returned no answer payload",
            Some(json!({ "code": "ASK_ABORTED" })),
        )
        .await;
    }
    write_rpc_error(
        stdin,
        &id,
        -32000,
        "the user cancelled ask_user_question",
        Some(json!({ "code": "ASK_CANCELLED" })),
    )
    .await
}

async fn reject_pending_asks(
    stdin: &mut ChildStdin,
    pending_asks: &mut std::collections::HashMap<String, Value>,
    code: &str,
    message: &str,
) {
    let leftover: Vec<Value> = pending_asks.drain().map(|(_, id)| id).collect();
    for id in leftover {
        let _ = write_rpc_error(stdin, &id, -32000, message, Some(json!({ "code": code }))).await;
    }
}

async fn write_rpc_result(stdin: &mut ChildStdin, id: &Value, result: Value) -> Result<(), String> {
    write_rpc_frame(
        stdin,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }),
    )
    .await
}

async fn write_rpc_error(
    stdin: &mut ChildStdin,
    id: &Value,
    code: i64,
    message: &str,
    data: Option<Value>,
) -> Result<(), String> {
    let mut error = json!({
        "code": code,
        "message": message,
    });
    if let Some(data) = data {
        error
            .as_object_mut()
            .expect("error object")
            .insert("data".to_string(), data);
    }
    write_rpc_frame(
        stdin,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": error,
        }),
    )
    .await
}

async fn write_rpc_frame(stdin: &mut ChildStdin, payload: Value) -> Result<(), String> {
    let mut line = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|e| format!("write dsh: {e}"))?;
    stdin.flush().await.map_err(|e| format!("flush dsh: {e}"))
}

async fn write_rpc(
    stdin: &mut ChildStdin,
    id: u64,
    method: &str,
    params: Value,
) -> Result<(), String> {
    write_rpc_frame(
        stdin,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }),
    )
    .await
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

fn turn_rpc(session_id: &str, prompt: &str) -> (&'static str, Value) {
    if is_cli_slash_input(prompt) {
        (
            "session/command",
            json!({
                "sessionId": session_id,
                "line": prompt.trim(),
            }),
        )
    } else {
        (
            "session/prompt",
            json!({
                "sessionId": session_id,
                "contentBlocks": [{ "type": "text", "text": prompt }],
            }),
        )
    }
}

fn command_result_error(result: &Value) -> Option<String> {
    if result.get("kind").and_then(Value::as_str) != Some("error") {
        return None;
    }
    Some(
        result
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or("dsh command failed")
            .to_string(),
    )
}

/// 映射一条 dsh SessionEvent；返回 Some 表示看到了匹配轮次的终态。
fn map_session_event(
    event_type: &str,
    data: &Value,
    state: &mut MapSessionState,
    sink: &mut dyn FnMut(UnifiedAgentEvent),
) -> Option<Result<(), String>> {
    match event_type {
        "request/context" => {
            if let Some(window) = data.get("contextWindow").and_then(Value::as_u64) {
                state.context_window = Some(window);
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
                    if let Some(usage) = parse_usage(chunk.get("usage"), state.context_window) {
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
                if tracks_background_call(&name, &input) {
                    state.background_calls.insert(
                        id.clone(),
                        PendingBackground {
                            name: name.clone(),
                            description: optional_string(&input, "description")
                                .or_else(|| optional_string(&input, "command")),
                        },
                    );
                }
                sink(UnifiedAgentEvent::ToolUse { id, name, input });
            }
        }
        "todo/write" => {
            if data.get("todos").and_then(Value::as_array).is_some() {
                sink(UnifiedAgentEvent::TodoWrite {
                    todos: data.clone(),
                });
            }
        }
        "tool/result" => {
            map_tool_results(data, state, sink);
        }
        "compaction/start" => {
            sink(UnifiedAgentEvent::StatusNote {
                text: "正在压缩…".to_string(),
            });
            state.compact = Some(PendingCompact {
                trigger: compact_trigger(data),
                dropped_tokens: None,
            });
        }
        "compaction/summary" => {
            let trigger = compact_trigger(data);
            let dropped = data.get("shadowedTokenCount").and_then(Value::as_u64);
            if let Some(pending) = state.compact.as_mut() {
                if trigger == "manual" {
                    pending.trigger = trigger;
                }
                pending.dropped_tokens = dropped;
            } else {
                state.compact = Some(PendingCompact {
                    trigger,
                    dropped_tokens: dropped,
                });
            }
        }
        "compaction/end" => {
            if let Some(error) = data
                .get("error")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|error| !error.is_empty())
            {
                state.compact = None;
                sink(UnifiedAgentEvent::StatusNote {
                    text: format!("上下文压缩失败：{error}"),
                });
            } else {
                let pending = state.compact.take().unwrap_or_else(|| PendingCompact {
                    trigger: compact_trigger(data),
                    dropped_tokens: None,
                });
                let trigger = if compact_trigger(data) == "manual" {
                    "manual".to_string()
                } else {
                    pending.trigger
                };
                sink(UnifiedAgentEvent::CliCompacted {
                    trigger,
                    pre_tokens: None,
                    post_tokens: None,
                    dropped_tokens: pending.dropped_tokens,
                    duration_ms: None,
                });
            }
        }
        "llm/retry" => {
            let retry = data.get("retry").and_then(Value::as_u64).unwrap_or(1);
            let of_max = data
                .get("maxRetries")
                .and_then(Value::as_u64)
                .map(|max| format!("/{max}"))
                .unwrap_or_default();
            sink(UnifiedAgentEvent::StatusNote {
                text: format!("上游重试 {retry}{of_max}"),
            });
        }
        "llm/retry-started" => {
            sink(UnifiedAgentEvent::StatusNote {
                text: "正在重试模型调用…".to_string(),
            });
        }
        "user/message" => {
            if let Some(event) = map_tool_jobs_notice(data) {
                sink(event);
            }
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

fn compact_trigger(data: &Value) -> String {
    if data
        .get("sourceCommandId")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
    {
        "manual".to_string()
    } else {
        "auto".to_string()
    }
}

fn is_background_capable_tool(name: &str) -> bool {
    matches!(name, "bash" | "pwsh" | "subagent" | "subagent_fork")
}

fn tracks_background_call(name: &str, input: &Value) -> bool {
    if !is_background_capable_tool(name) {
        return false;
    }
    match input.get("run_in_background").and_then(Value::as_bool) {
        Some(flag) => flag,
        // 官方 subagent 默认后台；没写字段也要记，否则派出回执对不上任务面板。
        None => matches!(name, "subagent" | "subagent_fork"),
    }
}

pub(crate) fn is_subagent_tool_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().replace(['_', '-'], "").as_str(),
        "subagent" | "subagentfork" | "workflow" | "ralph" | "agent" | "task"
    )
}

/// 派出回执（`started subagent <id>`）不是子代理跑完。调用方应保持工具卡 Running。
pub(crate) fn subagent_launch_task_id(name: &str, content: &str) -> Option<String> {
    if !is_subagent_tool_name(name) {
        return None;
    }
    let text = content.trim();
    if !(text.starts_with("started subagent ")
        || text.starts_with("started background subagent task "))
    {
        return None;
    }
    parse_background_task_id(text)
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn background_task_kind(name: &str) -> String {
    match name {
        "subagent" | "subagent_fork" => "local_agent".to_string(),
        _ => "local_bash".to_string(),
    }
}

fn parse_background_task_id(content: &str) -> Option<String> {
    if let Ok(value) = serde_json::from_str::<Value>(content) {
        for key in ["jobId", "job_id", "subagentId", "subagent_id"] {
            if let Some(id) = value
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
            {
                return Some(id.to_string());
            }
        }
    }
    for prefix in [
        "started background job ",
        "started background subagent task ",
        "started subagent ",
    ] {
        if let Some(rest) = content.strip_prefix(prefix) {
            let id = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-');
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

fn content_blocks_text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    if item.get("type").and_then(Value::as_str) == Some("text") {
                        item.get("text").and_then(Value::as_str).map(str::to_string)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn map_tool_jobs_notice(data: &Value) -> Option<UnifiedAgentEvent> {
    let source = data.get("source")?;
    if source.get("plugin").and_then(Value::as_str) != Some("tool-jobs") {
        return None;
    }
    let text = content_blocks_text(data.get("content"));
    let after = text.split("background job ").nth(1)?;
    let task_id = after
        .split(|c: char| c.is_whitespace() || c == '(')
        .next()
        .map(str::trim)
        .filter(|id| !id.is_empty())?
        .to_string();
    let status = extract_bracket_status(&text)
        .map(map_job_notice_status)
        .unwrap_or_else(|| "completed".to_string());
    Some(UnifiedAgentEvent::BackgroundTask {
        task_id,
        status,
        kind: None,
        description: None,
        summary: text.lines().next().map(str::to_string),
    })
}

fn extract_bracket_status(text: &str) -> Option<&str> {
    let start = text.find("[status: ")? + 9;
    let rest = text.get(start..)?;
    let end = rest.find(|c: char| c == ']' || c == '.' || c == ',')?;
    Some(rest[..end].trim())
}

fn map_job_notice_status(raw: &str) -> String {
    match raw {
        "completed" => "completed",
        "killed" | "stopping" => "stopped",
        "failed" => "failed",
        _ => "completed",
    }
    .to_string()
}

fn map_subagent_notification(method: &str, params: &Value) -> Option<UnifiedAgentEvent> {
    let task_id = params
        .get("childSessionId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())?
        .to_string();
    match method {
        "subagent.started" => Some(UnifiedAgentEvent::BackgroundTask {
            task_id,
            status: "running".to_string(),
            kind: Some("local_agent".to_string()),
            description: None,
            summary: None,
        }),
        "subagent.finished" => {
            let ok = params.get("status").and_then(Value::as_str) == Some("ok");
            let summary = content_blocks_text(params.get("lastAssistantMessage"));
            Some(UnifiedAgentEvent::BackgroundTask {
                task_id,
                status: if ok { "completed" } else { "failed" }.to_string(),
                kind: None,
                description: None,
                summary: (!summary.is_empty()).then_some(summary),
            })
        }
        _ => None,
    }
}

/// 父轮已经收尾后，`tool-jobs` 会对空闲 owner `followup` 再开一轮。
/// `turn/start` 打开收集窗口，正文进唤醒消息；job notice 仍立刻出任务边沿。
fn fold_idle_parent_session(
    event_type: &str,
    data: &Value,
    map_state: &mut MapSessionState,
    wake: &mut IdleWakeState,
) -> IdleParentFold {
    if event_type == "turn/start" {
        wake.collecting = true;
        wake.text.clear();
        wake.usage = None;
    }
    let mut mapped = Vec::new();
    let terminal = map_session_event(event_type, data, map_state, &mut |event| {
        mapped.push(event);
    });
    if let Some(index) = mapped
        .iter()
        .position(|event| matches!(event, UnifiedAgentEvent::BackgroundTask { .. }))
    {
        return IdleParentFold::Event(mapped.remove(index));
    }
    if wake.collecting {
        for event in mapped {
            match event {
                UnifiedAgentEvent::TextDelta { delta } => wake.text.push_str(&delta),
                UnifiedAgentEvent::Usage { usage } => wake.usage = Some(usage),
                _ => {}
            }
        }
    }
    if event_type != "turn/end" {
        return IdleParentFold::Quiet;
    }
    wake.collecting = false;
    let text = std::mem::take(&mut wake.text).trim().to_string();
    let usage = wake.usage.take();
    let ok = matches!(terminal, None | Some(Ok(())));
    if ok && !text.is_empty() {
        IdleParentFold::Wake { text, usage }
    } else {
        IdleParentFold::Quiet
    }
}

fn fold_child_session_params(
    params: &Value,
    child_progress: &mut HashMap<String, ChildProgress>,
) -> Option<UnifiedAgentEvent> {
    let session_id = params.get("sessionId").and_then(Value::as_str)?;
    let event = params.get("event")?;
    let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
    let data = event.get("data").unwrap_or(&Value::Null);
    fold_child_session_event(session_id, event_type, data, child_progress)
}

fn fold_child_session_event(
    session_id: &str,
    event_type: &str,
    data: &Value,
    child_progress: &mut HashMap<String, ChildProgress>,
) -> Option<UnifiedAgentEvent> {
    if session_id.is_empty() {
        return None;
    }
    let progress = child_progress
        .entry(session_id.to_string())
        .or_default();
    let mut force = false;
    match event_type {
        "tool/call" => {
            let name = data.get("name").and_then(Value::as_str).unwrap_or("");
            if name.is_empty() {
                return None;
            }
            let raw = data.get("arguments").and_then(Value::as_str).unwrap_or("");
            let input = serde_json::from_str(raw).unwrap_or_else(|_| {
                if raw.is_empty() {
                    Value::Null
                } else {
                    json!({ "raw": raw })
                }
            });
            let step = format_child_tool_step(name, &input);
            if progress.steps.last() != Some(&step) {
                progress.steps.push(step);
                if progress.steps.len() > CHILD_STEPS_MAX {
                    let drop = progress.steps.len() - CHILD_STEPS_MAX;
                    progress.steps.drain(0..drop);
                }
            }
            force = true;
        }
        "assistant/chunk" => {
            let chunk = data.get("chunk")?;
            if chunk.get("type").and_then(Value::as_str) != Some("text-delta") {
                return None;
            }
            let delta = chunk.get("text").and_then(Value::as_str).unwrap_or("");
            if delta.is_empty() {
                return None;
            }
            progress.preview.push_str(delta);
            let count = progress.preview.chars().count();
            if count > CHILD_PREVIEW_MAX_CHARS {
                let skip = count - CHILD_PREVIEW_MAX_CHARS;
                progress.preview = progress.preview.chars().skip(skip).collect();
            }
        }
        "assistant/message" => {
            let text = content_blocks_text(data.get("message").and_then(|value| value.get("content")));
            if text.is_empty() {
                return None;
            }
            progress.preview = clip_chars(&text, CHILD_PREVIEW_MAX_CHARS);
            force = true;
        }
        _ => return None,
    }
    let now = Instant::now();
    if !force {
        if let Some(last) = progress.last_emit {
            if now.duration_since(last) < CHILD_PROGRESS_EMIT_INTERVAL {
                return None;
            }
        }
    }
    progress.last_emit = Some(now);
    Some(UnifiedAgentEvent::SubagentProgress {
        task_id: session_id.to_string(),
        status: "running".to_string(),
        preview: progress.preview.clone(),
        steps: progress.steps.clone(),
    })
}

fn format_child_tool_step(name: &str, input: &Value) -> String {
    let target = optional_string(input, "path")
        .or_else(|| optional_string(input, "file_path"))
        .or_else(|| optional_string(input, "command"))
        .or_else(|| optional_string(input, "query"))
        .or_else(|| optional_string(input, "pattern"))
        .or_else(|| optional_string(input, "description"));
    let line = match target {
        Some(target) => format!("{name} {target}"),
        None => name.to_string(),
    };
    clip_chars(&line, CHILD_STEP_MAX_CHARS)
}

fn clip_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn rpc_error_line(id: &Value, code: i64, message: &str, data: Option<Value>) -> String {
    let mut error = json!({
        "code": code,
        "message": message,
    });
    if let Some(data) = data {
        error
            .as_object_mut()
            .expect("error object")
            .insert("data".to_string(), data);
    }
    let mut line = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": error,
    }))
    .unwrap_or_default();
    line.push('\n');
    line
}

/// 官方 `todo/write` 快照 → Kivio 对话上的 todo 列表。
///
/// 条目没有 id，官方用 **content** 当身份（重复 content 会被 execute 拒）。
/// 多个 `in_progress` 原样保留（preset 的 `allowParallelInProgress: true`）；
/// 不要走内置 `normalized_state`，那条会把多余的进行中降成 pending。
///
/// ponytail: 官方 web 在 `turn/start` 清掉 standing plan。Kivio 的列表挂在对话上，
/// 跨轮保留，和内置 `todo_write` 一样。
pub(crate) fn todo_state_from_write(data: &Value) -> Option<crate::chat::types::AgentTodoState> {
    let raw = data.get("todos")?.as_array()?;
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in raw {
        let Some(content) = item
            .get("content")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(content);
        if !seen.insert(id.to_string()) {
            continue;
        }
        let status = match item.get("status").and_then(Value::as_str) {
            Some("in_progress") => crate::chat::types::AgentTodoStatus::InProgress,
            Some("completed") => crate::chat::types::AgentTodoStatus::Completed,
            Some("pending") => crate::chat::types::AgentTodoStatus::Pending,
            _ => continue,
        };
        items.push(crate::chat::types::AgentTodoItem {
            id: id.to_string(),
            content: content.to_string(),
            status,
            ..Default::default()
        });
    }
    if items.is_empty() && !raw.is_empty() {
        return None;
    }
    Some(crate::chat::types::AgentTodoState {
        items,
        updated_at: chrono::Local::now().timestamp(),
    })
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

fn map_tool_results(
    data: &Value,
    state: &mut MapSessionState,
    sink: &mut dyn FnMut(UnifiedAgentEvent),
) {
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
        if let Some(pending) = state.background_calls.remove(&tool_use_id) {
            if let Some(task_id) = parse_background_task_id(&content) {
                sink(UnifiedAgentEvent::BackgroundTask {
                    task_id,
                    status: "running".to_string(),
                    kind: Some(background_task_kind(&pending.name)),
                    description: pending.description,
                    summary: None,
                });
            }
        }
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
    use serde_json::Value;

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
    fn recognizes_dsh_user_question_tools_and_incoming_ask_requests() {
        assert!(is_user_question_tool("ask_user_question"));
        assert!(is_user_question_tool("exit_plan_mode"));
        assert!(!is_user_question_tool("bash"));
        let ask = json!({
            "jsonrpc": "2.0",
            "id": "req_abc",
            "method": "session/ask",
            "params": {
                "sessionId": "kivio-1",
                "questions": [{ "id": "q1", "question": "Tea or coffee?" }]
            }
        });
        assert!(is_incoming_rpc_request(&ask));
        assert_eq!(rpc_id_key(&ask["id"]).as_deref(), Some("req_abc"));
        assert!(!is_incoming_rpc_request(&json!({
            "jsonrpc": "2.0",
            "method": "session.event",
            "params": { "sessionId": "kivio-1" }
        })));
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
        let mut state = MapSessionState::default();
        map_session_event(
            "request/context",
            &json!({ "contextWindow": 1_000_000 }),
            &mut state,
            &mut |event| emitted.push(event),
        );
        map_session_event(
            "assistant/chunk",
            &json!({ "chunk": { "type": "text-delta", "text": "OK" } }),
            &mut state,
            &mut |event| emitted.push(event),
        );
        map_session_event(
            "assistant/chunk",
            &json!({ "chunk": { "type": "reasoning-delta", "text": "Think" } }),
            &mut state,
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
            &mut state,
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
    fn maps_todo_write_snapshot_to_the_conversation_list() {
        let mut emitted = Vec::new();
        let mut state = MapSessionState::default();
        map_session_event(
            "todo/write",
            &json!({
                "todos": [
                    { "content": "读协议", "status": "completed" },
                    { "content": "接线", "status": "in_progress" },
                    { "content": "补测试", "status": "pending" },
                    { "content": "接线", "status": "pending" },
                    { "content": "  ", "status": "pending" },
                    { "content": "没状态" },
                ]
            }),
            &mut state,
            &mut |event| emitted.push(event),
        );
        let UnifiedAgentEvent::TodoWrite { todos } = &emitted[0] else {
            panic!("expected TodoWrite");
        };
        let state = todo_state_from_write(todos).expect("必须能映射");
        assert_eq!(state.items.len(), 3);
        assert_eq!(state.items[0].id, "读协议");
        let with_id = todo_state_from_write(&json!({
            "todos": [{ "id": "a", "content": "有 id", "status": "pending" }]
        }))
        .expect("id 优先于 content");
        assert_eq!(with_id.items[0].id, "a");
        assert_eq!(with_id.items[0].content, "有 id");
        assert_eq!(
            state.items[0].status,
            crate::chat::types::AgentTodoStatus::Completed
        );
        assert_eq!(
            state.items[1].status,
            crate::chat::types::AgentTodoStatus::InProgress
        );
        assert_eq!(state.items[2].content, "补测试");
        assert!(state.updated_at > 0);
        assert!(todo_state_from_write(&json!({})).is_none());
        let cleared = todo_state_from_write(&json!({ "todos": [] })).expect("空表是合法整表替换");
        assert!(cleared.items.is_empty());
        assert!(todo_state_from_write(&json!({
            "todos": [{ "content": "没状态" }]
        }))
        .is_none());
    }

    #[test]
    fn maps_complete_tool_call_and_result() {
        let mut emitted = Vec::new();
        let mut state = MapSessionState::default();
        map_session_event(
            "tool/call",
            &json!({
                "callId": "call_1",
                "name": "bash",
                "arguments": "{\"command\":\"echo ok\"}"
            }),
            &mut state,
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
            &mut state,
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
        let mut state = MapSessionState::default();
        let result = map_session_event(
            "turn/end",
            &json!({ "reason": { "kind": "error", "error": {
                "message": "missing credential", "code": "MISSING_CREDENTIAL"
            } } }),
            &mut state,
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
        let mut state = MapSessionState::default();
        map_session_event(
            "assistant/chunk",
            &json!({ "chunk": {
                "type": "tool-call-delta",
                "id": "call_1",
                "name": "bash",
                "argumentsDelta": "{\"command\""
            } }),
            &mut state,
            &mut |event| emitted.push(event),
        );
        assert!(emitted.is_empty());
    }

    fn map_events(pairs: &[(&str, Value)]) -> Vec<UnifiedAgentEvent> {
        let mut emitted = Vec::new();
        let mut state = MapSessionState::default();
        for (event_type, data) in pairs {
            map_session_event(event_type, data, &mut state, &mut |event| {
                emitted.push(event)
            });
        }
        emitted
    }

    #[test]
    fn maps_compaction_summary_and_end_to_one_cli_compacted() {
        let events = map_events(&[
            ("compaction/start", json!({ "compactionId": "c1", "turn": 2 })),
            (
                "compaction/summary",
                json!({
                    "compactionId": "c1",
                    "shadowedTokenCount": 1200
                }),
            ),
            ("compaction/end", json!({ "compactionId": "c1", "turn": 2 })),
        ]);
        assert!(matches!(
            &events[0],
            UnifiedAgentEvent::StatusNote { text } if text == "正在压缩…"
        ));
        assert!(matches!(
            &events[1],
            UnifiedAgentEvent::CliCompacted {
                trigger,
                dropped_tokens,
                ..
            } if trigger == "auto" && *dropped_tokens == Some(1200)
        ));
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn maps_manual_compaction_when_source_command_id_is_present() {
        let events = map_events(&[
            (
                "compaction/start",
                json!({
                    "compactionId": "c2",
                    "sourceCommandId": "cmd_1",
                    "turn": null
                }),
            ),
            (
                "compaction/summary",
                json!({
                    "compactionId": "c2",
                    "sourceCommandId": "cmd_1",
                    "shadowedTokenCount": 80
                }),
            ),
            (
                "compaction/end",
                json!({
                    "compactionId": "c2",
                    "sourceCommandId": "cmd_1",
                    "turn": null
                }),
            ),
        ]);
        assert!(matches!(
            &events[1],
            UnifiedAgentEvent::CliCompacted { trigger, dropped_tokens, .. }
                if trigger == "manual" && *dropped_tokens == Some(80)
        ));
    }

    #[test]
    fn compaction_error_is_a_status_note_not_a_divider() {
        let events = map_events(&[
            ("compaction/start", json!({ "compactionId": "c3", "turn": 1 })),
            (
                "compaction/end",
                json!({
                    "compactionId": "c3",
                    "turn": 1,
                    "error": "No compactable history"
                }),
            ),
        ]);
        assert!(events.iter().any(|event| matches!(
            event,
            UnifiedAgentEvent::StatusNote { text } if text.contains("No compactable history")
        )));
        assert!(!events
            .iter()
            .any(|event| matches!(event, UnifiedAgentEvent::CliCompacted { .. })));
    }

    #[test]
    fn maps_llm_retry_to_status_note() {
        let events = map_events(&[
            (
                "llm/retry",
                json!({ "retry": 2, "maxRetries": 5, "delayMs": 800 }),
            ),
            ("llm/retry-started", json!({ "retry": 2 })),
        ]);
        assert!(matches!(
            &events[0],
            UnifiedAgentEvent::StatusNote { text } if text == "上游重试 2/5"
        ));
        assert!(matches!(
            &events[1],
            UnifiedAgentEvent::StatusNote { text } if text == "正在重试模型调用…"
        ));
    }

    #[test]
    fn maps_background_bash_result_to_a_running_task() {
        let events = map_events(&[
            (
                "tool/call",
                json!({
                    "callId": "call_bg",
                    "name": "pwsh",
                    "arguments": "{\"command\":\"npm run dev\",\"run_in_background\":true,\"description\":\"dev server\"}"
                }),
            ),
            (
                "tool/result",
                json!({ "message": { "content": [{
                    "type": "tool-result",
                    "toolCallId": "call_bg",
                    "content": [{ "type": "text", "text": "started background job bash-3" }],
                    "isError": false
                }] } }),
            ),
        ]);
        assert!(matches!(
            events.iter().find(|event| matches!(event, UnifiedAgentEvent::BackgroundTask { .. })),
            Some(UnifiedAgentEvent::BackgroundTask {
                task_id,
                status,
                kind,
                description,
                ..
            }) if task_id == "bash-3"
                && status == "running"
                && kind.as_deref() == Some("local_bash")
                && description.as_deref() == Some("dev server")
        ));
    }

    #[test]
    fn default_background_subagent_launch_emits_a_running_task() {
        let events = map_events(&[
            (
                "tool/call",
                json!({
                    "callId": "call_sa",
                    "name": "subagent",
                    "arguments": "{\"description\":\"搜索最新AI资讯\",\"prompt\":\"去搜\"}"
                }),
            ),
            (
                "tool/result",
                json!({ "message": { "content": [{
                    "type": "tool-result",
                    "toolCallId": "call_sa",
                    "content": [{ "type": "text", "text": "started subagent 018b08fc-ee7f-4ea5-b77c-9c5d1c6ecf50" }],
                    "isError": false
                }] } }),
            ),
        ]);
        assert!(matches!(
            events.iter().find(|event| matches!(event, UnifiedAgentEvent::BackgroundTask { .. })),
            Some(UnifiedAgentEvent::BackgroundTask {
                task_id,
                status,
                kind,
                description,
                ..
            }) if task_id == "018b08fc-ee7f-4ea5-b77c-9c5d1c6ecf50"
                && status == "running"
                && kind.as_deref() == Some("local_agent")
                && description.as_deref() == Some("搜索最新AI资讯")
        ));
    }

    #[test]
    fn maps_tool_jobs_notice_to_a_terminal_task() {
        let events = map_events(&[(
            "user/message",
            json!({
                "id": "n1",
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "background job bash-3 (bash: npm run dev) finished [status: killed]. Read its output with job_output."
                }],
                "source": { "kind": "plugin", "plugin": "tool-jobs", "form": "notice" }
            }),
        )]);
        assert!(matches!(
            &events[0],
            UnifiedAgentEvent::BackgroundTask {
                task_id,
                status,
                summary,
                ..
            } if task_id == "bash-3"
                && status == "stopped"
                && summary.as_deref().is_some_and(|text| text.contains("bash-3"))
        ));
    }

    #[test]
    fn maps_subagent_edges_only_for_the_parent_session() {
        assert!(matches!(
            map_subagent_notification(
                "subagent.started",
                &json!({
                    "parentSessionId": "kivio-1",
                    "childSessionId": "child-9"
                }),
            ),
            Some(UnifiedAgentEvent::BackgroundTask {
                task_id,
                status,
                kind,
                ..
            }) if task_id == "child-9"
                && status == "running"
                && kind.as_deref() == Some("local_agent")
        ));
        assert!(matches!(
            map_subagent_notification(
                "subagent.finished",
                &json!({
                    "parentSessionId": "kivio-1",
                    "childSessionId": "child-9",
                    "status": "error",
                    "stopReason": "error",
                    "lastAssistantMessage": [{ "type": "text", "text": "boom" }]
                }),
            ),
            Some(UnifiedAgentEvent::BackgroundTask {
                task_id,
                status,
                summary,
                ..
            })             if task_id == "child-9"
                && status == "failed"
                && summary.as_deref() == Some("boom")
        ));
    }

    #[test]
    fn child_session_tools_and_text_fold_into_progress() {
        let mut progress = HashMap::new();
        let first = fold_child_session_event(
            "child-9",
            "tool/call",
            &json!({
                "callId": "c1",
                "name": "web_search",
                "arguments": "{\"query\":\"最新AI资讯\"}"
            }),
            &mut progress,
        );
        assert!(matches!(
            first,
            Some(UnifiedAgentEvent::SubagentProgress {
                task_id,
                status,
                steps,
                ..
            }) if task_id == "child-9"
                && status == "running"
                && steps == ["web_search 最新AI资讯".to_string()]
        ));
        let second = fold_child_session_event(
            "child-9",
            "assistant/chunk",
            &json!({ "chunk": { "type": "text-delta", "text": "正在检索" } }),
            &mut progress,
        );
        // 紧跟着的正文被节流；下一步工具会带上攒下的 preview。
        assert!(second.is_none());
        let third = fold_child_session_event(
            "child-9",
            "tool/call",
            &json!({
                "callId": "c2",
                "name": "web_fetch",
                "arguments": "{\"path\":\"https://example.com\"}"
            }),
            &mut progress,
        );
        assert!(matches!(
            third,
            Some(UnifiedAgentEvent::SubagentProgress {
                preview,
                steps,
                ..
            }) if preview == "正在检索"
                && steps == [
                    "web_search 最新AI资讯".to_string(),
                    "web_fetch https://example.com".to_string()
                ]
        ));
        assert!(
            fold_child_session_event(
                "child-9",
                "turn/end",
                &json!({ "reason": { "kind": "completed" } }),
                &mut progress,
            )
            .is_none()
        );
    }

    #[test]
    fn idle_parent_wake_turn_keeps_the_assistant_reply() {
        let mut map_state = MapSessionState::default();
        let mut wake = IdleWakeState::default();
        assert!(matches!(
            fold_idle_parent_session(
                "turn/start",
                &json!({ "turn": 2 }),
                &mut map_state,
                &mut wake,
            ),
            IdleParentFold::Quiet
        ));
        assert!(matches!(
            fold_idle_parent_session(
                "user/message",
                &json!({
                    "id": "n1",
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": "background job subagent-1 (subagent: 搜索最新AI资讯) finished [status: completed]. Read its output with job_output."
                    }],
                    "source": { "kind": "plugin", "plugin": "tool-jobs", "form": "notice" }
                }),
                &mut map_state,
                &mut wake,
            ),
            IdleParentFold::Event(UnifiedAgentEvent::BackgroundTask { task_id, status, .. })
                if task_id == "subagent-1" && status == "completed"
        ));
        assert!(matches!(
            fold_idle_parent_session(
                "assistant/chunk",
                &json!({ "chunk": { "type": "text-delta", "text": "12 条资讯" } }),
                &mut map_state,
                &mut wake,
            ),
            IdleParentFold::Quiet
        ));
        assert!(matches!(
            fold_idle_parent_session(
                "turn/end",
                &json!({ "turn": 2, "reason": { "kind": "completed" } }),
                &mut map_state,
                &mut wake,
            ),
            IdleParentFold::Wake { text, .. } if text == "12 条资讯"
        ));
    }

    #[test]
    fn subagent_launch_receipt_is_not_a_finished_result() {
        assert_eq!(
            subagent_launch_task_id(
                "subagent",
                "started subagent 018b08fc-ee7f-4ea5-b77c-9c5d1c6ecf50"
            )
            .as_deref(),
            Some("018b08fc-ee7f-4ea5-b77c-9c5d1c6ecf50")
        );
        assert_eq!(
            subagent_launch_task_id("subagent_fork", "started background subagent task job_9")
                .as_deref(),
            Some("job_9")
        );
        assert!(subagent_launch_task_id("bash", "started background job bash-3").is_none());
        assert!(subagent_launch_task_id("subagent", "the child found three sources").is_none());
    }

    #[test]
    fn slash_turns_use_session_command_not_prompt() {
        let (method, params) = turn_rpc("kivio-1", "  /compact ");
        assert_eq!(method, "session/command");
        assert_eq!(params["sessionId"], "kivio-1");
        assert_eq!(params["line"], "/compact");
        let (method, params) = turn_rpc("kivio-1", "hello");
        assert_eq!(method, "session/prompt");
        assert_eq!(params["contentBlocks"][0]["text"], "hello");
        assert_eq!(
            command_result_error(&json!({
                "kind": "error",
                "text": "No compactable history"
            }))
            .as_deref(),
            Some("No compactable history")
        );
        assert!(command_result_error(&json!({ "kind": "success", "text": "ok" })).is_none());
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
            None,
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
                None,
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
            None,
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
                None,
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
            None,
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
                None,
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
            None,
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
                None,
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
                None,
            )
            .await
            .expect("session should remain usable after cancel");
        session.close().await;
    }
}
