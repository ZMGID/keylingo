use std::path::Path;

use chrono::{Local, TimeZone};
use tauri::AppHandle;

use super::{storage::load_conversation, Attachment, ChatMessage, Conversation};

#[derive(Clone, Copy)]
struct ExportLabels {
    created: &'static str,
    updated: &'static str,
    model: &'static str,
    user: &'static str,
    assistant: &'static str,
    image: &'static str,
    attachment: &'static str,
}

fn labels(language: &str) -> ExportLabels {
    if language.eq_ignore_ascii_case("en") {
        ExportLabels {
            created: "Created",
            updated: "Updated",
            model: "Model",
            user: "User",
            assistant: "Assistant",
            image: "Image",
            attachment: "Attachment",
        }
    } else {
        ExportLabels {
            created: "创建时间",
            updated: "更新时间",
            model: "模型",
            user: "用户",
            assistant: "助手",
            image: "图片",
            attachment: "附件",
        }
    }
}

fn format_timestamp(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|value| value.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

fn markdown_title(title: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        "Conversation".to_string()
    } else {
        title
    }
}

fn attachment_placeholder(attachment: &Attachment, labels: ExportLabels) -> String {
    let kind = if attachment.attachment_type == "image" {
        labels.image
    } else {
        labels.attachment
    };
    let name = attachment.name.replace(['\r', '\n'], " ");
    if name.trim().is_empty() {
        format!("[{kind}]")
    } else {
        format!("[{kind}: {name}]")
    }
}

fn render_message(message: &ChatMessage, labels: ExportLabels) -> Option<String> {
    let body = message.content.trim();
    if body.is_empty() && message.attachments.is_empty() {
        return None;
    }

    let role = if message.role == "user" {
        labels.user
    } else if message.role == "assistant" {
        labels.assistant
    } else {
        return None;
    };

    let mut section = format!("**{role}** · {}\n", format_timestamp(message.timestamp));
    if !body.is_empty() {
        section.push('\n');
        section.push_str(body);
        section.push('\n');
    }
    for attachment in &message.attachments {
        section.push('\n');
        section.push_str(&attachment_placeholder(attachment, labels));
        section.push('\n');
    }
    Some(section.trim_end().to_string())
}

pub(crate) fn render_conversation_markdown(conversation: &Conversation, language: &str) -> String {
    let labels = labels(language);
    let mut sections = vec![format!(
        "# {}\n\n- {}: {}\n- {}: {}\n- {}: {}",
        markdown_title(&conversation.title),
        labels.created,
        format_timestamp(conversation.created_at),
        labels.updated,
        format_timestamp(conversation.updated_at),
        labels.model,
        conversation.model,
    )];

    sections.extend(
        conversation
            .messages
            .iter()
            .filter_map(|message| render_message(message, labels)),
    );
    format!("{}\n", sections.join("\n\n---\n\n"))
}

#[tauri::command]
pub(crate) fn chat_export_conversation_markdown(
    app: AppHandle,
    conversation_id: String,
    path: String,
    language: String,
) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("export path is empty".to_string());
    }
    let conversation = load_conversation(&app, &conversation_id)?;
    let markdown = render_conversation_markdown(&conversation, &language);
    std::fs::write(Path::new(&path), markdown)
        .map_err(|err| format!("write conversation export: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{
        AgentPlanState, AgentRuntimeConfig, AgentTodoState, ConversationContextState,
    };
    use std::collections::HashMap;

    fn conversation() -> Conversation {
        Conversation {
            id: "conv_test".to_string(),
            title: "Export test".to_string(),
            provider_id: "secret-provider".to_string(),
            model: "gpt-test".to_string(),
            messages: vec![
                ChatMessage {
                    id: "user_1".to_string(),
                    role: "user".to_string(),
                    content: "Hello".to_string(),
                    attachments: vec![Attachment {
                        id: "att_1".to_string(),
                        attachment_type: "file".to_string(),
                        name: "notes.pdf".to_string(),
                        path: "private/notes.pdf".to_string(),
                    }],
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
                    timestamp: 1_700_000_000,
                    degraded: None,
                },
                ChatMessage {
                    id: "assistant_1".to_string(),
                    role: "assistant".to_string(),
                    content: "Final answer".to_string(),
                    attachments: vec![Attachment {
                        id: "att_2".to_string(),
                        attachment_type: "image".to_string(),
                        name: "chart.png".to_string(),
                        path: "private/chart.png".to_string(),
                    }],
                    reasoning: Some("private reasoning".to_string()),
                    artifacts: vec![],
                    tool_calls: vec![],
                    segments: vec![],
                    agent_plan: None,
                    api_messages: vec![serde_json::json!({"secret": "api transcript"})],
                    model_messages: vec![],
                    active_skill_id: None,
                    run_entry: None,
                    stream_outcome: Some("completed".to_string()),
                    usage: None,
                    anchor_usage: None,
                    group_id: None,
                    provider_id: None,
                    model: None,
                    timestamp: 1_700_000_001,
                    degraded: None,
                },
            ],
            agent_runtime: AgentRuntimeConfig::default(),
            active_skill_id: None,
            assistant_id: None,
            assistant_snapshot: None,
            created_at: 1_700_000_000,
            updated_at: 1_700_000_001,
            pinned: false,
            folder: None,
            project_id: None,
            set_id: None,
            context_state: ConversationContextState::default(),
            agent_todo_state: AgentTodoState::default(),
            agent_plan_state: AgentPlanState::default(),
            knowledge_base_ids: vec![],
            force_knowledge_search: false,
            thinking_level: None,
            web_search_mode: None,
            reply_models: vec![],
            group_selections: HashMap::new(),
            forked_from: None,
        }
    }

    #[test]
    fn renders_readable_chinese_markdown_without_internal_fields() {
        let markdown = render_conversation_markdown(&conversation(), "zh");
        assert!(markdown.contains("# Export test"));
        assert!(markdown.contains("**用户** ·"));
        assert!(markdown.contains("**助手** ·"));
        assert!(markdown.contains("[附件: notes.pdf]"));
        assert!(markdown.contains("[图片: chart.png]"));
        assert!(markdown.contains("Final answer"));
        assert!(!markdown.contains("private reasoning"));
        assert!(!markdown.contains("api transcript"));
        assert!(!markdown.contains("secret-provider"));
        assert!(!markdown.contains("private/chart.png"));
    }

    #[test]
    fn renders_english_labels_and_skips_empty_internal_only_messages() {
        let mut conversation = conversation();
        conversation.messages.push(ChatMessage {
            id: "assistant_empty".to_string(),
            role: "assistant".to_string(),
            content: String::new(),
            attachments: vec![],
            reasoning: Some("hidden".to_string()),
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
            timestamp: 1_700_000_002,
            degraded: None,
        });

        let markdown = render_conversation_markdown(&conversation, "en");
        assert!(markdown.contains("- Created:"));
        assert!(markdown.contains("**User** ·"));
        assert!(markdown.contains("**Assistant** ·"));
        assert!(markdown.contains("[Attachment: notes.pdf]"));
        assert!(markdown.contains("[Image: chart.png]"));
        assert_eq!(markdown.matches("**Assistant** ·").count(), 1);
        assert!(!markdown.contains("hidden"));
    }
}
