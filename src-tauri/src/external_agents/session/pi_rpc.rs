use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio::time::timeout;

use crate::external_agents::context::parse_context_window_label;
use crate::external_agents::stream::{usage_from_parts, CliUsageParts};
use crate::external_agents::types::{
    default_model_option, ExternalCliSlashCommand, RuntimeModelOption, UnifiedAgentEvent,
};
use crate::proc::NoConsoleWindow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiRpcOutcome {
    Continue,
    AgentEnd,
}

/// Discover Pi slash commands via the RPC `get_commands` request.
/// Response shape: `{type:"response", command:"get_commands", data:{commands:[{name, description}]}}`.
pub async fn detect_pi_commands(
    bin: &Path,
    args: &[&str],
    cwd: &Path,
    timeout_secs: u64,
) -> Option<Vec<ExternalCliSlashCommand>> {
    let mut child = crate::external_agents::spawn::cli_command(bin)
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .no_console_window()
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;
    let mut reader = BufReader::new(stdout).lines();

    let req = json!({ "id": 1, "type": "get_commands" }).to_string();
    stdin.write_all(format!("{req}\n").as_bytes()).await.ok()?;

    let started = std::time::Instant::now();
    let mut commands: Option<Vec<ExternalCliSlashCommand>> = None;
    loop {
        if started.elapsed() > Duration::from_secs(timeout_secs) {
            break;
        }
        let line = match timeout(Duration::from_millis(200), reader.next_line()).await {
            Ok(Ok(Some(l))) => l,
            Ok(Ok(None)) => break,
            Ok(Err(_)) => break,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let is_get_commands = value.get("type").and_then(|v| v.as_str()) == Some("response")
            && value.get("command").and_then(|v| v.as_str()) == Some("get_commands");
        if !is_get_commands {
            continue;
        }
        let list = value
            .get("data")
            .and_then(|d| d.get("commands"))
            .and_then(|v| v.as_array());
        if let Some(list) = list {
            let mut out = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for raw in list {
                let Some(name) = raw
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    continue;
                };
                if seen.insert(name.to_string()) {
                    out.push(ExternalCliSlashCommand {
                        slash: format!("/{name}"),
                        name: name.to_string(),
                        description: raw
                            .get("description")
                            .and_then(|v| v.as_str())
                            .map(|d| d.trim().to_string())
                            .filter(|d| !d.is_empty()),
                        argument_hint: None,
                    });
                }
            }
            out.sort_by(|a, b| a.name.cmp(&b.name));
            commands = Some(out);
        }
        break;
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
    commands.filter(|c| !c.is_empty())
}

const FIRE_AND_FORGET: &[&str] = &[
    "setStatus",
    "setWidget",
    "notify",
    "setTitle",
    "set_editor_text",
];

const BTW_COMMANDS: &[&str] = &[
    "btw",
    "btw:tangent",
    "btw:new",
    "btw:clear",
    "btw:inject",
    "btw:summarize",
    "btw:model",
    "btw:thinking",
];
const BTW_ENTRY_TYPE: &str = "btw-thread-entry";
const BTW_COMMAND_PROBE_ID: &str = "kivio-btw-command-probe";
const BTW_ENTRIES_REQUEST_ID: &str = "kivio-btw-entries";

#[derive(Debug, Clone, PartialEq, Eq)]
struct PiBtwCommand {
    name: String,
    question: Option<String>,
}

fn normalize_space(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_pi_btw_command(prompt: &str) -> Option<PiBtwCommand> {
    let command = prompt.trim().strip_prefix('/')?;
    let mut parts = command.splitn(2, char::is_whitespace);
    let name = parts.next()?.trim();
    if !BTW_COMMANDS.contains(&name) {
        return None;
    }

    let args = parts.next().unwrap_or_default();
    let question = matches!(name, "btw" | "btw:tangent" | "btw:new")
        .then(|| {
            args.split_whitespace()
                .filter(|part| !matches!(*part, "--save" | "-s"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|value| !value.is_empty());

    Some(PiBtwCommand {
        name: name.to_string(),
        question,
    })
}

fn response_registers_command(value: &Value, command_name: &str) -> bool {
    value.get("type").and_then(Value::as_str) == Some("response")
        && value.get("id").and_then(Value::as_str) == Some(BTW_COMMAND_PROBE_ID)
        && value.get("success").and_then(Value::as_bool) == Some(true)
        && value
            .get("data")
            .and_then(|data| data.get("commands"))
            .and_then(Value::as_array)
            .is_some_and(|commands| {
                commands.iter().any(|command| {
                    command.get("name").and_then(Value::as_str) == Some(command_name)
                        && command.get("source").and_then(Value::as_str) == Some("extension")
                })
            })
}

async fn probe_registered_btw_command<R, W>(
    reader: &mut tokio::io::Lines<BufReader<R>>,
    stdin: &mut W,
    command: &PiBtwCommand,
) -> bool
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let request = json!({
        "id": BTW_COMMAND_PROBE_ID,
        "type": "get_commands",
    });
    let Ok(mut line) = serde_json::to_string(&request) else {
        return false;
    };
    line.push('\n');
    if stdin.write_all(line.as_bytes()).await.is_err() {
        return false;
    }

    let started = std::time::Instant::now();
    while started.elapsed() <= Duration::from_secs(3) {
        let raw = match timeout(Duration::from_millis(200), reader.next_line()).await {
            Ok(Ok(Some(raw))) => raw,
            Ok(Ok(None)) | Ok(Err(_)) => return false,
            Err(_) => continue,
        };
        let Ok(value) = serde_json::from_str::<Value>(raw.trim()) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) == Some("extension_ui_request") {
            let _ = reply_extension_ui(stdin, &value).await;
            continue;
        }
        if value.get("id").and_then(Value::as_str) == Some(BTW_COMMAND_PROBE_ID) {
            return response_registers_command(&value, &command.name);
        }
    }
    false
}

fn pi_btw_entry_events_from_response(
    response: &Value,
    command: &PiBtwCommand,
) -> Option<(UnifiedAgentEvent, UnifiedAgentEvent)> {
    let expected_question = command.question.as_deref()?;
    let entries = response
        .get("data")
        .and_then(|data| data.get("entries"))
        .and_then(Value::as_array)?;
    let entry = entries.iter().rev().find(|entry| {
        entry.get("type").and_then(Value::as_str) == Some("custom")
            && entry.get("customType").and_then(Value::as_str) == Some(BTW_ENTRY_TYPE)
            && entry
                .get("data")
                .and_then(|data| data.get("question"))
                .and_then(Value::as_str)
                .is_some_and(|question| {
                    normalize_space(question) == normalize_space(expected_question)
                })
    })?;
    pi_btw_entry_events(entry, expected_question)
}

fn pi_btw_entry_events(
    entry: &Value,
    expected_question: &str,
) -> Option<(UnifiedAgentEvent, UnifiedAgentEvent)> {
    if entry.get("type").and_then(Value::as_str) != Some("custom")
        || entry.get("customType").and_then(Value::as_str) != Some(BTW_ENTRY_TYPE)
    {
        return None;
    }
    let data = entry.get("data")?;
    let question = data.get("question").and_then(Value::as_str)?.trim();
    let answer = data.get("answer").and_then(Value::as_str)?.trim();
    if question.is_empty()
        || answer.is_empty()
        || normalize_space(question) != normalize_space(expected_question)
    {
        return None;
    }

    let entry_id = entry
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("completed");
    let tool_id = format!("pi_btw_{entry_id}");
    let provider = data
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let model_id = data
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let model = match (provider.is_empty(), model_id.is_empty()) {
        (false, false) => Some(format!("{provider}/{model_id}")),
        (true, false) => Some(model_id.to_string()),
        _ => None,
    };
    let usage = data.get("usage").and_then(Value::as_object).map(|usage| {
        let number = |key: &str| usage.get(key).and_then(Value::as_u64);
        let input_tokens = number("input")
            .unwrap_or(0)
            .saturating_add(number("cacheRead").unwrap_or(0))
            .saturating_add(number("cacheWrite").unwrap_or(0));
        json!({
            "inputTokens": input_tokens,
            "outputTokens": number("output"),
            "totalTokens": number("totalTokens"),
        })
    });
    let structured = json!({
        "type": "subagent",
        "agentType": "btw",
        "name": "BTW",
        "model": model,
        "depth": 1,
        "status": "completed",
        "prompt": question,
        "result": answer,
        "usage": usage,
    });

    Some((
        UnifiedAgentEvent::ToolUse {
            id: tool_id.clone(),
            name: "Agent".to_string(),
            input: structured,
        },
        UnifiedAgentEvent::ToolResult {
            tool_use_id: tool_id,
            content: answer.to_string(),
            is_error: false,
        },
    ))
}

pub fn parse_pi_models(stderr: &str) -> Option<Vec<RuntimeModelOption>> {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    if lines.len() <= 1 {
        return None;
    }
    let mut out = vec![default_model_option()];
    let mut seen = std::collections::HashSet::from(["default".to_string()]);
    for line in lines.iter().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let full_id = format!("{}/{}", parts[0], parts[1]);
        if seen.insert(full_id.clone()) {
            let context_window_tokens = parts
                .get(2)
                .and_then(|label| parse_context_window_label(label));
            out.push(RuntimeModelOption {
                id: full_id.clone(),
                label: full_id,
                context_window_tokens,
            });
        }
    }
    if out.len() > 1 {
        Some(out)
    } else {
        None
    }
}

pub fn map_pi_rpc_event(value: &Value, sink: &mut dyn FnMut(UnifiedAgentEvent)) -> PiRpcOutcome {
    let obj = match value.as_object() {
        Some(o) => o,
        None => return PiRpcOutcome::Continue,
    };
    let kind = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match kind {
        "agent_start" => {}
        "agent_end" => return PiRpcOutcome::AgentEnd,
        "turn_start" => {}
        "turn_end" => {
            if let Some(message) = obj.get("message").and_then(|v| v.as_object()) {
                if let Some(usage) = message.get("usage").and_then(|v| v.as_object()) {
                    let field = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
                    // 实测 pi 的 usage 形状：
                    //   {"input":6571,"output":1578,"cacheRead":4096,"cacheWrite":0,
                    //    "reasoning":26,"totalTokens":12245}
                    // 对账 6571 + 1578 + 4096 = 12245 = totalTokens，说明：
                    //   * cacheRead/cacheWrite 与 input 并列，**必须**计入（实测 cacheRead 占 62%）
                    //   * `reasoning` 已含在 output 内，刻意**不读**，否则重复计数
                    let parts = CliUsageParts {
                        input: field("input"),
                        output: field("output"),
                        cache_read: field("cacheRead"),
                        cache_creation: field("cacheWrite"),
                        // cacheRead/cacheWrite 与 input **不相交**（上面的对账已证明：
                        // 三者相加恰等于 pi 自报的 totalTokens）。codex 相反，别照抄。
                        cache_included_in_input: false,
                        ..Default::default()
                    };
                    if parts.input > 0
                        || parts.output > 0
                        || parts.cache_read > 0
                        || parts.cache_creation > 0
                    {
                        sink(UnifiedAgentEvent::Usage {
                            usage: usage_from_parts(parts),
                        });
                    }
                }
                if message.get("stopReason").and_then(|v| v.as_str()) == Some("error") {
                    let message_text = message
                        .get("errorMessage")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Pi agent error");
                    sink(UnifiedAgentEvent::Error {
                        message: message_text.to_string(),
                    });
                }
            }
        }
        "message_update" => {
            if let Some(ev) = obj.get("assistantMessageEvent").and_then(|v| v.as_object()) {
                let ev_type = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match ev_type {
                    "text_delta" => {
                        if let Some(delta) = ev.get("delta").and_then(|v| v.as_str()) {
                            sink(UnifiedAgentEvent::TextDelta {
                                delta: delta.to_string(),
                            });
                        }
                    }
                    "thinking_delta" => {
                        if let Some(delta) = ev.get("delta").and_then(|v| v.as_str()) {
                            sink(UnifiedAgentEvent::ThinkingDelta {
                                delta: delta.to_string(),
                            });
                        }
                    }
                    "error" => {
                        let message = ev
                            .get("reason")
                            .or_else(|| ev.get("delta"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("Agent error");
                        sink(UnifiedAgentEvent::Error {
                            message: message.to_string(),
                        });
                    }
                    _ => {}
                }
            }
        }
        "tool_execution_start" => {
            let id = obj
                .get("toolCallId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = obj
                .get("toolName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let input = obj.get("args").cloned().unwrap_or(Value::Null);
            if !id.is_empty() && !name.is_empty() {
                sink(UnifiedAgentEvent::ToolUse { id, name, input });
            }
        }
        "tool_execution_end" => {
            let tool_use_id = obj
                .get("toolCallId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let result = obj.get("result").and_then(|v| v.as_object());
            let content = result
                .and_then(|r| r.get("content"))
                .map(|c| match c {
                    Value::String(s) => s.clone(),
                    _ => c.to_string(),
                })
                .unwrap_or_default();
            let is_error = obj
                .get("isError")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !tool_use_id.is_empty() {
                sink(UnifiedAgentEvent::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                });
            }
        }
        "extension_error" => {
            let message = obj
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("Extension error");
            sink(UnifiedAgentEvent::Error {
                message: message.to_string(),
            });
        }
        "auto_retry_start" => {
            let attempt = obj.get("attempt").and_then(|v| v.as_u64()).unwrap_or(1);
            let max_attempts = obj
                .get("maxAttempts")
                .and_then(|v| v.as_u64())
                .unwrap_or(attempt);
            sink(UnifiedAgentEvent::StatusNote {
                text: format!("Pi 正在自动重试（{attempt}/{max_attempts}）…"),
            });
        }
        "auto_retry_end" if obj.get("success").and_then(|v| v.as_bool()) == Some(false) => {
            let message = obj
                .get("finalError")
                .and_then(|v| v.as_str())
                .unwrap_or("Auto-retry exhausted");
            sink(UnifiedAgentEvent::Error {
                message: message.to_string(),
            });
        }
        _ => {}
    }
    PiRpcOutcome::Continue
}

async fn reply_extension_ui<W>(stdin: &mut W, raw: &Value) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
{
    let id = raw.get("id").cloned();
    if id.is_none() {
        return Ok(());
    }
    if let Some(method) = raw.get("method").and_then(|v| v.as_str()) {
        if FIRE_AND_FORGET.contains(&method) {
            return Ok(());
        }
    }
    let result = if raw.get("method").and_then(|v| v.as_str()) == Some("confirm") {
        json!({ "confirmed": true })
    } else {
        let opts = raw
            .get("params")
            .and_then(|p| p.get("options"))
            .or_else(|| raw.get("options"))
            .and_then(|v| v.as_array());
        if let Some(opts) = opts {
            if let Some(first) = opts.first() {
                let value = first
                    .as_str()
                    .map(|s| s.to_string())
                    .or_else(|| {
                        first
                            .as_object()
                            .and_then(|o| o.get("label").or_else(|| o.get("value")))
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    })
                    .unwrap_or_default();
                json!({ "value": value })
            } else {
                json!({ "cancelled": true })
            }
        } else {
            json!({ "cancelled": true })
        }
    };
    let mut payload = json!({ "type": "extension_ui_response", "id": id });
    if let Some(obj) = payload.as_object_mut() {
        if let Some(result_obj) = result.as_object() {
            for (k, v) in result_obj {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    let mut line = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|e| e.to_string())
}

pub async fn run_pi_rpc_session(
    child: &mut Child,
    prompt: &str,
    _model: Option<&str>,
    mut sink: impl FnMut(UnifiedAgentEvent),
    cancel_check: impl Fn() -> bool,
) -> Result<(), String> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "stdin unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "stdout unavailable".to_string())?;
    let mut reader = BufReader::new(stdout).lines();

    // pi-btw owns its sub-session and persists completed exchanges as custom session entries.
    // Verify that the slash name is an installed extension command before enabling the adapter:
    // an uninstalled `/btw ...` is just a normal model prompt and must keep the ordinary stream.
    let btw_command = match parse_pi_btw_command(prompt) {
        Some(command) if probe_registered_btw_command(&mut reader, &mut stdin, &command).await => {
            Some(command)
        }
        _ => None,
    };

    let prompt_line = {
        let payload = json!({ "id": 1, "type": "prompt", "message": prompt });
        let mut line = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
        line.push('\n');
        line
    };
    stdin
        .write_all(prompt_line.as_bytes())
        .await
        .map_err(|e| e.to_string())?;

    let result = drain_pi_rpc_lines(
        &mut reader,
        &mut stdin,
        &mut sink,
        cancel_check,
        btw_command.as_ref(),
    )
    .await;
    match &result {
        Err(err) if err == "cancelled" => {
            let _ = child.start_kill();
        }
        Ok(()) => {
            // agent_end 已收、轮次在协议层完成。立刻终止子进程：drain 返回时 stdout 读端已随
            // reader drop 关闭，pi 若还在冲刷输出会撞 EPIPE 以退出码 1 死掉，被出口的
            // 「非零退出+stderr」规则误判为「生成异常结束」。主动 kill 使退出走信号
            // （status.code()=None），出口规则不触发。
            let _ = child.start_kill();
        }
        Err(_) => {}
    }
    result
}

#[cfg(test)]
async fn drain_pi_rpc_output<R, W>(
    stdout: R,
    stdin: &mut W,
    sink: &mut impl FnMut(UnifiedAgentEvent),
    cancel_check: impl Fn() -> bool,
) -> Result<(), String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = BufReader::new(stdout).lines();
    drain_pi_rpc_lines(&mut reader, stdin, sink, cancel_check, None).await
}

async fn drain_pi_rpc_lines<R, W>(
    reader: &mut tokio::io::Lines<BufReader<R>>,
    stdin: &mut W,
    sink: &mut impl FnMut(UnifiedAgentEvent),
    cancel_check: impl Fn() -> bool,
    btw_command: Option<&PiBtwCommand>,
) -> Result<(), String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut agent_ended = false;
    let mut btw_entries_requested = false;
    let mut btw_entry_emitted = false;
    // Pi emits the failed turn before it announces whether an automatic retry will follow.
    // Defer fatal errors until the final agent_end; otherwise a recovered retry still leaves
    // Kivio's turn permanently marked as failed.
    let mut pending_error: Option<String> = None;
    // agent_end 后等待 pi flush + 自行退出的宽限期。带 --session-id 时 pi 收尾要落盘会话，
    // 可能不再因 stdin EOF 立即退出——宽限期一到就主动 break，不再无限等 EOF（否则 UI 转圈不止）。
    let mut ended_at: Option<std::time::Instant> = None;
    const AGENT_END_GRACE: Duration = Duration::from_secs(3);

    loop {
        if cancel_check() {
            return Err("cancelled".to_string());
        }
        if let Some(since) = ended_at {
            if since.elapsed() > AGENT_END_GRACE {
                break;
            }
        }

        let line = match timeout(Duration::from_millis(200), reader.next_line()).await {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => break,
            Ok(Err(e)) => return Err(e.to_string()),
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        // `agent_end` is the logical end of the turn, but Pi still flushes queued RPC output while
        // shutting down after stdin EOF. Keep the stdout reader alive until EOF and ignore any
        // trailing protocol lines so Pi never writes into a pipe Kivio has already dropped.
        if agent_ended {
            continue;
        }

        let value: Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if value.get("type").and_then(Value::as_str) == Some("entry_appended") {
            if let (Some(command), Some(entry)) = (btw_command, value.get("entry")) {
                if let Some(question) = command.question.as_deref() {
                    if let Some((started, completed)) = pi_btw_entry_events(entry, question) {
                        sink(started);
                        sink(completed);
                        btw_entry_emitted = true;
                    }
                }
            }
            continue;
        }

        if value.get("type").and_then(|v| v.as_str()) == Some("extension_ui_request") {
            if let Some(command) = btw_command {
                let method = value
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if method == "notify" {
                    if let Some(message) = value.get("message").and_then(Value::as_str) {
                        if value.get("notifyType").and_then(Value::as_str) == Some("error") {
                            sink(UnifiedAgentEvent::Error {
                                message: message.to_string(),
                            });
                        } else if !message.trim().is_empty() {
                            sink(UnifiedAgentEvent::TextDelta {
                                delta: message.to_string(),
                            });
                        }
                    }
                } else if method == "setStatus" {
                    if let Some(text) = value
                        .get("statusText")
                        .and_then(Value::as_str)
                        .filter(|text| !text.trim().is_empty())
                    {
                        sink(UnifiedAgentEvent::StatusNote {
                            text: text.to_string(),
                        });
                    }
                } else if method == "setWidget" && command.name.starts_with("btw") {
                    if let Some(lines) = value.get("widgetLines").and_then(Value::as_array) {
                        if let Some(last) = lines.iter().rev().find_map(Value::as_str) {
                            sink(UnifiedAgentEvent::StatusNote {
                                text: last.to_string(),
                            });
                        }
                    }
                }
            }
            reply_extension_ui(stdin, &value).await?;
            continue;
        }

        if value.get("type").and_then(|v| v.as_str()) == Some("response") {
            if value.get("id").and_then(Value::as_str) == Some(BTW_ENTRIES_REQUEST_ID) {
                if value.get("success").and_then(Value::as_bool) == Some(true) {
                    if let Some(command) = btw_command {
                        if let Some((started, completed)) =
                            pi_btw_entry_events_from_response(&value, command)
                        {
                            sink(started);
                            sink(completed);
                        }
                    }
                }
                agent_ended = true;
                ended_at = Some(std::time::Instant::now());
                let _ = stdin.shutdown().await;
                continue;
            }
            if value.get("success").and_then(|v| v.as_bool()) == Some(false) {
                let err = value
                    .get("error")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "prompt rejected".to_string());
                return Err(err);
            }
            if !btw_entries_requested
                && value.get("command").and_then(Value::as_str) == Some("prompt")
            {
                if let Some(command) = btw_command {
                    if command.question.is_none() || btw_entry_emitted {
                        agent_ended = true;
                        ended_at = Some(std::time::Instant::now());
                        let _ = stdin.shutdown().await;
                        continue;
                    }
                    let request = json!({
                        "id": BTW_ENTRIES_REQUEST_ID,
                        "type": "get_entries",
                    });
                    let mut request_line =
                        serde_json::to_string(&request).map_err(|error| error.to_string())?;
                    request_line.push('\n');
                    stdin
                        .write_all(request_line.as_bytes())
                        .await
                        .map_err(|error| error.to_string())?;
                    btw_entries_requested = true;
                }
            }
            continue;
        }

        if value.get("type").and_then(|v| v.as_str()) == Some("auto_retry_end")
            && value.get("success").and_then(|v| v.as_bool()) == Some(true)
        {
            pending_error = None;
        }

        let outcome = map_pi_rpc_event(&value, &mut |event| match event {
            UnifiedAgentEvent::Error { message } => {
                if pending_error.is_none() {
                    pending_error = Some(message);
                }
            }
            other => sink(other),
        });
        if outcome == PiRpcOutcome::AgentEnd {
            // AgentSession emits agent_end for each failed attempt. `willRetry: true` means its
            // backoff/continuation state machine is still active, so keep both pipes open.
            if value.get("willRetry").and_then(|v| v.as_bool()) == Some(true) {
                continue;
            }
            if let Some(message) = pending_error.take() {
                sink(UnifiedAgentEvent::Error { message });
            }
            agent_ended = true;
            ended_at = Some(std::time::Instant::now());
            // The process may already be closing its stdin side after emitting agent_end. Shutdown
            // is only the signal to begin Pi's flush-and-exit path, so a concurrent close is safe.
            let _ = stdin.shutdown().await;
        }
    }

    if let Some(message) = pending_error {
        sink(UnifiedAgentEvent::Error { message });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use super::*;
    use tokio::io::{duplex, sink, AsyncReadExt};

    #[tokio::test]
    #[ignore = "requires live pi CLI on PATH"]
    async fn live_detect_pi_commands() {
        let bin = std::process::Command::new("which")
            .arg("pi")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|p| !p.is_empty())
            .map(std::path::PathBuf::from)
            .expect("pi on PATH");
        let cmds = detect_pi_commands(&bin, &["--mode", "rpc"], &std::env::temp_dir(), 10)
            .await
            .expect("pi get_commands");
        eprintln!("pi commands: {}", cmds.len());
        for c in cmds.iter().take(8) {
            eprintln!("  {}", c.slash);
        }
        assert!(!cmds.is_empty());
    }

    /// Live proof that L5 counts pi's cache tokens.
    ///
    /// 单测证明「给定这样的 JSON 会算上 cacheRead」；这条证明真实 pi 确实**发** cacheRead，
    /// 且它计入了 total。实测 pi 的 cacheRead 可占 input 的 62% —— 旧代码只读 input/output，
    /// 这部分被整段丢弃。
    #[tokio::test]
    #[ignore = "requires live pi CLI on PATH + login"]
    async fn pi_usage_counts_cache_tokens() {
        use tokio::time::{timeout, Duration};

        let cwd = std::env::temp_dir();
        let mut child = crate::external_agents::spawn::cli_command("pi")
            .args(["--mode", "rpc"])
            .current_dir(&cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn pi --mode rpc");

        let events = std::cell::RefCell::new(Vec::<UnifiedAgentEvent>::new());
        let result = timeout(
            Duration::from_secs(120),
            run_pi_rpc_session(
                &mut child,
                "Reply with exactly the token USAGE_OK and nothing else.",
                None,
                |event| events.borrow_mut().push(event),
                || false,
            ),
        )
        .await;
        let _ = child.start_kill();
        assert!(result.is_ok(), "pi rpc session HUNG past 120s guard");

        let usages: Vec<crate::chat::model::ModelUsage> = events
            .into_inner()
            .into_iter()
            .filter_map(|e| match e {
                UnifiedAgentEvent::Usage { usage } => Some(usage),
                _ => None,
            })
            .collect();
        for u in &usages {
            eprintln!(
                "pi usage: input={:?} output={:?} cache_read={:?} cache_write={:?} total={:?}",
                u.input_tokens,
                u.output_tokens,
                u.cached_input_tokens,
                u.cache_creation_input_tokens,
                u.total_tokens
            );
        }
        assert!(
            !usages.is_empty(),
            "pi reported no usage — turn_end usage parsing regressed"
        );
        // total 必须 >= input+output；有 cache 时必须严格大于（否则就是旧的丢弃口径）。
        for u in &usages {
            let plain = u.input_tokens.unwrap_or(0) + u.output_tokens.unwrap_or(0);
            let cache =
                u.cached_input_tokens.unwrap_or(0) + u.cache_creation_input_tokens.unwrap_or(0);
            assert!(
                u.total_tokens.unwrap_or(0) >= plain,
                "total must not be below input+output: {u:?}"
            );
            if cache > 0 {
                assert!(
                    u.total_tokens.unwrap_or(0) > plain,
                    "cache tokens were reported but not counted into total: {u:?}"
                );
            }
        }
    }

    #[test]
    fn parse_pi_models_from_tsv() {
        let stderr = "provider model context\nanthropic claude-sonnet-4-5 200K\nopenai gpt-5 128K";
        let models = parse_pi_models(stderr).unwrap();
        assert!(models.iter().any(|m| m.id == "anthropic/claude-sonnet-4-5"));
        assert!(models.iter().any(|m| m.id == "openai/gpt-5"));
        let claude = models
            .iter()
            .find(|m| m.id == "anthropic/claude-sonnet-4-5")
            .unwrap();
        assert_eq!(claude.context_window_tokens, Some(200_000));
    }

    #[test]
    fn map_pi_text_delta() {
        let raw = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"hi"}}"#;
        let value: Value = serde_json::from_str(raw).unwrap();
        let mut events = Vec::new();
        map_pi_rpc_event(&value, &mut |e| events.push(e));
        assert!(matches!(
            events.first(),
            Some(UnifiedAgentEvent::TextDelta { delta }) if delta == "hi"
        ));
    }

    #[test]
    fn map_pi_agent_end() {
        let raw = r#"{"type":"agent_end"}"#;
        let value: Value = serde_json::from_str(raw).unwrap();
        assert_eq!(
            map_pi_rpc_event(&value, &mut |_| {}),
            PiRpcOutcome::AgentEnd
        );
    }

    #[test]
    fn parses_only_supported_btw_extension_commands() {
        assert_eq!(
            parse_pi_btw_command("/btw --save  为什么失败？"),
            Some(PiBtwCommand {
                name: "btw".to_string(),
                question: Some("为什么失败？".to_string()),
            })
        );
        assert_eq!(
            parse_pi_btw_command("/btw:tangent -s compare A and B"),
            Some(PiBtwCommand {
                name: "btw:tangent".to_string(),
                question: Some("compare A and B".to_string()),
            })
        );
        assert_eq!(
            parse_pi_btw_command("/btw:clear"),
            Some(PiBtwCommand {
                name: "btw:clear".to_string(),
                question: None,
            })
        );
        assert_eq!(parse_pi_btw_command("/btw-unknown question"), None);
        assert_eq!(parse_pi_btw_command("explain /btw"), None);
    }

    #[test]
    fn command_probe_requires_the_registered_extension_source() {
        let registered = json!({
            "id": BTW_COMMAND_PROBE_ID,
            "type": "response",
            "command": "get_commands",
            "success": true,
            "data": { "commands": [
                { "name": "btw", "source": "extension" },
                { "name": "skill:btw", "source": "skill" }
            ] }
        });
        assert!(response_registers_command(&registered, "btw"));
        assert!(!response_registers_command(&registered, "btw:tangent"));

        let prompt_template = json!({
            "id": BTW_COMMAND_PROBE_ID,
            "type": "response",
            "success": true,
            "data": { "commands": [{ "name": "btw", "source": "prompt" }] }
        });
        assert!(!response_registers_command(&prompt_template, "btw"));
    }

    #[test]
    fn maps_completed_btw_entry_to_the_existing_subagent_tool_shape() {
        let response = json!({
            "id": BTW_ENTRIES_REQUEST_ID,
            "type": "response",
            "command": "get_entries",
            "success": true,
            "data": { "entries": [
                {
                    "type": "custom",
                    "id": "entry-7",
                    "customType": BTW_ENTRY_TYPE,
                    "data": {
                        "question": "compare A and B",
                        "answer": "A is safer; B is faster.",
                        "provider": "openai",
                        "model": "gpt-5-mini",
                        "thinkingLevel": "low",
                        "usage": {
                            "input": 100,
                            "output": 20,
                            "cacheRead": 30,
                            "cacheWrite": 5,
                            "totalTokens": 155
                        }
                    }
                }
            ] }
        });
        let command = PiBtwCommand {
            name: "btw".to_string(),
            question: Some("compare   A and B".to_string()),
        };
        let (started, completed) =
            pi_btw_entry_events_from_response(&response, &command).expect("matching BTW entry");

        match started {
            UnifiedAgentEvent::ToolUse { id, name, input } => {
                assert_eq!(id, "pi_btw_entry-7");
                assert_eq!(name, "Agent");
                assert_eq!(input["type"], "subagent");
                assert_eq!(input["agentType"], "btw");
                assert_eq!(input["prompt"], "compare A and B");
                assert_eq!(input["result"], "A is safer; B is faster.");
                assert_eq!(input["model"], "openai/gpt-5-mini");
                assert_eq!(input["usage"]["inputTokens"], 135);
                assert_eq!(input["usage"]["totalTokens"], 155);
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
        assert!(matches!(
            completed,
            UnifiedAgentEvent::ToolResult {
                tool_use_id,
                content,
                is_error: false,
            } if tool_use_id == "pi_btw_entry-7" && content == "A is safer; B is faster."
        ));
    }

    #[tokio::test]
    async fn btw_command_completion_fetches_entries_and_finishes_without_agent_end() {
        let (stdout_reader, mut stdout_writer) = duplex(8192);
        let writer = tokio::spawn(async move {
            for line in [
                r#"{"id":1,"type":"response","command":"prompt","success":true}"#,
                r#"{"id":"kivio-btw-entries","type":"response","command":"get_entries","success":true,"data":{"entries":[{"type":"custom","id":"e9","customType":"btw-thread-entry","data":{"question":"side question","answer":"side answer","provider":"p","model":"m"}}]}}"#,
            ] {
                stdout_writer.write_all(line.as_bytes()).await?;
                stdout_writer.write_all(b"\n").await?;
            }
            stdout_writer.shutdown().await
        });
        let (mut stdin_reader, mut stdin_writer) = duplex(4096);
        let command = PiBtwCommand {
            name: "btw".to_string(),
            question: Some("side question".to_string()),
        };
        let mut events = Vec::new();

        let result = drain_pi_rpc_lines(
            &mut BufReader::new(stdout_reader).lines(),
            &mut stdin_writer,
            &mut |event| events.push(event),
            || false,
            Some(&command),
        )
        .await;

        assert_eq!(result, Ok(()));
        assert!(writer.await.unwrap().is_ok());
        let mut requests = String::new();
        stdin_reader.read_to_string(&mut requests).await.unwrap();
        assert!(requests.contains(r#""type":"get_entries""#));
        assert!(events
            .iter()
            .any(|event| matches!(event, UnifiedAgentEvent::ToolUse { .. })));
        assert!(events
            .iter()
            .any(|event| matches!(event, UnifiedAgentEvent::ToolResult { .. })));
    }

    #[tokio::test]
    async fn btw_entry_event_uses_the_fast_path_without_reading_full_history() {
        let (stdout_reader, mut stdout_writer) = duplex(8192);
        let writer = tokio::spawn(async move {
            for line in [
                r#"{"type":"entry_appended","entry":{"type":"custom","id":"e10","customType":"btw-thread-entry","data":{"question":"quick aside","answer":"quick answer","provider":"p","model":"m"}}}"#,
                r#"{"id":1,"type":"response","command":"prompt","success":true}"#,
            ] {
                stdout_writer.write_all(line.as_bytes()).await?;
                stdout_writer.write_all(b"\n").await?;
            }
            stdout_writer.shutdown().await
        });
        let (mut stdin_reader, mut stdin_writer) = duplex(4096);
        let command = PiBtwCommand {
            name: "btw".to_string(),
            question: Some("quick aside".to_string()),
        };
        let mut events = Vec::new();

        let result = drain_pi_rpc_lines(
            &mut BufReader::new(stdout_reader).lines(),
            &mut stdin_writer,
            &mut |event| events.push(event),
            || false,
            Some(&command),
        )
        .await;

        assert_eq!(result, Ok(()));
        assert!(writer.await.unwrap().is_ok());
        let mut requests = String::new();
        stdin_reader.read_to_string(&mut requests).await.unwrap();
        assert!(!requests.contains(r#""type":"get_entries""#));
        assert!(events.iter().any(|event| matches!(
            event,
            UnifiedAgentEvent::ToolUse { id, .. } if id == "pi_btw_e10"
        )));
    }

    #[test]
    fn map_pi_auto_retry_start_as_status() {
        let value = serde_json::json!({
            "type": "auto_retry_start",
            "attempt": 2,
            "maxAttempts": 3,
            "delayMs": 4000
        });
        let mut events = Vec::new();
        map_pi_rpc_event(&value, &mut |event| events.push(event));
        assert!(matches!(
            events.as_slice(),
            [UnifiedAgentEvent::StatusNote { text }] if text.contains("2/3")
        ));
    }

    fn pi_usage(raw: &str) -> crate::chat::model::ModelUsage {
        let value: Value = serde_json::from_str(raw).unwrap();
        let mut events = Vec::new();
        map_pi_rpc_event(&value, &mut |e| events.push(e));
        events
            .into_iter()
            .find_map(|e| match e {
                UnifiedAgentEvent::Usage { usage } => Some(usage),
                _ => None,
            })
            .expect("turn_end 应产出 Usage")
    }

    #[test]
    fn turn_end_usage_counts_cache_and_skips_reasoning() {
        // 本机实测的真实数字：6571 + 1578 + 4096 = 12245 = pi 自报的 totalTokens。
        let usage = pi_usage(
            r#"{"type":"turn_end","message":{"usage":{"input":6571,"output":1578,
                "cacheRead":4096,"cacheWrite":0,"reasoning":26,"totalTokens":12245}}}"#,
        );
        assert_eq!(usage.input_tokens, Some(6571));
        assert_eq!(usage.output_tokens, Some(1578));
        assert_eq!(usage.cached_input_tokens, Some(4096));
        // 漏掉 cacheRead 只会得到 8149（低估 33%，cacheRead 占 input 侧的 62%）；
        // 若再把 reasoning 加进去会变成 12271（重复计数）。
        assert_eq!(usage.total_tokens, Some(12_245));
    }

    #[test]
    fn turn_end_usage_counts_cache_write() {
        let usage = pi_usage(
            r#"{"type":"turn_end","message":{"usage":{"input":10,"output":5,
                "cacheRead":0,"cacheWrite":2048}}}"#,
        );
        assert_eq!(usage.cache_creation_input_tokens, Some(2048));
        assert_eq!(usage.total_tokens, Some(2063));
    }

    #[test]
    fn turn_end_usage_emits_when_only_cache_is_nonzero() {
        // 全缓存命中的一轮：input/output 都是 0 但上下文实占 4096，不能静默丢弃。
        let usage = pi_usage(
            r#"{"type":"turn_end","message":{"usage":{"input":0,"output":0,"cacheRead":4096}}}"#,
        );
        assert_eq!(usage.total_tokens, Some(4096));
    }

    #[tokio::test]
    async fn drains_stdout_after_agent_end_until_writer_closes() {
        let (stdout_reader, mut stdout_writer) = duplex(1024);
        let writer = tokio::spawn(async move {
            stdout_writer
                .write_all(b"{\"type\":\"agent_end\"}\n")
                .await?;
            tokio::time::sleep(Duration::from_millis(20)).await;
            stdout_writer
                .write_all(b"{\"type\":\"response\",\"command\":\"prompt\",\"success\":true}\n")
                .await?;
            stdout_writer.shutdown().await
        });
        let mut stdin = sink();
        let mut events = Vec::new();

        let result = drain_pi_rpc_output(
            stdout_reader,
            &mut stdin,
            &mut |event| events.push(event),
            || false,
        )
        .await;

        assert_eq!(result, Ok(()));
        assert!(
            writer.await.unwrap().is_ok(),
            "trailing write must not hit EPIPE"
        );
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn keeps_pi_open_for_auto_retry_and_discards_recovered_error() {
        let (stdout_reader, mut stdout_writer) = duplex(4096);
        let writer = tokio::spawn(async move {
            for line in [
                r#"{"type":"turn_end","message":{"stopReason":"error","errorMessage":"network error"}}"#,
                r#"{"type":"agent_end","willRetry":true}"#,
                r#"{"type":"auto_retry_start","attempt":1,"maxAttempts":3,"delayMs":1}"#,
                r#"{"type":"auto_retry_end","success":true,"attempt":1}"#,
                r#"{"type":"turn_end","message":{"stopReason":"end_turn"}}"#,
                r#"{"type":"agent_end","willRetry":false}"#,
            ] {
                stdout_writer.write_all(line.as_bytes()).await?;
                stdout_writer.write_all(b"\n").await?;
            }
            stdout_writer.shutdown().await
        });
        let mut stdin = sink();
        let mut events = Vec::new();

        let result = drain_pi_rpc_output(
            stdout_reader,
            &mut stdin,
            &mut |event| events.push(event),
            || false,
        )
        .await;

        assert_eq!(result, Ok(()));
        assert!(writer.await.unwrap().is_ok());
        assert!(events
            .iter()
            .any(|event| matches!(event, UnifiedAgentEvent::StatusNote { .. })));
        assert!(!events
            .iter()
            .any(|event| matches!(event, UnifiedAgentEvent::Error { .. })));
    }

    #[tokio::test]
    async fn emits_deferred_pi_error_on_final_agent_end() {
        let (stdout_reader, mut stdout_writer) = duplex(2048);
        let writer = tokio::spawn(async move {
            stdout_writer
                .write_all(
                    b"{\"type\":\"turn_end\",\"message\":{\"stopReason\":\"error\",\"errorMessage\":\"stream_read_error\"}}\n",
                )
                .await?;
            stdout_writer
                .write_all(b"{\"type\":\"agent_end\",\"willRetry\":false}\n")
                .await?;
            stdout_writer.shutdown().await
        });
        let mut stdin = sink();
        let mut events = Vec::new();

        let result = drain_pi_rpc_output(
            stdout_reader,
            &mut stdin,
            &mut |event| events.push(event),
            || false,
        )
        .await;

        assert_eq!(result, Ok(()));
        assert!(writer.await.unwrap().is_ok());
        assert!(events.iter().any(|event| matches!(
            event,
            UnifiedAgentEvent::Error { message } if message == "stream_read_error"
        )));
    }

    #[tokio::test]
    async fn cancellation_still_interrupts_post_agent_end_drain() {
        let (stdout_reader, mut stdout_writer) = duplex(1024);
        stdout_writer
            .write_all(b"{\"type\":\"agent_end\"}\n")
            .await
            .unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_signal = Arc::clone(&cancelled);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancel_signal.store(true, Ordering::SeqCst);
        });
        let mut stdin = sink();

        let result = drain_pi_rpc_output(stdout_reader, &mut stdin, &mut |_| {}, || {
            cancelled.load(Ordering::SeqCst)
        })
        .await;

        assert_eq!(result, Err("cancelled".to_string()));
        drop(stdout_writer);
    }

    #[test]
    fn parse_pi_models_real_aligned_table() {
        // Real `pi --list-models` output: header + 6 space-aligned columns.
        let out = "provider          model          context  max-out  thinking  images\n\
                   zmfooogreencloud  mimo-v2.5-pro  128K     8.2K     no        no\n\
                   zmfooogreencloud  minimax-m2.7   128K     8.2K     no        no";
        let models = parse_pi_models(out).unwrap();
        assert!(models
            .iter()
            .any(|m| m.id == "zmfooogreencloud/mimo-v2.5-pro"));
        assert!(models
            .iter()
            .any(|m| m.id == "zmfooogreencloud/minimax-m2.7"));
        // Generic provider models must NOT appear (those were the bogus fallback).
        assert!(!models.iter().any(|m| m.id.starts_with("anthropic/")));
    }
}
