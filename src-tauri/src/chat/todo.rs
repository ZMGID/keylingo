use std::collections::HashSet;

use serde::Deserialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::chat::types::{AgentTodoItem, AgentTodoState, AgentTodoStatus};
use crate::mcp::types::McpToolCallResult;
use crate::mcp::ChatToolDefinition;

pub const TODO_WRITE_TOOL_NAME: &str = "todo_write";

#[derive(Debug, Deserialize)]
struct TodoWriteArgs {
    #[serde(default)]
    todos: Vec<AgentTodoItem>,
}

/// 一次 todo 工具调用的结果：归一后的状态 + 本次实际改动的字段（变更回执）。
pub struct TodoToolOutcome {
    pub state: AgentTodoState,
    pub changed: Vec<String>,
}

pub fn is_agent_todo_tool_name(name: &str) -> bool {
    name == TODO_WRITE_TOOL_NAME
}

pub fn append_tool_definitions(tools: &mut Vec<ChatToolDefinition>) {
    let tool = todo_write_tool();
    if !tools
        .iter()
        .any(|existing| existing.openai_tool_name() == tool.openai_tool_name())
    {
        tools.push(tool);
    }
}

pub fn todo_write_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__todo_write".to_string(),
        name: TODO_WRITE_TOOL_NAME.to_string(),
        description: "Replace the assistant's internal todo list for this conversation. Use for multi-step work progress; users cannot edit this list.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "maxItems": 50,
                    "items": todo_item_schema()
                }
            },
            "required": ["todos"],
            "additionalProperties": false
        }),
        sensitive: false,
        annotations: Some(serde_json::json!({
            "readOnlyHint": false,
            "destructiveHint": false,
            "openWorldHint": false
        })),
        output_schema: Some(todo_output_schema()),
    }
}

pub fn apply_todo_write(arguments: Value) -> Result<TodoToolOutcome, String> {
    let args: TodoWriteArgs = serde_json::from_value(arguments)
        .map_err(|err| format!("Invalid todo_write arguments: {err}"))?;
    let state = normalized_state(args.todos, None)?;
    Ok(TodoToolOutcome {
        state,
        changed: vec!["todos".to_string()],
    })
}

pub fn format_prompt(state: &AgentTodoState, todo_tools_available: bool) -> String {
    let current = format_state_lines(state);
    let tool_hint = if todo_tools_available {
        "Use todo_write to maintain it (full-list replace)."
    } else {
        "Todo tools are unavailable for this request; use it as context only."
    };
    format!(
        "Agent todo list (internal working state): this list is owned by the assistant; the user cannot edit it manually. {tool_hint} For complex, multi-step, or continuing work, keep concise actionable items; mark one item in_progress when starting or switching work, mark items completed when done, and keep at most one in_progress item. Do not tell the user they can edit todos.\n\nCurrent todo state:\n{current}"
    )
}

/// Conversation-scoped registry handler: load the conversation, apply the
/// todo tool, persist, emit the `chat-todo` event, and return the tool
/// result. Mirrors the legacy `RegistryToolExecutor` todo special case and
/// deliberately does not resolve a native tool workspace.
pub fn handle_conversation_tool_call<'a>(
    app: &'a AppHandle,
    conversation_id: &'a str,
    tool_name: &'a str,
    arguments: Value,
) -> crate::mcp::native_registry::NativeToolFuture<'a> {
    Box::pin(async move {
    if !is_agent_todo_tool_name(tool_name) {
        return Err(format!("Unknown todo tool: {tool_name}"));
    }
    let outcome = apply_todo_write(arguments)?;
    crate::chat::repository::repository(app)
        .update_todo(app, conversation_id, outcome.state.clone())
        .await
        .map_err(crate::chat::repository::repository_error)?;
    emit_chat_todo_state(app, conversation_id, &outcome.state);
    Ok(tool_result(&outcome.state, &outcome.changed))
    })
}

pub fn emit_chat_todo_state(app: &AppHandle, conversation_id: &str, todo_state: &AgentTodoState) {
    let _ = app.emit(
        "chat-todo",
        serde_json::json!({
            "conversationId": conversation_id,
            "todoState": todo_state,
        }),
    );
}

pub fn tool_result(state: &AgentTodoState, changed: &[String]) -> McpToolCallResult {
    let structured = serde_json::json!({ "todoState": state, "changed": changed });
    let changed_line = if changed.is_empty() {
        String::new()
    } else {
        format!("Changed: {}\n\n", changed.join(", "))
    };
    McpToolCallResult {
        content: format!(
            "Todo list updated.\n\n{changed_line}{}",
            format_state_lines(state)
        ),
        is_error: false,
        raw: structured.clone(),
        artifacts: Vec::new(),
        structured_content: Some(structured),
        follow_up_user_messages: Vec::new(),
    }
}

fn normalized_state(
    items: Vec<AgentTodoItem>,
    preferred_in_progress_id: Option<&str>,
) -> Result<AgentTodoState, String> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for item in items {
        let id = item.id.trim().to_string();
        let content = item.content.trim().to_string();
        if id.is_empty() {
            return Err("Todo item id cannot be empty".to_string());
        }
        if content.is_empty() {
            return Err(format!("Todo item `{id}` content cannot be empty"));
        }
        if !seen.insert(id.clone()) {
            return Err(format!("Todo item id must be unique: {id}"));
        }
        normalized.push(AgentTodoItem {
            id,
            content,
            description: item.description.filter(|d| !d.trim().is_empty()),
            status: item.status,
            blocks: item.blocks,
            blocked_by: item.blocked_by,
            owner: item.owner.filter(|o| !o.trim().is_empty()),
        });
    }

    // 单 in_progress 不变量：cancelled/completed/pending 不参与；多余的 in_progress 降级 pending。
    let mut active_seen = false;
    for item in &mut normalized {
        let keep_active = if item.status == AgentTodoStatus::InProgress {
            match preferred_in_progress_id {
                Some(preferred) => item.id == preferred,
                None => !active_seen,
            }
        } else {
            false
        };
        if keep_active {
            active_seen = true;
        } else if item.status == AgentTodoStatus::InProgress {
            item.status = AgentTodoStatus::Pending;
        }
    }

    sync_dependency_edges(&mut normalized);

    Ok(AgentTodoState {
        items: normalized,
        updated_at: chrono::Local::now().timestamp(),
    })
}

/// 依赖边写侧同步：丢弃指向不存在 id 或自指的边，去重，并补全双向对端
/// （A.blocks 含 B ⇔ B.blocked_by 含 A）。删除条目时其残留边在此被自然清理。
fn sync_dependency_edges(items: &mut [AgentTodoItem]) {
    let ids: HashSet<String> = items.iter().map(|item| item.id.clone()).collect();
    // 先清洗每条自己声明的边（去掉无效目标/自指/重复）。
    for item in items.iter_mut() {
        let self_id = item.id.clone();
        let clean = |edges: &mut Vec<String>| {
            let mut deduped = HashSet::new();
            edges.retain(|target| {
                target != &self_id && ids.contains(target) && deduped.insert(target.clone())
            });
        };
        clean(&mut item.blocks);
        clean(&mut item.blocked_by);
    }
    // 收集应存在的双向边，再回填，保证 A.blocks∋B ⇔ B.blocked_by∋A。
    let mut blocks_pairs: Vec<(String, String)> = Vec::new(); // (blocker, blocked)
    for item in items.iter() {
        for blocked in &item.blocks {
            blocks_pairs.push((item.id.clone(), blocked.clone()));
        }
        for blocker in &item.blocked_by {
            blocks_pairs.push((blocker.clone(), item.id.clone()));
        }
    }
    for item in items.iter_mut() {
        for (blocker, blocked) in &blocks_pairs {
            if &item.id == blocker && !item.blocks.contains(blocked) {
                item.blocks.push(blocked.clone());
            }
            if &item.id == blocked && !item.blocked_by.contains(blocker) {
                item.blocked_by.push(blocker.clone());
            }
        }
    }
}

fn format_state_lines(state: &AgentTodoState) -> String {
    if state.items.is_empty() {
        return "- No current todos.".to_string();
    }
    state
        .items
        .iter()
        .map(|item| {
            let mut line = format!(
                "- [{}] {}: {}",
                status_name(&item.status),
                item.id,
                item.content
            );
            if let Some(owner) = &item.owner {
                line.push_str(&format!(" (owner: {owner})"));
            }
            if !item.blocked_by.is_empty() {
                line.push_str(&format!(" (blocked by: {})", item.blocked_by.join(", ")));
            }
            if let Some(description) = &item.description {
                line.push_str(&format!("\n    {description}"));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn status_name(status: &AgentTodoStatus) -> &'static str {
    match status {
        AgentTodoStatus::Pending => "pending",
        AgentTodoStatus::InProgress => "in_progress",
        AgentTodoStatus::Completed => "completed",
        AgentTodoStatus::Cancelled => "cancelled",
    }
}

fn todo_item_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "minLength": 1,
                "maxLength": 80,
                "description": "Stable id; reuse the same id across todo_write calls to preserve an item"
            },
            "content": {
                "type": "string",
                "minLength": 1,
                "maxLength": 240,
                "description": "Concise actionable task (one-line subject)"
            },
            "description": {
                "type": "string",
                "maxLength": 2000,
                "description": "Optional longer details for this task"
            },
            "status": status_schema(),
            "blocks": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Ids of tasks that cannot start until this one is done (reverse edge auto-synced)"
            },
            "blocked_by": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Ids of tasks that must finish before this one can start (reverse edge auto-synced)"
            },
            "owner": {
                "type": "string",
                "maxLength": 80,
                "description": "Optional claimant of this task"
            }
        },
        "required": ["id", "content", "status"],
        "additionalProperties": false
    })
}

fn status_schema() -> Value {
    serde_json::json!({
        "type": "string",
        "enum": ["pending", "in_progress", "completed", "cancelled"]
    })
}

fn todo_output_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "todoState": {
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array",
                        "items": todo_item_schema()
                    },
                    "updated_at": {
                        "type": "integer"
                    }
                },
                "required": ["items", "updated_at"],
                "additionalProperties": false
            },
            "changed": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Fields changed by this call (receipt)"
            }
        },
        "required": ["todoState"],
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::types::Conversation;

    #[test]
    fn old_conversation_json_defaults_todo_state() {
        let json = serde_json::json!({
            "id": "conv_test",
            "title": "test",
            "provider_id": "provider",
            "model": "model",
            "messages": [],
            "created_at": 1,
            "updated_at": 1
        });
        let conversation: Conversation =
            serde_json::from_value(json).expect("old conversation should deserialize");

        assert!(conversation.agent_todo_state.items.is_empty());
        assert_eq!(conversation.agent_todo_state.updated_at, 0);
    }

    #[test]
    fn write_normalizes_to_single_in_progress_item() {
        let outcome = apply_todo_write(serde_json::json!({
            "todos": [
                { "id": "a", "content": "First", "status": "in_progress" },
                { "id": "b", "content": "Second", "status": "in_progress" }
            ]
        }))
        .expect("todo write should succeed");

        assert_eq!(outcome.state.items[0].status, AgentTodoStatus::InProgress);
        assert_eq!(outcome.state.items[1].status, AgentTodoStatus::Pending);
    }

    #[test]
    fn write_switches_in_progress_item() {
        // 整表替换即可切换 in_progress：把 b 设为 in_progress、a 设为 pending。
        let outcome = apply_todo_write(serde_json::json!({
            "todos": [
                { "id": "a", "content": "First", "status": "pending" },
                { "id": "b", "content": "Second", "status": "in_progress" }
            ]
        }))
        .expect("todo write should succeed");

        assert_eq!(outcome.state.items[0].status, AgentTodoStatus::Pending);
        assert_eq!(outcome.state.items[1].status, AgentTodoStatus::InProgress);
        assert_eq!(outcome.changed, vec!["todos".to_string()]);
    }

    #[test]
    fn cancelled_does_not_count_as_in_progress() {
        let outcome = apply_todo_write(serde_json::json!({
            "todos": [
                { "id": "a", "content": "First", "status": "cancelled" },
                { "id": "b", "content": "Second", "status": "in_progress" }
            ]
        }))
        .expect("write");
        assert_eq!(outcome.state.items[0].status, AgentTodoStatus::Cancelled);
        assert_eq!(outcome.state.items[1].status, AgentTodoStatus::InProgress);
    }

    #[test]
    fn write_removing_item_cleans_edges() {
        // 整表重写为只剩 b（删除 a）；b.blocked_by 仍写着已删除的 a，应被清理。
        let outcome = apply_todo_write(serde_json::json!({
            "todos": [
                { "id": "b", "content": "Second", "status": "pending", "blocked_by": ["a"] }
            ]
        }))
        .expect("write");
        assert_eq!(outcome.state.items.len(), 1);
        assert_eq!(outcome.state.items[0].id, "b");
        // 指向已删除 a 的反向边被清理
        assert!(outcome.state.items[0].blocked_by.is_empty());
        assert_eq!(outcome.changed, vec!["todos".to_string()]);
    }

    #[test]
    fn dependency_reverse_edge_is_auto_synced() {
        let outcome = apply_todo_write(serde_json::json!({
            "todos": [
                { "id": "a", "content": "First", "status": "pending", "blocks": ["b"] },
                { "id": "b", "content": "Second", "status": "pending" }
            ]
        }))
        .expect("write");
        // 只声明了 a.blocks=[b]，b.blocked_by 应被自动补成 [a]
        assert_eq!(outcome.state.items[0].blocks, vec!["b".to_string()]);
        assert_eq!(outcome.state.items[1].blocked_by, vec!["a".to_string()]);
    }

    #[test]
    fn invalid_and_self_edges_are_dropped() {
        let outcome = apply_todo_write(serde_json::json!({
            "todos": [
                { "id": "a", "content": "First", "status": "pending", "blocks": ["a", "ghost", "b"] },
                { "id": "b", "content": "Second", "status": "pending" }
            ]
        }))
        .expect("write");
        // 自指 a、不存在的 ghost 被丢弃，只留 b
        assert_eq!(outcome.state.items[0].blocks, vec!["b".to_string()]);
    }

    #[test]
    fn description_and_owner_round_trip_and_clear() {
        let current = apply_todo_write(serde_json::json!({
            "todos": [{ "id": "a", "content": "Task", "status": "pending", "description": "details", "owner": "researcher" }]
        }))
        .expect("write")
        .state;
        assert_eq!(current.items[0].description.as_deref(), Some("details"));
        assert_eq!(current.items[0].owner.as_deref(), Some("researcher"));

        // 整表重写不带 description/owner → 字段被清除。
        let outcome = apply_todo_write(serde_json::json!({
            "todos": [{ "id": "a", "content": "Task", "status": "pending" }]
        }))
        .expect("write");
        assert_eq!(outcome.state.items[0].description, None);
        assert_eq!(outcome.state.items[0].owner, None);
    }

    #[test]
    fn old_item_json_without_new_fields_deserializes() {
        // 老 JSON：无 description/blocks/blocked_by/owner，状态只有三态
        let item: AgentTodoItem = serde_json::from_value(serde_json::json!({
            "id": "a", "content": "old", "status": "completed"
        }))
        .expect("old item should deserialize");
        assert!(item.description.is_none());
        assert!(item.blocks.is_empty());
        assert!(item.blocked_by.is_empty());
        assert!(item.owner.is_none());
    }
}
