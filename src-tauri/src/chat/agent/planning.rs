use serde_json::Value;

use crate::chat::model::{
    generate_request_from_openai_messages, stream_with_chat_provider,
    GenerateOptions, GenerateOutput, GenerateRequest, GenerateRequestContext, ModelError,
    PendingToolCall, StreamPart, StreamSink,
};
use crate::chat::types::{ChatMessageSegment, ChatMessageSegmentKind, ChatMessageSegmentPhase};
use crate::mcp::ChatToolDefinition;

use super::finalize::{
    cancelled_run_result_from_state, cancelled_tool_round_run_result,
    tool_planning_failed_run_result, RunResultBuilder,
};
use super::host::AgentHost;
use super::loop_::{LoopEnv, RunState};
use super::rounds::visible_tool_segment_calls;
use super::stop::{
    assistant_content_from_api_message, extract_reasoning_content, extract_tool_calls,
    is_tools_unsupported_error, sanitize_assistant_text_response,
};
use super::stream::{
    validate_stream_output, AgentStreamSink, ChatStreamOutput, ToolCallDraftTracker,
    WebSearchCardTracker,
};
use super::types::{AgentRunResult, AgentStreamPolicy};

pub(crate) struct ChatPlanningStep {
    pub(crate) message: Value,
    pub(crate) streamed: bool,
}

/// 流式响应中途断包后，重连**同一条流式请求**的次数上限。
///
/// 为什么不是降级到非流式（这是本常量取代的旧行为）：
/// - 非流式意味着彻底丢掉已经流出来的部分，还要从头重新生成完整回答；
/// - 非流式带总超时，长思考模型（high reasoning + 大 max_output_tokens）结构性跑不完
///   —— 实测流式跑 135s 断包后回落非流式，3 次 60s 全超，白等 195s（现已放宽到
///   `api::CHAT_COMPLETION_REQUEST_TIMEOUT`，但方向仍然是错的）；
/// - 官方客户端的做法就是重连流式：Codex CLI 断流后重连最多 5 次，且在 0.130.0
///   直接删掉了非流式（`wire_api = "chat"`）这条回退路径。
///
/// SSE 没有续传能力（无 offset / sequence），所以"重连"必然等于"重跑"——业界共识是
/// 对话客户端做有界重试 + 退避就够，不值得为此上 Redis buffer / Last-Event-ID 那套。
/// 取 2 次而非 Codex 的 5 次：每次重试都要重传整个请求体，2 次已覆盖偶发断包。
const STREAM_INTERRUPT_RETRIES: u32 = 2;

/// 断流重连前的退避基数（第 n 次重试等 n × 该值）。
const STREAM_INTERRUPT_BACKOFF: std::time::Duration = std::time::Duration::from_secs(1);

pub(crate) struct PlannedToolRound {
    pub(crate) message: Value,
    pub(crate) tool_calls: Vec<PendingToolCall>,
}

pub(crate) enum PlanningStepOutcome {
    /// `state.planning_final_message` / `planning_final_streamed` were written;
    /// the skeleton breaks out of the tool loop.
    FinalAnswer,
    /// Planning produced tool calls; the skeleton hands them to `run_tool_round`.
    ToolCalls(PlannedToolRound),
    /// `state.tools` was narrowed to skill-native tools; the skeleton retries.
    RetryWithSkillTools,
    /// Planning returned an empty assistant response (no text, no tool calls) —
    /// flaky gateways do this intermittently. Retried once via the skeleton's
    /// `continue`; a second empty lands in FinalAnswer and fails at finalize
    /// with the existing "empty assistant response" error.
    RetryEmptyResponse,
    /// Provider rejected tools; `state.provider_tools_unsupported` was set and a
    /// step was pushed. The skeleton breaks out of the tool loop.
    ToolsUnsupported,
    /// Tool-call argument drafting failed mid-stream; the run ends immediately.
    DraftFailed(AgentRunResult),
    /// A later-round planning call hard-failed but tool results already exist;
    /// the run ends with those gathered results instead of bubbling an error
    /// (统一恢复:不空手而归).
    Recovered(AgentRunResult),
    /// Streaming was cancelled after partial plain-text output (no tool drafts
    /// started); the run ends immediately preserving the generated text. The
    /// stream layer already emitted the single done("cancelled") event.
    Cancelled(AgentRunResult),
}

pub(crate) async fn planning_step(
    env: &LoopEnv<'_>,
    state: &mut RunState,
    round: u32,
) -> Result<PlanningStepOutcome, String> {
    let config = env.config;
    let host = env.host;
    let step_number = state.step_number;
    // 循环内上下文治理：超限时先 snip / 摘要，得到本步发送视图（未超限时为原样 clone）。
    let send_messages = super::compaction::maybe_compact_send_view(env, state).await;

    // Gap 2（Layer 3 anti-thrashing）：连续多轮「需要压缩但压不下去」时（摘要调用反复失败/为空），
    // 不要再用必然超窗的发送视图去打规划调用、再失败——而是用已收集的工具结果优雅收尾。
    // 复用 recovery 的确定性降级路径（`assemble_results_from_tool_records`），不另造终止通道。
    if state.compaction_unresolved_rounds >= super::loop_::COMPACTION_THRASH_LIMIT {
        eprintln!(
            "Chat context compaction could not reduce context after {} rounds; ending turn with gathered results (anti-thrashing)",
            state.compaction_unresolved_rounds
        );
        let kind = crate::chat::agent::recovery::FailureKind::ContextOverflow;
        let content = crate::chat::agent::recovery::assemble_results_from_tool_records(
            &state.tool_records,
            &config.language,
            kind,
        );
        // 有可兜底素材 → 用降级摘要收尾；没有素材（content 为空）→ 退回去敏/超长静态文案，
        // 但仍以「已收尾」结束本轮，绝不再循环触发压缩失败。
        let content = if content.trim().is_empty() {
            crate::chat::agent::recovery::overflow_static_message(&config.language)
        } else {
            content
        };
        // 为降级文案预约一个文本 segment，让它在 transcript 里正常渲染（与其它收尾路径一致）。
        let degrade_segment = state.segment_builder.reserve(
            ChatMessageSegmentKind::Text,
            ChatMessageSegmentPhase::Plain,
            Some(step_number),
            Some(round),
            &format!("step_{step_number}_compaction_thrash"),
        );
        return Ok(PlanningStepOutcome::Recovered(
            RunResultBuilder::new(host, env.ids(), content)
                .segment(&degrade_segment)
                .emit_done("done")
                .outcome("compaction_thrash")
                .finish(
                    std::mem::take(&mut state.segment_builder),
                    &state.planning_reasoning_parts,
                    std::mem::take(&mut state.tool_records),
                    std::mem::take(&mut state.generated_api_messages),
                ),
        ));
    }

    let active_tools = state.tools.clone();
    let stream_policy = AgentStreamPolicy::PlanningNoDoneUntilNoTools;
    let planning_reasoning_segment = state.segment_builder.reserve(
        ChatMessageSegmentKind::Reasoning,
        ChatMessageSegmentPhase::ToolLoop,
        Some(step_number),
        Some(round),
        &format!("step_{step_number}_reasoning"),
    );
    // 在 reasoning 与正文之间预留内置搜索实时卡的 order 槽（任务 07-23）：搜索发生时卡插到
    // 这里 → 渲染在答案文本之前；未搜索则该 order 留空洞无害。
    let web_search_order = state.segment_builder.reserve_order();
    let planning_text_segment = state.segment_builder.reserve(
        ChatMessageSegmentKind::Text,
        ChatMessageSegmentPhase::ToolLoop,
        Some(step_number),
        Some(round),
        &format!("step_{step_number}_text"),
    );
    let planning_tool_drafts = ToolCallDraftTracker::new(
        active_tools.clone(),
        round,
        Some(step_number),
        state.segment_builder.next_order(),
    );
    // 内置搜索实时卡追踪器：仅在会话 Builtin 模式且模型支持时建（否则 None，走现状路径）。
    let planning_web_search_tracker = if config.builtin_web_search_active() {
        Some(WebSearchCardTracker::new(
            web_search_order,
            Some(round),
            ChatMessageSegmentPhase::ToolLoop,
            config.provider.name.clone(),
        ))
    } else {
        None
    };
    // 内置搜索由实时卡追踪器边流边合成（take_card 落 Success 终态卡）。
    let mut interrupt_attempt = 0u32;
    let planning_result = loop {
        match stream_scoped_chat_completion_inner(
            config.state,
            host,
            &config.provider,
            &config.model,
            send_messages.clone(),
            Some(&active_tools),
            config.retry_attempts,
            config.thinking_enabled,
            config.thinking_level.clone(),
            config.builtin_web_search_active(),
            config.max_output_tokens,
            &config.conversation_id,
            &config.run_id,
            &config.message_id,
            config.generation,
            "Chat tools planning",
            stream_policy,
            Some(planning_text_segment.clone()),
            Some(planning_reasoning_segment.clone()),
            Some(planning_tool_drafts.clone()),
            planning_web_search_tracker.clone(),
        )
        .await
        {
            Ok(mut stream) => {
                if stream.cancelled {
                    let partial = sanitize_assistant_text_response(&stream.content);
                    if partial.trim().is_empty() || planning_tool_drafts.has_started() {
                        // No preservable partial answer (empty text, or tool-arg
                        // drafting had started and is now incomplete). The stream
                        // layer already emitted the single done("cancelled") event,
                        // so build the cancelled result WITHOUT re-emitting — but
                        // still end with Ok(cancelled_result) carrying the rounds
                        // already accumulated, so the turn is persisted instead of
                        // dropped via a bare Err("cancelled").
                        return Ok(PlanningStepOutcome::Cancelled(
                            cancelled_tool_round_run_result(
                                &config.language,
                                &state.planning_reasoning_parts,
                                std::mem::take(&mut state.tool_records),
                                std::mem::take(&mut state.segment_builder).all(),
                                std::mem::take(&mut state.generated_api_messages),
                            ),
                        ));
                    }
                    // Partial plain text was already streamed to the frontend and the
                    // stream layer already emitted the single done("cancelled") event;
                    // preserve the generated text instead of dropping the whole turn.
                    // Append the reasoning segment first (its reserved order is lower
                    // than the text segment's) so the persisted timeline keeps reasoning
                    // above the answer; otherwise normalize_assistant_segments would add
                    // a trailing reasoning segment that renders below the text.
                    if let Some(reasoning_text) = stream
                        .reasoning
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        let mut reasoning_segment = planning_reasoning_segment.clone();
                        reasoning_segment.phase = ChatMessageSegmentPhase::Plain;
                        state.segment_builder.append_text_from_template(
                            &reasoning_segment,
                            reasoning_text.to_string(),
                        );
                    }
                    let mut segment = planning_text_segment.clone();
                    segment.phase = ChatMessageSegmentPhase::Plain;
                    return Ok(PlanningStepOutcome::Cancelled(
                        RunResultBuilder::new(host, env.ids(), partial)
                            .segment(&segment)
                            .api_reasoning(stream.reasoning.clone())
                            .reasoning_tail(stream.reasoning)
                            .outcome("cancelled")
                            .finish(
                                std::mem::take(&mut state.segment_builder),
                                &state.planning_reasoning_parts,
                                std::mem::take(&mut state.tool_records),
                                std::mem::take(&mut state.generated_api_messages),
                            ),
                    ));
                }
                state.merge_usage(stream.usage.clone());
                state.generated_images.append(&mut stream.images);
                break Ok(ChatPlanningStep {
                    message: stream.to_openai_compatible_message(),
                    streamed: true,
                });
            }
            Err(err) if planning_tool_drafts.has_started() => {
                eprintln!(
                    "Chat tools planning stream interrupted while generating tool arguments; surfacing tool draft error without retry: {}",
                    err
                );
                return Ok(PlanningStepOutcome::DraftFailed(
                    tool_planning_failed_run_result(
                        host,
                        config,
                        std::mem::take(&mut state.segment_builder),
                        planning_text_segment.clone(),
                        planning_tool_drafts,
                        &state.planning_reasoning_parts,
                        std::mem::take(&mut state.generated_api_messages),
                        err.to_string(),
                    ),
                ));
            }
            Err(err)
                if err.is_stream_read_interrupted()
                    && interrupt_attempt < STREAM_INTERRUPT_RETRIES =>
            {
                interrupt_attempt += 1;
                eprintln!(
                    "Chat tools planning stream interrupted; reconnecting the stream ({interrupt_attempt}/{STREAM_INTERRUPT_RETRIES}): {err}"
                );
                // 退避必须接取消：否则用户点停止后要等满退避 + 下一次完整流。
                tokio::select! {
                    _ = tokio::time::sleep(STREAM_INTERRUPT_BACKOFF * interrupt_attempt) => {}
                    _ = host.wait_for_generation_inactive(&config.conversation_id, config.generation) => {
                        return Ok(PlanningStepOutcome::Cancelled(
                            cancelled_run_result_from_state(env, state),
                        ));
                    }
                }
                continue;
            }
            Err(err) => break Err(err.to_string()),
        }
    };
    let message = match planning_result {
        Ok(step) => {
            state.planning_final_streamed = step.streamed;
            step.message
        }
        Err(err) if is_tools_unsupported_error(&err) => {
            let skill_only: Vec<ChatToolDefinition> = state
                .tools
                .iter()
                .filter(|tool| tool.source == "skill")
                .cloned()
                .collect();
            if !state.tried_skill_only_tools
                && skill_only.len() < state.tools.len()
                && !skill_only.is_empty()
            {
                eprintln!(
                    "Chat provider {} rejected tools; retrying with skill-native tools only",
                    config.provider.id
                );
                state.tools = skill_only;
                state.tried_skill_only_tools = true;
                return Ok(PlanningStepOutcome::RetryWithSkillTools);
            }
            eprintln!(
                "Chat provider {} rejected tools; falling back to plain chat",
                config.provider.id
            );
            state.provider_tools_unsupported = true;
            return Ok(PlanningStepOutcome::ToolsUnsupported);
        }
        Err(err) => {
            // 统一恢复:多轮中途 planning 调用硬失败时,若已收集到工具结果,不要让
            // 整轮报错丢弃成果——走与 synthesis 同一条恢复阶梯(去敏重做 → 确定性兜底),
            // 而非直接堆原始结果。
            if !state.tool_records.is_empty() {
                let content = super::synthesis::recover_synthesis(env, state, &err).await;
                if !content.trim().is_empty() {
                    eprintln!("Chat planning call failed mid-run; recovered: {err}");
                    // 降级文案是本轮的**正文**，段 phase 必须是 Synthesis（不能留 ToolLoop）：
                    // content_from_segments 只认 Plain|Synthesis，留 ToolLoop 会让
                    // normalize_assistant_segments 以为正文没落段而再补一条，正文渲染两遍。
                    let mut segment = planning_text_segment.clone();
                    segment.phase = ChatMessageSegmentPhase::Synthesis;
                    return Ok(PlanningStepOutcome::Recovered(
                        RunResultBuilder::new(host, env.ids(), content)
                            .segment(&segment)
                            .emit_done("done")
                            .outcome("recovered")
                            .degraded(state.degraded.take())
                            .finish(
                                std::mem::take(&mut state.segment_builder),
                                &state.planning_reasoning_parts,
                                std::mem::take(&mut state.tool_records),
                                std::mem::take(&mut state.generated_api_messages),
                            ),
                    ));
                }
            }
            return Err(err);
        }
    };
    // 内置搜索：实时卡追踪器边流边合成（take_card 落 Success 终态卡）。
    if let Some(tracker) = planning_web_search_tracker.as_ref() {
        if let Some((record, segment)) = tracker.take_card() {
            state
                .segment_builder
                .append_existing_segments(vec![segment]);
            state.tool_records.push(record);
        }
    }
    let tool_calls = extract_tool_calls(&message);
    if tool_calls.is_empty() {
        let response =
            sanitize_assistant_text_response(&assistant_content_from_api_message(&message));
        // 空响应重试（一次）：正文空 + 无工具调用 = 抽风网关的典型症状（HTTP 200 但
        // 正文什么都没给，可能残留一段 reasoning）。这种消息走到 finalize 必报
        // "empty assistant response"——与其断轮不如原地重试一次；再空则照旧报错。
        // 例外：本轮出了图（hosted image generation，如 Responses 的 image_generation_call）
        // 时正文本就是空串——图即答案，重试只会再生成一张，必须放行。
        if response.trim().is_empty()
            && state.generated_images.is_empty()
            && !state.planning_empty_retried
        {
            state.planning_empty_retried = true;
            eprintln!("Chat tools planning returned an empty response; retrying once");
            return Ok(PlanningStepOutcome::RetryEmptyResponse);
        }
        if !response.trim().is_empty() {
            let mut segment = planning_text_segment.clone();
            segment.phase = ChatMessageSegmentPhase::Plain;
            if state.planning_final_streamed {
                host.emit_stream_delta(
                    &config.conversation_id,
                    &config.run_id,
                    &config.message_id,
                    "",
                    None,
                    Some(&segment),
                );
            }
            state
                .segment_builder
                .append_text_from_template(&segment, response);
        }
        if let Some(reasoning) = extract_reasoning_content(&message) {
            let mut segment = planning_reasoning_segment.clone();
            segment.phase = ChatMessageSegmentPhase::Plain;
            if state.planning_final_streamed {
                host.emit_stream_delta(
                    &config.conversation_id,
                    &config.run_id,
                    &config.message_id,
                    "",
                    None,
                    Some(&segment),
                );
            }
            state
                .segment_builder
                .append_text_from_template(&segment, reasoning);
        }
        state.planning_final_message = Some(message);
        return Ok(PlanningStepOutcome::FinalAnswer);
    }
    state.planning_final_streamed = false;
    let planning_text =
        sanitize_assistant_text_response(&assistant_content_from_api_message(&message));
    if !planning_text.trim().is_empty() {
        state
            .segment_builder
            .append_text_from_template(&planning_text_segment, planning_text);
    }
    if let Some(reasoning) = extract_reasoning_content(&message) {
        state
            .segment_builder
            .append_text_from_template(&planning_reasoning_segment, reasoning.clone());
        state.planning_reasoning_parts.push(reasoning);
    }

    let visible_tool_calls =
        visible_tool_segment_calls(&state.tools, &state.blocked_tool_calls, &tool_calls);
    let draft_tool_segments = planning_tool_drafts.segments();
    if draft_tool_segments.is_empty() {
        let tool_segments = state.segment_builder.append_tool_calls(
            ChatMessageSegmentPhase::ToolLoop,
            Some(step_number),
            round,
            &visible_tool_calls,
        );
        for segment in &tool_segments {
            host.emit_stream_delta(
                &config.conversation_id,
                &config.run_id,
                &config.message_id,
                "",
                None,
                Some(segment),
            );
        }
    } else {
        state
            .segment_builder
            .append_existing_segments(draft_tool_segments);
    }
    Ok(PlanningStepOutcome::ToolCalls(PlannedToolRound {
        message,
        tool_calls,
    }))
}

/// 模型调用并同时返回 provider 报告的 usage，
/// 供循环把每次模型调用的 token 消耗累计进 AgentRunResult。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn call_chat_completion_message_with_usage(
    state: &crate::state::AppState,
    provider: &crate::settings::ModelProvider,
    model: &str,
    messages: Vec<Value>,
    tools: Option<&[ChatToolDefinition]>,
    retry_attempts: usize,
    thinking_enabled: bool,
    thinking_level: Option<String>,
    builtin_web_search: bool,
    max_output_tokens: u32,
    conversation_id: &str,
    message_id: &str,
    label: &str,
) -> Result<(Value, Option<crate::chat::model::ModelUsage>), String> {
    let (message, usage, _web_search, _images) = call_chat_completion_output_with_usage(
        state,
        provider,
        model,
        messages,
        tools,
        retry_attempts,
        thinking_enabled,
        thinking_level,
        builtin_web_search,
        max_output_tokens,
        conversation_id,
        message_id,
        label,
    )
    .await?;
    Ok((message, usage))
}

/// 同 `call_chat_completion_message_with_usage`，但额外回传适配器解析出的内置搜索引用
/// （`GenerateOutput.web_search`）。非流式答案路径用它把内置搜索可视化成工具卡（任务 07-23）。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn call_chat_completion_output_with_usage(
    state: &crate::state::AppState,
    provider: &crate::settings::ModelProvider,
    model: &str,
    messages: Vec<Value>,
    tools: Option<&[ChatToolDefinition]>,
    retry_attempts: usize,
    thinking_enabled: bool,
    thinking_level: Option<String>,
    builtin_web_search: bool,
    max_output_tokens: u32,
    conversation_id: &str,
    message_id: &str,
    label: &str,
) -> Result<
    (
        Value,
        Option<crate::chat::model::ModelUsage>,
        Option<crate::chat::model::BuiltinWebSearch>,
        Vec<crate::chat::model::GeneratedImageData>,
    ),
    String,
> {
    let request = generate_request_from_openai_messages(
        model,
        messages,
        tools,
        GenerateOptions {
            thinking_enabled,
            thinking_level,
            builtin_web_search,
            max_tokens: max_output_tokens,
            ..GenerateOptions::default()
        },
        label,
        GenerateRequestContext::new(Some(conversation_id), Some(message_id)),
    );
    // **一律走流式**（`generate_via_stream_collect`），哪怕调用方只要一个完整结果。
    // 部分 openai_responses 代理只可靠地服务流式请求，非流式 `generate` 直接报
    // "Unknown Responses API error" —— 压缩、标题总结都各自被这个坑绊过一次。
    // 这里是所有「要完整结果」的模型调用的唯一出口（planning / synthesis / 恢复重试 /
    // 原生工具都汇到这儿），改在这一处就等于全线走流式；`GenerateOutput` 两条路径同型，
    // usage / web_search / images 照旧回传（正常答案路径本来就只走流式，是被走熟的那条）。
    let output = generate_via_stream_collect(state, provider, retry_attempts, request)
        .await
        .map_err(|err| err.to_string())?;
    let usage = output.usage.clone();
    let web_search = output.web_search.clone();
    let images = output.images.clone();
    Ok((
        output.to_openai_compatible_message(),
        usage,
        web_search,
        images,
    ))
}

/// 与 `call_chat_completion_message_with_usage` 同形（返回 `to_openai_compatible_message()` Value），
/// 但走**流式**路径而非非流式 `generate`：内部用 `generate_via_stream_collect` 触发流式后
/// 取回 provider 组装好的 `GenerateOutput`。
///
/// 动机：压缩的摘要调用是 agent 里**唯一**的非流式模型调用；部分 provider（如 `openai_responses`
/// 代理）只可靠地服务流式请求，非流式摘要调用会失败（"Unknown Responses API error"），导致压缩在
/// 这类 provider 上永远摘不动。整个 agent 的 planning/synthesis 已经证明流式在该 provider 上可用，
/// 故把摘要调用也改走流式。流式被所有 provider 普遍支持，对支持非流式的 GUI provider 也无退化。
///
/// 这是**无头**收集（不涉及 `AgentHost`、不发任何 host 事件），与手动压缩/非 UI 路径一致。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn call_chat_completion_message_streamed(
    state: &crate::state::AppState,
    provider: &crate::settings::ModelProvider,
    model: &str,
    messages: Vec<Value>,
    tools: Option<&[ChatToolDefinition]>,
    retry_attempts: usize,
    thinking_enabled: bool,
    max_output_tokens: u32,
    conversation_id: &str,
    message_id: &str,
    label: &str,
) -> Result<Value, String> {
    let request = generate_request_from_openai_messages(
        model,
        messages,
        tools,
        GenerateOptions {
            thinking_enabled,
            max_tokens: max_output_tokens,
            ..GenerateOptions::default()
        },
        label,
        GenerateRequestContext::new(Some(conversation_id), Some(message_id)),
    );
    let output = generate_via_stream_collect(state, provider, retry_attempts, request)
        .await
        .map_err(|err| err.to_string())?;
    Ok(output.to_openai_compatible_message())
}

/// 无头流式收集 sink：丢弃所有增量。摘要调用只消费返回的 `GenerateOutput.text`
/// （所有 provider 适配器都在流式路径上从累积增量填好 `text`），故无需在此累积；
/// 仅把 `StreamPart::Error` 上抛为 `ModelError`。不向任何 `AgentHost` 发事件。
struct DiscardStreamSink;

impl StreamSink for DiscardStreamSink {
    fn emit(&mut self, part: StreamPart) -> Result<(), ModelError> {
        if let StreamPart::Error { message } = part {
            return Err(ModelError::new(message));
        }
        Ok(())
    }
}

/// 走 provider 的**流式** `stream(...)`（经 `send_with_failover` + SSE 累积，与 planning/synthesis
/// 同一路径），用一个丢弃增量的 sink 触发流式，返回 provider 组装好的 `GenerateOutput`。
///
/// 所有适配器（`openai` / `anthropic` / `responses`）都在流式路径上把累积结果填进返回的
/// `GenerateOutput.text`/`reasoning`，故无需 sink 侧再累积兜底。
pub(crate) async fn generate_via_stream_collect(
    state: &crate::state::AppState,
    provider: &crate::settings::ModelProvider,
    retry_attempts: usize,
    request: GenerateRequest,
) -> Result<GenerateOutput, ModelError> {
    let mut sink = DiscardStreamSink;
    stream_with_chat_provider(state, provider, retry_attempts, request, &mut sink).await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn stream_scoped_chat_completion_inner(
    state: &crate::state::AppState,
    host: &dyn AgentHost,
    provider: &crate::settings::ModelProvider,
    model: &str,
    messages: Vec<Value>,
    tools: Option<&[ChatToolDefinition]>,
    retry_attempts: usize,
    thinking_enabled: bool,
    thinking_level: Option<String>,
    builtin_web_search: bool,
    max_output_tokens: u32,
    conversation_id: &str,
    run_id: &str,
    message_id: &str,
    generation: u64,
    label: &str,
    policy: AgentStreamPolicy,
    text_segment: Option<ChatMessageSegment>,
    reasoning_segment: Option<ChatMessageSegment>,
    tool_draft_tracker: Option<ToolCallDraftTracker>,
    web_search_tracker: Option<WebSearchCardTracker>,
) -> Result<ChatStreamOutput, ModelError> {
    let request = generate_request_from_openai_messages(
        model,
        messages,
        tools,
        GenerateOptions {
            thinking_enabled,
            thinking_level,
            builtin_web_search,
            max_tokens: max_output_tokens,
            ..GenerateOptions::default()
        },
        label,
        GenerateRequestContext::new(Some(conversation_id), Some(message_id)),
    );
    let mut sink = AgentStreamSink::new(
        host,
        conversation_id,
        run_id,
        message_id,
        matches!(policy, AgentStreamPolicy::PlanningNoDoneUntilNoTools),
        text_segment,
        reasoning_segment,
        tool_draft_tracker.clone(),
        web_search_tracker,
    );
    let output = tokio::select! {
        result = stream_with_chat_provider(
            state,
            provider,
            retry_attempts,
            request,
            &mut sink,
        ) => result?,
        _ = host.wait_for_generation_inactive(conversation_id, generation) => {
            let (content, reasoning) = sink.snapshot();
            return Ok(ChatStreamOutput::new(
                content.trim().to_string(),
                reasoning.trim().to_string(),
                true,
            ));
        }
    };
    sink.flush_pending_text();
    // 内置搜索实时卡定稿：流成功结束 ⇒ 翻 Success 并发终态记录（取消路径已在上面的
    // select 分支 return，不会到这里，故取消时实时卡不落 Success，与 draft 行为一致）。
    if let Some(record) = sink.finish_web_search_card() {
        host.emit_tool_record(conversation_id, run_id, message_id, &record);
    }
    let (snapshot_content, snapshot_reasoning) = sink.snapshot();
    let stream_output = ChatStreamOutput::from_generate_output_with_snapshot(
        output,
        snapshot_content,
        snapshot_reasoning,
    );
    validate_stream_output(label, policy, &stream_output).map_err(ModelError::new)?;
    Ok(stream_output)
}
