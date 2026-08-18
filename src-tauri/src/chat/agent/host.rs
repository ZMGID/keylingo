use std::{future::Future, pin::Pin};

use crate::chat::ask_user::{AskUserPromptPayload, AskUserResponseResult};
use crate::chat::hooks::HookDispatcher;
use crate::chat::types::{ChatMessageSegment, CompactionBoundaryRecord, ToolCallRecord};

use super::execute::ToolExecutionContext;

pub type AgentHostFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait AgentHost: Send + Sync {
    fn emit_stream_delta(
        &self,
        conversation_id: &str,
        run_id: &str,
        message_id: &str,
        delta: &str,
        reasoning_delta: Option<&str>,
        segment: Option<&ChatMessageSegment>,
    );

    fn emit_tool_record(
        &self,
        conversation_id: &str,
        run_id: &str,
        message_id: &str,
        record: &ToolCallRecord,
    );

    /// Live compaction progress for chat timeline UI. Default no-op.
    fn emit_compaction_status(
        &self,
        _conversation_id: &str,
        _phase: &str,
        _trigger: Option<&str>,
        _boundary: Option<&CompactionBoundaryRecord>,
    ) {
    }

    /// 生成过程中的上下文占用活数（分子 + 分母），让用量条在长轮次里跟着走而不是轮末
    /// 才跳一个数。由 `compaction::maybe_compact_send_view` 每轮调用一次——那里**已经**
    /// 按权威口径算出了这两个数（压缩阈值判定要用），所以实时通道是零额外计算。
    ///
    /// **默认 no-op，且子 agent host 必须保持默认**：子 agent 有自己独立的上下文窗口
    /// （常是便宜的小模型，窗口小 5 倍），它的占用混进主对话会让用量条来回乱跳。
    fn emit_context_usage_live(
        &self,
        _conversation_id: &str,
        _used_tokens: u64,
        _context_window_tokens: Option<u64>,
    ) {
    }

    /// Persist a best-effort snapshot of the in-progress assistant message to
    /// durable storage after a completed tool round. The full assistant message
    /// is otherwise written only once, after the loop returns; if the process
    /// dies mid-run (crash / forced exit) that whole turn — including tool work
    /// already done — is lost. This checkpoint keeps it recoverable on the next
    /// load. `api_messages` carries the loop's accumulated provider messages
    /// (assistant tool_calls + tool results) up to this round so the draft is
    /// replayable on a later "continue" — without them an `interrupted` draft
    /// loses all tool context and the model restarts from scratch. Default
    /// no-op for hosts that don't own persistence (sub-agents, tests).
    fn persist_partial_assistant<'a>(
        &'a self,
        _conversation_id: &'a str,
        _message_id: &'a str,
        _tool_records: &'a [ToolCallRecord],
        _segments: &'a [ChatMessageSegment],
        _api_messages: &'a [serde_json::Value],
    ) -> AgentHostFuture<'a, ()> {
        Box::pin(async {})
    }

    fn request_tool_approval<'a>(
        &'a self,
        ctx: &'a ToolExecutionContext<'a>,
        record: &'a ToolCallRecord,
    ) -> AgentHostFuture<'a, bool>;

    /// Ask the user once per conversation to authorize the file/shell tool
    /// family (full-disk read/write + command execution). Hosts that can prompt
    /// (the chat window) surface a consent dialog and cache the grant; hosts
    /// that cannot (sub-agents) deny. Default denies — a host must opt in.
    fn request_session_consent<'a>(
        &'a self,
        _ctx: &'a ToolExecutionContext<'a>,
    ) -> AgentHostFuture<'a, bool> {
        Box::pin(async { false })
    }

    fn request_user_response<'a>(
        &'a self,
        ctx: &'a ToolExecutionContext<'a>,
        record: &'a ToolCallRecord,
        prompt: AskUserPromptPayload,
    ) -> AgentHostFuture<'a, AskUserResponseResult>;

    fn is_generation_active(&self, conversation_id: &str, generation: u64) -> bool;

    /// 运行中用户插话（steering）：轮首取走该会话待注入的用户消息（take 语义，取一次清一次）。
    /// 默认空 —— 只有能收到用户输入的宿主（GUI chat）才有这条通道；子 agent / probe / 测试保持默认。
    fn take_steering_messages(&self, _conversation_id: &str) -> Vec<super::SteeringMessage> {
        Vec::new()
    }

    /// 原生 follow-up：终答边界取走（take 语义）。不在轮首注入，避免打断还在跑的工具循环。
    /// 默认空；GUI chat 才有这条通道。
    fn take_follow_up_messages(&self, _conversation_id: &str) -> Vec<super::SteeringMessage> {
        Vec::new()
    }

    fn wait_for_generation_inactive<'a>(
        &'a self,
        conversation_id: &'a str,
        generation: u64,
    ) -> AgentHostFuture<'a, ()>;

    /// User-configured lifecycle hooks for this run, or `None` when the host has
    /// none (sub-agents, probe, tests) or the user configured zero enabled hooks.
    /// `None` is the whole zero-cost path: the loop never builds a payload.
    fn hooks(&self) -> Option<&HookDispatcher> {
        None
    }
}
