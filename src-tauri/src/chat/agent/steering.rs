//! 运行中用户插话（steering）：把用户在生成期间发来的消息注入正在跑的这一轮。
//!
//! 对齐 Codex CLI 的 steer —— 不打断、不丢已做的工具工作，只在**下一个轮次边界**（工具跑完、
//! 下次调模型之前）把用户那句话塞进模型历史，模型带着新指示继续。
//!
//! 显示走「合成一条 display-only `ToolCallRecord` + 一个 Tool 段」这条已验证的路子（与内置联网
//! 搜索卡同构，见 `finalize::emit_builtin_web_search_card`）：因此**不新增 segment kind、不动
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

/// 轮首注入：取走信箱里所有插话，逐条推进模型历史并合成一张卡。
///
/// 两处 push 都是必须的：`runtime_messages` 让**本轮**模型看见，`generated_api_messages` 让它
/// 随 assistant 消息落盘（`model_messages_from_openai_messages` 认 `"user"` role），**下一轮回放
/// 不丢**。工具结果之后紧跟一条 user 文本对三种线格式都安全：Anthropic / Gemini 适配器各有
/// `merge_consecutive_*_roles` 把它合进同一个 turn，OpenAI 系天然接受连续 user。
pub(crate) fn inject_steering_messages(env: &LoopEnv<'_>, state: &mut RunState, round: u32) {
    let pending = env
        .host
        .take_steering_messages(&env.config.conversation_id);
    if pending.is_empty() {
        return;
    }
    let ids = env.ids();
    for message in pending {
        state.runtime_messages.push(serde_json::json!({
            "role": "user",
            "content": message.text,
        }));
        state.generated_api_messages.push(serde_json::json!({
            "role": "user",
            "content": message.text,
        }));

        let record = build_steer_record(&message, round);
        env.host
            .emit_tool_record(ids.conversation_id, ids.run_id, ids.message_id, &record);
        let order = state.segment_builder.next_order();
        state
            .segment_builder
            .append_existing_segments(vec![build_steer_segment(order, &record.id, round)]);
        state.tool_records.push(record);
    }
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
}
