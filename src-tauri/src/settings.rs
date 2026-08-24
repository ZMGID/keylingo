use chrono::{Datelike, Local};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_store::StoreBuilder;

// 设置存储文件名
const SETTINGS_STORE: &str = "settings.json";
const LEGACY_APPLE_INTELLIGENCE_BASE_URL: &str = "applefoundation://local";

// ========== 数据结构定义 ==========

/**
 * 旧版 OpenAI 配置（用于迁移兼容）
 */
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OpenAIConfig {
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_openai_base_url")]
    pub base_url: String,
    #[serde(default = "default_openai_model")]
    pub model: String,
}

impl Default for OpenAIConfig {
    fn default() -> Self {
        Self {
            api_key: "".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-4o".to_string(),
        }
    }
}

/// 供应商级自定义请求头的一条。key/value 的合法性由 `provider_request` 校验，
/// 非法或命中保留名单的条目在 `sanitize_settings` 里丢弃（设置文件可被手改，后端必须自己拦）。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ProviderCustomHeader {
    pub key: String,
    pub value: String,
}

/// 供应商级「请求配置」：自定义请求头 / 代理 / prompt 缓存 / CLI 身份伪装。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderRequestConfig {
    /// 附加到该供应商所有请求上的自定义头。同名时覆盖 CLI 身份预设。
    pub custom_headers: Vec<ProviderCustomHeader>,
    /// 是否跟随系统代理。默认 true —— 与加这个开关之前的行为一致；关掉才走直连。
    pub use_system_proxy: bool,
    /// 遗留 on/off。sanitize 时迁移进 `prompt_cache_retention` 后清空，新配置不再写入。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_caching: Option<bool>,
    /// Prompt 缓存策略（对齐 pi `CacheRetention`）：
    /// - `none`：不发客户端缓存字段
    /// - `short`（默认）：OpenAI 发 `prompt_cache_key`；Anthropic 打 ephemeral 断点
    /// - `long`：在 short 上叠加长保留（Anthropic `ttl:1h` / OpenAI `prompt_cache_retention:24h`）
    pub prompt_cache_retention: String,
    /// CLI 身份伪装：`""` 关闭 | `claude_code` | `codex` | `grok`。
    pub cli_identity: String,
    /// 身份版本号，空则用内置常量。
    pub cli_identity_version: String,
}

impl Default for ProviderRequestConfig {
    fn default() -> Self {
        Self {
            custom_headers: Vec::new(),
            use_system_proxy: true,
            prompt_caching: None,
            prompt_cache_retention: "short".to_string(),
            cli_identity: String::new(),
            cli_identity_version: String::new(),
        }
    }
}

/**
 * AI 模型提供商配置
 *
 * api_keys 支持多 key failover：第一个为主 key，后续为备用 key；
 * 当某个 key 触发配额/限流/鉴权失败时会自动切换到下一个。
 *
 * api_key_legacy 字段仅用于反序列化兼容旧版（v2.3.1 及之前）单 key 配置，
 * sanitize_settings 会把它合并到 api_keys[0]。
 */
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProvider {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub api_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "apiKey")]
    pub api_key_legacy: Option<String>,
    pub base_url: String,
    #[serde(default)]
    pub available_models: Vec<String>,
    #[serde(default)]
    pub enabled_models: Vec<String>,
    /// 关闭后该供应商不会出现在模型选择器中，已引用它的功能会在保存时切到第一个启用的供应商。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// API 格式：`openai_chat` 或 `anthropic_messages`。
    /// 旧值 `openai` / `anthropic` 会在 `sanitize_settings` 中归一化。
    #[serde(default = "default_api_format")]
    pub api_format: String,
    /// 用户自定义的模型参数覆盖（仅持久化用户显式修改的字段）
    #[serde(default)]
    pub model_overrides: std::collections::HashMap<String, ModelInfo>,
    /// 是否对请求体做 gzip 压缩再发送。默认关。
    /// 用于个别供应商前置的 Cloudflare WAF 会扫明文请求体、把 agent 工具/系统提示里的
    /// shell 命令、文件路径、SQL 等文本误判为攻击而返回 403「Blocked」的情况——
    /// gzip 后 WAF 不解析压缩体即可放行（后端需接受 gzip 请求，多数 OpenAI 兼容网关支持）。
    /// 不接受 gzip 的供应商（如官方 DeepSeek）请保持关闭，否则会 400。
    #[serde(default)]
    pub compress_request_body: bool,
    /// 「请求配置」：自定义头 / 代理 / prompt 缓存 / CLI 身份。
    #[serde(default)]
    pub request: ProviderRequestConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderApiFormat {
    OpenAiChat,
    AnthropicMessages,
    /// OpenAI Responses API (`POST /v1/responses`). Used by Codex / Responses-native
    /// models and proxies that only emit tool-call arguments over this protocol.
    OpenAiResponses,
    /// Google Gemini native `generateContent` protocol. Avoids the OpenAI-compat
    /// endpoint's strict rejection of unknown fields (e.g. `promptCacheKey` 400).
    Gemini,
    /// xAI (Grok) 的 Responses 端点。**线协议与 `OpenAiResponses` 相同**，走同一个适配器；
    /// 区别是 xAI 严格拒收一批 OpenAI 专有字段（`store` / `prompt_cache_key` /
    /// `instructions` / `metadata` / `stream_options` …），思考档位也是它自己的一套
    /// （`none|low|medium|high|xhigh`）。做成独立协议而不是按 base_url 猜：中转站可以把
    /// grok 挂在任意域名上，靠域名判断必然漏。
    XaiResponses,
}

impl ProviderApiFormat {
    pub fn from_raw(raw: &str) -> Self {
        match raw.trim() {
            "anthropic" | "anthropic_messages" => Self::AnthropicMessages,
            "openai_responses" | "responses" => Self::OpenAiResponses,
            "gemini" | "google" | "gemini_generate" => Self::Gemini,
            "xai" | "xai_responses" | "grok" => Self::XaiResponses,
            _ => Self::OpenAiChat,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiChat => "openai_chat",
            Self::AnthropicMessages => "anthropic_messages",
            Self::OpenAiResponses => "openai_responses",
            Self::Gemini => "gemini",
            Self::XaiResponses => "xai_responses",
        }
    }
}

impl ModelProvider {
    pub fn api_format_kind(&self) -> ProviderApiFormat {
        ProviderApiFormat::from_raw(&self.api_format)
    }

    /// Prompt 缓存策略。非法/空串视为 `short`（与 sanitize 一致）。
    pub fn cache_retention(&self) -> CacheRetention {
        CacheRetention::parse(&self.request.prompt_cache_retention)
    }

    /// 是否发送客户端缓存字段（`retention != none`）。
    /// Gemini / xAI 适配器本身不发字段；此处只表示用户策略。
    pub fn prompt_caching_enabled(&self) -> bool {
        !matches!(self.cache_retention(), CacheRetention::None)
    }
}

/// 对齐 pi：`none | short | long`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheRetention {
    None,
    Short,
    Long,
}

impl CacheRetention {
    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "none" => Self::None,
            "long" => Self::Long,
            _ => Self::Short,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Short => "short",
            Self::Long => "long",
        }
    }
}

/**
 * 模型能力信息（来自内置数据库或用户自定义）
 */
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelInfo {
    pub display_name: Option<String>,
    pub context_window: Option<u64>,
    pub max_output: Option<u64>,
    /// 模型级采样温度；None 表示请求默认不发送 temperature。
    pub temperature: Option<f64>,
    /// 用户显式清空温度时阻止回落到模型库默认值。仅用于 model_overrides。
    pub omit_temperature: Option<bool>,
    pub capabilities: Option<ModelCapabilities>,
    pub pricing: Option<ModelPricing>,
    /// 每模型「思考等级」白名单，覆盖模型库的 `reasoningEfforts`。
    /// `None` = 跟随模型库；`Some([])` = 该模型没有 effort 旋钮（请求不带任何等级字段）；
    /// `Some([..])` = 只这几档可选可下发。仅用于 model_overrides。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_efforts: Option<Vec<String>>,
    /// 每模型额外请求体字段（原样 merge 进 chat/completions body 根部）。
    /// 用于给严格 OpenAI-compat 端点塞标准 schema 之外的私有旋钮，例如 NVIDIA NIM /
    /// vLLM / SGLang 的 `chat_template_kwargs`、GLM 的 thinking 开关等。仅用于 model_overrides。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_body: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelCapabilities {
    pub vision: Option<bool>,
    pub function_calling: Option<bool>,
    pub reasoning: Option<bool>,
    pub streaming: Option<bool>,
    pub web_search: Option<bool>,
    pub image_generation: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelPricing {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cached_input: Option<f64>,
}

/**
 * OCR 引擎模式（截图翻译用）
 *
 * - CloudVision：发图给多模态 provider 一次完成 OCR+翻译（旧 use_system_ocr=false 等价行为）
 * - System：调用 macOS Apple Vision 或 Windows.Media.Ocr 识别文字，再交 provider 翻译
 * - RapidOcr：本地 RapidOCR (PaddleOCR ONNX) 识别文字，再交 provider 翻译。onnxruntime
 *   dylib 与模型文件均由用户在设置页面下载到 app data 目录，安装包不含。
 * - Legacy：反序列化兜底，吸收旧版本 settings.json 里的未知字符串（如 "tesseract"），
 *   sanitize_settings 会迁移到 RapidOcr，保留旧版离线 OCR 的隐私边界。
 *
 * 字段在 sanitize_settings 中由 use_system_ocr 自动迁移：true→System，false→CloudVision。
 * persist_settings 写盘时反向镜像到 use_system_ocr 维持降级到 v2.5.x 的兼容性。
 * RapidOcr 模式降级会落回 CloudVision（use_system_ocr=false），可接受。
 */
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrMode {
    CloudVision,
    System,
    RapidOcr,
    #[serde(other)]
    Legacy,
}

impl Default for OcrMode {
    fn default() -> Self {
        OcrMode::CloudVision
    }
}

/**
 * 截图翻译功能配置
 */
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScreenshotTranslationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_screenshot_translation_hotkey")]
    pub hotkey: String,
    #[serde(default = "default_screenshot_translation_text_hotkey")]
    pub text_hotkey: String,
    /// 替换翻译独立热键（框选后在原位覆盖译文，固定 RapidOCR）。
    #[serde(default = "default_screenshot_translation_replace_hotkey")]
    pub replace_hotkey: String,
    #[serde(default = "default_true")]
    pub replace_enabled: bool,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default = "default_openai_model")]
    pub model: String,
    #[serde(default = "default_false")]
    pub direct_translate: bool,
    /// 是否启用思考模式（OCR 模型 + 翻译模型）。默认 false：截图翻译追求快，思考通常没必要。
    #[serde(default = "default_false")]
    pub thinking_enabled: bool,
    /// 是否流式输出 OCR + 翻译。默认 true：用户看着字逐步出现的体感比等"加载完"更顺。
    #[serde(default = "default_true")]
    pub stream_enabled: bool,
    /// 截图后是否保留 lens 全屏覆盖。默认 true：选区高亮 + 译文卡同屏；false → lens 缩成浮动小窗，不挡下层 app。
    #[serde(default = "default_true")]
    pub keep_fullscreen_after_capture: bool,

    /// 快速翻译结果卡左右宽度（px）。截图翻译与选中文本翻译两种卡共用，保证宽度一致且可调。
    #[serde(default = "default_translate_card_width")]
    pub card_width: u32,
    /// 用平台本地 OCR 做文字识别，把识别出的文字喂给翻译模型（macOS Apple Vision / Windows OCR）。
    /// true → 系统 OCR + provider 文字翻译（provider 可是任意 OpenAI 兼容 endpoint）
    /// false → provider 必须是多模态模型，一次完成 OCR+翻译
    ///
    /// 从 vNext 起，截图翻译路由实际走 ocr_mode 字段；本字段仅作降级镜像保留：
    /// - persist_settings 写盘时根据 ocr_mode 反向镜像到这里（System→true，其它→false），
    ///   让降级到 v2.5.x 的版本仍能从 useSystemOcr 字段读到对应行为。
    /// - sanitize_settings 在 ocr_mode 缺省时会从这里反推迁移。
    #[serde(default = "default_false")]
    pub use_system_ocr: bool,
    /// OCR 引擎选择（vNext+）。None 表示老版本数据，会在 sanitize_settings 中按 use_system_ocr 迁移。
    #[serde(default)]
    pub ocr_mode: Option<OcrMode>,
    /// 截图(OCR/视觉)翻译自定义提示词。空 → 用内置截图模板。
    #[serde(default)]
    pub prompt: Option<String>,
    /// 选中文本翻译自定义提示词。空 → 用内置选中文本模板。独立于 `prompt`：
    /// 选中文本是干净结构化文本，与 OCR 噪声场景的提示词需求不同。
    #[serde(default)]
    pub text_prompt: Option<String>,
    /// 替换翻译自定义提示词（仅注入翻译规则块，JSON 输出契约固定）。空 → 用内置替换模板。
    #[serde(default)]
    pub replace_prompt: Option<String>,
    /// RapidOCR 模型档位:"standard"(默认,PP-OCRv5 mobile,速度优先) | "high"(PP-OCRv6 medium,精度优先)。
    /// 仅在 ocr_mode = RapidOcr 时生效;替换翻译（固定走 RapidOCR）跟随此字段。
    #[serde(default = "default_rapid_ocr_tier")]
    pub rapid_ocr_tier: String,
    // 旧版字段，用于迁移
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openai: Option<OpenAIConfig>,
}

impl Default for ScreenshotTranslationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hotkey: "CommandOrControl+Shift+A".to_string(),
            text_hotkey: "CommandOrControl+Shift+T".to_string(),
            replace_hotkey: "CommandOrControl+Shift+R".to_string(),
            replace_enabled: true,
            provider_id: "default-ocr".to_string(),
            model: "gpt-4o".to_string(),
            direct_translate: false,
            thinking_enabled: false,
            stream_enabled: true,
            keep_fullscreen_after_capture: true,
            card_width: default_translate_card_width(),
            use_system_ocr: false,
            ocr_mode: Some(OcrMode::CloudVision),
            prompt: None,
            text_prompt: None,
            replace_prompt: None,
            rapid_ocr_tier: default_rapid_ocr_tier(),
            openai: None,
        }
    }
}

/// RapidOCR 档位默认值,截图翻译用(要速度)。
fn default_rapid_ocr_tier() -> String {
    "standard".to_string()
}

/// 知识库文档处理默认走高精度(v6 medium):入库不在乎慢,要识别质量。
fn default_rapid_ocr_tier_high() -> String {
    "high".to_string()
}

/**
 * 对话消息（Lens 多轮对话）
 */
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplainMessage {
    pub role: String,
    pub content: String,
}

/**
 * Lens 联网搜索提供商。
 */
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchProvider {
    Tavily,
    Exa,
    ExaMcp,
    Ollama,
    Grok,
    Deepseek,
    Brave,
    Serper,
    Bocha,
    Zhipu,
    Tinyfish,
    TinyfishMcp,
    Searxng,
    /// 前端可能列出尚未接入后端的占位服务商；持久化时兜底为未知，避免旧值导致整份设置解析失败。
    #[serde(other)]
    Unknown,
}

impl Default for WebSearchProvider {
    fn default() -> Self {
        WebSearchProvider::Tavily
    }
}

/**
 * Lens 联网搜索配置。
 *
 * 手动模式由前端在单次提问时传 web_search=true；后端仍会检查 enabled 和 key。
 */
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LensWebSearchConfig {
    #[serde(default = "default_false")]
    pub enabled: bool,
    #[serde(default)]
    pub provider: WebSearchProvider,
    #[serde(default)]
    pub tavily_api_key: String,
    #[serde(default = "default_tavily_base_url")]
    pub tavily_base_url: String,
    #[serde(default)]
    pub exa_api_key: String,
    #[serde(default = "default_exa_base_url")]
    pub exa_base_url: String,
    #[serde(default = "default_exa_mcp_url")]
    pub exa_mcp_url: String,
    #[serde(default)]
    pub ollama_api_key: String,
    #[serde(default = "default_ollama_base_url")]
    pub ollama_base_url: String,
    #[serde(default)]
    pub grok_api_key: String,
    #[serde(default = "default_grok_model")]
    pub grok_model: String,
    #[serde(default = "default_grok_base_url")]
    pub grok_base_url: String,
    #[serde(default = "default_grok_system_prompt")]
    pub grok_system_prompt: String,
    #[serde(default)]
    pub deepseek_api_key: String,
    #[serde(default = "default_deepseek_model")]
    pub deepseek_model: String,
    #[serde(default = "default_deepseek_base_url")]
    pub deepseek_base_url: String,
    #[serde(default = "default_grok_system_prompt")]
    pub deepseek_system_prompt: String,
    #[serde(default)]
    pub brave_api_key: String,
    #[serde(default = "default_brave_base_url")]
    pub brave_base_url: String,
    #[serde(default)]
    pub serper_api_key: String,
    #[serde(default = "default_serper_base_url")]
    pub serper_base_url: String,
    #[serde(default)]
    pub bocha_api_key: String,
    #[serde(default = "default_bocha_base_url")]
    pub bocha_base_url: String,
    #[serde(default)]
    pub zhipu_api_key: String,
    #[serde(default = "default_zhipu_base_url")]
    pub zhipu_base_url: String,
    #[serde(default)]
    pub tinyfish_api_key: String,
    #[serde(default = "default_tinyfish_base_url")]
    pub tinyfish_base_url: String,
    #[serde(default = "default_tinyfish_mcp_url")]
    pub tinyfish_mcp_url: String,
    /// TinyFish MCP 走 OAuth 2.1，不贴 API Key。授权结果存在这里，搜索时带 Authorization。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tinyfish_mcp_auth: Option<ConnectorAuth>,
    #[serde(default)]
    pub searxng_base_url: String,
    #[serde(default = "default_web_search_max_results")]
    pub max_results: u8,
    #[serde(default = "default_web_search_depth")]
    pub search_depth: String,
}

impl Default for LensWebSearchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: WebSearchProvider::Tavily,
            tavily_api_key: String::new(),
            tavily_base_url: default_tavily_base_url(),
            exa_api_key: String::new(),
            exa_base_url: default_exa_base_url(),
            exa_mcp_url: default_exa_mcp_url(),
            ollama_api_key: String::new(),
            ollama_base_url: default_ollama_base_url(),
            grok_api_key: String::new(),
            grok_model: default_grok_model(),
            grok_base_url: default_grok_base_url(),
            grok_system_prompt: default_grok_system_prompt(),
            deepseek_api_key: String::new(),
            deepseek_model: default_deepseek_model(),
            deepseek_base_url: default_deepseek_base_url(),
            deepseek_system_prompt: default_grok_system_prompt(),
            brave_api_key: String::new(),
            brave_base_url: default_brave_base_url(),
            serper_api_key: String::new(),
            serper_base_url: default_serper_base_url(),
            bocha_api_key: String::new(),
            bocha_base_url: default_bocha_base_url(),
            zhipu_api_key: String::new(),
            zhipu_base_url: default_zhipu_base_url(),
            tinyfish_api_key: String::new(),
            tinyfish_base_url: default_tinyfish_base_url(),
            tinyfish_mcp_url: default_tinyfish_mcp_url(),
            tinyfish_mcp_auth: None,
            searxng_base_url: String::new(),
            max_results: default_web_search_max_results(),
            search_depth: default_web_search_depth(),
        }
    }
}

fn default_exa_mcp_url() -> String {
    "https://mcp.exa.ai/mcp".to_string()
}

fn default_tavily_base_url() -> String {
    "https://api.tavily.com".to_string()
}

fn default_exa_base_url() -> String {
    "https://api.exa.ai".to_string()
}

fn default_ollama_base_url() -> String {
    "https://ollama.com".to_string()
}

fn default_grok_model() -> String {
    "grok-4-1-fast-non-reasoning".to_string()
}

fn default_grok_base_url() -> String {
    "https://api.x.ai/v1".to_string()
}

fn default_deepseek_model() -> String {
    "deepseek-v4-flash".to_string()
}

fn default_deepseek_base_url() -> String {
    "https://api.deepseek.com".to_string()
}

fn default_brave_base_url() -> String {
    "https://api.search.brave.com".to_string()
}

fn default_serper_base_url() -> String {
    "https://google.serper.dev".to_string()
}

fn default_bocha_base_url() -> String {
    "https://api.bochaai.com".to_string()
}

fn default_zhipu_base_url() -> String {
    "https://open.bigmodel.cn/api/paas/v4".to_string()
}

fn default_tinyfish_base_url() -> String {
    "https://api.search.tinyfish.ai".to_string()
}

fn default_tinyfish_mcp_url() -> String {
    "https://agent.tinyfish.ai/mcp".to_string()
}

pub fn default_grok_system_prompt() -> String {
    "You are a helpful search assistant. Search the web to find accurate and up-to-date information for the user's query. Provide a comprehensive answer with citations."
        .to_string()
}

fn default_web_search_max_results() -> u8 {
    5
}

fn default_web_search_depth() -> String {
    "basic".to_string()
}

/**
 * Lens 模式配置
 * 启用后可通过热键进入：屏幕高亮选择窗口/区域 → 截图 → 在悬浮对话栏内提问。
 */
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LensConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_lens_hotkey")]
    pub hotkey: String,
    /// provider/model 留空时 fallback 到 translator_provider_id / translator_model
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub model: String,
    /// 响应语言（"zh"/"en"）。空字符串表示跟随 settings.target_lang，"auto" 则用 "zh"。
    #[serde(default)]
    pub default_language: String,
    /// 是否流式返回，默认 true。
    #[serde(default = "default_true")]
    pub stream_enabled: bool,
    /// 是否启用思考模式（推理链）。默认 true。
    /// false 时会向请求 body 注入各家厂商关闭思考的字段并集（不认识的会被 provider 忽略）。
    #[serde(default = "default_true")]
    pub thinking_enabled: bool,
    /// 自定义 system prompt。空字符串使用 default_system_prompt 模板。
    #[serde(default)]
    pub system_prompt: String,
    /// 自定义 question prompt。空字符串使用 default_question_prompt 模板。
    #[serde(default)]
    pub question_prompt: String,
    /// Lens 提问默认发送到 AI 客户端。关闭后保留旧的 Lens 浮窗内回答。
    #[serde(default = "default_true")]
    pub send_to_chat: bool,
    /// 消息排序："asc" 老到新（默认），"desc" 新到老
    #[serde(default = "default_message_order")]
    pub message_order: String,
    /// 进入截图选择态时是否显示顶部提示。默认 true，避免用户按下快捷键后看不出已进入截图模式。
    #[serde(default = "default_true")]
    pub show_capture_hint: bool,
    #[serde(default)]
    pub web_search: LensWebSearchConfig,
}

fn default_message_order() -> String {
    "asc".to_string()
}

pub fn default_chat_max_output_tokens() -> u32 {
    32768
}

pub(crate) fn clamp_chat_max_output_tokens(value: u32) -> u32 {
    value.clamp(512, 65_536)
}

impl Default for LensConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hotkey: "CommandOrControl+Shift+G".to_string(),
            provider_id: String::new(),
            model: String::new(),
            default_language: String::new(),
            stream_enabled: true,
            thinking_enabled: true,
            system_prompt: String::new(),
            question_prompt: String::new(),
            send_to_chat: true,
            message_order: "asc".to_string(),
            show_capture_hint: true,
            web_search: LensWebSearchConfig::default(),
        }
    }
}

/**
 * AI 客户端（Chat）行为配置：与 Lens 分离，避免截图问答与对话客户端共用开关。
 */
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatConfig {
    #[serde(default = "default_true")]
    pub stream_enabled: bool,
    #[serde(default = "default_true")]
    pub thinking_enabled: bool,
    /// Chat 模型最终回答最大输出 tokens。
    #[serde(default = "default_chat_max_output_tokens")]
    pub max_output_tokens: u32,
    /// 响应语言（"zh"/"en" 等）。空字符串表示跟随 Lens 默认语言，再跟随 target_lang。
    #[serde(default)]
    pub default_language: String,
    /// 自定义 system prompt；空则使用内置 Chat 模板（Kivio Agent 运行时）。
    #[serde(default)]
    pub system_prompt: String,
    /// Chat 侧栏显示的用户名；空则前端使用默认文案。
    #[serde(default)]
    pub user_display_name: String,
    /// 头像图片 URL 或 data URL；空则显示首字母占位头像。
    #[serde(default)]
    pub user_avatar: String,
    /// 新建对话默认 Agent 运行时（内置 loop 或外部 CLI）。
    #[serde(default)]
    pub default_agent_runtime: crate::chat::AgentRuntimeConfig,
    /// 本地 CLI Agent 的用户覆盖，key = agent id（claude/codex/…）。缺省 = 全默认。
    #[serde(default)]
    pub external_cli_agents: std::collections::HashMap<String, ExternalCliAgentConfig>,
    /// Kivio Chat 运行时专属设置（与 Kivio Agent 的工具/提示词分离）。
    #[serde(default)]
    pub chat_mode: ChatModeConfig,
}

/// Kivio Chat runtime settings — conversational tools + optional custom prompt.
/// Independent from Agent native-tool toggles (write/shell/skills stay Agent-only).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatModeConfig {
    /// Optional extra Chat instructions. Empty → no Chat identity essay;
    /// runtime still injects date plus conversation context (assistant / set /
    /// memory / knowledge base). File/shell limits are the tool filter, not prompt.
    pub system_prompt: String,
    pub web_search: bool,
    pub web_fetch: bool,
    pub knowledge_search: bool,
    /// `memory_read` / `memory_search` in Chat runtime.
    pub memory_tools: bool,
    /// Allow MCP tools that pass `is_read_only_tool()`.
    pub mcp_read_only: bool,
}

impl Default for ChatModeConfig {
    fn default() -> Self {
        Self {
            system_prompt: String::new(),
            web_search: true,
            web_fetch: true,
            knowledge_search: true,
            memory_tools: true,
            mcp_read_only: true,
        }
    }
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self {
            stream_enabled: true,
            thinking_enabled: true,
            max_output_tokens: default_chat_max_output_tokens(),
            default_language: String::new(),
            system_prompt: String::new(),
            user_display_name: String::new(),
            user_avatar: String::new(),
            default_agent_runtime: crate::chat::AgentRuntimeConfig::default(),
            external_cli_agents: std::collections::HashMap::new(),
            chat_mode: ChatModeConfig::default(),
        }
    }
}

/// 单个本地 CLI Agent 的用户覆盖（设置页「本地 CLI Agent」）。全字段可缺省 = 保持内置行为。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExternalCliAgentConfig {
    /// 停用后不出现在 Chat 的运行时选择器里。已绑定该 CLI 的旧会话不受影响——
    /// 一 agent 一对话，停用是「别再新建」而不是「把历史会话弄坏」。
    pub disabled: bool,
    /// 自定义可执行文件路径；空 = 走 PATH 探测。
    pub path: String,
    /// 注入该 CLI 子进程的环境变量（ANTHROPIC_BASE_URL / OPENAI_API_KEY 之类）。
    pub env: Vec<CliEnvVar>,
    /// 用户手填的模型，合并进探测出来的模型下拉。
    pub custom_models: Vec<CliCustomModel>,
    /// 该 CLI 的第三方供应商（中转站）列表。每个 CLI 各自一份，同 ccgui 的分桶方式。
    pub providers: Vec<ExternalCliProvider>,
    /// 当前默认供应商 id；空 = 使用 CLI 自己的默认配置。
    /// Pi / OpenCode / dsh 的 providers 会全部并存，此字段只决定未显式选模型时的默认项。
    pub current_provider: String,
}

/// 一个第三方供应商（中转站）。**各 CLI 用到的字段不同**：
/// - claude / gemini / 其余 env 系：只用 `env`（`ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN` …）；
///   claude 聊天选模按 cc-switch 写入 `~/.claude/settings.json` 的 `model` / `env.ANTHROPIC_MODEL`
/// - codex：只用 `config_toml` / `auth_json`，物化成一个私有 `CODEX_HOME`；聊天选模写入
///   `~/.codex/config.toml` 顶层 `model`（文件已存在时）以及 CLI 正在读的那份
/// - grok：只用 `config_toml`，把其中的 `[models]` / `[model.*]` 合并进 `~/.grok/config.toml`
/// - opencode / pi：用 `config_json` / `auth_json` / `default_model` 合并进 CLI 原生全局配置
/// - dsh：用 `config_json` 在 Kivio 私有 profile 中挂载 `llm-pi-ai`，Key 通过 `env` 注入
/// - pi：另用 `default_reasoning` 写入 `settings.json.defaultThinkingLevel`
///
/// 扁平结构而不是 tagged enum：settings.json 是用户可手改的文件，enum 的 tag 写错整条读不出来。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExternalCliProvider {
    /// 从 cc-switch 导入时**保留原 id**，这样二次导入走更新而不是新增一条重复的。
    pub id: String,
    pub name: String,
    /// 仅对支持并存的 CLI 生效；false = 启用（兼容旧配置），true = 不物化、不注入。
    pub disabled: bool,
    pub remark: String,
    pub env: Vec<CliEnvVar>,
    /// codex：私有 CODEX_HOME 里 config.toml 的全文；grok：写入原生 `~/.grok/config.toml` 的片段（至少含 models / model）。
    pub config_toml: String,
    /// opencode / pi：单个原生 provider 对象的 JSON（不含 provider id 外层）。
    pub config_json: String,
    /// codex：私有 CODEX_HOME 的完整 auth.json；opencode / pi：单个原生凭据对象。
    pub auth_json: String,
    /// Kivio 自用的模型覆盖状态；不写入 CLI 原生配置。
    pub model_metadata_json: String,
    /// opencode / pi / dsh：模型引用使用的稳定供应商 id（dsh wire 为 `provider:model`）。
    pub native_provider_id: String,
    /// opencode / pi / dsh：启用该供应商时使用的默认模型 id（不含 provider 前缀）。
    pub default_model: String,
    /// pi：终端独立启动时使用的默认 thinking 档位。
    pub default_reasoning: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CliEnvVar {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CliCustomModel {
    pub id: String,
    /// 显示名；空则前端回落显示 id。
    pub label: String,
}

/**
 * Chat 记忆系统配置。
 *
 * 记忆正文不存 settings.json；这里只保存运行开关。正文保存在 app data 的 chat-memory/L1.md
 * 与 chat-memory/L2.md 中。
 */
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatMemoryConfig {
    #[serde(default = "default_false")]
    pub enabled: bool,
    /// 已废弃：memory 工具现均无需用户确认；保留字段仅作旧配置兼容。
    #[serde(default = "default_false")]
    pub tool_write_confirm: bool,
}

impl Default for ChatMemoryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            tool_write_confirm: false,
        }
    }
}

/**
 * 可选模型选择：provider_id 为空表示未单独设置。
 */
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DefaultModelSelection {
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub model: String,
}

impl Default for DefaultModelSelection {
    fn default() -> Self {
        Self {
            provider_id: String::new(),
            model: String::new(),
        }
    }
}

impl DefaultModelSelection {
    fn is_configured(&self) -> bool {
        !self.provider_id.trim().is_empty()
    }
}

/// 当前 Chat 会话的主模型（顶栏选择），用于混音器 auto 时解析副任务路由。
#[derive(Debug, Clone, Copy)]
pub struct SessionModel<'a> {
    pub provider_id: &'a str,
    pub model: &'a str,
}

impl<'a> SessionModel<'a> {
    pub fn is_set(self) -> bool {
        !self.provider_id.trim().is_empty() && !self.model.trim().is_empty()
    }
}

fn resolve_mixer_side_model(
    selection: &DefaultModelSelection,
    session: Option<SessionModel<'_>>,
    settings: &Settings,
) -> (String, String) {
    if selection.is_configured() {
        return (selection.provider_id.clone(), selection.model.clone());
    }
    if let Some(session) = session.filter(|session| session.is_set()) {
        return (session.provider_id.to_string(), session.model.to_string());
    }
    settings.effective_chat_model()
}

/**
 * 默认模型配置。
 *
 * chat：新建 Chat 对话的全局默认模型；为空时沿用 Lens → 输入翻译的兜底链路。
 * vision：图片附件分析副任务使用；为空时继承当前会话主模型（无会话时回退有效 Chat 默认）。
 * title_summary：标题总结副任务使用；为空时继承当前会话主模型（无会话时回退有效 Chat 默认）。
 * compression：上下文/历史对话压缩副任务使用；为空时继承当前会话主模型（无会话时回退有效 Chat 默认）。
 * image_generation：生图副任务使用；为空时若当前会话主模型支持直接生图则继承该模型。
 * advisor：顾问模型（executor-advisor 模式）——主循环模型可用 `advisor` 工具向它
 *   单次咨询；为空 = 功能关闭（工具不注册），没有继承语义。
 */
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DefaultModelsConfig {
    #[serde(default)]
    pub chat: DefaultModelSelection,
    #[serde(default)]
    pub vision: DefaultModelSelection,
    #[serde(default)]
    pub title_summary: DefaultModelSelection,
    #[serde(default)]
    pub compression: DefaultModelSelection,
    #[serde(default)]
    pub image_generation: DefaultModelSelection,
    #[serde(default)]
    pub advisor: DefaultModelSelection,
}

impl Default for DefaultModelsConfig {
    fn default() -> Self {
        Self {
            chat: DefaultModelSelection::default(),
            vision: DefaultModelSelection::default(),
            title_summary: DefaultModelSelection::default(),
            compression: DefaultModelSelection::default(),
            image_generation: DefaultModelSelection::default(),
            advisor: DefaultModelSelection::default(),
        }
    }
}

/// 解析 Chat 使用的响应语言代码。
pub fn resolve_chat_language(settings: &Settings) -> String {
    if !settings.chat.default_language.trim().is_empty() {
        return settings.chat.default_language.trim().to_string();
    }
    if !settings.lens.default_language.trim().is_empty() {
        return settings.lens.default_language.trim().to_string();
    }
    match settings.target_lang.as_str() {
        "en" => "en".to_string(),
        "zh-Hant" | "zh-TW" => "zh-Hant".to_string(),
        _ => "zh".to_string(),
    }
}

/**
 * Chat MCP stdio server 配置。
 *
 * settings.json 使用 camelCase；env 与 API keys 一样按本地明文设置策略保存。
 */
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatMcpServer {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub transport: String,
    pub url: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: std::collections::HashMap<String, String>,
    pub headers: std::collections::HashMap<String, String>,
    pub cwd: Option<String>,
    pub enabled_tools: Vec<String>,
    /// 连接器目录 id（如 "github"/"notion"/"composio" 或 "custom-xxx"）。
    /// 非空表示这条 server 由「连接器」页管理，不在 MCP 页重复展示。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_id: Option<String>,
    /// 连接器认证信息。Phase A 只用 `{ kind: "token" }`；OAuth 字段留待 Phase B。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<ConnectorAuth>,
}

impl Default for ChatMcpServer {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: false,
            transport: "stdio".to_string(),
            url: String::new(),
            command: String::new(),
            args: Vec::new(),
            env: std::collections::HashMap::new(),
            headers: std::collections::HashMap::new(),
            cwd: None,
            enabled_tools: Vec::new(),
            connector_id: None,
            auth: None,
        }
    }
}

/**
 * 连接器认证信息。与 `providers[].apiKeys` 一样按本地明文设置策略保存。
 *
 * Phase A 仅使用 `kind: "token"` + `access_token`；其余字段（refresh/expires/
 * token_endpoint/client_id/scopes）为 Phase B 的 OAuth 流程预留。
 */
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ConnectorAuth {
    pub kind: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub token_endpoint: Option<String>,
    pub client_id: Option<String>,
    pub scopes: Vec<String>,
    /// 真实账户标识（邮箱 / 工作区名 / 用户名），授权时尽力提取，拿不到则 None。
    /// 明文存储，向后兼容（旧设置无此字段时反序列化为 None）。
    #[serde(default)]
    pub account: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatNativeToolsConfig {
    pub web_search: bool,
    #[serde(default)]
    pub web_fetch: bool,
    pub skill_runtime: bool,
    #[serde(default)]
    pub read_file: bool,
    #[serde(default)]
    pub write_file: bool,
    #[serde(default)]
    pub edit_file: bool,
    #[serde(default)]
    pub run_command: bool,
    #[serde(default = "default_true")]
    pub knowledge_search: bool,
    /// Default root for ordinary (non-project) conversation workbenches.
    /// Missing legacy configs deserialize to an empty string so sanitize can
    /// migrate `workspace_roots[0]` before falling back to the platform default.
    #[serde(default)]
    pub working_directory: String,
    /// Legacy compatibility only. Runtime code must not use this as a path boundary.
    #[serde(default)]
    pub workspace_roots: Vec<String>,
}

impl ChatNativeToolsConfig {
    pub fn any_enabled(&self) -> bool {
        self.web_search
            || self.web_fetch
            || self.skill_runtime
            || self.read_file
            || self.write_file
            || self.edit_file
            || self.run_command
            || self.knowledge_search
    }
}

impl Default for ChatNativeToolsConfig {
    fn default() -> Self {
        // Agentic-app baseline: native tools are ON by default. Reading files and
        // running commands are table stakes for the agent (and its sub-agents),
        // not opt-in extras. Safety lives at execution time in the session-consent
        // gate (chat/agent/execute.rs), which the UI lets cautious users tighten
        // back to per-conversation confirmation. web_search still only surfaces
        // when a provider key is configured.
        Self {
            web_search: true,
            web_fetch: true,
            skill_runtime: true,
            read_file: true,
            write_file: true,
            edit_file: true,
            run_command: true,
            knowledge_search: true,
            working_directory: default_chat_working_directory(),
            workspace_roots: Vec::new(),
        }
    }
}

pub fn default_chat_working_directory() -> String {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().join("Kivio").join("workspace"))
        .unwrap_or_else(|| std::path::PathBuf::from("Kivio").join("workspace"))
        .to_string_lossy()
        .to_string()
}

fn default_skill_auto_match() -> bool {
    true
}

fn default_skill_fallback_mode() -> String {
    "progressive".to_string()
}

pub const CHAT_TOOL_MIN_TIMEOUT_MS: u64 = 1_000;
pub const CHAT_TOOL_MAX_TIMEOUT_MS: u64 = 300_000;
/// 旧版工具轮次默认值。现默认**不限**（`None`），此常量仅供一次性迁移
/// （`sanitize_settings` 把存量 20 归一到不限）与前端展示预设使用。
pub const CHAT_TOOL_LEGACY_DEFAULT_ROUNDS: u32 = 20;
pub const CHAT_TOOL_MIN_ROUNDS: u32 = 1;
pub const CHAT_TOOL_MAX_ROUNDS: u32 = 100;
/// 单条工具结果字符上限的合法区间。低于下限会把编译错误/测试输出截到没意义；
/// 高于上限则失去"防上下文撑爆"的作用。`None`（不截断）由 sanitize 归一到默认值。
pub const CHAT_TOOL_MIN_OUTPUT_CHARS: usize = 2_000;
pub const CHAT_TOOL_MAX_OUTPUT_CHARS: usize = 200_000;
/// 默认单条工具结果字符上限 ≈ 6K token（头 1/2 + 尾 1/4 保留约 3/4）。
pub const DEFAULT_MAX_TOOL_OUTPUT_CHARS: usize = 24_000;
/// Orchestrate 模式下的最低工具轮次预算：编排者主动 fan-out 子 agent + 先规划再分派，
/// 单条用户消息内可能需要更多轮次，因此抬到 max(用户配置, 此值)，但不放开为无限。
pub const ORCHESTRATE_MIN_TOOL_ROUNDS: u32 = 40;
/// MCP 持久连接空闲超时下限：太小会让长连接频繁回收失去意义。
pub const MCP_IDLE_TIMEOUT_MIN_MS: u64 = 60_000;
/// MCP 持久连接空闲超时上限：避免死连接长期占用子进程。
pub const MCP_IDLE_TIMEOUT_MAX_MS: u64 = 24 * 60 * 60 * 1_000;

fn default_chat_tool_timeout_ms() -> u64 {
    60_000
}

fn default_sub_agent_concurrency() -> usize {
    crate::chat::sub_agent::DEFAULT_SUB_AGENT_CONCURRENCY
}

fn default_mcp_idle_timeout_ms() -> u64 {
    600_000
}

/// 工具调用轮次默认**不限**。到达上限的收尾机制（step_limit_system_message）
/// 仍保留，供用户在 MCP 页显式选择 5/10/20/50/100 时使用。
fn default_chat_max_tool_rounds() -> Option<u32> {
    None
}

/// 单条工具结果进入上下文前的字符上限（头 1/2 + 尾 1/4 保留，实际约 3/4）。
/// 默认 [`DEFAULT_MAX_TOOL_OUTPUT_CHARS`]：从源头掐住 read_file / bash / grep 等大输出，
/// 避免它们以全量累积进 runtime_messages 撑爆上下文。`None` = 不截断（旧行为，sanitize 会归一到默认）。
fn default_max_tool_output_chars() -> Option<usize> {
    Some(DEFAULT_MAX_TOOL_OUTPUT_CHARS)
}

fn default_chat_approval_policy() -> String {
    // Green-light by default: file/shell tools run without a per-conversation
    // prompt. The consent mechanism stays available — the UI can switch this to
    // "always_confirm" or the per-conversation prompt for cautious users.
    "auto".to_string()
}

/// The pre-green-light default. `sanitize_settings`' one-shot migration only
/// flips an existing install to "auto" when its stored policy still equals this
/// string, so a user who deliberately chose another policy is never stomped.
const LEGACY_DEFAULT_APPROVAL_POLICY: &str = "readonly_auto_sensitive_confirm";

/// Hook 超时的合法区间与默认值。
pub const HOOK_MIN_TIMEOUT_MS: u64 = 1_000;
pub const HOOK_MAX_TIMEOUT_MS: u64 = 600_000;
pub const HOOK_DEFAULT_TIMEOUT_MS: u64 = 60_000;

fn default_hook_timeout_ms() -> u64 {
    HOOK_DEFAULT_TIMEOUT_MS
}

fn default_hook_method() -> String {
    "POST".to_string()
}

/// 对话生命周期 Hook：在 agent loop 的某个事件点执行 Shell 脚本或发一个 HTTP 请求。
/// 一律 fire-and-forget —— 不阻断、不改写工具调用（见 07-28-hooks PRD 的「非目标」）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HookDef {
    pub id: String,
    pub name: String,
    pub description: String,
    /// 8 个生命周期事件之一（见 `chat::hooks::HookEvent`）。未知值在 sanitize 时丢弃。
    pub event: String,
    pub enabled: bool,
    /// "command" | "http"
    #[serde(rename = "type")]
    pub kind: String,
    /// kind == "command" 时的脚本正文。
    pub script: String,
    /// kind == "http" 时的目标。
    pub url: String,
    #[serde(default = "default_hook_method")]
    pub method: String,
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(default = "default_hook_timeout_ms")]
    pub timeout_ms: u64,
}

impl Default for HookDef {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            event: String::new(),
            enabled: false,
            kind: "command".to_string(),
            script: String::new(),
            url: String::new(),
            method: default_hook_method(),
            headers: std::collections::BTreeMap::new(),
            timeout_ms: default_hook_timeout_ms(),
        }
    }
}

/// 归一 Hook 列表：丢弃事件名/类型非法或目标为空的条目，补空 id，钳制超时。
/// 无效条目直接丢弃而非修正——一个 event 打错字的 Hook 没有合理的「就近」事件可猜。
/// 把 JSON `null` 当成空 `Vec` 收下，而不是报类型错误。
/// 见 `ChatToolsConfig::hooks` 上的注释：一个字段被前端漏传就会在磁盘上留下 null，
/// 而 serde 的 `default` 兜不住 null，会让整个父结构解析失败。
fn null_tolerant_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

fn sanitize_hooks(hooks: &mut Vec<HookDef>) {
    hooks.retain_mut(|hook| {
        hook.event = hook.event.trim().to_string();
        hook.kind = hook.kind.trim().to_lowercase();
        if crate::chat::hooks::HookEvent::parse(&hook.event).is_none() {
            return false;
        }
        match hook.kind.as_str() {
            "command" => {
                if hook.script.trim().is_empty() {
                    return false;
                }
            }
            "http" => {
                if hook.url.trim().is_empty() {
                    return false;
                }
                hook.method = hook.method.trim().to_uppercase();
                if hook.method.is_empty() {
                    hook.method = default_hook_method();
                }
            }
            _ => return false,
        }
        if hook.id.trim().is_empty() {
            hook.id = uuid::Uuid::new_v4().to_string();
        }
        hook.timeout_ms = hook
            .timeout_ms
            .clamp(HOOK_MIN_TIMEOUT_MS, HOOK_MAX_TIMEOUT_MS);
        true
    });
}

/**
 * Chat 工具与 Skill 配置。
 */
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatToolsConfig {
    pub enabled: bool,
    pub servers: Vec<ChatMcpServer>,
    pub skill_scan_paths: Vec<String>,
    #[serde(default = "default_skill_auto_match")]
    pub skill_auto_match: bool,
    #[serde(default = "default_skill_fallback_mode")]
    pub skill_fallback_mode: String,
    /// Skill ids the user turned off in Settings. Omitted ids are enabled.
    #[serde(default)]
    pub disabled_skill_ids: Vec<String>,
    #[serde(default = "default_chat_max_tool_rounds")]
    pub max_tool_rounds: Option<u32>,
    #[serde(default = "default_chat_tool_timeout_ms")]
    pub tool_timeout_ms: u64,
    /// MCP 持久连接空闲超时（ms）：会话 last_used 超过此值后被 reaper 回收，下次调用透明重连。
    #[serde(default = "default_mcp_idle_timeout_ms")]
    pub mcp_idle_timeout_ms: u64,
    #[serde(default = "default_max_tool_output_chars")]
    pub max_tool_output_chars: Option<usize>,
    #[serde(default = "default_chat_approval_policy")]
    pub approval_policy: String,
    /// 同一时刻最多并行运行的子 agent 数。受 [`SUB_AGENT_CONCURRENCY_MIN`]..[`MAX`] 钳制。
    #[serde(default = "default_sub_agent_concurrency")]
    pub sub_agent_concurrency: usize,
    /// 子代理全局模型覆盖：spawn 的 sub-agent 用这个 provider+model 而非父会话的。
    /// 两者皆空 = 跟随父会话（现状）。agent 定义文件里的 model 字段仍优先于此设置。
    #[serde(default)]
    pub sub_agent_provider_id: String,
    #[serde(default)]
    pub sub_agent_model: String,
    /// 开发者「请求调试」总开关：开启后每次 provider 调用被记录到内存环形缓冲（脱敏）。
    /// 默认关闭；关闭时 adapter 零开销（不构造记录）。仅内存、不落盘。
    #[serde(default)]
    pub request_debug_enabled: bool,
    /// 对话生命周期 Hooks（07-28-hooks）。空数组 = 无 Hook = agent loop 零开销。
    ///
    /// `deserialize_with` 而非光 `default`：字段刚上线时前端漏传，`invoke` 把缺失字段
    /// 序列化成 **null** 落进了 settings.json，而 `default` 只兜「键不存在」，遇到
    /// null 会以 `invalid type: null, expected a sequence` 让**整个 chatTools 解析失败**
    /// （连 MCP 服务器、原生工具开关一起丢）。null 归一到空数组。
    #[serde(default, deserialize_with = "null_tolerant_vec")]
    pub hooks: Vec<HookDef>,
    pub native_tools: ChatNativeToolsConfig,
}

impl Default for ChatToolsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            servers: Vec::new(),
            skill_scan_paths: Vec::new(),
            skill_auto_match: default_skill_auto_match(),
            skill_fallback_mode: default_skill_fallback_mode(),
            disabled_skill_ids: Vec::new(),
            max_tool_rounds: default_chat_max_tool_rounds(),
            tool_timeout_ms: default_chat_tool_timeout_ms(),
            mcp_idle_timeout_ms: default_mcp_idle_timeout_ms(),
            max_tool_output_chars: default_max_tool_output_chars(),
            approval_policy: default_chat_approval_policy(),
            sub_agent_concurrency: default_sub_agent_concurrency(),
            sub_agent_provider_id: String::new(),
            sub_agent_model: String::new(),
            request_debug_enabled: false,
            hooks: Vec::new(),
            native_tools: ChatNativeToolsConfig::default(),
        }
    }
}

/// 第三方文档解析服务（MinerU / Doc2X / LlamaParse / 自定义端点）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DocProcessorProvider {
    pub id: String,
    pub name: String,
    /// "mineru" | "doc2x" | "llamaparse" | "custom"
    pub kind: String,
    pub api_keys: Vec<String>,
    pub base_url: String,
    pub enabled: bool,
}

/// 知识库文档处理：Kivio 内置解析 + 可选第三方解析服务及路由策略。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DocumentProcessingConfig {
    /// 图片/可 OCR 内容用的引擎: "off"(默认) | "system" | "rapid_ocr"
    pub ocr_engine: String,
    /// RapidOCR 模型档位:"standard"(PP-OCRv5 mobile) | "high"(默认,PP-OCRv6 medium)。
    /// 知识库入库不在乎慢、要识别质量,故默认高精度。仅在 ocr_engine = "rapid_ocr" 时生效。
    #[serde(default = "default_rapid_ocr_tier_high")]
    pub rapid_ocr_tier: String,
    /// PDF 处理: "text"(默认,文字层) | "force_ocr"(扫描版重扫——内置未启用,会报错)
    pub pdf_strategy: String,
    /// "" = Kivio 内置（本地 Rust）；否则为某第三方 provider id
    pub active_processor: String,
    /// 内置解析失败（如扫描版 PDF）时自动回退到第一个启用的第三方服务。
    pub fallback_to_third_party: bool,
    pub providers: Vec<DocProcessorProvider>,
}

impl Default for DocumentProcessingConfig {
    fn default() -> Self {
        Self {
            ocr_engine: "off".into(),
            rapid_ocr_tier: default_rapid_ocr_tier_high(),
            pdf_strategy: "text".into(),
            active_processor: String::new(),
            fallback_to_third_party: false,
            providers: Vec::new(),
        }
    }
}

/// 知识库检索配置：hybrid(向量+关键词 RRF) 权重 + 可选全局 rerank。
/// 只配 embedding 即可用：hybrid 免配可关，rerank 留空即关、失败降级。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KnowledgeBaseConfig {
    /// 是否启用关键词(BM25)+向量 hybrid 融合（关掉=纯向量）。
    pub hybrid_enabled: bool,
    /// RRF 融合权重（hybrid 开启时生效）。
    pub weight_vector: f32,
    pub weight_keyword: f32,
    /// 全局 rerank：留空即关闭。provider 引用 providers[]，model 为该 provider 的 rerank 模型。
    pub rerank_provider_id: String,
    pub rerank_model: String,
    /// 入库分块目标 tokens（使用处 clamp 到 256..=8192；只影响新导入/重建）。
    pub chunk_tokens: u32,
    /// knowledge_search 默认返回片段数（使用处 clamp 到 1..=20；工具入参可覆盖）。
    /// 这是最终进入上下文的片段数（contextTopK）。
    pub top_k: u32,
    /// 每库融合候选池大小（clamp 20..=200）。召回→融合的候选数，越大召回越全、
    /// 本地检索成本略增；不影响送 rerank 的数量。
    pub candidate_k: u32,
    /// 送 rerank 的候选数（clamp 5..=50）。只在 rerank 开启时生效，直接决定
    /// rerank 网络调用的文档数。
    pub rerank_top_k: u32,
    /// 相关性阈值（D5，0..=1；0 = 关闭，保守默认不误杀）。rerank 开启时对齐
    /// rerank relevance 分数；关闭时为向量-only 命中的余弦相似度下限（词法命中恒过）。
    pub min_score: f32,
}

impl Default for KnowledgeBaseConfig {
    fn default() -> Self {
        Self {
            hybrid_enabled: true,
            weight_vector: 1.0,
            weight_keyword: 1.0,
            rerank_provider_id: String::new(),
            rerank_model: String::new(),
            chunk_tokens: 480,
            top_k: 5,
            candidate_k: 60,
            rerank_top_k: 20,
            min_score: 0.0,
        }
    }
}

impl KnowledgeBaseConfig {
    /// Per-library fused candidate pool size, clamped to safe bounds.
    pub fn candidate_k_clamped(&self) -> usize {
        (self.candidate_k as usize).clamp(20, 200)
    }
    /// How many top candidates to send to the reranker, clamped to safe bounds.
    pub fn rerank_top_k_clamped(&self) -> usize {
        (self.rerank_top_k as usize).clamp(5, 50)
    }
}

/**
 * 独立截图标注功能配置（截图 → 箭头/矩形/马赛克标注 → 复制/保存）
 */
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScreenshotAnnotateConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_screenshot_annotate_hotkey")]
    pub hotkey: String,
}

fn default_screenshot_annotate_hotkey() -> String {
    "CommandOrControl+Shift+S".to_string()
}

impl Default for ScreenshotAnnotateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hotkey: default_screenshot_annotate_hotkey(),
        }
    }
}

/**
 * 应用完整设置
 */
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    /// 打开 AI 客户端（chat 窗口）的全局热键。
    #[serde(default = "default_chat_hotkey")]
    pub chat_hotkey: String,
    /// 关闭 AI 客户端（chat 窗口）的全局热键。
    #[serde(default = "default_close_chat_hotkey")]
    pub close_chat_hotkey: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_theme_color")]
    pub theme_color: String,
    #[serde(default = "default_false")]
    pub translucent_sidebar: bool,
    /// UI 整体缩放（作用于聊天窗口根元素 zoom），范围 0.8–1.4，1.0 为默认。
    #[serde(default = "default_ui_font_scale")]
    pub ui_font_scale: f32,
    /// 自定义 UI 字体名（系统已装字体，拼到字体栈最前）。空串 = 系统默认。
    #[serde(default)]
    pub ui_font_family: String,
    /// 自定义代码/等宽字体名（作用于 --font-mono）。空串 = 系统默认。
    #[serde(default)]
    pub ui_font_mono: String,
    #[serde(default = "default_target_lang")]
    pub target_lang: String,
    #[serde(default = "default_true")]
    pub auto_paste: bool,
    #[serde(default = "default_false")]
    pub launch_at_startup: bool,
    /// 启动后不打开聊天窗口，进程留在托盘。开机自启用户常开这个，避免每次开机弹出界面。
    /// `--from-autostart` 仍会单独跳过弹窗（插件参数偶发丢失时靠本开关兜底）。
    #[serde(default = "default_false")]
    pub launch_minimized_to_tray: bool,
    #[serde(default)]
    pub translator_provider_id: String,
    #[serde(default = "default_openai_model")]
    pub translator_model: String,
    #[serde(default)]
    pub chat_provider_id: String,
    #[serde(default)]
    pub chat_model: String,
    #[serde(default)]
    pub default_models: DefaultModelsConfig,
    #[serde(default)]
    pub translator_prompt: Option<String>,
    #[serde(default)]
    pub providers: Vec<ModelProvider>,
    #[serde(default)]
    pub screenshot_translation: ScreenshotTranslationConfig,
    #[serde(default)]
    pub screenshot_annotate: ScreenshotAnnotateConfig,
    #[serde(default, alias = "cowork")]
    pub lens: LensConfig,
    #[serde(default)]
    pub chat: ChatConfig,
    #[serde(default)]
    pub chat_memory: ChatMemoryConfig,
    #[serde(default)]
    pub chat_tools: ChatToolsConfig,
    #[serde(default)]
    pub document_processing: DocumentProcessingConfig,
    #[serde(default)]
    pub knowledge_base: KnowledgeBaseConfig,
    /// 供应商自定义图标：provider id → 图标 key（前端 PROVIDER_BRANDS 的键）。
    /// ponytail: 单独一张表而不是 ModelProvider 上加字段——那个结构体有 50 处字面量构造，
    /// 而且只有设置界面关心图标。
    #[serde(default)]
    pub provider_icons: std::collections::HashMap<String, String>,
    /// 一次性：将 Lens 的流式/思考开关复制到独立的 Chat 配置（旧版共用 Lens 行为）。
    #[serde(default)]
    pub chat_behavior_migrated_from_lens: bool,
    /// 一次性：工具轮次默认从 20 改为不限后，把存量配置里的旧默认 20 迁到不限。
    /// 迁移后置 true；此后用户在 MCP 页显式选 20 不再被动。
    #[serde(default)]
    pub tool_rounds_unlimited_migrated: bool,
    #[serde(default = "default_settings_language")]
    pub settings_language: Option<String>,
    #[serde(default = "default_retry_enabled")]
    pub retry_enabled: bool,
    #[serde(default = "default_retry_attempts")]
    pub retry_attempts: u8,
    /// 一次性迁移标记：内置专家（写作/编程/研究/数据）已 seed 进 assistants.json 后置 true。
    /// 该迁移会清空整个助手索引（含用户自建——用户明确选择）再装入这 4 个内置专家，
    /// 仅在首次启动跑一次；之后用户新建/删除专家不受影响。
    #[serde(default)]
    pub builtin_assistants_seeded_v1: bool,
    /// 一次性迁移标记（v2，非破坏性）：按 id upsert 新一批内置专家（升级 4 个 + 新增前端/翻译/文档），
    /// 更新旧内置、补齐新增，**保留用户自建**。已 seed v1 的老用户靠它拿到新专家；置 true 后不再跑。
    #[serde(default)]
    pub builtin_assistants_seeded_v2: bool,
    /// 一次性迁移标记（v3，非破坏性）：按 id upsert 补齐产品/法务/财务/教学/审查/求职。
    /// 已 seed v2 的老用户靠它拿到新专家；置 true 后不再跑。
    #[serde(default)]
    pub builtin_assistants_seeded_v3: bool,
    /// 一次性迁移标记：把 pre-green-light 安装（原生工具默认全关 + 旧 approval_policy）
    /// 带到新默认——原生文件/命令工具置 true，且仅当 approval_policy 仍是旧默认时改 "auto"。
    /// 幂等：置 true 后不再翻转，尊重用户此后手动关闭某工具或改 policy 的选择。
    #[serde(default)]
    pub chat_tools_greenlit_v1: bool,
    /// 首次使用引导状态：`pending` | `completed` | `skipped`。
    /// 缺省为空字符串：老版本无此字段时由 `normalize_onboarding_status` 按是否已有 provider 决定。
    #[serde(default)]
    pub onboarding_status: String,
    /// 启动时静默检查 GitHub Releases 是否有新版（默认 true）
    /// 仅做"提示 + 跳转 GH 下载页"，不集成 auto-installer，避免签名密钥那套
    #[serde(default = "default_true")]
    pub auto_check_update: bool,
    /// 截图自动归档开关（默认 false）
    #[serde(default = "default_false")]
    pub image_archive_enabled: bool,
    /// 自动归档目标目录路径（空字符串表示未设置）
    #[serde(default)]
    pub image_archive_path: String,
    /// 用户 Obsidian 笔记库本地路径（空表示未配置）；注入系统提示供 agent 读取笔记。
    #[serde(default)]
    pub obsidian_vault_path: String,
    /// 收藏并置顶的模型键（"providerId:model"）；列表顺序即置顶顺序。
    /// 只在 chat 模型选择器里展示为顶部"收藏"组；失效项（provider 删/禁用/模型没了）展示时过滤。
    #[serde(default)]
    pub favorite_models: Vec<String>,
    // 旧版字段，用于迁移
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openai: Option<OpenAIConfig>,
}

impl Settings {
    /**
     * 根据 ID 查找提供商
     */
    pub fn get_provider(&self, id: &str) -> Option<&ModelProvider> {
        self.providers.iter().find(|p| p.id == id)
    }

    pub fn effective_chat_model(&self) -> (String, String) {
        if self.default_models.chat.is_configured() {
            return (
                self.default_models.chat.provider_id.clone(),
                self.default_models.chat.model.clone(),
            );
        }
        if !self.lens.provider_id.trim().is_empty() {
            return (self.lens.provider_id.clone(), self.lens.model.clone());
        }
        (
            self.translator_provider_id.clone(),
            self.translator_model.clone(),
        )
    }

    pub fn effective_title_summary_model_for_session(
        &self,
        session: Option<SessionModel<'_>>,
    ) -> (String, String) {
        resolve_mixer_side_model(&self.default_models.title_summary, session, self)
    }

    pub fn has_explicit_vision_model(&self) -> bool {
        self.default_models.vision.is_configured()
    }

    pub fn effective_vision_model(&self) -> (String, String) {
        self.effective_vision_model_for_session(None)
    }

    pub fn effective_vision_model_for_session(
        &self,
        session: Option<SessionModel<'_>>,
    ) -> (String, String) {
        resolve_mixer_side_model(&self.default_models.vision, session, self)
    }

    pub fn effective_compression_model_for_session(
        &self,
        session: Option<SessionModel<'_>>,
    ) -> (String, String) {
        resolve_mixer_side_model(&self.default_models.compression, session, self)
    }

    pub fn image_generation_model(&self) -> Option<(String, String)> {
        if self.default_models.image_generation.is_configured()
            && !self.default_models.image_generation.model.trim().is_empty()
        {
            Some((
                self.default_models.image_generation.provider_id.clone(),
                self.default_models.image_generation.model.clone(),
            ))
        } else {
            None
        }
    }

    /// Advisor model (executor-advisor pattern): the `advisor` tool is exposed
    /// only when both provider and model are set. No inheritance — blank = off.
    pub fn advisor_model(&self) -> Option<(String, String)> {
        if self.default_models.advisor.is_configured()
            && !self.default_models.advisor.model.trim().is_empty()
        {
            Some((
                self.default_models.advisor.provider_id.clone(),
                self.default_models.advisor.model.clone(),
            ))
        } else {
            None
        }
    }

    /// Extra OAuth tokens stored outside `chat_tools.servers`, keyed by MCP resource URL.
    /// The MCP manager persists a refreshed token to every slot bound to that URL
    /// without knowing each product name.
    pub(crate) fn detached_oauth_auth_for_url_mut(
        &mut self,
        resource_url: &str,
    ) -> Option<&mut Option<ConnectorAuth>> {
        fn url_matches(left: &str, right: &str) -> bool {
            left.trim().trim_end_matches('/') == right.trim().trim_end_matches('/')
        }
        if url_matches(self.lens.web_search.tinyfish_mcp_url.trim(), resource_url) {
            return Some(&mut self.lens.web_search.tinyfish_mcp_auth);
        }
        None
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "CommandOrControl+Alt+T".to_string(),
            chat_hotkey: "CommandOrControl+Shift+K".to_string(),
            close_chat_hotkey: default_close_chat_hotkey(),
            theme: "system".to_string(),
            theme_color: default_theme_color(),
            translucent_sidebar: false,
            ui_font_scale: default_ui_font_scale(),
            ui_font_family: String::new(),
            ui_font_mono: String::new(),
            target_lang: "auto".to_string(),
            auto_paste: true,
            launch_at_startup: false,
            launch_minimized_to_tray: false,
            translator_provider_id: "default-translator".to_string(),
            translator_model: "gpt-4o".to_string(),
            chat_provider_id: String::new(),
            chat_model: String::new(),
            default_models: DefaultModelsConfig::default(),
            translator_prompt: None,
            providers: vec![],
            screenshot_translation: ScreenshotTranslationConfig::default(),
            screenshot_annotate: ScreenshotAnnotateConfig::default(),
            lens: LensConfig::default(),
            chat: ChatConfig::default(),
            chat_memory: ChatMemoryConfig::default(),
            chat_tools: ChatToolsConfig::default(),
            document_processing: DocumentProcessingConfig::default(),
            knowledge_base: KnowledgeBaseConfig::default(),
            provider_icons: std::collections::HashMap::new(),
            chat_behavior_migrated_from_lens: false,
            tool_rounds_unlimited_migrated: false,
            settings_language: Some("zh".to_string()),
            retry_enabled: default_retry_enabled(),
            retry_attempts: default_retry_attempts(),
            builtin_assistants_seeded_v1: false,
            builtin_assistants_seeded_v2: false,
            builtin_assistants_seeded_v3: false,
            chat_tools_greenlit_v1: false,
            onboarding_status: default_onboarding_status(),
            auto_check_update: true,
            image_archive_enabled: false,
            image_archive_path: String::new(),
            obsidian_vault_path: String::new(),
            favorite_models: Vec::new(),
            openai: None,
        }
    }
}

/**
 * 设置数据清理与迁移
 *
 * 执行以下操作：
 * 1. 从旧版单提供商配置迁移到多提供商体系
 * 2. 确保空 provider 字段有默认值
 * 3. 如果当前模型不在 enabled_models 中则清空或切到第一个启用模型
 * 4. 规范化快捷键字符串
 * 5. 确保必要字段不为空
 */
pub fn chat_native_tools_enabled(chat_tools: &ChatToolsConfig) -> bool {
    chat_tools.native_tools.any_enabled()
}

pub fn chat_memory_tools_enabled(settings: &Settings) -> bool {
    settings.chat_memory.enabled
}

pub fn chat_image_generation_enabled_for_session(
    settings: &Settings,
    session: Option<SessionModel<'_>>,
) -> bool {
    crate::chat::model_metadata::image_generation_model_for_session(settings, session).is_some()
}

pub fn is_skill_enabled(chat_tools: &ChatToolsConfig, skill_id: &str) -> bool {
    let skill_id = skill_id.trim();
    if skill_id.is_empty() {
        return false;
    }
    !chat_tools
        .disabled_skill_ids
        .iter()
        .any(|disabled| disabled == skill_id)
}

/// Bundled skill ids for the Obsidian connector — hidden until a vault path is
/// configured. Adapted from kepano/obsidian-skills (see resources/skills/NOTICE.md).
pub const OBSIDIAN_CONNECTOR_SKILL_IDS: &[&str] = &[
    "obsidian-markdown",
    "obsidian-bases",
    "json-canvas",
    "obsidian-cli",
];

/// The Obsidian connector is "configured" once a vault path is set.
pub fn obsidian_connector_configured(vault_path: &str) -> bool {
    !vault_path.trim().is_empty()
}

/// Connector-backed skills stay unavailable until their connector is configured.
pub fn skill_connector_satisfied(skill_id: &str, obsidian_vault_configured: bool) -> bool {
    if OBSIDIAN_CONNECTOR_SKILL_IDS.contains(&skill_id) {
        return obsidian_vault_configured;
    }
    true
}

/// Global skill gate: Settings enable list + connector prerequisites + 插件门闸。
/// 插件附属 skill（如 officecli）仅在对应插件「已安装且启用」时可用。
pub fn skill_globally_available(
    chat_tools: &ChatToolsConfig,
    skill_id: &str,
    obsidian_vault_configured: bool,
) -> bool {
    if !crate::plugins::plugin_skill_available(skill_id) {
        return false;
    }
    is_skill_enabled(chat_tools, skill_id)
        && skill_connector_satisfied(skill_id, obsidian_vault_configured)
}

/// When [`skill_globally_available`] is false, returns a loop/UI-friendly error.
pub fn skill_global_unavailable_error(
    chat_tools: &ChatToolsConfig,
    skill_id: &str,
    obsidian_vault_configured: bool,
    skill_name: &str,
) -> Option<String> {
    if !crate::plugins::plugin_skill_available(skill_id) {
        if let Some(plugin_id) = crate::plugins::skill_owned_by_plugin(skill_id) {
            return Some(format!(
                "Skill is managed by plugin «{plugin_id}» — enable it in 扩展 → 插件: {skill_name}"
            ));
        }
        return Some(format!("Skill is unavailable: {skill_name}"));
    }
    if !is_skill_enabled(chat_tools, skill_id) {
        return Some(format!("Skill is disabled in Settings: {skill_name}"));
    }
    if !skill_connector_satisfied(skill_id, obsidian_vault_configured) {
        return Some(format!(
            "Skill requires a configured Obsidian connector: {skill_name}"
        ));
    }
    None
}

fn sanitize_default_model_selection(
    selection: &mut DefaultModelSelection,
    providers: &[ModelProvider],
) {
    selection.provider_id = selection.provider_id.trim().to_string();
    selection.model = selection.model.trim().to_string();
    if selection.provider_id.is_empty() {
        selection.model.clear();
        return;
    }

    let Some(provider) = providers
        .iter()
        .find(|p| p.id == selection.provider_id && p.enabled)
    else {
        selection.provider_id.clear();
        selection.model.clear();
        return;
    };

    if !provider.enabled_models.is_empty() && !provider.enabled_models.contains(&selection.model) {
        selection.model = provider.enabled_models.first().cloned().unwrap_or_default();
    }
}

fn sync_legacy_chat_model_fields(settings: &mut Settings) {
    let (provider_id, model) = settings.effective_chat_model();
    settings.chat_provider_id = provider_id;
    settings.chat_model = model;
}

fn mirror_explicit_chat_default_for_persistence(settings: &mut Settings) {
    if settings.default_models.chat.is_configured() {
        settings.chat_provider_id = settings.default_models.chat.provider_id.clone();
        settings.chat_model = settings.default_models.chat.model.clone();
    } else {
        settings.chat_provider_id.clear();
        settings.chat_model.clear();
    }
}

pub fn sanitize_settings(mut settings: Settings) -> Settings {
    // RapidOCR 档位归一:非法值回落到各自默认(截图=standard,文档处理=high)。
    if settings.screenshot_translation.rapid_ocr_tier != "standard"
        && settings.screenshot_translation.rapid_ocr_tier != "high"
    {
        settings.screenshot_translation.rapid_ocr_tier = default_rapid_ocr_tier();
    }
    if settings.document_processing.rapid_ocr_tier != "standard"
        && settings.document_processing.rapid_ocr_tier != "high"
    {
        settings.document_processing.rapid_ocr_tier = default_rapid_ocr_tier_high();
    }
    // 1. 从旧版配置迁移
    if settings.providers.is_empty() {
        // 迁移翻译提供商
        if let Some(old_openai) = settings.openai.take() {
            let legacy_key = old_openai.api_key.trim().to_string();
            let api_keys = if legacy_key.is_empty() {
                vec![]
            } else {
                vec![legacy_key]
            };
            settings.providers.push(ModelProvider {
                id: "default-translator".to_string(),
                name: "OpenAI (Translator)".to_string(),
                api_keys,
                api_key_legacy: None,
                base_url: old_openai.base_url,
                available_models: vec![],
                enabled_models: vec![old_openai.model.clone()],
                enabled: true,
                api_format: "openai".to_string(),
                model_overrides: std::collections::HashMap::new(),
                compress_request_body: false,
                request: Default::default(),
            });
            settings.translator_provider_id = "default-translator".to_string();
            settings.translator_model = old_openai.model;
        }
        // 迁移 OCR 提供商
        if let Some(old_ocr) = settings.screenshot_translation.openai.take() {
            let legacy_key = old_ocr.api_key.trim().to_string();
            let api_keys = if legacy_key.is_empty() {
                vec![]
            } else {
                vec![legacy_key]
            };
            settings.providers.push(ModelProvider {
                id: "default-ocr".to_string(),
                name: "OpenAI (OCR)".to_string(),
                api_keys,
                api_key_legacy: None,
                base_url: old_ocr.base_url,
                available_models: vec![],
                enabled_models: vec![old_ocr.model.clone()],
                enabled: true,
                api_format: "openai".to_string(),
                model_overrides: std::collections::HashMap::new(),
                compress_request_body: false,
                request: Default::default(),
            });
            settings.screenshot_translation.provider_id = "default-ocr".to_string();
            settings.screenshot_translation.model = old_ocr.model;
        }
    }

    // 1b. 单 key → 多 key 迁移（v2.3.1 → v2.4 升级路径）
    for provider in &mut settings.providers {
        provider.api_format = provider.api_format_kind().as_str().to_string();
        if let Some(legacy) = provider.api_key_legacy.take() {
            let trimmed = legacy.trim().to_string();
            if !trimmed.is_empty() && !provider.api_keys.contains(&trimmed) {
                provider.api_keys.insert(0, trimmed);
            }
        }
        // 去重 + 去空
        let mut seen = std::collections::HashSet::new();
        provider.api_keys.retain(|k| {
            let trimmed = k.trim();
            !trimmed.is_empty() && seen.insert(trimmed.to_string())
        });

        // 请求配置归一：settings.json 是用户可以手改的文件，非法头名会让 reqwest
        // 构造请求时直接失败，头值里的 CR/LF 是 header 注入，这里一并丢掉。
        provider
            .request
            .custom_headers
            .retain(crate::provider_request::is_usable_header);
        // Prompt 缓存：合法 none|short|long 优先；旧 bool 仅在 retention 缺失/非法时迁移。
        let retention_ok = matches!(
            provider.request.prompt_cache_retention.as_str(),
            "none" | "short" | "long"
        );
        if let Some(enabled) = provider.request.prompt_caching.take() {
            if !retention_ok {
                provider.request.prompt_cache_retention = if enabled {
                    "short".to_string()
                } else {
                    "none".to_string()
                };
            }
            // retention 已合法：保留用户/新字段，只丢掉遗留 bool。
        } else if !retention_ok {
            provider.request.prompt_cache_retention = "short".to_string();
        }
        if !matches!(
            provider.request.cli_identity.as_str(),
            "" | "claude_code" | "codex" | "grok"
        ) {
            provider.request.cli_identity = String::new();
        }
        // 版本号会被拼进 User-Agent，非法值清空（`identity_version` 会退回内置版本）。
        // 先 trim 再判，与 `identity_version` 同口径：粘贴常带尾换行，那不该算非法。
        let trimmed_version = provider.request.cli_identity_version.trim().to_string();
        provider.request.cli_identity_version =
            if crate::provider_request::is_valid_header_value(&trimmed_version) {
                trimmed_version
            } else {
                String::new()
            };
    }

    let removed_legacy_local_provider_ids: std::collections::HashSet<String> = settings
        .providers
        .iter()
        .filter(|provider| provider.base_url == LEGACY_APPLE_INTELLIGENCE_BASE_URL)
        .map(|provider| provider.id.clone())
        .collect();
    if !removed_legacy_local_provider_ids.is_empty() {
        settings
            .providers
            .retain(|provider| provider.base_url != LEGACY_APPLE_INTELLIGENCE_BASE_URL);
        let fallback = settings.providers.iter().find(|p| p.enabled).map(|p| {
            (
                p.id.clone(),
                p.enabled_models.first().cloned().unwrap_or_default(),
            )
        });

        if removed_legacy_local_provider_ids.contains(&settings.chat_provider_id) {
            if let Some((id, model)) = fallback.as_ref() {
                settings.chat_provider_id = id.clone();
                settings.chat_model = model.clone();
            } else {
                settings.chat_provider_id.clear();
                settings.chat_model.clear();
            }
        }
        if removed_legacy_local_provider_ids.contains(&settings.translator_provider_id) {
            if let Some((id, model)) = fallback.as_ref() {
                settings.translator_provider_id = id.clone();
                settings.translator_model = model.clone();
            } else {
                settings.translator_provider_id.clear();
                settings.translator_model.clear();
            }
        }
        if removed_legacy_local_provider_ids.contains(&settings.screenshot_translation.provider_id)
        {
            if let Some((id, model)) = fallback.as_ref() {
                settings.screenshot_translation.provider_id = id.clone();
                settings.screenshot_translation.model = model.clone();
            } else {
                settings.screenshot_translation.provider_id.clear();
                settings.screenshot_translation.model.clear();
            }
        }
        if !settings.lens.provider_id.is_empty()
            && removed_legacy_local_provider_ids.contains(&settings.lens.provider_id)
        {
            settings.lens.provider_id.clear();
            settings.lens.model.clear();
        }
        for selection in [
            &mut settings.default_models.chat,
            &mut settings.default_models.vision,
            &mut settings.default_models.title_summary,
            &mut settings.default_models.compression,
            &mut settings.default_models.image_generation,
        ] {
            if removed_legacy_local_provider_ids.contains(&selection.provider_id) {
                if let Some((id, model)) = fallback.as_ref() {
                    selection.provider_id = id.clone();
                    selection.model = model.clone();
                } else {
                    selection.provider_id.clear();
                    selection.model.clear();
                }
            }
        }
    }

    let provider_exists = |id: &str| settings.providers.iter().any(|p| p.id == id);
    let provider_selectable = |id: &str| settings.providers.iter().any(|p| p.id == id && p.enabled);
    let first_selectable_provider = || settings.providers.iter().find(|p| p.enabled);

    // 2. 为空字段设置默认值
    if settings.translator_provider_id.is_empty() {
        if let Some(first) = first_selectable_provider() {
            settings.translator_provider_id = first.id.clone();
        }
    }
    if settings.screenshot_translation.provider_id.is_empty() {
        if let Some(first) = first_selectable_provider() {
            settings.screenshot_translation.provider_id = first.id.clone();
        }
    }
    if !settings.chat_provider_id.trim().is_empty()
        && settings.default_models.chat.provider_id.trim().is_empty()
    {
        settings.default_models.chat.provider_id = settings.chat_provider_id.clone();
        settings.default_models.chat.model = settings.chat_model.clone();
    }

    if settings.providers.is_empty() {
        settings.translator_provider_id.clear();
        settings.default_models = DefaultModelsConfig::default();
        settings.screenshot_translation.provider_id.clear();
        settings.lens.provider_id.clear();
        settings.chat_tools.sub_agent_provider_id.clear();
        settings.chat_tools.sub_agent_model.clear();
    } else {
        if !provider_selectable(&settings.translator_provider_id) {
            if let Some(first) = first_selectable_provider() {
                settings.translator_provider_id = first.id.clone();
                if let Some(model) = first.enabled_models.first() {
                    settings.translator_model = model.clone();
                }
            } else if !provider_exists(&settings.translator_provider_id) {
                settings.translator_provider_id.clear();
                settings.translator_model.clear();
            }
        }
        if !provider_selectable(&settings.screenshot_translation.provider_id) {
            if let Some(first) = first_selectable_provider() {
                settings.screenshot_translation.provider_id = first.id.clone();
                if let Some(model) = first.enabled_models.first() {
                    settings.screenshot_translation.model = model.clone();
                }
            } else if !provider_exists(&settings.screenshot_translation.provider_id) {
                settings.screenshot_translation.provider_id.clear();
                settings.screenshot_translation.model.clear();
            }
        }
        // lens provider 可空（空时 call_vision_api 走 translator_provider_id fallback）；
        // 但若用户填了一个不存在或已禁用的，重置为空让其走 fallback。
        if !settings.lens.provider_id.is_empty()
            && (!provider_exists(&settings.lens.provider_id)
                || !provider_selectable(&settings.lens.provider_id))
        {
            settings.lens.provider_id.clear();
            settings.lens.model.clear();
        }

        // 子代理模型覆盖可空（空 = 跟随父会话）；填了不存在/已禁用的 provider 则重置回跟随。
        if !settings.chat_tools.sub_agent_provider_id.is_empty()
            && !provider_selectable(&settings.chat_tools.sub_agent_provider_id)
        {
            settings.chat_tools.sub_agent_provider_id.clear();
            settings.chat_tools.sub_agent_model.clear();
        }

        sanitize_default_model_selection(&mut settings.default_models.chat, &settings.providers);
        sanitize_default_model_selection(&mut settings.default_models.vision, &settings.providers);
        sanitize_default_model_selection(
            &mut settings.default_models.title_summary,
            &settings.providers,
        );
        sanitize_default_model_selection(
            &mut settings.default_models.compression,
            &settings.providers,
        );
        sanitize_default_model_selection(
            &mut settings.default_models.image_generation,
            &settings.providers,
        );
        sanitize_default_model_selection(&mut settings.default_models.advisor, &settings.providers);
    }

    // 3. 确保当前使用的模型确实在该 provider 的 enabled_models 中。
    // enabled_models 可以为空：预设 provider 不再自带模型。
    for provider in &mut settings.providers {
        if settings.translator_provider_id == provider.id
            && !provider.enabled_models.contains(&settings.translator_model)
        {
            settings.translator_model =
                provider.enabled_models.first().cloned().unwrap_or_default();
        }
        if settings.screenshot_translation.provider_id == provider.id
            && !provider
                .enabled_models
                .contains(&settings.screenshot_translation.model)
        {
            settings.screenshot_translation.model =
                provider.enabled_models.first().cloned().unwrap_or_default();
        }
        if !settings.lens.provider_id.is_empty()
            && settings.lens.provider_id == provider.id
            && !settings.lens.model.is_empty()
            && !provider.enabled_models.contains(&settings.lens.model)
        {
            settings.lens.model = provider.enabled_models.first().cloned().unwrap_or_default();
        }
    }

    sync_legacy_chat_model_fields(&mut settings);

    // 4. 规范化快捷键字符串
    settings.hotkey = normalize_hotkey(&settings.hotkey);
    settings.chat_hotkey = normalize_hotkey(&settings.chat_hotkey);
    settings.close_chat_hotkey = normalize_hotkey(&settings.close_chat_hotkey);
    settings.screenshot_translation.hotkey =
        normalize_hotkey(&settings.screenshot_translation.hotkey);
    settings.screenshot_translation.text_hotkey =
        normalize_hotkey(&settings.screenshot_translation.text_hotkey);
    settings.screenshot_translation.replace_hotkey =
        normalize_hotkey(&settings.screenshot_translation.replace_hotkey);
    settings.lens.hotkey = normalize_hotkey(&settings.lens.hotkey);
    settings.screenshot_annotate.hotkey = normalize_hotkey(&settings.screenshot_annotate.hotkey);

    // 规范化提示词（去除首尾空白，空值转为 None）
    settings.translator_prompt = normalize_optional_prompt(settings.translator_prompt.take());
    settings.screenshot_translation.prompt =
        normalize_optional_prompt(settings.screenshot_translation.prompt.take());
    settings.screenshot_translation.text_prompt =
        normalize_optional_prompt(settings.screenshot_translation.text_prompt.take());
    settings.screenshot_translation.replace_prompt =
        normalize_optional_prompt(settings.screenshot_translation.replace_prompt.take());
    // 翻译卡宽度单一真源：import / 手改 settings.json 也在此兜底到 360–720，
    // 与 set_translate_card_size 命令、设置页输入框、Lens 缩放 clamp 同域。
    settings.screenshot_translation.card_width =
        settings.screenshot_translation.card_width.clamp(360, 720);

    // 5. 其他字段验证
    if !matches!(settings.theme.as_str(), "system" | "light" | "dark") {
        settings.theme = default_theme();
    }
    if !matches!(settings.theme_color.as_str(), "neutral" | "warm" | "cool") {
        settings.theme_color = default_theme_color();
    }
    settings.ui_font_scale = if settings.ui_font_scale.is_finite() {
        settings.ui_font_scale.clamp(0.8, 1.4)
    } else {
        default_ui_font_scale()
    };
    if settings.lens.message_order != "asc" && settings.lens.message_order != "desc" {
        settings.lens.message_order = "asc".to_string();
    }
    settings.lens.web_search.tavily_api_key =
        settings.lens.web_search.tavily_api_key.trim().to_string();
    settings.lens.web_search.exa_api_key = settings.lens.web_search.exa_api_key.trim().to_string();
    settings.lens.web_search.ollama_api_key =
        settings.lens.web_search.ollama_api_key.trim().to_string();
    settings.lens.web_search.grok_api_key =
        settings.lens.web_search.grok_api_key.trim().to_string();
    settings.lens.web_search.deepseek_api_key =
        settings.lens.web_search.deepseek_api_key.trim().to_string();
    settings.lens.web_search.brave_api_key =
        settings.lens.web_search.brave_api_key.trim().to_string();
    settings.lens.web_search.serper_api_key =
        settings.lens.web_search.serper_api_key.trim().to_string();
    settings.lens.web_search.bocha_api_key =
        settings.lens.web_search.bocha_api_key.trim().to_string();
    settings.lens.web_search.zhipu_api_key =
        settings.lens.web_search.zhipu_api_key.trim().to_string();
    settings.lens.web_search.tinyfish_api_key =
        settings.lens.web_search.tinyfish_api_key.trim().to_string();
    settings.lens.web_search.tinyfish_mcp_url = {
        let trimmed = settings.lens.web_search.tinyfish_mcp_url.trim();
        if trimmed.is_empty() {
            default_tinyfish_mcp_url()
        } else {
            trimmed.to_string()
        }
    };
    if let Some(auth) = settings.lens.web_search.tinyfish_mcp_auth.as_mut() {
        auth.access_token = auth.access_token.trim().to_string();
    }
    if settings
        .lens
        .web_search
        .tinyfish_mcp_auth
        .as_ref()
        .is_some_and(|auth| auth.access_token.is_empty())
    {
        settings.lens.web_search.tinyfish_mcp_auth = None;
    }
    settings.lens.web_search.searxng_base_url =
        settings.lens.web_search.searxng_base_url.trim().to_string();
    settings.lens.web_search.grok_model = {
        let trimmed = settings.lens.web_search.grok_model.trim();
        if trimmed.is_empty() {
            default_grok_model()
        } else {
            trimmed.to_string()
        }
    };
    settings.lens.web_search.grok_base_url = {
        let trimmed = settings.lens.web_search.grok_base_url.trim();
        if trimmed.is_empty() {
            default_grok_base_url()
        } else {
            trimmed.to_string()
        }
    };
    if settings
        .lens
        .web_search
        .grok_system_prompt
        .trim()
        .is_empty()
    {
        settings.lens.web_search.grok_system_prompt = default_grok_system_prompt();
    }
    settings.lens.web_search.deepseek_model = {
        let trimmed = settings.lens.web_search.deepseek_model.trim();
        if trimmed.is_empty() {
            default_deepseek_model()
        } else {
            trimmed.to_string()
        }
    };
    settings.lens.web_search.deepseek_base_url = {
        let trimmed = settings.lens.web_search.deepseek_base_url.trim();
        if trimmed.is_empty() {
            default_deepseek_base_url()
        } else {
            trimmed.to_string()
        }
    };
    if settings
        .lens
        .web_search
        .deepseek_system_prompt
        .trim()
        .is_empty()
    {
        settings.lens.web_search.deepseek_system_prompt = default_grok_system_prompt();
    }
    settings.lens.web_search.exa_mcp_url = {
        let trimmed = settings.lens.web_search.exa_mcp_url.trim();
        if trimmed.is_empty() {
            default_exa_mcp_url()
        } else {
            trimmed.to_string()
        }
    };
    // 未知/占位服务商回退到 Tavily，避免选中尚未接入的源导致搜索直接报错。
    if matches!(
        settings.lens.web_search.provider,
        WebSearchProvider::Unknown
    ) {
        settings.lens.web_search.provider = WebSearchProvider::Tavily;
    }
    settings.lens.web_search.max_results = settings.lens.web_search.max_results.clamp(1, 10);
    if !matches!(
        settings.lens.web_search.search_depth.as_str(),
        "ultra-fast" | "fast" | "basic" | "advanced"
    ) {
        settings.lens.web_search.search_depth = default_web_search_depth();
    }

    // ponytail: 流式输出 / 思考模式的设置项已从 UI 移除，恒为开启（老配置里的 false 在此归位）
    settings.chat.stream_enabled = true;
    settings.chat.thinking_enabled = true;

    if !settings.chat_behavior_migrated_from_lens {
        if settings.lens.default_language.trim().is_empty() {
            // keep chat.default_language empty → inherit chain unchanged
        } else {
            settings.chat.default_language = settings.lens.default_language.clone();
        }
        settings.chat_behavior_migrated_from_lens = true;
    }
    if !matches!(
        settings.chat.default_language.trim(),
        "" | "zh" | "zh-Hant" | "en"
    ) {
        settings.chat.default_language.clear();
    }
    settings.chat.max_output_tokens = clamp_chat_max_output_tokens(settings.chat.max_output_tokens);
    settings.chat.system_prompt = settings.chat.system_prompt.trim().to_string();

    // 一次性：工具轮次默认从 20 改为不限。存量配置里的 Some(20) 绝大多数是旧默认而非
    // 显式选择，一并归一到不限；显式想要 20 的用户可在 MCP 页重新选（迁移标记置位后
    // 不再改动）。其它显式值（5/10/50/100）原样保留。
    if !settings.tool_rounds_unlimited_migrated {
        if settings.chat_tools.max_tool_rounds == Some(CHAT_TOOL_LEGACY_DEFAULT_ROUNDS) {
            settings.chat_tools.max_tool_rounds = None;
        }
        settings.tool_rounds_unlimited_migrated = true;
    }
    settings.chat_tools.max_tool_rounds = settings
        .chat_tools
        .max_tool_rounds
        .map(|rounds| rounds.clamp(CHAT_TOOL_MIN_ROUNDS, CHAT_TOOL_MAX_ROUNDS));
    settings.chat_tools.tool_timeout_ms = settings
        .chat_tools
        .tool_timeout_ms
        .clamp(CHAT_TOOL_MIN_TIMEOUT_MS, CHAT_TOOL_MAX_TIMEOUT_MS);
    settings.chat_tools.sub_agent_concurrency = settings.chat_tools.sub_agent_concurrency.clamp(
        crate::chat::sub_agent::SUB_AGENT_CONCURRENCY_MIN,
        crate::chat::sub_agent::SUB_AGENT_CONCURRENCY_MAX,
    );
    settings.chat_tools.mcp_idle_timeout_ms = settings
        .chat_tools
        .mcp_idle_timeout_ms
        .clamp(MCP_IDLE_TIMEOUT_MIN_MS, MCP_IDLE_TIMEOUT_MAX_MS);
    // 工具输出截断：None（旧的"不截断"）归一到默认值，Some 值钳到合法区间。
    // 旧逻辑在此无条件置 None（等于永不截断 → 上下文撑爆主因），现改为始终保底截断。
    settings.chat_tools.max_tool_output_chars = Some(
        settings
            .chat_tools
            .max_tool_output_chars
            .unwrap_or(DEFAULT_MAX_TOOL_OUTPUT_CHARS)
            .clamp(CHAT_TOOL_MIN_OUTPUT_CHARS, CHAT_TOOL_MAX_OUTPUT_CHARS),
    );
    if !matches!(
        settings.chat_tools.approval_policy.trim(),
        "readonly_auto_sensitive_confirm" | "always_confirm" | "auto"
    ) {
        settings.chat_tools.approval_policy = default_chat_approval_policy();
    }
    sanitize_hooks(&mut settings.chat_tools.hooks);
    // One-shot green-light migration: bring a pre-green-light install (native
    // tools defaulted OFF + old approval_policy) to the new baseline. Idempotent
    // via `chat_tools_greenlit_v1` so a user who later turns a tool back off, or
    // picks a stricter policy, is never re-flipped. The policy is only changed
    // when it still equals the legacy default, so an explicit choice survives.
    if !settings.chat_tools_greenlit_v1 {
        let native = &mut settings.chat_tools.native_tools;
        native.read_file = true;
        native.write_file = true;
        native.edit_file = true;
        native.run_command = true;
        native.web_fetch = true;
        native.web_search = true;
        if settings.chat_tools.approval_policy == LEGACY_DEFAULT_APPROVAL_POLICY {
            settings.chat_tools.approval_policy = "auto".to_string();
        }
        settings.chat_tools_greenlit_v1 = true;
    }
    settings.chat_tools.skill_scan_paths = settings
        .chat_tools
        .skill_scan_paths
        .into_iter()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .collect();
    if !matches!(
        settings.chat_tools.skill_fallback_mode.trim(),
        "progressive" | "skill_md_only" | "legacy_full_body"
    ) {
        settings.chat_tools.skill_fallback_mode = default_skill_fallback_mode();
    }
    settings.chat_tools.disabled_skill_ids = settings
        .chat_tools
        .disabled_skill_ids
        .into_iter()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect();
    settings.chat_tools.native_tools.workspace_roots = settings
        .chat_tools
        .native_tools
        .workspace_roots
        .into_iter()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .collect();
    let mut seen_roots = std::collections::HashSet::new();
    settings
        .chat_tools
        .native_tools
        .workspace_roots
        .retain(|path| seen_roots.insert(path.clone()));
    settings.chat_tools.native_tools.working_directory = settings
        .chat_tools
        .native_tools
        .working_directory
        .trim()
        .to_string();
    if settings
        .chat_tools
        .native_tools
        .working_directory
        .is_empty()
    {
        settings.chat_tools.native_tools.working_directory = settings
            .chat_tools
            .native_tools
            .workspace_roots
            .first()
            .cloned()
            .unwrap_or_else(default_chat_working_directory);
    }
    // Persist only the new single-directory setting after legacy migration.
    settings.chat_tools.native_tools.workspace_roots.clear();
    for server in &mut settings.chat_tools.servers {
        server.id = server.id.trim().to_string();
        if server.id.is_empty() {
            server.id = format!("mcp-{}", uuid::Uuid::new_v4());
        }
        server.name = server.name.trim().to_string();
        if server.name.is_empty() {
            server.name = server.id.clone();
        }
        server.transport = server.transport.trim().to_ascii_lowercase();
        if server.transport == "http" || server.transport == "sse" {
            server.transport = "streamable_http".to_string();
        }
        if server.transport != "stdio" && server.transport != "streamable_http" {
            server.transport = "stdio".to_string();
        }
        server.url = server.url.trim().to_string();
        server.command = server.command.trim().to_string();
        server.args = server
            .args
            .iter()
            .map(|arg| arg.trim().to_string())
            .filter(|arg| !arg.is_empty())
            .collect();
        server.env = server
            .env
            .iter()
            .filter_map(|(key, value)| {
                let key = key.trim();
                if key.is_empty() {
                    None
                } else {
                    Some((key.to_string(), value.clone()))
                }
            })
            .collect();
        server.headers = server
            .headers
            .iter()
            .filter_map(|(key, value)| {
                let key = key.trim();
                if key.is_empty() {
                    None
                } else {
                    Some((key.to_string(), value.trim().to_string()))
                }
            })
            .collect();
        server.cwd = server.cwd.take().and_then(|cwd| {
            let trimmed = cwd.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        });
        server.enabled_tools = server
            .enabled_tools
            .iter()
            .map(|tool| tool.trim().to_string())
            .filter(|tool| !tool.is_empty())
            .collect();
        // 连接器 id 去空白；空串归一为 None，使其退回普通 MCP server。
        server.connector_id = server.connector_id.take().and_then(|cid| {
            let trimmed = cid.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        });
    }

    // 清理归档目录路径（去除首尾空白）
    settings.image_archive_path = settings.image_archive_path.trim().to_string();
    settings.obsidian_vault_path = settings.obsidian_vault_path.trim().to_string();

    settings.retry_attempts = clamp_retry_attempts(settings.retry_attempts);

    // 系统 OCR 依赖平台本地 OCR 能力（macOS Apple Vision / Windows.Media.Ocr）。其它平台
    // 同步来的旧配置必须关闭，否则截图翻译会误入不可用分支。
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        settings.screenshot_translation.use_system_ocr = false;
    }

    // OCR 引擎模式迁移（vNext+）：
    // 1. 反序列化兜底变体 OcrMode::Legacy（如旧版 "tesseract" 字符串）→ RapidOcr，
    //    保留用户此前选择离线 OCR 的隐私边界；模型未下载时由前端引导下载。
    // 2. 若 ocr_mode 缺省（老版本数据），按 use_system_ocr 反推：
    //    true→System，false→CloudVision
    // 3. Linux 不支持 System / RapidOcr，强制落回 CloudVision
    if matches!(
        settings.screenshot_translation.ocr_mode,
        Some(OcrMode::Legacy)
    ) {
        settings.screenshot_translation.ocr_mode = Some(OcrMode::RapidOcr);
    }
    if settings.screenshot_translation.ocr_mode.is_none() {
        settings.screenshot_translation.ocr_mode =
            Some(if settings.screenshot_translation.use_system_ocr {
                OcrMode::System
            } else {
                OcrMode::CloudVision
            });
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if matches!(
            settings.screenshot_translation.ocr_mode,
            Some(OcrMode::System) | Some(OcrMode::RapidOcr)
        ) {
            settings.screenshot_translation.ocr_mode = Some(OcrMode::CloudVision);
        }
    }

    settings.onboarding_status = normalize_onboarding_status(&settings);

    sanitize_external_cli_agents(&mut settings.chat.external_cli_agents);

    settings
}

/// settings.json 是用户可手改的文件：环境变量名/模型 id 空白或全空格会被原样塞进子进程环境，
/// 表现成难查的「CLI 启动即报错」。这里统一 trim 并丢掉空条目，未知 agent id 的整条配置也丢掉。
fn sanitize_external_cli_agents(
    agents: &mut std::collections::HashMap<String, ExternalCliAgentConfig>,
) {
    agents.retain(|id, cfg| {
        if crate::external_agents::registry::get_agent_def(id).is_none() {
            return false;
        }
        cfg.path = cfg.path.trim().to_string();
        for pair in cfg.env.iter_mut() {
            pair.key = pair.key.trim().to_string();
            pair.value = pair.value.trim().to_string();
        }
        cfg.env.retain(|pair| !pair.key.is_empty());
        for model in cfg.custom_models.iter_mut() {
            model.id = model.id.trim().to_string();
            model.label = model.label.trim().to_string();
        }
        cfg.custom_models.retain(|model| !model.id.is_empty());
        for provider in cfg.providers.iter_mut() {
            provider.id = provider.id.trim().to_string();
            provider.name = provider.name.trim().to_string();
            for pair in provider.env.iter_mut() {
                pair.key = pair.key.trim().to_string();
                pair.value = pair.value.trim().to_string();
            }
            provider.env.retain(|pair| !pair.key.is_empty());
        }
        cfg.providers
            .retain(|provider| !provider.id.is_empty() && !provider.name.is_empty());
        // 悬空的 current_provider（供应商被删了 / 手改错了）必须归零：留着会让注入层
        // 找不到条目而**静默什么都不注入**，用户看到的是「选了供应商但没生效」。
        cfg.current_provider = cfg.current_provider.trim().to_string();
        if !cfg
            .providers
            .iter()
            .any(|provider| provider.id == cfg.current_provider && !provider.disabled)
        {
            cfg.current_provider = String::new();
        }
        true
    });
}

fn default_onboarding_status() -> String {
    "pending".to_string()
}

fn onboarding_status_is_set(raw: &str) -> bool {
    matches!(raw.trim(), "pending" | "completed" | "skipped")
}

fn provider_has_usable_config(provider: &ModelProvider) -> bool {
    provider.enabled
        && provider.api_keys.iter().any(|k| !k.trim().is_empty())
        && !provider.enabled_models.is_empty()
}

fn settings_has_usable_provider_config(settings: &Settings) -> bool {
    settings.providers.iter().any(provider_has_usable_config)
}

fn normalize_onboarding_status(settings: &Settings) -> String {
    let raw = settings.onboarding_status.trim();
    if onboarding_status_is_set(raw) {
        return raw.to_string();
    }
    if settings_has_usable_provider_config(settings) {
        "completed".to_string()
    } else {
        "pending".to_string()
    }
}

/**
 * 持久化设置到存储文件
 * 从 v2.4 起 API Key 直接保存在 settings.json 的 api_keys 数组中
 *
 * 降级兼容：写盘前把 api_keys[0] 镜像到 api_key_legacy（serde rename = "apiKey"）字段，
 * 这样老版本（v2.3.x）反序列化时仍能从 apiKey 字段读到主 key 不丢。
 * 新版加载时 sanitize_settings 会把 api_key_legacy.take() 合并回 api_keys 并去重，无副作用。
 */
pub fn persist_settings(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    crate::external_agents::overrides::sync_from_settings(settings);
    // 镜像同步之后立刻物化：供应商的落地文件（claude 的 `--settings` 覆盖 / codex 的私有
    // CODEX_HOME）必须与设置同生共死。放在这里而不是让前端保存后再调一个命令，是因为
    // 前端只要漏调一次，用户就会得到「选了供应商但没生效」——而这种 bug 完全不报错。
    crate::external_agents::provider_profile::materialize_all();
    // 供应商可能变了：模型列表（300s）与可用性（600s）两个探测缓存都得作废。
    // 首次启动的内置专家迁移会在 AppState manage 之前保存设置，此时还没有缓存可清；
    // 必须用 try_state，否则新装用户会在启动期 panic。
    use tauri::Manager;
    if let Some(state) = app.try_state::<crate::state::AppState>() {
        state.clear_all_external_agent_models_cache();
        state.clear_detected_agents_cache();
    }
    let mut to_persist = settings.clone();
    // Keep legacy top-level chat fields from turning Lens/Translator fallback into
    // an explicit defaultModels.chat selection on the next load.
    mirror_explicit_chat_default_for_persistence(&mut to_persist);

    for provider in &mut to_persist.providers {
        if let Some(primary) = provider.api_keys.first() {
            if !primary.trim().is_empty() {
                provider.api_key_legacy = Some(primary.clone());
            }
        }
    }

    // 降级镜像：把 ocr_mode 投影回 use_system_ocr，让降级到 v2.5.x 的版本仍能从 useSystemOcr 字段
    // 读到对应行为。RapidOcr 模式镜像为 false（v2.5.x 没有 RapidOCR 概念，落回 CloudVision）。
    let ocr_mode = to_persist
        .screenshot_translation
        .ocr_mode
        .unwrap_or(OcrMode::CloudVision);
    to_persist.screenshot_translation.use_system_ocr = matches!(ocr_mode, OcrMode::System);
    to_persist.screenshot_translation.ocr_mode = Some(ocr_mode);

    let store = StoreBuilder::new(app, SETTINGS_STORE)
        .build()
        .map_err(|e| e.to_string())?;
    store.set(
        "settings".to_string(),
        serde_json::to_value(&to_persist).map_err(|e| e.to_string())?,
    );
    store.save().map_err(|e| e.to_string())
}

/**
 * 一次性数据迁移：v2.4.5 把 identifier 从 com.zmair.keylingo 改为 com.zmair.kivio。
 * Tauri 的 app_data_dir 直接由 identifier 派生，改名后新目录是空的，
 * 老用户升级会丢失 settings.json / lens-history。这里在新目录还没数据时，
 * 把同级的旧目录整个递归拷贝过来。
 *
 * 幂等：新目录已存在 settings.json → 跳过；旧目录不存在 → 跳过（全新安装）。
 */
fn migrate_legacy_app_data(app: &AppHandle) {
    use tauri::Manager;
    let new_dir = match app.path().app_data_dir() {
        Ok(d) => d,
        Err(err) => {
            eprintln!("[migrate-app-data] app_data_dir unavailable: {err}");
            return;
        }
    };
    if new_dir.join(SETTINGS_STORE).exists() {
        return;
    }

    let Some(parent) = new_dir.parent() else {
        return;
    };
    // 旧 identifier 的目录名就是 identifier 本身（macOS / Windows / Linux 都一致）
    let legacy_dir = parent.join("com.zmair.keylingo");
    if !legacy_dir.is_dir() {
        return;
    }

    if let Err(err) = std::fs::create_dir_all(&new_dir) {
        eprintln!("[migrate-app-data] mkdir new dir failed: {err}");
        return;
    }

    match copy_dir_recursive(&legacy_dir, &new_dir) {
        Ok(()) => eprintln!(
            "[migrate-app-data] copied legacy app data: {} → {}",
            legacy_dir.display(),
            new_dir.display()
        ),
        Err(err) => eprintln!("[migrate-app-data] copy failed: {err}"),
    }
}

fn copy_dir_recursive(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_dir_recursive(&src, &dst)?;
        } else if src.is_file() && !dst.exists() {
            // 不覆盖已有目标文件：避免与用户在新路径下手动建/写过的内容冲突
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/**
 * 从存储文件加载设置
 * 执行清理迁移（legacy identifier 目录、sanitize）
 */
pub fn load_settings(app: &AppHandle) -> Settings {
    // 入口先把旧 identifier 目录的数据搬到新目录（幂等）
    migrate_legacy_app_data(app);
    let store = StoreBuilder::new(app, SETTINGS_STORE).build();
    let settings = match store {
        Ok(store) => store
            .get("settings")
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default(),
        Err(_) => Settings::default(),
    };
    let settings = sanitize_settings(settings);
    crate::external_agents::overrides::sync_from_settings(&settings);
    settings
}

// ========== 默认提示词生成 ==========

/**
 * 获取默认系统提示词
 * has_image=true 时为视觉助手；为 false 时为通用对话助手（不假设有图片）
 * 风格统一：简短直答、无小标题、思考过程尽量精简
 */
/// Local system date for Chat date questions. Models must not guess dates from training data.
/// 只到日期级、不含时分：系统提示词是每轮请求的公共前缀，分钟级时钟会让同一对话每轮前缀
/// 都变——打穿 provider 的 prompt cache（前缀匹配），也让会话亲和型代理无法续会话。
/// 回答"今天/明天/星期几"日期粒度已足够。
pub fn chat_current_datetime_context(language: &str) -> String {
    let now = Local::now();
    let weekday = weekday_label(language, now.weekday());
    if language.starts_with("zh") {
        format!(
            "\n\n当前日期（系统时钟；回答今天/明天/星期几等日期问题必须以此为准，禁止凭记忆臆测）：{}年{}月{}日 {}。",
            now.year(),
            now.month(),
            now.day(),
            weekday
        )
    } else {
        format!(
            "\n\nToday's date (system clock; use for today/tomorrow/weekday questions—never guess from training data): {}-{:02}-{:02} {}.",
            now.year(),
            now.month(),
            now.day(),
            weekday
        )
    }
}

fn weekday_label(language: &str, weekday: chrono::Weekday) -> &'static str {
    if language.starts_with("zh") {
        match weekday {
            chrono::Weekday::Mon => "星期一",
            chrono::Weekday::Tue => "星期二",
            chrono::Weekday::Wed => "星期三",
            chrono::Weekday::Thu => "星期四",
            chrono::Weekday::Fri => "星期五",
            chrono::Weekday::Sat => "星期六",
            chrono::Weekday::Sun => "星期日",
        }
    } else {
        match weekday {
            chrono::Weekday::Mon => "Monday",
            chrono::Weekday::Tue => "Tuesday",
            chrono::Weekday::Wed => "Wednesday",
            chrono::Weekday::Thu => "Thursday",
            chrono::Weekday::Fri => "Friday",
            chrono::Weekday::Sat => "Saturday",
            chrono::Weekday::Sun => "Sunday",
        }
    }
}

/// Lens 默认系统提示（含截图翻译后的视觉问答）：输出紧凑，尽量不输出空行。
pub fn default_lens_system_prompt(language: &str, has_image: bool) -> String {
    match (language.starts_with("zh"), has_image) {
        (true, true) => "你是一位智能助手，能够看到用户分享的截图。请将其作为视觉上下文来理解和回答，可以涉及信息提取、概念解释、操作协助或任何相关话题。保持回答简洁直接，自然流畅，不用小标题和编号。输出必须紧凑：不要输出空行；只有在真正需要分隔段落、列表项、表格行、代码块或数学公式时才换行；列表项之间不要留空行。数学公式用 LaTeX（$...$ 或 $$...$$）。思考保持简洁，避免反复重述。".to_string(),
        (true, false) => "你是一位智能助手。直接给出答案，回答简洁、自然流畅，不要小标题或编号。输出必须紧凑：不要输出空行；只有在真正需要分隔段落、列表项、表格行、代码块或数学公式时才换行；列表项之间不要留空行。数学公式用 LaTeX（$...$ 或 $$...$$）。思考保持简洁，避免反复重述。".to_string(),
        (_, true) => "You are a helpful assistant that can see the user's screenshot. Use it as visual context to understand and answer, whether extracting information, explaining concepts, assisting with tasks, or any relevant topic. Keep responses short and natural, with no headings or bullet points unless a list is genuinely useful. Keep output compact: do not output blank lines; use a single newline only when needed for clear paragraph boundaries, list items, table rows, code blocks, or math; never put empty lines between list items. Use LaTeX ($...$ or $$...$$) for math. Think briefly; avoid repeating yourself.".to_string(),
        (_, false) => "You are a helpful assistant. Answer directly. Keep responses short and natural, with no headings or bullet points unless a list is genuinely useful. Keep output compact: do not output blank lines; use a single newline only when needed for clear paragraph boundaries, list items, table rows, code blocks, or math; never put empty lines between list items. Use LaTeX ($...$ or $$...$$) for math. Think briefly; avoid repeating yourself.".to_string(),
    }
}

/// Chat 客户端默认系统提示：允许正常 Markdown（含表格），不强制「不要空行」。
pub fn default_chat_system_prompt(has_image: bool) -> String {
    if has_image {
        "You are the AI assistant inside Kivio. You can help users write, analyze documents/data, search the web, run code for calculations, edit files, and answer questions. You can use images the user provides. Answer clearly and concisely; Markdown is welcome (tables, lists, code blocks—each table row on its own line). Use LaTeX ($...$ or $$...$$) for math. Think briefly.".to_string()
    } else {
        "You are the AI assistant inside Kivio. You can help users write, analyze documents/data, search the web, run code for calculations, edit files, and answer questions. Answer clearly, directly, and concisely; Markdown is welcome (tables, lists, code blocks—each table row on its own line). Use LaTeX ($...$ or $$...$$) for math. Think briefly.".to_string()
    }
}

/**
 * Lens：关闭思考模式时附加到系统提示词末尾（含紧凑输出要求）。
 */
pub fn no_think_instruction(language: &str) -> &'static str {
    if language.starts_with("zh") {
        "\n\n严格要求：直接给出最终答案，不要输出任何思考过程、推理步骤或 <think> 内容。保持输出紧凑，不要输出空行。"
    } else {
        "\n\nStrict requirement: output only the final answer; do NOT include any thinking, reasoning steps, or <think> content. Keep output compact; do not output blank lines."
    }
}

/// Chat：关闭思考模式时的附加指令（不要求紧凑、不禁止空行）。
pub fn chat_no_think_instruction() -> &'static str {
    "\n\nStrict requirement: output only the final answer; do NOT include any thinking, reasoning steps, or <think> content."
}

/**
 * 获取默认问答提示词
 * has_image=true 时让模型聚焦图片内容；has_image=false 时返回空串（不附加前缀，直接传用户原话）
 */
pub fn default_question_prompt(language: &str, has_image: bool) -> String {
    if !has_image {
        return String::new();
    }
    if language.starts_with("zh") {
        "用户分享了这张截图，请结合其中的视觉信息来理解和回答：".to_string()
    } else {
        "The user shared this screenshot. Use the visual context to understand and answer:"
            .to_string()
    }
}

// ========== 默认值辅助函数 ==========

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_api_format() -> String {
    "openai_chat".to_string()
}

fn default_hotkey() -> String {
    "CommandOrControl+Alt+T".to_string()
}

fn default_chat_hotkey() -> String {
    "CommandOrControl+Shift+K".to_string()
}

fn default_close_chat_hotkey() -> String {
    "CommandOrControl+Shift+W".to_string()
}

fn default_screenshot_translation_hotkey() -> String {
    "CommandOrControl+Shift+A".to_string()
}

fn default_screenshot_translation_text_hotkey() -> String {
    "CommandOrControl+Shift+T".to_string()
}

fn default_screenshot_translation_replace_hotkey() -> String {
    "CommandOrControl+Shift+R".to_string()
}

/// 快速翻译结果卡默认宽度（px）。介于旧截图卡(~514)与选中文本卡(420)之间。
fn default_translate_card_width() -> u32 {
    480
}

fn default_lens_hotkey() -> String {
    "CommandOrControl+Shift+G".to_string()
}

fn default_theme() -> String {
    "system".to_string()
}

fn default_theme_color() -> String {
    "neutral".to_string()
}

fn default_ui_font_scale() -> f32 {
    1.0
}

fn default_target_lang() -> String {
    "auto".to_string()
}

fn default_openai_base_url() -> String {
    "https://api.openai.com/v1".to_string()
}

fn default_openai_model() -> String {
    "gpt-4o".to_string()
}

fn default_settings_language() -> Option<String> {
    Some("zh".to_string())
}

fn default_retry_attempts() -> u8 {
    5
}

fn default_retry_enabled() -> bool {
    true
}

fn clamp_retry_attempts(value: u8) -> u8 {
    value.clamp(1, 8)
}

/**
 * 规范化可选提示词：去除空白，空字符串转为 None
 */
fn normalize_optional_prompt(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/**
 * 规范化快捷键字符串：去除各部分首尾空白并过滤空部分
 */
fn normalize_hotkey(value: &str) -> String {
    value
        .split('+')
        .map(|part| {
            let trimmed = part.trim();
            match trimmed.to_lowercase().as_str() {
                "cmd" | "command" | "commandorcontrol" => "CommandOrControl".to_string(),
                "ctrl" | "control" => "Control".to_string(),
                "opt" | "option" | "alt" => "Alt".to_string(),
                "shift" => "Shift".to_string(),
                "super" | "meta" => "Super".to_string(),
                "plus" => "Plus".to_string(),
                _ => trimmed.to_string(),
            }
        })
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_settings_disable_translucent_sidebar_by_default() {
        let settings: Settings =
            serde_json::from_str("{}").expect("legacy settings should deserialize");
        assert!(!settings.translucent_sidebar);
    }

    #[test]
    fn legacy_model_info_without_temperature_deserializes_as_absent() {
        let info: ModelInfo = serde_json::from_str(
            r#"{"displayName":"Legacy","contextWindow":8192,"maxOutput":2048}"#,
        )
        .expect("legacy model info should deserialize");
        assert_eq!(info.temperature, None);
        assert_eq!(info.omit_temperature, None);
    }

    // ===== normalize_hotkey =====

    #[test]
    fn normalize_hotkey_canonicalizes_aliases() {
        // 仅规范修饰键名（cmd/ctrl/opt/super/meta），按键名 case 透传
        assert_eq!(normalize_hotkey("cmd+shift+a"), "CommandOrControl+Shift+a");
        assert_eq!(normalize_hotkey("Command+Alt+T"), "CommandOrControl+Alt+T");
        assert_eq!(normalize_hotkey("ctrl+shift+G"), "Control+Shift+G");
        assert_eq!(normalize_hotkey("opt+space"), "Alt+space");
        assert_eq!(normalize_hotkey("option+x"), "Alt+x");
        assert_eq!(normalize_hotkey("super+L"), "Super+L");
        assert_eq!(normalize_hotkey("meta+L"), "Super+L");
    }

    #[test]
    fn normalize_hotkey_preserves_key_case() {
        // 按键名大小写不被改动（Tauri 全局快捷键大小写敏感）
        assert_eq!(normalize_hotkey("cmd+a"), "CommandOrControl+a");
        assert_eq!(normalize_hotkey("cmd+A"), "CommandOrControl+A");
    }

    #[test]
    fn normalize_hotkey_trims_whitespace() {
        assert_eq!(
            normalize_hotkey(" cmd + shift + a "),
            "CommandOrControl+Shift+a"
        );
    }

    #[test]
    fn normalize_hotkey_filters_empty_parts() {
        assert_eq!(normalize_hotkey("cmd++a"), "CommandOrControl+a");
        assert_eq!(normalize_hotkey("+cmd+a+"), "CommandOrControl+a");
    }

    #[test]
    fn normalize_hotkey_preserves_unknown_keys_verbatim() {
        // F1, Backspace 等键名直接透传，不做 case 转换
        assert_eq!(normalize_hotkey("cmd+F1"), "CommandOrControl+F1");
        assert_eq!(normalize_hotkey("ctrl+Backspace"), "Control+Backspace");
    }

    // ===== sanitize_settings =====

    #[test]
    fn sanitize_settings_clamps_retry_attempts() {
        let mut s = Settings::default();
        s.retry_attempts = 0;
        let s = sanitize_settings(s);
        assert!((1..=8).contains(&s.retry_attempts));

        let mut s = Settings::default();
        s.retry_attempts = 99;
        let s = sanitize_settings(s);
        assert!((1..=8).contains(&s.retry_attempts));
    }

    #[test]
    fn sanitize_settings_clamps_chat_max_output_tokens() {
        let mut s = Settings::default();
        s.chat.max_output_tokens = 0;
        let s = sanitize_settings(s);
        assert_eq!(s.chat.max_output_tokens, 512);

        let mut s = Settings::default();
        s.chat.max_output_tokens = 100_000;
        let s = sanitize_settings(s);
        assert_eq!(s.chat.max_output_tokens, 65_536);
    }

    #[test]
    fn sanitize_settings_resets_unknown_theme_values() {
        let mut s = Settings::default();
        s.theme = "sepia".to_string();
        s.theme_color = "mint".to_string();
        let s = sanitize_settings(s);
        assert_eq!(s.theme, "system");
        assert_eq!(s.theme_color, "neutral");
    }

    #[test]
    fn sanitize_settings_clamps_ui_font_scale() {
        let mut s = Settings::default();
        s.ui_font_scale = 5.0;
        assert_eq!(sanitize_settings(s).ui_font_scale, 1.4);
        let mut s = Settings::default();
        s.ui_font_scale = 0.1;
        assert_eq!(sanitize_settings(s).ui_font_scale, 0.8);
        let mut s = Settings::default();
        s.ui_font_scale = f32::NAN;
        assert_eq!(sanitize_settings(s).ui_font_scale, 1.0);
    }

    #[test]
    fn sanitize_settings_normalizes_hotkeys() {
        let mut s = Settings::default();
        s.hotkey = "cmd+alt+T".to_string();
        s.chat_hotkey = "cmd+shift+K".to_string();
        s.close_chat_hotkey = "cmd+shift+W".to_string();
        s.screenshot_translation.hotkey = "ctrl+shift+A".to_string();
        s.screenshot_translation.text_hotkey = "cmd+shift+T".to_string();
        s.lens.hotkey = "cmd+shift+G".to_string();
        let s = sanitize_settings(s);
        assert_eq!(s.hotkey, "CommandOrControl+Alt+T");
        assert_eq!(s.chat_hotkey, "CommandOrControl+Shift+K");
        assert_eq!(s.close_chat_hotkey, "CommandOrControl+Shift+W");
        assert_eq!(s.screenshot_translation.hotkey, "Control+Shift+A");
        assert_eq!(
            s.screenshot_translation.text_hotkey,
            "CommandOrControl+Shift+T"
        );
        assert_eq!(s.lens.hotkey, "CommandOrControl+Shift+G");
    }

    #[test]
    fn sanitize_settings_preserves_empty_hotkeys() {
        let mut s = Settings::default();
        s.hotkey = String::new();
        s.screenshot_translation.hotkey = String::new();
        s.screenshot_translation.text_hotkey = String::new();
        s.lens.hotkey = String::new();
        let s = sanitize_settings(s);
        assert_eq!(s.hotkey, "");
        assert_eq!(s.screenshot_translation.hotkey, "");
        assert_eq!(s.screenshot_translation.text_hotkey, "");
        assert_eq!(s.lens.hotkey, "");
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn sanitize_settings_disables_system_ocr_on_unsupported_platforms() {
        let mut s = Settings::default();
        s.screenshot_translation.ocr_mode = Some(OcrMode::System);
        let s = sanitize_settings(s);
        assert_eq!(
            s.screenshot_translation.ocr_mode,
            Some(OcrMode::CloudVision)
        );
    }

    #[test]
    fn sanitize_settings_migrates_use_system_ocr_true_to_system_mode() {
        // 老版本数据：useSystemOcr=true 但没有 ocr_mode 字段
        let mut s = Settings::default();
        s.screenshot_translation.use_system_ocr = true;
        s.screenshot_translation.ocr_mode = None;
        let s = sanitize_settings(s);
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        assert_eq!(s.screenshot_translation.ocr_mode, Some(OcrMode::System));
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        assert_eq!(
            s.screenshot_translation.ocr_mode,
            Some(OcrMode::CloudVision)
        );
    }

    #[test]
    fn sanitize_settings_migrates_use_system_ocr_false_to_cloud_vision_mode() {
        let mut s = Settings::default();
        s.screenshot_translation.use_system_ocr = false;
        s.screenshot_translation.ocr_mode = None;
        let s = sanitize_settings(s);
        assert_eq!(
            s.screenshot_translation.ocr_mode,
            Some(OcrMode::CloudVision)
        );
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn sanitize_settings_preserves_rapidocr_mode() {
        let mut s = Settings::default();
        s.screenshot_translation.ocr_mode = Some(OcrMode::RapidOcr);
        let s = sanitize_settings(s);
        assert_eq!(s.screenshot_translation.ocr_mode, Some(OcrMode::RapidOcr));
    }

    #[test]
    fn rapid_ocr_tier_defaults_for_legacy_configs() {
        // 旧版 settings.json 没有 rapid_ocr_tier 字段:截图翻译默认 "standard"(现有 v5
        // mobile 用户零感知),知识库文档处理默认 "high"(入库要识别质量)。
        let screenshot: ScreenshotTranslationConfig =
            serde_json::from_str("{}").expect("empty screenshot config should load");
        assert_eq!(screenshot.rapid_ocr_tier, "standard");

        let doc_processing: DocumentProcessingConfig =
            serde_json::from_str("{}").expect("empty document processing config should load");
        assert_eq!(doc_processing.rapid_ocr_tier, "high");
    }

    #[test]
    fn sanitize_settings_normalizes_invalid_rapid_ocr_tier() {
        let mut s = Settings::default();
        s.screenshot_translation.rapid_ocr_tier = "garbage".to_string();
        s.document_processing.rapid_ocr_tier = "garbage".to_string();
        let s = sanitize_settings(s);
        assert_eq!(s.screenshot_translation.rapid_ocr_tier, "standard");
        assert_eq!(s.document_processing.rapid_ocr_tier, "high");
    }

    #[test]
    fn sanitize_settings_migrates_legacy_tesseract_to_rapidocr() {
        // 旧版本 settings.json 含 "ocrMode": "tesseract"——序列化后落到 OcrMode::Legacy
        // 兜底变体,sanitize_settings 把它迁移到 RapidOcr,避免从本地 OCR 静默变成云端视觉。
        let json = r#"{"ocrMode":"tesseract"}"#;
        let cfg: ScreenshotTranslationConfig =
            serde_json::from_str(json).expect("legacy variant should deserialize");
        assert_eq!(cfg.ocr_mode, Some(OcrMode::Legacy));

        let mut s = Settings::default();
        s.screenshot_translation.ocr_mode = Some(OcrMode::Legacy);
        let s = sanitize_settings(s);
        assert_eq!(s.screenshot_translation.ocr_mode, Some(OcrMode::RapidOcr));
    }

    #[test]
    fn ocr_mode_serializes_with_snake_case() {
        // ocrMode 在 settings.json 里是 snake_case 字符串(cloud_vision / system / rapid_ocr)。
        // 前端 union type 'cloud_vision' | 'system' | 'rapid_ocr' 直接对齐。
        let modes = [
            (OcrMode::CloudVision, "\"cloud_vision\""),
            (OcrMode::System, "\"system\""),
            (OcrMode::RapidOcr, "\"rapid_ocr\""),
        ];
        for (mode, expected) in modes {
            assert_eq!(serde_json::to_string(&mode).unwrap(), expected);
        }
    }

    #[test]
    fn sanitize_settings_removes_legacy_apple_local_provider() {
        let mut s = Settings::default();
        s.providers.push(ModelProvider {
            id: "apple".to_string(),
            name: "Legacy Apple Local".to_string(),
            api_keys: vec!["__on_device__".to_string()],
            api_key_legacy: None,
            base_url: LEGACY_APPLE_INTELLIGENCE_BASE_URL.to_string(),
            available_models: vec![],
            enabled_models: vec!["apple-foundation".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.providers.push(ModelProvider {
            id: "cloud".to_string(),
            name: "Cloud".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["gpt-4o".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.translator_provider_id = "apple".to_string();
        s.translator_model = "apple-foundation".to_string();
        s.screenshot_translation.provider_id = "apple".to_string();
        s.screenshot_translation.model = "apple-foundation".to_string();
        s.lens.provider_id = "apple".to_string();
        s.lens.model = "apple-foundation".to_string();
        s.chat_provider_id = "apple".to_string();
        s.chat_model = "apple-foundation".to_string();
        s.default_models.chat.provider_id = "apple".to_string();
        s.default_models.chat.model = "apple-foundation".to_string();
        s.default_models.vision.provider_id = "apple".to_string();
        s.default_models.vision.model = "apple-foundation".to_string();
        s.default_models.title_summary.provider_id = "apple".to_string();
        s.default_models.title_summary.model = "apple-foundation".to_string();
        s.default_models.compression.provider_id = "apple".to_string();
        s.default_models.compression.model = "apple-foundation".to_string();
        s.default_models.image_generation.provider_id = "apple".to_string();
        s.default_models.image_generation.model = "apple-foundation".to_string();

        let s = sanitize_settings(s);
        assert!(s.providers.iter().all(|provider| provider.id != "apple"));
        assert_eq!(s.translator_provider_id, "cloud");
        assert_eq!(s.translator_model, "gpt-4o");
        assert_eq!(s.screenshot_translation.provider_id, "cloud");
        assert_eq!(s.screenshot_translation.model, "gpt-4o");
        assert_eq!(s.lens.provider_id, "");
        assert_eq!(s.lens.model, "");
        assert_eq!(s.default_models.chat.provider_id, "cloud");
        assert_eq!(s.default_models.chat.model, "gpt-4o");
        assert_eq!(s.default_models.vision.provider_id, "cloud");
        assert_eq!(s.default_models.title_summary.provider_id, "cloud");
        assert_eq!(s.default_models.compression.provider_id, "cloud");
        assert_eq!(s.default_models.image_generation.provider_id, "cloud");
    }

    #[test]
    fn sanitize_settings_migrates_legacy_apikey_to_apikeys() {
        let mut s = Settings::default();
        s.providers.push(ModelProvider {
            id: "p".to_string(),
            name: "P".to_string(),
            api_keys: vec![],
            api_key_legacy: Some("sk-legacy".to_string()),
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["m".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        let s = sanitize_settings(s);
        let p = s.get_provider("p").unwrap();
        assert_eq!(p.api_keys, vec!["sk-legacy".to_string()]);
        assert!(p.api_key_legacy.is_none(), "legacy field should be drained");
    }

    #[test]
    fn sanitize_settings_dedupes_apikey_legacy_against_apikeys() {
        let mut s = Settings::default();
        s.providers.push(ModelProvider {
            id: "p".to_string(),
            name: "P".to_string(),
            api_keys: vec!["sk-1".to_string(), "sk-2".to_string()],
            api_key_legacy: Some("sk-1".to_string()), // 已在 api_keys 中
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["m".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        let s = sanitize_settings(s);
        let p = s.get_provider("p").unwrap();
        assert_eq!(
            p.api_keys.len(),
            2,
            "duplicate legacy key should not be inserted"
        );
    }

    #[test]
    fn sanitize_settings_filters_empty_apikeys() {
        let mut s = Settings::default();
        s.providers.push(ModelProvider {
            id: "p".to_string(),
            name: "P".to_string(),
            api_keys: vec!["sk-1".to_string(), "  ".to_string(), String::new()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["m".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        let s = sanitize_settings(s);
        let p = s.get_provider("p").unwrap();
        assert_eq!(p.api_keys, vec!["sk-1".to_string()]);
    }

    #[test]
    fn chat_tools_default_limits_keep_tool_round_cap() {
        // 工具轮次默认不限（None）；输出截断默认仍在。
        assert_eq!(ChatToolsConfig::default().max_tool_rounds, None);
        assert_eq!(
            ChatToolsConfig::default().max_tool_output_chars,
            Some(DEFAULT_MAX_TOOL_OUTPUT_CHARS)
        );

        let cfg: ChatToolsConfig =
            serde_json::from_str("{}").expect("empty chat tools config should load");
        assert_eq!(cfg.max_tool_rounds, None);
        // 缺省字段经 serde default 补成默认截断值（而非 None/不截断）。
        assert_eq!(
            cfg.max_tool_output_chars,
            Some(DEFAULT_MAX_TOOL_OUTPUT_CHARS)
        );
    }

    #[test]
    fn sanitize_settings_clamps_chat_tool_round_limit_and_keeps_unlimited() {
        let mut settings = Settings::default();
        settings.chat_tools.max_tool_rounds = Some(CHAT_TOOL_MAX_ROUNDS + 30);
        settings.chat_tools.max_tool_output_chars = Some(12_000);

        let settings = sanitize_settings(settings);

        assert_eq!(
            settings.chat_tools.max_tool_rounds,
            Some(CHAT_TOOL_MAX_ROUNDS)
        );
        // 合法区间内的值原样保留（不再被无条件清成 None）。
        assert_eq!(settings.chat_tools.max_tool_output_chars, Some(12_000));

        let mut settings = Settings::default();
        settings.chat_tools.max_tool_rounds = None;

        let settings = sanitize_settings(settings);

        assert_eq!(settings.chat_tools.max_tool_rounds, None);
    }

    #[test]
    fn sanitize_migrates_legacy_default_tool_rounds_to_unlimited_once() {
        // 存量配置：旧默认 20、迁移标记未置位 → 归一到不限。
        let mut settings = Settings::default();
        settings.tool_rounds_unlimited_migrated = false;
        settings.chat_tools.max_tool_rounds = Some(CHAT_TOOL_LEGACY_DEFAULT_ROUNDS);
        let settings = sanitize_settings(settings);
        assert_eq!(settings.chat_tools.max_tool_rounds, None);
        assert!(settings.tool_rounds_unlimited_migrated);

        // 迁移后用户显式选回 20 → 保留，不再被动。
        let mut settings = settings;
        settings.chat_tools.max_tool_rounds = Some(CHAT_TOOL_LEGACY_DEFAULT_ROUNDS);
        let settings = sanitize_settings(settings);
        assert_eq!(
            settings.chat_tools.max_tool_rounds,
            Some(CHAT_TOOL_LEGACY_DEFAULT_ROUNDS)
        );

        // 显式非默认值（如 50）即使标记未置位也原样保留。
        let mut settings = Settings::default();
        settings.tool_rounds_unlimited_migrated = false;
        settings.chat_tools.max_tool_rounds = Some(50);
        let settings = sanitize_settings(settings);
        assert_eq!(settings.chat_tools.max_tool_rounds, Some(50));
    }

    #[test]
    fn greenlight_migration_enables_tools_and_flips_legacy_policy() {
        // Simulate a pre-green-light install: flag unset, native tools off, old policy.
        let mut settings = Settings::default();
        settings.chat_tools_greenlit_v1 = false;
        settings.chat_tools.native_tools = ChatNativeToolsConfig {
            skill_runtime: true,
            ..Default::default()
        };
        settings.chat_tools.native_tools.read_file = false;
        settings.chat_tools.native_tools.write_file = false;
        settings.chat_tools.native_tools.run_command = false;
        settings.chat_tools.approval_policy = LEGACY_DEFAULT_APPROVAL_POLICY.to_string();

        let settings = sanitize_settings(settings);

        assert!(settings.chat_tools_greenlit_v1);
        assert!(settings.chat_tools.native_tools.read_file);
        assert!(settings.chat_tools.native_tools.write_file);
        assert!(settings.chat_tools.native_tools.run_command);
        assert_eq!(settings.chat_tools.approval_policy, "auto");
    }

    #[test]
    fn greenlight_migration_is_idempotent_and_keeps_explicit_choices() {
        // Already migrated: a user-disabled tool and an explicit policy must survive.
        let mut settings = Settings::default();
        settings.chat_tools_greenlit_v1 = true;
        settings.chat_tools.native_tools.run_command = false;
        settings.chat_tools.approval_policy = "always_confirm".to_string();

        let settings = sanitize_settings(settings);

        assert!(!settings.chat_tools.native_tools.run_command);
        assert_eq!(settings.chat_tools.approval_policy, "always_confirm");
    }

    #[test]
    fn sanitize_settings_migrates_prompt_cache_retention() {
        let mut settings = Settings::default();
        settings.providers = vec![ModelProvider {
            id: "p1".into(),
            name: "P".into(),
            api_keys: vec!["k".into()],
            api_key_legacy: None,
            base_url: "https://api.openai.com/v1".into(),
            available_models: vec![],
            enabled_models: vec![],
            enabled: true,
            api_format: "openai_chat".into(),
            model_overrides: Default::default(),
            compress_request_body: false,
            request: ProviderRequestConfig {
                prompt_caching: Some(false),
                prompt_cache_retention: "garbage".into(),
                ..Default::default()
            },
        }];
        // 非法 retention + bool false → none
        let s = sanitize_settings(settings.clone());
        assert_eq!(s.providers[0].request.prompt_cache_retention, "none");
        assert!(s.providers[0].request.prompt_caching.is_none());

        // 合法 retention 优先：false + long → 保留 long
        settings.providers[0].request.prompt_caching = Some(false);
        settings.providers[0].request.prompt_cache_retention = "long".into();
        let s = sanitize_settings(settings.clone());
        assert_eq!(s.providers[0].request.prompt_cache_retention, "long");

        // true + 非法 → short
        settings.providers[0].request.prompt_caching = Some(true);
        settings.providers[0].request.prompt_cache_retention = "???".into();
        let s = sanitize_settings(settings.clone());
        assert_eq!(s.providers[0].request.prompt_cache_retention, "short");

        // 无 bool、非法 → short
        settings.providers[0].request.prompt_caching = None;
        settings.providers[0].request.prompt_cache_retention = "".into();
        let s = sanitize_settings(settings);
        assert_eq!(s.providers[0].request.prompt_cache_retention, "short");
    }

    #[test]
    fn hooks_default_to_empty_and_survive_legacy_settings() {
        // 纯新增字段：旧 settings.json 缺 hooks → 空数组，行为与现状一致。
        let cfg: ChatToolsConfig =
            serde_json::from_str("{}").expect("ChatToolsConfig defaults from empty object");
        assert!(cfg.hooks.is_empty());
    }

    #[test]
    fn hook_def_wire_shape_matches_the_frontend_type() {
        // 前端 `HookDef`（src/api/tauri.ts）逐字段镜像这个结构。字段名/大小写漂移了，
        // 保存时会静默丢字段（serde default 兜住，用户只看到「配了但没生效」）。
        let hook = HookDef {
            id: "h1".to_string(),
            name: "lint-guard".to_string(),
            description: "d".to_string(),
            event: "tool_execution_start".to_string(),
            enabled: true,
            kind: "http".to_string(),
            script: String::new(),
            url: "https://example.test/h".to_string(),
            method: "POST".to_string(),
            headers: [("X-A".to_string(), "1".to_string())].into_iter().collect(),
            timeout_ms: 60_000,
        };
        let value = serde_json::to_value(&hook).expect("serialize");
        let keys: Vec<&str> = value
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            vec![
                "description",
                "enabled",
                "event",
                "headers",
                "id",
                "method",
                "name",
                "script",
                "timeoutMs",
                "type",
                "url",
            ],
            "wire field names drifted from the TS HookDef"
        );

        // 前端写回的 JSON 也必须原样读回来。
        let round_tripped: HookDef = serde_json::from_value(value).expect("deserialize");
        assert_eq!(round_tripped.kind, "http");
        assert_eq!(round_tripped.timeout_ms, 60_000);
        assert_eq!(
            round_tripped.headers.get("X-A").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn sanitize_hooks_drops_invalid_entries_and_clamps_timeout() {
        let mut settings = Settings::default();
        settings.chat_tools.hooks = vec![
            // 合法 command：补 id、钳制超时下限。
            HookDef {
                name: "log".to_string(),
                event: " agent_end ".to_string(),
                enabled: true,
                kind: "Command".to_string(),
                script: "echo hi".to_string(),
                timeout_ms: 1,
                ..Default::default()
            },
            // 合法 http：方法归一到大写。
            HookDef {
                id: "http-1".to_string(),
                event: "turn_start".to_string(),
                kind: "http".to_string(),
                url: "https://example.test/hook".to_string(),
                method: "post".to_string(),
                timeout_ms: 10_000_000,
                ..Default::default()
            },
            // 事件名非法 → 丢弃（没有合理的「就近」事件可猜）。
            HookDef {
                event: "agent_started".to_string(),
                kind: "command".to_string(),
                script: "echo x".to_string(),
                ..Default::default()
            },
            // 类型非法 → 丢弃。
            HookDef {
                event: "agent_end".to_string(),
                kind: "webhook".to_string(),
                ..Default::default()
            },
            // command 但脚本为空 / http 但 url 为空 → 丢弃。
            HookDef {
                event: "agent_end".to_string(),
                kind: "command".to_string(),
                script: "   ".to_string(),
                ..Default::default()
            },
            HookDef {
                event: "agent_end".to_string(),
                kind: "http".to_string(),
                ..Default::default()
            },
        ];

        let settings = sanitize_settings(settings);
        let hooks = &settings.chat_tools.hooks;

        assert_eq!(hooks.len(), 2, "only the two valid hooks survive");
        assert_eq!(hooks[0].event, "agent_end", "event is trimmed");
        assert_eq!(hooks[0].kind, "command", "kind is lowercased");
        assert!(!hooks[0].id.is_empty(), "blank id is filled in");
        assert_eq!(hooks[0].timeout_ms, HOOK_MIN_TIMEOUT_MS);
        assert_eq!(hooks[1].method, "POST", "method is uppercased");
        assert_eq!(hooks[1].timeout_ms, HOOK_MAX_TIMEOUT_MS);
    }

    #[test]
    fn greenlight_migration_does_not_stomp_explicit_policy_on_first_run() {
        // Pre-green-light flag, but the user had explicitly chosen always_confirm.
        let mut settings = Settings::default();
        settings.chat_tools_greenlit_v1 = false;
        settings.chat_tools.approval_policy = "always_confirm".to_string();

        let settings = sanitize_settings(settings);

        // Tools still get enabled, but the explicit policy is preserved.
        assert!(settings.chat_tools_greenlit_v1);
        assert!(settings.chat_tools.native_tools.read_file);
        assert_eq!(settings.chat_tools.approval_policy, "always_confirm");
    }

    #[test]
    fn sanitize_settings_normalizes_and_clamps_tool_output_chars() {
        // None（旧的"不截断"）→ 归一到默认截断值，绝不再保留 None（上下文撑爆根因）。
        let mut settings = Settings::default();
        settings.chat_tools.max_tool_output_chars = None;
        let settings = sanitize_settings(settings);
        assert_eq!(
            settings.chat_tools.max_tool_output_chars,
            Some(DEFAULT_MAX_TOOL_OUTPUT_CHARS)
        );

        // 过小钳到下限。
        let mut settings = Settings::default();
        settings.chat_tools.max_tool_output_chars = Some(1);
        let settings = sanitize_settings(settings);
        assert_eq!(
            settings.chat_tools.max_tool_output_chars,
            Some(CHAT_TOOL_MIN_OUTPUT_CHARS)
        );

        // 过大钳到上限。
        let mut settings = Settings::default();
        settings.chat_tools.max_tool_output_chars = Some(usize::MAX);
        let settings = sanitize_settings(settings);
        assert_eq!(
            settings.chat_tools.max_tool_output_chars,
            Some(CHAT_TOOL_MAX_OUTPUT_CHARS)
        );
    }

    #[test]
    fn sanitize_settings_clamps_mcp_idle_timeout_and_keeps_default() {
        // 默认值保持不变（在范围内）。
        assert_eq!(ChatToolsConfig::default().mcp_idle_timeout_ms, 600_000);

        // 太小钳到下限 60s。
        let mut settings = Settings::default();
        settings.chat_tools.mcp_idle_timeout_ms = 1_000;
        let settings = sanitize_settings(settings);
        assert_eq!(
            settings.chat_tools.mcp_idle_timeout_ms,
            MCP_IDLE_TIMEOUT_MIN_MS
        );

        // 太大钳到上限 24h。
        let mut settings = Settings::default();
        settings.chat_tools.mcp_idle_timeout_ms = u64::MAX;
        let settings = sanitize_settings(settings);
        assert_eq!(
            settings.chat_tools.mcp_idle_timeout_ms,
            MCP_IDLE_TIMEOUT_MAX_MS
        );

        // 缺省（旧 settings.json 无此字段）走 serde default 600000。
        let cfg: ChatToolsConfig =
            serde_json::from_str("{}").expect("ChatToolsConfig defaults from empty object");
        assert_eq!(cfg.mcp_idle_timeout_ms, 600_000);
    }

    #[test]
    fn sanitize_settings_keeps_empty_models_for_unfetched_provider() {
        let mut s = Settings::default();
        s.providers.push(ModelProvider {
            id: "p".to_string(),
            name: "P".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec![],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.translator_provider_id = "p".to_string();
        s.screenshot_translation.provider_id = "p".to_string();

        let s = sanitize_settings(s);
        let p = s.get_provider("p").unwrap();
        assert!(p.available_models.is_empty());
        assert!(p.enabled_models.is_empty());
        assert!(s.translator_model.is_empty());
        assert!(s.screenshot_translation.model.is_empty());
    }

    #[test]
    fn sanitize_settings_defaults_chat_to_lens_then_translator() {
        let mut s = Settings::default();
        s.providers.push(ModelProvider {
            id: "translator".to_string(),
            name: "Translator".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["gpt-4o".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.providers.push(ModelProvider {
            id: "lens".to_string(),
            name: "Lens".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["vision-model".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.translator_provider_id = "translator".to_string();
        s.translator_model = "gpt-4o".to_string();
        s.lens.provider_id = "lens".to_string();
        s.lens.model = "vision-model".to_string();

        let s = sanitize_settings(s);
        assert_eq!(s.chat_provider_id, "lens");
        assert_eq!(s.chat_model, "vision-model");
        assert!(
            s.default_models.chat.provider_id.is_empty(),
            "Lens fallback should not become an explicit Chat default slot"
        );
    }

    #[test]
    fn unset_auxiliary_models_inherit_effective_chat_model() {
        let mut s = Settings::default();
        s.providers.push(ModelProvider {
            id: "translator".to_string(),
            name: "Translator".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["gpt-4o".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.providers.push(ModelProvider {
            id: "lens".to_string(),
            name: "Lens".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["vision-model".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.translator_provider_id = "translator".to_string();
        s.translator_model = "gpt-4o".to_string();
        s.lens.provider_id = "lens".to_string();
        s.lens.model = "vision-model".to_string();

        let s = sanitize_settings(s);

        assert_eq!(
            s.effective_chat_model(),
            ("lens".to_string(), "vision-model".to_string())
        );
        assert!(!s.has_explicit_vision_model());
        assert_eq!(s.effective_vision_model(), s.effective_chat_model());
        assert_eq!(
            s.effective_title_summary_model_for_session(None),
            s.effective_chat_model()
        );
        assert_eq!(
            s.effective_compression_model_for_session(None),
            s.effective_chat_model()
        );
        assert!(s.image_generation_model().is_none());
        assert!(s.default_models.vision.provider_id.is_empty());
        assert!(s.default_models.title_summary.provider_id.is_empty());
        assert!(s.default_models.compression.provider_id.is_empty());
        assert!(s.default_models.image_generation.provider_id.is_empty());
    }

    #[test]
    fn effective_side_models_auto_prefer_session_over_global_chat_default() {
        let mut settings = Settings::default();
        settings.providers.push(ModelProvider {
            id: "global".to_string(),
            name: "Global".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["gemini-3.1-flash-lite".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        settings.providers.push(ModelProvider {
            id: "session".to_string(),
            name: "Session".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["gpt-4.1".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        settings.default_models.chat.provider_id = "global".to_string();
        settings.default_models.chat.model = "gemini-3.1-flash-lite".to_string();

        let session = SessionModel {
            provider_id: "session",
            model: "gpt-4.1",
        };

        assert_eq!(
            settings.effective_title_summary_model_for_session(Some(session)),
            ("session".to_string(), "gpt-4.1".to_string())
        );
        assert_eq!(
            settings.effective_compression_model_for_session(Some(session)),
            ("session".to_string(), "gpt-4.1".to_string())
        );
        assert_eq!(
            settings.effective_vision_model_for_session(Some(session)),
            ("session".to_string(), "gpt-4.1".to_string())
        );
    }

    #[test]
    fn sanitize_settings_keeps_valid_chat_model() {
        let mut s = Settings::default();
        s.providers.push(ModelProvider {
            id: "chat".to_string(),
            name: "Chat".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["m1".to_string(), "m2".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.chat_provider_id = "chat".to_string();
        s.chat_model = "m2".to_string();

        let s = sanitize_settings(s);
        assert_eq!(s.chat_provider_id, "chat");
        assert_eq!(s.chat_model, "m2");
        assert_eq!(s.default_models.chat.provider_id, "chat");
        assert_eq!(s.default_models.chat.model, "m2");
    }

    #[test]
    fn explicit_default_model_slots_are_independent() {
        let mut s = Settings::default();
        s.providers.push(ModelProvider {
            id: "chat".to_string(),
            name: "Chat".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["chat-model".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.providers.push(ModelProvider {
            id: "vision".to_string(),
            name: "Vision".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["vision-model".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.providers.push(ModelProvider {
            id: "title".to_string(),
            name: "Title".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["title-model".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.providers.push(ModelProvider {
            id: "compression".to_string(),
            name: "Compression".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["compression-model".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.providers.push(ModelProvider {
            id: "image".to_string(),
            name: "Image".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["image-model".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.translator_provider_id = "chat".to_string();
        s.translator_model = "chat-model".to_string();
        s.default_models.chat.provider_id = "chat".to_string();
        s.default_models.chat.model = "chat-model".to_string();
        s.default_models.vision.provider_id = "vision".to_string();
        s.default_models.vision.model = "vision-model".to_string();
        s.default_models.title_summary.provider_id = "title".to_string();
        s.default_models.title_summary.model = "title-model".to_string();
        s.default_models.compression.provider_id = "compression".to_string();
        s.default_models.compression.model = "compression-model".to_string();
        s.default_models.image_generation.provider_id = "image".to_string();
        s.default_models.image_generation.model = "image-model".to_string();

        let s = sanitize_settings(s);

        assert_eq!(
            s.effective_chat_model(),
            ("chat".to_string(), "chat-model".to_string())
        );
        assert_eq!(
            s.effective_title_summary_model_for_session(None),
            ("title".to_string(), "title-model".to_string())
        );
        assert!(s.has_explicit_vision_model());
        assert_eq!(
            s.effective_vision_model(),
            ("vision".to_string(), "vision-model".to_string())
        );
        assert_eq!(
            s.effective_compression_model_for_session(None),
            ("compression".to_string(), "compression-model".to_string())
        );
        assert_eq!(
            s.image_generation_model(),
            Some(("image".to_string(), "image-model".to_string()))
        );
    }

    #[test]
    fn sanitize_settings_repairs_invalid_default_model_slots() {
        let mut s = Settings::default();
        s.providers.push(ModelProvider {
            id: "chat".to_string(),
            name: "Chat".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["m1".to_string(), "m2".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.translator_provider_id = "chat".to_string();
        s.translator_model = "m1".to_string();
        s.default_models.chat.provider_id = "chat".to_string();
        s.default_models.chat.model = "removed".to_string();
        s.default_models.vision.provider_id = "chat".to_string();
        s.default_models.vision.model = String::new();
        s.default_models.title_summary.provider_id = "deleted-provider".to_string();
        s.default_models.title_summary.model = "ghost".to_string();
        s.default_models.compression.provider_id = "chat".to_string();
        s.default_models.compression.model = String::new();
        s.default_models.image_generation.provider_id = "chat".to_string();
        s.default_models.image_generation.model = String::new();

        let s = sanitize_settings(s);

        assert_eq!(s.default_models.chat.provider_id, "chat");
        assert_eq!(s.default_models.chat.model, "m1");
        assert_eq!(s.default_models.vision.provider_id, "chat");
        assert_eq!(s.default_models.vision.model, "m1");
        assert!(s.default_models.title_summary.provider_id.is_empty());
        assert!(s.default_models.title_summary.model.is_empty());
        assert_eq!(s.default_models.compression.provider_id, "chat");
        assert_eq!(s.default_models.compression.model, "m1");
        assert_eq!(s.default_models.image_generation.provider_id, "chat");
        assert_eq!(s.default_models.image_generation.model, "m1");
        assert_eq!(s.chat_provider_id, "chat");
        assert_eq!(s.chat_model, "m1");
    }

    #[test]
    fn persistence_mirror_keeps_unset_chat_default_unset() {
        let mut s = Settings::default();
        s.providers.push(ModelProvider {
            id: "translator".to_string(),
            name: "Translator".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["gpt-4o".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.providers.push(ModelProvider {
            id: "lens".to_string(),
            name: "Lens".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["vision-model".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.translator_provider_id = "translator".to_string();
        s.translator_model = "gpt-4o".to_string();
        s.lens.provider_id = "lens".to_string();
        s.lens.model = "vision-model".to_string();

        let mut s = sanitize_settings(s);
        assert_eq!(s.chat_provider_id, "lens");
        assert_eq!(s.chat_model, "vision-model");
        assert!(s.default_models.chat.provider_id.is_empty());

        mirror_explicit_chat_default_for_persistence(&mut s);

        assert!(s.chat_provider_id.is_empty());
        assert!(s.chat_model.is_empty());
        assert!(s.default_models.chat.provider_id.is_empty());
    }

    #[test]
    fn default_models_serialize_as_structured_camel_case_settings() {
        let mut s = Settings::default();
        s.default_models.vision.provider_id = "vision-provider".to_string();
        s.default_models.vision.model = "vision-model".to_string();
        s.default_models.title_summary.provider_id = "title-provider".to_string();
        s.default_models.title_summary.model = "title-model".to_string();
        s.default_models.image_generation.provider_id = "image-provider".to_string();
        s.default_models.image_generation.model = "image-model".to_string();
        let value = serde_json::to_value(&s).expect("settings should serialize");

        assert_eq!(
            value["defaultModels"]["vision"]["providerId"],
            "vision-provider"
        );
        assert_eq!(value["defaultModels"]["vision"]["model"], "vision-model");
        assert_eq!(
            value["defaultModels"]["titleSummary"]["providerId"],
            "title-provider"
        );
        assert_eq!(
            value["defaultModels"]["titleSummary"]["model"],
            "title-model"
        );
        assert_eq!(
            value["defaultModels"]["imageGeneration"]["providerId"],
            "image-provider"
        );
        assert_eq!(
            value["defaultModels"]["imageGeneration"]["model"],
            "image-model"
        );
        assert!(value["defaultModels"]["chat"]["providerId"]
            .as_str()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn sanitize_settings_preserves_streamable_http_mcp_server() {
        let mut s = Settings::default();
        let mut headers = std::collections::HashMap::new();
        headers.insert(" Authorization ".to_string(), " Bearer token ".to_string());
        s.chat_tools.servers.push(ChatMcpServer {
            id: " http-server ".to_string(),
            name: " Remote ".to_string(),
            enabled: true,
            transport: "sse".to_string(),
            url: " https://example.com/mcp ".to_string(),
            command: " ignored ".to_string(),
            args: vec![" ".to_string(), "--unused".to_string()],
            env: std::collections::HashMap::new(),
            headers,
            cwd: None,
            enabled_tools: vec![" fetch ".to_string(), "".to_string()],
            connector_id: None,
            auth: None,
        });

        let s = sanitize_settings(s);
        let server = &s.chat_tools.servers[0];
        assert_eq!(server.id, "http-server");
        assert_eq!(server.name, "Remote");
        assert_eq!(server.transport, "streamable_http");
        assert_eq!(server.url, "https://example.com/mcp");
        assert_eq!(
            server.headers.get("Authorization").map(String::as_str),
            Some("Bearer token"),
        );
        assert_eq!(server.enabled_tools, vec!["fetch".to_string()]);
    }

    #[test]
    fn sanitize_settings_resets_unknown_mcp_transport_to_stdio() {
        let mut s = Settings::default();
        s.chat_tools.servers.push(ChatMcpServer {
            id: "mcp-1".to_string(),
            name: "Local".to_string(),
            enabled: false,
            transport: "websocket".to_string(),
            url: String::new(),
            command: " npx ".to_string(),
            args: Vec::new(),
            env: std::collections::HashMap::new(),
            headers: std::collections::HashMap::new(),
            cwd: None,
            enabled_tools: Vec::new(),
            connector_id: None,
            auth: None,
        });

        let s = sanitize_settings(s);
        let server = &s.chat_tools.servers[0];
        assert_eq!(server.transport, "stdio");
        assert_eq!(server.command, "npx");
    }

    #[test]
    fn sanitize_settings_clamps_unknown_message_order() {
        let mut s = Settings::default();
        s.lens.message_order = "garbage".to_string();
        let s = sanitize_settings(s);
        assert_eq!(s.lens.message_order, "asc");
    }

    #[test]
    fn lens_capture_hint_defaults_to_enabled() {
        let s = Settings::default();
        assert!(s.lens.show_capture_hint);

        let cfg: LensConfig = serde_json::from_str("{}").expect("empty lens config should load");
        assert!(cfg.show_capture_hint);
    }

    #[test]
    fn lens_send_to_chat_defaults_to_enabled() {
        let s = Settings::default();
        assert!(s.lens.send_to_chat);

        let cfg: LensConfig = serde_json::from_str("{}").expect("empty lens config should load");
        assert!(cfg.send_to_chat);
    }

    #[test]
    fn sanitize_settings_resets_lens_provider_when_pointing_to_nonexistent() {
        let mut s = Settings::default();
        s.providers.push(ModelProvider {
            id: "real".to_string(),
            name: "Real".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["m".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.lens.provider_id = "nonexistent".to_string();
        s.lens.model = "ghost-model".to_string();
        let s = sanitize_settings(s);
        assert_eq!(s.lens.provider_id, "");
        assert_eq!(s.lens.model, "");
    }

    #[test]
    fn sanitize_settings_marks_onboarding_completed_for_existing_provider_config() {
        let mut s = Settings::default();
        s.onboarding_status.clear();
        s.providers.push(ModelProvider {
            id: "active".to_string(),
            name: "Active".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://active.example/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["live-model".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        let s = sanitize_settings(s);
        assert_eq!(s.onboarding_status, "completed");
    }

    #[test]
    fn sanitize_settings_keeps_pending_onboarding_for_fresh_install() {
        let s = sanitize_settings(Settings::default());
        assert_eq!(s.onboarding_status, "pending");
    }

    #[test]
    fn sanitize_settings_keeps_explicit_pending_for_restart_onboarding() {
        let mut s = Settings::default();
        s.onboarding_status = "pending".to_string();
        s.providers.push(ModelProvider {
            id: "active".to_string(),
            name: "Active".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://active.example/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["live-model".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        let s = sanitize_settings(s);
        assert_eq!(s.onboarding_status, "pending");
    }

    #[test]
    fn sanitize_settings_reassigns_disabled_provider_selections() {
        let mut s = Settings::default();
        s.providers.push(ModelProvider {
            id: "disabled".to_string(),
            name: "Disabled".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://disabled.example/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["off-model".to_string()],
            api_format: "openai".to_string(),
            enabled: false,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.providers.push(ModelProvider {
            id: "active".to_string(),
            name: "Active".to_string(),
            api_keys: vec!["sk".to_string()],
            api_key_legacy: None,
            base_url: "https://active.example/v1".to_string(),
            available_models: vec![],
            enabled_models: vec!["live-model".to_string()],
            api_format: "openai".to_string(),
            enabled: true,
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        });
        s.translator_provider_id = "disabled".to_string();
        s.translator_model = "off-model".to_string();
        s.screenshot_translation.provider_id = "disabled".to_string();
        s.screenshot_translation.model = "off-model".to_string();
        s.lens.provider_id = "disabled".to_string();
        s.lens.model = "off-model".to_string();
        s.default_models.chat.provider_id = "disabled".to_string();
        s.default_models.chat.model = "off-model".to_string();

        let s = sanitize_settings(s);

        assert_eq!(s.translator_provider_id, "active");
        assert_eq!(s.translator_model, "live-model");
        assert_eq!(s.screenshot_translation.provider_id, "active");
        assert_eq!(s.screenshot_translation.model, "live-model");
        assert_eq!(s.lens.provider_id, "");
        assert_eq!(s.lens.model, "");
        assert_eq!(s.default_models.chat.provider_id, "");
        assert_eq!(s.default_models.chat.model, "");
    }

    #[test]
    fn chat_current_datetime_context_uses_local_clock() {
        let now = Local::now();
        let zh = chat_current_datetime_context("zh");
        assert!(zh.contains("系统时钟"));
        assert!(zh.contains(&format!("{}年", now.year())));
        let en = chat_current_datetime_context("en");
        assert!(en.contains("system clock"));
        assert!(en.contains(&format!("{}-", now.year())));
    }

    #[test]
    fn chat_current_datetime_context_is_date_only_prefix_stable() {
        // 前缀稳定性：不含时分（HH:MM 会让同一对话每轮系统提示词都变，打穿 prompt cache）。
        // 同一天内多次调用必须逐字节一致。
        let has_hh_mm = |s: &str| {
            s.as_bytes().windows(5).any(|w| {
                w[0].is_ascii_digit()
                    && w[1].is_ascii_digit()
                    && w[2] == b':'
                    && w[3].is_ascii_digit()
                    && w[4].is_ascii_digit()
            })
        };
        for lang in ["zh", "en"] {
            let a = chat_current_datetime_context(lang);
            let b = chat_current_datetime_context(lang);
            assert_eq!(a, b, "same-day calls must be byte-identical ({lang})");
            assert!(
                !has_hh_mm(&a),
                "no HH:MM clock in the prompt prefix ({lang}): {a}"
            );
        }
    }

    #[test]
    fn skill_globally_available_hides_obsidian_without_vault() {
        let chat_tools = ChatToolsConfig::default();
        for id in OBSIDIAN_CONNECTOR_SKILL_IDS {
            // No vault configured → each Obsidian skill is unavailable.
            assert!(
                !skill_globally_available(&chat_tools, id, false),
                "{id} should be hidden without a vault"
            );
            assert!(!skill_connector_satisfied(id, false));
            assert!(
                skill_globally_available(&chat_tools, id, true),
                "{id} should be available with a vault"
            );
            assert!(skill_connector_satisfied(id, true));
        }
        // Non-connector skills are unaffected by vault state.
        assert!(skill_globally_available(&chat_tools, "pdf", false));
    }

    #[test]
    fn skill_global_unavailable_error_distinguishes_disabled_and_connector() {
        let mut chat_tools = ChatToolsConfig::default();
        chat_tools.disabled_skill_ids = vec!["obsidian-markdown".to_string()];
        assert_eq!(
            skill_global_unavailable_error(
                &chat_tools,
                "obsidian-markdown",
                true,
                "obsidian-markdown",
            )
            .as_deref(),
            Some("Skill is disabled in Settings: obsidian-markdown")
        );

        chat_tools.disabled_skill_ids.clear();
        assert_eq!(
            skill_global_unavailable_error(
                &chat_tools,
                "obsidian-markdown",
                false,
                "obsidian-markdown"
            )
            .as_deref(),
            Some("Skill requires a configured Obsidian connector: obsidian-markdown")
        );
        assert_eq!(
            skill_global_unavailable_error(
                &chat_tools,
                "obsidian-markdown",
                true,
                "obsidian-markdown"
            ),
            None
        );
    }

    #[test]
    fn sanitize_native_tools_migrates_first_legacy_workspace_root() {
        let mut settings = Settings::default();
        settings.chat_tools.native_tools.working_directory.clear();
        settings.chat_tools.native_tools.workspace_roots = vec![
            "  C:/legacy/workspace  ".to_string(),
            "C:/ignored".to_string(),
        ];
        let settings = sanitize_settings(settings);
        assert_eq!(
            settings.chat_tools.native_tools.working_directory,
            "C:/legacy/workspace"
        );
        assert!(settings.chat_tools.native_tools.workspace_roots.is_empty());
    }

    #[test]
    fn sanitize_native_tools_uses_platform_default_when_directory_is_missing() {
        let mut settings = Settings::default();
        settings.chat_tools.native_tools.working_directory.clear();
        settings.chat_tools.native_tools.workspace_roots.clear();
        let settings = sanitize_settings(settings);
        assert_eq!(
            settings.chat_tools.native_tools.working_directory,
            default_chat_working_directory()
        );
    }
}

#[cfg(test)]
mod hooks_disk_compat_tests {
    use super::*;

    /// 回归：hooks 字段刚上线时前端漏传，`invoke` 把它序列化成 null 写进了
    /// settings.json。serde 的 `default` 只兜「键不存在」，遇到 null 会报
    /// `invalid type: null, expected a sequence` —— 整个 chatTools 解析失败，
    /// 连 MCP 服务器和原生工具开关一起丢。
    #[test]
    fn null_hooks_on_disk_does_not_break_chat_tools() {
        let json = serde_json::json!({
            "enabled": true,
            "hooks": null,
            "servers": [{ "id": "s1", "name": "srv" }],
        });
        let config: ChatToolsConfig =
            serde_json::from_value(json).expect("null hooks must not break chatTools");
        assert!(config.hooks.is_empty());
        assert!(config.enabled, "sibling fields must survive");
        assert_eq!(config.servers.len(), 1, "sibling fields must survive");
    }

    /// 键完全不存在（更旧的磁盘文件）同样要能读。
    #[test]
    fn missing_hooks_key_defaults_to_empty() {
        let config: ChatToolsConfig =
            serde_json::from_value(serde_json::json!({ "enabled": true }))
                .expect("missing hooks key must parse");
        assert!(config.hooks.is_empty());
    }
}
