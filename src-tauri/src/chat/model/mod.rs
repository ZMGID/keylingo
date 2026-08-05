//! Provider-agnostic Chat model contracts and provider adapters.
//!
//! Runtime code should exchange `GenerateRequest`, `GenerateOutput`, and `StreamPart`.
//! Provider-specific JSON belongs inside this module's adapters.

pub mod anthropic;
pub mod gemini;
pub mod openai;
pub mod responses;
pub mod types;

pub use anthropic::AnthropicMessagesProvider;
pub use gemini::GeminiProvider;
pub use openai::OpenAiChatProvider;
pub use responses::OpenAiResponsesProvider;
pub use types::*;

/// 按供应商 `api_format` 分发到对应适配器的非流式调用。全 crate 统一入口：
/// 聊天 planning、以及翻译/截图/Lens 等旧调用路径都应经由这里，而不是各自 match 协议。
pub(crate) async fn generate_with_chat_provider(
    state: &crate::state::AppState,
    provider: &crate::settings::ModelProvider,
    retry_attempts: usize,
    request: GenerateRequest,
) -> Result<GenerateOutput, ModelError> {
    use crate::settings::ProviderApiFormat;
    match provider.api_format_kind() {
        ProviderApiFormat::OpenAiChat => {
            OpenAiChatProvider::new(state, provider, retry_attempts)
                .generate(request)
                .await
        }
        ProviderApiFormat::AnthropicMessages => {
            AnthropicMessagesProvider::new(state, provider, retry_attempts)
                .generate(request)
                .await
        }
        // xAI 与 OpenAI 的 Responses 是同一条线协议，共用适配器；差异只在请求体清洗，
        // 由 `responses.rs` 内部按 `api_format_kind()` 分叉。
        ProviderApiFormat::OpenAiResponses | ProviderApiFormat::XaiResponses => {
            OpenAiResponsesProvider::new(state, provider, retry_attempts)
                .generate(request)
                .await
        }
        ProviderApiFormat::Gemini => {
            GeminiProvider::new(state, provider, retry_attempts)
                .generate(request)
                .await
        }
    }
}

/// `generate_with_chat_provider` 的流式版本。同为全 crate 统一分发入口。
pub(crate) async fn stream_with_chat_provider(
    state: &crate::state::AppState,
    provider: &crate::settings::ModelProvider,
    retry_attempts: usize,
    request: GenerateRequest,
    sink: &mut (dyn StreamSink + Send),
) -> Result<GenerateOutput, ModelError> {
    use crate::settings::ProviderApiFormat;
    match provider.api_format_kind() {
        ProviderApiFormat::OpenAiChat => {
            OpenAiChatProvider::new(state, provider, retry_attempts)
                .stream(request, sink)
                .await
        }
        ProviderApiFormat::AnthropicMessages => {
            AnthropicMessagesProvider::new(state, provider, retry_attempts)
                .stream(request, sink)
                .await
        }
        // xAI 与 OpenAI 的 Responses 是同一条线协议，共用适配器；差异只在请求体清洗，
        // 由 `responses.rs` 内部按 `api_format_kind()` 分叉。
        ProviderApiFormat::OpenAiResponses | ProviderApiFormat::XaiResponses => {
            OpenAiResponsesProvider::new(state, provider, retry_attempts)
                .stream(request, sink)
                .await
        }
        ProviderApiFormat::Gemini => {
            GeminiProvider::new(state, provider, retry_attempts)
                .stream(request, sink)
                .await
        }
    }
}
