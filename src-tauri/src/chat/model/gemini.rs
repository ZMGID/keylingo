use reqwest::header::{HeaderMap, HeaderValue, ACCEPT_ENCODING};
use serde_json::Value;

use crate::api::{send_with_failover, with_chat_request_timeout};
use crate::settings::ModelProvider;
use crate::state::AppState;
use crate::usage::{
    chat_usage_source_for_label, error_kind_from_message, operation_from_label, record_model_call,
    UsageRecordInput,
};

use super::{
    parse_tool_arguments, stream_read_error, BuiltinWebSearch, FirstTokenStreamSink,
    GenerateOutput, GenerateRequest, GeneratedImageData, LanguageModelProvider, MessagePart,
    ModelError, ModelFuture, ModelMessage, ModelRole, ModelTool, ModelUsage, PendingToolCall,
    StreamPart, StreamSink, WebCitation,
};

/// Google Gemini **原生** `generateContent` adapter（peer of openai/anthropic）。
/// 用 Gemini 原生协议，天然不发 OpenAI 专有字段（`promptCacheKey`/`tool_choice`/…），
/// 绕开 Gemini OpenAI-compat 端点对未知 body 字段的 400 严格校验。
/// wire 形状据 opencode 真实流量确认（见任务 research/opencode-real-traffic.md）。
pub struct GeminiProvider<'a> {
    state: &'a AppState,
    provider: &'a ModelProvider,
    retry_attempts: usize,
}

impl<'a> GeminiProvider<'a> {
    pub fn new(state: &'a AppState, provider: &'a ModelProvider, retry_attempts: usize) -> Self {
        Self {
            state,
            provider,
            retry_attempts,
        }
    }
}

impl LanguageModelProvider for GeminiProvider<'_> {
    fn generate<'a>(&'a self, request: GenerateRequest) -> ModelFuture<'a, GenerateOutput> {
        Box::pin(async move { self.generate_inner(request).await })
    }

    fn stream<'a>(
        &'a self,
        request: GenerateRequest,
        sink: &'a mut (dyn StreamSink + Send),
    ) -> ModelFuture<'a, GenerateOutput> {
        Box::pin(async move { self.stream_inner(request, sink).await })
    }
}

impl GeminiProvider<'_> {
    async fn generate_inner(&self, request: GenerateRequest) -> Result<GenerateOutput, ModelError> {
        let label = request_label(&request, "Gemini generateContent");
        let started_at = chrono::Local::now().timestamp();
        let started = std::time::Instant::now();
        let body = self.request_body(&request, false);
        let url = self.endpoint_url(&request.model, false);
        let response = send_with_failover(
            self.state,
            &label,
            self.retry_attempts,
            &self.provider.id,
            &self.provider.api_keys,
            |key| {
                with_chat_request_timeout(crate::api::attach_json_body(
                    crate::provider_request::apply(
                        self.state
                            .client_for(self.provider)
                            .post(&url)
                            .headers(gemini_headers(key).unwrap_or_default())
                            .header(ACCEPT_ENCODING, "identity"),
                        self.provider,
                        request.metadata.conversation_id.as_deref(),
                    ),
                    &body,
                    self.provider.compress_request_body,
                ))
                .send()
            },
        )
        .await
        .map_err(|err| {
            self.record_usage_failure(&request, &label, started_at, started.elapsed(), &err);
            self.record_debug_failure(&request, &label, false, &err, started_at, started.elapsed());
            ModelError::new(err)
        })?;

        let raw = response.text().await.map_err(|err| {
            let message = format!("{label} read body: {err}");
            self.record_usage_failure(&request, &label, started_at, started.elapsed(), &message);
            self.record_debug_failure(
                &request,
                &label,
                false,
                &message,
                started_at,
                started.elapsed(),
            );
            ModelError::new(message)
        })?;
        let value: Value = serde_json::from_str(&raw).map_err(|err| {
            let message = format!(
                "{label} parse JSON: {} (body: {})",
                err,
                raw.chars().take(500).collect::<String>()
            );
            self.record_usage_failure(&request, &label, started_at, started.elapsed(), &message);
            self.record_debug_failure(
                &request,
                &label,
                false,
                &message,
                started_at,
                started.elapsed(),
            );
            ModelError::new(message)
        })?;
        let output = output_from_gemini_response(&value, &label)?;
        self.record_usage_success(
            &request,
            &label,
            started_at,
            started.elapsed(),
            output.usage.clone(),
            None,
        );
        self.record_debug_success(
            &request,
            &label,
            false,
            &output,
            started_at,
            started.elapsed(),
        );
        Ok(output)
    }

    async fn stream_inner(
        &self,
        request: GenerateRequest,
        sink: &mut (dyn StreamSink + Send),
    ) -> Result<GenerateOutput, ModelError> {
        let label = request_label(&request, "Gemini stream");
        let started_at = chrono::Local::now().timestamp();
        let started = std::time::Instant::now();
        let mut measured_sink = FirstTokenStreamSink::new(sink, started);
        let sink = &mut measured_sink;
        let body = self.request_body(&request, true);
        let url = self.endpoint_url(&request.model, true);
        let mut response = send_with_failover(
            self.state,
            &label,
            self.retry_attempts,
            &self.provider.id,
            &self.provider.api_keys,
            |key| {
                crate::api::attach_json_body(
                    crate::provider_request::apply(
                        self.state
                            .client_for(self.provider)
                            .post(&url)
                            .headers(gemini_headers(key).unwrap_or_default())
                            .header(ACCEPT_ENCODING, "identity"),
                        self.provider,
                        request.metadata.conversation_id.as_deref(),
                    ),
                    &body,
                    self.provider.compress_request_body,
                )
                .send()
            },
        )
        .await
        .map_err(|err| {
            self.record_usage_failure(&request, &label, started_at, started.elapsed(), &err);
            self.record_debug_failure(&request, &label, true, &err, started_at, started.elapsed());
            ModelError::new(err)
        })?;

        let mut buffer = String::new();
        let mut utf8 = crate::api::Utf8StreamDecoder::default();
        let mut full = String::new();
        let mut reasoning_full = String::new();
        let mut tool_calls: Vec<PendingToolCall> = Vec::new();
        // thoughtSignature 兜底（跨 chunk 记住，functionCall 自身没有时回退用它）。
        let mut carry_sig: Option<String> = None;
        let mut finish_reason = "stop".to_string();
        let mut usage: Option<ModelUsage> = None;
        // 内置搜索引用：groundingMetadata 通常在末段 chunk，逐段合并兜底。
        let mut web_search: Option<BuiltinWebSearch> = None;
        // 模型生成的图片：逐 chunk 的 inlineData part 累积，finish 后并入 output。
        let mut images: Vec<GeneratedImageData> = Vec::new();

        loop {
            let chunk = response.chunk().await.map_err(|err| {
                let model_error = stream_read_error(&label, &err);
                self.record_usage_failure(
                    &request,
                    &label,
                    started_at,
                    started.elapsed(),
                    &model_error.to_string(),
                );
                self.record_debug_failure(
                    &request,
                    &label,
                    true,
                    &model_error.to_string(),
                    started_at,
                    started.elapsed(),
                );
                model_error
            })?;
            let Some(chunk) = chunk else {
                break;
            };
            buffer.push_str(&utf8.push(&chunk));

            // Gemini `alt=sse`：每个 `data:` 行是一整段 GenerateContentResponse 片段。
            while let Some(pos) = buffer.find('\n') {
                let line: String = buffer.drain(..=pos).collect();
                let line = line.trim();
                if !line.starts_with("data:") {
                    continue;
                }
                let data = line.trim_start_matches("data:").trim();
                if data.is_empty() {
                    continue;
                }
                let Ok(value) = serde_json::from_str::<Value>(data) else {
                    continue;
                };
                if let Some(err) = gemini_error_message(&value) {
                    sink.emit(StreamPart::Error {
                        message: err.clone(),
                    })?;
                    return Err(ModelError::new(format!("Gemini stream error: {err}")));
                }
                // 逐 part：text/thought → 增量；functionCall → 完整工具调用。
                // 先整块预扫一个候选签名（thoughtSignature 可能在 functionCall 兄弟 part 上、
                // 且顺序不定），functionCall 自身无签名时回退用它。
                if let Some(sig) = gemini_candidate_signature(&value) {
                    carry_sig = Some(sig);
                }
                for part in gemini_response_parts(&value) {
                    if let Some(mut call) = gemini_tool_call_from_part(part) {
                        if call.signature.is_none() {
                            call.signature = carry_sig.clone();
                        }
                        sink.emit(StreamPart::ToolCallStart {
                            id: call.id.clone(),
                            name: call.function_name.clone(),
                        })?;
                        sink.emit(StreamPart::ToolCallDelta {
                            id: call.id.clone(),
                            delta: call.arguments_raw.clone(),
                        })?;
                        sink.emit(StreamPart::ToolCallDone { call: call.clone() })?;
                        tool_calls.push(call);
                    } else if let Some(image) = gemini_image_from_part(part) {
                        sink.emit(StreamPart::ImageData {
                            mime_type: image.mime_type.clone(),
                            data: image.data.clone(),
                        })?;
                        images.push(image);
                    } else if let Some((text, is_thought)) = gemini_text_from_part(part) {
                        if is_thought {
                            reasoning_full.push_str(&text);
                            sink.emit(StreamPart::ReasoningDelta { delta: text })?;
                        } else {
                            full.push_str(&text);
                            sink.emit(StreamPart::TextDelta { delta: text })?;
                        }
                    }
                }
                if let Some(reason) = gemini_finish_reason_str(&value) {
                    finish_reason = reason;
                }
                if let Some(next_usage) = gemini_usage(&value) {
                    usage = Some(next_usage);
                }
                // 实时卡（任务 07-23）：grounding 通常在末段到达，实时性打折但答案前定位
                // 收益照拿；仅当本 chunk 真有新数据才发（避免空帧），随后并入累加值。
                let chunk_ws = web_search_from_gemini_value(&value);
                if let Some(ws) = chunk_ws.as_ref() {
                    if !ws.is_empty() {
                        sink.emit(StreamPart::WebSearch {
                            queries: ws.queries.clone(),
                            citations: ws.citations.clone(),
                        })?;
                    }
                }
                merge_gemini_web_search(&mut web_search, chunk_ws);
            }
        }

        // 有工具调用则结束原因归一为 tool_calls（Gemini 常仍返回 STOP）。
        let finish_reason = normalize_finish_reason(&finish_reason, !tool_calls.is_empty());
        sink.emit(StreamPart::Finish {
            reason: finish_reason.clone(),
            full: full.clone(),
        })?;
        let mut output = stream_output(full, reasoning_full, tool_calls, finish_reason, usage);
        output.web_search = web_search;
        output.images = images;
        self.record_usage_success(
            &request,
            &label,
            started_at,
            started.elapsed(),
            output.usage.clone(),
            sink.first_token_ms(),
        );
        self.record_debug_success(
            &request,
            &label,
            true,
            &output,
            started_at,
            started.elapsed(),
        );
        Ok(output)
    }

    fn endpoint_url(&self, model: &str, stream: bool) -> String {
        gemini_url(&self.provider.base_url, model, stream)
    }

    fn request_body(&self, request: &GenerateRequest, _stream: bool) -> Value {
        // 注意：Gemini 的 model/method/stream 都在 URL 上，不在 body 里。
        let mut body = serde_json::json!({
            "contents": gemini_contents_from_generate_request(request),
            "generationConfig": {
                "maxOutputTokens": request.options.max_tokens,
            },
        });
        if let Some(temperature) = crate::chat::model_metadata::temperature_for_request(
            request.options.temperature,
            Some(self.provider),
            &request.model,
        ) {
            body["generationConfig"]["temperature"] = serde_json::json!(temperature);
        }
        if !request.system.trim().is_empty() {
            body["systemInstruction"] = serde_json::json!({
                "parts": [{ "text": request.system }]
            });
        }
        // 出图模型：显式声明 TEXT+IMAGE 输出模态（否则只回文本）。红线：仅对出图模型下发,
        // 普通文本模型请求体保持字节不变(Gemini 对未知/多余字段严格校验会 400)。
        if crate::chat::model_metadata::is_image_output_model(&request.model) {
            body["generationConfig"]["responseModalities"] = serde_json::json!(["TEXT", "IMAGE"]);
        }
        let declarations = gemini_function_declarations(&request.tools);
        let mut tools_arr: Vec<Value> = Vec::new();
        if !declarations.is_empty() {
            tools_arr.push(serde_json::json!({ "functionDeclarations": declarations }));
            // 显式声明工具调用模式（对齐 opencode 真实流量）。
            body["toolConfig"] = serde_json::json!({
                "functionCallingConfig": { "mode": "AUTO" }
            });
        }
        // 模型原生内置联网搜索（任务 07-23）：Gemini grounding，作为并列 tool 对象加入。
        if request.options.builtin_web_search {
            tools_arr.push(serde_json::json!({ "google_search": {} }));
        }
        if !tools_arr.is_empty() {
            body["tools"] = Value::Array(tools_arr);
        }
        // 思考等级 → thinkingConfig，原样下发（档位由模型库 reasoningEfforts 门控）。
        // 版本分叉:Gemini 3.x 用 `thinkingLevel`(字符串档位),2.5 系只认 `thinkingBudget`(数值),
        // 两者互斥、传错会 400。故 thinkingLevel 只对 3.x 下发;其余(2.5/未知)回退到仅 `includeThoughts`
        // (开思维输出、不强加档位)——保持改动前对 2.5 的非回归行为。includeThoughts 两条路都带,拿摘要。
        if let Some(level) = request.options.thinking_level.as_deref() {
            let mut thinking = serde_json::json!({ "includeThoughts": true });
            if gemini_supports_thinking_level(&request.model) {
                thinking["thinkingLevel"] = Value::String(level.to_string());
            }
            body["generationConfig"]["thinkingConfig"] = thinking;
        }
        if let Some(overrides) = request.options.provider_options.as_object() {
            for (key, value) in overrides {
                body[key] = value.clone();
            }
        }
        body
    }

    /// 重建本次请求实际会带的 headers（脱敏后）供请求调试面板展示。
    fn debug_request_headers(
        &self,
        metadata: &crate::chat::model::RequestMetadata,
    ) -> std::collections::BTreeMap<String, String> {
        let mut headers = std::collections::BTreeMap::new();
        if let Some(key) = self.provider.api_keys.first() {
            headers.insert("x-goog-api-key".to_string(), key.clone());
        }
        headers.insert("content-type".to_string(), "application/json".to_string());
        headers.insert("Accept-Encoding".to_string(), "identity".to_string());
        for (name, value) in crate::provider_request::header_pairs(
            self.provider,
            metadata.conversation_id.as_deref(),
        ) {
            headers.insert(name, value);
        }
        crate::chat::request_debug::sanitize_headers(headers)
    }

    fn record_debug_success(
        &self,
        request: &GenerateRequest,
        label: &str,
        stream: bool,
        output: &GenerateOutput,
        started_at: i64,
        duration: std::time::Duration,
    ) {
        if !self.state.request_debug_enabled() {
            return;
        }
        let record = crate::chat::request_debug::build_debug_record(
            crate::chat::request_debug::DebugRecordArgs {
                provider: self.provider,
                request,
                label,
                started_at,
                duration_ms: duration.as_millis() as u64,
                status: "success",
                url: self.endpoint_url(&request.model, stream),
                headers: self.debug_request_headers(&request.metadata),
                body: self.request_body(request, stream),
                stream,
                response: crate::chat::request_debug::RequestDebugResponse::from_output(
                    output,
                    Some(200),
                ),
            },
        );
        crate::chat::request_debug::record(self.state, record);
    }

    fn record_debug_failure(
        &self,
        request: &GenerateRequest,
        label: &str,
        stream: bool,
        error: &str,
        started_at: i64,
        duration: std::time::Duration,
    ) {
        if !self.state.request_debug_enabled() {
            return;
        }
        let record = crate::chat::request_debug::build_debug_record(
            crate::chat::request_debug::DebugRecordArgs {
                provider: self.provider,
                request,
                label,
                started_at,
                duration_ms: duration.as_millis() as u64,
                status: "error",
                url: self.endpoint_url(&request.model, stream),
                headers: self.debug_request_headers(&request.metadata),
                body: self.request_body(request, stream),
                stream,
                response: crate::chat::request_debug::RequestDebugResponse::from_error(
                    error,
                    crate::api::extract_status_code(error),
                ),
            },
        );
        crate::chat::request_debug::record(self.state, record);
    }

    fn record_usage_success(
        &self,
        request: &GenerateRequest,
        label: &str,
        started_at: i64,
        duration: std::time::Duration,
        usage: Option<ModelUsage>,
        first_token_ms: Option<u64>,
    ) {
        let source = request
            .metadata
            .usage_source
            .clone()
            .unwrap_or_else(|| chat_usage_source_for_label(label));
        let operation = request
            .metadata
            .usage_operation
            .clone()
            .unwrap_or_else(|| operation_from_label(label));
        record_model_call(
            self.state,
            UsageRecordInput {
                provider: self.provider,
                model: &request.model,
                source: &source,
                operation: &operation,
                status: "success",
                status_code: Some(200),
                usage,
                usage_source: "provider_reported",
                started_at,
                duration_ms: duration.as_millis() as u64,
                first_token_ms,
                reasoning_effort: crate::usage::reasoning_effort_for_request(request),
                conversation_id: request.metadata.conversation_id.clone(),
                message_id: request.metadata.message_id.clone(),
                error_kind: None,
            },
        );
    }

    fn record_usage_failure(
        &self,
        request: &GenerateRequest,
        label: &str,
        started_at: i64,
        duration: std::time::Duration,
        error: &str,
    ) {
        let source = request
            .metadata
            .usage_source
            .clone()
            .unwrap_or_else(|| chat_usage_source_for_label(label));
        let operation = request
            .metadata
            .usage_operation
            .clone()
            .unwrap_or_else(|| operation_from_label(label));
        record_model_call(
            self.state,
            UsageRecordInput {
                provider: self.provider,
                model: &request.model,
                source: &source,
                operation: &operation,
                status: crate::usage::failure_status_from_message(error),
                status_code: crate::api::extract_status_code(error),
                usage: None,
                usage_source: "missing",
                started_at,
                duration_ms: duration.as_millis() as u64,
                first_token_ms: None,
                reasoning_effort: crate::usage::reasoning_effort_for_request(request),
                conversation_id: request.metadata.conversation_id.clone(),
                message_id: request.metadata.message_id.clone(),
                error_kind: Some(error_kind_from_message(error)),
            },
        );
    }
}

fn gemini_headers(api_key: &str) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-goog-api-key",
        HeaderValue::from_str(api_key).map_err(|err| format!("Invalid API key: {err}"))?,
    );
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    Ok(headers)
}

/// `{base}/models/{model}:generateContent` 或 `:streamGenerateContent?alt=sse`。
/// base_url 用户配 Gemini 原生根（如 `https://generativelanguage.googleapis.com/v1beta`）。
fn gemini_url(base_url: &str, model: &str, stream: bool) -> String {
    let base = base_url.trim_end_matches('/');
    // model 可能已带 "models/" 前缀（如 "models/gemini-3.1-flash-lite"），去重。
    let model = model.trim_start_matches("models/");
    if stream {
        format!("{base}/models/{model}:streamGenerateContent?alt=sse")
    } else {
        format!("{base}/models/{model}:generateContent")
    }
}

/// Gemini 3.x+ 才支持 `thinkingConfig.thinkingLevel`（字符串档位）；2.5 及更早只认
/// `thinkingBudget`（数值），传错会 400。保守解析模型名里 `gemini-<major>` 的主版本号，
/// 取不到（未知/非 gemini 命名）则视为不支持，回退仅 `includeThoughts`。
fn gemini_supports_thinking_level(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    let Some(rest) = lower.split("gemini-").nth(1) else {
        return false;
    };
    let major: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    major.parse::<u32>().map(|v| v >= 3).unwrap_or(false)
}

/// canonical messages → Gemini `contents[]`。Tool 结果按函数名关联（Gemini 无 call id），
/// 先扫 assistant 的 functionCall 建 `tool_call_id → name` 映射。
pub fn gemini_contents_from_generate_request(request: &GenerateRequest) -> Vec<Value> {
    let id_to_name = tool_call_id_to_name(&request.messages);
    let mut contents: Vec<Value> = Vec::new();
    for message in &request.messages {
        let role = match message.role {
            ModelRole::Assistant => "model",
            // Gemini contents 只有 user/model；Tool 结果作为 user 载体。
            ModelRole::User | ModelRole::Tool => "user",
        };
        let parts = gemini_parts_from_message(message, &id_to_name);
        if parts.is_empty() {
            continue;
        }
        contents.push(serde_json::json!({ "role": role, "parts": parts }));
    }
    merge_consecutive_gemini_roles(&mut contents);
    contents
}

fn tool_call_id_to_name(messages: &[ModelMessage]) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for message in messages {
        for part in &message.content {
            if let MessagePart::ToolCall { id, name, .. } = part {
                if !id.is_empty() {
                    map.insert(id.clone(), name.clone());
                }
            }
        }
    }
    map
}

fn gemini_parts_from_message(
    message: &ModelMessage,
    id_to_name: &std::collections::HashMap<String, String>,
) -> Vec<Value> {
    let mut parts = Vec::new();
    for part in &message.content {
        match part {
            MessagePart::Text { text } => {
                if !text.is_empty() {
                    parts.push(serde_json::json!({ "text": text }));
                }
            }
            MessagePart::Image {
                mime_type, data, ..
            } => {
                if matches!(message.role, ModelRole::User) {
                    if data.is_empty() {
                        parts.push(serde_json::json!({
                            "text": crate::chat::model::MISSING_IMAGE_PLACEHOLDER
                        }));
                    } else {
                        parts.push(serde_json::json!({
                            "inlineData": { "mimeType": mime_type, "data": data }
                        }));
                    }
                }
            }
            MessagePart::ImageUrl { url } => {
                if matches!(message.role, ModelRole::User) {
                    parts.push(serde_json::json!({ "fileData": { "fileUri": url } }));
                }
            }
            MessagePart::ToolCall {
                name,
                arguments,
                arguments_raw,
                signature,
                ..
            } => {
                if matches!(message.role, ModelRole::Assistant) {
                    let args = if arguments.is_null() {
                        serde_json::from_str(arguments_raw).unwrap_or(serde_json::json!({}))
                    } else {
                        arguments.clone()
                    };
                    let mut part = serde_json::json!({
                        "functionCall": { "name": name, "args": args }
                    });
                    // Gemini 3.x：回放 functionCall 必须带回响应给的 thoughtSignature，否则 400。
                    if let Some(sig) = signature {
                        part["thoughtSignature"] = Value::String(sig.clone());
                    }
                    parts.push(part);
                }
            }
            MessagePart::ToolResult {
                tool_call_id,
                content,
                ..
            } => {
                // Gemini 按函数名关联；用 id→name 映射还原，回退用 id 本身。
                let name = id_to_name
                    .get(tool_call_id)
                    .cloned()
                    .unwrap_or_else(|| tool_call_id.clone());
                // response 必须是 JSON 对象；字符串输出包进 { output }。
                let response = serde_json::from_str::<Value>(content)
                    .ok()
                    .filter(|v| v.is_object())
                    .unwrap_or_else(|| serde_json::json!({ "output": content }));
                parts.push(serde_json::json!({
                    "functionResponse": { "name": name, "response": response }
                }));
            }
            MessagePart::Reasoning { .. } => {
                // 思维文本回放时丢弃（thoughtSignature 未保存；对连续性影响可接受）。
            }
        }
    }
    parts
}

fn merge_consecutive_gemini_roles(contents: &mut Vec<Value>) {
    if contents.len() < 2 {
        return;
    }
    let mut i = 1;
    while i < contents.len() {
        let prev_role = contents[i - 1]
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let curr_role = contents[i]
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if prev_role == curr_role {
            let curr_parts = contents[i]
                .get("parts")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if let Some(prev) = contents[i - 1]
                .get_mut("parts")
                .and_then(|v| v.as_array_mut())
            {
                prev.extend(curr_parts);
            }
            contents.remove(i);
        } else {
            i += 1;
        }
    }
}

pub fn gemini_function_declarations(tools: &[ModelTool]) -> Vec<Value> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for tool in tools {
        let name = tool.openai_tool_name();
        if name.is_empty() || !seen.insert(name.clone()) {
            continue;
        }
        out.push(serde_json::json!({
            "name": name,
            "description": tool.description,
            "parameters": normalize_gemini_schema(tool.input_schema.clone()),
        }));
    }
    out
}

/// Gemini 只收 OpenAPI 子集：剥 JSON Schema 专有键（`$schema`/`additionalProperties`/
/// `$defs` 等），nullable `anyOf` 折成非空分支。递归处理 properties/items/组合子。
/// `required` 只允许出现在 `type: object` 上，否则 Gemini 400
///（`required: only allowed for OBJECT type`，issue #33）。
fn normalize_gemini_schema(schema: Value) -> Value {
    match schema {
        Value::Object(mut map) => {
            // nullable anyOf（[T, null]）→ 取非空分支。
            if let Some(any_of) = map.get("anyOf").and_then(|v| v.as_array()).cloned() {
                if any_of.len() == 2
                    && any_of
                        .iter()
                        .any(|it| it.get("type").and_then(|v| v.as_str()) == Some("null"))
                {
                    if let Some(non_null) = any_of
                        .iter()
                        .find(|it| it.get("type").and_then(|v| v.as_str()) != Some("null"))
                    {
                        let mut result = normalize_gemini_schema(non_null.clone());
                        if let (Some(obj), Some(desc)) =
                            (result.as_object_mut(), map.get("description"))
                        {
                            obj.insert("description".into(), desc.clone());
                        }
                        return result;
                    }
                }
            }
            // 约束型组合子（所有分支都无 type，只有 required 等纯约束）：Vertex 要求
            // anyOf 分支是带 type 的完整 schema，此形态直接剥掉，其余字段保留。
            for key in ["anyOf", "oneOf", "allOf"] {
                let constraint_only = map
                    .get(key)
                    .and_then(|v| v.as_array())
                    .is_some_and(|branches| branches.iter().all(|it| it.get("type").is_none()));
                if constraint_only {
                    map.remove(key);
                }
            }
            for key in [
                "$schema",
                "additionalProperties",
                "$defs",
                "definitions",
                "$id",
                "$ref",
                "title",
            ] {
                map.remove(key);
            }
            if let Some(props) = map.get_mut("properties").and_then(|v| v.as_object_mut()) {
                for value in props.values_mut() {
                    *value = normalize_gemini_schema(value.clone());
                }
            }
            if let Some(items) = map.get("items").cloned() {
                map.insert("items".into(), normalize_gemini_schema(items));
            }
            // 留下的 typed anyOf/oneOf/allOf 也要递归，否则分支里的非 object
            // `required` 会原样透传给 Gemini。
            for key in ["anyOf", "oneOf", "allOf"] {
                if let Some(branches) = map.get(key).and_then(|v| v.as_array()).cloned() {
                    let normalized: Vec<Value> =
                        branches.into_iter().map(normalize_gemini_schema).collect();
                    map.insert(key.into(), Value::Array(normalized));
                }
            }
            // 有 properties 却没写 type 的 MCP schema：补成 object，required 才合法。
            if map.contains_key("properties") && map.get("type").is_none() {
                map.insert("type".into(), Value::String("object".into()));
            }
            let is_object = map.get("type").and_then(|v| v.as_str()) == Some("object");
            if !is_object {
                map.remove("required");
            }
            Value::Object(map)
        }
        other => other,
    }
}

pub fn output_from_gemini_response(
    value: &Value,
    label: &str,
) -> Result<GenerateOutput, ModelError> {
    if let Some(msg) = gemini_error_message(value) {
        return Err(ModelError::new(format!("{label}: {msg}")));
    }
    let mut content_parts: Vec<String> = Vec::new();
    let mut reasoning_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<PendingToolCall> = Vec::new();
    let mut images: Vec<GeneratedImageData> = Vec::new();
    // thoughtSignature 可能在 functionCall 兄弟 part 上：先扫全 parts 取一个候选签名兜底。
    let carry_sig = gemini_candidate_signature(value);
    for part in gemini_response_parts(value) {
        if let Some(mut call) = gemini_tool_call_from_part(part) {
            if call.signature.is_none() {
                call.signature = carry_sig.clone();
            }
            tool_calls.push(call);
        } else if let Some(image) = gemini_image_from_part(part) {
            images.push(image);
        } else if let Some((text, is_thought)) = gemini_text_from_part(part) {
            if is_thought {
                reasoning_parts.push(text);
            } else {
                content_parts.push(text);
            }
        }
    }
    let finish_reason = normalize_finish_reason(
        gemini_finish_reason_str(value).as_deref().unwrap_or("stop"),
        !tool_calls.is_empty(),
    );
    let content = content_parts.join("");
    let reasoning = if reasoning_parts.is_empty() {
        None
    } else {
        Some(reasoning_parts.join(""))
    };
    let usage = gemini_usage(value);
    let provider_message = openai_compatible_message(
        &content,
        reasoning.as_deref(),
        &tool_calls,
        Some(&finish_reason),
    );
    Ok(GenerateOutput {
        text: content,
        reasoning,
        tool_calls,
        usage,
        finish_reason: Some(finish_reason),
        provider_messages: vec![provider_message],
        cancelled: false,
        web_search: web_search_from_gemini_value(value),
        images,
    })
}

/// 从 Gemini 响应 `candidates[0].groundingMetadata` 解析内置搜索痕迹：
/// `webSearchQueries[]` → 查询词；`groundingChunks[].web.{uri,title}` → 来源。
/// 全空则 None。片段响应亦同结构（末段带 groundingMetadata），故流式逐段调用累积。
fn web_search_from_gemini_value(value: &Value) -> Option<BuiltinWebSearch> {
    let grounding = value
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|cand| cand.get("groundingMetadata"))?;
    let mut result = BuiltinWebSearch::default();
    if let Some(queries) = grounding.get("webSearchQueries").and_then(Value::as_array) {
        for query in queries {
            if let Some(query) = query
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                if !result.queries.iter().any(|existing| existing == query) {
                    result.queries.push(query.to_string());
                }
            }
        }
    }
    // 块下标 → citations 下标（groundingSupports.groundingChunkIndices 引的是
    // groundingChunks 数组下标，非 web 块或重复 url 记 None）。
    let mut chunk_to_citation: Vec<Option<usize>> = Vec::new();
    if let Some(chunks) = grounding.get("groundingChunks").and_then(Value::as_array) {
        for chunk in chunks {
            let Some(web) = chunk.get("web") else {
                chunk_to_citation.push(None);
                continue;
            };
            let Some(url) = web
                .get("uri")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                chunk_to_citation.push(None);
                continue;
            };
            if result.citations.iter().any(|c| c.url == url) {
                chunk_to_citation.push(None);
                continue;
            }
            let title = web
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(url)
                .to_string();
            result.citations.push(WebCitation {
                title,
                url: url.to_string(),
                ..Default::default()
            });
            chunk_to_citation.push(Some(result.citations.len() - 1));
        }
    }
    // groundingSupports[]:segment.text 是「答案正文里支持某来源」的片段，
    // groundingChunkIndices 指向它支撑的块。把它映射成对应来源的 snippet（首个命中即止）。
    if let Some(supports) = grounding.get("groundingSupports").and_then(Value::as_array) {
        for support in supports {
            let Some(text) = support
                .get("segment")
                .and_then(|segment| segment.get("text"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let Some(indices) = support.get("groundingChunkIndices").and_then(Value::as_array)
            else {
                continue;
            };
            for index in indices.iter().filter_map(Value::as_u64) {
                let Some(Some(citation_index)) = chunk_to_citation.get(index as usize).copied()
                else {
                    continue;
                };
                if result.citations[citation_index].snippet.is_none() {
                    result.citations[citation_index].snippet = Some(text.chars().take(400).collect());
                }
            }
        }
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// 把新解析出的内置搜索片段合并进累积值（流式逐段调用；去重按 url / query 文本）。
/// 同一 url 的后到片段若带了 snippet/published_date 而累积值没有，则补全
/// （groundingSupports 可能随后续流式段才到，而 chunk 首段就建立了引用）。
fn merge_gemini_web_search(acc: &mut Option<BuiltinWebSearch>, next: Option<BuiltinWebSearch>) {
    let Some(next) = next else {
        return;
    };
    let target = acc.get_or_insert_with(BuiltinWebSearch::default);
    for query in next.queries {
        if !target.queries.iter().any(|existing| *existing == query) {
            target.queries.push(query);
        }
    }
    for citation in next.citations {
        if let Some(existing) = target.citations.iter_mut().find(|c| c.url == citation.url) {
            if existing.snippet.is_none() {
                existing.snippet = citation.snippet;
            }
            if existing.published_date.is_none() {
                existing.published_date = citation.published_date;
            }
            continue;
        }
        target.citations.push(citation);
    }
}

/// 取 `candidates[0].content.parts[]`（片段响应亦同）。
fn gemini_response_parts(value: &Value) -> impl Iterator<Item = &Value> {
    value
        .get("candidates")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|cand| cand.get("content"))
        .and_then(|content| content.get("parts"))
        .and_then(|p| p.as_array())
        .map(|v| v.iter())
        .unwrap_or_else(|| [].iter())
}

/// part 里的 functionCall → PendingToolCall（合成 id；args 是对象；捕获同 part 上的 thoughtSignature）。
fn gemini_tool_call_from_part(part: &Value) -> Option<PendingToolCall> {
    let call = part.get("functionCall")?;
    let name = call.get("name").and_then(|v| v.as_str())?.to_string();
    let args = call.get("args").cloned().unwrap_or(serde_json::json!({}));
    let arguments_raw = serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string());
    let (arguments, arguments_parse_error) = parse_tool_arguments(&arguments_raw);
    Some(PendingToolCall {
        id: format!("call_{}", uuid::Uuid::new_v4()),
        function_name: name,
        arguments,
        arguments_raw,
        arguments_parse_error,
        signature: gemini_part_thought_signature(part),
    })
}

/// part 里的 text → (文本, 是否思维)。空文本返回 None（如仅带 thoughtSignature 的占位 part）。
fn gemini_text_from_part(part: &Value) -> Option<(String, bool)> {
    let text = part.get("text").and_then(|v| v.as_str())?;
    if text.is_empty() {
        return None;
    }
    let is_thought = part
        .get("thought")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Some((text.to_string(), is_thought))
}

/// part 里的 inlineData → 模型生成的图片（base64 + mime）。非图片 part 返回 None。
/// wire 形状：`{"inlineData": {"mimeType": "image/png", "data": "<base64>"}}`。
fn gemini_image_from_part(part: &Value) -> Option<GeneratedImageData> {
    let inline = part.get("inlineData")?;
    let data = inline
        .get("data")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?;
    let mime_type = inline
        .get("mimeType")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("image/png")
        .to_string();
    Some(GeneratedImageData {
        mime_type,
        data: data.to_string(),
    })
}

/// part 上的 thoughtSignature（Gemini 3.x 思维签名，回放 functionCall 时须带回）。
fn gemini_part_thought_signature(part: &Value) -> Option<String> {
    part.get("thoughtSignature")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// 扫一个候选（或流片段）里所有 part，取第一个 thoughtSignature——签名可能不在
/// functionCall part 自身上，而在同轮兄弟 part 上，且顺序不定。
fn gemini_candidate_signature(value: &Value) -> Option<String> {
    gemini_response_parts(value).find_map(gemini_part_thought_signature)
}

fn gemini_finish_reason_str(value: &Value) -> Option<String> {
    value
        .get("candidates")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|cand| cand.get("finishReason"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Gemini finishReason → canonical。有工具调用时恒为 tool_calls（Gemini 常仍返回 STOP）。
fn normalize_finish_reason(reason: &str, has_tool_calls: bool) -> String {
    if has_tool_calls {
        return "tool_calls".to_string();
    }
    match reason {
        "STOP" => "stop",
        "MAX_TOKENS" => "length",
        other => {
            if other.eq_ignore_ascii_case("stop") {
                "stop"
            } else {
                return other.to_ascii_lowercase();
            }
        }
    }
    .to_string()
}

fn gemini_usage(value: &Value) -> Option<ModelUsage> {
    let meta = value.get("usageMetadata")?;
    let get = |key: &str| meta.get(key).and_then(|v| v.as_u64());
    Some(ModelUsage {
        input_tokens: get("promptTokenCount"),
        output_tokens: get("candidatesTokenCount"),
        total_tokens: get("totalTokenCount"),
        cached_input_tokens: get("cachedContentTokenCount"),
        cache_creation_input_tokens: None,
        reasoning_tokens: get("thoughtsTokenCount"),
        // 内置 provider 路径：窗口来自 model_metadata，不由响应携带。
        context_window_tokens: None,
    })
}

fn gemini_error_message(value: &Value) -> Option<String> {
    value
        .get("error")
        .and_then(|err| err.get("message"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn openai_compatible_message(
    text: &str,
    reasoning: Option<&str>,
    tool_calls: &[PendingToolCall],
    finish_reason: Option<&str>,
) -> Value {
    let mut message = serde_json::json!({
        "role": "assistant",
        "content": if text.is_empty() { Value::Null } else { Value::String(text.to_string()) },
    });
    if let Some(reasoning) = reasoning.map(str::trim).filter(|v| !v.is_empty()) {
        message["reasoning_content"] = Value::String(reasoning.to_string());
    }
    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(
            tool_calls
                .iter()
                .map(|call| {
                    let mut tc = serde_json::json!({
                        "id": call.id,
                        "type": "function",
                        "function": { "name": call.function_name, "arguments": call.arguments_raw }
                    });
                    // Gemini thoughtSignature：写在自定义键上，经存储/回放带回（其他 provider 忽略）。
                    if let Some(sig) = &call.signature {
                        tc["thought_signature"] = Value::String(sig.clone());
                    }
                    tc
                })
                .collect(),
        );
    }
    if let Some(finish_reason) = finish_reason {
        message["finish_reason"] = Value::String(finish_reason.to_string());
    }
    message
}

fn stream_output(
    text: String,
    reasoning: String,
    tool_calls: Vec<PendingToolCall>,
    finish_reason: String,
    usage: Option<ModelUsage>,
) -> GenerateOutput {
    let reasoning = non_empty(reasoning);
    let provider_message = openai_compatible_message(
        &text,
        reasoning.as_deref(),
        &tool_calls,
        Some(&finish_reason),
    );
    GenerateOutput {
        text,
        reasoning,
        tool_calls,
        usage,
        finish_reason: Some(finish_reason),
        provider_messages: vec![provider_message],
        cancelled: false,
        web_search: None,
        images: Vec::new(),
    }
}

fn request_label(request: &GenerateRequest, fallback: &str) -> String {
    request
        .metadata
        .label
        .trim()
        .is_empty()
        .then(|| fallback.to_string())
        .unwrap_or_else(|| request.metadata.label.clone())
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::model::GenerateOptions;
    use crate::settings::ModelInfo;

    fn provider() -> crate::settings::ModelProvider {
        crate::settings::ModelProvider {
            id: "test".into(),
            name: "Gemini".into(),
            api_keys: vec!["AIza-test".into()],
            api_key_legacy: None,
            base_url: "https://generativelanguage.googleapis.com/v1beta".into(),
            available_models: vec!["gemini-3.1-flash-lite".into()],
            enabled_models: vec!["gemini-3.1-flash-lite".into()],
            enabled: true,
            api_format: "gemini".into(),
            model_overrides: Default::default(),
            compress_request_body: false,
            request: Default::default(),
        }
    }

    fn body_for(request: &GenerateRequest, stream: bool) -> Value {
        let state = crate::state::AppState::new_headless(
            crate::settings::Settings::default(),
            std::env::temp_dir(),
        );
        let p = provider();
        GeminiProvider::new(&state, &p, 1).request_body(request, stream)
    }

    #[test]
    fn thinking_level_maps_to_thinking_level_field() {
        let base = GenerateRequest {
            model: "gemini-3.1-flash-lite".into(),
            system: String::new(),
            messages: vec![ModelMessage {
                role: ModelRole::User,
                content: vec![MessagePart::Text { text: "hi".into() }],
            }],
            tools: Vec::new(),
            options: GenerateOptions::default(),
            metadata: Default::default(),
        };
        // 无档位 ⇒ 不发 thinkingConfig。
        let none = body_for(&base, false);
        assert!(
            none["generationConfig"].get("thinkingConfig").is_none(),
            "body: {none}"
        );
        // 设 medium ⇒ thinkingConfig.thinkingLevel=medium。
        let mut req = base.clone();
        req.options.thinking_level = Some("medium".into());
        let med = body_for(&req, false);
        assert_eq!(
            med["generationConfig"]["thinkingConfig"]["thinkingLevel"], "medium",
            "body: {med}"
        );
        // 档位原样下发，适配器不再做协议级收敛：Gemini 的 thinkingLevel 只有 low/medium/high，
        // 传 max 就吃 400 —— 该不该出现 max 由模型库 `reasoningEfforts` 门控（Gemini 条目均无
        // 此字段，走 low/medium/high 兜底，故实际取不到 max）。
        let mut req_max = base.clone();
        req_max.options.thinking_level = Some("max".into());
        let mx = body_for(&req_max, false);
        assert_eq!(
            mx["generationConfig"]["thinkingConfig"]["thinkingLevel"], "max",
            "body: {mx}"
        );
        // Gemini 2.5：thinkingLevel 会 400，回退到仅 includeThoughts（非回归）。
        let mut req_25 = base.clone();
        req_25.model = "gemini-2.5-flash".into();
        req_25.options.thinking_level = Some("high".into());
        let g25 = body_for(&req_25, false);
        assert!(
            g25["generationConfig"]["thinkingConfig"]
                .get("thinkingLevel")
                .is_none(),
            "2.5 不应带 thinkingLevel: {g25}"
        );
        assert_eq!(
            g25["generationConfig"]["thinkingConfig"]["includeThoughts"], true,
            "body: {g25}"
        );
    }

    #[test]
    fn builtin_web_search_appends_google_search_tool() {
        let base = GenerateRequest {
            model: "gemini-3.1-flash-lite".into(),
            system: String::new(),
            messages: vec![ModelMessage {
                role: ModelRole::User,
                content: vec![MessagePart::Text { text: "hi".into() }],
            }],
            tools: Vec::new(),
            options: GenerateOptions::default(),
            metadata: Default::default(),
        };
        // off ⇒ 无 tools。
        let off = body_for(&base, false);
        assert!(off.get("tools").is_none(), "body: {off}");
        // on ⇒ tools 数组含 {"google_search":{}}。
        let mut req = base.clone();
        req.options.builtin_web_search = true;
        let on = body_for(&req, false);
        let tools = on["tools"].as_array().expect("tools array present");
        assert!(
            tools.iter().any(|t| t.get("google_search").is_some()),
            "body: {on}"
        );
    }

    #[test]
    fn web_search_parsed_from_grounding_metadata() {
        // groundingMetadata.webSearchQueries → 查询；groundingChunks[].web → 来源（去重）；
        // groundingSupports[].segment.text → 按 groundingChunkIndices 映射成对应来源的 snippet。
        let value = serde_json::json!({
            "candidates": [{
                "content": { "parts": [{ "text": "见来源" }] },
                "groundingMetadata": {
                    "webSearchQueries": ["吉林 天气", "吉林 天气"],
                    "groundingChunks": [
                        { "web": { "uri": "https://a.com", "title": "A 站" } },
                        { "web": { "uri": "https://a.com", "title": "dup" } },
                        { "web": { "uri": "https://b.com" } }
                    ],
                    "groundingSupports": [
                        {
                            "segment": { "text": "长春今天晴。" },
                            "groundingChunkIndices": [0, 5]
                        },
                        {
                            "segment": { "text": "来源见气象台。" },
                            "groundingChunkIndices": [2]
                        },
                        {
                            "segment": { "text": "" },
                            "groundingChunkIndices": [0]
                        }
                    ]
                }
            }]
        });
        let parsed = web_search_from_gemini_value(&value).expect("web_search present");
        assert_eq!(parsed.queries, vec!["吉林 天气".to_string()]);
        assert_eq!(parsed.citations.len(), 2);
        assert_eq!(parsed.citations[0].url, "https://a.com");
        assert_eq!(parsed.citations[0].snippet.as_deref(), Some("长春今天晴。"));
        assert_eq!(parsed.citations[1].title, "https://b.com");
        assert_eq!(parsed.citations[1].snippet.as_deref(), Some("来源见气象台。"));
        assert!(parsed.citations[0].published_date.is_none());
    }

    #[test]
    fn web_search_none_without_grounding() {
        let value = serde_json::json!({
            "candidates": [{ "content": { "parts": [{ "text": "无搜索" }] } }]
        });
        assert!(web_search_from_gemini_value(&value).is_none());
    }

    #[test]
    fn merge_gemini_web_search_accumulates_and_dedups_across_chunks() {
        // 流式逐段调用 merge：query/url 去重，跨片段累积。
        let mut acc: Option<BuiltinWebSearch> = None;
        merge_gemini_web_search(&mut acc, None); // 无 grounding 片段不建结构。
        assert!(acc.is_none());
        merge_gemini_web_search(
            &mut acc,
            Some(BuiltinWebSearch {
                queries: vec!["q1".into()],
                citations: vec![WebCitation {
                    title: "A".into(),
                    url: "https://a.com".into(),
                    ..Default::default()
                }],
            }),
        );
        // 第二段带重复 url + 重复 query + 新来源；重复 url 带 snippet 应补全进首条。
        merge_gemini_web_search(
            &mut acc,
            Some(BuiltinWebSearch {
                queries: vec!["q1".into(), "q2".into()],
                citations: vec![
                    WebCitation {
                        title: "A-dup".into(),
                        url: "https://a.com".into(),
                        snippet: Some("天气不错".into()),
                        ..Default::default()
                    },
                    WebCitation {
                        title: "B".into(),
                        url: "https://b.com".into(),
                        ..Default::default()
                    },
                ],
            }),
        );
        let merged = acc.expect("accumulated");
        assert_eq!(merged.queries, vec!["q1".to_string(), "q2".to_string()]);
        assert_eq!(merged.citations.len(), 2);
        assert_eq!(merged.citations[0].url, "https://a.com");
        assert_eq!(merged.citations[0].snippet.as_deref(), Some("天气不错"));
        assert_eq!(merged.citations[1].url, "https://b.com");
    }

    fn temperature_body(
        provider_temperature: Option<f64>,
        request_temperature: Option<f64>,
    ) -> Value {
        let state = crate::state::AppState::new_headless(
            crate::settings::Settings::default(),
            std::env::temp_dir(),
        );
        let mut p = provider();
        let model = "custom-temperature-model";
        if let Some(temperature) = provider_temperature {
            p.model_overrides.insert(
                model.into(),
                ModelInfo {
                    temperature: Some(temperature),
                    ..ModelInfo::default()
                },
            );
        }
        let request = GenerateRequest {
            model: model.into(),
            system: String::new(),
            messages: vec![ModelMessage {
                role: ModelRole::User,
                content: vec![MessagePart::Text { text: "hi".into() }],
            }],
            tools: Vec::new(),
            options: GenerateOptions {
                temperature: request_temperature,
                ..Default::default()
            },
            metadata: Default::default(),
        };
        GeminiProvider::new(&state, &p, 1).request_body(&request, false)
    }

    #[test]
    fn url_builds_generate_and_stream_and_dedupes_models_prefix() {
        let base = "https://generativelanguage.googleapis.com/v1beta/";
        assert_eq!(
            gemini_url(base, "gemini-3.1-flash-lite", false),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.1-flash-lite:generateContent"
        );
        assert_eq!(
            gemini_url(base, "models/gemini-3.1-flash-lite", true),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.1-flash-lite:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn request_body_shape_and_no_openai_specific_fields() {
        let request = GenerateRequest {
            model: "gemini-3.1-flash-lite".into(),
            system: "sys".into(),
            messages: vec![ModelMessage {
                role: ModelRole::User,
                content: vec![MessagePart::Text { text: "hi".into() }],
            }],
            tools: vec![ModelTool {
                id: "native__glob".into(),
                name: "glob".into(),
                description: "find files".into(),
                source: "native".into(),
                server_id: None,
                server_name: None,
                input_schema: serde_json::json!({
                    "type": "object",
                    "$schema": "http://json-schema.org/draft-07/schema#",
                    "additionalProperties": false,
                    "properties": { "pattern": { "type": "string" } },
                    "required": ["pattern"]
                }),
                sensitive: false,
            }],
            options: GenerateOptions::default(),
            metadata: Default::default(),
        };
        let body = body_for(&request, true);
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "sys");
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "hi");
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 8192);
        assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "AUTO");
        let decl = &body["tools"][0]["functionDeclarations"][0];
        assert_eq!(decl["name"], "glob");
        // schema 归一化：JSON Schema 专有键被剥掉。
        assert!(decl["parameters"].get("$schema").is_none());
        assert!(decl["parameters"].get("additionalProperties").is_none());
        assert_eq!(
            decl["parameters"]["properties"]["pattern"]["type"],
            "string"
        );
        // 绝不含 OpenAI 专有字段（撞 Gemini 400 的元凶）。
        assert!(body.get("promptCacheKey").is_none());
        assert!(body.get("prompt_cache_key").is_none());
        assert!(body.get("tool_choice").is_none());
        assert!(body.get("stream_options").is_none());
        assert!(body.get("model").is_none()); // model 在 URL，不在 body
    }

    #[test]
    fn request_body_omits_temperature_by_default() {
        let body = temperature_body(None, None);
        assert!(
            body["generationConfig"].get("temperature").is_none(),
            "body: {body}"
        );
    }

    #[test]
    fn request_body_uses_provider_temperature_override() {
        let body = temperature_body(Some(0.4), None);
        assert_eq!(
            body["generationConfig"]["temperature"],
            serde_json::json!(0.4),
            "body: {body}"
        );
    }

    #[test]
    fn explicit_request_temperature_wins_over_provider_override() {
        let body = temperature_body(Some(0.4), Some(1.2));
        assert_eq!(
            body["generationConfig"]["temperature"],
            serde_json::json!(1.2),
            "body: {body}"
        );
    }

    #[test]
    fn tool_round_trip_maps_functioncall_and_functionresponse_by_name() {
        let request = GenerateRequest {
            model: "gemini".into(),
            system: String::new(),
            messages: vec![
                ModelMessage {
                    role: ModelRole::Assistant,
                    content: vec![MessagePart::ToolCall {
                        id: "call_abc".into(),
                        name: "glob".into(),
                        arguments: serde_json::json!({ "pattern": "*.rs" }),
                        arguments_raw: "{\"pattern\":\"*.rs\"}".into(),
                        signature: Some("SIG123".into()),
                    }],
                },
                ModelMessage {
                    role: ModelRole::Tool,
                    content: vec![MessagePart::ToolResult {
                        tool_call_id: "call_abc".into(),
                        content: "found 5 files".into(),
                        is_error: false,
                        artifacts: Vec::new(),
                    }],
                },
            ],
            tools: Vec::new(),
            options: Default::default(),
            metadata: Default::default(),
        };
        let contents = gemini_contents_from_generate_request(&request);
        // assistant functionCall（args 为对象）
        assert_eq!(contents[0]["role"], "model");
        assert_eq!(contents[0]["parts"][0]["functionCall"]["name"], "glob");
        assert_eq!(
            contents[0]["parts"][0]["functionCall"]["args"]["pattern"],
            "*.rs"
        );
        // thoughtSignature 回放时带回（Gemini 3.x 必需）。
        assert_eq!(contents[0]["parts"][0]["thoughtSignature"], "SIG123");
        // tool functionResponse：按 call id → name 还原为 "glob"；字符串输出包成 { output }
        assert_eq!(contents[1]["role"], "user");
        let fr = &contents[1]["parts"][0]["functionResponse"];
        assert_eq!(fr["name"], "glob");
        assert_eq!(fr["response"]["output"], "found 5 files");
    }

    #[test]
    fn parses_gemini_response_text_and_finish() {
        let response = serde_json::json!({
            "candidates": [{
                "content": { "role": "model", "parts": [
                    { "text": "收到" },
                    { "text": "", "thoughtSignature": "abc" }
                ] },
                "finishReason": "STOP"
            }],
            "usageMetadata": { "promptTokenCount": 9449, "candidatesTokenCount": 7, "totalTokenCount": 9456 }
        });
        let out = output_from_gemini_response(&response, "test").expect("output");
        assert_eq!(out.text, "收到");
        assert_eq!(out.finish_reason.as_deref(), Some("stop"));
        assert!(out.tool_calls.is_empty());
        assert_eq!(out.usage.as_ref().and_then(|u| u.total_tokens), Some(9456));
        assert_eq!(out.usage.as_ref().and_then(|u| u.input_tokens), Some(9449));
    }

    #[test]
    fn parses_gemini_response_functioncall_forces_tool_calls_finish() {
        let response = serde_json::json!({
            "candidates": [{
                "content": { "role": "model", "parts": [
                    { "functionCall": { "name": "glob", "args": { "pattern": "*.rs" } } }
                ] },
                "finishReason": "STOP"  // Gemini 有工具调用仍常返回 STOP
            }]
        });
        let out = output_from_gemini_response(&response, "test").expect("output");
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].function_name, "glob");
        assert_eq!(out.tool_calls[0].arguments["pattern"], "*.rs");
        assert!(out.tool_calls[0].id.starts_with("call_"));
        // 由 functionCall 存在推导 tool_calls（而非 STOP→stop）
        assert_eq!(out.finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn normalize_strips_constraint_only_anyof() {
        // present_artifacts 形态：顶层 anyOf 各分支只有 required 无 type，Vertex 会拒。
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "artifact_ids": { "type": "array", "items": { "type": "string" } },
                "paths": { "type": "array", "items": { "type": "string" } },
                "caption": { "type": "string" }
            },
            "anyOf": [
                { "required": ["artifact_ids"] },
                { "required": ["paths"] }
            ]
        });
        let out = normalize_gemini_schema(schema);
        assert!(out.get("anyOf").is_none());
        assert_eq!(out["type"], "object");
        assert_eq!(out["properties"]["caption"]["type"], "string");
    }

    #[test]
    fn function_declarations_for_real_present_artifacts_pass_vertex_validation() {
        // 验收标准：真实 present_artifacts 工具定义经 gemini_function_declarations 后，
        // parameters 无 anyOf 且顶层带 type:object（否则 Vertex 拒整个请求）。
        let def = crate::mcp::types::native_present_artifacts_tool();
        let decls = gemini_function_declarations(&[ModelTool::from(&def)]);
        assert_eq!(decls.len(), 1);
        let params = &decls[0]["parameters"];
        assert!(params.get("anyOf").is_none());
        assert_eq!(params["type"], "object");
        assert!(params.get("additionalProperties").is_none());
    }

    #[test]
    fn normalize_strips_nested_constraint_only_combinators() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "inner": {
                    "type": "object",
                    "properties": { "a": { "type": "string" } },
                    "oneOf": [ { "required": ["a"] } ]
                }
            },
            "allOf": [ { "required": ["inner"] } ]
        });
        let out = normalize_gemini_schema(schema);
        assert!(out.get("allOf").is_none());
        assert!(out["properties"]["inner"].get("oneOf").is_none());
        assert_eq!(out["properties"]["inner"]["type"], "object");
    }

    #[test]
    fn normalize_still_collapses_nullable_anyof() {
        let schema = serde_json::json!({
            "description": "可空字符串",
            "anyOf": [ { "type": "string" }, { "type": "null" } ]
        });
        let out = normalize_gemini_schema(schema);
        assert_eq!(out["type"], "string");
        assert_eq!(out["description"], "可空字符串");
        assert!(out.get("anyOf").is_none());
    }

    #[test]
    fn normalize_keeps_typed_polymorphic_anyof() {
        // 分支带 type 的多态 anyOf：Vertex 支持，原样透传。
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "value": {
                    "anyOf": [ { "type": "string" }, { "type": "integer" } ]
                }
            }
        });
        let out = normalize_gemini_schema(schema);
        let branches = out["properties"]["value"]["anyOf"].as_array().unwrap();
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0]["type"], "string");
        assert_eq!(branches[1]["type"], "integer");
    }

    #[test]
    fn normalize_strips_required_unless_type_object() {
        // Gemini: `required` is only legal on OBJECT. MCP schemas often park it
        // on string / untyped nodes (and inside typed anyOf branches).
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "required": ["query"]
                },
                "payload": {
                    "anyOf": [
                        {
                            "type": "object",
                            "properties": { "id": { "type": "string" } },
                            "required": ["id"]
                        },
                        { "type": "string", "required": ["id"] }
                    ]
                }
            },
            "required": ["query"]
        });
        let out = normalize_gemini_schema(schema);
        assert!(out["properties"]["query"].get("required").is_none());
        assert_eq!(out["required"], serde_json::json!(["query"]));
        let branches = out["properties"]["payload"]["anyOf"].as_array().unwrap();
        assert_eq!(branches[0]["required"], serde_json::json!(["id"]));
        assert!(branches[1].get("required").is_none());
    }

    #[test]
    fn normalize_infers_object_type_when_properties_present() {
        let schema = serde_json::json!({
            "properties": { "q": { "type": "string" } },
            "required": ["q"]
        });
        let out = normalize_gemini_schema(schema);
        assert_eq!(out["type"], "object");
        assert_eq!(out["required"], serde_json::json!(["q"]));
    }

    fn image_request(model: &str) -> GenerateRequest {
        GenerateRequest {
            model: model.into(),
            system: String::new(),
            messages: vec![ModelMessage {
                role: ModelRole::User,
                content: vec![MessagePart::Text {
                    text: "画一只猫".into(),
                }],
            }],
            tools: Vec::new(),
            options: GenerateOptions::default(),
            metadata: Default::default(),
        }
    }

    #[test]
    fn response_modalities_only_for_image_model() {
        // 文本模型：请求体不含 responseModalities（红线——多余字段会撞 Gemini 400）。
        let text = body_for(&image_request("gemini-3.1-flash-lite"), false);
        assert!(
            text["generationConfig"].get("responseModalities").is_none(),
            "text model must NOT carry responseModalities: {text}"
        );
        // 出图模型：generationConfig.responseModalities = ["TEXT","IMAGE"]。
        let image = body_for(&image_request("gemini-3.1-flash-image"), false);
        assert_eq!(
            image["generationConfig"]["responseModalities"],
            serde_json::json!(["TEXT", "IMAGE"]),
            "image model must carry responseModalities: {image}"
        );
    }

    #[test]
    fn parses_gemini_response_text_and_inline_image() {
        // 非流式：text part + inlineData part → text 与 images 都解析出来。
        let response = serde_json::json!({
            "candidates": [{
                "content": { "role": "model", "parts": [
                    { "text": "这是你的猫" },
                    { "inlineData": { "mimeType": "image/png", "data": "AAAA" } }
                ] },
                "finishReason": "STOP"
            }]
        });
        let out = output_from_gemini_response(&response, "test").expect("output");
        assert_eq!(out.text, "这是你的猫");
        assert_eq!(out.images.len(), 1);
        assert_eq!(out.images[0].mime_type, "image/png");
        assert_eq!(out.images[0].data, "AAAA");
    }

    #[test]
    fn stream_emits_image_data_and_aggregates_images() {
        // 流式：mock 两个 SSE chunk（一个文本 + 一个 inlineData），断言发射 ImageData
        // 且 finish 后 output.images 聚合正确。这里直接复用解析辅助（stream_inner 需网络），
        // 逐 part 走与 stream_inner 相同的分支逻辑，验证 helper 契约。
        let chunk_text = serde_json::json!({
            "candidates": [{ "content": { "parts": [{ "text": "生成中" }] } }]
        });
        let chunk_image = serde_json::json!({
            "candidates": [{ "content": { "parts": [
                { "inlineData": { "mimeType": "image/jpeg", "data": "BBBB" } }
            ] } }]
        });

        let mut emitted: Vec<StreamPart> = Vec::new();
        let mut images: Vec<GeneratedImageData> = Vec::new();
        for value in [&chunk_text, &chunk_image] {
            for part in gemini_response_parts(value) {
                if let Some(image) = gemini_image_from_part(part) {
                    emitted.push(StreamPart::ImageData {
                        mime_type: image.mime_type.clone(),
                        data: image.data.clone(),
                    });
                    images.push(image);
                } else if let Some((text, _)) = gemini_text_from_part(part) {
                    emitted.push(StreamPart::TextDelta { delta: text });
                }
            }
        }

        // 恰好发了一帧 ImageData（mime/data 正确）。
        let image_frames: Vec<_> = emitted
            .iter()
            .filter(|part| matches!(part, StreamPart::ImageData { .. }))
            .collect();
        assert_eq!(image_frames.len(), 1);
        match image_frames[0] {
            StreamPart::ImageData { mime_type, data } => {
                assert_eq!(mime_type, "image/jpeg");
                assert_eq!(data, "BBBB");
            }
            _ => unreachable!(),
        }
        // 聚合到最终 images。
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].mime_type, "image/jpeg");
        assert_eq!(images[0].data, "BBBB");
    }
}
