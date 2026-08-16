use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::chat::model::{
    GenerateOutput, GeneratedImageData, ModelError, PendingToolCall, StreamPart, StreamSink,
    WebCitation,
};
use crate::chat::types::{
    ChatMessageSegment, ChatMessageSegmentKind, ChatMessageSegmentPhase, ToolCallRecord,
    ToolCallStatus,
};
use crate::mcp::ChatToolDefinition;

use super::finalize::{build_web_search_record, build_web_search_segment};
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

/// 模型**原生内置联网搜索**的实时卡追踪器（任务 07-23）。镜像 `ToolCallDraftTracker`
/// 的「sink 持 tracker 边流边累加 → 流后 caller 读回落盘」模式：`Arc<Mutex>` 让 sink 与
/// 调用方（planning/synthesis）共享同一状态。稳定 id + reasoning/正文之间预留的 order 槽
/// 使卡渲染在答案文本**之前**；去重累加 queries/citations；`started` 保证段只发一次。
#[derive(Clone)]
pub struct WebSearchCardTracker {
    inner: Arc<Mutex<WebSearchCardState>>,
}

struct WebSearchCardState {
    id: String,
    order: u32,
    round: Option<u32>,
    phase: ChatMessageSegmentPhase,
    provider_label: String,
    queries: Vec<String>,
    citations: Vec<WebCitation>,
    started: bool,
    record: Option<ToolCallRecord>,
    segment: Option<ChatMessageSegment>,
}

/// 一次 `note` 的产物：`segment` 仅首帧为 `Some`（段只发一次），`record` 每帧都发
/// （同 id 幂等 merge → 前端原地更新 Running 卡）。
pub(crate) struct WebSearchCardUpdate {
    pub(crate) segment: Option<ChatMessageSegment>,
    pub(crate) record: ToolCallRecord,
}

impl WebSearchCardTracker {
    pub(crate) fn new(
        order: u32,
        round: Option<u32>,
        phase: ChatMessageSegmentPhase,
        provider_label: String,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(WebSearchCardState {
                id: format!("websearch_{}", uuid::Uuid::new_v4()),
                order,
                round,
                phase,
                provider_label,
                queries: Vec::new(),
                citations: Vec::new(),
                started: false,
                record: None,
                segment: None,
            })),
        }
    }

    /// 收到一帧内置搜索增量：去重累加查询/来源，回传本帧要 emit 的段（仅首帧）+ Running 记录。
    pub(crate) fn note(
        &self,
        queries: &[String],
        citations: &[WebCitation],
    ) -> WebSearchCardUpdate {
        let mut guard = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        for query in queries {
            let query = query.trim();
            if !query.is_empty() && !guard.queries.iter().any(|existing| existing == query) {
                guard.queries.push(query.to_string());
            }
        }
        for citation in citations {
            let url = citation.url.trim();
            if url.is_empty() {
                continue;
            }
            if let Some(existing) = guard
                .citations
                .iter_mut()
                .find(|existing| existing.url == url)
            {
                // 同 url 后到帧带新字段（Gemini 的 groundingSupports 可能晚于 chunk）→ 补全。
                if existing.snippet.is_none() {
                    existing.snippet = citation.snippet.clone();
                }
                if existing.published_date.is_none() {
                    existing.published_date = citation.published_date.clone();
                }
                continue;
            }
            guard.citations.push(citation.clone());
        }
        let record = build_web_search_record(
            &guard.id,
            &guard.provider_label,
            &guard.queries,
            &guard.citations,
            ToolCallStatus::Running,
            guard.round,
        );
        guard.record = Some(record.clone());
        let segment = if guard.started {
            None
        } else {
            guard.started = true;
            let segment =
                build_web_search_segment(guard.order, &guard.id, guard.phase.clone(), guard.round);
            guard.segment = Some(segment.clone());
            Some(segment)
        };
        WebSearchCardUpdate { segment, record }
    }

    /// 流成功结束 ⇒ 把卡翻 Success 并回传终态记录（供 caller emit）。从未开牌则 `None`。
    pub(crate) fn finalize_success(&self) -> Option<ToolCallRecord> {
        let mut guard = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        if !guard.started {
            return None;
        }
        let record = build_web_search_record(
            &guard.id,
            &guard.provider_label,
            &guard.queries,
            &guard.citations,
            ToolCallStatus::Success,
            guard.round,
        );
        guard.record = Some(record.clone());
        Some(record)
    }

    /// 取回卡的（记录, 段）供落盘。已 `finalize_success` 则记录为 Success。从未开牌则 `None`
    /// （取消中途 kill / 未发生搜索：实时卡不落盘，与 draft tracker 现状一致）。
    pub(crate) fn take_card(&self) -> Option<(ToolCallRecord, ChatMessageSegment)> {
        let guard = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        match (guard.record.clone(), guard.segment.clone()) {
            (Some(record), Some(segment)) => Some((record, segment)),
            _ => None,
        }
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
    web_search_tracker: Option<WebSearchCardTracker>,
    text_buffer: String,
    text_suppressed: bool,
}

impl<'a> AgentStreamSink<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        host: &'a dyn AgentHost,
        conversation_id: &str,
        run_id: &str,
        message_id: &str,
        buffer_tool_planning_text: bool,
        text_segment: Option<ChatMessageSegment>,
        reasoning_segment: Option<ChatMessageSegment>,
        tool_draft_tracker: Option<ToolCallDraftTracker>,
        web_search_tracker: Option<WebSearchCardTracker>,
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
            web_search_tracker,
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

    /// 消费一帧内置搜索增量（任务 07-23）：无 tracker（非内置模式）直接忽略——保证
    /// `web_search_tracker=None` 时行为与现状逐字节一致。首帧 emit 段（把卡插入答案之前的
    /// 预留槽），每帧 emit Running 记录（同 id 幂等 → 前端原地更新）。
    fn handle_web_search(&mut self, queries: Vec<String>, citations: Vec<WebCitation>) {
        let Some(tracker) = self.web_search_tracker.as_ref() else {
            return;
        };
        let update = tracker.note(&queries, &citations);
        if let Some(segment) = update.segment.as_ref() {
            self.host.emit_stream_delta(
                &self.conversation_id,
                &self.run_id,
                &self.message_id,
                "",
                None,
                Some(segment),
            );
        }
        self.host.emit_tool_record(
            &self.conversation_id,
            &self.run_id,
            &self.message_id,
            &update.record,
        );
    }

    /// 流成功结束时把内置搜索实时卡翻 Success，回传终态记录供 caller emit。转发到 tracker
    /// （与 caller 共享同一 `Arc`，故 caller 随后 `take_card()` 读到的即是 Success）。
    pub fn finish_web_search_card(&self) -> Option<ToolCallRecord> {
        self.web_search_tracker
            .as_ref()
            .and_then(|tracker| tracker.finalize_success())
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
            StreamPart::WebSearch { queries, citations } => {
                self.handle_web_search(queries, citations)
            }
            // 出图数据帧：Step 3 才落地为 artifact，此处先占位不处理。
            StreamPart::ImageData { .. } => {}
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
    /// 模型原生生成的图片（Gemini native image gen，任务 07-24）：finish 时聚合到
    /// `GenerateOutput.images`，循环据此落成 assistant 消息级 artifacts。空 = 未出图。
    pub images: Vec<GeneratedImageData>,
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
            images: Vec::new(),
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
        let images = output.images;
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
        result.images = images;
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

pub fn validate_stream_output(
    label: &str,
    policy: AgentStreamPolicy,
    output: &ChatStreamOutput,
) -> Result<(), String> {
    let tool_calls_from_stream = !output.tool_calls.is_empty()
        || !pending_tool_calls_from_dsml(&output.raw_content).is_empty();
    // hosted 出图（Responses `image_generation_call` / Gemini inlineData）时正文本就是
    // 空串——图即答案。此时空正文不是失败，否则整轮报「空助手响应」而图被丢掉。
    if output.content.trim().is_empty() && !output.images.is_empty() {
        return Ok(());
    }
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
            None,
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
            None,
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
    fn image_only_output_validates_despite_empty_text() {
        // 真机形态（xb1520 + gpt-5.6 走 Responses hosted image_generation_call）：正文空串、
        // 无工具调用，但出了图——图即答案。此前 planning 策略会判成「空助手响应」把图丢掉。
        let mut output = ChatStreamOutput::from_generate_output(
            String::new(),
            String::new(),
            String::new(),
            Vec::new(),
            Some("stop".to_string()),
            false,
        );
        output.images = vec![GeneratedImageData {
            mime_type: "image/png".to_string(),
            data: "aGVsbG8=".to_string(),
        }];

        validate_stream_output(
            "Chat tools planning",
            AgentStreamPolicy::PlanningNoDoneUntilNoTools,
            &output,
        )
        .expect("image-only planning output must validate");

        // 没图时仍照旧报错（不放宽既有守门）。
        output.images.clear();
        assert!(validate_stream_output(
            "Chat tools planning",
            AgentStreamPolicy::PlanningNoDoneUntilNoTools,
            &output
        )
        .is_err());
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

        validate_stream_output(
            "Chat stream",
            AgentStreamPolicy::SynthesisDeferEmpty,
            &output,
        )
        .expect("non-empty synthesis should validate");
    }

    /// 内置搜索实时卡（任务 07-23）：首帧开牌 order 落在正文段之前的预留槽、同 id 增量原地
    /// 合并（段只发一次、queries/citations 去重累加）、finalize 翻 Success、take_card 返回终态。
    #[test]
    fn web_search_card_streams_running_before_text_and_merges_by_id() {
        let host = TestHost::default();
        // 预留槽：reasoning=1000 → web_search 卡=1001 → 正文=1002。
        let tracker = WebSearchCardTracker::new(
            1001,
            Some(2),
            ChatMessageSegmentPhase::ToolLoop,
            "Test Provider".to_string(),
        );
        let text_segment = ChatMessageSegment {
            id: "seg_1002_text".to_string(),
            kind: ChatMessageSegmentKind::Text,
            phase: ChatMessageSegmentPhase::ToolLoop,
            order: 1002,
            step_number: Some(1),
            round: Some(2),
            text: None,
            tool_call_id: None,
        };
        let mut sink = AgentStreamSink::new(
            &host,
            "conversation",
            "run",
            "message",
            true,
            Some(text_segment.clone()),
            None,
            None,
            Some(tracker.clone()),
        );

        // 首帧空 = 开牌；随后带查询词；再带来源（含重复 url，应去重）。
        sink.emit(StreamPart::WebSearch {
            queries: Vec::new(),
            citations: Vec::new(),
        })
        .expect("open card");
        sink.emit(StreamPart::WebSearch {
            queries: vec!["kivio release".to_string()],
            citations: Vec::new(),
        })
        .expect("query frame");
        sink.emit(StreamPart::WebSearch {
            queries: vec!["kivio release".to_string()],
            citations: vec![
                WebCitation {
                    title: "A".to_string(),
                    url: "https://a.com".to_string(),
                    ..Default::default()
                },
                WebCitation {
                    title: "dup".to_string(),
                    url: "https://a.com".to_string(),
                    ..Default::default()
                },
            ],
        })
        .expect("citation frame");

        let segments = host.segments.lock().unwrap_or_else(|err| err.into_inner());
        // 段只发一次，order 落预留槽且在正文段之前。
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].order, 1001);
        assert!(segments[0].order < text_segment.order);
        assert_eq!(segments[0].kind, ChatMessageSegmentKind::Tool);
        let card_id = segments[0].tool_call_id.clone().expect("card id");
        drop(segments);

        let records = host.records.lock().unwrap_or_else(|err| err.into_inner());
        // 三帧 → 三条 Running 记录，同 id（前端原地翻牌）。
        assert_eq!(records.len(), 3);
        for record in records.iter() {
            assert_eq!(record.id, card_id);
            assert_eq!(record.name, "web_search");
            assert!(matches!(record.status, ToolCallStatus::Running));
        }
        let last = records.last().expect("last record");
        let structured = last.structured_content.as_ref().expect("structured");
        // 去重后仅 1 条来源。
        assert_eq!(
            structured
                .get("citations")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            structured
                .get("queries")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        drop(records);

        // 流成功结束 → 翻 Success。
        let success = sink
            .finish_web_search_card()
            .expect("started card finalizes");
        assert!(matches!(success.status, ToolCallStatus::Success));
        assert_eq!(success.id, card_id);
        // take_card 返回 Success 终态 + 预留槽段。
        let (record, segment) = tracker.take_card().expect("card present after start");
        assert!(matches!(record.status, ToolCallStatus::Success));
        assert_eq!(record.id, card_id);
        assert_eq!(segment.order, 1001);
    }

    /// 从未开牌（未发生内置搜索 / 取消中途）：take_card 与 finalize_success 均为 None，
    /// 不落卡——保证「web_search_tracker=None 或未搜索」时行为与现状一致。
    #[test]
    fn web_search_card_absent_before_first_frame() {
        let tracker = WebSearchCardTracker::new(
            1001,
            None,
            ChatMessageSegmentPhase::Synthesis,
            "P".to_string(),
        );
        assert!(tracker.take_card().is_none());
        assert!(tracker.finalize_success().is_none());
    }

    /// 无 tracker（非内置模式）：WebSearch part 被静默忽略，不发任何段/记录——逐字节
    /// 兼容现状。
    #[test]
    fn web_search_part_without_tracker_is_ignored() {
        let host = TestHost::default();
        let mut sink = AgentStreamSink::new(&host, "c", "r", "m", true, None, None, None, None);
        sink.emit(StreamPart::WebSearch {
            queries: vec!["q".to_string()],
            citations: Vec::new(),
        })
        .expect("ignored");
        assert!(host
            .segments
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .is_empty());
        assert!(host
            .records
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .is_empty());
    }
}
