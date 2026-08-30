use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::state::AppState;

use super::agent;
use super::events;
use super::history;
use super::interpolate::{eval_if, interpolate, node_disabled};
use super::notify;
use super::storage;
use super::types::{
    Automation, AutomationRun, AutomationRunNode, AutomationRunStarted, FlowNode, NodeOutput,
    RunOrigin,
};

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
    state.cancel_chat_generation(&super::workspace::conversation_id(id));
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
        let result = execute_node(app, &automation.id, run_id, node, &prev, origin).await;
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
    automation_id: &str,
    run_id: &str,
    node: &FlowNode,
    prev: &NodeOutput,
    origin: RunOrigin,
) -> Result<(NodeOutput, Option<String>), String> {
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
        "action.agent" => {
            let prompt = node
                .data
                .get("agent")
                .and_then(|v| v.get("prompt"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let skill_id = node
                .data
                .get("agent")
                .and_then(|v| v.get("skillId"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let prompt = interpolate(prompt, prev);
            let output = agent::run_agent_node(app, automation_id, run_id, &prompt, skill_id).await?;
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
        other => Err(format!("unsupported node type: {other}")),
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
        .filter(|edge| match handle {
            Some(wanted) => edge.source_handle.as_deref().unwrap_or("true") == wanted,
            None => true,
        })
        .map(|edge| edge.target.clone())
        .collect()
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
}
