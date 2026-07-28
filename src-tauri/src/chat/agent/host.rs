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

    fn emit_stream_done(
        &self,
        conversation_id: &str,
        run_id: &str,
        message_id: &str,
        reason: &str,
        full: &str,
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
    fn persist_partial_assistant(
        &self,
        _conversation_id: &str,
        _message_id: &str,
        _tool_records: &[ToolCallRecord],
        _segments: &[ChatMessageSegment],
        _api_messages: &[serde_json::Value],
    ) {
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
