use reqwest::header::{HeaderMap, HeaderValue, ACCEPT_ENCODING};
use serde_json::Value;

use crate::api::{send_with_failover, with_chat_request_timeout};
use crate::settings::ModelProvider;
use crate::state::AppState;
use crate::usage::{
    chat_usage_source_for_label, error_kind_from_message, model_usage_from_anthropic_value,
    operation_from_label, record_model_call, UsageRecordInput,
};

use super::{
    parse_tool_arguments, stream_read_error, BuiltinWebSearch, FirstTokenStreamSink,
    GenerateOutput, GenerateRequest, LanguageModelProvider, MessagePart, ModelError, ModelFuture,
    ModelMessage, ModelRole, ModelTool, ModelUsage, PendingToolCall, StreamPart, StreamSink,
    WebCitation,
};

const ANTHROPIC_VERSION: &str = "2023-06-01";
/// 1 小时缓存的 beta 开关；不带这个头时 `ttl: "1h"` 会被拒。
const EXTENDED_CACHE_TTL_BETA: &str = "extended-cache-ttl-2025-04-11";

/// 一次请求的 prompt 缓存设置。
struct PromptCache {
    long_ttl: bool,
}

impl PromptCache {
    fn value(&self) -> Value {
        if self.long_ttl {
            serde_json::json!({ "type": "ephemeral", "ttl": "1h" })
        } else {
            serde_json::json!({ "type": "ephemeral" })
        }
    }
}

/// 在请求体上打 prompt 缓存断点。用文档化的**显式断点**写法（给内容块加 `cache_control`），
/// 而不是顶层 `cache_control`——后者只在官方域名成立，兼容网关的行为没法保证。
///
/// 断点位置按前缀稳定性从前往后排：tools → system → 最后一条消息的最后一个可缓存块。
/// 前两个是每轮不变的长前缀（省钱的大头），第三个把本轮的对话历史也一起缓存进去，
/// 下一轮就能直接命中。Anthropic 上限 4 个断点，这里最多用 3 个。
fn apply_prompt_cache_breakpoints(body: &mut Value, cache_control: &Value) {
    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
        if let Some(last) = tools.last_mut().and_then(Value::as_object_mut) {
            last.insert("cache_control".to_string(), cache_control.clone());
        }
    }

    // system 平时是裸字符串，缓存要求块结构 —— 只在开缓存时改形状，关缓存的请求体保持原样。
    if let Some(system) = body.get("system").and_then(Value::as_str) {
        body["system"] = serde_json::json!([{
            "type": "text",
            "text": system,
            "cache_control": cache_control.clone(),
        }]);
    } else if let Some(blocks) = body.get_mut("system").and_then(Value::as_array_mut) {
        mark_last_cacheable_block(blocks, cache_control);
    }

    if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
        if let Some(content) = messages
            .last_mut()
            .and_then(|m| m.get_mut("content"))
            .and_then(Value::as_array_mut)
        {
            mark_last_cacheable_block(content, cache_control);
        }
    }
}

/// 给最后一个可缓存的块加断点。`thinking` 块与空 text 块不能带 cache_control。
fn mark_last_cacheable_block(blocks: &mut [Value], cache_control: &Value) {
    for block in blocks.iter_mut().rev() {
        let Some(obj) = block.as_object_mut() else {
            continue;
        };
        match obj.get("type").and_then(Value::as_str) {
            Some("thinking") | Some("redacted_thinking") => continue,
            Some("text")
                if obj
                    .get("text")
                    .and_then(Value::as_str)
                    .is_none_or(|t| t.trim().is_empty()) =>
            {
                continue
            }
            _ => {}
        }
        obj.insert("cache_control".to_string(), cache_control.clone());
        return;
    }
}

pub struct AnthropicMessagesProvider<'a> {
    state: &'a AppState,
    provider: &'a ModelProvider,
    retry_attempts: usize,
}

impl<'a> AnthropicMessagesProvider<'a> {
    pub fn new(state: &'a AppState, provider: &'a ModelProvider, retry_attempts: usize) -> Self {
        Self {
            state,
            provider,
            retry_attempts,
        }
    }
}

impl LanguageModelProvider for AnthropicMessagesProvider<'_> {
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

impl AnthropicMessagesProvider<'_> {
    async fn generate_inner(&self, request: GenerateRequest) -> Result<GenerateOutput, ModelError> {
        let label = request_label(&request, "Anthropic Messages API");
        let started_at = chrono::Local::now().timestamp();
        let started = std::time::Instant::now();
        let body = self.request_body(&request, false);
        let response = send_with_failover(
            self.state,
            &label,
            self.retry_attempts,
            &self.provider.id,
            &self.provider.api_keys,
            |key| {
                with_chat_request_timeout(crate::api::attach_json_body(
                    self.with_extra_headers(
                        self.state
                            .client_for(self.provider)
                            .post(self.messages_url())
                            .headers(anthropic_headers(key).unwrap_or_default())
                            .header(ACCEPT_ENCODING, "identity"),
                        &request.metadata,
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
        let output = output_from_anthropic_message(&value, &label)?;
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
        let label = request_label(&request, "Anthropic stream");
        let started_at = chrono::Local::now().timestamp();
        let started = std::time::Instant::now();
        let mut measured_sink = FirstTokenStreamSink::new(sink, started);
        let sink = &mut measured_sink;
        let body = self.request_body(&request, true);
        let mut response = send_with_failover(
            self.state,
            &label,
            self.retry_attempts,
            &self.provider.id,
            &self.provider.api_keys,
            |key| {
                crate::api::attach_json_body(
                    self.with_extra_headers(
                        self.state
                            .client_for(self.provider)
                            .post(self.messages_url())
                            .headers(anthropic_headers(key).unwrap_or_default())
                            .header(ACCEPT_ENCODING, "identity"),
                        &request.metadata,
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
        let mut tool_calls = Vec::new();
        let mut current_tool_id = String::new();
        let mut current_tool_name = String::new();
        let mut current_tool_input_parts: Vec<String> = Vec::new();
        let mut finish_reason = "stop".to_string();
        let mut usage: Option<ModelUsage> = None;
        // 内置搜索引用（服务端块尽力收集，随 content_block_start 完整到达）。
        let mut web_search: Option<BuiltinWebSearch> = None;

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

            while let Some(pos) = buffer.find('\n') {
                let line: String = buffer.drain(..=pos).collect();
                match parse_anthropic_sse_event(&line) {
                    Some(AnthropicSseEvent::TextDelta(text)) => {
                        full.push_str(&text);
                        sink.emit(StreamPart::TextDelta { delta: text })?;
                    }
                    Some(AnthropicSseEvent::ThinkingDelta(thinking)) => {
                        reasoning_full.push_str(&thinking);
                        sink.emit(StreamPart::ReasoningDelta { delta: thinking })?;
                    }
                    Some(AnthropicSseEvent::ToolUseStart { id, name }) => {
                        sink.emit(StreamPart::ToolCallStart {
                            id: id.clone(),
                            name: name.clone(),
                        })?;
                        current_tool_id = id;
                        current_tool_name = name;
                        current_tool_input_parts.clear();
                    }
                    Some(AnthropicSseEvent::ToolInputDelta(json)) => {
                        if !current_tool_id.is_empty() {
                            sink.emit(StreamPart::ToolCallDelta {
                                id: current_tool_id.clone(),
                                delta: json.clone(),
                            })?;
                        }
                        current_tool_input_parts.push(json);
                    }
                    Some(AnthropicSseEvent::ContentBlockStop) => {
                        if !current_tool_id.is_empty() {
                            let call = assemble_tool_call_from_stream(
                                &current_tool_id,
                                &current_tool_name,
                                &current_tool_input_parts,
                            );
                            sink.emit(StreamPart::ToolCallDone { call: call.clone() })?;
                            tool_calls.push(call);
                        }
                        current_tool_id.clear();
                        current_tool_name.clear();
                        current_tool_input_parts.clear();
                    }
                    Some(AnthropicSseEvent::WebSearch(ws)) => {
                        // 实时卡（任务 07-23）：发本次增量（server_tool_use 查询 /
                        // web_search_tool_result 来源），sink 去重累加并原地更新卡。
                        sink.emit(StreamPart::WebSearch {
                            queries: ws.queries.clone(),
                            citations: ws.citations.clone(),
                        })?;
                        merge_anthropic_web_search(&mut web_search, ws);
                    }
                    Some(AnthropicSseEvent::MessageStop) => {
                        sink.emit(StreamPart::Finish {
                            reason: finish_reason.clone(),
                            full: full.clone(),
                        })?;
                        let mut output =
                            stream_output(full, reasoning_full, tool_calls, finish_reason, usage);
                        output.web_search = web_search;
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
                        return Ok(output);
                    }
                    Some(AnthropicSseEvent::MessageStopWithReason {
                        reason,
                        usage: next_usage,
                    }) => {
                        finish_reason = finish_reason_from_anthropic_stop_reason(&reason);
                        if next_usage.is_some() {
                            usage = next_usage;
                        }
                        sink.emit(StreamPart::Finish {
                            reason: finish_reason.clone(),
                            full: full.clone(),
                        })?;
                        let mut output =
                            stream_output(full, reasoning_full, tool_calls, finish_reason, usage);
                        output.web_search = web_search;
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
                        return Ok(output);
                    }
                    Some(AnthropicSseEvent::Error(err)) => {
                        sink.emit(StreamPart::Error {
                            message: err.clone(),
                        })?;
                        return Err(ModelError::new(format!("Anthropic stream error: {err}")));
                    }
                    None => {}
                }
            }
        }

        sink.emit(StreamPart::Finish {
            reason: finish_reason.clone(),
            full: full.clone(),
        })?;
        let mut output = stream_output(full, reasoning_full, tool_calls, finish_reason, usage);
        output.web_search = web_search;
        Ok(output)
    }

    fn messages_url(&self) -> String {
        anthropic_messages_url(&self.provider.base_url)
    }

    fn request_body(&self, request: &GenerateRequest, stream: bool) -> Value {
        let mut body = serde_json::json!({
            "model": request.model,
            "messages": anthropic_messages_from_generate_request(request),
            "max_tokens": request.options.max_tokens,
        });
        if let Some(temperature) = crate::chat::model_metadata::temperature_for_request(
            request.options.temperature,
            Some(self.provider),
            &request.model,
        ) {
            body["temperature"] = serde_json::json!(temperature);
        }
        if !request.system.trim().is_empty() {
            body["system"] = Value::String(request.system.clone());
        }
        if stream {
            body["stream"] = Value::Bool(true);
        }
        let mut tools = anthropic_tools_from_model_tools(&request.tools);
        // 模型原生内置联网搜索（任务 07-23）：Anthropic 服务端工具，需组织在 Console 开启。
        if request.options.builtin_web_search {
            tools.push(serde_json::json!({
                "type": "web_search_20250305",
                "name": "web_search",
            }));
        }
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools);
        }
        // 思考等级 → adaptive thinking + `output_config.effort`，原样下发。
        // `output_config.effort` 只有 4.6+ / 5 代（Fable5、Sonnet5、Opus 4.6/4.7/4.8…）认，
        // 4.5 及更早传了会 400 —— 这些模型在模型库里是 `reasoningEfforts: []`，
        // 上游 `resolve_thinking` 已把等级抹成 None，故这里的 `if let` 自然不成立。
        // budget_tokens 已被移除（发了 400）。
        if let Some(effort) = request.options.thinking_level.as_deref() {
            body["thinking"] = serde_json::json!({ "type": "adaptive" });
            body["output_config"] = serde_json::json!({ "effort": effort });
        }
        if let Some(overrides) = request.options.provider_options.as_object() {
            for (key, value) in overrides {
                body[key] = value.clone();
            }
        }
        // prompt caching 放在最后：断点必须打在最终 body 上，否则 provider_options
        // 覆盖掉 system/tools 时断点就落在被丢弃的旧内容上了。
        if let Some(cache) = self.prompt_cache_control(&request.metadata) {
            apply_prompt_cache_breakpoints(&mut body, &cache.value());
        }
        body
    }

    /// 该供应商本次是否要打 prompt 缓存断点。
    ///
    /// 除了开关，还要求本次请求属于某个**会话**（有 `conversation_id`）。翻译器 / 截图翻译 /
    /// Lens / 上下文压缩 / 标题总结走的是同一个生成入口但都是一次性调用，给它们打断点等于
    /// 按 1.25× 写一份下次内容全变、永远读不到的缓存 —— 纯加价。OpenAI 侧靠「没有会话 id 就
    /// 不发 prompt_cache_key」天然排除了这些路径，这里对齐同一条门。
    fn prompt_cache_control(
        &self,
        metadata: &crate::chat::model::RequestMetadata,
    ) -> Option<PromptCache> {
        if !self.provider.prompt_caching_enabled() {
            return None;
        }
        metadata
            .conversation_id
            .as_deref()
            .filter(|id| !id.is_empty())?;
        // 对齐 pi：long → ttl:1h（默认 supportsLongCacheRetention=true，不做主机名启发式）。
        let long_ttl = matches!(
            self.provider.cache_retention(),
            crate::settings::CacheRetention::Long
        );
        Some(PromptCache { long_ttl })
    }

    /// 供应商「请求配置」带来的附加头（CLI 身份 / 自定义头）+ prompt 缓存的 beta 头。
    /// 发送路径与请求调试面板共用，杜绝「面板显示的和实际发的不一致」。
    fn extra_header_pairs(
        &self,
        metadata: &crate::chat::model::RequestMetadata,
    ) -> Vec<(String, String)> {
        let mut pairs = crate::provider_request::header_pairs(
            self.provider,
            metadata.conversation_id.as_deref(),
        );
        // 1 小时缓存是 beta 能力，必须显式声明才生效。anthropic-beta 不是保留头（用户可能
        // 要开别的 beta），所以这里得跟用户填的那条合并成一行 —— 发两行的话调试面板（BTreeMap）
        // 只显示一条，就和实际发出去的对不上了。
        if self
            .prompt_cache_control(metadata)
            .is_some_and(|c| c.long_ttl)
        {
            let existing = pairs
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("anthropic-beta"))
                .map(|(_, value)| value.clone());
            let merged = match existing {
                Some(value)
                    if value
                        .split(',')
                        .any(|v| v.trim() == EXTENDED_CACHE_TTL_BETA) =>
                {
                    value
                }
                Some(value) => format!("{value}, {EXTENDED_CACHE_TTL_BETA}"),
                None => EXTENDED_CACHE_TTL_BETA.to_string(),
            };
            crate::provider_request::upsert_pair(&mut pairs, "anthropic-beta".to_string(), merged);
        }
        pairs
    }

    /// 把 `extra_header_pairs` 贴到请求上。
    fn with_extra_headers(
        &self,
        request: reqwest::RequestBuilder,
        metadata: &crate::chat::model::RequestMetadata,
    ) -> reqwest::RequestBuilder {
        let mut request = request;
        for (name, value) in self.extra_header_pairs(metadata) {
            request = request.header(name, value);
        }
        request
    }

    /// 重建本次请求实际会带的 headers（脱敏后）供请求调试面板展示。镜像 `anthropic_headers`
    /// （x-api-key / anthropic-version / content-type）+ 发送路径另加的 Accept-Encoding
    /// 与 `extra_header_pairs`。x-api-key 用首个 key（正常发送用的也是它）派生脱敏预览。
    fn debug_request_headers(
        &self,
        metadata: &crate::chat::model::RequestMetadata,
    ) -> std::collections::BTreeMap<String, String> {
        let mut headers = std::collections::BTreeMap::new();
        if let Some(key) = self.provider.api_keys.first() {
            headers.insert("x-api-key".to_string(), key.clone());
        }
        headers.insert(
            "anthropic-version".to_string(),
            ANTHROPIC_VERSION.to_string(),
        );
        headers.insert("content-type".to_string(), "application/json".to_string());
        headers.insert("Accept-Encoding".to_string(), "identity".to_string());
        for (name, value) in self.extra_header_pairs(metadata) {
            headers.insert(name, value);
        }
        crate::chat::request_debug::sanitize_headers(headers)
    }

    /// 记录一次成功调用到请求调试缓冲。开关关时首行短路（零开销）。
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
                url: self.messages_url(),
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

    /// 记录一次失败调用到请求调试缓冲。开关关时首行短路（零开销）。
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
                url: self.messages_url(),
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

fn anthropic_headers(api_key: &str) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-api-key",
        HeaderValue::from_str(api_key).map_err(|err| format!("Invalid API key: {err}"))?,
    );
    headers.insert(
        "anthropic-version",
        HeaderValue::from_static(ANTHROPIC_VERSION),
    );
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    Ok(headers)
}

fn anthropic_messages_url(base_url: &str) -> String {
    format!("{}/messages", base_url.trim_end_matches('/'))
}

pub fn anthropic_messages_from_generate_request(request: &GenerateRequest) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();
    for message in &request.messages {
        match message.role {
            ModelRole::User => messages.push(serde_json::json!({
                "role": "user",
                "content": anthropic_content_blocks(message, ModelRole::User),
            })),
            ModelRole::Assistant => messages.push(serde_json::json!({
                "role": "assistant",
                "content": anthropic_content_blocks(message, ModelRole::Assistant),
            })),
            ModelRole::Tool => messages.push(serde_json::json!({
                "role": "user",
                "content": anthropic_content_blocks(message, ModelRole::Tool),
            })),
        }
    }
    merge_consecutive_anthropic_roles(&mut messages);
    ensure_tool_result_pairing(&mut messages);
    messages
}

/// Anthropic 强约束:每个 assistant `tool_use` 的**下一条**消息必须含配对 `tool_result`,
/// 且 `tool_result` 必须引用紧邻前一条 assistant 的 `tool_use`。历史 replay 存档可能因某回合
/// 被中断(停止/报错/崩溃/工具 pending)而留下未闭合的 `tool_use`(OpenAI 兼容端宽容,Anthropic 会 400)。
/// 这里在发送前兜底:给缺失结果的 `tool_use` 注入合成中断态结果,并丢弃无前置 `tool_use` 的反向孤儿 `tool_result`。
/// ponytail: 只处理"assistant→其后 user"这一相邻对;无前置 assistant 的游离 tool_result 属更深层损坏,不在此覆盖。
fn ensure_tool_result_pairing(messages: &mut Vec<Value>) {
    let is_role = |m: &Value, role: &str| m.get("role").and_then(Value::as_str) == Some(role);
    let synthetic = |id: &str| {
        serde_json::json!({
            "type": "tool_result",
            "tool_use_id": id,
            "content": "Tool call was interrupted; no result was recorded.",
            "is_error": true,
        })
    };

    let mut i = 0;
    while i < messages.len() {
        if !is_role(&messages[i], "assistant") {
            i += 1;
            continue;
        }
        let tool_use_ids: Vec<String> = messages[i]
            .get("content")
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
                    .filter_map(|b| b.get("id").and_then(Value::as_str).map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if tool_use_ids.is_empty() {
            i += 1;
            continue;
        }

        if messages.get(i + 1).map(|m| is_role(m, "user")) != Some(true) {
            // 下一条不存在或不是 user:插入一条含全部合成结果的 user 消息。
            let blocks: Vec<Value> = tool_use_ids.iter().map(|id| synthetic(id)).collect();
            messages.insert(
                i + 1,
                serde_json::json!({ "role": "user", "content": blocks }),
            );
            i += 2;
            continue;
        }

        // 重排下一条 user:先放已配对的 tool_result(原序),补齐缺失的合成结果,再接非 tool_result 块。
        let existing = messages[i + 1]
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let is_tool_result =
            |b: &Value| b.get("type").and_then(Value::as_str) == Some("tool_result");
        let matched_ids: std::collections::HashSet<&str> = existing
            .iter()
            .filter(|b| is_tool_result(b))
            .filter_map(|b| b.get("tool_use_id").and_then(Value::as_str))
            .filter(|id| tool_use_ids.iter().any(|expected| expected == id))
            .collect();

        let mut rebuilt: Vec<Value> = Vec::with_capacity(existing.len() + tool_use_ids.len());
        // 已配对的结果(丢弃反向孤儿:tool_use_id 不在本 assistant tool_use 集合内)。
        for block in existing.iter().filter(|b| is_tool_result(b)) {
            if block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .map(|id| tool_use_ids.iter().any(|expected| expected == id))
                .unwrap_or(false)
            {
                rebuilt.push(block.clone());
            }
        }
        // 缺失结果补合成占位(保持 tool_use 顺序)。
        for id in &tool_use_ids {
            if !matched_ids.contains(id.as_str()) {
                rebuilt.push(synthetic(id));
            }
        }
        // 非 tool_result 块(text/image 等)原序接在后面。
        rebuilt.extend(existing.into_iter().filter(|b| !is_tool_result(b)));

        messages[i + 1]["content"] = Value::Array(rebuilt);
        i += 2;
    }
}

pub fn anthropic_tools_from_model_tools(tools: &[ModelTool]) -> Vec<Value> {
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
            "input_schema": normalize_anthropic_schema(tool.input_schema.clone()),
        }));
    }
    out
}

pub fn output_from_anthropic_message(
    value: &Value,
    label: &str,
) -> Result<GenerateOutput, ModelError> {
    if let Some(error) = value.get("error") {
        let msg = error
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("Unknown Anthropic error");
        return Err(ModelError::new(format!("{label}: {msg}")));
    }

    let parsed = parse_anthropic_response(value);
    let finish_reason = parsed.finish_reason.clone();
    let usage = anthropic_usage(value);
    let provider_message = openai_compatible_message(
        &parsed.content,
        parsed.reasoning.as_deref(),
        &parsed.tool_calls,
        Some(&finish_reason),
    );
    Ok(GenerateOutput {
        text: parsed.content,
        reasoning: parsed.reasoning,
        tool_calls: parsed.tool_calls,
        usage,
        finish_reason: Some(finish_reason),
        provider_messages: vec![provider_message],
        cancelled: false,
        web_search: parsed.web_search,
        images: Vec::new(),
    })
}

struct AnthropicParsedResponse {
    content: String,
    reasoning: Option<String>,
    tool_calls: Vec<PendingToolCall>,
    finish_reason: String,
    web_search: Option<BuiltinWebSearch>,
}

/// 从一个 `web_search_tool_result` block 的 `content[]` 收集来源到累积值（去重按 url）。
/// 每项形如 `{type:"web_search_result", url, title}`。尽力而为，缺字段即跳过。
fn collect_anthropic_web_search_results(block: &Value, acc: &mut BuiltinWebSearch) {
    let Some(results) = block.get("content").and_then(Value::as_array) else {
        return;
    };
    for item in results {
        let Some(url) = item
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        if acc.citations.iter().any(|c| c.url == url) {
            continue;
        }
        let title = item
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(url)
            .to_string();
        acc.citations.push(WebCitation {
            title,
            url: url.to_string(),
            ..Default::default()
        });
    }
}

fn parse_anthropic_response(response: &Value) -> AnthropicParsedResponse {
    let mut content_parts = Vec::new();
    let mut reasoning_parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut web_search = BuiltinWebSearch::default();

    if let Some(blocks) = response.get("content").and_then(|value| value.as_array()) {
        for block in blocks {
            match block
                .get("type")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
            {
                "text" => {
                    if let Some(text) = block
                        .get("text")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.is_empty())
                    {
                        content_parts.push(text.to_string());
                    }
                }
                "thinking" => {
                    if let Some(thinking) = block
                        .get("thinking")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.is_empty())
                    {
                        reasoning_parts.push(thinking.to_string());
                    }
                }
                // 内置搜索：服务端工具调用（`server_tool_use`）带查询词，结果块
                // （`web_search_tool_result`）带来源。二者都由 Anthropic 服务端产生，
                // 不进 tool_calls（那是客户端函数工具），只收进 web_search 供卡片可视化。
                "server_tool_use" => {
                    if block.get("name").and_then(Value::as_str) == Some("web_search") {
                        if let Some(query) = block
                            .get("input")
                            .and_then(|input| input.get("query"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                        {
                            if !web_search.queries.iter().any(|q| q == query) {
                                web_search.queries.push(query.to_string());
                            }
                        }
                    }
                }
                "web_search_tool_result" => {
                    collect_anthropic_web_search_results(block, &mut web_search);
                }
                "tool_use" => {
                    let id = block
                        .get("id")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                    let arguments_raw = if input.is_null() {
                        "{}".to_string()
                    } else {
                        serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string())
                    };
                    tool_calls.push(PendingToolCall {
                        id,
                        function_name: name,
                        arguments: if input.is_null() {
                            serde_json::json!({})
                        } else {
                            input
                        },
                        arguments_raw,
                        arguments_parse_error: None,
                        signature: None,
                    });
                }
                _ => {}
            }
        }
    }

    let stop_reason = response
        .get("stop_reason")
        .and_then(|value| value.as_str())
        .unwrap_or("end_turn");

    AnthropicParsedResponse {
        content: content_parts.join("\n\n"),
        reasoning: if reasoning_parts.is_empty() {
            None
        } else {
            Some(reasoning_parts.join("\n\n"))
        },
        tool_calls,
        finish_reason: finish_reason_from_anthropic_stop_reason(stop_reason),
        web_search: if web_search.is_empty() {
            None
        } else {
            Some(web_search)
        },
    }
}

enum AnthropicSseEvent {
    TextDelta(String),
    ThinkingDelta(String),
    ToolUseStart {
        id: String,
        name: String,
    },
    ToolInputDelta(String),
    ContentBlockStop,
    /// 内置搜索的服务端块（`server_tool_use` 查询 / `web_search_tool_result` 结果），
    /// 完整随 `content_block_start` 到达，尽力收集来源；解析不到即无此事件。
    WebSearch(BuiltinWebSearch),
    MessageStop,
    MessageStopWithReason {
        reason: String,
        usage: Option<ModelUsage>,
    },
    Error(String),
}

fn parse_anthropic_sse_event(line: &str) -> Option<AnthropicSseEvent> {
    let line = line.trim();
    if !line.starts_with("data:") {
        return None;
    }
    let data = line.trim_start_matches("data:").trim();
    if data.is_empty() {
        return None;
    }
    let value: Value = serde_json::from_str(data).ok()?;
    match value
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
    {
        "content_block_start" => {
            let block = value.get("content_block")?;
            match block.get("type").and_then(|value| value.as_str()) {
                Some("tool_use") => Some(AnthropicSseEvent::ToolUseStart {
                    id: block
                        .get("id")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    name: block
                        .get("name")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                }),
                // 内置搜索服务端块：查询词（server_tool_use.input.query）或结果（web_search_tool_result）。
                Some("server_tool_use") => {
                    if block.get("name").and_then(Value::as_str) != Some("web_search") {
                        return None;
                    }
                    let query = block
                        .get("input")
                        .and_then(|input| input.get("query"))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?;
                    Some(AnthropicSseEvent::WebSearch(BuiltinWebSearch {
                        queries: vec![query.to_string()],
                        citations: Vec::new(),
                    }))
                }
                Some("web_search_tool_result") => {
                    let mut web_search = BuiltinWebSearch::default();
                    collect_anthropic_web_search_results(block, &mut web_search);
                    if web_search.is_empty() {
                        None
                    } else {
                        Some(AnthropicSseEvent::WebSearch(web_search))
                    }
                }
                _ => None,
            }
        }
        "content_block_delta" => {
            let delta = value.get("delta")?;
            match delta
                .get("type")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
            {
                "text_delta" => delta
                    .get("text")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .map(|text| AnthropicSseEvent::TextDelta(text.to_string())),
                "thinking_delta" => delta
                    .get("thinking")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .map(|thinking| AnthropicSseEvent::ThinkingDelta(thinking.to_string())),
                "input_json_delta" => Some(AnthropicSseEvent::ToolInputDelta(
                    delta
                        .get("partial_json")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                )),
                _ => None,
            }
        }
        "content_block_stop" => Some(AnthropicSseEvent::ContentBlockStop),
        "message_delta" => {
            let reason = value
                .get("delta")
                .and_then(|delta| delta.get("stop_reason"))
                .and_then(|value| value.as_str())
                .unwrap_or("end_turn")
                .to_string();
            Some(AnthropicSseEvent::MessageStopWithReason {
                reason,
                usage: model_usage_from_anthropic_value(&value),
            })
        }
        "message_stop" => Some(AnthropicSseEvent::MessageStop),
        "error" => Some(AnthropicSseEvent::Error(
            value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(|value| value.as_str())
                .unwrap_or("Unknown Anthropic error")
                .to_string(),
        )),
        _ => None,
    }
}

fn assemble_tool_call_from_stream(
    id: &str,
    name: &str,
    input_json_parts: &[String],
) -> PendingToolCall {
    let raw = input_json_parts.join("");
    let arguments_raw = if raw.trim().is_empty() {
        "{}".to_string()
    } else {
        raw
    };
    let (arguments, arguments_parse_error) = parse_tool_arguments(&arguments_raw);
    PendingToolCall {
        id: id.to_string(),
        function_name: name.to_string(),
        arguments,
        arguments_raw,
        arguments_parse_error,
        signature: None,
    }
}

fn anthropic_content_blocks(message: &ModelMessage, role: ModelRole) -> Vec<Value> {
    let mut blocks = Vec::new();
    for part in &message.content {
        match part {
            MessagePart::Text { text } => blocks.push(serde_json::json!({
                "type": "text",
                "text": text,
            })),
            MessagePart::Image {
                mime_type, data, ..
            } => {
                if matches!(role, ModelRole::User) {
                    if data.is_empty() {
                        blocks.push(serde_json::json!({
                            "type": "text",
                            "text": crate::chat::model::MISSING_IMAGE_PLACEHOLDER,
                        }));
                    } else {
                        blocks.push(serde_json::json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": mime_type,
                                "data": data,
                            }
                        }));
                    }
                }
            }
            MessagePart::ImageUrl { url } => {
                if matches!(role, ModelRole::User) {
                    blocks.push(serde_json::json!({
                        "type": "image",
                        "source": { "type": "url", "url": url },
                    }));
                }
            }
            MessagePart::ToolCall {
                id,
                name,
                arguments,
                arguments_raw,
                ..
            } => {
                if matches!(role, ModelRole::Assistant) {
                    let input = if arguments.is_null() {
                        serde_json::from_str(arguments_raw).unwrap_or(Value::Null)
                    } else {
                        arguments.clone()
                    };
                    blocks.push(serde_json::json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": input,
                    }));
                }
            }
            MessagePart::ToolResult {
                tool_call_id,
                content,
                is_error,
                ..
            } => {
                blocks.push(serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": tool_call_id,
                    "content": content,
                    "is_error": is_error,
                }));
            }
            MessagePart::Reasoning { text } => {
                if matches!(role, ModelRole::Assistant) {
                    blocks.push(serde_json::json!({
                        "type": "thinking",
                        "thinking": text,
                    }));
                }
            }
        }
    }
    if blocks.is_empty() {
        blocks.push(serde_json::json!({
            "type": "text",
            "text": "",
        }));
    }
    blocks
}

fn merge_consecutive_anthropic_roles(messages: &mut Vec<Value>) {
    if messages.len() < 2 {
        return;
    }
    let mut i = 1;
    while i < messages.len() {
        let prev_role = messages[i - 1]
            .get("role")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let curr_role = messages[i]
            .get("role")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if prev_role == curr_role {
            let curr_content = messages[i]
                .get("content")
                .and_then(|value| value.as_array())
                .cloned()
                .unwrap_or_default();
            if let Some(prev) = messages[i - 1]
                .get_mut("content")
                .and_then(|value| value.as_array_mut())
            {
                prev.extend(curr_content);
            }
            messages.remove(i);
        } else {
            i += 1;
        }
    }
}

fn normalize_anthropic_schema(schema: Value) -> Value {
    if let Some(any_of) = schema.get("anyOf").and_then(|value| value.as_array()) {
        if any_of.len() == 2 {
            let has_null = any_of
                .iter()
                .any(|item| item.get("type").and_then(|value| value.as_str()) == Some("null"));
            if has_null {
                if let Some(non_null) = any_of
                    .iter()
                    .find(|item| item.get("type").and_then(|value| value.as_str()) != Some("null"))
                {
                    let mut result = non_null.clone();
                    if let Some(description) = schema.get("description") {
                        result["description"] = description.clone();
                    }
                    return result;
                }
            }
        }
    }

    if schema.get("type").and_then(|value| value.as_str()) == Some("object") {
        let mut result = schema.clone();
        if let Some(obj) = result.as_object_mut() {
            // Anthropic/Bedrock reject oneOf/allOf/anyOf at the top level of a tool
            // input_schema (even alongside `type: object`). The constraints they encode
            // (e.g. present_artifacts' "require one of artifact_ids/paths") can't be
            // expressed here anyway — drop them; tool execution still validates.
            obj.remove("oneOf");
            obj.remove("allOf");
            obj.remove("anyOf");
            // Anthropic requires a properties map even when empty.
            obj.entry("properties")
                .or_insert_with(|| serde_json::json!({}));
        }
        return result;
    }

    schema
}

fn anthropic_usage(value: &Value) -> Option<ModelUsage> {
    model_usage_from_anthropic_value(value)
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
    if let Some(reasoning) = reasoning.map(str::trim).filter(|value| !value.is_empty()) {
        message["reasoning_content"] = Value::String(reasoning.to_string());
    }
    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(
            tool_calls
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

/// 把流式逐块解析出的内置搜索片段合并进累积值（去重按 url / query 文本）。
fn merge_anthropic_web_search(acc: &mut Option<BuiltinWebSearch>, next: BuiltinWebSearch) {
    let target = acc.get_or_insert_with(BuiltinWebSearch::default);
    for query in next.queries {
        if !target.queries.iter().any(|existing| *existing == query) {
            target.queries.push(query);
        }
    }
    for citation in next.citations {
        if !target.citations.iter().any(|c| c.url == citation.url) {
            target.citations.push(citation);
        }
    }
}

fn finish_reason_from_anthropic_stop_reason(reason: &str) -> String {
    match reason {
        "end_turn" => "stop",
        "tool_use" => "tool_calls",
        "max_tokens" => "length",
        _ => "stop",
    }
    .to_string()
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

    #[test]
    fn strips_top_level_combinators_from_object_schema() {
        // present_artifacts-style schema: object + top-level anyOf. Anthropic rejects
        // the combinator; normalize must drop it and keep the object usable.
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "paths": { "type": "array" } },
            "anyOf": [{ "required": ["artifact_ids"] }, { "required": ["paths"] }],
            "additionalProperties": false
        });
        let out = normalize_anthropic_schema(schema);
        assert!(
            out.get("anyOf").is_none(),
            "top-level anyOf must be dropped"
        );
        assert!(out.get("oneOf").is_none());
        assert!(out.get("allOf").is_none());
        assert!(out.get("properties").is_some());
        assert_eq!(out["additionalProperties"], serde_json::json!(false));
    }

    #[test]
    fn object_without_properties_gets_empty_map() {
        let out = normalize_anthropic_schema(serde_json::json!({ "type": "object" }));
        assert_eq!(out["properties"], serde_json::json!({}));
    }

    #[test]
    fn tool_result_and_following_image_user_merge_into_one_turn() {
        // `read` appends an image as a separate user message after the tool
        // result. Both Tool and User roles serialize to Anthropic "user", so
        // the merge must fold tool_result + image into a single user turn.
        let mut messages = vec![
            serde_json::json!({
                "role": "user",
                "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "read it" }]
            }),
            serde_json::json!({
                "role": "user",
                "content": [{
                    "type": "image",
                    "source": { "type": "base64", "media_type": "image/png", "data": "AAAA" }
                }]
            }),
        ];
        merge_consecutive_anthropic_roles(&mut messages);

        assert_eq!(messages.len(), 1, "consecutive user turns must merge");
        let blocks = messages[0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "tool_result");
        assert_eq!(blocks[1]["type"], "image");
    }

    #[test]
    fn dangling_tool_use_gets_synthetic_result_injected() {
        // A prior interrupted turn left an assistant tool_use with no paired
        // tool_result in the next message — Anthropic 400s on this.
        let mut messages = vec![
            serde_json::json!({
                "role": "assistant",
                "content": [{ "type": "tool_use", "id": "toolu_x", "name": "read", "input": {} }]
            }),
            serde_json::json!({ "role": "user", "content": [{ "type": "text", "text": "and then?" }] }),
        ];
        ensure_tool_result_pairing(&mut messages);

        // The synthetic result is injected into the following user turn, before its text.
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["role"], "user");
        let injected = messages[1]["content"].as_array().unwrap();
        assert_eq!(injected[0]["type"], "tool_result");
        assert_eq!(injected[0]["tool_use_id"], "toolu_x");
        assert_eq!(injected[0]["is_error"], true);
        assert!(
            injected.iter().any(|b| b["type"] == "text"),
            "user text preserved"
        );
    }

    #[test]
    fn missing_result_appended_reverse_orphan_dropped() {
        // Two tool_use ids; next user has a result for one plus a stray result
        // referencing an unrelated id (reverse orphan) and a text block.
        let mut messages = vec![
            serde_json::json!({
                "role": "assistant",
                "content": [
                    { "type": "tool_use", "id": "a", "name": "read", "input": {} },
                    { "type": "tool_use", "id": "b", "name": "read", "input": {} }
                ]
            }),
            serde_json::json!({
                "role": "user",
                "content": [
                    { "type": "tool_result", "tool_use_id": "a", "content": "ok" },
                    { "type": "tool_result", "tool_use_id": "ghost", "content": "stray" },
                    { "type": "text", "text": "follow" }
                ]
            }),
        ];
        ensure_tool_result_pairing(&mut messages);

        assert_eq!(messages.len(), 2);
        let blocks = messages[1]["content"].as_array().unwrap();
        let result_ids: Vec<&str> = blocks
            .iter()
            .filter(|b| b["type"] == "tool_result")
            .map(|b| b["tool_use_id"].as_str().unwrap())
            .collect();
        assert_eq!(result_ids, vec!["a", "b"], "ghost dropped, b synthesized");
        // Non-tool_result content preserved.
        assert!(blocks.iter().any(|b| b["type"] == "text"));
    }

    #[test]
    fn trailing_tool_use_gets_new_user_turn() {
        // Dangling tool_use as the very last message (no following turn at all).
        let mut messages = vec![serde_json::json!({
            "role": "assistant",
            "content": [{ "type": "tool_use", "id": "z", "name": "read", "input": {} }]
        })];
        ensure_tool_result_pairing(&mut messages);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"][0]["tool_use_id"], "z");
    }

    #[test]
    fn end_to_end_openai_dangling_history_becomes_paired_wire() {
        // Real entry point: an OpenAI-format history where an assistant tool_call
        // was never answered (interrupted turn), followed by the next user prompt.
        use crate::chat::model::types::generate_request_from_openai_messages;
        let openai = vec![
            serde_json::json!({ "role": "user", "content": "hi" }),
            serde_json::json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "toolu_bdrk_dangling",
                    "type": "function",
                    "function": { "name": "read", "arguments": "{}" }
                }]
            }),
            serde_json::json!({ "role": "user", "content": "never mind, do this instead" }),
        ];
        let request = generate_request_from_openai_messages(
            "claude-opus-4-8",
            openai,
            None,
            Default::default(),
            "test",
            Default::default(),
        );
        let wire = anthropic_messages_from_generate_request(&request);

        // Every tool_use id must have a tool_result in the immediately following turn.
        for (i, msg) in wire.iter().enumerate() {
            let Some(blocks) = msg["content"].as_array() else {
                continue;
            };
            for block in blocks {
                if block["type"] == "tool_use" {
                    let id = block["id"].as_str().unwrap();
                    let next = &wire[i + 1]["content"];
                    let paired = next
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|b| b["type"] == "tool_result" && b["tool_use_id"] == id);
                    assert!(paired, "tool_use {id} must be paired in the next turn");
                }
            }
        }
    }

    /// Build a real Anthropic request body via the production `request_body`
    /// path and assert how `thinking_level` maps to the wire.
    fn build_anthropic_body(
        thinking_level: Option<&str>,
        provider_temperature: Option<f64>,
        request_temperature: Option<f64>,
    ) -> Value {
        let state = crate::state::AppState::new_headless(
            crate::settings::Settings::default(),
            std::env::temp_dir(),
        );
        let mut model_overrides = std::collections::HashMap::new();
        if let Some(temperature) = provider_temperature {
            model_overrides.insert(
                "claude-opus-4-8".into(),
                ModelInfo {
                    temperature: Some(temperature),
                    ..ModelInfo::default()
                },
            );
        }
        let provider = crate::settings::ModelProvider {
            id: "test".into(),
            name: "Test".into(),
            api_keys: vec!["sk-test".into()],
            api_key_legacy: None,
            base_url: "https://api.anthropic.com".into(),
            available_models: vec!["claude-opus-4-8".into()],
            enabled_models: vec!["claude-opus-4-8".into()],
            enabled: true,
            api_format: "anthropic_messages".into(),
            model_overrides,
            compress_request_body: false,
            request: Default::default(),
        };
        let adapter = AnthropicMessagesProvider::new(&state, &provider, 1);
        let request = GenerateRequest {
            model: "claude-opus-4-8".into(),
            system: "sys".into(),
            messages: vec![ModelMessage {
                role: ModelRole::User,
                content: vec![MessagePart::Text { text: "hi".into() }],
            }],
            tools: Vec::new(),
            options: GenerateOptions {
                temperature: request_temperature,
                thinking_level: thinking_level.map(|s| s.to_string()),
                ..Default::default()
            },
            metadata: Default::default(),
        };
        adapter.request_body(&request, false)
    }

    #[test]
    fn thinking_level_maps_to_adaptive_effort() {
        // 未设等级 → 不发 thinking / output_config（与改动前一致：Anthropic 默认不思考）。
        let none = build_anthropic_body(None, None, None);
        assert!(none.get("thinking").is_none(), "body: {none}");
        assert!(none.get("output_config").is_none(), "body: {none}");

        // 选了等级 → adaptive thinking + output_config.effort（4.6+ 正确写法，非 budget_tokens）。
        let high = build_anthropic_body(Some("high"), None, None);
        eprintln!("[anthropic effort=high] {high}");
        assert_eq!(high["thinking"]["type"], "adaptive");
        assert_eq!(high["output_config"]["effort"], "high");
        assert!(
            high.get("budget_tokens").is_none(),
            "must not send budget_tokens"
        );
    }

    #[test]
    fn request_body_temperature_is_optional_and_model_scoped() {
        let default_body = build_anthropic_body(None, None, None);
        assert!(default_body.get("temperature").is_none());

        let configured_body = build_anthropic_body(None, Some(0.4), None);
        assert_eq!(configured_body["temperature"], serde_json::json!(0.4));

        let explicit_body = build_anthropic_body(None, Some(0.4), Some(1.2));
        assert_eq!(explicit_body["temperature"], serde_json::json!(1.2));
    }

    #[test]
    fn builtin_web_search_appends_server_tool() {
        let state = crate::state::AppState::new_headless(
            crate::settings::Settings::default(),
            std::env::temp_dir(),
        );
        let provider = crate::settings::ModelProvider {
            id: "test".into(),
            name: "Test".into(),
            api_keys: vec!["sk-test".into()],
            api_key_legacy: None,
            base_url: "https://api.anthropic.com".into(),
            available_models: vec!["claude-opus-4-8".into()],
            enabled_models: vec!["claude-opus-4-8".into()],
            enabled: true,
            api_format: "anthropic_messages".into(),
            model_overrides: Default::default(),
            compress_request_body: false,
            request: Default::default(),
        };
        let adapter = AnthropicMessagesProvider::new(&state, &provider, 1);
        let base = GenerateRequest {
            model: "claude-opus-4-8".into(),
            system: String::new(),
            messages: vec![ModelMessage {
                role: ModelRole::User,
                content: vec![MessagePart::Text { text: "hi".into() }],
            }],
            tools: Vec::new(),
            options: GenerateOptions::default(),
            metadata: Default::default(),
        };
        // 默认（off）：无客户端工具且未开内置 ⇒ 不发 tools。
        let off = adapter.request_body(&base, false);
        assert!(off.get("tools").is_none(), "body: {off}");
        // 开内置 ⇒ tools 含 web_search_20250305 服务端工具。
        let mut req = base.clone();
        req.options.builtin_web_search = true;
        let on = adapter.request_body(&req, false);
        let tools = on["tools"].as_array().expect("tools array present");
        assert!(
            tools
                .iter()
                .any(|t| t["type"] == "web_search_20250305" && t["name"] == "web_search"),
            "body: {on}"
        );
    }

    #[test]
    fn prompt_caching_off_leaves_body_untouched() {
        let state = crate::state::AppState::new_headless(
            crate::settings::Settings::default(),
            std::env::temp_dir(),
        );
        let provider = cache_test_provider(false, "short");
        let adapter = AnthropicMessagesProvider::new(&state, &provider, 1);
        let body = adapter.request_body(&cache_test_request(), false);
        // 关缓存时 system 仍是裸字符串、没有任何 cache_control —— 与加这个功能之前逐字节一致。
        assert_eq!(body["system"], serde_json::json!("you are kivio"));
        assert!(!body.to_string().contains("cache_control"), "body: {body}");
    }

    #[test]
    fn prompt_caching_marks_tools_system_and_last_message() {
        let state = crate::state::AppState::new_headless(
            crate::settings::Settings::default(),
            std::env::temp_dir(),
        );
        let provider = cache_test_provider(true, "short");
        let adapter = AnthropicMessagesProvider::new(&state, &provider, 1);
        let body = adapter.request_body(&cache_test_request(), false);

        let ephemeral = serde_json::json!({ "type": "ephemeral" });
        // system 从字符串变成块数组，最后一块带断点。
        assert_eq!(
            body["system"][0]["text"],
            serde_json::json!("you are kivio")
        );
        assert_eq!(body["system"][0]["cache_control"], ephemeral);
        // 只有最后一个工具带断点（前缀越长命中越多，中间打断点是浪费）。
        let tools = body["tools"].as_array().expect("tools");
        assert_eq!(tools.len(), 2);
        assert!(tools[0].get("cache_control").is_none());
        assert_eq!(tools[1]["cache_control"], ephemeral);
        // 最后一条消息的最后一个内容块带断点。
        let messages = body["messages"].as_array().expect("messages");
        let last = messages.last().expect("last message");
        let content = last["content"].as_array().expect("content blocks");
        assert_eq!(content.last().unwrap()["cache_control"], ephemeral);
        // 断点总数 ≤ Anthropic 的 4 个上限。
        assert_eq!(body.to_string().matches("cache_control").count(), 3);
    }

    #[test]
    fn long_retention_adds_ttl_and_beta_header() {
        let state = crate::state::AppState::new_headless(
            crate::settings::Settings::default(),
            std::env::temp_dir(),
        );
        let provider = cache_test_provider(true, "long");
        let adapter = AnthropicMessagesProvider::new(&state, &provider, 1);
        let body = adapter.request_body(&cache_test_request(), false);
        assert_eq!(body["system"][0]["cache_control"]["ttl"], "1h");
        // 1h 不带 beta 头会被拒，所以头和 ttl 必须同进同出。
        let in_session = crate::chat::model::RequestMetadata {
            conversation_id: Some("conv_abc".into()),
            ..Default::default()
        };
        let headers = adapter.debug_request_headers(&in_session);
        assert_eq!(
            headers.get("anthropic-beta").map(String::as_str),
            Some(EXTENDED_CACHE_TTL_BETA)
        );

        let short_provider = cache_test_provider(true, "short");
        let short_adapter = AnthropicMessagesProvider::new(&state, &short_provider, 1);
        assert!(short_adapter
            .debug_request_headers(&in_session)
            .get("anthropic-beta")
            .is_none());
        // 一次性调用（无会话 id）连断点都不打，自然也不该带 beta 头。
        assert!(adapter
            .debug_request_headers(&Default::default())
            .get("anthropic-beta")
            .is_none());
    }

    #[test]
    fn long_retention_on_any_host_sends_ttl() {
        // 对齐 pi：long 默认 supportsLongCacheRetention=true，不做主机名启发式。
        let state = crate::state::AppState::new_headless(
            crate::settings::Settings::default(),
            std::env::temp_dir(),
        );
        let mut provider = cache_test_provider(true, "long");
        provider.base_url = "https://api.deepseek.com/anthropic".into();
        let adapter = AnthropicMessagesProvider::new(&state, &provider, 1);
        let body = adapter.request_body(&cache_test_request(), false);
        assert_eq!(body["system"][0]["cache_control"]["ttl"], "1h");
        let in_session = crate::chat::model::RequestMetadata {
            conversation_id: Some("conv_abc".into()),
            ..Default::default()
        };
        assert_eq!(
            adapter
                .debug_request_headers(&in_session)
                .get("anthropic-beta")
                .map(String::as_str),
            Some(EXTENDED_CACHE_TTL_BETA)
        );
    }

    #[test]
    fn one_shot_calls_get_no_breakpoints_even_with_caching_on() {
        // 翻译器 / 截图翻译 / Lens / 上下文压缩 / 标题总结走同一个生成入口，但都没有
        // conversation_id。给它们打断点 = 按 1.25× 写一份下次内容全变、永远读不到的缓存。
        let state = crate::state::AppState::new_headless(
            crate::settings::Settings::default(),
            std::env::temp_dir(),
        );
        let provider = cache_test_provider(true, "short");
        let adapter = AnthropicMessagesProvider::new(&state, &provider, 1);
        let mut request = cache_test_request();
        request.metadata.conversation_id = None;
        let body = adapter.request_body(&request, false);
        assert_eq!(body["system"], serde_json::json!("you are kivio"));
        assert!(!body.to_string().contains("cache_control"), "body: {body}");
        // 空串也算没有会话。
        request.metadata.conversation_id = Some(String::new());
        assert!(!adapter
            .request_body(&request, false)
            .to_string()
            .contains("cache_control"));
    }

    #[test]
    fn anthropic_and_openai_default_to_short() {
        // 统一 short 默认：OpenAI / Anthropic 均启用客户端缓存字段。
        let mut p = cache_test_provider(true, "short");
        p.request.prompt_caching = None;
        p.request.prompt_cache_retention = "short".into();
        assert!(p.prompt_caching_enabled());
        p.api_format = "openai_chat".into();
        assert!(p.prompt_caching_enabled());
        p.request.prompt_cache_retention = "none".into();
        assert!(!p.prompt_caching_enabled());
    }

    #[test]
    fn cache_breakpoint_skips_thinking_and_empty_text_blocks() {
        let mut blocks = vec![
            serde_json::json!({ "type": "text", "text": "real content" }),
            serde_json::json!({ "type": "text", "text": "   " }),
            serde_json::json!({ "type": "thinking", "thinking": "hmm" }),
        ];
        mark_last_cacheable_block(&mut blocks, &serde_json::json!({ "type": "ephemeral" }));
        // thinking 块和空 text 块不能带 cache_control，断点要往前退到第一块。
        assert!(blocks[0].get("cache_control").is_some());
        assert!(blocks[1].get("cache_control").is_none());
        assert!(blocks[2].get("cache_control").is_none());
    }

    fn cache_test_provider(caching: bool, retention: &str) -> crate::settings::ModelProvider {
        crate::settings::ModelProvider {
            id: "test".into(),
            name: "Test".into(),
            api_keys: vec!["sk-test".into()],
            api_key_legacy: None,
            base_url: "https://api.anthropic.com".into(),
            available_models: vec!["claude-opus-4-8".into()],
            enabled_models: vec!["claude-opus-4-8".into()],
            enabled: true,
            api_format: "anthropic_messages".into(),
            model_overrides: Default::default(),
            compress_request_body: false,
            request: crate::settings::ProviderRequestConfig {
                prompt_cache_retention: if caching {
                    retention.into()
                } else {
                    "none".into()
                },
                ..Default::default()
            },
        }
    }

    fn cache_test_tool(name: &str) -> ModelTool {
        ModelTool {
            id: name.into(),
            name: name.into(),
            description: format!("{name} a file"),
            source: "native".into(),
            server_id: None,
            server_name: None,
            input_schema: serde_json::json!({ "type": "object", "properties": {} }),
            sensitive: false,
        }
    }

    fn cache_test_request() -> GenerateRequest {
        GenerateRequest {
            model: "claude-opus-4-8".into(),
            system: "you are kivio".into(),
            messages: vec![ModelMessage {
                role: ModelRole::User,
                content: vec![MessagePart::Text { text: "hi".into() }],
            }],
            tools: vec![cache_test_tool("read"), cache_test_tool("write")],
            options: GenerateOptions::default(),
            // 有会话 id 才会打断点（一次性调用不该付这份钱），见 prompt_cache_control。
            metadata: crate::chat::model::RequestMetadata {
                conversation_id: Some("conv_abc".into()),
                ..Default::default()
            },
        }
    }

    #[test]
    fn web_search_parsed_from_content_blocks() {
        // server_tool_use(web_search) → 查询；web_search_tool_result.content[] → 来源（去重）。
        let response = serde_json::json!({
            "content": [
                { "type": "server_tool_use", "name": "web_search", "input": { "query": "kivio" } },
                {
                    "type": "web_search_tool_result",
                    "content": [
                        { "type": "web_search_result", "url": "https://a.com", "title": "A 站" },
                        { "type": "web_search_result", "url": "https://a.com", "title": "dup" },
                        { "type": "web_search_result", "url": "https://b.com" }
                    ]
                },
                { "type": "text", "text": "见来源。" }
            ],
            "stop_reason": "end_turn"
        });
        let parsed = parse_anthropic_response(&response);
        let web_search = parsed.web_search.expect("web_search present");
        assert_eq!(web_search.queries, vec!["kivio".to_string()]);
        assert_eq!(web_search.citations.len(), 2);
        assert_eq!(web_search.citations[0].title, "A 站");
        assert_eq!(web_search.citations[1].title, "https://b.com");
        assert_eq!(parsed.content, "见来源。");
    }

    #[test]
    fn web_search_none_without_server_blocks() {
        let response = serde_json::json!({
            "content": [{ "type": "text", "text": "无搜索" }],
            "stop_reason": "end_turn"
        });
        assert!(parse_anthropic_response(&response).web_search.is_none());
    }

    /// 内置搜索实时卡（任务 07-23）：流式 `content_block_start` 的 `server_tool_use`（查询）
    /// 与 `web_search_tool_result`（来源）各解析成一个 `WebSearch` 增量事件——正是流循环里
    /// 逐帧 `emit` 成 `StreamPart::WebSearch` 的来源。此测试钉住这条 wire → 增量的映射。
    #[test]
    fn stream_web_search_events_parse_query_then_results_increments() {
        let query_line = "data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"server_tool_use\",\"name\":\"web_search\",\"input\":{\"query\":\"kivio\"}}}";
        match parse_anthropic_sse_event(query_line) {
            Some(AnthropicSseEvent::WebSearch(ws)) => {
                assert_eq!(ws.queries, vec!["kivio".to_string()]);
                assert!(ws.citations.is_empty());
            }
            _ => panic!("expected WebSearch query increment"),
        }
        let result_line = "data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"web_search_tool_result\",\"content\":[{\"type\":\"web_search_result\",\"url\":\"https://a.com\",\"title\":\"A 站\"}]}}";
        match parse_anthropic_sse_event(result_line) {
            Some(AnthropicSseEvent::WebSearch(ws)) => {
                assert!(ws.queries.is_empty());
                assert_eq!(ws.citations.len(), 1);
                assert_eq!(ws.citations[0].url, "https://a.com");
            }
            _ => panic!("expected WebSearch result increment"),
        }
    }

    #[test]
    fn canonical_text_image_and_tool_result_become_content_blocks() {
        let request = GenerateRequest {
            model: "claude".to_string(),
            system: "system".to_string(),
            messages: vec![
                ModelMessage {
                    role: ModelRole::User,
                    content: vec![
                        MessagePart::Text {
                            text: "Look".to_string(),
                        },
                        MessagePart::Image {
                            mime_type: "image/png".to_string(),
                            data: "abc".to_string(),
                            path: None,
                        },
                    ],
                },
                ModelMessage {
                    role: ModelRole::Tool,
                    content: vec![MessagePart::ToolResult {
                        tool_call_id: "toolu_1".to_string(),
                        content: "done".to_string(),
                        is_error: false,
                        artifacts: Vec::new(),
                    }],
                },
            ],
            tools: Vec::new(),
            options: Default::default(),
            metadata: Default::default(),
        };

        let messages = anthropic_messages_from_generate_request(&request);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        let content = messages[0]["content"].as_array().expect("content");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["source"]["type"], "base64");
        assert_eq!(content[2]["type"], "tool_result");
    }

    #[test]
    fn canonical_assistant_tool_call_becomes_tool_use() {
        let request = GenerateRequest {
            model: "claude".to_string(),
            system: String::new(),
            messages: vec![ModelMessage {
                role: ModelRole::Assistant,
                content: vec![MessagePart::ToolCall {
                    id: "toolu_1".to_string(),
                    name: "web_search".to_string(),
                    arguments: serde_json::json!({"query": "rust"}),
                    arguments_raw: "{\"query\":\"rust\"}".to_string(),
                    signature: None,
                }],
            }],
            tools: Vec::new(),
            options: Default::default(),
            metadata: Default::default(),
        };

        let messages = anthropic_messages_from_generate_request(&request);

        assert_eq!(messages[0]["content"][0]["type"], "tool_use");
        assert_eq!(messages[0]["content"][0]["input"]["query"], "rust");
    }

    #[test]
    fn parses_anthropic_output_to_generate_output() {
        let response = serde_json::json!({
            "content": [
                {"type": "thinking", "thinking": "Plan"},
                {"type": "text", "text": "Answer"},
                {"type": "tool_use", "id": "toolu_1", "name": "web_search", "input": {"query": "kivio"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 7, "output_tokens": 11}
        });

        let output = output_from_anthropic_message(&response, "test").expect("output");

        assert_eq!(output.text, "Answer");
        assert_eq!(output.reasoning.as_deref(), Some("Plan"));
        assert_eq!(output.tool_calls.len(), 1);
        assert_eq!(output.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(
            output.usage.as_ref().and_then(|usage| usage.total_tokens),
            Some(18)
        );
    }

    #[test]
    fn parses_anthropic_stream_text_reasoning_and_tool_use() {
        let events = [
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Plan\"}}",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}",
            "data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_123\",\"name\":\"web_search\",\"input\":{}}}",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\"\"}}",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\":\\\"kivio\\\"}\"}}",
            "data: {\"type\":\"content_block_stop\"}",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}",
        ];

        assert!(matches!(
            parse_anthropic_sse_event(events[0]),
            Some(AnthropicSseEvent::ThinkingDelta(delta)) if delta == "Plan"
        ));
        assert!(matches!(
            parse_anthropic_sse_event(events[1]),
            Some(AnthropicSseEvent::TextDelta(delta)) if delta == "Hello"
        ));
        assert!(matches!(
            parse_anthropic_sse_event(events[2]),
            Some(AnthropicSseEvent::ToolUseStart { id, name })
                if id == "toolu_123" && name == "web_search"
        ));
        assert!(matches!(
            parse_anthropic_sse_event(events[5]),
            Some(AnthropicSseEvent::ContentBlockStop)
        ));
        assert!(matches!(
            parse_anthropic_sse_event(events[6]),
            Some(AnthropicSseEvent::MessageStopWithReason { reason, .. }) if reason == "tool_use"
        ));

        let input_parts = events[3..=4]
            .iter()
            .filter_map(|event| match parse_anthropic_sse_event(event) {
                Some(AnthropicSseEvent::ToolInputDelta(delta)) => Some(delta),
                _ => None,
            })
            .collect::<Vec<_>>();
        let call = assemble_tool_call_from_stream("toolu_123", "web_search", &input_parts);

        assert_eq!(call.function_name, "web_search");
        assert_eq!(call.arguments["query"], "kivio");
        assert!(call.arguments_parse_error.is_none());
    }
}
