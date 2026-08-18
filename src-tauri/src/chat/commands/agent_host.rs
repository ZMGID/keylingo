use serde_json::Value;
use tauri::AppHandle;

use crate::chat::{ChatMessageSegment, CompactionBoundaryRecord, ToolCallRecord};
use crate::mcp::{self, ChatToolDefinition};
use crate::skills;
use crate::state::AppState;

use super::context::{emit_chat_compaction_state, emit_chat_context_usage_live};
use super::interaction::{
    emit_chat_stream_delta, emit_chat_tool_record, request_session_consent, request_tool_approval,
    request_user_response, wait_for_chat_cancel,
};
use super::messages::persist_partial_assistant_snapshot;
pub(super) struct ChatAgentHost<'a> {
    pub(super) app: AppHandle,
    pub(super) state: &'a AppState,
    pub(super) run_id: String,
    /// 多模型臂置 true：抑制 mid-run 部分快照落盘（协调者统一落盘）。默认 false（现状）。
    pub(super) suppress_partial_persist: bool,
    /// 用户配置的生命周期 Hooks。无启用 Hook 时为 `None`，loop 零开销。
    pub(super) hooks: Option<crate::chat::hooks::HookDispatcher>,
}

impl crate::chat::agent::AgentHost for ChatAgentHost<'_> {
    fn emit_stream_delta(
        &self,
        _conversation_id: &str,
        run_id: &str,
        _message_id: &str,
        delta: &str,
        reasoning_delta: Option<&str>,
        segment: Option<&ChatMessageSegment>,
    ) {
        emit_chat_stream_delta(&self.app, run_id, delta, reasoning_delta, segment);
    }

    fn emit_tool_record(
        &self,
        _conversation_id: &str,
        run_id: &str,
        _message_id: &str,
        record: &ToolCallRecord,
    ) {
        emit_chat_tool_record(&self.app, run_id, record);
    }

    fn emit_compaction_status(
        &self,
        conversation_id: &str,
        phase: &str,
        trigger: Option<&str>,
        boundary: Option<&CompactionBoundaryRecord>,
    ) {
        emit_chat_compaction_state(
            &self.app,
            conversation_id,
            &self.run_id,
            phase,
            trigger,
            boundary,
        );
    }

    fn emit_context_usage_live(
        &self,
        conversation_id: &str,
        used_tokens: u64,
        context_window_tokens: Option<u64>,
    ) {
        emit_chat_context_usage_live(
            &self.app,
            conversation_id,
            &self.run_id,
            used_tokens,
            context_window_tokens,
        );
    }

    fn persist_partial_assistant<'a>(
        &'a self,
        conversation_id: &'a str,
        message_id: &'a str,
        tool_records: &'a [ToolCallRecord],
        segments: &'a [ChatMessageSegment],
        api_messages: &'a [Value],
    ) -> crate::chat::agent::AgentHostFuture<'a, ()> {
        Box::pin(async move {
            if self.suppress_partial_persist {
                return;
            }
            if let Err(err) = persist_partial_assistant_snapshot(
                &self.app,
                conversation_id,
                message_id,
                tool_records,
                segments,
                api_messages,
            )
            .await
            {
                eprintln!("persist partial assistant snapshot failed: {err}");
            }
        })
    }

    fn request_tool_approval<'a>(
        &'a self,
        ctx: &'a crate::chat::agent::ToolExecutionContext<'a>,
        record: &'a ToolCallRecord,
    ) -> crate::chat::agent::AgentHostFuture<'a, bool> {
        Box::pin(async move {
            request_tool_approval(
                &self.app,
                self.state,
                ctx.conversation_id,
                ctx.run_id,
                ctx.generation,
                record,
            )
            .await
        })
    }

    fn request_session_consent<'a>(
        &'a self,
        ctx: &'a crate::chat::agent::ToolExecutionContext<'a>,
    ) -> crate::chat::agent::AgentHostFuture<'a, bool> {
        Box::pin(async move {
            request_session_consent(
                &self.app,
                self.state,
                ctx.tool_conversation_id,
                ctx.run_id,
                ctx.generation,
            )
            .await
        })
    }

    fn request_user_response<'a>(
        &'a self,
        ctx: &'a crate::chat::agent::ToolExecutionContext<'a>,
        record: &'a ToolCallRecord,
        prompt: crate::chat::ask_user::AskUserPromptPayload,
    ) -> crate::chat::agent::AgentHostFuture<'a, crate::chat::ask_user::AskUserResponseResult> {
        Box::pin(async move {
            request_user_response(
                &self.app,
                self.state,
                ctx.conversation_id,
                ctx.run_id,
                ctx.generation,
                record,
                prompt,
            )
            .await
        })
    }

    fn is_generation_active(&self, conversation_id: &str, generation: u64) -> bool {
        self.state
            .is_chat_generation_active(conversation_id, generation)
    }

    fn take_steering_messages(
        &self,
        conversation_id: &str,
    ) -> Vec<crate::chat::agent::SteeringMessage> {
        self.state.take_chat_steering(conversation_id)
    }

    fn take_follow_up_messages(
        &self,
        conversation_id: &str,
    ) -> Vec<crate::chat::agent::SteeringMessage> {
        self.state.take_chat_follow_up(conversation_id)
    }

    fn wait_for_generation_inactive<'a>(
        &'a self,
        conversation_id: &'a str,
        generation: u64,
    ) -> crate::chat::agent::AgentHostFuture<'a, ()> {
        Box::pin(async move {
            wait_for_chat_cancel(self.state, conversation_id, generation).await;
        })
    }

    fn hooks(&self) -> Option<&crate::chat::hooks::HookDispatcher> {
        self.hooks.as_ref()
    }
}

/// 无头测试通道（probe）的 AgentHost，仅 debug 构建。跑的是与 GUI 完全相同的生成核心
/// （`complete_assistant_reply_inner`），但所有需要 GUI 应答的交互门一律自动放行：审批 /
/// 会话 consent → 允许，`ask_user` → 取消态（不阻塞）。事件发射 no-op（结果从落盘的 assistant
/// 消息内联读取，不靠事件）。generation 相关沿用标准机制，保证超时/取消能生效。
#[cfg(debug_assertions)]
pub(super) struct ProbeAgentHost<'a> {
    pub(super) state: &'a AppState,
}

#[cfg(debug_assertions)]
impl crate::chat::agent::AgentHost for ProbeAgentHost<'_> {
    fn emit_stream_delta(
        &self,
        _conversation_id: &str,
        _run_id: &str,
        _message_id: &str,
        _delta: &str,
        _reasoning_delta: Option<&str>,
        _segment: Option<&ChatMessageSegment>,
    ) {
    }

    fn emit_tool_record(
        &self,
        _conversation_id: &str,
        _run_id: &str,
        _message_id: &str,
        _record: &ToolCallRecord,
    ) {
    }

    fn request_tool_approval<'a>(
        &'a self,
        _ctx: &'a crate::chat::agent::ToolExecutionContext<'a>,
        _record: &'a ToolCallRecord,
    ) -> crate::chat::agent::AgentHostFuture<'a, bool> {
        Box::pin(async { true })
    }

    fn request_session_consent<'a>(
        &'a self,
        _ctx: &'a crate::chat::agent::ToolExecutionContext<'a>,
    ) -> crate::chat::agent::AgentHostFuture<'a, bool> {
        Box::pin(async { true })
    }

    fn request_user_response<'a>(
        &'a self,
        _ctx: &'a crate::chat::agent::ToolExecutionContext<'a>,
        _record: &'a ToolCallRecord,
        _prompt: crate::chat::ask_user::AskUserPromptPayload,
    ) -> crate::chat::agent::AgentHostFuture<'a, crate::chat::ask_user::AskUserResponseResult> {
        // 无头：不能向用户提问，直接返回取消态让 loop 继续（不阻塞）。
        Box::pin(async { crate::chat::ask_user::cancelled_response() })
    }

    fn is_generation_active(&self, conversation_id: &str, generation: u64) -> bool {
        self.state
            .is_chat_generation_active(conversation_id, generation)
    }

    fn wait_for_generation_inactive<'a>(
        &'a self,
        conversation_id: &'a str,
        generation: u64,
    ) -> crate::chat::agent::AgentHostFuture<'a, ()> {
        Box::pin(async move {
            wait_for_chat_cancel(self.state, conversation_id, generation).await;
        })
    }
}

pub(super) struct RegistryToolExecutor<'a> {
    pub(super) app: AppHandle,
    pub(super) state: &'a AppState,
}
impl crate::chat::agent::ToolExecutor for RegistryToolExecutor<'_> {
    fn call<'a>(
        &'a self,
        ctx: &'a crate::chat::agent::ToolExecutionContext<'a>,
        tool: &'a ChatToolDefinition,
        arguments: Value,
        skill_cache: Option<&'a mut skills::SkillRunCache>,
    ) -> crate::chat::agent::ToolExecutorFuture<'a> {
        Box::pin(async move {
            let native_ctx = mcp::registry::NativeToolContext {
                // Conversation-scoped tools (todo / native workspace) target the
                // tool conversation, which equals the run conversation for a
                // top-level run and the PARENT conversation for a sub-agent run.
                conversation_id: ctx.tool_conversation_id.to_string(),
                message_id: ctx.message_id.to_string(),
                tool_call_id: Some(ctx.tool_call_id.to_string()),
                run_id: ctx.run_id.to_string(),
                generation: ctx.generation,
                depth: ctx.depth,
            };
            let result = mcp::registry::call_tool(
                &self.app,
                self.state,
                tool,
                arguments,
                skill_cache,
                Some(native_ctx),
            )
            .await;
            // run 域这条有 replay 价值，保留；但别为它再整读一遍会话 JSON——
            // todo_write 的结构化返回里本来就带着刚落盘的权威 todoState。
            if crate::mcp::types::canonical_tool_name(&tool.name) == "todo_write" {
                if let Some(todo_state) = result.as_ref().ok().and_then(|res| {
                    serde_json::from_value::<crate::chat::types::AgentTodoState>(
                        res.structured_content.as_ref()?.get("todoState")?.clone(),
                    )
                    .ok()
                }) {
                    crate::chat::protocol::emit_run_event(
                        &self.app,
                        ctx.run_id,
                        crate::chat::protocol::ChatRunEvent::TodoUpdated {
                            todo_state: (&todo_state).into(),
                        },
                    );
                }
            }
            result
        })
    }
}
