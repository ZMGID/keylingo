//! Host-agnostic automation tools for chat agents (and, later, an MCP server
//! for external CLIs). Chat native tools are thin wrappers over this module;
//! do not put protocol-specific framing here.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::mcp::types::McpToolCallResult;
use crate::native_tools::TOOL_OUTPUT_MAX_BYTES;
use crate::state::AppState;

use super::commands;
use super::history;
use super::runner;
use super::storage;
use super::types::{
    Automation, AutomationRun, NodeOutput, RunOrigin, SCHEMA_VERSION, ValidationIssue,
};
use super::validate;

pub(crate) const RUN_DEFAULT_TIMEOUT_SECS: u64 = 600;
pub(crate) const RUN_MAX_TIMEOUT_SECS: u64 = 1_800;
const RUN_TIMEOUT_BUFFER_MS: u64 = 5_000;
const POLL_MS: u64 = 300;

pub(crate) fn run_timeout_ms(arguments: &Value) -> u64 {
    parse_timeout_secs(arguments)
        .saturating_mul(1000)
        .saturating_add(RUN_TIMEOUT_BUFFER_MS)
}

pub(crate) fn list(app: &AppHandle) -> Result<McpToolCallResult, String> {
    let items = storage::list(app)?;
    ok_json(json!({ "automations": items }))
}

pub(crate) fn get(app: &AppHandle, arguments: &Value) -> Result<McpToolCallResult, String> {
    let id = required_id(arguments)?;
    let automation = storage::get(app, &id)?;
    ok_json(json!({ "automation": automation }))
}

pub(crate) fn upsert(app: &AppHandle, arguments: &Value) -> Result<McpToolCallResult, String> {
    let mut automation = parse_upsert_automation(arguments)?;
    validate::prepare_for_upsert(&mut automation);
    if automation.name.trim().is_empty() {
        automation.name = "Untitled".to_string();
    }
    if !automation.id.trim().is_empty() {
        if let Ok(existing) = storage::get(app, &automation.id) {
            if automation.created_at.trim().is_empty() {
                automation.created_at = existing.created_at;
            }
        }
    }
    let issues = validate::validate(&automation);
    let dry_run = arguments
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if validate::has_errors(&issues) {
        return Ok(validation_error(&issues, Some(&automation)));
    }
    if dry_run {
        return ok_json(json!({
            "dryRun": true,
            "automation": automation,
            "issues": issues,
        }));
    }
    let previous = if automation.id.trim().is_empty() {
        None
    } else {
        storage::get(app, &automation.id).ok()
    };
    let saved = storage::save(app, automation)?;
    let hotkey_changed = previous
        .as_ref()
        .map(|old| hotkey_fingerprint(old) != hotkey_fingerprint(&saved))
        .unwrap_or(saved.enabled);
    if hotkey_changed {
        commands::refresh_hotkeys(app);
    }
    ok_json(json!({
        "automation": saved,
        "issues": issues,
        "created": previous.is_none(),
    }))
}

pub(crate) fn set_enabled(app: &AppHandle, arguments: &Value) -> Result<McpToolCallResult, String> {
    let id = required_id(arguments)?;
    let enabled = arguments
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| "enabled must be a boolean".to_string())?;
    let saved = storage::set_enabled(app, &id, enabled)?;
    commands::refresh_hotkeys(app);
    ok_json(json!({ "automation": saved.meta() }))
}

pub(crate) fn delete(app: &AppHandle, arguments: &Value) -> Result<McpToolCallResult, String> {
    let id = required_id(arguments)?;
    runner::cancel(app, &id)?;
    storage::delete(app, &id)?;
    commands::refresh_hotkeys(app);
    ok_json(json!({ "deleted": id }))
}

pub(crate) fn runs(app: &AppHandle, arguments: &Value) -> Result<McpToolCallResult, String> {
    let id = required_id(arguments)?;
    let run_id = arguments
        .get("run_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(run_id) = run_id {
        let run = history::get(app, &id, run_id)?;
        return ok_json(json!({ "run": clip_run(&run) }));
    }
    let runs = history::list(app, &id)?;
    ok_json(json!({ "runs": runs }))
}

pub(crate) async fn run(
    app: &AppHandle,
    arguments: &Value,
    chat_generation: Option<(&str, u64)>,
) -> Result<McpToolCallResult, String> {
    let id = required_id(arguments)?;
    let automation = storage::get(app, &id)?;
    let input = parse_run_input(arguments.get("input"));
    let timeout = Duration::from_secs(parse_timeout_secs(arguments));
    let started = runner::enqueue(
        app.clone(),
        id.clone(),
        RunOrigin::Agent,
        None,
        input,
    )?;
    let run_id = started.run_id;
    let mut guard = CancelOnDrop {
        app: app.clone(),
        id: id.clone(),
        armed: true,
    };
    let deadline = Instant::now() + timeout;
    loop {
        if let Some((conversation_id, generation)) = chat_generation {
            if !app
                .state::<AppState>()
                .is_chat_generation_active(conversation_id, generation)
            {
                let _ = runner::cancel(app, &id);
                guard.disarm();
                return Ok(error_json(
                    "cancelled",
                    json!({
                        "type": "automation_run",
                        "automationId": id,
                        "runId": run_id,
                        "name": automation.name,
                        "status": "cancelled",
                    }),
                    "Automation run was cancelled.",
                ));
            }
        }
        if Instant::now() >= deadline {
            guard.disarm();
            return ok_json(json!({
                "status": "running",
                "runId": run_id,
                "automationId": id,
                "name": automation.name,
                "message": "Automation is still running. Use automation_runs with this run_id to inspect progress.",
            }));
        }
        if !is_run_active(app, &id, &run_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
    }
    guard.disarm();
    let record = history::get(app, &id, &run_id)?;
    Ok(run_tool_result(&automation.name, &record))
}

fn is_run_active(app: &AppHandle, automation_id: &str, run_id: &str) -> bool {
    app.state::<AppState>()
        .automation_active_runs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(automation_id)
        .map(|active| active == run_id)
        .unwrap_or(false)
}

struct CancelOnDrop {
    app: AppHandle,
    id: String,
    armed: bool,
}

impl CancelOnDrop {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            let _ = runner::cancel(&self.app, &self.id);
        }
    }
}

fn parse_upsert_automation(arguments: &Value) -> Result<Automation, String> {
    let value = arguments
        .get("automation")
        .cloned()
        .ok_or_else(|| "automation object is required".to_string())?;
    let mut automation: Automation = serde_json::from_value(value)
        .map_err(|err| format!("invalid automation graph: {err}"))?;
    automation.schema_version = SCHEMA_VERSION;
    Ok(automation)
}

pub(crate) fn parse_run_input(value: Option<&Value>) -> Option<NodeOutput> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    if let Some(text) = value.as_str() {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        return Some(NodeOutput::from_text(text));
    }
    if let Some(obj) = value.as_object() {
        let text = obj
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let json = obj.get("json").cloned().unwrap_or(Value::Null);
        if text.trim().is_empty() && json.is_null() {
            return None;
        }
        let text = if text.trim().is_empty() {
            json.to_string()
        } else {
            text
        };
        let json = if json.is_null() {
            json!({ "text": text })
        } else {
            json
        };
        return Some(NodeOutput::with_json(text, json));
    }
    None
}

fn parse_timeout_secs(arguments: &Value) -> u64 {
    arguments
        .get("timeout_seconds")
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_i64().and_then(|n| u64::try_from(n).ok()))
        })
        .unwrap_or(RUN_DEFAULT_TIMEOUT_SECS)
        .clamp(5, RUN_MAX_TIMEOUT_SECS)
}

fn required_id(arguments: &Value) -> Result<String, String> {
    let id = arguments
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "id is required".to_string())?;
    storage::validate_id(id)?;
    Ok(id.to_string())
}

fn hotkey_fingerprint(automation: &Automation) -> String {
    let acc = automation
        .nodes
        .iter()
        .find(|node| node.node_type == "trigger.hotkey")
        .and_then(|node| {
            node.data
                .get("hotkey")
                .and_then(|v| v.get("accelerator"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("")
        .trim()
        .to_string();
    format!("{}:{acc}", automation.enabled)
}

fn clip_run(run: &AutomationRun) -> Value {
    json!({
        "id": run.id,
        "automationId": run.automation_id,
        "origin": run.origin,
        "status": run.status,
        "startedAt": run.started_at,
        "finishedAt": run.finished_at,
        "error": run.error,
        "nodes": run.nodes.iter().map(|node| json!({
            "nodeId": node.node_id,
            "nodeType": node.node_type,
            "status": node.status,
            "output": node.output.as_deref().map(|text| clip(text, 2000)),
            "error": node.error,
        })).collect::<Vec<_>>(),
    })
}

fn last_output(run: &AutomationRun) -> Option<String> {
    run.nodes
        .iter()
        .rev()
        .find_map(|node| node.output.clone())
}

fn run_tool_result(name: &str, run: &AutomationRun) -> McpToolCallResult {
    let output = last_output(run).map(|text| clip(&text, TOOL_OUTPUT_MAX_BYTES));
    let body = json!({
        "status": run.status,
        "runId": run.id,
        "automationId": run.automation_id,
        "name": name,
        "origin": run.origin,
        "error": run.error,
        "output": output,
        "nodes": run.nodes.iter().map(|node| json!({
            "nodeId": node.node_id,
            "nodeType": node.node_type,
            "status": node.status,
            "output": node.output.as_deref().map(|text| clip(text, 400)),
            "error": node.error,
        })).collect::<Vec<_>>(),
    });
    let structured = json!({
        "type": "automation_run",
        "automationId": run.automation_id,
        "runId": run.id,
        "name": name,
        "status": run.status,
        "nodes": body["nodes"],
    });
    let is_error = run.status == "error";
    McpToolCallResult {
        content: stringify(&body),
        is_error,
        raw: body,
        artifacts: Vec::new(),
        structured_content: Some(structured),
        follow_up_user_messages: Vec::new(),
    }
}

fn validation_error(issues: &[ValidationIssue], automation: Option<&Automation>) -> McpToolCallResult {
    let schema_hint = super::types::upsert_schema_hint();
    let body = json!({
        "ok": false,
        "issues": issues,
        "automation": automation,
        "allowedNodeTypes": super::types::ALLOWED_NODE_TYPES,
        "schemaHint": schema_hint,
        "message": "Fix every error issue and submit the COMPLETE graph in one automation_upsert. Do not probe types with dry_run. Warnings can be ignored.",
    });
    error_json(
        "validation",
        json!({
            "type": "automation_validation",
            "issues": issues,
        }),
        &stringify(&body),
    )
}

fn ok_json(value: Value) -> Result<McpToolCallResult, String> {
    Ok(McpToolCallResult {
        content: stringify(&value),
        is_error: false,
        raw: value.clone(),
        artifacts: Vec::new(),
        structured_content: Some(value),
        follow_up_user_messages: Vec::new(),
    })
}

fn error_json(kind: &str, structured: Value, content: &str) -> McpToolCallResult {
    let _ = kind;
    McpToolCallResult {
        content: content.to_string(),
        is_error: true,
        raw: structured.clone(),
        artifacts: Vec::new(),
        structured_content: Some(structured),
        follow_up_user_messages: Vec::new(),
    }
}

fn stringify(value: &Value) -> String {
    clip(
        &serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
        TOOL_OUTPUT_MAX_BYTES,
    )
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let clipped: String = text.chars().take(max).collect();
        format!("{clipped}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::types::{Automation, FlowNode, SCHEMA_VERSION, Vec2, Viewport};
    use crate::automation::validate::{has_errors, prepare_for_upsert, validate};

    #[test]
    fn parse_run_input_accepts_text_or_object() {
        assert!(parse_run_input(None).is_none());
        let text = parse_run_input(Some(&json!("  hi  "))).unwrap();
        assert_eq!(text.text, "hi");
        let obj = parse_run_input(Some(&json!({ "text": "q", "json": { "n": 1 } }))).unwrap();
        assert_eq!(obj.text, "q");
        assert_eq!(obj.json["n"], 1);
    }

    #[test]
    fn run_timeout_clamps_and_adds_buffer() {
        assert_eq!(
            run_timeout_ms(&json!({})),
            RUN_DEFAULT_TIMEOUT_SECS * 1000 + RUN_TIMEOUT_BUFFER_MS
        );
        assert_eq!(
            run_timeout_ms(&json!({ "timeout_seconds": 1 })),
            5 * 1000 + RUN_TIMEOUT_BUFFER_MS
        );
        assert_eq!(
            run_timeout_ms(&json!({ "timeout_seconds": 99_999 })),
            RUN_MAX_TIMEOUT_SECS * 1000 + RUN_TIMEOUT_BUFFER_MS
        );
    }

    #[test]
    fn upsert_parse_then_validate_rejects_bad_graphs() {
        let mut automation: Automation = serde_json::from_value(json!({
            "schemaVersion": SCHEMA_VERSION,
            "id": "",
            "name": "demo",
            "enabled": false,
            "nodes": [{ "id": "t", "type": "action.notify", "data": { "notify": { "body": "x" } } }],
            "edges": [],
            "createdAt": "",
            "updatedAt": "",
        }))
        .unwrap();
        prepare_for_upsert(&mut automation);
        assert!(has_errors(&validate(&automation)));
    }

    #[test]
    fn upsert_parse_accepts_omitted_positions() {
        let automation: Automation = serde_json::from_value(json!({
            "schemaVersion": SCHEMA_VERSION,
            "id": "",
            "name": "demo",
            "nodes": [
                { "id": "t", "type": "trigger.manual" },
                { "id": "n", "type": "action.notify", "data": { "notify": { "body": "{{output}}" } } }
            ],
            "edges": [{ "id": "e", "source": "t", "target": "n" }],
            "createdAt": "",
            "updatedAt": "",
        }))
        .unwrap();
        assert_eq!(automation.nodes[0].position.x, 0.0);
        let mut automation = automation;
        prepare_for_upsert(&mut automation);
        assert!(!has_errors(&validate(&automation)));
        assert!(automation.nodes[1].position.x > 0.0);
    }

    #[test]
    fn published_upsert_example_validates() {
        let mut automation: Automation =
            serde_json::from_str(crate::automation::types::UPSERT_MINIMAL_EXAMPLE).unwrap();
        prepare_for_upsert(&mut automation);
        assert!(!has_errors(&validate(&automation)), "{:?}", validate(&automation));
    }

    #[test]
    fn validation_error_returns_schema_hint() {
        let issues = validate(&Automation {
            schema_version: SCHEMA_VERSION,
            id: String::new(),
            name: "x".into(),
            enabled: false,
            nodes: vec![FlowNode {
                id: "n".into(),
                node_type: "action.teleport".into(),
                position: Vec2 { x: 0.0, y: 0.0 },
                data: serde_json::json!({}),
            }],
            edges: vec![],
            viewport: Viewport::default(),
            created_at: String::new(),
            updated_at: String::new(),
        });
        let result = validation_error(&issues, None);
        assert!(result.is_error);
        assert!(
            result.content.contains("schemaHint"),
            "{}",
            result.content
        );
        assert!(
            result.content.contains("trigger.schedule"),
            "{}",
            result.content
        );
        assert!(
            result.content.contains("allowedNodeTypes"),
            "{}",
            result.content
        );
    }

    #[test]
    fn last_output_prefers_the_final_success_node() {
        let run = AutomationRun {
            id: "r".into(),
            automation_id: "a".into(),
            origin: "agent".into(),
            status: "success".into(),
            started_at: "t0".into(),
            finished_at: Some("t1".into()),
            error: None,
            nodes: vec![
                crate::automation::types::AutomationRunNode {
                    node_id: "t".into(),
                    node_type: "trigger.manual".into(),
                    status: "success".into(),
                    output: Some("agent".into()),
                    error: None,
                },
                crate::automation::types::AutomationRunNode {
                    node_id: "n".into(),
                    node_type: "action.notify".into(),
                    status: "success".into(),
                    output: Some("done".into()),
                    error: None,
                },
            ],
        };
        assert_eq!(last_output(&run).as_deref(), Some("done"));
        let result = run_tool_result("demo", &run);
        assert!(!result.is_error);
        assert_eq!(
            result.structured_content.as_ref().unwrap()["type"],
            "automation_run"
        );
    }

    #[test]
    fn missing_positions_deserialize_on_flow_node() {
        let node: FlowNode = serde_json::from_value(json!({
            "id": "t",
            "type": "trigger.manual"
        }))
        .unwrap();
        assert_eq!(node.position, Vec2 { x: 0.0, y: 0.0 });
        let _ = Viewport::default();
    }
}
