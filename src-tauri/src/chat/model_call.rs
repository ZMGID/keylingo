use serde_json::Value;
use tauri::State;

use crate::chat::model::{
    generate_request_from_openai_messages, generate_with_chat_provider, GenerateOptions,
    GenerateRequestContext,
};
use crate::mcp::ChatToolDefinition;
use crate::settings::{ModelProvider, SessionModel};
use crate::state::AppState;

use super::Conversation;

pub(super) async fn call_chat_completion_message(
    state: &State<'_, AppState>,
    provider: &ModelProvider,
    model: &str,
    messages: Vec<Value>,
    tools: Option<&[ChatToolDefinition]>,
    retry_attempts: usize,
    thinking_enabled: bool,
    conversation_id: Option<&str>,
    message_id: Option<&str>,
    label: &str,
) -> Result<Value, String> {
    let request = generate_request_from_openai_messages(
        model,
        messages,
        tools,
        GenerateOptions {
            thinking_enabled,
            ..GenerateOptions::default()
        },
        label,
        GenerateRequestContext::new(conversation_id, message_id),
    );
    let output = generate_with_chat_provider(state.inner(), provider, retry_attempts, request)
        .await
        .map_err(|err| err.to_string())?;
    Ok(output.to_openai_compatible_message())
}

pub(super) fn format_chat_missing_api_key_error(provider_name: &str) -> String {
    let provider = provider_name.trim();
    if provider.is_empty() {
        "Chat 模型供应商缺少 API Key，请到设置 > 模型中填写后再发送。".to_string()
    } else {
        format!("Chat 模型供应商「{provider}」缺少 API Key，请到设置 > 模型中填写后再发送。")
    }
}

pub(super) fn chat_missing_model_error() -> String {
    "请先为当前 Chat 对话选择模型，或到设置 > AI 客户端配置默认模型。".to_string()
}

/// 混音器未单独指定压缩模型时，用当前会话的 provider/model（顶栏主模型），
/// 而不是设置里的全局 Chat 默认（`effective_chat_model`）。
pub(super) fn session_model_for_conversation(conversation: &Conversation) -> SessionModel<'_> {
    SessionModel {
        provider_id: conversation.provider_id.as_str(),
        model: conversation.model.as_str(),
    }
}
