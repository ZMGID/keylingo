//! 运行中用户插话（steering）：把用户在生成期间发来的消息注入正在跑的这一轮。
//!
//! 对齐 Codex CLI 的 steer —— 不打断、不丢已做的工具工作，只在**下一个轮次边界**（工具跑完、
//! 下次调模型之前）把用户那句话塞进模型历史，模型带着新指示继续。
//!
//! 显示走「合成一条 display-only `ToolCallRecord` + 一个 Tool 段」这条已验证的路子（与内置联网
//! 搜索卡同构，见 `finalize::build_web_search_record`）：因此**不新增 segment kind、不动
//! protocol.rs**，落盘（assistant 消息的 `tool_calls` + `segments`）与实时流两条路都是现成的。

use serde::{Deserialize, Serialize};

use crate::chat::types::{
    ChatMessageSegment, ChatMessageSegmentKind, ChatMessageSegmentPhase, ToolCallRecord,
    ToolCallStatus,
};

use super::loop_::{LoopEnv, RunState};

/// 插话卡的保留工具名。前端按 `source == "native" && name == STEER_TOOL_NAME &&
/// structured.type == "user_steer"` 三条一起认——这张卡渲染成「用户说过的话」，
/// 不能让某个 MCP 服务器的工具结果冒充。
pub const STEER_TOOL_NAME: &str = "user_steer";
pub const FOLLOW_UP_TOOL_NAME: &str = "user_follow_up";

/// 单条插话文本上限。与 `ask_user.rs` 那批 `MAX_*_CHARS` 同一取舍：越界截断而不是报错，
/// 用户已经打完的字不该因为长度被整条丢掉。
pub const MAX_STEER_CHARS: usize = 4000;

/// 一条待注入的用户插话。`id` 由前端生成并原样回到卡片的 `structured_content.steer_id`，
/// 前端据此把队列里那条标记为「已生效」并出队。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SteeringMessage {
    pub id: String,
    pub text: String,
}

impl SteeringMessage {
    /// 规范化：trim + 截断。文本为空则 None（空插话没有意义，也不该占一张卡）。
    pub fn new(id: String, text: &str) -> Option<Self> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        let text = if trimmed.chars().count() > MAX_STEER_CHARS {
            trimmed.chars().take(MAX_STEER_CHARS).collect()
        } else {
            trimmed.to_string()
        };
        Some(Self { id, text })
    }
}

/// 轮首注入：**one-at-a-time**（对齐 pi `PendingMessageQueue` 的默认 `QueueMode`）——先把
/// 信箱里新到的插话全部拉进 RunState 本地队列，再只弹**一条**注入本轮。模型逐条应对，
/// 多条排队的插话不会一次灌进同一轮；剩余的由后续轮次边界与 FinalAnswer 边界的
/// [`steering_pending`] 检查保证送达（队列清空前 loop 不收束）。
///
/// 两处 push 都是必须的：`runtime_messages` 让**本轮**模型看见，`generated_api_messages` 让它
/// 随 assistant 消息落盘（`model_messages_from_openai_messages` 认 `"user"` role），**下一轮回放
/// 不丢**。工具结果之后紧跟一条 user 文本对三种线格式都安全：Anthropic / Gemini 适配器各有
/// `merge_consecutive_*_roles` 把它合进同一个 turn，OpenAI 系天然接受连续 user。
pub(crate) fn inject_steering_messages(env: &LoopEnv<'_>, state: &mut RunState, round: u32) {
    state
        .pending_steering
        .extend(env.host.take_steering_messages(&env.config.conversation_id));
    let Some(message) = state.pending_steering.pop_front() else {
        return;
    };
    append_injected_user_turn(
        env,
        state,
        round,
        &message,
        build_steer_record(&message, round),
    );
}

/// FinalAnswer 边界的外层检查（对齐 pi agent-loop 的外层 `while`：agent 本要停下时轮询
/// followUp 队列）：模型给出了终答，但信箱/本地队列里还有没送达的插话——run 不该在用户
/// 话没说完时收束。把信箱新到的拉进本地队列后判空；true ⇒ 调用方把终答落成中间消息并
/// 继续循环。
pub(crate) fn steering_pending(env: &LoopEnv<'_>, state: &mut RunState) -> bool {
    state
        .pending_steering
        .extend(env.host.take_steering_messages(&env.config.conversation_id));
    !state.pending_steering.is_empty()
}

/// 终答边界才取 follow-up 信箱。轮首不取，避免把「下一轮再问」做成「下一步就插进工具循环」。
pub(crate) fn follow_up_pending(env: &LoopEnv<'_>, state: &mut RunState) -> bool {
    state
        .pending_follow_up
        .extend(env.host.take_follow_up_messages(&env.config.conversation_id));
    !state.pending_follow_up.is_empty()
}

/// 终答已被吸收后注入**一条** follow-up（one-at-a-time，与 steer 同一纪律）。
pub(crate) fn inject_follow_up_messages(env: &LoopEnv<'_>, state: &mut RunState, round: u32) {
    let Some(message) = state.pending_follow_up.pop_front() else {
        return;
    };
    append_injected_user_turn(
        env,
        state,
        round,
        &message,
        build_follow_up_record(&message, round),
    );
}

fn append_injected_user_turn(
    env: &LoopEnv<'_>,
    state: &mut RunState,
    round: u32,
    message: &SteeringMessage,
    record: ToolCallRecord,
) {
    let ids = env.ids();
    state.runtime_messages.push(serde_json::json!({
        "role": "user",
        "content": message.text,
    }));
    state.generated_api_messages.push(serde_json::json!({
        "role": "user",
        "content": message.text,
    }));

    env.host
        .emit_tool_record(ids.conversation_id, ids.run_id, ids.message_id, &record);
    let order = state.segment_builder.next_order();
    state
        .segment_builder
        .append_existing_segments(vec![build_steer_segment(order, &record.id, round)]);
    state.tool_records.push(record);
}

/// 把本该收束的终答落成一条**中间** assistant 消息（对齐 pi：终答照常成为历史的一部分，
/// 随后的插话开启新的一轮）。文本/推理段已由 planning 落进 `segment_builder`（时间线顺序
/// 正确），这里只补两本历史账；下一轮 planning 的请求即带着这条 assistant + 新注入的插话。
pub(crate) fn absorb_final_answer(state: &mut RunState, message: serde_json::Value) {
    state.runtime_messages.push(message.clone());
    state.generated_api_messages.push(message);
    // 下一轮 planning 重新决定是否流式收尾；上一轮终答的「已流出」状态不能带过去。
    state.planning_final_streamed = false;
}

/// 插话卡的 `ToolCallRecord`。永远是 Success（它不是一次调用，是一件已经发生的事）。
///
/// 外部 CLI 那条路（`external_agents::run`）也用这一份：同一个工具名、同一个
/// `structured_content` 形状，前端的判据与出队对账两条路共用。
pub fn build_steer_record(message: &SteeringMessage, round: u32) -> ToolCallRecord {
    ToolCallRecord {
        id: format!("steer_{}", message.id),
        name: STEER_TOOL_NAME.to_string(),
        source: "native".to_string(),
        server_id: None,
        arguments: serde_json::json!({ "text": message.text }).to_string(),
        status: ToolCallStatus::Success,
        // 纯文本兜底：不识别 structured 的旧前端仍能看到这句话，而不是一张空卡。
        result_preview: Some(message.text.clone()),
        error: None,
        duration_ms: None,
        started_at: None,
        completed_at: None,
        round,
        sensitive: false,
        artifacts: Vec::new(),
        trace_id: None,
        span_id: None,
        structured_content: Some(serde_json::json!({
            "type": "user_steer",
            "steer_id": message.id,
            "text": message.text,
        })),
    }
}

/// 原生 follow-up 的显示记录。它与 steer 同样渲染成时间线里的用户小气泡，
/// 但保留独立类型，避免把“当前轮次引导”和“轮末追加”混成一种协议语义。
pub fn build_follow_up_record(message: &SteeringMessage, round: u32) -> ToolCallRecord {
    ToolCallRecord {
        id: format!("follow_up_{}", message.id),
        name: FOLLOW_UP_TOOL_NAME.to_string(),
        source: "native".to_string(),
        server_id: None,
        arguments: serde_json::json!({ "text": message.text }).to_string(),
        status: ToolCallStatus::Success,
        result_preview: Some(message.text.clone()),
        error: None,
        duration_ms: None,
        started_at: None,
        completed_at: None,
        round,
        sensitive: false,
        artifacts: Vec::new(),
        trace_id: None,
        span_id: None,
        structured_content: Some(serde_json::json!({
            "type": "user_follow_up",
            "follow_up_id": message.id,
            "text": message.text,
        })),
    }
}

/// 对应的 Tool 段。`step_number=None` 让它与正文段纯按 order 排序（同内置搜索卡的取舍）。
fn build_steer_segment(order: u32, record_id: &str, round: u32) -> ChatMessageSegment {
    ChatMessageSegment {
        id: format!("seg_{order}_tool_{record_id}"),
        kind: ChatMessageSegmentKind::Tool,
        phase: ChatMessageSegmentPhase::ToolLoop,
        order,
        step_number: None,
        round: Some(round),
        text: None,
        tool_call_id: Some(record_id.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_rejects_blank() {
        assert!(SteeringMessage::new("a".into(), "   \n ").is_none());
        let message = SteeringMessage::new("a".into(), "  改用 rg  ").expect("non-blank");
        assert_eq!(message.text, "改用 rg");
        // 截断按字符数（不是字节），CJK 不会被切半个字。
        let long = "字".repeat(MAX_STEER_CHARS + 10);
        let clipped = SteeringMessage::new("b".into(), &long).expect("non-blank");
        assert_eq!(clipped.text.chars().count(), MAX_STEER_CHARS);
    }

    #[test]
    fn follow_up_record_is_not_a_steer_card() {
        let message = SteeringMessage::new("f1".into(), "接着做").expect("non-blank");
        let record = build_follow_up_record(&message, 2);
        assert_eq!(record.name, FOLLOW_UP_TOOL_NAME);
        assert_eq!(
            record.structured_content.as_ref().and_then(|value| value.get("type")),
            Some(&serde_json::json!("user_follow_up"))
        );
        assert_eq!(
            record
                .structured_content
                .as_ref()
                .and_then(|value| value.get("follow_up_id")),
            Some(&serde_json::json!("f1"))
        );
    }
}
