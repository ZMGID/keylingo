use serde_json::{json, Value};

use crate::chat::types::{ChatMessageSegment, ChatMessageSegmentKind, ToolCallStatus};

use super::finalize::{
    empty_synthesis_fallback_response, segment_phase_for_agent_phase,
    stopped_generation_content, synthesis_failed_fallback_response, RunResultBuilder,
};
use super::loop_::{LoopEnv, RunState};
use super::planning::{
    call_chat_completion_message_with_usage, stream_scoped_chat_completion_inner,
};
use super::recovery::{self, RecoveryAction};
use super::stop::{
    empty_assistant_response_error, final_assistant_api_message,
    merge_reasoning, sanitize_assistant_text_response,
};
use super::stream::{ChatStreamOutput, WebSearchCardTracker};
use super::types::{AgentPhase, AgentRunConfig, AgentRunResult, AgentStreamPolicy};

pub(crate) struct SynthesisCompleted {
    pub(crate) response: String,
    pub(crate) reasoning: Option<String>,
    pub(crate) response_reasoning: Option<String>,
    pub(crate) response_segment: ChatMessageSegment,
    pub(crate) response_reasoning_segment: ChatMessageSegment,
}

pub(crate) enum SynthesisFlow {
    Completed(SynthesisCompleted),
    Early(AgentRunResult),
}

pub(crate) async fn synthesis_step(
    env: &LoopEnv<'_>,
    state: &mut RunState,
) -> Result<SynthesisFlow, String> {
    let config = env.config;
    let host = env.host;
    state.step_number = state.step_number.saturating_add(1);
    let step_number = state.step_number;
    let phase = if state.tool_records.is_empty() && !state.provider_tools_unsupported {
        AgentPhase::Plain
    } else {
        AgentPhase::Synthesis
    };
    // 循环内上下文治理：超限时先 snip / 摘要（与 planning_step 相同的发送视图）。
    let send_messages = super::compaction::maybe_compact_send_view(env, state).await;
    let synthesis_stream_policy = if state.tool_records.is_empty() {
        AgentStreamPolicy::SynthesisAlwaysDone
    } else {
        AgentStreamPolicy::SynthesisDeferEmpty
    };
    let response_phase = segment_phase_for_agent_phase(phase);
    let response_reasoning_segment = state.segment_builder.reserve(
        ChatMessageSegmentKind::Reasoning,
        response_phase.clone(),
        Some(step_number),
        None,
        &format!("step_{step_number}_reasoning"),
    );
    // reasoning 与正文之间预留内置搜索实时卡的 order 槽（任务 07-23，与 planning 同款）。
    let web_search_order = state.segment_builder.reserve_order();
    let response_segment = state.segment_builder.reserve(
        ChatMessageSegmentKind::Text,
        response_phase.clone(),
        Some(step_number),
        None,
        &format!("step_{step_number}_text"),
    );
    // 内置搜索实时卡追踪器：仅 Builtin 模式且模型支持时建（否则 None，走现状路径）。
    let synth_web_search_tracker = if config.builtin_web_search_active() {
        Some(WebSearchCardTracker::new(
            web_search_order,
            None,
            response_phase.clone(),
            config.provider.name.clone(),
        ))
    } else {
        None
    };

    let stream = stream_scoped_chat_completion_inner(
        config.state,
        host,
        &config.provider,
        &config.model,
        send_messages,
        None,
        config.retry_attempts,
        config.thinking_enabled,
        config.thinking_level.clone(),
        config.builtin_web_search_active(),
        config.max_output_tokens,
        &config.conversation_id,
        &config.run_id,
        &config.message_id,
        config.generation,
        "Chat stream",
        synthesis_stream_policy,
        Some(response_segment.clone()),
        Some(response_reasoning_segment.clone()),
        None,
        synth_web_search_tracker.clone(),
    )
    .await
    .map_err(|err| err.to_string());
    let mut stream = match stream {
        Ok(stream) => stream,
        Err(err) if !state.tool_records.is_empty() => {
            eprintln!("Chat synthesis stream failed after tool records; recovering: {err}");
            let recovered = recover_synthesis(env, state, &err).await;
            let content = if recovered.trim().is_empty() {
                synthesis_failed_fallback_response(&config.language)
            } else {
                recovered
            };
            return Ok(SynthesisFlow::Early(
                RunResultBuilder::new(host, env.ids(), content)
                    .segment(&response_segment)
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
        Err(err) => return Err(err),
    };
    if stream.cancelled {
        if !state.tool_records.is_empty() {
            let stored_content = if stream.content.trim().is_empty() {
                stopped_generation_content(&config.language)
            } else {
                stream.content.clone()
            };
            return Ok(SynthesisFlow::Early(
                RunResultBuilder::new(host, env.ids(), stored_content)
                    .segment(&response_segment)
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
        let partial = sanitize_assistant_text_response(&stream.content);
        if partial.trim().is_empty() {
            return Err("cancelled".to_string());
        }
        // Plain-text streaming was cancelled after partial output; the stream
        // layer already emitted the single done("cancelled") event. Preserve
        // the generated text instead of dropping the whole turn.
        return Ok(SynthesisFlow::Early(
            RunResultBuilder::new(host, env.ids(), partial)
                .segment(&response_segment)
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
    let final_reasoning_for_api = stream.reasoning.clone();
    let reasoning = merge_reasoning(&state.planning_reasoning_parts, stream.reasoning.clone());
    let response = sanitize_assistant_text_response(&stream.content);
    let (response, reasoning, response_reasoning) = if response.trim().is_empty() {
        if !state.tool_records.is_empty() {
            log_empty_synthesis_output(config, phase, &stream, state.tool_records.len());
            let recovered = recover_synthesis(env, state, "").await;
            let content = if recovered.trim().is_empty() {
                empty_synthesis_fallback_response(&config.language)
            } else {
                recovered
            };
            let fallback = RunResultBuilder::new(host, env.ids(), content)
                .emit_segment_opt(Some(response_segment.clone()))
                .api_reasoning(final_reasoning_for_api.clone())
                .emit_done("done")
                .emit_and_record(&mut state.generated_api_messages);
            (fallback, reasoning, final_reasoning_for_api)
        } else {
            return Err(empty_assistant_response_error("Chat stream"));
        }
    } else {
        if !state.generated_api_messages.is_empty() {
            state
                .generated_api_messages
                .push(final_assistant_api_message(
                    &response,
                    final_reasoning_for_api.as_deref(),
                ));
        }
        (response, reasoning, final_reasoning_for_api)
    };

    // 内置搜索：实时卡追踪器边流边合成（take_card 落 Success 终态卡）。
    if let Some(tracker) = synth_web_search_tracker.as_ref() {
        if let Some((record, segment)) = tracker.take_card() {
            state
                .segment_builder
                .append_existing_segments(vec![segment]);
            state.tool_records.push(record);
        }
    }

    Ok(SynthesisFlow::Completed(SynthesisCompleted {
        response,
        reasoning,
        response_reasoning,
        response_segment,
        response_reasoning_segment,
    }))
}

fn last_user_text(messages: &[Value]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
}

/// 收集本轮成功工具产出的可读摘要(用于去敏重做的输入)。
fn gathered_previews(state: &RunState) -> Vec<String> {
    state
        .tool_records
        .iter()
        .filter(|r| r.status == ToolCallStatus::Success)
        .filter_map(|r| {
            r.result_preview
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(|p| format!("【{}】\n{}", r.name, p))
        })
        .collect()
}

/// 去敏 + 精简的恢复输入:仅用「用户问题 + 工具产出摘要 + 中立指令」重做一次合成,
/// 去掉触发审核的完整正文/历史。
fn build_neutral_reduced_messages(state: &RunState) -> Vec<Value> {
    let question = last_user_text(&state.runtime_messages).unwrap_or_default();
    let previews = gathered_previews(state).join("\n\n");
    let system =
        "Answer the user's question objectively and neutrally, strictly based on the search snippets below. Only organize and state information already present in the snippets; add no commentary, stance, or outside content.";
    let user = format!("User question: {question}\n\nSearch snippets:\n{previews}");
    vec![
        json!({ "role": "system", "content": system }),
        json!({ "role": "user", "content": user }),
    ]
}

/// 统一恢复入口(恢复策略中枢的执行端):按 `recovery::decide` 走
/// 「overflow 压缩重发 → 去敏重做 → 确定性兜底」阶梯。
/// 返回非空内容即视为已恢复;返回空串表示无可恢复(调用方退回静态文案)。
/// planning 阶段中途失败也复用此入口,保证两条路径同一恢复策略。
///
/// 取 `&mut RunState`:overflow 分支需要调用 `maybe_compact_send_view` 压缩历史
/// (会写回 `state.runtime_messages` 工作副本)。其它分支不修改 state。
pub(crate) async fn recover_synthesis(
    env: &LoopEnv<'_>,
    state: &mut RunState,
    failure_message: &str,
) -> String {
    let config = env.config;
    let kind = silent_overflow_aware_kind(env, state, recovery::classify(failure_message));
    let has_results = !state.tool_records.is_empty();
    // 恢复中枢只在此处被调用一次/次失败,故 already_remediated / overflow_recovery_attempted
    // 都从 false 起算;真正的「只重试一次」守门在各分支内部用本地标志实现。
    match recovery::decide(kind, has_results, false, false) {
        RecoveryAction::Surface => String::new(),
        RecoveryAction::DegradeToGathered => {
            // 结构化留存一份供前端渲染成独立卡片；返回值仍是同样的纯文本，
            // 兼容旧前端 / 外部 CLI（它们只读 content）。
            let degraded = recovery::assemble_degraded_answer(
                &state.tool_records,
                &config.language,
                kind,
                Some(failure_message),
            );
            let text = degraded
                .as_ref()
                .map(|d| d.text.clone())
                .unwrap_or_default();
            state.degraded = degraded;
            text
        }
        RecoveryAction::CompactAndRetry => recover_overflow_compact_and_retry(env, state).await,
        RecoveryAction::Remediate => recover_remediate(env, state, kind, failure_message).await,
    }
}

/// 把 `Empty` 在**有用量证据**时改判成 `ContextOverflow`（pi `isContextOverflow`
/// case 2/3 的落地，判据见 [`recovery::is_silent_overflow`]）。
///
/// 「模型调用成功但正文为空」有两种成因，处置完全相反：真的没话说 → 重来无意义,
/// 拿工具结果降级；被静默吞掉的超窗请求 → 压缩一次再重发，多半就出来了。
/// `classify` 只有一个空串可看，分不出来；provider 实报的 prompt token 能。
///
/// 只改判 `Empty`：其它 kind 都有明确的错误文案，不该被用量猜测覆盖。
fn silent_overflow_aware_kind(
    env: &LoopEnv<'_>,
    state: &RunState,
    kind: recovery::FailureKind,
) -> recovery::FailureKind {
    if kind != recovery::FailureKind::Empty {
        return kind;
    }
    let Some(usage) = state.last_step_usage.as_ref() else {
        return kind;
    };
    let config = env.config;
    let window = crate::chat::model_metadata::context_window_for_model(
        Some(&config.provider),
        &config.model,
    )
    .0;
    let prompt = super::context_estimate::prompt_tokens(usage, &config.provider.api_format);
    if !recovery::is_silent_overflow(prompt, window) {
        return kind;
    }
    eprintln!(
        "Chat: empty response with prompt {prompt:?} tokens against window {window} — \
         treating as silent context overflow (compact and retry)"
    );
    recovery::FailureKind::ContextOverflow
}

/// CompactAndRetry 执行:压缩一次历史 → 用压缩后的发送视图重发一次合成。/// 成功 → 用其结果;仍失败 → 降级到确定性兜底(对应 decide 的 overflow_recovery_attempted 臂)。
/// 单次守门:本函数只压缩-重试一次,绝不递归,杜绝「压完仍超 → 再压」死循环。
async fn recover_overflow_compact_and_retry(env: &LoopEnv<'_>, state: &mut RunState) -> String {
    let config = env.config;
    // 压缩一次(L1 snip → L2 摘要);返回压缩后的发送视图,并已写回 state.runtime_messages。
    let compacted = super::compaction::maybe_compact_send_view(env, state).await;
    // 恢复重试内部有 send_with_retry 多次退避——必须接取消，否则用户点停止后卡到重试耗尽。
    let result = tokio::select! {
        result = call_chat_completion_message_with_usage(
            config.state,
            &config.provider,
            &config.model,
            compacted,
            None,
            config.retry_attempts,
            config.thinking_enabled,
            config.thinking_level.clone(),
            config.builtin_web_search_active(),
            config.max_output_tokens,
            &config.conversation_id,
            &config.message_id,
            "Chat synthesis overflow recovery",
        ) => result,
        _ = env.host.wait_for_generation_inactive(&config.conversation_id, config.generation) => {
            Err("cancelled".to_string())
        }
    };
    // 重试自己的报错也要留给卡片——否则用户只看到"压缩后仍失败"这句空话。
    let mut retry_error: Option<String> = None;
    let text = match result {
        Ok((message, usage)) => {
            state.merge_usage(usage);
            sanitize_assistant_text_response(
                message
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default(),
            )
        }
        Err(err) => {
            eprintln!("Chat synthesis overflow compact-and-retry failed: {err}");
            retry_error = Some(err);
            String::new()
        }
    };
    if !text.trim().is_empty() {
        text
    } else {
        // 压缩重试仍失败 → 确定性兜底(decide 的 overflow_recovery_attempted=true 臂)。
        let degraded = recovery::assemble_degraded_answer(
            &state.tool_records,
            &config.language,
            recovery::FailureKind::ContextOverflow,
            retry_error.as_deref(),
        );
        let text = degraded
            .as_ref()
            .map(|d| d.text.clone())
            .unwrap_or_default();
        state.degraded = degraded;
        text
    }
}

/// Remediate 执行:用「用户问题 + 工具产出摘要 + 中立指令」去敏精简后重做一次合成。
/// 仍失败 → 确定性兜底(decide 的 already_remediated 臂)。
async fn recover_remediate(
    env: &LoopEnv<'_>,
    state: &mut RunState,
    kind: recovery::FailureKind,
    failure_message: &str,
) -> String {
    let config = env.config;
    let reduced = build_neutral_reduced_messages(state);
    // 同 recover_overflow_compact_and_retry：恢复重试必须接取消。
    let result = tokio::select! {
        result = call_chat_completion_message_with_usage(
            config.state,
            &config.provider,
            &config.model,
            reduced,
            None,
            config.retry_attempts,
            config.thinking_enabled,
            config.thinking_level.clone(),
            config.builtin_web_search_active(),
            config.max_output_tokens,
            &config.conversation_id,
            &config.message_id,
            "Chat synthesis recovery",
        ) => result,
        _ = env.host.wait_for_generation_inactive(&config.conversation_id, config.generation) => {
            Err("cancelled".to_string())
        }
    };
    let text = match result {
        Ok((message, usage)) => {
            state.merge_usage(usage);
            sanitize_assistant_text_response(
                message
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default(),
            )
        }
        Err(_) => String::new(),
    };
    if !text.trim().is_empty() {
        text
    } else {
        // 去敏重试仍失败 → 确定性兜底(decide 的 already_remediated 臂)。
        // detail 用**最初**那次失败的报错：去敏重试只是补救手段，用户要排查的是原始原因。
        let degraded = recovery::assemble_degraded_answer(
            &state.tool_records,
            &config.language,
            kind,
            Some(failure_message),
        );
        let text = degraded
            .as_ref()
            .map(|d| d.text.clone())
            .unwrap_or_default();
        state.degraded = degraded;
        text
    }
}

fn log_empty_synthesis_output(
    config: &AgentRunConfig<'_>,
    phase: AgentPhase,
    stream: &ChatStreamOutput,
    tool_record_count: usize,
) {
    eprintln!(
        "Chat agent empty synthesis fallback: conversation_id={} run_id={} provider_id={} model={} phase={:?} stream=true tool_records={} finish_reason={} raw_chars={} cleaned_chars={} reasoning_chars={} stream_tool_calls={}",
        config.conversation_id,
        config.run_id,
        config.provider.id,
        config.model,
        phase,
        tool_record_count,
        stream.finish_reason.as_deref().unwrap_or("unknown"),
        stream.raw_content.chars().count(),
        stream.content.chars().count(),
        stream.reasoning.as_deref().map(|value| value.chars().count()).unwrap_or(0),
        stream.tool_calls.len(),
    );
}
