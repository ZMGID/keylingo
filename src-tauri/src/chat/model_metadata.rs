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

/// 用户模型库覆盖文件名，放在 app data 目录下（与 `settings.json` 同级）。
const USER_MODEL_DATABASE_FILE: &str = "modelDatabase.json";

/// 把用户覆盖逐 key 合并进内置基线：新 key 直接新增，已有 key **按顶层字段合并**
/// （只写 `reasoningEfforts` 不会把 `pricing` 抹掉；但 `capabilities` 这类嵌套对象是整块替换）。
/// key 统一小写，否则 `model_database_entry` 用小写模型名永远匹配不到，改了像没生效。
/// `_meta` 只是出处记录，不接受覆盖。
fn merge_user_entries(
    base: &mut serde_json::Map<String, Value>,
    user: serde_json::Map<String, Value>,
) {
    for (key, value) in user {
        let key = key.trim().to_ascii_lowercase();
        if key.is_empty() || key == "_meta" {
            continue;
        }
        match (base.get_mut(&key).and_then(Value::as_object_mut), value) {
            (Some(existing), Value::Object(fields)) => existing.extend(fields),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

fn load_user_model_database(path: &std::path::Path) -> Option<serde_json::Map<String, Value>> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Some(map),
        // 手写 JSON 打错了不能让整个模型库瘫掉，只警告并退回内置基线。
        _ => {
            eprintln!("[model_metadata] 忽略无效的用户模型库 {}", path.display());
            None
        }
    }
}

fn model_database_entries() -> Option<&'static serde_json::Map<String, Value>> {
    static MODEL_DATABASE: OnceLock<Value> = OnceLock::new();
    MODEL_DATABASE
        .get_or_init(|| {
            let mut base: Value =
                serde_json::from_str(include_str!("../../../src/data/modelDatabase.json"))
                    .unwrap_or(Value::Null);
            // 内置那份是编译期 `include_str!` 进二进制的基线；补新模型 / 改某个字段不必重编译，
            // 写 `<app_data>/modelDatabase.json` 即可（进程内只读一次，改完重启应用生效）。
            if let Some(map) = base.as_object_mut() {
                if let Some(user) = crate::app_data::app_data_dir()
                    .map(|dir| dir.join(USER_MODEL_DATABASE_FILE))
                    .filter(|path| path.exists())
                    .and_then(|path| load_user_model_database(&path))
                {
                    merge_user_entries(map, user);
                }
            }
            base
        })
        .as_object()
}

/// 版本分隔符归一化：数据库键用点号（`claude-sonnet-4.6`），不少 provider 返回连字符
/// （`claude-sonnet-4-6`）。与前端 `src/data/modelMatching.ts` 的 `normalizeSep` 同构。
fn normalize_model_sep(value: &str) -> String {
    value.replace('.', "-")
}

/// 版本延续判定：DB key 以数字结尾，且候选紧跟 1~2 位纯数字分段（`gpt-5` ← `gpt-5-6-luna`）
/// 时，不能退化到基础版本，否则会把未知次级版本错配成 `gpt-5`。
/// 日期/快照后缀（≥3 位数字段，如 `claude-opus-4-8-20260101`）不算版本延续。
/// 与前端 `isVersionContinuation` 同构。
fn is_version_continuation(key_norm: &str, candidate: &str, end_idx: usize) -> bool {
    let Some(last) = key_norm.chars().last() else {
        return false;
    };
    if !last.is_ascii_digit() {
        return false;
    }
    if candidate.as_bytes().get(end_idx) != Some(&b'-') {
        return false;
    }
    // `str::split` 至少产出一段（可为空），对齐前端 `[0]`。
    let next_segment = candidate[end_idx + 1..].split('-').next().unwrap();
    let len = next_segment.len();
    (1..=2).contains(&len) && next_segment.bytes().all(|b| b.is_ascii_digit())
}

/// 与前端模块级 `normalizedExact` / `normalizedEntries` 对应：进程内只建一次。
/// key 的 `&'static str` 来自 `model_database_entries` 的静态 JSON map。
struct ModelDbNormIndex {
    /// 归一化键 → 原始库 key（首个占位；当前库无仅靠 `.`/`-` 区分的重复键）
    exact_norm: std::collections::HashMap<String, &'static str>,
    /// `(原始 key, 归一化 key)`，供前缀 / 包含匹配复用
    pairs: Vec<(&'static str, String)>,
}

fn model_db_norm_index() -> Option<&'static ModelDbNormIndex> {
    static INDEX: OnceLock<Option<ModelDbNormIndex>> = OnceLock::new();
    INDEX
        .get_or_init(|| {
            let entries = model_database_entries()?;
            let mut exact_norm = std::collections::HashMap::new();
            let mut pairs = Vec::new();
            for key in entries.keys() {
                if key == "_meta" {
                    continue;
                }
                let norm = normalize_model_sep(key);
                exact_norm.entry(norm.clone()).or_insert(key.as_str());
                pairs.push((key.as_str(), norm));
            }
            Some(ModelDbNormIndex { exact_norm, pairs })
        })
        .as_ref()
}

/// 从内置模型库匹配条目。
///
/// 与前端 `matchModel` / `matchModelExact` **同构**（精确 → 归一化精确 → 最长前缀 →
/// 最长包含；均带版本延续保护）。两边必须命中同一条记录，否则 UI 展示的 efforts/定价
/// 会和真实请求侧不一致（例如 `claude-sonnet-4-6` 前端认 4.6、后端退化到 4）。
///
/// 不做生图命名启发式（那是前端 `matchKnownImageGenerationModel` 的展示层兜底）；
/// 也不为某个 CLI 别名（如 kimi 的 `k3`）放宽匹配——那会污染所有 provider。
fn model_database_entry(model: &str) -> Option<&'static Value> {
    let model = model.trim();
    if model.is_empty() {
        return None;
    }

    let entries = model_database_entries()?;
    let name = model.to_ascii_lowercase();
    let stripped = name.rsplit('/').next().unwrap_or(name.as_str());

    // 1. 精确匹配（含 OpenRouter 风格 `provider/model` 去前缀）
    if let Some(entry) = entries.get(name.as_str()) {
        return Some(entry);
    }
    if stripped != name.as_str() {
        if let Some(entry) = entries.get(stripped) {
            return Some(entry);
        }
    }

    let index = model_db_norm_index()?;

    // 1b. 分隔符归一化后的精确匹配（`claude-sonnet-4-6` → `claude-sonnet-4.6`）
    if let Some(orig) = index
        .exact_norm
        .get(&normalize_model_sep(&name))
        .or_else(|| index.exact_norm.get(&normalize_model_sep(stripped)))
    {
        return entries.get(*orig);
    }

    let norm_name = normalize_model_sep(&name);
    let norm_stripped = normalize_model_sep(stripped);
    let candidates: Vec<&str> = if norm_name == norm_stripped {
        vec![norm_stripped.as_str()]
    } else {
        vec![norm_name.as_str(), norm_stripped.as_str()]
    };

    // 2. 前缀匹配（归一化后最长 key 优先，带版本延续保护）
    let mut best_prefix: Option<(&str, usize)> = None;
    for (orig, norm) in &index.pairs {
        let norm_len = norm.len();
        let hit = candidates.iter().any(|candidate| {
            candidate.starts_with(norm.as_str())
                && norm_len < candidate.len()
                && !is_version_continuation(norm, candidate, norm_len)
        });
        if hit && best_prefix.map_or(true, |(_, len)| norm_len > len) {
            best_prefix = Some((*orig, norm_len));
        }
    }
    if let Some((orig, _)) = best_prefix {
        return entries.get(orig);
    }

    // 3. 包含匹配（归一化后最长 key 优先，带版本延续保护）
    let mut best_contains: Option<(&str, usize)> = None;
    for (orig, norm) in &index.pairs {
        let norm_len = norm.len();
        let hit = candidates.iter().any(|candidate| {
            if norm.as_str() == *candidate {
                return false;
            }
            match candidate.find(norm.as_str()) {
                Some(idx) => !is_version_continuation(norm, candidate, idx + norm_len),
                None => false,
            }
        });
        if hit && best_contains.map_or(true, |(_, len)| norm_len > len) {
            best_contains = Some((*orig, norm_len));
        }
    }
    best_contains.and_then(|(orig, _)| entries.get(orig))
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
/// 三层优先级：
/// 1. provider 的 `model_overrides[model].reasoningEfforts`（模型详情抽屉里手填，改完即生效）；
/// 2. 模型库 `reasoningEfforts` 显式列表（各家支持不构成单调子集，故逐模型列举）；
/// 3. 都没有才按家族兜底：Anthropic 给全档(low..max)，其余给通用安全子集 low/medium/high。
///
/// 前两层的**显式空数组 = 该模型没有 effort 旋钮**（Anthropic 4.6 以下、GLM-4.7、Kimi K2.x、
/// 通义……），上游 `resolve_thinking` 据此不下发任何等级字段。
/// 始终只保留已知合法值并去重，避免脏数据进入请求。
pub fn reasoning_efforts_for_model(provider: Option<&ModelProvider>, model: &str) -> Vec<String> {
    if let Some(list) = provider
        .and_then(|provider| override_model_info(provider, model))
        .and_then(|info| info.reasoning_efforts.as_deref())
    {
        return sanitize_efforts(list.iter().map(String::as_str));
    }
    if let Some(list) = model_database_entry(model)
        .and_then(|entry| entry.get("reasoningEfforts"))
        .and_then(Value::as_array)
    {
        return sanitize_efforts(list.iter().filter_map(Value::as_str));
    }
    if provider.map(ModelProvider::api_format_kind)
        == Some(crate::settings::ProviderApiFormat::AnthropicMessages)
    {
        return REASONING_EFFORT_LEVELS
            .iter()
            .map(|s| s.to_string())
            .collect();
    }
    vec!["low".into(), "medium".into(), "high".into()]
}

const REASONING_EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

fn sanitize_efforts<'a>(items: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        if REASONING_EFFORT_LEVELS.contains(&item) && !out.iter().any(|kept| kept == item) {
            out.push(item.to_string());
        }
    }
    out
}

// 思考等级不再在此做「协议级白名单 → 收敛」的二次映射。哪个模型认哪几档是**逐模型**的
// （xhigh 在 Chat Completions 和 Responses 两个端点都收，但 gpt-5-codex 传了就 400），
// 用 api_format 粒度去卡必然错，且静默改档会让用户在 UI 选了 xhigh 却发出 high，毫无信号。
// 唯一判据是模型库的 `reasoningEfforts`（见 `reasoning_efforts_for_model`），前端选择器只
// 渲染这个列表；值本身已被校验两遍（落盘 `update_conversation_settings` + 取用
// `resolve_thinking`），到适配器手上必是 `low|medium|high|xhigh|max`。选错档就吃 provider
// 的 400 —— 那是正确的报错，不是需要兜底的异常。

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

/// 归一化模型名：小写 + 去 `models/` 前缀 + trim。出图路由 / override 生图能力判定 /
/// `is_image_output_model` / 名字启发式统一走这里，消除「换大小写/加 `models/` 前缀就路由错、
/// override 精确匹配静默失效」三类脆弱。
pub(crate) fn normalize_model_name(model: &str) -> String {
    let lower = model.trim().to_ascii_lowercase();
    lower
        .strip_prefix("models/")
        .unwrap_or(&lower)
        .trim()
        .to_string()
}

pub(crate) fn model_supports_image_generation(
    provider: Option<&ModelProvider>,
    model: &str,
) -> Option<bool> {
    let provider = provider?;
    override_image_generation(provider, model)
        .or_else(|| model_database_image_generation(model))
        .or_else(|| image_generation_model_name_heuristic(provider, model))
}

/// 读 `model_overrides` 的生图能力：先精确 `get(model)`，未命中再按**归一化名**遍历匹配
/// （小 HashMap，O(n) 可接受），消除大小写 / `models/` 前缀导致 override 静默失效。
fn override_image_generation(provider: &ModelProvider, model: &str) -> Option<bool> {
    model_image_generation_from_model_info(override_model_info(provider, model))
}

/// 取该模型的用户覆盖：先精确 `get(model)`，未命中再按**归一化名**遍历匹配
/// （小 HashMap，O(n) 可接受），消除大小写 / `models/` 前缀导致 override 静默失效。
fn override_model_info<'a>(provider: &'a ModelProvider, model: &str) -> Option<&'a ModelInfo> {
    if let Some(info) = provider.model_overrides.get(model) {
        return Some(info);
    }
    let normalized = normalize_model_name(model);
    provider
        .model_overrides
        .iter()
        .find(|(key, _)| normalize_model_name(key) == normalized)
        .map(|(_, info)| info)
}

pub(crate) fn model_can_generate_images_directly(provider: &ModelProvider, model: &str) -> bool {
    model_supports_image_generation(Some(provider), model) == Some(true)
        && crate::chat::image_generation::has_known_direct_image_generation_route(provider, model)
}

/// 当前 provider 是否支持模型**原生内置联网搜索**（任务 07-23）。
/// 依 `api_format`：OpenAI Responses / Gemini / Anthropic Messages 支持；OpenAI Chat
/// Completions 不支持（gpt-5 在其上开 `web_search` 会 400）。前端据此把「内置」选项置灰。
pub(crate) fn builtin_web_search_supported(provider: &ModelProvider) -> bool {
    use crate::settings::ProviderApiFormat::{
        AnthropicMessages, Gemini, OpenAiResponses, XaiResponses,
    };
    // xAI 的 Responses 同样有服务端 web_search（还多 x_search），与 OpenAI Responses 同形。
    matches!(
        provider.api_format_kind(),
        OpenAiResponses | XaiResponses | Gemini | AnthropicMessages
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
        provider.name,
        provider.base_url,
        provider.api_format,
        normalize_model_name(model)
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
/// 判据轻量：归一化名（小写 + 去可选 `models/` 前缀）含 `image` 或以 `imagen` 开头。
pub fn is_image_output_model(model: &str) -> bool {
    let name = normalize_model_name(model);
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
        // 256k：`kimi-code/k3-256k` 这类名字里明写窗口的模型，缺这一项会一路掉到
        // FALLBACK_CONTEXT_WINDOW_TOKENS(200K)。值取 2^18（对齐 modelDatabase 里 kimi 系列）。
        ("256k", 262_144usize),
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

    fn db_display_name(model: &str) -> Option<String> {
        model_database_entry(model)
            .and_then(|entry| entry.get("displayName"))
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// 与前端 `src/data/modelMatching.test.ts` 对齐：连字符版本 / 版本延续 / 日期后缀。
    #[test]
    fn model_database_matching_is_isomorphic_with_frontend() {
        // 空白 → 无匹配
        assert!(model_database_entry("").is_none());
        assert!(model_database_entry("   ").is_none());

        // 点号键 ↔ 连字符 id
        assert_eq!(
            db_display_name("claude-sonnet-4-6").as_deref(),
            Some("Claude Sonnet 4.6")
        );
        assert_eq!(
            db_display_name("claude-opus-4-8").as_deref(),
            Some("Claude Opus 4.8")
        );
        assert_eq!(
            db_display_name("claude-opus-4-7").as_deref(),
            Some("Claude Opus 4.7")
        );
        assert_eq!(
            db_display_name("claude-haiku-4-5").as_deref(),
            Some("Claude Haiku 4.5")
        );
        assert_eq!(db_display_name("kimi-k2-7").as_deref(), Some("Kimi K2.7"));

        // 主版本本身仍命中自己的条目，不能被次级版本抢
        assert_eq!(
            db_display_name("claude-sonnet-4").as_deref(),
            Some("Claude Sonnet 4")
        );
        assert_eq!(
            db_display_name("claude-opus-4").as_deref(),
            Some("Claude Opus 4")
        );

        // 日期快照后缀：最长归一化前缀，仍认 4.8（数字段 ≥3 不算版本延续）
        assert_eq!(
            db_display_name("claude-opus-4-8-20260101").as_deref(),
            Some("Claude Opus 4.8")
        );

        // 版本延续保护：未知次级版本不能退化到 gpt-5
        assert!(model_database_entry("gpt-5.7-nebula").is_none());
        // 连字符写法的已知 5.6 变体要认到 5.6 条目，不能落到 gpt-5
        assert_eq!(
            db_display_name("gpt-5-6-luna").as_deref(),
            Some("GPT-5.6 Luna")
        );
        assert_eq!(
            db_display_name("gpt-5.6-luna").as_deref(),
            Some("GPT-5.6 Luna")
        );
        assert_eq!(
            db_display_name("gpt-5.6-sol").as_deref(),
            Some("GPT-5.6 Sol")
        );
        assert_eq!(
            db_display_name("gpt-5.6-terra").as_deref(),
            Some("GPT-5.6 Terra")
        );
        assert_eq!(db_display_name("gpt-5.5").as_deref(), Some("GPT-5.5"));
        assert_eq!(db_display_name("gpt-5").as_deref(), Some("GPT-5"));

        // provider 前缀（含连字符版本 id）+ 点号键本身
        assert_eq!(db_display_name("openai/gpt-4o"), db_display_name("gpt-4o"));
        assert_eq!(
            db_display_name("anthropic/claude-sonnet-4-6").as_deref(),
            Some("Claude Sonnet 4.6")
        );
        assert_eq!(
            db_display_name("claude-sonnet-4.6").as_deref(),
            Some("Claude Sonnet 4.6")
        );

        // 包含匹配路径：带 tag 的变体（`gemma4:31b`）靠前缀/包含吃到 `gemma4`
        assert_eq!(db_display_name("gemma4:31b").as_deref(), Some("Gemma 4"));

        // 未知模型
        assert!(model_database_entry("totally-unknown-model-xyz-9999").is_none());

        // 有意不放宽：kimi CLI 短别名 `k3` 不得命中 `kimi-k3`（见 external_agents/context.rs）
        assert!(model_database_entry("k3").is_none());
    }

    /// 连字符 id 必须吃到正确条目的 efforts，不能退化到旧主版本的 `[]`。
    #[test]
    fn dash_versioned_ids_resolve_correct_reasoning_efforts() {
        // 4.6 有 low..max；旧 `claude-sonnet-4` 是显式空数组（无旋钮）。
        assert_eq!(
            reasoning_efforts_for_model(None, "claude-sonnet-4-6"),
            vec!["low", "medium", "high", "max"]
        );
        assert_eq!(
            reasoning_efforts_for_model(None, "anthropic/claude-sonnet-4-6"),
            vec!["low", "medium", "high", "max"]
        );
        assert!(reasoning_efforts_for_model(None, "claude-sonnet-4").is_empty());
        // haiku-4-5 在库里是 `[]`；旧逻辑连字符 id 会 miss 整条，掉进家族兜底。
        assert!(reasoning_efforts_for_model(None, "claude-haiku-4-5").is_empty());
        assert!(reasoning_efforts_for_model(None, "claude-haiku-4.5").is_empty());
    }

    #[test]
    fn user_model_database_overlay_merges_fields_and_normalizes_keys() {
        let mut base = serde_json::json!({
            "_meta": { "version": "builtin" },
            "gpt-5.6": { "contextWindow": 256000, "pricing": { "input": 5.0 } },
        })
        .as_object()
        .unwrap()
        .clone();
        let user = serde_json::json!({
            "_meta": { "version": "user" },
            "GPT-5.6": { "reasoningEfforts": ["max"] },
            "My-New-Model": { "contextWindow": 999 },
        })
        .as_object()
        .unwrap()
        .clone();

        merge_user_entries(&mut base, user);

        // 已有 key：按顶层字段合并，pricing 不被抹掉；key 大小写归一。
        let gpt = &base["gpt-5.6"];
        assert_eq!(gpt["pricing"]["input"], 5.0);
        assert_eq!(gpt["contextWindow"], 256000);
        assert_eq!(gpt["reasoningEfforts"], serde_json::json!(["max"]));
        // 新 key 直接新增，且键名已小写（否则匹配不到）。
        assert_eq!(base["my-new-model"]["contextWindow"], 999);
        // _meta 不接受覆盖。
        assert_eq!(base["_meta"]["version"], "builtin");
    }

    #[test]
    fn reasoning_efforts_resolve_from_db_family_and_default() {
        // 模型库显式列表：DeepSeek V4 含 xhigh+max（含用户的代理别名变体，靠前缀匹配）。
        // 库里能查到时 provider 无关，传 None。
        let ds = reasoning_efforts_for_model(None, "DeepSeek-V4-Flash");
        assert!(
            ds.contains(&"max".to_string()) && ds.contains(&"xhigh".to_string()),
            "{ds:?}"
        );
        // GPT-5：有 xhigh、无 max（max 是 5.6 一代才加的）。
        let gpt = reasoning_efforts_for_model(None, "gpt-5.5");
        assert!(
            gpt.contains(&"xhigh".to_string()) && !gpt.contains(&"max".to_string()),
            "{gpt:?}"
        );
        // gpt-5.6 全家（含 sol/terra/luna）的 reasoning_options 含 max。
        for model in ["gpt-5.6", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            assert_eq!(
                reasoning_efforts_for_model(None, model),
                vec!["low", "medium", "high", "xhigh", "max"],
                "{model}"
            );
        }
        // Gemma：有 max、无 xhigh（非单调子集）。
        let gemma = reasoning_efforts_for_model(None, "gemma4:31b");
        assert!(
            gemma.contains(&"max".to_string()) && !gemma.contains(&"xhigh".to_string()),
            "{gemma:?}"
        );
        // 库里没有 + 非 Anthropic → 安全子集 low/medium/high。
        let unknown = reasoning_efforts_for_model(None, "some-random-model");
        assert_eq!(unknown, vec!["low", "medium", "high"]);
        // Anthropic 家族兜底 → 全档。api_format 用设置里真实存的别名 "anthropic"（不是
        // 规范名），归一化后同样命中。
        let mut anthropic = test_provider_with_overrides(HashMap::new());
        for raw in ["anthropic", "anthropic_messages"] {
            anthropic.api_format = raw.to_string();
            let anth = reasoning_efforts_for_model(Some(&anthropic), "whatever");
            assert!(
                anth.contains(&"xhigh".to_string()) && anth.contains(&"max".to_string()),
                "{raw}: {anth:?}"
            );
        }
        // Kimi K3：reasoning_effort 只认 low/high/max（无 medium/xhigh）。
        assert_eq!(
            reasoning_efforts_for_model(None, "kimi-k3"),
            vec!["low", "high", "max"]
        );
    }

    #[test]
    fn model_override_reasoning_efforts_wins_over_database() {
        // 模型详情抽屉里手填的档位优先于模型库；脏值被过滤、重复被去掉；
        // 大小写 / `models/` 前缀不同也要命中（走 normalize_model_name）。
        let mut overrides = HashMap::new();
        overrides.insert(
            "GPT-5.6".to_string(),
            ModelInfo {
                reasoning_efforts: Some(vec![
                    "low".into(),
                    "low".into(),
                    "bogus".into(),
                    "max".into(),
                ]),
                ..Default::default()
            },
        );
        let provider = test_provider_with_overrides(overrides);
        assert_eq!(
            reasoning_efforts_for_model(Some(&provider), "gpt-5.6"),
            vec!["low", "max"]
        );
        // 未被覆盖的模型仍走模型库。
        assert_eq!(
            reasoning_efforts_for_model(Some(&provider), "kimi-k3"),
            vec!["low", "high", "max"]
        );

        // 显式空数组 = 用户宣布这个模型没有 effort 旋钮，压过模型库的非空列表。
        let mut muted = HashMap::new();
        muted.insert(
            "gpt-5.6".to_string(),
            ModelInfo {
                reasoning_efforts: Some(Vec::new()),
                ..Default::default()
            },
        );
        assert!(reasoning_efforts_for_model(
            Some(&test_provider_with_overrides(muted)),
            "models/GPT-5.6"
        )
        .is_empty());
    }

    #[test]
    fn explicit_empty_list_means_no_effort_knob() {
        // 显式 `[]` 必须原样返回空，不能掉进家族兜底 —— 上游 `resolve_thinking` 靠它判定
        // 「这个模型没有 effort 旋钮」。Claude 4.5 及更早传 output_config.effort 会 400。
        let mut anthropic = test_provider_with_overrides(HashMap::new());
        anthropic.api_format = "anthropic".to_string();
        for model in [
            "claude-sonnet-4.5",
            "claude-opus-4",
            "claude-haiku-4.5",
            "claude-3.5-haiku",
            "glm-4.7",     // 思考是模式控制，没有 effort
            "kimi-k2.7",   // 思考强制开，无 effort
            "qwen3.7-max", // 用 thinking_budget 数值预算
            "minimax-m3",
        ] {
            assert!(
                reasoning_efforts_for_model(Some(&anthropic), model).is_empty(),
                "{model} 应为空档位"
            );
        }
        // 4.6 与 4.7 的分界：4.6 止步 max，没有 xhigh。
        assert_eq!(
            reasoning_efforts_for_model(Some(&anthropic), "claude-opus-4.6"),
            vec!["low", "medium", "high", "max"]
        );
        assert_eq!(
            reasoning_efforts_for_model(Some(&anthropic), "claude-opus-4.7"),
            vec!["low", "medium", "high", "xhigh", "max"]
        );
    }

    #[test]
    fn xhigh_gated_per_model_not_per_protocol() {
        // 删掉 reasoning_effort_wire 的协议级白名单后，模型库是**唯一**门控：适配器原样下发
        // 用户所选档位，选错就吃 provider 的 400。所以这份数据必须准，尤其是 xhigh 这一档
        // ——它随 gpt-5.1-codex-max 引入，之前的 gpt-5 / gpt-5-codex 传了会 400。
        for model in ["gpt-5", "gpt-5-pro", "gpt-5-codex"] {
            assert!(
                !reasoning_efforts_for_model(None, model).contains(&"xhigh".to_string()),
                "{model} 不支持 xhigh，不能出现在选择器里"
            );
        }
        for model in ["gpt-5.4", "gpt-5.6", "grok-4.6"] {
            assert!(
                reasoning_efforts_for_model(None, model).contains(&"xhigh".to_string()),
                "{model} 支持 xhigh"
            );
        }
        // Gemini thinkingLevel 只有 minimal/low/medium/high，无 xhigh/max；库里无条目，
        // 走非 Anthropic 兜底 low/medium/high 即正确。
        let mut gemini = test_provider_with_overrides(HashMap::new());
        gemini.api_format = "gemini".to_string();
        assert_eq!(
            reasoning_efforts_for_model(Some(&gemini), "gemini-3.2-pro"),
            vec!["low", "medium", "high"]
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
            request: Default::default(),
        }
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
    fn normalize_model_name_lowercases_strips_models_prefix_and_trims() {
        assert_eq!(
            normalize_model_name("Gemini-3.1-Flash-Image"),
            "gemini-3.1-flash-image"
        );
        assert_eq!(
            normalize_model_name("models/Gemini-3.1-Flash-Image"),
            "gemini-3.1-flash-image"
        );
        assert_eq!(normalize_model_name("  MODELS/imagen-4  "), "imagen-4");
        // 裸名与带前缀 / 大小写归一到同一结果。
        assert_eq!(
            normalize_model_name("gemini-3.1-flash-image"),
            normalize_model_name("models/Gemini-3.1-Flash-Image")
        );
    }

    #[test]
    fn override_image_generation_matches_by_normalized_name() {
        // override key 用带前缀 + 大小写变体，仍应命中查询的裸名（此前精确匹配会静默失效）。
        let mut overrides = HashMap::new();
        overrides.insert(
            "models/Gemini-3.1-Flash-Image".to_string(),
            ModelInfo {
                capabilities: Some(crate::settings::ModelCapabilities {
                    image_generation: Some(true),
                    ..Default::default()
                }),
                ..ModelInfo::default()
            },
        );
        let provider = test_provider_with_overrides(overrides);

        // 归一化后命中 override 的生图开关。
        assert_eq!(
            override_image_generation(&provider, "gemini-3.1-flash-image"),
            Some(true)
        );
        assert_eq!(
            override_image_generation(&provider, "GEMINI-3.1-FLASH-IMAGE"),
            Some(true)
        );
        assert_eq!(
            model_supports_image_generation(Some(&provider), "gemini-3.1-flash-image"),
            Some(true)
        );
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

    #[test]
    fn context_window_recognizes_256k_in_model_name() {
        // 关键词表缺 256k 时这两个会掉到 200K 兜底（kimi k3-256k 的真实症状）。
        assert_eq!(
            context_window_for_model(None, "kimi-code/k3-256k"),
            (262_144, false)
        );
        assert_eq!(
            context_window_for_model(None, "some-model-256K"),
            (262_144, false)
        );
    }
}
