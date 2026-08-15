use std::{future::Future, pin::Pin};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::mcp::{self, types::ChatToolArtifact, ChatToolDefinition};

/// 外置图片读不回来时的替身文本。落盘的 `MessagePart::Image` 只有 `path`，回放前
/// 必须经 `attachments::rehydrate_model_message_images` 填回 `data`；文件被用户删掉
/// （或历史遗留的坏引用）就走这里，而不是发一个空 base64 让 provider 400。
pub const MISSING_IMAGE_PLACEHOLDER: &str = "[图片已不可用（附件文件缺失）]";

/// Last paragraph of the chat system prompt when a per-conversation workbench
/// is in play. Must stay last so `split_workbench_system_suffix` can peel it
/// off the stable prefix (cross-conversation prompt cache).
pub const WORKBENCH_LOCATION_PROMPT_HEAD: &str = "Current default workbench:";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    User,
    Assistant,
    Tool,
}

impl ModelRole {
    pub fn as_str(self) -> &'static str {
        match self {
            ModelRole::User => "user",
            ModelRole::Assistant => "assistant",
            ModelRole::Tool => "tool",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessagePart {
    Text {
        text: String,
    },
    /// 用户/工具上行的一张图。
    ///
    /// **落盘时 `data` 必须为空、`path` 必须有值**：图片字节存进
    /// `<会话id>_attachments/`，JSON 里只留文件名（见
    /// `attachments::externalize_model_message_images`）。运行期反过来——回放时
    /// `attachments::rehydrate_model_message_images` 按 `path` 读盘填回 `data`，
    /// 各 provider 适配器只认 `data`，感知不到这层。
    ///
    /// 一张 1.8MB 的截图 base64 后 2.5MB；被 `read` 看三次就是三份，实测把会话
    /// 文件撑到 7.59MB（99.8% 是同一张图的副本），而每轮工具执行完的快照都要把
    /// 整本读出来、clone、序列化、fsync 一遍。所以这里绝不能存 base64。
    Image {
        mime_type: String,
        /// base64 编码的图片字节。**运行期形态**，落盘前会被外置清空。
        #[serde(default, skip_serializing_if = "String::is_empty")]
        data: String,
        /// 外置后的附件文件名（相对于会话附件目录）。**落盘形态**。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    ImageUrl {
        url: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
        arguments_raw: String,
        /// Provider-specific opaque signature that must be echoed back when this
        /// tool call is replayed (Gemini 3.x `thoughtSignature`: required on
        /// `functionCall` parts in follow-up requests, else 400). Other providers
        /// leave this `None` and ignore it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    ToolResult {
        tool_call_id: String,
        content: String,
        is_error: bool,
        artifacts: Vec<ChatToolArtifact>,
    },
    Reasoning {
        text: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMessage {
    pub role: ModelRole,
    pub content: Vec<MessagePart>,
}

impl ModelMessage {
    pub fn text(role: ModelRole, text: impl Into<String>) -> Self {
        Self {
            role,
            content: vec![MessagePart::Text { text: text.into() }],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelTool {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: String,
    pub server_id: Option<String>,
    pub server_name: Option<String>,
    pub input_schema: Value,
    pub sensitive: bool,
}

impl ModelTool {
    pub fn openai_tool_name(&self) -> String {
        match self.source.as_str() {
            "native" | "skill" | "mixer" => mcp::types::apply_reserved_wire_alias(
                &mcp::types::sanitize_openai_tool_name(&self.name),
            ),
            _ => mcp::types::sanitize_openai_tool_name(&self.id),
        }
    }

    pub fn to_openai_tool(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.openai_tool_name(),
                "description": self.description,
                "parameters": self.input_schema,
            }
        })
    }

    /// OpenAI **Responses** API tool shape: flat `name`/`description`/`parameters`,
    /// not nested under a `function` object (unlike Chat Completions' `to_openai_tool`).
    pub fn to_openai_responses_tool(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "name": self.openai_tool_name(),
            "description": self.description,
            "parameters": self.input_schema,
        })
    }
}

impl From<&ChatToolDefinition> for ModelTool {
    fn from(tool: &ChatToolDefinition) -> Self {
        Self {
            id: tool.id.clone(),
            name: tool.name.clone(),
            description: tool.description.clone(),
            source: tool.source.clone(),
            server_id: tool.server_id.clone(),
            server_name: tool.server_name.clone(),
            input_schema: tool.input_schema.clone(),
            sensitive: tool.sensitive,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateOptions {
    /// 显式的单次请求覆盖；None 时由 provider/model 元数据解析，仍缺省则不发送。
    pub temperature: Option<f64>,
    pub max_tokens: u32,
    /// 思考开关。UI「Off」时为 false：适配器**必须显式**下发关闭信号
    /// （OpenAI Chat → `reasoning_effort:"none"`；DeepSeek/Kimi → `thinking.type=disabled`；
    /// Responses → `reasoning.effort:"none"`），不能靠省略字段——多家默认 effort=high。
    pub thinking_enabled: bool,
    /// 每对话「思考等级」(`"low"|"medium"|"high"|…`)。`None` = 未设档：
    /// - `thinking_enabled=false` 时必为 None（`resolve_thinking` 保证），走关闭分支；
    /// - `thinking_enabled=true` 时 None = 模型无 effort 旋钮，不发等级字段。
    /// 选了等级时由适配器按家族映射：OpenAI→`reasoning_effort`，Anthropic→`output_config.effort`，
    /// Responses→`reasoning.effort`，Gemini→`thinkingLevel`。
    #[serde(default)]
    pub thinking_level: Option<String>,
    /// 是否请求模型的**原生内置联网搜索**（任务 07-23）。仅在会话为 Builtin 模式且当前
    /// provider 支持（`builtin_web_search_supported`）时置 true；各适配器据此往请求体
    /// 追加各家原生搜索工具（OpenAI Responses `web_search` / Gemini `google_search` /
    /// Anthropic `web_search_20250305`）。默认 false，不支持的适配器忽略。
    #[serde(default)]
    pub builtin_web_search: bool,
    #[serde(default)]
    pub provider_options: Value,
}

impl Default for GenerateOptions {
    fn default() -> Self {
        Self {
            temperature: None,
            max_tokens: 8192,
            thinking_enabled: true,
            thinking_level: None,
            builtin_web_search: false,
            provider_options: Value::Object(Default::default()),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequestMetadata {
    pub label: String,
    #[serde(default)]
    pub usage_source: Option<String>,
    #[serde(default)]
    pub usage_operation: Option<String>,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub message_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct GenerateRequestContext {
    pub conversation_id: Option<String>,
    pub message_id: Option<String>,
}

impl GenerateRequestContext {
    pub fn new(conversation_id: Option<&str>, message_id: Option<&str>) -> Self {
        Self {
            conversation_id: conversation_id.map(str::to_string),
            message_id: message_id.map(str::to_string),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateRequest {
    pub model: String,
    pub system: String,
    pub messages: Vec<ModelMessage>,
    pub tools: Vec<ModelTool>,
    pub options: GenerateOptions,
    pub metadata: RequestMetadata,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    #[serde(default)]
    pub cached_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
    /// CLI 自报的**上下文窗口总大小**（不是用量）。来源：ACP `usage_update.size`
    /// （官方 RFD「Session Usage and Context Status」，字段平铺在 `update` 下）。
    ///
    /// 只有外部 CLI 会话填；内置 provider 恒 `None`——内置路径的窗口来自
    /// `model_metadata::context_window_for_model`，与本字段无关。放在 `ModelUsage` 里
    /// 是因为窗口与用量由 CLI 在同一次上报中给出，随消息一起持久化后
    /// `collect_external_session_usage` 能一并读到，无需为分母另开事件通道。
    #[serde(default)]
    pub context_window_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingToolCall {
    pub id: String,
    pub function_name: String,
    pub arguments: Value,
    pub arguments_raw: String,
    pub arguments_parse_error: Option<String>,
    /// Provider-specific opaque signature echoed back on replay (Gemini 3.x
    /// `thoughtSignature`; other providers leave `None`). Rides through the
    /// stream accumulator + provider_messages so tool-call turns replay intact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// 单条内置搜索引用（服务端托管的原生联网搜索返回的来源）。仅取 {title,url}，
/// 前端渲染成可点来源脚注（任务 07-23，MVP 只做来源列表、不做正文 `[n]` 锚定）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebCitation {
    pub title: String,
    pub url: String,
}

/// 模型**原生内置联网搜索**的解析产物：模型执行的搜索词 + 来源引用。
/// 内置搜索由 provider 服务端执行，agent 循环看不到工具调用，故适配器从响应里
/// 尽力解析出查询/引用，循环据此合成一张「网络搜索」工具卡可视化给用户。
/// 任一项解析不到即为空；整体解析不到则 `GenerateOutput.web_search` 为 None。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuiltinWebSearch {
    #[serde(default)]
    pub queries: Vec<String>,
    #[serde(default)]
    pub citations: Vec<WebCitation>,
}

impl BuiltinWebSearch {
    /// 无查询也无引用视为「没发生可见的内置搜索」，不合成卡片。
    pub fn is_empty(&self) -> bool {
        self.queries.is_empty() && self.citations.is_empty()
    }
}

/// 模型**生成**的一张图片（协议无关形态）。适配器把各家 wire 格式（Gemini
/// `inlineData` 等）解析成 base64 + mime，穿过契约层交给 runtime 落地为 artifact。
/// 与 `MessagePart::Image`（用户上行的图）区分：这是模型的输出图。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedImageData {
    pub mime_type: String,
    /// base64 编码的图片字节（不含 data: 前缀）。
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateOutput {
    pub text: String,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<PendingToolCall>,
    pub usage: Option<ModelUsage>,
    pub finish_reason: Option<String>,
    pub provider_messages: Vec<Value>,
    pub cancelled: bool,
    /// 模型原生内置联网搜索的解析结果（仅 provider 服务端执行了内置搜索时为 Some）。
    /// 默认 None：绝大多数调用不开内置搜索，解析失败也降级为 None，不阻断答案。
    #[serde(default)]
    pub web_search: Option<BuiltinWebSearch>,
    /// 模型**生成的图片**（协议无关）。适配器解析各家 wire 出图格式填充；默认空，
    /// 绝大多数调用不出图。runtime 据此把图落地为 assistant 消息 artifact。
    #[serde(default)]
    pub images: Vec<GeneratedImageData>,
}

impl GenerateOutput {
    pub fn text(text: String, reasoning: Option<String>, provider_message: Value) -> Self {
        Self {
            text,
            reasoning,
            tool_calls: Vec::new(),
            usage: None,
            finish_reason: None,
            provider_messages: vec![provider_message],
            cancelled: false,
            web_search: None,
            images: Vec::new(),
        }
    }

    pub fn cancelled(text: String, reasoning: Option<String>) -> Self {
        Self {
            text,
            reasoning,
            tool_calls: Vec::new(),
            usage: None,
            finish_reason: Some("cancelled".to_string()),
            provider_messages: Vec::new(),
            cancelled: true,
            web_search: None,
            images: Vec::new(),
        }
    }

    pub fn to_openai_compatible_message(&self) -> Value {
        if let Some(message) = self.provider_messages.first() {
            return message.clone();
        }
        let mut message = serde_json::json!({
            "role": "assistant",
            "content": if self.text.is_empty() { Value::Null } else { Value::String(self.text.clone()) },
        });
        if let Some(reasoning) = self
            .reasoning
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            message["reasoning_content"] = Value::String(reasoning.to_string());
        }
        if !self.tool_calls.is_empty() {
            message["tool_calls"] = Value::Array(
                self.tool_calls
                    .iter()
                    .map(|call| {
                        serde_json::json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.function_name,
                                "arguments": call.arguments_raw,
                            }
                        })
                    })
                    .collect(),
            );
        }
        message
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamPart {
    TextDelta {
        delta: String,
    },
    ReasoningDelta {
        delta: String,
    },
    ToolCallStart {
        id: String,
        name: String,
    },
    ToolCallDelta {
        id: String,
        delta: String,
    },
    ToolCallDone {
        call: PendingToolCall,
    },
    /// 模型**原生内置联网搜索**的实时进度（任务 07-23）。适配器在服务端执行搜索的
    /// 过程中逐帧发射：首帧可为空（= 开牌 Running），随后带查询词/来源增量。为什么单变体
    /// 而非拆 Start/Update——sink 把「首次收到」当开牌、后续当增量累加，对三家适配器最省事。
    /// 仅 `AgentStreamSink` 消费（合成一张实时「网络搜索」卡置于答案文本之前）；Lens/丢弃
    /// 等其它 sink 走兜底 `_ => {}`，不受影响。
    WebSearch {
        queries: Vec<String>,
        citations: Vec<WebCitation>,
    },
    /// 模型**生成的一张图片**的流式帧（协议无关）。适配器解析各家 wire 出图（Gemini
    /// `inlineData` 等）逐张发射；`AgentStreamSink` 收集并落地为 artifact。其它 sink
    /// （Lens/丢弃/planning）走各自兜底，不受影响。
    ImageData {
        mime_type: String,
        data: String,
    },
    Finish {
        reason: String,
        full: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelErrorKind {
    Other,
    StreamReadInterrupted,
    /// 调用方主动取消（如 Lens 流的 `explain_stream_generation` 代际失配）。
    /// 由 sink 在 emit 时返回，适配器沿 `?` 上抛；包装层据此把取消当正常结束处理。
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct ModelError {
    pub message: String,
    pub kind: ModelErrorKind,
}

impl ModelError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ModelErrorKind::Other,
        }
    }

    pub fn with_kind(message: impl Into<String>, kind: ModelErrorKind) -> Self {
        Self {
            message: message.into(),
            kind,
        }
    }

    pub fn is_stream_read_interrupted(&self) -> bool {
        self.kind == ModelErrorKind::StreamReadInterrupted
    }

    pub fn is_cancelled(&self) -> bool {
        self.kind == ModelErrorKind::Cancelled
    }
}

pub fn stream_read_error(label: &str, err: &reqwest::Error) -> ModelError {
    let reason = if err.is_timeout() {
        "the provider stream timed out"
    } else if err.is_connect() {
        "the provider connection was interrupted"
    } else if err.is_decode() {
        "the provider sent an incomplete or invalid encoded stream chunk"
    } else {
        "the provider stream ended unexpectedly"
    };
    ModelError::with_kind(
        format!(
            "{label} 流式响应读取中断：{reason}。这通常是临时的网络、代理或模型服务流式断包问题，请重试。"
        ),
        ModelErrorKind::StreamReadInterrupted,
    )
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ModelError {}

impl From<String> for ModelError {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for ModelError {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

pub trait StreamSink: Send {
    fn emit(&mut self, part: StreamPart) -> Result<(), ModelError>;
}

/// Wraps a stream sink and records time-to-first visible text or reasoning delta.
pub struct FirstTokenStreamSink<'a> {
    inner: &'a mut (dyn StreamSink + Send),
    started: std::time::Instant,
    first_token_ms: Option<u64>,
}

impl<'a> FirstTokenStreamSink<'a> {
    pub fn new(inner: &'a mut (dyn StreamSink + Send), started: std::time::Instant) -> Self {
        Self {
            inner,
            started,
            first_token_ms: None,
        }
    }

    pub fn first_token_ms(&self) -> Option<u64> {
        self.first_token_ms
    }
}

impl StreamSink for FirstTokenStreamSink<'_> {
    fn emit(&mut self, part: StreamPart) -> Result<(), ModelError> {
        let is_first_visible_delta = self.first_token_ms.is_none()
            && match &part {
                StreamPart::TextDelta { delta } | StreamPart::ReasoningDelta { delta } => {
                    !delta.is_empty()
                }
                _ => false,
            };
        let elapsed_ms = is_first_visible_delta.then(|| self.started.elapsed().as_millis() as u64);
        let result = self.inner.emit(part);
        if result.is_ok() {
            self.first_token_ms = self.first_token_ms.or(elapsed_ms);
        }
        result
    }
}

impl<F> StreamSink for F
where
    F: FnMut(StreamPart) -> Result<(), ModelError> + Send,
{
    fn emit(&mut self, part: StreamPart) -> Result<(), ModelError> {
        self(part)
    }
}

pub type ModelFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ModelError>> + Send + 'a>>;

pub trait LanguageModelProvider {
    fn generate<'a>(&'a self, request: GenerateRequest) -> ModelFuture<'a, GenerateOutput>;
    fn stream<'a>(
        &'a self,
        request: GenerateRequest,
        sink: &'a mut (dyn StreamSink + Send),
    ) -> ModelFuture<'a, GenerateOutput>;
}

pub fn parse_tool_arguments(arguments_raw: &str) -> (Value, Option<String>) {
    let raw = if arguments_raw.trim().is_empty() {
        "{}"
    } else {
        arguments_raw
    };
    match serde_json::from_str(raw) {
        Ok(arguments) => (arguments, None),
        Err(err) => (
            Value::Null,
            Some(format!(
                "Tool arguments JSON is invalid or incomplete: {err}"
            )),
        ),
    }
}

/// 把 OpenAI `function.arguments` 归一成 raw JSON 字符串。
///
/// OpenAI 规范里它是 JSON **字符串**，但不少 OpenAI 兼容网关（含部分 codex / 代理模型，
/// 如 `gpt-*-codex-*`）直接发已解析的 JSON **对象**。只认字符串会把对象静默丢成 `{}`，
/// 于是 `query` 等必填参数缺失、schema 校验反复失败、模型空手重试形成死循环。两种形态都接：
/// 字符串原样返回，对象 / 数组 / 其它序列化成字符串，null / 缺失回退 `{}`。
pub fn tool_arguments_to_raw(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => "{}".to_string(),
        Some(other) => serde_json::to_string(other).unwrap_or_else(|_| "{}".to_string()),
    }
}

/// Pull the per-conversation workbench paragraph out of the system prompt.
///
/// Ordinary chats interpolate `…/conv_<id>` into that paragraph, which would
/// otherwise sit in the middle of `system` and break provider prefix-cache
/// matching for everything after it (skills + tool schemas). Callers park the
/// suffix on the first user message so it lands *after* tools in the token
/// stream.
pub fn split_workbench_system_suffix(system: &str) -> Option<(String, String)> {
    let mut paras: Vec<&str> = system.split("\n\n").collect();
    let idx = paras
        .iter()
        .rposition(|para| para.trim().starts_with(WORKBENCH_LOCATION_PROMPT_HEAD))?;
    let suffix = paras.remove(idx).trim().to_string();
    if suffix.is_empty() {
        return None;
    }
    let prefix = paras
        .into_iter()
        .map(str::trim)
        .filter(|para| !para.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    Some((prefix, suffix))
}

fn prepend_text_to_first_user_message(messages: &mut [ModelMessage], prefix: &str) -> bool {
    let Some(message) = messages.iter_mut().find(|m| m.role == ModelRole::User) else {
        return false;
    };
    if let Some(MessagePart::Text { text }) = message
        .content
        .iter_mut()
        .find(|part| matches!(part, MessagePart::Text { .. }))
    {
        if text.is_empty() {
            *text = prefix.to_string();
        } else {
            *text = format!("{prefix}\n\n{text}");
        }
    } else {
        message.content.insert(
            0,
            MessagePart::Text {
                text: prefix.to_string(),
            },
        );
    }
    true
}

pub fn generate_request_from_openai_messages(
    model: &str,
    messages: Vec<Value>,
    tools: Option<&[ChatToolDefinition]>,
    options: GenerateOptions,
    label: &str,
    context: GenerateRequestContext,
) -> GenerateRequest {
    let mut system_parts = Vec::new();
    let mut model_messages = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if role == "system" {
            if let Some(content) = openai_message_text_content(&message) {
                system_parts.push(content);
            }
            continue;
        }
        if let Some(model_message) = model_message_from_openai_message(&message) {
            model_messages.push(model_message);
        }
    }
    if let Some(first) = system_parts.first_mut() {
        if let Some((prefix, suffix)) = split_workbench_system_suffix(first) {
            *first = prefix;
            if !prepend_text_to_first_user_message(&mut model_messages, &suffix) {
                // No user turn to park it on — keep the paragraph in system.
                if first.is_empty() {
                    *first = suffix;
                } else {
                    first.push_str("\n\n");
                    first.push_str(&suffix);
                }
            }
        }
    }
    system_parts.retain(|part| !part.trim().is_empty());
    GenerateRequest {
        model: model.to_string(),
        system: system_parts.join("\n\n"),
        messages: model_messages,
        tools: tools
            .unwrap_or_default()
            .iter()
            .map(ModelTool::from)
            .collect(),
        options,
        metadata: RequestMetadata {
            label: label.to_string(),
            conversation_id: context.conversation_id,
            message_id: context.message_id,
            ..RequestMetadata::default()
        },
    }
}

pub fn model_messages_from_openai_messages(messages: Vec<Value>) -> Vec<ModelMessage> {
    messages
        .into_iter()
        .filter_map(|message| model_message_from_openai_message(&message))
        .collect()
}

pub fn openai_messages_from_generate_request(request: &GenerateRequest) -> Vec<Value> {
    let mut messages = Vec::new();
    if !request.system.trim().is_empty() {
        messages.push(serde_json::json!({
            "role": "system",
            "content": request.system,
        }));
    }
    for message in &request.messages {
        messages.extend(openai_messages_from_model_message(message));
    }
    messages
}

pub fn openai_messages_from_model_messages(messages: &[ModelMessage]) -> Vec<Value> {
    messages
        .iter()
        .flat_map(openai_messages_from_model_message)
        .collect()
}

fn openai_message_text_content(message: &Value) -> Option<String> {
    match message.get("content")? {
        Value::String(value) => Some(value.clone()),
        Value::Array(parts) => {
            let texts = parts
                .iter()
                .filter_map(|part| {
                    if part.get("type").and_then(|value| value.as_str()) == Some("text") {
                        part.get("text").and_then(|value| value.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join("\n"))
            }
        }
        _ => None,
    }
}

fn model_message_from_openai_message(message: &Value) -> Option<ModelMessage> {
    let role = match message
        .get("role")
        .and_then(|value| value.as_str())
        .unwrap_or("")
    {
        "assistant" => ModelRole::Assistant,
        "tool" => ModelRole::Tool,
        "user" => ModelRole::User,
        _ => return None,
    };
    let mut parts = Vec::new();
    if role == ModelRole::Tool {
        parts.push(MessagePart::ToolResult {
            tool_call_id: message
                .get("tool_call_id")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            content: message
                .get("content")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            is_error: false,
            artifacts: Vec::new(),
        });
        return Some(ModelMessage {
            role,
            content: parts,
        });
    }
    if let Some(content) = message.get("content") {
        match content {
            Value::String(text) if !text.is_empty() => {
                parts.push(MessagePart::Text { text: text.clone() });
            }
            Value::Array(items) => {
                for item in items {
                    match item
                        .get("type")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                    {
                        "text" => {
                            if let Some(text) = item.get("text").and_then(|value| value.as_str()) {
                                parts.push(MessagePart::Text {
                                    text: text.to_string(),
                                });
                            }
                        }
                        "image_url" => {
                            let url = item
                                .get("image_url")
                                .and_then(|value| value.get("url"))
                                .and_then(|value| value.as_str())
                                .unwrap_or_default();
                            if let Some((mime_type, data)) = parse_data_image_url(url) {
                                parts.push(MessagePart::Image {
                                    mime_type,
                                    data,
                                    path: None,
                                });
                            } else if !url.is_empty() {
                                parts.push(MessagePart::ImageUrl {
                                    url: url.to_string(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(reasoning) = message
        .get("reasoning_content")
        .or_else(|| message.get("reasoning"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
    {
        parts.push(MessagePart::Reasoning {
            text: reasoning.to_string(),
        });
    }
    for call in pending_tool_calls_from_openai_message(message) {
        parts.push(MessagePart::ToolCall {
            id: call.id,
            name: call.function_name,
            arguments: call.arguments,
            arguments_raw: call.arguments_raw,
            signature: call.signature,
        });
    }
    Some(ModelMessage {
        role,
        content: parts,
    })
}

pub fn pending_tool_calls_from_openai_message(message: &Value) -> Vec<PendingToolCall> {
    message
        .get("tool_calls")
        .and_then(|value| value.as_array())
        .map(|calls| {
            calls
                .iter()
                .filter_map(|call| {
                    let function = call.get("function")?;
                    let name = function.get("name")?.as_str()?.to_string();
                    let arguments_raw = tool_arguments_to_raw(function.get("arguments"));
                    let (arguments, arguments_parse_error) = parse_tool_arguments(&arguments_raw);
                    Some(PendingToolCall {
                        id: call
                            .get("id")
                            .and_then(|value| value.as_str())
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("tool_{}", uuid::Uuid::new_v4())),
                        function_name: name,
                        arguments,
                        arguments_raw,
                        arguments_parse_error,
                        // Gemini thoughtSignature（自定义键）随回放带回。
                        signature: call
                            .get("thought_signature")
                            .and_then(|value| value.as_str())
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn openai_messages_from_model_message(message: &ModelMessage) -> Vec<Value> {
    if message.role == ModelRole::Tool {
        return message
            .content
            .iter()
            .filter_map(|part| match part {
                MessagePart::ToolResult {
                    tool_call_id,
                    content,
                    ..
                } => Some(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": tool_call_id,
                    "content": content,
                })),
                _ => None,
            })
            .collect();
    }
    let mut text_parts = Vec::new();
    let mut multimodal_parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut reasoning: Option<String> = None;
    for part in &message.content {
        match part {
            MessagePart::Text { text } => {
                text_parts.push(text.clone());
                multimodal_parts.push(serde_json::json!({ "type": "text", "text": text }));
            }
            MessagePart::Image {
                mime_type, data, ..
            } => {
                // data 为空 = 外置了但没 rehydrate（回放路径漏了读盘）。发个占位符
                // 而不是 `data:image/png;base64,`，免得让 provider 400。
                if data.is_empty() {
                    multimodal_parts.push(
                        serde_json::json!({ "type": "text", "text": MISSING_IMAGE_PLACEHOLDER }),
                    );
                } else {
                    multimodal_parts.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": { "url": format!("data:{mime_type};base64,{data}") },
                    }));
                }
            }
            MessagePart::ImageUrl { url } => {
                multimodal_parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": { "url": url },
                }));
            }
            MessagePart::ToolCall {
                id,
                name,
                arguments_raw,
                signature,
                ..
            } => {
                let mut call = serde_json::json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": arguments_raw,
                    }
                });
                // Gemini thoughtSignature 挂在自定义键上带过存储/回放（其他 provider 忽略）。
                if let Some(signature) = signature {
                    call["thought_signature"] = Value::String(signature.clone());
                }
                tool_calls.push(call);
            }
            MessagePart::Reasoning { text } => {
                reasoning = Some(text.clone());
            }
            MessagePart::ToolResult { .. } => {}
        }
    }
    let content = if multimodal_parts
        .iter()
        .any(|part| part.get("type").and_then(|value| value.as_str()) == Some("image_url"))
    {
        Value::Array(multimodal_parts)
    } else if text_parts.is_empty() && !tool_calls.is_empty() {
        Value::Null
    } else {
        Value::String(text_parts.join("\n"))
    };
    let mut out = serde_json::json!({
        "role": message.role.as_str(),
        "content": content,
    });
    if !tool_calls.is_empty() {
        out["tool_calls"] = Value::Array(tool_calls);
    }
    if let Some(reasoning) = reasoning {
        out["reasoning_content"] = Value::String(reasoning);
    }
    vec![out]
}

fn parse_data_image_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("data:")?;
    let (mime, data) = rest.split_once(";base64,")?;
    Some((mime.to_string(), data.to_string()))
}

/// Build the OpenAI **Responses** API `input` array from canonical `ModelMessage`s.
///
/// The Responses API models conversation state as a flat list of *items* rather than
/// chat messages: tool calls become `{"type":"function_call",...}` items and tool
/// results become `{"type":"function_call_output",...}` items (NOT `role:"tool"`
/// messages). User/assistant text use `input_text` / `output_text` content parts, and
/// user images use `input_image`. Mirrors `openai_messages_from_model_message` but for
/// the Responses item shapes. (System text is carried separately as `instructions`.)
pub fn responses_input_from_model_messages(messages: &[ModelMessage]) -> Vec<Value> {
    let mut items = Vec::new();
    for message in messages {
        responses_items_from_model_message(message, &mut items);
    }
    items
}

fn responses_items_from_model_message(message: &ModelMessage, items: &mut Vec<Value>) {
    if message.role == ModelRole::Tool {
        for part in &message.content {
            if let MessagePart::ToolResult {
                tool_call_id,
                content,
                ..
            } = part
            {
                items.push(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": tool_call_id,
                    "output": content,
                }));
            }
        }
        return;
    }

    let is_assistant = message.role == ModelRole::Assistant;
    let text_part_type = if is_assistant {
        "output_text"
    } else {
        "input_text"
    };
    let mut content_parts: Vec<Value> = Vec::new();
    // Tool calls are sibling items, emitted AFTER the assistant's text content so the
    // ordering matches a natural turn (message, then the calls it made).
    let mut tool_call_items: Vec<Value> = Vec::new();

    for part in &message.content {
        match part {
            MessagePart::Text { text } => {
                content_parts.push(serde_json::json!({ "type": text_part_type, "text": text }));
            }
            MessagePart::Image {
                mime_type, data, ..
            } => {
                if data.is_empty() {
                    content_parts.push(
                        serde_json::json!({ "type": text_part_type, "text": MISSING_IMAGE_PLACEHOLDER }),
                    );
                } else {
                    content_parts.push(serde_json::json!({
                        "type": "input_image",
                        "image_url": format!("data:{mime_type};base64,{data}"),
                    }));
                }
            }
            MessagePart::ImageUrl { url } => {
                content_parts.push(serde_json::json!({
                    "type": "input_image",
                    "image_url": url,
                }));
            }
            MessagePart::ToolCall {
                id,
                name,
                arguments_raw,
                ..
            } => {
                tool_call_items.push(serde_json::json!({
                    "type": "function_call",
                    "call_id": id,
                    "name": name,
                    "arguments": arguments_raw,
                }));
            }
            // Reasoning is omitted on replay; ToolResult only appears on Tool messages.
            MessagePart::Reasoning { .. } | MessagePart::ToolResult { .. } => {}
        }
    }

    if !content_parts.is_empty() {
        items.push(serde_json::json!({
            "role": message.role.as_str(),
            "content": content_parts,
        }));
    }
    items.extend(tool_call_items);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_error_kind_marks_stream_read_interrupts_without_message_matching() {
        let stream_error = ModelError::with_kind(
            "temporary stream failure",
            ModelErrorKind::StreamReadInterrupted,
        );
        let generic_error = ModelError::new("temporary stream failure");

        assert!(stream_error.is_stream_read_interrupted());
        assert!(!generic_error.is_stream_read_interrupted());
    }

    #[test]
    fn pending_tool_calls_accept_string_and_object_arguments() {
        // 字符串形态（OpenAI 规范）。
        let string_msg = serde_json::json!({
            "tool_calls": [{
                "id": "call_1",
                "function": { "name": "web_search", "arguments": "{\"query\":\"a\"}" }
            }]
        });
        let calls = pending_tool_calls_from_openai_message(&string_msg);
        assert_eq!(calls[0].arguments["query"], "a");
        assert!(calls[0].arguments_parse_error.is_none());

        // 对象形态（部分 OpenAI 兼容网关 / codex 代理）。回归：旧逻辑会丢成 `{}`。
        let object_msg = serde_json::json!({
            "tool_calls": [{
                "id": "call_2",
                "function": { "name": "web_search", "arguments": { "query": "b" } }
            }]
        });
        let calls = pending_tool_calls_from_openai_message(&object_msg);
        assert_eq!(calls[0].arguments["query"], "b");
        assert!(calls[0].arguments_parse_error.is_none());

        // 缺失 / null → 回退空对象，不报错。
        let null_msg = serde_json::json!({
            "tool_calls": [{
                "id": "call_3",
                "function": { "name": "web_search", "arguments": null }
            }]
        });
        let calls = pending_tool_calls_from_openai_message(&null_msg);
        assert_eq!(calls[0].arguments_raw, "{}");
        assert!(calls[0].arguments_parse_error.is_none());
    }

    #[test]
    fn split_workbench_system_suffix_extracts_path_even_when_not_last() {
        let system = "static role\n\n\
Current default workbench: `/tmp/conv_abc`. When the user does not specify a location, use relative paths or the default cwd so files and basic work land here.\n\n\
MCP note after the path.";
        let (prefix, suffix) = split_workbench_system_suffix(system).expect("suffix");
        assert_eq!(prefix, "static role\n\nMCP note after the path.");
        assert!(suffix.starts_with(WORKBENCH_LOCATION_PROMPT_HEAD));
        assert!(suffix.contains("conv_abc"));
        assert!(!prefix.contains("conv_abc"));
    }

    #[test]
    fn generate_request_parks_workbench_on_first_user_not_system() {
        let workbench = format!(
            "{WORKBENCH_LOCATION_PROMPT_HEAD} `/tmp/conv_abc`. When the user does not specify a location, use relative paths or the default cwd so files and basic work land here."
        );
        let messages = vec![
            serde_json::json!({
                "role": "system",
                "content": format!("You are Kivio.\n\n{workbench}"),
            }),
            serde_json::json!({ "role": "user", "content": "hello" }),
            serde_json::json!({ "role": "assistant", "content": "hi" }),
            serde_json::json!({ "role": "user", "content": "next" }),
        ];
        let request = generate_request_from_openai_messages(
            "m",
            messages,
            None,
            Default::default(),
            "t",
            Default::default(),
        );
        assert_eq!(request.system, "You are Kivio.");
        assert!(!request.system.contains("conv_abc"));

        let first_user = request
            .messages
            .iter()
            .find(|m| m.role == ModelRole::User)
            .expect("first user");
        let MessagePart::Text { text } = &first_user.content[0] else {
            panic!("expected text part");
        };
        assert!(text.starts_with(&workbench));
        assert!(text.ends_with("hello"));

        let last_user = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == ModelRole::User)
            .expect("last user");
        let MessagePart::Text { text } = &last_user.content[0] else {
            panic!("expected text part");
        };
        assert_eq!(text, "next");
    }

    #[test]
    fn generate_request_leaves_system_alone_without_workbench_paragraph() {
        let messages = vec![
            serde_json::json!({ "role": "system", "content": "You are Kivio." }),
            serde_json::json!({ "role": "user", "content": "hello" }),
        ];
        let request = generate_request_from_openai_messages(
            "m",
            messages,
            None,
            Default::default(),
            "t",
            Default::default(),
        );
        assert_eq!(request.system, "You are Kivio.");
        let MessagePart::Text { text } = &request.messages[0].content[0] else {
            panic!("expected text part");
        };
        assert_eq!(text, "hello");
    }

    #[test]
    fn responses_input_maps_tool_call_and_result_to_items() {
        let messages = vec![
            ModelMessage::text(ModelRole::User, "明天吉林市天气？"),
            ModelMessage {
                role: ModelRole::Assistant,
                content: vec![MessagePart::ToolCall {
                    id: "call_1".to_string(),
                    name: "web_search".to_string(),
                    arguments: serde_json::json!({ "query": "吉林市 明天 天气" }),
                    arguments_raw: "{\"query\":\"吉林市 明天 天气\"}".to_string(),
                    signature: None,
                }],
            },
            ModelMessage {
                role: ModelRole::Tool,
                content: vec![MessagePart::ToolResult {
                    tool_call_id: "call_1".to_string(),
                    content: "多云转晴 16-24℃".to_string(),
                    is_error: false,
                    artifacts: Vec::new(),
                }],
            },
        ];
        let items = responses_input_from_model_messages(&messages);

        // user message → role item with input_text
        assert_eq!(items[0]["role"], "user");
        assert_eq!(items[0]["content"][0]["type"], "input_text");
        assert_eq!(items[0]["content"][0]["text"], "明天吉林市天气？");
        // assistant tool call → function_call item (no empty assistant message emitted)
        assert_eq!(items[1]["type"], "function_call");
        assert_eq!(items[1]["call_id"], "call_1");
        assert_eq!(items[1]["name"], "web_search");
        assert_eq!(items[1]["arguments"], "{\"query\":\"吉林市 明天 天气\"}");
        // tool result → function_call_output item
        assert_eq!(items[2]["type"], "function_call_output");
        assert_eq!(items[2]["call_id"], "call_1");
        assert_eq!(items[2]["output"], "多云转晴 16-24℃");
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn responses_input_maps_user_image_to_input_image() {
        let messages = vec![ModelMessage {
            role: ModelRole::User,
            content: vec![
                MessagePart::ImageUrl {
                    url: "data:image/png;base64,AAAA".to_string(),
                },
                MessagePart::Text {
                    text: "what is this?".to_string(),
                },
            ],
        }];
        let items = responses_input_from_model_messages(&messages);
        assert_eq!(items.len(), 1);
        let content = items[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "input_image");
        assert_eq!(content[0]["image_url"], "data:image/png;base64,AAAA");
        assert_eq!(content[1]["type"], "input_text");
    }

    #[test]
    fn first_token_sink_ignores_non_visible_parts_and_records_first_delta() {
        let mut emitted = Vec::new();
        let mut downstream = |part| {
            emitted.push(part);
            Ok(())
        };
        let started = std::time::Instant::now() - std::time::Duration::from_millis(10);
        let mut sink = FirstTokenStreamSink::new(&mut downstream, started);

        sink.emit(StreamPart::WebSearch {
            queries: vec!["query".to_string()],
            citations: Vec::new(),
        })
        .expect("web search event");
        assert_eq!(sink.first_token_ms(), None);

        sink.emit(StreamPart::ReasoningDelta {
            delta: "thinking".to_string(),
        })
        .expect("reasoning delta");
        let first = sink.first_token_ms().expect("first token timing");
        assert!(first >= 10);

        sink.emit(StreamPart::TextDelta {
            delta: "answer".to_string(),
        })
        .expect("text delta");
        assert_eq!(sink.first_token_ms(), Some(first));
        drop(sink);
        assert_eq!(emitted.len(), 3);
    }
}
