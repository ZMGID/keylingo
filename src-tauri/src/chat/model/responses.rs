//! OpenAI **Responses API** adapter (`POST /v1/responses`).
//!
//! Peer to `openai.rs` (Chat Completions) and `anthropic.rs` (Messages). Codex /
//! Responses-native models (and proxies wrapping them) often emit tool-call arguments
//! ONLY over this protocol's streaming events (`response.function_call_arguments.*`) —
//! on Chat Completions they return empty `arguments`. This adapter speaks the Responses
//! wire format while presenting the same provider-agnostic `LanguageModelProvider`
//! surface, so the agent loop is unchanged.
//!
//! Conversation state is modelled as a flat `input` item list rather than chat messages:
//! tool calls are `function_call` items and tool results are `function_call_output`
//! items (see `responses_input_from_model_messages` in `types.rs`).

use reqwest::header::ACCEPT_ENCODING;
use serde_json::Value;

use crate::api::{send_with_failover, with_chat_request_timeout};
use crate::settings::{ModelProvider, ProviderApiFormat};
use crate::state::AppState;
use crate::usage::{
    chat_usage_source_for_label, error_kind_from_message, model_usage_from_openai_value,
    operation_from_label, record_model_call, UsageRecordInput,
};

use super::{
    parse_tool_arguments, responses_input_from_model_messages, stream_read_error, BuiltinWebSearch,
    FirstTokenStreamSink, GenerateOutput, GenerateRequest, GeneratedImageData,
    LanguageModelProvider, ModelError, ModelFuture, ModelUsage, PendingToolCall, StreamPart,
    StreamSink, WebCitation,
};

/// UI 档位 → xAI 官方 effort。
///
/// 「关闭思考」不经这里：`resolve_thinking` 把 UI `off` 变成 `(enabled=false, level=None)`，
/// 适配器在 `!thinking_enabled` 分支直接发 `reasoning.effort: "none"`。本函数只映射
/// 真正的等级字符串（`low/medium/high/xhigh/max`，以及偶发透传的 `minimal`/`none`）。
fn xai_reasoning_effort(effort: &str) -> Option<&'static str> {
    match effort.trim().to_ascii_lowercase().as_str() {
        "none" | "off" => Some("none"),
        "minimal" | "low" => Some("low"),
        "medium" => Some("medium"),
        "high" | "max" => Some("high"),
        "xhigh" => Some("xhigh"),
        _ => None,
    }
}

/// grok 的 `include`：按已启用的服务端工具列出要回传的字段，不显式 include 的话
/// 搜索来源和执行输出根本不会回来。没有服务端工具时返回空（调用方据此不发该字段）。
fn xai_include_values(tools: Option<&Value>) -> Vec<Value> {
    let mut values: Vec<Value> = Vec::new();
    let Some(tools) = tools.and_then(Value::as_array) else {
        return values;
    };
    for tool in tools {
        let extra = match tool.get("type").and_then(Value::as_str).map(str::trim) {
            Some("web_search") => "web_search_call.action.sources",
            Some("file_search") => "file_search_call.results",
            Some("code_interpreter") => "code_interpreter_call.outputs",
            _ => continue,
        };
        let extra = Value::String(extra.to_string());
        if !values.contains(&extra) {
            values.push(extra);
        }
    }
    values
}

pub struct OpenAiResponsesProvider<'a> {
    state: &'a AppState,
    provider: &'a ModelProvider,
    retry_attempts: usize,
}

impl<'a> OpenAiResponsesProvider<'a> {
    pub fn new(state: &'a AppState, provider: &'a ModelProvider, retry_attempts: usize) -> Self {
        Self {
            state,
            provider,
            retry_attempts,
        }
    }
}

impl LanguageModelProvider for OpenAiResponsesProvider<'_> {
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

impl OpenAiResponsesProvider<'_> {
    /// 发送 Responses 请求；若严格端点拒绝 `prompt_cache_key` / `prompt_cache_retention`
    /// 而 400，记入 state 后去掉对应字段重试（与 openai.rs::send_chat_body 同形状）。
    async fn send_responses_body(
        &self,
        request: &GenerateRequest,
        stream: bool,
        label: &str,
    ) -> Result<reqwest::Response, String> {
        let body = self.request_body(request, stream);
        let result = self.post_responses(request, &body, stream, label).await;
        if let Err(ref err) = result {
            let mut learned = false;
            if body.get("prompt_cache_key").is_some()
                && super::openai::error_rejects_prompt_cache_key(err)
            {
                self.state
                    .mark_prompt_cache_key_unsupported(&self.provider.base_url);
                learned = true;
            } else if body.get("prompt_cache_retention").is_some()
                && super::openai::error_rejects_prompt_cache_retention(err)
            {
                self.state
                    .mark_prompt_cache_retention_unsupported(&self.provider.base_url);
                learned = true;
            }
            if learned {
                let retry_body = self.request_body(request, stream);
                return self
                    .post_responses(request, &retry_body, stream, label)
                    .await;
            }
        }
        result
    }

    /// 单次发送（带多 key failover）。非流式套总超时；流式不套，避免活跃 SSE 被砍断。
    async fn post_responses(
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
                    crate::provider_request::apply(
                        self.state
                            .client_for(self.provider)
                            .post(self.responses_url())
                            .bearer_auth(key)
                            .header(ACCEPT_ENCODING, "identity"),
                        self.provider,
                        request.metadata.conversation_id.as_deref(),
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
        let label = request_label(&request, "Responses API");
        let started_at = chrono::Local::now().timestamp();
        let started = std::time::Instant::now();
        let response = self
            .send_responses_body(&request, false, &label)
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
        match serde_json::from_str::<Value>(&raw) {
            Ok(value) => {
                let output = output_from_responses(&value, &raw, &label)?;
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
            Err(json_err) => {
                // Some Responses-API proxies stream an SSE body even when the request
                // sets `stream:false`. The body is then `event: …\ndata: {…}` lines, not a
                // JSON object, so `from_str` fails. Tolerate that: parse the SSE body
                // through the same accumulation the streaming path uses.
                if !looks_like_sse(&raw) {
                    let message = format!(
                        "{label} parse JSON: {} (body: {})",
                        json_err,
                        raw.chars().take(500).collect::<String>()
                    );
                    self.record_usage_failure(
                        &request,
                        &label,
                        started_at,
                        started.elapsed(),
                        &message,
                    );
                    self.record_debug_failure(
                        &request,
                        &label,
                        false,
                        &message,
                        started_at,
                        started.elapsed(),
                    );
                    return Err(ModelError::new(message));
                }
                match output_from_sse_body(&raw) {
                    Ok(output) => {
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
                    Err(err) => {
                        self.record_usage_failure(
                            &request,
                            &label,
                            started_at,
                            started.elapsed(),
                            &err.to_string(),
                        );
                        self.record_debug_failure(
                            &request,
                            &label,
                            false,
                            &err.to_string(),
                            started_at,
                            started.elapsed(),
                        );
                        Err(err)
                    }
                }
            }
        }
    }

    async fn stream_inner(
        &self,
        request: GenerateRequest,
        sink: &mut (dyn StreamSink + Send),
    ) -> Result<GenerateOutput, ModelError> {
        let label = request_label(&request, "Responses stream");
        let started_at = chrono::Local::now().timestamp();
        let started = std::time::Instant::now();
        let mut measured_sink = FirstTokenStreamSink::new(sink, started);
        let sink = &mut measured_sink;
        let mut response = self
            .send_responses_body(&request, true, &label)
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
        let mut state = ResponsesStreamState::default();

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
                if let Some(err) = process_sse_line(&line, &mut state, sink)? {
                    self.record_usage_failure(
                        &request,
                        &label,
                        started_at,
                        started.elapsed(),
                        &err,
                    );
                    self.record_debug_failure(
                        &request,
                        &label,
                        true,
                        &err,
                        started_at,
                        started.elapsed(),
                    );
                    return Err(ModelError::new(err));
                }
            }
        }

        let output = state.finish(sink)?;
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

    fn responses_url(&self) -> String {
        format!("{}/responses", self.provider.base_url.trim_end_matches('/'))
    }

    fn request_body(&self, request: &GenerateRequest, stream: bool) -> Value {
        // 协议来自用户在设置里选的「Grok (xAI)」，不是猜 base_url——中转站可以把 grok
        // 挂在任意域名上，靠域名判断必然漏。
        let is_xai = self.provider.api_format_kind() == ProviderApiFormat::XaiResponses;
        let mut body = serde_json::json!({
            "model": request.model,
            "input": responses_input_from_model_messages(&request.messages),
        });
        if let Some(temperature) = crate::chat::model_metadata::temperature_for_request(
            request.options.temperature,
            Some(self.provider),
            &request.model,
        ) {
            body["temperature"] = serde_json::json!(temperature);
        }
        if !request.system.trim().is_empty() {
            if is_xai {
                // 放进 `input` 首位而不是用 `instructions`。
                //
                // xAI 的文档把 `instructions` 列在 `/v1/responses` 的请求体参数里，所以它
                // 大概是能收的；但 `input` 里带 `role:system` 项在两种情况下都成立，是更
                // 稳的一条路。参考实现（LiveAgent）把 `instructions` 直接删掉不补——照抄
                // 会让系统提示词凭空消失，所以是**搬**不是丢。
                if let Some(input) = body["input"].as_array_mut() {
                    input.insert(
                        0,
                        serde_json::json!({
                            "role": "system",
                            "content": [{ "type": "input_text", "text": request.system }],
                        }),
                    );
                }
            } else {
                body["instructions"] = Value::String(request.system.clone());
            }
        }
        if request.options.max_tokens > 0 {
            body["max_output_tokens"] = Value::from(request.options.max_tokens);
        }
        if stream {
            body["stream"] = Value::Bool(true);
        }
        // 工具 = 客户端函数工具 +（可选）模型原生内置联网搜索（任务 07-23）。
        // 内置搜索仅 Responses 端点支持；`builtin_web_search` 由会话 Builtin 模式驱动。
        let mut tools_arr: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| tool.to_openai_responses_tool())
            .collect();
        if request.options.builtin_web_search {
            tools_arr.push(serde_json::json!({ "type": "web_search" }));
        }
        if !tools_arr.is_empty() {
            body["tools"] = Value::Array(tools_arr);
            body["tool_choice"] = Value::String("auto".to_string());
        }
        // 思考 → Responses `reasoning.effort`。
        //
        // UI Off（`thinking_enabled=false`）必须**显式**发 `"none"`：OpenAI 把 none 列为一等
        // effort；DeepSeek Responses 文档也写 `none/low/high/max`（none = 关闭思考），且
        // **默认思考开、effort=high**——省略字段等于白关。xAI 部分模型文档写「不能关」，
        // 仍按用户选择发 none，而不是静默默认到 high。
        //
        // 选了具体档位则原样下发（档位由模型库 reasoningEfforts 门控）。
        // 注意：gpt-5 系列在 minimal reasoning 下不执行 hosted web_search；resolve_thinking
        // 已把「未显式设档」默认成 high，故内置搜索天然拿到非 minimal。
        if !request.options.thinking_enabled {
            body["reasoning"] = serde_json::json!({ "effort": "none" });
            // 关思考仍走无状态：不依赖服务端会话，也不要 encrypted reasoning。
            body["store"] = Value::Bool(false);
        } else if let Some(effort) = request.options.thinking_level.as_deref() {
            if is_xai {
                // xAI 有自己的一套 effort 档位。
                if let Some(mapped) = xai_reasoning_effort(effort) {
                    body["reasoning"] = serde_json::json!({ "effort": mapped });
                }
            } else {
                body["reasoning"] = serde_json::json!({ "effort": effort });
                // 无状态模式：Responses 的 `store` 默认 true（服务端保存会话状态并按
                // response id 串联轮次）。我们每轮都自带完整 input，不依赖服务端状态，
                // 让服务端白存一份没有意义；代理渠道多半也没真正实现存储。
                //
                // 与 Codex 官方客户端对齐：同一模型/同一渠道，它发的就是 store:false +
                // include:["reasoning.encrypted_content"]（见抓包 trace_dd26cbe7）——
                // 思考链靠 encrypted_content 随响应回传，不依赖服务端存储。
                //
                // 注意：这不能修复该渠道对大请求的间歇性 502（同一 payload 重打 5 次 2 失败，
                // 与本字段无关，见 commit message）。此处只是对齐官方客户端的正确用法。
                body["store"] = Value::Bool(false);
                body["include"] = serde_json::json!(["reasoning.encrypted_content"]);
            }
        }
        if is_xai {
            // **必须显式关掉服务端存储。** xAI 的 Responses 是有状态设计，`store` 默认 true，
            // 文档原文：「New responses will be stored for 30 days and then permanently
            // deleted.」我们每轮都自带完整 input、从不使用 `previous_response_id`，那份
            // 服务端副本对我们毫无用处，却让用户的每一次 Grok 对话在 xAI 存 30 天。
            // 与本适配器在 OpenAI Responses 上的做法一致。
            body["store"] = Value::Bool(false);

            // grok 的服务端工具只有显式 include 才回传来源与执行输出，不 include 就拿不到
            // 搜索来源。没有服务端工具时不必发这个字段。
            //
            // 注意这里**不带** `reasoning.encrypted_content`：加密推理项在 `types.rs` 的
            // 回放里是被丢弃的（"Reasoning is omitted on replay"），要来只是让每次响应多背
            // 一份没人消费的密文。
            let include = xai_include_values(body.get("tools"));
            if include.is_empty() {
                body.as_object_mut().map(|o| o.remove("include"));
            } else {
                body["include"] = Value::Array(include);
            }
        }
        // Prompt 缓存 short/long：与 Chat Completions 同发 `prompt_cache_key`；
        // long → `prompt_cache_retention:24h`（对齐 pi）。xAI 永不发（靠 previous_response_id）。
        if !is_xai
            && self.provider.prompt_caching_enabled()
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
        if let Some(overrides) = request.options.provider_options.as_object() {
            for (key, value) in overrides {
                body[key] = value.clone();
            }
        }
        body
    }

    /// 重建本次请求实际会带的 headers（脱敏后）供请求调试面板展示。镜像发送路径：
    /// bearer_auth(key) + Accept-Encoding identity + JSON content-type。
    /// Authorization 用首个 key（正常发送用的也是它）派生脱敏预览。
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
        for (name, value) in crate::provider_request::header_pairs(
            self.provider,
            metadata.conversation_id.as_deref(),
        ) {
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
                url: self.responses_url(),
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
                url: self.responses_url(),
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

/// A function-call item being assembled across streaming events. Keyed internally by
/// `item_id` (the `fc_…` id the delta/done events reference); `call_id` (the `call_…`
/// id) is what the model expects echoed back as `function_call_output.call_id`.
struct ResponsesToolPartial {
    item_id: String,
    call_id: String,
    name: String,
    arguments: String,
    done: bool,
}

#[derive(Default)]
struct ResponsesStreamState {
    text: String,
    reasoning: String,
    tool_calls: Vec<ResponsesToolPartial>,
    finish_reason: Option<String>,
    usage: Option<ModelUsage>,
    /// 内置搜索引用：在 `response.completed` 事件里从完整 `response.output` 一次性解析。
    web_search: Option<BuiltinWebSearch>,
    /// grok(xAI) 特有:`web_search_call.action.sources[]` 是检索命中的 URL 列表。
    /// 质量低于正文 `url_citation`(模型逐句引用),故只收集不直接入 citations;
    /// `finish()` 时若整轮没有任何 url_citation(grok 搜完常直接岔去客户端 web_fetch、
    /// 该轮不产正文注解)才把 sources 转正为 citations 兜底,保证搜索卡有来源可显示。
    sources_fallback: Vec<WebCitation>,
    /// hosted image generation（`image_generation_call`）产出的图。gpt-5.x 在
    /// Responses 上可自行决定用托管出图回答，此时 `message.output_text` 是空串
    /// ——图就是答案。不收这些图会让整轮变成「空助手响应」而报错。
    images: Vec<GeneratedImageData>,
}

impl ResponsesStreamState {
    fn partial_mut(&mut self, item_id: &str) -> Option<&mut ResponsesToolPartial> {
        self.tool_calls
            .iter_mut()
            .find(|partial| partial.item_id == item_id)
    }

    /// 累加一个内置搜索查询词（去重）。流式下 web_search_call 走 `output_item.done` 事件，
    /// 且不重现在 `response.completed` 的 output 里，故必须边流边收。
    fn push_web_search_query(&mut self, query: &str) {
        let query = query.trim();
        if query.is_empty() {
            return;
        }
        let ws = self
            .web_search
            .get_or_insert_with(BuiltinWebSearch::default);
        if !ws.queries.iter().any(|q| q == query) {
            ws.queries.push(query.to_string());
        }
    }

    /// 累加一条内置搜索来源（按 url 去重）。流式下走 `output_text.annotation.added` 事件。
    fn push_web_search_citation(&mut self, url: &str, title: &str) {
        let url = url.trim();
        if url.is_empty() {
            return;
        }
        let ws = self
            .web_search
            .get_or_insert_with(BuiltinWebSearch::default);
        if !ws.citations.iter().any(|c| c.url == url) {
            ws.citations.push(WebCitation {
                title: title.trim().to_string(),
                url: url.to_string(),
                ..Default::default()
            });
        }
    }

    /// 发一帧内置搜索实时进度（任务 07-23）：把当前累加的查询/来源快照发给 sink，sink 据此
    /// 实时开牌 / 更新「网络搜索」卡（置于答案文本之前）。首帧（web_search 尚为 None）发空快照
    /// = 开牌 Running。无头 sink（非流式 SSE 兜底）忽略此 part，行为不变。
    fn emit_web_search_snapshot(
        &self,
        sink: &mut (dyn StreamSink + Send),
    ) -> Result<(), ModelError> {
        let (queries, citations) = match &self.web_search {
            Some(ws) => (ws.queries.clone(), ws.citations.clone()),
            None => (Vec::new(), Vec::new()),
        };
        sink.emit(StreamPart::WebSearch { queries, citations })
    }

    fn finalize_tool_call(
        &mut self,
        item_id: &str,
        sink: &mut (dyn StreamSink + Send),
    ) -> Result<(), ModelError> {
        if let Some(partial) = self.tool_calls.iter_mut().find(|p| p.item_id == item_id) {
            if partial.done {
                return Ok(());
            }
            partial.done = true;
            let call = pending_tool_call_from_partial(partial);
            sink.emit(StreamPart::ToolCallDone { call })?;
        }
        Ok(())
    }

    fn finish(mut self, sink: &mut (dyn StreamSink + Send)) -> Result<GenerateOutput, ModelError> {
        // Flush any function calls that never received an explicit output_item.done.
        let pending_ids: Vec<String> = self
            .tool_calls
            .iter()
            .filter(|partial| !partial.done)
            .map(|partial| partial.item_id.clone())
            .collect();
        for item_id in pending_ids {
            self.finalize_tool_call(&item_id, sink)?;
        }
        let tool_calls: Vec<PendingToolCall> = self
            .tool_calls
            .iter()
            .map(pending_tool_call_from_partial)
            .collect();
        let reason = self.finish_reason.clone().unwrap_or_else(|| {
            if tool_calls.is_empty() {
                "stop".to_string()
            } else {
                "tool_calls".to_string()
            }
        });
        sink.emit(StreamPart::Finish {
            reason: reason.clone(),
            full: self.text.clone(),
        })?;
        // grok sources 兜底:整轮没有任何正文 url_citation(grok 搜完常直接岔去客户端
        // web_fetch,该轮不产注解)时,把检索命中的 sources URL 转正为来源;有 url_citation
        // 时以注解为准,sources 丢弃。
        let citations_empty = self
            .web_search
            .as_ref()
            .map(|ws| ws.citations.is_empty())
            .unwrap_or(true);
        if citations_empty && !self.sources_fallback.is_empty() {
            let ws = self
                .web_search
                .get_or_insert_with(BuiltinWebSearch::default);
            ws.citations = std::mem::take(&mut self.sources_fallback);
        }
        // 终帧同步:部分网关(如 loki)在 response.completed 里重现 web_search_call,
        // completed 合并只更新 state.web_search、不发帧(防重复),实时卡会漏掉这些来源。
        // 结束前无条件发一帧最终快照,保证实时卡与 GenerateOutput 一致(tracker 去重,幂等)。
        if let Some(ws) = self.web_search.as_ref().filter(|ws| !ws.is_empty()) {
            sink.emit(StreamPart::WebSearch {
                queries: ws.queries.clone(),
                citations: ws.citations.clone(),
            })?;
        }
        Ok(GenerateOutput {
            text: self.text,
            reasoning: non_empty(self.reasoning),
            tool_calls,
            usage: self.usage,
            finish_reason: Some(reason),
            provider_messages: Vec::new(),
            cancelled: false,
            web_search: self.web_search,
            images: self.images,
        })
    }
}

/// 从一个 `image_generation_call` output item 取出生成的图。`result` 是裸 base64
/// （无 `data:` 前缀），mime 由 `output_format`（png/jpeg/webp）推出，缺省 png。
/// 解析尽力而为：没有 `result` 就返回 None，绝不 panic。
fn responses_image_from_item(item: &Value) -> Option<GeneratedImageData> {
    let data = item
        .get("result")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let format = item
        .get("output_format")
        .and_then(Value::as_str)
        .unwrap_or("png")
        .trim()
        .to_ascii_lowercase();
    let mime_type = match format.as_str() {
        "jpeg" | "jpg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "image/png",
    };
    Some(GeneratedImageData {
        mime_type: mime_type.to_string(),
        data: data.to_string(),
    })
}

/// Dispatch a single streaming Responses event into the accumulating state, emitting
/// `StreamPart`s as content arrives. Returns `Ok(Some(err))` for a terminal provider
/// error (caller records the failure and aborts), `Ok(None)` otherwise. Free function
/// (no `&self`) so the stream loop and unit tests share one code path.
fn handle_responses_stream_event(
    value: &Value,
    state: &mut ResponsesStreamState,
    sink: &mut (dyn StreamSink + Send),
) -> Result<Option<String>, ModelError> {
    let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    match event_type {
        "response.output_text.delta" => {
            if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                if !delta.is_empty() {
                    state.text.push_str(delta);
                    sink.emit(StreamPart::TextDelta {
                        delta: delta.to_string(),
                    })?;
                }
            }
        }
        "response.reasoning_summary_text.delta"
        | "response.reasoning_text.delta"
        | "response.reasoning_summary.delta" => {
            if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                if !delta.is_empty() {
                    state.reasoning.push_str(delta);
                    sink.emit(StreamPart::ReasoningDelta {
                        delta: delta.to_string(),
                    })?;
                }
            }
        }
        "response.output_item.added" => {
            if let Some(item) = value.get("item") {
                match item.get("type").and_then(Value::as_str) {
                    Some("function_call") => {
                        let item_id = item
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let call_id = item
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or(&item_id)
                            .to_string();
                        let name = item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        sink.emit(StreamPart::ToolCallStart {
                            id: call_id.clone(),
                            name: name.clone(),
                        })?;
                        state.tool_calls.push(ResponsesToolPartial {
                            item_id,
                            call_id,
                            name,
                            arguments: String::new(),
                            done: false,
                        });
                    }
                    // 内置搜索开始：查询词此刻还没到，发一帧空快照 = 立即开一张 Running 卡。
                    Some("web_search_call") => {
                        state.emit_web_search_snapshot(sink)?;
                    }
                    _ => {}
                }
            }
        }
        "response.function_call_arguments.delta" => {
            let item_id = value.get("item_id").and_then(Value::as_str).unwrap_or("");
            if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                if let Some(partial) = state.partial_mut(item_id) {
                    partial.arguments.push_str(delta);
                    sink.emit(StreamPart::ToolCallDelta {
                        id: partial.call_id.clone(),
                        delta: delta.to_string(),
                    })?;
                }
            }
        }
        "response.function_call_arguments.done" => {
            let item_id = value.get("item_id").and_then(Value::as_str).unwrap_or("");
            if let Some(arguments) = value.get("arguments").and_then(Value::as_str) {
                if let Some(partial) = state.partial_mut(item_id) {
                    // The `done` event carries the full argument string — prefer it over
                    // the accumulated deltas to avoid any drift.
                    partial.arguments = arguments.to_string();
                }
            }
        }
        "response.output_item.done" => {
            if let Some(item) = value.get("item") {
                match item.get("type").and_then(Value::as_str) {
                    Some("function_call") => {
                        let item_id = item.get("id").and_then(Value::as_str).unwrap_or("");
                        if let Some(arguments) = item.get("arguments").and_then(Value::as_str) {
                            if let Some(partial) = state.partial_mut(item_id) {
                                if !arguments.is_empty() {
                                    partial.arguments = arguments.to_string();
                                }
                            }
                        }
                        state.finalize_tool_call(item_id, sink)?;
                    }
                    // 内置搜索：hosted web_search 的查询词在此事件到达（completed 的 output 里不重现）。
                    Some("web_search_call") => {
                        if let Some(action) = item.get("action") {
                            if let Some(q) = action.get("query").and_then(Value::as_str) {
                                state.push_web_search_query(q);
                            }
                            if let Some(list) = action.get("queries").and_then(Value::as_array) {
                                for q in list.iter().filter_map(Value::as_str) {
                                    state.push_web_search_query(q);
                                }
                            }
                            // grok(xAI) 特有:action.sources[] 是检索命中的 URL,先收集,
                            // finish() 时无 url_citation 才转正为兜底来源。
                            if let Some(sources) = action.get("sources").and_then(Value::as_array) {
                                for source in sources {
                                    let Some(citation) = grok_source_citation(source) else {
                                        continue;
                                    };
                                    if !state
                                        .sources_fallback
                                        .iter()
                                        .any(|c| c.url == citation.url)
                                    {
                                        state.sources_fallback.push(citation);
                                    }
                                }
                            }
                        }
                        // push 完 emit 当前累加快照 → 实时更新卡（带查询词）。
                        state.emit_web_search_snapshot(sink)?;
                    }
                    // hosted 出图：完整 base64 在 item.result。message 的 output_text 常为
                    // 空串（图就是答案），不收这张图整轮会被判成空助手响应。
                    Some("image_generation_call") => {
                        if let Some(image) = responses_image_from_item(item) {
                            sink.emit(StreamPart::ImageData {
                                mime_type: image.mime_type.clone(),
                                data: image.data.clone(),
                            })?;
                            state.images.push(image);
                        }
                    }
                    _ => {}
                }
            }
        }
        // 内置搜索来源:url_citation 注解随正文流式到达。
        "response.output_text.annotation.added" => {
            if let Some(annotation) = value.get("annotation") {
                if annotation.get("type").and_then(Value::as_str) == Some("url_citation") {
                    if let Some(url) = annotation.get("url").and_then(Value::as_str) {
                        let title = annotation
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        state.push_web_search_citation(url, title);
                        // push 完 emit 当前累加快照 → 实时更新卡（带来源）。
                        state.emit_web_search_snapshot(sink)?;
                    }
                }
            }
        }
        "response.completed" | "response.incomplete" => {
            if let Some(response) = value.get("response") {
                if let Some(usage) = model_usage_from_openai_value(response) {
                    state.usage = Some(usage);
                }
                if let Some(status) = response.get("status").and_then(Value::as_str) {
                    state.finish_reason = Some(responses_finish_reason(status, response, state));
                }
                // 完整 output 里若还带 web_search_call / message 注解，合并进已流式累加的结果
                // （不覆盖：hosted web_search_call 通常只在 output_item.done 里出现，不重现在此）。
                if let Some(output) = response.get("output").and_then(Value::as_array) {
                    if let Some(extra) = web_search_from_responses_output(output) {
                        for q in extra.queries {
                            state.push_web_search_query(&q);
                        }
                        for c in extra.citations {
                            state.push_web_search_citation(&c.url, &c.title);
                        }
                    }
                }
            }
        }
        "response.failed" | "error" => {
            let error_obj = value
                .get("response")
                .and_then(|response| response.get("error"))
                .or_else(|| value.get("error"));
            let message = error_obj
                .map(responses_error_message)
                .unwrap_or_else(|| "Responses stream failed".to_string());
            return Ok(Some(message));
        }
        _ => {}
    }
    Ok(None)
}

/// Process one raw SSE line from a Responses stream into the accumulating state.
///
/// Shared by the live streaming loop (`stream_inner`, one drained `\n`-terminated line
/// at a time) and the non-stream SSE fallback (`output_from_sse_body`, lines split off a
/// fully-buffered body). Responses SSE carries `event:` and `data:` lines; the `data:`
/// JSON already includes a `type` field mirroring the event name, so only the `data:`
/// payload matters. Returns `Ok(Some(err))` for a terminal provider error, `Ok(None)`
/// otherwise; non-`data:` lines, blanks, `[DONE]`, and unparseable payloads are skipped.
fn process_sse_line(
    line: &str,
    state: &mut ResponsesStreamState,
    sink: &mut (dyn StreamSink + Send),
) -> Result<Option<String>, ModelError> {
    let line = line.trim();
    if !line.starts_with("data:") {
        return Ok(None);
    }
    let data = line.trim_start_matches("data:").trim();
    if data.is_empty() || data == "[DONE]" {
        return Ok(None);
    }
    let value: Value = match serde_json::from_str(data) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    handle_responses_stream_event(&value, state, sink)
}

/// True if `body` looks like a Responses SSE stream rather than a JSON object — i.e. it
/// contains a line starting with `data:` or `event:` (tolerant of CRLF and leading
/// whitespace). Used to decide whether a non-JSON non-stream response body can be salvaged.
fn looks_like_sse(body: &str) -> bool {
    body.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("data:") || line.starts_with("event:")
    })
}

/// Parse a fully-buffered Responses **SSE** body (a provider that streamed despite
/// `stream:false`) into a `GenerateOutput`, reusing the exact streaming accumulation. The
/// non-stream path has no live consumer, so events are fed through a discarding sink.
fn output_from_sse_body(body: &str) -> Result<GenerateOutput, ModelError> {
    let mut state = ResponsesStreamState::default();
    let mut sink = DiscardSink;
    for line in body.split('\n') {
        if let Some(err) = process_sse_line(line, &mut state, &mut sink)? {
            return Err(ModelError::new(err));
        }
    }
    state.finish(&mut sink)
}

/// A `StreamSink` that drops every part. Used by the non-stream SSE fallback where the
/// accumulated `GenerateOutput` is the only consumer and no live deltas are needed.
struct DiscardSink;

impl StreamSink for DiscardSink {
    fn emit(&mut self, _part: StreamPart) -> Result<(), ModelError> {
        Ok(())
    }
}

fn pending_tool_call_from_partial(partial: &ResponsesToolPartial) -> PendingToolCall {
    let raw = if partial.arguments.trim().is_empty() {
        "{}".to_string()
    } else {
        partial.arguments.clone()
    };
    let (arguments, arguments_parse_error) = parse_tool_arguments(&raw);
    PendingToolCall {
        id: partial.call_id.clone(),
        function_name: partial.name.clone(),
        arguments,
        arguments_raw: raw,
        arguments_parse_error,
        signature: None,
    }
}

/// Parse a non-streaming `/v1/responses` body into a `GenerateOutput`.
pub fn output_from_responses(
    value: &Value,
    raw: &str,
    label: &str,
) -> Result<GenerateOutput, ModelError> {
    if let Some(error) = value.get("error") {
        return Err(ModelError::new(format!(
            "{label}: {}",
            responses_error_message(error)
        )));
    }
    let output = value
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_response(label, raw))?;

    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut images = Vec::new();
    for item in output {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        if part.get("type").and_then(Value::as_str) == Some("output_text") {
                            if let Some(part_text) = part.get("text").and_then(Value::as_str) {
                                text.push_str(part_text);
                            }
                        }
                    }
                }
            }
            Some("function_call") => {
                let call_id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let raw_args = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("{}")
                    .to_string();
                let (arguments, arguments_parse_error) = parse_tool_arguments(&raw_args);
                tool_calls.push(PendingToolCall {
                    id: call_id,
                    function_name: name,
                    arguments,
                    arguments_raw: raw_args,
                    arguments_parse_error,
                    signature: None,
                });
            }
            // hosted 出图：与流式 `output_item.done` 同一形态，走同一提取器。
            Some("image_generation_call") => {
                if let Some(image) = responses_image_from_item(item) {
                    images.push(image);
                }
            }
            _ => {}
        }
    }

    let usage = model_usage_from_openai_value(value);
    let finish_reason = value.get("status").and_then(Value::as_str).map(|status| {
        if status == "completed" && !tool_calls.is_empty() {
            "tool_calls".to_string()
        } else if status == "completed" {
            "stop".to_string()
        } else {
            status.to_string()
        }
    });

    Ok(GenerateOutput {
        text,
        reasoning: None,
        tool_calls,
        usage,
        finish_reason,
        provider_messages: Vec::new(),
        cancelled: false,
        web_search: web_search_from_responses_output(output),
        images,
    })
}

/// 把 grok(xAI) `web_search_call.action.sources[]` 的单项解析成 `WebCitation`。
/// 尽力而为：url 必须有；title 取 `title`/`name`（都没有则留空，前端按域名兜底），
/// snippet 取 `description`/`snippet`（截断到 400 字符，防超大载荷拖慢流式卡）。
fn grok_source_citation(source: &Value) -> Option<WebCitation> {
    let url = source
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let title = source
        .get("title")
        .or_else(|| source.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string();
    let snippet = source
        .get("description")
        .or_else(|| source.get("snippet"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(400).collect());
    Some(WebCitation {
        title,
        url: url.to_string(),
        snippet,
        ..Default::default()
    })
}

/// 从 Responses `output[]` 数组解析内置搜索痕迹：`web_search_call` 项取查询词、
/// `message` 项内容的 `annotations[type=url_citation]` 取来源。任一为空则整体可能为空，
/// 全空时返回 None（视作没发生可见的内置搜索）。解析尽力而为，绝不 panic。
fn web_search_from_responses_output(output: &[Value]) -> Option<BuiltinWebSearch> {
    let mut result = BuiltinWebSearch::default();
    let mut seen_urls: std::collections::HashSet<String> = std::collections::HashSet::new();
    // grok(xAI) 特有:web_search_call.action.sources[](检索命中 URL)。仅当整个输出
    // 没有任何正文 url_citation 时才转正为兜底来源(注解质量更高,优先)。
    let mut sources_fallback: Vec<WebCitation> = Vec::new();
    for item in output {
        match item.get("type").and_then(Value::as_str) {
            Some("web_search_call") => {
                // 查询词可能在 action.query 或顶层 query（wire 随版本略有差异，两处都试）。
                let query = item
                    .get("action")
                    .and_then(|action| action.get("query"))
                    .or_else(|| item.get("query"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                if let Some(query) = query {
                    if !result.queries.iter().any(|existing| existing == query) {
                        result.queries.push(query.to_string());
                    }
                }
                if let Some(sources) = item
                    .get("action")
                    .and_then(|action| action.get("sources"))
                    .and_then(Value::as_array)
                {
                    for source in sources {
                        let Some(citation) = grok_source_citation(source) else {
                            continue;
                        };
                        if !sources_fallback.iter().any(|c| c.url == citation.url) {
                            sources_fallback.push(citation);
                        }
                    }
                }
            }
            Some("message") => {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        let Some(annotations) = part.get("annotations").and_then(Value::as_array)
                        else {
                            continue;
                        };
                        for annotation in annotations {
                            if annotation.get("type").and_then(Value::as_str)
                                != Some("url_citation")
                            {
                                continue;
                            }
                            let Some(url) = annotation
                                .get("url")
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .filter(|value| !value.is_empty())
                            else {
                                continue;
                            };
                            if !seen_urls.insert(url.to_string()) {
                                continue;
                            }
                            let title = annotation
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
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // 无正文注解时才用 sources 兜底(有注解则丢弃 sources,注解更准)。
    if result.citations.is_empty() && !sources_fallback.is_empty() {
        result.citations = sources_fallback;
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn responses_finish_reason(status: &str, response: &Value, state: &ResponsesStreamState) -> String {
    match status {
        "completed" if !state.tool_calls.is_empty() => "tool_calls".to_string(),
        "completed" => "stop".to_string(),
        // Responses 的输出截断不叫 length：status=="incomplete" + incomplete_details.reason
        // =="max_output_tokens"。归一化为 Chat Completions 的 "length"，让上层
        // 「finish_reason==length ⇒ 整批工具调用作废」的防护对 openai_responses /
        // xai_responses 同样生效（anthropic/gemini 适配器已各自归一化，这里此前漏了）。
        "incomplete"
            if response
                .get("incomplete_details")
                .and_then(|details| details.get("reason"))
                .and_then(Value::as_str)
                == Some("max_output_tokens") =>
        {
            "length".to_string()
        }
        other => other.to_string(),
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

fn invalid_response(label: &str, raw: &str) -> ModelError {
    ModelError::new(format!(
        "Invalid {label} response: {}",
        raw.chars().take(500).collect::<String>()
    ))
}

/// Extract the most informative human-readable message from a Responses API `error`
/// object. Providers vary: some put the real reason in `message`, some only in `code`
/// or `type`, and some proxies return a bare object. Try `message` → `code` → `type` →
/// the compact JSON of the whole error value; only fall back to a generic string when the
/// error object carries nothing at all. This surfaces the provider's real reason (rate
/// limit, 502, context length, …) instead of a useless "Unknown Responses API error".
fn responses_error_message(error: &Value) -> String {
    for key in ["message", "code", "type"] {
        if let Some(text) = error.get(key).and_then(Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    // No standard field carried a string. If the error value is a non-empty object/array
    // or a non-empty scalar, serialize it compactly so the real payload is visible.
    let serialized = error.to_string();
    if !error.is_null() && serialized != "{}" && serialized != "\"\"" && !serialized.is_empty() {
        return serialized;
    }
    "Unknown Responses API error".to_string()
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::model::{GenerateOptions, MessagePart, ModelMessage, ModelRole};
    use crate::settings::ModelInfo;

    fn responses_temperature_body(
        provider_temperature: Option<f64>,
        request_temperature: Option<f64>,
    ) -> Value {
        let state = crate::state::AppState::new_headless(
            crate::settings::Settings::default(),
            std::env::temp_dir(),
        );
        let model = "custom-temperature-model";
        let mut model_overrides = std::collections::HashMap::new();
        if let Some(temperature) = provider_temperature {
            model_overrides.insert(
                model.into(),
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
            base_url: "https://api.openai.com/v1".into(),
            available_models: vec![model.into()],
            enabled_models: vec![model.into()],
            enabled: true,
            api_format: "openai_responses".into(),
            model_overrides,
            compress_request_body: false,
            request: Default::default(),
        };
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
        OpenAiResponsesProvider::new(&state, &provider, 1).request_body(&request, false)
    }

    fn xai_body(model: &str, effort: Option<&str>, builtin_search: bool) -> Value {
        xai_body_with(model, effort, true, builtin_search)
    }

    fn xai_body_with(
        model: &str,
        effort: Option<&str>,
        thinking_enabled: bool,
        builtin_search: bool,
    ) -> Value {
        let state = crate::state::AppState::new_headless(
            crate::settings::Settings::default(),
            std::env::temp_dir(),
        );
        let provider = ModelProvider {
            id: "xai".into(),
            name: "xAI".into(),
            api_keys: vec!["sk-test".into()],
            api_key_legacy: None,
            // 故意用一个非 api.x.ai 的域名：协议来自用户的选择，不是猜域名。
            base_url: "https://relay.example.com/v1".into(),
            available_models: vec![model.into()],
            enabled_models: vec![model.into()],
            enabled: true,
            api_format: "xai_responses".into(),
            model_overrides: Default::default(),
            compress_request_body: false,
            request: Default::default(),
        };
        let request = GenerateRequest {
            model: model.into(),
            system: "你是 Kivio".into(),
            messages: vec![ModelMessage {
                role: ModelRole::User,
                content: vec![MessagePart::Text { text: "hi".into() }],
            }],
            tools: Vec::new(),
            options: GenerateOptions {
                thinking_enabled,
                thinking_level: effort.map(str::to_string),
                builtin_web_search: builtin_search,
                ..Default::default()
            },
            metadata: crate::chat::model::RequestMetadata {
                conversation_id: Some("conv_abc".into()),
                ..Default::default()
            },
        };
        OpenAiResponsesProvider::new(&state, &provider, 1).request_body(&request, false)
    }

    #[test]
    fn xai_body_omits_what_is_useless_and_opts_out_of_storage() {
        let body = xai_body("grok-4.3", Some("high"), false);
        // `instructions` 搬进了 input（见下一条测试）；`prompt_cache_key` 对 xAI 没用
        // ——它的缓存靠 previous_response_id 自动做，不认这个键。
        for field in ["instructions", "prompt_cache_key"] {
            assert!(
                body.get(field).is_none(),
                "{field} must not be sent: {body}"
            );
        }
        // **必须显式关存储**：xAI 的 Responses `store` 默认 true、服务端留 30 天，
        // 而我们从不用 previous_response_id，那份副本纯属白存用户的对话。
        assert_eq!(
            body["store"], false,
            "must opt out of xAI server-side storage: {body}"
        );
    }

    #[test]
    fn xai_moves_the_system_prompt_into_input_instead_of_dropping_it() {
        let body = xai_body("grok-4.3", None, false);
        // 直接删 instructions 会让系统提示词凭空消失——必须搬进 input 首位。
        let input = body["input"].as_array().expect("input array");
        assert_eq!(input[0]["role"], "system");
        assert_eq!(input[0]["content"][0]["text"], "你是 Kivio");
        assert_eq!(input[1]["role"], "user");
    }

    #[test]
    fn xai_maps_effort_onto_its_own_ladder() {
        assert_eq!(
            xai_body("grok-4.3", Some("minimal"), false)["reasoning"]["effort"],
            "low"
        );
        assert_eq!(
            xai_body("grok-4.3", Some("max"), false)["reasoning"]["effort"],
            "high"
        );
        assert_eq!(
            xai_body("grok-4.3", Some("xhigh"), false)["reasoning"]["effort"],
            "xhigh"
        );
        // 开思考但未设档 → 不发 reasoning（不擅自兜底）。
        assert!(xai_body("grok-4.3", None, false).get("reasoning").is_none());
        // UI Off → 显式 effort:"none"（thinking_enabled=false，level=None）。
        let off = xai_body_with("grok-4.3", None, false, false);
        assert_eq!(off["reasoning"]["effort"], "none", "body: {off}");
    }

    #[test]
    fn xai_includes_server_tool_outputs() {
        // 没有服务端工具时不发这个字段（尤其别带 reasoning.encrypted_content：
        // types.rs 的回放会丢弃推理项，要来只是让每次响应多背一份没人消费的密文）。
        assert!(xai_body("grok-4.3", None, false).get("include").is_none());
        // 开了内置搜索才发，且必须含来源字段——不 include 就拿不到 grok 的搜索来源。
        let searching = xai_body("grok-4.3", None, true);
        let include = searching["include"].as_array().expect("include");
        assert!(
            include.contains(&Value::String("web_search_call.action.sources".to_string())),
            "{searching}"
        );
    }

    #[test]
    fn openai_responses_is_untouched_by_the_xai_branch() {
        // 同一个适配器伺候两种协议，OpenAI 那条路必须逐字节照旧。
        let state = crate::state::AppState::new_headless(
            crate::settings::Settings::default(),
            std::env::temp_dir(),
        );
        let provider = ModelProvider {
            id: "test".into(),
            name: "OpenAI".into(),
            api_keys: vec!["sk-test".into()],
            api_key_legacy: None,
            base_url: "https://api.openai.com/v1".into(),
            available_models: vec!["gpt-5.5".into()],
            enabled_models: vec!["gpt-5.5".into()],
            enabled: true,
            api_format: "openai_responses".into(),
            model_overrides: Default::default(),
            compress_request_body: false,
            request: Default::default(),
        };
        let request = GenerateRequest {
            model: "gpt-5.5".into(),
            system: "你是 Kivio".into(),
            messages: vec![ModelMessage {
                role: ModelRole::User,
                content: vec![MessagePart::Text { text: "hi".into() }],
            }],
            tools: Vec::new(),
            options: GenerateOptions {
                thinking_level: Some("high".into()),
                ..Default::default()
            },
            metadata: crate::chat::model::RequestMetadata {
                conversation_id: Some("conv_abc".into()),
                ..Default::default()
            },
        };
        let body = OpenAiResponsesProvider::new(&state, &provider, 1).request_body(&request, false);
        assert_eq!(body["instructions"], "你是 Kivio");
        assert_eq!(body["store"], false);
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["prompt_cache_key"], "conv_abc");
        assert!(body.get("prompt_cache_retention").is_none());
        assert_eq!(body["input"][0]["role"], "user");
    }

    #[test]
    fn responses_prompt_cache_retention_none_long_and_learn() {
        let state = crate::state::AppState::new_headless(
            crate::settings::Settings::default(),
            std::env::temp_dir(),
        );
        let body_with = |retention: &str| {
            let provider = ModelProvider {
                id: "test".into(),
                name: "OpenAI".into(),
                api_keys: vec!["sk-test".into()],
                api_key_legacy: None,
                base_url: "https://api.openai.com/v1".into(),
                available_models: vec!["gpt-5.5".into()],
                enabled_models: vec!["gpt-5.5".into()],
                enabled: true,
                api_format: "openai_responses".into(),
                model_overrides: Default::default(),
                compress_request_body: false,
                request: crate::settings::ProviderRequestConfig {
                    prompt_cache_retention: retention.into(),
                    ..Default::default()
                },
            };
            let request = GenerateRequest {
                model: "gpt-5.5".into(),
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
            OpenAiResponsesProvider::new(&state, &provider, 1).request_body(&request, false)
        };
        assert!(body_with("none").get("prompt_cache_key").is_none());
        let short = body_with("short");
        assert_eq!(short["prompt_cache_key"], "conv_abc");
        assert!(short.get("prompt_cache_retention").is_none());
        let long = body_with("long");
        assert_eq!(long["prompt_cache_key"], "conv_abc");
        assert_eq!(long["prompt_cache_retention"], "24h");
        state.mark_prompt_cache_retention_unsupported("https://api.openai.com/v1");
        let after = body_with("long");
        assert_eq!(after["prompt_cache_key"], "conv_abc");
        assert!(after.get("prompt_cache_retention").is_none());
    }

    #[test]
    fn builtin_web_search_appends_responses_tool() {
        let state = crate::state::AppState::new_headless(
            crate::settings::Settings::default(),
            std::env::temp_dir(),
        );
        let provider = ModelProvider {
            id: "test".into(),
            name: "Test".into(),
            api_keys: vec!["sk-test".into()],
            api_key_legacy: None,
            base_url: "https://api.openai.com/v1".into(),
            available_models: vec!["gpt-5.5".into()],
            enabled_models: vec!["gpt-5.5".into()],
            enabled: true,
            api_format: "openai_responses".into(),
            model_overrides: Default::default(),
            compress_request_body: false,
            request: Default::default(),
        };
        let base = GenerateRequest {
            model: "gpt-5.5".into(),
            system: String::new(),
            messages: vec![ModelMessage {
                role: ModelRole::User,
                content: vec![MessagePart::Text { text: "hi".into() }],
            }],
            tools: Vec::new(),
            options: GenerateOptions::default(),
            metadata: Default::default(),
        };
        let adapter = OpenAiResponsesProvider::new(&state, &provider, 1);
        // off ⇒ 无 tools。
        let off = adapter.request_body(&base, false);
        assert!(off.get("tools").is_none(), "body: {off}");
        // on ⇒ tools 含 {"type":"web_search"} + tool_choice=auto。
        let mut req = base.clone();
        req.options.builtin_web_search = true;
        let on = adapter.request_body(&req, false);
        let tools = on["tools"].as_array().expect("tools array present");
        assert!(
            tools.iter().any(|t| t["type"] == "web_search"),
            "body: {on}"
        );
        assert_eq!(on["tool_choice"], "auto");
        // 开思考但无档位 ⇒ 不发 reasoning（effort 由 resolve_thinking 在上游决定，适配器不兜底）。
        assert!(on.get("reasoning").is_none(), "body: {on}");
        // 显式设 high ⇒ reasoning.effort=high。
        let mut req_high = base.clone();
        req_high.options.builtin_web_search = true;
        req_high.options.thinking_level = Some("high".into());
        let high = adapter.request_body(&req_high, false);
        assert_eq!(high["reasoning"]["effort"], "high", "body: {high}");
        // xhigh 原样下发（gpt-5.1-codex-max 起支持；哪些模型认由模型库 reasoningEfforts 门控，
        // 适配器不再按协议收敛成 high）。
        let mut req_x = base.clone();
        req_x.options.thinking_level = Some("xhigh".into());
        let xh = adapter.request_body(&req_x, false);
        assert_eq!(xh["reasoning"]["effort"], "xhigh", "body: {xh}");
        // 纯对话（开思考、无内置、无档）⇒ 不发 reasoning。
        assert!(off.get("reasoning").is_none(), "body: {off}");
        // UI Off → 显式 none（OpenAI / DeepSeek Responses 文档；省略会默认 high）。
        let mut req_off = base.clone();
        req_off.options.thinking_enabled = false;
        let thinking_off = adapter.request_body(&req_off, false);
        assert_eq!(
            thinking_off["reasoning"]["effort"], "none",
            "body: {thinking_off}"
        );
        assert_eq!(thinking_off["store"], false, "body: {thinking_off}");
        // 关思考不需要 encrypted reasoning content。
        assert!(
            thinking_off.get("include").is_none(),
            "body: {thinking_off}"
        );

        // 发 reasoning 档位时同时发 store:false + include（与 Codex 官方客户端一致）：
        // 我们每轮自带完整 input，不依赖服务端会话状态，思考链走 encrypted_content。
        assert_eq!(high["store"], false, "body: {high}");
        assert_eq!(
            high["include"],
            serde_json::json!(["reasoning.encrypted_content"]),
            "body: {high}"
        );
        // 开思考但不发 reasoning 档时不该无故附带这两项（保持与既有纯对话请求字节兼容）。
        assert!(off.get("store").is_none(), "body: {off}");
        assert!(off.get("include").is_none(), "body: {off}");
    }

    #[test]
    fn web_search_parsed_from_responses_output() {
        // web_search_call → query；message.content.annotations[url_citation] → 来源（去重）。
        let output = serde_json::json!([
            { "type": "web_search_call", "action": { "type": "search", "query": "kivio release" } },
            {
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": "见来源。",
                    "annotations": [
                        { "type": "url_citation", "url": "https://a.com", "title": "A 站" },
                        { "type": "url_citation", "url": "https://a.com", "title": "重复应去掉" },
                        { "type": "url_citation", "url": "https://b.com" }
                    ]
                }]
            }
        ]);
        let parsed = web_search_from_responses_output(output.as_array().unwrap())
            .expect("web_search present");
        assert_eq!(parsed.queries, vec!["kivio release".to_string()]);
        assert_eq!(parsed.citations.len(), 2);
        assert_eq!(parsed.citations[0].title, "A 站");
        assert_eq!(parsed.citations[0].url, "https://a.com");
        // 缺 title 时回退用 url。
        assert_eq!(parsed.citations[1].title, "https://b.com");
    }

    #[test]
    fn web_search_none_without_search_traces() {
        let output = serde_json::json!([
            { "type": "message", "content": [{ "type": "output_text", "text": "无搜索" }] }
        ]);
        assert!(web_search_from_responses_output(output.as_array().unwrap()).is_none());
    }

    #[test]
    fn web_search_sources_fallback_only_without_citations() {
        // grok(xAI):action.sources[] 是检索命中 URL。无正文 url_citation 时兜底转正;
        // 有注解时以注解为准、sources 丢弃。
        let sources_only = serde_json::json!([
            { "type": "web_search_call", "action": { "type": "search", "query": "nvda close",
                "sources": [
                    { "type": "url", "url": "https://finance.yahoo.com/quote/NVDA/" },
                    { "type": "url", "url": "https://finance.yahoo.com/quote/NVDA/" },
                    { "type": "url", "url": "https://www.wsj.com/market-data/quotes/NVDA" }
                ] } },
            { "type": "message", "content": [{ "type": "output_text", "text": "无注解正文" }] }
        ]);
        let parsed = web_search_from_responses_output(sources_only.as_array().unwrap())
            .expect("web_search present");
        assert_eq!(parsed.citations.len(), 2, "sources 去重后兜底转正");
        assert_eq!(
            parsed.citations[0].url,
            "https://finance.yahoo.com/quote/NVDA/"
        );

        // 有 url_citation ⇒ 注解优先,sources 不混入。
        let with_annotation = serde_json::json!([
            { "type": "web_search_call", "action": { "type": "search", "query": "q",
                "sources": [ { "type": "url", "url": "https://ignored.example.com" } ] } },
            { "type": "message", "content": [{ "type": "output_text", "text": "见[1]",
                "annotations": [ { "type": "url_citation", "url": "https://cited.example.com", "title": "Cited" } ] }] }
        ]);
        let parsed = web_search_from_responses_output(with_annotation.as_array().unwrap())
            .expect("web_search present");
        assert_eq!(parsed.citations.len(), 1);
        assert_eq!(parsed.citations[0].url, "https://cited.example.com");
    }

    #[test]
    fn sse_stream_hosted_image_generation_is_captured_with_empty_text() {
        // 真机形态（xb1520 网关 + gpt-5.6）：模型自行走 hosted image_generation_call 回答，
        // message 的 output_text 是空串（图即答案），且没有任何 output_text.delta。
        // 不收 image_generation_call.result 会让整轮变成「空助手响应」而报错。
        let body = concat!(
            "data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"id\":\"ig_1\",\"type\":\"image_generation_call\",\"status\":\"completed\",\"action\":\"generate\",\"output_format\":\"png\",\"result\":\"aGVsbG8=\"}}\n",
            "data: {\"type\":\"response.output_text.done\",\"output_index\":2,\"text\":\"\"}\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"\"}]}]}}\n",
            "data: [DONE]\n",
        );
        let output = output_from_sse_body(body).expect("sse output");
        assert!(output.text.trim().is_empty(), "正文确实是空的");
        assert_eq!(output.images.len(), 1, "hosted 出图必须被收下");
        assert_eq!(output.images[0].mime_type, "image/png");
        assert_eq!(output.images[0].data, "aGVsbG8=");
    }

    #[test]
    fn nonstream_hosted_image_generation_is_captured() {
        // 非流式同形态：output[] 里的 image_generation_call 也要出图，mime 按 output_format。
        let value = serde_json::json!({
            "status": "completed",
            "output": [
                { "type": "image_generation_call", "output_format": "jpeg", "result": "aGVsbG8=" },
                { "type": "message", "content": [{ "type": "output_text", "text": "" }] }
            ]
        });
        let output = output_from_responses(&value, "{}", "test").expect("output");
        assert_eq!(output.images.len(), 1);
        assert_eq!(output.images[0].mime_type, "image/jpeg");
    }

    #[test]
    fn sse_stream_sources_fallback_when_no_citation_arrives() {
        // 流式:web_search_call(带 sources)到达但整轮无 url_citation(grok 岔去客户端
        // fetch 的典型形态)⇒ finish 时 sources 兜底进 citations。
        let body = concat!(
            "data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"id\":\"ws_1\",\"type\":\"web_search_call\",\"status\":\"completed\",\"action\":{\"type\":\"search\",\"query\":\"nvda close\",\"sources\":[{\"type\":\"url\",\"url\":\"https://finance.yahoo.com/quote/NVDA/\"},{\"type\":\"url\",\"url\":\"https://www.cnbc.com/quotes/NVDA\"}]}}}\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"answer\"}]}]}}\n",
            "data: [DONE]\n",
        );
        let output = output_from_sse_body(body).expect("sse output");
        let ws = output.web_search.expect("web_search present");
        assert_eq!(ws.queries, vec!["nvda close".to_string()]);
        assert_eq!(ws.citations.len(), 2, "无注解时 sources 兜底");
        assert_eq!(ws.citations[0].url, "https://finance.yahoo.com/quote/NVDA/");
    }

    #[test]
    fn request_body_temperature_is_optional_and_model_scoped() {
        let default_body = responses_temperature_body(None, None);
        assert!(default_body.get("temperature").is_none());

        let configured_body = responses_temperature_body(Some(0.4), None);
        assert_eq!(configured_body["temperature"], serde_json::json!(0.4));

        let explicit_body = responses_temperature_body(Some(0.4), Some(1.2));
        assert_eq!(explicit_body["temperature"], serde_json::json!(1.2));
    }

    /// Drive the streaming event handler with the exact `data:` JSON I captured from the
    /// live `gpt-5.3-codex-spark` Responses stream, asserting the tool call's arguments
    /// (which Chat Completions dropped) come through.
    fn run_events(events: &[Value]) -> (Vec<StreamPart>, GenerateOutput) {
        let mut parts = Vec::new();
        let mut state = ResponsesStreamState::default();
        let mut sink = |part: StreamPart| {
            parts.push(part);
            Ok(())
        };
        for event in events {
            handle_responses_stream_event(event, &mut state, &mut sink).expect("event");
        }
        let output = state.finish(&mut sink).expect("finish");
        (parts, output)
    }

    #[test]
    fn stream_function_call_arguments_are_captured() {
        let events = vec![
            serde_json::json!({
                "type": "response.output_item.added",
                "output_index": 1,
                "item": { "id": "fc_1", "type": "function_call", "status": "in_progress", "arguments": "", "call_id": "call_abc", "name": "web_search" }
            }),
            serde_json::json!({
                "type": "response.function_call_arguments.done",
                "arguments": "{\"query\":\"吉林市 明天 天气\"}",
                "item_id": "fc_1",
                "output_index": 1
            }),
            serde_json::json!({
                "type": "response.output_item.done",
                "output_index": 1,
                "item": { "id": "fc_1", "type": "function_call", "status": "completed", "arguments": "{\"query\":\"吉林市 明天 天气\"}", "call_id": "call_abc", "name": "web_search" }
            }),
            serde_json::json!({
                "type": "response.completed",
                "response": { "status": "completed", "usage": { "input_tokens": 10, "output_tokens": 5, "total_tokens": 15 } }
            }),
        ];
        let (parts, output) = run_events(&events);

        assert_eq!(output.tool_calls.len(), 1);
        let call = &output.tool_calls[0];
        assert_eq!(call.id, "call_abc");
        assert_eq!(call.function_name, "web_search");
        assert_eq!(call.arguments["query"], "吉林市 明天 天气");
        assert!(call.arguments_parse_error.is_none());
        assert_eq!(output.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(output.usage.and_then(|u| u.total_tokens), Some(15));
        assert!(parts
            .iter()
            .any(|p| matches!(p, StreamPart::ToolCallStart { .. })));
        assert!(parts
            .iter()
            .any(|p| matches!(p, StreamPart::ToolCallDone { .. })));
    }

    #[test]
    fn stream_text_deltas_accumulate() {
        let events = vec![
            serde_json::json!({ "type": "response.output_text.delta", "delta": "你好" }),
            serde_json::json!({ "type": "response.output_text.delta", "delta": "，世界" }),
            serde_json::json!({ "type": "response.completed", "response": { "status": "completed" } }),
        ];
        let (parts, output) = run_events(&events);
        assert_eq!(output.text, "你好，世界");
        assert_eq!(output.finish_reason.as_deref(), Some("stop"));
        assert_eq!(
            parts
                .iter()
                .filter(|p| matches!(p, StreamPart::TextDelta { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn non_stream_output_parses_text_and_tool_call() {
        let value = serde_json::json!({
            "status": "completed",
            "output": [
                { "type": "function_call", "call_id": "call_x", "name": "web_search", "arguments": "{\"query\":\"a\"}" }
            ],
            "usage": { "input_tokens": 3, "output_tokens": 2, "total_tokens": 5 }
        });
        let output = output_from_responses(&value, "{}", "test").expect("output");
        assert_eq!(output.tool_calls.len(), 1);
        assert_eq!(output.tool_calls[0].arguments["query"], "a");
        assert_eq!(output.finish_reason.as_deref(), Some("tool_calls"));

        let text_value = serde_json::json!({
            "status": "completed",
            "output": [ { "type": "message", "content": [ { "type": "output_text", "text": "hi" } ] } ]
        });
        let out = output_from_responses(&text_value, "{}", "test").expect("output");
        assert_eq!(out.text, "hi");
        assert_eq!(out.finish_reason.as_deref(), Some("stop"));
    }

    /// A provider that streams an SSE body despite `stream:false`. `output_from_sse_body`
    /// must reuse the streaming accumulation and yield the same `GenerateOutput` the live
    /// stream path produces — text/tool-call args/usage — instead of a JSON parse error.
    #[test]
    fn sse_body_fallback_parses_tool_call_and_usage() {
        let body = concat!(
            "event: response.created\r\n",
            "data: {\"type\":\"response.created\",\"response\":{\"status\":\"in_progress\"}}\r\n",
            "\r\n",
            "event: response.output_item.added\r\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"status\":\"in_progress\",\"arguments\":\"\",\"call_id\":\"call_abc\",\"name\":\"web_search\"}}\r\n",
            "\r\n",
            "event: response.function_call_arguments.done\r\n",
            "data: {\"type\":\"response.function_call_arguments.done\",\"arguments\":\"{\\\"query\\\":\\\"吉林市 明天 天气\\\"}\",\"item_id\":\"fc_1\",\"output_index\":1}\r\n",
            "\r\n",
            "event: response.output_item.done\r\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"status\":\"completed\",\"arguments\":\"{\\\"query\\\":\\\"吉林市 明天 天气\\\"}\",\"call_id\":\"call_abc\",\"name\":\"web_search\"}}\r\n",
            "\r\n",
            "event: response.completed\r\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15}}}\r\n",
            "\r\n",
            "data: [DONE]\r\n",
        );
        assert!(looks_like_sse(body));
        let output = output_from_sse_body(body).expect("sse output");
        assert_eq!(output.tool_calls.len(), 1);
        let call = &output.tool_calls[0];
        assert_eq!(call.id, "call_abc");
        assert_eq!(call.function_name, "web_search");
        assert_eq!(call.arguments["query"], "吉林市 明天 天气");
        assert!(call.arguments_parse_error.is_none());
        assert_eq!(output.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(output.usage.and_then(|u| u.total_tokens), Some(15));
    }

    /// 内置搜索(hosted web_search)流式捕获:web_search_call 走 output_item.done、
    /// 来源走 output_text.annotation.added——都不重现在 response.completed 的 output 里
    /// (completed 只剩 message),故必须边流边收。此测试钉住这条真实 wire 行为。
    #[test]
    fn sse_stream_captures_builtin_web_search_from_events() {
        let body = concat!(
            "data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"id\":\"ws_1\",\"type\":\"web_search_call\",\"status\":\"completed\",\"action\":{\"type\":\"search\",\"queries\":[\"TSLA latest close\"],\"query\":\"TSLA latest close\"}}}\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"answer\"}\n",
            "data: {\"type\":\"response.output_text.annotation.added\",\"annotation\":{\"type\":\"url_citation\",\"title\":\"ChartExchange\",\"url\":\"https://chartexchange.com/x\"}}\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"answer\"}]}]}}\n",
            "data: [DONE]\n",
        );
        let output = output_from_sse_body(body).expect("sse output");
        let ws = output
            .web_search
            .expect("web_search captured from stream events");
        assert_eq!(ws.queries, vec!["TSLA latest close".to_string()]);
        assert_eq!(ws.citations.len(), 1);
        assert_eq!(ws.citations[0].url, "https://chartexchange.com/x");
        assert_eq!(ws.citations[0].title, "ChartExchange");
    }

    /// 内置搜索实时卡（任务 07-23）：流式过程中逐帧发 `StreamPart::WebSearch`——
    /// `output_item.added(web_search_call)` 开牌（空快照）、`output_item.done` 带查询、
    /// `annotation.added` 带来源；`response.completed` 合并块**不**再发（避免重复）。
    #[test]
    fn sse_stream_emits_web_search_parts_for_realtime_card() {
        #[derive(Default)]
        struct CapturingSink {
            parts: Vec<StreamPart>,
        }
        impl StreamSink for CapturingSink {
            fn emit(&mut self, part: StreamPart) -> Result<(), ModelError> {
                self.parts.push(part);
                Ok(())
            }
        }

        let body = concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"id\":\"ws_1\",\"type\":\"web_search_call\",\"status\":\"in_progress\"}}\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"id\":\"ws_1\",\"type\":\"web_search_call\",\"status\":\"completed\",\"action\":{\"type\":\"search\",\"queries\":[\"TSLA latest close\"],\"query\":\"TSLA latest close\"}}}\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"answer\"}\n",
            "data: {\"type\":\"response.output_text.annotation.added\",\"annotation\":{\"type\":\"url_citation\",\"title\":\"ChartExchange\",\"url\":\"https://chartexchange.com/x\"}}\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"answer\"}]}]}}\n",
            "data: [DONE]\n",
        );
        let mut state = ResponsesStreamState::default();
        let mut sink = CapturingSink::default();
        for line in body.split('\n') {
            process_sse_line(line, &mut state, &mut sink).expect("process line");
        }
        state.finish(&mut sink).expect("finish");

        let web_search_parts: Vec<(Vec<String>, Vec<WebCitation>)> = sink
            .parts
            .iter()
            .filter_map(|part| match part {
                StreamPart::WebSearch { queries, citations } => {
                    Some((queries.clone(), citations.clone()))
                }
                _ => None,
            })
            .collect();
        // 开牌 + 查询 + 来源 + finish 终帧同步 = 4 帧(completed 合并不发,finish 无条件发终帧)。
        assert_eq!(web_search_parts.len(), 4);
        // 首帧空快照 = 开牌。
        assert!(web_search_parts[0].0.is_empty() && web_search_parts[0].1.is_empty());
        // 末帧累加了查询与来源。
        let last = web_search_parts.last().expect("last frame");
        assert_eq!(last.0, vec!["TSLA latest close".to_string()]);
        assert_eq!(last.1.len(), 1);
        assert_eq!(last.1[0].url, "https://chartexchange.com/x");
    }

    /// SSE body carrying only text deltas accumulates the same as the live stream path.
    #[test]
    fn sse_body_fallback_accumulates_text() {
        let body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"你好\"}\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"，世界\"}\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n",
        );
        let output = output_from_sse_body(body).expect("sse output");
        assert_eq!(output.text, "你好，世界");
        assert_eq!(output.finish_reason.as_deref(), Some("stop"));
    }

    /// A terminal `response.failed` event in the SSE body surfaces as a `ModelError`.
    #[test]
    fn sse_body_fallback_surfaces_provider_error() {
        let body = concat!(
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"boom\"}}}\n",
        );
        let err = output_from_sse_body(body).expect_err("should error");
        assert!(err.to_string().contains("boom"));
    }

    /// A plain JSON body is not mistaken for SSE; the happy path stays unchanged.
    #[test]
    fn plain_json_body_is_not_sse() {
        let body = r#"{"status":"completed","output":[]}"#;
        assert!(!looks_like_sse(body));
    }

    /// Gap 4: a non-stream Responses error object surfaces the provider's real reason.
    #[test]
    fn output_from_responses_surfaces_real_error_message() {
        let value: Value = serde_json::from_str(r#"{"error":{"message":"boom"}}"#).unwrap();
        let err = output_from_responses(&value, "{}", "Chat planning").expect_err("error");
        assert!(err.to_string().contains("boom"), "got {err}");
        assert!(!err.to_string().contains("Unknown"));
    }

    /// Gap 4: error message extraction falls through message → code → type → JSON, and
    /// only uses the generic fallback when the error object is truly empty.
    #[test]
    fn responses_error_message_falls_through_fields() {
        assert_eq!(
            responses_error_message(&serde_json::json!({"message": "rate limited"})),
            "rate limited"
        );
        assert_eq!(
            responses_error_message(&serde_json::json!({"code": "context_length_exceeded"})),
            "context_length_exceeded"
        );
        assert_eq!(
            responses_error_message(&serde_json::json!({"type": "server_error"})),
            "server_error"
        );
        // No standard field but a non-empty object → compact JSON is surfaced.
        let json_only = responses_error_message(&serde_json::json!({"detail": "502 bad gateway"}));
        assert!(json_only.contains("502 bad gateway"), "got {json_only}");
        // Truly empty error object → generic fallback (last resort only).
        assert_eq!(
            responses_error_message(&serde_json::json!({})),
            "Unknown Responses API error"
        );
    }

    /// Responses 的输出截断（status=incomplete + reason=max_output_tokens）必须归一化为
    /// "length"，否则「finish_reason==length ⇒ 整批工具调用作废」防护对 Responses 系失效。
    #[test]
    fn incomplete_max_output_tokens_normalizes_to_length() {
        let state = ResponsesStreamState::default();
        let truncated = serde_json::json!({
            "status": "incomplete",
            "incomplete_details": { "reason": "max_output_tokens" }
        });
        assert_eq!(
            responses_finish_reason("incomplete", &truncated, &state),
            "length"
        );
        // 其它 incomplete 原因（如 content_filter）原样透传，不冒充截断。
        let filtered = serde_json::json!({
            "status": "incomplete",
            "incomplete_details": { "reason": "content_filter" }
        });
        assert_eq!(
            responses_finish_reason("incomplete", &filtered, &state),
            "incomplete"
        );
        // completed 的两条既有映射不受影响。
        let done = serde_json::json!({ "status": "completed" });
        assert_eq!(responses_finish_reason("completed", &done, &state), "stop");
    }
}
