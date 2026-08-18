use serde_json::Value;

use crate::chat::model::PendingToolCall;
use crate::chat::types::{ToolCallRecord, ToolCallStatus};
use crate::mcp::ChatToolDefinition;
use crate::settings::Settings;
use crate::skills;

use super::execute::{
    disabled_tool_content, execute_tool_call, invalid_tool_arguments_record, match_tool_call,
    tool_requires_approval, unknown_tool_record, ToolExecutionContext, ToolExecutor,
};
use super::finalize::cancelled_tool_round_run_result;
use super::host::AgentHost;
use super::loop_::{LoopEnv, RunState};
use super::planning::PlannedToolRound;
use super::stop::{assistant_api_message_for_tool_calls, step_limit_system_message};
use super::types::AgentRunResult;

/// 单回合内并行工具调用的批宽上限。与 `SubAgentManager` 的默认并发(12)对齐,
/// 使一回合 fan-out 多个 subagent 时信号量成为真正的瓶颈而非这里。批内用
/// `join_all` 并发执行,故任意 ≤ 此值的批宽都安全。
pub(crate) const MAX_PARALLEL_TOOL_CALLS_PER_ROUND: usize = 12;

pub(crate) enum ToolRoundOutcome {
    /// The round completed; the skeleton continues to the next planning round.
    Continue,
    /// The round limit was reached; the limit system message was injected and
    /// the last step's stop reason rewritten. The skeleton breaks out.
    RoundLimit,
    /// The round was cancelled mid-flight; done("cancelled") was emitted and a
    /// final result built. The skeleton returns it.
    Cancelled(AgentRunResult),
}

pub(crate) async fn run_tool_round(
    env: &LoopEnv<'_>,
    state: &mut RunState,
    round: u32,
    planned: PlannedToolRound,
) -> ToolRoundOutcome {
    let config = env.config;
    let host = env.host;
    let assistant_message =
        assistant_api_message_for_tool_calls(&planned.message, &planned.tool_calls);
    state.runtime_messages.push(assistant_message);
    // 真实用量锚点的 trailing 切点：本次规划响应刚 push 进 runtime，其后的工具结果即 trailing
    // 增量（`runtime_messages[runtime_len_at_last_call..]`）。响应本身用锚点里的真实 output 计入，
    // 故切点设在响应**之后**，避免与 output 双算（对齐 pi 的「anchor + 响应后消息估算」）。
    state.runtime_len_at_last_call = state.runtime_messages.len();
    state.generated_api_messages.push(
        state
            .runtime_messages
            .last()
            .cloned()
            .unwrap_or(Value::Null),
    );
    let round_result = execute_tool_round(
        host,
        env.executor,
        &config.settings,
        ToolRoundContext {
            conversation_id: &config.conversation_id,
            run_id: &config.run_id,
            message_id: &config.message_id,
            generation: config.generation,
            round,
            depth: config.depth,
            tool_conversation_id: &config.tool_conversation_id,
            finish_reason: planned
                .message
                .get("finish_reason")
                .and_then(|v| v.as_str()),
        },
        &state.tools,
        &state.blocked_tool_calls,
        planned.tool_calls,
        &mut state.skill_cache,
    )
    .await;
    let round_cancelled = round_result.cancelled;
    state
        .runtime_messages
        .extend(round_result.response_messages.iter().cloned());
    state
        .generated_api_messages
        .extend(round_result.response_messages.iter().cloned());
    state.tool_records.extend(round_result.tool_records);
    if round_cancelled {
        return ToolRoundOutcome::Cancelled(cancelled_tool_round_run_result(
            &config.language,
            &state.planning_reasoning_parts,
            std::mem::take(&mut state.tool_records),
            std::mem::take(&mut state.segment_builder).all(),
            std::mem::take(&mut state.generated_api_messages),
        ));
    }
    if tool_round_limit_reached(config.effective_chat_tools.max_tool_rounds, round) {
        state.runtime_messages.push(step_limit_system_message());
        return ToolRoundOutcome::RoundLimit;
    }
    ToolRoundOutcome::Continue
}

pub(crate) struct ToolRoundContext<'a> {
    pub(crate) conversation_id: &'a str,
    pub(crate) run_id: &'a str,
    pub(crate) message_id: &'a str,
    pub(crate) generation: u64,
    pub(crate) round: u32,
    pub(crate) depth: u8,
    pub(crate) tool_conversation_id: &'a str,
    /// finish_reason of the generation that produced these tool calls. When
    /// `"length"`, the model hit max_output_tokens and any tool-call JSON is
    /// truncated — so we explain *that* instead of "invalid JSON, retry".
    pub(crate) finish_reason: Option<&'a str>,
}

pub(crate) struct ToolRoundResult {
    pub(crate) response_messages: Vec<Value>,
    pub(crate) tool_records: Vec<ToolCallRecord>,
    pub(crate) cancelled: bool,
}

struct ToolExecutionResult {
    response_message: Value,
    record: Option<ToolCallRecord>,
    cancelled: bool,
    /// Extra user-role messages (OpenAI shape) appended right after this tool's
    /// result message — used by `read` to feed an image to a vision model.
    follow_up_messages: Vec<Value>,
}

struct ExecutableToolCall<'a> {
    call: PendingToolCall,
    tool: &'a ChatToolDefinition,
}

pub(crate) fn visible_tool_segment_calls(
    tools: &[ChatToolDefinition],
    blocked_tool_calls: &[ChatToolDefinition],
    tool_calls: &[PendingToolCall],
) -> Vec<PendingToolCall> {
    tool_calls
        .iter()
        .filter(|call| {
            match_tool_call(tools, &call.function_name).is_some()
                || match_tool_call(blocked_tool_calls, &call.function_name).is_some()
                || disabled_tool_content(call).is_none()
        })
        .cloned()
        .collect()
}

pub(crate) fn tool_round_limit_reached(max_tool_rounds: Option<u32>, round: u32) -> bool {
    max_tool_rounds.is_some_and(|limit| round >= limit)
}

pub(crate) async fn execute_tool_round(
    host: &dyn AgentHost,
    executor: &dyn ToolExecutor,
    settings: &Settings,
    ctx: ToolRoundContext<'_>,
    tools: &[ChatToolDefinition],
    blocked_tool_calls: &[ChatToolDefinition],
    tool_calls: Vec<PendingToolCall>,
    skill_cache: &mut skills::SkillRunCache,
) -> ToolRoundResult {
    // 对齐 pi `failToolCallsFromTruncatedMessage`：finish_reason=="length" 说明整条响应
    // 在 max_output_tokens 处被拦腰截断——流式工具参数经 salvage 收尾后完全可能「JSON 合法
    // 但语义残缺」（写文件只写了半个 content、编辑少了半条 edit），逐个看解析是否成功不可靠。
    // 整批作废、一个都不执行，让模型带着完整参数重发。
    if ctx.finish_reason == Some("length") && !tool_calls.is_empty() {
        let mut response_messages = Vec::new();
        let mut tool_records = Vec::new();
        for tool_call in tool_calls {
            let result = truncated_tool_call_result(host, &ctx, tools, tool_call);
            push_tool_execution_result(result, &mut response_messages, &mut tool_records);
        }
        return ToolRoundResult {
            response_messages,
            tool_records,
            cancelled: false,
        };
    }

    let mut response_messages = Vec::new();
    let mut tool_records = Vec::new();
    let mut parallel_batch: Vec<ExecutableToolCall<'_>> = Vec::new();
    let mut cancelled = false;

    let mut tool_calls = tool_calls.into_iter();
    while let Some(tool_call) = tool_calls.next() {
        let Some(tool) = match_tool_call(tools, &tool_call.function_name) else {
            if flush_parallel_batch(
                &mut parallel_batch,
                &mut response_messages,
                &mut tool_records,
                host,
                executor,
                settings,
                &ctx,
            )
            .await
            {
                cancelled = true;
                push_cancelled_tool_call(
                    host,
                    &ctx,
                    tools,
                    tool_call,
                    &mut response_messages,
                    &mut tool_records,
                );
                push_cancelled_tool_calls(
                    host,
                    &ctx,
                    tools,
                    tool_calls,
                    &mut response_messages,
                    &mut tool_records,
                );
                break;
            }
            let result =
                unknown_or_disabled_tool_result(host, &ctx, tools, blocked_tool_calls, tool_call);
            push_tool_execution_result(result, &mut response_messages, &mut tool_records);
            continue;
        };

        if let Some(error) = tool_call.arguments_parse_error.clone() {
            if flush_parallel_batch(
                &mut parallel_batch,
                &mut response_messages,
                &mut tool_records,
                host,
                executor,
                settings,
                &ctx,
            )
            .await
            {
                cancelled = true;
                push_cancelled_tool_call(
                    host,
                    &ctx,
                    tools,
                    tool_call,
                    &mut response_messages,
                    &mut tool_records,
                );
                push_cancelled_tool_calls(
                    host,
                    &ctx,
                    tools,
                    tool_calls,
                    &mut response_messages,
                    &mut tool_records,
                );
                break;
            }
            let result = invalid_tool_arguments_result(host, &ctx, &tool_call, tool, error);
            push_tool_execution_result(result, &mut response_messages, &mut tool_records);
            continue;
        }

        if tool_call_parallel_eligible(settings, tool) {
            parallel_batch.push(ExecutableToolCall {
                call: tool_call,
                tool,
            });
            if parallel_batch.len() >= MAX_PARALLEL_TOOL_CALLS_PER_ROUND {
                if flush_parallel_batch(
                    &mut parallel_batch,
                    &mut response_messages,
                    &mut tool_records,
                    host,
                    executor,
                    settings,
                    &ctx,
                )
                .await
                {
                    cancelled = true;
                    push_cancelled_tool_calls(
                        host,
                        &ctx,
                        tools,
                        tool_calls,
                        &mut response_messages,
                        &mut tool_records,
                    );
                    break;
                }
            }
            continue;
        }

        if flush_parallel_batch(
            &mut parallel_batch,
            &mut response_messages,
            &mut tool_records,
            host,
            executor,
            settings,
            &ctx,
        )
        .await
        {
            cancelled = true;
            push_cancelled_tool_call(
                host,
                &ctx,
                tools,
                tool_call,
                &mut response_messages,
                &mut tool_records,
            );
            push_cancelled_tool_calls(
                host,
                &ctx,
                tools,
                tool_calls,
                &mut response_messages,
                &mut tool_records,
            );
            break;
        }
        let result =
            execute_serial_tool_call(host, executor, settings, &ctx, tool, tool_call, skill_cache)
                .await;
        if push_tool_execution_result(result, &mut response_messages, &mut tool_records) {
            cancelled = true;
            push_cancelled_tool_calls(
                host,
                &ctx,
                tools,
                tool_calls,
                &mut response_messages,
                &mut tool_records,
            );
            break;
        }
    }

    if !cancelled {
        cancelled = flush_parallel_batch(
            &mut parallel_batch,
            &mut response_messages,
            &mut tool_records,
            host,
            executor,
            settings,
            &ctx,
        )
        .await;
    }

    ToolRoundResult {
        response_messages,
        tool_records,
        cancelled,
    }
}

fn push_tool_execution_result(
    result: ToolExecutionResult,
    response_messages: &mut Vec<Value>,
    tool_records: &mut Vec<ToolCallRecord>,
) -> bool {
    let cancelled = result.cancelled;
    if let Some(record) = result.record {
        tool_records.push(record);
    }
    response_messages.push(result.response_message);
    // Follow-up user messages (e.g. an image for a vision model) must come
    // after the tool-result message so tool_call_ids are answered first.
    response_messages.extend(result.follow_up_messages);
    cancelled
}

async fn flush_parallel_batch(
    batch: &mut Vec<ExecutableToolCall<'_>>,
    response_messages: &mut Vec<Value>,
    tool_records: &mut Vec<ToolCallRecord>,
    host: &dyn AgentHost,
    executor: &dyn ToolExecutor,
    settings: &Settings,
    ctx: &ToolRoundContext<'_>,
) -> bool {
    if batch.is_empty() {
        return false;
    }

    let mut cancelled = false;
    while !batch.is_empty() {
        let limit = batch.len().min(MAX_PARALLEL_TOOL_CALLS_PER_ROUND);
        let mut chunk = batch.drain(..limit).collect::<Vec<_>>();
        let results = execute_parallel_chunk(host, executor, settings, ctx, &mut chunk).await;
        for result in results {
            cancelled |= push_tool_execution_result(result, response_messages, tool_records);
        }
        if cancelled {
            for item in batch.drain(..) {
                let result = cancelled_tool_result(host, ctx, &item.call, Some(item.tool));
                push_tool_execution_result(result, response_messages, tool_records);
            }
            break;
        }
    }
    cancelled
}

async fn execute_parallel_chunk(
    host: &dyn AgentHost,
    executor: &dyn ToolExecutor,
    settings: &Settings,
    ctx: &ToolRoundContext<'_>,
    chunk: &mut [ExecutableToolCall<'_>],
) -> Vec<ToolExecutionResult> {
    // join_all preserves input order and drives the whole chunk concurrently,
    // so the batch width can be anything up to MAX_PARALLEL_TOOL_CALLS_PER_ROUND
    // without per-arity hand-wiring (and without silently dropping calls past 4).
    futures::future::join_all(
        chunk
            .iter()
            .map(|item| execute_parallel_tool_call(host, executor, settings, ctx, item)),
    )
    .await
}

async fn execute_parallel_tool_call(
    host: &dyn AgentHost,
    executor: &dyn ToolExecutor,
    settings: &Settings,
    ctx: &ToolRoundContext<'_>,
    item: &ExecutableToolCall<'_>,
) -> ToolExecutionResult {
    execute_tool_call_result(
        host,
        executor,
        settings,
        ctx,
        item.tool,
        item.call.clone(),
        None,
    )
    .await
}

async fn execute_serial_tool_call(
    host: &dyn AgentHost,
    executor: &dyn ToolExecutor,
    settings: &Settings,
    ctx: &ToolRoundContext<'_>,
    tool: &ChatToolDefinition,
    tool_call: PendingToolCall,
    skill_cache: &mut skills::SkillRunCache,
) -> ToolExecutionResult {
    execute_tool_call_result(
        host,
        executor,
        settings,
        ctx,
        tool,
        tool_call,
        Some(skill_cache),
    )
    .await
}

async fn execute_tool_call_result(
    host: &dyn AgentHost,
    executor: &dyn ToolExecutor,
    settings: &Settings,
    round_ctx: &ToolRoundContext<'_>,
    tool: &ChatToolDefinition,
    tool_call: PendingToolCall,
    skill_cache: Option<&mut skills::SkillRunCache>,
) -> ToolExecutionResult {
    let tool_call_id = tool_call.id.clone();
    let execution_ctx = ToolExecutionContext {
        conversation_id: round_ctx.conversation_id,
        run_id: round_ctx.run_id,
        message_id: round_ctx.message_id,
        generation: round_ctx.generation,
        round: round_ctx.round,
        depth: round_ctx.depth,
        tool_conversation_id: round_ctx.tool_conversation_id,
        tool_call_id: &tool_call_id,
    };
    // 单点：串行与并行两条路径都经过这里，所以 start/end 一定成对。并行 chunk 里多个
    // 工具的事件会交错——那是真实语义，脚本靠 toolCallId 自己配对，不做重排。
    let hooks = host.hooks();
    if let Some(hooks) = hooks {
        hooks.dispatch(
            crate::chat::hooks::HookEvent::ToolExecutionStart,
            Some(tool.name.as_str()),
            Some(round_ctx.round),
            Some(tool_call_id.as_str()),
        );
    }
    let (record, tool_content, follow_up_messages) = execute_tool_call(
        host,
        executor,
        settings,
        &execution_ctx,
        tool,
        tool_call,
        skill_cache,
    )
    .await;
    if let Some(hooks) = hooks {
        hooks.dispatch(
            crate::chat::hooks::HookEvent::ToolExecutionEnd,
            Some(tool.name.as_str()),
            Some(round_ctx.round),
            Some(tool_call_id.as_str()),
        );
    }
    let cancelled = matches!(record.status, ToolCallStatus::Cancelled);
    ToolExecutionResult {
        response_message: tool_message(tool_call_id, tool_content),
        record: Some(record),
        cancelled,
        follow_up_messages,
    }
}

fn push_cancelled_tool_calls(
    host: &dyn AgentHost,
    ctx: &ToolRoundContext<'_>,
    tools: &[ChatToolDefinition],
    tool_calls: impl IntoIterator<Item = PendingToolCall>,
    response_messages: &mut Vec<Value>,
    tool_records: &mut Vec<ToolCallRecord>,
) {
    for tool_call in tool_calls {
        push_cancelled_tool_call(host, ctx, tools, tool_call, response_messages, tool_records);
    }
}

fn push_cancelled_tool_call(
    host: &dyn AgentHost,
    ctx: &ToolRoundContext<'_>,
    tools: &[ChatToolDefinition],
    tool_call: PendingToolCall,
    response_messages: &mut Vec<Value>,
    tool_records: &mut Vec<ToolCallRecord>,
) {
    let tool = match_tool_call(tools, &tool_call.function_name);
    let result = cancelled_tool_result(host, ctx, &tool_call, tool);
    push_tool_execution_result(result, response_messages, tool_records);
}

fn cancelled_tool_result(
    host: &dyn AgentHost,
    ctx: &ToolRoundContext<'_>,
    tool_call: &PendingToolCall,
    tool: Option<&ChatToolDefinition>,
) -> ToolExecutionResult {
    let now = chrono::Local::now().timestamp();
    let record = ToolCallRecord {
        id: tool_call.id.clone(),
        name: tool
            .map(|tool| tool.name.clone())
            .unwrap_or_else(|| tool_call.function_name.clone()),
        source: tool
            .map(|tool| tool.source.clone())
            .unwrap_or_else(|| "unknown".to_string()),
        server_id: tool.and_then(|tool| tool.server_id.clone()),
        arguments: tool_call.arguments_raw.clone(),
        status: ToolCallStatus::Cancelled,
        result_preview: None,
        error: Some("Tool call cancelled".to_string()),
        duration_ms: Some(0),
        started_at: Some(now),
        completed_at: Some(now),
        round: ctx.round,
        sensitive: tool.map(|tool| tool.sensitive).unwrap_or(false),
        artifacts: Vec::new(),
        trace_id: Some(ctx.run_id.to_string()),
        span_id: Some(tool_span_id(ctx, &tool_call.id)),
        structured_content: None,
    };
    host.emit_tool_record(ctx.conversation_id, ctx.run_id, ctx.message_id, &record);
    ToolExecutionResult {
        response_message: tool_message(tool_call.id.clone(), "Tool call cancelled"),
        record: Some(record),
        cancelled: true,
        follow_up_messages: Vec::new(),
    }
}

fn unknown_or_disabled_tool_result(
    host: &dyn AgentHost,
    ctx: &ToolRoundContext<'_>,
    tools: &[ChatToolDefinition],
    blocked_tool_calls: &[ChatToolDefinition],
    tool_call: PendingToolCall,
) -> ToolExecutionResult {
    if let Some(tool) = match_tool_call(blocked_tool_calls, &tool_call.function_name) {
        let error = format!(
            "Tool `{}` is blocked in Plan mode. Switch to Act / execute the plan before using side-effecting tools.",
            tool.openai_tool_name()
        );
        let mut record = skipped_tool_record(ctx, &tool_call, tool, error.clone());
        attach_tool_trace(ctx, &mut record);
        host.emit_tool_record(ctx.conversation_id, ctx.run_id, ctx.message_id, &record);
        return ToolExecutionResult {
            response_message: tool_message(tool_call.id, error),
            record: Some(record),
            cancelled: false,
            follow_up_messages: Vec::new(),
        };
    }

    let disabled = disabled_tool_content(&tool_call);
    let record = if disabled.is_none() {
        let error = format!("Unknown tool requested: {}", tool_call.function_name);
        let mut record = unknown_tool_record(&tool_call, ctx.round, error);
        attach_tool_trace(ctx, &mut record);
        host.emit_tool_record(ctx.conversation_id, ctx.run_id, ctx.message_id, &record);
        Some(record)
    } else {
        None
    };
    // 喂回自愈（对齐 opencode）：错误作为 tool result 返回时附上已声明工具清单，
    // 让模型下一轮自我纠正（Cursor 系模型会间歇性按训练时的工具名出牌，如大写 Grep）。
    let content = disabled.unwrap_or_else(|| {
        let mut available: Vec<String> = tools
            .iter()
            .map(|tool| tool.openai_tool_name())
            .collect();
        available.sort();
        available.dedup();
        format!(
            "Unknown tool: {}. Available tools: {}. Please call one of the declared tools with its exact name.",
            tool_call.function_name,
            available.join(", ")
        )
    });
    ToolExecutionResult {
        response_message: tool_message(tool_call.id, content),
        record,
        cancelled: false,
        follow_up_messages: Vec::new(),
    }
}

fn skipped_tool_record(
    ctx: &ToolRoundContext<'_>,
    call: &PendingToolCall,
    tool: &ChatToolDefinition,
    error: String,
) -> ToolCallRecord {
    let now = chrono::Local::now().timestamp();
    ToolCallRecord {
        id: call.id.clone(),
        name: tool.name.clone(),
        source: tool.source.clone(),
        server_id: tool.server_id.clone(),
        arguments: call.arguments_raw.clone(),
        status: ToolCallStatus::Skipped,
        result_preview: None,
        error: Some(error),
        duration_ms: Some(0),
        started_at: Some(now),
        completed_at: Some(now),
        round: ctx.round,
        sensitive: tool.sensitive,
        artifacts: Vec::new(),
        trace_id: None,
        span_id: None,
        structured_content: None,
    }
}

fn invalid_tool_arguments_result(
    host: &dyn AgentHost,
    ctx: &ToolRoundContext<'_>,
    tool_call: &PendingToolCall,
    tool: &ChatToolDefinition,
    error: String,
) -> ToolExecutionResult {
    // finish_reason == "length" means the model hit max_output_tokens and the
    // tool-call JSON was cut off mid-stream — not malformed JSON. Telling the
    // model to "retry compact" is misleading; name the real cause so it (and
    // the user) can react: split a large write, or raise max output tokens.
    let truncated = ctx.finish_reason == Some("length");
    let (record_error, model_hint) = if truncated {
        (
            format!(
                "Output truncated: hit the max output token limit (finish_reason=length), \
                 so the tool-call arguments are incomplete. Original error: {error}"
            ),
            "Your previous response was cut off at the max output token limit \
             (finish_reason=length), so this tool call's arguments are incomplete. \
             Don't retry the same oversized call — produce smaller output (e.g. write \
             the file in several smaller chunks via multiple calls), or ask the user to \
             raise the chat \"max output tokens\" setting for this model."
                .to_string(),
        )
    } else {
        (
            error,
            "Tool arguments JSON is invalid or incomplete. Retry this tool call with a \
             compact, valid JSON object for arguments."
                .to_string(),
        )
    };
    let mut record = invalid_tool_arguments_record(tool_call, tool, ctx.round, record_error);
    attach_tool_trace(ctx, &mut record);
    host.emit_tool_record(ctx.conversation_id, ctx.run_id, ctx.message_id, &record);
    ToolExecutionResult {
        response_message: tool_message(tool_call.id.clone(), model_hint),
        record: Some(record),
        cancelled: false,
        follow_up_messages: Vec::new(),
    }
}

/// finish_reason=="length" 的整批作废结果：不执行，只给模型一条「参数可能被截断，
/// 请重发」的错误结果（每个 tool_call_id 都必须被应答，缺一条下一轮请求直接 400）。
fn truncated_tool_call_result(
    host: &dyn AgentHost,
    ctx: &ToolRoundContext<'_>,
    tools: &[ChatToolDefinition],
    tool_call: PendingToolCall,
) -> ToolExecutionResult {
    let error = "Not executed: the response hit the max output token limit \
                 (finish_reason=length), so the tool-call arguments may be truncated."
        .to_string();
    let model_hint = format!(
        "Tool call \"{}\" was not executed: the response hit the output token limit, so its \
         arguments may be truncated. Re-issue the tool call with complete arguments — and if \
         the arguments themselves are large (e.g. a big file write), produce smaller output \
         by splitting the work across several calls.",
        tool_call.function_name
    );
    let mut record = match match_tool_call(tools, &tool_call.function_name) {
        Some(tool) => invalid_tool_arguments_record(&tool_call, tool, ctx.round, error),
        None => unknown_tool_record(&tool_call, ctx.round, error),
    };
    attach_tool_trace(ctx, &mut record);
    host.emit_tool_record(ctx.conversation_id, ctx.run_id, ctx.message_id, &record);
    ToolExecutionResult {
        response_message: tool_message(tool_call.id, model_hint),
        record: Some(record),
        cancelled: false,
        follow_up_messages: Vec::new(),
    }
}

fn attach_tool_trace(ctx: &ToolRoundContext<'_>, record: &mut ToolCallRecord) {
    record.trace_id = Some(ctx.run_id.to_string());
    record.span_id = Some(tool_span_id(ctx, &record.id));
}

fn tool_span_id(ctx: &ToolRoundContext<'_>, tool_call_id: &str) -> String {
    format!("tool_round_{}_{}", ctx.round, tool_call_id)
}

pub(crate) fn tool_message(tool_call_id: String, content: impl Into<String>) -> Value {
    serde_json::json!({
        "role": "tool",
        "tool_call_id": tool_call_id,
        "content": content.into(),
    })
}

pub(crate) fn tool_call_parallel_eligible(settings: &Settings, tool: &ChatToolDefinition) -> bool {
    if tool_requires_approval(settings, tool) {
        return false;
    }
    if tool.source == "native" {
        // The native parallel-safe set is intentionally narrow (see
        // agent-runtime spec); it lives in the static registry.
        return crate::mcp::native_registry::find_entry(&tool.name)
            .is_some_and(|entry| entry.parallel_safe);
    }
    tool.source == "mcp" && tool.is_read_only_tool()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn follow_up_messages_are_appended_after_tool_result() {
        // Mirrors `read` feeding an image to a vision model: the tool result is
        // text, the actual image rides as a follow-up user message that must
        // land right after the tool message so the tool_call_id is answered.
        let image_msg = json!({
            "role": "user",
            "content": [{
                "type": "image_url",
                "image_url": { "url": "data:image/png;base64,AAAA" }
            }]
        });
        let result = ToolExecutionResult {
            response_message: tool_message("call_1".to_string(), "已读取图片 x.png，见下方。"),
            record: None,
            cancelled: false,
            follow_up_messages: vec![image_msg.clone()],
        };
        let mut response_messages = Vec::new();
        let mut tool_records = Vec::new();
        let cancelled =
            push_tool_execution_result(result, &mut response_messages, &mut tool_records);

        assert!(!cancelled);
        assert_eq!(response_messages.len(), 2);
        assert_eq!(response_messages[0]["role"], "tool");
        assert_eq!(response_messages[0]["tool_call_id"], "call_1");
        assert_eq!(response_messages[1], image_msg);
    }

    #[test]
    fn no_follow_up_leaves_single_tool_result() {
        let result = ToolExecutionResult {
            response_message: tool_message("call_2".to_string(), "plain text"),
            record: None,
            cancelled: false,
            follow_up_messages: Vec::new(),
        };
        let mut response_messages = Vec::new();
        let mut tool_records = Vec::new();
        push_tool_execution_result(result, &mut response_messages, &mut tool_records);

        assert_eq!(response_messages.len(), 1);
        assert_eq!(response_messages[0]["role"], "tool");
    }
}
