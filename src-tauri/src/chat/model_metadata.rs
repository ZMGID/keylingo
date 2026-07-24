use std::sync::OnceLock;

use serde_json::Value;

use crate::settings::{ModelInfo, ModelPricing, ModelProvider};

const FALLBACK_CONTEXT_WINDOW_TOKENS: usize = 200_000;
const MIN_TEMPERATURE: f64 = 0.0;
const MAX_TEMPERATURE: f64 = 2.0;

fn context_window_from_model_info(info: Option<&ModelInfo>) -> Option<usize> {
    info.and_then(|info| info.context_window)
        .and_then(|tokens| usize::try_from(tokens).ok())
        .filter(|tokens| *tokens > 0)
}

fn max_output_from_model_info(info: Option<&ModelInfo>) -> Option<u32> {
    info.and_then(|info| info.max_output)
        .and_then(|tokens| u32::try_from(tokens).ok())
        .filter(|tokens| *tokens > 0)
}

fn valid_temperature(value: f64) -> Option<f64> {
    (value.is_finite() && (MIN_TEMPERATURE..=MAX_TEMPERATURE).contains(&value)).then_some(value)
}

fn temperature_from_model_info(info: Option<&ModelInfo>) -> Option<f64> {
    info.and_then(|info| info.temperature)
        .and_then(valid_temperature)
}

fn model_vision_from_model_info(info: Option<&ModelInfo>) -> Option<bool> {
    info.and_then(|info| info.capabilities.as_ref())
        .and_then(|capabilities| capabilities.vision)
}

fn model_image_generation_from_model_info(info: Option<&ModelInfo>) -> Option<bool> {
    info.and_then(|info| info.capabilities.as_ref())
        .and_then(|capabilities| capabilities.image_generation)
}

fn model_database_entries() -> Option<&'static serde_json::Map<String, Value>> {
    static MODEL_DATABASE: OnceLock<Value> = OnceLock::new();
    MODEL_DATABASE
        .get_or_init(|| {
            serde_json::from_str(include_str!("../../../src/data/modelDatabase.json"))
                .unwrap_or(Value::Null)
        })
        .as_object()
}

fn model_database_entry(model: &str) -> Option<&'static Value> {
    let model = model.trim();
    if model.is_empty() {
        return None;
    }

    let entries = model_database_entries()?;
    let name = model.to_ascii_lowercase();
    let stripped = name.rsplit('/').next().unwrap_or(&name);

    if let Some(entry) = entries.get(name.as_str()) {
        return Some(entry);
    }
    if let Some(entry) = entries.get(stripped) {
        return Some(entry);
    }

    let candidates = if name == stripped {
        vec![stripped]
    } else {
        vec![name.as_str(), stripped]
    };

    entries
        .iter()
        .filter_map(|(key, entry)| {
            if key == "_meta"
                || !candidates
                    .iter()
                    .any(|candidate| candidate.starts_with(key) && key.len() < candidate.len())
            {
                return None;
            }
            Some((key.len(), entry))
        })
        .max_by_key(|(key_len, _)| *key_len)
        .map(|(_, entry)| entry)
        .or_else(|| {
            entries
                .iter()
                .filter_map(|(key, entry)| {
                    if key == "_meta"
                        || !candidates
                            .iter()
                            .any(|candidate| key != candidate && candidate.contains(key))
                    {
                        return None;
                    }
                    Some((key.len(), entry))
                })
                .max_by_key(|(key_len, _)| *key_len)
                .map(|(_, entry)| entry)
        })
}

fn model_database_context_window(model: &str) -> Option<usize> {
    context_window_from_database_entry(model_database_entry(model))
}

fn model_database_max_output(model: &str) -> Option<u32> {
    max_output_from_database_entry(model_database_entry(model))
}

fn model_database_temperature(model: &str) -> Option<f64> {
    model_database_entry(model)?
        .get("temperature")
        .and_then(Value::as_f64)
        .and_then(valid_temperature)
}

fn context_window_from_database_entry(entry: Option<&Value>) -> Option<usize> {
    entry?
        .get("contextWindow")
        .and_then(Value::as_u64)
        .and_then(|tokens| usize::try_from(tokens).ok())
        .filter(|tokens| *tokens > 0)
}

fn max_output_from_database_entry(entry: Option<&Value>) -> Option<u32> {
    entry?
        .get("maxOutput")
        .and_then(Value::as_u64)
        .and_then(|tokens| u32::try_from(tokens).ok())
        .filter(|tokens| *tokens > 0)
}

fn model_database_vision(model: &str) -> Option<bool> {
    model_database_entry(model)?
        .get("capabilities")
        .and_then(|capabilities| capabilities.get("vision"))
        .and_then(Value::as_bool)
}

fn model_database_image_generation(model: &str) -> Option<bool> {
    model_database_entry(model)?
        .get("capabilities")
        .and_then(|capabilities| capabilities.get("imageGeneration"))
        .and_then(Value::as_bool)
}

/// 某模型支持的「思考等级」(reasoning effort) 列表，供前端的等级选择器决定显示哪些档。
/// 优先取模型库 `reasoningEfforts` 显式列表（各家支持不构成单调子集，故逐模型列举，便于维护）；
/// 库里没有时：Anthropic 家族给全档(low..max)，其余给通用安全子集 low/medium/high。
/// 始终只保留已知合法值并去重，避免脏数据进入请求。
pub fn reasoning_efforts_for_model(model: &str, api_format: &str) -> Vec<String> {
    const KNOWN: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
    if let Some(list) = model_database_entry(model)
        .and_then(|entry| entry.get("reasoningEfforts"))
        .and_then(Value::as_array)
    {
        let mut out: Vec<String> = Vec::new();
        for v in list {
            if let Some(s) = v.as_str() {
                if KNOWN.contains(&s) && !out.iter().any(|x| x == s) {
                    out.push(s.to_string());
                }
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    if api_format == "anthropic_messages" {
        return KNOWN.iter().map(|s| s.to_string()).collect();
    }
    vec!["low".into(), "medium".into(), "high".into()]
}

/// 把「每对话思考等级」规范成各家 wire 接受的 **reasoning effort 值**（任务 07-23 统一）。
///
/// 输入是 `resolve_thinking` 产出的规范等级：`off` 已在上游转为 `None`（关思考），故这里
/// 只会见到 `None` 或 `low|medium|high|xhigh|max`。这是**四个适配器共用的单一事实源**，
/// 避免各家各写一份映射而漂移（此前 openai/anthropic 原样透传、responses 各映一套、
/// gemini 干脆忽略等级）。各适配器仍各自负责把返回值塞进自己的 wire 字段
/// （openai `reasoning_effort` / responses `reasoning.effort` / anthropic
/// `output_config.effort` / gemini `thinkingConfig.thinkingLevel`）。
///
/// `None` = 不发 effort（关思考或未设且调用方不兜底）。
pub(crate) fn reasoning_effort_wire(
    kind: crate::settings::ProviderApiFormat,
    level: Option<&str>,
) -> Option<String> {
    use crate::settings::ProviderApiFormat::{
        AnthropicMessages, Gemini, OpenAiChat, OpenAiResponses,
    };
    let level = level.map(str::trim).filter(|l| !l.is_empty())?;
    let mapped = match kind {
        // OpenAI（Chat/Responses）、Gemini 3.x thinkingLevel：官方档 minimal/low/medium/high。
        // 我们的规范集无 minimal（off 已转 None）；xhigh/max 这两档它们不认，收敛到 high。
        OpenAiChat | OpenAiResponses | Gemini => match level {
            "low" | "medium" | "high" => level,
            _ => "high", // xhigh/max/未知 → high
        },
        // Anthropic output_config.effort：支持全档，原样透传（前端已按模型库 reasoningEfforts 门控）。
        AnthropicMessages => match level {
            "low" | "medium" | "high" | "xhigh" | "max" => level,
            _ => "high",
        },
    };
    Some(mapped.to_string())
}

fn model_pricing_from_model_info(info: Option<&ModelInfo>) -> Option<ModelPricing> {
    info.and_then(|info| info.pricing.clone())
}

fn model_database_pricing(model: &str) -> Option<ModelPricing> {
    let pricing = model_database_entry(model)?.get("pricing")?;
    Some(ModelPricing {
        input: pricing.get("input").and_then(Value::as_f64),
        output: pricing.get("output").and_then(Value::as_f64),
        cached_input: pricing.get("cachedInput").and_then(Value::as_f64),
    })
}

pub(crate) fn model_supports_vision(provider: Option<&ModelProvider>, model: &str) -> Option<bool> {
    let provider = provider?;
    model_vision_from_model_info(provider.model_overrides.get(model))
        .or_else(|| model_database_vision(model))
}

pub(crate) fn model_supports_image_generation(
    provider: Option<&ModelProvider>,
    model: &str,
) -> Option<bool> {
    let provider = provider?;
    model_image_generation_from_model_info(provider.model_overrides.get(model))
        .or_else(|| model_database_image_generation(model))
        .or_else(|| image_generation_model_name_heuristic(provider, model))
}

pub(crate) fn model_can_generate_images_directly(provider: &ModelProvider, model: &str) -> bool {
    model_supports_image_generation(Some(provider), model) == Some(true)
        && crate::chat::image_generation::has_known_direct_image_generation_route(provider, model)
}

/// 当前 provider 是否支持模型**原生内置联网搜索**（任务 07-23）。
/// 依 `api_format`：OpenAI Responses / Gemini / Anthropic Messages 支持；OpenAI Chat
/// Completions 不支持（gpt-5 在其上开 `web_search` 会 400）。前端据此把「内置」选项置灰。
pub(crate) fn builtin_web_search_supported(provider: &ModelProvider) -> bool {
    use crate::settings::ProviderApiFormat::{AnthropicMessages, Gemini, OpenAiResponses};
    matches!(
        provider.api_format_kind(),
        OpenAiResponses | Gemini | AnthropicMessages
    )
}

pub(crate) fn image_generation_model_for_session(
    settings: &crate::settings::Settings,
    session: Option<crate::settings::SessionModel<'_>>,
) -> Option<(String, String)> {
    if !settings
        .default_models
        .image_generation
        .provider_id
        .trim()
        .is_empty()
        && !settings
            .default_models
            .image_generation
            .model
            .trim()
            .is_empty()
    {
        return Some((
            settings.default_models.image_generation.provider_id.clone(),
            settings.default_models.image_generation.model.clone(),
        ));
    }
    let session = session.filter(|session| session.is_set())?;
    let provider = settings.get_provider(session.provider_id)?;
    if model_can_generate_images_directly(provider, session.model) {
        Some((session.provider_id.to_string(), session.model.to_string()))
    } else {
        None
    }
}

fn image_generation_model_name_heuristic(provider: &ModelProvider, model: &str) -> Option<bool> {
    let descriptor = format!(
        "{} {} {} {}",
        provider.name, provider.base_url, provider.api_format, model
    )
    .to_ascii_lowercase();
    let known_image_model = [
        "gpt-image",
        "dall-e",
        "grok-imagine-image",
        "gemini-3.1-flash-image",
        "gemini-3-pro-image",
        "gemini-2.5-flash-image",
        "flux",
        "recraft",
        "riverflow",
        "stable-diffusion",
        "sdxl",
        "ideogram",
        "imagen",
        "image-generation",
        "image_generation",
    ]
    .iter()
    .any(|needle| descriptor.contains(needle));
    if known_image_model {
        Some(true)
    } else {
        None
    }
}

/// 判定一个模型是否为**出图**模型（其响应会带图片）。用于 Gemini 原生 `generateContent`：
/// 生图模型才追加 `responseModalities:["TEXT","IMAGE"]`（普通文本模型加了会 400）。
/// 判据轻量：模型名（去掉可选 `models/` 前缀后）含 `image` 或以 `imagen` 开头，大小写不敏感。
pub fn is_image_output_model(model: &str) -> bool {
    let name = model
        .strip_prefix("models/")
        .unwrap_or(model)
        .to_ascii_lowercase();
    name.contains("image") || name.starts_with("imagen")
}

pub(crate) fn context_window_for_model(
    provider: Option<&ModelProvider>,
    model: &str,
) -> (usize, bool) {
    if let Some(tokens) = context_window_from_model_info(
        provider.and_then(|provider| provider.model_overrides.get(model)),
    ) {
        return (tokens, false);
    }
    if let Some(tokens) = model_database_context_window(model) {
        return (tokens, false);
    }

    let lower = model.to_ascii_lowercase();
    let known = [
        ("1m", 1_000_000usize),
        ("200k", 200_000usize),
        ("128k", 128_000usize),
        ("100k", 100_000usize),
        ("64k", 64_000usize),
        ("32k", 32_000usize),
        ("16k", 16_000usize),
        ("8k", 8_000usize),
    ];
    for (needle, tokens) in known {
        if lower.contains(needle) {
            return (tokens, false);
        }
    }
    if lower.contains("claude") {
        return (200_000, false);
    }
    if lower.contains("gpt-4o")
        || lower.contains("gpt-4.1")
        || lower.contains("gpt-5")
        || lower.contains("deepseek")
        || lower.contains("qwen")
        || lower.contains("gemini")
    {
        return (128_000, true);
    }
    (FALLBACK_CONTEXT_WINDOW_TOKENS, true)
}

pub(crate) fn chat_max_output_tokens_for_model(
    provider: Option<&ModelProvider>,
    model: &str,
    fallback: u32,
) -> u32 {
    max_output_from_model_info(provider.and_then(|provider| provider.model_overrides.get(model)))
        .or_else(|| model_database_max_output(model))
        .unwrap_or(fallback)
}

/// 解析模型级 temperature。用户显式清空优先于数据库值；所有来源都缺省时不发送。
pub(crate) fn temperature_for_model(provider: Option<&ModelProvider>, model: &str) -> Option<f64> {
    if let Some(info) = provider.and_then(|provider| provider.model_overrides.get(model)) {
        if info.omit_temperature == Some(true) {
            return None;
        }
        if let Some(temperature) = temperature_from_model_info(Some(info)) {
            return Some(temperature);
        }
    }
    model_database_temperature(model)
}

/// 单次请求显式值优先；非法显式值不会进入请求体，并回退到模型级配置。
pub(crate) fn temperature_for_request(
    explicit: Option<f64>,
    provider: Option<&ModelProvider>,
    model: &str,
) -> Option<f64> {
    explicit
        .and_then(valid_temperature)
        .or_else(|| temperature_for_model(provider, model))
}

pub(crate) fn pricing_for_model(
    provider: Option<&ModelProvider>,
    model: &str,
) -> Option<(ModelPricing, String)> {
    if let Some(pricing) = model_pricing_from_model_info(
        provider.and_then(|provider| provider.model_overrides.get(model)),
    ) {
        return Some((pricing, "user_override".to_string()));
    }
    model_database_pricing(model).map(|pricing| (pricing, "model_pricing".to_string()))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::settings::{ModelInfo, ModelProvider};

    use super::*;

    #[test]
    fn reasoning_efforts_resolve_from_db_family_and_default() {
        // 模型库显式列表：DeepSeek V4 含 xhigh+max（含用户的代理别名变体，靠前缀匹配）。
        let ds = reasoning_efforts_for_model("DeepSeek-V4-Flash", "openai_chat");
        assert!(
            ds.contains(&"max".to_string()) && ds.contains(&"xhigh".to_string()),
            "{ds:?}"
        );
        // GPT-5：有 xhigh、无 max。
        let gpt = reasoning_efforts_for_model("gpt-5.5", "openai_chat");
        assert!(
            gpt.contains(&"xhigh".to_string()) && !gpt.contains(&"max".to_string()),
            "{gpt:?}"
        );
        // Gemma：有 max、无 xhigh（非单调子集）。
        let gemma = reasoning_efforts_for_model("gemma4:31b", "openai_chat");
        assert!(
            gemma.contains(&"max".to_string()) && !gemma.contains(&"xhigh".to_string()),
            "{gemma:?}"
        );
        // 库里没有 + 非 Anthropic → 安全子集 low/medium/high。
        let unknown = reasoning_efforts_for_model("some-random-model", "openai_chat");
        assert_eq!(unknown, vec!["low", "medium", "high"]);
        // Anthropic 家族兜底 → 全档。
        let anth = reasoning_efforts_for_model("whatever", "anthropic_messages");
        assert!(
            anth.contains(&"xhigh".to_string()) && anth.contains(&"max".to_string()),
            "{anth:?}"
        );
        assert_eq!(
            reasoning_efforts_for_model("kimi-k3", "openai_chat"),
            vec!["max"]
        );
    }

    fn test_provider_with_overrides(model_overrides: HashMap<String, ModelInfo>) -> ModelProvider {
        ModelProvider {
            id: "provider".to_string(),
            name: "Provider".to_string(),
            api_keys: vec!["sk-test".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: Vec::new(),
            enabled_models: Vec::new(),
            enabled: true,
            api_format: "openai_chat".to_string(),
            model_overrides,
            compress_request_body: false,
        }
    }

    #[test]
    fn reasoning_effort_wire_maps_per_family() {
        use crate::settings::ProviderApiFormat::{
            AnthropicMessages, Gemini, OpenAiChat, OpenAiResponses,
        };
        // None / 空 ⇒ 不发。
        assert_eq!(reasoning_effort_wire(OpenAiChat, None), None);
        assert_eq!(reasoning_effort_wire(OpenAiResponses, Some("  ")), None);
        // OpenAI（Chat/Responses）+ Gemini：low/medium/high 透传，xhigh/max 收敛到 high。
        for kind in [OpenAiChat, OpenAiResponses, Gemini] {
            assert_eq!(
                reasoning_effort_wire(kind, Some("low")).as_deref(),
                Some("low")
            );
            assert_eq!(
                reasoning_effort_wire(kind, Some("high")).as_deref(),
                Some("high")
            );
            assert_eq!(
                reasoning_effort_wire(kind, Some("xhigh")).as_deref(),
                Some("high"),
                "xhigh 应收敛到 high: {kind:?}"
            );
            assert_eq!(
                reasoning_effort_wire(kind, Some("max")).as_deref(),
                Some("high")
            );
        }
        // Anthropic：全档原样透传。
        assert_eq!(
            reasoning_effort_wire(AnthropicMessages, Some("xhigh")).as_deref(),
            Some("xhigh")
        );
        assert_eq!(
            reasoning_effort_wire(AnthropicMessages, Some("max")).as_deref(),
            Some("max")
        );
    }

    #[test]
    fn builtin_web_search_supported_by_api_format() {
        let mut p = test_provider_with_overrides(HashMap::new());
        // Chat Completions（含别名/空）⇒ 不支持内置搜索（gpt-5 在其上会 400）。
        for fmt in ["openai_chat", "openai", ""] {
            p.api_format = fmt.into();
            assert!(
                !builtin_web_search_supported(&p),
                "should be unsupported: {fmt:?}"
            );
        }
        // Responses / Gemini / Anthropic（含各别名）⇒ 支持。
        for fmt in [
            "openai_responses",
            "responses",
            "gemini",
            "google",
            "anthropic",
            "anthropic_messages",
        ] {
            p.api_format = fmt.into();
            assert!(
                builtin_web_search_supported(&p),
                "should be supported: {fmt:?}"
            );
        }
    }

    #[test]
    fn context_window_uses_model_override_before_name_heuristic() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "deepseek-v4-flash".to_string(),
            ModelInfo {
                context_window: Some(1_048_576),
                ..ModelInfo::default()
            },
        );
        let provider = test_provider_with_overrides(overrides);

        assert_eq!(
            context_window_for_model(Some(&provider), "deepseek-v4-flash"),
            (1_048_576, false)
        );
    }

    #[test]
    fn is_image_output_model_matches_image_and_imagen() {
        assert!(is_image_output_model("gemini-3.1-flash-image"));
        assert!(is_image_output_model("models/gemini-2.5-flash-image"));
        assert!(is_image_output_model("Imagen-4"));
        assert!(is_image_output_model("models/imagen-3.0"));
        // 普通文本模型不判为出图（红线：非出图模型不会追加 responseModalities）。
        assert!(!is_image_output_model("gemini-3.1-flash-lite"));
        assert!(!is_image_output_model("models/gemini-2.5-pro"));
        assert!(!is_image_output_model("gpt-5"));
    }

    #[test]
    fn context_window_uses_builtin_model_database_defaults() {
        assert_eq!(
            context_window_for_model(None, "deepseek-v4-flash"),
            (1_048_576, false)
        );
    }

    #[test]
    fn chat_max_output_uses_builtin_model_database_defaults() {
        assert_eq!(
            chat_max_output_tokens_for_model(None, "deepseek-v4-flash", 32_768),
            131_072
        );
        assert_eq!(
            chat_max_output_tokens_for_model(None, "kimi-k3", 32_768),
            1_048_576
        );
    }

    #[test]
    fn chat_max_output_uses_model_override_before_database() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "deepseek-v4-flash".to_string(),
            ModelInfo {
                max_output: Some(65_536),
                ..ModelInfo::default()
            },
        );
        let provider = test_provider_with_overrides(overrides);

        assert_eq!(
            chat_max_output_tokens_for_model(Some(&provider), "deepseek-v4-flash", 32_768),
            65_536
        );
    }

    #[test]
    fn chat_max_output_falls_back_to_setting_when_metadata_missing() {
        assert_eq!(
            chat_max_output_tokens_for_model(None, "custom-model", 32_768),
            32_768
        );
    }

    #[test]
    fn temperature_is_absent_when_metadata_is_missing() {
        assert_eq!(temperature_for_model(None, "custom-model"), None);
    }

    #[test]
    fn temperature_uses_model_override() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "custom-model".to_string(),
            ModelInfo {
                temperature: Some(0.4),
                ..ModelInfo::default()
            },
        );
        let provider = test_provider_with_overrides(overrides);

        assert_eq!(
            temperature_for_model(Some(&provider), "custom-model"),
            Some(0.4)
        );
    }

    #[test]
    fn temperature_explicit_omit_wins_over_override_value() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "custom-model".to_string(),
            ModelInfo {
                temperature: Some(0.4),
                omit_temperature: Some(true),
                ..ModelInfo::default()
            },
        );
        let provider = test_provider_with_overrides(overrides);

        assert_eq!(temperature_for_model(Some(&provider), "custom-model"), None);
    }

    #[test]
    fn temperature_ignores_invalid_or_out_of_range_values() {
        for invalid in [f64::NAN, f64::INFINITY, -0.1, 2.1] {
            let mut overrides = HashMap::new();
            overrides.insert(
                "custom-model".to_string(),
                ModelInfo {
                    temperature: Some(invalid),
                    ..ModelInfo::default()
                },
            );
            let provider = test_provider_with_overrides(overrides);
            assert_eq!(
                temperature_for_model(Some(&provider), "custom-model"),
                None,
                "invalid temperature {invalid:?} should be ignored"
            );
        }
    }

    #[test]
    fn request_temperature_is_validated_and_wins_when_valid() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "custom-model".to_string(),
            ModelInfo {
                temperature: Some(0.4),
                ..ModelInfo::default()
            },
        );
        let provider = test_provider_with_overrides(overrides);

        assert_eq!(
            temperature_for_request(Some(1.2), Some(&provider), "custom-model"),
            Some(1.2)
        );
        assert_eq!(
            temperature_for_request(Some(3.0), Some(&provider), "custom-model"),
            Some(0.4)
        );
    }

    #[test]
    fn context_window_keeps_name_heuristic_when_metadata_missing() {
        assert_eq!(
            context_window_for_model(None, "custom-200k"),
            (200_000, false)
        );
        assert_eq!(
            context_window_for_model(None, "custom-deepseek-model"),
            (128_000, true)
        );
    }
}
