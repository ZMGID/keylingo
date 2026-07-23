use std::path::Path;

use crate::chat::agent::prepare as agent_prepare;
use crate::chat::model::ModelUsage;
use crate::chat::model_metadata::context_window_for_model;
use crate::chat::types::{ContextUsageSegment, Conversation, ConversationContextState};
use crate::external_agents::detection::detect_single_agent;
use crate::external_agents::registry::get_agent_def;
use crate::external_agents::session::claude_init::{
    context_window_from_claude_model_alias, context_window_from_claude_resolved_model,
};
use crate::external_agents::types::RuntimeModelOption;

pub const CONTEXT_SOURCE_BUILTIN: &str = "kivio_builtin";
pub const CONTEXT_SOURCE_EXTERNAL: &str = "external_cli";
pub const TOKEN_COUNT_CLI: &str = "cli_reported";
pub const TOKEN_COUNT_ESTIMATED: &str = "estimated";
/// 内置路径把 provider 实报 `usage` 作为真实用量锚点时的口径标记（对齐 CLI 的 `cli_reported`）。
/// 前端 ContextIndicator 据此把 footer 标为「模型实报」（精确值，不带 `~` 前缀）。
pub const TOKEN_COUNT_PROVIDER_REPORTED: &str = "provider_reported";

pub fn parse_context_window_label(label: &str) -> Option<u32> {
    let s = label.trim().to_ascii_uppercase();
    if s.is_empty() {
        return None;
    }
    let (num_part, multiplier) = if let Some(rest) = s.strip_suffix('M') {
        (rest, 1_000_000u32)
    } else if let Some(rest) = s.strip_suffix('K') {
        (rest, 1_000u32)
    } else {
        (s.as_str(), 1u32)
    };
    num_part
        .parse::<f64>()
        .ok()
        .filter(|value| *value > 0.0)
        .map(|value| (value * multiplier as f64).round() as u32)
}

pub fn context_window_for_external_model(
    agent_id: &str,
    model: &str,
    detected_models: Option<&[RuntimeModelOption]>,
) -> (usize, bool) {
    let model = model.trim();
    let lookup_id = if model.is_empty() || model == "default" {
        "default"
    } else {
        model
    };

    if let Some(models) = detected_models {
        if let Some(found) = models.iter().find(|item| item.id == lookup_id) {
            if let Some(tokens) = found.context_window_tokens {
                return (tokens as usize, false);
            }
        }
    }

    if agent_id == "claude" {
        if let Some(tokens) = context_window_from_claude_model_alias(lookup_id) {
            return (tokens as usize, false);
        }
        if lookup_id != "default" {
            if let Some(tokens) = context_window_from_claude_resolved_model(lookup_id) {
                return (tokens as usize, false);
            }
        }
    }

    if !model.is_empty() && model != "default" {
        return context_window_for_model(None, model);
    }
    (context_window_for_model(None, model).0, true)
}

pub struct ExternalSessionUsage {
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub token_count_source: &'static str,
}

pub fn collect_external_session_usage(conversation: &Conversation) -> ExternalSessionUsage {
    let mut latest: Option<&ModelUsage> = None;
    for message in conversation.messages.iter().rev() {
        if message.role != "assistant" {
            continue;
        }
        if let Some(usage) = message.usage.as_ref() {
            if usage.input_tokens.is_some() || usage.output_tokens.is_some() {
                latest = Some(usage);
                break;
            }
        }
    }

    if let Some(usage) = latest {
        return ExternalSessionUsage {
            input_tokens: usage.input_tokens.unwrap_or(0) as usize,
            output_tokens: usage.output_tokens.unwrap_or(0) as usize,
            token_count_source: TOKEN_COUNT_CLI,
        };
    }

    let transcript = conversation
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    ExternalSessionUsage {
        input_tokens: agent_prepare::estimate_tokens(&transcript),
        output_tokens: 0,
        token_count_source: TOKEN_COUNT_ESTIMATED,
    }
}

pub fn compute_external_context_state(
    conversation: &Conversation,
    agent_id: &str,
    model: &str,
    detected_models: Option<&[RuntimeModelOption]>,
    compact_usage: Option<&ModelUsage>,
) -> ConversationContextState {
    let usage = compact_usage
        .map(|item| ExternalSessionUsage {
            input_tokens: item.input_tokens.unwrap_or(0) as usize,
            output_tokens: item.output_tokens.unwrap_or(0) as usize,
            token_count_source: TOKEN_COUNT_CLI,
        })
        .unwrap_or_else(|| collect_external_session_usage(conversation));

    let (context_window_tokens, context_window_estimated) =
        context_window_for_external_model(agent_id, model, detected_models);
    let usage_ratio = if context_window_tokens == 0 {
        None
    } else {
        Some(usage.input_tokens as f32 / context_window_tokens as f32)
    };
    let status = external_context_status(usage_ratio);
    let segments = external_context_segments(&usage);
    let last_compressed_at = conversation.context_state.last_compressed_at;
    let compression_count = conversation.context_state.compression_count;

    ConversationContextState {
        estimated_input_tokens: usage.input_tokens,
        context_window_tokens: Some(context_window_tokens),
        context_window_estimated,
        usage_ratio,
        status,
        segments,
        last_measured_at: chrono::Local::now().timestamp(),
        last_compressed_at,
        compressed_message_count: 0,
        compression_count,
        summary: None,
        compaction_boundaries: conversation.context_state.compaction_boundaries.clone(),
        warning: conversation.context_state.warning.clone(),
        context_source: Some(CONTEXT_SOURCE_EXTERNAL.to_string()),
        token_count_source: Some(usage.token_count_source.to_string()),
        session_input_tokens: Some(usage.input_tokens),
        session_output_tokens: Some(usage.output_tokens),
        external_agent_id: Some(agent_id.to_string()),
        external_model: if model.trim().is_empty() || model == "default" {
            None
        } else {
            Some(model.to_string())
        },
    }
}

fn external_context_status(usage_ratio: Option<f32>) -> String {
    let Some(ratio) = usage_ratio else {
        return "unknown".to_string();
    };
    if ratio >= 0.95 {
        "critical".to_string()
    } else if ratio >= 0.70 {
        "warning".to_string()
    } else {
        "normal".to_string()
    }
}

fn external_context_segments(usage: &ExternalSessionUsage) -> Vec<ContextUsageSegment> {
    if usage.input_tokens == 0 {
        return Vec::new();
    }
    let label = if usage.token_count_source == TOKEN_COUNT_CLI {
        "CLI session context".to_string()
    } else {
        "Estimated transcript".to_string()
    };
    vec![ContextUsageSegment {
        id: "external-session".to_string(),
        label,
        estimated_tokens: usage.input_tokens,
        color: Some("#4A7FD7".to_string()),
    }]
}

pub async fn compute_external_context_state_with_probe(
    conversation: &Conversation,
    probe_models: bool,
    compact_usage: Option<&ModelUsage>,
    cached_models: Option<&[RuntimeModelOption]>,
    probe_cwd: Option<&Path>,
) -> ConversationContextState {
    let agent_id = conversation
        .agent_runtime
        .external_agent_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .unwrap_or("");
    let model = conversation
        .agent_runtime
        .external_model
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("default");
    let detected_models = if probe_models {
        if let Some(def) = get_agent_def(agent_id) {
            let fallback_cwd = std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir());
            Some(
                detect_single_agent(def, probe_cwd.unwrap_or(fallback_cwd.as_path()))
                    .await
                    .models,
            )
        } else {
            None
        }
    } else {
        cached_models.map(|models| models.to_vec())
    };
    compute_external_context_state(
        conversation,
        agent_id,
        model,
        detected_models.as_ref().map(|models| models.as_slice()),
        compact_usage,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::types::{
        AgentPlanState, AgentRuntimeConfig, AgentRuntimeKind, AgentTodoState, ChatMessage,
        Conversation, ConversationContextState,
    };

    fn empty_conversation() -> Conversation {
        Conversation {
            id: "c1".to_string(),
            title: "t".to_string(),
            provider_id: "p".to_string(),
            model: "m".to_string(),
            messages: vec![],
            agent_runtime: AgentRuntimeConfig {
                kind: AgentRuntimeKind::External,
                external_agent_id: Some("pi".to_string()),
                external_model: Some("anthropic/claude-sonnet-4-5".to_string()),
                external_reasoning: None,
                external_sandbox: None,
            },
            active_skill_id: None,
            assistant_id: None,
            assistant_snapshot: None,
            created_at: 0,
            updated_at: 0,
            pinned: false,
            folder: None,
            project_id: None,
            set_id: None,
            context_state: ConversationContextState::default(),
            agent_plan_state: AgentPlanState::default(),
            agent_todo_state: AgentTodoState::default(),
            knowledge_base_ids: Vec::new(),
            force_knowledge_search: false,
            thinking_level: None,
            web_search_mode: None,
            reply_models: Vec::new(),
            group_selections: std::collections::HashMap::new(),
            forked_from: None,
        }
    }

    #[test]
    fn parse_context_window_labels() {
        assert_eq!(parse_context_window_label("200K"), Some(200_000));
        assert_eq!(parse_context_window_label("128k"), Some(128_000));
        assert_eq!(parse_context_window_label("1M"), Some(1_000_000));
    }

    #[test]
    fn external_context_uses_latest_assistant_usage() {
        let mut conversation = empty_conversation();
        conversation.messages.push(ChatMessage {
            id: "u1".to_string(),
            role: "user".to_string(),
            content: "hi".to_string(),
            attachments: vec![],
            reasoning: None,
            artifacts: vec![],
            tool_calls: vec![],
            segments: vec![],
            agent_plan: None,
            api_messages: vec![],
            model_messages: vec![],
            active_skill_id: None,
            run_entry: None,
            stream_outcome: None,
            usage: None,
            anchor_usage: None,
            group_id: None,
            provider_id: None,
            model: None,
            timestamp: 1,
        });
        conversation.messages.push(ChatMessage {
            id: "a1".to_string(),
            role: "assistant".to_string(),
            content: "hello".to_string(),
            attachments: vec![],
            reasoning: None,
            artifacts: vec![],
            tool_calls: vec![],
            segments: vec![],
            agent_plan: None,
            api_messages: vec![],
            model_messages: vec![],
            active_skill_id: None,
            run_entry: None,
            stream_outcome: None,
            usage: Some(ModelUsage {
                input_tokens: Some(1439),
                output_tokens: Some(28),
                ..Default::default()
            }),
            anchor_usage: None,
            group_id: None,
            provider_id: None,
            model: None,
            timestamp: 2,
        });

        let state = compute_external_context_state(
            &conversation,
            "pi",
            "anthropic/claude-sonnet-4-5",
            None,
            None,
        );
        assert_eq!(state.estimated_input_tokens, 1439);
        assert_eq!(state.token_count_source.as_deref(), Some(TOKEN_COUNT_CLI));
        assert_eq!(
            state.context_source.as_deref(),
            Some(CONTEXT_SOURCE_EXTERNAL)
        );
        assert!(state.summary.is_none());
        assert!(state.usage_ratio.unwrap() > 0.0);
    }
}
