use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::timeout;

use crate::external_agents::context::parse_context_window_label;
use crate::external_agents::session::live::{
    MessageInjectionKind, PiSessionRequest, PiSessionRpcResult,
};
use crate::external_agents::stream::{usage_from_parts, CliUsageParts};
use crate::external_agents::types::{
    default_model_option, ExternalCliSlashCommand, RuntimeModelOption, UnifiedAgentEvent,
};
use crate::proc::NoConsoleWindow;

type SharedPiWriter<W> = Arc<Mutex<W>>;
struct PiControlWaiter {
    completion: oneshot::Sender<Result<(), String>>,
    injection: Option<(MessageInjectionKind, String, String)>,
}

type PiControlWaiters = Arc<Mutex<HashMap<String, PiControlWaiter>>>;

fn pi_rpc_images(images: &[crate::external_agents::attachments::ImageBlock]) -> Vec<Value> {
    images
        .iter()
        .map(|image| {
            json!({
                "type": "image",
                "data": image.data_base64,
                "mimeType": image.mime,
            })
        })
        .collect()
}

async fn write_rpc_value<W>(stdin: &SharedPiWriter<W>, payload: &Value) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
{
    let mut line = serde_json::to_string(payload).map_err(|error| error.to_string())?;
    line.push('\n');
    stdin
        .lock()
        .await
        .write_all(line.as_bytes())
        .await
        .map_err(|error| error.to_string())
}

async fn shutdown_rpc_writer<W>(stdin: &SharedPiWriter<W>)
where
    W: AsyncWrite + Unpin,
{
    let _ = stdin.lock().await.shutdown().await;
}

async fn issue_control_command<W>(
    stdin: &SharedPiWriter<W>,
    waiters: &PiControlWaiters,
    id: String,
    injection: Option<(MessageInjectionKind, String, String)>,
    mut payload: Value,
) -> Result<oneshot::Receiver<Result<(), String>>, String>
where
    W: AsyncWrite + Unpin,
{
    payload["id"] = Value::String(id.clone());
    let (tx, rx) = oneshot::channel();
    waiters.lock().await.insert(
        id.clone(),
        PiControlWaiter {
            completion: tx,
            injection,
        },
    );
    if let Err(error) = write_rpc_value(stdin, &payload).await {
        waiters.lock().await.remove(&id);
        return Err(error);
    }
    Ok(rx)
}

fn pi_session_request_payload(request: &PiSessionRequest) -> Value {
    match request {
        PiSessionRequest::GetTree => json!({ "type": "get_tree" }),
        PiSessionRequest::GetEntries { since } => {
            let mut payload = json!({ "type": "get_entries" });
            if let Some(since) = since
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                payload["since"] = Value::String(since.to_string());
            }
            payload
        }
        PiSessionRequest::GetForkMessages => json!({ "type": "get_fork_messages" }),
        PiSessionRequest::Fork { entry_id } => {
            json!({ "type": "fork", "entryId": entry_id })
        }
        PiSessionRequest::Clone => json!({ "type": "clone" }),
        PiSessionRequest::Switch { session_path } => {
            json!({ "type": "switch_session", "sessionPath": session_path })
        }
    }
}

async fn run_idle_rpc_request<R, W>(
    reader: &mut tokio::io::Lines<BufReader<R>>,
    stdin: &SharedPiWriter<W>,
    id: &str,
    mut payload: Value,
) -> Result<Value, String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    payload["id"] = Value::String(id.to_string());
    write_rpc_value(stdin, &payload).await?;
    loop {
        let raw = reader
            .next_line()
            .await
            .map_err(|error| format!("read Pi RPC response: {error}"))?
            .ok_or_else(|| "Pi RPC exited before session response".to_string())?;
        if raw.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(raw.trim())
            .map_err(|error| format!("parse Pi RPC response: {error}"))?;
        if value.get("type").and_then(Value::as_str) == Some("extension_ui_request") {
            reject_extension_ui(stdin, &value).await?;
            continue;
        }
        if value.get("type").and_then(Value::as_str) != Some("response")
            || value.get("id").and_then(Value::as_str) != Some(id)
        {
            continue;
        }
        if value.get("success").and_then(Value::as_bool) != Some(true) {
            return Err(value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Pi session command failed")
                .to_string());
        }
        return Ok(value.get("data").cloned().unwrap_or(Value::Null));
    }
}

async fn run_idle_pi_session_request<R, W>(
    reader: &mut tokio::io::Lines<BufReader<R>>,
    stdin: &SharedPiWriter<W>,
    request_id: &str,
    request: &PiSessionRequest,
) -> Result<PiSessionRpcResult, String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let data = run_idle_rpc_request(
        reader,
        stdin,
        request_id,
        pi_session_request_payload(request),
    )
    .await?;
    let changed =
        request.changes_session() && data.get("cancelled").and_then(Value::as_bool) != Some(true);
    let needs_state = changed || matches!(request, PiSessionRequest::GetTree);
    let state = if needs_state {
        Some(
            run_idle_rpc_request(
                reader,
                stdin,
                &format!("{request_id}-state"),
                json!({ "type": "get_state" }),
            )
            .await?,
        )
    } else {
        None
    };
    Ok(PiSessionRpcResult { data, state })
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiRpcOutcome {
    Continue,
    AgentEnd,
    AgentSettled,
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
            // pi 内建命令不经 get_commands 上报（rpc.md 明说 built-in TUI commands 不含
            // 在内，也无法探测），与 codex 的 CODEX_BUILTIN_COMMANDS 同策略：只列 Kivio
            // 适配层真正会执行的那几个，先播种再合并动态结果（同名以内建为准）。
            for (name, description) in PI_BUILTIN_COMMANDS {
                seen.insert((*name).to_string());
                out.push(ExternalCliSlashCommand {
                    slash: format!("/{name}"),
                    name: (*name).to_string(),
                    description: Some((*description).to_string()),
                    argument_hint: None,
                });
            }
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

/// pi 内建命令白名单（get_commands 不上报内建，rpc.md：built-in TUI commands 走
/// prompt 不会执行）。只列 Kivio 适配层拦截并转成真 RPC 的命令——列而不适配等于
/// 骗用户。`/compact` → `{"type":"compact"}`（见 `run_pi_rpc_session`）。
const PI_BUILTIN_COMMANDS: &[(&str, &str)] = &[("compact", "压缩对话历史")];

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

fn flatten_pi_tool_content(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(pi_tool_content_block_text)
            .collect::<Vec<_>>()
            .join("\n"),
        other => other
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| other.to_string()),
    }
}

fn pi_tool_content_block_text(block: &Value) -> Option<String> {
    if let Some(text) = block.as_str() {
        return Some(text.to_string());
    }
    match block.get("type").and_then(Value::as_str).unwrap_or("text") {
        "image" => {
            let mime = block
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("image");
            Some(format!("[image: {mime}]"))
        }
        _ => block
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

async fn fail_pending_controls(waiters: &PiControlWaiters, error: &str) {
    let leftover: Vec<PiControlWaiter> = waiters
        .lock()
        .await
        .drain()
        .map(|(_, waiter)| waiter)
        .collect();
    for waiter in leftover {
        let _ = waiter.completion.send(Err(error.to_string()));
    }
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
            // 下拉框标签用「模型 · 供应商」：供应商 id 现在已是短名（relay），
            // 比 `kivio-p-xxx/model` 整串塞进一行可读得多；id 仍保留完整 provider/model。
            out.push(RuntimeModelOption {
                id: full_id,
                label: format!("{} · {}", parts[1], parts[0]),
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

/// 从 `pi --list-models` 表格解析每个模型的 thinking 支持（`thinking` 列 yes/no）。
/// 按表头定位列号（列序可能随版本变化）；无表头/无该列 → 空表（调用方视为未知，
/// 不隐藏任何档位）。id 形态与 `parse_pi_models` 一致（`provider/model`）。
pub fn parse_pi_model_thinking(text: &str) -> std::collections::HashMap<String, bool> {
    let mut out = std::collections::HashMap::new();
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    let Some(header) = lines.first() else {
        return out;
    };
    let Some(col) = header
        .split_whitespace()
        .position(|c| c.eq_ignore_ascii_case("thinking"))
    else {
        return out;
    };
    for line in lines.iter().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let Some(flag) = parts.get(col) else {
            continue;
        };
        out.insert(
            format!("{}/{}", parts[0], parts[1]),
            flag.eq_ignore_ascii_case("yes"),
        );
    }
    out
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
        "agent_settled" => return PiRpcOutcome::AgentSettled,
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
            let content = obj
                .get("result")
                .and_then(|result| result.get("content"))
                .map(flatten_pi_tool_content)
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
        // start 只发状态行；CliCompacted 必须等 end，否则会插一条假分隔线。
        "compaction_start" => {
            sink(UnifiedAgentEvent::StatusNote {
                text: "Pi 正在压缩上下文…".to_string(),
            });
        }
        "compaction_end" => {
            if let Some(result) = obj.get("result").and_then(|v| v.as_object()) {
                let reason = obj.get("reason").and_then(|v| v.as_str()).unwrap_or("");
                sink(UnifiedAgentEvent::CliCompacted {
                    trigger: if reason == "manual" {
                        "manual".to_string()
                    } else {
                        "auto".to_string()
                    },
                    pre_tokens: result.get("tokensBefore").and_then(|v| v.as_u64()),
                    post_tokens: result.get("estimatedTokensAfter").and_then(|v| v.as_u64()),
                    dropped_tokens: None,
                    duration_ms: None,
                });
            } else if let Some(err) = obj
                .get("errorMessage")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
            {
                // 压缩失败不终止轮次（overflow 场景 pi 自己决定重试与否），只报状态行。
                sink(UnifiedAgentEvent::StatusNote {
                    text: format!("上下文压缩失败：{err}"),
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

fn extension_ui_tool_name(method: &str) -> Option<&'static str> {
    match method {
        "confirm" => Some("PiExtensionConfirm"),
        "select" => Some("PiExtensionSelect"),
        "input" => Some("PiExtensionInput"),
        "editor" => Some("PiExtensionEditor"),
        _ => None,
    }
}

async fn write_extension_ui_response<W>(
    stdin: &SharedPiWriter<W>,
    id: Value,
    result: Value,
) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
{
    let mut payload = json!({ "type": "extension_ui_response", "id": id });
    if let (Some(payload), Some(result)) = (payload.as_object_mut(), result.as_object()) {
        for (key, value) in result {
            payload.insert(key.clone(), value.clone());
        }
    }
    write_rpc_value(stdin, &payload).await
}

async fn reject_extension_ui<W>(stdin: &SharedPiWriter<W>, raw: &Value) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
{
    let Some(id) = raw.get("id").cloned() else {
        return Ok(());
    };
    if raw
        .get("method")
        .and_then(Value::as_str)
        .is_some_and(|method| FIRE_AND_FORGET.contains(&method))
    {
        return Ok(());
    }
    write_extension_ui_response(stdin, id, json!({ "cancelled": true })).await
}

async fn bridge_extension_ui<W>(
    stdin: &SharedPiWriter<W>,
    raw: &Value,
    approvals: Option<&mut crate::external_agents::session::live::ApprovalBridge>,
    cancel_check: &impl Fn() -> bool,
) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
{
    let Some(id) = raw.get("id").cloned() else {
        return Ok(());
    };
    let Some(request_id) = id.as_str().map(str::to_string) else {
        return reject_extension_ui(stdin, raw).await;
    };
    let Some(method) = raw.get("method").and_then(Value::as_str) else {
        return reject_extension_ui(stdin, raw).await;
    };
    let Some(tool_name) = extension_ui_tool_name(method) else {
        return reject_extension_ui(stdin, raw).await;
    };
    let Some(bridge) = approvals else {
        return reject_extension_ui(stdin, raw).await;
    };

    let ask = crate::external_agents::session::live::ApprovalAsk {
        request_id: request_id.clone(),
        tool_call_id: format!("pi-extension-ui-{request_id}"),
        tool_name: tool_name.to_string(),
        input: raw.clone(),
        requires_user_interaction: true,
    };
    if bridge.requests.send(ask).await.is_err() {
        return reject_extension_ui(stdin, raw).await;
    }

    let requested_timeout = raw
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or(300_000);
    let deadline = tokio::time::Instant::now()
        + Duration::from_millis(requested_timeout.clamp(1_000, 300_000));
    let result = loop {
        if cancel_check() || tokio::time::Instant::now() >= deadline {
            break json!({ "cancelled": true });
        }
        match timeout(Duration::from_millis(200), bridge.decisions.recv()).await {
            Ok(Some(decision)) if decision.request_id == request_id => {
                break if decision.approved {
                    decision
                        .updated_input
                        .filter(Value::is_object)
                        .unwrap_or_else(|| json!({ "cancelled": true }))
                } else {
                    json!({ "cancelled": true })
                };
            }
            Ok(Some(_)) | Err(_) => continue,
            Ok(None) => break json!({ "cancelled": true }),
        }
    };
    write_extension_ui_response(stdin, id, result).await
}

async fn run_pi_rpc_io<R, W>(
    reader: &mut tokio::io::Lines<BufReader<R>>,
    stdin: &SharedPiWriter<W>,
    prompt: &str,
    images: &[crate::external_agents::attachments::ImageBlock],
    sink: &mut impl FnMut(UnifiedAgentEvent),
    approvals: Option<&mut crate::external_agents::session::live::ApprovalBridge>,
    pending_controls: Option<&PiControlWaiters>,
    cancel_check: impl Fn() -> bool,
    persistent: bool,
) -> Result<(), String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // 内建 /compact 必须发 type:compact，走 prompt 不会执行。
    let manual_compact = prompt.trim() == "/compact";

    // pi-btw owns its sub-session and persists completed exchanges as custom session entries.
    // Official RPC says extension commands go out as an ordinary `prompt`; enable the card
    // adapter from the slash shape and only paint a card if a `btw-thread-entry` actually
    // arrives. Do not probe `get_commands` first — that stalls the prompt up to 3s and can
    // swallow leftover stdout from the resident session.
    let btw_command = if manual_compact {
        None
    } else {
        parse_pi_btw_command(prompt)
    };

    let payload = if manual_compact {
        json!({ "id": 1, "type": "compact" })
    } else {
        let rpc_images = pi_rpc_images(images);
        let mut payload = json!({ "id": 1, "type": "prompt", "message": prompt });
        if !rpc_images.is_empty() {
            payload["images"] = Value::Array(rpc_images);
        }
        payload
    };
    write_rpc_value(stdin, &payload).await?;

    drain_pi_rpc_lines(
        reader,
        stdin,
        sink,
        approvals,
        pending_controls,
        cancel_check,
        btw_command.as_ref(),
        manual_compact,
        persistent,
    )
    .await
}

pub async fn run_pi_rpc_session(
    child: &mut Child,
    prompt: &str,
    _model: Option<&str>,
    mut sink: impl FnMut(UnifiedAgentEvent),
    cancel_check: impl Fn() -> bool,
) -> Result<(), String> {
    let stdin = Arc::new(Mutex::new(
        child
            .stdin
            .take()
            .ok_or_else(|| "stdin unavailable".to_string())?,
    ));
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "stdout unavailable".to_string())?;
    let mut reader = BufReader::new(stdout).lines();
    let result = run_pi_rpc_io(
        &mut reader,
        &stdin,
        prompt,
        &[],
        &mut sink,
        None,
        None,
        cancel_check,
        false,
    )
    .await;
    if result.is_ok() || matches!(result.as_ref(), Err(err) if err == "cancelled") {
        let _ = child.start_kill();
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
    let stdin = Arc::new(Mutex::new(stdin));
    drain_pi_rpc_lines(
        &mut reader,
        &stdin,
        sink,
        None,
        None,
        cancel_check,
        None,
        false,
        false,
    )
    .await
}

async fn drain_pi_rpc_lines<R, W>(
    reader: &mut tokio::io::Lines<BufReader<R>>,
    stdin: &SharedPiWriter<W>,
    sink: &mut impl FnMut(UnifiedAgentEvent),
    mut approvals: Option<&mut crate::external_agents::session::live::ApprovalBridge>,
    pending_controls: Option<&PiControlWaiters>,
    cancel_check: impl Fn() -> bool,
    btw_command: Option<&PiBtwCommand>,
    // 本次发出的是 `{"type":"compact"}` 而非 prompt：compact RPC 不产生 agent_end，
    // 以它的 response 作为轮次终点。
    manual_compact: bool,
    // Persistent actors keep stdin/stdout open across turns and finish on `agent_settled`.
    persistent: bool,
) -> Result<(), String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut agent_ended = false;
    let mut btw_entries_requested = false;
    let mut btw_entry_emitted = false;
    // 本轮是否已从 compaction_end 事件发出 CliCompacted（事件是首选的唯一来源）。
    // 手动 compact 时若 response 先于/代替事件到达，由 response 兜底合成，靠这个
    // 标志保证两个来源不重复发（重复 = 前端两条分隔线 + 边界计数翻倍）。
    let mut compaction_emitted = false;
    // Pi emits the failed turn before it announces whether an automatic retry will follow.
    // Defer fatal errors until the final agent_end; otherwise a recovered retry still leaves
    // Kivio's turn permanently marked as failed.
    let mut pending_error: Option<String> = None;
    // Special RPC commands can finish on their response while Pi still flushes a late
    // `agent_settled`. A reused reader must not let that stale boundary finish the next prompt.
    let mut current_turn_seen = manual_compact;
    // agent_end 后等待 pi flush + 自行退出的宽限期。带 --session-id 时 pi 收尾要落盘会话，
    // 可能不再因 stdin EOF 立即退出——宽限期一到就主动 break，不再无限等 EOF（否则 UI 转圈不止）。
    let mut ended_at: Option<std::time::Instant> = None;
    const AGENT_END_GRACE: Duration = Duration::from_secs(3);

    loop {
        if cancel_check() && !persistent {
            return Err("cancelled".to_string());
        }
        if !persistent {
            if let Some(since) = ended_at {
                if since.elapsed() > AGENT_END_GRACE {
                    break;
                }
            }
        }

        let line = match timeout(Duration::from_millis(200), reader.next_line()).await {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) if persistent => {
                return Err("Pi RPC process exited before agent_settled".to_string());
            }
            Ok(Ok(None)) => break,
            Ok(Err(e)) => return Err(e.to_string()),
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        // One-shot callers stop at `agent_end` and only keep draining Pi's shutdown tail. A
        // persistent actor must continue until `agent_settled`, because retry/compaction/queued
        // continuations may legally follow the low-level end event.
        if agent_ended && !persistent {
            continue;
        }

        let value: Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if !current_turn_seen {
            let kind = value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let response_is_pending_control = if kind == "response" {
                match (pending_controls, value.get("id").and_then(Value::as_str)) {
                    (Some(waiters), Some(id)) => waiters.lock().await.contains_key(id),
                    _ => false,
                }
            } else {
                false
            };
            current_turn_seen = matches!(kind, "agent_start" | "turn_start" | "message_start")
                || response_is_pending_control
                || (kind == "response"
                    && value.get("command").and_then(Value::as_str) == Some("prompt")
                    && value.get("id").and_then(Value::as_i64) == Some(1));
        }
        if persistent && agent_ended {
            let kind = value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if matches!(
                kind,
                "agent_start"
                    | "turn_start"
                    | "message_start"
                    | "auto_retry_start"
                    | "compaction_start"
                    | "auto_compaction_start"
            ) {
                agent_ended = false;
                ended_at = None;
            }
        }
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
            let method = value
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if method == "notify" {
                if let Some(message) = value
                    .get("message")
                    .and_then(Value::as_str)
                    .filter(|message| !message.trim().is_empty())
                {
                    let level = value
                        .get("notifyType")
                        .and_then(Value::as_str)
                        .filter(|level| *level != "info")
                        .map(|level| format!("[{level}] "))
                        .unwrap_or_default();
                    sink(UnifiedAgentEvent::StatusNote {
                        text: format!("{level}{message}"),
                    });
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
            } else if method == "setWidget" {
                if let Some(last) = value
                    .get("widgetLines")
                    .and_then(Value::as_array)
                    .and_then(|lines| lines.iter().rev().find_map(Value::as_str))
                {
                    sink(UnifiedAgentEvent::StatusNote {
                        text: last.to_string(),
                    });
                }
            }
            bridge_extension_ui(stdin, &value, approvals.as_deref_mut(), &cancel_check).await?;
            continue;
        }

        if value.get("type").and_then(|v| v.as_str()) == Some("response") {
            if let (Some(waiters), Some(id)) =
                (pending_controls, value.get("id").and_then(Value::as_str))
            {
                if let Some(waiter) = waiters.lock().await.remove(id) {
                    let succeeded = value.get("success").and_then(Value::as_bool) == Some(true);
                    if succeeded {
                        if let Some((kind, id, text)) = waiter.injection {
                            match kind {
                                MessageInjectionKind::Steer => {
                                    sink(UnifiedAgentEvent::UserSteer { id, text });
                                }
                                MessageInjectionKind::FollowUp => {
                                    sink(UnifiedAgentEvent::UserFollowUp { id, text });
                                }
                            }
                        }
                    }
                    let result = if succeeded {
                        Ok(())
                    } else {
                        Err(value
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("Pi rejected the control command")
                            .to_string())
                    };
                    let _ = waiter.completion.send(result);
                    continue;
                }
            }
            if value
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.starts_with("kivio-control-"))
            {
                continue;
            }
            if value.get("id").and_then(Value::as_str) == Some(BTW_ENTRIES_REQUEST_ID) {
                if !btw_entries_requested {
                    continue;
                }
                let mut painted = false;
                if value.get("success").and_then(Value::as_bool) == Some(true) {
                    if let Some(command) = btw_command {
                        if let Some((started, completed)) =
                            pi_btw_entry_events_from_response(&value, command)
                        {
                            sink(started);
                            sink(completed);
                            btw_entry_emitted = true;
                            painted = true;
                        }
                    }
                }
                btw_entries_requested = false;
                if painted {
                    if persistent {
                        break;
                    }
                    agent_ended = true;
                    ended_at = Some(std::time::Instant::now());
                    shutdown_rpc_writer(stdin).await;
                }
                continue;
            }
            if value.get("success").and_then(|v| v.as_bool()) == Some(false) {
                let err = value
                    .get("error")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "prompt rejected".to_string());
                return Err(err);
            }
            // compact RPC 没有 agent_end；事件缺席时从 response 兜底，且只发一条。
            if manual_compact && value.get("command").and_then(Value::as_str) == Some("compact") {
                if !compaction_emitted {
                    let data = value.get("data").and_then(|v| v.as_object());
                    let field = |key: &str| data.and_then(|d| d.get(key)).and_then(|v| v.as_u64());
                    sink(UnifiedAgentEvent::CliCompacted {
                        trigger: "manual".to_string(),
                        pre_tokens: field("tokensBefore"),
                        post_tokens: field("estimatedTokensAfter"),
                        dropped_tokens: None,
                        duration_ms: None,
                    });
                    compaction_emitted = true;
                }
                if persistent {
                    break;
                }
                agent_ended = true;
                ended_at = Some(std::time::Instant::now());
                shutdown_rpc_writer(stdin).await;
                continue;
            }
            if !btw_entries_requested
                && value.get("command").and_then(Value::as_str) == Some("prompt")
            {
                if let Some(command) = btw_command {
                    if command.question.is_some() && !btw_entry_emitted {
                        let request = json!({
                            "id": BTW_ENTRIES_REQUEST_ID,
                            "type": "get_entries",
                        });
                        write_rpc_value(stdin, &request).await?;
                        btw_entries_requested = true;
                    }
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
            event @ UnifiedAgentEvent::CliCompacted { .. } => {
                compaction_emitted = true;
                sink(event);
            }
            other => sink(other),
        });
        match outcome {
            PiRpcOutcome::AgentSettled => {
                if persistent && !current_turn_seen {
                    continue;
                }
                if btw_entries_requested {
                    continue;
                }
                if let Some(message) = pending_error.take() {
                    sink(UnifiedAgentEvent::Error { message });
                }
                break;
            }
            PiRpcOutcome::AgentEnd => {
                // AgentSession emits agent_end for each failed attempt. `willRetry: true` means its
                // backoff/continuation state machine is still active, so keep both pipes open.
                if value.get("willRetry").and_then(|v| v.as_bool()) == Some(true) {
                    continue;
                }
                agent_ended = true;
                ended_at = Some(std::time::Instant::now());
                if !persistent {
                    if let Some(message) = pending_error.take() {
                        sink(UnifiedAgentEvent::Error { message });
                    }
                    // One-shot mode closes stdin so Pi can flush and exit. Persistent mode keeps
                    // both pipes alive and waits for the higher-level `agent_settled` boundary.
                    shutdown_rpc_writer(stdin).await;
                }
            }
            PiRpcOutcome::Continue => {}
        }
    }

    if let Some(message) = pending_error {
        sink(UnifiedAgentEvent::Error { message });
    }

    if persistent && cancel_check() {
        Err("cancelled".to_string())
    } else {
        Ok(())
    }
}

fn persistent_session_args(
    args: &[String],
    resume_native: Option<&str>,
) -> Result<(Vec<String>, String, bool), String> {
    let mut effective_args = args.to_vec();
    let resume_native = resume_native.map(str::trim).filter(|id| !id.is_empty());
    if let Some(id) = resume_native {
        if let Some(index) = effective_args.iter().position(|arg| arg == "--session-id") {
            if index + 1 < effective_args.len() {
                effective_args[index + 1] = id.to_string();
            } else {
                effective_args.push(id.to_string());
            }
        } else {
            effective_args.push("--session-id".to_string());
            effective_args.push(id.to_string());
        }
    }
    let session_id = effective_args
        .windows(2)
        .find(|pair| pair[0] == "--session-id")
        .map(|pair| pair[1].clone())
        .ok_or_else(|| "Pi persistent session requires --session-id".to_string())?;
    Ok((effective_args, session_id, resume_native.is_some()))
}

/// Pi `--session-id` is create-or-resume. Claiming `resumed` from the id alone would send only
/// the latest user message into a blank native session after the JSONL was deleted.
pub fn is_missing_pi_session_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    (lower.contains("pi session")
        || lower.contains("pi rpc")
        || lower.contains("--session-id")
        || lower.contains("native session"))
        && (lower.contains("not found")
            || lower.contains("no such session")
            || lower.contains("unknown session")
            || lower.contains("is gone"))
}

fn pi_native_session_present(session_id: &str) -> bool {
    pi_session_file_candidates(session_id).into_iter().any(|path| path.is_file())
}

fn pi_session_file_candidates(session_id: &str) -> Vec<PathBuf> {
    let Some(home) = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()) else {
        return Vec::new();
    };
    let sessions = home.join(".pi").join("agent").join("sessions");
    vec![
        sessions.join(format!("{session_id}.jsonl")),
        sessions.join(session_id).join("session.jsonl"),
        sessions.join(session_id).join(format!("{session_id}.jsonl")),
    ]
}

/// A live Pi RPC connection. The actor owns the child process while stdin/stdout are shared with
/// the in-flight turn task so control commands can still be received during generation.
pub struct PiRpcSession {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    reader: Arc<Mutex<tokio::io::Lines<BufReader<ChildStdout>>>>,
    session_id: String,
    resumed: bool,
    stderr_tail: Arc<Mutex<String>>,
    _stderr_task: tokio::task::JoinHandle<()>,
}

impl PiRpcSession {
    pub async fn connect(
        bin: &Path,
        args: &[String],
        cwd: &Path,
        resume_native: Option<&str>,
    ) -> Result<Self, String> {
        // The persisted live handle is the authoritative binding for this Kivio conversation.
        // It can exist even when the regular binding was not flushed before a crash.
        let (effective_args, session_id, claimed_resume) =
            persistent_session_args(args, resume_native)?;
        let resumed = claimed_resume && pi_native_session_present(&session_id);

        let mut child = crate::external_agents::spawn::cli_command(bin)
            .args(&effective_args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .no_console_window()
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("start Pi RPC: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Pi RPC stdin unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Pi RPC stdout unavailable".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "Pi RPC stderr unavailable".to_string())?;
        let stderr_tail = Arc::new(Mutex::new(String::new()));
        let stderr_capture = stderr_tail.clone();
        let stderr_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    eprintln!("[external-agent:pi] {line}");
                    let mut tail = stderr_capture.lock().await;
                    if !tail.is_empty() {
                        tail.push('\n');
                    }
                    tail.push_str(&line);
                    if tail.chars().count() > 8000 {
                        *tail = crate::external_agents::spawn::tail_chars(&tail, 8000);
                    }
                }
            }
        });

        Ok(Self {
            child,
            stdin: Arc::new(Mutex::new(stdin)),
            reader: Arc::new(Mutex::new(BufReader::new(stdout).lines())),
            session_id,
            resumed,
            stderr_tail,
            _stderr_task: stderr_task,
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn resumed(&self) -> bool {
        self.resumed
    }

    pub fn child_pid(&self) -> Option<u32> {
        self.child.id()
    }

    async fn close(&mut self) {
        crate::external_agents::spawn::kill_agent_process_tree(&mut self.child);
        let _ = self.child.wait().await;
    }
}

/// Spawn the actor that serializes Pi turns while keeping one RPC process alive across turns.
pub fn spawn_pi_rpc_session_actor(
    mut session: PiRpcSession,
) -> mpsc::Sender<crate::external_agents::session::live::SessionCommand> {
    use crate::external_agents::session::live::SessionCommand;

    let (tx, mut rx) = mpsc::channel::<SessionCommand>(8);
    tokio::spawn(async move {
        let mut next_control_id = 2_u64;
        while let Some(command) = rx.recv().await {
            match command {
                SessionCommand::RunTurn {
                    prompt,
                    images,
                    events,
                    done,
                    approvals,
                    model: _,
                    reasoning: _,
                } => {
                    let cancelled = Arc::new(AtomicBool::new(false));
                    let turn_cancelled = cancelled.clone();
                    let reader = session.reader.clone();
                    let stdin = session.stdin.clone();
                    let stderr_tail = session.stderr_tail.clone();
                    let pending_controls: PiControlWaiters = Arc::new(Mutex::new(HashMap::new()));
                    let turn_pending_controls = pending_controls.clone();
                    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
                    let forward = tokio::spawn(async move {
                        while let Some(event) = event_rx.recv().await {
                            if events.send(event).await.is_err() {
                                break;
                            }
                        }
                    });
                    let mut approvals = approvals;
                    let mut turn = tokio::spawn(async move {
                        let mut reader = reader.lock().await;
                        let result = run_pi_rpc_io(
                            &mut reader,
                            &stdin,
                            &prompt,
                            &images,
                            &mut |event| {
                                let _ = event_tx.send(event);
                            },
                            approvals.as_mut(),
                            Some(&turn_pending_controls),
                            || turn_cancelled.load(Ordering::Acquire),
                            true,
                        )
                        .await;
                        let result = match result {
                            Err(error) if error != "cancelled" => {
                                let tail = stderr_tail.lock().await.clone();
                                if tail.trim().is_empty() {
                                    Err(error)
                                } else {
                                    Err(format!("{error}\n\nPi stderr:\n{tail}"))
                                }
                            }
                            other => other,
                        };
                        drop(event_tx);
                        let _ = forward.await;
                        let _ = done.send(result.clone());
                        result
                    });

                    let mut close_after_turn = false;
                    let turn_result = loop {
                        tokio::select! {
                            joined = &mut turn => {
                                break joined.unwrap_or_else(|error| Err(format!("Pi session actor task failed: {error}")));
                            }
                            next = rx.recv() => {
                                match next {
                                    Some(SessionCommand::Cancel) => {
                                        cancelled.store(true, Ordering::Release);
                                        let request_id = format!("kivio-control-{}", next_control_id);
                                        next_control_id = next_control_id.saturating_add(1);
                                        if issue_control_command(
                                            &session.stdin,
                                            &pending_controls,
                                            request_id,
                                            None,
                                            json!({ "type": "abort" }),
                                        )
                                        .await
                                        .is_err()
                                        {
                                            crate::external_agents::spawn::kill_agent_process_tree(&mut session.child);
                                            close_after_turn = true;
                                        }
                                    }
                                    Some(SessionCommand::Close) | None => {
                                        cancelled.store(true, Ordering::Release);
                                        crate::external_agents::spawn::kill_agent_process_tree(&mut session.child);
                                        close_after_turn = true;
                                    }
                                    Some(SessionCommand::Steer { id, text, images, kind, accepted }) => {
                                        let request_id = format!("kivio-control-{}", next_control_id);
                                        next_control_id = next_control_id.saturating_add(1);
                                        let command_type = match kind {
                                            MessageInjectionKind::Steer => "steer",
                                            MessageInjectionKind::FollowUp => "follow_up",
                                        };
                                        let display_text = if text.trim().is_empty() && !images.is_empty() {
                                            format!("附带图片（{}）", images.len())
                                        } else {
                                            text.clone()
                                        };
                                        let mut payload = json!({ "type": command_type, "message": text });
                                        let rpc_images = pi_rpc_images(&images);
                                        if !rpc_images.is_empty() {
                                            payload["images"] = Value::Array(rpc_images);
                                        }
                                        match issue_control_command(
                                            &session.stdin,
                                            &pending_controls,
                                            request_id,
                                            Some((kind, id, display_text)),
                                            payload,
                                        )
                                        .await
                                        {
                                            Ok(response) => {
                                                // Do not time out and drop the waiter. Drain may be
                                                // blocked on extension UI, so the ack can legally
                                                // arrive later; dropping it would ignore a success
                                                // and let the frontend resend the same text as a
                                                // new prompt. If the turn ends first,
                                                // fail_pending_controls unblocks this wait as false.
                                                tokio::spawn(async move {
                                                    let ok = matches!(response.await, Ok(Ok(())));
                                                    let _ = accepted.send(ok);
                                                });
                                            }
                                            Err(_) => {
                                                let _ = accepted.send(false);
                                            }
                                        }
                                    }
                                    Some(SessionCommand::RunTurn { done, .. }) => {
                                        let _ = done.send(Err("Pi RPC session is busy".to_string()));
                                    }
                                    Some(SessionCommand::PiSession { reply, .. }) => {
                                        let _ = reply.send(Err("Pi session is busy; wait for the current run to finish".to_string()));
                                    }
                                    Some(SessionCommand::StopTask { .. }) => {}
                                }
                            }
                        }
                    };
                    fail_pending_controls(
                        &pending_controls,
                        "Pi turn ended before the control command was acknowledged",
                    )
                    .await;
                    let session_lost = close_after_turn
                        || matches!(turn_result.as_ref(), Err(error) if error != "cancelled");
                    if session_lost {
                        session.close().await;
                        return;
                    }
                }
                SessionCommand::PiSession { request, reply } => {
                    let request_id = format!("kivio-session-{}", next_control_id);
                    next_control_id = next_control_id.saturating_add(1);
                    let result = {
                        let mut reader = session.reader.lock().await;
                        match timeout(
                            Duration::from_secs(15),
                            run_idle_pi_session_request(
                                &mut reader,
                                &session.stdin,
                                &request_id,
                                &request,
                            ),
                        )
                        .await
                        {
                            Ok(result) => result,
                            Err(_) => Err("Pi session command timed out".to_string()),
                        }
                    };
                    if let Ok(result) = &result {
                        if let Some(session_id) = result
                            .state
                            .as_ref()
                            .and_then(|state| state.get("sessionId"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                        {
                            session.session_id = session_id.to_string();
                        }
                    }
                    let _ = reply.send(result);
                }
                SessionCommand::Steer { accepted, .. } => {
                    let _ = accepted.send(false);
                }
                SessionCommand::Cancel => {}
                SessionCommand::StopTask { .. } => {}
                SessionCommand::Close => {
                    session.close().await;
                    return;
                }
            }
        }
        session.close().await;
    });
    tx
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
    fn map_pi_tool_execution_end_flattens_content_blocks() {
        let raw = r#"{"type":"tool_execution_end","toolCallId":"call_1","toolName":"bash","result":{"content":[{"type":"text","text":"total 48\nfile.txt"}]},"isError":false}"#;
        let value: Value = serde_json::from_str(raw).unwrap();
        let mut events = Vec::new();
        map_pi_rpc_event(&value, &mut |e| events.push(e));
        assert!(matches!(
            events.as_slice(),
            [UnifiedAgentEvent::ToolResult {
                tool_use_id,
                content,
                is_error
            }] if tool_use_id == "call_1" && content == "total 48\nfile.txt" && !*is_error
        ));
    }

    #[test]
    fn map_pi_tool_execution_end_keeps_legacy_string_content() {
        let raw = r#"{"type":"tool_execution_end","toolCallId":"call_2","result":{"content":"plain"},"isError":true}"#;
        let value: Value = serde_json::from_str(raw).unwrap();
        let mut events = Vec::new();
        map_pi_rpc_event(&value, &mut |e| events.push(e));
        assert!(matches!(
            events.as_slice(),
            [UnifiedAgentEvent::ToolResult {
                tool_use_id,
                content,
                is_error
            }] if tool_use_id == "call_2" && content == "plain" && *is_error
        ));
    }

    #[test]
    fn flatten_pi_tool_content_joins_text_blocks() {
        assert_eq!(
            flatten_pi_tool_content(&json!([
                {"type": "text", "text": "one"},
                {"type": "text", "text": "two"},
                {"type": "image", "mimeType": "image/png"}
            ])),
            "one\ntwo\n[image: image/png]"
        );
        assert_eq!(
            flatten_pi_tool_content(&json!([{"type": "text", "text": "total 48"}])),
            "total 48"
        );
        assert_ne!(
            flatten_pi_tool_content(&json!([{"type": "text", "text": "total 48"}])),
            json!([{"type": "text", "text": "total 48"}]).to_string()
        );
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
    fn map_pi_compaction_end_emits_cli_compacted_with_tokens() {
        let raw = r#"{"type":"compaction_end","reason":"threshold","result":{"summary":"s","tokensBefore":150000,"estimatedTokensAfter":32000},"aborted":false,"willRetry":false}"#;
        let value: Value = serde_json::from_str(raw).unwrap();
        let mut events = Vec::new();
        assert_eq!(
            map_pi_rpc_event(&value, &mut |e| events.push(e)),
            PiRpcOutcome::Continue
        );
        assert!(matches!(
            events.as_slice(),
            [UnifiedAgentEvent::CliCompacted {
                trigger,
                pre_tokens: Some(150000),
                post_tokens: Some(32000),
                ..
            }] if trigger == "auto"
        ));
    }

    #[test]
    fn map_pi_compaction_end_manual_reason_and_failure() {
        let manual = r#"{"type":"compaction_end","reason":"manual","result":{"tokensBefore":10,"estimatedTokensAfter":5}}"#;
        let value: Value = serde_json::from_str(manual).unwrap();
        let mut events = Vec::new();
        map_pi_rpc_event(&value, &mut |e| events.push(e));
        assert!(matches!(
            events.as_slice(),
            [UnifiedAgentEvent::CliCompacted { trigger, .. }] if trigger == "manual"
        ));

        // 失败（result:null + errorMessage）不发分隔线，只发状态行。
        let failed = r#"{"type":"compaction_end","reason":"overflow","result":null,"aborted":false,"errorMessage":"quota exceeded"}"#;
        let value: Value = serde_json::from_str(failed).unwrap();
        let mut events = Vec::new();
        map_pi_rpc_event(&value, &mut |e| events.push(e));
        assert!(matches!(
            events.as_slice(),
            [UnifiedAgentEvent::StatusNote { text }] if text.contains("quota exceeded")
        ));

        // 中止（result:null、无 errorMessage）什么都不发。
        let aborted = r#"{"type":"compaction_end","reason":"manual","result":null,"aborted":true}"#;
        let value: Value = serde_json::from_str(aborted).unwrap();
        let mut events = Vec::new();
        map_pi_rpc_event(&value, &mut |e| events.push(e));
        assert!(events.is_empty());
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
        let (mut stdin_reader, stdin_writer) = duplex(4096);
        let stdin_writer = Arc::new(Mutex::new(stdin_writer));
        let command = PiBtwCommand {
            name: "btw".to_string(),
            question: Some("side question".to_string()),
        };
        let mut events = Vec::new();

        let result = drain_pi_rpc_lines(
            &mut BufReader::new(stdout_reader).lines(),
            &stdin_writer,
            &mut |event| events.push(event),
            None,
            None,
            || false,
            Some(&command),
            false,
            false,
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
                r#"{"type":"agent_end"}"#,
            ] {
                stdout_writer.write_all(line.as_bytes()).await?;
                stdout_writer.write_all(b"\n").await?;
            }
            stdout_writer.shutdown().await
        });
        let (mut stdin_reader, stdin_writer) = duplex(4096);
        let stdin_writer = Arc::new(Mutex::new(stdin_writer));
        let command = PiBtwCommand {
            name: "btw".to_string(),
            question: Some("quick aside".to_string()),
        };
        let mut events = Vec::new();

        let result = drain_pi_rpc_lines(
            &mut BufReader::new(stdout_reader).lines(),
            &stdin_writer,
            &mut |event| events.push(event),
            None,
            None,
            || false,
            Some(&command),
            false,
            false,
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

    #[tokio::test]
    async fn btw_slash_sends_prompt_without_get_commands_probe() {
        let (stdout_reader, mut stdout_writer) = duplex(8192);
        let writer = tokio::spawn(async move {
            for line in [
                r#"{"id":1,"type":"response","command":"prompt","success":true}"#,
                r#"{"id":"kivio-btw-entries","type":"response","command":"get_entries","success":true,"data":{"entries":[]}}"#,
                r#"{"type":"agent_settled"}"#,
            ] {
                stdout_writer.write_all(line.as_bytes()).await?;
                stdout_writer.write_all(b"\n").await?;
            }
            stdout_writer.shutdown().await
        });
        let (mut stdin_reader, stdin_writer) = duplex(4096);
        let stdin_writer = Arc::new(Mutex::new(stdin_writer));
        let mut events = Vec::new();

        let result = run_pi_rpc_io(
            &mut BufReader::new(stdout_reader).lines(),
            &stdin_writer,
            "/btw side question",
            &[],
            &mut |event| events.push(event),
            None,
            None,
            || false,
            true,
        )
        .await;

        assert_eq!(result, Ok(()));
        assert!(writer.await.unwrap().is_ok());
        drop(stdin_writer);
        let mut requests = String::new();
        stdin_reader.read_to_string(&mut requests).await.unwrap();
        assert!(
            !requests.contains("get_commands"),
            "btw must not stall on a command probe: {requests}"
        );
        assert!(requests.contains(r#""type":"prompt""#));
        assert!(requests.contains("/btw side question"));
    }

    /// 手动 /compact：compaction_end 事件发分隔线，compact response 收尾轮次，
    /// response 兜底不得重复发（compaction_emitted 去重）。
    #[tokio::test]
    async fn manual_compact_ends_on_response_and_emits_single_divider() {
        let (stdout_reader, mut stdout_writer) = duplex(8192);
        let writer = tokio::spawn(async move {
            for line in [
                r#"{"type":"compaction_start","reason":"manual"}"#,
                r#"{"type":"compaction_end","reason":"manual","result":{"summary":"s","tokensBefore":90000,"estimatedTokensAfter":20000},"aborted":false}"#,
                r#"{"id":1,"type":"response","command":"compact","success":true,"data":{"summary":"s","tokensBefore":90000,"estimatedTokensAfter":20000}}"#,
            ] {
                stdout_writer.write_all(line.as_bytes()).await?;
                stdout_writer.write_all(b"\n").await?;
            }
            stdout_writer.shutdown().await
        });
        let (_stdin_reader, stdin_writer) = duplex(4096);
        let stdin_writer = Arc::new(Mutex::new(stdin_writer));
        let mut events = Vec::new();

        let result = drain_pi_rpc_lines(
            &mut BufReader::new(stdout_reader).lines(),
            &stdin_writer,
            &mut |event| events.push(event),
            None,
            None,
            || false,
            None,
            true,
            false,
        )
        .await;

        assert_eq!(result, Ok(()));
        assert!(writer.await.unwrap().is_ok());
        let dividers = events
            .iter()
            .filter(|event| matches!(event, UnifiedAgentEvent::CliCompacted { .. }))
            .count();
        assert_eq!(dividers, 1, "恰好一条压缩分隔线：{events:?}");
        assert!(matches!(
            events
                .iter()
                .find(|e| matches!(e, UnifiedAgentEvent::CliCompacted { .. })),
            Some(UnifiedAgentEvent::CliCompacted {
                trigger,
                pre_tokens: Some(90000),
                post_tokens: Some(20000),
                ..
            }) if trigger == "manual"
        ));
    }

    /// compaction_end 事件缺席（或晚于 response 被丢弃）时，从 response 的 data 兜底
    /// 合成分隔线，保证手动压缩至少有一条可见记录。
    #[tokio::test]
    async fn manual_compact_synthesizes_divider_from_response_when_event_missing() {
        let (stdout_reader, mut stdout_writer) = duplex(8192);
        let writer = tokio::spawn(async move {
            let line = r#"{"id":1,"type":"response","command":"compact","success":true,"data":{"summary":"s","tokensBefore":50000,"estimatedTokensAfter":12000}}"#;
            stdout_writer.write_all(line.as_bytes()).await?;
            stdout_writer.write_all(b"\n").await?;
            stdout_writer.shutdown().await
        });
        let (_stdin_reader, stdin_writer) = duplex(4096);
        let stdin_writer = Arc::new(Mutex::new(stdin_writer));
        let mut events = Vec::new();

        let result = drain_pi_rpc_lines(
            &mut BufReader::new(stdout_reader).lines(),
            &stdin_writer,
            &mut |event| events.push(event),
            None,
            None,
            || false,
            None,
            true,
            false,
        )
        .await;

        assert_eq!(result, Ok(()));
        assert!(writer.await.unwrap().is_ok());
        assert!(matches!(
            events.as_slice(),
            [UnifiedAgentEvent::CliCompacted {
                trigger,
                pre_tokens: Some(50000),
                post_tokens: Some(12000),
                ..
            }] if trigger == "manual"
        ));
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

    #[tokio::test]
    async fn extension_ui_confirm_bridges_user_decision_and_fails_closed_without_host() {
        let raw = json!({
            "type": "extension_ui_request",
            "id": "ui-1",
            "method": "confirm",
            "title": "Delete?",
            "message": "Cannot be undone",
        });
        let (mut output_reader, output_writer) = duplex(2048);
        let output_writer = Arc::new(Mutex::new(output_writer));
        let (ask_tx, mut ask_rx) = mpsc::channel(1);
        let (decision_tx, decision_rx) = mpsc::channel(1);
        let mut bridge = crate::external_agents::session::live::ApprovalBridge {
            requests: ask_tx,
            decisions: decision_rx,
        };
        let host = async move {
            let ask = ask_rx.recv().await.expect("extension ask");
            assert_eq!(ask.request_id, "ui-1");
            assert_eq!(ask.tool_name, "PiExtensionConfirm");
            decision_tx
                .send(crate::external_agents::session::live::ApprovalDecision {
                    request_id: ask.request_id,
                    approved: true,
                    updated_input: Some(json!({ "confirmed": false })),
                    set_permission_mode: None,
                })
                .await
                .expect("decision");
        };
        let (result, ()) = tokio::join!(
            bridge_extension_ui(&output_writer, &raw, Some(&mut bridge), &|| false),
            host
        );
        result.expect("bridged response");
        let line = BufReader::new(&mut output_reader)
            .lines()
            .next_line()
            .await
            .expect("read")
            .expect("response");
        let response: Value = serde_json::from_str(&line).expect("json");
        assert_eq!(response["id"], "ui-1");
        assert_eq!(response["confirmed"], false);

        let (mut rejected_reader, rejected_writer) = duplex(1024);
        let rejected_writer = Arc::new(Mutex::new(rejected_writer));
        bridge_extension_ui(&rejected_writer, &raw, None, &|| false)
            .await
            .expect("fail closed response");
        let line = BufReader::new(&mut rejected_reader)
            .lines()
            .next_line()
            .await
            .expect("read")
            .expect("response");
        let response: Value = serde_json::from_str(&line).expect("json");
        assert_eq!(response["cancelled"], true);
        assert!(response.get("confirmed").is_none());
    }

    #[tokio::test]
    async fn control_response_is_correlated_before_settlement() {
        let (stdout_reader, mut stdout_writer) = duplex(2048);
        let stdin = Arc::new(Mutex::new(sink()));
        let waiters: PiControlWaiters = Arc::new(Mutex::new(HashMap::new()));
        let response = issue_control_command(
            &stdin,
            &waiters,
            "steer-1".to_string(),
            Some((
                MessageInjectionKind::Steer,
                "frontend-1".to_string(),
                "new direction".to_string(),
            )),
            json!({ "type": "steer", "message": "new direction" }),
        )
        .await
        .expect("issue steer");
        stdout_writer
            .write_all(
                b"{\"id\":\"steer-1\",\"type\":\"response\",\"command\":\"steer\",\"success\":true}\n{\"type\":\"agent_settled\"}\n",
            )
            .await
            .expect("events");
        let mut events = Vec::new();
        let result = drain_pi_rpc_lines(
            &mut BufReader::new(stdout_reader).lines(),
            &stdin,
            &mut |event| events.push(event),
            None,
            Some(&waiters),
            || false,
            None,
            false,
            true,
        )
        .await;
        assert_eq!(result, Ok(()));
        assert!(matches!(response.await, Ok(Ok(()))));
        assert!(matches!(
            events.as_slice(),
            [UnifiedAgentEvent::UserSteer { id, text }]
                if id == "frontend-1" && text == "new direction"
        ));
    }

    #[tokio::test]
    async fn leftover_control_waiters_are_failed_when_the_turn_ends() {
        let stdin = Arc::new(Mutex::new(sink()));
        let waiters: PiControlWaiters = Arc::new(Mutex::new(HashMap::new()));
        let response = issue_control_command(
            &stdin,
            &waiters,
            "steer-late".to_string(),
            Some((
                MessageInjectionKind::Steer,
                "frontend-1".to_string(),
                "hello".to_string(),
            )),
            json!({ "type": "steer", "message": "hello" }),
        )
        .await
        .expect("issue steer");
        assert!(waiters.lock().await.contains_key("steer-late"));
        fail_pending_controls(
            &waiters,
            "Pi turn ended before the control command was acknowledged",
        )
        .await;
        assert!(waiters.lock().await.is_empty());
        assert!(matches!(
            response.await,
            Ok(Err(error)) if error.contains("turn ended")
        ));
    }

    #[test]
    fn session_request_payloads_match_pi_rpc_contract() {
        assert_eq!(
            pi_session_request_payload(&PiSessionRequest::GetEntries {
                since: Some("entry-1".to_string()),
            }),
            json!({ "type": "get_entries", "since": "entry-1" })
        );
        assert_eq!(
            pi_session_request_payload(&PiSessionRequest::GetForkMessages),
            json!({ "type": "get_fork_messages" })
        );
        assert_eq!(
            pi_session_request_payload(&PiSessionRequest::Clone),
            json!({ "type": "clone" })
        );
        assert_eq!(
            pi_session_request_payload(&PiSessionRequest::Switch {
                session_path: "/tmp/session.jsonl".to_string(),
            }),
            json!({ "type": "switch_session", "sessionPath": "/tmp/session.jsonl" })
        );
    }

    #[tokio::test]
    async fn idle_tree_request_returns_tree_and_authoritative_state() {
        let (client_stdin, server_stdin) = duplex(4096);
        let (client_stdout, mut server_stdout) = duplex(4096);
        let server = tokio::spawn(async move {
            let mut requests = BufReader::new(server_stdin).lines();
            let tree = requests.next_line().await.unwrap().unwrap();
            let tree: Value = serde_json::from_str(&tree).unwrap();
            assert_eq!(tree["type"], "get_tree");
            assert_eq!(tree["id"], "tree-1");
            server_stdout
                .write_all(
                    b"{\"id\":\"tree-1\",\"type\":\"response\",\"command\":\"get_tree\",\"success\":true,\"data\":{\"tree\":[],\"leafId\":null}}\n",
                )
                .await
                .unwrap();
            let state = requests.next_line().await.unwrap().unwrap();
            let state: Value = serde_json::from_str(&state).unwrap();
            assert_eq!(state["type"], "get_state");
            server_stdout
                .write_all(
                    format!(
                        "{{\"id\":{},\"type\":\"response\",\"command\":\"get_state\",\"success\":true,\"data\":{{\"sessionId\":\"s1\",\"sessionFile\":\"/tmp/s1.jsonl\"}}}}\n",
                        serde_json::to_string(&state["id"]).unwrap()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let stdin = Arc::new(Mutex::new(client_stdin));
        let result = run_idle_pi_session_request(
            &mut BufReader::new(client_stdout).lines(),
            &stdin,
            "tree-1",
            &PiSessionRequest::GetTree,
        )
        .await
        .unwrap();
        server.await.unwrap();
        assert_eq!(result.data["tree"], json!([]));
        assert_eq!(result.state.unwrap()["sessionId"], "s1");
    }

    #[tokio::test]
    async fn idle_fork_request_uses_entry_id_and_reads_new_session_identity() {
        let (client_stdin, server_stdin) = duplex(4096);
        let (client_stdout, mut server_stdout) = duplex(4096);
        let server = tokio::spawn(async move {
            let mut requests = BufReader::new(server_stdin).lines();
            let fork = requests.next_line().await.unwrap().unwrap();
            let fork: Value = serde_json::from_str(&fork).unwrap();
            assert_eq!(fork["type"], "fork");
            assert_eq!(fork["entryId"], "user-2");
            server_stdout
                .write_all(
                    b"{\"id\":\"fork-1\",\"type\":\"response\",\"command\":\"fork\",\"success\":true,\"data\":{\"text\":\"second prompt\",\"cancelled\":false}}\n",
                )
                .await
                .unwrap();
            let state = requests.next_line().await.unwrap().unwrap();
            let state: Value = serde_json::from_str(&state).unwrap();
            server_stdout
                .write_all(
                    format!(
                        "{{\"id\":{},\"type\":\"response\",\"command\":\"get_state\",\"success\":true,\"data\":{{\"sessionId\":\"forked\",\"sessionFile\":\"/tmp/forked.jsonl\"}}}}\n",
                        serde_json::to_string(&state["id"]).unwrap()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let stdin = Arc::new(Mutex::new(client_stdin));
        let result = run_idle_pi_session_request(
            &mut BufReader::new(client_stdout).lines(),
            &stdin,
            "fork-1",
            &PiSessionRequest::Fork {
                entry_id: "user-2".to_string(),
            },
        )
        .await
        .unwrap();
        server.await.unwrap();
        assert_eq!(result.data["text"], "second prompt");
        assert_eq!(result.state.unwrap()["sessionId"], "forked");
    }

    #[tokio::test]
    async fn follow_up_command_serializes_images_and_emits_distinct_event() {
        let (stdout_reader, mut stdout_writer) = duplex(2048);
        let (command_reader, command_writer) = duplex(2048);
        let stdin = Arc::new(Mutex::new(command_writer));
        let waiters: PiControlWaiters = Arc::new(Mutex::new(HashMap::new()));
        let response = issue_control_command(
            &stdin,
            &waiters,
            "follow-up-1".to_string(),
            Some((
                MessageInjectionKind::FollowUp,
                "frontend-follow-up".to_string(),
                "附带图片（1）".to_string(),
            )),
            json!({
                "type": "follow_up",
                "message": "",
                "images": [{ "type": "image", "data": "abc", "mimeType": "image/png" }]
            }),
        )
        .await
        .expect("issue follow-up");
        let mut command_lines = BufReader::new(command_reader).lines();
        let command: Value = serde_json::from_str(
            &command_lines
                .next_line()
                .await
                .expect("command line")
                .expect("command"),
        )
        .expect("json command");
        assert_eq!(command["type"], "follow_up");
        assert_eq!(command["images"][0]["mimeType"], "image/png");

        stdout_writer
            .write_all(
                b"{\"id\":\"follow-up-1\",\"type\":\"response\",\"command\":\"follow_up\",\"success\":true}\n{\"type\":\"agent_settled\"}\n",
            )
            .await
            .expect("events");
        let mut events = Vec::new();
        let result = drain_pi_rpc_lines(
            &mut BufReader::new(stdout_reader).lines(),
            &stdin,
            &mut |event| events.push(event),
            None,
            Some(&waiters),
            || false,
            None,
            false,
            true,
        )
        .await;
        assert_eq!(result, Ok(()));
        assert!(matches!(response.await, Ok(Ok(()))));
        assert!(matches!(
            events.as_slice(),
            [UnifiedAgentEvent::UserFollowUp { id, text }]
                if id == "frontend-follow-up" && text == "附带图片（1）"
        ));
    }

    #[tokio::test]
    async fn persistent_abort_drains_to_settled_and_returns_cancelled() {
        let (stdout_reader, mut stdout_writer) = duplex(2048);
        let stdin = Arc::new(Mutex::new(sink()));
        let waiters: PiControlWaiters = Arc::new(Mutex::new(HashMap::new()));
        let response = issue_control_command(
            &stdin,
            &waiters,
            "abort-1".to_string(),
            None,
            json!({ "type": "abort" }),
        )
        .await
        .expect("issue abort");
        stdout_writer
            .write_all(
                b"{\"id\":\"abort-1\",\"type\":\"response\",\"command\":\"abort\",\"success\":true}\n{\"type\":\"agent_end\"}\n{\"type\":\"agent_settled\"}\n",
            )
            .await
            .expect("events");
        let result = drain_pi_rpc_lines(
            &mut BufReader::new(stdout_reader).lines(),
            &stdin,
            &mut |_| {},
            None,
            Some(&waiters),
            || true,
            None,
            false,
            true,
        )
        .await;
        assert_eq!(result, Err("cancelled".to_string()));
        assert!(matches!(response.await, Ok(Ok(()))));
    }

    #[tokio::test]
    async fn pi_prompt_serializes_native_images() {
        let (client_stdin, server_stdin) = duplex(4096);
        let stdin = Arc::new(Mutex::new(client_stdin));
        let (client_stdout, mut server_stdout) = duplex(4096);
        let server = tokio::spawn(async move {
            let request = BufReader::new(server_stdin)
                .lines()
                .next_line()
                .await
                .expect("read")
                .expect("request");
            let request: Value = serde_json::from_str(&request).expect("json");
            assert_eq!(request["images"][0]["type"], "image");
            assert_eq!(request["images"][0]["data"], "aGVsbG8=");
            assert_eq!(request["images"][0]["mimeType"], "image/png");
            server_stdout
                .write_all(
                    b"{\"id\":1,\"type\":\"response\",\"command\":\"prompt\",\"success\":true}\n{\"type\":\"agent_settled\"}\n",
                )
                .await
                .expect("settled");
        });
        let mut reader = BufReader::new(client_stdout).lines();
        let image = crate::external_agents::attachments::ImageBlock {
            data_base64: "aGVsbG8=".to_string(),
            mime: "image/png".to_string(),
            path: std::path::PathBuf::from("image.png"),
        };
        run_pi_rpc_io(
            &mut reader,
            &stdin,
            "inspect",
            &[image],
            &mut |_| {},
            None,
            None,
            || false,
            true,
        )
        .await
        .expect("image turn");
        server.await.expect("server");
    }

    #[tokio::test]
    async fn persistent_eof_before_settled_is_an_error() {
        let (stdout_reader, stdout_writer) = duplex(256);
        drop(stdout_writer);
        let mut reader = BufReader::new(stdout_reader).lines();
        let stdin = Arc::new(Mutex::new(sink()));
        let result = drain_pi_rpc_lines(
            &mut reader,
            &stdin,
            &mut |_| {},
            None,
            None,
            || false,
            None,
            false,
            true,
        )
        .await;
        assert_eq!(
            result,
            Err("Pi RPC process exited before agent_settled".to_string())
        );
    }

    #[test]
    fn persisted_live_session_id_overrides_a_fresh_argv_id() {
        let args = vec![
            "--mode".to_string(),
            "rpc".to_string(),
            "--session-id".to_string(),
            "fresh-id".to_string(),
        ];
        let (effective, session_id, resumed) =
            persistent_session_args(&args, Some("persisted-id")).expect("session args");
        assert_eq!(session_id, "persisted-id");
        assert!(resumed);
        assert_eq!(
            effective.windows(2).find(|pair| pair[0] == "--session-id"),
            Some(&["--session-id".to_string(), "persisted-id".to_string()][..])
        );
    }

    #[tokio::test]
    async fn persistent_io_serves_two_turns_on_the_same_streams() {
        let (client_stdin, server_stdin) = duplex(4096);
        let (client_stdout, mut server_stdout) = duplex(4096);
        let server = tokio::spawn(async move {
            let mut requests = BufReader::new(server_stdin).lines();
            for expected in ["first", "second"] {
                let request = requests
                    .next_line()
                    .await
                    .expect("read request")
                    .expect("request line");
                let request: Value = serde_json::from_str(&request).expect("request json");
                assert_eq!(request["type"], "prompt");
                assert_eq!(request["message"], expected);
                let delta = json!({
                    "type": "message_update",
                    "assistantMessageEvent": {"type": "text_delta", "delta": expected},
                });
                for event in [
                    json!({"id": 1, "type": "response", "command": "prompt", "success": true}),
                    delta,
                    json!({"type": "agent_end"}),
                    json!({"type": "agent_settled"}),
                ] {
                    server_stdout
                        .write_all(format!("{event}\n").as_bytes())
                        .await
                        .expect("write event");
                }
            }
        });

        let mut reader = BufReader::new(client_stdout).lines();
        let stdin = Arc::new(Mutex::new(client_stdin));
        let mut deltas = Vec::new();
        for prompt in ["first", "second"] {
            run_pi_rpc_io(
                &mut reader,
                &stdin,
                prompt,
                &[],
                &mut |event| {
                    if let UnifiedAgentEvent::TextDelta { delta } = event {
                        deltas.push(delta);
                    }
                },
                None,
                None,
                || false,
                true,
            )
            .await
            .expect("persistent turn");
        }

        server.await.expect("mock server");
        assert_eq!(deltas, ["first", "second"]);
    }

    #[tokio::test]
    async fn stale_settlement_after_compact_cannot_finish_the_next_prompt() {
        let (client_stdin, server_stdin) = duplex(4096);
        let (client_stdout, mut server_stdout) = duplex(4096);
        let server = tokio::spawn(async move {
            let mut requests = BufReader::new(server_stdin).lines();
            let compact = requests
                .next_line()
                .await
                .expect("compact read")
                .expect("compact request");
            let compact: Value = serde_json::from_str(&compact).expect("compact json");
            assert_eq!(compact["type"], "compact");
            for event in [
                json!({
                    "type": "compaction_end",
                    "reason": "manual",
                    "aborted": false,
                    "result": {"tokensBefore": 100, "estimatedTokensAfter": 10},
                }),
                json!({
                    "id": 1,
                    "type": "response",
                    "command": "compact",
                    "success": true,
                    "data": {"tokensBefore": 100, "estimatedTokensAfter": 10},
                }),
                json!({"type": "agent_settled"}),
            ] {
                server_stdout
                    .write_all(format!("{event}\n").as_bytes())
                    .await
                    .expect("compact event");
            }

            let prompt = requests
                .next_line()
                .await
                .expect("prompt read")
                .expect("prompt request");
            let prompt: Value = serde_json::from_str(&prompt).expect("prompt json");
            assert_eq!(prompt["message"], "after compact");
            for event in [
                json!({"id": 1, "type": "response", "command": "prompt", "success": true}),
                json!({
                    "type": "message_update",
                    "assistantMessageEvent": {"type": "text_delta", "delta": "real answer"},
                }),
                json!({"type": "agent_end"}),
                json!({"type": "agent_settled"}),
            ] {
                server_stdout
                    .write_all(format!("{event}\n").as_bytes())
                    .await
                    .expect("prompt event");
            }
        });

        let mut reader = BufReader::new(client_stdout).lines();
        let stdin = Arc::new(Mutex::new(client_stdin));
        run_pi_rpc_io(
            &mut reader,
            &stdin,
            "/compact",
            &[],
            &mut |_| {},
            None,
            None,
            || false,
            true,
        )
        .await
        .expect("compact turn");
        let mut text = String::new();
        run_pi_rpc_io(
            &mut reader,
            &stdin,
            "after compact",
            &[],
            &mut |event| {
                if let UnifiedAgentEvent::TextDelta { delta } = event {
                    text.push_str(&delta);
                }
            },
            None,
            None,
            || false,
            true,
        )
        .await
        .expect("prompt turn");
        server.await.expect("server");
        assert_eq!(text, "real answer");
    }

    #[tokio::test]
    async fn leftover_btw_entries_cannot_finish_the_next_prompt() {
        let (client_stdin, server_stdin) = duplex(4096);
        let (client_stdout, mut server_stdout) = duplex(4096);
        let server = tokio::spawn(async move {
            let mut requests = BufReader::new(server_stdin).lines();
            let _first = requests
                .next_line()
                .await
                .expect("first prompt")
                .expect("first line");
            for event in [
                json!({"id": 1, "type": "response", "command": "prompt", "success": true}),
                json!({"id": "kivio-btw-entries", "type": "response", "command": "get_entries", "success": true, "data": {"entries": []}}),
                json!({"type": "agent_settled"}),
                json!({"id": "kivio-btw-entries", "type": "response", "command": "get_entries", "success": true, "data": {"entries": []}}),
            ] {
                server_stdout
                    .write_all(format!("{event}\n").as_bytes())
                    .await
                    .expect("write first turn");
            }

            let prompt = loop {
                let line = requests
                    .next_line()
                    .await
                    .expect("follow-up request")
                    .expect("request line");
                let value: Value = serde_json::from_str(&line).expect("request json");
                // /btw with a question also writes get_entries on the same stdin; skip it.
                if value.get("type").and_then(Value::as_str) == Some("get_entries") {
                    continue;
                }
                break value;
            };
            assert_eq!(prompt["type"], "prompt");
            assert_eq!(prompt["message"], "follow up");
            for event in [
                json!({"id": 1, "type": "response", "command": "prompt", "success": true}),
                json!({
                    "type": "message_update",
                    "assistantMessageEvent": {"type": "text_delta", "delta": "still here"},
                }),
                json!({"type": "agent_end"}),
                json!({"type": "agent_settled"}),
            ] {
                server_stdout
                    .write_all(format!("{event}\n").as_bytes())
                    .await
                    .expect("write second turn");
            }
        });

        let mut reader = BufReader::new(client_stdout).lines();
        let stdin = Arc::new(Mutex::new(client_stdin));
        run_pi_rpc_io(
            &mut reader,
            &stdin,
            "/btw leftover",
            &[],
            &mut |_| {},
            None,
            None,
            || false,
            true,
        )
        .await
        .expect("btw turn");
        let mut text = String::new();
        run_pi_rpc_io(
            &mut reader,
            &stdin,
            "follow up",
            &[],
            &mut |event| {
                if let UnifiedAgentEvent::TextDelta { delta } = event {
                    text.push_str(&delta);
                }
            },
            None,
            None,
            || false,
            true,
        )
        .await
        .expect("follow-up turn");
        server.await.expect("server");
        assert_eq!(text, "still here");
    }

    #[test]
    fn missing_pi_session_file_is_not_a_successful_resume() {
        assert!(!pi_native_session_present("kivio-missing-session-id-for-test"));
        assert!(is_missing_pi_session_error(
            "Pi session \"abc\" not found"
        ));
        assert!(!is_missing_pi_session_error("Pi RPC timed out"));
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

    #[test]
    fn parse_pi_model_thinking_reads_column_by_header() {
        let out = "provider          model          context  max-out  thinking  images\n\
                   edgefn            DeepSeek-Flash 128K     8.2K     no        no\n\
                   kivio-p           claude-son-5   1M       128K     yes       yes";
        let thinking = parse_pi_model_thinking(out);
        assert_eq!(thinking.get("edgefn/DeepSeek-Flash"), Some(&false));
        assert_eq!(thinking.get("kivio-p/claude-son-5"), Some(&true));
        // 表头缺 thinking 列 → 空表（未知，不隐藏档位）。
        assert!(parse_pi_model_thinking("provider model context\na b 1K").is_empty());
        assert!(parse_pi_model_thinking("").is_empty());
    }
}
