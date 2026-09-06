use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use futures::FutureExt;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::native_tools::{
    read_file, run_captured_command, write_file, NativeToolWorkspace, TOOL_OUTPUT_MAX_BYTES,
};
use crate::settings::{CHAT_TOOL_MAX_TIMEOUT_MS, CHAT_TOOL_MIN_TIMEOUT_MS};
use crate::state::AppState;

use super::agent;
use super::events;
use super::history;
use super::interpolate::{check_references, eval_if, interpolate, node_disabled};
use super::notify;
use super::storage;
use super::types::{
    Automation, AutomationRun, AutomationRunNode, AutomationRunStarted, FlowEdge, FlowNode,
    NodeOutput, RunOrigin,
};
use super::workspace;

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_BODY_MAX: usize = 50 * 1024;
/// 全局并发帽：不同 id 的 schedule/hotkey/manual 同时触发时，每条都是完整的
/// agent loop / CLI 子进程，无帽会线性叠满 CPU/内存/供应商配额（对比 sub-agent
/// 有 12 的信号量）。超限直接拒绝并落错误——排队会让定时任务悄悄堆积。
const MAX_CONCURRENT_AUTOMATION_RUNS: usize = 4;

pub fn enqueue(
    app: AppHandle,
    id: String,
    origin: RunOrigin,
    until_node_id: Option<String>,
    input: Option<NodeOutput>,
) -> Result<AutomationRunStarted, String> {
    enqueue_mode(app, id, origin, until_node_id, input, None)
}

pub fn test_node(app: AppHandle, id: String, node_id: String, input: NodeOutput) -> Result<AutomationRunStarted, String> {
    enqueue_mode(app, id, RunOrigin::Manual, None, Some(input), Some(node_id))
}

fn enqueue_mode(
    app: AppHandle, id: String, origin: RunOrigin, until_node_id: Option<String>,
    input: Option<NodeOutput>, single_node_id: Option<String>,
) -> Result<AutomationRunStarted, String> {
    let automation = storage::get(&app, &id)?;
    execution_start(&automation, origin, single_node_id.as_deref())?;
    if origin.is_production() && !automation.enabled {
        return Err("automation is not enabled".to_string());
    }
    if automation.nodes.is_empty() {
        return Err("automation has no nodes".to_string());
    }
    let state = app.state::<AppState>();
    let run_id = Uuid::new_v4().to_string();
    {
        let mut active = state
            .automation_active_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if active.contains_key(&id) {
            return Err("automation is already running".to_string());
        }
        if active.len() >= MAX_CONCURRENT_AUTOMATION_RUNS {
            return Err(format!(
                "too many automations running concurrently (max {MAX_CONCURRENT_AUTOMATION_RUNS})"
            ));
        }
        active.insert(id.clone(), run_id.clone());
    }

    let mut record = AutomationRun {
        id: run_id.clone(),
        automation_id: id.clone(),
        origin: if single_node_id.is_some() { "test".into() } else { origin.as_str().into() },
        status: "running".into(),
        started_at: now_iso(),
        finished_at: None,
        error: None,
        nodes: Vec::new(),
    };
    let cleanup = RunCleanup {
        app: app.clone(),
        id,
        run_id: run_id.clone(),
    };
    history::write_run(&app, &record)?;
    let app_run = app.clone();
    let run_id_spawn = run_id.clone();
    tauri::async_runtime::spawn(async move {
        let _cleanup = cleanup;
        let outcome = std::panic::AssertUnwindSafe(execute_graph(
            &app_run,
            automation,
            origin,
            until_node_id,
            &run_id_spawn,
            input,
            &mut record,
            single_node_id,
        ))
        .catch_unwind()
        .await;
        if outcome.is_err() {
            let _ = finish_run(
                &app_run,
                &mut record,
                "error",
                Some("automation execution panicked".into()),
            );
        }
    });
    Ok(AutomationRunStarted { run_id })
}

/// Release the concurrency slot even if the future panics or is dropped.
struct RunCleanup {
    app: AppHandle,
    id: String,
    run_id: String,
}

impl Drop for RunCleanup {
    fn drop(&mut self) {
        let state = self.app.state::<AppState>();
        state
            .automation_active_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
        state
            .automation_cancelled_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.run_id);
    }
}

/// 退出前兜底：把所有运行中的自动化标记取消。返回条数。
/// 真正的收尾靠 [`wait_all_finished`]——等待期间运行时还活着，
/// 命令节点的 `select!` 被取消分支唤醒后 drop 掉 Child（kill_on_drop）才来得及执行。
pub fn cancel_all(app: &AppHandle) -> usize {
    let ids: Vec<String> = {
        let state = app.state::<AppState>();
        let active = state
            .automation_active_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        active.keys().cloned().collect()
    };
    for id in &ids {
        let _ = cancel(app, id);
    }
    ids.len()
}

/// 等到 active 表清空（配合外层 timeout 使用）。
pub async fn wait_all_finished(app: &AppHandle) {
    loop {
        let empty = app
            .state::<AppState>()
            .automation_active_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty();
        if empty {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
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
                state.cancel_chat_generation(&workspace::external_conversation_id(id, &node.id));
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

// Keep history bounded. Missing snapshots are explicitly unavailable for replay.
fn snapshot(output: &NodeOutput) -> Option<NodeOutput> {
    (serde_json::to_vec(output).ok()?.len() <= 512 * 1024).then(|| output.clone())
}

fn execution_start<'a>(automation: &'a Automation, origin: RunOrigin, single: Option<&str>) -> Result<&'a FlowNode, String> {
    if let Some(id) = single {
        let node = automation.nodes.iter().find(|node| node.id == id).ok_or("node does not exist")?;
        if !node.node_type.starts_with("action.") && !node.node_type.starts_with("logic.") {
            return Err("only action and logic nodes can be tested".into());
        }
        if node_disabled(&node.data) { return Err("enable the node before testing it".into()); }
        Ok(node)
    } else {
        start_trigger(automation, origin).ok_or_else(|| "no enabled trigger for this run".into())
    }
}

async fn execute_graph(
    app: &AppHandle,
    automation: Automation,
    origin: RunOrigin,
    until_node_id: Option<String>,
    run_id: &str,
    input: Option<NodeOutput>,
    record: &mut AutomationRun,
    single_node_id: Option<String>,
) -> Result<(), String> {
    events::run_started(app, &automation.id, run_id);

    let trigger = match execution_start(&automation, origin, single_node_id.as_deref()) {
        Ok(node) => node,
        Err(error) => return finish_run(app, record, "error", Some(error)),
    };

    let mut incoming = if single_node_id.is_some() { input.unwrap_or_else(|| NodeOutput::from_text("")) }
        else { seed_incoming(origin, &record.started_at, input) };
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

        record.nodes.push(AutomationRunNode {
            node_id: node.id.clone(),
            node_type: node.node_type.clone(),
            status: "running".into(),
            input: snapshot(&prev),
            result: None,
            output: None,
            error: None,
        });
        let node_index = record.nodes.len() - 1;
        let _ = history::write_run(app, record);
        events::node_started(app, &automation.id, run_id, &node.id);
        let result = execute_node(app, &automation, run_id, node, &prev, origin).await;
        let handle_hint = match result {
            Ok((mut output, next_handle)) => {
                output.sources = prev.sources.clone();
                output.sources.insert(node.id.clone(), json!({ "text": output.text, "json": output.json }));
                let preview = clip(&output.text, 2000);
                let status = if node_disabled(&node.data) {
                    "skipped"
                } else {
                    "success"
                };
                record.nodes[node_index] = AutomationRunNode {
                    node_id: node.id.clone(),
                    node_type: node.node_type.clone(),
                    status: status.into(),
                    input: snapshot(&prev),
                    result: snapshot(&output),
                    output: Some(preview.clone()),
                    error: None,
                };
                let _ = history::write_run(app, record);
                events::node_finished(
                    app,
                    &automation.id,
                    run_id,
                    &node.id,
                    status,
                    Some(preview),
                    None,
                );
                incoming = output;
                next_handle
            }
            Err(err) => {
                let cancelled = err == "cancelled" || is_cancelled(app, run_id);
                let status = if cancelled { "cancelled" } else { "error" };
                record.nodes[node_index] = AutomationRunNode {
                    node_id: node.id.clone(),
                    node_type: node.node_type.clone(),
                    status: status.into(),
                    input: snapshot(&prev),
                    result: None,
                    output: None,
                    error: Some(err.clone()),
                };
                events::node_finished(
                    app,
                    &automation.id,
                    run_id,
                    &node.id,
                    status,
                    None,
                    Some(err.clone()),
                );
                return finish_run(app, record, status, Some(err));
            }
        };

        if single_node_id.is_some() || until_node_id.as_deref() == Some(node.id.as_str()) {
            hit_until = true;
            continue;
        }
        for next in next_node_ids(&automation, &node.id, handle_hint.as_deref()) {
            if let Some(until) = until_node_id.as_deref() {
                if !reaches(&automation, &next, until) {
                    continue;
                }
            }
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
    record: &mut AutomationRun,
    status: &str,
    error: Option<String>,
) -> Result<(), String> {
    record.status = status.to_string();
    record.finished_at = Some(now_iso());
    record.error = error.clone();
    for node in &mut record.nodes {
        if node.status == "running" {
            node.status = status.to_string();
            node.error = error.clone();
            events::node_finished(
                app,
                &record.automation_id,
                &record.id,
                &node.node_id,
                status,
                None,
                error.clone(),
            );
        }
    }
    let _ = history::write_run(app, record);
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
        let handle = match node.node_type.as_str() {
            "logic.if" => Some("true".to_string()),
            "logic.switch" => Some("default".to_string()),
            _ => None,
        };
        return Ok((prev.clone(), handle));
    }
    if node.node_type != "action.agent" { check_references(&node.data, prev)?; }
    match node.node_type.as_str() {
        t if t.starts_with("trigger.") => Ok((trigger_output(origin, t, prev), None)),
        t if t.starts_with("agent.") => Ok((prev.clone(), None)),
        "action.agent" => {
            let mut spec = compose_agent_spec(automation, node);
            check_references(&spec, prev)?;
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
            notify::show(app, title, &body);
            Ok((NodeOutput::from_text(body), None))
        }
        "action.http" => {
            let output = tokio::select! {
                result = execute_http(app, node, prev) => result?,
                _ = wait_until_cancelled(app, run_id) => return Err("cancelled".into()),
            };
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
        "logic.switch" => {
            let handle = eval_switch(node, prev);
            Ok((
                NodeOutput::with_json(handle.clone(), json!({ "result": handle.clone() })),
                Some(handle),
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
        "action.command" => Ok((
            execute_command(app, automation_id, run_id, node, prev).await?,
            None,
        )),
        other => Err(format!("unsupported node type: {other}")),
    }
}

fn build_set_output(fields: &Value, prev: &NodeOutput) -> NodeOutput {
    let mut map = serde_json::Map::new();
    if let Some(items) = fields.as_array() {
        for item in items {
            let key = item.get("key").and_then(Value::as_str).unwrap_or("").trim();
            if key.is_empty() {
                continue;
            }
            let value = item.get("value").and_then(Value::as_str).unwrap_or("");
            map.insert(key.to_string(), parse_set_value(&interpolate(value, prev)));
        }
    }
    let object = Value::Object(map);
    let text = serde_json::to_string_pretty(&object).unwrap_or_else(|_| "{}".to_string());
    NodeOutput::with_json(text, object)
}

fn parse_set_value(raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Value::String(raw.to_string());
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| Value::String(raw.to_string()))
}

async fn execute_delay(app: &AppHandle, run_id: &str, node: &FlowNode) -> Result<(), String> {
    let seconds = node
        .data
        .get("delay")
        .and_then(|v| v.get("seconds"))
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_i64().and_then(|n| u64::try_from(n).ok()))
        })
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
        return Ok(NodeOutput::from_text(clip(&text, TOOL_OUTPUT_MAX_BYTES)));
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
    let working_directory = app
        .state::<AppState>()
        .settings_read()
        .chat_tools
        .native_tools
        .working_directory
        .clone();
    let Some(base) = workspace::workbench_dir(&working_directory, automation_id) else {
        return Err("set a working directory in Settings before using the File node".to_string());
    };
    let confined = workspace::confine_file_path(&base, &path)?;
    let path = confined.to_string_lossy().to_string();
    let workspace = NativeToolWorkspace::conversation(base);
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
    run_id: &str,
    node: &FlowNode,
    prev: &NodeOutput,
) -> Result<NodeOutput, String> {
    let spec = node.data.get("command").cloned().unwrap_or(Value::Null);
    let cmd = interpolate(
        spec.get("command").and_then(Value::as_str).unwrap_or(""),
        prev,
    );
    if cmd.trim().is_empty() {
        return Err("command is empty".to_string());
    }
    let continue_on_fail = spec
        .get("continueOnFail")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let timeout_ms = spec
        .get("timeoutSeconds")
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_i64().and_then(|n| u64::try_from(n).ok()))
        })
        .unwrap_or(30)
        .saturating_mul(1000)
        .clamp(CHAT_TOOL_MIN_TIMEOUT_MS, CHAT_TOOL_MAX_TIMEOUT_MS);
    let cwd_raw = interpolate(spec.get("cwd").and_then(Value::as_str).unwrap_or(""), prev);
    let working_directory = app
        .state::<AppState>()
        .settings_read()
        .chat_tools
        .native_tools
        .working_directory
        .clone();
    let cwd = command_cwd(&working_directory, automation_id, cwd_raw.trim())?;
    let state = app.state::<AppState>();
    let captured = tokio::select! {
        result = run_captured_command(&cmd, cwd.clone(), timeout_ms, Some(&*state)) => result?,
        _ = wait_until_cancelled(app, run_id) => return Err("cancelled".to_string()),
    };
    let stdout = clip(&captured.stdout, TOOL_OUTPUT_MAX_BYTES);
    let stderr = clip(&captured.stderr, TOOL_OUTPUT_MAX_BYTES);
    if captured.exit_code != 0 && !continue_on_fail {
        let mut err = format!("exit {}", captured.exit_code);
        if !stderr.is_empty() {
            err.push('\n');
            err.push_str(&stderr);
        } else if !stdout.is_empty() {
            err.push('\n');
            err.push_str(&stdout);
        }
        return Err(err);
    }
    Ok(command_node_output(
        &cmd,
        &cwd.to_string_lossy(),
        captured.exit_code,
        &stdout,
        &stderr,
    ))
}

fn command_cwd(
    working_directory: &str,
    automation_id: &str,
    cwd: &str,
) -> Result<std::path::PathBuf, String> {
    let Some(base) = workspace::workbench_dir(working_directory, automation_id) else {
        if cwd.is_empty() {
            return crate::native_tools::user_home_dir();
        }
        return Err(
            "set a working directory in Settings before using a custom command cwd".to_string(),
        );
    };
    if cwd.is_empty() {
        return Ok(base);
    }
    let confined = workspace::confine_file_path(&base, cwd)?;
    if !confined.is_dir() {
        return Err(format!(
            "Working directory is not a directory: {}",
            confined.display()
        ));
    }
    Ok(confined)
}

fn command_node_output(
    command: &str,
    cwd: &str,
    exit_code: i32,
    stdout: &str,
    stderr: &str,
) -> NodeOutput {
    let text = if !stdout.is_empty() {
        stdout.to_string()
    } else if !stderr.is_empty() {
        stderr.to_string()
    } else {
        String::new()
    };
    NodeOutput::with_json(
        text,
        json!({
            "command": command,
            "cwd": cwd,
            "exitCode": exit_code,
            "stdout": stdout,
            "stderr": stderr,
        }),
    )
}

async fn wait_until_cancelled(app: &AppHandle, run_id: &str) {
    loop {
        if is_cancelled(app, run_id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn eval_switch(node: &FlowNode, prev: &NodeOutput) -> String {
    let Some(cases) = node
        .data
        .get("switch")
        .and_then(|v| v.get("cases"))
        .and_then(Value::as_array)
    else {
        return "default".to_string();
    };
    for case in cases {
        let id = case
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(id) = id else {
            continue;
        };
        let op = case.get("op").and_then(Value::as_str).unwrap_or("contains");
        let expected = interpolate(
            case.get("value").and_then(Value::as_str).unwrap_or(""),
            prev,
        );
        if eval_if(op, &expected, &prev.text) {
            return id.to_string();
        }
    }
    "default".to_string()
}

fn start_trigger(automation: &Automation, origin: RunOrigin) -> Option<&FlowNode> {
    let wanted = match origin {
        RunOrigin::Manual | RunOrigin::Agent => "trigger.manual",
        RunOrigin::Schedule => "trigger.schedule",
        RunOrigin::Hotkey => "trigger.hotkey",
    };
    automation
        .nodes
        .iter()
        .filter(|node| !node_disabled(&node.data))
        .find(|node| node.node_type == wanted)
        .or_else(|| {
            if origin.is_production() {
                return None;
            }
            automation
                .nodes
                .iter()
                .find(|node| node.node_type.starts_with("trigger.") && !node_disabled(&node.data))
        })
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
    let url = interpolate(http.get("url").and_then(|v| v.as_str()).unwrap_or(""), prev);
    let url = url.trim();
    if url.is_empty() {
        return Err("HTTP URL is empty".to_string());
    }
    let parsed = url::Url::parse(url).map_err(|err| format!("invalid URL: {err}"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("HTTP node only allows http:// or https:// URLs".to_string());
    }
    let headers_raw = http.get("headers").and_then(|v| v.as_str()).unwrap_or("");
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
    read_http_response(request, HTTP_TIMEOUT).await
}

async fn read_http_response(
    request: reqwest::RequestBuilder,
    timeout: Duration,
) -> Result<NodeOutput, String> {
    tokio::time::timeout(timeout, async {
        let mut response = request
            .send()
            .await
            .map_err(|err| format!("HTTP request failed: {err}"))?;
        let status = response.status().as_u16();
        // Read at most the preview budget plus one byte to detect truncation.
        // Dropping the response here also closes an endless streaming body.
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| format!("read HTTP body failed: {err}"))?
        {
            let take = chunk.len().min(HTTP_BODY_MAX + 1 - bytes.len());
            bytes.extend_from_slice(&chunk[..take]);
            if bytes.len() > HTTP_BODY_MAX {
                break;
            }
        }
        let text = clip_bytes(&String::from_utf8_lossy(&bytes), HTTP_BODY_MAX);
        let display = format!("HTTP {status}\n{text}");
        Ok(NodeOutput::with_json(
            display,
            json!({ "status": status, "body": text }),
        ))
    })
    .await
    .map_err(|_| "HTTP request timed out".to_string())?
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

pub(crate) fn reaches(automation: &Automation, from: &str, until: &str) -> bool {
    let mut stack = vec![from.to_string()];
    let mut seen = HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if id == until {
            return true;
        }
        stack.extend(next_node_ids(automation, &id, None));
    }
    false
}

pub(crate) fn next_node_ids(
    automation: &Automation,
    from: &str,
    handle: Option<&str>,
) -> Vec<String> {
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
    crate::automation::types::is_slot_target_handle(edge.target_handle.as_deref())
}

pub(crate) fn seed_incoming(
    origin: RunOrigin,
    started_at: &str,
    input: Option<NodeOutput>,
) -> NodeOutput {
    match input {
        Some(payload) => NodeOutput::with_json(
            payload.text,
            json!({
                "origin": origin.as_str(),
                "at": started_at,
                "input": payload.json,
            }),
        ),
        None => NodeOutput::with_json(
            origin.as_str(),
            json!({ "origin": origin.as_str(), "at": started_at }),
        ),
    }
}

pub(crate) fn trigger_output(origin: RunOrigin, trigger: &str, prev: &NodeOutput) -> NodeOutput {
    let mut json = if prev.json.is_object() {
        prev.json.clone()
    } else {
        json!({})
    };
    if let Some(obj) = json.as_object_mut() {
        obj.insert("origin".into(), json!(origin.as_str()));
        obj.insert("trigger".into(), json!(trigger));
    }
    let text = if prev.text.is_empty() {
        origin.as_str().to_string()
    } else {
        prev.text.clone()
    };
    NodeOutput::with_json(text, json)
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
    let mut spec = node.data.get("agent").cloned().unwrap_or_else(|| json!({}));
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
        // A connected disabled slot still shadows legacy inline configuration.
        let part = if node_disabled(&src.data) {
            json!({ "runtimeKind": "builtin", "externalAgentId": null, "externalModel": null,
                "providerId": null, "model": null, "prompt": "", "toolIds": [], "skillIds": [] })
        } else {
            src.data.get("agent").cloned().unwrap_or(json!({}))
        };
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
            obj.remove("skillId");
        }
        match obj.get("runtimeKind").and_then(Value::as_str) {
            Some("external") => {
                obj.insert("providerId".into(), Value::Null);
                obj.insert("model".into(), Value::Null);
            }
            _ => {
                obj.insert("externalAgentId".into(), Value::Null);
                obj.insert("externalModel".into(), Value::Null);
            }
        }
    }
    spec
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn clip(text: &str, max_bytes: usize) -> String {
    clip_bytes(text, max_bytes)
}

pub(crate) fn clip_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let ellipsis = "…";
    let budget = max_bytes.saturating_sub(ellipsis.len());
    let mut end = budget;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{ellipsis}", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::types::{FlowEdge, Vec2, Viewport, SCHEMA_VERSION};

    async fn streaming_server(body: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
                .await
                .unwrap();
            if !body.is_empty() {
                socket
                    .write_all(format!("{:x}\r\n", body.len()).as_bytes())
                    .await
                    .unwrap();
                socket.write_all(&body).await.unwrap();
                socket.write_all(b"\r\n").await.unwrap();
            }
            // Intentionally never terminate the body, even after sending its data.
            std::future::pending::<()>().await;
        });
        (format!("http://{address}"), server)
    }

    #[tokio::test]
    async fn http_stops_reading_at_budget_without_waiting_for_eof_or_panicking_on_cjk() {
        let (url, server) = streaming_server("中".repeat(18_000).into_bytes()).await;
        let request = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(url);
        let output = read_http_response(request, Duration::from_secs(2)).await;
        server.abort();
        let output = output.unwrap();
        let body = output.json["body"].as_str().unwrap();
        assert!(body.len() <= HTTP_BODY_MAX);
        assert!(body.ends_with('…'));
        assert!(!body.contains('\u{fffd}'));
        assert_eq!(output.json["status"], 200);
    }

    #[tokio::test]
    async fn http_timeout_includes_body_after_headers_have_arrived() {
        let (url, server) = streaming_server(b"partial".to_vec()).await;
        let request = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(url);
        let result = read_http_response(request, Duration::from_millis(150)).await;
        server.abort();
        assert_eq!(result.unwrap_err(), "HTTP request timed out");
    }

    #[test]
    fn disabled_slots_shadow_legacy_tools_skills_and_prompt() {
        let automation: Automation = serde_json::from_value(json!({
            "nodes": [
                {"id":"a","type":"action.agent","data":{"agent":{"prompt":"legacy","toolIds":["old"],"skillId":"old-skill"}}},
                {"id":"t","type":"agent.tool","data":{"disabled":true,"agent":{"toolIds":["write_file"]}}},
                {"id":"s","type":"agent.skill","data":{"disabled":true,"agent":{"skillIds":["pdf"]}}},
                {"id":"c","type":"agent.context","data":{"disabled":true,"agent":{"prompt":"disabled"}}}
            ],
            "edges": [
                {"id":"t-a","source":"t","target":"a","targetHandle":"tool"},
                {"id":"s-a","source":"s","target":"a","targetHandle":"skill"},
                {"id":"c-a","source":"c","target":"a","targetHandle":"context"}
            ]
        })).unwrap();
        let spec = compose_agent_spec(&automation, &automation.nodes[0]);
        assert_eq!(spec["toolIds"], json!([]));
        assert_eq!(spec["skillIds"], json!([]));
        assert!(spec.get("skillId").is_none());
        assert_eq!(spec["prompt"], "");
    }

    #[test]
    fn production_never_falls_back_to_another_or_disabled_trigger() {
        let mut disabled = node("s", "trigger.schedule");
        disabled.data = json!({"disabled":true});
        let automation = graph(vec![disabled, node("m", "trigger.manual")]);
        assert!(start_trigger(&automation, RunOrigin::Schedule).is_none());
        assert!(start_trigger(&automation, RunOrigin::Hotkey).is_none());
        assert_eq!(
            start_trigger(&automation, RunOrigin::Manual).unwrap().id,
            "m"
        );
        let only_disabled = graph(vec![automation.nodes[0].clone()]);
        assert!(start_trigger(&only_disabled, RunOrigin::Manual).is_none());
    }

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
        assert_eq!(
            next_node_ids(&automation, "yes", None),
            Vec::<String>::new()
        );
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
        assert!(reaches(&automation, "t", "a"));
        assert!(reaches(&automation, "a", "a"));
        assert!(!reaches(&automation, "a", "b"));
        assert!(!reaches(&automation, "b", "a"));
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
        runtime.data =
            json!({ "agent": { "runtimeKind": "chat", "model": "m", "providerId": "p" } });
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
        assert!(spec.get("externalAgentId").unwrap().is_null());
        assert!(spec.get("externalModel").unwrap().is_null());
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
    fn set_parses_json_literals_and_keeps_plain_text() {
        let prev = NodeOutput::with_json("x", json!({ "status": 200, "ok": true }));
        let out = build_set_output(
            &json!([
                { "key": "n", "value": "{{json.status}}" },
                { "key": "flag", "value": "{{json.ok}}" },
                { "key": "note", "value": "plain" },
            ]),
            &prev,
        );
        assert_eq!(out.json["n"], 200);
        assert_eq!(out.json["flag"], true);
        assert_eq!(out.json["note"], "plain");
    }

    #[test]
    fn set_empty_fields_outputs_empty_object() {
        let prev = NodeOutput::from_text("x");
        let out = build_set_output(&json!([]), &prev);
        assert_eq!(out.json, json!({}));
        assert_eq!(out.text, "{}");
    }

    fn graph(nodes: Vec<FlowNode>) -> Automation {
        Automation {
            schema_version: SCHEMA_VERSION,
            id: "a".into(),
            name: "t".into(),
            enabled: false,
            nodes,
            edges: vec![],
            viewport: Viewport::default(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn start_trigger_matches_run_origin() {
        let automation = graph(vec![
            node("s", "trigger.schedule"),
            node("m", "trigger.manual"),
        ]);
        assert_eq!(
            start_trigger(&automation, RunOrigin::Manual).map(|n| n.id.as_str()),
            Some("m")
        );
        assert_eq!(
            start_trigger(&automation, RunOrigin::Agent).map(|n| n.id.as_str()),
            Some("m")
        );
        assert_eq!(
            start_trigger(&automation, RunOrigin::Schedule).map(|n| n.id.as_str()),
            Some("s")
        );
        assert_eq!(
            start_trigger(&automation, RunOrigin::Hotkey).map(|n| n.id.as_str()),
            None
        );
    }

    #[test]
    fn switch_picks_first_matching_case_else_default() {
        let mut sw = node("s", "logic.switch");
        sw.data = json!({
            "switch": {
                "cases": [
                    { "id": "ok", "op": "contains", "value": "pass" },
                    { "id": "fail", "op": "equals", "value": "error" }
                ]
            }
        });
        assert_eq!(
            eval_switch(&sw, &NodeOutput::from_text("looks like pass")),
            "ok"
        );
        assert_eq!(eval_switch(&sw, &NodeOutput::from_text("error")), "fail");
        assert_eq!(eval_switch(&sw, &NodeOutput::from_text("other")), "default");
    }

    #[test]
    fn command_output_prefers_stdout_and_keeps_json() {
        let out = command_node_output("echo hi", "/tmp", 0, "hi\n", "");
        assert_eq!(out.text, "hi\n");
        assert_eq!(out.json["exitCode"], 0);
        assert_eq!(out.json["stdout"], "hi\n");
        let err = command_node_output("false", "/tmp", 1, "", "nope");
        assert_eq!(err.text, "nope");
        assert_eq!(err.json["exitCode"], 1);
    }

    #[test]
    fn seed_incoming_nests_agent_payload() {
        let input = NodeOutput::with_json("hello", json!({ "q": "hello" }));
        let seeded = seed_incoming(RunOrigin::Agent, "t0", Some(input));
        assert_eq!(seeded.text, "hello");
        assert_eq!(seeded.json["origin"], "agent");
        assert_eq!(seeded.json["input"]["q"], "hello");
        let trigger = trigger_output(RunOrigin::Agent, "trigger.manual", &seeded);
        assert_eq!(trigger.text, "hello");
        assert_eq!(trigger.json["trigger"], "trigger.manual");
        assert_eq!(trigger.json["input"]["q"], "hello");
    }

    #[test]
    fn trigger_output_keeps_origin_text_without_input() {
        let prev = seed_incoming(RunOrigin::Manual, "t0", None);
        let trigger = trigger_output(RunOrigin::Manual, "trigger.manual", &prev);
        assert_eq!(trigger.text, "manual");
        assert_eq!(trigger.json["origin"], "manual");
        assert!(trigger.json.get("input").is_none());
    }

    #[test]
    fn clip_bytes_does_not_exceed_budget_on_cjk() {
        let text = "你好世界";
        let clipped = clip_bytes(text, 7);
        assert!(clipped.len() <= 7);
        assert!(clipped.ends_with('…'));
    }

    #[test]
    fn isolated_test_starts_at_requested_action_even_without_a_trigger() {
        let mut automation = graph(vec![node("step", "action.set")]);
        assert_eq!(execution_start(&automation, RunOrigin::Manual, Some("step")).unwrap().id, "step");
        assert!(execution_start(&automation, RunOrigin::Manual, None).is_err());
        assert!(execution_start(&automation, RunOrigin::Manual, Some("missing")).is_err());
        automation.nodes[0].data = json!({"disabled": true});
        assert!(execution_start(&automation, RunOrigin::Manual, Some("step")).is_err());
        automation.nodes.push(node("slot", "agent.tool"));
        assert!(execution_start(&automation, RunOrigin::Manual, Some("slot")).is_err());
    }

    #[test]
    fn replay_snapshot_keeps_structured_input_but_omits_oversized_records() {
        let input = NodeOutput::with_json("商品", json!({"items": [1, 2, 3]}));
        let stored = snapshot(&input).unwrap();
        assert_eq!(stored.json, input.json);
        assert_eq!(stored.text, input.text);
        assert!(snapshot(&NodeOutput::from_text("大".repeat(512 * 1024))).is_none());
    }
}
