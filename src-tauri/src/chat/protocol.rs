use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(debug_assertions)]
use tauri::Emitter;
use tauri::{AppHandle, Manager};
use ts_rs::TS;

use crate::state::AppState;

pub const CHAT_PROTOCOL_VERSION: u32 = 1;
pub const CHAT_PROTOCOL_EVENT: &str = "chat-protocol";
const MAX_REPLAY_EVENTS: usize = 512;
const MAX_REPLAY_BYTES: usize = 2 * 1024 * 1024;
const COMPLETED_RUN_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_COMPLETED_RUNS: usize = 32;
/// Text/Reasoning delta 的合帧窗口。多对话并发时「每 token 一条 IPC 事件」是压垮
/// WebView 主线程和协议锁的主因；窗口内的同段 delta 合并成一条再入库+emit。
/// 前端渲染本身按 50–220ms 合帧，25ms 的后端窗口对观感不可见。
const DELTA_COALESCE_WINDOW: Duration = Duration::from_millis(25);
/// 单条合帧缓冲的尺寸上限：SSE 偶发的大块（整段贴文）不该在缓冲里再攒一份。
const DELTA_COALESCE_MAX_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ChatProtocolScope {
    Run,
    Conversation,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ChatSegmentKind {
    Text,
    Reasoning,
    Tool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ChatSegmentPhase {
    Auxiliary,
    Plain,
    ToolLoop,
    Synthesis,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatSegmentPayload {
    pub id: String,
    pub kind: ChatSegmentKind,
    pub phase: ChatSegmentPhase,
    pub order: u32,
    pub step_number: Option<u8>,
    pub round: Option<u32>,
    pub text: Option<String>,
    pub tool_call_id: Option<String>,
}

impl From<&crate::chat::ChatMessageSegment> for ChatSegmentPayload {
    fn from(segment: &crate::chat::ChatMessageSegment) -> Self {
        Self {
            id: segment.id.clone(),
            kind: match segment.kind {
                crate::chat::ChatMessageSegmentKind::Text => ChatSegmentKind::Text,
                crate::chat::ChatMessageSegmentKind::Reasoning => ChatSegmentKind::Reasoning,
                crate::chat::ChatMessageSegmentKind::Tool => ChatSegmentKind::Tool,
            },
            phase: match segment.phase {
                crate::chat::ChatMessageSegmentPhase::Auxiliary => ChatSegmentPhase::Auxiliary,
                crate::chat::ChatMessageSegmentPhase::Plain => ChatSegmentPhase::Plain,
                crate::chat::ChatMessageSegmentPhase::ToolLoop => ChatSegmentPhase::ToolLoop,
                crate::chat::ChatMessageSegmentPhase::Synthesis => ChatSegmentPhase::Synthesis,
            },
            order: segment.order,
            step_number: segment.step_number,
            round: segment.round,
            text: segment.text.clone(),
            tool_call_id: segment.tool_call_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatToolArtifactPayload {
    pub id: Option<String>,
    pub name: String,
    pub mime_type: String,
    pub data_url: String,
    pub size_bytes: Option<u64>,
    pub path: Option<String>,
}

impl From<&crate::mcp::types::ChatToolArtifact> for ChatToolArtifactPayload {
    fn from(artifact: &crate::mcp::types::ChatToolArtifact) -> Self {
        Self {
            id: artifact.id.clone(),
            name: artifact.name.clone(),
            mime_type: artifact.mime_type.clone(),
            data_url: artifact.data_url.clone(),
            size_bytes: artifact.size_bytes,
            path: artifact.path.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatToolPayload {
    pub id: String,
    pub name: String,
    pub source: String,
    pub server_id: Option<String>,
    pub status: String,
    pub arguments_preview: String,
    pub result_preview: Option<String>,
    pub error: Option<String>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub duration_ms: Option<u64>,
    pub round: u32,
    pub sensitive: bool,
    pub artifacts: Vec<ChatToolArtifactPayload>,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    #[ts(type = "unknown")]
    pub structured_content: Option<Value>,
}

impl ChatToolPayload {
    pub fn from_record(record: &crate::chat::ToolCallRecord, arguments_preview: String) -> Self {
        let status = serde_json::to_value(&record.status)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "pending".to_string());
        Self {
            id: record.id.clone(),
            name: record.name.clone(),
            source: record.source.clone(),
            server_id: record.server_id.clone(),
            status,
            arguments_preview,
            result_preview: record.result_preview.clone(),
            error: record.error.clone(),
            started_at: record.started_at,
            completed_at: record.completed_at,
            duration_ms: record.duration_ms,
            round: record.round,
            sensitive: record.sensitive,
            artifacts: record.artifacts.iter().map(Into::into).collect(),
            trace_id: record.trace_id.clone(),
            span_id: record.span_id.clone(),
            structured_content: record.structured_content.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatContextUsagePayload {
    pub used_tokens: u64,
    pub context_window_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatRunRecoveryMetadata {
    pub group_id: String,
    pub group_size: u32,
    pub arm_index: u32,
    pub provider_id: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ChatTodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatTodoItemPayload {
    pub id: String,
    pub content: String,
    pub description: Option<String>,
    pub status: ChatTodoStatus,
    pub blocks: Vec<String>,
    pub blocked_by: Vec<String>,
    pub owner: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatTodoStatePayload {
    pub items: Vec<ChatTodoItemPayload>,
    pub updated_at: i64,
}

impl From<&crate::chat::AgentTodoState> for ChatTodoStatePayload {
    fn from(state: &crate::chat::AgentTodoState) -> Self {
        Self {
            items: state
                .items
                .iter()
                .map(|item| ChatTodoItemPayload {
                    id: item.id.clone(),
                    content: item.content.clone(),
                    description: item.description.clone(),
                    status: match item.status {
                        crate::chat::AgentTodoStatus::Pending => ChatTodoStatus::Pending,
                        crate::chat::AgentTodoStatus::InProgress => ChatTodoStatus::InProgress,
                        crate::chat::AgentTodoStatus::Completed => ChatTodoStatus::Completed,
                        crate::chat::AgentTodoStatus::Cancelled => ChatTodoStatus::Cancelled,
                    },
                    blocks: item.blocks.clone(),
                    blocked_by: item.blocked_by.clone(),
                    owner: item.owner.clone(),
                })
                .collect(),
            updated_at: state.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ChatPlanMode {
    Act,
    Plan,
    Orchestrate,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ChatPlanStatus {
    Empty,
    Draft,
    Approved,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatPlanStatePayload {
    pub mode: ChatPlanMode,
    pub status: ChatPlanStatus,
    pub plan: Option<String>,
    pub updated_at: i64,
}

impl From<&crate::chat::AgentPlanState> for ChatPlanStatePayload {
    fn from(state: &crate::chat::AgentPlanState) -> Self {
        Self {
            mode: match state.mode {
                crate::chat::AgentPlanMode::Act => ChatPlanMode::Act,
                crate::chat::AgentPlanMode::Plan => ChatPlanMode::Plan,
                crate::chat::AgentPlanMode::Orchestrate => ChatPlanMode::Orchestrate,
            },
            status: match state.status {
                crate::chat::AgentPlanStatus::Empty => ChatPlanStatus::Empty,
                crate::chat::AgentPlanStatus::Draft => ChatPlanStatus::Draft,
                crate::chat::AgentPlanStatus::Approved => ChatPlanStatus::Approved,
            },
            plan: state.plan.clone(),
            updated_at: state.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatCompactionBoundaryPayload {
    pub id: String,
    pub source_until_message_id: String,
    pub display_after_message_id: Option<String>,
    pub token_estimate_before: u64,
    pub token_estimate_after: u64,
    pub summary_content: String,
    pub trigger: String,
    pub created_at: i64,
}

impl From<&crate::chat::CompactionBoundaryRecord> for ChatCompactionBoundaryPayload {
    fn from(value: &crate::chat::CompactionBoundaryRecord) -> Self {
        Self {
            id: value.id.clone(),
            source_until_message_id: value.source_until_message_id.clone(),
            display_after_message_id: value.display_after_message_id.clone(),
            token_estimate_before: value.token_estimate_before as u64,
            token_estimate_after: value.token_estimate_after as u64,
            summary_content: value.summary_content.clone(),
            trigger: value.trigger.clone(),
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatContextClearBoundaryPayload {
    pub id: String,
    pub source_until_message_id: String,
    pub created_at: i64,
}

impl From<&crate::chat::ContextClearBoundaryRecord> for ChatContextClearBoundaryPayload {
    fn from(value: &crate::chat::ContextClearBoundaryRecord) -> Self {
        Self {
            id: value.id.clone(),
            source_until_message_id: value.source_until_message_id.clone(),
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatContextUsageSegmentPayload {
    pub id: String,
    pub label: String,
    pub estimated_tokens: u64,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatFileLedgerPayload {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
    pub omitted_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatContextSummaryPayload {
    pub id: String,
    pub content: String,
    pub source_message_ids: Vec<String>,
    pub source_until_message_id: String,
    pub token_estimate_before: u64,
    pub token_estimate_after: u64,
    pub created_at: i64,
    pub provider_id: String,
    pub model: String,
    pub stale: bool,
    pub file_ledger: Option<ChatFileLedgerPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatContextStatePayload {
    pub estimated_input_tokens: u64,
    pub context_window_tokens: Option<u64>,
    pub context_window_estimated: bool,
    pub usage_ratio: Option<f32>,
    pub status: String,
    pub segments: Vec<ChatContextUsageSegmentPayload>,
    pub last_measured_at: i64,
    pub last_compressed_at: Option<i64>,
    pub compressed_message_count: u64,
    pub compression_count: u64,
    pub summary: Option<ChatContextSummaryPayload>,
    pub compaction_boundaries: Vec<ChatCompactionBoundaryPayload>,
    pub clear_boundaries: Vec<ChatContextClearBoundaryPayload>,
    pub warning: Option<String>,
    pub context_source: Option<String>,
    pub token_count_source: Option<String>,
    pub session_input_tokens: Option<u64>,
    pub session_output_tokens: Option<u64>,
    pub external_agent_id: Option<String>,
    pub external_model: Option<String>,
}

impl From<&crate::chat::ConversationContextState> for ChatContextStatePayload {
    fn from(state: &crate::chat::ConversationContextState) -> Self {
        Self {
            estimated_input_tokens: state.estimated_input_tokens as u64,
            context_window_tokens: state.context_window_tokens.map(|value| value as u64),
            context_window_estimated: state.context_window_estimated,
            usage_ratio: state.usage_ratio,
            status: state.status.clone(),
            segments: state
                .segments
                .iter()
                .map(|segment| ChatContextUsageSegmentPayload {
                    id: segment.id.clone(),
                    label: segment.label.clone(),
                    estimated_tokens: segment.estimated_tokens as u64,
                    color: segment.color.clone(),
                })
                .collect(),
            last_measured_at: state.last_measured_at,
            last_compressed_at: state.last_compressed_at,
            compressed_message_count: state.compressed_message_count as u64,
            compression_count: state.compression_count as u64,
            summary: state
                .summary
                .as_ref()
                .map(|summary| ChatContextSummaryPayload {
                    id: summary.id.clone(),
                    content: summary.content.clone(),
                    source_message_ids: summary.source_message_ids.clone(),
                    source_until_message_id: summary.source_until_message_id.clone(),
                    token_estimate_before: summary.token_estimate_before as u64,
                    token_estimate_after: summary.token_estimate_after as u64,
                    created_at: summary.created_at,
                    provider_id: summary.provider_id.clone(),
                    model: summary.model.clone(),
                    stale: summary.stale,
                    file_ledger: summary
                        .file_ledger
                        .as_ref()
                        .map(|ledger| ChatFileLedgerPayload {
                            read_files: ledger.read_files.clone(),
                            modified_files: ledger.modified_files.clone(),
                            omitted_count: ledger.omitted_count as u64,
                        }),
                }),
            compaction_boundaries: state.compaction_boundaries.iter().map(Into::into).collect(),
            clear_boundaries: state.clear_boundaries.iter().map(Into::into).collect(),
            warning: state.warning.clone(),
            context_source: state.context_source.clone(),
            token_count_source: state.token_count_source.clone(),
            session_input_tokens: state.session_input_tokens.map(|value| value as u64),
            session_output_tokens: state.session_output_tokens.map(|value| value as u64),
            external_agent_id: state.external_agent_id.clone(),
            external_model: state.external_model.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatAskUserOptionPayload {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatAskUserQuestionPayload {
    pub id: String,
    pub prompt: String,
    pub options: Vec<ChatAskUserOptionPayload>,
    pub allow_multiple: bool,
    pub allow_custom: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatAskUserPromptPayload {
    pub title: Option<String>,
    pub questions: Vec<ChatAskUserQuestionPayload>,
}

impl From<&crate::chat::ask_user::AskUserPromptPayload> for ChatAskUserPromptPayload {
    fn from(prompt: &crate::chat::ask_user::AskUserPromptPayload) -> Self {
        Self {
            title: prompt.title.clone(),
            questions: prompt
                .questions
                .iter()
                .map(|question| ChatAskUserQuestionPayload {
                    id: question.id.clone(),
                    prompt: question.prompt.clone(),
                    options: question
                        .options
                        .iter()
                        .map(|option| ChatAskUserOptionPayload {
                            id: option.id.clone(),
                            label: option.label.clone(),
                            description: option.description.clone(),
                        })
                        .collect(),
                    allow_multiple: question.allow_multiple,
                    allow_custom: question.allow_custom,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[ts(tag = "type", rename_all = "snake_case")]
pub enum ChatRunEvent {
    RunStarted {
        recovery: Option<ChatRunRecoveryMetadata>,
    },
    TextDelta {
        delta: String,
        segment: Option<ChatSegmentPayload>,
    },
    ReasoningDelta {
        delta: String,
        segment: Option<ChatSegmentPayload>,
    },
    ToolUpdated {
        tool: ChatToolPayload,
    },
    SubagentUpdated {
        parent_tool_call_id: String,
        task_id: String,
        name: String,
        model: Option<String>,
        depth: u8,
        status: String,
        preview: Option<String>,
        steps: Vec<String>,
    },
    ContextUsageUpdated {
        usage: ChatContextUsagePayload,
    },
    CompactionUpdated {
        phase: String,
        trigger: Option<String>,
        boundary: Option<ChatCompactionBoundaryPayload>,
    },
    TodoUpdated {
        todo_state: ChatTodoStatePayload,
    },
    PlanUpdated {
        plan_state: ChatPlanStatePayload,
    },
    SessionConsentRequested,
    ToolApprovalRequested {
        tool_call_id: String,
        name: String,
        source: String,
        server_id: Option<String>,
        target: Option<String>,
        arguments_preview: String,
        sensitivity: String,
    },
    ToolApprovalWithdrawn {
        tool_call_id: String,
    },
    UserPromptRequested {
        tool_call_id: String,
        name: String,
        source: String,
        prompt: ChatAskUserPromptPayload,
        #[ts(type = "unknown")]
        structured_content: Option<Value>,
    },
    HookFailed {
        hook_name: String,
        event: String,
        message: String,
    },
    /// 生成过程的**瞬态状态一行字**（claude `api_retry`、codex 重连进度等）。
    /// 挂在流状态行（StreamStatusLine）上而不是消息正文——正文一恢复流动前端就清掉。
    /// `note: None` = 显式清除。
    StatusNoteUpdated {
        note: Option<String>,
    },
    RunCompleted {
        full: String,
        conversation_revision: u64,
    },
    RunFailed {
        error: String,
        full: String,
        conversation_revision: u64,
    },
    RunCancelled {
        full: String,
        conversation_revision: u64,
    },
}

impl ChatRunEvent {
    fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::RunCompleted { .. } | Self::RunFailed { .. } | Self::RunCancelled { .. }
        )
    }

    fn is_hook_failed(&self) -> bool {
        matches!(self, Self::HookFailed { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatRunEventEnvelope {
    #[ts(type = "typeof CHAT_PROTOCOL_VERSION")]
    pub protocol_version: u32,
    #[ts(type = "\"run\"")]
    pub scope: ChatProtocolScope,
    pub conversation_id: String,
    pub run_id: String,
    pub message_id: String,
    pub seq: u64,
    pub base_revision: u64,
    #[serde(flatten)]
    #[ts(flatten)]
    pub event: ChatRunEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[ts(tag = "type", rename_all = "snake_case")]
pub enum ChatConversationEvent {
    ContextUpdated {
        context_state: ChatContextStatePayload,
    },
    TodoUpdated {
        todo_state: ChatTodoStatePayload,
    },
    PlanUpdated {
        plan_state: ChatPlanStatePayload,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatConversationEventEnvelope {
    #[ts(type = "typeof CHAT_PROTOCOL_VERSION")]
    pub protocol_version: u32,
    #[ts(type = "\"conversation\"")]
    pub scope: ChatProtocolScope,
    pub conversation_id: String,
    pub revision: u64,
    #[serde(flatten)]
    #[ts(flatten)]
    pub event: ChatConversationEvent,
}

#[derive(Debug, Clone, Serialize, JsonSchema, TS, PartialEq)]
#[serde(untagged)]
#[ts(untagged)]
pub enum ChatProtocolEvent {
    Run(ChatRunEventEnvelope),
    Conversation(ChatConversationEventEnvelope),
}

impl<'de> Deserialize<'de> for ChatProtocolEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct RawRunEnvelope {
            protocol_version: u32,
            scope: ChatProtocolScope,
            conversation_id: String,
            run_id: String,
            message_id: String,
            seq: u64,
            base_revision: u64,
            #[serde(flatten)]
            event: ChatRunEvent,
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct RawConversationEnvelope {
            protocol_version: u32,
            scope: ChatProtocolScope,
            conversation_id: String,
            revision: u64,
            #[serde(flatten)]
            event: ChatConversationEvent,
        }

        let value = Value::deserialize(deserializer)?;
        let scope = value
            .get("scope")
            .and_then(Value::as_str)
            .ok_or_else(|| serde::de::Error::custom("chat protocol scope is missing"))?;
        let event = match scope {
            "run" => {
                let raw: RawRunEnvelope =
                    serde_json::from_value(value.clone()).map_err(serde::de::Error::custom)?;
                if raw.protocol_version != CHAT_PROTOCOL_VERSION {
                    return Err(serde::de::Error::custom(format!(
                        "chat protocol version mismatch: expected {}, received {}",
                        CHAT_PROTOCOL_VERSION, raw.protocol_version
                    )));
                }
                if raw.scope != ChatProtocolScope::Run {
                    return Err(serde::de::Error::custom("invalid run scope"));
                }
                Self::Run(ChatRunEventEnvelope {
                    protocol_version: raw.protocol_version,
                    scope: raw.scope,
                    conversation_id: raw.conversation_id,
                    run_id: raw.run_id,
                    message_id: raw.message_id,
                    seq: raw.seq,
                    base_revision: raw.base_revision,
                    event: raw.event,
                })
            }
            "conversation" => {
                let raw: RawConversationEnvelope =
                    serde_json::from_value(value.clone()).map_err(serde::de::Error::custom)?;
                if raw.protocol_version != CHAT_PROTOCOL_VERSION {
                    return Err(serde::de::Error::custom(format!(
                        "chat protocol version mismatch: expected {}, received {}",
                        CHAT_PROTOCOL_VERSION, raw.protocol_version
                    )));
                }
                if raw.scope != ChatProtocolScope::Conversation {
                    return Err(serde::de::Error::custom("invalid conversation scope"));
                }
                Self::Conversation(ChatConversationEventEnvelope {
                    protocol_version: raw.protocol_version,
                    scope: raw.scope,
                    conversation_id: raw.conversation_id,
                    revision: raw.revision,
                    event: raw.event,
                })
            }
            _ => return Err(serde::de::Error::custom("unknown chat protocol scope")),
        };
        let canonical = serde_json::to_value(&event).map_err(serde::de::Error::custom)?;
        if canonical != value {
            return Err(serde::de::Error::custom(
                "chat protocol event contains unknown or non-canonical fields",
            ));
        }
        Ok(event)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ChatRunStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[ts(tag = "type", rename_all = "snake_case")]
pub enum ChatSubagentSnapshot {
    SubagentUpdated {
        parent_tool_call_id: String,
        task_id: String,
        name: String,
        model: Option<String>,
        depth: u8,
        status: String,
        preview: Option<String>,
        steps: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[ts(tag = "type", rename_all = "snake_case")]
pub enum ChatCompactionSnapshot {
    CompactionUpdated {
        phase: String,
        trigger: Option<String>,
        boundary: Option<ChatCompactionBoundaryPayload>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[ts(tag = "type", rename_all = "snake_case")]
pub enum ChatPendingInteractionSnapshot {
    SessionConsentRequested,
    ToolApprovalRequested {
        tool_call_id: String,
        name: String,
        source: String,
        server_id: Option<String>,
        target: Option<String>,
        arguments_preview: String,
        sensitivity: String,
    },
    UserPromptRequested {
        tool_call_id: String,
        name: String,
        source: String,
        prompt: ChatAskUserPromptPayload,
        #[ts(type = "unknown")]
        structured_content: Option<Value>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[ts(tag = "type", rename_all = "snake_case")]
pub enum ChatWarningSnapshot {
    HookFailed {
        hook_name: String,
        event: String,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[ts(tag = "type", rename_all = "snake_case")]
pub enum ChatTerminalSnapshot {
    RunCompleted {
        full: String,
        conversation_revision: u64,
    },
    RunFailed {
        error: String,
        full: String,
        conversation_revision: u64,
    },
    RunCancelled {
        full: String,
        conversation_revision: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatRunSnapshot {
    #[ts(type = "typeof CHAT_PROTOCOL_VERSION")]
    pub protocol_version: u32,
    pub conversation_id: String,
    pub run_id: String,
    pub message_id: String,
    pub last_seq: u64,
    pub base_revision: u64,
    pub recovery: Option<ChatRunRecoveryMetadata>,
    pub status: ChatRunStatus,
    pub content: String,
    pub reasoning: String,
    pub segments: Vec<ChatSegmentPayload>,
    pub tools: Vec<ChatToolPayload>,
    pub context_usage: Option<ChatContextUsagePayload>,
    pub subagents: Vec<ChatSubagentSnapshot>,
    pub compaction: Option<ChatCompactionSnapshot>,
    pub todo_state: Option<ChatTodoStatePayload>,
    pub plan_state: Option<ChatPlanStatePayload>,
    pub pending_interactions: Vec<ChatPendingInteractionSnapshot>,
    pub warnings: Vec<ChatWarningSnapshot>,
    /// 流状态行上的瞬态一行字（上游重试等）。见 `ChatRunEvent::StatusNoteUpdated`。
    pub status_note: Option<String>,
    pub terminal: Option<ChatTerminalSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatRunCursor {
    pub run_id: String,
    pub last_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatSyncRequest {
    #[ts(type = "typeof CHAT_PROTOCOL_VERSION")]
    pub protocol_version: u32,
    pub conversation_id: String,
    pub cursors: Vec<ChatRunCursor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[ts(tag = "kind", rename_all = "snake_case")]
pub enum ChatRunSync {
    Events {
        run_id: String,
        from_seq: u64,
        through_seq: u64,
        events: Vec<ChatRunEventEnvelope>,
    },
    Snapshot {
        snapshot: ChatRunSnapshot,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ChatSyncResult {
    #[ts(type = "typeof CHAT_PROTOCOL_VERSION")]
    pub protocol_version: u32,
    pub conversation_revision: u64,
    pub missing_run_ids: Vec<String>,
    pub runs: Vec<ChatRunSync>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingDeltaKind {
    Text,
    Reasoning,
}

/// 合帧缓冲：还没入库（没有 seq、不在 replay/snapshot 里）的流式增量。
/// 只有「同 kind + 段 payload 完全相等」的连续 delta 会并进来，所以冲刷时
/// 折叠结果与逐条入库逐字节一致。
#[derive(Debug)]
struct PendingDelta {
    kind: PendingDeltaKind,
    delta: String,
    segment: Option<ChatSegmentPayload>,
    buffered_at: Instant,
}

impl PendingDelta {
    fn into_event(self) -> ChatRunEvent {
        match self.kind {
            PendingDeltaKind::Text => ChatRunEvent::TextDelta {
                delta: self.delta,
                segment: self.segment,
            },
            PendingDeltaKind::Reasoning => ChatRunEvent::ReasoningDelta {
                delta: self.delta,
                segment: self.segment,
            },
        }
    }
}

struct RunState {
    snapshot: ChatRunSnapshot,
    replay: VecDeque<(usize, ChatRunEventEnvelope)>,
    replay_bytes: usize,
    terminal_at: Option<Instant>,
    pending_delta: Option<PendingDelta>,
    /// 已有一个延迟冲刷任务在飞，别再叠加计时器。
    flush_scheduled: bool,
}

#[derive(Default)]
pub struct ChatProtocolHub {
    runs: HashMap<String, RunState>,
}

impl ChatProtocolHub {
    fn prune(&mut self) {
        let now = Instant::now();
        self.runs.retain(|_, run| {
            run.terminal_at.map_or(true, |finished| {
                now.duration_since(finished) <= COMPLETED_RUN_TTL
            })
        });
        let mut completed: Vec<_> = self
            .runs
            .iter()
            .filter_map(|(id, run)| run.terminal_at.map(|at| (id.clone(), at)))
            .collect();
        completed.sort_by_key(|(_, at)| *at);
        let remove_count = completed.len().saturating_sub(MAX_COMPLETED_RUNS);
        for (id, _) in completed.into_iter().take(remove_count) {
            self.runs.remove(&id);
        }
    }

    #[cfg(test)]
    fn register(
        &mut self,
        conversation_id: &str,
        run_id: &str,
        message_id: &str,
        base_revision: u64,
    ) -> Option<ChatRunEventEnvelope> {
        self.register_with_recovery(conversation_id, run_id, message_id, base_revision, None)
    }

    fn register_with_recovery(
        &mut self,
        conversation_id: &str,
        run_id: &str,
        message_id: &str,
        base_revision: u64,
        recovery: Option<ChatRunRecoveryMetadata>,
    ) -> Option<ChatRunEventEnvelope> {
        self.prune();
        if self.runs.contains_key(run_id) {
            return None;
        }
        let snapshot = ChatRunSnapshot {
            protocol_version: CHAT_PROTOCOL_VERSION,
            conversation_id: conversation_id.to_string(),
            run_id: run_id.to_string(),
            message_id: message_id.to_string(),
            last_seq: 0,
            base_revision,
            recovery: recovery.clone(),
            status: ChatRunStatus::Running,
            content: String::new(),
            reasoning: String::new(),
            segments: Vec::new(),
            tools: Vec::new(),
            context_usage: None,
            subagents: Vec::new(),
            compaction: None,
            todo_state: None,
            plan_state: None,
            pending_interactions: Vec::new(),
            warnings: Vec::new(),
            status_note: None,
            terminal: None,
        };
        self.runs.insert(
            run_id.to_string(),
            RunState {
                snapshot,
                replay: VecDeque::new(),
                replay_bytes: 0,
                terminal_at: None,
                pending_delta: None,
                flush_scheduled: false,
            },
        );
        self.push(run_id, ChatRunEvent::RunStarted { recovery })
            .ok()
    }

    fn push(&mut self, run_id: &str, event: ChatRunEvent) -> Result<ChatRunEventEnvelope, String> {
        let run = self
            .runs
            .get_mut(run_id)
            .ok_or_else(|| format!("chat protocol run is not registered: {run_id}"))?;
        // HookFailed 是旁路警告（对齐 Pi 的 extension_error），不该被 run 封口绑死。
        // agent_end 脚本常常在 finish_run 之后才跑完；其它内容事件仍拒绝。
        if run.terminal_at.is_some() && !event.is_hook_failed() {
            return Err(format!("chat protocol run is already terminal: {run_id}"));
        }
        let seq = run.snapshot.last_seq.saturating_add(1);
        let envelope = ChatRunEventEnvelope {
            protocol_version: CHAT_PROTOCOL_VERSION,
            scope: ChatProtocolScope::Run,
            conversation_id: run.snapshot.conversation_id.clone(),
            run_id: run.snapshot.run_id.clone(),
            message_id: run.snapshot.message_id.clone(),
            seq,
            base_revision: run.snapshot.base_revision,
            event,
        };
        fold_snapshot(&mut run.snapshot, &envelope.event);
        run.snapshot.last_seq = seq;
        if envelope.event.is_terminal() {
            run.terminal_at = Some(Instant::now());
        }
        let bytes = replay_bytes_estimate(&envelope);
        run.replay.push_back((bytes, envelope.clone()));
        run.replay_bytes = run.replay_bytes.saturating_add(bytes);
        while run.replay.len() > MAX_REPLAY_EVENTS || run.replay_bytes > MAX_REPLAY_BYTES {
            if let Some((removed, _)) = run.replay.pop_front() {
                run.replay_bytes = run.replay_bytes.saturating_sub(removed);
            } else {
                break;
            }
        }
        if envelope.event.is_terminal() {
            self.prune();
        }
        Ok(envelope)
    }

    /// 把该 run 的合帧缓冲作为一条正式事件入库（拿 seq、进 replay/snapshot），
    /// 返回待 emit 的 envelope。没有缓冲或 run 已不在时是 no-op。
    fn flush_pending_delta(&mut self, run_id: &str) -> Option<ChatRunEventEnvelope> {
        let pending = self.runs.get_mut(run_id)?.pending_delta.take()?;
        self.push(run_id, pending.into_event()).ok()
    }

    /// 尝试把一条流式 delta 并入合帧缓冲。`Err` 把 delta 原样退回，让调用方走常规
    /// `push`——run 不存在 / 已终态的错误信息是 push 的对外契约，不在这里复刻。
    /// `Ok` 返回（需要立即 emit 的已入库事件, 是否要调度一次延迟冲刷）。
    fn buffer_delta(
        &mut self,
        run_id: &str,
        pending: PendingDelta,
    ) -> Result<(Vec<ChatRunEventEnvelope>, bool), PendingDelta> {
        let now = pending.buffered_at;
        let (previous, flush_now, schedule_flush) = {
            let Some(run) = self.runs.get_mut(run_id) else {
                return Err(pending);
            };
            if run.terminal_at.is_some() {
                return Err(pending);
            }
            let previous = match run.pending_delta.as_mut() {
                Some(current)
                    if current.kind == pending.kind && current.segment == pending.segment =>
                {
                    current.delta.push_str(&pending.delta);
                    None
                }
                // 段位或种类切换：旧缓冲先按原顺序入库，再另起新缓冲。
                _ => {
                    let previous = run.pending_delta.take();
                    run.pending_delta = Some(pending);
                    previous
                }
            };
            let current = run
                .pending_delta
                .as_ref()
                .expect("pending delta was just set");
            let flush_now = current.delta.len() >= DELTA_COALESCE_MAX_BYTES
                || now.duration_since(current.buffered_at) >= DELTA_COALESCE_WINDOW;
            let schedule_flush = !flush_now && !run.flush_scheduled;
            if schedule_flush {
                run.flush_scheduled = true;
            }
            (previous, flush_now, schedule_flush)
        };
        let mut envelopes = Vec::new();
        if let Some(previous) = previous {
            if let Ok(envelope) = self.push(run_id, previous.into_event()) {
                envelopes.push(envelope);
            }
        }
        if flush_now {
            if let Some(envelope) = self.flush_pending_delta(run_id) {
                envelopes.push(envelope);
            }
        }
        Ok((envelopes, schedule_flush))
    }

    fn sync(&mut self, request: &ChatSyncRequest) -> ChatSyncResult {
        self.prune();
        let cursors: HashMap<_, _> = request
            .cursors
            .iter()
            .map(|cursor| (cursor.run_id.as_str(), cursor.last_seq))
            .collect();
        let known_run_ids: std::collections::HashSet<_> = self
            .runs
            .values()
            .filter(|run| run.snapshot.conversation_id == request.conversation_id)
            .map(|run| run.snapshot.run_id.as_str())
            .collect();
        let missing_run_ids = request
            .cursors
            .iter()
            .filter(|cursor| !known_run_ids.contains(cursor.run_id.as_str()))
            .map(|cursor| cursor.run_id.clone())
            .collect();
        let mut runs = Vec::new();
        for run in self
            .runs
            .values()
            .filter(|run| run.snapshot.conversation_id == request.conversation_id)
        {
            let Some(last_seq) = cursors.get(run.snapshot.run_id.as_str()).copied() else {
                if run.terminal_at.is_none() {
                    runs.push(ChatRunSync::Snapshot {
                        snapshot: run.snapshot.clone(),
                    });
                }
                continue;
            };
            let cursor_has_gap = last_seq > run.snapshot.last_seq
                || match run.replay.front() {
                    Some((_, first_available)) => last_seq.saturating_add(1) < first_available.seq,
                    None => last_seq < run.snapshot.last_seq,
                };
            if cursor_has_gap {
                runs.push(ChatRunSync::Snapshot {
                    snapshot: run.snapshot.clone(),
                });
                continue;
            }
            let events: Vec<_> = run
                .replay
                .iter()
                .filter(|(_, event)| event.seq > last_seq)
                .map(|(_, event)| event.clone())
                .collect();
            runs.push(ChatRunSync::Events {
                run_id: run.snapshot.run_id.clone(),
                from_seq: last_seq.saturating_add(1),
                through_seq: run.snapshot.last_seq,
                events,
            });
        }
        ChatSyncResult {
            protocol_version: CHAT_PROTOCOL_VERSION,
            conversation_revision: 0,
            missing_run_ids,
            runs,
        }
    }

    fn withdraw_tool_approval(&mut self, tool_call_id: &str) -> Option<ChatRunEventEnvelope> {
        let run_id = self.runs.iter().find_map(|(run_id, run)| {
            run.snapshot
                .pending_interactions
                .iter()
                .any(|event| {
                    matches!(
                        event,
                        ChatPendingInteractionSnapshot::ToolApprovalRequested {
                            tool_call_id: pending_id,
                            ..
                        } if pending_id == tool_call_id
                    )
                })
                .then(|| run_id.clone())
        })?;
        self.push(
            &run_id,
            ChatRunEvent::ToolApprovalWithdrawn {
                tool_call_id: tool_call_id.to_string(),
            },
        )
        .ok()
    }

    fn resolve_session_consent(&mut self, run_id: &str) {
        let Some(run) = self.runs.get_mut(run_id) else {
            return;
        };
        run.snapshot.pending_interactions.retain(|pending| {
            !matches!(
                pending,
                ChatPendingInteractionSnapshot::SessionConsentRequested
            )
        });
    }

    fn resolve_user_prompt(&mut self, run_id: &str, tool_call_id: &str) {
        let Some(run) = self.runs.get_mut(run_id) else {
            return;
        };
        run.snapshot.pending_interactions.retain(|pending| {
            !matches!(
                pending,
                ChatPendingInteractionSnapshot::UserPromptRequested {
                    tool_call_id: pending_id,
                    ..
                } if pending_id == tool_call_id
            )
        });
    }
}

fn fold_snapshot(snapshot: &mut ChatRunSnapshot, event: &ChatRunEvent) {
    match event {
        ChatRunEvent::TextDelta { delta, segment } => {
            snapshot.content.push_str(delta);
            upsert_segment(&mut snapshot.segments, segment, delta);
        }
        ChatRunEvent::ReasoningDelta { delta, segment } => {
            snapshot.reasoning.push_str(delta);
            upsert_segment(&mut snapshot.segments, segment, delta);
        }
        ChatRunEvent::ToolUpdated { tool } => {
            if let Some(existing) = snapshot.tools.iter_mut().find(|item| item.id == tool.id) {
                *existing = tool.clone();
            } else {
                snapshot.tools.push(tool.clone());
            }
            snapshot
                .pending_interactions
                .retain(|pending| match pending {
                    ChatPendingInteractionSnapshot::UserPromptRequested {
                        tool_call_id, ..
                    } => tool_call_id != &tool.id,
                    // 会话级授权不带 tool_call_id，无法按工具匹配，所以这里一条都不能清：
                    // 一轮最多 12 个并行工具，B 完成时 A 可能还在等授权，无条件清会把 A 的
                    // 授权卡从快照里抹掉（live 流没事，但重开窗口/sync 恢复出来就没卡可答，
                    // A 干等 60s 超时按拒绝处理）。清理统一交给 resolve_session_consent，
                    // request_session_consent 的三条出口（应答/取消/超时）都会调它。
                    _ => true,
                });
        }
        ChatRunEvent::SubagentUpdated {
            parent_tool_call_id,
            task_id,
            name,
            model,
            depth,
            status,
            preview,
            steps,
        } => {
            let update = ChatSubagentSnapshot::SubagentUpdated {
                parent_tool_call_id: parent_tool_call_id.clone(),
                task_id: task_id.clone(),
                name: name.clone(),
                model: model.clone(),
                depth: *depth,
                status: status.clone(),
                preview: preview.clone(),
                steps: steps.clone(),
            };
            if let Some(existing) = snapshot.subagents.iter_mut().find(|pending| {
                matches!(
                    pending,
                    ChatSubagentSnapshot::SubagentUpdated {
                        task_id: pending_id,
                        ..
                    } if pending_id == task_id
                )
            }) {
                *existing = update;
            } else {
                snapshot.subagents.push(update);
            }
        }
        ChatRunEvent::ContextUsageUpdated { usage } => snapshot.context_usage = Some(usage.clone()),
        ChatRunEvent::CompactionUpdated {
            phase,
            trigger,
            boundary,
        } => {
            snapshot.compaction = Some(ChatCompactionSnapshot::CompactionUpdated {
                phase: phase.clone(),
                trigger: trigger.clone(),
                boundary: boundary.clone(),
            })
        }
        ChatRunEvent::TodoUpdated { todo_state } => snapshot.todo_state = Some(todo_state.clone()),
        ChatRunEvent::PlanUpdated { plan_state } => snapshot.plan_state = Some(plan_state.clone()),
        ChatRunEvent::SessionConsentRequested => snapshot
            .pending_interactions
            .push(ChatPendingInteractionSnapshot::SessionConsentRequested),
        ChatRunEvent::ToolApprovalRequested {
            tool_call_id,
            name,
            source,
            server_id,
            target,
            arguments_preview,
            sensitivity,
        } => {
            snapshot.pending_interactions.push(
                ChatPendingInteractionSnapshot::ToolApprovalRequested {
                    tool_call_id: tool_call_id.clone(),
                    name: name.clone(),
                    source: source.clone(),
                    server_id: server_id.clone(),
                    target: target.clone(),
                    arguments_preview: arguments_preview.clone(),
                    sensitivity: sensitivity.clone(),
                },
            );
        }
        ChatRunEvent::UserPromptRequested {
            tool_call_id,
            name,
            source,
            prompt,
            structured_content,
        } => {
            snapshot.pending_interactions.push(
                ChatPendingInteractionSnapshot::UserPromptRequested {
                    tool_call_id: tool_call_id.clone(),
                    name: name.clone(),
                    source: source.clone(),
                    prompt: prompt.clone(),
                    structured_content: structured_content.clone(),
                },
            );
        }
        ChatRunEvent::ToolApprovalWithdrawn { tool_call_id } => {
            snapshot
                .pending_interactions
                .retain(|pending| match pending {
                    ChatPendingInteractionSnapshot::ToolApprovalRequested {
                        tool_call_id: pending_id,
                        ..
                    } => pending_id != tool_call_id,
                    _ => true,
                });
        }
        ChatRunEvent::HookFailed {
            hook_name,
            event,
            message,
        } => {
            snapshot.warnings.push(ChatWarningSnapshot::HookFailed {
                hook_name: hook_name.clone(),
                event: event.clone(),
                message: message.clone(),
            });
            if snapshot.warnings.len() > 20 {
                snapshot.warnings.remove(0);
            }
        }
        ChatRunEvent::StatusNoteUpdated { note } => {
            snapshot.status_note = note.clone();
        }
        ChatRunEvent::RunCompleted {
            full,
            conversation_revision,
        } => {
            snapshot.status = ChatRunStatus::Completed;
            if !full.is_empty() || snapshot.content.is_empty() {
                snapshot.content = full.clone();
            }
            snapshot.terminal = Some(ChatTerminalSnapshot::RunCompleted {
                full: full.clone(),
                conversation_revision: *conversation_revision,
            });
            snapshot.pending_interactions.clear();
        }
        ChatRunEvent::RunFailed {
            error,
            full,
            conversation_revision,
        } => {
            snapshot.status = ChatRunStatus::Failed;
            if !full.is_empty() || snapshot.content.is_empty() {
                snapshot.content = full.clone();
            }
            snapshot.terminal = Some(ChatTerminalSnapshot::RunFailed {
                error: error.clone(),
                full: full.clone(),
                conversation_revision: *conversation_revision,
            });
            snapshot.pending_interactions.clear();
        }
        ChatRunEvent::RunCancelled {
            full,
            conversation_revision,
        } => {
            snapshot.status = ChatRunStatus::Cancelled;
            if !full.is_empty() || snapshot.content.is_empty() {
                snapshot.content = full.clone();
            }
            snapshot.terminal = Some(ChatTerminalSnapshot::RunCancelled {
                full: full.clone(),
                conversation_revision: *conversation_revision,
            });
            snapshot.pending_interactions.clear();
        }
        _ => {}
    }
}

fn upsert_segment(
    segments: &mut Vec<ChatSegmentPayload>,
    segment: &Option<ChatSegmentPayload>,
    delta: &str,
) {
    let Some(segment) = segment else { return };
    if let Some(existing) = segments.iter_mut().find(|item| item.id == segment.id) {
        // move + push_str，不 clone：clone+format 是 O(已累积长度)，长答案下每个
        // delta 都整段拷贝，多对话并发时在协议锁内滚成显著 CPU。
        let mut accumulated = existing.text.take().unwrap_or_default();
        accumulated.push_str(delta);
        *existing = segment.clone();
        existing.text = Some(accumulated);
    } else {
        let mut segment = segment.clone();
        let base = segment.text.take().unwrap_or_default();
        segment.text = Some(if base.ends_with(delta) {
            base
        } else {
            format!("{base}{delta}")
        });
        segments.push(segment);
        segments.sort_by_key(|item| item.order);
    }
}

/// 实时协议的唯一出口。生产路径走 Tauri ipc `Channel`(点对点、保序、绕过全局事件总线,
/// 不再向每个 WebView 广播+反序列化);channel 未订阅(chat 窗口未建/已销毁)时事件直接
/// 丢弃——协议本就为此设计了 sync/replay,前端挂载时 `chat_sync_state` 全量对账补齐。
/// debug 构建额外广播到全局事件总线,喂 chat probe(probe.rs 靠 `app.listen` 收实时载荷)。
fn emit_protocol(app: &AppHandle, event: ChatProtocolEvent) {
    #[cfg(debug_assertions)]
    if let Err(error) = app.emit(CHAT_PROTOCOL_EVENT, &event) {
        eprintln!("Failed to emit {CHAT_PROTOCOL_EVENT}: {error}");
    }
    let state = app.state::<AppState>();
    let mut slot = state
        .chat_protocol_channel
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(channel) = slot.as_ref() {
        // send 失败 = WebView 已销毁:清槽,后续事件不再白序列化,窗口重建后重新订阅。
        if channel.send(event).is_err() {
            *slot = None;
        }
    }
}

/// Chat 窗口挂载时调用,把协议直连通道注册进单槽(新 WebView 挂载/重载替换旧槽)。
/// 订阅后前端立即对每个打开的会话跑 `chat_sync_state`,补齐订阅空窗期的事件。
#[tauri::command]
pub fn chat_protocol_subscribe(
    state: tauri::State<'_, AppState>,
    channel: tauri::ipc::Channel<ChatProtocolEvent>,
) {
    *state
        .chat_protocol_channel
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(channel);
}

pub fn register_run(
    app: &AppHandle,
    conversation_id: &str,
    run_id: &str,
    message_id: &str,
    base_revision: u64,
) {
    register_run_with_recovery(
        app,
        conversation_id,
        run_id,
        message_id,
        base_revision,
        None,
    );
}

pub fn register_run_with_recovery(
    app: &AppHandle,
    conversation_id: &str,
    run_id: &str,
    message_id: &str,
    base_revision: u64,
    recovery: Option<ChatRunRecoveryMetadata>,
) {
    let state = app.state::<AppState>();
    let event = state
        .chat_protocol
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .register_with_recovery(conversation_id, run_id, message_id, base_revision, recovery);
    if let Some(event) = event {
        emit_protocol(app, ChatProtocolEvent::Run(event));
    }
}

/// 父轮已经终态之后仍要刷子代理进度。协议 hub 拒收终态 run 的后续事件，
/// 所以这条只走直播，不进 replay。前端靠 `onChatSubagent` 补到已落库的工具卡上。
pub fn emit_live_run_event(app: &AppHandle, conversation_id: &str, event: ChatRunEvent) {
    emit_protocol(
        app,
        ChatProtocolEvent::Run(ChatRunEventEnvelope {
            protocol_version: CHAT_PROTOCOL_VERSION,
            scope: ChatProtocolScope::Run,
            conversation_id: conversation_id.to_string(),
            run_id: String::new(),
            message_id: String::new(),
            seq: 0,
            base_revision: 0,
            event,
        }),
    );
}

/// replay 预算用的字节估算。delta 事件每秒成百上千条，为算长度整包 `serde_json::to_vec`
/// 一遍纯属浪费——按字段长度估；低频事件（工具卡、终态携带全文）才实序列化拿准数。
fn replay_bytes_estimate(envelope: &ChatRunEventEnvelope) -> usize {
    const ENVELOPE_OVERHEAD: usize = 128;
    const SEGMENT_OVERHEAD: usize = 160;
    let base = ENVELOPE_OVERHEAD
        + envelope.conversation_id.len()
        + envelope.run_id.len()
        + envelope.message_id.len();
    match &envelope.event {
        ChatRunEvent::TextDelta { delta, segment } | ChatRunEvent::ReasoningDelta { delta, segment } => {
            let segment_bytes = segment.as_ref().map_or(0, |segment| {
                SEGMENT_OVERHEAD
                    + segment.id.len()
                    + segment.text.as_ref().map_or(0, |text| text.len())
            });
            base + delta.len() + segment_bytes
        }
        _ => serde_json::to_vec(envelope)
            .map(|value| value.len())
            .unwrap_or(0)
            .max(base),
    }
}

/// 能进合帧缓冲的 delta 拆出来；其余事件原样退回。段 payload 自带 `text` 的 delta 不合
/// 帧——fold 的 `ends_with` 去重语义对「合并后的 delta」不再成立，宁可单发。
fn split_coalescible_delta(event: ChatRunEvent) -> Result<PendingDelta, ChatRunEvent> {
    let has_inline_text = |segment: &Option<ChatSegmentPayload>| {
        segment
            .as_ref()
            .is_some_and(|segment| segment.text.is_some())
    };
    match event {
        ChatRunEvent::TextDelta { delta, segment } if !has_inline_text(&segment) => {
            Ok(PendingDelta {
                kind: PendingDeltaKind::Text,
                delta,
                segment,
                buffered_at: Instant::now(),
            })
        }
        ChatRunEvent::ReasoningDelta { delta, segment } if !has_inline_text(&segment) => {
            Ok(PendingDelta {
                kind: PendingDeltaKind::Reasoning,
                delta,
                segment,
                buffered_at: Instant::now(),
            })
        }
        other => Err(other),
    }
}

/// 合帧窗口到点后的兜底冲刷：没有后续事件（模型停顿、流已结束但终态还没到）时，
/// 缓冲里的尾巴也必须在 ~25ms 内上屏。
fn schedule_delta_flush(app: &AppHandle, run_id: &str) {
    let app = app.clone();
    let run_id = run_id.to_string();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(DELTA_COALESCE_WINDOW).await;
        let state = app.state::<AppState>();
        // emit 必须在锁内：本任务与 emit_run_event 并发时，锁外 emit 会把已按 seq
        // 入库的事件乱序发出（前端会误判丢事件、白触发一次 sync）。
        let mut hub = state
            .chat_protocol
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(run) = hub.runs.get_mut(&run_id) {
            run.flush_scheduled = false;
        }
        if let Some(envelope) = hub.flush_pending_delta(&run_id) {
            emit_protocol(&app, ChatProtocolEvent::Run(envelope));
        }
    });
}

pub fn emit_run_event(app: &AppHandle, run_id: &str, event: ChatRunEvent) {
    let state = app.state::<AppState>();
    let mut schedule = false;
    {
        // emit 留在锁内：入库（拿 seq）与发出必须是同一个临界区，否则与
        // 延迟冲刷任务并发时事件会乱序到达前端。合帧后事件频率已经很低，
        // 锁内一次 payload 序列化不构成争用点。
        let mut hub = state
            .chat_protocol
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let passthrough = match split_coalescible_delta(event) {
            Ok(pending) => match hub.buffer_delta(run_id, pending) {
                Ok((envelopes, schedule_flush)) => {
                    for envelope in envelopes {
                        emit_protocol(app, ChatProtocolEvent::Run(envelope));
                    }
                    schedule = schedule_flush;
                    None
                }
                Err(pending) => Some(pending.into_event()),
            },
            Err(event) => {
                // 非 delta 事件必须先把缓冲按原顺序入库，seq 才与到达顺序一致。
                if let Some(envelope) = hub.flush_pending_delta(run_id) {
                    emit_protocol(app, ChatProtocolEvent::Run(envelope));
                }
                Some(event)
            }
        };
        if let Some(event) = passthrough {
            match hub.push(run_id, event) {
                Ok(envelope) => emit_protocol(app, ChatProtocolEvent::Run(envelope)),
                Err(error) => eprintln!("Failed to record chat protocol event: {error}"),
            }
        }
    }
    if schedule {
        schedule_delta_flush(app, run_id);
    }
}

/// Hook 失败警告。run 还在 hub 里就入 replay；已被 prune 则只走直播（黄条仍能出）。
pub fn emit_hook_failed(
    app: &AppHandle,
    conversation_id: &str,
    run_id: &str,
    hook_name: String,
    event: String,
    message: String,
) {
    let event = ChatRunEvent::HookFailed {
        hook_name,
        event,
        message,
    };
    let result = app
        .state::<AppState>()
        .chat_protocol
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push(run_id, event.clone());
    match result {
        Ok(envelope) => emit_protocol(app, ChatProtocolEvent::Run(envelope)),
        Err(_) => emit_protocol(
            app,
            ChatProtocolEvent::Run(ChatRunEventEnvelope {
                protocol_version: CHAT_PROTOCOL_VERSION,
                scope: ChatProtocolScope::Run,
                conversation_id: conversation_id.to_string(),
                run_id: run_id.to_string(),
                message_id: String::new(),
                seq: 0,
                base_revision: 0,
                event,
            }),
        ),
    }
}

pub fn finish_run(
    app: &AppHandle,
    run_id: &str,
    reason: &str,
    full: &str,
    conversation_revision: u64,
) {
    let event = match reason {
        "done" | "completed" | "recovered" => ChatRunEvent::RunCompleted {
            full: full.to_string(),
            conversation_revision,
        },
        "cancelled" => ChatRunEvent::RunCancelled {
            full: full.to_string(),
            conversation_revision,
        },
        error => ChatRunEvent::RunFailed {
            error: error.to_string(),
            full: full.to_string(),
            conversation_revision,
        },
    };
    emit_run_event(app, run_id, event);
}

pub struct RegisteredRunGuard {
    app: AppHandle,
    run_id: String,
    base_revision: u64,
    deferred: bool,
}

impl RegisteredRunGuard {
    pub fn new(app: &AppHandle, run_id: &str, base_revision: u64) -> Self {
        Self {
            app: app.clone(),
            run_id: run_id.to_string(),
            base_revision,
            deferred: false,
        }
    }

    pub fn defer_terminal(&mut self) {
        self.deferred = true;
    }
}

impl Drop for RegisteredRunGuard {
    fn drop(&mut self) {
        let already_terminal = self
            .app
            .state::<AppState>()
            .chat_protocol
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .runs
            .get(&self.run_id)
            .is_some_and(|run| run.terminal_at.is_some());
        if !self.deferred && !already_terminal {
            finish_run(
                &self.app,
                &self.run_id,
                "run exited before persistence completed",
                "",
                self.base_revision,
            );
        }
    }
}

pub fn withdraw_tool_approval(app: &AppHandle, tool_call_id: &str) {
    let state = app.state::<AppState>();
    let event = state
        .chat_protocol
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .withdraw_tool_approval(tool_call_id);
    if let Some(event) = event {
        emit_protocol(app, ChatProtocolEvent::Run(event));
    }
}

pub fn resolve_session_consent(app: &AppHandle, run_id: &str) {
    app.state::<AppState>()
        .chat_protocol
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .resolve_session_consent(run_id);
}

pub fn resolve_user_prompt(app: &AppHandle, run_id: &str, tool_call_id: &str) {
    app.state::<AppState>()
        .chat_protocol
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .resolve_user_prompt(run_id, tool_call_id);
}

pub fn emit_conversation_event(
    app: &AppHandle,
    conversation_id: &str,
    revision: u64,
    event: ChatConversationEvent,
) {
    emit_protocol(
        app,
        ChatProtocolEvent::Conversation(ChatConversationEventEnvelope {
            protocol_version: CHAT_PROTOCOL_VERSION,
            scope: ChatProtocolScope::Conversation,
            conversation_id: conversation_id.to_string(),
            revision,
            event,
        }),
    );
}

#[tauri::command]
pub fn chat_sync_state(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    request: ChatSyncRequest,
) -> Result<ChatSyncResult, String> {
    if request.protocol_version != CHAT_PROTOCOL_VERSION {
        return Err(format!(
            "chat protocol version mismatch: expected {}, received {}",
            CHAT_PROTOCOL_VERSION, request.protocol_version
        ));
    }
    let mut result = state
        .chat_protocol
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .sync(&request);
    result.conversation_revision =
        crate::chat::storage::load_conversation(&app, &request.conversation_id)
            .map(|conversation| conversation.revision)
            .unwrap_or(0);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_exact_camel_case_wire_shape() {
        let mut hub = ChatProtocolHub::default();
        let event = hub.register("conv", "run", "message", 7).unwrap();
        assert_eq!(
            serde_json::to_value(ChatProtocolEvent::Run(event)).unwrap(),
            serde_json::json!({
                "protocolVersion": 1,
                "scope": "run",
                "conversationId": "conv",
                "runId": "run",
                "messageId": "message",
                "seq": 1,
                "baseRevision": 7,
                "type": "run_started",
                "recovery": null
            })
        );
    }

    #[test]
    fn all_protocol_events_have_exact_wire_shapes() {
        let run_events = vec![
            serde_json::json!({"type": "run_started", "recovery": null}),
            serde_json::json!({"type": "text_delta", "delta": "text", "segment": null}),
            serde_json::json!({"type": "reasoning_delta", "delta": "thought", "segment": null}),
            serde_json::json!({
                "type": "tool_updated",
                "tool": {
                    "id": "tool", "name": "read_file", "source": "native", "serverId": null,
                    "status": "running", "argumentsPreview": "{}", "resultPreview": null,
                    "error": null, "startedAt": 1, "completedAt": null, "durationMs": null,
                    "round": 1, "sensitive": false, "artifacts": [], "traceId": null,
                    "spanId": null, "structuredContent": null
                }
            }),
            serde_json::json!({
                "type": "subagent_updated", "parentToolCallId": "tool", "taskId": "task",
                "name": "reviewer", "model": null, "depth": 1, "status": "running",
                "preview": null, "steps": []
            }),
            serde_json::json!({
                "type": "context_usage_updated",
                "usage": {"usedTokens": 10, "contextWindowTokens": 100}
            }),
            serde_json::json!({
                "type": "compaction_updated", "phase": "started", "trigger": null,
                "boundary": null
            }),
            serde_json::json!({
                "type": "todo_updated", "todoState": {"items": [], "updatedAt": 1}
            }),
            serde_json::json!({
                "type": "plan_updated",
                "planState": {"mode": "plan", "status": "draft", "plan": "step", "updatedAt": 1}
            }),
            serde_json::json!({"type": "session_consent_requested"}),
            serde_json::json!({
                "type": "tool_approval_requested", "toolCallId": "tool", "name": "shell",
                "source": "native", "serverId": null, "target": "repo",
                "argumentsPreview": "cmd", "sensitivity": "sensitive"
            }),
            serde_json::json!({"type": "tool_approval_withdrawn", "toolCallId": "tool"}),
            serde_json::json!({
                "type": "user_prompt_requested", "toolCallId": "ask", "name": "ask_user",
                "source": "native", "prompt": {"title": null, "questions": []},
                "structuredContent": null
            }),
            serde_json::json!({
                "type": "hook_failed", "hookName": "after", "event": "stop", "message": "failed"
            }),
            serde_json::json!({"type": "status_note_updated", "note": "retry 2/10"}),
            serde_json::json!({"type": "status_note_updated", "note": null}),
            serde_json::json!({
                "type": "run_completed", "full": "answer", "conversationRevision": 2
            }),
            serde_json::json!({
                "type": "run_failed", "error": "failed", "full": "partial",
                "conversationRevision": 2
            }),
            serde_json::json!({
                "type": "run_cancelled", "full": "partial", "conversationRevision": 2
            }),
        ];
        for (index, expected_event) in run_events.into_iter().enumerate() {
            let event: ChatRunEvent = serde_json::from_value(expected_event.clone()).unwrap();
            let envelope = ChatRunEventEnvelope {
                protocol_version: CHAT_PROTOCOL_VERSION,
                scope: ChatProtocolScope::Run,
                conversation_id: "conv".to_string(),
                run_id: "run".to_string(),
                message_id: "message".to_string(),
                seq: index as u64 + 1,
                base_revision: 7,
                event,
            };
            let mut expected = serde_json::json!({
                "protocolVersion": 1, "scope": "run", "conversationId": "conv",
                "runId": "run", "messageId": "message", "seq": index as u64 + 1,
                "baseRevision": 7
            });
            expected
                .as_object_mut()
                .unwrap()
                .extend(expected_event.as_object().unwrap().clone());
            assert_eq!(serde_json::to_value(envelope).unwrap(), expected);
        }

        let conversation_events = vec![
            serde_json::json!({
                "type": "context_updated",
                "contextState": {
                    "estimatedInputTokens": 0, "contextWindowTokens": null,
                    "contextWindowEstimated": false, "usageRatio": null, "status": "idle",
                    "segments": [], "lastMeasuredAt": 1, "lastCompressedAt": null,
                    "compressedMessageCount": 0, "compressionCount": 0, "summary": null,
                    "compactionBoundaries": [], "clearBoundaries": [], "warning": null, "contextSource": null,
                    "tokenCountSource": null, "sessionInputTokens": null,
                    "sessionOutputTokens": null, "externalAgentId": null, "externalModel": null
                }
            }),
            serde_json::json!({
                "type": "todo_updated", "todoState": {"items": [], "updatedAt": 1}
            }),
            serde_json::json!({
                "type": "plan_updated",
                "planState": {"mode": "act", "status": "empty", "plan": null, "updatedAt": 1}
            }),
        ];
        for (index, expected_event) in conversation_events.into_iter().enumerate() {
            let event: ChatConversationEvent =
                serde_json::from_value(expected_event.clone()).unwrap();
            let envelope = ChatConversationEventEnvelope {
                protocol_version: CHAT_PROTOCOL_VERSION,
                scope: ChatProtocolScope::Conversation,
                conversation_id: "conv".to_string(),
                revision: index as u64 + 1,
                event,
            };
            let mut expected = serde_json::json!({
                "protocolVersion": 1, "scope": "conversation", "conversationId": "conv",
                "revision": index as u64 + 1
            });
            expected
                .as_object_mut()
                .unwrap()
                .extend(expected_event.as_object().unwrap().clone());
            assert_eq!(serde_json::to_value(envelope).unwrap(), expected);
        }
    }

    #[test]
    fn rejects_unknown_wire_fields() {
        let value = serde_json::json!({
            "protocolVersion": 1,
            "scope": "run",
            "conversationId": "conv",
            "runId": "run",
            "messageId": "message",
            "seq": 1,
            "baseRevision": 0,
            "type": "run_started",
            "recovery": null,
            "unexpected": true
        });
        assert!(serde_json::from_value::<ChatProtocolEvent>(value).is_err());
    }

    #[test]
    fn rejects_wrong_protocol_version_during_rust_decode() {
        let value = serde_json::json!({
            "protocolVersion": CHAT_PROTOCOL_VERSION + 1,
            "scope": "run",
            "conversationId": "conv",
            "runId": "run",
            "messageId": "message",
            "seq": 1,
            "baseRevision": 0,
            "type": "run_started",
            "recovery": null
        });
        assert!(serde_json::from_value::<ChatProtocolEvent>(value).is_err());
    }

    #[test]
    fn assigns_sequence_and_replays_after_cursor() {
        let mut hub = ChatProtocolHub::default();
        let started = hub.register("conv", "run", "message", 7).unwrap();
        assert_eq!(started.seq, 1);
        hub.push(
            "run",
            ChatRunEvent::TextDelta {
                delta: "a".into(),
                segment: None,
            },
        )
        .unwrap();
        let result = hub.sync(&ChatSyncRequest {
            protocol_version: CHAT_PROTOCOL_VERSION,
            conversation_id: "conv".into(),
            cursors: vec![ChatRunCursor {
                run_id: "run".into(),
                last_seq: 1,
            }],
        });
        let ChatRunSync::Events { events, .. } = &result.runs[0] else {
            panic!("expected replay");
        };
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].seq, 2);
    }

    #[test]
    fn rejects_events_after_terminal() {
        let mut hub = ChatProtocolHub::default();
        hub.register("conv", "run", "message", 0).unwrap();
        hub.push(
            "run",
            ChatRunEvent::RunCompleted {
                full: "done".into(),
                conversation_revision: 3,
            },
        )
        .unwrap();
        assert!(hub
            .push(
                "run",
                ChatRunEvent::TextDelta {
                    delta: "late".into(),
                    segment: None,
                },
            )
            .is_err());
    }

    #[test]
    fn accepts_hook_failed_after_terminal() {
        let mut hub = ChatProtocolHub::default();
        hub.register("conv", "run", "message", 0).unwrap();
        hub.push(
            "run",
            ChatRunEvent::RunCompleted {
                full: "done".into(),
                conversation_revision: 3,
            },
        )
        .unwrap();
        let envelope = hub
            .push(
                "run",
                ChatRunEvent::HookFailed {
                    hook_name: "after".into(),
                    event: "agent_end".into(),
                    message: "boom".into(),
                },
            )
            .expect("hook_failed must land after the run is terminal");
        assert!(matches!(
            envelope.event,
            ChatRunEvent::HookFailed { ref hook_name, .. } if hook_name == "after"
        ));
        assert_eq!(
            hub.runs.get("run").unwrap().snapshot.warnings.len(),
            1,
            "post-terminal hook_failed still folds into the snapshot"
        );
        assert!(
            hub.push(
                "run",
                ChatRunEvent::TextDelta {
                    delta: "late".into(),
                    segment: None,
                },
            )
            .is_err(),
            "content events stay rejected after terminal"
        );
    }

    #[test]
    fn hook_failed_on_unknown_run_is_not_recorded() {
        let mut hub = ChatProtocolHub::default();
        assert!(hub
            .push(
                "missing",
                ChatRunEvent::HookFailed {
                    hook_name: "after".into(),
                    event: "agent_end".into(),
                    message: "boom".into(),
                },
            )
            .is_err());
    }

    #[test]
    fn falls_back_to_snapshot_when_cursor_is_evicted() {
        let mut hub = ChatProtocolHub::default();
        hub.register("conv", "run", "message", 0).unwrap();
        for _ in 0..MAX_REPLAY_EVENTS + 10 {
            hub.push(
                "run",
                ChatRunEvent::TextDelta {
                    delta: "x".into(),
                    segment: None,
                },
            )
            .unwrap();
        }
        let result = hub.sync(&ChatSyncRequest {
            protocol_version: CHAT_PROTOCOL_VERSION,
            conversation_id: "conv".into(),
            cursors: vec![ChatRunCursor {
                run_id: "run".into(),
                last_seq: 1,
            }],
        });
        assert!(matches!(result.runs[0], ChatRunSync::Snapshot { .. }));
    }

    #[test]
    fn first_mount_returns_aggregate_snapshot() {
        let mut hub = ChatProtocolHub::default();
        hub.register("conv", "run", "message", 0).unwrap();
        hub.push(
            "run",
            ChatRunEvent::TextDelta {
                delta: "complete state".into(),
                segment: None,
            },
        )
        .unwrap();
        let result = hub.sync(&ChatSyncRequest {
            protocol_version: CHAT_PROTOCOL_VERSION,
            conversation_id: "conv".into(),
            cursors: Vec::new(),
        });
        let ChatRunSync::Snapshot { snapshot } = &result.runs[0] else {
            panic!("expected snapshot");
        };
        assert_eq!(snapshot.content, "complete state");
        assert_eq!(snapshot.last_seq, 2);
    }

    #[test]
    fn recovery_metadata_is_preserved_in_run_started_and_snapshot() {
        let mut hub = ChatProtocolHub::default();
        let recovery = ChatRunRecoveryMetadata {
            group_id: "group".to_string(),
            group_size: 2,
            arm_index: 1,
            provider_id: "provider".to_string(),
            model: "model".to_string(),
        };
        let started = hub
            .register_with_recovery("conv", "run", "message", 4, Some(recovery.clone()))
            .unwrap();
        assert_eq!(
            started.event,
            ChatRunEvent::RunStarted {
                recovery: Some(recovery.clone())
            }
        );

        let sync = hub.sync(&ChatSyncRequest {
            protocol_version: CHAT_PROTOCOL_VERSION,
            conversation_id: "conv".to_string(),
            cursors: Vec::new(),
        });
        let ChatRunSync::Snapshot { snapshot } = &sync.runs[0] else {
            panic!("first mount should receive an active run snapshot");
        };
        assert_eq!(snapshot.recovery.as_ref(), Some(&recovery));
    }

    #[test]
    fn first_mount_does_not_restore_retained_completed_runs() {
        let mut hub = ChatProtocolHub::default();
        hub.register("conv", "run", "message", 0).unwrap();
        hub.push(
            "run",
            ChatRunEvent::RunCompleted {
                full: "done".into(),
                conversation_revision: 1,
            },
        )
        .unwrap();
        let result = hub.sync(&ChatSyncRequest {
            protocol_version: CHAT_PROTOCOL_VERSION,
            conversation_id: "conv".into(),
            cursors: Vec::new(),
        });
        assert!(result.runs.is_empty());
    }

    #[test]
    fn cursor_ahead_of_server_falls_back_to_snapshot() {
        let mut hub = ChatProtocolHub::default();
        hub.register("conv", "run", "message", 0).unwrap();
        let result = hub.sync(&ChatSyncRequest {
            protocol_version: CHAT_PROTOCOL_VERSION,
            conversation_id: "conv".into(),
            cursors: vec![ChatRunCursor {
                run_id: "run".into(),
                last_seq: 9,
            }],
        });
        assert!(matches!(result.runs[0], ChatRunSync::Snapshot { .. }));
    }

    #[test]
    fn snapshot_accumulates_delta_text_into_segment_timeline() {
        let mut hub = ChatProtocolHub::default();
        hub.register("conv", "run", "message", 0).unwrap();
        let segment = ChatSegmentPayload {
            id: "segment".into(),
            kind: ChatSegmentKind::Text,
            phase: ChatSegmentPhase::ToolLoop,
            order: 1,
            step_number: Some(1),
            round: Some(1),
            text: None,
            tool_call_id: None,
        };
        for delta in ["hel", "lo"] {
            hub.push(
                "run",
                ChatRunEvent::TextDelta {
                    delta: delta.into(),
                    segment: Some(segment.clone()),
                },
            )
            .unwrap();
        }
        assert_eq!(
            hub.runs["run"].snapshot.segments[0].text.as_deref(),
            Some("hello")
        );
    }

    #[test]
    fn empty_delta_with_segment_reserves_its_timeline_slot() {
        // 工具卡 / 内置搜索卡的占位事件：delta 是空的，段本身才是信息（order 决定它插在
        // 时间线哪一格）。快照必须收下它，否则前端流式期间没有卡可渲染。
        let mut hub = ChatProtocolHub::default();
        hub.register("conv", "run", "message", 0).unwrap();
        let tool_slot = ChatSegmentPayload {
            id: "tool-slot".into(),
            kind: ChatSegmentKind::Tool,
            phase: ChatSegmentPhase::ToolLoop,
            order: 1,
            step_number: Some(1),
            round: Some(1),
            text: None,
            tool_call_id: Some("call-1".into()),
        };
        hub.push(
            "run",
            ChatRunEvent::TextDelta {
                delta: String::new(),
                segment: Some(tool_slot),
            },
        )
        .unwrap();
        hub.push(
            "run",
            ChatRunEvent::TextDelta {
                delta: "answer".into(),
                segment: Some(ChatSegmentPayload {
                    id: "text".into(),
                    kind: ChatSegmentKind::Text,
                    phase: ChatSegmentPhase::Plain,
                    order: 2,
                    step_number: None,
                    round: None,
                    text: None,
                    tool_call_id: None,
                }),
            },
        )
        .unwrap();
        let segments = &hub.runs["run"].snapshot.segments;
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].id, "tool-slot");
        assert_eq!(segments[0].order, 1);
        assert_eq!(segments[0].tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(segments[1].id, "text");
        assert_eq!(hub.runs["run"].snapshot.content, "answer");
    }

    #[test]
    fn empty_failure_terminal_preserves_accumulated_snapshot_content() {
        let mut hub = ChatProtocolHub::default();
        hub.register("conv", "run", "message", 0).unwrap();
        hub.push(
            "run",
            ChatRunEvent::TextDelta {
                delta: "partial".into(),
                segment: None,
            },
        )
        .unwrap();
        hub.push(
            "run",
            ChatRunEvent::RunFailed {
                error: "disconnected".into(),
                full: String::new(),
                conversation_revision: 1,
            },
        )
        .unwrap();
        assert_eq!(hub.runs["run"].snapshot.content, "partial");
    }

    #[test]
    fn sequence_allocation_is_monotonic_under_concurrent_callers() {
        let hub = std::sync::Arc::new(std::sync::Mutex::new(ChatProtocolHub::default()));
        hub.lock()
            .unwrap()
            .register("conv", "run", "message", 0)
            .unwrap();
        let workers: Vec<_> = (0..16)
            .map(|_| {
                let hub = hub.clone();
                std::thread::spawn(move || {
                    hub.lock()
                        .unwrap()
                        .push(
                            "run",
                            ChatRunEvent::TextDelta {
                                delta: "x".into(),
                                segment: None,
                            },
                        )
                        .unwrap()
                        .seq
                })
            })
            .collect();
        let mut sequences: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        sequences.sort_unstable();
        assert_eq!(sequences, (2..=17).collect::<Vec<_>>());
    }

    fn pending_text(delta: &str, segment: Option<ChatSegmentPayload>) -> PendingDelta {
        PendingDelta {
            kind: PendingDeltaKind::Text,
            delta: delta.to_string(),
            segment,
            buffered_at: Instant::now(),
        }
    }

    fn text_segment(id: &str) -> ChatSegmentPayload {
        ChatSegmentPayload {
            id: id.to_string(),
            kind: ChatSegmentKind::Text,
            phase: ChatSegmentPhase::Plain,
            order: 0,
            step_number: None,
            round: None,
            text: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn buffered_deltas_merge_into_single_event_with_identical_fold() {
        let mut hub = ChatProtocolHub::default();
        hub.register("conv", "run", "message", 0).unwrap();
        let (envelopes, schedule) = hub.buffer_delta("run", pending_text("你好", None)).unwrap();
        assert!(envelopes.is_empty());
        assert!(schedule);
        let (envelopes, schedule) = hub
            .buffer_delta("run", pending_text("，世界", None))
            .unwrap();
        assert!(envelopes.is_empty());
        // 已有一个在飞的延迟冲刷，不重复调度。
        assert!(!schedule);
        // 缓冲期间不占 seq、不进快照。
        assert_eq!(hub.runs["run"].snapshot.last_seq, 1);
        assert_eq!(hub.runs["run"].snapshot.content, "");
        let flushed = hub.flush_pending_delta("run").unwrap();
        assert_eq!(flushed.seq, 2);
        assert!(matches!(
            &flushed.event,
            ChatRunEvent::TextDelta { delta, .. } if delta == "你好，世界"
        ));
        assert_eq!(hub.runs["run"].snapshot.content, "你好，世界");
        assert!(hub.flush_pending_delta("run").is_none());
    }

    #[test]
    fn segment_switch_flushes_previous_buffer_in_arrival_order() {
        let mut hub = ChatProtocolHub::default();
        hub.register("conv", "run", "message", 0).unwrap();
        hub.buffer_delta("run", pending_text("a", Some(text_segment("seg_1"))))
            .unwrap();
        let (envelopes, _) = hub
            .buffer_delta("run", pending_text("b", Some(text_segment("seg_2"))))
            .unwrap();
        // 段位切换：旧缓冲立即入库，seq 在新缓冲之前。
        assert_eq!(envelopes.len(), 1);
        assert!(matches!(
            &envelopes[0].event,
            ChatRunEvent::TextDelta { delta, segment: Some(segment) }
                if delta == "a" && segment.id == "seg_1"
        ));
        let flushed = hub.flush_pending_delta("run").unwrap();
        assert!(flushed.seq > envelopes[0].seq);
        assert!(matches!(
            &flushed.event,
            ChatRunEvent::TextDelta { delta, segment: Some(segment) }
                if delta == "b" && segment.id == "seg_2"
        ));
    }

    #[test]
    fn oversized_buffer_flushes_immediately() {
        let mut hub = ChatProtocolHub::default();
        hub.register("conv", "run", "message", 0).unwrap();
        let (envelopes, schedule) = hub
            .buffer_delta("run", pending_text(&"x".repeat(DELTA_COALESCE_MAX_BYTES), None))
            .unwrap();
        assert_eq!(envelopes.len(), 1);
        assert!(!schedule);
        assert!(hub.runs["run"].pending_delta.is_none());
    }

    #[test]
    fn buffer_delta_refuses_unknown_and_terminal_runs() {
        let mut hub = ChatProtocolHub::default();
        assert!(hub.buffer_delta("missing", pending_text("x", None)).is_err());
        hub.register("conv", "run", "message", 0).unwrap();
        hub.push(
            "run",
            ChatRunEvent::RunCompleted {
                full: String::new(),
                conversation_revision: 1,
            },
        )
        .unwrap();
        assert!(hub.buffer_delta("run", pending_text("x", None)).is_err());
    }

    #[test]
    fn oversized_event_falls_back_to_snapshot_when_replay_is_empty() {
        let mut hub = ChatProtocolHub::default();
        hub.register("conv", "run", "message", 0).unwrap();
        hub.push(
            "run",
            ChatRunEvent::TextDelta {
                delta: "x".repeat(MAX_REPLAY_BYTES + 1),
                segment: None,
            },
        )
        .unwrap();
        assert!(hub.runs["run"].replay.is_empty());
        let result = hub.sync(&ChatSyncRequest {
            protocol_version: CHAT_PROTOCOL_VERSION,
            conversation_id: "conv".into(),
            cursors: vec![ChatRunCursor {
                run_id: "run".into(),
                last_seq: 1,
            }],
        });
        assert!(matches!(result.runs[0], ChatRunSync::Snapshot { .. }));
    }

    #[test]
    fn resolved_consent_and_user_prompt_are_removed_from_snapshot() {
        let mut hub = ChatProtocolHub::default();
        hub.register("conv", "run", "message", 0).unwrap();
        hub.push("run", ChatRunEvent::SessionConsentRequested)
            .unwrap();
        hub.push(
            "run",
            ChatRunEvent::UserPromptRequested {
                tool_call_id: "ask-1".to_string(),
                name: "ask_user".to_string(),
                source: "native".to_string(),
                prompt: ChatAskUserPromptPayload {
                    title: None,
                    questions: Vec::new(),
                },
                structured_content: None,
            },
        )
        .unwrap();

        hub.resolve_session_consent("run");
        hub.resolve_user_prompt("run", "ask-1");

        assert!(hub.runs["run"].snapshot.pending_interactions.is_empty());
    }

    #[test]
    fn parallel_tool_completion_keeps_other_pending_session_consent() {
        // 工具 A 弹会话授权、同轮工具 B 完成：B 的 tool_updated 不能把 A 的授权卡清掉。
        let mut hub = ChatProtocolHub::default();
        hub.register("conv", "run", "message", 0).unwrap();
        hub.push("run", ChatRunEvent::SessionConsentRequested)
            .unwrap();
        let record: crate::chat::ToolCallRecord = serde_json::from_value(serde_json::json!({
            "id": "tool-b",
            "name": "read_file",
            "status": "success"
        }))
        .unwrap();
        hub.push(
            "run",
            ChatRunEvent::ToolUpdated {
                tool: ChatToolPayload::from_record(&record, String::new()),
            },
        )
        .unwrap();

        assert_eq!(
            hub.runs["run"].snapshot.pending_interactions,
            vec![ChatPendingInteractionSnapshot::SessionConsentRequested]
        );

        hub.resolve_session_consent("run");
        assert!(hub.runs["run"].snapshot.pending_interactions.is_empty());
    }

    #[test]
    fn sync_marks_evicted_cursor_for_persisted_conversation_fallback() {
        let mut hub = ChatProtocolHub::default();
        let result = hub.sync(&ChatSyncRequest {
            protocol_version: CHAT_PROTOCOL_VERSION,
            conversation_id: "conv".into(),
            cursors: vec![ChatRunCursor {
                run_id: "evicted-run".into(),
                last_seq: 9,
            }],
        });
        assert_eq!(result.missing_run_ids, vec!["evicted-run"]);
        assert!(result.runs.is_empty());
    }

    #[test]
    fn completed_run_cleanup_enforces_ttl_and_global_limit() {
        let mut hub = ChatProtocolHub::default();
        for index in 0..MAX_COMPLETED_RUNS + 3 {
            let run_id = format!("run-{index}");
            hub.register("conv", &run_id, "message", 0).unwrap();
            hub.push(
                &run_id,
                ChatRunEvent::RunCompleted {
                    full: String::new(),
                    conversation_revision: 1,
                },
            )
            .unwrap();
        }
        hub.prune();
        assert_eq!(hub.runs.len(), MAX_COMPLETED_RUNS);

        let expired = hub.runs.keys().next().unwrap().clone();
        hub.runs.get_mut(&expired).unwrap().terminal_at =
            Some(Instant::now() - COMPLETED_RUN_TTL - Duration::from_secs(1));
        hub.prune();
        assert!(!hub.runs.contains_key(&expired));
    }
}
