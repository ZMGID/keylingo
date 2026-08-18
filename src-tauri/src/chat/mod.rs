// Chat 模块：AI 客户端核心功能
pub mod agent;
pub mod ask_user;
pub mod attachments;
pub mod commands;
pub mod dsml_tools;
pub mod export;
pub mod gc;
pub mod hooks;
pub mod image_generation;
pub mod knowledge_base;
mod mcp_image_feedback;
pub mod memory;
pub mod model;
pub mod model_metadata;
pub mod plan;
#[cfg(debug_assertions)]
pub mod probe;
pub mod protocol;
pub mod repository;
pub mod request_debug;
pub mod storage;
pub mod sub_agent;
pub mod todo;
pub mod types;
mod vision;

pub use types::*;

// 这里曾经有一个非流式的 `call_chat_completion_message`（`generate_with_chat_provider`）。
// **已删除，不要再加回来**：部分 openai_responses 代理只可靠地服务流式请求，非流式调用报
// "Unknown Responses API error"——压缩、标题总结、辅助视觉先后各被绊过一次。需要「一次拿完整
// 结果」的调用统一用 `chat::agent::planning::call_chat_completion_message_streamed`
// （或要 usage/引用时用 `call_chat_completion_output_with_usage`，它内部也已走流式）。

pub(crate) fn format_chat_missing_api_key_error(provider_name: &str) -> String {
    let provider = provider_name.trim();
    if provider.is_empty() {
        "Chat 模型供应商缺少 API Key，请到设置 > 模型中填写后再发送。".to_string()
    } else {
        format!("Chat 模型供应商「{provider}」缺少 API Key，请到设置 > 模型中填写后再发送。")
    }
}

pub(crate) fn chat_missing_model_error() -> String {
    "请先为当前 Chat 对话选择模型，或到设置 > AI 客户端配置默认模型。".to_string()
}

/// 混音器未单独指定压缩模型时，用当前会话的 provider/model（顶栏主模型），
/// 而不是设置里的全局 Chat 默认（`effective_chat_model`）。
pub(crate) fn session_model_for_conversation(
    conversation: &Conversation,
) -> crate::settings::SessionModel<'_> {
    crate::settings::SessionModel {
        provider_id: conversation.provider_id.as_str(),
        model: conversation.model.as_str(),
    }
}
