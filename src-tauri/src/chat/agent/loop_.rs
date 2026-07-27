use serde_json::Value;

use crate::chat::types::ToolCallRecord;
use crate::mcp::ChatToolDefinition;
use crate::skills;

use super::execute::ToolExecutor;
use super::finalize::{
    cancelled_run_result_from_state, finalize_completed, finalize_planning_final, SegmentBuilder,
};
use super::host::AgentHost;
use super::planning::{planning_step, PlanningStepOutcome};
use super::rounds::{run_tool_round, ToolRoundOutcome};
use super::stop::patch_system_message;
use super::synthesis::{synthesis_step, SynthesisFlow};
use super::types::{AgentRunConfig, AgentRunResult};

/// Immutable per-run environment shared by every loop phase.
pub(crate) struct LoopEnv<'a> {
    pub(crate) config: &'a AgentRunConfig<'a>,
    pub(crate) host: &'a dyn AgentHost,
    pub(crate) executor: &'a dyn ToolExecutor,
}

impl LoopEnv<'_> {
    pub(crate) fn ids(&self) -> RunIds<'_> {
        RunIds {
            conversation_id: &self.config.conversation_id,
            run_id: &self.config.run_id,
            message_id: &self.config.message_id,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct RunIds<'a> {
    pub(crate) conversation_id: &'a str,
    pub(crate) run_id: &'a str,
    pub(crate) message_id: &'a str,
}

/// Mutable accumulators threaded through the loop phases.
pub(crate) struct RunState {
    pub(crate) runtime_messages: Vec<Value>,
    pub(crate) tools: Vec<ChatToolDefinition>,
    pub(crate) blocked_tool_calls: Vec<ChatToolDefinition>,
    pub(crate) generated_api_messages: Vec<Value>,
    pub(crate) tool_records: Vec<ToolCallRecord>,
    pub(crate) planning_reasoning_parts: Vec<String>,
    pub(crate) segment_builder: SegmentBuilder,
    pub(crate) step_number: u8,
    pub(crate) provider_tools_unsupported: bool,
    pub(crate) tried_skill_only_tools: bool,
    pub(crate) planning_final_message: Option<Value>,
    pub(crate) planning_final_streamed: bool,
    /// 空响应重试守门（一次）：抽风网关会间歇性返回 200 + 空正文（无文本无工具调用）。
    /// 第一次遇到时 planning 返回 `RetryEmptyResponse` 原地重试；已重试过则照旧走
    /// FinalAnswer → finalize 报 "empty assistant response"。
    pub(crate) planning_empty_retried: bool,
    pub(crate) skill_cache: skills::SkillRunCache,
    /// 本轮全部模型调用（规划/合成/压缩摘要）的 usage 累计；provider 不报则保持 None。
    pub(crate) usage: Option<crate::chat::model::ModelUsage>,
    /// 本轮**最后一次**模型调用的 usage（真实用量锚点，与累计 `usage` 区分——累计是多步之和、
    /// 会虚高数倍，不能当锚点）。`maybe_compact_send_view` 据此把上下文占用锚定到 provider
    /// 实报值，只对锚点之后新增的消息做字符估算（对齐 pi/opencode 的 ground-truth 口径）。
    /// 压缩发生后清空（消息序列已变，旧锚点失真），下次模型调用重新填充。
    pub(crate) last_step_usage: Option<crate::chat::model::ModelUsage>,
    /// 记录锚点**响应 push 之后** `runtime_messages` 的长度——`runtime_messages[该值..]` =
    /// 锚点响应之后新增的消息（工具结果等），即 trailing 增量。在 `rounds.rs` push 完 assistant
    /// 响应后设置（而非 merge_usage 时），使 trailing 严格「响应之后」、不与锚点 output 双算。
    pub(crate) runtime_len_at_last_call: usize,
    /// `config.initial_anchor_*`（来自上一轮落盘 usage）是否仍可用：run 首次压缩检查前为 true；
    /// 一旦发生压缩即失效（回落纯估算，直到本轮模型调用产生新的 `last_step_usage`）。
    pub(crate) initial_anchor_valid: bool,
    /// 本轮是否真正发生过 L2 压缩（摘要已写回 `runtime_messages`）。finalize 据此
    /// 把压缩后的完整历史回传到 `AgentRunResult.compacted_history`，让跨轮调用方
    /// 用压缩后的历史替换其累积副本（压缩真正跨轮生效，而非仅当轮发送视图瘦身）。
    pub(crate) compacted: bool,
    /// Anti-thrashing 计数（Gap 2，Layer 3）：连续多少轮「需要压缩（超预算）但压缩没能减小
    /// 上下文」（摘要调用失败/为空/无可摘要旧段）。在 `maybe_compact_send_view` 里维护——
    /// 压成功并降到预算内则清零，否则递增。达到 `COMPACTION_THRASH_LIMIT` 时规划循环优雅收尾
    /// （用已收集的工具结果降级），而不是反复触发压缩并连续失败后才报错。
    pub(crate) compaction_unresolved_rounds: u32,
    pub(crate) pending_compaction_boundary: Option<crate::chat::types::CompactionBoundaryRecord>,
    /// L2 压缩产出的落盘 summary（与 boundary 同期生成）。run 结束时由 `attach_usage`
    /// 挂到 `AgentRunResult.compaction_summary`，commands.rs 据此写回 `context_state.summary`
    /// + `compression_count`（L2 不再只 push boundary，对齐落盘路径）。
    pub(crate) pending_compaction_summary: Option<crate::chat::types::ConversationContextSummary>,
    /// 模型原生生成的图片（Gemini native image gen，任务 07-24）：跨轮累积每次答案调用
    /// （planning/synthesis）产出的 `GenerateOutput.images`，run 结束时由 `attach_usage`
    /// 挂到 `AgentRunResult.images` → reply 落成 assistant 消息级 artifacts。
    pub(crate) generated_images: Vec<crate::chat::model::GeneratedImageData>,
    /// 本轮若走了降级兜底（模型失败但已有工具结果），这里带结构化描述，
    /// 最终落到 `ChatMessage.degraded` 让前端渲染成独立卡片而非混进正文。
    pub(crate) degraded: Option<crate::chat::agent::recovery::DegradedAnswer>,
}

/// 连续「需要压缩但压不下去」多少轮后停止工具循环、优雅收尾（Gap 2，Layer 3 anti-thrashing）。
/// 取 2：给压缩一次重试机会（provider 偶发抖动可能第二轮成功），第二次仍失败则判定压缩无能为力，
/// 不再硬撑——避免实测里出现的「压缩连续失败 6+ 次后才超窗报错」。
pub(crate) const COMPACTION_THRASH_LIMIT: u32 = 2;

impl RunState {
    /// 把单次模型调用的 usage 累加进本轮总账（None 入参不改变现状）。
    /// 同时把这次调用记为**真实用量锚点**（`last_step_usage`）——累计 `usage` 是多步之和不能当
    /// 锚点，锚点必须是单次调用的 usage。`runtime_len_at_last_call`（trailing 切点）不在这里设，
    /// 而在 `rounds.rs` push 完该次响应后设——保证 trailing = 锚点响应**之后**新增（对齐 pi、避免
    /// 与锚点里的 output 双算）。
    /// 注：即便这次是 recovery（发送的是精简/压缩输入）导致锚点偏小，`effective_context_tokens`
    /// 的 `max(纯估算)` 下限也会兜底，绝不会因锚点偏小而比现状更乐观。
    pub(crate) fn merge_usage(&mut self, next: Option<crate::chat::model::ModelUsage>) {
        let Some(next) = next else { return };
        self.last_step_usage = Some(next.clone());
        let total = self.usage.get_or_insert_with(Default::default);
        let add = |slot: &mut Option<u64>, value: Option<u64>| {
            if let Some(value) = value {
                *slot = Some(slot.unwrap_or(0).saturating_add(value));
            }
        };
        add(&mut total.input_tokens, next.input_tokens);
        add(&mut total.output_tokens, next.output_tokens);
        add(&mut total.total_tokens, next.total_tokens);
        add(&mut total.cached_input_tokens, next.cached_input_tokens);
        add(
            &mut total.cache_creation_input_tokens,
            next.cache_creation_input_tokens,
        );
        add(&mut total.reasoning_tokens, next.reasoning_tokens);
    }
}

pub async fn run_agent_loop(
    mut config: AgentRunConfig<'_>,
    host: &dyn AgentHost,
    executor: &dyn ToolExecutor,
) -> Result<AgentRunResult, String> {
    let mut state = RunState {
        runtime_messages: std::mem::take(&mut config.runtime_messages),
        tools: std::mem::take(&mut config.tools),
        blocked_tool_calls: std::mem::take(&mut config.blocked_tool_calls),
        generated_api_messages: Vec::new(),
        tool_records: Vec::new(),
        planning_reasoning_parts: Vec::new(),
        segment_builder: SegmentBuilder::new(),
        step_number: 0,
        provider_tools_unsupported: false,
        tried_skill_only_tools: false,
        planning_final_message: None,
        planning_final_streamed: false,
        planning_empty_retried: false,
        skill_cache: skills::SkillRunCache::default(),
        usage: None,
        last_step_usage: None,
        runtime_len_at_last_call: 0,
        initial_anchor_valid: true,
        compacted: false,
        compaction_unresolved_rounds: 0,
        pending_compaction_boundary: None,
        pending_compaction_summary: None,
        generated_images: Vec::new(),
        degraded: None,
    };
    // 把助手的技能白名单冻结进 skill_cache,作为 skill_activate 执行派发的硬 gate。
    // 无助手 = None = 不限(全局行为)。
    state.skill_cache.set_allowed_skill_ids(
        config
            .assistant_snapshot
            .as_ref()
            .map(|assistant| assistant.skill_ids.clone()),
    );
    let env = LoopEnv {
        config: &config,
        host,
        executor,
    };

    if !state.tools.is_empty() {
        let mut round = 0u32;
        loop {
            round = round.saturating_add(1);
            state.step_number = state.step_number.saturating_add(1);
            if !host.is_generation_active(&config.conversation_id, config.generation) {
                // Cancelled at the loop top (before this round's planning call).
                // Preserve whatever previous rounds already accumulated
                // (tool_records / segments / api_messages) by ending with
                // Ok(cancelled_result) instead of a bare Err("cancelled") — the
                // latter skipped persistence and dropped the whole turn.
                let result = cancelled_run_result_from_state(&env, &mut state);
                return Ok(attach_usage(result, &mut state));
            }

            let planned = match planning_step(&env, &mut state, round).await? {
                PlanningStepOutcome::FinalAnswer => break,
                PlanningStepOutcome::ToolsUnsupported => break,
                PlanningStepOutcome::RetryWithSkillTools => continue,
                PlanningStepOutcome::RetryEmptyResponse => continue,
                PlanningStepOutcome::DraftFailed(result) => {
                    return Ok(attach_usage(result, &mut state))
                }
                PlanningStepOutcome::Recovered(result) => {
                    return Ok(attach_usage(result, &mut state))
                }
                PlanningStepOutcome::Cancelled(result) => {
                    return Ok(attach_usage(result, &mut state))
                }
                PlanningStepOutcome::ToolCalls(planned) => planned,
            };

            match run_tool_round(&env, &mut state, round, planned).await {
                ToolRoundOutcome::Continue => {}
                ToolRoundOutcome::RoundLimit => break,
                ToolRoundOutcome::Cancelled(result) => return Ok(attach_usage(result, &mut state)),
            }
            // Crash-safety checkpoint: persist a best-effort snapshot of the
            // partial assistant message after each completed tool round. The
            // final assistant message is otherwise written only once, after this
            // loop returns (`push_assistant_message`); if the process dies mid-run
            // the whole turn — including files just written by tools this round —
            // vanishes. Persisting here keeps the work recoverable on next load.
            // The final write replaces this draft (same `message_id`), so there
            // is no duplication on the normal success path.
            host.persist_partial_assistant(
                &config.conversation_id,
                &config.message_id,
                &state.tool_records,
                state.segment_builder.segments(),
                &state.generated_api_messages,
            );
        }
    }

    if state.provider_tools_unsupported {
        patch_system_message(
            &mut state.runtime_messages,
            &config.provider_tools_fallback_system_prompt,
        );
    }

    if let Some(message) = state.planning_final_message.take() {
        return finalize_planning_final(&env, &mut state, message)
            .map(|result| attach_usage(result, &mut state));
    }

    match synthesis_step(&env, &mut state).await? {
        SynthesisFlow::Early(result) => Ok(attach_usage(result, &mut state)),
        SynthesisFlow::Completed(completed) => {
            let result = finalize_completed(&env, &mut state, completed);
            Ok(attach_usage(result, &mut state))
        }
    }
}

/// 把本轮累计的 provider usage 挂到运行结果上（finalize 构造器们不感知 usage）。
/// 同时：本轮若发生过 L2 压缩，把压缩后的完整历史 +最终 assistant 回答回传到
/// `compacted_history`，供跨轮调用方替换其累积历史（finalize 构造器们也不感知压缩）。
fn attach_usage(mut result: AgentRunResult, state: &mut RunState) -> AgentRunResult {
    result.usage = state.usage.take();
    result.last_step_usage = state.last_step_usage.take();
    if state.compacted {
        let mut history = std::mem::take(&mut state.runtime_messages);
        let final_message =
            super::stop::final_assistant_api_message(&result.content, result.reasoning.as_deref());
        history.push(final_message);
        result.compacted_history = Some(history);
    }
    result.compaction_boundary = state.pending_compaction_boundary.take();
    result.compaction_summary = state.pending_compaction_summary.take();
    result.images = std::mem::take(&mut state.generated_images);
    result
}

#[cfg(test)]
pub(crate) use super::execute::ToolExecutionContext;
#[cfg(test)]
pub(crate) use super::finalize::{
    cancelled_tool_round_run_result, empty_synthesis_fallback_response, stopped_generation_content,
    synthesis_failed_fallback_response, tool_planning_failed_fallback_response,
    tool_planning_failed_run_result,
};
#[cfg(test)]
pub(crate) use super::rounds::{
    execute_tool_round, tool_call_parallel_eligible, tool_message, tool_round_limit_reached,
    visible_tool_segment_calls, ToolRoundContext,
};
#[cfg(test)]
pub(crate) use super::stream::{AgentStreamSink, ToolCallDraftTracker};
#[cfg(test)]
pub(crate) use crate::chat::model::PendingToolCall;
#[cfg(test)]
pub(crate) use crate::chat::types::{
    ChatMessageSegment, ChatMessageSegmentKind, ChatMessageSegmentPhase,
};
#[cfg(test)]
pub(crate) use crate::settings::Settings;

#[cfg(test)]
#[path = "loop_tests.rs"]
mod tests;
