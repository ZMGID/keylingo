use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::chat::model::{GenerateOutput, ModelError, PendingToolCall, StreamPart, StreamSink};
use crate::chat::types::{
    ChatMessageSegment, ChatMessageSegmentKind, ChatMessageSegmentPhase, ToolCallRecord,
    ToolCallStatus,
};
use crate::mcp::ChatToolDefinition;

use super::host::AgentHost;
use super::prepare::disabled_builtin_tool_feedback;
use super::stop::{
    empty_assistant_response_error, final_assistant_api_message, pending_tool_calls_from_dsml,
    sanitize_assistant_text_response,
};
use super::types::AgentStreamPolicy;

#[derive(Default)]
struct ChatStreamAccumulator {
    content: String,
    reasoning: String,
}

struct ChatStreamSnapshot {
    content: String,
    reasoning: String,
}

#[derive(Clone)]
pub struct ToolCallDraftTracker {
    inner: Arc<Mutex<ToolCallDraftState>>,
}

struct ToolCallDraftState {
    tools: Vec<ChatToolDefinition>,
    round: u32,
    step_number: Option<u8>,
    next_order: u32,
    drafts: Vec<ToolCallDraft>,
}

struct ToolCallDraft {
    model_name: String,
    arguments_raw: String,
    record: ToolCallRecord,
    segment: ChatMessageSegment,
    last_emitted_argument_chars: usize,
    done: bool,
}

impl ToolCallDraftTracker {
    pub fn new(
        tools: Vec<ChatToolDefinition>,
        round: u32,
        step_number: Option<u8>,
        first_order: u32,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ToolCallDraftState {
                tools,
                round,
                step_number,
                next_order: first_order,
                drafts: Vec::new(),
            })),
        }
    }

    pub fn has_started(&self) -> bool {
        !self
            .inner
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .drafts
            .is_empty()
    }

    pub fn segments(&self) -> Vec<ChatMessageSegment> {
        self.inner
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .drafts
            .iter()
            .map(|draft| draft.segment.clone())
            .collect()
    }

    pub fn has_unfinished_drafts(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .drafts
            .iter()
            .any(|draft| !draft.done)
    }

    pub fn mark_error(&self, error: &str) -> Vec<ToolCallRecord> {
        let now = chrono::Local::now().timestamp();
        let mut guard = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        for draft in &mut guard.drafts {
            draft.record.status = ToolCallStatus::Error;
            draft.record.completed_at = Some(now);
            draft.record.duration_ms = draft
                .record
                .started_at
                .map(|started| (now.saturating_sub(started) as u64).saturating_mul(1000))
                .or(Some(0));
            draft.record.error = Some(error.to_string());
            draft.record.result_preview = None;
            draft.record.structured_content = Some(tool_draft_structured_content(
                &draft.model_name,
                "error",
                draft.arguments_raw.chars().count(),
            ));
        }
        guard
            .drafts
            .iter()
            .map(|draft| draft.record.clone())
            .collect()
    }
}

fn chat_stream_snapshot(accumulator: &Arc<Mutex<ChatStreamAccumulator>>) -> ChatStreamSnapshot {
    let guard = accumulator.lock().unwrap_or_else(|err| err.into_inner());
    ChatStreamSnapshot {
        content: guard.content.clone(),
        reasoning: guard.reasoning.clone(),
    }
}

pub struct AgentStreamSink<'a> {
    host: &'a dyn AgentHost,
    conversation_id: String,
    run_id: String,
    message_id: String,
    accumulator: Arc<Mutex<ChatStreamAccumulator>>,
    buffer_tool_planning_text: bool,
    text_segment: Option<ChatMessageSegment>,
    reasoning_segment: Option<ChatMessageSegment>,
    tool_draft_tracker: Option<ToolCallDraftTracker>,
    text_buffer: String,
    text_suppressed: bool,
}

impl<'a> AgentStreamSink<'a> {
    pub fn new(
        host: &'a dyn AgentHost,
        conversation_id: &str,
        run_id: &str,
        message_id: &str,
        buffer_tool_planning_text: bool,
        text_segment: Option<ChatMessageSegment>,
        reasoning_segment: Option<ChatMessageSegment>,
        tool_draft_tracker: Option<ToolCallDraftTracker>,
    ) -> Self {
        Self {
            host,
            conversation_id: conversation_id.to_string(),
            run_id: run_id.to_string(),
            message_id: message_id.to_string(),
            accumulator: Arc::new(Mutex::new(ChatStreamAccumulator::default())),
            buffer_tool_planning_text,
            text_segment,
            reasoning_segment,
            tool_draft_tracker,
            text_buffer: String::new(),
            text_suppressed: false,
        }
    }

    pub fn snapshot(&self) -> (String, String) {
        let snapshot = chat_stream_snapshot(&self.accumulator);
        (snapshot.content, snapshot.reasoning)
    }

    fn emit_text_delta(&self, delta: &str) {
        self.host.emit_stream_delta(
            &self.conversation_id,
            &self.run_id,
            &self.message_id,
            delta,
            None,
            self.text_segment.as_ref(),
        );
    }

    fn handle_text_delta(&mut self, delta: String) {
        self.accumulator
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .content
            .push_str(&delta);

        if self.text_suppressed {
            return;
        }
        if !self.buffer_tool_planning_text {
            self.emit_text_delta(&delta);
            return;
        }

        self.text_buffer.push_str(&delta);
        if crate::chat::dsml_tools::contains_dsml_tool_markup(&self.text_buffer) {
            self.text_buffer.clear();
            self.text_suppressed = true;
            return;
        }
        if should_flush_tool_planning_text_buffer(&self.text_buffer) {
            self.flush_pending_text();
        }
    }

    pub fn flush_pending_text(&mut self) {
        if self.text_suppressed || self.text_buffer.is_empty() {
            return;
        }
        let delta = std::mem::take(&mut self.text_buffer);
        self.emit_text_delta(&delta);
    }

    fn emit_tool_call_start(&mut self, id: String, name: String) {
        let Some(tracker) = self.tool_draft_tracker.as_ref() else {
            return;
        };
        let mut guard = tracker.inner.lock().unwrap_or_else(|err| err.into_inner());
        if guard.drafts.iter().any(|draft| draft.record.id == id) {
            return;
        }
        if find_tool_definition(&guard.tools, &name).is_none()
            && disabled_builtin_tool_feedback(&name).is_some()
        {
            return;
        }
        let (record_name, source, server_id, sensitive) =
            if let Some(tool) = find_tool_definition(&guard.tools, &name) {
                (
                    tool.name.clone(),
                    tool.source.clone(),
                    tool.server_id.clone(),
                    tool.sensitive,
                )
            } else {
                (name.clone(), "unknown".to_string(), None, false)
            };
        let order = guard.next_order;
        guard.next_order = guard.next_order.saturating_add(1);
        let now = chrono::Local::now().timestamp();
        let segment = ChatMessageSegment {
            id: format!("seg_{}_tool_{}", order, id),
            kind: ChatMessageSegmentKind::Tool,
            phase: ChatMessageSegmentPhase::ToolLoop,
            order,
            step_number: guard.step_number,
            round: Some(guard.round),
            text: None,
            tool_call_id: Some(id.clone()),
        };
        let record = ToolCallRecord {
            id: id.clone(),
            name: record_name,
            source,
            server_id,
            arguments: tool_draft_arguments(&name, "generating_arguments", 0),
            status: ToolCallStatus::Pending,
            result_preview: Some(tool_draft_preview(&name, "generating_arguments", 0)),
            error: None,
            duration_ms: None,
            started_at: Some(now),
            completed_at: None,
            round: guard.round,
            sensitive,
            artifacts: Vec::new(),
            trace_id: Some(self.run_id.clone()),
            span_id: Some(tool_draft_span_id(guard.round, &id)),
            structured_content: Some(tool_draft_structured_content(
                &name,
                "generating_arguments",
                0,
            )),
        };
        guard.drafts.push(ToolCallDraft {
            model_name: name,
            arguments_raw: String::new(),
            record: record.clone(),
            segment: segment.clone(),
            last_emitted_argument_chars: 0,
            done: false,
        });
        drop(guard);
        self.host.emit_stream_delta(
            &self.conversation_id,
            &self.run_id,
            &self.message_id,
            "",
            None,
            Some(&segment),
        );
        self.host.emit_tool_record(
            &self.conversation_id,
            &self.run_id,
            &self.message_id,
            &record,
        );
    }

    fn emit_tool_call_delta(&mut self, id: String, delta: String) {
        let Some(tracker) = self.tool_draft_tracker.as_ref() else {
            return;
        };
        let record_to_emit = {
            let mut guard = tracker.inner.lock().unwrap_or_else(|err| err.into_inner());
            let Some(draft) = guard.drafts.iter_mut().find(|draft| draft.record.id == id) else {
                return;
            };
            draft.arguments_raw.push_str(&delta);
            let chars = draft.arguments_raw.chars().count();
            if chars == 0 || chars.saturating_sub(draft.last_emitted_argument_chars) < 2048 {
                return;
            }
            draft.last_emitted_argument_chars = chars;
            draft.record.arguments =
                tool_draft_arguments(&draft.model_name, "generating_arguments", chars);
            draft.record.result_preview = Some(tool_draft_preview(
                &draft.model_name,
                "generating_arguments",
                chars,
            ));
            draft.record.structured_content = Some(tool_draft_structured_content(
                &draft.model_name,
                "generating_arguments",
                chars,
            ));
            Some(draft.record.clone())
        };
        if let Some(record) = record_to_emit {
            self.host.emit_tool_record(
                &self.conversation_id,
                &self.run_id,
                &self.message_id,
                &record,
            );
        }
    }

    fn emit_tool_call_done(&mut self, call: &PendingToolCall) {
        let Some(tracker) = self.tool_draft_tracker.as_ref() else {
            return;
        };
        let record_to_emit = {
            let mut guard = tracker.inner.lock().unwrap_or_else(|err| err.into_inner());
            let Some(draft) = guard
                .drafts
                .iter_mut()
                .find(|draft| draft.record.id == call.id)
            else {
                return;
            };
            draft.done = true;
            draft.arguments_raw = call.arguments_raw.clone();
            let chars = draft.arguments_raw.chars().count();
            draft.last_emitted_argument_chars = chars;
            draft.record.arguments = call.arguments_raw.clone();
            draft.record.result_preview = Some(tool_draft_preview(
                &draft.model_name,
                "arguments_ready",
                chars,
            ));
            draft.record.structured_content = Some(tool_draft_structured_content(
                &draft.model_name,
                "arguments_ready",
                chars,
            ));
            Some(draft.record.clone())
        };
        if let Some(record) = record_to_emit {
            self.host.emit_tool_record(
                &self.conversation_id,
                &self.run_id,
                &self.message_id,
                &record,
            );
        }
    }
}

impl StreamSink for AgentStreamSink<'_> {
    fn emit(&mut self, part: StreamPart) -> Result<(), ModelError> {
        match part {
            StreamPart::TextDelta { delta } => {
                self.handle_text_delta(delta);
            }
            StreamPart::ReasoningDelta { delta } => {
                self.accumulator
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .reasoning
                    .push_str(&delta);
                self.host.emit_stream_delta(
                    &self.conversation_id,
                    &self.run_id,
                    &self.message_id,
                    "",
                    Some(&delta),
                    self.reasoning_segment.as_ref(),
                );
            }
            StreamPart::Error { message } => return Err(ModelError::new(message)),
            StreamPart::ToolCallStart { id, name } => self.emit_tool_call_start(id, name),
            StreamPart::ToolCallDelta { id, delta } => self.emit_tool_call_delta(id, delta),
            StreamPart::ToolCallDone { call } => self.emit_tool_call_done(&call),
            StreamPart::Finish { .. } => {}
        }
        Ok(())
    }
}

fn find_tool_definition<'a>(
    tools: &'a [ChatToolDefinition],
    function_name: &str,
) -> Option<&'a ChatToolDefinition> {
    tools
        .iter()
        .find(|tool| tool.openai_tool_name() == function_name || tool.name == function_name)
}

fn tool_draft_span_id(round: u32, tool_call_id: &str) -> String {
    format!("tool_round_{}_{}", round, tool_call_id)
}

fn tool_draft_arguments(name: &str, phase: &str, argument_chars: usize) -> String {
    serde_json::json!({
        "_kivioToolDraft": true,
        "tool": name,
        "phase": phase,
        "argumentChars": argument_chars,
    })
    .to_string()
}

fn tool_draft_structured_content(name: &str, phase: &str, argument_chars: usize) -> Value {
    serde_json::json!({
        "toolDraft": {
            "toolName": name,
            "phase": phase,
            "argumentChars": argument_chars,
        }
    })
}

fn tool_draft_preview(name: &str, phase: &str, argument_chars: usize) -> String {
    if phase == "arguments_ready" {
        return "工具参数已生成，等待调用…".to_string();
    }
    let prefix = match name {
        "write" => "正在生成文件内容",
        "edit" => "正在生成编辑参数",
        _ => "正在生成工具参数",
    };
    if argument_chars == 0 {
        format!("{prefix}…")
    } else {
        format!("{prefix}…已收到 {} 字符", format_count(argument_chars))
    }
}

fn format_count(value: usize) -> String {
    let text = value.to_string();
    let mut out = String::new();
    for (idx, ch) in text.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

pub fn should_flush_tool_planning_text_buffer(buffer: &str) -> bool {
    let trimmed = buffer.trim_start();
    if trimmed.starts_with('<') && trimmed.len() < 64 {
        return false;
    }
    buffer.chars().count() >= 12 || buffer.contains('\n')
}

pub struct ChatStreamOutput {
    pub content: String,
    pub raw_content: String,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<PendingToolCall>,
    pub finish_reason: Option<String>,
    pub cancelled: bool,
    /// Provider-reported usage for this single model call (None when the
    /// provider does not report usage or the stream was cancelled mid-flight).
    pub usage: Option<crate::chat::model::ModelUsage>,
    /// 模型原生内置联网搜索的解析结果（仅内置搜索发生时为 Some）。循环据此合成一张
    /// 「网络搜索」工具卡（任务 07-23）。
    pub web_search: Option<crate::chat::model::BuiltinWebSearch>,
}

impl ChatStreamOutput {
    pub fn new(content: String, reasoning: String, cancelled: bool) -> Self {
        Self::from_generate_output(
            content.clone(),
            content,
            reasoning,
            Vec::new(),
            None,
            cancelled,
        )
    }

    pub fn from_generate_output(
        content: String,
        raw_content: String,
        reasoning: String,
        tool_calls: Vec<PendingToolCall>,
        finish_reason: Option<String>,
        cancelled: bool,
    ) -> Self {
        Self {
            content,
            raw_content,
            reasoning: if reasoning.trim().is_empty() {
                None
            } else {
                Some(reasoning)
            },
            tool_calls,
            finish_reason,
            cancelled,
            usage: None,
            web_search: None,
        }
    }

    pub fn from_generate_output_with_snapshot(
        output: GenerateOutput,
        snapshot_content: String,
        snapshot_reasoning: String,
    ) -> Self {
        let raw_content = if output.text.trim().is_empty() {
            snapshot_content
        } else {
            output.text
        };
        let cleaned = sanitize_assistant_text_response(raw_content.trim());
        let reasoning = output.reasoning.unwrap_or(snapshot_reasoning);
        let web_search = output.web_search;
        let mut result = Self::from_generate_output(
            cleaned,
            raw_content,
            reasoning,
            output.tool_calls,
            output.finish_reason,
            false,
        );
        result.usage = output.usage;
        result.web_search = web_search;
        result
    }

    pub fn to_openai_compatible_message(&self) -> Value {
        let content = if self.raw_content.trim().is_empty() {
            self.content.clone()
        } else {
            self.raw_content.clone()
        };
        let mut message = final_assistant_api_message(&content, self.reasoning.as_deref());
        if !self.tool_calls.is_empty() {
            message["tool_calls"] = Value::Array(
                self.tool_calls
                    .iter()
                    .map(|call| {
                        let mut tc = serde_json::json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.function_name,
                                "arguments": call.arguments_raw,
                            }
                        });
                        // Gemini thoughtSignature：搭在自定义键上，经存储/回放 → canonical
                        // MessagePart::ToolCall.signature → 回放时带回 functionCall（其他 provider 无此字段）。
                        if let Some(signature) = &call.signature {
                            tc["thought_signature"] = Value::String(signature.clone());
                        }
                        tc
                    })
                    .collect(),
            );
        }
        if let Some(finish_reason) = self
            .finish_reason
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            message["finish_reason"] = Value::String(finish_reason.to_string());
        }
        message
    }
}

pub fn should_emit_done(policy: AgentStreamPolicy, output: &ChatStreamOutput) -> bool {
    match policy {
        AgentStreamPolicy::SynthesisAlwaysDone => true,
        AgentStreamPolicy::SynthesisDeferEmpty => !output.content.trim().is_empty(),
        AgentStreamPolicy::PlanningNoDoneUntilNoTools => {
            output.tool_calls.is_empty()
                && pending_tool_calls_from_dsml(&output.raw_content).is_empty()
        }
    }
}

pub fn validate_stream_output(
    label: &str,
    policy: AgentStreamPolicy,
    output: &ChatStreamOutput,
) -> Result<(), String> {
    let tool_calls_from_stream = !output.tool_calls.is_empty()
        || !pending_tool_calls_from_dsml(&output.raw_content).is_empty();
    if output.content.trim().is_empty() {
        match policy {
            AgentStreamPolicy::SynthesisAlwaysDone => {
                return Err(empty_assistant_response_error(label));
            }
            AgentStreamPolicy::SynthesisDeferEmpty => return Ok(()),
            AgentStreamPolicy::PlanningNoDoneUntilNoTools if !tool_calls_from_stream => {
                return Err(empty_assistant_response_error(label));
            }
            AgentStreamPolicy::PlanningNoDoneUntilNoTools => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::chat::agent::execute::ToolExecutionContext;
    use crate::chat::agent::host::AgentHostFuture;
    use crate::chat::ask_user::{AskUserPromptPayload, AskUserResponseResult};
    use crate::mcp::types::native_write_file_tool;

    #[derive(Default)]
    struct TestHost {
        records: Mutex<Vec<ToolCallRecord>>,
        segments: Mutex<Vec<ChatMessageSegment>>,
        done_reasons: Mutex<Vec<String>>,
    }

    impl AgentHost for TestHost {
        fn emit_stream_delta(
            &self,
            _conversation_id: &str,
            _run_id: &str,
            _message_id: &str,
            _delta: &str,
            _reasoning_delta: Option<&str>,
            segment: Option<&ChatMessageSegment>,
        ) {
            if let Some(segment) = segment {
                self.segments
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .push(segment.clone());
            }
        }

        fn emit_stream_done(
            &self,
            _conversation_id: &str,
            _run_id: &str,
            _message_id: &str,
            reason: &str,
            _full: &str,
        ) {
            self.done_reasons
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .push(reason.to_string());
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

        fn request_tool_approval<'a>(
            &'a self,
            _ctx: &'a ToolExecutionContext<'a>,
            _record: &'a ToolCallRecord,
        ) -> AgentHostFuture<'a, bool> {
            Box::pin(async { true })
        }

        fn request_session_consent<'a>(
            &'a self,
            _ctx: &'a ToolExecutionContext<'a>,
        ) -> AgentHostFuture<'a, bool> {
            Box::pin(async { true })
        }

        fn request_user_response<'a>(
            &'a self,
            _ctx: &'a ToolExecutionContext<'a>,
            _record: &'a ToolCallRecord,
            _prompt: AskUserPromptPayload,
        ) -> AgentHostFuture<'a, AskUserResponseResult> {
            Box::pin(async { crate::chat::ask_user::skipped_response() })
        }

        fn is_generation_active(&self, _conversation_id: &str, _generation: u64) -> bool {
            true
        }

        fn wait_for_generation_inactive<'a>(
            &'a self,
            _conversation_id: &'a str,
            _generation: u64,
        ) -> AgentHostFuture<'a, ()> {
            Box::pin(async { std::future::pending::<()>().await })
        }
    }

    #[test]
    fn tool_planning_text_buffer_delays_possible_dsml_prefix() {
        assert!(!should_flush_tool_planning_text_buffer("<|DSML|"));
        assert!(!should_flush_tool_planning_text_buffer("   <invoke"));
        assert!(should_flush_tool_planning_text_buffer(
            "普通回答已经足够长，可以开始流式显示了"
        ));
        assert!(should_flush_tool_planning_text_buffer("first line\n"));
    }

    #[test]
    fn tool_call_stream_parts_emit_draft_records_and_segments() {
        let host = TestHost::default();
        let tracker = ToolCallDraftTracker::new(vec![native_write_file_tool()], 2, Some(3), 1002);
        let mut sink = AgentStreamSink::new(
            &host,
            "conversation",
            "run",
            "message",
            true,
            None,
            None,
            Some(tracker.clone()),
        );

        sink.emit(StreamPart::ToolCallStart {
            id: "call_write".to_string(),
            name: "write".to_string(),
        })
        .expect("start should emit");
        sink.emit(StreamPart::ToolCallDelta {
            id: "call_write".to_string(),
            delta: "{\"path\":\"demo.html\",\"content\":\"".to_string(),
        })
        .expect("delta should emit");
        let call = PendingToolCall {
            id: "call_write".to_string(),
            function_name: "write".to_string(),
            arguments: serde_json::json!({
                "path": "demo.html",
                "content": "<html></html>"
            }),
            arguments_raw: "{\"path\":\"demo.html\",\"content\":\"<html></html>\"}".to_string(),
            arguments_parse_error: None,
            signature: None,
        };
        sink.emit(StreamPart::ToolCallDone { call })
            .expect("done should emit");

        let records = host.records.lock().unwrap_or_else(|err| err.into_inner());
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].id, "call_write");
        assert_eq!(records[0].name, "write");
        assert!(matches!(records[0].status, ToolCallStatus::Pending));
        assert!(records[0]
            .result_preview
            .as_deref()
            .unwrap_or_default()
            .contains("正在生成文件内容"));
        assert_eq!(records[0].trace_id.as_deref(), Some("run"));
        assert_eq!(
            records[0].span_id.as_deref(),
            Some("tool_round_2_call_write")
        );
        assert!(records[1]
            .result_preview
            .as_deref()
            .unwrap_or_default()
            .contains("工具参数已生成"));
        assert!(records[1].arguments.contains("demo.html"));

        let segments = host.segments.lock().unwrap_or_else(|err| err.into_inner());
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].kind, ChatMessageSegmentKind::Tool);
        assert_eq!(segments[0].phase, ChatMessageSegmentPhase::ToolLoop);
        assert_eq!(segments[0].tool_call_id.as_deref(), Some("call_write"));
        assert_eq!(segments[0].order, 1002);
        assert!(tracker.has_started());
        assert!(!tracker.has_unfinished_drafts());
    }

    #[test]
    fn tool_call_draft_error_preserves_backend_record_after_stream_failure() {
        let host = TestHost::default();
        let tracker = ToolCallDraftTracker::new(vec![native_write_file_tool()], 1, Some(1), 1000);
        let mut sink = AgentStreamSink::new(
            &host,
            "conversation",
            "run",
            "message",
            true,
            None,
            None,
            Some(tracker.clone()),
        );

        sink.emit(StreamPart::ToolCallStart {
            id: "call_write".to_string(),
            name: "write".to_string(),
        })
        .expect("start should emit");
        sink.emit(StreamPart::ToolCallDelta {
            id: "call_write".to_string(),
            delta: "{\"path\":\"large.html\",\"content\":\"".to_string(),
        })
        .expect("delta should emit");

        let failed = tracker.mark_error("Chat tools planning read body failed");

        assert_eq!(failed.len(), 1);
        let record = &failed[0];
        assert_eq!(record.id, "call_write");
        assert_eq!(record.name, "write");
        assert!(matches!(record.status, ToolCallStatus::Error));
        assert_eq!(
            record.error.as_deref(),
            Some("Chat tools planning read body failed")
        );
        assert_eq!(record.trace_id.as_deref(), Some("run"));
        assert_eq!(record.span_id.as_deref(), Some("tool_round_1_call_write"));
        assert_eq!(
            record
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/toolDraft/phase"))
                .and_then(Value::as_str),
            Some("error")
        );
        assert_eq!(
            record
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/toolDraft/argumentChars"))
                .and_then(Value::as_u64),
            Some("{\"path\":\"large.html\",\"content\":\"".chars().count() as u64)
        );

        let segments = host.segments.lock().unwrap_or_else(|err| err.into_inner());
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].tool_call_id.as_deref(), Some("call_write"));
        assert!(tracker.has_unfinished_drafts());
    }

    #[test]
    fn synthesis_defer_empty_allows_agent_fallback_without_done() {
        let output = ChatStreamOutput::from_generate_output(
            String::new(),
            String::new(),
            String::new(),
            Vec::new(),
            Some("done".to_string()),
            false,
        );

        assert!(!should_emit_done(
            AgentStreamPolicy::SynthesisDeferEmpty,
            &output
        ));
        assert!(validate_stream_output(
            "Chat stream",
            AgentStreamPolicy::SynthesisDeferEmpty,
            &output
        )
        .is_ok());
        assert_eq!(
            validate_stream_output(
                "Chat stream",
                AgentStreamPolicy::SynthesisAlwaysDone,
                &output
            )
            .expect_err("strict synthesis should still reject empty output"),
            "Chat stream returned an empty assistant response"
        );
    }

    #[test]
    fn synthesis_defer_empty_emits_done_for_non_empty_output() {
        let output = ChatStreamOutput::from_generate_output(
            "final".to_string(),
            "final".to_string(),
            String::new(),
            Vec::new(),
            Some("done".to_string()),
            false,
        );

        assert!(should_emit_done(
            AgentStreamPolicy::SynthesisDeferEmpty,
            &output
        ));
        validate_stream_output(
            "Chat stream",
            AgentStreamPolicy::SynthesisDeferEmpty,
            &output,
        )
        .expect("non-empty synthesis should validate");
    }
}
