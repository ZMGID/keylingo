use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::native_tools::{read_file, run_command, write_file, NativeToolWorkspace};
use crate::state::AppState;

use super::agent;
use super::events;
use super::history;
use super::interpolate::{eval_if, interpolate, node_disabled};
use super::notify;
use super::storage;
use super::types::{
    Automation, AutomationRun, AutomationRunNode, AutomationRunStarted, FlowEdge, FlowNode,
    NodeOutput, RunOrigin,
};
use super::workspace;

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_BODY_MAX: usize = 50 * 1024;

pub fn enqueue(
    app: AppHandle,
    id: String,
    origin: RunOrigin,
    until_node_id: Option<String>,
) -> Result<AutomationRunStarted, String> {
    let automation = storage::get(&app, &id)?;
    if origin.is_production() && !automation.enabled {
        return Err("automation is not enabled".to_string());
    }
    if automation.nodes.is_empty() {
        return Err("automation has no nodes".to_string());
    }
    let state = app.state::<AppState>();
    {
        let active = state
            .automation_active_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if active.contains_key(&id) {
            return Err("automation is already running".to_string());
        }
    }
    let run_id = Uuid::new_v4().to_string();
    state
        .automation_active_runs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.clone(), run_id.clone());

    let app_run = app.clone();
    let run_id_spawn = run_id.clone();
    tauri::async_runtime::spawn(async move {
        let status = execute_graph(&app_run, automation, origin, until_node_id, &run_id_spawn).await;
        let state = app_run.state::<AppState>();
        state
            .automation_active_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        state
            .automation_cancelled_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&run_id_spawn);
        let _ = status;
    });
    Ok(AutomationRunStarted { run_id })
}

pub fn cancel(app: &AppHandle, id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let run_id = state
        .automation_active_runs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .cloned();
    let Some(run_id) = run_id else {
        return Ok(());
    };
    state
        .automation_cancelled_runs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(run_id);
        state.cancel_chat_generation(&workspace::conversation_id(id));
        state.cancel_chat_generation(&workspace::external_conversation_id(id, ""));
        if let Ok(automation) = storage::get(app, id) {
            for node in automation.nodes {
                if node.node_type == "action.agent" {
                    state.cancel_chat_generation(&workspace::external_conversation_id(
                        id,
                        &node.id,
                    ));
            }
        }
    }
    Ok(())
}

fn is_cancelled(app: &AppHandle, run_id: &str) -> bool {
    app.state::<AppState>()
        .automation_cancelled_runs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(run_id)
}

async fn execute_graph(
    app: &AppHandle,
    automation: Automation,
    origin: RunOrigin,
    until_node_id: Option<String>,
    run_id: &str,
) -> Result<(), String> {
    let started_at = now_iso();
    let mut record = AutomationRun {
        id: run_id.to_string(),
        automation_id: automation.id.clone(),
        origin: origin.as_str().to_string(),
        status: "running".into(),
        started_at: started_at.clone(),
        finished_at: None,
        error: None,
        nodes: Vec::new(),
    };
    let _ = history::write_run(app, &record);
    events::run_started(app, &automation.id, run_id);

    let Some(trigger) = automation
        .nodes
        .iter()
        .find(|node| node.node_type.starts_with("trigger."))
    else {
        return finish_run(app, record, "error", Some("no trigger node".into()));
    };

    let mut incoming = NodeOutput::with_json(
        origin.as_str(),
        json!({ "origin": origin.as_str(), "at": started_at }),
    );
    let mut queue = VecDeque::from([(trigger.id.clone(), incoming.clone())]);
    let mut visited = HashSet::new();
    let mut hit_until = until_node_id.is_none();

    while let Some((current_id, prev)) = queue.pop_front() {
        if !visited.insert(current_id.clone()) {
            continue;
        }
        if is_cancelled(app, run_id) {
            return finish_run(app, record, "cancelled", Some("cancelled".into()));
        }
        let Some(node) = automation.nodes.iter().find(|n| n.id == current_id) else {
            return finish_run(app, record, "error", Some("missing node".into()));
        };

        events::node_started(app, &automation.id, run_id, &node.id);
        let result = execute_node(app, &automation, run_id, node, &prev, origin).await;
        let handle_hint = match result {
            Ok((output, next_handle)) => {
                let preview = clip(&output.text, 2000);
                record.nodes.push(AutomationRunNode {
                    node_id: node.id.clone(),
                    node_type: node.node_type.clone(),
                    status: "success".into(),
                    output: Some(preview.clone()),
                    error: None,
                });
                events::node_finished(
                    app,
                    &automation.id,
                    run_id,
                    &node.id,
                    "success",
                    Some(preview),
                    None,
                );
                incoming = output;
                next_handle
            }
            Err(err) => {
                record.nodes.push(AutomationRunNode {
                    node_id: node.id.clone(),
                    node_type: node.node_type.clone(),
                    status: "error".into(),
                    output: None,
                    error: Some(err.clone()),
                });
                events::node_finished(
                    app,
                    &automation.id,
                    run_id,
                    &node.id,
                    "error",
                    None,
                    Some(err.clone()),
                );
                return finish_run(app, record, "error", Some(err));
            }
        };

        if until_node_id.as_deref() == Some(node.id.as_str()) {
            hit_until = true;
            continue;
        }
        for next in next_node_ids(&automation, &node.id, handle_hint.as_deref()) {
            queue.push_back((next, incoming.clone()));
        }
    }

    if !hit_until {
        return finish_run(
            app,
            record,
            "error",
            Some("that step is not on this run's path".into()),
        );
    }

    finish_run(app, record, "success", None)
}

fn finish_run(
    app: &AppHandle,
    mut record: AutomationRun,
    status: &str,
    error: Option<String>,
) -> Result<(), String> {
    record.status = status.to_string();
    record.finished_at = Some(now_iso());
    record.error = error.clone();
    let _ = history::write_run(app, &record);
    events::run_finished(app, &record.automation_id, &record.id, status, error);
    Ok(())
}

async fn execute_node(
    app: &AppHandle,
    automation: &Automation,
    run_id: &str,
    node: &FlowNode,
    prev: &NodeOutput,
    origin: RunOrigin,
) -> Result<(NodeOutput, Option<String>), String> {
    let automation_id = automation.id.as_str();
    if node_disabled(&node.data) {
        let handle = if node.node_type == "logic.if" {
            Some("true".to_string())
        } else {
            None
        };
        return Ok((prev.clone(), handle));
    }
    match node.node_type.as_str() {
        t if t.starts_with("trigger.") => Ok((
            NodeOutput::with_json(
                origin.as_str(),
                json!({ "origin": origin.as_str(), "trigger": t }),
            ),
            None,
        )),
        t if t.starts_with("agent.") => Ok((prev.clone(), None)),
        "action.agent" => {
            let mut spec = compose_agent_spec(automation, node);
            if let Some(prompt) = spec.get("prompt").and_then(|v| v.as_str()) {
                let interpolated = interpolate(prompt, prev);
                if let Some(obj) = spec.as_object_mut() {
                    obj.insert("prompt".into(), json!(interpolated));
                }
            }
            let output = agent::run_agent_node(app, automation_id, run_id, &node.id, &spec).await?;
            Ok((output, None))
        }
        "action.notify" => {
            let template = node
                .data
                .get("notify")
                .and_then(|v| v.get("body"))
                .and_then(|v| v.as_str())
                .unwrap_or("{{output}}");
            let body = interpolate(template, prev);
            let language = app
                .state::<AppState>()
                .settings_read()
                .settings_language
                .clone()
                .unwrap_or_else(|| "zh".to_string());
            let title = if language == "en" {
                "Kivio automation"
            } else {
                "Kivio 自动化"
            };
            notify::show(title, &body);
            Ok((NodeOutput::from_text(body), None))
        }
        "action.http" => {
            let output = execute_http(app, node, prev).await?;
            Ok((output, None))
        }
        "logic.if" => {
            let op = node
                .data
                .get("if")
                .and_then(|v| v.get("op"))
                .and_then(|v| v.as_str())
                .unwrap_or("contains");
            let expected = node
                .data
                .get("if")
                .and_then(|v| v.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let expected = interpolate(expected, prev);
            let passed = eval_if(op, &expected, &prev.text);
            let handle = if passed { "true" } else { "false" };
            Ok((
                NodeOutput::with_json(
                    handle,
                    json!({ "result": passed, "op": op, "value": expected }),
                ),
                Some(handle.to_string()),
            ))
        }
        "action.set" => {
            let fields = node
                .data
                .get("set")
                .and_then(|v| v.get("fields"))
                .cloned()
                .unwrap_or(json!([]));
            Ok((build_set_output(&fields, prev), None))
        }
        "logic.delay" => {
            execute_delay(app, run_id, node).await?;
            Ok((prev.clone(), None))
        }
        "action.clipboard" => Ok((execute_clipboard(node, prev)?, None)),
        "action.file" => Ok((execute_file(app, automation_id, node, prev)?, None)),
        "action.command" => Ok((execute_command(app, automation_id, node, prev).await?, None)),
        other => Err(format!("unsupported node type: {other}")),
    }
}

fn build_set_output(fields: &Value, prev: &NodeOutput) -> NodeOutput {
    let mut map = serde_json::Map::new();
    if let Some(items) = fields.as_array() {
        for item in items {
            let key = item
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if key.is_empty() {
                continue;
            }
            let value = item.get("value").and_then(Value::as_str).unwrap_or("");
            map.insert(key.to_string(), json!(interpolate(value, prev)));
        }
    }
    let object = Value::Object(map);
    let text = serde_json::to_string_pretty(&object).unwrap_or_else(|_| "{}".to_string());
    NodeOutput::with_json(text, object)
}

async fn execute_delay(app: &AppHandle, run_id: &str, node: &FlowNode) -> Result<(), String> {
    let seconds = node
        .data
        .get("delay")
        .and_then(|v| v.get("seconds"))
        .and_then(|v| v.as_u64().or_else(|| v.as_i64().and_then(|n| u64::try_from(n).ok())))
        .unwrap_or(1)
        .clamp(1, 600);
    let sleep = tokio::time::sleep(Duration::from_secs(seconds));
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            _ = &mut sleep => return Ok(()),
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                if is_cancelled(app, run_id) {
                    return Err("cancelled".to_string());
                }
            }
        }
    }
}

fn execute_clipboard(node: &FlowNode, prev: &NodeOutput) -> Result<NodeOutput, String> {
    let clipboard = node.data.get("clipboard").cloned().unwrap_or(Value::Null);
    let op = clipboard
        .get("op")
        .and_then(Value::as_str)
        .unwrap_or("copy");
    let mut board = arboard::Clipboard::new().map_err(|err| err.to_string())?;
    if op == "read" {
        let text = board.get_text().map_err(|err| err.to_string())?;
        return Ok(NodeOutput::from_text(text));
    }
    let text = interpolate(
        clipboard.get("text").and_then(Value::as_str).unwrap_or(""),
        prev,
    );
    board.set_text(&text).map_err(|err| err.to_string())?;
    Ok(NodeOutput::from_text(text))
}

fn execute_file(
    app: &AppHandle,
    automation_id: &str,
    node: &FlowNode,
    prev: &NodeOutput,
) -> Result<NodeOutput, String> {
    let file = node.data.get("file").cloned().unwrap_or(Value::Null);
    let op = file.get("op").and_then(Value::as_str).unwrap_or("write");
    let path = interpolate(file.get("path").and_then(Value::as_str).unwrap_or(""), prev);
    let workspace = node_workspace(app, automation_id);
    if op == "read" {
        let result = read_file(&workspace, &json!({ "path": path }))?;
        return Ok(NodeOutput::from_text(result.content));
    }
    let content = interpolate(
        file.get("content").and_then(Value::as_str).unwrap_or(""),
        prev,
    );
    write_file(&workspace, &json!({ "path": path, "content": content }))?;
    Ok(NodeOutput::with_json(
        path.clone(),
        json!({ "path": path, "op": "write" }),
    ))
}

async fn execute_command(
    app: &AppHandle,
    automation_id: &str,
    node: &FlowNode,
    prev: &NodeOutput,
) -> Result<NodeOutput, String> {
    let cmd = interpolate(
        node.data
            .get("command")
            .and_then(|v| v.get("command"))
            .and_then(Value::as_str)
            .unwrap_or(""),
        prev,
    );
    if cmd.trim().is_empty() {
        return Err("command is empty".to_string());
    }
    let workspace = node_workspace(app, automation_id);
    let state = app.state::<AppState>();
    let conv_id = workspace::conversation_id(automation_id);
    let output = run_command(
        &workspace,
        30_000,
        &json!({ "command": cmd }),
        Some(&*state),
        Some(&conv_id),
    )
    .await?;
    Ok(NodeOutput::from_text(output))
}

fn node_workspace(app: &AppHandle, automation_id: &str) -> NativeToolWorkspace {
    let working_directory = app
        .state::<AppState>()
        .settings_read()
        .chat_tools
        .native_tools
        .working_directory
        .clone();
    match workspace::workbench_dir(&working_directory, automation_id) {
        Some(dir) => NativeToolWorkspace::conversation(dir),
        None => NativeToolWorkspace::standalone(),
    }
}

async fn execute_http(
    app: &AppHandle,
    node: &FlowNode,
    prev: &NodeOutput,
) -> Result<NodeOutput, String> {
    let http = node.data.get("http").cloned().unwrap_or(Value::Null);
    let method = http
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("GET")
        .to_ascii_uppercase();
    let url = interpolate(
        http.get("url").and_then(|v| v.as_str()).unwrap_or(""),
        prev,
    );
    let url = url.trim();
    if url.is_empty() {
        return Err("HTTP URL is empty".to_string());
    }
    let parsed = url::Url::parse(url).map_err(|err| format!("invalid URL: {err}"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("HTTP node only allows http:// or https:// URLs".to_string());
    }
    let headers_raw = http
        .get("headers")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let body = interpolate(
        http.get("body").and_then(|v| v.as_str()).unwrap_or(""),
        prev,
    );

    let client = app.state::<AppState>().http.clone();
    let mut request = match method.as_str() {
        "POST" => client.post(url),
        "PUT" => client.put(url),
        "PATCH" => client.patch(url),
        "DELETE" => client.delete(url),
        _ => client.get(url),
    };
    for (name, value) in parse_headers(headers_raw) {
        let value = interpolate(&value, prev);
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::try_from(name),
            reqwest::header::HeaderValue::try_from(value),
        ) {
            request = request.header(name, value);
        }
    }
    if !body.is_empty() && method != "GET" {
        request = request.body(body);
    }
    let response = tokio::time::timeout(HTTP_TIMEOUT, request.send())
        .await
        .map_err(|_| "HTTP request timed out".to_string())?
        .map_err(|err| format!("HTTP request failed: {err}"))?;
    let status = response.status().as_u16();
    let bytes = response
        .bytes()
        .await
        .map_err(|err| format!("read HTTP body failed: {err}"))?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if text.len() > HTTP_BODY_MAX {
        text.truncate(HTTP_BODY_MAX);
        text.push('…');
    }
    let display = format!("HTTP {status}\n{text}");
    Ok(NodeOutput::with_json(
        display,
        json!({ "status": status, "body": text }),
    ))
}

fn parse_headers(raw: &str) -> Vec<(String, String)> {
    raw.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (name, value) = line.split_once(':')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some((name.to_string(), value.trim().to_string()))
        })
        .collect()
}

pub(crate) fn next_node_ids(automation: &Automation, from: &str, handle: Option<&str>) -> Vec<String> {
    automation
        .edges
        .iter()
        .filter(|edge| edge.source == from)
        .filter(|edge| !is_slot_edge(edge))
        .filter(|edge| match handle {
            Some(wanted) => edge.source_handle.as_deref().unwrap_or("true") == wanted,
            None => true,
        })
        .map(|edge| edge.target.clone())
        .collect()
}

fn is_slot_edge(edge: &FlowEdge) -> bool {
    matches!(
        edge.target_handle.as_deref(),
        Some("runtime" | "context" | "tool" | "skill")
    )
}

fn json_string_list(value: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(items)) = value else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for item in items {
        if let Some(id) = item.as_str().map(str::trim).filter(|s| !s.is_empty()) {
            if !ids.iter().any(|existing| existing == id) {
                ids.push(id.to_string());
            }
        }
    }
    ids
}

fn merge_agent_object(spec: &mut serde_json::Map<String, Value>, part: &Value, keys: &[&str]) {
    let Some(obj) = part.as_object() else {
        return;
    };
    for key in keys {
        if let Some(value) = obj.get(*key) {
            spec.insert((*key).to_string(), value.clone());
        }
    }
}

fn compose_agent_spec(automation: &Automation, node: &FlowNode) -> Value {
    let mut spec = node
        .data
        .get("agent")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !spec.is_object() {
        spec = json!({});
    }
    let mut tool_ids = Vec::new();
    let mut skill_ids = Vec::new();
    let mut saw_tools = false;
    let mut saw_skills = false;
    for edge in &automation.edges {
        if edge.target != node.id || !is_slot_edge(edge) {
            continue;
        }
        let Some(src) = automation.nodes.iter().find(|item| item.id == edge.source) else {
            continue;
        };
        let part = src.data.get("agent").cloned().unwrap_or(json!({}));
        let obj = spec.as_object_mut().expect("object");
        match edge.target_handle.as_deref() {
            Some("runtime") => merge_agent_object(
                obj,
                &part,
                &[
                    "runtimeKind",
                    "externalAgentId",
                    "externalModel",
                    "providerId",
                    "model",
                ],
            ),
            Some("context") => merge_agent_object(obj, &part, &["prompt"]),
            Some("tool") => {
                saw_tools = true;
                for id in json_string_list(part.get("toolIds")) {
                    if !tool_ids.iter().any(|existing| existing == &id) {
                        tool_ids.push(id);
                    }
                }
            }
            Some("skill") => {
                saw_skills = true;
                let mut ids = json_string_list(part.get("skillIds"));
                if let Some(legacy) = part
                    .get("skillId")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    if !ids.iter().any(|id| id == legacy) {
                        ids.insert(0, legacy.to_string());
                    }
                }
                for id in ids {
                    if !skill_ids.iter().any(|existing| existing == &id) {
                        skill_ids.push(id);
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(obj) = spec.as_object_mut() {
        if saw_tools {
            obj.insert("toolIds".into(), json!(tool_ids));
        }
        if saw_skills {
            obj.insert("skillIds".into(), json!(skill_ids));
        }
    }
    spec
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
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
    use crate::automation::types::{FlowEdge, SCHEMA_VERSION, Vec2, Viewport};

    fn node(id: &str, node_type: &str) -> FlowNode {
        FlowNode {
            id: id.into(),
            node_type: node_type.into(),
            position: Vec2 { x: 0.0, y: 0.0 },
            data: json!({}),
        }
    }

    fn edge(source: &str, target: &str, handle: Option<&str>) -> FlowEdge {
        FlowEdge {
            id: format!("e-{source}-{target}"),
            source: source.into(),
            target: target.into(),
            source_handle: handle.map(|s| s.to_string()),
            target_handle: None,
        }
    }

    #[test]
    fn follows_if_true_handle() {
        let automation = Automation {
            schema_version: SCHEMA_VERSION,
            id: "a".into(),
            name: "t".into(),
            enabled: false,
            nodes: vec![
                node("t", "trigger.manual"),
                node("i", "logic.if"),
                node("yes", "action.notify"),
                node("no", "action.notify"),
            ],
            edges: vec![
                edge("t", "i", None),
                edge("i", "yes", Some("true")),
                edge("i", "no", Some("false")),
            ],
            viewport: Viewport::default(),
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert_eq!(next_node_ids(&automation, "t", None), vec!["i".to_string()]);
        assert_eq!(
            next_node_ids(&automation, "i", Some("true")),
            vec!["yes".to_string()]
        );
        assert_eq!(
            next_node_ids(&automation, "i", Some("false")),
            vec!["no".to_string()]
        );
        assert_eq!(next_node_ids(&automation, "yes", None), Vec::<String>::new());
    }

    #[test]
    fn fans_out_all_outgoing() {
        let automation = Automation {
            schema_version: SCHEMA_VERSION,
            id: "a".into(),
            name: "t".into(),
            enabled: false,
            nodes: vec![
                node("t", "trigger.manual"),
                node("a", "action.agent"),
                node("b", "action.agent"),
            ],
            edges: vec![edge("t", "a", None), edge("t", "b", None)],
            viewport: Viewport::default(),
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert_eq!(
            next_node_ids(&automation, "t", None),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn slot_edges_do_not_advance_the_main_flow() {
        let automation = Automation {
            schema_version: SCHEMA_VERSION,
            id: "a".into(),
            name: "t".into(),
            enabled: false,
            nodes: vec![
                node("t", "trigger.manual"),
                node("a", "action.agent"),
                node("r", "agent.runtime"),
            ],
            edges: vec![
                edge("t", "a", None),
                FlowEdge {
                    id: "e-r-a".into(),
                    source: "r".into(),
                    target: "a".into(),
                    source_handle: Some("slot".into()),
                    target_handle: Some("runtime".into()),
                },
            ],
            viewport: Viewport::default(),
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert_eq!(next_node_ids(&automation, "t", None), vec!["a".to_string()]);
        assert_eq!(next_node_ids(&automation, "r", None), Vec::<String>::new());
    }

    #[test]
    fn compose_agent_reads_plugged_slot_nodes() {
        let mut agent = node("a", "action.agent");
        agent.data = json!({ "agent": { "prompt": "legacy" } });
        let mut runtime = node("r", "agent.runtime");
        runtime.data = json!({ "agent": { "runtimeKind": "chat", "model": "m", "providerId": "p" } });
        let mut context = node("c", "agent.context");
        context.data = json!({ "agent": { "prompt": "hello {{output}}" } });
        let automation = Automation {
            schema_version: SCHEMA_VERSION,
            id: "a".into(),
            name: "t".into(),
            enabled: false,
            nodes: vec![agent, runtime, context],
            edges: vec![
                FlowEdge {
                    id: "e1".into(),
                    source: "r".into(),
                    target: "a".into(),
                    source_handle: Some("slot".into()),
                    target_handle: Some("runtime".into()),
                },
                FlowEdge {
                    id: "e2".into(),
                    source: "c".into(),
                    target: "a".into(),
                    source_handle: Some("slot".into()),
                    target_handle: Some("context".into()),
                },
            ],
            viewport: Viewport::default(),
            created_at: String::new(),
            updated_at: String::new(),
        };
        let spec = compose_agent_spec(&automation, &automation.nodes[0]);
        assert_eq!(spec["runtimeKind"], "chat");
        assert_eq!(spec["prompt"], "hello {{output}}");
        assert_eq!(spec["model"], "m");
    }

    #[test]
    fn set_interpolates_output_into_fields() {
        let prev = NodeOutput::from_text("hello");
        let out = build_set_output(
            &json!([
                { "key": "msg", "value": "got {{output}}" },
                { "key": "", "value": "skip" },
                { "key": "  ", "value": "also skip" },
            ]),
            &prev,
        );
        assert_eq!(out.json["msg"], "got hello");
        assert!(out.json.get("").is_none());
        assert!(out.text.contains("got hello"));
    }

    #[test]
    fn set_empty_fields_outputs_empty_object() {
        let prev = NodeOutput::from_text("x");
        let out = build_set_output(&json!([]), &prev);
        assert_eq!(out.json, json!({}));
        assert_eq!(out.text, "{}");
    }
}
