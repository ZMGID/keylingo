//! 对话生命周期 Hooks：在内置 agent loop 的 8 个事件点执行用户配置的 Shell 脚本
//! 或 HTTP 请求（任务 07-28-hooks）。
//!
//! **调度端到端在 Rust**。生命周期事件天然发生在 `chat/agent/loop_.rs`，把执行链放到
//! 前端就要为每个事件加一条 Tauri 事件 + 一次 IPC 往返 + 一套跨进程 scope 取消注册表。
//! 前端只负责配置 UI 和展示失败警告（typed protocol event）。
//!
//! Hook 一律 **fire-and-forget**：不能拒绝工具调用、不能改写参数、不能强制模型继续。
//! 失败只上报警告，绝不打断对话。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::AppHandle;

use crate::settings::HookDef;

/// 队列上限。溢出即丢弃（并只警告一次）——Hook 是旁路观测，宁可丢事件也不能反压 loop。
const MAX_QUEUED: usize = 64;

/// 对话生命周期事件。8 个，与 loop 的阶段一一对应，不发明新词。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    AgentStart,
    TurnStart,
    MessageStart,
    MessageEnd,
    ToolExecutionStart,
    ToolExecutionEnd,
    TurnEnd,
    AgentEnd,
}

impl HookEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentStart => "agent_start",
            Self::TurnStart => "turn_start",
            Self::MessageStart => "message_start",
            Self::MessageEnd => "message_end",
            Self::ToolExecutionStart => "tool_execution_start",
            Self::ToolExecutionEnd => "tool_execution_end",
            Self::TurnEnd => "turn_end",
            Self::AgentEnd => "agent_end",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "agent_start" => Self::AgentStart,
            "turn_start" => Self::TurnStart,
            "message_start" => Self::MessageStart,
            "message_end" => Self::MessageEnd,
            "tool_execution_start" => Self::ToolExecutionStart,
            "tool_execution_end" => Self::ToolExecutionEnd,
            "turn_end" => Self::TurnEnd,
            "agent_end" => Self::AgentEnd,
            _ => return None,
        })
    }
}

/// Hook 脚本 stdin 收到的一行 JSON（HTTP Hook 的 POST body 同此）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HookPayload<'a> {
    event: &'a str,
    hook_name: &'a str,
    conversation_id: &'a str,
    run_id: &'a str,
    message_id: &'a str,
    cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    round: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
    model: &'a str,
}

/// 每个 run 固定不变的上下文（会话/run/消息 id、工作目录、模型）。
struct RunContext {
    conversation_id: String,
    run_id: String,
    message_id: String,
    cwd: PathBuf,
    model: String,
}

struct HookJob {
    event: HookEvent,
    tool_name: Option<String>,
    tool_call_id: Option<String>,
    round: Option<u32>,
    /// 派发时的取消世代。worker 见到比当前世代旧的 job 直接丢弃。
    epoch: u64,
}

/// 失败上报通道。生产环境走 typed protocol hub；单测注入一个收集闭包，
/// 于是整个执行器不依赖 Tauri 运行时即可测试。
type FailureSink = Box<dyn Fn(serde_json::Value) + Send + Sync>;

struct Shared {
    ctx: RunContext,
    /// event → 该事件下启用的 Hook。
    by_event: HashMap<&'static str, Vec<HookDef>>,
    sink: FailureSink,
    /// 当前取消世代；`cancel()` 递增。
    epoch: AtomicU64,
    /// `cancel()` 叫醒正在 await 的 HTTP，好让 `select!` 丢掉 in-flight 请求。
    cancel_notify: tokio::sync::Notify,
    /// 在跑的脚本的进程组 leader pid（用于 `cancel()` 杀进程）。
    running_pid: Mutex<Option<u32>>,
    queued: AtomicUsize,
    overflow_warned: AtomicBool,
    /// 单测用的派发流水：记录每次 `dispatch` 的 `(event, tool_name)`，用于在 `loop_tests`
    /// 里断言事件配对，而不必为此起 8 个子进程。
    #[cfg(test)]
    log: Mutex<Vec<(&'static str, Option<String>)>>,
    /// 单测置 false：只记流水、不真的执行 Hook。
    execute: bool,
}

/// 按 run 创建的 Hook 调度器。串行消费，保证事件顺序。
pub struct HookDispatcher {
    tx: tokio::sync::mpsc::UnboundedSender<HookJob>,
    /// 生产路径不 join worker（detached，Drop 后自行排空退出）；只有单测的 `drain` 会等它。
    #[cfg_attr(not(test), allow(dead_code))]
    worker: tauri::async_runtime::JoinHandle<()>,
    shared: Arc<Shared>,
}

impl HookDispatcher {
    /// 这批配置里有没有可用的 Hook。调用侧据此在**构造入参之前**短路，
    /// 于是「没配 Hook」连那几个 id / model 字符串都不会分配（验收 6）。
    pub fn any_enabled(hooks: &[HookDef]) -> bool {
        hooks
            .iter()
            .any(|hook| hook.enabled && HookEvent::parse(&hook.event).is_some())
    }

    /// 无启用 Hook 时返回 `None` —— 调用侧零开销（不构造载荷、不进队列）。
    pub fn new(
        app: AppHandle,
        hooks: &[HookDef],
        conversation_id: String,
        run_id: String,
        message_id: String,
        cwd: PathBuf,
        model: String,
    ) -> Option<Self> {
        let sink_run_id = run_id.clone();
        let sink_conversation_id = conversation_id.clone();
        let sink: FailureSink = Box::new(move |payload| {
            crate::chat::protocol::emit_hook_failed(
                &app,
                payload
                    .get("conversationId")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(&sink_conversation_id),
                &sink_run_id,
                payload
                    .get("hookName")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                payload
                    .get("event")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                payload
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
        });
        Self::with_sink(
            hooks,
            RunContext {
                conversation_id,
                run_id,
                message_id,
                cwd,
                model,
            },
            sink,
        )
    }

    fn with_sink(hooks: &[HookDef], ctx: RunContext, sink: FailureSink) -> Option<Self> {
        Self::build(hooks, ctx, sink, true)
    }

    /// 只记派发流水、不真的执行 Hook 的调度器（`loop_tests` 用它断言事件配对）。
    #[cfg(test)]
    pub(crate) fn recording() -> Self {
        let hook = HookDef {
            id: "rec".to_string(),
            name: "rec".to_string(),
            enabled: true,
            kind: "command".to_string(),
            script: "true".to_string(),
            timeout_ms: 1_000,
            ..Default::default()
        };
        // 8 个事件各挂一个，才能把每次 dispatch 都记下来（dispatch 对没配 Hook 的事件短路）。
        let hooks: Vec<HookDef> = [
            "agent_start",
            "turn_start",
            "message_start",
            "message_end",
            "tool_execution_start",
            "tool_execution_end",
            "turn_end",
            "agent_end",
        ]
        .iter()
        .map(|event| HookDef {
            event: event.to_string(),
            ..hook.clone()
        })
        .collect();
        Self::build(
            &hooks,
            RunContext {
                conversation_id: "conv".to_string(),
                run_id: "run".to_string(),
                message_id: "msg".to_string(),
                cwd: std::env::temp_dir(),
                model: "m".to_string(),
            },
            Box::new(|_| {}),
            false,
        )
        .expect("recording dispatcher")
    }

    /// 已派发事件流水（`(event, tool_name)`，按序）。
    #[cfg(test)]
    pub(crate) fn recorded(&self) -> Vec<(&'static str, Option<String>)> {
        self.shared
            .log
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    /// 取消世代。>0 即「本 run 期间至少 `cancel()` 过一次」，`loop_tests` 用它断言
    /// 取消真的传到了调度器（光看事件流水看不出来：取消路径的事件序列与正常路径同形）。
    #[cfg(test)]
    pub(crate) fn cancel_epoch(&self) -> u64 {
        self.shared.epoch.load(Ordering::SeqCst)
    }

    fn build(
        hooks: &[HookDef],
        mut ctx: RunContext,
        sink: FailureSink,
        execute: bool,
    ) -> Option<Self> {
        let mut by_event: HashMap<&'static str, Vec<HookDef>> = HashMap::new();
        for hook in hooks.iter().filter(|hook| hook.enabled) {
            let Some(event) = HookEvent::parse(&hook.event) else {
                continue;
            };
            by_event
                .entry(event.as_str())
                .or_default()
                .push(hook.clone());
        }
        if by_event.is_empty() {
            return None;
        }

        // 会话工作目录是**懒创建**的（只有原生工具真的解析出 cwd 时才 mkdir），所以一个
        // 从没用过工具的普通会话，这个路径可能根本不存在 —— 直接拿去当 `current_dir`
        // 会让 spawn 以 ENOENT 失败，用户看到的是「hook 启动失败」而不是自己的脚本输出。
        // 落到临时目录（对齐设计里「解析不出则用临时目录」）而不是 mkdir：不能因为配了
        // 一个 Hook 就在工作区根下凭空生出空目录。
        if !ctx.cwd.is_dir() {
            ctx.cwd = std::env::temp_dir();
        }

        let shared = Arc::new(Shared {
            ctx,
            by_event,
            sink,
            epoch: AtomicU64::new(0),
            cancel_notify: tokio::sync::Notify::new(),
            running_pid: Mutex::new(None),
            queued: AtomicUsize::new(0),
            overflow_warned: AtomicBool::new(false),
            #[cfg(test)]
            log: Mutex::new(Vec::new()),
            execute,
        });
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<HookJob>();
        let worker_shared = Arc::clone(&shared);
        let worker = tauri::async_runtime::spawn(async move {
            while let Some(job) = rx.recv().await {
                worker_shared.queued.fetch_sub(1, Ordering::Relaxed);
                run_job(&worker_shared, job).await;
            }
        });
        Some(Self { tx, worker, shared })
    }

    /// 只是入队，永不阻塞 loop。
    pub fn dispatch(
        &self,
        event: HookEvent,
        tool_name: Option<&str>,
        round: Option<u32>,
        tool_call_id: Option<&str>,
    ) {
        if !self.shared.by_event.contains_key(event.as_str()) {
            return;
        }
        // 流水记在入队处（而非 worker 里）：断言的是 loop 的派发顺序，不是执行调度。
        #[cfg(test)]
        if let Ok(mut log) = self.shared.log.lock() {
            log.push((event.as_str(), tool_name.map(str::to_string)));
        }
        if !self.shared.execute {
            return;
        }
        if self.shared.queued.load(Ordering::Relaxed) >= MAX_QUEUED {
            if !self.shared.overflow_warned.swap(true, Ordering::Relaxed) {
                self.shared.report(
                    event.as_str(),
                    "",
                    &format!("Hook queue overflowed ({MAX_QUEUED}); further events are dropped"),
                );
            }
            return;
        }
        self.shared.queued.fetch_add(1, Ordering::Relaxed);
        let job = HookJob {
            event,
            tool_name: tool_name.map(str::to_string),
            tool_call_id: tool_call_id.map(str::to_string),
            round,
            epoch: self.shared.epoch.load(Ordering::SeqCst),
        };
        if self.tx.send(job).is_err() {
            self.shared.queued.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// 用户停止 / run 取消：丢弃排队中的 Hook，杀掉在跑的脚本。
    /// **不永久禁用**——`agent_end` 在取消后仍需触发（它由 Drop guard 派发），
    /// 所以这里只作废「此刻之前」入队的 job，之后新派发的照常执行。
    pub fn cancel(&self) {
        self.shared.epoch.fetch_add(1, Ordering::SeqCst);
        self.shared.cancel_notify.notify_waiters();
        let pid = self
            .shared
            .running_pid
            .lock()
            .ok()
            .and_then(|mut p| p.take());
        if let Some(pid) = pid {
            crate::native_tools::kill_process_group(pid);
        }
    }

    /// 关闭队列并等 worker 排空。单测用（生产路径靠 Drop：sender 落地 → 通道关闭 →
    /// worker 排空剩余事件后自然退出，所以最后的 `agent_end` 不会被吞掉）。
    #[cfg(test)]
    async fn drain(self) {
        let Self { tx, worker, .. } = self;
        drop(tx);
        let _ = worker.await;
    }
}

impl Shared {
    fn report(&self, event: &str, hook_name: &str, message: &str) {
        (self.sink)(serde_json::json!({
            "conversationId": self.ctx.conversation_id,
            "runId": self.ctx.run_id,
            "hookName": hook_name,
            "event": event,
            "message": message,
        }));
    }
}

async fn run_job(shared: &Shared, job: HookJob) {
    let Some(hooks) = shared.by_event.get(job.event.as_str()) else {
        return;
    };
    for hook in hooks {
        if shared.epoch.load(Ordering::SeqCst) != job.epoch {
            return;
        }
        let payload = HookPayload {
            event: job.event.as_str(),
            hook_name: &hook.name,
            conversation_id: &shared.ctx.conversation_id,
            run_id: &shared.ctx.run_id,
            message_id: &shared.ctx.message_id,
            cwd: shared.ctx.cwd.display().to_string(),
            round: job.round,
            tool_name: job.tool_name.as_deref(),
            tool_call_id: job.tool_call_id.as_deref(),
            model: &shared.ctx.model,
        };
        let body = match serde_json::to_string(&payload) {
            Ok(body) => body,
            Err(err) => {
                shared.report(job.event.as_str(), &hook.name, &format!("payload: {err}"));
                continue;
            }
        };
        let result = match hook.kind.as_str() {
            "http" => {
                tokio::select! {
                    biased;
                    _ = wait_cancelled(shared, job.epoch) => Err("hook cancelled".to_string()),
                    result = run_http_hook(hook, &body) => result,
                }
            }
            _ => run_command_hook(hook, &body, &payload, shared, job.epoch).await,
        };
        // 被 cancel 杀掉的脚本会以失败返回，那不是用户该看的错误 —— 世代已变则静默。
        if let Err(err) = result {
            if shared.epoch.load(Ordering::SeqCst) == job.epoch {
                shared.report(job.event.as_str(), &hook.name, &err);
            }
        }
    }
}

/// 执行 command 类 Hook：载荷 JSON 写 stdin，`KIVIO_*` env 注入，超时杀进程组。
async fn run_command_hook(
    hook: &HookDef,
    body: &str,
    payload: &HookPayload<'_>,
    shared: &Shared,
    job_epoch: u64,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt as _;

    let mut cmd = crate::native_tools::build_shell_command(&hook.script);
    cmd.current_dir(&payload.cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("KIVIO_HOOK_EVENT", payload.event)
        .env("KIVIO_HOOK_NAME", payload.hook_name)
        .env("KIVIO_CONVERSATION_ID", payload.conversation_id)
        .env("KIVIO_RUN_ID", payload.run_id)
        .env("KIVIO_WORKDIR", &payload.cwd)
        .env("KIVIO_TOOL_NAME", payload.tool_name.unwrap_or(""))
        .env("KIVIO_TOOL_CALL_ID", payload.tool_call_id.unwrap_or(""))
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        use crate::proc::NoConsoleWindow as _;
        cmd.no_console_window();
    }
    // setsid：让 `kill_process_group` 能连子孙一起杀（对齐 native_tools::shell）。
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = cmd
        .spawn()
        .map_err(|err| format!("failed to start hook script: {err}"))?;
    let pid = child.id();
    // 登记 pid 供 `cancel()` 杀进程。登记后必须复检世代：cancel 可能恰好落在
    // 「上面 run_job 检查过世代」与「这里登记」之间 —— 那次 cancel 看到的是空槽位，
    // 于是这个刚起来的进程谁也不会杀（脚本是 `sleep 600` 就泄漏到 run 结束之后）。
    // 复检发现世代已变则自己收尾。
    if let Some(pid) = pid {
        if let Ok(mut slot) = shared.running_pid.lock() {
            *slot = Some(pid);
        }
        if shared.epoch.load(Ordering::SeqCst) != job_epoch {
            if let Ok(mut slot) = shared.running_pid.lock() {
                if *slot == Some(pid) {
                    *slot = None;
                }
            }
            crate::native_tools::kill_process_group(pid);
            return Err("hook cancelled".to_string());
        }
    }
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(body.as_bytes()).await;
        let _ = stdin.write_all(b"\n").await;
        let _ = stdin.shutdown().await;
    }

    let waited = tokio::time::timeout(
        std::time::Duration::from_millis(hook.timeout_ms),
        child.wait_with_output(),
    )
    .await;
    if let Ok(mut slot) = shared.running_pid.lock() {
        if *slot == pid {
            *slot = None;
        }
    }

    let output = match waited {
        Err(_) => {
            if let Some(pid) = pid {
                crate::native_tools::kill_process_group(pid);
            }
            return Err(format!("hook timed out after {}ms", hook.timeout_ms));
        }
        Ok(Err(err)) => return Err(format!("hook script failed: {err}")),
        Ok(Ok(output)) => output,
    };
    if output.status.success() {
        return Ok(());
    }
    let code = output
        .status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "signal".to_string());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    if detail.is_empty() {
        Err(format!("hook exited with code {code}"))
    } else {
        Err(format!("hook exited with code {code}: {detail}"))
    }
}

/// 等到取消世代越过 `job_epoch`。先订阅 Notify 再读 epoch，避免 cancel 落在
/// 检查与 wait 之间而永久挂起。
async fn wait_cancelled(shared: &Shared, job_epoch: u64) {
    loop {
        let notified = shared.cancel_notify.notified();
        if shared.epoch.load(Ordering::SeqCst) != job_epoch {
            return;
        }
        notified.await;
    }
}

/// 执行 http 类 Hook。仅 2xx 视为成功。GET/HEAD 不带 body。
// ponytail: 载荷永远是 Hook 自己的 JSON —— `HookDef` 没有自定义 body 字段，
// 需要别的形状就用 command 类 Hook 加一行 curl。
async fn run_http_hook(hook: &HookDef, body: &str) -> Result<(), String> {
    let method = reqwest::Method::from_bytes(hook.method.as_bytes())
        .map_err(|_| format!("invalid HTTP method {}", hook.method))?;
    let send_body = method != reqwest::Method::GET && method != reqwest::Method::HEAD;
    let mut request = crate::api::build_http_client()
        .request(method, hook.url.trim())
        .timeout(std::time::Duration::from_millis(hook.timeout_ms))
        .header("content-type", "application/json")
        .header("X-Kivio-Hook-Event", hook.event.as_str());
    if send_body {
        request = request.body(body.to_string());
    }
    for (key, value) in &hook.headers {
        request = request.header(key, value);
    }
    let response = request
        .send()
        .await
        .map_err(|err| format!("hook request failed: {err}"))?;
    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(format!("hook responded {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    /// 自清理的临时目录（本 crate 无 `tempfile` 依赖，沿用 shell.rs 的 temp_dir+uuid 写法）。
    #[cfg(unix)]
    struct TempDir(PathBuf);
    #[cfg(unix)]
    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("kivio_hook_{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).expect("mkdir temp");
            Self(path)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    #[cfg(unix)]
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn command_hook(script: &str) -> HookDef {
        HookDef {
            id: "h1".to_string(),
            name: "probe".to_string(),
            event: "agent_end".to_string(),
            enabled: true,
            kind: "command".to_string(),
            script: script.to_string(),
            timeout_ms: 10_000,
            ..Default::default()
        }
    }

    fn ctx(cwd: PathBuf) -> RunContext {
        RunContext {
            conversation_id: "conv-1".to_string(),
            run_id: "run-1".to_string(),
            message_id: "msg-1".to_string(),
            cwd,
            model: "prov:model-x".to_string(),
        }
    }

    /// `(dispatcher, failure receiver)`
    fn dispatcher(
        hooks: &[HookDef],
        cwd: PathBuf,
    ) -> Option<(HookDispatcher, mpsc::Receiver<serde_json::Value>)> {
        let (tx, rx) = mpsc::channel();
        let sink: FailureSink = Box::new(move |payload| {
            let _ = tx.send(payload);
        });
        HookDispatcher::with_sink(hooks, ctx(cwd), sink).map(|dispatcher| (dispatcher, rx))
    }

    #[test]
    fn new_returns_none_without_enabled_hooks() {
        // 验收 6：无 Hook / 全禁用 → 调用侧零开销。
        assert!(dispatcher(&[], std::env::temp_dir()).is_none());
        let mut disabled = command_hook("true");
        disabled.enabled = false;
        assert!(dispatcher(&[disabled], std::env::temp_dir()).is_none());
        // 事件名非法的条目也不算数（sanitize 之外的第二道防线）。
        let mut bogus = command_hook("true");
        bogus.event = "not_an_event".to_string();
        assert!(dispatcher(&[bogus], std::env::temp_dir()).is_none());
    }

    #[test]
    fn parse_round_trips_all_events() {
        for event in [
            HookEvent::AgentStart,
            HookEvent::TurnStart,
            HookEvent::MessageStart,
            HookEvent::MessageEnd,
            HookEvent::ToolExecutionStart,
            HookEvent::ToolExecutionEnd,
            HookEvent::TurnEnd,
            HookEvent::AgentEnd,
        ] {
            assert_eq!(HookEvent::parse(event.as_str()), Some(event));
        }
        assert_eq!(HookEvent::parse("agent_started"), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn script_receives_stdin_json_and_env() {
        let dir = TempDir::new();
        let out = dir.path().join("payload.json");
        let env_out = dir.path().join("env.txt");
        let hook = command_hook(&format!(
            "cat > {out}; printf '%s|%s|%s|%s|%s' \"$KIVIO_HOOK_EVENT\" \"$KIVIO_HOOK_NAME\" \"$KIVIO_TOOL_NAME\" \"$KIVIO_TOOL_CALL_ID\" \"$KIVIO_CONVERSATION_ID\" > {env}",
            out = out.display(),
            env = env_out.display()
        ));
        let (dispatcher, failures) =
            dispatcher(&[hook], dir.path().to_path_buf()).expect("dispatcher");
        dispatcher.dispatch(HookEvent::AgentEnd, Some("write_file"), Some(2), Some("call-1"));
        dispatcher.drain().await;

        let payload: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&out).expect("payload written"))
                .expect("payload is json");
        assert_eq!(payload["event"], "agent_end");
        assert_eq!(payload["hookName"], "probe");
        assert_eq!(payload["conversationId"], "conv-1");
        assert_eq!(payload["runId"], "run-1");
        assert_eq!(payload["messageId"], "msg-1");
        assert_eq!(payload["toolName"], "write_file");
        assert_eq!(payload["toolCallId"], "call-1");
        assert_eq!(payload["round"], 2);
        assert_eq!(payload["model"], "prov:model-x");

        assert_eq!(
            std::fs::read_to_string(&env_out).expect("env written"),
            "agent_end|probe|write_file|call-1|conv-1"
        );
        assert!(
            failures.try_recv().is_err(),
            "successful hook reports nothing"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn non_zero_exit_is_reported_without_breaking_the_run() {
        let dir = TempDir::new();
        let hook = command_hook("echo boom >&2; exit 3");
        let (dispatcher, failures) =
            dispatcher(&[hook], dir.path().to_path_buf()).expect("dispatcher");
        dispatcher.dispatch(HookEvent::AgentEnd, None, None, None);
        dispatcher.drain().await;

        let failure = failures.try_recv().expect("failure reported");
        assert_eq!(failure["event"], "agent_end");
        assert_eq!(failure["hookName"], "probe");
        assert_eq!(failure["conversationId"], "conv-1");
        let message = failure["message"].as_str().unwrap_or_default();
        assert!(message.contains("code 3"), "unexpected message: {message}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancel_drops_queued_hooks_and_kills_the_running_script() {
        let dir = TempDir::new();
        let started = dir.path().join("started");
        let finished = dir.path().join("finished");
        let queued_marker = dir.path().join("queued");

        let mut slow = command_hook(&format!(
            "touch {started}; sleep 30; touch {finished}",
            started = started.display(),
            finished = finished.display()
        ));
        slow.timeout_ms = 60_000;
        let mut queued = command_hook(&format!("touch {}", queued_marker.display()));
        queued.name = "queued".to_string();
        queued.event = "turn_end".to_string();

        let (dispatcher, _failures) =
            dispatcher(&[slow, queued], dir.path().to_path_buf()).expect("dispatcher");
        dispatcher.dispatch(HookEvent::AgentEnd, None, None, None);
        dispatcher.dispatch(HookEvent::TurnEnd, None, None, None);

        // 等第一个脚本真正跑起来，再取消——否则测的是「还没开始」而非「被杀」。
        for _ in 0..200 {
            if started.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(started.exists(), "slow hook never started");
        dispatcher.cancel();
        dispatcher.drain().await;

        assert!(!finished.exists(), "running script survived cancel");
        assert!(!queued_marker.exists(), "queued hook ran after cancel");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_end_dispatched_after_cancel_still_runs() {
        // 取消只作废「此刻之前」入队的 job；Drop guard 之后派发的 agent_end 仍须触发。
        let dir = TempDir::new();
        let marker = dir.path().join("end");
        let hook = command_hook(&format!("touch {}", marker.display()));
        let (dispatcher, _failures) =
            dispatcher(&[hook], dir.path().to_path_buf()).expect("dispatcher");
        dispatcher.cancel();
        dispatcher.dispatch(HookEvent::AgentEnd, None, None, None);
        dispatcher.drain().await;
        assert!(marker.exists(), "agent_end was swallowed by cancel");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn missing_conversation_workdir_falls_back_to_temp() {
        // 会话工作目录是懒创建的：一个从没用过工具的会话，这个路径可能不存在。
        // 直接拿去当 current_dir 会让 spawn 以 ENOENT 失败（脚本根本没跑）。
        let dir = TempDir::new();
        let marker = dir.path().join("ran");
        let hook = command_hook(&format!("touch {}", marker.display()));
        let missing = dir.path().join("conv_never_created");
        let (dispatcher, failures) = dispatcher(&[hook], missing).expect("dispatcher");
        dispatcher.dispatch(HookEvent::AgentEnd, None, None, None);
        dispatcher.drain().await;

        assert!(
            marker.exists(),
            "hook never ran: spawn failed on missing cwd"
        );
        assert!(failures.try_recv().is_err(), "hook should not have failed");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancel_racing_the_spawn_does_not_leak_the_process() {
        // cancel 落在「worker 检查完世代」与「pid 登记完」之间时，那次 cancel 看到的是
        // 空槽位。若 spawn 后不复检世代，这个进程就没人杀 —— 一路活到 run 结束之后。
        let dir = TempDir::new();
        let finished = dir.path().join("finished");
        let mut slow = command_hook(&format!(
            "sleep 30; touch {finished}",
            finished = finished.display()
        ));
        slow.timeout_ms = 60_000;
        let (dispatcher, _failures) =
            dispatcher(&[slow], dir.path().to_path_buf()).expect("dispatcher");
        dispatcher.dispatch(HookEvent::AgentEnd, None, None, None);
        // 不等脚本起来就立刻取消，尽量落进 spawn 与 pid 登记之间的窗口。
        dispatcher.cancel();
        dispatcher.drain().await;
        assert!(
            !finished.exists(),
            "script outlived a cancel racing its spawn"
        );
    }

    /// 生产路径不是 `drain()`（那是单测特权），而是 **Drop**：`ChatAgentHost` 随
    /// `complete_assistant_reply_inner` 返回而落地，带走 dispatcher。这条验证
    /// 「dispatch(agent_end) 后立刻 drop，脚本仍然跑完」——之前所有测试都 drain，
    /// 等于从没测过真实的收尾路径。
    #[cfg(unix)]
    #[tokio::test]
    async fn agent_end_survives_drop_without_drain() {
        let dir = TempDir::new();
        let marker = dir.path().join("dropped");
        let hook = command_hook(&format!("touch {}", marker.display()));
        let (dispatcher, _failures) =
            dispatcher(&[hook], dir.path().to_path_buf()).expect("dispatcher");
        dispatcher.dispatch(HookEvent::AgentEnd, None, None, None);
        drop(dispatcher);
        // 给 detached worker 时间跑完（生产里 app 一直活着，这里模拟那段时间）。
        for _ in 0..100 {
            if marker.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(
            marker.exists(),
            "agent_end was lost when the dispatcher dropped"
        );
    }

    fn http_hook(url: &str, method: &str) -> HookDef {
        HookDef {
            id: "h1".to_string(),
            name: "probe".to_string(),
            event: "agent_end".to_string(),
            enabled: true,
            kind: "http".to_string(),
            url: url.to_string(),
            method: method.to_string(),
            timeout_ms: 10_000,
            ..Default::default()
        }
    }

    /// 本地 HTTP/1.1 一次性应答。`delay` 在读完请求之后、写响应之前等待。
    /// `accepted` 在 accept 之后置位，给 cancel 测试一个「请求已经在飞」的边沿。
    async fn spawn_http_once(
        status_line: &'static str,
        delay: std::time::Duration,
        accepted: Option<std::sync::Arc<AtomicBool>>,
    ) -> String {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind hook http fixture");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            if let Some(flag) = accepted {
                flag.store(true, Ordering::SeqCst);
            }
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await;
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let body = b"ok";
            let resp = format!(
                "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes()).await;
            let _ = stream.write_all(body).await;
        });
        format!("http://{addr}/hook")
    }

    #[tokio::test]
    async fn http_2xx_is_success() {
        let url = spawn_http_once("200 OK", std::time::Duration::ZERO, None).await;
        let (dispatcher, failures) =
            dispatcher(&[http_hook(&url, "POST")], std::env::temp_dir()).expect("dispatcher");
        dispatcher.dispatch(HookEvent::AgentEnd, None, None, None);
        dispatcher.drain().await;
        assert!(
            failures.try_recv().is_err(),
            "2xx http hook should not report failure"
        );
    }

    #[tokio::test]
    async fn http_non_2xx_is_reported() {
        let url = spawn_http_once("500 Internal Server Error", std::time::Duration::ZERO, None)
            .await;
        let (dispatcher, failures) =
            dispatcher(&[http_hook(&url, "POST")], std::env::temp_dir()).expect("dispatcher");
        dispatcher.dispatch(HookEvent::AgentEnd, None, None, None);
        dispatcher.drain().await;
        let failure = failures.try_recv().expect("failure reported");
        let message = failure["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("500"),
            "unexpected message: {message}"
        );
    }

    #[tokio::test]
    async fn http_redirect_is_not_success() {
        let url = spawn_http_once("302 Found", std::time::Duration::ZERO, None).await;
        let (dispatcher, failures) =
            dispatcher(&[http_hook(&url, "POST")], std::env::temp_dir()).expect("dispatcher");
        dispatcher.dispatch(HookEvent::AgentEnd, None, None, None);
        dispatcher.drain().await;
        let failure = failures.try_recv().expect("3xx must fail");
        let message = failure["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("302"),
            "unexpected message: {message}"
        );
    }

    #[tokio::test]
    async fn cancel_aborts_in_flight_http_without_reporting() {
        let accepted = std::sync::Arc::new(AtomicBool::new(false));
        let url = spawn_http_once(
            "200 OK",
            std::time::Duration::from_secs(30),
            Some(Arc::clone(&accepted)),
        )
        .await;
        let mut slow = http_hook(&url, "POST");
        slow.timeout_ms = 60_000;
        let (dispatcher, failures) =
            dispatcher(&[slow], std::env::temp_dir()).expect("dispatcher");
        dispatcher.dispatch(HookEvent::AgentEnd, None, None, None);
        for _ in 0..200 {
            if accepted.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(accepted.load(Ordering::SeqCst), "http hook never started");
        dispatcher.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(3), dispatcher.drain())
            .await
            .expect("cancel must abort the in-flight http hook");
        assert!(
            failures.try_recv().is_err(),
            "cancelled http hook must stay silent"
        );
    }

    #[tokio::test]
    async fn get_http_hook_omits_body() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let saw = std::sync::Arc::new(Mutex::new(String::new()));
        let saw_clone = Arc::clone(&saw);
        tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            if let Ok(mut slot) = saw_clone.lock() {
                *slot = String::from_utf8_lossy(&buf[..n]).into_owned();
            }
            let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
            let _ = stream.write_all(resp).await;
        });
        let url = format!("http://{addr}/hook");
        let (dispatcher, failures) =
            dispatcher(&[http_hook(&url, "GET")], std::env::temp_dir()).expect("dispatcher");
        dispatcher.dispatch(HookEvent::AgentEnd, None, None, None);
        dispatcher.drain().await;
        assert!(failures.try_recv().is_err(), "GET 200 should succeed");
        let request = saw.lock().map(|slot| slot.clone()).unwrap_or_default();
        assert!(
            request.starts_with("GET "),
            "expected GET, got: {request:?}"
        );
        assert!(
            !request.contains("\r\n\r\n{"),
            "GET must not carry a JSON body: {request:?}"
        );
    }
}
