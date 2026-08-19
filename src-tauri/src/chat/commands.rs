use super::vision::image_content_part;
use super::{
    AgentPlanState, ChatMessage, ChatMessageSegment, ChatMessageSegmentKind,
    ChatMessageSegmentPhase, Conversation, ToolCallRecord, ToolCallStatus,
};

mod agent_host;

pub(crate) mod attachments;

pub(crate) mod catalog;

pub(crate) use catalog::create_assistant_via_builder;

pub(crate) mod context;

pub(crate) mod interaction;

pub(crate) mod title;

mod tooling;

mod messages;

mod sanitization;

mod reply_runtime;
use reply_runtime::{ChatSendReservation, CHAT_REPLY_BUSY_ERROR, MAX_REPLY_MODELS};

mod fan_out;

pub(crate) mod send;

pub(crate) mod reasoning;
use reasoning::resolve_thinking;

mod vision_compat;
pub(crate) use vision_compat::{attach_image_artifacts_for_model, read_image_as_tool_result};

mod reply;
use reply::{agent_run_entry_label, complete_assistant_reply, complete_assistant_reply_inner};

mod direct_image;

#[cfg(debug_assertions)]
mod probe_runtime;
#[cfg(debug_assertions)]
pub(crate) use probe_runtime::run_chat_probe;

pub(crate) mod mutations;

pub(crate) use interaction::{emit_chat_stream_delta, emit_chat_tool_record};
pub(crate) use messages::push_assistant_message;
use tooling::{
    append_agent_ask_user_tools, append_agent_todo_tools, apply_agent_plan_tool_filter,
    apply_chat_mode_tool_filter, apply_inline_code_request_tool_filter, list_tools_for_chat,
    resolve_forced_skill_id,
};

#[cfg(test)]
mod tests;
