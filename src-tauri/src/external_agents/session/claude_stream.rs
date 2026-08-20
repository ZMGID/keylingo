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
use crate::external_agents::session::live::{
    ApprovalAsk, ApprovalBridge, SessionCommand, CANCELLED_SESSION_LOST,
};
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

/// 等 `set_model` 的 `control_response` 最多多久。超时按「CLI 不认这条请求」处理，
/// 退回换进程 —— 宁可白跑一次重连，也不能让用户换了模型却静默跑旧的。
/// 这不是模型推理时间，只是一次本地控制往返，3s 已经很宽松。
const SET_MODEL_ACK_TIMEOUT: Duration = Duration::from_secs(3);
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
        std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            // 非法 UTF-8：tokio 的 `read_line` 会把这一行的字节丢掉、**reader 仍然可用**
            // （见 `read_line.rs::finish_string_read`）。判成不可恢复的话，一个完好的进程
            // 会被当成「常驻会话在轮次中退出」⇒ 整轮 prompt 重发一次（工具副作用可能重跑）。
            | std::io::ErrorKind::InvalidData
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

/// 一行 `stop_task` 控制请求（含结尾换行）。Agent SDK `Query.stopTask()` 的线上形态：
/// 载荷 `{subtype:"stop_task", task_id}`，`request_id` 与 `interrupt`/`set_model`
/// 一样在**帧顶层**。CLI 停掉任务后发 `status:"stopped"` 的 `task_notification`。
fn stop_task_request_line(request_id: &str, task_id: &str) -> String {
    format!(
        "{}\n",
        json!({
            "type": "control_request",
            "request_id": request_id,
            "request": { "subtype": "stop_task", "task_id": task_id },
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
    /// claude 在问「这个工具能用吗」，且本轮有宿主可问 ⇒ 挂起，等用户答复。
    Ask(ApprovalAsk),
    /// claude 撤回了先前那条询问（已在别处答掉）⇒ 把挂起表里那条删掉，**不回任何东西**
    /// （CLI 自己收到这条时也只 `abort()` 在飞的请求）。
    CancelAsk(String),
}

/// claude 在 stream-json 下能**向我们**发出的、需要回复的 `control_request` 子型。
///
/// **协议事实**（claude 2.1.220，`grep -a` 读本机二进制里的 zod schema 与 `sendRequest` 构造处）：
/// CLI→客户端的 `control_request.request` 是一个 12 成员 union
/// （`can_use_tool` / `request_user_dialog` / `elicitation` / `set_cwd` / `message_rated` /
/// `oauth_token_refresh` / `host_auth_token_refresh` / `stop_task` / `background_tasks` /
/// `apply_flag_settings` / `get_settings` / `submit_feedback`），全都是「问我们、等我们答」。
///
/// 我们只实现了 `can_use_tool`（工具审批），其余一律回一条 error —— 沉默的代价是整轮永久挂死。
const UNSUPPORTED_CONTROL_REQUEST: &str = "Kivio 尚不支持这个控制请求";

/// 用户点「拒绝」时回给 CLI 的话。它会原样变成那次工具调用的 `tool_result`（实测），
/// 所以要写成模型看得懂、且能据此改变计划的一句中文。
pub const APPROVAL_DENIED_MESSAGE: &str =
    "用户拒绝了这次操作。不要重试这个调用；请说明你原本想做什么，并等待用户的进一步指示。";
/// 取消 / 关会话 / 进程消失时，批量拒掉所有挂起询问用的话。
const APPROVAL_ABORTED_MESSAGE: &str = "用户中止了本轮，这次操作未被批准。";
/// 需要用户在卡片上直接作答的工具（`AskUserQuestion` / `ExitPlanMode`）。
///
/// **官方机制**（[Agent SDK · user-input](https://code.claude.com/docs/en/agent-sdk/user-input)）：
/// 这类工具**就是从同一条 `can_use_tool` 通道**答的 ——
/// `{"behavior":"allow","updatedInput":{"questions":<原样回传>,"answers":{"<问题文本>":"<选中的 label>"}}}`。
/// 没有第二条控制请求。此前这里的注释说「批准后 CLI 会紧接着发一条我们还没实现的
/// `request_user_dialog`」是**错的**，`request_user_dialog` 与这条路无关。
///
/// 之所以今天仍然拒：答复要带用户**选了哪个选项**，而会话层拿不到 `AppHandle`
/// （审批是宿主的事），`ApprovalDecision` 现在只有一个 bool，装不下选项。要放开得先让
/// 审批通道能回载荷 —— 那是一件独立的功能，不是这里改个判据能顺手带上的。
///
/// 另外 `--permission-prompt-tool` 那条「allow 会被转成 deny」的官方限制，作用域是
/// **MCP 工具**；`AskUserQuestion` / `ExitPlanMode` 是内置工具，不在其中 —— 所以放开之后
/// 不会撞上那条。
const APPROVAL_INTERACTIVE_UNSUPPORTED: &str =
    "Kivio 暂不支持这个需要在卡片上直接作答的工具，请改用普通回复继续。";

/// 一条已经送去问用户、还在等答复的询问。
///
/// **存在的理由是「必须回复」**：CLI 那侧的 `pendingRequests` 是个**没有超时**的 Promise，
/// 我们的轮次读循环也没有超时 —— 一条永远不被回复的询问在结构上就是一次永久挂死。
/// 所以取消 / 关会话 / 轮次收尾时这张表必须被**整批**拒掉（`reject_pending_lines`）。
///
/// **诚实的边界**（2026-07-29 本机 claude 2.1.220 实测，别把这一手当成「修了一个正在发生的
/// 死锁」）：把批量拒绝**注释掉**之后，`live_cancelling_with_a_pending_approval_leaves_the_session_usable`
/// **仍然通过** —— 因为我们那条 `interrupt` 会让 CLI 自己 abort 掉在飞的权限请求
/// （二进制里那个 promise 挂在本轮的 `AbortController` 上，`u.abort()` 会带走它）。
/// 也就是说：**今天可观测的死锁并不存在**，批量拒绝是 fail-closed 的兜底 ——
/// 它把「不挂死」从「依赖 CLI 内部恰好会 abort」变成「我们自己保证」，代价为零
/// （几条 JSON 行）。真正的红→绿判据在单测 `cancelling_rejects_every_pending_approval_exactly_once`，
/// 不是那条真机测试。
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingApproval {
    request_id: String,
    tool_name: String,
}

/// 挂起表 → 一批 deny 响应行（每条询问恰好一行，顺序与挂起顺序一致）。
///
/// 抽成纯函数是因为它承载了本项最不能出错的一条不变量：**一条都不能漏**。
/// 单测直接断言「每个挂起的 request_id 都出现且只出现一次、且一律是 deny」。
fn reject_pending_lines(pending: &[PendingApproval], reason: &str) -> Vec<String> {
    pending
        .iter()
        .map(|entry| approval_response_line(&entry.request_id, false, reason, None))
        .collect()
}

/// 一条 `can_use_tool` 的答复行（含结尾换行）。
///
/// **协议形状**（claude 2.1.220，二进制里的 zod union `UdE | BdE` + 本机真机对照）：
/// - 允许：`{"behavior":"allow"}`（可选 `updatedInput` / `updatedPermissions`；我们不改入参，
///   所以只发 `behavior`——CLI 会把原入参当作 `updatedInput`）；
/// - 拒绝：`{"behavior":"deny","message":<必填>,"interrupt":false}`，`message` 会原样成为
///   那次工具调用的 `tool_result`（实测）。`interrupt:true` 会让 CLI abort 整轮，我们不要
///   ——拒一个工具不等于停掉整轮，用户想停有「停止」按钮。
///
/// **注意载荷这一层是 camelCase，而外面三件套是 snake_case**：这条协议两套命名混用
/// （`subtype` / `request_id` / `error` 是 snake_case，`behavior` / `updatedInput` /
/// `updatedPermissions` 是 camelCase）。而 `request_id` 嵌在 `response` **里面**、不在帧顶层
/// —— 放错层级 = CLI 匹配不到 = 等于没回。
fn approval_response_line(
    request_id: &str,
    approved: bool,
    deny_message: &str,
    updated_input: Option<&Value>,
) -> String {
    let payload = if approved {
        match updated_input {
            // `AskUserQuestion` 的答复就走这里：`updatedInput` 带回 `{questions, answers}`。
            Some(input) => json!({ "behavior": "allow", "updatedInput": input }),
            None => json!({ "behavior": "allow" }),
        }
    } else {
        json!({ "behavior": "deny", "message": deny_message, "interrupt": false })
    };
    format!(
        "{}\n",
        json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request_id,
                "response": payload,
            },
        })
    )
}

/// 一行 `set_model` 控制请求（含结尾换行）。
///
/// **官方依据**：Agent SDK 的 `Query.setModel(model?)`，限制是 "only available in **streaming
/// input mode**" —— `--input-format stream-json` 就是 streaming input mode。changelog 2.1.212
/// 起中途切换还能在**当前轮**生效（此前要等下一轮）；2.1.208 修的是「非字符串 `set_model`
/// 载荷让会话永久挂死」，也就是说这条通道在 CLI 侧一直是实现了的。
///
/// **wire 字段名没有官方文档，是推断的**（`interrupt` 的形状 + `setModel(model)` 的签名）。
/// 所以调用方**必须**读回 `control_response` 再决定信不信：认不出来时 CLI 会回一条 error
/// （2.1.208 之后不再挂死），我们据此退回「换进程 + 新 `--model`」那条老路。
/// 猜错的代价因此是「白跑一次重连」，不是「用户换了模型却静默没生效」。
fn set_model_request_line(request_id: &str, model: &str) -> String {
    format!(
        "{}
",
        json!({
            "type": "control_request",
            "request_id": request_id,
            "request": { "subtype": "set_model", "model": model },
        })
    )
}

/// 一条 `set_permission_mode` 控制请求（含结尾换行）。
///
/// **批准计划（`ExitPlanMode`）必须先发这条**：CLI 不会因为那次 `allow` 自己离开计划档，
/// 不切档位的话它下一句 `Edit` 又被挡回去 —— 用户点了「批准」却什么都没发生。
///
/// **wire 形状是二进制核实的**（claude 2.1.207）：CLI 自己构造这条请求的代码是
/// `{type:"control_request", request_id:`set-mode-…`, request:{subtype:"set_permission_mode",
/// mode:e.permissionMode, ultraplan:e.ultraplan}}`，且 `set_permission_mode` 就在它接受的
/// 控制请求集合里（`new Set(["set_model","set_permission_mode","interrupt",…])`）。
/// `ultraplan` 是可选的，我们不发。
///
/// 与 `set_model` 同样的纪律：**调用方要读回 `control_response`**。二进制里有一条
/// `set_permission_mode is not supported in this context (onSetPermissionMode callback not
/// registered)` 的错误串 —— 也就是说它可能失败，失败了就得让用户知道，而不是让他对着一个
/// 「批准了但 claude 还在计划档」的会话发呆。
fn set_permission_mode_request_line(request_id: &str, mode: &str) -> String {
    format!(
        "{}\n",
        json!({
            "type": "control_request",
            "request_id": request_id,
            "request": { "subtype": "set_permission_mode", "mode": mode },
        })
    )
}

/// 一条 `control_response` 是不是在答我们那个 request_id；是的话返回它成功了没有。
///
/// 形状与我们**发出去**的答复同构（`response.subtype` 为 `success` / `error`，
/// `response.request_id` 回显）。
fn control_response_verdict(frame: &Value, request_id: &str) -> Option<bool> {
    if frame.get("type").and_then(|v| v.as_str()) != Some("control_response") {
        return None;
    }
    let response = frame.get("response")?;
    if response.get("request_id").and_then(|v| v.as_str()) != Some(request_id) {
        return None;
    }
    Some(response.get("subtype").and_then(|v| v.as_str()) == Some("success"))
}

/// 这条询问该问用户，还是当场就能定？返回 `Err(理由)` = 直接拒，不打扰用户。
///
/// 目前只有一条当场拒的规则：`requires_user_interaction` 且**不是我们已经接住的那两个**
/// （见 `APPROVAL_INTERACTIVE_UNSUPPORTED`）。抽成纯函数是为了让「哪些不问用户」有单测可证。
fn approval_verdict(ask: &ApprovalAsk) -> Result<(), &'static str> {
    // 三个交互工具都答得了：
    // - `AskUserQuestion` → Kivio 自己的问用户卡片，选项经 `updated_input` 回去；
    // - `ExitPlanMode`   → 审批卡，批准时经 `set_permission_mode` 先切档位再放行；
    // - `EnterPlanMode`  → 审批卡，**放行就够** —— 它的实现里自己就切了档
    //   （二进制：`setToolPermissionContext(… {type:"setMode",mode:"plan",destination:"session"})`）。
    // 其余（CLI 将来新增的交互工具）仍当场拒 —— 沉默地放行一个我们答不了的交互工具，
    // 结果是那一轮挂在它自己的对话框上。
    if ask.requires_user_interaction
        && !is_ask_user_question(&ask.tool_name)
        && !is_exit_plan_mode(&ask.tool_name)
        && !is_enter_plan_mode(&ask.tool_name)
    {
        return Err(APPROVAL_INTERACTIVE_UNSUPPORTED);
    }
    Ok(())
}

/// claude 内置的「反问用户」工具名。大小写按 CLI 原样（`PascalCase` 有意义，不归一化），
/// 但比对时放宽 —— 名字是 CLI 给的，不值得为大小写差异丢掉整个功能。
pub fn is_ask_user_question(tool_name: &str) -> bool {
    crate::external_agents::ask_user::matches_tool("claude", tool_name)
}

/// claude 内置的「计划写完了，批准我去做」工具名。同样放宽大小写比对。
pub fn is_exit_plan_mode(tool_name: &str) -> bool {
    tool_name.eq_ignore_ascii_case("ExitPlanMode")
}

/// claude 内置的「让我先探索、出个方案再动手」工具名（claude 自己要求进入计划档）。
///
/// 与 `ExitPlanMode` 的**不对称**：这个工具的实现里自己就把会话切进了计划档
/// （二进制里 `call` 直接 `setMode: plan, destination: "session"`），所以宿主只要放行，
/// 不需要额外发 `set_permission_mode`。反向那次（离开计划档）落在哪一档是用户在卡片上
/// 三选一决定的，那次才要发切档帧。
pub fn is_enter_plan_mode(tool_name: &str) -> bool {
    tool_name.eq_ignore_ascii_case("EnterPlanMode")
}

/// 从一条 `can_use_tool` 请求里读出问用户所需的字段。
///
/// 字段形状（请求侧全是 **snake_case**，二进制核实 + 真机样本对照）：
/// `tool_name` / `display_name` / `input` / `tool_use_id` / `description` /
/// `permission_suggestions` / `requires_user_interaction` / …
///
/// `tool_use_id` 在 schema 里是 optional：缺失时回落 `request_id` 当 Kivio 侧的卡片 id
/// —— 卡片必须有个稳定 id 才能被答复，宁可与工具卡对不上也不能没有。
fn approval_ask_from_request(request_id: &str, request: &Value) -> ApprovalAsk {
    let tool_call_id = request
        .get("tool_use_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(request_id)
        .to_string();
    ApprovalAsk {
        request_id: request_id.to_string(),
        tool_call_id,
        tool_name: request
            .get("tool_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
        input: request.get("input").cloned().unwrap_or(Value::Null),
        requires_user_interaction: request
            .get("requires_user_interaction")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    }
}

/// 分流一帧 stdout JSON。
///
/// `can_ask` = 本轮有没有宿主可问（`SessionCommand::RunTurn.approvals`）。没有时
/// `can_use_tool` 仍走那条 fail-closed 的 error 兜底 —— 沉默的代价是整轮永久挂死。
fn classify_inbound_frame(value: &Value, can_ask: bool) -> InboundFrame {
    let Some(obj) = value.as_object() else {
        return InboundFrame::Stream;
    };
    match obj.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        // **必须回复**：claude 发出 `control_request` 之后就挂在那儿等 `control_response`
        // （它那侧是 `pendingRequests` 里的一个 Promise，**没有超时**）。我们不回 ⇒ 那个
        // 工具调用永远不返回 ⇒ 本轮的 `result` 永远不来 ⇒ 轮次读循环永久挂死。
        //
        // **`can_use_tool` 是我们唯一实现的子型**：它只在 argv 带
        // `--permission-prompt-tool stdio` 时才会到达（见 `defs::claude::claude_permission_prompt_args`；
        // 本机 2.1.220 实测：带这个 flag + `--permission-mode default` 让它写文件 ⇒ 收到
        // `can_use_tool`；不带 flag 的对照组一条都没有，权限被 CLI 直接拒）。
        // 二进制侧同一结论：`ekm(mode)` 里 `if(mode==="stdio") return createCanUseTool(...)`，
        // 官方 SDK 在用户提供 `canUseTool` 回调时也正是 push 这个 flag 值。
        //
        // 其余 11 个子型仍回 error（fail-closed）：宁可一次工具用不了，也不要整轮挂死。
        "control_request" => match request_id_of(obj) {
            Some(request_id) => {
                let request = obj.get("request");
                let subtype = request
                    .and_then(|r| r.get("subtype"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if can_ask && subtype == "can_use_tool" {
                    let request = request.cloned().unwrap_or(Value::Null);
                    InboundFrame::Ask(approval_ask_from_request(&request_id, &request))
                } else {
                    InboundFrame::Reply(control_error_response_line(&request_id, &subtype))
                }
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
        // **不回任何东西**（二进制里 CLI 自己收到这条时也只有 `Hn?.abort(...)`），但**必须把
        // 挂起表里那条删掉** —— 留着它，取消时的批量拒绝会对一条早已作废的询问回响应，
        // 而更糟的是它会让「本轮还有人在等」这个判断永久为真。
        "control_cancel_request" => match request_id_of(obj) {
            Some(request_id) => InboundFrame::CancelAsk(request_id),
            None => InboundFrame::Ignore,
        },
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
    /// 这个进程当前跑的模型（启动时来自 argv 的 `--model`，之后由 `set_model` 更新）。
    /// `None` = 启动时没指定（CLI 用它自己的默认）。用来判断「本轮要不要发 `set_model`」。
    active_model: Option<String>,
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
    /// 唤醒轮正文的收集窗口是否打开（轮间见到 `system/init` 开、见到 origin 为
    /// task-notification 的 `result` 关）。窗口外的空闲帧是取消残留，直接丢。
    idle_collecting: bool,
    /// 收集窗口内攒下的唤醒轮正文（TextDelta 累积），收尾时整段落成一条助手消息。
    idle_wake_text: String,
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
    pub async fn connect(resolved_bin: &Path, args: &[String], cwd: &Path) -> Result<Self, String> {
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
                active_model: crate::external_agents::defs::claude::claude_model_from_args(args),
                stderr_tail: Some(stderr_tail),
                stale_frames_left: 0,
                idle_collecting: false,
                idle_wake_text: String::new(),
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
    /// **model 在会话内切换**（2026-07-30 起）：`apply_model_change` 在写 prompt 之前发一条
    /// `set_model` 控制请求并等 ack，失败退回 `NEEDS_RECONNECT`（换进程 + 新 `--model`）。
    /// 官方依据是 Agent SDK 的 `Query.setModel()`，限制只有「streaming input mode」这一条，
    /// 而 `--input-format stream-json` 正是它。**别把 model 加回 `LaunchConfig` 指纹**——
    /// 那等于让用户每换一次模型都付一次冷启动 + 一整段历史重放。
    ///
    /// `reasoning` 仍然在这里**有意忽略**：`--effort` 的官方入口是
    /// `applyFlagSettings({effortLevel})`，wire 形状没核实过，所以它（连同 permission-mode）
    /// 继续由注册表的 `LaunchConfig` 指纹拦下、换进程生效（见 `session/live.rs::LaunchConfig`），
    /// 而不是在这里假装切换成功。
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
    /// 本轮要换模型的话，在会话内发一条 `set_model` 并**等它的 ack**。
    ///
    /// 为什么要等 ack 而不是 fire-and-forget：wire 字段名没有官方文档（见
    /// `set_model_request_line`）。不等的话猜错就是「用户换了模型、界面显示新模型、
    /// 实际还在跑旧模型」—— 静默且没有任何可观测信号，是最坏的失败方式。
    /// 等到 ack 是 error / 超时 ⇒ 返回 `NEEDS_RECONNECT`，`run.rs` 走既有的
    /// 「换进程 + 新 `--model`」那条路，也就是这次改动之前的行为。
    ///
    /// 这段读到的非 ack 帧一律丢弃：prompt 还没写出去，此刻能来的只有上一轮取消后
    /// 的残帧（本来就归 `stale_frame_verdict` 管）。`system/init` 不在其中 ——
    /// 它排在本轮第一条 user 消息之后。
    async fn apply_model_change(
        &mut self,
        model: Option<&str>,
        control: &mut mpsc::Receiver<SessionCommand>,
    ) -> Result<(), String> {
        // Catalog id (claude-sonnet-5) → runtime id (settings/env mapped). active_model is
        // always runtime-space (seeded from launch argv which already went through resolve).
        let Some(runtime) = crate::external_agents::session::claude_init::needs_set_model(
            self.active_model.as_deref(),
            model.unwrap_or(""),
        ) else {
            // Auto/empty, or already on the mapped runtime — no control request.
            return Ok(());
        };
        let request_id = format!("kivio-set-model-{}", Uuid::new_v4());
        let line = set_model_request_line(&request_id, &runtime);
        if self.stdin.write_all(line.as_bytes()).await.is_err() || self.stdin.flush().await.is_err()
        {
            return Err(crate::external_agents::session::acp::NEEDS_RECONNECT.to_string());
        }

        let deadline = tokio::time::Instant::now() + SET_MODEL_ACK_TIMEOUT;
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(crate::external_agents::session::acp::NEEDS_RECONNECT.to_string());
            }
            // 用户在这 3s 里点了停止 / 关会话：立刻按取消收场，别让他多等一个握手。
            match control.try_recv() {
                Ok(SessionCommand::Cancel) => return Err("cancelled".to_string()),
                Ok(SessionCommand::Close) => return Err("closed".to_string()),
                // 还在换模型的握手里，轮次都没开始，无从注入。
                Ok(SessionCommand::Steer { accepted, .. }) => {
                    let _ = accepted.send(false);
                }
                Ok(SessionCommand::RunTurn { done, .. }) => {
                    let _ = done.send(Err("session busy".to_string()));
                }
                // 换模型握手只有 3s，停任务不急这一下：静默丢掉，用户重点一次即可。
                Ok(SessionCommand::StopTask { .. }) => {}
                Err(mpsc::error::TryRecvError::Empty) => {}
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    return Err("control channel closed".to_string())
                }
            }
            match self.next_line().await {
                ReadStep::Line(line) => {
                    let Some(frame) = crate::external_agents::spawn::parse_json_line(&line) else {
                        continue;
                    };
                    match control_response_verdict(&frame, &request_id) {
                        Some(true) => {
                            self.active_model = Some(runtime);
                            return Ok(());
                        }
                        // CLI 不认这个控制请求（或拒绝了）⇒ 退回换进程。
                        Some(false) => {
                            return Err(
                                crate::external_agents::session::acp::NEEDS_RECONNECT.to_string()
                            )
                        }
                        None => {
                            // **不能无脑丢**：这一窗口里若来了别的 `control_request`（上一轮
                            // 没收干净的残留），不回复就是让 CLI 那条没有超时的 promise 永远挂着
                            // —— 正是 `classify_inbound_frame` 那套 fail-closed 要消灭的死法。
                            // `can_ask=false`：此刻还没有审批通道，一律回 error 而不是问用户。
                            if let InboundFrame::Reply(line) = classify_inbound_frame(&frame, false)
                            {
                                let _ = self.stdin.write_all(line.as_bytes()).await;
                                let _ = self.stdin.flush().await;
                            }
                            continue;
                        }
                    }
                }
                ReadStep::Idle => continue,
                // 进程没了 / 读挂了：交给既有的重连路径，别在这里自己发明处置。
                ReadStep::Eof | ReadStep::Fatal(_) => {
                    return Err(crate::external_agents::session::acp::NEEDS_RECONNECT.to_string())
                }
            }
        }
    }

    pub async fn run_turn(
        &mut self,
        prompt: &str,
        model: Option<&str>,
        images: &[ImageBlock],
        events: &mpsc::Sender<UnifiedAgentEvent>,
        control: &mut mpsc::Receiver<SessionCommand>,
        approvals: Option<ApprovalBridge>,
    ) -> Result<(), String> {
        // 换模型：先在**会话内**试一次 `set_model`，不行才退回换进程。
        // 必须排在写 prompt 之前 —— 否则这一轮的问题已经喂给旧模型了。
        self.apply_model_change(model, control).await?;

        let payload = stream_json_user_line(prompt, images)?;
        if let Err(err) = self.stdin.write_all(payload.as_bytes()).await {
            return Err(self
                .fold_tail(format!("写入 claude stdin 失败: {err}"))
                .await);
        }
        if let Err(err) = self.stdin.flush().await {
            return Err(self
                .fold_tail(format!("刷新 claude stdin 失败: {err}"))
                .await);
        }

        let (ask_tx, mut decisions) = match approvals {
            Some(ApprovalBridge {
                requests,
                decisions,
            }) => (Some(requests), Some(decisions)),
            None => (None, None),
        };
        // 已送去问用户、还在等答复的询问。**这张表的每一条都欠 CLI 一个 `control_response`**：
        // 漏一条 = 那一轮永久挂死（CLI 侧的 promise 没有超时，我们的读循环也没有）。
        // 所以下面每一条 `return` 之前都要先把它整批拒掉。
        let mut pending: Vec<PendingApproval> = Vec::new();

        let mut cancelled = false;
        let mut recoverable_errors = 0u32;
        // `--resume` 的会话在 claude 那边已经不存在了（见 `stream::claude::is_missing_session_error`）。
        // 记下原始文案在轮末返回，让 `run.rs` 的重连策略把它降级成「换个新会话重连 + 上下文已重置
        // 提示」，而不是把一句英文原文甩给用户。
        let mut missing_session: Option<String> = None;
        // 已发出、还没读到 `control_response` 的 `set_permission_mode`（批准计划时切档）。
        // `(request_id, 目标档位)`。见下面读到答复时的处理。
        let mut pending_mode_switch: Option<(String, String)> = None;
        loop {
            match control.try_recv() {
                Ok(SessionCommand::Cancel) => {
                    if !cancelled {
                        cancelled = true;
                        // **顺序不能反：先拒掉挂起的权限询问，再发中断。**
                        // 反过来的话 CLI 可能在还挂着一条没答复的询问时就开始收尾，那条询问
                        // 永远等不到回复 —— 而中断本身也就永远收不了尾。
                        self.reject_pending(&mut pending, APPROVAL_ABORTED_MESSAGE)
                            .await;
                        // 协议级中断，**不 kill**：进程要留给下一轮（常驻的核心收益）。
                        // 写失败也不立刻放弃 —— 继续读，`result` 可能已经在路上。
                        let line =
                            interrupt_request_line(&format!("kivio-interrupt-{}", Uuid::new_v4()));
                        let _ = self.stdin.write_all(line.as_bytes()).await;
                        let _ = self.stdin.flush().await;
                    }
                }
                Ok(SessionCommand::Close) => {
                    self.reject_pending(&mut pending, APPROVAL_ABORTED_MESSAGE)
                        .await;
                    return Err("closed".to_string());
                }
                // **不能**往在飞的轮次里写第二条 user 行。官方对 stream-json 输入的语义是
                // 「多条消息**顺序**处理」（Streaming Input 文档 + `--input-format stream-json`
                // 没有任何注入用的 control_request：只有 initialize / interrupt /
                // set_permission_mode / can_use_tool / hook_callback / mcp_message / set_model）。
                // 真写进去的后果是 CLI 在本轮 `result` 之后**自己再起一轮**回答它，那一轮的事件
                // 会漏进下一次 run_turn 的读循环里，把轮次边界搞乱。
                //
                // TUI 里那种「打字即插话」是交互界面自己的循环做的，不经过 stdin 协议。
                // 所以这里回 false：那条消息留在前端队列里，轮末按普通消息发出去。
                Ok(SessionCommand::Steer { accepted, .. }) => {
                    let _ = accepted.send(false);
                }
                Ok(SessionCommand::RunTurn { done, .. }) => {
                    let _ = done.send(Err("session busy".to_string()));
                }
                // 面板的「停止后台任务」：轮内直接写 `stop_task` 控制请求。CLI 收到后
                // 会发 `status:"stopped"` 的 `task_notification`，注册表由那帧修正。
                Ok(SessionCommand::StopTask { task_id }) => {
                    let line = stop_task_request_line(
                        &format!("kivio-stop-task-{}", Uuid::new_v4()),
                        &task_id,
                    );
                    let _ = self.stdin.write_all(line.as_bytes()).await;
                    let _ = self.stdin.flush().await;
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.reject_pending(&mut pending, APPROVAL_ABORTED_MESSAGE)
                        .await;
                    return Err("control channel closed".to_string());
                }
            }

            // 用户答复了某条询问 ⇒ 回一条 `control_response` 并从挂起表里摘掉。
            // 与 control 通道同一个 200ms 轮询粒度：审批本来就是人的反应时间，够快。
            if let Some(rx) = decisions.as_mut() {
                while let Ok(decision) = rx.try_recv() {
                    let Some(index) = pending
                        .iter()
                        .position(|entry| entry.request_id == decision.request_id)
                    else {
                        // 已经被 `control_cancel_request` 撤回、或已在别处答过：不能重复回复。
                        continue;
                    };
                    pending.remove(index);
                    // 先切档位、再放行（顺序不能反）：`ExitPlanMode` 的批准要让 CLI 真的
                    // 离开计划档，否则它下一句 `Edit` 又被挡回来。同一条 stdin 上的两帧按
                    // 写入顺序处理，所以这里只要保证「切档在前」即可。
                    if let Some(mode) = decision.set_permission_mode.as_deref() {
                        let request_id = format!("kivio-set-mode-{}", Uuid::new_v4());
                        let line = set_permission_mode_request_line(&request_id, mode);
                        let _ = self.stdin.write_all(line.as_bytes()).await;
                        let _ = self.stdin.flush().await;
                        // 读回它的 `control_response`：这条请求**可能不被支持**（二进制里有
                        // 那条 `onSetPermissionMode callback not registered` 的错误串）。
                        // 失败了要让用户看见，而不是留他对着一个「批准了却还在计划档」的会话。
                        pending_mode_switch = Some((request_id, mode.to_string()));
                    }
                    let line = approval_response_line(
                        &decision.request_id,
                        decision.approved,
                        APPROVAL_DENIED_MESSAGE,
                        decision.updated_input.as_ref(),
                    );
                    let _ = self.stdin.write_all(line.as_bytes()).await;
                    let _ = self.stdin.flush().await;
                }
            }

            let line = match self.next_line().await {
                ReadStep::Line(line) => {
                    recoverable_errors = 0;
                    line
                }
                ReadStep::Idle => continue,
                ReadStep::Eof => {
                    // 进程在轮次中间没了 ⇒ 挂起的询问再也回不掉了（管道已断），但表要清空，
                    // 免得下一轮误以为还有人在等。取消途中遇到这个：按「已取消但会话作废」
                    // 上报，否则重试逻辑会把用户刚刚停掉的这一轮原样重发一遍。
                    pending.clear();
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
                        pending.clear();
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
                    self.handler
                        .handle_line(&line, &mut |event| buf.push(event));
                    for event in buf {
                        let _ = events.send(event).await;
                    }
                    continue;
                }
            };
            // 切档位请求的回执。失败必须说出来 —— 用户刚点了「批准计划」，而 claude 还在
            // 计划档里，它接下来每一次编辑都会被挡回去，界面上却什么都看不出来。
            if let Some((request_id, mode)) = pending_mode_switch.as_ref() {
                if let Some(ok) = control_response_verdict(&value, request_id) {
                    if !ok {
                        let _ = events
                            .send(UnifiedAgentEvent::TextDelta {
                                delta: format!(
                                    "\n\n> ⚠️ 已批准计划，但把 claude 切到「{mode}」档位失败了 —— 它可能仍在计划模式，无法真正修改文件。\n\n"
                                ),
                            })
                            .await;
                    }
                    pending_mode_switch = None;
                    continue;
                }
            }
            match classify_inbound_frame(&value, ask_tx.is_some()) {
                // **fail-closed**：认不出来的控制请求也必须回一条 error 响应。沉默会让 claude
                // 永远等下去（它那侧没有超时），而本轮的读循环也没有超时 —— 那一轮就永久挂死。
                InboundFrame::Reply(reply) => {
                    let _ = self.stdin.write_all(reply.as_bytes()).await;
                    let _ = self.stdin.flush().await;
                    continue;
                }
                // 「这个工具能用吗」——送去问用户，挂起等答复。
                InboundFrame::Ask(ask) => {
                    let request_id = ask.request_id.clone();
                    let tool_name = ask.tool_name.clone();
                    // 当场就能定的（需要在卡片上作答的交互式工具）直接拒，不打扰用户。
                    let immediate_deny = match approval_verdict(&ask) {
                        Err(reason) => Some(reason),
                        // 宿主那边的通道满了 / 已经没人接了 ⇒ 同样当场拒。
                        // **绝不能在这里静默**：那一轮会永久挂死。
                        Ok(()) => match ask_tx.as_ref().map(|tx| tx.try_send(ask)) {
                            Some(Ok(())) => None,
                            _ => Some(APPROVAL_ABORTED_MESSAGE),
                        },
                    };
                    match immediate_deny {
                        Some(reason) => {
                            let line = approval_response_line(&request_id, false, reason, None);
                            let _ = self.stdin.write_all(line.as_bytes()).await;
                            let _ = self.stdin.flush().await;
                        }
                        None => pending.push(PendingApproval {
                            request_id,
                            tool_name,
                        }),
                    }
                    continue;
                }
                // CLI 撤回了那条询问：从挂起表摘掉，**不回任何东西**。
                InboundFrame::CancelAsk(request_id) => {
                    pending.retain(|entry| entry.request_id != request_id);
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
                // 唯一豁免：后台任务生命周期。窗口丢的是**轮次内容**，而任务状态是跨轮的
                // ——task_notification 落进窗口（取消/重新生成后正好撞上，实测）会让面板
                // 那条永远卡在「运行中」。
                if let Some(event) =
                    crate::external_agents::stream::claude::background_task_event(&value)
                {
                    let _ = events.send(event).await;
                }
                continue;
            }

            // 轮次边界靠解析器的 `result` 计数判定：喂一行前后各取一次。这样既复用了唯一
            // 那份解析逻辑（spec 第 2 条），又不用为了找边界把同一行 JSON 再解析一遍。
            let before = self.handler.completed_result_turns();
            let mut buf: Vec<UnifiedAgentEvent> = Vec::new();
            self.handler
                .handle_value(&value, &mut |event| buf.push(event));
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
                // 本轮收尾了 ⇒ 任何还挂着的询问都不会再被 CLI 用到，但**仍要回掉**：
                // 一条没回的询问会跨轮留在 CLI 的 `pendingRequests` 里，而下一轮的读循环
                // 同样没有超时。正常路径下这张表在这里已经是空的（收尾意味着工具都定了）。
                self.reject_pending(&mut pending, APPROVAL_ABORTED_MESSAGE)
                    .await;
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

    /// 把挂起表**整批**拒掉并清空。取消 / 关会话 / 轮次收尾都要过这一手。
    ///
    /// 不变量：**一条都不能漏**。一条永远不被回复的询问在结构上就是一次永久挂死
    /// （CLI 侧的 promise 没有超时，我们的读循环也没有）。
    /// 边界说明（实测过、别误读成「修了一个正在发生的死锁」）见 `PendingApproval` 的注释。
    /// 写失败（管道已断）不影响清空：那种情况下 CLI 也已经不在了。
    async fn reject_pending(&mut self, pending: &mut Vec<PendingApproval>, reason: &str) {
        if pending.is_empty() {
            return;
        }
        for line in reject_pending_lines(pending, reason) {
            let _ = self.stdin.write_all(line.as_bytes()).await;
        }
        let _ = self.stdin.flush().await;
        pending.clear();
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

    /// 轮次之间写一条 `stop_task` 控制请求。claude 常驻进程在轮间照常读 stdin，
    /// 停止立即生效；它发的 `task_notification`（stopped）由轮间空闲读消费进注册表
    /// （发起方已乐观置 stopped，见 interaction.rs）。
    pub async fn send_stop_task(&mut self, task_id: &str) {
        let line = stop_task_request_line(&format!("kivio-stop-task-{}", Uuid::new_v4()), task_id);
        self.write_control_line(&line).await;
    }

    /// 往 stdin 写一整行控制帧。只在**不会被 select 取消**的上下文调用（actor 的
    /// handler 段 / 轮内读循环）——写到一半被取消会留半行破 JSON。
    async fn write_control_line(&mut self, line: &str) {
        let _ = self.stdin.write_all(line.as_bytes()).await;
        let _ = self.stdin.flush().await;
    }

    /// 轮间空闲读一步：**只读 + 分类，不写**。
    ///
    /// 只读的原因：这个 future 挂在 actor 的 `select!` 里，取消发生在 await 点——
    /// 读用的 `Lines::next_line` 官方保证 cancel-safe，而写到一半被取消会往 stdin
    /// 留半行破 JSON。所以写回动作（Reply 行）交给调用方在 select 之外执行。
    /// `next_line` 之后没有任何 await，分类一定跑完。
    ///
    /// **唤醒轮正文收集**：唤醒轮以自己的 `system/init` 开场、以带
    /// `origin.kind=="task-notification"` 的 `result` 收尾。init 打开收集窗口，
    /// 窗口内的内容帧喂共享解析器攒 TextDelta（复用它的流式/整块去重），收尾时整段
    /// 交给 sink 落成一条助手消息。窗口外的杂帧（取消残留等）直接丢弃——它们属于
    /// 已收尾的轮次，攒进正文会污染唤醒消息。
    async fn read_idle_frame(&mut self) -> IdlePump {
        let line = match self.next_line().await {
            ReadStep::Line(line) => line,
            ReadStep::Idle => return IdlePump::Quiet,
            // EOF / 不可恢复错误：停止空闲读（别对着死管道忙等）。会话的真实死因
            // 留给下一轮 RunTurn 的既有失败路径去发现和上报。
            ReadStep::Eof => return IdlePump::Dead,
            // 偶发可恢复错误（EINTR 等）：这次当无事发生；连发不停由调用方计数升级 Dead。
            ReadStep::Fatal(_) => return IdlePump::Hiccup,
        };
        if line.trim().is_empty() {
            return IdlePump::Quiet;
        }
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            // 非 JSON 行在轮间没有展示位，丢弃。
            return IdlePump::Quiet;
        };
        match classify_inbound_frame(&value, false) {
            // can_ask=false ⇒ 一切控制请求都以 error 回复（fail-closed，沉默=挂死）。
            InboundFrame::Reply(reply) => IdlePump::Reply(reply),
            InboundFrame::Ask(_) | InboundFrame::CancelAsk(_) | InboundFrame::Ignore => {
                IdlePump::Quiet
            }
            InboundFrame::Stream => self.classify_idle_stream_frame(&value),
        }
    }

    /// `read_idle_frame` 的 Stream 分支（纯同步）。
    fn classify_idle_stream_frame(&mut self, value: &Value) -> IdlePump {
        use crate::external_agents::stream::claude as claude_stream;
        // 唤醒轮收尾：把窗口里攒下的正文整段交出去，附上模型与真实用量（出处证明）。
        if claude_stream::is_task_notification_result(value) {
            let text = std::mem::take(&mut self.idle_wake_text);
            self.idle_collecting = false;
            let text = text.trim().to_string();
            if text.is_empty() {
                return IdlePump::Quiet;
            }
            return IdlePump::Side(IdleSideEffect::WakeMessage {
                text,
                model: self.handler.resolved_model().map(str::to_string),
                usage: claude_stream::result_usage(value),
            });
        }
        // 任务生命周期帧：入面板注册表（与收集窗口无关，started 帧也可能出现在唤醒轮里
        // ——唤醒轮的模型可以再派新的后台任务）。
        if let Some(event) = claude_stream::background_task_event(value) {
            return IdlePump::Side(IdleSideEffect::Task(event));
        }
        let obj = value.as_object();
        let kind = obj
            .and_then(|o| o.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let subtype = obj
            .and_then(|o| o.get("subtype"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        // 唤醒轮开场：打开收集窗口。init 每轮都发且先于任何正文（实测，见模块头）。
        if kind == "system" && subtype == "init" {
            self.idle_collecting = true;
            self.idle_wake_text.clear();
            return IdlePump::Quiet;
        }
        if self.idle_collecting {
            // 喂共享解析器攒正文：复用它的「流式 delta vs 整块 assistant 帧」去重。
            // 只收 TextDelta；唤醒轮的工具调用/思考在轮间没有承载位，作为 v1 边界丢弃。
            let ClaudeStreamJsonSession {
                handler,
                idle_wake_text,
                ..
            } = self;
            handler.handle_value(value, &mut |event| {
                if let UnifiedAgentEvent::TextDelta { delta } = event {
                    idle_wake_text.push_str(&delta);
                }
            });
        }
        // 窗口外的杂帧（取消残留的迟到帧等）：属于已收尾的轮次，丢弃。
        IdlePump::Quiet
    }

    /// 清掉半截的唤醒轮收集状态。轮次开始前调用：用户消息一旦写入，后续帧都属于
    /// 新轮次，半截唤醒正文没有完整的收尾信号，落一半的消息比不落更糟。
    fn reset_idle_collector(&mut self) {
        self.idle_wake_text.clear();
        self.idle_collecting = false;
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
        if timeout(CLAUDE_SHUTDOWN_TIMEOUT, child.wait())
            .await
            .is_err()
        {
            kill_agent_process_tree(&mut child);
            let _ = child.wait().await;
        }
        if let Some(handle) = stderr_tail {
            let _ = handle.await;
        }
    }
}

/// 轮间空闲读一步的结果（`read_idle_frame`）。
enum IdlePump {
    /// 无事发生（超时 / 空行 / 非 JSON / 不需要动作的帧）。
    Quiet,
    /// 偶发可恢复读错误：调用方计数，连击超限升级 Dead。
    Hiccup,
    /// stdout 已结束/不可恢复：调用方停止空闲读。
    Dead,
    /// claude 在等回复的控制请求（fail-closed 的 error 响应行），调用方写回 stdin。
    Reply(String),
    /// 需要旁路出口处理的副作用（任务事件 / 唤醒轮正文），调用方喂给 sink。
    Side(IdleSideEffect),
}

/// 轮间空闲读产出的副作用，经 `BackgroundTaskSink` 交给宿主（run.rs）处理。
pub enum IdleSideEffect {
    /// 后台任务生命周期事件 → AppState 注册表（面板数据源）。
    Task(UnifiedAgentEvent),
    /// 唤醒轮（后台任务完成后 CLI 自起的一轮）的完整正文 → 落成一条真正的助手消息。
    /// claude 在它自己的会话里确实「通知过了」，不落地的话用户永远看不到那句汇报。
    /// `model`/`usage` 是这条消息的出处证明：不带的话前端只能显示估算的「~N tokens」，
    /// 看着像脚本拼的而不是模型产出（实测被用户质疑过）。
    WakeMessage {
        text: String,
        model: Option<String>,
        usage: Option<crate::chat::model::ModelUsage>,
    },
}

/// 轮间空闲读副作用的旁路出口。actor 里没有 `AppHandle`（会话层不该有），
/// 宿主在建会话时把「upsert 注册表 / 追加唤醒消息」包成闭包递进来。
pub type BackgroundTaskSink = std::sync::Arc<dyn Fn(IdleSideEffect) + Send + Sync>;

/// Spawn the actor task that owns a connected session and serves `SessionCommand`s.
pub fn spawn_claude_stream_session_actor(
    session: ClaudeStreamJsonSession,
) -> mpsc::Sender<SessionCommand> {
    spawn_claude_stream_session_actor_with_sink(session, None)
}

/// 带轮间空闲读的 actor。
///
/// **为什么要空闲读**（2026-08-06 实测）：后台任务在两轮之间完成时，claude 自己起一个
/// 唤醒轮把 `task_notification` + 汇报文字写进 stdout——此前轮间没人读，通知积压到
/// 下一条用户消息才被消费：面板上一个 5 秒的任务能挂着「运行中」几分钟，唤醒轮的
/// 汇报文字还会漏进下一个回答的开头。空闲读只干三件事：回掉必须回复的
/// `control_request`（fail-closed，沉默=挂死）、把任务生命周期事件喂给 `sink`、
/// 其余内容帧有意丢弃（轮间没有承载位；要接唤醒轮的正文得先有「CLI 自起轮次」的
/// 消息模型，那是另一个任务）。
pub fn spawn_claude_stream_session_actor_with_sink(
    mut session: ClaudeStreamJsonSession,
    sink: Option<BackgroundTaskSink>,
) -> mpsc::Sender<SessionCommand> {
    enum ActorStep {
        Cmd(Option<SessionCommand>),
        Idle(IdlePump),
    }
    let (tx, mut rx) = mpsc::channel::<SessionCommand>(8);
    tokio::spawn(async move {
        // stdout 死了就停止空闲读（别对着死管道忙等）；会话的真实死因留给下一轮
        // RunTurn 的既有失败路径去发现和上报，这里不重复造报错出口。
        let mut idle_dead = false;
        let mut idle_hiccups = 0u32;
        loop {
            // select 里**只读**：读是 cancel-safe 的（`Lines::next_line` 官方保证），
            // 写回动作放到 select 之外执行，写到一半被取消会留半行破 JSON。
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
                        IdlePump::Quiet => idle_hiccups = 0,
                        IdlePump::Hiccup => {
                            idle_hiccups += 1;
                            if idle_hiccups >= MAX_RECOVERABLE_READ_ERRORS {
                                idle_dead = true;
                            }
                        }
                        IdlePump::Dead => idle_dead = true,
                        IdlePump::Reply(line) => {
                            idle_hiccups = 0;
                            session.write_control_line(&line).await;
                        }
                        IdlePump::Side(effect) => {
                            idle_hiccups = 0;
                            if let Some(sink) = sink.as_ref() {
                                sink(effect);
                            }
                        }
                    }
                    continue;
                }
                ActorStep::Cmd(cmd) => cmd,
            };
            let Some(cmd) = cmd else { break };
            match cmd {
                SessionCommand::RunTurn {
                    prompt,
                    model,
                    images,
                    events,
                    done,
                    approvals,
                    ..
                } => {
                    // Invariant (A4)：`run_turn` 在返回前发完所有 `event`，mpsc 保序，
                    // 所以调用方在 `done` 之后的 drain 能看到全部事件。`done.send` 永远最后。
                    // 半截的唤醒轮收集先清掉：prompt 一写入，后续帧都属于新轮次。
                    session.reset_idle_collector();
                    let result = session
                        .run_turn(
                            &prompt,
                            model.as_deref(),
                            &images,
                            &events,
                            &mut rx,
                            approvals,
                        )
                        .await;
                    let _ = done.send(result);
                }
                // 轮次之间没有可注入的对象：回 false 让前端把这条留在队列里、
                // 轮末按普通消息发出去（绝不静默吞掉）。
                SessionCommand::Steer { accepted, .. } => {
                    let _ = accepted.send(false);
                }
                SessionCommand::Cancel => {} // 轮次之间没有在跑的轮次
                SessionCommand::StopTask { task_id } => {
                    session.send_stop_task(&task_id).await;
                }
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
        assert!(read_error_is_recoverable(&Error::from(
            ErrorKind::Interrupted
        )));
        assert!(read_error_is_recoverable(&Error::from(
            ErrorKind::WouldBlock
        )));
        assert!(read_error_is_recoverable(&Error::from(ErrorKind::TimedOut)));
        // Windows 的 ERROR_OPERATION_ABORTED（995）——中断挂起的 pipe 读就是这个。
        assert!(read_error_is_recoverable(&Error::from_raw_os_error(
            WINDOWS_ERROR_OPERATION_ABORTED
        )));
        // 非法 UTF-8：tokio 丢掉这一行后 reader 仍可用，进程完好。判成致命的话，一次坏字节
        // 就会把整轮 prompt 重发一遍（工具副作用可能重跑）。
        assert!(read_error_is_recoverable(&Error::from(
            ErrorKind::InvalidData
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
        ] {
            assert!(
                !read_error_is_recoverable(&Error::from(kind)),
                "{kind:?} 不该判为可恢复"
            );
        }
    }

    #[test]
    fn set_model_request_shape_matches_the_control_request_envelope() {
        let line = set_model_request_line("req-9", "opus");
        assert!(line.ends_with('\n'), "必须是一整行");
        let value: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(value["type"], serde_json::json!("control_request"));
        // request_id 在**帧顶层**（与 `interrupt` 一致）；放错层级 = CLI 匹配不到 = 等于没发。
        assert_eq!(value["request_id"], serde_json::json!("req-9"));
        assert_eq!(value["request"]["subtype"], serde_json::json!("set_model"));
        // 载荷必须是**字符串**：changelog 2.1.208 说非字符串的 `set_model` 载荷曾让会话永久挂死。
        assert_eq!(value["request"]["model"], serde_json::json!("opus"));
        assert!(value["request"]["model"].is_string());
    }

    /// ack 只认自己那条 request_id，且必须能区分 success / error ——
    /// 认错了就会把「CLI 不支持」当成「切换成功」，于是用户换了模型却静默跑旧的。
    /// 切档位请求的形状。**字段名是二进制核实的**（CLI 自己构造这条请求时用的就是
    /// `{subtype:"set_permission_mode", mode:…}`）—— 猜错的表现是用户批准了计划、
    /// claude 却还在计划档里，谁都看不出为什么。
    #[test]
    fn set_permission_mode_request_shape_matches_the_control_request_envelope() {
        let line = set_permission_mode_request_line("req-plan", "acceptEdits");
        assert!(line.ends_with('\n'), "必须是一整行");
        let value: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(value["type"], serde_json::json!("control_request"));
        // request_id 在**帧顶层**（与 `set_model` / `interrupt` 一致）。
        assert_eq!(value["request_id"], serde_json::json!("req-plan"));
        assert_eq!(
            value["request"]["subtype"],
            serde_json::json!("set_permission_mode")
        );
        assert_eq!(value["request"]["mode"], serde_json::json!("acceptEdits"));
        assert!(value["request"]["mode"].is_string());
    }

    #[test]
    fn control_response_verdict_matches_only_our_request_id() {
        let ok: Value = serde_json::from_str(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"a"}}"#,
        )
        .unwrap();
        assert_eq!(control_response_verdict(&ok, "a"), Some(true));
        // 别人的 request_id：不是在答我们。
        assert_eq!(control_response_verdict(&ok, "b"), None);

        let err: Value = serde_json::from_str(
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"a","error":"unknown subtype"}}"#,
        )
        .unwrap();
        assert_eq!(control_response_verdict(&err, "a"), Some(false));

        // 不是 control_response 的帧一律 None（正文帧不能被当成 ack）。
        let text: Value =
            serde_json::from_str(r#"{"type":"assistant","message":{"content":[]}}"#).unwrap();
        assert_eq!(control_response_verdict(&text, "a"), None);
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

    /// `stop_task` 的线上形态：`request_id` 在帧顶层（与 `interrupt`/`set_model` 一致），
    /// 载荷 `{subtype:"stop_task", task_id}`。放错层级 = CLI 匹配不到 = 停止静默无效。
    #[test]
    fn stop_task_request_matches_the_control_request_envelope() {
        let line = stop_task_request_line("req-stop", "b2foykvcu");
        assert!(line.ends_with('\n'), "必须是一整行");
        let value: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(value["type"], serde_json::json!("control_request"));
        assert_eq!(value["request_id"], serde_json::json!("req-stop"));
        assert_eq!(value["request"]["subtype"], serde_json::json!("stop_task"));
        assert_eq!(value["request"]["task_id"], serde_json::json!("b2foykvcu"));
        assert!(value["request"]["task_id"].is_string());
    }

    // ---- 帧分流：对不认识的控制请求必须回复（fail-closed）----

    fn frame(raw: &str) -> Value {
        serde_json::from_str(raw).expect("test fixture is valid json")
    }

    fn reply_for(raw: &str) -> Option<String> {
        // `can_ask=false` = 本轮没有宿主可问 ⇒ 一切控制请求都走 fail-closed 的 error 兜底。
        match classify_inbound_frame(&frame(raw), false) {
            InboundFrame::Reply(line) => Some(line),
            _ => None,
        }
    }

    /// 带宿主时的分流结果（用于权限审批那组断言）。
    fn ask_for(raw: &str) -> Option<ApprovalAsk> {
        match classify_inbound_frame(&frame(raw), true) {
            InboundFrame::Ask(ask) => Some(ask),
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
        assert!(
            value.get("request_id").is_none(),
            "顶层不该有 request_id：{value}"
        );
        // 错误文案要能让人看出是哪个子型没实现（会进 CLI 的 tool_result / 诊断）。
        let error = value["response"]["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("can_use_tool"),
            "错误文案应带上子型：{error}"
        );
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
            classify_inbound_frame(
                &frame(r#"{"type":"control_request","request":{"subtype":"can_use_tool"}}"#),
                true
            ),
            InboundFrame::Ignore
        ));
    }

    /// `keep_alive` / `control_response`：**有意不接、也不需要回复**。
    ///
    /// 核实依据（claude 2.1.220 二进制）：`keep_alive` 的 schema 是 `{type:"keep_alive"}`
    /// ——无字段、无 request_id，CLI 自己的两个读取点都是直接跳过，没有任何需要回应的语义。
    #[test]
    fn control_channel_noise_is_ignored_without_a_reply() {
        for raw in [
            r#"{"type":"keep_alive"}"#,
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"kivio-interrupt-1","response":{"still_queued":[]}}}"#,
        ] {
            assert!(
                matches!(
                    classify_inbound_frame(&frame(raw), true),
                    InboundFrame::Ignore
                ),
                "{raw} 应被安全忽略"
            );
        }
    }

    /// `control_cancel_request` = CLI 撤回它先前那条询问。**不回任何东西**（CLI 自己收到这条
    /// 时也只 `abort()`），但必须能把挂起表里那条摘掉 —— 否则取消时的批量拒绝会对一条早已
    /// 作废的询问回响应，而「本轮还有人在等」会永久为真。
    #[test]
    fn a_cancelled_control_request_is_routed_to_the_pending_table_not_answered() {
        match classify_inbound_frame(
            &frame(r#"{"type":"control_cancel_request","request_id":"req-42"}"#),
            true,
        ) {
            InboundFrame::CancelAsk(id) => assert_eq!(id, "req-42"),
            other => panic!("应路由成 CancelAsk，实际是 {:?}", frame_kind(&other)),
        }
        // 没有 id 就无从对应，安全忽略。
        assert!(matches!(
            classify_inbound_frame(&frame(r#"{"type":"control_cancel_request"}"#), true),
            InboundFrame::Ignore
        ));
    }

    fn frame_kind(frame: &InboundFrame) -> &'static str {
        match frame {
            InboundFrame::Stream => "Stream",
            InboundFrame::Ignore => "Ignore",
            InboundFrame::Reply(_) => "Reply",
            InboundFrame::Ask(_) => "Ask",
            InboundFrame::CancelAsk(_) => "CancelAsk",
        }
    }

    // ---- 工具审批：can_use_tool 的问答 ----

    /// **本项功能的核心分流断言**：有宿主可问时，`can_use_tool` 不再回 error，而是变成一条
    /// 待用户答复的询问，且带齐问用户所需的字段。
    ///
    /// fixture 是**真机原文**（claude 2.1.220，`--permission-prompt-tool stdio`
    /// + `--permission-mode default` 让它写文件，2026-07-29 本机实测）。
    #[test]
    fn a_can_use_tool_request_becomes_a_pending_ask_when_a_host_can_answer() {
        let ask = ask_for(
            r#"{"type":"control_request","request_id":"f4d4ddfd-1be4-4630-baec-2b764030870c",
                "request":{"subtype":"can_use_tool","tool_name":"Write","display_name":"Write",
                           "input":{"file_path":"C:\\tmp\\probe.txt","content":"HELLO42"},
                           "description":"probe.txt",
                           "permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}],
                           "tool_use_id":"toolu_015Jc1MjkwjbJL6rsCoaG3q8"}}"#,
        )
        .expect("有宿主时必须变成待答的询问");
        // 回复时要原样回显的那个 id。
        assert_eq!(ask.request_id, "f4d4ddfd-1be4-4630-baec-2b764030870c");
        // 卡片 id 用 claude 的 tool_use_id ⇒ 审批卡与工具卡指向同一次调用。
        assert_eq!(ask.tool_call_id, "toolu_015Jc1MjkwjbJL6rsCoaG3q8");
        // 工具原名**不归一化**：`Write` 是 PascalCase，前端展示要的就是本名。
        assert_eq!(ask.tool_name, "Write");
        assert_eq!(
            ask.input["file_path"],
            serde_json::json!("C:\\tmp\\probe.txt")
        );
        assert!(!ask.requires_user_interaction);
    }

    /// 没有宿主（用户没开审批 / 别的协议）时**必须**退回 fail-closed 的 error 兜底，
    /// 绝不能变成「挂起但没人答」—— 那就是永久挂死。
    #[test]
    fn a_can_use_tool_request_without_a_host_still_gets_the_fail_closed_error() {
        let line = reply_for(
            r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{}}}"#,
        )
        .expect("没有宿主也必须回复");
        let value = frame(line.trim());
        assert_eq!(value["response"]["subtype"], serde_json::json!("error"));
        assert_eq!(value["response"]["request_id"], serde_json::json!("r1"));
    }

    /// `tool_use_id` 在 schema 里是 optional：缺失时卡片 id 回落 `request_id`。
    /// 卡片必须有个稳定 id 才能被答复，宁可与工具卡对不上也不能没有。
    #[test]
    fn a_missing_tool_use_id_falls_back_to_the_request_id() {
        let ask = ask_for(
            r#"{"type":"control_request","request_id":"r7","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"}}}"#,
        )
        .expect("ask");
        assert_eq!(ask.tool_call_id, "r7");
    }

    /// 允许 / 拒绝的线上形态必须与真机实测一致。
    ///
    /// 实测（2026-07-29，claude 2.1.220）：回 `{behavior:"allow"}` 文件真的被创建；
    /// 回 `{behavior:"deny",message}` 文件没建，且 `message` **原样**成为那次调用的
    /// `tool_result`（`is_error:true`），模型据此改口说「写入被拒绝，我没有成功」。
    #[test]
    fn approval_responses_match_the_measured_wire_shape() {
        for (approved, expected_behavior) in [(true, "allow"), (false, "deny")] {
            let line = approval_response_line("req-9", approved, "拒了", None);
            assert!(line.ends_with('\n'), "必须是一整行");
            let value = frame(line.trim());
            assert_eq!(value["type"], serde_json::json!("control_response"));
            // 三件套是 **snake_case**，且 `request_id` 嵌在 `response` 里、不在帧顶层 ——
            // 放错层级 = CLI 匹配不到 = 等于没回。
            assert_eq!(value["response"]["subtype"], serde_json::json!("success"));
            assert_eq!(value["response"]["request_id"], serde_json::json!("req-9"));
            assert!(value.get("request_id").is_none(), "顶层不该有 request_id");
            // 而载荷这一层是 **camelCase**（同一条协议两套命名混用）。
            let payload = &value["response"]["response"];
            assert_eq!(payload["behavior"], serde_json::json!(expected_behavior));
            if approved {
                // 我们不改入参 ⇒ 不发 `updatedInput`，CLI 会沿用原入参。
                assert!(payload.get("message").is_none());
            } else {
                // deny 分支的 `message` 是**必填**（schema `BdE`），而且会变成 tool_result。
                assert_eq!(payload["message"], serde_json::json!("拒了"));
                // `interrupt:true` 会让 CLI abort 整轮 —— 拒一个工具不等于停掉整轮。
                assert_eq!(payload["interrupt"], serde_json::json!(false));
            }
        }
    }

    /// **本项最不能出错的一条断言**：取消时挂起表里的每一条询问都必须被拒掉，一条都不能漏。
    ///
    /// 一条永远不被回复的询问在结构上就是一次永久挂死（CLI 侧的 `pendingRequests` 是个没有
    /// 超时的 Promise，我们的轮次读循环也没有超时）。**这条单测就是那个不变量的红→绿判据**
    /// —— 真机测试证明不了它（见 `PendingApproval` 注释里那段实测边界：注释掉批量拒绝之后
    /// 真机测试仍然通过，因为 CLI 自己的 interrupt 会 abort 掉在飞的请求）。
    #[test]
    fn cancelling_rejects_every_pending_approval_exactly_once() {
        let pending: Vec<PendingApproval> = ["r1", "r2", "r3", "r4", "r5"]
            .iter()
            .map(|id| PendingApproval {
                request_id: (*id).to_string(),
                tool_name: "Bash".to_string(),
            })
            .collect();
        let lines = reject_pending_lines(&pending, "用户中止了本轮");
        assert_eq!(lines.len(), pending.len(), "每条挂起的询问都要有一行答复");

        let mut answered: Vec<String> = Vec::new();
        for line in &lines {
            assert!(line.ends_with('\n'));
            let value = frame(line.trim());
            assert_eq!(value["response"]["subtype"], serde_json::json!("success"));
            assert_eq!(
                value["response"]["response"]["behavior"],
                serde_json::json!("deny"),
                "批量收尾一律拒绝（fail-closed），绝不能默认放行"
            );
            answered.push(
                value["response"]["request_id"]
                    .as_str()
                    .expect("request_id 必须是字符串")
                    .to_string(),
            );
        }
        answered.sort();
        let mut expected: Vec<String> = pending.iter().map(|p| p.request_id.clone()).collect();
        expected.sort();
        assert_eq!(answered, expected, "有询问被漏掉或被重复回复 ⇒ 会永久挂死");
    }

    /// 空表不产生任何写入（避免往 stdin 灌无意义的行）。
    #[test]
    fn rejecting_an_empty_pending_table_writes_nothing() {
        assert!(reject_pending_lines(&[], "x").is_empty());
    }

    /// 两个交互工具都必须问到用户；CLI 将来新增的交互工具仍当场拒
    /// （给一张点了也没用的卡片比诚实拒掉更糟）。
    #[test]
    fn interactive_tools_are_denied_without_bothering_the_user() {
        // `AskUserQuestion`：宿主把它转成 Kivio 的问用户卡片，
        // 选项经 `ApprovalDecision::updated_input` 回给 CLI（官方 `allow + updatedInput`）。
        let ask_user = ask_for(
            r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{},"requires_user_interaction":true}}"#,
        )
        .expect("ask");
        assert!(
            approval_verdict(&ask_user).is_ok(),
            "AskUserQuestion 必须问用户，不能当场拒"
        );

        // `ExitPlanMode`：走普通审批卡，批准时经 `ApprovalDecision::set_permission_mode`
        // 先把 CLI 切出计划档再放行。
        let plan = ask_for(
            r#"{"type":"control_request","request_id":"r3","request":{"subtype":"can_use_tool","tool_name":"ExitPlanMode","input":{"plan":"1. 改 a.rs"},"requires_user_interaction":true}}"#,
        )
        .expect("ask");
        assert!(
            approval_verdict(&plan).is_ok(),
            "ExitPlanMode 必须问用户 —— 计划批不批是用户的决定"
        );

        // 认不出来的交互工具（CLI 将来新增的）仍当场拒。
        let future = ask_for(
            r#"{"type":"control_request","request_id":"r4","request":{"subtype":"can_use_tool","tool_name":"SomeFutureDialog","input":{},"requires_user_interaction":true}}"#,
        )
        .expect("ask");
        assert!(approval_verdict(&future).is_err());

        // `EnterPlanMode`：claude 自己要求进入计划档。放行就够（它的实现里自己切档），
        // 但要不要进只读档是用户的决定 ⇒ 必须问。
        let enter_plan = ask_for(
            r#"{"type":"control_request","request_id":"r5","request":{"subtype":"can_use_tool","tool_name":"EnterPlanMode","input":{},"requires_user_interaction":true}}"#,
        )
        .expect("ask");
        assert!(
            approval_verdict(&enter_plan).is_ok(),
            "EnterPlanMode 必须问用户 —— 把一次生成中途变成只读是用户的决定"
        );

        let ordinary = ask_for(
            r#"{"type":"control_request","request_id":"r2","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"}}}"#,
        )
        .expect("ask");
        assert!(approval_verdict(&ordinary).is_ok(), "普通工具必须问用户");
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
                matches!(
                    classify_inbound_frame(&frame(raw), true),
                    InboundFrame::Stream
                ),
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
            step(
                &mut budget,
                r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"上一轮的半截话"}}}"#
            ),
            "上一轮的正文漏进了下一轮的回答"
        );
        assert!(
            step(
                &mut budget,
                r#"{"type":"result","subtype":"error_during_execution","terminal_reason":"aborted_streaming"}"#
            ),
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
        assert!(!step(
            &mut budget,
            r#"{"type":"result","subtype":"success"}"#
        ));
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
        assert!(!is_missing_session_error(
            "Not logged in · Please run /login"
        ));
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
        live_args_with_sandbox(session_id, model, None)
    }

    /// 同上，但指定权限档位。`Some("default")` = 用户在权限胶囊里选了「每次确认」，
    /// argv 会带上 `--permission-prompt-tool stdio`（见
    /// `defs::claude::claude_permission_prompt_args`）。
    fn live_args_with_sandbox(
        session_id: &str,
        model: Option<&str>,
        sandbox: Option<&str>,
    ) -> Vec<String> {
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
                sandbox: sandbox.map(str::to_string),
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
        one_turn_with_approvals(control, prompt, cancel_after_text, None).await
    }

    /// 同上，但带一条审批通道。`approvals` 的另一半（询问接收端 + 答复发送端）由调用方持有。
    async fn one_turn_with_approvals(
        control: &mpsc::Sender<SessionCommand>,
        prompt: &str,
        cancel_after_text: bool,
        approvals: Option<ApprovalBridge>,
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
                approvals,
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
        match ClaudeStreamJsonSession::connect(&bin, &live_args(session_id, model), &workdir).await
        {
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
    ///
    /// 关停**必须在超时内完成**，这条是断言不是尽力而为：`close()` 少一行 `drop(stdin)`
    /// 就会让每次关停白等满 `CLAUDE_SHUTDOWN_TIMEOUT` 再被杀。那个 bug 犯过一次，当初是靠
    /// 「测试耗时 53s → 169s」发现的 —— 被删掉的 `claude_persist_probe_tests` 里有条真断言，
    /// 搬过来时退化成了 `let _ =`，回归会重新变成静默通过。
    async fn close_and_cleanup(control: mpsc::Sender<SessionCommand>, workdir: &std::path::Path) {
        let _ = control.send(SessionCommand::Close).await;
        let closed = timeout(CLAUDE_SHUTDOWN_TIMEOUT, control.closed()).await;
        let _ = std::fs::remove_dir_all(workdir);
        assert!(
            closed.is_ok(),
            "会话必须在 {CLAUDE_SHUTDOWN_TIMEOUT:?} 内关停（关 stdin 后 claude 自己退出）；\
             超时通常意味着关停路径漏了 drop(stdin)，只能靠 kill 兜底"
        );
    }

    /// 跑一轮并**指定模型**（走 `set_model` 那条路）。
    async fn one_turn_with_model(
        control: &mpsc::Sender<SessionCommand>,
        prompt: &str,
        model: &str,
    ) -> TurnOutput {
        let (etx, mut erx) = mpsc::channel::<UnifiedAgentEvent>(256);
        let (dtx, drx) = oneshot::channel();
        control
            .send(SessionCommand::RunTurn {
                prompt: prompt.to_string(),
                model: Some(model.to_string()),
                reasoning: None,
                images: vec![],
                events: etx,
                done: dtx,
                approvals: None,
            })
            .await
            .expect("actor alive");
        let mut text = String::new();
        let result = drx
            .await
            .unwrap_or_else(|_| Err("actor dropped".to_string()));
        while let Ok(event) = erx.try_recv() {
            if let UnifiedAgentEvent::TextDelta { delta } = event {
                text.push_str(&delta);
            }
        }
        TurnOutput { text, result }
    }

    /// **`allow + updatedInput` 的真机判据**：claude 用 `AskUserQuestion` 反问，我们把用户
    /// 选的选项从**同一条** `can_use_tool` 通道回过去，它得能接受并据此继续。
    ///
    /// 这条测试是 `updatedInput` 那个形状唯一能证伪的地方（官方文档给了单选的例子，多选的
    /// 分隔符是我们自己定的）。形状不对的表现：CLI 把这次调用当失败 / 那一轮报错。
    #[tokio::test]
    #[ignore = "requires live claude login + network"]
    async fn live_ask_user_question_accepts_our_updated_input() {
        live_ask_user_round_trip("default").await;
    }

    /// 同一条往返，跑在**默认档**（「完全」= `bypassPermissions`）上。
    ///
    /// 单测只能证明 argv 带上了 flag、以及带 flag 的 init 里有 `AskUserQuestion`
    /// （`defs::claude::claude_permission_prompt_args` 的实测矩阵）。**它证明不了**这一档下
    /// CLI 真的会为 `AskUserQuestion` 发 `can_use_tool` —— bypass 的语义是「咨询回调之前就
    /// 放行」，若它对这个工具也照样短路，模型拿到的就是一份没有 answers 的原始入参。
    /// 那正是这条测试要证伪的东西：跑绿了才说明默认档的问用户是真的通的。
    #[tokio::test]
    #[ignore = "requires live claude login + network"]
    async fn live_ask_user_question_works_in_the_default_full_access_mode() {
        live_ask_user_round_trip(crate::external_agents::defs::claude::DEFAULT_PERMISSION_MODE)
            .await;
    }

    async fn live_ask_user_round_trip(sandbox: &str) {
        let Some(bin) = resolve_binary(&CLAUDE_AGENT_DEF).await else {
            eprintln!("SKIP: 本机没有可用的 claude CLI");
            return;
        };
        let session_id = Uuid::new_v4().to_string();
        let workdir = std::env::temp_dir().join(format!("kivio-claude-askuser-{session_id}"));
        std::fs::create_dir_all(&workdir).expect("create workdir");

        // `AskUserQuestion` 只在带 `--permission-prompt-tool stdio` 时才出现在工具表里
        // （档位无关，见 `defs::claude::claude_permission_prompt_args` 的实测矩阵）。
        let args = live_args_with_sandbox(&session_id, None, Some(sandbox));
        let Ok(session) = ClaudeStreamJsonSession::connect(&bin, &args, &workdir).await else {
            eprintln!("SKIP: 连接失败（未登录 / 网络？）");
            let _ = std::fs::remove_dir_all(&workdir);
            return;
        };
        let control = spawn_claude_stream_session_actor(session);

        let (ask_tx, mut ask_rx) = mpsc::channel::<ApprovalAsk>(8);
        let (dec_tx, dec_rx) = mpsc::channel(8);
        let bridge = ApprovalBridge {
            requests: ask_tx,
            decisions: dec_rx,
        };
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let picked = std::sync::Arc::new(std::sync::Mutex::new(Option::<String>::None));
        let asked_for_task = asked.clone();
        let picked_for_task = picked.clone();
        // 扮演宿主 + 用户：`AskUserQuestion` 一律选**第一个选项**，其余工具照常放行。
        let answerer = tokio::spawn(async move {
            while let Some(ask) = ask_rx.recv().await {
                eprintln!("  ask: tool={} input={}", ask.tool_name, ask.input);
                asked_for_task
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(ask.tool_name.clone());
                let updated_input = if is_ask_user_question(&ask.tool_name) {
                    let questions = ask
                        .input
                        .get("questions")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let mut answers = serde_json::Map::new();
                    for question in &questions {
                        let Some(text) = question.get("question").and_then(|v| v.as_str()) else {
                            continue;
                        };
                        let Some(label) = question
                            .get("options")
                            .and_then(|v| v.as_array())
                            .and_then(|options| options.first())
                            .and_then(|option| option.get("label"))
                            .and_then(|v| v.as_str())
                        else {
                            continue;
                        };
                        *picked_for_task.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(label.to_string());
                        answers.insert(text.to_string(), json!(label));
                    }
                    Some(json!({ "questions": questions, "answers": answers }))
                } else {
                    None
                };
                let sent = dec_tx
                    .send(crate::external_agents::session::live::ApprovalDecision {
                        request_id: ask.request_id,
                        approved: true,
                        updated_input,
                        set_permission_mode: None,
                    })
                    .await;
                if sent.is_err() {
                    break;
                }
            }
        });

        let out = one_turn_with_approvals(
            &control,
            "Use the AskUserQuestion tool to ask me whether I prefer tea or coffee              (exactly two options, single select). After I answer, reply with one short              sentence naming the drink I picked.",
            false,
            Some(bridge),
        )
        .await;
        answerer.abort();
        let asked = asked.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let picked = picked.lock().unwrap_or_else(|e| e.into_inner()).clone();
        eprintln!(
            "result={:?}
asked={asked:?}
picked={picked:?}
text={}",
            out.result,
            out.text.trim()
        );

        if out.result.is_err() {
            eprintln!("SKIP: 这一轮失败（未登录 / 网络？）：{:?}", out.result);
            close_and_cleanup(control, &workdir).await;
            return;
        }
        if !asked.iter().any(|name| is_ask_user_question(name)) {
            eprintln!(
                "SKIP: 这一轮模型没有用 AskUserQuestion（提示词没引导住，不是形状问题）：{asked:?}"
            );
            close_and_cleanup(control, &workdir).await;
            return;
        }
        let picked = picked.expect("既然问了，就该解析出选项 label");
        assert!(
            out.text.to_lowercase().contains(&picked.to_lowercase()),
            "claude 没有采纳我们回的 updatedInput（选的是 {picked:?}）—— 形状多半不对，             回答是：{:?}",
            out.text
        );

        close_and_cleanup(control, &workdir).await;
    }

    /// **批准计划的真机判据**：计划档下让 claude 提一个改文件的计划 → 它调 `ExitPlanMode`
    /// → 我们批准并经 `set_permission_mode` 把它切到 `acceptEdits` → 它真的把文件建出来。
    ///
    /// 这条测试是 `set_permission_mode` 那个 wire 形状唯一能证伪的地方（字段名是从二进制里
    /// 读出来的，不是文档）。切档失败的表现：文件没建出来，且输出里带我们那条警告。
    #[tokio::test]
    #[ignore = "requires live claude login + network"]
    async fn live_plan_approval_switches_the_cli_out_of_plan_mode() {
        let Some(bin) = resolve_binary(&CLAUDE_AGENT_DEF).await else {
            eprintln!("SKIP: 本机没有可用的 claude CLI");
            return;
        };
        let session_id = Uuid::new_v4().to_string();
        let workdir = std::env::temp_dir().join(format!("kivio-claude-plan-{session_id}"));
        std::fs::create_dir_all(&workdir).expect("create workdir");
        let target = workdir.join("plan-proof.txt");

        let args = live_args_with_sandbox(&session_id, None, Some("plan"));
        let Ok(session) = ClaudeStreamJsonSession::connect(&bin, &args, &workdir).await else {
            eprintln!("SKIP: 连接失败（未登录 / 网络？）");
            let _ = std::fs::remove_dir_all(&workdir);
            return;
        };
        let control = spawn_claude_stream_session_actor(session);

        let (ask_tx, mut ask_rx) = mpsc::channel::<ApprovalAsk>(8);
        let (dec_tx, dec_rx) = mpsc::channel(8);
        let bridge = ApprovalBridge {
            requests: ask_tx,
            decisions: dec_rx,
        };
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let asked_for_task = asked.clone();
        // 扮演宿主 + 用户：批准一切；`ExitPlanMode` 额外带上切档位（生产代码同款）。
        let answerer = tokio::spawn(async move {
            while let Some(ask) = ask_rx.recv().await {
                eprintln!("  ask: tool={}", ask.tool_name);
                asked_for_task
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(ask.tool_name.clone());
                let set_permission_mode =
                    is_exit_plan_mode(&ask.tool_name).then(|| "acceptEdits".to_string());
                let sent = dec_tx
                    .send(crate::external_agents::session::live::ApprovalDecision {
                        request_id: ask.request_id,
                        approved: true,
                        updated_input: None,
                        set_permission_mode,
                    })
                    .await;
                if sent.is_err() {
                    break;
                }
            }
        });

        let out = one_turn_with_approvals(
            &control,
            "Propose a one-step plan to create a file named plan-proof.txt containing the \
             single word ok in the current directory, then call ExitPlanMode. Once the plan \
             is approved, create that file.",
            false,
            Some(bridge),
        )
        .await;
        answerer.abort();
        let asked = asked.lock().unwrap_or_else(|e| e.into_inner()).clone();
        eprintln!(
            "result={:?}\nasked={asked:?}\ntext={}",
            out.result,
            out.text.trim()
        );

        if out.result.is_err() {
            eprintln!("SKIP: 这一轮失败（未登录 / 网络？）：{:?}", out.result);
            close_and_cleanup(control, &workdir).await;
            return;
        }
        if !asked.iter().any(|name| is_exit_plan_mode(name)) {
            eprintln!("SKIP: 这一轮模型没有用 ExitPlanMode（提示词没引导住）：{asked:?}");
            close_and_cleanup(control, &workdir).await;
            return;
        }
        assert!(
            !out.text.contains("切到"),
            "切档位被 CLI 拒了（形状多半不对）：{}",
            out.text
        );
        let created = target.exists();
        close_and_cleanup(control, &workdir).await;
        assert!(
            created,
            "批准了计划但文件没建出来 —— claude 多半还卡在计划档里（set_permission_mode 没生效）"
        );
    }

    /// **多选答案的分隔符判据**（Issue 5）：官方文档只给了单选的例子，我们用 `", "` 拼串。
    ///
    /// 这条真机测试就是为了不再靠猜：让 claude 发一个 `multiSelect` 的问题，我们把**两个**
    /// 选项拼成一个字符串回过去，看它认不认。它若不认，表现是那次调用失败 / 模型答不出我们
    /// 选的两项 —— 那就说明该换成数组或别的分隔符。
    #[tokio::test]
    #[ignore = "requires live claude login + network"]
    async fn live_ask_user_question_accepts_a_multi_select_answer() {
        let Some(bin) = resolve_binary(&CLAUDE_AGENT_DEF).await else {
            eprintln!("SKIP: 本机没有可用的 claude CLI");
            return;
        };
        let session_id = Uuid::new_v4().to_string();
        let workdir = std::env::temp_dir().join(format!("kivio-claude-multi-{session_id}"));
        std::fs::create_dir_all(&workdir).expect("create workdir");
        let args = live_args_with_sandbox(&session_id, None, Some("default"));
        let Ok(session) = ClaudeStreamJsonSession::connect(&bin, &args, &workdir).await else {
            eprintln!("SKIP: 连接失败（未登录 / 网络？）");
            let _ = std::fs::remove_dir_all(&workdir);
            return;
        };
        let control = spawn_claude_stream_session_actor(session);

        let (ask_tx, mut ask_rx) = mpsc::channel::<ApprovalAsk>(8);
        let (dec_tx, dec_rx) = mpsc::channel(8);
        let bridge = ApprovalBridge {
            requests: ask_tx,
            decisions: dec_rx,
        };
        let saw_multi = std::sync::Arc::new(std::sync::Mutex::new(false));
        let picked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let saw_multi_task = saw_multi.clone();
        let picked_task = picked.clone();
        let answerer = tokio::spawn(async move {
            while let Some(ask) = ask_rx.recv().await {
                eprintln!("  ask: tool={} input={}", ask.tool_name, ask.input);
                let updated_input = if is_ask_user_question(&ask.tool_name) {
                    let questions = ask
                        .input
                        .get("questions")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let mut answers = serde_json::Map::new();
                    for question in &questions {
                        let Some(text) = question.get("question").and_then(|v| v.as_str()) else {
                            continue;
                        };
                        if question
                            .get("multiSelect")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                        {
                            *saw_multi_task.lock().unwrap_or_else(|e| e.into_inner()) = true;
                        }
                        // 取前两个选项 —— 这正是生产代码 `ask_user::encode_claude` 的拼法。
                        let labels: Vec<String> = question
                            .get("options")
                            .and_then(|v| v.as_array())
                            .map(|options| {
                                options
                                    .iter()
                                    .take(2)
                                    .filter_map(|o| {
                                        o.get("label").and_then(|v| v.as_str()).map(str::to_string)
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        if labels.is_empty() {
                            continue;
                        }
                        *picked_task.lock().unwrap_or_else(|e| e.into_inner()) = labels.clone();
                        answers.insert(text.to_string(), json!(labels.join(", ")));
                    }
                    Some(json!({ "questions": questions, "answers": answers }))
                } else {
                    None
                };
                let sent = dec_tx
                    .send(crate::external_agents::session::live::ApprovalDecision {
                        request_id: ask.request_id,
                        approved: true,
                        updated_input,
                        set_permission_mode: None,
                    })
                    .await;
                if sent.is_err() {
                    break;
                }
            }
        });

        let out = one_turn_with_approvals(
            &control,
            "Use the AskUserQuestion tool to ask me which programming languages I like,              with multiSelect enabled and at least three options (Rust, Python, Go).              After I answer, list back every language I picked, separated by commas.",
            false,
            Some(bridge),
        )
        .await;
        answerer.abort();
        let saw_multi = *saw_multi.lock().unwrap_or_else(|e| e.into_inner());
        let picked = picked.lock().unwrap_or_else(|e| e.into_inner()).clone();
        eprintln!(
            "result={:?}
multiSelect={saw_multi}
picked={picked:?}
text={}",
            out.result,
            out.text.trim()
        );

        if out.result.is_err() {
            eprintln!("SKIP: 这一轮失败（未登录 / 网络？）：{:?}", out.result);
            close_and_cleanup(control, &workdir).await;
            return;
        }
        if !saw_multi || picked.len() < 2 {
            eprintln!(
                "SKIP: 模型没发出 multiSelect 的问题（提示词没引导住，不是分隔符问题）：                 multi={saw_multi} picked={picked:?}"
            );
            close_and_cleanup(control, &workdir).await;
            return;
        }
        let text = out.text.to_lowercase();
        let missing: Vec<&String> = picked
            .iter()
            .filter(|label| !text.contains(&label.to_lowercase()))
            .collect();
        assert!(
            missing.is_empty(),
            "claude 没把我们回的多选项全部认下来（缺 {missing:?}）—— `\", \"` 这个分隔符多半不对，             该改成数组或别的形状。回答是：{:?}",
            out.text
        );

        close_and_cleanup(control, &workdir).await;
    }

    /// **`set_model` 的真机判据**：同一个常驻进程里换模型，既换得动、又不丢上下文。
    ///
    /// 这条测试是这批改动里唯一能证伪 `set_model` wire 形状的地方 —— 字段名是推断的
    /// （见 `set_model_request_line`）。形状错的话 `apply_model_change` 会拿到 error/超时
    /// 并返回 `NEEDS_RECONNECT`，这里表现为**第二轮直接失败**（本测试直接驱动会话，
    /// 没有 `run.rs` 那层换进程兜底）—— 也就是说它红就是形状错了。
    #[tokio::test]
    #[ignore = "requires live claude login + network"]
    async fn live_switching_the_model_mid_session_keeps_the_context() {
        let session_id = uuid::Uuid::new_v4().to_string();
        let Some((control, workdir)) = connect_live(&session_id, Some("sonnet")).await else {
            return;
        };

        let first = one_turn(
            &control,
            "Answer with one word only. Remember the number 4242.",
            false,
        )
        .await;
        assert!(
            first.result.is_ok(),
            "第 1 轮应正常完成: {:?}",
            first.result
        );

        // 换模型 —— 走会话内 `set_model`，同一个进程。
        let second = one_turn_with_model(
            &control,
            "What number did I ask you to remember? Reply with just the number.",
            "opus",
        )
        .await;
        eprintln!(
            "switch-model turn: result={:?} text={:?}",
            second.result, second.text
        );
        assert!(
            second.result.is_ok(),
            "换模型那一轮失败了 —— 多半是 `set_model` 的 wire 形状猜错了（见              `set_model_request_line` 的注释）。修法：读 ack 的 error 文案，按它改字段名: {:?}",
            second.result
        );
        assert!(
            second.text.contains("4242"),
            "换模型把上下文弄丢了（应该是同一个常驻进程、同一个会话）: {:?}",
            second.text
        );

        close_and_cleanup(control, &workdir).await;
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

    /// 空闲不会把进程弄死：35s 没有任何输入之后，同一个会话仍然响应。
    ///
    /// 若这条红了，说明 CLI 会自行超时退出 ⇒ 常驻方案需要心跳（目前刻意没有心跳）。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real-machine: spawns the installed claude CLI, needs login, costs tokens"]
    async fn live_persistent_session_survives_an_idle_gap() {
        let session_id = Uuid::new_v4().to_string();
        let Some((control, workdir)) = connect_live(&session_id, None).await else {
            return;
        };

        let warmup = one_turn(&control, "Reply with just the word READY.", false).await;
        if warmup.result.is_err() {
            eprintln!("SKIP: 热身轮失败（未登录 / 网络？）：{:?}", warmup.result);
            close_and_cleanup(control, &workdir).await;
            return;
        }

        tokio::time::sleep(Duration::from_secs(35)).await;

        let after = one_turn(&control, "Reply with just the word DONE.", false).await;
        eprintln!("post-idle: {}", after.text.trim());
        assert!(
            after.result.is_ok(),
            "空闲 35s 后这一轮失败 ⇒ 进程自己超时退出了：{:?}",
            after.result
        );
        assert!(
            after.text.to_uppercase().contains("DONE"),
            "空闲后回答是 {:?}",
            after.text
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
        let Ok(session) = ClaudeStreamJsonSession::connect(&bin, &first_args, &workdir).await
        else {
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
        assert!(after.result.is_ok(), "重连后无法回复：{:?}", after.result);
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
        assert!(dead_args
            .windows(2)
            .any(|w| w == ["--resume", dead_id.as_str()]));
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
        eprintln!(
            "after downgrade: result={outcome:?} text={:?}",
            turn.text.trim()
        );
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

    // ---- 工具审批的真机验收 ----

    /// 开一条审批通道，并把它的宿主半边（询问接收端 + 答复发送端）返给调用方。
    fn live_approval_bridge() -> (
        ApprovalBridge,
        mpsc::Receiver<ApprovalAsk>,
        mpsc::Sender<crate::external_agents::session::live::ApprovalDecision>,
    ) {
        let (ask_tx, ask_rx) = mpsc::channel::<ApprovalAsk>(8);
        let (dec_tx, dec_rx) = mpsc::channel(8);
        (
            ApprovalBridge {
                requests: ask_tx,
                decisions: dec_rx,
            },
            ask_rx,
            dec_tx,
        )
    }

    /// 跑一轮，并对收到的每条权限询问按 `approve` 作答。返回 (正文, 终态, 被问过的工具名)。
    async fn one_turn_answering(
        control: &mpsc::Sender<SessionCommand>,
        prompt: &str,
        approve: bool,
    ) -> (TurnOutput, Vec<String>) {
        let (bridge, mut ask_rx, dec_tx) = live_approval_bridge();
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let asked_for_task = asked.clone();
        // 独立任务扮演「宿主 + 用户」：收到询问立刻按 `approve` 回话。
        let answerer = tokio::spawn(async move {
            while let Some(ask) = ask_rx.recv().await {
                eprintln!(
                    "  ask: tool={} request_id={} tool_use_id={} input={}",
                    ask.tool_name, ask.request_id, ask.tool_call_id, ask.input
                );
                asked_for_task
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(ask.tool_name.clone());
                let sent = dec_tx
                    .send(crate::external_agents::session::live::ApprovalDecision {
                        request_id: ask.request_id,
                        approved: approve,
                        updated_input: None,
                        set_permission_mode: None,
                    })
                    .await;
                if sent.is_err() {
                    break;
                }
            }
        });
        let out = one_turn_with_approvals(control, prompt, false, Some(bridge)).await;
        answerer.abort();
        let asked = asked.lock().unwrap_or_else(|e| e.into_inner()).clone();
        (out, asked)
    }

    /// **工具审批的核心验收**：单测只能证明我们拼对了 JSON —— 证明不了**真实的 CLI 会照做**。
    /// 所以这条对真实二进制断言两件事，每件都用一个可证伪的文件系统事实：
    ///
    /// 1. 答 `allow` ⇒ 文件**真的被创建**且内容正确；
    /// 2. 答 `deny`  ⇒ 文件**没有**被创建，且 claude 的回答里说明了它被拒。
    ///
    /// 前置条件（缺一条这个功能就是死的，所以每条都断言而不是假设）：
    /// - argv 必须带 `--permission-prompt-tool stdio` —— 不带的话 CLI 一条 `control_request`
    ///   都不发，权限被它自己直接拒（2026-07-29 本机对照实测）；
    /// - 权限档位必须是 `default` —— `bypassPermissions` 会在咨询我们**之前**就放行一切。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real-machine: spawns the installed claude CLI, needs login, costs tokens"]
    async fn live_tool_approval_allow_really_runs_the_tool_and_deny_really_blocks_it() {
        let Some(bin) = resolve_binary(&CLAUDE_AGENT_DEF).await else {
            eprintln!("SKIP: 本机没有可用的 claude CLI");
            return;
        };

        for (approve, label) in [(true, "allow"), (false, "deny")] {
            let session_id = Uuid::new_v4().to_string();
            let workdir = std::env::temp_dir().join(format!("kivio-claude-approve-{session_id}"));
            std::fs::create_dir_all(&workdir).expect("create workdir");
            let target = workdir.join("approved.txt");

            // 「每次确认」档位 ⇒ argv 带审批 flag。这两条是功能的前提，直接断在这里。
            let args = live_args_with_sandbox(&session_id, None, Some("default"));
            assert!(
                args.windows(2)
                    .any(|w| w == ["--permission-prompt-tool", "stdio"]),
                "argv 没带 --permission-prompt-tool stdio ⇒ CLI 根本不会来问：{args:?}"
            );
            assert!(
                args.windows(2)
                    .any(|w| w == ["--permission-mode", "default"]),
                "bypassPermissions 会在咨询我们之前就放行一切：{args:?}"
            );

            let Ok(session) = ClaudeStreamJsonSession::connect(&bin, &args, &workdir).await else {
                eprintln!("SKIP: 连接失败（未登录 / 网络？）");
                let _ = std::fs::remove_dir_all(&workdir);
                return;
            };
            let control = spawn_claude_stream_session_actor(session);
            let (out, asked) = one_turn_answering(
                &control,
                "Create a file named approved.txt in the current working directory whose entire \
                 content is exactly PERMITTED. Then say in one short sentence whether the write \
                 succeeded.",
                approve,
            )
            .await;
            eprintln!("[{label}] result={:?}", out.result);
            eprintln!("[{label}] asked={asked:?}");
            eprintln!("[{label}] text={}", out.text.trim());

            if out.result.is_err() {
                eprintln!(
                    "SKIP[{label}]: 这一轮失败（未登录 / 网络？）：{:?}",
                    out.result
                );
                close_and_cleanup(control, &workdir).await;
                return;
            }
            // 没被问过 ⇒ 整条链路是死的（要么 flag 没生效，要么分流没把 can_use_tool 接住）。
            assert!(
                !asked.is_empty(),
                "[{label}] CLI 一次都没来问权限 ⇒ 审批链路没接通"
            );

            let exists = target.exists();
            if approve {
                assert!(exists, "[{label}] 答了 allow，但文件没被创建 ⇒ CLI 没照做");
                let content = std::fs::read_to_string(&target).unwrap_or_default();
                assert!(
                    content.contains("PERMITTED"),
                    "[{label}] 文件内容不对：{content:?}"
                );
            } else {
                assert!(
                    !exists,
                    "[{label}] 答了 deny，文件却还是被创建了 ⇒ 拒绝没生效"
                );
                // 拒绝的 message 会原样成为 tool_result，模型应据此改口。
                let text = out.text.to_lowercase();
                assert!(
                    text.contains("deny")
                        || text.contains("denied")
                        || text.contains("not")
                        || text.contains("fail")
                        || out.text.contains("拒"),
                    "[{label}] 被拒之后模型没说明失败，回答是：{:?}",
                    out.text
                );
            }

            close_and_cleanup(control, &workdir).await;
        }
    }

    /// **取消路径的真机回归闸门**：一轮里挂着一条没答的权限询问时点「停止」——
    /// 出口必须是「已取消」，而且**同一个会话下一轮仍然正常返回**。
    ///
    /// **这条测不出批量拒绝**（诚实说明，别把它当那件事的判据）：实测把
    /// `reject_pending` 从 Cancel 分支注释掉之后本条**仍然通过** —— 我们那条 `interrupt`
    /// 会让 CLI 自己 abort 掉在飞的权限请求。它真正守住的是「取消 + 挂起询问」这个组合
    /// 不会把常驻会话搞坏（比如把一条迟到的答复漏进下一轮、或让会话卡在等答复上）。
    /// 批量拒绝那条不变量的判据是单测 `cancelling_rejects_every_pending_approval_exactly_once`。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real-machine: spawns the installed claude CLI, needs login, costs tokens"]
    async fn live_cancelling_with_a_pending_approval_leaves_the_session_usable() {
        let Some(bin) = resolve_binary(&CLAUDE_AGENT_DEF).await else {
            eprintln!("SKIP: 本机没有可用的 claude CLI");
            return;
        };
        let session_id = Uuid::new_v4().to_string();
        let workdir = std::env::temp_dir().join(format!("kivio-claude-approvecancel-{session_id}"));
        std::fs::create_dir_all(&workdir).expect("create workdir");
        let args = live_args_with_sandbox(&session_id, None, Some("default"));

        let Ok(session) = ClaudeStreamJsonSession::connect(&bin, &args, &workdir).await else {
            eprintln!("SKIP: 连接失败（未登录 / 网络？）");
            let _ = std::fs::remove_dir_all(&workdir);
            return;
        };
        let control = spawn_claude_stream_session_actor(session);

        // 第一轮：故意**不答**那条权限询问，等它挂上来之后点「停止」。
        let (bridge, mut ask_rx, _dec_tx) = live_approval_bridge();
        let control_for_cancel = control.clone();
        let canceller = tokio::spawn(async move {
            // 收到询问 ⇒ 挂着不答 ⇒ 直接取消。这正是最容易死锁的时刻。
            if let Some(ask) = ask_rx.recv().await {
                eprintln!("  got ask (deliberately unanswered): {}", ask.tool_name);
                let _ = control_for_cancel.send(SessionCommand::Cancel).await;
                true
            } else {
                false
            }
        });
        let aborted = one_turn_with_approvals(
            &control,
            "Create a file named never.txt containing NOPE in the current working directory.",
            false,
            Some(bridge),
        )
        .await;
        let asked = canceller.await.unwrap_or(false);
        eprintln!("cancelled-with-pending-approval -> {:?}", aborted.result);
        if !asked {
            eprintln!(
                "SKIP: CLI 没来问权限（未登录 / 网络？）：{:?}",
                aborted.result
            );
            close_and_cleanup(control, &workdir).await;
            return;
        }
        let cancel_outcome = aborted
            .result
            .as_ref()
            .err()
            .map(String::as_str)
            .unwrap_or("");
        assert!(
            cancel_outcome == "cancelled" || cancel_outcome == CANCELLED_SESSION_LOST,
            "挂着权限询问时取消，出口不是「取消」：{cancel_outcome:?}"
        );

        // **验收点**：会话没被那条未答的询问卡死，下一轮照常回话。
        let after = one_turn(&control, "Reply with exactly the word ALIVE.", false).await;
        eprintln!("post-cancel turn: {}", after.text.trim());
        assert!(
            after.result.is_ok(),
            "取消时有挂起的权限询问 ⇒ 会话死锁了（这就是漏掉批量拒绝的症状）：{:?}",
            after.result
        );
        assert!(
            after.text.to_uppercase().contains("ALIVE"),
            "下一轮没正常回答：{:?}",
            after.text
        );

        close_and_cleanup(control, &workdir).await;
    }
}
