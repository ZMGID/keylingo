use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

use tokio::time::{sleep, Duration};

use super::*;
use crate::chat::agent::SteeringMessage;
use crate::chat::model::{StreamPart, StreamSink as _};
use crate::chat::types::ToolCallStatus;
use crate::mcp::types::{
    native_read_file_tool, native_run_command_tool, native_web_fetch_tool, native_write_file_tool,
    McpToolCallResult,
};
use crate::settings::{ChatToolsConfig, ModelProvider};
use crate::state::AppState;

#[derive(Clone, Debug)]
struct RecordedDelta {
    delta: String,
    reasoning_delta: Option<String>,
    segment: Option<ChatMessageSegment>,
}

#[derive(Default)]
struct TestHost {
    records: Mutex<Vec<ToolCallRecord>>,
    deltas: Mutex<Vec<RecordedDelta>>,
    /// Per-call snapshot sizes from `persist_partial_assistant`:
    /// `(message_id, tool_records_len, segments_len, api_messages_len)`.
    persists: Mutex<Vec<(String, usize, usize, usize)>>,
    /// 生成过程中推给前端的上下文占用活数：`(分子, 分母)`，每轮一条。
    context_ticks: Mutex<Vec<(u64, Option<u64>)>>,
    cancel_after: Option<Duration>,
    cancel_flag: Arc<AtomicBool>,
    cancel_on_first_text_delta: bool,
    /// Flip `cancel_flag` from inside `persist_partial_assistant`, so a tool
    /// round completes uncancelled (its records preserved) and the NEXT
    /// round's loop-top generation check observes the cancellation. Used to
    /// exercise the planning/loop-top cancellation path deterministically.
    cancel_on_persist: bool,
    /// 待注入的用户插话（steering），按「批」排队：`take_steering_messages` 每调一次弹出
    /// 一批——模拟真实信箱「取一次清一次、之后新到的下次才取到」的时序。
    steering: Mutex<std::collections::VecDeque<Vec<SteeringMessage>>>,
    /// 原生 follow-up 信箱，同样按批。只在终答边界被 `take_follow_up_messages` 取走。
    follow_up: Mutex<std::collections::VecDeque<Vec<SteeringMessage>>>,
}

impl TestHost {
    fn cancelling_after(delay: Duration) -> Self {
        Self {
            cancel_after: Some(delay),
            ..Self::default()
        }
    }

    fn with_cancel_flag(cancel_flag: Arc<AtomicBool>) -> Self {
        Self {
            cancel_flag,
            ..Self::default()
        }
    }

    fn cancelling_on_first_text_delta() -> Self {
        Self {
            cancel_on_first_text_delta: true,
            ..Self::default()
        }
    }

    fn cancelling_on_persist() -> Self {
        Self {
            cancel_on_persist: true,
            ..Self::default()
        }
    }

    /// 起手就有一条待注入的插话（模拟用户在第一轮工具跑着时点了「立刻引导」）。
    fn with_steering(id: &str, text: &str) -> Self {
        Self::with_steering_batches(vec![vec![
            SteeringMessage::new(id.to_string(), text).expect("non-blank steering text")
        ]])
    }

    /// 按批排队的插话：第 N 次 take 弹出第 N 批（用空批表示「那个时刻信箱是空的」）。
    fn with_steering_batches(batches: Vec<Vec<SteeringMessage>>) -> Self {
        Self {
            steering: Mutex::new(batches.into()),
            ..Self::default()
        }
    }

    fn with_follow_up_batches(batches: Vec<Vec<SteeringMessage>>) -> Self {
        Self {
            follow_up: Mutex::new(batches.into()),
            ..Self::default()
        }
    }

    fn recorded_deltas(&self) -> Vec<RecordedDelta> {
        self.deltas
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    fn recorded_tool_records(&self) -> Vec<ToolCallRecord> {
        self.records
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    fn recorded_persists(&self) -> Vec<(String, usize, usize, usize)> {
        self.persists
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    fn recorded_context_ticks(&self) -> Vec<(u64, Option<u64>)> {
        self.context_ticks
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }
}

impl AgentHost for TestHost {
    fn emit_stream_delta(
        &self,
        _conversation_id: &str,
        _run_id: &str,
        _message_id: &str,
        delta: &str,
        reasoning_delta: Option<&str>,
        segment: Option<&ChatMessageSegment>,
    ) {
        if self.cancel_on_first_text_delta && !delta.is_empty() {
            self.cancel_flag.store(true, Ordering::SeqCst);
        }
        self.deltas
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .push(RecordedDelta {
                delta: delta.to_string(),
                reasoning_delta: reasoning_delta.map(str::to_string),
                segment: segment.cloned(),
            });
    }

    fn emit_tool_record(
        &self,
        _conversation_id: &str,
        _run_id: &str,
        _message_id: &str,
        record: &ToolCallRecord,
    ) {
        self.records
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .push(record.clone());
    }

    fn emit_context_usage_live(
        &self,
        _conversation_id: &str,
        used_tokens: u64,
        context_window_tokens: Option<u64>,
    ) {
        self.context_ticks
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .push((used_tokens, context_window_tokens));
    }

    fn persist_partial_assistant<'a>(
        &'a self,
        _conversation_id: &'a str,
        message_id: &'a str,
        tool_records: &'a [ToolCallRecord],
        segments: &'a [ChatMessageSegment],
        api_messages: &'a [serde_json::Value],
    ) -> super::super::host::AgentHostFuture<'a, ()> {
        if self.cancel_on_persist {
            self.cancel_flag.store(true, Ordering::SeqCst);
        }
        self.persists
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .push((
                message_id.to_string(),
                tool_records.len(),
                segments.len(),
                api_messages.len(),
            ));
        Box::pin(async {})
    }

    fn request_tool_approval<'a>(
        &'a self,
        _ctx: &'a ToolExecutionContext<'a>,
        _record: &'a ToolCallRecord,
    ) -> super::super::host::AgentHostFuture<'a, bool> {
        Box::pin(async { true })
    }

    fn request_session_consent<'a>(
        &'a self,
        _ctx: &'a ToolExecutionContext<'a>,
    ) -> super::super::host::AgentHostFuture<'a, bool> {
        Box::pin(async { true })
    }

    fn request_user_response<'a>(
        &'a self,
        _ctx: &'a ToolExecutionContext<'a>,
        _record: &'a ToolCallRecord,
        _prompt: crate::chat::ask_user::AskUserPromptPayload,
    ) -> super::super::host::AgentHostFuture<'a, crate::chat::ask_user::AskUserResponseResult> {
        Box::pin(async { crate::chat::ask_user::skipped_response() })
    }

    fn is_generation_active(&self, _conversation_id: &str, _generation: u64) -> bool {
        !self.cancel_flag.load(Ordering::SeqCst)
    }

    fn take_steering_messages(&self, _conversation_id: &str) -> Vec<SteeringMessage> {
        self.steering
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .pop_front()
            .unwrap_or_default()
    }

    fn take_follow_up_messages(&self, _conversation_id: &str) -> Vec<SteeringMessage> {
        self.follow_up
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .pop_front()
            .unwrap_or_default()
    }

    fn wait_for_generation_inactive<'a>(
        &'a self,
        _conversation_id: &'a str,
        _generation: u64,
    ) -> super::super::host::AgentHostFuture<'a, ()> {
        let cancel_after = self.cancel_after;
        let cancel_flag = self.cancel_flag.clone();
        Box::pin(async move {
            let started = tokio::time::Instant::now();
            loop {
                if cancel_flag.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(delay) = cancel_after {
                    if started.elapsed() >= delay {
                        return;
                    }
                }
                sleep(Duration::from_millis(2)).await;
            }
        })
    }
}

#[derive(Default)]
struct RecordingExecutor {
    active: AtomicUsize,
    max_active: AtomicUsize,
    events: Arc<Mutex<Vec<String>>>,
}

impl RecordingExecutor {
    fn max_active(&self) -> usize {
        self.max_active.load(Ordering::SeqCst)
    }

    fn events(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }
}

impl ToolExecutor for RecordingExecutor {
    fn call<'a>(
        &'a self,
        _ctx: &'a ToolExecutionContext<'a>,
        tool: &'a ChatToolDefinition,
        _arguments: Value,
        _skill_cache: Option<&'a mut skills::SkillRunCache>,
    ) -> super::super::execute::ToolExecutorFuture<'a> {
        let name = tool.name.clone();
        let events = self.events.clone();
        Box::pin(async move {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            events
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .push(format!("start:{name}"));
            sleep(Duration::from_millis(25)).await;
            events
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .push(format!("finish:{name}"));
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(McpToolCallResult {
                content: format!("result:{name}"),
                is_error: false,
                raw: Value::Null,
                artifacts: Vec::new(),
                structured_content: None,
                follow_up_user_messages: Vec::new(),
            })
        })
    }
}

/// Tool executor that succeeds immediately and flips the shared cancel flag while
/// executing, so cancellation deterministically lands between the tool round and
/// the synthesis request (the executor branch wins the `tokio::select!` because it
/// completes synchronously on its first poll).
struct CancelAfterToolExecutor {
    cancel_flag: Arc<AtomicBool>,
}

impl ToolExecutor for CancelAfterToolExecutor {
    fn call<'a>(
        &'a self,
        _ctx: &'a ToolExecutionContext<'a>,
        tool: &'a ChatToolDefinition,
        _arguments: Value,
        _skill_cache: Option<&'a mut skills::SkillRunCache>,
    ) -> super::super::execute::ToolExecutorFuture<'a> {
        let name = tool.name.clone();
        let cancel_flag = self.cancel_flag.clone();
        Box::pin(async move {
            cancel_flag.store(true, Ordering::SeqCst);
            Ok(McpToolCallResult {
                content: format!("result:{name}"),
                is_error: false,
                raw: Value::Null,
                artifacts: Vec::new(),
                structured_content: None,
                follow_up_user_messages: Vec::new(),
            })
        })
    }
}

/// Scripted HTTP mock for the OpenAI-compatible chat completions endpoint.
/// Responses are served in connection-accept order; each response closes (or
/// deliberately breaks) its connection so reqwest opens a fresh one per request.
enum MockResponse {
    /// SSE stream; each entry becomes one `data: <entry>` event.
    ///
    /// 没有「完整 JSON 响应」这个变体：所有模型调用都走流式线（见
    /// `sse_from_completion_json`）。写着更好读的非流式固件用那个函数转过来。
    Sse(Vec<String>),
    /// Plain HTTP error status with a JSON body.
    Status(u16, String),
    /// Chunked SSE that drops the connection without the chunked terminator,
    /// producing a reqwest decode error (StreamReadInterrupted).
    SseInterrupt(Vec<String>),
    /// Chunked SSE that writes the given events then keeps the connection open,
    /// simulating a hung provider so cancellation paths can win the select.
    SseThenHang(Vec<String>),
}

struct MockModelServer {
    base_url: String,
    captured_bodies: Arc<Mutex<Vec<String>>>,
}

impl MockModelServer {
    fn start(responses: Vec<MockResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock model server");
        let addr = listener.local_addr().expect("mock model server addr");
        let captured_bodies = Arc::new(Mutex::new(Vec::new()));
        let captured_for_thread = Arc::clone(&captured_bodies);
        std::thread::spawn(move || {
            for response in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .ok();
                match read_http_request(&mut stream) {
                    Ok(body) => {
                        captured_for_thread
                            .lock()
                            .unwrap_or_else(|err| err.into_inner())
                            .push(body);
                    }
                    Err(_) => continue,
                }
                serve_mock_response(stream, response);
            }
        });
        Self {
            base_url: format!("http://{addr}/v1"),
            captured_bodies,
        }
    }

    fn captured_bodies(&self) -> Vec<String> {
        self.captured_bodies
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }
}

/// 读完整 HTTP 请求并返回 body 文本（供测试断言请求内容）。
fn read_http_request(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "client closed before request end",
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let headers = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buf.len() < header_end + content_length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(String::from_utf8_lossy(&buf[header_end..]).into_owned())
}

fn sse_body(events: &[String]) -> String {
    events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect()
}

fn serve_mock_response(mut stream: TcpStream, response: MockResponse) {
    match response {
        MockResponse::Status(code, body) => {
            let _ = write!(
                    stream,
                    "HTTP/1.1 {code} Mock Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
        }
        MockResponse::Sse(events) => {
            let body = sse_body(&events);
            let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
        }
        MockResponse::SseInterrupt(events) => {
            let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n"
                );
            for event in events {
                let payload = format!("data: {event}\n\n");
                let _ = write!(stream, "{:x}\r\n{}\r\n", payload.len(), payload);
            }
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return;
        }
        MockResponse::SseThenHang(events) => {
            let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n"
                );
            for event in events {
                let payload = format!("data: {event}\n\n");
                let _ = write!(stream, "{:x}\r\n{}\r\n", payload.len(), payload);
            }
            let _ = stream.flush();
            std::thread::sleep(std::time::Duration::from_secs(5));
            return;
        }
    }
    let _ = stream.flush();
}

/// Minimal in-memory AppState for run_agent_loop tests. Settings live only in
/// memory and usage records go to a unique temp dir, so user settings/providers
/// are never touched.
fn test_app_state() -> AppState {
    let offline_models =
        crate::offline_models::OfflineModelManager::headless(reqwest::Client::new());
    AppState::base(
        Settings::default(),
        std::env::temp_dir().join(format!(
            "kivio-agent-loop-test-usage-{}",
            uuid::Uuid::new_v4()
        )),
        reqwest::Client::new(),
        #[cfg(target_os = "macos")]
        crate::macos_ocr::MacOcrClient::disabled(),
        offline_models.clone(),
        crate::rapidocr::RapidOcrClient::new(offline_models.clone()),
        crate::inpainting::InpaintingClient::new(offline_models),
    )
}

fn test_provider(base_url: &str) -> ModelProvider {
    ModelProvider {
        id: "test-provider".to_string(),
        name: "Test Provider".to_string(),
        api_keys: vec!["test-key".to_string()],
        api_key_legacy: None,
        base_url: base_url.to_string(),
        available_models: Vec::new(),
        enabled_models: Vec::new(),
        enabled: true,
        api_format: "openai_chat".to_string(),
        model_overrides: std::collections::HashMap::new(),
        compress_request_body: false,
        request: Default::default(),
    }
}

fn test_run_config<'a>(
    state: &'a AppState,
    base_url: &str,
) -> AgentRunConfig<'a> {
    AgentRunConfig {
        state,
        conversation_id: "conversation".to_string(),
        tool_conversation_id: "conversation".to_string(),
        depth: 0,
        run_id: "run".to_string(),
        message_id: "message".to_string(),
        generation: 1,
        provider: test_provider(base_url),
        model: "test-model".to_string(),
        runtime_messages: vec![
            serde_json::json!({ "role": "system", "content": "system prompt" }),
            serde_json::json!({ "role": "user", "content": "请读取文件" }),
        ],
        tools: vec![native_read_file_tool()],
        blocked_tool_calls: Vec::new(),
        settings: Settings::default(),
        effective_chat_tools: ChatToolsConfig {
            max_tool_rounds: Some(1),
            ..ChatToolsConfig::default()
        },
        language: "zh-CN".to_string(),
        thinking_enabled: false,
        thinking_level: None,
        web_search_mode: crate::chat::types::WebSearchMode::Off,
        max_output_tokens: 1024,
        retry_attempts: 1,
        assistant_snapshot: None,
        provider_tools_fallback_system_prompt: String::new(),
        initial_anchor_total_tokens: None,
        initial_anchor_trailing_estimate: 0,
        skill_project_cwd: None,
    }
}

/// 构造一条 SSE delta，其 content 为 `prefix + 240 个填充字符`（总 ≥200 chars），
/// 满足 compaction 质量兜底 `MIN_SUMMARY_CHARS`。L2 摘要 mock 用它返回达标摘要。
fn long_summary_sse(prefix: &str) -> String {
    let pad = "细节填充".repeat(60);
    format!(r#"{{"choices":[{{"delta":{{"content":"{prefix}{pad}"}}}}]}}"#)
}

/// 同上，但 content 包在 `<summary>...</summary>` 内（验证 extract_summary_text 抽签）。
fn long_summary_sse_tagged(prefix: &str) -> String {
    let pad = "细节填充".repeat(60);
    format!(r#"{{"choices":[{{"delta":{{"content":"<summary>\n{prefix}{pad}\n</summary>"}}}}]}}"#)
}

/// Streaming planning step: one `read` tool call, then `[DONE]`.
fn planning_tool_call_sse_events() -> Vec<String> {
    vec![
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_read","function":{"name":"read","arguments":"{\"path\":\"/tmp/kivio-test.txt\"}"}}]}}]}"#
                .to_string(),
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#.to_string(),
            "[DONE]".to_string(),
        ]
}

/// 把一份非流式 chat-completion JSON 固件转成等价的 SSE 事件序列。
///
/// 「要完整结果」的模型调用现在**一律走流式线**（`generate_via_stream_collect`，理由见
/// planning.rs 上 `call_chat_completion_output_with_usage` 的注释）。
/// 固件用非流式 JSON 写着更好读，所以在这里做一次机械转换，而不是把每个测试都手抄成 SSE。
fn sse_from_completion_json(body: &str) -> Vec<String> {
    let parsed: Value = serde_json::from_str(body).expect("mock body must be valid JSON");
    let message = &parsed["choices"][0]["message"];
    let mut events = Vec::new();
    if let Some(reasoning) = message["reasoning_content"]
        .as_str()
        .filter(|text| !text.is_empty())
    {
        events.push(
            serde_json::json!({"choices":[{"delta":{"reasoning_content": reasoning}}]}).to_string(),
        );
    }
    if let Some(content) = message["content"].as_str().filter(|text| !text.is_empty()) {
        events.push(serde_json::json!({"choices":[{"delta":{"content": content}}]}).to_string());
    }
    if let Some(tool_calls) = message["tool_calls"].as_array() {
        // 流式约定要 index；参数不切片（测试不关心分片，只关心最终 draft）。
        let deltas = tool_calls
            .iter()
            .enumerate()
            .map(|(index, call)| {
                let mut call = call.clone();
                call["index"] = serde_json::json!(index);
                call
            })
            .collect::<Vec<_>>();
        events.push(serde_json::json!({"choices":[{"delta":{"tool_calls": deltas}}]}).to_string());
    }
    if let Some(finish) = parsed["choices"][0]["finish_reason"].as_str() {
        events.push(
            serde_json::json!({"choices":[{"delta":{},"finish_reason": finish}]}).to_string(),
        );
    }
    // usage 必须带过来：静默超窗的判定就靠 provider 实报的 prompt_tokens（空正文 + 顶窗
    // ⇒ 改判 ContextOverflow）。流式里 usage 走末尾那个 choices 为空的块。
    if let Some(usage) = parsed.get("usage").filter(|usage| !usage.is_null()) {
        events.push(serde_json::json!({"choices":[],"usage": usage}).to_string());
    }
    events.push("[DONE]".to_string());
    events
}

fn test_round_context() -> ToolRoundContext<'static> {
    ToolRoundContext {
        conversation_id: "conversation",
        run_id: "run",
        message_id: "message",
        generation: 1,
        round: 1,
        depth: 0,
        tool_conversation_id: "conversation",
        finish_reason: None,
    }
}

fn pending_tool_call(id: &str, function_name: &str) -> PendingToolCall {
    let arguments = test_tool_arguments(function_name);
    PendingToolCall {
        id: id.to_string(),
        function_name: function_name.to_string(),
        arguments_raw: serde_json::to_string(&arguments).expect("serialize test arguments"),
        arguments,
        arguments_parse_error: None,
        signature: None,
    }
}

fn test_tool_arguments(function_name: &str) -> Value {
    match function_name.to_ascii_lowercase().as_str() {
        "read" => serde_json::json!({ "path": "/tmp/kivio-test.txt" }),
        "web_fetch" => serde_json::json!({ "url": "https://example.com" }),
        "bash" | "run_command" => serde_json::json!({ "command": "echo 1" }),
        "ask_user" => serde_json::json!({
            "questions": [
                {
                    "id": "scope",
                    "prompt": "Which scope should I use?",
                    "options": [
                        { "id": "mvp", "label": "MVP" },
                        { "id": "full", "label": "Full" }
                    ]
                }
            ]
        }),
        _ => serde_json::json!({}),
    }
}

#[test]
fn visible_tool_segment_calls_skip_hidden_disabled_builtin_feedback() {
    let tools = vec![native_read_file_tool()];
    let blocked = vec![native_run_command_tool()];
    let calls = vec![
        pending_tool_call("call_read", "read"),
        pending_tool_call("call_blocked", "bash"),
        pending_tool_call("call_hidden_disabled", "web_search"),
        pending_tool_call("call_unknown", "mcp__server__tool"),
    ];

    let visible = visible_tool_segment_calls(&tools, &blocked, &calls);

    assert_eq!(
        visible
            .iter()
            .map(|call| call.id.as_str())
            .collect::<Vec<_>>(),
        vec!["call_read", "call_blocked", "call_unknown"]
    );
}

#[test]
fn reasoning_segment_order_precedes_text_in_same_step() {
    let mut builder = SegmentBuilder::new();
    let reasoning = builder.reserve(
        ChatMessageSegmentKind::Reasoning,
        ChatMessageSegmentPhase::ToolLoop,
        Some(1),
        Some(1),
        "step_1_reasoning",
    );
    let text = builder.reserve(
        ChatMessageSegmentKind::Text,
        ChatMessageSegmentPhase::ToolLoop,
        Some(1),
        Some(1),
        "step_1_text",
    );

    assert!(reasoning.order < text.order);
}

fn test_mcp_tool(name: &str, annotations: Value) -> ChatToolDefinition {
    ChatToolDefinition {
        id: format!("mcp__demo__{name}"),
        name: name.to_string(),
        description: "MCP test tool".to_string(),
        source: "mcp".to_string(),
        server_id: Some("demo".to_string()),
        server_name: Some("Demo".to_string()),
        input_schema: serde_json::json!({ "type": "object", "properties": {} }),
        sensitive: false,
        annotations: Some(annotations),
        output_schema: None,
    }
}

fn tool_call_ids(messages: &[Value]) -> Vec<&str> {
    messages
        .iter()
        .filter_map(|message| message.get("tool_call_id").and_then(Value::as_str))
        .collect()
}

#[test]
fn tool_round_limit_reached_only_for_finite_limits_at_boundary() {
    assert!(!tool_round_limit_reached(None, 10));
    assert!(!tool_round_limit_reached(Some(3), 2));
    assert!(tool_round_limit_reached(Some(3), 3));
    assert!(tool_round_limit_reached(Some(3), 4));
}

#[tokio::test]
async fn tool_round_runs_parallel_eligible_tools_concurrently_and_keeps_result_order() {
    let host = TestHost::default();
    let executor = RecordingExecutor::default();
    let settings = Settings::default();
    let tools = vec![native_read_file_tool(), native_web_fetch_tool()];
    let mut skill_cache = skills::SkillRunCache::default();

    let result = execute_tool_round(
        &host,
        &executor,
        &settings,
        test_round_context(),
        &tools,
        &[],
        vec![
            pending_tool_call("call_read", "read"),
            pending_tool_call("call_fetch", "web_fetch"),
        ],
        &mut skill_cache,
    )
    .await;

    assert_eq!(executor.max_active(), 2);
    let events = executor.events();
    let first_finish = events
        .iter()
        .position(|event| event.starts_with("finish:"))
        .expect("finish event");
    assert_eq!(
        first_finish, 2,
        "both calls should start before either finishes"
    );
    assert_eq!(result.response_messages.len(), 2);
    assert_eq!(
        result.response_messages[0]
            .get("tool_call_id")
            .and_then(Value::as_str),
        Some("call_read")
    );
    assert_eq!(
        result.response_messages[1]
            .get("tool_call_id")
            .and_then(Value::as_str),
        Some("call_fetch")
    );
    assert_eq!(result.tool_records.len(), 2);
    assert!(result
        .tool_records
        .iter()
        .all(|record| matches!(record.status, ToolCallStatus::Success)));
}

#[test]
fn write_tools_stay_outside_parallel_whitelist() {
    let mut settings = Settings::default();
    settings.chat_tools.approval_policy = "auto".to_string();
    for tool in [
        crate::mcp::types::native_write_file_tool(),
        crate::mcp::types::native_edit_file_tool(),
    ] {
        assert!(
            !tool_call_parallel_eligible(&settings, &tool),
            "{} must stay serial even when approval is auto",
            tool.name
        );
    }
    assert!(
        tool_call_parallel_eligible(&settings, &native_read_file_tool()),
        "read-only tools remain parallel-eligible"
    );
}

#[tokio::test]
async fn tool_round_runs_read_only_mcp_tools_concurrently() {
    let host = TestHost::default();
    let executor = RecordingExecutor::default();
    let settings = Settings::default();
    let tools = vec![
        test_mcp_tool("search_a", serde_json::json!({ "readOnlyHint": true })),
        test_mcp_tool("search_b", serde_json::json!({ "readOnlyHint": true })),
    ];
    let mut skill_cache = skills::SkillRunCache::default();

    let result = execute_tool_round(
        &host,
        &executor,
        &settings,
        test_round_context(),
        &tools,
        &[],
        vec![
            pending_tool_call("call_mcp_a", "search_a"),
            pending_tool_call("call_mcp_b", "search_b"),
        ],
        &mut skill_cache,
    )
    .await;

    assert_eq!(executor.max_active(), 2);
    assert_eq!(
        tool_call_ids(&result.response_messages),
        vec!["call_mcp_a", "call_mcp_b"]
    );
    assert!(result
        .tool_records
        .iter()
        .all(|record| matches!(record.status, ToolCallStatus::Success)));
}

#[tokio::test]
async fn tool_round_keeps_ask_user_serial_between_parallel_safe_tools() {
    let host = TestHost::default();
    let executor = RecordingExecutor::default();
    let settings = Settings::default();
    let mut tools = vec![native_read_file_tool(), native_web_fetch_tool()];
    crate::chat::ask_user::append_tool_definitions(&mut tools);
    let mut skill_cache = skills::SkillRunCache::default();

    let result = execute_tool_round(
        &host,
        &executor,
        &settings,
        test_round_context(),
        &tools,
        &[],
        vec![
            pending_tool_call("call_read", "read"),
            pending_tool_call("call_ask", "ask_user"),
            pending_tool_call("call_fetch", "web_fetch"),
        ],
        &mut skill_cache,
    )
    .await;

    assert_eq!(executor.max_active(), 1);
    assert_eq!(
        executor.events(),
        vec![
            "start:read",
            "finish:read",
            "start:web_fetch",
            "finish:web_fetch"
        ]
    );
    assert_eq!(
        tool_call_ids(&result.response_messages),
        vec!["call_read", "call_ask", "call_fetch"]
    );
    let ask_user_record = result
        .tool_records
        .iter()
        .find(|record| record.id == "call_ask")
        .expect("ask_user record");
    assert!(matches!(ask_user_record.status, ToolCallStatus::Skipped));
    assert_eq!(
        ask_user_record.name,
        crate::chat::ask_user::ASK_USER_TOOL_NAME
    );
    assert_eq!(ask_user_record.trace_id.as_deref(), Some("run"));
    assert_eq!(
        ask_user_record.span_id.as_deref(),
        Some("tool_round_1_call_ask")
    );
}

#[tokio::test]
async fn tool_round_keeps_destructive_mcp_tools_serial() {
    let host = TestHost::default();
    let executor = RecordingExecutor::default();
    let mut settings = Settings::default();
    settings.chat_tools.approval_policy = "auto".to_string();
    let tools = vec![test_mcp_tool(
        "write_remote",
        serde_json::json!({ "destructiveHint": true }),
    )];
    let mut skill_cache = skills::SkillRunCache::default();

    let result = execute_tool_round(
        &host,
        &executor,
        &settings,
        test_round_context(),
        &tools,
        &[],
        vec![
            pending_tool_call("call_mcp_write_1", "write_remote"),
            pending_tool_call("call_mcp_write_2", "write_remote"),
        ],
        &mut skill_cache,
    )
    .await;

    assert_eq!(executor.max_active(), 1);
    assert_eq!(
        tool_call_ids(&result.response_messages),
        vec!["call_mcp_write_1", "call_mcp_write_2"]
    );
}

#[tokio::test]
async fn tool_round_keeps_open_world_mcp_tools_serial_even_when_read_only() {
    let host = TestHost::default();
    let executor = RecordingExecutor::default();
    let mut settings = Settings::default();
    settings.chat_tools.approval_policy = "auto".to_string();
    let tools = vec![test_mcp_tool(
        "remote_search",
        serde_json::json!({ "readOnlyHint": true, "openWorldHint": true }),
    )];
    let mut skill_cache = skills::SkillRunCache::default();

    let result = execute_tool_round(
        &host,
        &executor,
        &settings,
        test_round_context(),
        &tools,
        &[],
        vec![
            pending_tool_call("call_mcp_remote_1", "remote_search"),
            pending_tool_call("call_mcp_remote_2", "remote_search"),
        ],
        &mut skill_cache,
    )
    .await;

    assert_eq!(executor.max_active(), 1);
    assert_eq!(
        tool_call_ids(&result.response_messages),
        vec!["call_mcp_remote_1", "call_mcp_remote_2"]
    );
}

#[tokio::test]
async fn tool_round_preserves_unknown_and_invalid_call_order() {
    let host = TestHost::default();
    let executor = RecordingExecutor::default();
    let settings = Settings::default();
    let tools = vec![native_read_file_tool(), native_web_fetch_tool()];
    let mut skill_cache = skills::SkillRunCache::default();
    let mut invalid_fetch = pending_tool_call("call_bad_args", "web_fetch");
    invalid_fetch.arguments_parse_error = Some("expected compact object".to_string());

    let result = execute_tool_round(
        &host,
        &executor,
        &settings,
        test_round_context(),
        &tools,
        &[],
        vec![
            pending_tool_call("call_read", "read"),
            pending_tool_call("call_fetch", "web_fetch"),
            pending_tool_call("call_missing", "missing_tool"),
            pending_tool_call("call_read_after_unknown", "read"),
            invalid_fetch,
            pending_tool_call("call_final", "read"),
        ],
        &mut skill_cache,
    )
    .await;

    let error_records = result
        .tool_records
        .iter()
        .filter(|record| matches!(record.status, ToolCallStatus::Error))
        .collect::<Vec<_>>();

    assert_eq!(executor.max_active(), 2);
    assert_eq!(
        tool_call_ids(&result.response_messages),
        vec![
            "call_read",
            "call_fetch",
            "call_missing",
            "call_read_after_unknown",
            "call_bad_args",
            "call_final"
        ]
    );
    assert_eq!(
        result
            .tool_records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "call_read",
            "call_fetch",
            "call_missing",
            "call_read_after_unknown",
            "call_bad_args",
            "call_final"
        ]
    );
    assert_eq!(
        error_records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>(),
        vec!["call_missing", "call_bad_args"]
    );
    assert!(error_records
        .iter()
        .all(|record| record.trace_id.as_deref() == Some("run")));
    assert_eq!(
        error_records
            .iter()
            .map(|record| record.span_id.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("tool_round_1_call_missing"),
            Some("tool_round_1_call_bad_args")
        ]
    );
    let start_events = executor
        .events()
        .into_iter()
        .filter(|event| event.starts_with("start:"))
        .collect::<Vec<_>>();
    assert_eq!(start_events.len(), 4, "only executable tools should run");

    // 喂回自愈（对齐 opencode）：未知工具的 tool result 必须列出已声明工具，
    // 让模型下一轮自我纠正，而不是只丢一句 Unknown。
    let unknown_message = result
        .response_messages
        .iter()
        .find(|message| message["tool_call_id"] == "call_missing")
        .expect("unknown tool response present");
    let content = unknown_message["content"].as_str().unwrap_or_default();
    assert!(content.contains("Unknown tool: missing_tool"), "{content}");
    assert!(content.contains("Available tools:"), "{content}");
    assert!(content.contains("read"), "{content}");
    assert!(content.contains("web_fetch"), "{content}");
}

#[tokio::test]
async fn tool_round_matches_capitalized_tool_names_case_insensitively() {
    // Cursor 系模型（grok-composer 等）会按训练时的大写工具名出牌（Read/Grep）。
    // 唯一命中时按声明工具执行，不走未知工具路径。
    let host = TestHost::default();
    let executor = RecordingExecutor::default();
    let settings = Settings::default();
    let tools = vec![native_read_file_tool(), native_web_fetch_tool()];
    let mut skill_cache = skills::SkillRunCache::default();

    let result = execute_tool_round(
        &host,
        &executor,
        &settings,
        test_round_context(),
        &tools,
        &[],
        vec![pending_tool_call("call_cap_read", "Read")],
        &mut skill_cache,
    )
    .await;

    assert!(
        result
            .tool_records
            .iter()
            .all(|record| !matches!(record.status, ToolCallStatus::Error)),
        "capitalized Read must execute, not error: {:?}",
        result.tool_records
    );
    let start_events = executor
        .events()
        .into_iter()
        .filter(|event| event.starts_with("start:"))
        .collect::<Vec<_>>();
    assert_eq!(start_events.len(), 1, "the case-variant call executes");
}

#[tokio::test]
async fn tool_round_records_plan_blocked_tool_as_skipped() {
    let host = TestHost::default();
    let executor = RecordingExecutor::default();
    let settings = Settings::default();
    let tools = vec![native_read_file_tool()];
    let blocked = vec![native_run_command_tool()];
    let mut skill_cache = skills::SkillRunCache::default();

    let result = execute_tool_round(
        &host,
        &executor,
        &settings,
        test_round_context(),
        &tools,
        &blocked,
        vec![pending_tool_call("call_bash", "bash")],
        &mut skill_cache,
    )
    .await;

    assert_eq!(executor.max_active(), 0);
    assert_eq!(result.response_messages.len(), 1);
    assert!(result.response_messages[0]
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .contains("blocked in Plan mode"));
    assert_eq!(result.tool_records.len(), 1);
    let record = &result.tool_records[0];
    assert_eq!(record.id, "call_bash");
    assert_eq!(record.name, "bash");
    assert!(matches!(record.status, ToolCallStatus::Skipped));
    assert!(record
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("blocked in Plan mode"));
    assert_eq!(record.trace_id.as_deref(), Some("run"));
    assert_eq!(record.span_id.as_deref(), Some("tool_round_1_call_bash"));
}

#[tokio::test]
async fn tool_round_cancels_unstarted_calls_after_running_tool_is_cancelled() {
    let host = TestHost::cancelling_after(Duration::from_millis(5));
    let executor = RecordingExecutor::default();
    let settings = Settings::default();
    let tools = vec![native_read_file_tool(), native_run_command_tool()];
    let mut skill_cache = skills::SkillRunCache::default();

    let result = execute_tool_round(
        &host,
        &executor,
        &settings,
        test_round_context(),
        &tools,
        &[],
        vec![
            pending_tool_call("call_read", "read"),
            pending_tool_call("call_bash", "bash"),
        ],
        &mut skill_cache,
    )
    .await;

    assert!(result.cancelled);
    assert_eq!(
        tool_call_ids(&result.response_messages),
        vec!["call_read", "call_bash"]
    );
    assert_eq!(
        result
            .tool_records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>(),
        vec!["call_read", "call_bash"]
    );
    assert!(result
        .tool_records
        .iter()
        .all(|record| matches!(record.status, ToolCallStatus::Cancelled)));
    let start_events = executor
        .events()
        .into_iter()
        .filter(|event| event.starts_with("start:"))
        .collect::<Vec<_>>();
    assert_eq!(
        start_events,
        vec!["start:read"],
        "remaining serial tools must not start after cancellation"
    );
}

#[test]
fn cancelled_tool_round_result_preserves_replay_messages_for_storage() {
    let tool_record = ToolCallRecord {
        id: "call_read".to_string(),
        name: "read".to_string(),
        source: "native".to_string(),
        server_id: None,
        arguments: "{}".to_string(),
        status: ToolCallStatus::Cancelled,
        result_preview: None,
        error: Some("Tool call cancelled".to_string()),
        duration_ms: Some(5),
        started_at: Some(10),
        completed_at: Some(11),
        round: 1,
        sensitive: false,
        artifacts: Vec::new(),
        trace_id: Some("run".to_string()),
        span_id: Some("tool_round_1_call_read".to_string()),
        structured_content: None,
    };
    let assistant_message = serde_json::json!({
        "role": "assistant",
        "content": Value::Null,
        "tool_calls": [{
            "id": "call_read",
            "type": "function",
            "function": {
                "name": "read",
                "arguments": "{}",
            }
        }],
    });
    let tool_response = tool_message("call_read".to_string(), "Tool call cancelled");
    let result = cancelled_tool_round_run_result(
        "zh-CN",
        &["planning".to_string()],
        vec![tool_record.clone()],
        vec![ChatMessageSegment {
            id: "seg_1000_tool_call_read".to_string(),
            kind: ChatMessageSegmentKind::Tool,
            phase: ChatMessageSegmentPhase::ToolLoop,
            order: 1000,
            step_number: Some(1),
            round: Some(1),
            text: None,
            tool_call_id: Some("call_read".to_string()),
        }],
        vec![assistant_message.clone(), tool_response.clone()],
    );

    assert_eq!(result.content, "已停止生成。");
    assert_eq!(result.reasoning.as_deref(), Some("planning"));
    assert_eq!(result.tool_records.len(), 1);
    assert!(result.segments.iter().any(|segment| {
        segment.kind == ChatMessageSegmentKind::Tool
            && segment.tool_call_id.as_deref() == Some("call_read")
    }));
    assert!(result.segments.iter().any(|segment| {
        segment.kind == ChatMessageSegmentKind::Text
            && segment.phase == ChatMessageSegmentPhase::Synthesis
            && segment.text.as_deref() == Some("已停止生成。")
    }));
    assert!(matches!(
        result.tool_records[0].status,
        ToolCallStatus::Cancelled
    ));
    assert_eq!(result.api_messages.len(), 3);
    assert_eq!(
        result.api_messages[0]
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        result.api_messages[1]
            .get("tool_call_id")
            .and_then(Value::as_str),
        Some("call_read")
    );
    assert_eq!(
        result.api_messages[2]
            .get("content")
            .and_then(Value::as_str),
        Some("已停止生成。")
    );
}

#[tokio::test]
async fn tool_round_keeps_serial_only_tools_non_overlapping() {
    let host = TestHost::default();
    let executor = RecordingExecutor::default();
    let settings = Settings::default();
    let tools = vec![native_run_command_tool()];
    let mut skill_cache = skills::SkillRunCache::default();

    let result = execute_tool_round(
        &host,
        &executor,
        &settings,
        test_round_context(),
        &tools,
        &[],
        vec![
            pending_tool_call("call_bash_1", "bash"),
            pending_tool_call("call_bash_2", "bash"),
        ],
        &mut skill_cache,
    )
    .await;

    assert_eq!(executor.max_active(), 1);
    assert_eq!(
        executor.events(),
        vec![
            "start:bash",
            "finish:bash",
            "start:bash",
            "finish:bash"
        ]
    );
    assert_eq!(result.response_messages.len(), 2);
    assert_eq!(
        result.response_messages[0]
            .get("tool_call_id")
            .and_then(Value::as_str),
        Some("call_bash_1")
    );
    assert_eq!(
        result.response_messages[1]
            .get("tool_call_id")
            .and_then(Value::as_str),
        Some("call_bash_2")
    );
}

// ===== Fallback scenarios (6 synthesis/planning fallbacks in run_agent_loop) =====

#[test]
fn fallback_responses_are_bilingual() {
    assert_eq!(
            empty_synthesis_fallback_response("zh-CN"),
            "工具调用已经完成，但模型没有返回最终总结。上方工具结果已保存在本轮回复中，你可以继续追问，或让我重新生成总结。"
        );
    assert_eq!(
            empty_synthesis_fallback_response("en-US"),
            "The tool calls completed, but the model did not return a final summary. The tool results above were saved with this reply; you can continue from them or regenerate the summary."
        );
    assert_eq!(
            synthesis_failed_fallback_response("zh-CN"),
            "最终总结生成失败(可能是模型供应商内容审核拦截)。上方工具结果已保存在本轮回复中,你可以继续追问、让我重新生成,或更换聊天模型再试。"
        );
    assert_eq!(
            synthesis_failed_fallback_response("en-US"),
            "Final summary generation failed (possibly provider content moderation). The tool results above were saved with this reply; you can continue from them, regenerate, or switch the chat model and retry."
        );
    assert_eq!(
            tool_planning_failed_fallback_response("zh-CN"),
            "工具调用参数生成失败，这一步还没有真正执行写入。主对话已保留，你可以让我缩小范围、改用补丁，或重新生成。"
        );
    assert_eq!(
            tool_planning_failed_fallback_response("en-US"),
            "Tool-call argument generation failed before the write actually ran. This conversation was preserved; you can ask me to narrow the scope, use a patch, or regenerate."
        );
    assert_eq!(stopped_generation_content("zh-CN"), "已停止生成。");
    assert_eq!(stopped_generation_content("en-US"), "Generation stopped.");
}

/// Fallback A (helper level, deterministic): a planning stream dies while tool
/// argument drafts are in flight; the run result must mark every draft record
/// as error, emit the bilingual fallback, and finish with stream_outcome "error".
#[test]
fn tool_planning_failed_run_result_marks_drafts_error_and_emits_fallback() {
    let state = test_app_state();
    let config = test_run_config(&state, "http://127.0.0.1:9/v1");
    let host = TestHost::default();

    let mut segment_builder = SegmentBuilder::new();
    let _reasoning_segment = segment_builder.reserve(
        ChatMessageSegmentKind::Reasoning,
        ChatMessageSegmentPhase::ToolLoop,
        Some(1),
        Some(1),
        "step_1_reasoning",
    );
    let planning_text_segment = segment_builder.reserve(
        ChatMessageSegmentKind::Text,
        ChatMessageSegmentPhase::ToolLoop,
        Some(1),
        Some(1),
        "step_1_text",
    );
    let tracker = ToolCallDraftTracker::new(
        vec![native_write_file_tool()],
        1,
        Some(1),
        segment_builder.next_order(),
    );
    let mut sink = AgentStreamSink::new(
        &host,
        "conversation",
        "run",
        "message",
        true,
        None,
        None,
        Some(tracker.clone()),
        None,
    );
    sink.emit(StreamPart::ToolCallStart {
        id: "call_write".to_string(),
        name: "write".to_string(),
    })
    .expect("tool call start should emit");
    sink.emit(StreamPart::ToolCallDelta {
        id: "call_write".to_string(),
        delta: "{\"path\":\"large.html\",\"content\":\"".to_string(),
    })
    .expect("tool call delta should emit");
    assert!(tracker.has_started());

    let result = tool_planning_failed_run_result(
        &host,
        &config,
        segment_builder,
        planning_text_segment,
        tracker,
        &["planning thought".to_string()],
        Vec::new(),
        "Chat tools planning read body failed".to_string(),
    );

    let fallback = tool_planning_failed_fallback_response("zh-CN");
    assert_eq!(result.stream_outcome, "error");
    assert_eq!(result.content, fallback);
    assert_eq!(result.reasoning.as_deref(), Some("planning thought"));
    assert_eq!(result.tool_records.len(), 1);
    assert_eq!(result.tool_records[0].id, "call_write");
    assert!(matches!(
        result.tool_records[0].status,
        ToolCallStatus::Error
    ));
    assert_eq!(
        result.tool_records[0].error.as_deref(),
        Some("Chat tools planning read body failed")
    );
    assert!(result.tool_records[0].completed_at.is_some());

    // The error record was re-emitted through the host (after the pending draft).
    let emitted = host.recorded_tool_records();
    let last_emitted = emitted.last().expect("error record emitted");
    assert_eq!(last_emitted.id, "call_write");
    assert!(matches!(last_emitted.status, ToolCallStatus::Error));

    // Draft tool segment is preserved and the fallback text segment is
    // synthesis-phase with no round.
    assert!(result.segments.iter().any(|segment| {
        segment.kind == ChatMessageSegmentKind::Tool
            && segment.tool_call_id.as_deref() == Some("call_write")
    }));
    let fallback_segment = result
        .segments
        .iter()
        .find(|segment| segment.kind == ChatMessageSegmentKind::Text)
        .expect("fallback text segment");
    assert_eq!(fallback_segment.phase, ChatMessageSegmentPhase::Synthesis);
    assert_eq!(fallback_segment.round, None);
    assert_eq!(fallback_segment.text.as_deref(), Some(fallback.as_str()));

    // Fallback delta.
    assert!(host
        .recorded_deltas()
        .iter()
        .any(|delta| delta.delta == fallback));

    // The final assistant message is pushed unconditionally here.
    assert_eq!(result.api_messages.len(), 1);
    assert_eq!(
        result.api_messages[0]
            .get("content")
            .and_then(Value::as_str),
        Some(fallback.as_str())
    );

    // The whole turn is preserved: exactly the one failed tool record.
    assert_eq!(result.tool_records.len(), 1);
}

/// Fallback A (integration): the provider stream breaks mid-connection after a
/// tool-call draft has started; run_agent_loop must return Ok with
/// stream_outcome "error" instead of bubbling an invoke error.
#[tokio::test]
async fn run_loop_stream_planning_interrupt_after_tool_draft_returns_error_result() {
    let server = MockModelServer::start(vec![MockResponse::SseInterrupt(vec![
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_read","function":{"name":"read","arguments":"{\"path\":\"/tmp/"}}]}}]}"#
                .to_string(),
        ])]);
    let state = test_app_state();
    let config = test_run_config(&state, &server.base_url);
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("planning interruption with tool draft must not bubble Err");

    let fallback = tool_planning_failed_fallback_response("zh-CN");
    assert_eq!(result.stream_outcome, "error");
    assert_eq!(result.content, fallback);
    assert_eq!(result.tool_records.len(), 1);
    assert_eq!(result.tool_records[0].id, "call_read");
    assert!(matches!(
        result.tool_records[0].status,
        ToolCallStatus::Error
    ));
    assert!(
        executor.events().is_empty(),
        "interrupted tool drafts must never execute"
    );
    assert_eq!(result.api_messages.len(), 1);
    assert_eq!(
        result.api_messages[0]
            .get("content")
            .and_then(Value::as_str),
        Some(fallback.as_str())
    );
}

/// Fallback B: streamed synthesis request fails (HTTP 400) after a successful
/// tool round; the tool records must survive with the bilingual fallback text.
#[tokio::test]
async fn run_loop_stream_synthesis_failure_preserves_tool_records_with_fallback() {
    let server = MockModelServer::start(vec![
        MockResponse::Sse(planning_tool_call_sse_events()),
        MockResponse::Status(400, r#"{"error":"mock synthesis failure"}"#.to_string()),
    ]);
    let state = test_app_state();
    let config = test_run_config(&state, &server.base_url);
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("synthesis failure after tool records must not bubble Err");

    // 框架恢复:合成被拒(400)后不再吐静态"失败"文案,而是用已收集的工具结果兜底。
    let recovered = crate::chat::agent::recovery::assemble_results_from_tool_records(
        &result.tool_records,
        "zh-CN",
        crate::chat::agent::recovery::FailureKind::Other,
    );
    assert!(recovered.contains("result:read"));
    assert_eq!(result.stream_outcome, "recovered");
    assert_eq!(result.content, recovered);
    assert_eq!(result.tool_records.len(), 1);
    assert_eq!(result.tool_records[0].id, "call_read");
    assert!(matches!(
        result.tool_records[0].status,
        ToolCallStatus::Success
    ));

    // assistant tool_calls message + tool result + final recovered message.
    assert_eq!(result.api_messages.len(), 3);
    assert_eq!(
        result.api_messages[2]
            .get("content")
            .and_then(Value::as_str),
        Some(recovered.as_str())
    );

    let fallback_delta = host
        .recorded_deltas()
        .into_iter()
        .find(|delta| delta.delta == recovered)
        .expect("recovered delta emitted");
    assert_eq!(fallback_delta.reasoning_delta, None);
    let segment = fallback_delta.segment.expect("fallback delta has segment");
    assert_eq!(segment.kind, ChatMessageSegmentKind::Text);
    assert_eq!(segment.phase, ChatMessageSegmentPhase::Synthesis);

    // 错误卡片要拿到**供应商的原话**，而不是只有"模型调用失败"这句分类话术。
    let degraded = result
        .degraded
        .expect("degrade path must attach a card payload");
    assert_eq!(degraded.kind, "unknown");
    assert_eq!(
        degraded.detail.as_deref(),
        Some("400 Bad Request - mock synthesis failure (attempt 1/1)"),
        "raw provider error must survive into the card"
    );
}

/// Fallback C: streamed synthesis is cancelled after tool results exist; the run
/// returns Ok("cancelled") with the stopped-generation placeholder content.
#[tokio::test]
async fn run_loop_stream_synthesis_cancelled_returns_cancelled_with_stopped_content() {
    let server = MockModelServer::start(vec![
        MockResponse::Sse(planning_tool_call_sse_events()),
        MockResponse::SseThenHang(Vec::new()),
    ]);
    let state = test_app_state();
    let config = test_run_config(&state, &server.base_url);
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let host = TestHost::with_cancel_flag(cancel_flag.clone());
    let executor = CancelAfterToolExecutor { cancel_flag };

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("cancelled synthesis with tool records must not bubble Err");

    assert_eq!(result.stream_outcome, "cancelled");
    assert_eq!(result.content, stopped_generation_content("zh-CN"));
    assert_eq!(result.tool_records.len(), 1);
    assert!(matches!(
        result.tool_records[0].status,
        ToolCallStatus::Success
    ));

    assert_eq!(
        result
            .api_messages
            .last()
            .and_then(|message| message.get("content"))
            .and_then(Value::as_str),
        Some("已停止生成。")
    );
    assert!(result.segments.iter().any(|segment| {
        segment.kind == ChatMessageSegmentKind::Text
            && segment.text.as_deref() == Some("已停止生成。")
    }));
}

/// Fallback C variant: synthesis streamed some text before cancellation; the
/// partial text must be kept instead of the stopped-generation placeholder.
#[tokio::test]
async fn run_loop_stream_synthesis_cancelled_keeps_partial_content() {
    let server = MockModelServer::start(vec![
        MockResponse::Sse(planning_tool_call_sse_events()),
        MockResponse::SseThenHang(vec![
            r#"{"choices":[{"delta":{"content":"部分回答"}}]}"#.to_string()
        ]),
    ]);
    let state = test_app_state();
    let config = test_run_config(&state, &server.base_url);
    let host = TestHost::cancelling_on_first_text_delta();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("cancelled synthesis with partial content must not bubble Err");

    assert_eq!(result.stream_outcome, "cancelled");
    assert_eq!(result.content, "部分回答");
    assert_eq!(result.tool_records.len(), 1);
    assert_eq!(
        result
            .api_messages
            .last()
            .and_then(|message| message.get("content"))
            .and_then(Value::as_str),
        Some("部分回答")
    );
}

/// Bug fix regression (Fix 2): cancellation observed at the loop top of a
/// later round (after an earlier tool round already completed) must end with
/// Ok(cancelled_result) carrying the tool records gathered so far — not a bare
/// Err("cancelled") that drops the whole turn. The host flips the cancel flag
/// from `persist_partial_assistant` (end of round 1), so round 2's loop-top
/// generation check fires while round 1's record is fully preserved.
#[tokio::test]
async fn run_loop_planning_top_cancelled_preserves_gathered_tool_records() {
    let server = MockModelServer::start(vec![
        // Round 1 planning: one read tool call. The tool executes, the round
        // completes, then persist flips cancel → round 2 loop-top cancels.
        MockResponse::Sse(planning_tool_call_sse_events()),
    ]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    // Allow a second round so cancellation lands at the loop top rather than
    // the round limit.
    config.effective_chat_tools.max_tool_rounds = Some(2);
    let host = TestHost::cancelling_on_persist();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("loop-top cancellation must return Ok, not bubble Err");

    assert_eq!(result.stream_outcome, "cancelled");
    assert_eq!(result.content, stopped_generation_content("zh-CN"));
    // Round 1's tool record is preserved (the bug dropped it entirely).
    assert_eq!(result.tool_records.len(), 1);
    assert_eq!(result.tool_records[0].name, "read");
    assert!(matches!(
        result.tool_records[0].status,
        ToolCallStatus::Success
    ));
    // The tool actually executed in round 1 before cancellation.
    assert_eq!(
        executor.events(),
        vec!["start:read".to_string(), "finish:read".to_string()]
    );

    // The turn is persistable: the stopped-generation placeholder is appended
    // as a synthesis api message + segment alongside the preserved records.
    assert!(result.segments.iter().any(|segment| {
        segment.kind == ChatMessageSegmentKind::Text
            && segment.text.as_deref() == Some("已停止生成。")
    }));
}

/// Bug fix regression: a plain-text planning stream (no tool calls started)
/// cancelled after partial text must keep the generated text as an
/// Ok("cancelled") run result instead of bubbling Err and dropping the turn.
#[tokio::test]
async fn run_loop_stream_planning_cancelled_keeps_partial_text() {
    let server = MockModelServer::start(vec![MockResponse::SseThenHang(vec![
        r#"{"choices":[{"delta":{"content":"这是已经生成的部分回答内容"}}]}"#.to_string(),
    ])]);
    let state = test_app_state();
    let config = test_run_config(&state, &server.base_url);
    let host = TestHost::cancelling_on_first_text_delta();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("cancelled plain-text planning with partial content must not bubble Err");

    assert_eq!(result.stream_outcome, "cancelled");
    assert_eq!(result.content, "这是已经生成的部分回答内容");
    assert!(result.tool_records.is_empty());
    assert!(
        executor.events().is_empty(),
        "no tools may execute in a cancelled plain-text turn"
    );

    assert!(result.segments.iter().any(|segment| {
        segment.kind == ChatMessageSegmentKind::Text
            && segment.phase == ChatMessageSegmentPhase::Plain
            && segment.text.as_deref() == Some("这是已经生成的部分回答内容")
    }));
}

/// Regression: a cancelled plain-text reply that streamed reasoning before the
/// answer must persist the reasoning segment ahead of the text segment, so the
/// reloaded timeline keeps "Thinking" above the answer instead of below it.
#[tokio::test]
async fn run_loop_stream_planning_cancelled_orders_reasoning_before_text() {
    let server = MockModelServer::start(vec![MockResponse::SseThenHang(vec![
            r#"{"choices":[{"delta":{"reasoning_content":"先构思一下整体结构","content":"这是已经生成的部分回答内容"}}]}"#.to_string(),
        ])]);
    let state = test_app_state();
    let config = test_run_config(&state, &server.base_url);
    let host = TestHost::cancelling_on_first_text_delta();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("cancelled plain-text planning with reasoning must not bubble Err");

    assert_eq!(result.stream_outcome, "cancelled");

    let reasoning = result
        .segments
        .iter()
        .find(|segment| segment.kind == ChatMessageSegmentKind::Reasoning)
        .expect("a reasoning segment must be persisted");
    let text = result
        .segments
        .iter()
        .find(|segment| segment.kind == ChatMessageSegmentKind::Text)
        .expect("a text segment must be persisted");
    assert!(
            reasoning.order < text.order,
            "reasoning segment (order {}) must precede the text segment (order {}) so Thinking renders above the answer",
            reasoning.order,
            text.order,
        );
}

/// Bug fix regression (Fix 2): a plain-text stream cancelled before any text
/// or tool draft was generated must now end with Ok(cancelled_result) carrying
/// the stopped-generation placeholder — not a bare Err("cancelled") that
/// skipped persistence. With no prior rounds there are no tool records to
/// preserve, but the turn is still persistable.
#[tokio::test]
async fn run_loop_stream_planning_cancelled_with_no_text_returns_cancelled() {
    let server = MockModelServer::start(vec![MockResponse::SseThenHang(Vec::new())]);
    let state = test_app_state();
    let config = test_run_config(&state, &server.base_url);
    let host = TestHost::cancelling_after(Duration::from_millis(20));
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("cancelled stream with zero generated text must return Ok, not Err");

    assert_eq!(result.stream_outcome, "cancelled");
    assert_eq!(result.content, stopped_generation_content("zh-CN"));
    assert!(result.tool_records.is_empty());
}

/// Bug fix regression: when no tools are configured, the plain synthesis path
/// cancelled after partial text must also preserve the generated text.
#[tokio::test]
async fn run_loop_stream_plain_synthesis_cancelled_keeps_partial_text() {
    let server = MockModelServer::start(vec![MockResponse::SseThenHang(vec![
        r#"{"choices":[{"delta":{"content":"部分回答"}}]}"#.to_string(),
    ])]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    config.tools = Vec::new();
    let host = TestHost::cancelling_on_first_text_delta();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("cancelled plain synthesis with partial content must not bubble Err");

    assert_eq!(result.stream_outcome, "cancelled");
    assert_eq!(result.content, "部分回答");
    assert!(result.tool_records.is_empty());

    assert!(result.segments.iter().any(|segment| {
        segment.kind == ChatMessageSegmentKind::Text
            && segment.phase == ChatMessageSegmentPhase::Plain
            && segment.text.as_deref() == Some("部分回答")
    }));
}

/// Tool executor returning a huge text result, to push the loop over a tiny
/// context window and exercise in-loop compaction.
struct HugeResultExecutor {
    chars: usize,
}

impl ToolExecutor for HugeResultExecutor {
    fn call<'a>(
        &'a self,
        _ctx: &'a ToolExecutionContext<'a>,
        _tool: &'a ChatToolDefinition,
        _arguments: Value,
        _skill_cache: Option<&'a mut skills::SkillRunCache>,
    ) -> super::super::execute::ToolExecutorFuture<'a> {
        Box::pin(async move {
            Ok(McpToolCallResult {
                content: "A".repeat(self.chars),
                is_error: false,
                raw: Value::Null,
                artifacts: Vec::new(),
                structured_content: None,
                follow_up_user_messages: Vec::new(),
            })
        })
    }
}

/// In-loop compaction is L2-only (snip removed): an oversized EARLIER-round
/// tool output (outside the keep-recent tail) triggers a summary that replaces
/// the old segment in the send view, while THIS round's own tool result is
/// persisted raw in api_messages. The compacted full history is returned via
/// `compacted_history` so the cross-turn caller can adopt it (R3).
#[tokio::test]
async fn run_loop_l2_compacts_old_history_keeps_current_round_raw() {
    let server = MockModelServer::start(vec![
        // 1) L2 摘要请求（**流式 SSE**）——压缩摘要调用现在走流式路径。
        MockResponse::Sse(vec![
            long_summary_sse("SUMMARY_MARKER: 早前轮次摘要。"),
            "[DONE]".to_string(),
        ]),
        // 2) 压缩后的规划请求 → 发起一次 read 工具调用。
        MockResponse::Sse(planning_tool_call_sse_events()),
        // 3) 合成请求 → 最终回答。
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"总结完成，工具输出已分析。"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
    ]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    // 600 token 窗口（预算 510）：旧工具输出先经 microcompact，再由大段非工具历史确保
    // 发送视图仍超预算，稳定进入 Layer2 摘要。
    config.provider.model_overrides.insert(
        "test-model".to_string(),
        crate::settings::ModelInfo {
            context_window: Some(600),
            ..Default::default()
        },
    );
    // 预填早前轮次历史：大段普通对话不受 tool-result microcompact 影响；超大 tool 输出
    // 仍保留，用来验证 L2 会替换旧段且不动本轮新工具结果。
    let large_non_tool = "D".repeat(9_000);
    config.runtime_messages.push(serde_json::json!({
        "role": "user", "content": large_non_tool
    }));
    let huge = "A".repeat(9_000);
    config.runtime_messages.push(serde_json::json!({
        "role": "assistant", "content": "", "tool_calls": [
            {"id": "old_call", "type": "function", "function": {"name": "read", "arguments": "{}"}}
        ]
    }));
    config.runtime_messages.push(serde_json::json!({
        "role": "tool", "tool_call_id": "old_call", "content": huge
    }));
    for i in 0..8 {
        config.runtime_messages.push(serde_json::json!({
            "role": if i % 2 == 0 { "user" } else { "assistant" },
            "content": format!("history {i}")
        }));
    }
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("compacted run completes");

    assert_eq!(result.stream_outcome, "completed");
    assert_eq!(result.content, "总结完成，工具输出已分析。");

    // 三次请求：L2 摘要 + 规划 + 合成。摘要后的请求不再携带旧原文。
    let bodies = server.captured_bodies();
    assert_eq!(bodies.len(), 3, "summary + planning + synthesis requests");
    assert!(bodies[0].contains("context summarization assistant"));
    for body in &bodies[1..] {
        assert!(
            !body.contains(&"A".repeat(1_000)),
            "post-compaction request must not carry the full old tool output"
        );
        assert!(
            !body.contains(&"D".repeat(1_000)),
            "post-compaction request must not carry the full old conversation"
        );
    }

    // 持久化：本轮 api_messages 不含摘要标记（摘要只作用于发送视图/工作副本）。
    assert!(result
        .api_messages
        .iter()
        .all(|message| !message.to_string().contains("SUMMARY_MARKER")));
    // 本轮工具结果原文留存（压缩只动旧段，不动当前轮）。
    let persisted_tool = result
        .api_messages
        .iter()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("tool"))
        .expect("persisted tool message from this round");
    assert_eq!(persisted_tool["content"], "result:read");

    // R3：压缩发生 → 回传压缩后的完整历史，且末条为本轮最终 assistant 回答。
    let compacted = result
        .compacted_history
        .as_ref()
        .expect("compacted_history present when L2 ran");
    assert!(compacted
        .iter()
        .any(|m| m.to_string().contains("SUMMARY_MARKER")));
    assert!(!compacted
        .iter()
        .any(|m| m.to_string().contains(&"A".repeat(1_000))));
    assert!(!compacted
        .iter()
        .any(|m| m.to_string().contains(&"D".repeat(1_000))));
    let last = compacted.last().expect("non-empty compacted history");
    assert_eq!(last["role"], "assistant");
    assert_eq!(last["content"], "总结完成，工具输出已分析。");
}

/// 真实用量锚点：即便**字符估算**远低于压缩阈值，只要上一轮 provider 实报 usage（config
/// 锚点）超过预算，run 首次规划前就应触发压缩。构造 window=40k（预算 ~34976）、history
/// 字符估算 ~25k（< 预算，纯估算不会压），但 initial_anchor_total_tokens=40k（> 预算）→
/// 首个请求必须是摘要（"context summarization assistant"）。
#[tokio::test]
async fn run_loop_usage_anchor_triggers_compaction_when_estimate_below_budget() {
    let server = MockModelServer::start(vec![
        // 1) 因锚点触发的 L2 摘要请求（流式）。
        MockResponse::Sse(vec![
            long_summary_sse("SUMMARY_MARKER: 早前轮次摘要。"),
            "[DONE]".to_string(),
        ]),
        // 2) 压缩后的规划请求直接给最终答案（无工具）结束循环。
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"已按锚点压缩后作答。"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
    ]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    config.provider.model_overrides.insert(
        "test-model".to_string(),
        crate::settings::ModelInfo {
            context_window: Some(40_000),
            ..Default::default()
        },
    );
    // 5 条各 5000 token（20000 ASCII 字符）→ 估算 ~25000 < 预算 34976（纯估算不触发压缩），
    // 但 keep 20000 只护住尾部 4 条，留 1 条旧段可摘要。
    config.runtime_messages =
        vec![serde_json::json!({ "role": "system", "content": "system prompt" })];
    for i in 0..5 {
        config.runtime_messages.push(serde_json::json!({
            "role": if i % 2 == 0 { "user" } else { "assistant" },
            "content": "a".repeat(20_000),
        }));
    }
    // config 锚点：上一轮 provider 实报 prompt=40000（> 预算 34976）→ 触发压缩。
    config.initial_anchor_total_tokens = Some(40_000);
    config.initial_anchor_trailing_estimate = 0;
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("anchor-triggered compaction run completes");

    assert_eq!(result.content, "已按锚点压缩后作答。");
    let bodies = server.captured_bodies();
    assert!(
        bodies[0].contains("context summarization assistant"),
        "usage anchor over budget must trigger compaction as the first request"
    );
}

/// 对照组：同样的 ~25k 估算 history，但**没有** config 锚点 → 纯字符估算 < 预算 →
/// 首个请求应是普通规划（携带原始 history），而非摘要。证明锚点是触发压缩的必要条件。
#[tokio::test]
async fn run_loop_no_anchor_skips_compaction_when_estimate_below_budget() {
    let server = MockModelServer::start(vec![
        // 无压缩：首个请求即规划，直接给最终答案。
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"无需压缩直接作答。"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
    ]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    config.provider.model_overrides.insert(
        "test-model".to_string(),
        crate::settings::ModelInfo {
            context_window: Some(40_000),
            ..Default::default()
        },
    );
    config.runtime_messages =
        vec![serde_json::json!({ "role": "system", "content": "system prompt" })];
    for i in 0..5 {
        config.runtime_messages.push(serde_json::json!({
            "role": if i % 2 == 0 { "user" } else { "assistant" },
            "content": "a".repeat(20_000),
        }));
    }
    // 无锚点（默认 None）。
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("no-anchor run completes without compaction");

    assert_eq!(result.content, "无需压缩直接作答。");
    let bodies = server.captured_bodies();
    assert!(
        !bodies[0].contains("context summarization assistant"),
        "without an anchor, an under-budget estimate must NOT trigger compaction"
    );
}

/// **内置路径的实时用量**：上下文占用要在生成过程中就推给前端，而不是等一轮完全结束。
///
/// 内置路径唯一的等价实时来源是 `maybe_compact_send_view` —— 它每个 planning 轮都跑一次，
/// 且已经按权威口径算出了分子（`effective_context_tokens`）与分母
/// （`context_window_for_model`），所以实时通道零额外计算。这条断言两件事：
/// 一轮里**多次**上报（多次工具往返 ⇒ 多轮 ⇒ 多次上报），且数字单调不减。
#[tokio::test]
async fn run_loop_reports_live_context_usage_each_round() {
    let server = MockModelServer::start(vec![
        // 第 1 轮：调一次工具。
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"a.txt\"}"}}]}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
        // 第 2 轮：给最终答案收尾。
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"读完了。"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
    ]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    config.provider.model_overrides.insert(
        "test-model".to_string(),
        crate::settings::ModelInfo {
            context_window: Some(200_000),
            ..Default::default()
        },
    );
    config.tools = vec![native_read_file_tool()];
    config.runtime_messages = vec![
        serde_json::json!({ "role": "system", "content": "system prompt" }),
        serde_json::json!({ "role": "user", "content": "a".repeat(4_000) }),
    ];
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("run completes");
    assert_eq!(result.content, "读完了。");

    let ticks = host.recorded_context_ticks();
    assert!(
        ticks.len() >= 2,
        "多轮的 run 必须在过程中多次上报占用（修复前一次都没有）：{ticks:?}"
    );
    assert!(
        ticks
            .iter()
            .all(|(used, window)| *used > 0 && *window == Some(200_000)),
        "分子必须非零、分母必须是模型窗口：{ticks:?}"
    );
    // 单调不减：工具结果进入历史后占用只会涨（压缩才会降，本例不触发）。
    assert!(
        ticks.windows(2).all(|pair| pair[1].0 >= pair[0].0),
        "过程中的占用不该往回跳：{ticks:?}"
    );
}

/// Crash-safety: after a tool round that returns `Continue` (more rounds
/// allowed), the loop must checkpoint a partial-assistant snapshot carrying
/// the round's tool work, so a mid-run crash before the final write keeps the
/// turn recoverable instead of discarding it.
#[tokio::test]
async fn run_loop_persists_partial_assistant_after_completed_tool_round() {
    let server = MockModelServer::start(vec![
        // Round 1 planning: one read tool call. With max_tool_rounds=2
        // this round returns Continue, so the checkpoint fires.
        MockResponse::Sse(planning_tool_call_sse_events()),
        // Round 2 planning: a natural final answer (no tools) ends the loop.
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"完成。"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
    ]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    config.effective_chat_tools.max_tool_rounds = Some(2);
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("multi-round run completes");

    // The checkpoint fired after round 1's completed tool round, snapshotting
    // the work done so far under the run's message id.
    let persists = host.recorded_persists();
    let snapshot = persists
        .iter()
        .find(|(_, tool_records_len, _, _)| *tool_records_len >= 1)
        .expect("a checkpoint snapshot includes the completed round's tool record");
    assert_eq!(snapshot.0, "message", "snapshot keyed by run message id");
    assert!(
        snapshot.1 >= 1,
        "snapshot carries the round's tool record(s)"
    );
    // The draft also carries the loop's accumulated provider messages
    // (assistant tool_call + tool result) so an `interrupted` draft stays
    // replayable on a later "continue" instead of losing all tool context.
    assert!(
        snapshot.3 >= 1,
        "snapshot carries accumulated api_messages for replay, not an empty Vec"
    );

    // The run still completed normally end-to-end.
    assert_eq!(result.stream_outcome, "completed");
    assert!(result.tool_records.iter().any(|r| r.id == "call_read"));
}

/// 运行中「立刻引导」：轮首注入的用户插话必须①进下一次模型请求，②在时间线上留一张
/// `user_steer` 卡（否则用户说过的话在历史里查无此人），③随 api_messages 落盘让下一轮回放不丢。
#[tokio::test]
async fn run_loop_steering_injects_user_message_and_emits_card() {
    let server = MockModelServer::start(vec![
        // Round 1：一次 read 工具调用（跑完 → Continue，进第二轮）。
        MockResponse::Sse(planning_tool_call_sse_events()),
        // Round 2：终答。这一次的请求体里应当已经带上插话。
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"改用 rg 重跑了。"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
    ]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    config.effective_chat_tools.max_tool_rounds = Some(3);
    let host = TestHost::with_steering("steer-1", "别用 grep，改用 rg");
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("run with steering completes");

    // ① 第二轮的请求带上了插话（第一轮请求发出时信箱还没被取，所以只可能在第 2 次之后）。
    let bodies = server.captured_bodies();
    assert!(bodies.len() >= 2, "expected at least two provider requests");
    assert!(
        bodies[1].contains("改用 rg"),
        "the steered user message must reach the next model call: {}",
        bodies[1]
    );

    // ② 卡片：实时发过一条，且留在 run 结果里（→ 落盘到 assistant 消息的 tool_calls）。
    let steer_card = |records: &[ToolCallRecord]| {
        records.iter().any(|record| {
            record.name == crate::chat::agent::STEER_TOOL_NAME
                && record.source == "native"
                && record
                    .structured_content
                    .as_ref()
                    .and_then(|value| value.get("steer_id"))
                    .and_then(|value| value.as_str())
                    == Some("steer-1")
        })
    };
    assert!(
        steer_card(&host.recorded_tool_records()),
        "a user_steer card is emitted live"
    );
    assert!(
        steer_card(&result.tool_records),
        "the user_steer card is part of the run result (persisted with the message)"
    );

    // 卡片有对应的 Tool 段，否则时间线上没有它的位置（只会挂到 orphan 兜底区）。
    let card_id = result
        .tool_records
        .iter()
        .find(|record| record.name == crate::chat::agent::STEER_TOOL_NAME)
        .map(|record| record.id.clone())
        .expect("steer record present");
    assert!(
        result
            .segments
            .iter()
            .any(|segment| segment.tool_call_id.as_deref() == Some(card_id.as_str())),
        "the steer card owns a timeline segment"
    );

    // ③ 落盘的 api_messages 里有这条 user 消息，下一轮回放才不丢。
    assert!(
        result.api_messages.iter().any(|message| {
            message.get("role").and_then(|role| role.as_str()) == Some("user")
                && message
                    .get("content")
                    .and_then(|content| content.as_str())
                    .map(|content| content.contains("改用 rg"))
                    .unwrap_or(false)
        }),
        "the steered message is stored for replay: {:?}",
        result.api_messages
    );

    assert_eq!(result.stream_outcome, "completed");
}

/// one-at-a-time（对齐 pi PendingMessageQueue 默认模式）：同一批到达的两条插话不一次
/// 灌进同一轮——每个轮次边界只注入一条，剩余的下一个边界送达。
#[tokio::test]
async fn run_loop_steering_delivers_one_message_per_round() {
    let server = MockModelServer::start(vec![
        // Round 1：工具调用（此前注入了第 1 条插话）。
        MockResponse::Sse(planning_tool_call_sse_events()),
        // Round 2：终答（此前注入了第 2 条插话）。
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"两条都照办。"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
    ]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    config.effective_chat_tools.max_tool_rounds = Some(3);
    let host = TestHost::with_steering_batches(vec![vec![
        SteeringMessage::new("steer-a".into(), "第一条：先跑测试").expect("non-blank"),
        SteeringMessage::new("steer-b".into(), "第二条：再改文档").expect("non-blank"),
    ]]);
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("run with queued steering completes");

    let bodies = server.captured_bodies();
    assert_eq!(bodies.len(), 2);
    // 第一轮只带第一条；第二条要等下一个轮次边界。
    assert!(bodies[0].contains("先跑测试"), "round 1 carries message #1");
    assert!(
        !bodies[0].contains("再改文档"),
        "message #2 must NOT be flushed into the same round: {}",
        bodies[0]
    );
    assert!(bodies[1].contains("再改文档"), "round 2 carries message #2");
    // 两条各有自己的卡。
    let steer_ids: Vec<_> = result
        .tool_records
        .iter()
        .filter(|record| record.name == crate::chat::agent::STEER_TOOL_NAME)
        .filter_map(|record| {
            record
                .structured_content
                .as_ref()
                .and_then(|value| value.get("steer_id"))
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .collect();
    assert_eq!(steer_ids, vec!["steer-a", "steer-b"]);
    assert_eq!(result.stream_outcome, "completed");
}

/// pi 外层 while 的 followUp 语义：模型给出终答时信箱里还有插话 ⇒ 终答落成中间
/// assistant 消息、run 继续跑下一轮送达插话，而不是收束后靠前端把话重发成新消息。
#[tokio::test]
async fn run_loop_final_answer_with_pending_steering_continues() {
    let server = MockModelServer::start(vec![
        // Round 1：模型直接给终答（无工具调用）。
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"先答一版。"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
        // Round 2：带着插话续答。
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"补充完毕。"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
    ]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    config.effective_chat_tools.max_tool_rounds = Some(3);
    // 时序：轮首取信箱时是空的（批 1），插话在终答流式期间到达（批 2 在
    // FinalAnswer 边界的 steering_pending 检查被取到）。
    let host = TestHost::with_steering_batches(vec![
        Vec::new(),
        vec![SteeringMessage::new("steer-late".into(), "顺便把 README 也更新").expect("non-blank")],
    ]);
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("run continues past the pending-steering final answer");

    let bodies = server.captured_bodies();
    assert_eq!(bodies.len(), 2, "final answer + one continuation round");
    // 续答请求带着：被吸收成中间消息的第一版终答 + 插话。
    assert!(
        bodies[1].contains("先答一版"),
        "absorbed assistant answer replays"
    );
    assert!(
        bodies[1].contains("README"),
        "the late steering message reaches the model"
    );
    // run 的最终正文是第二版；第一版存在于段与 api_messages 里（时间线完整）。
    assert_eq!(result.content, "补充完毕。");
    assert!(result.api_messages.iter().any(|message| {
        message.get("role").and_then(|role| role.as_str()) == Some("assistant")
            && message
                .get("content")
                .and_then(|content| content.as_str())
                .map(|content| content.contains("先答一版"))
                .unwrap_or(false)
    }));
    assert!(result.api_messages.iter().any(|message| {
        message.get("role").and_then(|role| role.as_str()) == Some("user")
            && message
                .get("content")
                .and_then(|content| content.as_str())
                .map(|content| content.contains("README"))
                .unwrap_or(false)
    }));
    // 插话卡照常出。
    assert!(result
        .tool_records
        .iter()
        .any(|record| record.name == crate::chat::agent::STEER_TOOL_NAME));
    assert_eq!(result.stream_outcome, "completed");
}

/// 内置循环的原生 follow-up：终答时信箱有下一轮消息 ⇒ 吸收终答、注入 `user_follow_up`
/// 卡、同一 run 续跑。不是 `user_steer`（那会在轮首打断工具循环）。
#[tokio::test]
async fn run_loop_final_answer_with_pending_follow_up_continues() {
    let server = MockModelServer::start(vec![
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"先答一版。"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"补充完毕。"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
    ]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    config.effective_chat_tools.max_tool_rounds = Some(3);
    let host = TestHost::with_follow_up_batches(vec![vec![SteeringMessage::new(
        "follow-late".into(),
        "顺便把 README 也更新",
    )
    .expect("non-blank")]]);
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("run continues past the pending follow-up final answer");

    let bodies = server.captured_bodies();
    assert_eq!(bodies.len(), 2, "final answer + one continuation round");
    assert!(
        bodies[1].contains("先答一版"),
        "absorbed assistant answer replays"
    );
    assert!(
        bodies[1].contains("README"),
        "the follow-up message reaches the model"
    );
    assert_eq!(result.content, "补充完毕。");
    assert!(result
        .tool_records
        .iter()
        .any(|record| record.name == crate::chat::agent::FOLLOW_UP_TOOL_NAME));
    assert!(result
        .tool_records
        .iter()
        .all(|record| record.name != crate::chat::agent::STEER_TOOL_NAME));
    assert_eq!(result.stream_outcome, "completed");
}

/// follow-up 不能在工具循环的轮首注入：工具轮的请求里没有它，终答之后才进下一轮。
#[tokio::test]
async fn run_loop_follow_up_waits_until_final_answer() {
    let server = MockModelServer::start(vec![
        MockResponse::Sse(planning_tool_call_sse_events()),
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"文件看过了。"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"README 也改了。"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
    ]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    config.effective_chat_tools.max_tool_rounds = Some(4);
    let host = TestHost::with_follow_up_batches(vec![vec![SteeringMessage::new(
        "follow-after-tools".into(),
        "再改 README",
    )
    .expect("non-blank")]]);
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("follow-up after tools completes");

    let bodies = server.captured_bodies();
    assert_eq!(bodies.len(), 3, "tool round + final answer + follow-up round");
    assert!(
        !bodies[0].contains("README"),
        "follow-up must not enter the tool-call round: {}",
        bodies[0]
    );
    assert!(
        !bodies[1].contains("README"),
        "follow-up must not enter the pre-final tool-result round: {}",
        bodies[1]
    );
    assert!(
        bodies[2].contains("README"),
        "follow-up reaches the continuation round"
    );
    assert!(result
        .tool_records
        .iter()
        .any(|record| record.name == crate::chat::agent::FOLLOW_UP_TOOL_NAME));
    assert_eq!(result.stream_outcome, "completed");
}

/// 对齐 pi failToolCallsFromTruncatedMessage：finish_reason=="length" 时整批工具调用作废
/// ——salvage 收尾可能产出「JSON 合法但语义残缺」的参数，一个都不执行，让模型重发。
#[tokio::test]
async fn run_loop_length_truncated_tool_calls_fail_whole_batch() {
    let server = MockModelServer::start(vec![
        // Round 1：两个 read 调用（参数 JSON 完整可解析），但 finish_reason=length。
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"read","arguments":"{\"path\":\"/tmp/a.txt\"}"}},{"index":1,"id":"call_b","function":{"name":"read","arguments":"{\"path\":\"/tmp/b.txt\"}"}}]}}]}"#
                .to_string(),
            r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
        // Round 2：模型收到整批错误后正常收尾。
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"重发完成。"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
    ]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    config.effective_chat_tools.max_tool_rounds = Some(3);
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("run recovers after the truncated batch");

    // 一个工具都没真正执行（参数可解析也不行——可能是被截断的半成品）。
    assert!(
        executor.events().is_empty(),
        "no tool may execute from a length-truncated message: {:?}",
        executor.events()
    );
    // 两个调用都留了错误记录，注明截断原因。
    let truncated_records: Vec<_> = result
        .tool_records
        .iter()
        .filter(|record| {
            matches!(record.status, ToolCallStatus::Error)
                && record
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("finish_reason=length"))
        })
        .collect();
    assert_eq!(truncated_records.len(), 2, "{:?}", result.tool_records);
    // 两个 tool_call_id 在下一轮请求里都有应答（缺一条严格 provider 会 400）。
    let bodies = server.captured_bodies();
    assert!(bodies.len() >= 2);
    assert!(bodies[1].contains("call_a") && bodies[1].contains("call_b"));
    assert!(bodies[1].contains("may be truncated"));
    assert_eq!(result.content, "重发完成。");
    assert_eq!(result.stream_outcome, "completed");
}

/// Layer2 compaction: when the history is over budget, a summary request fires
/// first and the next provider request carries the summary instead of the old
/// history; the summary itself stays out of persisted api_messages, and the
/// compacted full history is returned via `compacted_history` (R3).
#[tokio::test]
async fn run_loop_layer2_replaces_old_history_with_summary() {
    let server = MockModelServer::start(vec![
        // 1) Layer2 摘要请求（**流式 SSE**）——压缩摘要调用现在走流式路径。
        MockResponse::Sse(vec![
            long_summary_sse("SUMMARY_MARKER: 早前轮次已读取大文件。"),
            "[DONE]".to_string(),
        ]),
        // 2) 压缩后的规划请求 → 直接给出最终回答（无工具调用）。
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"基于摘要继续完成任务。"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
    ]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    // 600 token 窗口（预算 510）：即使旧工具结果先被 microcompact，大段非工具历史仍
    // 保持超预算，必然升级 Layer2。
    config.provider.model_overrides.insert(
        "test-model".to_string(),
        crate::settings::ModelInfo {
            context_window: Some(600),
            ..Default::default()
        },
    );
    let large_non_tool = "E".repeat(9_000);
    config.runtime_messages.push(serde_json::json!({
        "role": "user", "content": large_non_tool
    }));
    let huge = "B".repeat(9_000);
    config.runtime_messages.push(serde_json::json!({
        "role": "tool", "tool_call_id": "old_call", "content": huge
    }));
    for i in 0..8 {
        config.runtime_messages.push(serde_json::json!({
            "role": if i % 2 == 0 { "user" } else { "assistant" },
            "content": format!("recent history {i}")
        }));
    }
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("layer2-compacted run completes");
    assert_eq!(result.stream_outcome, "completed");
    assert_eq!(result.content, "基于摘要继续完成任务。");

    let bodies = server.captured_bodies();
    assert_eq!(bodies.len(), 2, "summary request + planning request");
    // 摘要请求带着 Claude Code 结构化 prompt 与旧段样本。
    assert!(bodies[0].contains("context summarization assistant"));
    // L2 摘要调用现在走**流式**路径——摘要请求体携带 `"stream":true`（这是它能在仅支持
    // 流式的 provider 上成功摘要的关键：mock 摘要响应以 SSE 提供而非非流式 JSON）。
    assert!(
        bodies[0].contains("\"stream\":true"),
        "compaction summary call must use the streaming path"
    );
    // 压缩后的规划请求：携带摘要，不再携带旧原文，保留最近历史。
    assert!(bodies[1].contains("SUMMARY_MARKER"));
    assert!(!bodies[1].contains(&"B".repeat(1_000)));
    assert!(!bodies[1].contains(&"E".repeat(1_000)));
    assert!(bodies[1].contains("recent history 7"));
    // 摘要只存在于发送视图/工作副本，不进持久化 api_messages。
    assert!(result
        .api_messages
        .iter()
        .all(|message| !message.to_string().contains("SUMMARY_MARKER")));
    // R3：压缩发生 → compacted_history 携带摘要、剔除旧原文。
    let compacted = result
        .compacted_history
        .as_ref()
        .expect("compacted_history present when L2 ran");
    assert!(compacted
        .iter()
        .any(|m| m.to_string().contains("SUMMARY_MARKER")));
    assert!(!compacted
        .iter()
        .any(|m| m.to_string().contains(&"B".repeat(1_000))));
    assert!(!compacted
        .iter()
        .any(|m| m.to_string().contains(&"E".repeat(1_000))));
}

/// Streaming-only provider: the compaction summary call MUST stream. This is the
/// "real path" L2 test the prior mock tests lacked — the user's provider (xb1520.com
/// `gpt-5.3-codex-spark`, openai_responses) only reliably serves STREAMING, so the
/// lone non-stream summary call failed and compaction could never compress on it.
///
/// We model a streaming-only provider by serving the summary as an SSE stream while
/// FAILING any non-stream call shape: the summary mock is the FIRST request; if the
/// agent issued a non-stream `generate` for it, the captured body would lack
/// `"stream":true`. We assert the summary request streamed AND that compaction
/// SUCCEEDED (old history replaced by the summary, `compacted_history` produced).
#[tokio::test]
async fn run_loop_compaction_summary_streams_on_streaming_only_provider() {
    let server = MockModelServer::start(vec![
        // 1) 摘要请求：仅以 SSE 提供（模拟仅支持流式的 provider）。
        MockResponse::Sse(vec![
            long_summary_sse_tagged("SUMMARY_MARKER: streamed summary. "),
            "[DONE]".to_string(),
        ]),
        // 2) 压缩后的规划请求 → 直接给出最终回答（无工具调用）。
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"流式摘要后继续完成任务。"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
    ]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    config.provider.model_overrides.insert(
        "test-model".to_string(),
        crate::settings::ModelInfo {
            context_window: Some(600),
            ..Default::default()
        },
    );
    let large_non_tool = "F".repeat(9_000);
    config.runtime_messages.push(serde_json::json!({
        "role": "user", "content": large_non_tool
    }));
    let huge = "C".repeat(9_000);
    config.runtime_messages.push(serde_json::json!({
        "role": "tool", "tool_call_id": "old_call", "content": huge
    }));
    for i in 0..8 {
        config.runtime_messages.push(serde_json::json!({
            "role": if i % 2 == 0 { "user" } else { "assistant" },
            "content": format!("recent history {i}")
        }));
    }
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("streaming-only compacted run completes");
    assert_eq!(result.stream_outcome, "completed");
    assert_eq!(result.content, "流式摘要后继续完成任务。");

    let bodies = server.captured_bodies();
    assert_eq!(
        bodies.len(),
        2,
        "streamed summary request + planning request"
    );
    // 关键：摘要请求走流式（仅流式 provider 上能成功的根本原因）。
    assert!(
        bodies[0].contains("context summarization assistant"),
        "first request is the summary call"
    );
    assert!(
        bodies[0].contains("\"stream\":true"),
        "summary call must stream on a streaming-only provider"
    );
    // 压缩成功：旧原文被摘要取代，compacted_history 产出。
    assert!(bodies[1].contains("SUMMARY_MARKER"));
    assert!(!bodies[1].contains(&"C".repeat(1_000)));
    assert!(!bodies[1].contains(&"F".repeat(1_000)));
    let compacted = result
        .compacted_history
        .as_ref()
        .expect("compacted_history present when streamed L2 compaction succeeds");
    assert!(compacted
        .iter()
        .any(|m| m.to_string().contains("SUMMARY_MARKER")));
    assert!(!compacted
        .iter()
        .any(|m| m.to_string().contains(&"C".repeat(1_000))));
    assert!(!compacted
        .iter()
        .any(|m| m.to_string().contains(&"F".repeat(1_000))));
}

/// Gap 2 (Layer 3 anti-thrashing): when the history is persistently over the
/// compaction budget and every summary call FAILS, the loop must NOT keep
/// re-summarizing-and-failing forever (the real-world "6× failed compaction"
/// regression). After `COMPACTION_THRASH_LIMIT` (2) unresolved rounds it ends
/// the turn gracefully with the gathered tool results — a degraded answer, not
/// an `Err`, and a BOUNDED number of model calls.
#[tokio::test]
async fn run_loop_compaction_thrash_degrades_with_gathered_results() {
    let overflow_400 = || {
        MockResponse::Status(
            400,
            r#"{"error":{"message":"This model's maximum context length is 600 tokens"}}"#
                .to_string(),
        )
    };
    let server = MockModelServer::start(vec![
        // Round 1 entry: compaction summary call #1 → fails (unresolved 0→1).
        overflow_400(),
        // Round 1 planning → one read tool call (gathers a result).
        MockResponse::Sse(planning_tool_call_sse_events()),
        // Round 2 entry: compaction summary call #2 → fails (unresolved 1→2);
        // the thrash limit is now reached, so the loop degrades BEFORE any
        // further planning call. No more responses are consumed after this.
        overflow_400(),
    ]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    // Small window so the pre-filled ordinary history remains over budget even
    // after the huge tool output is microcompacted, forcing a summary attempt at
    // the top of every planning round.
    config.provider.model_overrides.insert(
        "test-model".to_string(),
        crate::settings::ModelInfo {
            context_window: Some(600),
            ..Default::default()
        },
    );
    // Allow a second round so the loop re-enters planning after the tool round
    // and trips the thrash guard there.
    config.effective_chat_tools.max_tool_rounds = Some(2);
    // Pre-fill oversized ordinary history plus an earlier tool output and a small
    // recent tail. The summary call always errors, so the send view never shrinks
    // enough and unresolved_rounds climbs to the limit.
    let large_non_tool = "G".repeat(9_000);
    config.runtime_messages.push(serde_json::json!({
        "role": "user", "content": large_non_tool
    }));
    let huge = "A".repeat(9_000);
    config.runtime_messages.push(serde_json::json!({
        "role": "assistant", "content": "", "tool_calls": [
            {"id": "old_call", "type": "function", "function": {"name": "read", "arguments": "{}"}}
        ]
    }));
    config.runtime_messages.push(serde_json::json!({
        "role": "tool", "tool_call_id": "old_call", "content": huge
    }));
    for i in 0..8 {
        config.runtime_messages.push(serde_json::json!({
            "role": if i % 2 == 0 { "user" } else { "assistant" },
            "content": format!("history {i}")
        }));
    }
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("anti-thrashing must end the turn, not bubble Err");

    // Degraded but completed via the recovery path — not an error, not a loop.
    assert_eq!(result.stream_outcome, "compaction_thrash");
    // The gathered round-1 tool result is surfaced in the degraded answer.
    assert_eq!(result.tool_records.len(), 1);
    assert!(matches!(
        result.tool_records[0].status,
        ToolCallStatus::Success
    ));
    assert!(
        result.content.contains("result:read"),
        "degraded answer must carry the gathered tool result, got: {}",
        result.content
    );
    // BOUNDED model calls: summary#1 + planning#1 + summary#2 = exactly 3.
    // The thrash guard fires before a 2nd planning call, so we never see the
    // 6× failed-compaction loop from the regression.
    assert_eq!(
        server.captured_bodies().len(),
        3,
        "anti-thrashing must bound model calls (summary + planning + summary), no repeat-fail loop"
    );
}

/// Under-budget runs must not be touched by compaction: the request body
/// carries the tool output verbatim.
#[tokio::test]
async fn run_loop_under_budget_sends_messages_untouched() {
    let server = MockModelServer::start(vec![
        MockResponse::Sse(planning_tool_call_sse_events()),
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"done"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
    ]);
    let state = test_app_state();
    let config = test_run_config(&state, &server.base_url);
    let host = TestHost::default();
    // 默认窗口（test-model 无覆盖 → 200k fallback），小输出远不达 0.85。
    let executor = HugeResultExecutor { chars: 600 };

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("run completes");
    assert_eq!(result.stream_outcome, "completed");

    let bodies = server.captured_bodies();
    assert_eq!(bodies.len(), 2);
    assert!(
        !bodies[1].contains("chars snipped"),
        "under-budget send view must be untouched"
    );
    assert!(bodies[1].contains(&"A".repeat(600)));
}

/// Fallback D: streamed synthesis returns an empty answer after tool results;
/// the loop substitutes the bilingual fallback and completes normally
/// (stream_outcome "completed", not "error").
#[tokio::test]
async fn run_loop_stream_synthesis_empty_output_uses_fallback_and_completes() {
    let server = MockModelServer::start(vec![
        MockResponse::Sse(planning_tool_call_sse_events()),
        MockResponse::Sse(vec!["[DONE]".to_string()]),
    ]);
    let state = test_app_state();
    let config = test_run_config(&state, &server.base_url);
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("empty synthesis with tool records must not bubble Err");

    let recovered = crate::chat::agent::recovery::assemble_results_from_tool_records(
        &result.tool_records,
        "zh-CN",
        crate::chat::agent::recovery::FailureKind::Empty,
    );
    assert!(recovered.contains("result:read"));
    assert_eq!(result.stream_outcome, "completed");
    assert_eq!(result.content, recovered);
    assert_eq!(result.tool_records.len(), 1);
    assert!(matches!(
        result.tool_records[0].status,
        ToolCallStatus::Success
    ));

    assert!(result.segments.iter().any(|segment| {
        segment.kind == ChatMessageSegmentKind::Text
            && segment.phase == ChatMessageSegmentPhase::Synthesis
            && segment.text.as_deref() == Some(recovered.as_str())
    }));
    assert_eq!(
        result
            .api_messages
            .last()
            .and_then(|message| message.get("content"))
            .and_then(Value::as_str),
        Some(recovered.as_str())
    );
}

/// Fallback E: non-streamed synthesis request fails (HTTP 400) after a
/// successful tool round; tool records survive with the failure fallback.
#[tokio::test]
async fn run_loop_nonstream_synthesis_failure_preserves_tool_records_with_fallback() {
    let server = MockModelServer::start(vec![
        MockResponse::Sse(planning_tool_call_sse_events()),
        MockResponse::Status(400, r#"{"error":"mock synthesis failure"}"#.to_string()),
    ]);
    let state = test_app_state();
    let config = test_run_config(&state, &server.base_url);
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("non-stream synthesis failure after tool records must not bubble Err");

    let recovered = crate::chat::agent::recovery::assemble_results_from_tool_records(
        &result.tool_records,
        "zh-CN",
        crate::chat::agent::recovery::FailureKind::Other,
    );
    assert!(recovered.contains("result:read"));
    assert_eq!(result.stream_outcome, "recovered");
    assert_eq!(result.content, recovered);
    assert_eq!(result.tool_records.len(), 1);
    assert!(matches!(
        result.tool_records[0].status,
        ToolCallStatus::Success
    ));
    assert!(host
        .recorded_deltas()
        .iter()
        .any(|delta| delta.delta == recovered));
    assert_eq!(
        result
            .api_messages
            .last()
            .and_then(|message| message.get("content"))
            .and_then(Value::as_str),
        Some(recovered.as_str())
    );
}

/// Overflow recovery (success): non-stream synthesis fails with a 400 that
/// classifies as ContextOverflow; recovery compacts once and re-sends the
/// synthesis call, which succeeds. The retried answer (not the gathered
/// fallback) is used, and the loop completes via the "recovered" outcome.
#[tokio::test]
async fn run_loop_overflow_recovery_compacts_and_retries_success() {
    let retry_answer = serde_json::json!({
        "choices": [{
            "finish_reason": "stop",
            "message": {
                "role": "assistant",
                "content": "summary after compaction"
            }
        }]
    })
    .to_string();
    let server = MockModelServer::start(vec![
        MockResponse::Sse(planning_tool_call_sse_events()),
        // Synthesis call #1 fails with an overflow-shaped 400.
        MockResponse::Status(
            400,
            r#"{"error":{"message":"This model's maximum context length is 8192 tokens"}}"#
                .to_string(),
        ),
        // CompactAndRetry re-sends synthesis; this one succeeds.
        MockResponse::Sse(sse_from_completion_json(&retry_answer)),
    ]);
    let state = test_app_state();
    let config = test_run_config(&state, &server.base_url);
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("overflow recovery must not bubble Err");

    // The retried summary is used — NOT the gathered fallback.
    assert_eq!(result.content, "summary after compaction");
    assert_eq!(result.stream_outcome, "recovered");
    assert_eq!(result.tool_records.len(), 1);
    assert!(matches!(
        result.tool_records[0].status,
        ToolCallStatus::Success
    ));
    // The model was called exactly 3 times: planning, failed synthesis, retry.
    assert_eq!(server.captured_bodies().len(), 3);
}

/// 静默超窗（pi `isContextOverflow` case 2/3）：synthesis 调用 **HTTP 200 成功**，
/// 但正文为空、且 provider 实报的 prompt token 已经顶到窗口。没有任何错误文案可以
/// 匹配，`classify("")` 只会给 `Empty` → 直接拿工具结果降级；用量证据必须把它改判成
/// `ContextOverflow`，走「压缩一次再重发」。断言最终用的是重发的答案，不是兜底摘要。
#[tokio::test]
async fn run_loop_empty_response_at_context_window_recovers_as_silent_overflow() {
    // 200 + 空正文 + prompt 顶满 40k 窗口 = 被静默吞掉的超窗请求。
    let empty_at_window = serde_json::json!({
        "choices": [{
            "finish_reason": "stop",
            "message": { "role": "assistant", "content": "" }
        }],
        "usage": { "prompt_tokens": 40_000, "completion_tokens": 0, "total_tokens": 40_000 }
    })
    .to_string();
    let retry_answer = serde_json::json!({
        "choices": [{
            "finish_reason": "stop",
            "message": { "role": "assistant", "content": "压缩后重发拿到的答案" }
        }]
    })
    .to_string();
    let server = MockModelServer::start(vec![
        MockResponse::Sse(planning_tool_call_sse_events()),
        MockResponse::Sse(sse_from_completion_json(&empty_at_window)),
        // 改判成 overflow 后先压缩：摘要调用恒为流式，与 run 的 stream 设置无关。
        MockResponse::Sse(vec![
            long_summary_sse("SUMMARY_MARKER: 早前轮次摘要。"),
            "[DONE]".to_string(),
        ]),
        MockResponse::Sse(sse_from_completion_json(&retry_answer)),
    ]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    config.provider.model_overrides.insert(
        "test-model".to_string(),
        crate::settings::ModelInfo {
            context_window: Some(40_000),
            ..Default::default()
        },
    );
    // 25k token 的纯文本历史：规划/合成前的估算（<36k 预算）不触发压缩，所以前两次
    // 调用是干净的；但它超过 20k 的保护尾窗，改判后压缩有旧段可摘要（且旧段里没有
    // 工具结果 → microcompact 无从降级 → 必定走 LLM 摘要，调用次数确定）。
    config.runtime_messages =
        vec![serde_json::json!({ "role": "system", "content": "system prompt" })];
    for i in 0..5 {
        config.runtime_messages.push(serde_json::json!({
            "role": if i % 2 == 0 { "user" } else { "assistant" },
            "content": "a".repeat(20_000),
        }));
    }
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("silent overflow recovery must not bubble Err");

    assert_eq!(
        result.content, "压缩后重发拿到的答案",
        "空响应 + 用量顶窗必须走压缩重发，而不是当成 Empty 直接降级"
    );
    // 「成功但空正文」这条路走的是 synthesis 的正常收尾（既有行为），outcome 仍是
    // completed；只有**报错**的那条路才标 recovered。恢复是否发生看 content。
    assert_eq!(result.stream_outcome, "completed");
    assert!(
        result.degraded.is_none(),
        "重发成功就没有降级卡片，got: {:?}",
        result.degraded
    );
    let bodies = server.captured_bodies();
    assert_eq!(bodies.len(), 4, "规划 / 空响应合成 / 摘要 / 重发");
    assert!(
        bodies[2].contains("context summarization assistant"),
        "第 3 次调用必须是压缩摘要"
    );
}

/// Overflow recovery (single-attempt guard): both the synthesis call and the
/// compact-and-retry call fail with overflow 400. Recovery must NOT loop —
/// it degrades to the gathered-results fallback after exactly one retry.
#[tokio::test]
async fn run_loop_overflow_recovery_retries_once_then_degrades() {
    let overflow_400 = MockResponse::Status(
        400,
        r#"{"error":{"message":"prompt is too long: exceeds the maximum context length"}}"#
            .to_string(),
    );
    let server = MockModelServer::start(vec![
        MockResponse::Sse(planning_tool_call_sse_events()),
        // Synthesis call #1: overflow.
        MockResponse::Status(
            400,
            r#"{"error":{"message":"prompt is too long: exceeds the maximum context length"}}"#
                .to_string(),
        ),
        // CompactAndRetry re-send: still overflow → degrade, no further retry.
        overflow_400,
    ]);
    let state = test_app_state();
    let config = test_run_config(&state, &server.base_url);
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("overflow recovery exhaustion must not bubble Err");

    let recovered = crate::chat::agent::recovery::assemble_results_from_tool_records(
        &result.tool_records,
        "zh-CN",
        crate::chat::agent::recovery::FailureKind::ContextOverflow,
    );
    assert!(recovered.contains("result:read"));
    assert_eq!(result.content, recovered);
    assert_eq!(result.stream_outcome, "recovered");
    // Single-attempt guard: planning + failed synthesis + ONE retry = 3 calls,
    // not an unbounded compact→retry loop.
    assert_eq!(server.captured_bodies().len(), 3);
}

/// Fallback F: non-streamed synthesis returns empty content after tool results;
/// the bilingual fallback replaces it and reasoning is still passed through.
#[tokio::test]
async fn run_loop_nonstream_synthesis_empty_output_uses_fallback() {
    let empty_synthesis = serde_json::json!({
        "choices": [{
            "finish_reason": "stop",
            "message": {
                "role": "assistant",
                "content": "",
                "reasoning_content": "synthesis reasoning"
            }
        }]
    })
    .to_string();
    let server = MockModelServer::start(vec![
        MockResponse::Sse(planning_tool_call_sse_events()),
        MockResponse::Sse(sse_from_completion_json(&empty_synthesis)),
    ]);
    let state = test_app_state();
    let config = test_run_config(&state, &server.base_url);
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("non-stream empty synthesis with tool records must not bubble Err");

    let recovered = crate::chat::agent::recovery::assemble_results_from_tool_records(
        &result.tool_records,
        "zh-CN",
        crate::chat::agent::recovery::FailureKind::Empty,
    );
    assert!(recovered.contains("result:read"));
    assert_eq!(result.stream_outcome, "completed");
    assert_eq!(result.content, recovered);
    assert_eq!(result.reasoning.as_deref(), Some("synthesis reasoning"));
    assert_eq!(result.tool_records.len(), 1);

    let final_message = result.api_messages.last().expect("final api message");
    assert_eq!(
        final_message.get("content").and_then(Value::as_str),
        Some(recovered.as_str())
    );
    assert_eq!(
        final_message
            .get("reasoning_content")
            .and_then(Value::as_str),
        Some("synthesis reasoning")
    );
}

/// Responses 格式的一段内置搜索 SSE（web_search_call 查询 + 正文 + url_citation 来源），
/// 供内置搜索实时卡端到端集成测试用（任务 07-23）。无 function_call → planning 直接终答。
fn responses_web_search_sse_events() -> Vec<String> {
    vec![
        r#"{"type":"response.output_item.added","item":{"id":"ws_1","type":"web_search_call","status":"in_progress"}}"#.to_string(),
        r#"{"type":"response.output_item.done","item":{"id":"ws_1","type":"web_search_call","status":"completed","action":{"type":"search","query":"kivio latest release"}}}"#.to_string(),
        r#"{"type":"response.output_text.delta","delta":"Kivio 最新版本信息。"}"#.to_string(),
        r#"{"type":"response.output_text.annotation.added","annotation":{"type":"url_citation","title":"Kivio Release","url":"https://kivio.dev/releases"}}"#.to_string(),
        r#"{"type":"response.completed","response":{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"Kivio 最新版本信息。"}]}]}}"#.to_string(),
        "[DONE]".to_string(),
    ]
}

/// 收集本条 assistant 消息里的内置搜索卡段（tool_call_id 以 `websearch_` 打头）。
fn web_search_card_segments(result: &AgentRunResult) -> Vec<&ChatMessageSegment> {
    result
        .segments
        .iter()
        .filter(|segment| {
            segment.kind == ChatMessageSegmentKind::Tool
                && segment
                    .tool_call_id
                    .as_deref()
                    .is_some_and(|id| id.starts_with("websearch_"))
        })
        .collect()
}

/// 集成（流式）：内置模式 + 支持内置搜索的 Responses provider，模型执行 hosted web_search。
/// 期望——**单张** web_search 卡，落在答案文本**之前**（预留槽 order < 正文段 order），
/// 终态 Success；不双卡；FinalAnswer 路径不丢卡（任务 07-23）。
#[tokio::test]
async fn run_loop_stream_builtin_web_search_card_precedes_answer_single_card() {
    let server = MockModelServer::start(vec![MockResponse::Sse(responses_web_search_sse_events())]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    // Responses provider（支持内置搜索）+ 会话内置模式。
    config.provider.api_format = "openai_responses".to_string();
    config.web_search_mode = crate::chat::types::WebSearchMode::Builtin;
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("builtin web search run completes");

    assert_eq!(result.stream_outcome, "completed");
    assert_eq!(result.content, "Kivio 最新版本信息。");

    // 落盘工具记录：恰好一条 web_search，终态 Success。
    let web_records: Vec<_> = result
        .tool_records
        .iter()
        .filter(|record| record.name == "web_search")
        .collect();
    assert_eq!(
        web_records.len(),
        1,
        "exactly one persisted web_search card"
    );
    assert!(matches!(web_records[0].status, ToolCallStatus::Success));

    // 卡段唯一（不双卡），且 order 落在答案文本段之前。
    let cards = web_search_card_segments(&result);
    assert_eq!(cards.len(), 1, "no double card");
    let answer = result
        .segments
        .iter()
        .find(|segment| {
            segment.kind == ChatMessageSegmentKind::Text
                && segment.text.as_deref() == Some("Kivio 最新版本信息。")
        })
        .expect("answer text segment persisted");
    assert!(
        cards[0].order < answer.order,
        "web search card (order {}) must precede the answer text (order {})",
        cards[0].order,
        answer.order,
    );
}

/// Gemini 原生出图（任务 07-24）：模型在流式答案里返回 inlineData 图片 →
/// `GenerateOutput.images` → 循环跨阶段累积进 `RunState.generated_images` →
/// `attach_usage` 挂到 `AgentRunResult.images`。reply 侧据此落成 assistant 消息级
/// artifacts（data_url + size_bytes 断言见 commands::reply 的转换单测）。
#[tokio::test]
async fn run_loop_gemini_native_image_lands_in_run_result_images() {
    let server = MockModelServer::start(vec![MockResponse::Sse(vec![
        r#"{"candidates":[{"content":{"parts":[{"text":"这是为你生成的图片。"},{"inlineData":{"mimeType":"image/png","data":"aGVsbG8="}}]}}]}"#
            .to_string(),
        r#"{"candidates":[{"finishReason":"STOP"}]}"#.to_string(),
    ])]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    config.provider.api_format = "gemini".to_string();
    // 出图模型不调工具：给纯文本任务，模型直接以「文本 + 图片」作答（无 tool call）→ FinalAnswer。
    config.tools = Vec::new();
    config.runtime_messages = vec![
        serde_json::json!({ "role": "system", "content": "system prompt" }),
        serde_json::json!({ "role": "user", "content": "画一只猫" }),
    ];
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("gemini image-gen answer must not bubble Err");

    assert_eq!(
        result.images.len(),
        1,
        "the single native image reaches the run result"
    );
    assert_eq!(result.images[0].mime_type, "image/png");
    assert_eq!(result.images[0].data, "aGVsbG8=");
    assert!(result.content.contains("这是为你生成的图片。"));
}

// ===== 生命周期 Hooks 的事件配对（任务 07-28-hooks）=====
//
// 手点 GUI 只能验一条路径，而 loop 有 8 条 return。这里用「只记流水、不真执行」的
// 调度器，把每条路径的派发序列钉死：每个 turn_start 必配一个 turn_end，
// 每个 message_start 必配一个 message_end，agent_end 恰好一次且永远收尾。

/// 挂了 recording 调度器的 TestHost。
struct HookedHost {
    inner: TestHost,
    hooks: crate::chat::hooks::HookDispatcher,
}

impl HookedHost {
    fn new(inner: TestHost) -> Self {
        Self {
            inner,
            hooks: crate::chat::hooks::HookDispatcher::recording(),
        }
    }

    fn events(&self) -> Vec<&'static str> {
        self.hooks
            .recorded()
            .into_iter()
            .map(|(event, _)| event)
            .collect()
    }
}

/// 事件流水的自洽断言：首尾是 agent_start/agent_end 且各恰好一次；
/// turn/message 严格配对且不嵌套；tool 的 start/end 数量相等。
fn assert_hook_events_well_formed(events: &[&str]) {
    assert_eq!(events.first(), Some(&"agent_start"), "{events:?}");
    assert_eq!(events.last(), Some(&"agent_end"), "{events:?}");
    for name in ["agent_start", "agent_end"] {
        assert_eq!(
            events.iter().filter(|event| **event == name).count(),
            1,
            "{name} must fire exactly once: {events:?}"
        );
    }
    let (mut turn_open, mut message_open) = (false, false);
    let (mut tool_start, mut tool_end) = (0, 0);
    for event in events {
        match *event {
            "turn_start" => {
                assert!(!turn_open, "nested turn_start: {events:?}");
                turn_open = true;
            }
            "turn_end" => {
                assert!(turn_open, "turn_end without turn_start: {events:?}");
                assert!(!message_open, "turn_end before message_end: {events:?}");
                turn_open = false;
            }
            "message_start" => {
                assert!(!message_open, "nested message_start: {events:?}");
                assert!(turn_open, "message_start outside a turn: {events:?}");
                message_open = true;
            }
            "message_end" => {
                assert!(
                    message_open,
                    "message_end without message_start: {events:?}"
                );
                message_open = false;
            }
            "tool_execution_start" => tool_start += 1,
            "tool_execution_end" => tool_end += 1,
            _ => {}
        }
    }
    assert!(!turn_open, "turn never closed: {events:?}");
    assert!(!message_open, "message never closed: {events:?}");
    assert_eq!(tool_start, tool_end, "unpaired tool events: {events:?}");
}

impl AgentHost for HookedHost {
    fn emit_stream_delta(
        &self,
        conversation_id: &str,
        run_id: &str,
        message_id: &str,
        delta: &str,
        reasoning_delta: Option<&str>,
        segment: Option<&ChatMessageSegment>,
    ) {
        self.inner.emit_stream_delta(
            conversation_id,
            run_id,
            message_id,
            delta,
            reasoning_delta,
            segment,
        );
    }

    fn emit_tool_record(
        &self,
        conversation_id: &str,
        run_id: &str,
        message_id: &str,
        record: &ToolCallRecord,
    ) {
        self.inner
            .emit_tool_record(conversation_id, run_id, message_id, record);
    }

    fn request_tool_approval<'a>(
        &'a self,
        ctx: &'a ToolExecutionContext<'a>,
        record: &'a ToolCallRecord,
    ) -> super::super::host::AgentHostFuture<'a, bool> {
        self.inner.request_tool_approval(ctx, record)
    }

    fn request_session_consent<'a>(
        &'a self,
        ctx: &'a ToolExecutionContext<'a>,
    ) -> super::super::host::AgentHostFuture<'a, bool> {
        self.inner.request_session_consent(ctx)
    }

    fn request_user_response<'a>(
        &'a self,
        ctx: &'a ToolExecutionContext<'a>,
        record: &'a ToolCallRecord,
        prompt: crate::chat::ask_user::AskUserPromptPayload,
    ) -> super::super::host::AgentHostFuture<'a, crate::chat::ask_user::AskUserResponseResult> {
        self.inner.request_user_response(ctx, record, prompt)
    }

    fn is_generation_active(&self, conversation_id: &str, generation: u64) -> bool {
        self.inner.is_generation_active(conversation_id, generation)
    }

    fn wait_for_generation_inactive<'a>(
        &'a self,
        conversation_id: &'a str,
        generation: u64,
    ) -> super::super::host::AgentHostFuture<'a, ()> {
        self.inner
            .wait_for_generation_inactive(conversation_id, generation)
    }

    fn persist_partial_assistant<'a>(
        &'a self,
        conversation_id: &'a str,
        message_id: &'a str,
        tool_records: &'a [ToolCallRecord],
        segments: &'a [ChatMessageSegment],
        api_messages: &'a [Value],
    ) -> super::super::host::AgentHostFuture<'a, ()> {
        self.inner.persist_partial_assistant(
            conversation_id,
            message_id,
            tool_records,
            segments,
            api_messages,
        )
    }

    fn hooks(&self) -> Option<&crate::chat::hooks::HookDispatcher> {
        Some(&self.hooks)
    }
}

/// 正常路径：一轮工具调用 + 合成。
#[tokio::test]
async fn hook_events_pair_up_on_the_tool_round_path() {
    let server = MockModelServer::start(vec![
        MockResponse::Sse(planning_tool_call_sse_events()),
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"好了"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
    ]);
    let state = test_app_state();
    let config = test_run_config(&state, &server.base_url);
    let host = HookedHost::new(TestHost::default());
    let executor = RecordingExecutor::default();

    run_agent_loop(config, &host, &executor)
        .await
        .expect("loop must succeed");

    let events = host.events();
    assert_hook_events_well_formed(&events);
    assert!(
        events.contains(&"tool_execution_start"),
        "tool round must emit tool events: {events:?}"
    );
}

/// 无工具会话：整段跳过工具循环，此前一个 turn/message 事件都不发（回归守卫）。
#[tokio::test]
async fn hook_events_pair_up_on_the_no_tools_path() {
    let server = MockModelServer::start(vec![MockResponse::Sse(vec![
        r#"{"choices":[{"delta":{"content":"直接回答"}}]}"#.to_string(),
        "[DONE]".to_string(),
    ])]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    config.tools = Vec::new();
    let host = HookedHost::new(TestHost::default());
    let executor = RecordingExecutor::default();

    run_agent_loop(config, &host, &executor)
        .await
        .expect("loop must succeed");

    let events = host.events();
    assert_hook_events_well_formed(&events);
    assert!(
        events.contains(&"turn_start") && events.contains(&"message_end"),
        "a tool-less conversation is still one turn: {events:?}"
    );
}

/// 取消路径：用户在工具轮后停止，收尾事件仍须闭合。
#[tokio::test]
async fn hook_events_pair_up_when_cancelled() {
    let server = MockModelServer::start(vec![
        MockResponse::Sse(planning_tool_call_sse_events()),
        MockResponse::Sse(planning_tool_call_sse_events()),
    ]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    // 必须放行第二轮：默认 max_tool_rounds=Some(1) 会让第一轮后直接撞 RoundLimit
    // break，`cancelling_on_persist` 翻的旗子根本没有循环顶部再去读它 —— 这条测试
    // 就会挂着「when_cancelled」的名字实际走正常路径。
    config.effective_chat_tools.max_tool_rounds = Some(2);
    let host = HookedHost::new(TestHost::cancelling_on_persist());
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("cancellation must not bubble Err");
    assert_eq!(
        result.stream_outcome, "cancelled",
        "this test is only meaningful if the run really was cancelled"
    );

    assert_hook_events_well_formed(&host.events());
    // 事件流水看不出取消（取消路径与正常路径同形），得直接查世代。
    assert!(
        host.hooks.cancel_epoch() > 0,
        "the loop must tell the dispatcher it was cancelled"
    );
}

/// 回归：**synthesis 阶段**的取消也必须传到调度器。loop 里显式 `cancel()` 的三处
/// 全在工具轮分支上，而用户点「停止」最常落在流式最长的 synthesis 阶段——那条路
/// 走的是 `SynthesisFlow::Early(outcome=cancelled)`，一处 `cancel()` 都不经过。
/// 漏掉的后果不是少发个事件，而是「停止」之后排队的 Hook 照跑、在跑的脚本没人杀。
#[tokio::test]
async fn cancelling_during_synthesis_still_cancels_hooks() {
    let server = MockModelServer::start(vec![
        MockResponse::Sse(planning_tool_call_sse_events()),
        MockResponse::SseThenHang(vec![
            r#"{"choices":[{"delta":{"content":"部分回答"}}]}"#.to_string()
        ]),
    ]);
    let state = test_app_state();
    let config = test_run_config(&state, &server.base_url);
    let host = HookedHost::new(TestHost::cancelling_on_first_text_delta());
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("cancelled synthesis must not bubble Err");
    assert_eq!(result.stream_outcome, "cancelled");

    assert_hook_events_well_formed(&host.events());
    assert!(
        host.hooks.cancel_epoch() > 0,
        "a stop during synthesis must cancel queued/running hooks, not just emit agent_end"
    );
}

/// 正常完成的 run **不能**取消 Hook —— 兜底判的是 generation 而非「有没有出错」，
/// 判错了就会把成功路径上排队的 `agent_end` 之前的 Hook 静默丢掉。
#[tokio::test]
async fn a_successful_run_never_cancels_hooks() {
    let server = MockModelServer::start(vec![
        MockResponse::Sse(planning_tool_call_sse_events()),
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"好了"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
    ]);
    let state = test_app_state();
    let config = test_run_config(&state, &server.base_url);
    let host = HookedHost::new(TestHost::default());
    let executor = RecordingExecutor::default();

    run_agent_loop(config, &host, &executor)
        .await
        .expect("loop must succeed");

    assert_eq!(
        host.hooks.cancel_epoch(),
        0,
        "a clean run must not cancel hooks"
    );
}

// ---------------------------------------------------------------------------
// Coarse smoke gates — the happy paths a regression would break first.
// Keep these few and fat; edge-case coverage lives in the tests above.
// ---------------------------------------------------------------------------

/// Smoke: plain chat with no tools returns a completed answer.
#[tokio::test]
async fn run_loop_smoke_plain_answer_completes() {
    let server = MockModelServer::start(vec![MockResponse::Sse(vec![
        r#"{"choices":[{"delta":{"content":"你好，我是 Kivio。"}}]}"#.to_string(),
        "[DONE]".to_string(),
    ])]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    config.tools = Vec::new();
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("plain answer must not error");

    assert_eq!(result.stream_outcome, "completed");
    assert_eq!(result.content, "你好，我是 Kivio。");
    assert!(result.tool_records.is_empty());
    assert!(executor.events().is_empty(), "no tools configured");
}

/// Smoke: tool call → executor runs → tool result is fed back → final answer.
#[tokio::test]
async fn run_loop_smoke_tool_then_final_answer_round_trips() {
    let server = MockModelServer::start(vec![
        MockResponse::Sse(planning_tool_call_sse_events()),
        MockResponse::Sse(vec![
            r#"{"choices":[{"delta":{"content":"文件内容已读完。"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]),
    ]);
    let state = test_app_state();
    let mut config = test_run_config(&state, &server.base_url);
    config.effective_chat_tools.max_tool_rounds = Some(2);
    let host = TestHost::default();
    let executor = RecordingExecutor::default();

    let result = run_agent_loop(config, &host, &executor)
        .await
        .expect("tool round-trip must complete");

    assert_eq!(result.stream_outcome, "completed");
    assert_eq!(result.content, "文件内容已读完。");
    assert!(
        result.tool_records.iter().any(|r| r.id == "call_read"),
        "tool record must land on the run result: {:?}",
        result.tool_records
    );
    assert_eq!(
        executor.events(),
        vec!["start:read".to_string(), "finish:read".to_string()],
        "the native tool must actually execute"
    );

    let bodies = server.captured_bodies();
    assert_eq!(bodies.len(), 2, "planning + synthesis");
    // Second request must carry the tool result so the model can answer from it.
    assert!(
        bodies[1].contains("result:read") || bodies[1].contains("tool"),
        "synthesis request must include the tool result; body={}",
        bodies[1]
    );
    assert!(
        bodies[1].contains("call_read") || bodies[1].contains("\"role\":\"tool\""),
        "synthesis request must include the tool call id / tool role; body={}",
        bodies[1]
    );
}
