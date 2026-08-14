//! Claude Code 的任务列表 → Kivio 对话上已有的 Todo 条。
//!
//! 2.1.142 起默认是 `TaskCreate` / `TaskUpdate` / `TaskList` / `TaskGet`，不再整表
//! `TodoWrite`（可用 `CLAUDE_CODE_ENABLE_TASKS=0` 退回）。官方监测代码看 `tool_use` /
//! `tool_result`，按 task id 累加，不新做一套 UI。
//!
//! 核实过的线形状（claude 2.1.220 会话 jsonl）：
//! - `TaskCreate` 入参 `{ subject, description?, activeForm? }`；结果正文
//!   `Task #<id> created successfully: …`，结构化 `{ task: { id, subject } }` 只在
//!   落盘帧的 `toolUseResult` 里，stream-json 的 `tool_result.content` 是那句英文。
//! - `TaskUpdate` 入参 `{ taskId, status? }`（流里也可能是 `id` / `task_id`）。
//! - `status: "deleted"` 是删条，不是 Kivio 的 cancelled。
//!
//! 不要走内置 `normalized_state`：那条会把多个 `in_progress` 降成 pending。
//! `TaskGet` 只读，不改列表。

use serde_json::Value;

use crate::chat::types::{AgentTodoItem, AgentTodoState, AgentTodoStatus};
use crate::external_agents::session::dsh_jsonrpc::todo_state_from_write;

pub fn is_claude_todo_tool(name: &str) -> bool {
    matches!(name, "TodoWrite" | "TaskCreate" | "TaskUpdate" | "TaskList")
}

pub fn apply_claude_todo_tool(
    current: &AgentTodoState,
    name: &str,
    input: &Value,
    result: &str,
) -> Option<AgentTodoState> {
    match name {
        "TodoWrite" => todo_state_from_write(input),
        "TaskCreate" => apply_create(current, input, result),
        "TaskUpdate" => apply_update(current, input),
        "TaskList" => snapshot_from_task_list(result),
        _ => None,
    }
}

fn apply_create(current: &AgentTodoState, input: &Value, result: &str) -> Option<AgentTodoState> {
    let id = task_id_from_create_result(result)?;
    let content = string_field(input, &["subject", "content"])?;
    let mut items = current.items.clone();
    if let Some(item) = items.iter_mut().find(|item| item.id == id) {
        item.content = content;
        if let Some(description) = string_field(input, &["description"]) {
            item.description = Some(description);
        }
    } else {
        items.push(AgentTodoItem {
            id,
            content,
            description: string_field(input, &["description"]),
            status: AgentTodoStatus::Pending,
            ..Default::default()
        });
    }
    Some(stamped(items))
}

fn apply_update(current: &AgentTodoState, input: &Value) -> Option<AgentTodoState> {
    let id = task_id_from_update(input)?;
    if matches!(input.get("status").and_then(Value::as_str), Some("deleted")) {
        let items = current
            .items
            .iter()
            .filter(|item| item.id != id)
            .cloned()
            .collect();
        return Some(stamped(items));
    }
    let mut items = current.items.clone();
    if let Some(item) = items.iter_mut().find(|item| item.id == id) {
        patch_item(item, input);
    } else {
        let mut item = AgentTodoItem {
            id: id.clone(),
            content: string_field(input, &["subject", "content"]).unwrap_or_else(|| id.clone()),
            ..Default::default()
        };
        patch_item(&mut item, input);
        items.push(item);
    }
    Some(stamped(items))
}

fn patch_item(item: &mut AgentTodoItem, input: &Value) {
    if let Some(content) = string_field(input, &["subject", "content"]) {
        item.content = content;
    }
    if let Some(description) = string_field(input, &["description"]) {
        item.description = Some(description);
    }
    match input.get("status").and_then(Value::as_str) {
        Some("in_progress") => item.status = AgentTodoStatus::InProgress,
        Some("completed") => item.status = AgentTodoStatus::Completed,
        Some("pending") => item.status = AgentTodoStatus::Pending,
        Some("cancelled") => item.status = AgentTodoStatus::Cancelled,
        _ => {}
    }
    if let Some(owner) = string_field(input, &["owner"]) {
        item.owner = Some(owner);
    }
    extend_unique(&mut item.blocks, string_list(input.get("addBlocks")));
    extend_unique(&mut item.blocked_by, string_list(input.get("addBlockedBy")));
}

fn snapshot_from_task_list(result: &str) -> Option<AgentTodoState> {
    let value = parse_json_value(result)?;
    let tasks = value
        .get("tasks")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())?;
    let items = tasks.iter().filter_map(item_from_task).collect::<Vec<_>>();
    if items.is_empty() && !tasks.is_empty() {
        return None;
    }
    Some(stamped(items))
}

fn item_from_task(value: &Value) -> Option<AgentTodoItem> {
    let id = value.get("id").and_then(json_id)?;
    let content = string_field(value, &["subject", "content"])?;
    let status = match value.get("status").and_then(Value::as_str) {
        Some("in_progress") => AgentTodoStatus::InProgress,
        Some("completed") => AgentTodoStatus::Completed,
        Some("cancelled") => AgentTodoStatus::Cancelled,
        Some("deleted") => return None,
        Some("pending") | None => AgentTodoStatus::Pending,
        _ => return None,
    };
    let blocked_by = {
        let camel = string_list(value.get("blockedBy"));
        if camel.is_empty() {
            string_list(value.get("blocked_by"))
        } else {
            camel
        }
    };
    Some(AgentTodoItem {
        id,
        content,
        description: string_field(value, &["description"]),
        status,
        blocks: string_list(value.get("blocks")),
        blocked_by,
        owner: string_field(value, &["owner"]),
    })
}

fn task_id_from_create_result(result: &str) -> Option<String> {
    if let Some(value) = parse_json_value(result) {
        if let Some(id) = value
            .pointer("/task/id")
            .and_then(json_id)
            .or_else(|| value.get("id").and_then(json_id))
        {
            return Some(id);
        }
    }
    let rest = result.trim().strip_prefix("Task #")?;
    let id: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

fn task_id_from_update(input: &Value) -> Option<String> {
    ["taskId", "id", "task_id"]
        .into_iter()
        .find_map(|key| input.get(key).and_then(json_id))
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    })
}

fn json_id(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| value.as_u64().map(|n| n.to_string()))
        .or_else(|| value.as_i64().map(|n| n.to_string()))
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(json_id).collect::<Vec<_>>())
        .unwrap_or_default()
}

fn extend_unique(dest: &mut Vec<String>, extra: Vec<String>) {
    for item in extra {
        if !dest.iter().any(|existing| existing == &item) {
            dest.push(item);
        }
    }
}

fn parse_json_value(raw: &str) -> Option<Value> {
    let trimmed = raw.trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

fn stamped(items: Vec<AgentTodoItem>) -> AgentTodoState {
    AgentTodoState {
        items,
        updated_at: chrono::Local::now().timestamp(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pending(id: &str, content: &str) -> AgentTodoItem {
        AgentTodoItem {
            id: id.to_string(),
            content: content.to_string(),
            status: AgentTodoStatus::Pending,
            ..Default::default()
        }
    }

    #[test]
    fn maps_official_task_tools_onto_the_conversation_list() {
        assert!(is_claude_todo_tool("TaskCreate"));
        assert!(is_claude_todo_tool("TodoWrite"));
        assert!(!is_claude_todo_tool("todo_write"));
        assert!(!is_claude_todo_tool("TaskGet"));

        let created = apply_claude_todo_tool(
            &AgentTodoState::default(),
            "TaskCreate",
            &json!({
                "subject": "Phase 0：安装 .NET 10 SDK",
                "description": "用 winget 安装",
                "activeForm": "安装 .NET 10 SDK"
            }),
            "Task #1 created successfully: Phase 0：安装 .NET 10 SDK",
        )
        .expect("create");
        assert_eq!(created.items.len(), 1);
        assert_eq!(created.items[0].id, "1");
        assert_eq!(created.items[0].content, "Phase 0：安装 .NET 10 SDK");
        assert_eq!(created.items[0].status, AgentTodoStatus::Pending);
        assert_eq!(
            created.items[0].description.as_deref(),
            Some("用 winget 安装")
        );

        let started = apply_claude_todo_tool(
            &created,
            "TaskUpdate",
            &json!({ "taskId": "1", "status": "in_progress" }),
            "",
        )
        .expect("start");
        assert_eq!(started.items[0].status, AgentTodoStatus::InProgress);

        let aliased = apply_claude_todo_tool(
            &started,
            "TaskUpdate",
            &json!({ "task_id": "1", "status": "completed" }),
            "",
        )
        .expect("alias");
        assert_eq!(aliased.items[0].status, AgentTodoStatus::Completed);

        let second = apply_claude_todo_tool(
            &aliased,
            "TaskCreate",
            &json!({ "subject": "搭建骨架" }),
            r#"{"task":{"id":"2","subject":"搭建骨架"}}"#,
        )
        .expect("second");
        assert_eq!(second.items.len(), 2);
        assert_eq!(second.items[1].status, AgentTodoStatus::Pending);

        let deleted = apply_claude_todo_tool(
            &second,
            "TaskUpdate",
            &json!({ "taskId": "1", "status": "deleted" }),
            "",
        )
        .expect("delete");
        assert_eq!(deleted.items.len(), 1);
        assert_eq!(deleted.items[0].id, "2");

        let listed = apply_claude_todo_tool(
            &AgentTodoState::default(),
            "TaskList",
            &json!({}),
            r#"{"tasks":[{"id":"3","subject":"收尾","status":"in_progress","blockedBy":["2"]}]}"#,
        )
        .expect("list");
        assert_eq!(listed.items[0].id, "3");
        assert_eq!(listed.items[0].blocked_by, vec!["2".to_string()]);

        let legacy = apply_claude_todo_tool(
            &AgentTodoState::default(),
            "TodoWrite",
            &json!({
                "todos": [
                    { "id": "a", "content": "旧工具", "status": "pending", "activeForm": "在做" }
                ]
            }),
            "",
        )
        .expect("legacy");
        assert_eq!(legacy.items[0].id, "a");
        assert_eq!(legacy.items[0].content, "旧工具");

        assert!(apply_claude_todo_tool(
            &AgentTodoState {
                items: vec![pending("keep", "留着")],
                updated_at: 1,
            },
            "TaskCreate",
            &json!({ "subject": "没结果" }),
            "created ok",
        )
        .is_none());
        assert!(apply_claude_todo_tool(
            &AgentTodoState::default(),
            "TaskList",
            &json!({}),
            "No tasks found",
        )
        .is_none());
    }
}
