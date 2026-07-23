use serde_json::{json, Value};

use crate::chat::model::BuiltinWebSearch;
use crate::chat::types::{ChatMessageSegment, ChatMessageSegmentKind, ToolCallStatus};

use super::finalize::{
    emit_builtin_web_search_card, empty_synthesis_fallback_response, segment_phase_for_agent_phase,
    stopped_generation_content, synthesis_failed_fallback_response, RunResultBuilder,
};
use super::loop_::{LoopEnv, RunState};
use super::planning::{
    call_chat_completion_message_with_usage, call_chat_completion_output_with_usage,
    stream_scoped_chat_completion_inner,
};
use super::recovery::{self, RecoveryAction};
use super::stop::{
    empty_assistant_response_error, extract_reasoning_content, final_assistant_api_message,
    merge_reasoning, sanitize_assistant_text_response,
};
use super::stream::ChatStreamOutput;
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
    let response_segment = state.segment_builder.reserve(
        ChatMessageSegmentKind::Text,
        response_phase.clone(),
        Some(step_number),
        None,
        &format!("step_{step_number}_text"),
    );

    // 内置搜索引用（若开且模型支持）：从合成模型输出捕获，稍后合成来源卡。
    // 两条分支（流式/非流式）都会在读取前赋值，初值仅为满足声明。
    #[allow(unused_assignments)]
    let mut synth_web_search: Option<BuiltinWebSearch> = None;
    let (response, reasoning, response_reasoning) = if config.stream_enabled {
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
        )
        .await
        .map_err(|err| err.to_string());
        let stream = match stream {
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
        synth_web_search = stream.web_search.clone();
        let final_reasoning_for_api = stream.reasoning.clone();
        let reasoning = merge_reasoning(&state.planning_reasoning_parts, stream.reasoning.clone());
        let response = sanitize_assistant_text_response(&stream.content);
        if response.trim().is_empty() {
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
        }
    } else {
        // 有意行为统一：此前非流式分支发送原始 state.runtime_messages（历史 quirk），
        // 接入压缩后改为与流式分支相同的发送视图（send_messages 即压缩后的发送视图），
        // 保证两条合成路径在超限场景行为一致。
        let runtime_messages = send_messages.clone();
        let message_result = tokio::select! {
            result = call_chat_completion_output_with_usage(
                config.state,
                &config.provider,
                &config.model,
                runtime_messages,
                None,
                config.retry_attempts,
                config.thinking_enabled,
                config.thinking_level.clone(),
                config.builtin_web_search_active(),
                config.max_output_tokens,
                &config.conversation_id,
                &config.message_id,
                "Chat API",
            ) => result,
            _ = host.wait_for_generation_inactive(&config.conversation_id, config.generation) => {
                host.emit_stream_done(
                    &config.conversation_id,
                    &config.run_id,
                    &config.message_id,
                    "cancelled",
                    "",
                );
                return Err("cancelled".to_string());
            }
        };
        let message = match message_result {
            Ok((message, usage, web_search)) => {
                state.merge_usage(usage);
                synth_web_search = web_search;
                message
            }
            Err(err) if !state.tool_records.is_empty() => {
                eprintln!("Chat synthesis request failed after tool records; recovering: {err}");
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
        let response = sanitize_assistant_text_response(
            message
                .get("content")
                .and_then(|content| content.as_str())
                .unwrap_or_default(),
        );
        let reasoning = merge_reasoning(
            &state.planning_reasoning_parts,
            extract_reasoning_content(&message),
        );
        let response_reasoning = extract_reasoning_content(&message);
        if response.trim().is_empty() && !state.tool_records.is_empty() {
            eprintln!(
                "Chat agent empty synthesis fallback: conversation_id={} run_id={} provider_id={} model={} phase={:?} stream=false tool_records={} finish_reason={}",
                config.conversation_id,
                config.run_id,
                config.provider.id,
                config.model,
                phase,
                state.tool_records.len(),
                message
                    .get("finish_reason")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown"),
            );
            let recovered = recover_synthesis(env, state, "").await;
            let content = if recovered.trim().is_empty() {
                empty_synthesis_fallback_response(&config.language)
            } else {
                recovered
            };
            let fallback = RunResultBuilder::new(host, env.ids(), content)
                .emit_segment_opt(Some(response_segment.clone()))
                .api_reasoning(extract_reasoning_content(&message))
                .emit_done("done")
                .emit_and_record(&mut state.generated_api_messages);
            (fallback, reasoning, response_reasoning)
        } else if response.trim().is_empty() {
            host.emit_stream_done(
                &config.conversation_id,
                &config.run_id,
                &config.message_id,
                "error",
                "",
            );
            return Err(empty_assistant_response_error("Chat API"));
        } else {
            host.emit_stream_delta(
                &config.conversation_id,
                &config.run_id,
                &config.message_id,
                &response,
                None,
                Some(&response_segment),
            );
            host.emit_stream_done(
                &config.conversation_id,
                &config.run_id,
                &config.message_id,
                "done",
                &response,
            );
            if !state.generated_api_messages.is_empty() {
                state.generated_api_messages.push(message);
            }
            (response, reasoning, response_reasoning)
        }
    };

    // 内置搜索（服务端托管）发生过 ⇒ 合成来源卡（末尾 order → 答案下方脚注）。
    if let Some(web_search) = synth_web_search.as_ref() {
        emit_builtin_web_search_card(
            host,
            env.ids(),
            state,
            web_search,
            &config.provider.name,
            response_phase.clone(),
            None,
        );
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
    let kind = recovery::classify(failure_message);
    let has_results = !state.tool_records.is_empty();
    // 恢复中枢只在此处被调用一次/次失败,故 already_remediated / overflow_recovery_attempted
    // 都从 false 起算;真正的「只重试一次」守门在各分支内部用本地标志实现。
    match recovery::decide(kind, has_results, false, false) {
        RecoveryAction::Surface => String::new(),
        RecoveryAction::DegradeToGathered => recovery::assemble_results_from_tool_records(
            &state.tool_records,
            &config.language,
            kind,
        ),
        RecoveryAction::CompactAndRetry => recover_overflow_compact_and_retry(env, state).await,
        RecoveryAction::Remediate => recover_remediate(env, state, kind).await,
    }
}

/// CompactAndRetry 执行:压缩一次历史 → 用压缩后的发送视图重发一次合成。
/// 成功 → 用其结果;仍失败 → 降级到确定性兜底(对应 decide 的 overflow_recovery_attempted 臂)。
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
            String::new()
        }
    };
    if !text.trim().is_empty() {
        text
    } else {
        // 压缩重试仍失败 → 确定性兜底(decide 的 overflow_recovery_attempted=true 臂)。
        recovery::assemble_results_from_tool_records(
            &state.tool_records,
            &config.language,
            recovery::FailureKind::ContextOverflow,
        )
    }
}

/// Remediate 执行:用「用户问题 + 工具产出摘要 + 中立指令」去敏精简后重做一次合成。
/// 仍失败 → 确定性兜底(decide 的 already_remediated 臂)。
async fn recover_remediate(
    env: &LoopEnv<'_>,
    state: &mut RunState,
    kind: recovery::FailureKind,
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
        recovery::assemble_results_from_tool_records(&state.tool_records, &config.language, kind)
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
