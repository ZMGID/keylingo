use reqwest::header::ACCEPT_ENCODING;
use serde_json::Value;

use crate::api::{send_with_failover, with_chat_request_timeout};
use crate::settings::ModelProvider;
use crate::state::AppState;
use crate::usage::{
    chat_usage_source_for_label, error_kind_from_message, model_usage_from_openai_value,
    model_usage_from_stream_value, operation_from_label, record_model_call, UsageRecordInput,
};
use crate::utils;

use super::{
    openai_messages_from_generate_request, pending_tool_calls_from_openai_message,
    stream_read_error, FirstTokenStreamSink, GenerateOutput, GenerateRequest, GeneratedImageData,
    LanguageModelProvider, ModelError, ModelFuture, ModelUsage, PendingToolCall, StreamPart,
    StreamSink,
};

pub struct OpenAiChatProvider<'a> {
    state: &'a AppState,
    provider: &'a ModelProvider,
    retry_attempts: usize,
}

impl<'a> OpenAiChatProvider<'a> {
    pub fn new(state: &'a AppState, provider: &'a ModelProvider, retry_attempts: usize) -> Self {
        Self {
            state,
            provider,
            retry_attempts,
        }
    }
}

impl LanguageModelProvider for OpenAiChatProvider<'_> {
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

/// 判断错误是否是"端点拒绝 prompt_cache_key 字段"。仅在本次确实发了该字段时配合调用，
/// 错误体点名该字段即视为命中（NVIDIA NIM / 智谱 GLM 等严格端点的 400 会带上字段名）。
pub(super) fn error_rejects_prompt_cache_key(err: &str) -> bool {
    err.contains("prompt_cache_key")
}

/// 判断错误是否点名拒绝 `prompt_cache_retention`（long 的 24h 字段）。
pub(super) fn error_rejects_prompt_cache_retention(err: &str) -> bool {
    err.contains("prompt_cache_retention")
}

impl OpenAiChatProvider<'_> {
    /// 发送 chat 请求；若严格端点拒绝 `prompt_cache_key` / `prompt_cache_retention` 而 400，
    /// 记入 state 后去掉对应字段重试一次（本会话后续 request_body 就地跳过）。
    async fn send_chat_body(
        &self,
        request: &GenerateRequest,
        stream: bool,
        label: &str,
    ) -> Result<reqwest::Response, String> {
        let body = self.request_body(request, stream);
        let result = self.post_chat(request, &body, stream, label).await;
        if let Err(ref err) = result {
            let mut learned = false;
            // key 被拒：key 与 24h 一并消失（24h 只跟 key 一起发）。
            if body.get("prompt_cache_key").is_some() && error_rejects_prompt_cache_key(err) {
                self.state
                    .mark_prompt_cache_key_unsupported(&self.provider.base_url);
                learned = true;
            } else if body.get("prompt_cache_retention").is_some()
                && error_rejects_prompt_cache_retention(err)
            {
                // 只拒 24h：保留 key，停发 retention。
                self.state
                    .mark_prompt_cache_retention_unsupported(&self.provider.base_url);
                learned = true;
            }
            if learned {
                let retry_body = self.request_body(request, stream);
                return self.post_chat(request, &retry_body, stream, label).await;
            }
        }
        result
    }

    /// 单次发送（带多 key failover）。非流式套 `with_chat_request_timeout` 总超时；
    /// 流式不套，避免活跃 SSE 被总超时砍断（与改动前两条路径各自的行为一致）。
    async fn post_chat(
        &self,
        request: &GenerateRequest,
        body: &Value,
        stream: bool,
        label: &str,
    ) -> Result<reqwest::Response, String> {
        send_with_failover(
            self.state,
            label,
            self.retry_attempts,
            &self.provider.id,
            &self.provider.api_keys,
            |key| {
                let req = crate::api::attach_json_body(
                    self.with_session_headers(
                        self.state
                            .client_for(self.provider)
                            .post(self.chat_completions_url())
                            .bearer_auth(key)
                            .header(ACCEPT_ENCODING, "identity"),
                        &request.metadata,
                    ),
                    body,
                    self.provider.compress_request_body,
                );
                let req = if stream {
                    req
                } else {
                    with_chat_request_timeout(req)
                };
                req.send()
            },
        )
        .await
    }

    async fn generate_inner(&self, request: GenerateRequest) -> Result<GenerateOutput, ModelError> {
        let label = request_label(&request, "Chat API");
        let started_at = chrono::Local::now().timestamp();
        let started = std::time::Instant::now();
        let response = self
            .send_chat_body(&request, false, &label)
            .await
            .map_err(|err| {
                self.record_usage_failure(&request, &label, started_at, started.elapsed(), &err);
                self.record_debug_failure(
                    &request,
                    &label,
                    false,
                    &err,
                    started_at,
                    started.elapsed(),
                );
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
        let output = output_from_chat_completion(&value, &raw, &label)?;
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
        let label = request_label(&request, "Chat stream");
        let started_at = chrono::Local::now().timestamp();
        let started = std::time::Instant::now();
        let mut measured_sink = FirstTokenStreamSink::new(sink, started);
        let sink = &mut measured_sink;
        let mut response = self
            .send_chat_body(&request, true, &label)
            .await
            .map_err(|err| {
                self.record_usage_failure(&request, &label, started_at, started.elapsed(), &err);
                self.record_debug_failure(
                    &request,
                    &label,
                    true,
                    &err,
                    started_at,
                    started.elapsed(),
                );
                ModelError::new(err)
            })?;

        let mut buffer = String::new();
        let mut utf8 = crate::api::Utf8StreamDecoder::default();
        let mut full = String::new();
        let mut reasoning_full = String::new();
        let mut tool_partials: Vec<PartialToolCall> = Vec::new();
        let mut finish_reason: Option<String> = None;
        let mut usage: Option<ModelUsage> = None;
        // 代理仿 OpenRouter 出图：流式 chunk 的 `delta.images` 逐帧累积，finish 后并入 output。
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
                if data == "[DONE]" {
                    let tool_calls = finish_tool_call_partials(&mut tool_partials, sink)?;
                    let reason = finish_reason.unwrap_or_else(|| "done".to_string());
                    sink.emit(StreamPart::Finish {
                        reason: reason.clone(),
                        full: full.clone(),
                    })?;
                    let output = GenerateOutput {
                        text: full,
                        reasoning: non_empty(reasoning_full),
                        tool_calls,
                        usage,
                        finish_reason: Some(reason),
                        provider_messages: Vec::new(),
                        cancelled: false,
                        web_search: None,
                        images,
                    };
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
                let value: Value = match serde_json::from_str(data) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                if let Some(next_usage) = model_usage_from_stream_value(&value) {
                    usage = Some(next_usage);
                }
                if let Some(reason) = openai_stream_finish_reason(&value) {
                    finish_reason = Some(reason.to_string());
                }
                handle_openai_stream_tool_calls(&value, &mut tool_partials, sink)?;
                if let Some((reasoning, mode)) = extract_chat_stream_reasoning(&value) {
                    if let Some(delta) =
                        append_chat_stream_text(&mut reasoning_full, reasoning, mode)
                    {
                        sink.emit(StreamPart::ReasoningDelta { delta })?;
                    }
                }
                if let Some((content, mode)) = extract_chat_stream_text(&value) {
                    if let Some(delta) = append_chat_stream_text(&mut full, content, mode) {
                        sink.emit(StreamPart::TextDelta { delta })?;
                    }
                }
                for image in extract_chat_stream_images(&value) {
                    sink.emit(StreamPart::ImageData {
                        mime_type: image.mime_type.clone(),
                        data: image.data.clone(),
                    })?;
                    images.push(image);
                }
            }
        }

        let tool_calls = finish_tool_call_partials(&mut tool_partials, sink)?;
        let reason = finish_reason.unwrap_or_else(|| "done".to_string());
        sink.emit(StreamPart::Finish {
            reason: reason.clone(),
            full: full.clone(),
        })?;
        let output = GenerateOutput {
            text: full,
            reasoning: non_empty(reasoning_full),
            tool_calls,
            usage,
            finish_reason: Some(reason),
            provider_messages: Vec::new(),
            cancelled: false,
            web_search: None,
            images,
        };
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

    fn chat_completions_url(&self) -> String {
        format!(
            "{}/chat/completions",
            self.provider.base_url.trim_end_matches('/')
        )
    }

    /// 会话亲和头（对齐 opencode）：同一对话每轮带同一 id。会话亲和型代理据此把请求
    /// 稳定路由到同一上游会话（不再靠前缀指纹猜，杜绝串台/复用脏会话）；正经 provider
    /// 忽略未知头，无副作用。发送与请求调试记录共用 `session_header_pairs`，杜绝漂移。
    /// 供应商「请求配置」里的 CLI 身份头与自定义头也在这里叠上（同名时自定义优先）。
    fn with_session_headers(
        &self,
        request: reqwest::RequestBuilder,
        metadata: &crate::chat::model::RequestMetadata,
    ) -> reqwest::RequestBuilder {
        // 先把会话头与请求配置的头合成一份「同名只留一条」的清单再贴：reqwest 的
        // `.header()` 是 append，分两轮贴会让用户自定义的 x-session-id 与我们自己的并存。
        let mut pairs: Vec<(String, String)> = Vec::new();
        for (name, value) in session_header_pairs(metadata) {
            crate::provider_request::upsert_pair(&mut pairs, name.to_string(), value);
        }
        for (name, value) in crate::provider_request::header_pairs(
            self.provider,
            metadata.conversation_id.as_deref(),
        ) {
            crate::provider_request::upsert_pair(&mut pairs, name, value);
        }
        let mut request = request;
        for (name, value) in pairs {
            request = request.header(name, value);
        }
        request
    }

    /// 重建本次请求实际会带的 headers（脱敏后）供请求调试面板展示。静态头（Authorization/
    /// Accept-Encoding/Content-Type）与发送路径一一对应；动态会话头共用 `session_header_pairs`，
    /// 故与真实发送零漂移。Authorization 用首个 key（正常发送用的也是它）派生脱敏预览。
    fn debug_request_headers(
        &self,
        metadata: &crate::chat::model::RequestMetadata,
    ) -> std::collections::BTreeMap<String, String> {
        let mut headers = std::collections::BTreeMap::new();
        if let Some(key) = self.provider.api_keys.first() {
            headers.insert("Authorization".to_string(), format!("Bearer {key}"));
        }
        headers.insert("Accept-Encoding".to_string(), "identity".to_string());
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        // 先按大小写不敏感合成一份，再折进 BTreeMap。BTreeMap 的 key 是大小写敏感的：
        // 用户把自定义头写成 `X-Session-Id`（不在保留名单里，允许覆盖）时，发送路径会
        // upsert 成一条，直接往 map 里塞会显示成两条——正是本模块要杜绝的那种不一致。
        let mut pairs: Vec<(String, String)> = Vec::new();
        for (name, value) in session_header_pairs(metadata) {
            crate::provider_request::upsert_pair(&mut pairs, name.to_string(), value);
        }
        for (name, value) in crate::provider_request::header_pairs(
            self.provider,
            metadata.conversation_id.as_deref(),
        ) {
            crate::provider_request::upsert_pair(&mut pairs, name, value);
        }
        for (name, value) in pairs {
            headers.insert(name, value);
        }
        crate::chat::request_debug::sanitize_headers(headers)
    }

    /// 记录一次成功调用到请求调试缓冲。开关关时首行短路 → 不构造 body/headers（零开销）。
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
                url: self.chat_completions_url(),
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
                url: self.chat_completions_url(),
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

    fn request_body(&self, request: &GenerateRequest, stream: bool) -> Value {
        let mut body = serde_json::json!({
            "model": request.model,
            "messages": openai_messages_from_generate_request(request),
            "max_tokens": request.options.max_tokens,
        });
        if let Some(temperature) = crate::chat::model_metadata::temperature_for_request(
            request.options.temperature,
            Some(self.provider),
            &request.model,
        ) {
            body["temperature"] = serde_json::json!(temperature);
        }
        if stream {
            body["stream"] = Value::Bool(true);
            // usage 随流返回（OpenAI 标准参数；AI SDK/opencode 同款）。缺省时部分
            // provider 不在流里带 usage，token 统计只能靠估算。
            body["stream_options"] = serde_json::json!({ "include_usage": true });
        }
        if !request.tools.is_empty() {
            body["tools"] = Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| tool.to_openai_tool())
                    .collect(),
            );
            body["tool_choice"] = Value::String("auto".to_string());
        }
        // Prompt 缓存 short/long：硬发 `prompt_cache_key`（会话 id）；none 不发。
        // 严格端点 400 后学习停发。long → `prompt_cache_retention:24h`（对齐 pi，默认能力开）。
        // 不发驼峰 `promptCacheKey`。Anthropic 走 cache_control，不经这里。
        if self.provider.prompt_caching_enabled()
            && !self
                .state
                .prompt_cache_key_unsupported(&self.provider.base_url)
        {
            if let Some(conversation_id) = request
                .metadata
                .conversation_id
                .as_deref()
                .filter(|id| !id.is_empty())
            {
                body["prompt_cache_key"] = Value::String(conversation_id.to_string());
                if matches!(
                    self.provider.cache_retention(),
                    crate::settings::CacheRetention::Long
                ) && !self
                    .state
                    .prompt_cache_retention_unsupported(&self.provider.base_url)
                {
                    body["prompt_cache_retention"] = Value::String("24h".to_string());
                }
            }
        }
        // UI「Off」→ 显式关闭思考。省略字段在多家默认会落到 high（DeepSeek 文档：
        // 思考默认开、effort 默认 high；OpenAI 也把 none 列为一等 effort）。
        //
        // 两套 wire：
        // - DeepSeek / Kimi 官方：Chat Completions 用 `thinking.type=disabled` 做开关
        //   （其 reasoning_effort 只认 low/high/max，没有 none）。
        // - 其余 OpenAI 兼容端（含代理 / OpenAI 自家）：`reasoning_effort: "none"`。
        if !request.options.thinking_enabled {
            if utils::provider_supports_thinking_field(&self.provider.base_url) {
                body["thinking"] = serde_json::json!({ "type": "disabled" });
            } else {
                body["reasoning_effort"] = Value::String("none".to_string());
            }
        }
        // 思考等级 → OpenAI Chat `reasoning_effort`，原样下发（档位由模型库 reasoningEfforts 门控）。
        // 代理普遍接受;不发 Qwen/vLLM 私有的 enable_thinking / chat_template_kwargs。
        // 与上面 Off 分支互斥：resolve_thinking 在 off 时把 level 抹成 None。
        if let Some(effort) = request.options.thinking_level.as_deref() {
            body["reasoning_effort"] = Value::String(effort.to_string());
        }
        // 模型级额外请求体字段（model_overrides[model].extra_body）：原样 merge 进 body 根部，
        // 给严格端点塞标准 schema 外的私有旋钮（NVIDIA NIM / vLLM `chat_template_kwargs` 等）。
        // 放在单次 provider_options 之前，让显式的单次覆盖仍然最后生效。
        if let Some(extra) = self
            .provider
            .model_overrides
            .get(&request.model)
            .and_then(|info| info.extra_body.as_ref())
            .and_then(|value| value.as_object())
        {
            for (key, value) in extra {
                body[key] = value.clone();
            }
        }
        if let Some(overrides) = request.options.provider_options.as_object() {
            for (key, value) in overrides {
                body[key] = value.clone();
            }
        }
        body
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

pub fn output_from_chat_completion(
    value: &Value,
    raw: &str,
    label: &str,
) -> Result<GenerateOutput, ModelError> {
    let choice = value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .ok_or_else(|| invalid_response(label, raw))?;
    let message = choice
        .get("message")
        .cloned()
        .ok_or_else(|| invalid_response(label, raw))?;
    let text = assistant_text_from_openai_message(&message);
    let reasoning = reasoning_from_openai_message(&message);
    let tool_calls = pending_tool_calls_from_openai_message(&message);
    let finish_reason = choice
        .get("finish_reason")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let usage = model_usage_from_openai_response(value);
    // 代理仿 OpenRouter 出图：非流式在 message.images 携带图片数组，解析为协议无关 images。
    let images = message
        .get("images")
        .map(generated_images_from_openai_images)
        .unwrap_or_default();

    Ok(GenerateOutput {
        text,
        reasoning,
        tool_calls,
        usage,
        finish_reason,
        provider_messages: vec![message],
        cancelled: false,
        web_search: None,
        images,
    })
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

/// 会话亲和头键值对。发送路径（`with_session_headers`）与请求调试记录
/// （`debug_request_headers`）共用此单一来源，保证记录的头与真实发送零漂移。
fn session_header_pairs(
    metadata: &crate::chat::model::RequestMetadata,
) -> Vec<(&'static str, String)> {
    match metadata
        .conversation_id
        .as_deref()
        .filter(|id| !id.is_empty())
    {
        Some(id) => vec![
            ("x-session-id", id.to_string()),
            ("x-session-affinity", id.to_string()),
        ],
        None => Vec::new(),
    }
}

fn invalid_response(label: &str, raw: &str) -> ModelError {
    ModelError::new(format!(
        "Invalid {label} response: {}",
        raw.chars().take(500).collect::<String>()
    ))
}

fn assistant_text_from_openai_message(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                if part.get("type").and_then(|value| value.as_str()) == Some("text") {
                    part.get("text").and_then(|value| value.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn reasoning_from_openai_message(message: &Value) -> Option<String> {
    message
        .get("reasoning_content")
        .or_else(|| message.get("reasoning"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn model_usage_from_openai_response(value: &Value) -> Option<ModelUsage> {
    model_usage_from_openai_value(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatStreamTextMode {
    Delta,
    Snapshot,
}

fn extract_chat_stream_text(value: &Value) -> Option<(&str, ChatStreamTextMode)> {
    let choice = value.get("choices").and_then(|choices| choices.get(0))?;

    if let Some(content) = choice
        .get("delta")
        .and_then(|delta| delta.get("content"))
        .and_then(|content| content.as_str())
        .filter(|content| !content.is_empty())
    {
        return Some((content, ChatStreamTextMode::Delta));
    }

    if let Some(content) = choice
        .get("text")
        .and_then(|content| content.as_str())
        .filter(|content| !content.is_empty())
    {
        return Some((content, ChatStreamTextMode::Delta));
    }

    choice
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .filter(|content| !content.is_empty())
        .map(|content| (content, ChatStreamTextMode::Snapshot))
}

fn extract_chat_stream_reasoning(value: &Value) -> Option<(&str, ChatStreamTextMode)> {
    let choice = value.get("choices").and_then(|choices| choices.get(0))?;

    if let Some(reasoning) = choice
        .get("delta")
        .and_then(|delta| {
            delta
                .get("reasoning_content")
                .or_else(|| delta.get("reasoning"))
        })
        .and_then(|content| content.as_str())
        .filter(|content| !content.is_empty())
    {
        return Some((reasoning, ChatStreamTextMode::Delta));
    }

    choice
        .get("message")
        .and_then(|message| {
            message
                .get("reasoning_content")
                .or_else(|| message.get("reasoning"))
        })
        .and_then(|content| content.as_str())
        .filter(|content| !content.is_empty())
        .map(|content| (content, ChatStreamTextMode::Snapshot))
}

fn append_chat_stream_text(
    full: &mut String,
    text: &str,
    mode: ChatStreamTextMode,
) -> Option<String> {
    if text.is_empty() {
        return None;
    }

    match mode {
        ChatStreamTextMode::Delta => {
            full.push_str(text);
            Some(text.to_string())
        }
        ChatStreamTextMode::Snapshot => {
            if text == full || full.starts_with(text) {
                return None;
            }
            if text.starts_with(full.as_str()) {
                let delta = text[full.len()..].to_string();
                full.clear();
                full.push_str(text);
                return if delta.is_empty() { None } else { Some(delta) };
            }

            full.push_str(text);
            Some(text.to_string())
        }
    }
}

#[derive(Debug, Default)]
struct PartialToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments_raw: String,
    started: bool,
}

fn handle_openai_stream_tool_calls(
    value: &Value,
    partials: &mut Vec<PartialToolCall>,
    sink: &mut (dyn StreamSink + Send),
) -> Result<(), ModelError> {
    let Some(calls) = value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("delta"))
        .and_then(|delta| delta.get("tool_calls"))
        .and_then(|tool_calls| tool_calls.as_array())
    else {
        return Ok(());
    };

    for call in calls {
        let index = call
            .get("index")
            .and_then(|value| value.as_u64())
            .unwrap_or(partials.len() as u64) as usize;
        while partials.len() <= index {
            partials.push(PartialToolCall::default());
        }
        let partial = &mut partials[index];
        // id 只在该 tool_call 首个分片给;续片不带 id 或带空串 `""`(如 SenseNova）。
        // 只在**非空**时才覆盖，否则空续片会把首片的真 id 冲掉 → 回放空 tool_call_id →
        // 严格校验端点(商汤 DeepSeek 等)400 invalid tool_call_id。与下面 name 的空过滤一致。
        if let Some(id) = call
            .get("id")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
        {
            partial.id = Some(id.to_string());
        }
        if let Some(name) = call
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
        {
            partial.name = Some(name.to_string());
        }
        let mut started_now = false;
        if !partial.started {
            if let (Some(id), Some(name)) = (partial.id.as_deref(), partial.name.as_deref()) {
                sink.emit(StreamPart::ToolCallStart {
                    id: id.to_string(),
                    name: name.to_string(),
                })?;
                partial.started = true;
                started_now = true;
            }
        }
        if started_now && !partial.arguments_raw.is_empty() {
            if let Some(id) = partial.id.as_deref() {
                sink.emit(StreamPart::ToolCallDelta {
                    id: id.to_string(),
                    delta: partial.arguments_raw.clone(),
                })?;
            }
        }
        if let Some(delta) = call
            .get("function")
            .and_then(|function| function.get("arguments"))
            .and_then(|value| match value {
                // 标准：参数以字符串分片流式到达，逐片累加。
                Value::String(text) => (!text.is_empty()).then(|| text.clone()),
                // 兼容：部分 OpenAI 兼容网关把 arguments 作为完整 JSON **对象**一次性发来
                // （非分片）。序列化成字符串当作整段累加，否则参数会被静默丢弃。
                Value::Null => None,
                other => serde_json::to_string(other).ok(),
            })
        {
            partial.arguments_raw.push_str(&delta);
            if partial.started {
                let Some(id) = partial.id.as_deref() else {
                    continue;
                };
                sink.emit(StreamPart::ToolCallDelta {
                    id: id.to_string(),
                    delta: delta.to_string(),
                })?;
            }
        }
    }
    Ok(())
}

fn finish_tool_call_partials(
    partials: &mut Vec<PartialToolCall>,
    sink: &mut (dyn StreamSink + Send),
) -> Result<Vec<PendingToolCall>, ModelError> {
    let mut calls = Vec::new();
    for (index, partial) in partials.drain(..).enumerate() {
        let Some(name) = partial.name else {
            continue;
        };
        // 兜底:None 或空串都生成合法非空 id,绝不产出空 tool_call_id。
        let id = partial
            .id
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("call_{index}"));
        let raw = if partial.arguments_raw.trim().is_empty() {
            "{}".to_string()
        } else {
            partial.arguments_raw
        };
        if !partial.started {
            sink.emit(StreamPart::ToolCallStart {
                id: id.clone(),
                name: name.clone(),
            })?;
            if raw != "{}" {
                sink.emit(StreamPart::ToolCallDelta {
                    id: id.clone(),
                    delta: raw.clone(),
                })?;
            }
        }
        let (arguments, arguments_parse_error) = super::parse_tool_arguments(&raw);
        let call = PendingToolCall {
            id,
            function_name: name,
            arguments,
            arguments_raw: raw,
            arguments_parse_error,
            signature: None,
        };
        sink.emit(StreamPart::ToolCallDone { call: call.clone() })?;
        calls.push(call);
    }
    Ok(calls)
}

/// 从 OpenRouter 风格的 `images` 数组解析出模型生成的图片（协议无关形态）。
/// 形如 `[{type:"image_url", image_url:{url:"data:image/png;base64,..."}}]`（这些代理仿
/// OpenRouter 出图）。仅收 data URL；http(s) 直链或格式异常项静默跳过（不报错，保持无图
/// 响应字节等价——绝大多数普通 chat 无该字段，行为完全不变）。
fn generated_images_from_openai_images(images: &Value) -> Vec<GeneratedImageData> {
    let Some(array) = images.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in array {
        let Some(url) = item
            .get("image_url")
            .or_else(|| item.get("imageUrl"))
            .and_then(|value| value.get("url"))
            .and_then(|value| value.as_str())
        else {
            continue;
        };
        if let Some(image) = parse_openai_image_data_url(url) {
            out.push(image);
        }
    }
    out
}

/// 解析 data URL 为 GeneratedImageData（mime + 纯 base64）。非 data URL（含 http(s) 直链）
/// 或非 base64 编码返回 None 静默跳过——现阶段不下载远程直链，只避免误伤正常 chat。
fn parse_openai_image_data_url(url: &str) -> Option<GeneratedImageData> {
    let trimmed = url.trim();
    let rest = trimmed.strip_prefix("data:")?;
    let (metadata, payload) = rest.split_once(',')?;
    if !metadata
        .split(';')
        .any(|part| part.eq_ignore_ascii_case("base64"))
    {
        return None;
    }
    let payload = payload.trim();
    if payload.is_empty() {
        return None;
    }
    let mime_type = metadata
        .split(';')
        .next()
        .map(str::trim)
        .filter(|value| value.starts_with("image/"))
        .unwrap_or("image/png")
        .to_string();
    Some(GeneratedImageData {
        mime_type,
        data: payload.to_string(),
    })
}

/// 流式 chunk 的 `choices[0].delta.images` → 协议无关 images（形状同非流式 message.images）。
fn extract_chat_stream_images(value: &Value) -> Vec<GeneratedImageData> {
    value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("delta"))
        .and_then(|delta| delta.get("images"))
        .map(generated_images_from_openai_images)
        .unwrap_or_default()
}

fn openai_stream_finish_reason(value: &Value) -> Option<&str> {
    value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(|value| value.as_str())
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
    use crate::chat::model::{GenerateOptions, MessagePart, ModelMessage, ModelRole};
    use crate::settings::ModelInfo;

    /// Build a real OpenAI-compatible provider request body via the production
    /// `request_body` path and assert how thinking maps to the wire.
    fn build_openai_body(thinking_level: Option<&str>, base_url: &str) -> Value {
        build_openai_body_with(thinking_level, true, base_url)
    }

    fn build_openai_body_with(
        thinking_level: Option<&str>,
        thinking_enabled: bool,
        base_url: &str,
    ) -> Value {
        let state =
            AppState::new_headless(crate::settings::Settings::default(), std::env::temp_dir());
        let provider = ModelProvider {
            id: "test".into(),
            name: "Test".into(),
            api_keys: vec!["sk-test".into()],
            api_key_legacy: None,
            base_url: base_url.into(),
            available_models: vec!["gpt-5".into()],
            enabled_models: vec!["gpt-5".into()],
            enabled: true,
            api_format: "openai_chat".into(),
            model_overrides: Default::default(),
            compress_request_body: false,
            request: Default::default(),
        };
        let adapter = OpenAiChatProvider::new(&state, &provider, 1);
        let request = GenerateRequest {
            model: "gpt-5".into(),
            system: "sys".into(),
            messages: vec![ModelMessage {
                role: ModelRole::User,
                content: vec![MessagePart::Text { text: "hi".into() }],
            }],
            tools: Vec::new(),
            options: GenerateOptions {
                thinking_enabled,
                thinking_level: thinking_level.map(|s| s.to_string()),
                ..Default::default()
            },
            metadata: Default::default(),
        };
        adapter.request_body(&request, false)
    }

    fn build_openai_temperature_body(
        provider_temperature: Option<f64>,
        request_temperature: Option<f64>,
    ) -> Value {
        let state =
            AppState::new_headless(crate::settings::Settings::default(), std::env::temp_dir());
        let mut model_overrides = std::collections::HashMap::new();
        if let Some(temperature) = provider_temperature {
            model_overrides.insert(
                "custom-temperature-model".into(),
                ModelInfo {
                    temperature: Some(temperature),
                    ..ModelInfo::default()
                },
            );
        }
        let provider = ModelProvider {
            id: "test".into(),
            name: "Test".into(),
            api_keys: vec!["sk-test".into()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".into(),
            available_models: vec!["custom-temperature-model".into()],
            enabled_models: vec!["custom-temperature-model".into()],
            enabled: true,
            api_format: "openai_chat".into(),
            model_overrides,
            compress_request_body: false,
            request: Default::default(),
        };
        let adapter = OpenAiChatProvider::new(&state, &provider, 1);
        let request = GenerateRequest {
            model: "custom-temperature-model".into(),
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
        adapter.request_body(&request, false)
    }

    #[test]
    fn thinking_level_maps_to_reasoning_effort() {
        // 开思考但未设等级 → 不发 reasoning_effort（与改动前一致）。
        let none = build_openai_body(None, "https://api.openai.com/v1");
        assert!(none.get("reasoning_effort").is_none(), "body: {none}");

        // 选了等级 → 发标准 reasoning_effort。
        let mid = build_openai_body(Some("medium"), "https://api.openai.com/v1");
        eprintln!("[openai reasoning_effort=medium] {mid}");
        assert_eq!(mid["reasoning_effort"], "medium");

        let high = build_openai_body(Some("high"), "https://api.openai.com/v1");
        assert_eq!(high["reasoning_effort"], "high");
    }

    #[test]
    fn thinking_off_sends_explicit_none_on_openai_compat() {
        // UI Off → 显式 reasoning_effort:"none"，不能省略（多家省略默认 high）。
        let off = build_openai_body_with(None, false, "https://api.openai.com/v1");
        assert_eq!(off["reasoning_effort"], "none", "body: {off}");
        assert!(off.get("thinking").is_none(), "body: {off}");

        // 代理 / 中转同样走 none（base_url 不含 deepseek.com / moonshot.cn）。
        let relay = build_openai_body_with(None, false, "https://relay.example.com/v1");
        assert_eq!(relay["reasoning_effort"], "none", "body: {relay}");
    }

    #[test]
    fn thinking_off_uses_thinking_disabled_on_deepseek_and_kimi() {
        // DeepSeek / Kimi 官方 Chat Completions：开关是 thinking.type，不是
        // reasoning_effort:"none"（官方 schema 的 effort 只有 low/high/max）。
        let ds = build_openai_body_with(None, false, "https://api.deepseek.com/v1");
        assert_eq!(ds["thinking"]["type"], "disabled", "body: {ds}");
        assert!(
            ds.get("reasoning_effort").is_none(),
            "DeepSeek Chat 不该发 none: {ds}"
        );

        let kimi = build_openai_body_with(None, false, "https://api.moonshot.cn/v1");
        assert_eq!(kimi["thinking"]["type"], "disabled", "body: {kimi}");
        assert!(kimi.get("reasoning_effort").is_none(), "body: {kimi}");
    }

    #[test]
    fn request_body_omits_temperature_by_default() {
        let body = build_openai_temperature_body(None, None);
        assert!(body.get("temperature").is_none(), "body: {body}");
    }

    #[test]
    fn request_body_uses_provider_temperature_override() {
        let body = build_openai_temperature_body(Some(0.4), None);
        assert_eq!(body["temperature"], serde_json::json!(0.4), "body: {body}");
    }

    #[test]
    fn explicit_request_temperature_wins_over_provider_override() {
        let body = build_openai_temperature_body(Some(0.4), Some(1.2));
        assert_eq!(body["temperature"], serde_json::json!(1.2), "body: {body}");
    }

    #[test]
    fn model_extra_body_merges_into_request_body_root() {
        // model_overrides[model].extra_body 原样进 body 根部（NVIDIA NIM chat_template_kwargs 用例）。
        let state =
            AppState::new_headless(crate::settings::Settings::default(), std::env::temp_dir());
        let mut model_overrides = std::collections::HashMap::new();
        model_overrides.insert(
            "deepseek-ai/deepseek-v4-pro".to_string(),
            ModelInfo {
                extra_body: Some(serde_json::json!({
                    "chat_template_kwargs": { "thinking": true }
                })),
                ..ModelInfo::default()
            },
        );
        let provider = ModelProvider {
            id: "nv".into(),
            name: "NV".into(),
            api_keys: vec!["nvapi-test".into()],
            api_key_legacy: None,
            base_url: "https://integrate.api.nvidia.com/v1".into(),
            available_models: vec!["deepseek-ai/deepseek-v4-pro".into()],
            enabled_models: vec!["deepseek-ai/deepseek-v4-pro".into()],
            enabled: true,
            api_format: "openai_chat".into(),
            model_overrides,
            compress_request_body: false,
            request: Default::default(),
        };
        let adapter = OpenAiChatProvider::new(&state, &provider, 1);
        let request = GenerateRequest {
            model: "deepseek-ai/deepseek-v4-pro".into(),
            system: String::new(),
            messages: vec![ModelMessage {
                role: ModelRole::User,
                content: vec![MessagePart::Text { text: "hi".into() }],
            }],
            tools: Vec::new(),
            options: GenerateOptions::default(),
            metadata: Default::default(),
        };
        let body = adapter.request_body(&request, false);
        assert_eq!(
            body["chat_template_kwargs"],
            serde_json::json!({ "thinking": true }),
            "body: {body}"
        );

        // 无 override 的模型不受影响。
        let plain = GenerateRequest {
            model: "other-model".into(),
            ..request
        };
        let plain_body = adapter.request_body(&plain, false);
        assert!(
            plain_body.get("chat_template_kwargs").is_none(),
            "body: {plain_body}"
        );
    }

    #[test]
    fn request_body_carries_session_cache_key_and_stream_usage() {
        let state =
            AppState::new_headless(crate::settings::Settings::default(), std::env::temp_dir());
        let provider = ModelProvider {
            id: "test".into(),
            name: "Test".into(),
            api_keys: vec!["sk-test".into()],
            api_key_legacy: None,
            base_url: "https://api.openai.com/v1".into(),
            available_models: vec!["gpt-5".into()],
            enabled_models: vec!["gpt-5".into()],
            enabled: true,
            api_format: "openai_chat".into(),
            model_overrides: Default::default(),
            compress_request_body: false,
            request: Default::default(),
        };
        let adapter = OpenAiChatProvider::new(&state, &provider, 1);
        let tool = crate::chat::model::ModelTool {
            id: "native__web_fetch".into(),
            name: "web_fetch".into(),
            description: "Fetch".into(),
            source: "native".into(),
            server_id: None,
            server_name: None,
            input_schema: serde_json::json!({ "type": "object" }),
            sensitive: false,
        };
        let mut request = GenerateRequest {
            model: "gpt-5".into(),
            system: "sys".into(),
            messages: vec![ModelMessage {
                role: ModelRole::User,
                content: vec![MessagePart::Text { text: "hi".into() }],
            }],
            tools: vec![tool],
            options: GenerateOptions::default(),
            metadata: crate::chat::model::RequestMetadata {
                conversation_id: Some("conv_abc".into()),
                ..Default::default()
            },
        };

        // 流式 + 有会话 id + 有工具：只发标准 snake_case 缓存键（不发驼峰 promptCacheKey，
        // 避免严格端点对未知字段 400）、stream_options、tool_choice 齐全。
        let body = adapter.request_body(&request, true);
        assert_eq!(body["prompt_cache_key"], "conv_abc");
        assert!(body.get("promptCacheKey").is_none());
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(body["tool_choice"], "auto");

        // 非流式：不带 stream_options；无会话 id：不带缓存键；无工具：不带 tool_choice。
        request.metadata.conversation_id = None;
        request.tools = Vec::new();
        let body = adapter.request_body(&request, false);
        assert!(body.get("stream_options").is_none());
        assert!(body.get("prompt_cache_key").is_none());
        assert!(body.get("promptCacheKey").is_none());
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn learned_unsupported_endpoint_skips_prompt_cache_key() {
        // 端点被记入 prompt_cache_key_unsupported 后，即便有会话 id 也不再发该字段；
        // 未记入的端点照常硬发。这是「默认开 + 400 自愈」学习后的就地跳过。
        let state =
            AppState::new_headless(crate::settings::Settings::default(), std::env::temp_dir());
        let make = |base_url: &str| {
            let provider = ModelProvider {
                id: "test".into(),
                name: "Test".into(),
                api_keys: vec!["sk-test".into()],
                api_key_legacy: None,
                base_url: base_url.into(),
                available_models: vec!["m".into()],
                enabled_models: vec!["m".into()],
                enabled: true,
                api_format: "openai_chat".into(),
                model_overrides: Default::default(),
                compress_request_body: false,
                request: Default::default(),
            };
            let adapter = OpenAiChatProvider::new(&state, &provider, 1);
            let request = GenerateRequest {
                model: "m".into(),
                system: "sys".into(),
                messages: vec![ModelMessage {
                    role: ModelRole::User,
                    content: vec![MessagePart::Text { text: "hi".into() }],
                }],
                tools: Vec::new(),
                options: GenerateOptions::default(),
                metadata: crate::chat::model::RequestMetadata {
                    conversation_id: Some("conv_abc".into()),
                    ..Default::default()
                },
            };
            adapter.request_body(&request, false)
        };

        // 未学习：中转站也硬发。
        assert_eq!(
            make("https://integrate.api.nvidia.com/v1")["prompt_cache_key"],
            "conv_abc"
        );
        // 学习该端点拒绝后：就地跳过。
        state.mark_prompt_cache_key_unsupported("https://integrate.api.nvidia.com/v1");
        assert!(make("https://integrate.api.nvidia.com/v1")
            .get("prompt_cache_key")
            .is_none());
        // 其它端点不受影响，仍发送。
        assert_eq!(
            make("https://api.openai.com/v1")["prompt_cache_key"],
            "conv_abc"
        );
    }

    #[test]
    fn prompt_caching_toggle_gates_the_cache_key() {
        // retention=none 不发；short 发 key；long 发 key+24h（对齐 pi，不按主机降级）。
        let state =
            AppState::new_headless(crate::settings::Settings::default(), std::env::temp_dir());
        let body_with = |base_url: &str, retention: &str| {
            let provider = ModelProvider {
                id: "test".into(),
                name: "Test".into(),
                api_keys: vec!["sk-test".into()],
                api_key_legacy: None,
                base_url: base_url.into(),
                available_models: vec!["m".into()],
                enabled_models: vec!["m".into()],
                enabled: true,
                api_format: "openai_chat".into(),
                model_overrides: Default::default(),
                compress_request_body: false,
                request: crate::settings::ProviderRequestConfig {
                    prompt_cache_retention: retention.into(),
                    ..Default::default()
                },
            };
            let adapter = OpenAiChatProvider::new(&state, &provider, 1);
            adapter.request_body(
                &GenerateRequest {
                    model: "m".into(),
                    system: "sys".into(),
                    messages: vec![ModelMessage {
                        role: ModelRole::User,
                        content: vec![MessagePart::Text { text: "hi".into() }],
                    }],
                    tools: Vec::new(),
                    options: GenerateOptions::default(),
                    metadata: crate::chat::model::RequestMetadata {
                        conversation_id: Some("conv_abc".into()),
                        ..Default::default()
                    },
                },
                false,
            )
        };
        let short = body_with("https://api.openai.com/v1", "short");
        assert_eq!(short["prompt_cache_key"], "conv_abc");
        assert!(short.get("prompt_cache_retention").is_none());
        assert!(body_with("https://api.openai.com/v1", "none")
            .get("prompt_cache_key")
            .is_none());
        let long = body_with("https://api.openai.com/v1", "long");
        assert_eq!(long["prompt_cache_key"], "conv_abc");
        assert_eq!(long["prompt_cache_retention"], "24h");
        let relay = body_with("https://api.deepseek.com/v1", "long");
        assert_eq!(relay["prompt_cache_key"], "conv_abc");
        assert_eq!(relay["prompt_cache_retention"], "24h");
        // 学习停发 24h 后仍保留 key。
        state.mark_prompt_cache_retention_unsupported("https://api.deepseek.com/v1");
        let after = body_with("https://api.deepseek.com/v1", "long");
        assert_eq!(after["prompt_cache_key"], "conv_abc");
        assert!(after.get("prompt_cache_retention").is_none());
        // 默认 short：OpenAI 与 Anthropic 均启用客户端缓存字段。
        let mut p = ModelProvider {
            id: "x".into(),
            name: "x".into(),
            api_keys: vec!["k".into()],
            api_key_legacy: None,
            base_url: "https://api.openai.com/v1".into(),
            available_models: Vec::new(),
            enabled_models: Vec::new(),
            enabled: true,
            api_format: "openai_chat".into(),
            model_overrides: Default::default(),
            compress_request_body: false,
            request: Default::default(),
        };
        assert_eq!(p.request.prompt_cache_retention, "short");
        assert!(p.prompt_caching_enabled());
        p.api_format = "anthropic_messages".into();
        assert!(p.prompt_caching_enabled(), "Anthropic 默认 short 也应启用");
    }

    #[test]
    fn error_rejects_prompt_cache_key_matches_field_name() {
        assert!(super::error_rejects_prompt_cache_key(
            "400 Bad Request - {\"message\":\"Unsupported parameter(s): `prompt_cache_key`\"}"
        ));
        assert!(!super::error_rejects_prompt_cache_key(
            "400 Bad Request - {\"message\":\"some other validation error\"}"
        ));
    }

    #[test]
    fn parses_text_reasoning_and_tool_calls() {
        let value = serde_json::json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": "I'll call a tool.",
                    "reasoning_content": "Need data.",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "web_search",
                            "arguments": "{\"query\":\"kivio\"}"
                        }
                    }]
                }
            }],
            "usage": {"prompt_tokens": 3, "completion_tokens": 5, "total_tokens": 8}
        });

        let output = output_from_chat_completion(&value, "{}", "test").expect("output");

        assert_eq!(output.text, "I'll call a tool.");
        assert_eq!(output.reasoning.as_deref(), Some("Need data."));
        assert_eq!(output.tool_calls.len(), 1);
        assert_eq!(output.tool_calls[0].function_name, "web_search");
        assert_eq!(
            output.usage.as_ref().and_then(|usage| usage.total_tokens),
            Some(8)
        );
        assert_eq!(output.finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn stream_tool_call_chunks_become_stream_parts() {
        let mut parts = Vec::new();
        let mut sink = |part| {
            parts.push(part);
            Ok(())
        };
        let mut partials = Vec::new();
        let start = serde_json::json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {"name": "web_search", "arguments": "{\"q\""}
                    }]
                }
            }]
        });
        let next = serde_json::json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {"arguments": ":\"rust\"}"}
                    }]
                }
            }]
        });

        handle_openai_stream_tool_calls(&start, &mut partials, &mut sink).expect("start");
        handle_openai_stream_tool_calls(&next, &mut partials, &mut sink).expect("next");
        let calls = finish_tool_call_partials(&mut partials, &mut sink).expect("finish");

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["q"], "rust");
        assert!(matches!(parts[0], StreamPart::ToolCallStart { .. }));
        assert!(matches!(
            parts.last(),
            Some(StreamPart::ToolCallDone { .. })
        ));
    }

    #[test]
    fn stream_tool_call_keeps_first_chunk_id_over_empty_continuation() {
        // SenseNova 等:首片给真 id,续片给空 id `""`。空续片不得覆盖真 id。
        let mut parts = Vec::new();
        let mut sink = |part| {
            parts.push(part);
            Ok(())
        };
        let mut partials = Vec::new();
        let start = serde_json::json!({
            "choices": [{ "delta": { "tool_calls": [{
                "index": 0, "id": "call_real", "function": {"name": "bash", "arguments": "{\"c\""}
            }]}}]
        });
        let cont = serde_json::json!({
            "choices": [{ "delta": { "tool_calls": [{
                "index": 0, "id": "", "function": {"arguments": ":\"x\"}"}
            }]}}]
        });
        handle_openai_stream_tool_calls(&start, &mut partials, &mut sink).expect("start");
        handle_openai_stream_tool_calls(&cont, &mut partials, &mut sink).expect("cont");
        let calls = finish_tool_call_partials(&mut partials, &mut sink).expect("finish");
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].id, "call_real",
            "empty continuation id must not overwrite real id"
        );
        assert_eq!(calls[0].arguments["c"], "x");
    }

    #[test]
    fn stream_tool_call_synthesizes_id_when_all_empty() {
        // id 从头到尾都空 → 生成合法非空 id,绝不发空 tool_call_id。
        let mut parts = Vec::new();
        let mut sink = |part| {
            parts.push(part);
            Ok(())
        };
        let mut partials = Vec::new();
        let chunk = serde_json::json!({
            "choices": [{ "delta": { "tool_calls": [{
                "index": 0, "id": "", "function": {"name": "bash", "arguments": "{}"}
            }]}}]
        });
        handle_openai_stream_tool_calls(&chunk, &mut partials, &mut sink).expect("chunk");
        let calls = finish_tool_call_partials(&mut partials, &mut sink).expect("finish");
        assert_eq!(calls.len(), 1);
        assert!(!calls[0].id.is_empty(), "id must not be empty");
        assert_eq!(calls[0].id, "call_0");
    }

    #[test]
    fn stream_tool_call_accepts_object_form_arguments() {
        // 部分 OpenAI 兼容网关（如 codex/spark 代理）把 function.arguments 作为完整 JSON
        // 对象一次性发来，而非字符串分片。回归：旧逻辑只认字符串会丢成 `{}`，使必填参数缺失。
        let mut parts = Vec::new();
        let mut sink = |part| {
            parts.push(part);
            Ok(())
        };
        let mut partials = Vec::new();
        let chunk = serde_json::json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": { "name": "web_search", "arguments": { "query": "吉林市 明天 天气" } }
                    }]
                }
            }]
        });

        handle_openai_stream_tool_calls(&chunk, &mut partials, &mut sink).expect("chunk");
        let calls = finish_tool_call_partials(&mut partials, &mut sink).expect("finish");

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function_name, "web_search");
        assert_eq!(calls[0].arguments["query"], "吉林市 明天 天气");
        assert!(calls[0].arguments_parse_error.is_none());
    }

    #[test]
    fn parses_message_images_from_openrouter_style_response() {
        // 代理仿 OpenRouter：非流式 message.images 携带 data URL 图片数组。
        let value = serde_json::json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {
                    "role": "assistant",
                    "content": "here is your image",
                    "images": [{
                        "type": "image_url",
                        "image_url": { "url": "data:image/png;base64,aGVsbG8=" }
                    }]
                }
            }]
        });

        let output = output_from_chat_completion(&value, "{}", "test").expect("output");
        assert_eq!(output.text, "here is your image");
        assert_eq!(output.images.len(), 1);
        assert_eq!(output.images[0].mime_type, "image/png");
        assert_eq!(output.images[0].data, "aGVsbG8=");
    }

    #[test]
    fn response_without_images_has_empty_images() {
        // 无 images 字段：行为字节等价，images 为空（普通 chat 非回归）。
        let value = serde_json::json!({
            "choices": [{
                "finish_reason": "stop",
                "message": { "role": "assistant", "content": "plain text" }
            }]
        });
        let output = output_from_chat_completion(&value, "{}", "test").expect("output");
        assert!(output.images.is_empty());
    }

    #[test]
    fn stream_delta_images_emit_image_data_and_aggregate() {
        // 流式 delta.images：解析出图 → 发 StreamPart::ImageData 且累积进最终 images。
        // 复刻 stream_inner 每 chunk 的 emit + push 逻辑（无需真实 HTTP）。
        let chunk = serde_json::json!({
            "choices": [{
                "delta": {
                    "images": [{
                        "type": "image_url",
                        "image_url": { "url": "data:image/webp;base64,d29ybGQ=" }
                    }]
                }
            }]
        });

        let mut parts = Vec::new();
        let mut images: Vec<GeneratedImageData> = Vec::new();
        {
            let mut sink = |part| {
                parts.push(part);
                Ok::<(), ModelError>(())
            };
            for image in extract_chat_stream_images(&chunk) {
                sink.emit(StreamPart::ImageData {
                    mime_type: image.mime_type.clone(),
                    data: image.data.clone(),
                })
                .expect("emit");
                images.push(image);
            }
        }

        assert_eq!(images.len(), 1);
        assert_eq!(images[0].mime_type, "image/webp");
        assert_eq!(images[0].data, "d29ybGQ=");
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            StreamPart::ImageData { mime_type, data } => {
                assert_eq!(mime_type, "image/webp");
                assert_eq!(data, "d29ybGQ=");
            }
            other => panic!("expected ImageData, got {other:?}"),
        }
    }

    #[test]
    fn stream_delta_without_images_yields_nothing() {
        // delta 无 images：解析为空（普通流式 chat 非回归）。
        let chunk = serde_json::json!({
            "choices": [{ "delta": { "content": "hi" } }]
        });
        assert!(extract_chat_stream_images(&chunk).is_empty());
    }

    #[test]
    fn parse_openai_image_data_url_skips_http_and_non_base64() {
        // http(s) 直链、非 base64 data URL、非 data URL 均静默跳过（返回 None，不报错）。
        assert!(parse_openai_image_data_url("https://example.com/a.png").is_none());
        assert!(parse_openai_image_data_url("data:image/png,notbase64").is_none());
        assert!(parse_openai_image_data_url("not-a-url").is_none());
        let ok = parse_openai_image_data_url("data:image/jpeg;base64,aGVsbG8=").expect("parsed");
        assert_eq!(ok.mime_type, "image/jpeg");
        assert_eq!(ok.data, "aGVsbG8=");
    }

    #[test]
    fn stream_tool_call_replays_arguments_buffered_before_start() {
        let mut parts = Vec::new();
        let mut partials = Vec::new();
        let early_arguments = serde_json::json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {"arguments": "{\"path\":\"demo.txt\""}
                    }]
                }
            }]
        });
        let delayed_start = serde_json::json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_delayed",
                        "function": {"name": "write_file"}
                    }]
                }
            }]
        });
        let final_arguments = serde_json::json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {"arguments": ",\"content\":\"ok\"}"}
                    }]
                }
            }]
        });

        {
            let mut sink = |part| {
                parts.push(part);
                Ok(())
            };
            handle_openai_stream_tool_calls(&early_arguments, &mut partials, &mut sink)
                .expect("early arguments");
        }
        assert!(parts.is_empty());
        let mut sink = |part| {
            parts.push(part);
            Ok(())
        };
        handle_openai_stream_tool_calls(&delayed_start, &mut partials, &mut sink)
            .expect("delayed start");
        handle_openai_stream_tool_calls(&final_arguments, &mut partials, &mut sink)
            .expect("final arguments");
        let calls = finish_tool_call_partials(&mut partials, &mut sink).expect("finish");

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_delayed");
        assert_eq!(calls[0].arguments["content"], "ok");
        assert!(matches!(parts[0], StreamPart::ToolCallStart { .. }));
        assert!(matches!(
            &parts[1],
            StreamPart::ToolCallDelta { delta, .. } if delta.contains("demo.txt")
        ));
        assert!(matches!(
            parts.last(),
            Some(StreamPart::ToolCallDone { .. })
        ));
    }
}
