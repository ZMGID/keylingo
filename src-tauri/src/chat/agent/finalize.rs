use serde_json::Value;

use crate::chat::model::{PendingToolCall, WebCitation};
use crate::chat::types::{
    ChatMessageSegment, ChatMessageSegmentKind, ChatMessageSegmentPhase, ToolCallRecord,
    ToolCallStatus,
};

use super::host::AgentHost;
use super::loop_::{LoopEnv, RunIds, RunState};
use super::stop::{
    final_assistant_api_message, final_response_from_planning_message_with_images, merge_reasoning,
};
use super::stream::ToolCallDraftTracker;
use super::synthesis::SynthesisCompleted;
use super::types::{AgentPhase, AgentRunConfig, AgentRunResult};

/// Converges the repeated "emit fallback + push api message + build AgentRunResult"
/// blocks in the agent loop. Emit order is fixed: stream delta (with optional
/// segment) -> stream done -> api message push (guarded or always) -> segment
/// append -> result construction.
pub(crate) struct RunResultBuilder<'a> {
    host: &'a dyn AgentHost,
    ids: RunIds<'a>,
    content: String,
    api_reasoning: Option<String>,
    reasoning_tail: Option<String>,
    emit_segment: Option<ChatMessageSegment>,
    append_template: Option<ChatMessageSegment>,
    emit_done_reason: Option<&'static str>,
    push_api_always: bool,
    outcome: &'static str,
    /// 降级兜底的结构化描述（仅降级路径设置）。透传到 `AgentRunResult.degraded`。
    degraded: Option<crate::chat::agent::recovery::DegradedAnswer>,
}

impl<'a> RunResultBuilder<'a> {
    pub(crate) fn new(host: &'a dyn AgentHost, ids: RunIds<'a>, content: String) -> Self {
        Self {
            host,
            ids,
            content,
            api_reasoning: None,
            reasoning_tail: None,
            emit_segment: None,
            append_template: None,
            emit_done_reason: None,
            push_api_always: false,
            outcome: "completed",
            degraded: None,
        }
    }

    /// Use `template` both as the segment attached to the emitted delta and as
    /// the template appended to the segment builder during `finish`.
    pub(crate) fn segment(mut self, template: &ChatMessageSegment) -> Self {
        self.emit_segment = Some(template.clone());
        self.append_template = Some(template.clone());
        self
    }

    /// Attach an already-appended segment to the emitted delta without
    /// re-appending it during `finish`.
    pub(crate) fn emit_segment_opt(mut self, segment: Option<ChatMessageSegment>) -> Self {
        self.emit_segment = segment;
        self
    }

    pub(crate) fn api_reasoning(mut self, reasoning: Option<String>) -> Self {
        self.api_reasoning = reasoning;
        self
    }

    pub(crate) fn reasoning_tail(mut self, reasoning: Option<String>) -> Self {
        self.reasoning_tail = reasoning;
        self
    }

    pub(crate) fn emit_done(mut self, reason: &'static str) -> Self {
        self.emit_done_reason = Some(reason);
        self
    }

    pub(crate) fn push_api_always(mut self) -> Self {
        self.push_api_always = true;
        self
    }

    pub(crate) fn outcome(mut self, outcome: &'static str) -> Self {
        self.outcome = outcome;
        self
    }

    /// 挂上降级兜底的结构化描述，让前端渲染成独立错误卡片。
    pub(crate) fn degraded(
        mut self,
        degraded: Option<crate::chat::agent::recovery::DegradedAnswer>,
    ) -> Self {
        self.degraded = degraded;
        self
    }

    fn emit(&self) {
        if self.emit_done_reason.is_some() {
            self.host.emit_stream_delta(
                self.ids.conversation_id,
                self.ids.run_id,
                self.ids.message_id,
                &self.content,
                None,
                self.emit_segment.as_ref(),
            );
        }
    }

    fn push_api_message(&self, generated_api_messages: &mut Vec<Value>) {
        if self.push_api_always || !generated_api_messages.is_empty() {
            generated_api_messages.push(final_assistant_api_message(
                &self.content,
                self.api_reasoning.as_deref(),
            ));
        }
    }

    /// Emit + api push only, for fallbacks that flow into the common loop tail
    /// (segment append and result construction happen there). Returns the
    /// fallback content.
    pub(crate) fn emit_and_record(self, generated_api_messages: &mut Vec<Value>) -> String {
        self.emit();
        self.push_api_message(generated_api_messages);
        self.content
    }

    /// Emit + api push + segment append + AgentRunResult construction, for
    /// fallbacks that end the run immediately.
    pub(crate) fn finish(
        self,
        mut segment_builder: SegmentBuilder,
        planning_reasoning_parts: &[String],
        tool_records: Vec<ToolCallRecord>,
        mut generated_api_messages: Vec<Value>,
    ) -> AgentRunResult {
        self.emit();
        self.push_api_message(&mut generated_api_messages);
        if let Some(template) = &self.append_template {
            segment_builder.append_text_from_template(template, self.content.clone());
        }
        AgentRunResult {
            content: self.content,
            reasoning: merge_reasoning(planning_reasoning_parts, self.reasoning_tail),
            tool_records,
            segments: segment_builder.all(),
            api_messages: generated_api_messages,
            stream_outcome: self.outcome.to_string(),
            usage: None,
            last_step_usage: None,
            compacted_history: None,
            compaction_boundary: None,
            compaction_summary: None,
            images: Vec::new(),
            degraded: self.degraded,
        }
    }
}

pub(crate) struct SegmentBuilder {
    next_order: u32,
    segments: Vec<ChatMessageSegment>,
}

impl Default for SegmentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SegmentBuilder {
    pub(crate) fn new() -> Self {
        Self {
            next_order: 1000,
            segments: Vec::new(),
        }
    }

    pub(crate) fn reserve(
        &mut self,
        kind: ChatMessageSegmentKind,
        phase: ChatMessageSegmentPhase,
        step_number: Option<u8>,
        round: Option<u32>,
        suffix: &str,
    ) -> ChatMessageSegment {
        let segment = ChatMessageSegment {
            id: format!("seg_{}_{}", self.next_order, suffix),
            kind,
            phase,
            order: self.next_order,
            step_number,
            round,
            text: None,
            tool_call_id: None,
        };
        self.next_order = self.next_order.saturating_add(1);
        segment
    }

    pub(crate) fn append_text_from_template(
        &mut self,
        template: &ChatMessageSegment,
        text: impl Into<String>,
    ) -> Option<ChatMessageSegment> {
        let text = text.into();
        if text.trim().is_empty() {
            return None;
        }
        let mut segment = template.clone();
        segment.text = Some(text);
        self.segments.push(segment.clone());
        Some(segment)
    }

    pub(crate) fn append_tool_calls(
        &mut self,
        phase: ChatMessageSegmentPhase,
        step_number: Option<u8>,
        round: u32,
        calls: &[PendingToolCall],
    ) -> Vec<ChatMessageSegment> {
        let mut segments = Vec::new();
        for call in calls {
            let segment = ChatMessageSegment {
                id: format!("seg_{}_tool_{}", self.next_order, call.id),
                kind: ChatMessageSegmentKind::Tool,
                phase: phase.clone(),
                order: self.next_order,
                step_number,
                round: Some(round),
                text: None,
                tool_call_id: Some(call.id.clone()),
            };
            self.next_order = self.next_order.saturating_add(1);
            self.segments.push(segment.clone());
            segments.push(segment);
        }
        segments
    }

    pub(crate) fn append_existing_segments(
        &mut self,
        mut segments: Vec<ChatMessageSegment>,
    ) -> Vec<ChatMessageSegment> {
        for segment in &segments {
            self.next_order = self.next_order.max(segment.order.saturating_add(1));
        }
        self.segments.extend(segments.iter().cloned());
        segments.sort_by_key(|segment| segment.order);
        segments
    }

    pub(crate) fn next_order(&self) -> u32 {
        self.next_order
    }

    /// 只吞一个 order 号、不造段——给流式实时卡（内置 web_search）在 reasoning 与正文之间
    /// 预留位置槽（任务 07-23）。未搜索时该 order 号留空洞无害：前端按 order sort、后端按
    /// `max()` 取下一个，均不要求 order 连续。
    pub(crate) fn reserve_order(&mut self) -> u32 {
        let order = self.next_order;
        self.next_order = self.next_order.saturating_add(1);
        order
    }

    /// Borrow the segments accumulated so far without consuming the builder.
    /// Used by the loop's per-round crash-safety checkpoint to snapshot the
    /// in-progress assistant message; `all()` (which moves) still produces the
    /// final segment list at finalize time.
    pub(crate) fn segments(&self) -> &[ChatMessageSegment] {
        &self.segments
    }

    pub(crate) fn all(self) -> Vec<ChatMessageSegment> {
        self.segments
    }
}

pub(crate) fn segment_phase_for_agent_phase(phase: AgentPhase) -> ChatMessageSegmentPhase {
    match phase {
        AgentPhase::Plain => ChatMessageSegmentPhase::Plain,
        AgentPhase::Synthesis => ChatMessageSegmentPhase::Synthesis,
        AgentPhase::ToolLoop => ChatMessageSegmentPhase::ToolLoop,
    }
}

/// Final result when planning ended with a natural answer (no tool calls left).
pub(crate) fn finalize_planning_final(
    env: &LoopEnv<'_>,
    state: &mut RunState,
    message: Value,
) -> Result<AgentRunResult, String> {
    let config = env.config;
    let (response, reasoning) = final_response_from_planning_message_with_images(
        &message,
        &state.planning_reasoning_parts,
        !state.generated_images.is_empty(),
    )?;
    if !state.planning_final_streamed {
        env.host.emit_stream_delta(
            &config.conversation_id,
            &config.run_id,
            &config.message_id,
            &response,
            None,
            None,
        );
    }
    if !state.generated_api_messages.is_empty() {
        state.generated_api_messages.push(message);
    }
    Ok(AgentRunResult {
        content: response,
        reasoning,
        tool_records: std::mem::take(&mut state.tool_records),
        segments: std::mem::take(&mut state.segment_builder).all(),
        api_messages: std::mem::take(&mut state.generated_api_messages),
        stream_outcome: "completed".to_string(),
        usage: None,
        last_step_usage: None,
        compacted_history: None,
        compaction_boundary: None,
        compaction_summary: None,
        images: Vec::new(),
        degraded: state.degraded.take(),
    })
}

/// Common loop tail: append final segments, push the closing step and build the
/// completed run result.
pub(crate) fn finalize_completed(
    _env: &LoopEnv<'_>,
    state: &mut RunState,
    completed: SynthesisCompleted,
) -> AgentRunResult {
    let SynthesisCompleted {
        response,
        reasoning,
        response_reasoning,
        response_segment,
        response_reasoning_segment,
    } = completed;
    if !response.trim().is_empty() {
        state
            .segment_builder
            .append_text_from_template(&response_segment, response.clone());
    }
    if let Some(reasoning_part) = response_reasoning
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        state
            .segment_builder
            .append_text_from_template(&response_reasoning_segment, reasoning_part);
    }

    AgentRunResult {
        content: response,
        reasoning,
        tool_records: std::mem::take(&mut state.tool_records),
        segments: std::mem::take(&mut state.segment_builder).all(),
        api_messages: std::mem::take(&mut state.generated_api_messages),
        stream_outcome: "completed".to_string(),
        usage: None,
        last_step_usage: None,
        compacted_history: None,
        compaction_boundary: None,
        compaction_summary: None,
        images: Vec::new(),
        degraded: state.degraded.take(),
    }
}

pub(crate) fn empty_synthesis_fallback_response(language: &str) -> String {
    if language.starts_with("zh") {
        "工具调用已经完成，但模型没有返回最终总结。上方工具结果已保存在本轮回复中，你可以继续追问，或让我重新生成总结。".to_string()
    } else {
        "The tool calls completed, but the model did not return a final summary. The tool results above were saved with this reply; you can continue from them or regenerate the summary.".to_string()
    }
}

pub(crate) fn synthesis_failed_fallback_response(language: &str) -> String {
    if language.starts_with("zh") {
        "最终总结生成失败(可能是模型供应商内容审核拦截)。上方工具结果已保存在本轮回复中,你可以继续追问、让我重新生成,或更换聊天模型再试。".to_string()
    } else {
        "Final summary generation failed (possibly provider content moderation). The tool results above were saved with this reply; you can continue from them, regenerate, or switch the chat model and retry.".to_string()
    }
}

pub(crate) fn tool_planning_failed_fallback_response(language: &str) -> String {
    if language.starts_with("zh") {
        "工具调用参数生成失败，这一步还没有真正执行写入。主对话已保留，你可以让我缩小范围、改用补丁，或重新生成。".to_string()
    } else {
        "Tool-call argument generation failed before the write actually ran. This conversation was preserved; you can ask me to narrow the scope, use a patch, or regenerate.".to_string()
    }
}

pub(crate) fn stopped_generation_content(language: &str) -> String {
    if language.starts_with("zh") {
        "已停止生成。".to_string()
    } else {
        "Generation stopped.".to_string()
    }
}

pub(crate) fn tool_planning_failed_run_result(
    host: &dyn AgentHost,
    config: &AgentRunConfig<'_>,
    mut segment_builder: SegmentBuilder,
    planning_text_segment: ChatMessageSegment,
    tool_draft_tracker: ToolCallDraftTracker,
    planning_reasoning_parts: &[String],
    generated_api_messages: Vec<Value>,
    error: String,
) -> AgentRunResult {
    let failed_records = tool_draft_tracker.mark_error(&error);
    for record in &failed_records {
        host.emit_tool_record(
            &config.conversation_id,
            &config.run_id,
            &config.message_id,
            record,
        );
    }

    segment_builder.append_existing_segments(tool_draft_tracker.segments());
    let content = tool_planning_failed_fallback_response(&config.language);
    let mut final_segment = planning_text_segment;
    final_segment.phase = ChatMessageSegmentPhase::Synthesis;
    final_segment.round = None;
    let appended_segment =
        segment_builder.append_text_from_template(&final_segment, content.clone());

    RunResultBuilder::new(
        host,
        RunIds {
            conversation_id: &config.conversation_id,
            run_id: &config.run_id,
            message_id: &config.message_id,
        },
        content,
    )
    .emit_segment_opt(appended_segment)
    .emit_done("done")
    .push_api_always()
    .outcome("error")
    .finish(
        segment_builder,
        planning_reasoning_parts,
        failed_records,
        generated_api_messages,
    )
}

pub(crate) fn cancelled_tool_round_run_result(
    language: &str,
    planning_reasoning_parts: &[String],
    tool_records: Vec<ToolCallRecord>,
    mut segments: Vec<ChatMessageSegment>,
    mut generated_api_messages: Vec<Value>,
) -> AgentRunResult {
    let stopped_content = stopped_generation_content(language);
    let next_order = segments
        .iter()
        .map(|segment| segment.order)
        .max()
        .unwrap_or(999)
        .saturating_add(1);
    segments.push(ChatMessageSegment {
        id: format!("seg_{}_cancelled_synthesis", next_order),
        kind: ChatMessageSegmentKind::Text,
        phase: ChatMessageSegmentPhase::Synthesis,
        order: next_order,
        step_number: None,
        round: None,
        text: Some(stopped_content.clone()),
        tool_call_id: None,
    });
    if !generated_api_messages.is_empty() {
        generated_api_messages.push(final_assistant_api_message(&stopped_content, None));
    }
    AgentRunResult {
        content: stopped_content,
        reasoning: merge_reasoning(planning_reasoning_parts, None),
        tool_records,
        segments,
        api_messages: generated_api_messages,
        stream_outcome: "cancelled".to_string(),
        usage: None,
        last_step_usage: None,
        compacted_history: None,
        compaction_boundary: None,
        compaction_summary: None,
        images: Vec::new(),
        degraded: None,
    }
}

/// Build a cancelled `AgentRunResult` from the loop's accumulated `state`.
/// Used by the planning/loop-top cancellation paths (no partial streamed
/// answer to preserve) so they end with `Ok(cancelled_result)` carrying the
/// tool records / segments / api messages gathered up to the cancel point —
/// mirroring the tool-round cancellation path so the whole turn is persisted
/// instead of being dropped via a bare `Err("cancelled")`.
pub(crate) fn cancelled_run_result_from_state(
    env: &LoopEnv<'_>,
    state: &mut RunState,
) -> AgentRunResult {
    let config = env.config;
    cancelled_tool_round_run_result(
        &config.language,
        &state.planning_reasoning_parts,
        std::mem::take(&mut state.tool_records),
        std::mem::take(&mut state.segment_builder).all(),
        std::mem::take(&mut state.generated_api_messages),
    )
}

/// 构造一张「内置联网搜索」工具卡的 `ToolCallRecord`。流式实时卡（Running）与流结束合成卡
/// （Success / 非流式路径）共用此构造，保证 `structured_content` 形状一致——前端按
/// `type == "builtin_web_search"` 路由、按 record id 幂等 merge（先 Running 后 Success 原地翻牌）。
/// `status` 参数化即为此：同一 id、同一形状，只切状态。
pub(crate) fn build_web_search_record(
    id: &str,
    provider_label: &str,
    queries: &[String],
    citations: &[WebCitation],
    status: ToolCallStatus,
    round: Option<u32>,
) -> ToolCallRecord {
    // structured_content 对齐前端内置搜索来源卡读取的形状（type 作路由标记）。
    let structured = serde_json::json!({
        "type": "builtin_web_search",
        "provider": provider_label,
        "queries": queries,
        "citations": citations,
    });
    // 折叠行「目标」显示查询词：ToolCallBlock 从 arguments.query 取。
    let arguments = serde_json::json!({ "query": queries.join("; ") }).to_string();
    // 纯文本兜底：前端旧版本不识别 structured 时仍能看到来源列表。
    let mut preview = String::new();
    for citation in citations {
        preview.push_str(&format!("- {} — {}\n", citation.title, citation.url));
    }
    ToolCallRecord {
        id: id.to_string(),
        name: "web_search".to_string(),
        source: "native".to_string(),
        server_id: None,
        arguments,
        status,
        result_preview: if preview.trim().is_empty() {
            None
        } else {
            Some(preview.trim().to_string())
        },
        error: None,
        duration_ms: None,
        started_at: None,
        completed_at: None,
        round: round.unwrap_or(0),
        sensitive: false,
        artifacts: Vec::new(),
        trace_id: None,
        span_id: None,
        structured_content: Some(structured),
    }
}

/// 构造内置搜索卡对应的 Tool 段。`order` 决定它在 transcript 里的位置（实时卡用预留槽 →
/// 答案之前）；`step_number=None` 让前端与正文段纯按 order 排序（见 `compareTimelineSegments`）。
pub(crate) fn build_web_search_segment(
    order: u32,
    id: &str,
    phase: ChatMessageSegmentPhase,
    round: Option<u32>,
) -> ChatMessageSegment {
    ChatMessageSegment {
        id: format!("seg_{}_tool_{}", order, id),
        kind: ChatMessageSegmentKind::Tool,
        phase,
        order,
        step_number: None,
        round,
        text: None,
        tool_call_id: Some(id.to_string()),
    }
}
