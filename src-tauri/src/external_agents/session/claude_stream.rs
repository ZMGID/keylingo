//! 一个会话一个常驻 `claude` 进程（B1）。
//!
//! 改动前每一轮都 spawn 一个新 `claude` 并用 `--resume` 重新贴上上下文，实测首轮到
//! `system/init` 约 **3.2 秒**；常驻之后第 2+ 轮约 **0.1 秒**。
//!
//! 全部协议事实来自 claude 2.1.220 本机实测（2026-07-29），可运行的探针在
//! `claude_persist_probe_tests.rs`。落地时依赖其中这几条，每条都影响下面的代码形状：
//!
//! - **吐完 `result` 之后继续读 stdin**，同一进程连服多轮、上下文自然延续、`session_id` 恒定。
//! - **进程只在 stdin 关闭时退出**（exit 0，约 0.5s）⇒ 关停路径是「关 stdin 然后 `wait()`」，
//!   **不是 kill**。
//! - **每轮恰好一个 `result`**（被中断的轮次也有）⇒ 它就是轮次边界信号。
//! - **`system/init` 每轮都发**，不是一次性握手；而且**在收到第一条 user 消息之前根本不发**
//!   （本机验证：不写 stdin 只能收到 `hook_started` / `hook_response`，没有 init）。
//!   所以 `connect` **不能**以 init 当握手信号 —— 那会死等。
//! - 首轮还有 `hook_started` / `hook_response` 两帧（用户自己配的 hook），第 2 轮起没有
//!   ⇒ 解析器不能假定固定的开头帧序列。
//! - 两轮之间 stdout 完全干净；35 秒空闲不超时，**不需要心跳**。
//! - stderr 全程零字节，但长活进程**仍必须排空**（管道写满会阻塞子进程，spec 第 4 条）。
//! - 中断走 stdin 的 `control_request` / `interrupt`（init 的 `capabilities` 里有
//!   `interrupt_receipt_v1`），**中断后进程完好、下一轮正常返回** —— 这是常驻的核心收益。

use std::path::Path;
use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::mpsc;
use tokio::time::timeout;
use uuid::Uuid;

use crate::external_agents::attachments::ImageBlock;
use crate::external_agents::session::live::{SessionCommand, CANCELLED_SESSION_LOST};
use crate::external_agents::spawn::{
    cli_command, fold_stderr, kill_agent_process_tree, spawn_stderr_tail, stream_json_user_line,
};
use crate::external_agents::stream::{create_stream_handler, StreamHandler};
use crate::external_agents::types::{StreamFormat, UnifiedAgentEvent};
use crate::proc::NoConsoleWindow;

// ---- 超时 / 上限常量（spec 第 7 条：集中在文件顶部，30s 起步）----

/// 关停时等进程自行退出的上限。实测关 stdin 后约 0.5s 退出（exit 0）；到点才升级到杀整棵树。
const CLAUDE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
/// 单次 stdout 读的轮询步长——用它把 control 通道的 poll 夹进读循环（与 acp / codex 一致）。
const READ_POLL: Duration = Duration::from_millis(200);
/// 连续可恢复读错误的上限：防止一个反复报错的 pipe 把读循环变成忙等。
const MAX_RECOVERABLE_READ_ERRORS: u32 = 32;

/// Windows `ERROR_OPERATION_ABORTED`：中断/取消会让挂起的 pipe 读以这个 errno 返回。
const WINDOWS_ERROR_OPERATION_ABORTED: i32 = 995;

/// 读 stdout 时遇到的这个错误能否**原地恢复**（继续读下一行），而不是当成流结束。
///
/// 中断会让底层挂起的 pipe 读抛出瞬时错误（unix 的 `EINTR`、Windows 的
/// `ERROR_OPERATION_ABORTED`/995）。把它们当成「流结束」会让一个**完好的**常驻进程被判定为
/// 死亡，进而触发重连并丢掉整个会话上下文 —— 而这恰好发生在用户点「停止」的那一刻，
/// 也就是最不该丢上下文的时候。真正的致命错误（BrokenPipe / UnexpectedEof / …）才结束本轮。
fn read_error_is_recoverable(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ) || err.raw_os_error() == Some(WINDOWS_ERROR_OPERATION_ABORTED)
}

/// 取消之后，本轮剩余帧里的 `Error` 事件一律吞掉。
///
/// 中断会在流里留下一串「本轮失败」的回声：assistant 帧的 `aborted`、`result` 的
/// `errors: ["[ede_diagnostic] …"]`。用户已经看到「已取消」，再叠一个红色错误气泡是纯噪音，
/// 而且 `run.rs` 的出口会因为 `stream_error` 非空把 `stream_outcome` 从 cancelled 翻成 error。
///
/// 只吞 `Error`：正文与「本轮回答被中止」这类提示仍要发出去（已经流出来的半截回答有效）。
fn suppress_after_cancel(cancelled: bool, event: &UnifiedAgentEvent) -> bool {
    cancelled && matches!(event, UnifiedAgentEvent::Error { .. })
}

/// 一行 `interrupt` 控制请求（含结尾换行）。
///
/// 实测回 `{"type":"control_response","response":{"subtype":"success","request_id":"<同一个>",
/// "response":{"still_queued":[]}}}`，随后该轮仍吐一条 `result`
/// （`terminal_reason:"aborted_streaming"`），进程完好。
fn interrupt_request_line(request_id: &str) -> String {
    format!(
        "{}\n",
        json!({
            "type": "control_request",
            "request_id": request_id,
            "request": { "subtype": "interrupt" },
        })
    )
}

/// 一次 stdout 读的结果。
enum ReadStep {
    Line(String),
    /// 本次轮询没有新行（正常空转），回去 poll control 通道。
    Idle,
    /// stdout EOF —— 进程结束了自己的输出。
    Eof,
    Fatal(String),
}

/// 一个活着的 claude stream-json 会话：一个进程服完整个对话。由它自己的 actor 任务独占。
pub struct ClaudeStreamJsonSession {
    child: Child,
    stdin: ChildStdin,
    reader: Lines<BufReader<ChildStdout>>,
    /// **跨轮存活**的解析器。绝不能每轮新建（spec 第 3 条 / 第 14h 条）：
    /// - `completed_result_turns` 是 per-session 计数，给顶层 usage 回退开闸门 —— 每轮新建
    ///   会让它恒为 0，闸门永久失效，第 2 轮起把「本轮计费总量」当成上下文快照；
    /// - `resolved_model` 要跨轮记住最近一次 `system/init` 报的模型，`modelUsage` 才能精确
    ///   定位当前模型的 `contextWindow`（分母）；
    /// - per-turn 的 `any_text_emitted` / `reported_assistant_errors` 由解析器自己在 `result`
    ///   帧复位，不需要外部干预。
    handler: StreamHandler,
    /// claude 原生 session id：启动参数里的 `--session-id` / `--resume` 值，第一轮被
    /// `system/init` / `result` 实报的 `session_id` 覆盖（实测两者一致，覆盖只是求稳）。
    session_id: String,
    /// stderr 环形尾部（8KB）。**必须**是 `spawn_stderr_tail` 而不是 `drain_stderr`：
    /// 后者读到 EOF 才返回，长活进程下 `await` 会永久挂死（spec 第 4 条）。
    /// 出错路径 `take()` 走它取尾部折进诊断，`close()` 收尾时 join。
    stderr_tail: Option<tokio::task::JoinHandle<String>>,
}

impl ClaudeStreamJsonSession {
    /// 拉起常驻进程。
    ///
    /// **握手 = 只 spawn**，不读任何帧：claude 在收到第一条 user 消息之前不发 `system/init`
    /// （本机验证，见模块头），以 init 当握手信号会死等。启动即失败（参数非法 /
    /// `--resume` 的 id 不存在）由这里的即时 `try_wait` 抓住；其余失败（未登录之类）
    /// 是流里的一条 `result`，走 `run_turn` → `errors::classify` 那条正常出口（spec 第 5 条）。
    ///
    /// 会话 id 不单独传参：claude 的会话 flag 由 `build_claude_args` 放进 `args`
    /// （`--session-id` 首次 / `--resume` 重连），不像 codex/ACP 在握手 RPC 里传。
    pub async fn connect(
        resolved_bin: &Path,
        args: &[String],
        cwd: &Path,
    ) -> Result<Self, String> {
        // spec 第 16 条：必须走 `cli_command` 剥掉父会话身份/宿主代管凭据标记，
        // 否则 Kivio 从某个 CLI 会话里启动时子进程会拒绝启动或报「未登录」。
        let mut child = cli_command(resolved_bin)
            .args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .no_console_window()
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("spawn: {e}"))?;
        let stderr_tail = spawn_stderr_tail(child.stderr.take());

        let mut take_pipes = || -> Result<(ChildStdin, ChildStdout), String> {
            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| "spawn: stdin unavailable".to_string())?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| "spawn: stdout unavailable".to_string())?;
            Ok((stdin, stdout))
        };
        let pipes = take_pipes();

        // 参数非法 / `--resume` 的 id 不存在这类失败会**立刻**退出（实测几十毫秒）。
        // 这里只查一次、不等待：连接阶段抓住它比伪装成一轮空回复好得多。
        let already_exited = match child.try_wait() {
            Ok(Some(status)) => Some(format!("claude-init: 进程启动后立刻退出（{status}）")),
            _ => None,
        };

        match (pipes, already_exited) {
            (Ok((stdin, stdout)), None) => Ok(Self {
                child,
                stdin,
                reader: BufReader::new(stdout).lines(),
                handler: create_stream_handler(StreamFormat::ClaudeStreamJson),
                session_id: crate::external_agents::defs::claude::claude_session_id_from_args(args)
                    .unwrap_or_default(),
                stderr_tail: Some(stderr_tail),
            }),
            (pipes, exited) => {
                let msg = exited
                    .or_else(|| pipes.err())
                    .unwrap_or_else(|| "claude-init: 会话建立失败".to_string());
                let tail =
                    crate::external_agents::spawn::join_stderr_tail(&mut child, stderr_tail).await;
                Err(fold_stderr(msg, &tail))
            }
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 跑一轮：往 stdin 写一行 stream-json user 消息 → 读 stdout 直到本轮的 `result` → 返回。
    /// **不关 stdin**（关了进程就退出了）。
    ///
    /// `SessionCommand::RunTurn` 携带的 model / reasoning 在这里**有意忽略**：claude 的模型与
    /// effort 是**启动 flag**，会话内无法切换。中途换配置由注册表的 `LaunchConfig` 指纹拦下
    /// 并重连一个新进程（见 `session/live.rs::LaunchConfig`），而不是在这里假装切换成功。
    pub async fn run_turn(
        &mut self,
        prompt: &str,
        images: &[ImageBlock],
        events: &mpsc::Sender<UnifiedAgentEvent>,
        control: &mut mpsc::Receiver<SessionCommand>,
    ) -> Result<(), String> {
        let payload = stream_json_user_line(prompt, images)?;
        if let Err(err) = self.stdin.write_all(payload.as_bytes()).await {
            return Err(self.fold_tail(format!("写入 claude stdin 失败: {err}")).await);
        }
        if let Err(err) = self.stdin.flush().await {
            return Err(self.fold_tail(format!("刷新 claude stdin 失败: {err}")).await);
        }

        let mut cancelled = false;
        let mut recoverable_errors = 0u32;
        loop {
            match control.try_recv() {
                Ok(SessionCommand::Cancel) => {
                    if !cancelled {
                        cancelled = true;
                        // 协议级中断，**不 kill**：进程要留给下一轮（常驻的核心收益）。
                        // 写失败也不立刻放弃 —— 继续读，`result` 可能已经在路上。
                        let line = interrupt_request_line(&format!("kivio-interrupt-{}", Uuid::new_v4()));
                        let _ = self.stdin.write_all(line.as_bytes()).await;
                        let _ = self.stdin.flush().await;
                    }
                }
                Ok(SessionCommand::Close) => return Err("closed".to_string()),
                Ok(SessionCommand::RunTurn { done, .. }) => {
                    let _ = done.send(Err("session busy".to_string()));
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    return Err("control channel closed".to_string())
                }
            }

            let line = match self.next_line().await {
                ReadStep::Line(line) => {
                    recoverable_errors = 0;
                    line
                }
                ReadStep::Idle => continue,
                ReadStep::Eof => {
                    // 进程在轮次中间没了。取消途中遇到这个：按「已取消但会话作废」上报，
                    // 否则重试逻辑会把用户刚刚停掉的这一轮原样重发一遍。
                    return Err(if cancelled {
                        CANCELLED_SESSION_LOST.to_string()
                    } else {
                        self.fold_tail("claude 常驻会话在轮次中退出".to_string())
                            .await
                    });
                }
                ReadStep::Fatal(err) => {
                    recoverable_errors += 1;
                    if recoverable_errors >= MAX_RECOVERABLE_READ_ERRORS {
                        return Err(self.fold_tail(err).await);
                    }
                    continue;
                }
            };
            if line.trim().is_empty() {
                continue;
            }

            // 轮次边界靠解析器的 `result` 计数判定：喂一行前后各取一次。这样既复用了唯一
            // 那份解析逻辑（spec 第 2 条），又不用为了找边界把同一行 JSON 再解析一遍。
            let before = self.handler.completed_result_turns();
            let mut buf: Vec<UnifiedAgentEvent> = Vec::new();
            self.handler.handle_line(&line, &mut |event| buf.push(event));
            for event in buf {
                if suppress_after_cancel(cancelled, &event) {
                    continue;
                }
                let _ = events.send(event).await;
            }

            if self.handler.completed_result_turns() > before {
                // 被中断的轮次同样有 `result`（形态见 `stream::claude::result_is_user_abort`）：
                // 走「已取消」出口而不是错误出口，否则每点一次停止都弹一个假错误气泡。
                if cancelled || self.handler.last_result_aborted() {
                    return Err("cancelled".to_string());
                }
                return Ok(());
            }
        }
    }

    /// 读一行 stdout，把「瞬时可恢复」与「真的结束了」分开（见 `read_error_is_recoverable`）。
    async fn next_line(&mut self) -> ReadStep {
        match timeout(READ_POLL, self.reader.next_line()).await {
            Ok(Ok(Some(line))) => ReadStep::Line(line),
            Ok(Ok(None)) => ReadStep::Eof,
            Ok(Err(err)) => {
                if read_error_is_recoverable(&err) {
                    ReadStep::Fatal(format!("读取 claude stdout 失败: {err}"))
                } else {
                    // 不可恢复：当成流结束，让上层按「进程没了」处理（重连 / 报错）。
                    ReadStep::Eof
                }
            }
            Err(_) => ReadStep::Idle,
        }
    }

    /// 把 stderr 环形尾部折进错误文案（spec 第 5 条的 `<details>` 素材）。
    async fn fold_tail(&mut self, msg: String) -> String {
        let Some(handle) = self.stderr_tail.take() else {
            return msg;
        };
        // 子进程死了 ⇒ stderr 也 EOF ⇒ 这个 await 立刻返回；还活着时给个短上限，
        // 绝不让诊断代码把出口挂住。
        let tail = match timeout(Duration::from_secs(2), handle).await {
            Ok(Ok(tail)) => tail,
            _ => String::new(),
        };
        fold_stderr(msg, &tail)
    }

    /// 关停 = **关 stdin 然后 `wait()`**，不是 kill。
    ///
    /// 实测 claude 只在 stdin 关闭时退出（exit 0，约 0.5s），正常退出时它会自己收尾
    /// （落盘会话、关掉自己拉起的 MCP 子进程）。到点还赖着不走才升级到
    /// `kill_agent_process_tree`（spec 第 8c 条：杀整棵树，不然漏一批孤儿 MCP 进程）。
    ///
    /// **`shutdown()` 之后必须 `drop(stdin)`**：tokio 的 `ChildStdin::poll_shutdown` 只 flush，
    /// **不关句柄** —— 句柄要到 drop 才关。少了这一行，子进程永远收不到 EOF，每次关停都要
    /// 白等满 `CLAUDE_SHUTDOWN_TIMEOUT` 再被杀掉（真机测试的耗时从 53s 涨到 169s 才暴露）。
    /// acp / codex 的 `close()` 是 `shutdown()` 紧跟 `start_kill()`，掩盖了同一个问题。
    pub async fn close(self) {
        let Self {
            mut child,
            mut stdin,
            stderr_tail,
            ..
        } = self;
        let _ = stdin.shutdown().await;
        drop(stdin);
        if timeout(CLAUDE_SHUTDOWN_TIMEOUT, child.wait()).await.is_err() {
            kill_agent_process_tree(&mut child);
            let _ = child.wait().await;
        }
        if let Some(handle) = stderr_tail {
            let _ = handle.await;
        }
    }
}

/// Spawn the actor task that owns a connected session and serves `SessionCommand`s.
pub fn spawn_claude_stream_session_actor(
    mut session: ClaudeStreamJsonSession,
) -> mpsc::Sender<SessionCommand> {
    let (tx, mut rx) = mpsc::channel::<SessionCommand>(8);
    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                SessionCommand::RunTurn {
                    prompt,
                    images,
                    events,
                    done,
                    ..
                } => {
                    // Invariant (A4)：`run_turn` 在返回前发完所有 `event`，mpsc 保序，
                    // 所以调用方在 `done` 之后的 drain 能看到全部事件。`done.send` 永远最后。
                    let result = session.run_turn(&prompt, &images, &events, &mut rx).await;
                    let _ = done.send(result);
                }
                SessionCommand::Cancel => {} // 轮次之间没有在跑的轮次
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
    use serde_json::Value;

    // ---- 取消后的两道防御（纯函数 + 单测，spec 第 13 条）----

    /// 迟到的 `Error` 回声必须吞掉：用户已经看到「已取消」，再叠一个红气泡是纯噪音，
    /// 而且会把 `stream_outcome` 从 cancelled 翻成 error。
    #[test]
    fn error_echoes_are_suppressed_after_a_cancel() {
        let err = UnifiedAgentEvent::Error {
            message: "[ede_diagnostic] aborted".to_string(),
        };
        assert!(suppress_after_cancel(true, &err));
        // 没取消时照常上报——这条判据不能顺手把真实失败也吞掉。
        assert!(!suppress_after_cancel(false, &err));
    }

    /// 只吞 `Error`：已经流出来的正文、以及「本轮回答被中止」这类提示仍要发出去。
    #[test]
    fn cancel_suppression_only_touches_the_error_channel() {
        for event in [
            UnifiedAgentEvent::TextDelta {
                delta: "half an answer".to_string(),
            },
            UnifiedAgentEvent::ThinkingDelta {
                delta: "…".to_string(),
            },
            UnifiedAgentEvent::Usage {
                usage: crate::chat::model::ModelUsage::default(),
            },
        ] {
            assert!(
                !suppress_after_cancel(true, &event),
                "取消后不该吞掉 {event:?}"
            );
        }
    }

    /// abort 类读错误要能原地恢复：把它们当成「流结束」会让一个完好的常驻进程被判定为死亡，
    /// 进而在用户点「停止」的那一刻丢掉整个会话上下文。
    #[test]
    fn abort_style_read_errors_are_recoverable() {
        use std::io::{Error, ErrorKind};
        assert!(read_error_is_recoverable(&Error::from(ErrorKind::Interrupted)));
        assert!(read_error_is_recoverable(&Error::from(ErrorKind::WouldBlock)));
        assert!(read_error_is_recoverable(&Error::from(ErrorKind::TimedOut)));
        // Windows 的 ERROR_OPERATION_ABORTED（995）——中断挂起的 pipe 读就是这个。
        assert!(read_error_is_recoverable(&Error::from_raw_os_error(
            WINDOWS_ERROR_OPERATION_ABORTED
        )));
    }

    /// 真正的致命错误不得被当成「再试一次」，否则读循环会在一个死掉的 pipe 上空转。
    #[test]
    fn fatal_read_errors_are_not_recoverable() {
        use std::io::{Error, ErrorKind};
        for kind in [
            ErrorKind::BrokenPipe,
            ErrorKind::UnexpectedEof,
            ErrorKind::PermissionDenied,
            ErrorKind::InvalidData,
        ] {
            assert!(
                !read_error_is_recoverable(&Error::from(kind)),
                "{kind:?} 不该判为可恢复"
            );
        }
    }

    /// 中断请求的线上形态必须与实测样本一致（多一个 `request_id` 都会拿不到
    /// `control_response`，于是取消变成静默无效）。
    #[test]
    fn interrupt_request_matches_the_measured_wire_shape() {
        let line = interrupt_request_line("req-1");
        assert!(line.ends_with('\n'), "必须是一整行");
        let value: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(value["type"], serde_json::json!("control_request"));
        assert_eq!(value["request_id"], serde_json::json!("req-1"));
        assert_eq!(value["request"]["subtype"], serde_json::json!("interrupt"));
    }

    #[test]
    fn interrupt_request_ids_are_unique() {
        let a = interrupt_request_line(&format!("kivio-interrupt-{}", Uuid::new_v4()));
        let b = interrupt_request_line(&format!("kivio-interrupt-{}", Uuid::new_v4()));
        assert_ne!(a, b);
    }
}

/// 真机验收（spec 第 15 条）。全部 `#[ignore]`；认证失效 / 网络问题一律**诚实 skip 并打印
/// 排查提示**，不 fail —— 一个过期的 key 不该伪装成代码回归。
///
/// ```powershell
/// pwsh scripts/win-cargo-test.ps1 --lib claude_stream
/// cd src-tauri; ./target/debug/deps/kivio-*.exe claude_stream --ignored --nocapture --test-threads=1
/// ```
#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::external_agents::defs::claude::{build_claude_args, CLAUDE_AGENT_DEF};
    use crate::external_agents::spawn::resolve_binary;
    use crate::external_agents::types::{RuntimeBuildOptions, RuntimeContext};
    use tokio::sync::oneshot;

    /// 生产 argv，直接取自出货用的 builder —— 测一条简化命令行等于没测。
    fn live_args(session_id: &str, model: Option<&str>) -> Vec<String> {
        build_claude_args(
            &RuntimeContext {
                extra_allowed_dirs: vec![],
                resume_session_id: None,
                new_session_id: Some(session_id.to_string()),
                include_partial_messages: true,
            },
            &RuntimeBuildOptions {
                model: model.map(str::to_string),
                reasoning: None,
                sandbox: None,
            },
            None,
        )
    }

    struct TurnOutput {
        text: String,
        result: Result<(), String>,
    }

    /// 跑一轮，收集正文与终态。`cancel_after_text` 为真时，一见到正文就发 `Cancel`。
    async fn one_turn(
        control: &mpsc::Sender<SessionCommand>,
        prompt: &str,
        cancel_after_text: bool,
    ) -> TurnOutput {
        let (etx, mut erx) = mpsc::channel::<UnifiedAgentEvent>(256);
        let (dtx, drx) = oneshot::channel();
        control
            .send(SessionCommand::RunTurn {
                prompt: prompt.to_string(),
                model: None,
                reasoning: None,
                images: vec![],
                events: etx,
                done: dtx,
            })
            .await
            .expect("actor alive");

        let mut text = String::new();
        let mut cancel_sent = false;
        let mut drx = drx;
        let result = loop {
            tokio::select! {
                biased;
                done = &mut drx => {
                    while let Ok(event) = erx.try_recv() {
                        if let UnifiedAgentEvent::TextDelta { delta } = event {
                            text.push_str(&delta);
                        }
                    }
                    break done.unwrap_or_else(|_| Err("actor dropped".to_string()));
                }
                event = erx.recv() => {
                    if let Some(UnifiedAgentEvent::TextDelta { delta }) = event {
                        text.push_str(&delta);
                    }
                    if cancel_after_text && !cancel_sent && text.chars().count() > 20 {
                        cancel_sent = true;
                        let _ = control.send(SessionCommand::Cancel).await;
                    }
                }
            }
        };
        TurnOutput { text, result }
    }

    async fn connect_live(
        session_id: &str,
        model: Option<&str>,
    ) -> Option<(mpsc::Sender<SessionCommand>, std::path::PathBuf)> {
        let Some(bin) = resolve_binary(&CLAUDE_AGENT_DEF).await else {
            eprintln!("SKIP: 本机没有可用的 claude CLI");
            return None;
        };
        let workdir = std::env::temp_dir().join(format!("kivio-claude-live-{session_id}"));
        std::fs::create_dir_all(&workdir).expect("create workdir");
        match ClaudeStreamJsonSession::connect(&bin, &live_args(session_id, model), &workdir).await {
            Ok(session) => Some((spawn_claude_stream_session_actor(session), workdir)),
            Err(err) => {
                eprintln!("SKIP: 连接失败（未登录 / 网络？）：{err}");
                eprintln!("      排查：claude -p \"hi\" --output-format stream-json --verbose");
                let _ = std::fs::remove_dir_all(&workdir);
                None
            }
        }
    }

    /// 关停并**等 actor 真正结束**（进程退出、cwd 释放）再删测试目录。
    /// 不等的话 Windows 会因为子进程还占着 cwd 拒绝删除，在用户 temp 里留一堆残渣。
    async fn close_and_cleanup(control: mpsc::Sender<SessionCommand>, workdir: &std::path::Path) {
        let _ = control.send(SessionCommand::Close).await;
        let _ = timeout(CLAUDE_SHUTDOWN_TIMEOUT, control.closed()).await;
        let _ = std::fs::remove_dir_all(workdir);
    }

    /// **核心验收 1**：同一个常驻会话连服三轮，第 2 / 3 轮记得前面轮次的内容。
    /// 断言可证伪的量（回答里含只可能来自上一轮的数字），而不是「没崩」。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real-machine: spawns the installed claude CLI, needs login, costs tokens"]
    async fn live_one_persistent_session_serves_three_turns_with_continuity() {
        let session_id = Uuid::new_v4().to_string();
        let Some((control, workdir)) = connect_live(&session_id, None).await else {
            return;
        };

        let first = one_turn(
            &control,
            "Answer with one word only. Remember the number 42.",
            false,
        )
        .await;
        if first.result.is_err() {
            eprintln!("SKIP: 第一轮就失败（未登录 / 网络？）：{:?}", first.result);
            close_and_cleanup(control, &workdir).await;
            return;
        }
        eprintln!("turn1: {}", first.text.trim());

        let second = one_turn(
            &control,
            "What number did I just ask you to remember? Reply with just the number.",
            false,
        )
        .await;
        eprintln!("turn2: {}", second.text.trim());
        assert!(second.result.is_ok(), "第 2 轮失败：{:?}", second.result);
        assert!(
            second.text.contains("42"),
            "第 2 轮没记住第 1 轮的内容 ⇒ 不是同一个活会话（回答：{:?}）",
            second.text
        );

        let third = one_turn(
            &control,
            "Add 1 to that number and reply with just the result.",
            false,
        )
        .await;
        eprintln!("turn3: {}", third.text.trim());
        assert!(third.result.is_ok(), "第 3 轮失败：{:?}", third.result);
        assert!(
            third.text.contains("43"),
            "第 3 轮回答是 {:?}，期望 43",
            third.text
        );

        close_and_cleanup(control, &workdir).await;
    }

    /// **核心验收 2（整个改造的验收点）**：取消一轮之后，**同一个会话**下一轮仍然正常返回，
    /// 而且还记得取消之前那一轮建立的上下文。取消一次就废掉会话 ⇒ 常驻白做。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real-machine: spawns the installed claude CLI, needs login, costs tokens"]
    async fn live_session_still_serves_the_next_turn_after_a_cancel() {
        let session_id = Uuid::new_v4().to_string();
        let Some((control, workdir)) = connect_live(&session_id, None).await else {
            return;
        };

        let warmup = one_turn(
            &control,
            "Answer with one word only. Remember the number 77.",
            false,
        )
        .await;
        if warmup.result.is_err() {
            eprintln!("SKIP: 热身轮失败（未登录 / 网络？）：{:?}", warmup.result);
            close_and_cleanup(control, &workdir).await;
            return;
        }

        // 一轮长回答，见到正文就打断。
        let aborted = one_turn(
            &control,
            "Write a long, detailed 800-word essay about the history of clouds in art. \
             Start immediately.",
            true,
        )
        .await;
        eprintln!("cancelled turn -> {:?}", aborted.result);
        assert_eq!(
            aborted.result.as_ref().err().map(String::as_str),
            Some("cancelled"),
            "被中断的轮次必须走「已取消」出口，而不是失败出口（否则每次点停止都弹假错误气泡）"
        );

        // **验收点**：同一个会话继续服务，且上下文没丢。
        let after = one_turn(
            &control,
            "What number did I ask you to remember earlier? Reply with just the number.",
            false,
        )
        .await;
        eprintln!("post-cancel turn: {}", after.text.trim());
        assert!(
            after.result.is_ok(),
            "取消之后同一个会话不能再用了 ⇒ 常驻改造的核心收益没了：{:?}",
            after.result
        );
        assert!(
            after.text.contains("77"),
            "取消把会话上下文也一起丢了（回答：{:?}）",
            after.text
        );

        close_and_cleanup(control, &workdir).await;
    }

    /// **核心验收 3**：配置变更触发重连（`claude_args_resuming` 把 `--session-id` 改成
    /// `--resume`）后仍能正常回复，并且**续上了**原来的会话。
    ///
    /// 这条是「重连参数改写」的唯一真机保险：仍带 `--session-id` 时 claude 会以「id 已存在」
    /// 拒绝启动，而单测只能证明字符串拼对了。argv 里顺带换了 `--model` —— 这证明 claude 接受
    /// 「`--resume` + 不同的 `--model`」而不报错；注意**真实的换模型走的不是这条路**：
    /// `resolve_agent_resume_context` 会刻意开一个新会话（claude 的 resume 会话钉死在旧模型上），
    /// 这条路覆盖的是 sandbox / effort / 系统指令变更那三种重连。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real-machine: spawns the installed claude CLI, needs login, costs tokens"]
    async fn live_reconnect_with_resume_keeps_answering_and_keeps_context() {
        use crate::external_agents::defs::claude::claude_args_resuming;

        let Some(bin) = resolve_binary(&CLAUDE_AGENT_DEF).await else {
            eprintln!("SKIP: 本机没有可用的 claude CLI");
            return;
        };
        let session_id = Uuid::new_v4().to_string();
        let workdir = std::env::temp_dir().join(format!("kivio-claude-reconnect-{session_id}"));
        std::fs::create_dir_all(&workdir).expect("create workdir");

        // 第一个进程：sonnet，建立会话并记一个数字。
        let first_args = live_args(&session_id, Some("sonnet"));
        let Ok(session) = ClaudeStreamJsonSession::connect(&bin, &first_args, &workdir).await else {
            eprintln!("SKIP: 首次连接失败（未登录 / 网络？）");
            let _ = std::fs::remove_dir_all(&workdir);
            return;
        };
        let native_id = session.session_id().to_string();
        assert_eq!(native_id, session_id, "session id 应来自启动参数");
        let control = spawn_claude_stream_session_actor(session);
        let warmup = one_turn(
            &control,
            "Answer with one word only. Remember the number 55.",
            false,
        )
        .await;
        if warmup.result.is_err() {
            eprintln!("SKIP: 热身轮失败：{:?}", warmup.result);
            close_and_cleanup(control, &workdir).await;
            return;
        }
        // 等第一个进程真的退出再拉第二个：同一个 session id 上不能有两个活进程。
        let _ = control.send(SessionCommand::Close).await;
        let _ = timeout(CLAUDE_SHUTDOWN_TIMEOUT, control.closed()).await;

        // 换模型 ⇒ 指纹变了 ⇒ 重连：新 flag（opus）+ `--resume <同一个 id>`。
        let reconnect_args =
            claude_args_resuming(&live_args(&session_id, Some("opus")), &native_id);
        assert!(!reconnect_args.contains(&"--session-id".to_string()));
        let session = match ClaudeStreamJsonSession::connect(&bin, &reconnect_args, &workdir).await
        {
            Ok(session) => session,
            Err(err) => panic!("带 --resume 的重连启动失败：{err}"),
        };
        let control = spawn_claude_stream_session_actor(session);
        let after = one_turn(
            &control,
            "What number did I ask you to remember? Reply with just the number.",
            false,
        )
        .await;
        eprintln!("post-reconnect: {}", after.text.trim());
        assert!(
            after.result.is_ok(),
            "重连后无法回复：{:?}",
            after.result
        );
        assert!(
            after.text.contains("55"),
            "重连（--resume）没续上原会话（回答：{:?}）",
            after.text
        );

        close_and_cleanup(control, &workdir).await;
    }
}
