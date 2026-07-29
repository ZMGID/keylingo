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

use serde_json::{json, Value};
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

/// 取消之后，最多把多少帧当成「上一轮的残帧」丢掉（见 `stale_frames_left`）。
///
/// 窗口的**正常**关闭条件是下一轮的 `system/init`（实测每轮都发、且在本轮任何正文之前），
/// 这个数字只是「init 万一不来」时的兜底闸门 —— 少了它，一次取消就可能把后面所有输出
/// 永久吞掉，那比原来的 bug 更糟。实测一轮取消后的残帧最多也就 assistant + result 两三条，
/// 64 有两个数量级的余量。
const STALE_FRAME_BUDGET: u32 = 64;

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

/// stdout 上这一帧该怎么处理。
///
/// 常驻之后 stdout 上不只有「本轮的内容」：控制通道（`control_request` /
/// `control_response` / `control_cancel_request`）和心跳（`keep_alive`）与内容帧混在同一条流里。
/// 白名单分流，剩下的交给流解析器 —— 但**绝不能对一条在等回复的 `control_request` 保持沉默**。
enum InboundFrame {
    /// 交给流解析器（正文 / 工具 / usage / 轮次边界）。
    Stream,
    /// 控制通道的帧，**有意不接**且不需要回复。
    Ignore,
    /// claude 在等我们回复：把这一整行写回 stdin。
    Reply(String),
}

/// claude 在 stream-json 下能**向我们**发出的、需要回复的 `control_request` 子型。
///
/// **协议事实**（claude 2.1.220，`grep -a` 读本机二进制里的 zod schema 与 `sendRequest` 构造处）：
/// CLI→客户端的 `control_request.request` 是一个 12 成员 union
/// （`can_use_tool` / `request_user_dialog` / `elicitation` / `set_cwd` / `message_rated` /
/// `oauth_token_refresh` / `host_auth_token_refresh` / `stop_task` / `background_tasks` /
/// `apply_flag_settings` / `get_settings` / `submit_feedback`），全都是「问我们、等我们答」。
///
/// 我们一种都没实现，所以这里**不列白名单**：任何 `control_request` 一律回一条 error。
/// 下一个任务实现权限问答时，在 `classify_inbound_frame` 里给 `can_use_tool` 加一条分支即可
/// （请求侧字段形状是 snake_case：`tool_name` / `input` / `tool_use_id` / `permission_suggestions`
/// / `blocked_path` / `decision_reason`；而**成功响应**的载荷是 camelCase 的
/// `{behavior, updatedInput?, updatedPermissions?, message?}` —— 这条协议两套命名混用，
/// 已从二进制核实，别照一侧推另一侧），其余仍走这条 fail-closed 的兜底。
const UNSUPPORTED_CONTROL_REQUEST: &str = "Kivio 尚不支持这个控制请求";

/// 分流一帧 stdout JSON。
fn classify_inbound_frame(value: &Value) -> InboundFrame {
    let Some(obj) = value.as_object() else {
        return InboundFrame::Stream;
    };
    match obj.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        // **必须回复**：claude 发出 `control_request` 之后就挂在那儿等 `control_response`
        // （它自己那侧是 `pendingRequests` 里的一个 Promise，**没有超时**）。我们不回 ⇒ 那个
        // 工具调用永远不返回 ⇒ 本轮的 `result` 永远不来 ⇒ 轮次读循环永久挂死。
        //
        // **claude 2.1.220 在我们这套 argv 下实测不会发**（两条独立证据，别把这条当成
        // 「修了一个正在发生的挂死」）：
        //   1. 真机：`--permission-mode default` + 让它写文件，权限被**直接拒**
        //      （result 里是「The write needs your permission to proceed」），stdout 上
        //      一条 `control_request` 都没有；
        //   2. 二进制：`can_use_tool` / `request_user_dialog` 的发送端（`qHS` / `zHS`）
        //      只挂在 **REPL / remote bridge** 那条传输上（`replBridgePermissionCallbacks`），
        //      纯 stdio 的 `-p` 走的是 `--permission-prompt-tool` 或直接拒。
        //
        // 那为什么还要写这一手：这是**保障**而不是绕过。整套机制在这个二进制里是完整的
        // （`can_use_tool` 出现 35 次、`control_cancel_request` 33 次），schema 里 CLI→客户端
        // 的 `control_request` 是个 12 成员 union；哪天它开始走 stdio、或用户装了别的版本 /
        // 换了权限档位，沉默的代价是**那一轮永久挂死**（读循环没有超时）。回一条 error 的
        // 代价只是「这一次工具用不了」。
        "control_request" => match request_id_of(obj) {
            Some(request_id) => {
                let subtype = obj
                    .get("request")
                    .and_then(|r| r.get("subtype"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                InboundFrame::Reply(control_error_response_line(&request_id, &subtype))
            }
            // 没有 `request_id` 就无从回复（CLI 自己的校验也会把这种帧判成
            // `Error: Missing request on control_request`）。只能丢掉。
            None => InboundFrame::Ignore,
        },
        // 我们**发出**的 `interrupt` 的应答。**有意不接**：取消的权威信号是那一轮的 `result`
        // （实测被中断的轮次一定有一条，`terminal_reason:"aborted_streaming"`），
        // 而这条 ack 可能在我们已经返回之后才到。读它没有任何决策价值。
        "control_response" => InboundFrame::Ignore,
        // claude 撤回它先前发给我们的某个 `control_request`（例如权限询问已在别处被答掉）。
        // **有意不接**：实测 CLI 自己收到这条时也只是 abort 掉在飞的请求、**不回任何东西**
        // （二进制里 `case "control_cancel_request"` 分支只有 `Hn?.abort(...)`），所以静默忽略
        // 是对的。等实现了权限问答，这里要改成「取消那条待答的询问」。
        "control_cancel_request" => InboundFrame::Ignore,
        // 心跳。schema 是 `{type:"keep_alive"}`（无字段、无 request_id），CLI 自己的两个读取点
        // 都是直接 `continue` / `return` —— **没有任何需要回应的语义**，静默忽略即正确处理。
        "keep_alive" => InboundFrame::Ignore,
        _ => InboundFrame::Stream,
    }
}

fn request_id_of(obj: &serde_json::Map<String, Value>) -> Option<String> {
    obj.get("request_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 一条 fail-closed 的 `control_response`（含结尾换行）。
///
/// **协议形状**（claude 2.1.220，从本机二进制核实，两处互相印证）：
/// 1. CLI 自己构造错误响应的地方 ——
///    `{type:"control_response",response:{subtype:"error",request_id:<回显>,error:<字符串>}}`；
/// 2. zod schema —— `control_response.response` 是 success | error 的 union，
///    error 分支是 `{subtype:"error", request_id: string, error: string}`。
///
/// 两个容易踩的点：
/// - **`request_id` 嵌在 `response` 里面**，不是帧顶层（顶层只有 `type`；远程 bridge 会再挂一个
///   `session_id`，stdio 这条路不需要）。放错层级 = CLI 匹配不到，等于没回；
/// - 这条协议**两套命名混用**：请求侧与这三个字段是 snake_case，而 `can_use_tool` 的**成功**
///   载荷是 camelCase（`behavior` / `updatedInput` / `updatedPermissions`）。错误分支只用
///   snake_case 三件套，别顺手写成 camelCase。
///
/// 真机核实（2026-07-29）：往 stdin 写一条这样的响应（`request_id` 是 CLI 从没问过的），
/// CLI 照常处理紧随其后的 user 消息（init → assistant → result），stderr 零字节、流不受污染。
/// 而对一条**真实**的询问回 error 时，CLI 那侧是 `pendingRequests` 的 promise 被 reject
/// （二进制：`if(t.response.subtype==="error"){o.reject(Error(t.response.error))}`）
/// ⇒ 那次工具调用按失败收场、本轮照常收尾 —— 正是 fail-closed 想要的：宁可这一次工具用不了，
/// 也不要整轮挂死。
fn control_error_response_line(request_id: &str, subtype: &str) -> String {
    let detail = if subtype.is_empty() {
        UNSUPPORTED_CONTROL_REQUEST.to_string()
    } else {
        format!("{UNSUPPORTED_CONTROL_REQUEST}: {subtype}")
    };
    format!(
        "{}\n",
        json!({
            "type": "control_response",
            "response": {
                "subtype": "error",
                "request_id": request_id,
                "error": detail,
            },
        })
    )
}

/// 这一帧是不是**新一轮的开始**（`system/init`）。
///
/// 用来关闭取消之后的残帧抑制窗口。判据可自证：claude 的 `system/init` **每轮都发**，
/// 而且**只在收到那一轮的 user 消息之后**才发（spec 第 24 条；本机两轮探针实测帧序为
/// `init → status → assistant → result → init → status → assistant → result`）。
/// 因此「本轮真正的输出」一定排在本轮的 init 之后，而上一轮的残帧一定排在它之前 ——
/// claude 的轮次循环是串行的，它必须先把上一轮收尾才会开始下一轮。
///
/// 子会话（sidechain）的 init 不算：它属于某个 `Task` 内部，不是主线新一轮的开始。
fn frame_starts_a_turn(value: &Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    let is_sidechain = obj
        .get("parent_tool_use_id")
        .and_then(|v| v.as_str())
        .is_some_and(|id| !id.trim().is_empty());
    !is_sidechain
        && obj.get("type").and_then(|v| v.as_str()) == Some("system")
        && obj.get("subtype").and_then(|v| v.as_str()) == Some("init")
}

/// 残帧窗口对这一帧的裁定。返回 `(是否丢弃, 更新后的预算)`；`budget == 0` = 不在窗口里。
///
/// 两条关闭条件都必不可少：
/// - **`system/init`**（正常出口）：新一轮的第一帧，窗口立刻关闭，这一帧本身照常处理。
///   本轮真正的输出全部排在它之后 ⇒ 结构上不可能被这层抑制吞掉。
/// - **预算耗尽**（兜底）：init 万一不来（CLI 变形 / 我们判据看漏），到点必须停止抑制。
///   一直抑制会把下一轮的真实回答整段吞掉 —— 那比「上一轮残帧漏一帧」糟得多。
fn stale_frame_verdict(budget: u32, value: &Value) -> (bool, u32) {
    if budget == 0 || frame_starts_a_turn(value) {
        return (false, 0);
    }
    (true, budget - 1)
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
    /// **跨轮**的残帧抑制窗口：上一轮是被取消收尾的，接下来最多这么多帧仍可能属于它。
    ///
    /// 为什么需要跨轮：取消之后我们在**本轮的 `result`** 上就返回了，而 claude 侧的收尾
    /// （以及我们那条 `interrupt` 的 ack）可能还有几帧在路上。它们会被**下一轮**的读循环读到，
    /// 于是上一轮的半截正文漏进新回答里，更糟的是上一轮迟到的 `result` 被当成新一轮的结束信号
    /// ——新回答还在流就被判定「本轮结束」。
    ///
    /// 窗口靠下一轮的 `system/init` 关闭（见 `frame_starts_a_turn`：那是新一轮的第一帧，
    /// 本轮任何真实输出都排在它之后），`STALE_FRAME_BUDGET` 只是 init 不来时的兜底闸门。
    /// **一直抑制会把下一轮真正的输出也吞掉，那比原 bug 更糟**，所以两道关闭条件都要有。
    ///
    /// **这一层覆盖不到的残留竞态**（有意留给下一个任务，不要以为是漏了）：我们那条
    /// `interrupt` 是异步写进 stdin 的，如果它到达 claude 的时刻恰好晚于上一轮的收尾，
    /// 中断就会落在**下一轮**头上 —— 那时新一轮的 `init` 已经过去、窗口早已关闭，帧序推断
    /// 无从分辨。`result` 帧上的 `user_message_uuid` 能直接回答「这条 result 属于哪条用户
    /// 消息」，那才是这个竞态的根治办法；它落地之后这一层可以简化掉。
    stale_frames_left: u32,
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
                stale_frames_left: 0,
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

    /// 常驻子进程的 pid。只作为注册表元数据（诊断 / 「两轮是不是同一个进程」），
    /// 关停一律走 `close()`（关 stdin + `wait`），绝不按 pid 杀。
    pub fn child_pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// 跑一轮：往 stdin 写一行 stream-json user 消息 → 读 stdout 直到本轮的 `result` → 返回。
    /// **不关 stdin**（关了进程就退出了）。
    ///
    /// `SessionCommand::RunTurn` 携带的 model / reasoning 在这里**有意忽略**：claude 的模型与
    /// effort 是**启动 flag**，会话内无法切换。中途换配置由注册表的 `LaunchConfig` 指纹拦下
    /// 并重连一个新进程（见 `session/live.rs::LaunchConfig`），而不是在这里假装切换成功。
    ///
    /// **这个读循环有意没有轮次超时**（不是漏了）：
    /// - 一轮**合法地**可以跑很久（连着调几十个工具、一个 `Bash` 跑十分钟），所以任何「总时长
    ///   上限」都会在正常使用里误杀，代价是用户丢掉整轮工作 + 会话被丢弃（上下文一起没）。
    /// - 「完全没有帧到达」的**静默超时**判据是成立的（claude 每 30s 会给在跑的工具发一条
    ///   `tool_progress` 心跳 —— 二进制里 `setInterval(…, 30000)` 发 `tool_heartbeat`），
    ///   但它换来的收益在本次修复之后基本归零：唯一已知的「会永久等下去」的成因是
    ///   **不回复 `control_request`**，而上面的分流已经让那件事在结构上不可能发生。
    /// - 而且用户始终有一条**有界的**逃生通道：点「停止」→ 协议级 interrupt，10 秒内没收尾
    ///   就升级到硬 `Close`（`run.rs::cancel_should_escalate`）。它由用户触发，没有误判。
    ///
    /// 真要加静默超时，判据必须是「距上一帧超过 N 分钟」（N 远大于 30s 心跳周期）且**不能**
    /// 顺手丢掉会话 —— 否则就是把一个罕见的挂死换成一个常见的误杀。
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
        // `--resume` 的会话在 claude 那边已经不存在了（见 `stream::claude::is_missing_session_error`）。
        // 记下原始文案在轮末返回，让 `run.rs` 的重连策略把它降级成「换个新会话重连 + 上下文已重置
        // 提示」，而不是把一句英文原文甩给用户。
        let mut missing_session: Option<String> = None;
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

            // 解析一次，然后按帧类型分流（控制通道 vs 内容）。非 JSON 行交给解析器唯一那条
            // `Raw` 出口，别在这里另开一份（spec 第 2 / 10 条）。
            let value = match serde_json::from_str::<Value>(line.trim()) {
                Ok(value) => value,
                Err(_) => {
                    let mut buf: Vec<UnifiedAgentEvent> = Vec::new();
                    self.handler.handle_line(&line, &mut |event| buf.push(event));
                    for event in buf {
                        let _ = events.send(event).await;
                    }
                    continue;
                }
            };
            match classify_inbound_frame(&value) {
                // **fail-closed**：认不出来的控制请求也必须回一条 error 响应。沉默会让 claude
                // 永远等下去（它那侧没有超时），而本轮的读循环也没有超时 —— 那一轮就永久挂死。
                InboundFrame::Reply(reply) => {
                    let _ = self.stdin.write_all(reply.as_bytes()).await;
                    let _ = self.stdin.flush().await;
                    continue;
                }
                InboundFrame::Ignore => continue,
                InboundFrame::Stream => {}
            }

            // 上一轮取消后的残帧窗口。窗口内的帧**整帧丢弃**（不喂解析器、不发事件、更不当
            // 轮次边界）：它们属于一个我们这边已经收尾的轮次，喂进去只会把本轮的 per-turn
            // 状态在中途清掉、并让上一轮迟到的 `result` 把本轮当场判定为结束。
            let (drop_stale, next_budget) = stale_frame_verdict(self.stale_frames_left, &value);
            self.stale_frames_left = next_budget;
            if drop_stale {
                continue;
            }

            // 轮次边界靠解析器的 `result` 计数判定：喂一行前后各取一次。这样既复用了唯一
            // 那份解析逻辑（spec 第 2 条），又不用为了找边界把同一行 JSON 再解析一遍。
            let before = self.handler.completed_result_turns();
            let mut buf: Vec<UnifiedAgentEvent> = Vec::new();
            self.handler.handle_value(&value, &mut |event| buf.push(event));
            for event in buf {
                if suppress_after_cancel(cancelled, &event) {
                    continue;
                }
                // `--resume` 的目标会话不存在：这条错误由本函数的返回值上报，**不进气泡**。
                // 用户该看到的是「上下文已重置」，不是 claude 的英文原句。
                if let UnifiedAgentEvent::Error { message } = &event {
                    if crate::external_agents::stream::claude::is_missing_session_error(message) {
                        missing_session = Some(message.clone());
                        continue;
                    }
                }
                let _ = events.send(event).await;
            }

            if self.handler.completed_result_turns() > before {
                if let Some(message) = missing_session {
                    return Err(message);
                }
                // 被中断的轮次同样有 `result`（形态见 `stream::claude::result_is_user_abort`）：
                // 走「已取消」出口而不是错误出口，否则每点一次停止都弹一个假错误气泡。
                if cancelled || self.handler.last_result_aborted() {
                    // 取消是在**本轮的 result** 上收尾的，claude 侧的收尾帧与 interrupt 的 ack
                    // 可能还有几帧在路上 —— 开一个窗口，别让它们漏进下一轮（见 `stale_frames_left`）。
                    self.stale_frames_left = STALE_FRAME_BUDGET;
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
    ///
    /// 注意这一层只管**本轮内**迟到的错误回声。「上一轮的残帧漏进下一轮」是另一件事，
    /// 由跨轮的 `stale_frame_verdict` 窗口负责（见下面那组单测）—— 两层的作用域不同，
    /// 别想着合并成一个判据。
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

    // ---- 帧分流：对不认识的控制请求必须回复（fail-closed）----

    fn frame(raw: &str) -> Value {
        serde_json::from_str(raw).expect("test fixture is valid json")
    }

    fn reply_for(raw: &str) -> Option<String> {
        match classify_inbound_frame(&frame(raw)) {
            InboundFrame::Reply(line) => Some(line),
            _ => None,
        }
    }

    /// **本项修复的核心断言**：喂一个我们不认识的 `control_request`，必须产生一条**带同一个
    /// `request_id`** 的 error 响应。
    ///
    /// 不回复的后果不是「少个功能」而是**永久挂死**：claude 那侧的 `pendingRequests` 没有超时，
    /// 我们的轮次读循环也没有超时 —— 那一轮再也不会结束。
    #[test]
    fn an_unknown_control_request_gets_an_error_response_with_the_same_request_id() {
        // `can_use_tool` 的真实形状（claude 2.1.220 二进制里的 zod schema + sendRequest 构造处）。
        let line = reply_for(
            r#"{"type":"control_request","request_id":"req-42",
                "request":{"subtype":"can_use_tool","tool_name":"Bash",
                           "display_name":"Bash","input":{"command":"rm -rf /"},
                           "tool_use_id":"toolu-1"}}"#,
        )
        .expect("必须回复，不能沉默");
        assert!(line.ends_with('\n'), "必须是一整行");
        let value = frame(line.trim());
        assert_eq!(value["type"], serde_json::json!("control_response"));
        assert_eq!(value["response"]["subtype"], serde_json::json!("error"));
        // **`request_id` 嵌在 `response` 里**，不是帧顶层（实测形状）。放错层级 = CLI 匹配不到
        // 这条响应，等于没回。
        assert_eq!(value["response"]["request_id"], serde_json::json!("req-42"));
        assert!(value.get("request_id").is_none(), "顶层不该有 request_id：{value}");
        // 错误文案要能让人看出是哪个子型没实现（会进 CLI 的 tool_result / 诊断）。
        let error = value["response"]["error"].as_str().unwrap_or_default();
        assert!(error.contains("can_use_tool"), "错误文案应带上子型：{error}");
        // 三个字段一律 snake_case —— 这条协议两套命名混用（`can_use_tool` 的**成功**载荷是
        // camelCase 的 `behavior`/`updatedInput`），error 分支千万别顺手写成 camelCase。
        assert!(value["response"].get("requestId").is_none());
    }

    /// 完全没见过的子型（未来新增）同样要回复 —— fail-closed 的意义就在于不认识也不能沉默。
    #[test]
    fn a_never_seen_control_request_subtype_still_gets_answered() {
        for raw in [
            r#"{"type":"control_request","request_id":"r1","request":{"subtype":"request_user_dialog","dialog_kind":"x"}}"#,
            r#"{"type":"control_request","request_id":"r2","request":{"subtype":"elicitation","mcp_server_name":"s","message":"m"}}"#,
            r#"{"type":"control_request","request_id":"r3","request":{"subtype":"totally_new_in_a_future_cli"}}"#,
            // `request` 缺失（CLI 自己会把这种判成 `Missing request on control_request`）：
            // 仍要回复，否则同样挂死。
            r#"{"type":"control_request","request_id":"r4"}"#,
        ] {
            let line = reply_for(raw).unwrap_or_else(|| panic!("没有回复：{raw}"));
            let value = frame(line.trim());
            assert_eq!(value["response"]["subtype"], serde_json::json!("error"));
            assert!(value["response"]["request_id"].is_string());
        }
    }

    /// 连 `request_id` 都没有 ⇒ 无从回复，只能丢（回一条没有 id 的响应对端也匹配不到）。
    #[test]
    fn a_control_request_without_a_request_id_is_dropped_not_answered() {
        assert!(matches!(
            classify_inbound_frame(&frame(
                r#"{"type":"control_request","request":{"subtype":"can_use_tool"}}"#
            )),
            InboundFrame::Ignore
        ));
    }

    /// `keep_alive` / `control_cancel_request` / `control_response`：**有意不接、也不需要回复**。
    ///
    /// 核实依据（claude 2.1.220 二进制）：`keep_alive` 的 schema 是 `{type:"keep_alive"}`
    /// ——无字段、无 request_id，CLI 自己的两个读取点都是直接跳过，没有任何需要回应的语义；
    /// `control_cancel_request` 在 CLI 自己那侧的处理只是 abort 掉在飞的请求、**不回任何东西**。
    #[test]
    fn control_channel_noise_is_ignored_without_a_reply() {
        for raw in [
            r#"{"type":"keep_alive"}"#,
            r#"{"type":"control_cancel_request","request_id":"req-42"}"#,
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"kivio-interrupt-1","response":{"still_queued":[]}}}"#,
        ] {
            assert!(
                matches!(classify_inbound_frame(&frame(raw)), InboundFrame::Ignore),
                "{raw} 应被安全忽略"
            );
        }
    }

    /// 内容帧照旧全部交给流解析器 —— 分流不能顺手把正文挡在门外。
    #[test]
    fn content_frames_still_go_to_the_stream_parser() {
        for raw in [
            r#"{"type":"system","subtype":"init","model":"claude-opus-4-8[1M]"}"#,
            r#"{"type":"assistant","message":{"id":"m","role":"assistant","content":[]}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}}}"#,
            r#"{"type":"user","message":{"role":"user","content":[]}}"#,
            r#"{"type":"result","subtype":"success"}"#,
            r#"{"type":"error","error":"boom"}"#,
            // 未来新增的顶层 type：交给解析器，由它的兜底分支安全忽略（spec 第 10 条）。
            r#"{"type":"some_future_frame"}"#,
        ] {
            assert!(
                matches!(classify_inbound_frame(&frame(raw)), InboundFrame::Stream),
                "{raw} 不该被分流挡掉"
            );
        }
    }

    // ---- 取消之后的跨轮残帧窗口 ----

    /// `system/init` 是新一轮的第一帧（实测每轮都发，且排在本轮任何输出之前）——
    /// 它就是窗口的关闭信号。
    #[test]
    fn only_a_main_line_init_marks_a_new_turn() {
        assert!(frame_starts_a_turn(&frame(
            r#"{"type":"system","subtype":"init","model":"m","session_id":"s"}"#
        )));
        // 子会话（Task 内部）的 init 不是主线新一轮的开始。
        assert!(!frame_starts_a_turn(&frame(
            r#"{"type":"system","subtype":"init","parent_tool_use_id":"toolu_sub_1"}"#
        )));
        for raw in [
            r#"{"type":"system","subtype":"status","status":"compacting"}"#,
            r#"{"type":"result","subtype":"success"}"#,
            r#"{"type":"assistant","message":{"id":"m","content":[]}}"#,
        ] {
            assert!(!frame_starts_a_turn(&frame(raw)), "{raw}");
        }
    }

    /// **本项修复的核心断言（第一半）**：取消之后迟到的帧不进下一轮 ——
    /// 既不把上一轮的半截正文漏进新回答，也不拿上一轮迟到的 `result` 当新一轮的结束信号。
    ///
    /// **（第二半，同样必须成立）**：窗口在新一轮的 `init` 上关闭，下一轮**真正的**输出
    /// 一帧都不能被吞。一直抑制比原 bug 更糟。
    #[test]
    fn stale_frames_are_dropped_until_the_next_turn_starts_and_not_after() {
        /// 走一帧，返回「是否被丢掉」并推进预算。
        fn step(budget: &mut u32, raw: &str) -> bool {
            let (dropped, next) = stale_frame_verdict(*budget, &frame(raw));
            *budget = next;
            dropped
        }
        let mut budget = STALE_FRAME_BUDGET;

        // 上一轮（被取消那一轮）的收尾残帧：正文 + 迟到的 result，两条都必须丢。
        assert!(
            step(&mut budget, r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"上一轮的半截话"}}}"#),
            "上一轮的正文漏进了下一轮的回答"
        );
        assert!(
            step(&mut budget, r#"{"type":"result","subtype":"error_during_execution","terminal_reason":"aborted_streaming"}"#),
            "上一轮迟到的 result 会被当成下一轮的结束信号"
        );

        // 新一轮开始：init 本身照常处理，窗口同时关闭。
        assert!(!step(
            &mut budget,
            r#"{"type":"system","subtype":"init","model":"m"}"#
        ));
        assert_eq!(budget, 0, "init 之后窗口必须彻底关闭");

        // 下一轮真正的输出一帧都不能被吞。
        assert!(!step(
            &mut budget,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"新回答"}}}"#
        ));
        assert!(!step(&mut budget, r#"{"type":"result","subtype":"success"}"#));
    }

    /// 兜底闸门：`init` 万一不来，抑制到点必须停 —— 否则一次取消能把后面所有输出永久吞掉。
    #[test]
    fn the_stale_window_gives_up_after_its_budget() {
        let noise = frame(r#"{"type":"assistant","message":{"id":"m","content":[]}}"#);
        let mut budget = STALE_FRAME_BUDGET;
        for i in 0..STALE_FRAME_BUDGET {
            let (dropped, next) = stale_frame_verdict(budget, &noise);
            assert!(dropped, "第 {i} 帧应仍在窗口内");
            budget = next;
        }
        assert_eq!(budget, 0);
        let (dropped, _) = stale_frame_verdict(budget, &noise);
        assert!(!dropped, "预算耗尽后必须停止抑制");
    }

    /// 没取消过（`budget == 0`）时这层完全透明 —— 包括 `init`。
    #[test]
    fn the_stale_window_is_transparent_when_no_cancel_happened() {
        for raw in [
            r#"{"type":"system","subtype":"init"}"#,
            r#"{"type":"result","subtype":"success"}"#,
            r#"{"type":"assistant","message":{"id":"m","content":[]}}"#,
        ] {
            assert_eq!(stale_frame_verdict(0, &frame(raw)), (false, 0), "{raw}");
        }
    }

    // ---- resume 失效的判据（降级动作在 `run.rs::persistent_failure_action`）----

    /// 本机实测原样本的 `errors[]` 文案必须被认出来：认不出 ⇒ 用户拿到一句英文原文，
    /// 而正确处置是丢掉那个已不存在的会话 id、开新会话继续，并提示上下文已重置。
    #[test]
    fn the_missing_session_error_text_is_recognized() {
        use crate::external_agents::stream::claude::is_missing_session_error;
        assert!(is_missing_session_error(
            "No conversation found with session ID: d85724b7-59e4-4690-8984-1f31ca9a3414"
        ));
        assert!(!is_missing_session_error("Not logged in · Please run /login"));
        assert!(!is_missing_session_error(
            "No message found with message.uuid of: abc"
        ));
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

    /// 同上，但把会话 flag 换成 `--resume <session_id>`（用来构造「resume 一个不存在的会话」）。
    fn live_resume_args(session_id: &str) -> Vec<String> {
        crate::external_agents::defs::claude::claude_args_resuming(
            &live_args(session_id, None),
            session_id,
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

    /// **resume 失效必须降级，不是甩个报错给用户**（真机验收）。
    ///
    /// 这条属于「效果对不对」类：单测能证明判据认得那句话、也能证明降级动作算得对，但**证明不了
    /// 真实的 CLI 确实会那样报、以及换新会话之后它真的起得来**。所以这里拿一个**不存在**的
    /// 会话 id 去 `--resume`，对真实二进制断言两件事：
    ///
    /// 1. 失败文案就是 `No conversation found with session ID`（判据的唯一依据），
    ///    并且我们的判据 `is_missing_session_error` 认得它；
    /// 2. 按降级动作改写 argv（摘掉 `--resume`、换一个新 `--session-id`）之后**同一个进程能起来
    ///    并跑到轮次收尾**，而且那一轮的失败（如果有）**不再是** resume 失效 ——
    ///    也就是说降级真的把用户从这个死胡同里救出来了。
    ///
    /// 这条**不需要登录**（resume 的加载发生在认证之前），所以在未登录的机器上也是有效验证；
    /// 第 2 步在未登录时会以「Not logged in」收尾，那不影响本条的判据。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real-machine: spawns the installed claude CLI"]
    async fn live_resuming_a_missing_session_degrades_to_a_fresh_session() {
        use crate::external_agents::defs::claude::claude_args_fresh_session;
        use crate::external_agents::stream::claude::is_missing_session_error;

        let Some(bin) = resolve_binary(&CLAUDE_AGENT_DEF).await else {
            eprintln!("SKIP: 本机没有可用的 claude CLI");
            return;
        };
        // 一个从没存在过的会话 id —— 模拟「claude 那边的会话记录被清理掉了」。
        let dead_id = Uuid::new_v4().to_string();
        let workdir = std::env::temp_dir().join(format!("kivio-claude-deadresume-{dead_id}"));
        std::fs::create_dir_all(&workdir).expect("create workdir");

        // ---- 第 1 步：确认真实 CLI 的失败形态与我们的判据一致 ----
        let dead_args = live_resume_args(&dead_id);
        assert!(dead_args.windows(2).any(|w| w == ["--resume", dead_id.as_str()]));
        let failure = match ClaudeStreamJsonSession::connect(&bin, &dead_args, &workdir).await {
            // 实测进程约 2.2s 才退出，所以 `connect` 的即时 `try_wait` 通常抓不到 ——
            // 失败以流里那条 `result` 的形式到达 `run_turn`。两条路都要能认出来。
            Ok(session) => {
                let control = spawn_claude_stream_session_actor(session);
                let turn = one_turn(&control, "say hi", false).await;
                let _ = control.send(SessionCommand::Close).await;
                let _ = timeout(CLAUDE_SHUTDOWN_TIMEOUT, control.closed()).await;
                turn.result.err().unwrap_or_default()
            }
            Err(err) => err,
        };
        eprintln!("dead-resume failure: {failure}");
        assert!(
            is_missing_session_error(&failure),
            "真机的 resume 失效文案与判据不符（判据会漏掉这个场景，用户拿到裸报错）：{failure}"
        );

        // ---- 第 2 步：按降级动作改写 argv，确认真的能继续 ----
        let fresh_id = Uuid::new_v4().to_string();
        let fresh_args = claude_args_fresh_session(&dead_args, &fresh_id);
        let session = match ClaudeStreamJsonSession::connect(&bin, &fresh_args, &workdir).await {
            Ok(session) => session,
            Err(err) => panic!("降级后仍然起不来（降级没救到用户）：{err}"),
        };
        assert_eq!(session.session_id(), fresh_id, "降级后应使用新的会话 id");
        let control = spawn_claude_stream_session_actor(session);
        let turn = one_turn(&control, "Reply with just the word OK.", false).await;
        let outcome = turn.result.as_ref().err().cloned().unwrap_or_default();
        eprintln!("after downgrade: result={outcome:?} text={:?}", turn.text.trim());
        assert!(
            !is_missing_session_error(&outcome),
            "降级之后还在报 resume 失效：{outcome}"
        );
        if turn.result.is_err() {
            eprintln!(
                "NOTE: 降级后的这一轮没成功（未登录 / 网络？）——本条的判据是「不再是 resume 失效」，仍然成立"
            );
        }

        close_and_cleanup(control, &workdir).await;
    }
}
