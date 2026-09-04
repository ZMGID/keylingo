use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::{engine::general_purpose, Engine as _};
use serde_json::Value;
use tauri::AppHandle;

use crate::api::send_with_failover;
use crate::chat::model_metadata::normalize_model_name;
use crate::mcp::types::{ChatToolArtifact, McpToolCallResult};
use crate::settings::{ModelProvider, ProviderApiFormat};
use crate::state::AppState;

const DEFAULT_SIZE: &str = "auto";
const DEFAULT_QUALITY: &str = "auto";
const MAX_PROMPT_CHARS: usize = 8_000;
const MAX_IMAGE_BYTES: usize = 24 * 1024 * 1024;
const MAX_INPUT_IMAGES: usize = 4;
/// xAI `/images/edits` 的 `images` 数组上限是 3。
const XAI_MAX_EDIT_IMAGES: usize = 3;
pub const IMAGE_GENERATION_TIMEOUT_MS: u64 = 300_000;
const IMAGE_GENERATION_HTTP_TIMEOUT: Duration = Duration::from_millis(IMAGE_GENERATION_TIMEOUT_MS);

/// 出图**端点选择**的唯一运行时枚举（不持久化，不进配置）。`resolve_image_route` 是端点
/// 判定的单一事实源，收敛此前散落的 `uses_*` 子串表；`generate_image_with_provider` 按此三分支
/// 调对应 `generate_with_*`。自愈只在 `Chat` ↔ `ImagesApi` 间摆动，`GeminiNative` 由 api_format 决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageRoute {
    GeminiNative,
    Chat,
    ImagesApi,
}

/// 归一化名字若命中这些 vendor 前缀（OpenRouter 风格 `vendor/model`），走 chat/completions 出图。
const OPENROUTER_CHAT_VENDOR_PREFIXES: [&str; 8] = [
    "black-forest-labs/",
    "bytedance-seed/",
    "google/",
    "microsoft/",
    "openai/gpt-5",
    "recraft/",
    "sourceful/",
    "x-ai/",
];

#[derive(Debug, Clone)]
struct ImageGenerationRequest {
    prompt: String,
    size: String,
    quality: String,
    n: usize,
    input_images: Vec<InputImage>,
}

#[derive(Debug, Clone)]
pub(crate) struct InputImage {
    mime_type: String,
    base64: String,
}

struct GeneratedImage {
    mime_type: String,
    base64: String,
    revised_prompt: Option<String>,
}

pub async fn tool_generate_image(
    app: &AppHandle,
    state: &AppState,
    conversation_id: Option<&str>,
    arguments: &Value,
) -> Result<McpToolCallResult, String> {
    let settings = state.settings_read().clone();
    let conversation = conversation_id.and_then(|conversation_id| {
        crate::chat::storage::load_conversation(app, conversation_id).ok()
    });
    let drafts = conversation_id
        .map(|conversation_id| {
            crate::chat::draft_journal::latest_drafts_for(app, conversation_id)
        })
        .unwrap_or_default();
    let session_ref = conversation
        .as_ref()
        .map(|conversation| crate::settings::SessionModel {
            provider_id: conversation.provider_id.as_str(),
            model: conversation.model.as_str(),
        });
    let (provider_id, model) =
        crate::chat::model_metadata::image_generation_model_for_session(&settings, session_ref)
            .ok_or_else(|| "Mixer image generation model is not configured".to_string())?;
    let provider = settings
        .get_provider(&provider_id)
        .cloned()
        .ok_or_else(|| "Mixer image generation provider is missing".to_string())?;
    let retry_attempts = crate::api::effective_retry_attempts(&settings);
    let input_images =
        collect_mixer_input_images(app, conversation.as_ref(), &drafts, arguments)?;
    generate_image_with_provider(
        state,
        &provider,
        &model,
        arguments,
        &input_images,
        retry_attempts,
        "Mixer image generation",
    )
    .await
}

pub(crate) async fn generate_image_with_provider(
    state: &AppState,
    provider: &ModelProvider,
    model: &str,
    arguments: &Value,
    input_images: &[InputImage],
    retry_attempts: usize,
    operation: &str,
) -> Result<McpToolCallResult, String> {
    let mut request = parse_request(arguments)?;
    reject_excess_reference_images(provider, model, input_images.len())?;
    request.input_images = input_images.to_vec();
    validate_provider(provider)?;

    // 端点选择：先查会话缓存（自愈学到的纠正结果），否则用单一解析器。
    let normalized_model = normalize_model_name(model);
    let cache_key = (provider.id.clone(), normalized_model);
    let cached_route = state
        .image_route_cache
        .lock()
        .ok()
        .and_then(|cache| cache.get(&cache_key).copied());
    let route = cached_route.unwrap_or_else(|| resolve_image_route(provider, model));

    // fallback_text: 模型有时返回纯文字（澄清/拒绝）而不出图——把这段文字透出，
    // 而不是丢弃后抛硬错误。
    let (images, fallback_text) = match call_image_route(
        state,
        provider,
        model,
        &request,
        retry_attempts,
        operation,
        route,
    )
    .await
    {
        Ok(result) => result,
        // 猜错自愈：选定 route 返回端点错配错误 → 换另一端点（Chat↔ImagesApi）重试一次；
        // 成功则记 (provider_id, normalized_model)→route 到会话缓存，下次同模型直达。
        // GeminiNative 无 alternate，不参与摆动。
        Err(err) if is_endpoint_mismatch_error(&err) && alternate_route(route).is_some() => {
            let alt = alternate_route(route).expect("checked by guard");
            let result = call_image_route(
                state,
                provider,
                model,
                &request,
                retry_attempts,
                operation,
                alt,
            )
            .await?;
            if let Ok(mut cache) = state.image_route_cache.lock() {
                cache.insert(cache_key, alt);
            }
            result
        }
        Err(err) => return Err(err),
    };

    if images.is_empty() {
        if let Some(text) = fallback_text.filter(|value| !value.trim().is_empty()) {
            return Ok(McpToolCallResult {
                content: text,
                is_error: false,
                raw: serde_json::json!({
                    "providerId": provider.id,
                    "providerName": provider.name,
                    "model": model,
                    "count": 0,
                }),
                artifacts: Vec::new(),
                structured_content: None,
                follow_up_user_messages: Vec::new(),
            });
        }
        return Err("Image generation response did not include an image".to_string());
    }

    let artifacts = images
        .iter()
        .enumerate()
        .map(|(idx, image)| {
            let extension = extension_for_mime(&image.mime_type);
            let name = format!("generated-image-{}.{}", idx + 1, extension);
            let size_bytes = decoded_base64_len(&image.base64);
            ChatToolArtifact {
                id: None,
                name,
                mime_type: image.mime_type.clone(),
                data_url: format!("data:{};base64,{}", image.mime_type, image.base64),
                size_bytes,
                path: None,
            }
        })
        .collect::<Vec<_>>();

    let mut content = if artifacts.len() == 1 {
        "Generated 1 image.".to_string()
    } else {
        format!("Generated {} images.", artifacts.len())
    };
    for artifact in &artifacts {
        content.push_str(&format!("\n\n![{}]({})", artifact.name, artifact.name));
    }

    Ok(McpToolCallResult {
        content,
        is_error: false,
        raw: serde_json::json!({
            "providerId": provider.id,
            "providerName": provider.name,
            "model": model,
            "count": artifacts.len(),
            "size": request.size,
            "quality": request.quality,
            "revisedPrompts": images
                .iter()
                .filter_map(|image| image.revised_prompt.clone())
                .collect::<Vec<_>>(),
        }),
        artifacts,
        structured_content: None,
        follow_up_user_messages: Vec::new(),
    })
}

/// 按选定 route 调对应 `generate_with_*`，统一返回 (图, 无图时的兜底文字)。
/// `ImagesApi` 无文字兜底（其响应体固定含图或报错）。
async fn call_image_route(
    state: &AppState,
    provider: &ModelProvider,
    model: &str,
    request: &ImageGenerationRequest,
    retry_attempts: usize,
    operation: &str,
    route: ImageRoute,
) -> Result<(Vec<GeneratedImage>, Option<String>), String> {
    match route {
        ImageRoute::GeminiNative => {
            generate_with_gemini_native(state, provider, model, request, retry_attempts, operation)
                .await
        }
        ImageRoute::Chat => {
            generate_with_openrouter_chat(
                state,
                provider,
                model,
                request,
                retry_attempts,
                operation,
            )
            .await
        }
        ImageRoute::ImagesApi => Ok((
            generate_with_images_api(state, provider, model, request, retry_attempts, operation)
                .await?,
            None,
        )),
    }
}

/// **端点选择的单一事实源**。优先级：① `api_format==Gemini` → GeminiNative；
/// ② base_url 含 `openrouter.ai` → Chat；③ 归一化名字启发式 → Chat | ImagesApi。
/// 收敛了旧 `uses_openrouter_chat_image_generation` 的全部判据（openrouter base、
/// api.openai.com / api.x.ai 早退、vendor 前缀表 + 裸名子串表合并为一处，均走归一化名）。
pub(crate) fn resolve_image_route(provider: &ModelProvider, model: &str) -> ImageRoute {
    if provider.api_format_kind() == ProviderApiFormat::Gemini {
        return ImageRoute::GeminiNative;
    }
    if is_openrouter_base_url(&provider.base_url) {
        return ImageRoute::Chat;
    }
    // 官方直连端点（OpenAI / xAI）不走 chat 仿 OpenRouter 出图，交给各自 images API。
    let base_url = provider.base_url.to_ascii_lowercase();
    if base_url.contains("api.openai.com") || base_url.contains("api.x.ai") {
        return ImageRoute::ImagesApi;
    }
    let normalized = normalize_model_name(model);
    if OPENROUTER_CHAT_VENDOR_PREFIXES
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
    {
        return ImageRoute::Chat;
    }
    // 裸名出图模型（通用 /v1 代理不带 vendor 前缀，如 `gemini-3.1-flash-image`）：这些代理
    // 不支持 /images/generations 出这些图，必须走 chat/completions 仿 OpenRouter 出图。
    // 注意：grok-imagine-image 相反——通用代理明确只支持 /v1/images/generations，故不在此列，
    // 交给 ImagesApi（body 变体由 uses_xai_images_api 命中，走 b64_json）。
    if (normalized.contains("gemini") && normalized.contains("image"))
        || normalized.contains("nano-banana")
        || normalized.starts_with("imagen")
    {
        return ImageRoute::Chat;
    }
    ImageRoute::ImagesApi
}

/// 自愈时的另一端点：Chat↔ImagesApi 互为备选；GeminiNative 由 api_format 决定，无备选。
fn alternate_route(route: ImageRoute) -> Option<ImageRoute> {
    match route {
        ImageRoute::Chat => Some(ImageRoute::ImagesApi),
        ImageRoute::ImagesApi => Some(ImageRoute::Chat),
        ImageRoute::GeminiNative => None,
    }
}

/// 端点错配错误识别：provider 明确报「此模型/端点用错」时的短语。命中即触发换端点自愈。
/// 错误串来自 `send_with_failover`（格式 `... Error: {status} - {body}`），故 body 里的短语可见。
fn is_endpoint_mismatch_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    // 覆盖两向真实代理错配错误：
    //   Chat 走错 → "only supported on /v1/images/generations and /v1/images/edits"
    //   ImagesApi 走错 → "... is not supported on /v1/images/generations or /v1/images/edits"
    // 用 "supported on /v1/images/" 同时匹配 only/not 两种措辞。
    lower.contains("supported on /v1/images/")
        || lower.contains("/chat/completions")
        || lower.contains("must be used with")
}

pub(crate) fn has_known_direct_image_generation_route(
    provider: &ModelProvider,
    model: &str,
) -> bool {
    // xAI 也算：`resolve_image_route` 已把 api.x.ai 判到 ImagesApi、`uses_xai_images_api`
    // 认得 grok-imagine，管子是通的，只差这道门。不放行的话，用 Grok 预设（xai_responses）
    // 的用户选 grok-imagine 模型直接打 prompt 会退化成一次普通文本请求。
    if !matches!(
        provider.api_format_kind(),
        ProviderApiFormat::OpenAiChat | ProviderApiFormat::XaiResponses
    ) {
        return false;
    }
    // 判据来源换成单一 resolver，但**不扩大直连范围**：Chat route 恒为已知直连；ImagesApi route
    // 仅当命中已知 images API 模型（xai grok-imagine / gpt-image / dall-e）才算已知直连，
    // 与旧 `openrouter_chat || xai_images || openai_images_model` 逐例等价。
    match resolve_image_route(provider, model) {
        ImageRoute::Chat => true,
        ImageRoute::ImagesApi => {
            uses_xai_images_api(provider, model) || uses_openai_images_api_model(model)
        }
        ImageRoute::GeminiNative => false,
    }
}

fn parse_request(arguments: &Value) -> Result<ImageGenerationRequest, String> {
    let prompt = arguments
        .get("prompt")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Image generation requires prompt".to_string())?;
    let prompt = truncate_chars(prompt, MAX_PROMPT_CHARS);
    let size = match arguments
        .get("size")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_SIZE)
    {
        valid @ ("auto" | "1024x1024" | "1024x1536" | "1536x1024") => valid,
        other => return Err(format!("Unsupported image size: {other}")),
    };
    let quality = match arguments
        .get("quality")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_QUALITY)
    {
        valid @ ("auto" | "low" | "medium" | "high") => valid,
        other => return Err(format!("Unsupported image quality: {other}")),
    };
    let n = arguments
        .get("n")
        .and_then(|value| value.as_u64())
        .unwrap_or(1)
        .clamp(1, 4) as usize;

    Ok(ImageGenerationRequest {
        prompt,
        size: size.to_string(),
        quality: quality.to_string(),
        n,
        input_images: Vec::new(),
    })
}

fn validate_provider(provider: &ModelProvider) -> Result<(), String> {
    match provider.api_format_kind() {
        // Gemini 原生走 generateContent 出图路径；OpenAI 系走 images/chat 路径。
        // xAI 出图走 `/images/generations`（`uses_xai_images_api`），与 OpenAI 系同路。
        ProviderApiFormat::OpenAiChat
        | ProviderApiFormat::OpenAiResponses
        | ProviderApiFormat::XaiResponses
        | ProviderApiFormat::Gemini => {}
        ProviderApiFormat::AnthropicMessages => {
            return Err("Mixer image generation requires an OpenAI-compatible provider".to_string())
        }
    }
    if provider.api_keys.is_empty() {
        return Err(format!(
            "Image generation provider `{}` has no API key configured",
            provider.name
        ));
    }
    Ok(())
}

async fn generate_with_images_api(
    state: &AppState,
    provider: &ModelProvider,
    model: &str,
    request: &ImageGenerationRequest,
    retry_attempts: usize,
    operation: &str,
) -> Result<Vec<GeneratedImage>, String> {
    if !request.input_images.is_empty() {
        return generate_with_images_edits(
            state,
            provider,
            model,
            request,
            retry_attempts,
            operation,
        )
        .await;
    }
    let url = images_api_url(&provider.base_url, false);
    let mut body = serde_json::json!({
        "model": model,
        "prompt": request.prompt.as_str(),
        "n": request.n,
    });
    if uses_xai_images_api(provider, model) {
        body["response_format"] = Value::String("b64_json".to_string());
        if let Some(aspect_ratio) = size_aspect_ratio(&request.size) {
            body["aspect_ratio"] = Value::String(aspect_ratio.to_string());
        }
    } else if uses_gpt_image_api_model(model) {
        body["size"] = Value::String(request.size.clone());
        body["background"] = Value::String("auto".to_string());
    } else if request.size != "auto" {
        body["size"] = Value::String(request.size.clone());
    }
    if !uses_xai_images_api(provider, model) && request.quality != "auto" {
        body["quality"] = Value::String(request.quality.clone());
    }

    let response = send_with_failover(
        state,
        operation,
        retry_attempts,
        &provider.id,
        &provider.api_keys,
        |key| {
            crate::provider_request::apply(
                state.client_for(provider).post(&url).bearer_auth(key),
                provider,
                None,
            )
            .timeout(IMAGE_GENERATION_HTTP_TIMEOUT)
            .json(&body)
            .send()
        },
    )
    .await?;
    read_images_api_response(state, provider, response).await
}

/// 有参考图时走 `/images/edits`：xAI 必须 JSON；官方 `api.openai.com` 必须 multipart；
/// 其余兼容中转跟 `/images/generations` 一样发 JSON data URL。
async fn generate_with_images_edits(
    state: &AppState,
    provider: &ModelProvider,
    model: &str,
    request: &ImageGenerationRequest,
    retry_attempts: usize,
    operation: &str,
) -> Result<Vec<GeneratedImage>, String> {
    let url = images_api_url(&provider.base_url, true);
    let response = if uses_xai_images_api(provider, model) {
        post_images_json(
            state,
            provider,
            &url,
            &xai_edits_body(model, request),
            retry_attempts,
            operation,
        )
        .await?
    } else if is_official_openai_images(provider) {
        let files = decoded_input_files(request)?;
        send_with_failover(
            state,
            operation,
            retry_attempts,
            &provider.id,
            &provider.api_keys,
            |key| {
                crate::provider_request::apply(
                    state.client_for(provider).post(&url).bearer_auth(key),
                    provider,
                    None,
                )
                .timeout(IMAGE_GENERATION_HTTP_TIMEOUT)
                .multipart(images_edits_form(model, request, &files))
                .send()
            },
        )
        .await?
    } else {
        post_images_json(
            state,
            provider,
            &url,
            &openai_compat_edits_json_body(model, request),
            retry_attempts,
            operation,
        )
        .await?
    };
    read_images_api_response(state, provider, response).await
}

async fn post_images_json(
    state: &AppState,
    provider: &ModelProvider,
    url: &str,
    body: &Value,
    retry_attempts: usize,
    operation: &str,
) -> Result<reqwest::Response, String> {
    send_with_failover(
        state,
        operation,
        retry_attempts,
        &provider.id,
        &provider.api_keys,
        |key| {
            crate::provider_request::apply(
                state.client_for(provider).post(url).bearer_auth(key),
                provider,
                None,
            )
            .timeout(IMAGE_GENERATION_HTTP_TIMEOUT)
            .json(body)
            .send()
        },
    )
    .await
}

async fn read_images_api_response(
    state: &AppState,
    provider: &ModelProvider,
    response: reqwest::Response,
) -> Result<Vec<GeneratedImage>, String> {
    let raw = response
        .text()
        .await
        .map_err(|err| format!("Mixer image generation read body: {err}"))?;
    let value: Value = serde_json::from_str(&raw).map_err(|err| {
        format!(
            "Mixer image generation parse JSON: {} (body: {})",
            err,
            raw.chars().take(500).collect::<String>()
        )
    })?;
    parse_images_api_response(state, provider, &value).await
}

async fn parse_images_api_response(
    state: &AppState,
    provider: &ModelProvider,
    value: &Value,
) -> Result<Vec<GeneratedImage>, String> {
    let Some(data) = value.get("data").and_then(|value| value.as_array()) else {
        return Err("Image generation response missing data array".to_string());
    };
    let mut images = Vec::new();
    for item in data {
        let revised_prompt = item
            .get("revised_prompt")
            .or_else(|| item.get("revisedPrompt"))
            .and_then(|value| value.as_str())
            .map(str::to_string);
        if let Some(b64) = item
            .get("b64_json")
            .or_else(|| item.get("b64Json"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let mime_type = item
                .get("mime_type")
                .or_else(|| item.get("mimeType"))
                .and_then(|value| value.as_str())
                .unwrap_or("image/png")
                .to_string();
            validate_base64_image(b64)?;
            images.push(GeneratedImage {
                mime_type,
                base64: b64.to_string(),
                revised_prompt,
            });
            continue;
        }
        if let Some(url) = item
            .get("url")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let (mime_type, base64) = fetch_image_url(state, provider, url).await?;
            images.push(GeneratedImage {
                mime_type,
                base64,
                revised_prompt,
            });
        }
    }
    Ok(images)
}

async fn generate_with_openrouter_chat(
    state: &AppState,
    provider: &ModelProvider,
    model: &str,
    request: &ImageGenerationRequest,
    retry_attempts: usize,
    operation: &str,
) -> Result<(Vec<GeneratedImage>, Option<String>), String> {
    let url = format!(
        "{}/chat/completions",
        provider.base_url.trim_end_matches('/')
    );
    let mut body = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "user",
                "content": chat_user_content(request),
            }
        ],
        "modalities": openrouter_modalities(model),
        "stream": false,
    });
    if let Some(aspect_ratio) = openrouter_aspect_ratio(&request.size) {
        body["image_config"] = serde_json::json!({ "aspect_ratio": aspect_ratio });
    }

    let response = send_with_failover(
        state,
        operation,
        retry_attempts,
        &provider.id,
        &provider.api_keys,
        |key| {
            crate::provider_request::apply(
                state.client_for(provider).post(&url).bearer_auth(key),
                provider,
                None,
            )
            .timeout(IMAGE_GENERATION_HTTP_TIMEOUT)
            .json(&body)
            .send()
        },
    )
    .await?;
    let raw = response
        .text()
        .await
        .map_err(|err| format!("Mixer image generation read body: {err}"))?;
    let value: Value = serde_json::from_str(&raw).map_err(|err| {
        format!(
            "Mixer image generation parse JSON: {} (body: {})",
            err,
            raw.chars().take(500).collect::<String>()
        )
    })?;
    let images = parse_openrouter_response(&value)?;
    let text = value
        .get("choices")
        .and_then(|value| value.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    Ok((images, text))
}

fn parse_openrouter_response(value: &Value) -> Result<Vec<GeneratedImage>, String> {
    let mut images = Vec::new();
    let choices = value
        .get("choices")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "OpenRouter image response missing choices array".to_string())?;
    for choice in choices {
        let Some(message) = choice.get("message") else {
            continue;
        };
        let Some(message_images) = message.get("images").and_then(|value| value.as_array()) else {
            continue;
        };
        for item in message_images {
            let Some(data_url) = item
                .get("image_url")
                .or_else(|| item.get("imageUrl"))
                .and_then(|value| value.get("url"))
                .and_then(|value| value.as_str())
            else {
                continue;
            };
            let (mime_type, base64) = parse_image_data_url(data_url)?;
            images.push(GeneratedImage {
                mime_type,
                base64,
                revised_prompt: None,
            });
        }
    }
    Ok(images)
}

/// Gemini 原生 `generateContent` 出图路径（api_format = gemini）。
/// n>1 时顺序多次调用；首次失败则透传错误，若已有成功图则返回已收集的图。
async fn generate_with_gemini_native(
    state: &AppState,
    provider: &ModelProvider,
    model: &str,
    request: &ImageGenerationRequest,
    retry_attempts: usize,
    operation: &str,
) -> Result<(Vec<GeneratedImage>, Option<String>), String> {
    let url = gemini_generate_content_url(&provider.base_url, model);
    let body = build_gemini_native_body(request);

    let mut images = Vec::new();
    let mut fallback_text: Option<String> = None;
    for idx in 0..request.n {
        let response = send_with_failover(
            state,
            operation,
            retry_attempts,
            &provider.id,
            &provider.api_keys,
            |key| {
                crate::provider_request::apply(
                    state
                        .client_for(provider)
                        .post(&url)
                        .header("x-goog-api-key", key),
                    provider,
                    None,
                )
                .timeout(IMAGE_GENERATION_HTTP_TIMEOUT)
                .json(&body)
                .send()
            },
        )
        .await;
        let response = match response {
            Ok(response) => response,
            Err(err) => {
                if images.is_empty() {
                    return Err(err);
                }
                eprintln!(
                    "Mixer image generation gemini call #{} failed: {err}",
                    idx + 1
                );
                break;
            }
        };
        let raw = match response.text().await {
            Ok(raw) => raw,
            Err(err) => {
                if images.is_empty() {
                    return Err(format!("Mixer image generation read body: {err}"));
                }
                eprintln!(
                    "Mixer image generation gemini read body #{} failed: {err}",
                    idx + 1
                );
                break;
            }
        };
        let value: Value = match serde_json::from_str(&raw) {
            Ok(value) => value,
            Err(err) => {
                let message = format!(
                    "Mixer image generation parse JSON: {} (body: {})",
                    err,
                    raw.chars().take(500).collect::<String>()
                );
                if images.is_empty() {
                    return Err(message);
                }
                eprintln!("{message}");
                break;
            }
        };
        match parse_gemini_native_response(&value) {
            Ok(mut parsed) => images.append(&mut parsed),
            Err(err) => {
                if images.is_empty() {
                    return Err(err);
                }
                eprintln!(
                    "Mixer image generation gemini parse #{} failed: {err}",
                    idx + 1
                );
                break;
            }
        }
        if fallback_text.is_none() {
            fallback_text = gemini_native_response_text(&value);
        }
    }
    Ok((images, fallback_text))
}

/// 从 Gemini 原生响应里拼出 candidates[*].content.parts[*].text（无图时透出的文字）。
fn gemini_native_response_text(value: &Value) -> Option<String> {
    let text = value
        .get("candidates")
        .and_then(|value| value.as_array())?
        .iter()
        .filter_map(|candidate| {
            candidate
                .get("content")
                .and_then(|content| content.get("parts"))
                .and_then(|parts| parts.as_array())
        })
        .flatten()
        .filter_map(|part| part.get("text").and_then(|value| value.as_str()))
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// 复刻 gemini.rs 的 `gemini_url`（私有，未编辑该文件）：base_url 去尾斜杠，
/// model 去重 "models/" 前缀，避免 "models/models/"。
fn gemini_generate_content_url(base_url: &str, model: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let model = model.trim_start_matches("models/");
    format!("{base}/models/{model}:generateContent")
}

fn build_gemini_native_body(request: &ImageGenerationRequest) -> Value {
    let mut generation_config = serde_json::json!({
        "responseModalities": ["TEXT", "IMAGE"],
    });
    if let Some(aspect_ratio) = size_aspect_ratio(&request.size) {
        generation_config["imageConfig"] = serde_json::json!({ "aspectRatio": aspect_ratio });
    }
    let mut parts = request
        .input_images
        .iter()
        .map(|image| {
            serde_json::json!({
                "inlineData": {
                    "mimeType": image.mime_type,
                    "data": image.base64,
                }
            })
        })
        .collect::<Vec<_>>();
    parts.push(serde_json::json!({ "text": request.prompt.as_str() }));
    serde_json::json!({
        "contents": [
            {
                "role": "user",
                "parts": parts,
            }
        ],
        "generationConfig": generation_config,
    })
}

fn parse_gemini_native_response(value: &Value) -> Result<Vec<GeneratedImage>, String> {
    let candidates = value
        .get("candidates")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "Gemini image response missing candidates array".to_string())?;
    let mut images = Vec::new();
    for candidate in candidates {
        let Some(parts) = candidate
            .get("content")
            .and_then(|content| content.get("parts"))
            .and_then(|parts| parts.as_array())
        else {
            continue;
        };
        for part in parts {
            let Some(inline_data) = part.get("inlineData").or_else(|| part.get("inline_data"))
            else {
                continue;
            };
            let Some(base64) = inline_data
                .get("data")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let mime_type = inline_data
                .get("mimeType")
                .or_else(|| inline_data.get("mime_type"))
                .and_then(|value| value.as_str())
                .filter(|value| value.starts_with("image/"))
                .unwrap_or("image/png")
                .to_string();
            validate_base64_image(base64)?;
            images.push(GeneratedImage {
                mime_type,
                base64: base64.to_string(),
                revised_prompt: None,
            });
        }
    }
    Ok(images)
}

/// 下载出图返回的图片 URL。走**该供应商的**客户端：关掉「跟随系统代理」的供应商，
/// 这一跳也该直连——否则 UI 上写着「该供应商所有请求」的开关在这条路径上不成立。
async fn fetch_image_url(
    state: &AppState,
    provider: &ModelProvider,
    url: &str,
) -> Result<(String, String), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Image generation returned a non-http image URL".to_string());
    }
    let response = state
        .client_for(provider)
        .get(url)
        .timeout(IMAGE_GENERATION_HTTP_TIMEOUT)
        .send()
        .await
        .map_err(|err| format!("Download generated image failed: {err}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Download generated image failed: HTTP {}",
            response.status()
        ));
    }
    let mime_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| value.starts_with("image/"))
        .unwrap_or("image/png")
        .to_string();
    let bytes = response
        .bytes()
        .await
        .map_err(|err| format!("Read generated image failed: {err}"))?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err("Generated image is too large to attach".to_string());
    }
    Ok((mime_type, general_purpose::STANDARD.encode(bytes)))
}

fn parse_image_data_url(data_url: &str) -> Result<(String, String), String> {
    let trimmed = data_url.trim();
    let Some(rest) = trimmed.strip_prefix("data:") else {
        return Err("OpenRouter image response did not return a data URL".to_string());
    };
    let Some((metadata, payload)) = rest.split_once(',') else {
        return Err("Image data URL is malformed".to_string());
    };
    let mime_type = metadata
        .split(';')
        .next()
        .map(str::trim)
        .filter(|value| value.starts_with("image/"))
        .unwrap_or("image/png")
        .to_string();
    if !metadata
        .split(';')
        .any(|part| part.eq_ignore_ascii_case("base64"))
    {
        return Err("Image data URL is not base64 encoded".to_string());
    }
    validate_base64_image(payload.trim())?;
    Ok((mime_type, payload.trim().to_string()))
}

fn validate_base64_image(value: &str) -> Result<(), String> {
    let decoded_len = decoded_base64_len(value).unwrap_or(0);
    if decoded_len == 0 {
        return Err("Generated image base64 is empty".to_string());
    }
    if decoded_len as usize > MAX_IMAGE_BYTES {
        return Err("Generated image is too large to attach".to_string());
    }
    if general_purpose::STANDARD
        .decode(value)
        .map(|bytes| !bytes.is_empty())
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err("Generated image base64 is invalid".to_string())
    }
}

pub(crate) fn decoded_base64_len(value: &str) -> Option<u64> {
    let compact_len = value.chars().filter(|ch| !ch.is_whitespace()).count();
    if compact_len == 0 {
        return Some(0);
    }
    let padding = value
        .chars()
        .rev()
        .take_while(|ch| ch.is_whitespace() || *ch == '=')
        .filter(|ch| *ch == '=')
        .count()
        .min(2);
    Some(((compact_len * 3) / 4).saturating_sub(padding) as u64)
}

pub(crate) fn extension_for_mime(mime_type: &str) -> &'static str {
    match mime_type {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        _ => "png",
    }
}

fn is_openrouter_base_url(base_url: &str) -> bool {
    base_url
        .trim()
        .to_ascii_lowercase()
        .contains("openrouter.ai")
}

fn openrouter_aspect_ratio(size: &str) -> Option<&'static str> {
    size_aspect_ratio(size)
}

fn size_aspect_ratio(size: &str) -> Option<&'static str> {
    match size {
        "1024x1024" => Some("1:1"),
        "1024x1536" => Some("2:3"),
        "1536x1024" => Some("3:2"),
        _ => None,
    }
}

fn uses_xai_images_api(provider: &ModelProvider, model: &str) -> bool {
    let descriptor =
        format!("{} {} {}", provider.base_url, provider.name, model).to_ascii_lowercase();
    descriptor.contains("api.x.ai") || descriptor.contains("grok-imagine-image")
}

fn uses_openai_images_api_model(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    uses_gpt_image_api_model(model) || lower.contains("dall-e")
}

fn uses_gpt_image_api_model(model: &str) -> bool {
    model.to_ascii_lowercase().contains("gpt-image")
}

fn openrouter_modalities(model: &str) -> Value {
    let lower = model.to_ascii_lowercase();
    let image_only = lower.contains("flux")
        || lower.contains("sourceful")
        || lower.contains("riverflow")
        || lower.contains("recraft")
        || lower.contains("seedream")
        || lower.contains("mai-image")
        || lower.contains("grok-imagine-image")
        || lower.contains("stable-diffusion")
        || lower.contains("sdxl")
        || lower.contains("imagen")
        || lower.contains("ideogram");
    if image_only {
        serde_json::json!(["image"])
    } else {
        serde_json::json!(["image", "text"])
    }
}

fn images_api_url(base_url: &str, has_input_images: bool) -> String {
    let path = if has_input_images {
        "images/edits"
    } else {
        "images/generations"
    };
    format!("{}/{path}", base_url.trim_end_matches('/'))
}

fn data_url_for_input(image: &InputImage) -> String {
    format!("data:{};base64,{}", image.mime_type, image.base64)
}

fn chat_user_content(request: &ImageGenerationRequest) -> Value {
    if request.input_images.is_empty() {
        return Value::String(request.prompt.clone());
    }
    let mut parts = vec![serde_json::json!({
        "type": "text",
        "text": request.prompt.as_str(),
    })];
    for image in &request.input_images {
        parts.push(serde_json::json!({
            "type": "image_url",
            "image_url": { "url": data_url_for_input(image) }
        }));
    }
    Value::Array(parts)
}

fn is_official_openai_images(provider: &ModelProvider) -> bool {
    provider
        .base_url
        .to_ascii_lowercase()
        .contains("api.openai.com")
}

fn openai_compat_edits_json_body(model: &str, request: &ImageGenerationRequest) -> Value {
    let mut body = serde_json::json!({
        "model": model,
        "prompt": request.prompt.as_str(),
        "n": request.n,
    });
    if uses_gpt_image_api_model(model) || request.size != "auto" {
        body["size"] = Value::String(request.size.clone());
    }
    if request.quality != "auto" {
        body["quality"] = Value::String(request.quality.clone());
    }
    let urls = request
        .input_images
        .iter()
        .map(|image| Value::String(data_url_for_input(image)))
        .collect::<Vec<_>>();
    body["image"] = if urls.len() == 1 {
        urls.into_iter().next().unwrap_or(Value::Null)
    } else {
        Value::Array(urls)
    };
    body
}

fn xai_edits_body(model: &str, request: &ImageGenerationRequest) -> Value {
    let mut body = serde_json::json!({
        "model": model,
        "prompt": request.prompt.as_str(),
        "n": request.n,
        "response_format": "b64_json",
    });
    if let Some(aspect_ratio) = size_aspect_ratio(&request.size) {
        body["aspect_ratio"] = Value::String(aspect_ratio.to_string());
    }
    let refs = request
        .input_images
        .iter()
        .map(|image| {
            serde_json::json!({
                "url": data_url_for_input(image),
                "type": "image_url",
            })
        })
        .collect::<Vec<_>>();
    if refs.len() == 1 {
        body["image"] = refs.into_iter().next().unwrap_or(Value::Null);
    } else {
        body["images"] = Value::Array(refs);
    }
    body
}

fn decoded_input_files(
    request: &ImageGenerationRequest,
) -> Result<Vec<(String, String, Vec<u8>)>, String> {
    request
        .input_images
        .iter()
        .enumerate()
        .map(|(idx, image)| {
            let bytes = general_purpose::STANDARD
                .decode(image.base64.as_bytes())
                .map_err(|_| "Input image base64 is invalid".to_string())?;
            if bytes.is_empty() {
                return Err("Input image is empty".to_string());
            }
            Ok((
                format!("image-{}.{}", idx + 1, extension_for_mime(&image.mime_type)),
                image.mime_type.clone(),
                bytes,
            ))
        })
        .collect()
}

fn images_edits_form(
    model: &str,
    request: &ImageGenerationRequest,
    files: &[(String, String, Vec<u8>)],
) -> reqwest::multipart::Form {
    let mut form = reqwest::multipart::Form::new()
        .text("model", model.to_string())
        .text("prompt", request.prompt.clone())
        .text("n", request.n.to_string());
    if uses_gpt_image_api_model(model) || request.size != "auto" {
        form = form.text("size", request.size.clone());
    }
    if request.quality != "auto" {
        form = form.text("quality", request.quality.clone());
    }
    let field = if uses_gpt_image_api_model(model) || files.len() > 1 {
        "image[]"
    } else {
        "image"
    };
    for (name, mime, bytes) in files {
        let part = reqwest::multipart::Part::bytes(bytes.clone()).file_name(name.clone());
        let part = part.mime_str(mime).unwrap_or_else(|_| {
            reqwest::multipart::Part::bytes(bytes.clone()).file_name(name.clone())
        });
        form = form.part(field, part);
    }
    form
}

pub(crate) fn load_input_images_from_paths(paths: &[PathBuf]) -> Result<Vec<InputImage>, String> {
    paths
        .iter()
        .map(|path| load_input_image_from_path(path))
        .collect()
}

fn max_reference_images(provider: &ModelProvider, model: &str) -> usize {
    if uses_xai_images_api(provider, model) {
        XAI_MAX_EDIT_IMAGES
    } else {
        MAX_INPUT_IMAGES
    }
}

fn reject_excess_reference_images(
    provider: &ModelProvider,
    model: &str,
    count: usize,
) -> Result<(), String> {
    let max_refs = max_reference_images(provider, model);
    if count > max_refs {
        Err(format!(
            "This image model accepts at most {max_refs} reference images (got {count})"
        ))
    } else {
        Ok(())
    }
}

fn load_input_image_from_path(path: &Path) -> Result<InputImage, String> {
    let bytes = fs::read(path).map_err(|err| format!("Read input image failed: {err}"))?;
    if bytes.is_empty() {
        return Err("Input image is empty".to_string());
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err("Input image is too large".to_string());
    }
    let (bytes, mime_type) = match super::image_prep::prepare_image_bytes_for_model(&bytes) {
        Some((prepared, mime)) => (prepared, mime.to_string()),
        None => (bytes, mime_for_image_path(path).to_string()),
    };
    Ok(InputImage {
        mime_type,
        base64: general_purpose::STANDARD.encode(bytes),
    })
}

fn mime_for_image_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tiff" | "tif" => "image/tiff",
        "heic" => "image/heic",
        "heif" => "image/heif",
        _ => "image/png",
    }
}

fn collect_mixer_input_images(
    app: &AppHandle,
    conversation: Option<&crate::chat::Conversation>,
    drafts: &[crate::chat::ChatMessage],
    arguments: &Value,
) -> Result<Vec<InputImage>, String> {
    let artifact_ids = string_list_arg(arguments, "artifact_ids");
    let paths = string_list_arg(arguments, "paths");
    let mut images = Vec::new();
    let mut missing = Vec::new();

    let artifacts = resolve_mixer_artifacts(conversation, drafts, &artifact_ids)?;
    if let Some(conversation) = conversation {
        for artifact in artifacts {
            match input_image_from_artifact(app, &conversation.id, artifact) {
                Ok(image) => push_input_image(&mut images, image),
                Err(err) => missing.push(err),
            }
        }
    }

    for path in &paths {
        match resolve_mixer_image_path(app, conversation, path)
            .and_then(|resolved| load_input_image_from_path(&resolved))
        {
            Ok(image) => push_input_image(&mut images, image),
            Err(err) => missing.push(err),
        }
    }

    if !artifact_ids.is_empty() || !paths.is_empty() {
        if !missing.is_empty() {
            return Err(missing.join("; "));
        }
        return Ok(images);
    }

    let Some(conversation) = conversation else {
        return Ok(Vec::new());
    };
    let Some(last_user) = conversation
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
    else {
        return Ok(Vec::new());
    };
    let attachment_paths = crate::chat::attachments::stored_image_paths_for_attachments(
        app,
        &conversation.id,
        &last_user.attachments,
    )?;
    load_input_images_from_paths(&attachment_paths)
}

fn string_list_arg(arguments: &Value, name: &str) -> Vec<String> {
    let Some(values) = arguments.get(name).and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for value in values {
        let Some(item) = value
            .as_str()
            .map(str::trim)
            .filter(|item| !item.is_empty())
        else {
            continue;
        };
        if !items.iter().any(|existing| existing == item) {
            items.push(item.to_string());
        }
    }
    items
}

fn push_input_image(images: &mut Vec<InputImage>, image: InputImage) {
    if images
        .iter()
        .any(|existing| existing.base64 == image.base64)
    {
        return;
    }
    images.push(image);
}

fn resolve_mixer_artifacts<'a>(
    conversation: Option<&'a crate::chat::Conversation>,
    drafts: &'a [crate::chat::ChatMessage],
    artifact_ids: &[String],
) -> Result<Vec<&'a ChatToolArtifact>, String> {
    if artifact_ids.is_empty() {
        return Ok(Vec::new());
    }
    let Some(conversation) = conversation else {
        return Err("artifact_ids require an active conversation".to_string());
    };
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for id in artifact_ids {
        match find_artifact_in_messages(drafts.iter().chain(conversation.messages.iter()), id) {
            Some(artifact) => found.push(artifact),
            None => missing.push(format!("Unknown artifact_id: {id}")),
        }
    }
    if !missing.is_empty() {
        return Err(missing.join("; "));
    }
    Ok(found)
}

fn find_artifact_in_messages<'a, I>(messages: I, artifact_id: &str) -> Option<&'a ChatToolArtifact>
where
    I: IntoIterator<Item = &'a crate::chat::ChatMessage>,
{
    messages.into_iter().find_map(|message| {
        message
            .artifacts
            .iter()
            .chain(
                message
                    .tool_calls
                    .iter()
                    .flat_map(|call| call.artifacts.iter()),
            )
            .find(|artifact| artifact.id.as_deref() == Some(artifact_id))
    })
}

fn input_image_from_artifact(
    app: &AppHandle,
    conversation_id: &str,
    artifact: &ChatToolArtifact,
) -> Result<InputImage, String> {
    if !artifact.mime_type.starts_with("image/") && !artifact.mime_type.is_empty() {
        return Err(format!(
            "Artifact `{}` is not an image",
            artifact.id.as_deref().unwrap_or(&artifact.name)
        ));
    }
    if let Some(rel) = artifact.path.as_deref().filter(|path| !path.is_empty()) {
        let candidate = Path::new(rel);
        let path = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            crate::chat::storage::conversation_attachments_dir(app, conversation_id)?.join(rel)
        };
        if path.is_file() {
            return load_input_image_from_path(&path);
        }
        return Err(format!(
            "Artifact `{}` image file is missing: {}",
            artifact.id.as_deref().unwrap_or(&artifact.name),
            path.display()
        ));
    }
    if artifact.data_url.starts_with("data:") {
        let (mime_type, base64) = parse_image_data_url(&artifact.data_url)?;
        return Ok(InputImage { mime_type, base64 });
    }
    Err(format!(
        "Artifact `{}` has no readable image data",
        artifact.id.as_deref().unwrap_or(&artifact.name)
    ))
}

fn resolve_mixer_image_path(
    app: &AppHandle,
    conversation: Option<&crate::chat::Conversation>,
    raw: &str,
) -> Result<PathBuf, String> {
    let path = PathBuf::from(raw);
    if path.is_file() {
        return Ok(path);
    }
    if let Some(conversation) = conversation {
        if let Ok(dir) = crate::chat::storage::conversation_attachments_dir(app, &conversation.id) {
            let name = Path::new(raw)
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| path.clone());
            let joined = dir.join(name);
            if joined.is_file() {
                return Ok(joined);
            }
        }
    }
    Err(format!("Input image not found: {raw}"))
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openrouter_image_data_url() {
        let value = serde_json::json!({
            "choices": [
                {
                    "message": {
                        "images": [
                            {
                                "type": "image_url",
                                "image_url": {
                                    "url": "data:image/png;base64,aGVsbG8="
                                }
                            }
                        ]
                    }
                }
            ]
        });

        let images = parse_openrouter_response(&value).expect("image should parse");

        assert_eq!(images.len(), 1);
        assert_eq!(images[0].mime_type, "image/png");
        assert_eq!(images[0].base64, "aGVsbG8=");
    }

    #[test]
    fn openrouter_bare_gemini_image_uses_image_text_modality() {
        // 裸名 gemini-3.1-flash-image 含 "image" 但不在 image_only 列表 → ["image","text"]。
        assert_eq!(
            openrouter_modalities("gemini-3.1-flash-image"),
            serde_json::json!(["image", "text"])
        );
    }

    #[test]
    fn text_only_image_response_yields_no_image_but_has_text() {
        // 模型对笼统提示返回纯文字（澄清），无 images → 解析出空图 + 文字透出。
        let value = serde_json::json!({
            "candidates": [
                { "content": { "parts": [
                    { "text": "请提供更多细节，" },
                    { "text": "例如性别、发色。" }
                ] } }
            ]
        });
        assert!(parse_gemini_native_response(&value)
            .expect("parse ok")
            .is_empty());
        assert_eq!(
            gemini_native_response_text(&value).as_deref(),
            Some("请提供更多细节，例如性别、发色。")
        );

        // 真出图时不误判为文字-only。
        let with_image = serde_json::json!({
            "candidates": [
                { "content": { "parts": [
                    { "inlineData": { "mimeType": "image/png", "data": "aGVsbG8=" } }
                ] } }
            ]
        });
        assert_eq!(gemini_native_response_text(&with_image), None);
    }

    #[test]
    fn bare_image_model_names_route_through_chat_on_generic_proxy() {
        // 通用 /v1 代理（非 openrouter.ai / 非 api.openai.com / 非 api.x.ai）暴露的裸名出图模型
        // 必须走 chat/completions 仿 OpenRouter 出图，而非 /images/generations。
        let proxy = ModelProvider {
            id: "proxy".to_string(),
            name: "Proxy".to_string(),
            api_keys: vec!["k".to_string()],
            api_key_legacy: None,
            base_url: "https://cpa.xb1520.com/v1".to_string(),
            available_models: Vec::new(),
            enabled_models: Vec::new(),
            enabled: true,
            api_format: "openai_chat".to_string(),
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
            active_key_index: 0,
        };

        assert_eq!(
            resolve_image_route(&proxy, "gemini-3.1-flash-image"),
            ImageRoute::Chat
        );
        // grok-imagine-image 相反：通用代理只支持 /images/generations，
        // 故走 ImagesApi（uses_xai_images_api body 变体命中）。
        assert_eq!(
            resolve_image_route(&proxy, "grok-imagine-image"),
            ImageRoute::ImagesApi
        );
        assert!(uses_xai_images_api(&proxy, "grok-imagine-image"));
        assert_eq!(resolve_image_route(&proxy, "nano-banana"), ImageRoute::Chat);
        assert_eq!(resolve_image_route(&proxy, "imagen-4.0"), ImageRoute::Chat);
        // 改名 / 加前缀 / 大小写变体仍正确路由（归一化）。
        assert_eq!(
            resolve_image_route(&proxy, "Gemini-3.1-Flash-Image"),
            ImageRoute::Chat
        );
        assert_eq!(
            resolve_image_route(&proxy, "models/gemini-3.1-flash-image"),
            ImageRoute::Chat
        );
        // 也走已知直连路由总闸（OpenAiChat + chat 出图）。
        assert!(has_known_direct_image_generation_route(
            &proxy,
            "gemini-3.1-flash-image"
        ));
        // 普通文本模型不误判。
        assert_eq!(resolve_image_route(&proxy, "gpt-4o"), ImageRoute::ImagesApi);

        // api.openai.com / api.x.ai 上的裸名仍被早退排除（走各自 images API，不走 chat）。
        let openai = ModelProvider {
            base_url: "https://api.openai.com/v1".to_string(),
            active_key_index: 0,
            ..proxy.clone()
        };
        assert_eq!(
            resolve_image_route(&openai, "gemini-3.1-flash-image"),
            ImageRoute::ImagesApi
        );
        let xai = ModelProvider {
            base_url: "https://api.x.ai/v1".to_string(),
            active_key_index: 0,
            ..proxy.clone()
        };
        assert_eq!(
            resolve_image_route(&xai, "grok-imagine-image"),
            ImageRoute::ImagesApi
        );
    }

    #[test]
    fn resolve_image_route_gemini_provider_uses_native() {
        assert_eq!(
            resolve_image_route(&gemini_provider(), "gemini-3.1-flash-image"),
            ImageRoute::GeminiNative
        );
        // Gemini api_format 恒 GeminiNative，即便名字看似普通文本模型。
        assert_eq!(
            resolve_image_route(&gemini_provider(), "gemini-2.5-pro"),
            ImageRoute::GeminiNative
        );
    }

    #[test]
    fn resolve_image_route_openrouter_base_uses_chat() {
        let openrouter = ModelProvider {
            id: "or".to_string(),
            name: "OpenRouter".to_string(),
            api_keys: vec!["k".to_string()],
            api_key_legacy: None,
            base_url: "https://openrouter.ai/api/v1".to_string(),
            available_models: Vec::new(),
            enabled_models: Vec::new(),
            enabled: true,
            api_format: "openai_chat".to_string(),
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
            active_key_index: 0,
        };
        assert_eq!(
            resolve_image_route(&openrouter, "google/gemini-3.1-flash-image-preview"),
            ImageRoute::Chat
        );
        assert_eq!(
            resolve_image_route(&openrouter, "grok-imagine-image"),
            ImageRoute::Chat
        );
    }

    #[test]
    fn endpoint_mismatch_error_matches_provider_phrases_not_generic_errors() {
        assert!(is_endpoint_mismatch_error(
            "Mixer image generation Error: 400 - {\"error\":{\"message\":\"This model is only supported on /v1/images/generations\"}}"
        ));
        assert!(is_endpoint_mismatch_error(
            "Error: 400 - only supported on /v1/images/edits"
        ));
        assert!(is_endpoint_mismatch_error(
            "Error: 404 - This model must be used with the chat endpoint"
        ));
        assert!(is_endpoint_mismatch_error(
            "Error: 400 - image models require the /chat/completions endpoint"
        ));
        // 真实代理错误串（两向,由真机测试捕获）：
        //   grok 走错 chat/completions（Chat→ImagesApi 方向）
        assert!(is_endpoint_mismatch_error(
            "Chat image generation Error: 503 Service Unavailable - {\"error\":{\"message\":\"model grok-imagine-image is only supported on /v1/images/generations and /v1/images/edits\"}}"
        ));
        //   gemini-image 走错 /v1/images/generations（ImagesApi→Chat 方向，措辞是 "not supported on"）
        assert!(is_endpoint_mismatch_error(
            "Mixer image generation Error: 400 - {\"error\":{\"message\":\"Model gemini-3.1-flash-image is not supported on /v1/images/generations or /v1/images/edits. Use gpt-image-1.5\"}}"
        ));
        // 普通错误不触发换端点。
        assert!(!is_endpoint_mismatch_error(
            "Image generation response missing data array"
        ));
        assert!(!is_endpoint_mismatch_error(
            "Mixer image generation Error: 500 - internal server error"
        ));
        assert!(!is_endpoint_mismatch_error("connection reset by peer"));
    }

    #[test]
    fn alternate_route_swings_chat_and_images_only() {
        assert_eq!(
            alternate_route(ImageRoute::Chat),
            Some(ImageRoute::ImagesApi)
        );
        assert_eq!(
            alternate_route(ImageRoute::ImagesApi),
            Some(ImageRoute::Chat)
        );
        assert_eq!(alternate_route(ImageRoute::GeminiNative), None);
    }

    #[test]
    fn rejects_empty_prompt() {
        let err = parse_request(&serde_json::json!({ "prompt": " " })).unwrap_err();
        assert!(err.contains("prompt"));
    }

    #[test]
    fn clamps_image_count() {
        let request = parse_request(&serde_json::json!({
            "prompt": "draw a small app icon",
            "n": 99,
        }))
        .expect("request should parse");

        assert_eq!(request.n, 4);
    }

    #[test]
    fn openrouter_flux_models_use_image_only_modality() {
        assert_eq!(
            openrouter_modalities("black-forest-labs/flux.2-pro"),
            serde_json::json!(["image"])
        );
        assert_eq!(
            openrouter_modalities("recraft/recraft-v4.1-pro"),
            serde_json::json!(["image"])
        );
        assert_eq!(
            openrouter_modalities("bytedance-seed/seedream-4.5"),
            serde_json::json!(["image"])
        );
        assert_eq!(
            openrouter_modalities("x-ai/grok-imagine-image-quality"),
            serde_json::json!(["image"])
        );
        assert_eq!(
            openrouter_modalities("google/gemini-3.1-flash-image-preview"),
            serde_json::json!(["image", "text"])
        );
    }

    #[test]
    fn xai_detection_matches_grok_imagine_models() {
        let provider = ModelProvider {
            id: "xai".to_string(),
            name: "xAI".to_string(),
            api_keys: Vec::new(),
            api_key_legacy: None,
            base_url: "https://api.x.ai/v1".to_string(),
            available_models: Vec::new(),
            enabled_models: Vec::new(),
            enabled: true,
            api_format: "openai_chat".to_string(),
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
            active_key_index: 0,
        };

        assert!(uses_xai_images_api(&provider, "grok-imagine-image-quality"));
        assert!(has_known_direct_image_generation_route(
            &provider,
            "grok-imagine-image-quality"
        ));
    }

    #[test]
    fn direct_route_detection_matches_known_provider_routes() {
        let openai = ModelProvider {
            id: "openai".to_string(),
            name: "OpenAI".to_string(),
            api_keys: Vec::new(),
            api_key_legacy: None,
            base_url: "https://api.openai.com/v1".to_string(),
            available_models: Vec::new(),
            enabled_models: Vec::new(),
            enabled: true,
            api_format: "openai_chat".to_string(),
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
            active_key_index: 0,
        };
        let openrouter = ModelProvider {
            base_url: "https://openrouter.ai/api/v1".to_string(),
            active_key_index: 0,
            ..openai.clone()
        };
        let openrouter_compatible_relay = ModelProvider {
            base_url: "https://relay.example.com/v1".to_string(),
            active_key_index: 0,
            ..openai.clone()
        };

        assert!(has_known_direct_image_generation_route(
            &openai,
            "gpt-image-1.5"
        ));
        assert!(has_known_direct_image_generation_route(
            &openrouter,
            "google/gemini-3.1-flash-image-preview"
        ));
        assert!(has_known_direct_image_generation_route(
            &openrouter_compatible_relay,
            "black-forest-labs/flux.2-pro"
        ));
        assert!(!has_known_direct_image_generation_route(
            &openai,
            "gemini-3.1-flash-image-preview"
        ));
        assert!(!has_known_direct_image_generation_route(
            &openai,
            "openai/gpt-5-image"
        ));
    }

    fn gemini_provider() -> ModelProvider {
        ModelProvider {
            id: "gemini".to_string(),
            name: "Gemini".to_string(),
            api_keys: vec!["k".to_string()],
            api_key_legacy: None,
            base_url: "https://generativelanguage.googleapis.com/v1beta".to_string(),
            available_models: Vec::new(),
            enabled_models: Vec::new(),
            enabled: true,
            api_format: "gemini".to_string(),
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
            active_key_index: 0,
        }
    }

    #[test]
    fn gemini_native_body_shape() {
        let request = ImageGenerationRequest {
            prompt: "draw a cat".to_string(),
            size: "1024x1024".to_string(),
            quality: "auto".to_string(),
            n: 1,
            input_images: Vec::new(),
        };
        let body = build_gemini_native_body(&request);
        assert_eq!(
            body["generationConfig"]["responseModalities"],
            serde_json::json!(["TEXT", "IMAGE"])
        );
        assert_eq!(
            body["generationConfig"]["imageConfig"]["aspectRatio"],
            serde_json::json!("1:1")
        );
        assert_eq!(
            body["contents"][0]["parts"].as_array().map(|p| p.len()),
            Some(1)
        );
        assert_eq!(body["contents"][0]["parts"][0]["text"], "draw a cat");

        let auto = ImageGenerationRequest {
            size: "auto".to_string(),
            ..request
        };
        let auto_body = build_gemini_native_body(&auto);
        assert!(auto_body["generationConfig"].get("imageConfig").is_none());
    }

    #[test]
    fn gemini_native_url_dedupes_models_prefix() {
        assert_eq!(
            gemini_generate_content_url(
                "https://generativelanguage.googleapis.com/v1beta/",
                "gemini-3.1-flash-image"
            ),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.1-flash-image:generateContent"
        );
        assert_eq!(
            gemini_generate_content_url(
                "https://generativelanguage.googleapis.com/v1beta",
                "models/gemini-3.1-flash-image"
            ),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.1-flash-image:generateContent"
        );
    }

    #[test]
    fn gemini_native_parses_inline_data() {
        let value = serde_json::json!({
            "candidates": [
                {
                    "content": {
                        "parts": [
                            { "text": "here is your image" },
                            {
                                "inlineData": {
                                    "mimeType": "image/png",
                                    "data": "aGVsbG8="
                                }
                            }
                        ]
                    }
                }
            ]
        });
        let images = parse_gemini_native_response(&value).expect("image should parse");
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].mime_type, "image/png");
        assert_eq!(images[0].base64, "aGVsbG8=");
        assert!(images[0].revised_prompt.is_none());
    }

    #[test]
    fn validate_provider_accepts_gemini_rejects_anthropic() {
        assert!(validate_provider(&gemini_provider()).is_ok());

        let anthropic = ModelProvider {
            api_format: "anthropic_messages".to_string(),
            active_key_index: 0,
            ..gemini_provider()
        };
        assert!(validate_provider(&anthropic).is_err());
    }

    fn sample_input_image() -> InputImage {
        InputImage {
            mime_type: "image/png".to_string(),
            base64: "aGVsbG8=".to_string(),
        }
    }

    #[test]
    fn images_api_switches_to_edits_when_input_images_present() {
        assert_eq!(
            images_api_url("https://api.openai.com/v1/", false),
            "https://api.openai.com/v1/images/generations"
        );
        assert_eq!(
            images_api_url("https://api.openai.com/v1", true),
            "https://api.openai.com/v1/images/edits"
        );
    }

    #[test]
    fn chat_content_stays_plain_text_without_input_images() {
        let request = ImageGenerationRequest {
            prompt: "a red mug".to_string(),
            size: "auto".to_string(),
            quality: "auto".to_string(),
            n: 1,
            input_images: Vec::new(),
        };
        assert_eq!(
            chat_user_content(&request),
            Value::String("a red mug".to_string())
        );
    }

    #[test]
    fn chat_content_includes_input_images_as_data_urls() {
        let request = ImageGenerationRequest {
            prompt: "make it night".to_string(),
            size: "auto".to_string(),
            quality: "auto".to_string(),
            n: 1,
            input_images: vec![sample_input_image()],
        };
        let content = chat_user_content(&request);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "make it night");
        assert_eq!(
            content[1]["image_url"]["url"],
            "data:image/png;base64,aGVsbG8="
        );
    }

    #[test]
    fn gemini_body_prepends_inline_input_images() {
        let request = ImageGenerationRequest {
            prompt: "make it night".to_string(),
            size: "auto".to_string(),
            quality: "auto".to_string(),
            n: 1,
            input_images: vec![sample_input_image()],
        };
        let body = build_gemini_native_body(&request);
        assert_eq!(
            body["contents"][0]["parts"][0]["inlineData"]["data"],
            "aGVsbG8="
        );
        assert_eq!(body["contents"][0]["parts"][1]["text"], "make it night");
    }

    #[test]
    fn xai_edits_json_uses_image_object_for_one_and_images_array_for_many() {
        let one = ImageGenerationRequest {
            prompt: "sketch".to_string(),
            size: "1024x1024".to_string(),
            quality: "auto".to_string(),
            n: 1,
            input_images: vec![sample_input_image()],
        };
        let body = xai_edits_body("grok-imagine-image", &one);
        assert_eq!(body["image"]["type"], "image_url");
        assert_eq!(body["image"]["url"], "data:image/png;base64,aGVsbG8=");
        assert!(body.get("images").is_none());
        assert_eq!(body["aspect_ratio"], "1:1");

        let many = ImageGenerationRequest {
            input_images: vec![sample_input_image(), sample_input_image()],
            ..one
        };
        let body = xai_edits_body("grok-imagine-image", &many);
        assert!(body.get("image").is_none());
        assert_eq!(body["images"].as_array().map(|v| v.len()), Some(2));
    }

    #[test]
    fn openai_compat_edits_json_puts_data_urls_on_image() {
        let request = ImageGenerationRequest {
            prompt: "make it night".to_string(),
            size: "1024x1024".to_string(),
            quality: "high".to_string(),
            n: 1,
            input_images: vec![sample_input_image()],
        };
        let body = openai_compat_edits_json_body("gpt-image-1.5", &request);
        assert_eq!(body["image"], "data:image/png;base64,aGVsbG8=");
        assert_eq!(body["size"], "1024x1024");
        assert_eq!(body["quality"], "high");

        let many = ImageGenerationRequest {
            input_images: vec![sample_input_image(), sample_input_image()],
            ..request
        };
        let body = openai_compat_edits_json_body("gpt-image-1.5", &many);
        assert_eq!(body["image"].as_array().map(|v| v.len()), Some(2));
    }

    #[test]
    fn loads_input_image_from_temp_file() {
        let dir = std::env::temp_dir().join(format!("kivio-img-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("photo.jpg");
        fs::write(&path, b"fake-jpeg-bytes").expect("write");
        let image = load_input_image_from_path(&path).expect("load");
        assert_eq!(image.mime_type, "image/jpeg");
        assert_eq!(
            general_purpose::STANDARD.decode(image.base64).expect("b64"),
            b"fake-jpeg-bytes"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    fn conversation_from_messages(messages: Vec<Value>) -> crate::chat::Conversation {
        serde_json::from_value(serde_json::json!({
            "id": "conv_1",
            "title": "t",
            "provider_id": "p",
            "model": "m",
            "messages": messages,
            "created_at": 0,
            "updated_at": 0,
        }))
        .expect("conversation")
    }

    fn assistant_with_tool_artifact(artifact_id: &str, b64: &str) -> Value {
        serde_json::json!({
            "id": "msg_1",
            "role": "assistant",
            "content": "",
            "timestamp": 0,
            "tool_calls": [{
                "id": "tc_1",
                "name": "mixer_generate_image",
                "status": "success",
                "artifacts": [{
                    "id": artifact_id,
                    "name": "generated-image-1.png",
                    "mime_type": "image/png",
                    "data_url": format!("data:image/png;base64,{b64}"),
                }]
            }]
        })
    }

    #[test]
    fn mixer_finds_same_turn_artifact_on_draft_not_main_json() {
        let conversation = conversation_from_messages(vec![]);
        let drafts: Vec<crate::chat::ChatMessage> = vec![serde_json::from_value(
            assistant_with_tool_artifact("art_new", "aGVsbG8="),
        )
        .expect("draft")];
        let found = resolve_mixer_artifacts(
            Some(&conversation),
            &drafts,
            &["art_new".to_string()],
        )
        .expect("draft artifact");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id.as_deref(), Some("art_new"));
    }

    #[test]
    fn mixer_unknown_artifact_errors_even_if_another_image_is_known() {
        let conversation = conversation_from_messages(vec![assistant_with_tool_artifact(
            "art_old",
            "aGVsbG8=",
        )]);
        let err = resolve_mixer_artifacts(
            Some(&conversation),
            &[],
            &["art_old".to_string(), "art_new".to_string()],
        )
        .expect_err("must not silently drop the missing id");
        assert!(err.contains("art_new"), "{err}");
        assert!(!err.contains("art_old") || err.contains("Unknown artifact_id: art_new"));
    }

    #[test]
    fn mixer_artifact_ids_require_a_conversation() {
        let err = resolve_mixer_artifacts(None, &[], &["art_new".to_string()])
            .expect_err("no conversation");
        assert!(err.contains("active conversation"), "{err}");
    }

    #[test]
    fn string_list_arg_dedupes_and_skips_blanks() {
        let arguments = serde_json::json!({
            "paths": ["a.png", " ", "a.png", "b.png"],
            "artifact_ids": "not-an-array",
        });
        assert_eq!(
            string_list_arg(&arguments, "paths"),
            vec!["a.png".to_string(), "b.png".to_string()]
        );
        assert!(string_list_arg(&arguments, "artifact_ids").is_empty());
        assert!(string_list_arg(&arguments, "missing").is_empty());
    }

    #[test]
    fn xai_reference_image_cap_is_three_and_does_not_silently_drop() {
        let xai = ModelProvider {
            id: "xai".to_string(),
            name: "xAI".to_string(),
            api_keys: vec!["k".to_string()],
            api_key_legacy: None,
            base_url: "https://api.x.ai/v1".to_string(),
            available_models: Vec::new(),
            enabled_models: Vec::new(),
            enabled: true,
            api_format: "xai_responses".to_string(),
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
            active_key_index: 0,
        };
        assert_eq!(max_reference_images(&xai, "grok-imagine-image"), 3);
        assert_eq!(
            max_reference_images(&gemini_provider(), "gemini-3.1-flash-image"),
            4
        );
        let err = reject_excess_reference_images(&xai, "grok-imagine-image", 4)
            .expect_err("xAI must refuse a 4th reference");
        assert!(err.contains("at most 3"), "{err}");
        assert!(err.contains("got 4"), "{err}");
        assert!(reject_excess_reference_images(&xai, "grok-imagine-image", 3).is_ok());
        assert!(
            reject_excess_reference_images(&gemini_provider(), "gemini-3.1-flash-image", 4).is_ok()
        );
        assert!(
            reject_excess_reference_images(&gemini_provider(), "gemini-3.1-flash-image", 5)
                .is_err()
        );

        let four = vec![
            sample_input_image(),
            InputImage {
                mime_type: "image/png".to_string(),
                base64: "two".to_string(),
            },
            InputImage {
                mime_type: "image/png".to_string(),
                base64: "three".to_string(),
            },
            InputImage {
                mime_type: "image/png".to_string(),
                base64: "four".to_string(),
            },
        ];
        let body = xai_edits_body(
            "grok-imagine-image",
            &ImageGenerationRequest {
                prompt: "sketch".to_string(),
                size: "auto".to_string(),
                quality: "auto".to_string(),
                n: 1,
                input_images: four[..3].to_vec(),
            },
        );
        assert_eq!(body["images"].as_array().map(|v| v.len()), Some(3));
    }
}
