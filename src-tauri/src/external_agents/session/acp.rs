use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::external_agents::session::live::SessionCommand;
use crate::external_agents::stream::{usage_from_parts, CliUsageParts};
use crate::external_agents::types::{
    default_model_option, ExternalCliSlashCommand, RuntimeModelOption, UnifiedAgentEvent,
};
use crate::proc::NoConsoleWindow;

const ACP_PROTOCOL_VERSION: i64 = 1;

/// ACP 模型探测结果：模型列表 + CLI 当前配置的模型/推理等级（用于胶囊回填，见 R "同步 CLI 当前配置"）。
pub struct AcpModelsProbe {
    pub models: Vec<RuntimeModelOption>,
    pub current_model: Option<String>,
    pub current_reasoning: Option<String>,
    /// CLI 在 `configOptions` 里自报的推理档位选项（如 kimi 的 `thinking`：low/high/max）。
    /// 空 = 该 CLI 不暴露档位，前端退回 `def.reasoning_options` 静态表。
    pub reasoning_options: Vec<RuntimeModelOption>,
}

/// Handshake timeouts (缺陷 4 / R3): Paseo uses 60s; desktop starts at 30s. `initialize` and
/// `session/new` each get their own budget so a slow one doesn't starve the other.
const ACP_INITIALIZE_TIMEOUT: Duration = Duration::from_secs(30);
const ACP_SESSION_NEW_TIMEOUT: Duration = Duration::from_secs(30);

use crate::external_agents::spawn::{fold_stderr, join_stderr_tail as join_tail};

#[derive(Debug, Clone)]
pub struct AcpMcpServer {
    pub server_type: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

fn build_session_new_params(cwd: &Path, mcp_servers: &[AcpMcpServer]) -> Value {
    let servers: Vec<Value> = mcp_servers
        .iter()
        .map(|s| {
            json!({
                "type": s.server_type,
                "name": s.name,
                "command": s.command,
                "args": s.args,
                "env": s.env.iter().map(|(name, value)| json!({ "name": name, "value": value })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "cwd": cwd.to_string_lossy(),
        "mcpServers": servers,
    })
}

async fn write_rpc(
    stdin: &mut tokio::process::ChildStdin,
    id: u64,
    method: &str,
    params: Value,
) -> Result<(), String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let mut line = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|e| e.to_string())
}

async fn write_rpc_result(
    stdin: &mut tokio::process::ChildStdin,
    id: &Value,
    result: Value,
) -> Result<(), String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    });
    let mut line = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|e| e.to_string())
}

fn rpc_error_message(value: &Value) -> Option<String> {
    let error = value.get("error")?;
    if let Some(message) = error.get("message").and_then(|v| v.as_str()) {
        return Some(message.to_string());
    }
    error.get("code").map(|c| c.to_string())
}

/// 从 ACP modelId 的方括号参数里抠出上下文窗口提示。
///
/// cursor 实测形态（`session/new` 的 `models.availableModels[]`）：
/// ```text
/// claude-opus-5[thinking=true,context=300k,effort=high,fast=false]
/// gpt-5.6-sol[context=272k,reasoning=medium,fast=false]
/// composer-2.5[fast=true]        <- 无 context，返回 None
/// ```
/// 32 个模型中 13 个带 `context=`，且**全部没有** `_meta.totalContextTokens`
/// （那是 grok 的形态）——不解析这里 cursor 的分母就永远没来源。
///
/// 复用 `context::parse_context_window_label`（"300k" → 300000，支持 K/M 后缀与浮点）。
/// 没有方括号、没有 `context=`、值解析不出来时一律 `None`——**不猜**。
fn context_window_from_model_id_params(model_id: &str) -> Option<u32> {
    let start = model_id.find('[')?;
    let end = model_id[start + 1..].find(']')? + start + 1;
    model_id[start + 1..end]
        .split(',')
        .filter_map(|param| param.split_once('='))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case("context"))
        .and_then(|(_, value)| {
            crate::external_agents::context::parse_context_window_label(value.trim())
        })
}

fn normalize_models(result: &Value) -> Vec<RuntimeModelOption> {
    let mut out = vec![default_model_option()];
    let mut seen = HashSet::from(["default".to_string()]);

    // 有 currentModelId（如 grok 的 models.currentModelId）时，把 default 占位标成
    // "Default (<真实模型名>)" —— 否则单模型 CLI 的胶囊上永远只见 "Default"，
    // 用户无从知道探测其实成功了（与 claude 的 label_for_claude_model 呈现一致）。
    let current_id = result
        .get("models")
        .and_then(|m| m.get("currentModelId"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());

    if let Some(config_options) = result.get("configOptions").and_then(|v| v.as_array()) {
        for raw_option in config_options {
            let option = match raw_option.as_object() {
                Some(o) => o,
                None => continue,
            };
            let config_id = option.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if config_id != "model"
                && option.get("category").and_then(|v| v.as_str()) != Some("model")
            {
                continue;
            }
            if let Some(values) = option.get("options").and_then(|v| v.as_array()) {
                for raw_value in values {
                    let value = match raw_value.as_object() {
                        Some(o) => o,
                        None => continue,
                    };
                    let id = value
                        .get("value")
                        .or_else(|| value.get("id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if id.is_empty() || !seen.insert(id.to_string()) {
                        continue;
                    }
                    let name = value.get("name").and_then(|v| v.as_str()).unwrap_or(id);
                    // cursor 的模型走的是**这条** configOptions 分支（不是下面的
                    // models.availableModels），且它把窗口写在 modelId 的方括号里
                    // （`claude-opus-5[thinking=true,context=300k,...]`）——实测 33 个模型
                    // 里 13 个带 `context=`，全都没有 `_meta.totalContextTokens`。
                    // 这里若留 None，cursor 的分母就永远没来源、只能吃兜底。
                    // `_meta.totalContextTokens` 仍优先（显式字段比字符串提示可靠）。
                    let window = value
                        .get("_meta")
                        .and_then(|m| m.get("totalContextTokens"))
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32)
                        .or_else(|| context_window_from_model_id_params(id));
                    if current_id == Some(id) {
                        out[0].label = format!("Default ({name})");
                        out[0].context_window_tokens = window;
                    }
                    out.push(RuntimeModelOption {
                        id: id.to_string(),
                        label: if name != id {
                            format!("{name} ({id})")
                        } else {
                            id.to_string()
                        },
                        context_window_tokens: window,
                    });
                }
            }
            if out.len() > 1 {
                return out;
            }
        }
    }

    if let Some(models) = result.get("models").and_then(|v| v.as_object()) {
        if let Some(available) = models.get("availableModels").and_then(|v| v.as_array()) {
            for model in available {
                let id = model.get("modelId").and_then(|v| v.as_str()).unwrap_or("");
                if id.is_empty() || !seen.insert(id.to_string()) {
                    continue;
                }
                let name = model.get("name").and_then(|v| v.as_str()).unwrap_or(id);
                // 窗口优先级：`_meta.totalContextTokens`（显式字段，grok）> modelId 方括号里的
                // `context=300k`（字符串提示，cursor）> None。显式字段比字符串提示可靠。
                let window = model
                    .get("_meta")
                    .and_then(|m| m.get("totalContextTokens"))
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32)
                    .or_else(|| context_window_from_model_id_params(id));
                if current_id == Some(id) {
                    out[0].label = format!("Default ({name})");
                    out[0].context_window_tokens = window;
                }
                out.push(RuntimeModelOption {
                    id: id.to_string(),
                    label: if name != id {
                        format!("{name} ({id})")
                    } else {
                        id.to_string()
                    },
                    // grok 等在 _meta.totalContextTokens 报真实窗口（如 500000）；
                    // cursor 把它写在 modelId 的 `context=300k` 里；都没有则留空。
                    context_window_tokens: window,
                });
            }
        }
    }

    out
}

/// 把某个 ACP `result`/`update` 里解析出的模型并入累加器（去重）。default 占位不进 `acc`，
/// 但若其标签被 `normalize_models` 用 currentModelId 富化过（"Default (Grok 4.5)"），
/// 则更新 `default_slot`，最终装配时替换普通占位。
fn merge_acp_models(
    acc: &mut Vec<RuntimeModelOption>,
    seen: &mut HashSet<String>,
    default_slot: &mut RuntimeModelOption,
    value: &Value,
) {
    for model in normalize_models(value) {
        if model.id == "default" {
            if model.label != "Default" {
                *default_slot = model;
            }
            continue;
        }
        if seen.insert(model.id.clone()) {
            acc.push(model);
        }
    }
}

/// 测试替身：复刻 `detect_acp_models` 读循环的行处理（跳空行 / 跳非 JSON banner / 合并
/// result 与异步 session/update 推送），不启子进程。生产循环与此共用 `merge_acp_models`。
#[cfg(test)]
fn collect_acp_models_from_lines(lines: &[&str]) -> Option<Vec<RuntimeModelOption>> {
    let mut collected: Vec<RuntimeModelOption> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut default_slot = default_model_option();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if value.get("method").and_then(|v| v.as_str()) == Some("session/update") {
            if let Some(update) = value.get("params").and_then(|p| p.get("update")) {
                merge_acp_models(&mut collected, &mut seen, &mut default_slot, update);
            }
            continue;
        }
        if let Some(result) = value.get("result") {
            merge_acp_models(&mut collected, &mut seen, &mut default_slot, result);
        }
    }
    if collected.is_empty() {
        return None;
    }
    let mut out = vec![default_slot];
    out.extend(collected);
    Some(out)
}

/// 从一个 ACP `result`/`update` 里抽取 CLI 当前配置：`models.currentModelId` 作为当前模型，
/// 当前模型条目的 `_meta.reasoningEfforts` 里 `default=true` 的 id 作为当前推理等级（grok 实测 high）。
/// 结构未知时保守返回 None（不破坏 default 富化路径）。
/// 从 ACP `configOptions` 里抽出**推理档位**那一项的选项列表 + 当前值。
///
/// kimi 实测形态（`session/new` 的 result）：
/// ```json
/// {"type":"select","id":"thinking","category":"thought_level","currentValue":"high",
///  "options":[{"value":"low","name":"Low"},{"value":"high","name":"High"},
///             {"value":"max","name":"Max"}]}
/// ```
/// 此前只有「设置」侧认这个（`acp_config_ids` 能发 set_config_option），
/// **发现侧完全没读** —— 于是胶囊上没有档位可选。
///
/// 识别口径与 `acp_config_ids` 保持一致（id 含 reasoning/thought/thinking/effort，
/// 或 category 为 reasoning/thought/thought_level），避免两处判据分叉。
/// 是否是无意义的开关型档位（kimi always_thinking 模型：options 只有 `On`）。
/// 这种不能当 effort 胶囊用——用户会看到莫名其妙的「On」。
fn is_boolean_toggle_efforts(options: &[RuntimeModelOption]) -> bool {
    if options.is_empty() {
        return false;
    }
    options.iter().all(|o| {
        matches!(
            o.id.to_lowercase().as_str(),
            "on" | "off" | "true" | "false" | "enabled" | "disabled" | "1" | "0" | "yes" | "no"
        )
    })
}

fn extract_acp_reasoning(value: &Value) -> (Vec<RuntimeModelOption>, Option<String>) {
    let Some(config_options) = value.get("configOptions").and_then(|v| v.as_array()) else {
        return (Vec::new(), None);
    };
    // 先收集所有候选，优先取多档 effort（low/high/max），跳过只有 On/Off 的开关。
    let mut best: Option<(Vec<RuntimeModelOption>, Option<String>)> = None;
    for raw in config_options {
        let Some(option) = raw.as_object() else {
            continue;
        };
        let id_l = option
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        let category = option
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let is_reasoning = id_l.contains("reasoning")
            || id_l.contains("thought")
            || id_l.contains("thinking")
            || id_l.contains("effort")
            || category == "reasoning"
            || category == "thought"
            || category == "thought_level";
        if !is_reasoning {
            continue;
        }
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for raw_value in option
            .get("options")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let Some(entry) = raw_value.as_object() else {
                continue;
            };
            let id = entry
                .get("value")
                .or_else(|| entry.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if id.is_empty() || !seen.insert(id.to_string()) {
                continue;
            }
            let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or(id);
            out.push(RuntimeModelOption {
                id: id.to_string(),
                label: name.to_string(),
                context_window_tokens: None,
            });
        }
        let current = option
            .get("currentValue")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        // 只有 On/Off 的开关：不当作 effort 档位（kimi K2.7 always_thinking）。
        if is_boolean_toggle_efforts(&out) {
            continue;
        }
        // 多档优先；同档数取先出现的。
        let take = match &best {
            None => true,
            Some((prev, _)) => out.len() > prev.len(),
        };
        if take {
            best = Some((out, current));
        }
    }
    best.unwrap_or((Vec::new(), None))
}

fn extract_acp_current(value: &Value) -> (Option<String>, Option<String>) {
    let models = value.get("models");
    let current_id = models
        .and_then(|m| m.get("currentModelId"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let mut current_reasoning = None;
    if let (Some(models_obj), Some(cur)) = (models, current_id.as_deref()) {
        if let Some(available) = models_obj.get("availableModels").and_then(|v| v.as_array()) {
            for model in available {
                if model.get("modelId").and_then(|v| v.as_str()) != Some(cur) {
                    continue;
                }
                let meta = model.get("_meta");
                // 首选 default=true 的 reasoningEfforts 条目；退而求其次读标量 reasoningEffort。
                if let Some(efforts) = meta
                    .and_then(|m| m.get("reasoningEfforts"))
                    .and_then(|v| v.as_array())
                {
                    for effort in efforts {
                        if effort.get("default").and_then(|v| v.as_bool()) == Some(true) {
                            current_reasoning = effort
                                .get("id")
                                .or_else(|| effort.get("value"))
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                                .map(str::to_string);
                            break;
                        }
                    }
                }
                if current_reasoning.is_none() {
                    current_reasoning = meta
                        .and_then(|m| m.get("reasoningEffort"))
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string);
                }
                break;
            }
        }
    }
    (current_id, current_reasoning)
}

pub async fn detect_acp_models(
    bin: &Path,
    args: &[&str],
    cwd: &Path,
    timeout_secs: u64,
) -> Option<AcpModelsProbe> {
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

    let mut expected_id: u64 = 1;
    let mut next_id: u64 = 2;
    let mut collected: Vec<RuntimeModelOption> = Vec::new();
    let mut default_slot = default_model_option();
    let mut seen: HashSet<String> = HashSet::new();
    let mut current_model: Option<String> = None;
    let mut current_reasoning: Option<String> = None;
    let mut reasoning_options: Vec<RuntimeModelOption> = Vec::new();
    let deadline = Duration::from_secs(timeout_secs);

    write_rpc(
        &mut stdin,
        1,
        "initialize",
        json!({
            "protocolVersion": ACP_PROTOCOL_VERSION,
            "clientCapabilities": { "terminal": false },
            "clientInfo": { "name": "kivio", "version": "external-agents" },
        }),
    )
    .await
    .ok()?;

    let started = std::time::Instant::now();
    // 收到 session/new 结果后再留一个短窗口（1.5s）收异步 session/update 推送的模型（N4）——
    // 部分 CLI 在 session/new 之后才异步推模型列表（参照 detect_acp_commands 的通知处理）。
    let mut post_window: Option<std::time::Instant> = None;
    loop {
        if started.elapsed() > deadline {
            let _ = child.start_kill();
            break;
        }
        if let Some(since) = post_window {
            if since.elapsed() > Duration::from_millis(1500) {
                let _ = child.start_kill();
                break;
            }
        }
        let line = match timeout(Duration::from_millis(200), reader.next_line()).await {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => break,
            Ok(Err(_)) => break,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        // banner / 日志等非 JSON 行跳过（对齐同文件其他 reader），别因一行噪声放弃整个探测（缺陷 3）。
        let value: Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // 异步模型推送：session/update 通知里携带的模型（N4）。
        if value.get("method").and_then(|v| v.as_str()) == Some("session/update") {
            if let Some(update) = value.get("params").and_then(|p| p.get("update")) {
                merge_acp_models(&mut collected, &mut seen, &mut default_slot, update);
                let (cm, cr) = extract_acp_current(update);
                current_model = current_model.or(cm);
                current_reasoning = current_reasoning.or(cr);
            }
            continue;
        }

        if rpc_error_message(&value).is_some() {
            if value.get("id").and_then(|v| v.as_u64()) != Some(expected_id) {
                continue;
            }
            // 匹配 id 的错误：结束探测；已收到的部分模型仍返回，否则判失败。
            let _ = child.start_kill();
            break;
        }
        if value.get("id").and_then(|v| v.as_u64()) != Some(expected_id) {
            continue;
        }
        let result = match value.get("result") {
            Some(r) => r,
            None => continue,
        };
        if expected_id == 1 {
            expected_id = next_id;
            write_rpc(
                &mut stdin,
                next_id,
                "session/new",
                build_session_new_params(cwd, &[]),
            )
            .await
            .ok()?;
            next_id += 1;
            continue;
        }
        if expected_id == 2 {
            merge_acp_models(&mut collected, &mut seen, &mut default_slot, result);
            let (cm, cr) = extract_acp_current(result);
            current_model = current_model.or(cm);
            current_reasoning = current_reasoning.or(cr);
            // configOptions 里的推理档位（kimi 的 `thinking`）——与 models._meta 那条
            // 并列，两种形态各占一家 CLI，都要认。
            let (opts, cur) = extract_acp_reasoning(result);
            if !opts.is_empty() && reasoning_options.is_empty() {
                reasoning_options = opts;
            }
            current_reasoning = current_reasoning.or(cur);
            // 不立即退出：再留 1.5s 收异步推送的模型。若此后无推送则窗口到点结束。
            post_window = Some(std::time::Instant::now());
            continue;
        }
    }

    if collected.is_empty() {
        return None;
    }
    let mut out = vec![default_slot];
    out.extend(collected);
    Some(AcpModelsProbe {
        models: out,
        current_model,
        current_reasoning,
        reasoning_options,
    })
}

fn parse_available_commands(
    update: &serde_json::Map<String, Value>,
) -> Vec<ExternalCliSlashCommand> {
    let list = update
        .get("availableCommands")
        .or_else(|| update.get("available_commands"))
        .and_then(|v| v.as_array());
    let Some(list) = list else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for raw in list {
        let Some(obj) = raw.as_object() else {
            continue;
        };
        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(name) = name else {
            continue;
        };
        let description = obj
            .get("description")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        out.push(ExternalCliSlashCommand {
            slash: format!("/{name}"),
            name: name.to_string(),
            description,
            argument_hint: None,
        });
    }
    out
}

/// Discover an ACP agent's slash commands. Mirrors `detect_acp_models`: run `initialize`
/// → `session/new`, then keep reading `session/update` *notifications* and capture the one
/// whose `sessionUpdate == "available_commands_update"` (cursor pushes this asynchronously,
/// up to ~10s after the session is created). Returns the deduped, sorted command list.
pub async fn detect_acp_commands(
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

    let mut expected_id: u64 = 1;
    let mut next_id: u64 = 2;
    let mut commands: Option<Vec<ExternalCliSlashCommand>> = None;
    let deadline = Duration::from_secs(timeout_secs);

    write_rpc(
        &mut stdin,
        1,
        "initialize",
        json!({
            "protocolVersion": ACP_PROTOCOL_VERSION,
            "clientCapabilities": { "terminal": false },
            "clientInfo": { "name": "kivio", "version": "external-agents" },
        }),
    )
    .await
    .ok()?;

    let started = std::time::Instant::now();
    loop {
        if started.elapsed() > deadline {
            let _ = child.start_kill();
            break;
        }
        let line = match timeout(Duration::from_millis(200), reader.next_line()).await {
            Ok(Ok(Some(line))) => line,
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

        // Capture the asynchronously-pushed available_commands_update notification.
        if value.get("method").and_then(|v| v.as_str()) == Some("session/update") {
            if let Some(update) = value
                .get("params")
                .and_then(|p| p.get("update"))
                .and_then(|v| v.as_object())
            {
                let session_update = update
                    .get("sessionUpdate")
                    .or_else(|| update.get("availableCommandsUpdate"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if session_update == "available_commands_update"
                    || session_update == "availableCommandsUpdate"
                    || update.contains_key("availableCommands")
                    || update.contains_key("available_commands")
                {
                    let parsed = parse_available_commands(update);
                    if !parsed.is_empty() {
                        commands = Some(parsed);
                        let _ = child.start_kill();
                        break;
                    }
                }
            }
            continue;
        }

        if rpc_error_message(&value).is_some() {
            if value.get("id").and_then(|v| v.as_u64()) != Some(expected_id) {
                continue;
            }
            let _ = child.start_kill();
            return None;
        }
        if value.get("id").and_then(|v| v.as_u64()) != Some(expected_id) {
            continue;
        }
        let result = value.get("result")?;
        if expected_id == 1 {
            expected_id = next_id;
            write_rpc(
                &mut stdin,
                next_id,
                "session/new",
                build_session_new_params(cwd, &[]),
            )
            .await
            .ok()?;
            next_id += 1;
            continue;
        }
        if expected_id == 2 {
            // session/new acknowledged; some agents include commands inline in the result.
            if let Some(update) = result.as_object() {
                let parsed = parse_available_commands(update);
                if !parsed.is_empty() {
                    commands = Some(parsed);
                    let _ = child.start_kill();
                    break;
                }
            }
            // Otherwise keep reading notifications until the agent pushes them or we time out.
            expected_id = 0; // no further responses expected
            continue;
        }
    }

    commands.map(|mut cmds| {
        cmds.sort_by(|a, b| a.name.cmp(&b.name));
        cmds.dedup_by(|a, b| a.name == b.name);
        cmds
    })
}

/// 选一个「批准」选项回给 ACP agent。
///
/// **优先挑 `allow_once`**：审批卡上的「允许一次」必须名副其实。以前优先挑
/// `approve_for_session` / `allow_always`，等于用户点「允许」就被静默升级成永久放行。
/// 「总是允许」由 Kivio 自己那张 `chat_tool_always_allow` 表兜住（后续同名工具不再弹卡，
/// 每次照样以 allow_once 回给 CLI），不需要把三态透传下来。
/// 只在 CLI 压根没给 once 选项时才退回 session/always（否则这一轮就卡死了）。
fn choose_permission_outcome(options: Option<&Value>) -> Option<String> {
    let list = options.and_then(|v| v.as_array())?;
    for item in list {
        if item.get("kind").and_then(|v| v.as_str()) == Some("allow_once") {
            if let Some(id) = item.get("optionId").and_then(|v| v.as_str()) {
                return Some(id.to_string());
            }
        }
    }
    for item in list {
        if item.get("optionId").and_then(|v| v.as_str()) == Some("approve_for_session") {
            return Some("approve_for_session".to_string());
        }
    }
    for item in list {
        if item.get("kind").and_then(|v| v.as_str()) == Some("allow_always") {
            if let Some(id) = item.get("optionId").and_then(|v| v.as_str()) {
                return Some(id.to_string());
            }
        }
    }
    None
}

/// 解析 ACP `PromptResponse.usage`（官方标记 UNSTABLE，字段缺失一律按 0，不报错）。
///
/// 四个分量**并列不重叠**，都要计入上下文占用。opencode 实测样本对账：
/// `inputTokens 11685 + outputTokens 4 + thoughtTokens 11 + cachedReadTokens 1792 = 13492`
/// 恰等于其自报的 `totalTokens`——所以 `thoughtTokens` 没有含在 `outputTokens` 里
/// （与 pi/codex 相反，那两家的 reasoning 已含在 output 内，见 `CliUsageParts::reasoning` 注释）。
///
/// 全零时返回 `None`（保留既有行为：没报就是没报，不要造出一个 0 用量覆盖真实值）。
fn format_acp_usage(usage: &Value) -> Option<crate::chat::model::ModelUsage> {
    let field = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
    let parts = CliUsageParts {
        input: field("inputTokens"),
        output: field("outputTokens"),
        cache_read: field("cachedReadTokens"),
        cache_creation: field("cachedWriteTokens"),
        reasoning: field("thoughtTokens"),
        // ACP 的 `cachedReadTokens` 与 `inputTokens` **不相交**（Anthropic 口径）：
        // opencode 实测 11685 + 4 + 11 + 1792 = 13492 = 其自报的 totalTokens，
        // 四者并列。这与 codex（cache ⊆ input）相反，别照抄那边。
        cache_included_in_input: false,
        // PromptResponse.usage 不带窗口——窗口只在 `usage_update.size` 里给。
        // 这里留 None 由 run.rs 的窗口粘滞保住先到的 usage_update.size。
        context_window: None,
    };
    if parts.input == 0
        && parts.output == 0
        && parts.cache_read == 0
        && parts.cache_creation == 0
        && parts.reasoning == 0
    {
        None
    } else {
        Some(usage_from_parts(parts))
    }
}

/// 从 `session/prompt` 的 result 里取用量，两条路依次试：
///
/// 1. `result.usage` —— ACP 官方（UNSTABLE）字段，opencode 在发。
/// 2. `result._meta` —— grok 私有位置。**grok 既不发 `usage_update`，也不填 `result.usage`**，
///    只在 `_meta` 里给（2026-07-30 本机 grok 0.2.114 实测），所以少了这一路 grok 的分子
///    恒为 0，用量条一路掉到字符估算兜底 —— 而 grok 空会话第一轮就是 14.8K（内置提示很长），
///    估算差一个数量级。
///
/// 走 `_meta` 时用它显式给的 `totalTokens` 作上下文占用，**不自己按分量累加**：grok 的
/// `cachedReadTokens` 是 `inputTokens` 的子集（实测 14870 含 128 cache，`totalTokens 14893
/// = 14870 + 23 output`），与 `format_acp_usage` 假设的 opencode「四项并列」口径相反，
/// 用那套 parser 会把 cache 双算。信它自报的 total 就绕开了整个口径分歧。
///
/// 取 `_meta` **顶层**而非 `_meta.usage`：带工具调用的一轮里顶层是**最后一次模型往返**
/// （实测 15137），`_meta.usage` 是本 prompt 内所有往返的**累计**（30113 / `numTurns: 2`）。
/// 上下文占用要前者，用累计值会随轮数虚涨。
fn usage_from_prompt_result(result: &Value) -> Option<crate::chat::model::ModelUsage> {
    if let Some(usage) = result.get("usage").and_then(format_acp_usage) {
        return Some(usage);
    }
    let meta = result.get("_meta")?;
    let field = |key: &str| meta.get(key).and_then(|v| v.as_u64());
    let total = field("totalTokens").filter(|v| *v > 0)?;
    Some(crate::chat::model::ModelUsage {
        input_tokens: field("inputTokens"),
        output_tokens: field("outputTokens"),
        total_tokens: Some(total),
        cached_input_tokens: field("cachedReadTokens").filter(|v| *v > 0),
        cache_creation_input_tokens: field("cachedWriteTokens").filter(|v| *v > 0),
        reasoning_tokens: field("reasoningTokens").filter(|v| *v > 0),
        // 窗口不在这里给（grok 在 session/new 的 `_meta.totalContextTokens`，已由模型探测读走）。
        context_window_tokens: None,
    })
}

/// 解析 ACP `session/update` 的 `usage_update` 变体（官方 RFD「Session Usage and Context
/// Status」）。字段**平铺在 `update` 下**，不嵌套在 `usage` 对象里：
///
/// ```json
/// {"sessionUpdate":"usage_update","used":13477,"size":200000,
///  "cost":{"amount":0,"currency":"USD"}}
/// ```
///
/// `used` **不是** prompt input——它是「当前上下文里现有的全部 token」（已含 cache 与历史）。
/// 之所以塞进 `input_tokens`，是因为下游 `external_agents::context::collect_external_session_usage`
/// 读的就是这个字段来当分子。不要"修正"成 prompt input 语义。
///
/// `size` 是上下文窗口总大小，走 `context_window_tokens` 当分母。
/// `cost` 暂不解析——Kivio 目前没有成本展示位。
///
/// `used`/`size` 都缺时返回 `None`（不是这个变体，或上游没给数据）。
fn parse_acp_usage_update(
    update: &serde_json::Map<String, Value>,
) -> Option<crate::chat::model::ModelUsage> {
    let used = update.get("used").and_then(|v| v.as_u64());
    let size = update
        .get("size")
        .and_then(|v| v.as_u64())
        .filter(|v| *v > 0);
    if used.is_none() && size.is_none() {
        return None;
    }
    Some(usage_from_parts(CliUsageParts {
        input: used.unwrap_or(0),
        context_window: size,
        ..Default::default()
    }))
}

fn acp_update_status(update: &serde_json::Map<String, Value>) -> Option<String> {
    update
        .get("status")
        .and_then(|v| v.as_str())
        .map(|status| status.trim().to_lowercase().replace([' ', '-'], "_"))
}

fn acp_tool_call_id(update: &serde_json::Map<String, Value>) -> Option<String> {
    update
        .get("toolCallId")
        .or_else(|| update.get("tool_call_id"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn acp_tool_name(update: &serde_json::Map<String, Value>) -> String {
    update
        .get("title")
        .or_else(|| update.get("toolName"))
        .or_else(|| update.get("name"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("tool")
        .to_string()
}

fn acp_is_terminal_success(status: &str) -> bool {
    matches!(status, "completed" | "complete" | "succeeded" | "success")
}

fn acp_is_terminal_failure(status: &str) -> bool {
    matches!(
        status,
        "failed" | "failure" | "error" | "cancelled" | "canceled"
    )
}

fn acp_result_content(update: &serde_json::Map<String, Value>) -> String {
    update
        .get("content")
        .or_else(|| update.get("output"))
        .or_else(|| update.get("result"))
        .map(|value| {
            if let Some(text) = value.as_str() {
                text.to_string()
            } else {
                value.to_string()
            }
        })
        .unwrap_or_else(|| acp_tool_name(update))
}

fn apply_acp_session_update(
    update: &serde_json::Map<String, Value>,
    emitted_tool_ids: &mut HashSet<String>,
    sink: &mut impl FnMut(UnifiedAgentEvent),
) -> bool {
    let session_update = update
        .get("sessionUpdate")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match session_update {
        "tool_call" => {
            let Some(id) = acp_tool_call_id(update) else {
                return true;
            };
            if emitted_tool_ids.insert(id.clone()) {
                sink(UnifiedAgentEvent::ToolUse {
                    id,
                    name: acp_tool_name(update),
                    input: Value::Object(update.clone()),
                });
            }
            true
        }
        "tool_call_update" => {
            let Some(id) = acp_tool_call_id(update) else {
                return true;
            };
            if !emitted_tool_ids.contains(&id) {
                emitted_tool_ids.insert(id.clone());
                sink(UnifiedAgentEvent::ToolUse {
                    id: id.clone(),
                    name: acp_tool_name(update),
                    input: Value::Object(update.clone()),
                });
            }
            if let Some(status) = acp_update_status(update) {
                if acp_is_terminal_success(&status) || acp_is_terminal_failure(&status) {
                    sink(UnifiedAgentEvent::ToolResult {
                        tool_use_id: id,
                        content: acp_result_content(update),
                        is_error: acp_is_terminal_failure(&status),
                    });
                }
            }
            true
        }
        "usage_update" => {
            // ACP 官方 usage 通道（opencode 实测在发）。一次性驱动没有游标状态，
            // 返回值无所谓；持久驱动那边会先单独匹配掉这个分支，不会走到这里。
            if let Some(usage) = parse_acp_usage_update(update) {
                sink(UnifiedAgentEvent::Usage { usage });
            }
            true
        }
        _ => false,
    }
}

/// 按消息边界维护助手正文/思考的累积游标,替代旧的全局 `emitted_text` 前缀裁剪(缺陷 2 / N7)。
///
/// 上游三种语义统一在一个分支里:
/// - 纯增量 delta(每条 chunk 是新片段)——`starts_with(current)` 除首条(current 为空)外不命中,
///   整段作为 delta 追加。
/// - 按消息累积快照(chunk 以本消息已发文本为前缀)——前缀裁剪出增量。
/// - 整轮累积快照(旧行为:快照以整轮全文为前缀)——边界事件后快照仍以旧前缀开头,`on_boundary`
///   置位的 `boundary_pending` 因 `starts_with` 命中而不触发重置,行为与旧全局前缀裁剪完全一致。
#[derive(Default)]
struct AcpTextAssembler {
    current: String,
    boundary_pending: bool,
}

impl AcpTextAssembler {
    /// 见到 tool_call / thought 等边界事件时置位。只置位,不立即清空——由下一条 chunk 的
    /// `starts_with` 检查决定是否真是新消息起点(保证整轮累积语义向后兼容)。
    fn on_boundary(&mut self) {
        self.boundary_pending = true;
    }

    /// 返回本条 chunk 应发出的增量;无新增内容时返回 `None`。
    fn push_chunk(&mut self, text: &str) -> Option<String> {
        // 边界后的第一条 chunk 若不再以当前消息累积文本为前缀,视为新消息:重置游标。
        if self.boundary_pending && !text.starts_with(self.current.as_str()) {
            self.current.clear();
        }
        self.boundary_pending = false;
        let delta = if text.starts_with(self.current.as_str()) {
            text[self.current.len()..].to_string()
        } else {
            text.to_string()
        };
        if delta.is_empty() {
            return None;
        }
        self.current.push_str(&delta);
        Some(delta)
    }
}

/// 一次 ACP turn 的去重状态:正文与思考各持一个消息级游标 + 已发工具 id 集合。
#[derive(Default)]
struct AcpUpdateState {
    text: AcpTextAssembler,
    thought: AcpTextAssembler,
    emitted_tools: HashSet<String>,
}

fn acp_update_text(update: &serde_json::Map<String, Value>) -> &str {
    update
        .get("content")
        .and_then(|c| c.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// 把一条 ACP `session/update` 映射为事件(text / thought / tool),一次性驱动
/// (`run_acp_session`)与持久驱动(`AcpSession::run_turn`)共用同一份去重逻辑。
fn acp_apply_session_update(
    update: &serde_json::Map<String, Value>,
    state: &mut AcpUpdateState,
    sink: &mut dyn FnMut(UnifiedAgentEvent),
) {
    let session_update = update
        .get("sessionUpdate")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match session_update {
        "agent_thought_chunk" => {
            // 思考块开始意味着上一条正文消息结束——为正文游标标记边界。
            state.text.on_boundary();
            let text = acp_update_text(update);
            if !text.is_empty() {
                if let Some(delta) = state.thought.push_chunk(text) {
                    sink(UnifiedAgentEvent::ThinkingDelta { delta });
                }
            }
        }
        "agent_message_chunk" => {
            // 正文块开始意味着上一条思考消息结束——为思考游标标记边界。
            state.thought.on_boundary();
            let text = acp_update_text(update);
            if !text.is_empty() {
                if let Some(delta) = state.text.push_chunk(text) {
                    sink(UnifiedAgentEvent::TextDelta { delta });
                }
            }
        }
        "usage_update" => {
            // **必须在 `_` 分支之前单独匹配**：usage 通知不是消息边界。若落进 `_`，
            // `apply_acp_session_update` 返回 true 会触发 text/thought 的 `on_boundary()`，
            // 正在流式的正文游标被重置后，后续累积快照的 starts_with 判断失效
            // → 整段正文重复发一遍。两处分发共用 `parse_acp_usage_update`（不是两份拷贝）。
            if let Some(usage) = parse_acp_usage_update(update) {
                sink(UnifiedAgentEvent::Usage { usage });
            }
        }
        _ => {
            // tool_call / tool_call_update 是消息边界:发出工具事件后,重置正文与思考游标,
            // 使其后到来的累积快照被识别为新消息起点。
            if apply_acp_session_update(update, &mut state.emitted_tools, &mut |e| sink(e)) {
                state.text.on_boundary();
                state.thought.on_boundary();
            }
        }
    }
}

/// grok 的 `retry_state` 通知 → 状态行上的一句短话（不进正文）。
///
/// **为什么必须接**：上游 503 / 429 时 grok 在**静默重试**（实测 `max_retries: 15`，退避到
/// 后面单次要等半分钟），界面上一个字都没有 —— 与 claude 的 `api_retry` 完全同一类问题，
/// 复用同一个 `StatusNote` 出口和同一套文案格式。这条通知走的是厂商私有方法
/// `_x.ai/session_notification`，不是标准 `session/update`。
///
/// 形状（grok 1.0.3 实测）：
/// `{sessionUpdate:"retry_state", type:"retrying", attempt:1, max_retries:15, reason:"API error …"}`
/// `type` 非 `retrying`（重试结束）时不发 —— 状态行是瞬态的，本轮继续流就自然被覆盖。
fn acp_retry_state_note(update: &serde_json::Map<String, Value>) -> Option<String> {
    if update.get("sessionUpdate").and_then(|v| v.as_str())? != "retry_state" {
        return None;
    }
    if update.get("type").and_then(|v| v.as_str()) != Some("retrying") {
        return None;
    }
    let attempt = update.get("attempt").and_then(|v| v.as_u64())?;
    let of_max = update
        .get("max_retries")
        .and_then(|v| v.as_u64())
        .map(|max| format!("/{max}"))
        .unwrap_or_default();
    let cause = update
        .get("reason")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        // reason 是一整句英文错误（含 body 片段），状态行放不下也没必要 —— 截到能认出
        // 「是什么故障」为止。
        .map(|s| head_chars(s, 80));
    Some(match cause {
        Some(text) => format!("retry {attempt}{of_max} · {text}"),
        None => format!("retry {attempt}{of_max}"),
    })
}

/// 取前 n 个字符（按字符而非字节，避免切裂多字节）。
fn head_chars(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let head: String = s.chars().take(n).collect();
    format!("{head}…")
}

// ===========================================================================================
// Persistent ACP session (Phase 2): keep the agent process alive across turns. Reuses the
// same `apply_acp_session_update` mapping + permission/usage helpers as the one-shot driver.
// ===========================================================================================

/// A live ACP connection: one `session/new` (or `session/load`) + `set_model`, then many
/// `session/prompt` turns over the same process. Owned exclusively by its actor task.
pub struct AcpSession {
    child: Child,
    stdin: ChildStdin,
    reader: Lines<BufReader<ChildStdout>>,
    session_id: String,
    next_id: u64,
    /// Ring-buffered stderr tail (N1): drained for the process lifetime, joined on close / error
    /// so a silent handshake/turn failure surfaces the CLI's stderr.
    stderr_tail: tokio::task::JoinHandle<String>,
    /// The `configOptions` id for the model selector, if this agent exposes one (used by
    /// `session/set_config_option`); `None` → fall back to `session/set_model`.
    model_config_id: Option<String>,
    /// The `configOptions` id for reasoning/thinking level, if any. `None` (e.g. grok, whose
    /// reasoning is a launch flag) → a reasoning change forces a reconnect instead.
    reasoning_config_id: Option<String>,
    /// Normalized model/reasoning the live session currently reflects, for mid-turn change
    /// detection (N3). `None` = agent default.
    current_model: Option<String>,
    current_reasoning: Option<String>,
}

/// Sentinel returned by `run_turn` when a config change (reasoning without a config option) can
/// only take effect by relaunching the CLI with new args — `run_persistent_turn` reconnects fresh.
pub const NEEDS_RECONNECT: &str = "__needs_reconnect__";

fn normalize_opt(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "default")
        .map(str::to_string)
}

/// Scan an ACP `session/new` result's `configOptions` for the model + reasoning selector ids.
fn find_config_ids(result: &Value) -> (Option<String>, Option<String>) {
    let mut model_id = None;
    let mut reasoning_id = None;
    if let Some(config_options) = result.get("configOptions").and_then(|v| v.as_array()) {
        for raw in config_options {
            let Some(option) = raw.as_object() else {
                continue;
            };
            let id = option.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let category = option
                .get("category")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let id_l = id.to_lowercase();
            if model_id.is_none() && (id == "model" || category == "model") {
                model_id = Some(id.to_string());
            } else if reasoning_id.is_none()
                && (id_l.contains("reasoning")
                    || id_l.contains("thought")
                    || id_l.contains("thinking")
                    || id_l.contains("effort")
                    || category == "reasoning"
                    || category == "thought")
            {
                reasoning_id = Some(id.to_string());
            }
        }
    }
    (model_id, reasoning_id)
}

/// Build the ACP request to switch the session model: `set_config_option` when the agent exposes a
/// model config id, else `set_model`. Pure so the per-turn model-switch (N3) is unit-testable.
fn model_set_rpc(
    session_id: &str,
    model_config_id: Option<&str>,
    chosen: &str,
) -> (&'static str, Value) {
    match model_config_id {
        Some(cfg) => (
            "session/set_config_option",
            json!({ "sessionId": session_id, "configId": cfg, "value": chosen }),
        ),
        None => (
            "session/set_model",
            json!({ "sessionId": session_id, "modelId": chosen }),
        ),
    }
}

/// What a mid-turn reasoning change needs (N3): nothing, an in-session `set_config_option`, or a
/// full relaunch (agents whose reasoning is a launch flag, e.g. grok).
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReasoningAction {
    NoChange,
    SetConfig { config_id: String, value: String },
    Reconnect,
}

fn reasoning_action(
    current: &Option<String>,
    desired: &Option<String>,
    config_id: &Option<String>,
) -> ReasoningAction {
    if current == desired {
        return ReasoningAction::NoChange;
    }
    match config_id {
        Some(cfg) => ReasoningAction::SetConfig {
            config_id: cfg.clone(),
            value: desired.clone().unwrap_or_else(|| "default".to_string()),
        },
        None => ReasoningAction::Reconnect,
    }
}

impl AcpSession {
    #[allow(clippy::too_many_arguments)]
    pub async fn connect(
        resolved_bin: &Path,
        args: &[String],
        cwd: &Path,
        model: Option<&str>,
        reasoning: Option<&str>,
        mcp_servers: &[AcpMcpServer],
        resume_session: Option<&str>,
    ) -> Result<Self, String> {
        let mut child = crate::external_agents::spawn::cli_command(resolved_bin)
            .args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .no_console_window()
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("spawn: {e}"))?;
        // N1: drain stderr for the process lifetime.
        let stderr_tail = crate::external_agents::spawn::spawn_stderr_tail(child.stderr.take());
        let mut stdin = match child.stdin.take() {
            Some(s) => s,
            None => {
                let tail = join_tail(&mut child, stderr_tail).await;
                return Err(fold_stderr("spawn: stdin unavailable".to_string(), &tail));
            }
        };
        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                let tail = join_tail(&mut child, stderr_tail).await;
                return Err(fold_stderr("spawn: stdout unavailable".to_string(), &tail));
            }
        };
        let mut reader = BufReader::new(stdout).lines();

        // Fallible handshake, isolated so an error path can kill the child and fold in stderr.
        let handshake = async {
            write_rpc(
                &mut stdin,
                1,
                "initialize",
                json!({
                    "protocolVersion": ACP_PROTOCOL_VERSION,
                    "clientCapabilities": { "terminal": false },
                    "clientInfo": { "name": "kivio", "version": "external-agents" },
                }),
            )
            .await
            .map_err(|e| format!("initialize: {e}"))?;
            acp_read_until_id(&mut reader, &mut stdin, 1, ACP_INITIALIZE_TIMEOUT)
                .await
                .map_err(|e| format!("initialize: {e}"))?;

            // session/new for a fresh session, session/load to resume a prior one.
            let mut next_id: u64 = 2;
            let (method, params) = match resume_session.filter(|s| !s.is_empty()) {
                Some(sid) => {
                    let mut p = build_session_new_params(cwd, mcp_servers);
                    p["sessionId"] = json!(sid);
                    ("session/load", p)
                }
                None => ("session/new", build_session_new_params(cwd, mcp_servers)),
            };
            write_rpc(&mut stdin, next_id, method, params)
                .await
                .map_err(|e| format!("session-new: {e}"))?;
            let result =
                acp_read_until_id(&mut reader, &mut stdin, next_id, ACP_SESSION_NEW_TIMEOUT)
                    .await
                    .map_err(|e| format!("session-new: {e}"))?;
            next_id += 1;

            let session_id = match resume_session.filter(|s| !s.is_empty()) {
                Some(sid) => sid.to_string(),
                None => result
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .ok_or_else(|| "session-new: invalid session/new response".to_string())?,
            };

            let (model_config_id, reasoning_config_id) = find_config_ids(&result);

            // Optional model selection (set_config_option / set_model), mirroring run_acp_session.
            let chosen_model = normalize_opt(model);
            if let Some(chosen) = chosen_model.as_deref() {
                let (set_method, set_params) =
                    model_set_rpc(&session_id, model_config_id.as_deref(), chosen);
                write_rpc(&mut stdin, next_id, set_method, set_params)
                    .await
                    .map_err(|e| format!("session-new: {e}"))?;
                // Best-effort: wait for the ack but don't fail the session if the agent ignores it.
                let _ =
                    acp_read_until_id(&mut reader, &mut stdin, next_id, Duration::from_secs(10))
                        .await;
                next_id += 1;
            }

            Ok::<_, String>((
                session_id,
                next_id,
                model_config_id,
                reasoning_config_id,
                chosen_model,
            ))
        }
        .await;

        match handshake {
            Ok((session_id, next_id, model_config_id, reasoning_config_id, current_model)) => {
                Ok(Self {
                    child,
                    stdin,
                    reader,
                    session_id,
                    next_id,
                    stderr_tail,
                    model_config_id,
                    reasoning_config_id,
                    current_model,
                    current_reasoning: normalize_opt(reasoning),
                })
            }
            Err(msg) => {
                let tail = join_tail(&mut child, stderr_tail).await;
                Err(fold_stderr(msg, &tail))
            }
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 常驻子进程的 pid。只作为注册表元数据（诊断 / 「两轮是不是同一个进程」），
    /// 关停一律走 actor 的 `Close`，绝不按 pid 杀。
    pub fn child_pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Run one prompt turn over the live session. Emits events into `events`; an incoming
    /// `Cancel` on `control` sends `session/cancel` without killing the process.
    ///
    /// `model`/`reasoning` are re-applied per turn (N3): a model change goes through
    /// `session/set_config_option` / `session/set_model` in-session; a reasoning change with no
    /// config option (grok's launch-flag reasoning) returns `Err(NEEDS_RECONNECT)` so the caller
    /// relaunches the CLI with new args.
    pub async fn run_turn(
        &mut self,
        prompt: &str,
        model: Option<&str>,
        reasoning: Option<&str>,
        images: &[crate::external_agents::attachments::ImageBlock],
        events: &mpsc::Sender<UnifiedAgentEvent>,
        control: &mut mpsc::Receiver<SessionCommand>,
    ) -> Result<(), String> {
        // Apply mid-session config changes before sending the prompt (N3).
        let desired_reasoning = normalize_opt(reasoning);
        match reasoning_action(
            &self.current_reasoning,
            &desired_reasoning,
            &self.reasoning_config_id,
        ) {
            ReasoningAction::NoChange => {}
            ReasoningAction::SetConfig { config_id, value } => {
                let id = self.next_id;
                self.next_id += 1;
                let _ = write_rpc(
                    &mut self.stdin,
                    id,
                    "session/set_config_option",
                    json!({ "sessionId": self.session_id, "configId": config_id, "value": value }),
                )
                .await;
                let _ = acp_read_until_id(
                    &mut self.reader,
                    &mut self.stdin,
                    id,
                    Duration::from_secs(10),
                )
                .await;
                self.current_reasoning = desired_reasoning;
            }
            // Reasoning is a launch flag (grok) — only a relaunch with new args applies it.
            ReasoningAction::Reconnect => return Err(NEEDS_RECONNECT.to_string()),
        }

        let desired_model = normalize_opt(model);
        if desired_model != self.current_model {
            if let Some(chosen) = desired_model.as_deref() {
                let id = self.next_id;
                self.next_id += 1;
                let (method, params) =
                    model_set_rpc(&self.session_id, self.model_config_id.as_deref(), chosen);
                let _ = write_rpc(&mut self.stdin, id, method, params).await;
                let _ = acp_read_until_id(
                    &mut self.reader,
                    &mut self.stdin,
                    id,
                    Duration::from_secs(10),
                )
                .await;
            }
            self.current_model = desired_model;
        }

        let prompt_id = self.next_id;
        self.next_id += 1;
        write_rpc(
            &mut self.stdin,
            prompt_id,
            "session/prompt",
            json!({
                "sessionId": self.session_id,
                "prompt": acp_prompt_blocks(prompt, images),
            }),
        )
        .await?;

        let mut update_state = AcpUpdateState::default();

        loop {
            match control.try_recv() {
                Ok(SessionCommand::Cancel) => {
                    let cid = self.next_id;
                    self.next_id += 1;
                    let _ = write_rpc(
                        &mut self.stdin,
                        cid,
                        "session/cancel",
                        json!({ "sessionId": self.session_id }),
                    )
                    .await;
                    return Err("cancelled".to_string());
                }
                Ok(SessionCommand::Close) => return Err("closed".to_string()),
                // ACP 没有「往在飞的 prompt 追加输入」的原语 —— 全部动作只有 `session/prompt`
                // 与 `session/cancel`。回 false，这条消息留在前端队列里等轮末自动发送。
                Ok(SessionCommand::Steer { accepted, .. }) => {
                    let _ = accepted.send(false);
                }
                Ok(SessionCommand::RunTurn { done, .. }) => {
                    let _ = done.send(Err("session busy".to_string()));
                }
                // ACP 无后台任务协议（stop_task 是 claude 专属），忽略。
                Ok(SessionCommand::StopTask { .. }) => {}
                Err(mpsc::error::TryRecvError::Empty) => {}
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    return Err("control channel closed".to_string())
                }
            }

            let line = match timeout(Duration::from_millis(200), self.reader.next_line()).await {
                Ok(Ok(Some(l))) => l,
                Ok(Ok(None)) => return Err("ACP session exited mid-turn".to_string()),
                Ok(Err(e)) => return Err(e.to_string()),
                Err(_) => continue,
            };
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = match serde_json::from_str(line.trim()) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if let Some(method) = value.get("method").and_then(|v| v.as_str()) {
                if method == "session/request_permission" {
                    let option_id = choose_permission_outcome(
                        value.get("params").and_then(|p| p.get("options")),
                    );
                    if let (Some(id), Some(option_id)) = (value.get("id"), option_id) {
                        write_rpc_result(
                            &mut self.stdin,
                            id,
                            json!({ "outcome": { "outcome": "selected", "optionId": option_id } }),
                        )
                        .await?;
                    }
                    continue;
                }
                if method == "session/update" {
                    if let Some(update) = value
                        .get("params")
                        .and_then(|p| p.get("update"))
                        .and_then(|v| v.as_object())
                    {
                        let mut buf: Vec<UnifiedAgentEvent> = Vec::new();
                        acp_apply_session_update(update, &mut update_state, &mut |e| buf.push(e));
                        for e in buf {
                            let _ = events.send(e).await;
                        }
                    }
                    continue;
                }
                // 厂商私有通知。目前只取 grok 的 `retry_state`：上游 503/429 的静默重试是
                // 「怎么卡住了」的头号成因，不接的话界面在整个重试期（可达数分钟）毫无输出。
                // 其余 `_x.ai/*`（mcp 进度 / 队列 / 会话列表变更）与本轮回答无关，继续忽略。
                if method == "_x.ai/session_notification" {
                    if let Some(update) = value
                        .get("params")
                        .and_then(|p| p.get("update"))
                        .and_then(|v| v.as_object())
                    {
                        if let Some(text) = acp_retry_state_note(update) {
                            let _ = events.send(UnifiedAgentEvent::StatusNote { text }).await;
                        }
                    }
                    continue;
                }
                continue;
            }

            if let Some(err) = rpc_error_message(&value) {
                if value.get("id").and_then(|v| v.as_u64()) == Some(prompt_id) {
                    return Err(err);
                }
                continue;
            }

            if value.get("id").and_then(|v| v.as_u64()) == Some(prompt_id) {
                if let Some(usage) = value.get("result").and_then(usage_from_prompt_result) {
                    let _ = events.send(UnifiedAgentEvent::Usage { usage }).await;
                }
                return Ok(());
            }
        }
    }

    pub async fn close(mut self) {
        let _ = self.stdin.shutdown().await;
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
        // Child is dead → its stderr hit EOF → the drain task finishes; join it so the task ends.
        let _ = self.stderr_tail.await;
    }
}

/// Read ACP JSON-RPC lines until the response for `target_id`, auto-answering permission
/// requests and skipping notifications.
async fn acp_read_until_id(
    reader: &mut Lines<BufReader<ChildStdout>>,
    stdin: &mut ChildStdin,
    target_id: u64,
    overall: Duration,
) -> Result<Value, String> {
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > overall {
            return Err("ACP handshake timeout".to_string());
        }
        let line = match timeout(Duration::from_millis(200), reader.next_line()).await {
            Ok(Ok(Some(l))) => l,
            Ok(Ok(None)) => return Err("ACP agent exited during handshake".to_string()),
            Ok(Err(e)) => return Err(e.to_string()),
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(method) = value.get("method").and_then(|v| v.as_str()) {
            if method == "session/request_permission" {
                let option_id =
                    choose_permission_outcome(value.get("params").and_then(|p| p.get("options")));
                if let (Some(id), Some(option_id)) = (value.get("id"), option_id) {
                    write_rpc_result(
                        stdin,
                        id,
                        json!({ "outcome": { "outcome": "selected", "optionId": option_id } }),
                    )
                    .await?;
                }
            }
            continue; // notification or handled request
        }
        if let Some(err) = rpc_error_message(&value) {
            if value.get("id").and_then(|v| v.as_u64()) == Some(target_id) {
                return Err(err);
            }
            continue;
        }
        if value.get("id").and_then(|v| v.as_u64()) == Some(target_id) {
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

/// Build the ACP `session/prompt` content array: text block first, then a native image block
/// (`{type:"image", data, mimeType}`) per attached image. Empty `images` → just the text block.
fn acp_prompt_blocks(
    prompt: &str,
    images: &[crate::external_agents::attachments::ImageBlock],
) -> Vec<serde_json::Value> {
    let mut blocks = vec![json!({ "type": "text", "text": prompt })];
    for img in images {
        blocks.push(json!({
            "type": "image",
            "data": img.data_base64,
            "mimeType": img.mime,
        }));
    }
    blocks
}

/// Spawn the actor task owning a connected ACP session.
pub fn spawn_acp_session_actor(mut session: AcpSession) -> mpsc::Sender<SessionCommand> {
    let (tx, mut rx) = mpsc::channel::<SessionCommand>(8);
    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                SessionCommand::RunTurn {
                    prompt,
                    model,
                    reasoning,
                    images,
                    events,
                    done,
                    // ACP 侧还没有权限审批（目前只有 claude 走 stdio 控制通道），忽略即可 ——
                    // 通道从来不会被建起来（`run.rs::turn_asks_for_permission` 只对带
                    // `--permission-prompt-tool` 的 argv 为真，那是 claude 专属 flag）。
                    approvals: _,
                    extra_writable_roots: _,
                } => {
                    // Invariant (A4): `run_turn` sends all its `events` before returning, and mpsc
                    // preserves order, so every event is already queued when `done` fires — the
                    // caller's post-`done` `try_recv` drain sees them all. Keep `done.send` LAST.
                    let result = session
                        .run_turn(
                            &prompt,
                            model.as_deref(),
                            reasoning.as_deref(),
                            &images,
                            &events,
                            &mut rx,
                        )
                        .await;
                    let _ = done.send(result);
                }
                // 轮次之间没有可注入的对象：回 false 让前端把这条留在队列里、
                // 轮末按普通消息发出去（绝不静默吞掉）。
                SessionCommand::Steer { accepted, .. } => {
                    let _ = accepted.send(false);
                }
                SessionCommand::Cancel => {}
                // ACP 无后台任务协议，忽略。
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
    use super::*;

    /// grok 上游 503 时的静默重试必须变成一行可见状态。样本取自本机 grok 1.0.3
    /// `_x.ai/session_notification` 的原样 update。
    #[test]
    fn retry_state_becomes_status_note() {
        let update = json!({
            "sessionUpdate": "retry_state",
            "type": "retrying",
            "attempt": 3,
            "max_retries": 15,
            "reason": "API error (status 503 Service Unavailable): api_error: Service temporarily unavailable"
        });
        let note = acp_retry_state_note(update.as_object().unwrap()).expect("retry note");
        assert!(note.starts_with("retry 3/15 · "), "got: {note}");
        assert!(note.contains("503"), "原因要能看出是什么故障: {note}");
    }

    /// 重试结束（非 `retrying`）不发状态行；别的 update 也不能误命中这条路。
    #[test]
    fn retry_state_note_ignores_non_retrying_and_other_updates() {
        let finished = json!({ "sessionUpdate": "retry_state", "type": "succeeded", "attempt": 4 });
        assert!(acp_retry_state_note(finished.as_object().unwrap()).is_none());
        let other = json!({ "sessionUpdate": "model_changed", "model_id": "grok-4.6" });
        assert!(acp_retry_state_note(other.as_object().unwrap()).is_none());
    }

    /// kimi 的 ACP 确实暴露推理档位——`configOptions` 里 `id="thinking"` /
    /// `category="thought_level"`。此前发现侧完全没读，胶囊上没有档位可选。
    /// 样本为本机 `kimi acp` 的 session/new 原样输出。
    #[test]
    fn reasoning_options_come_from_config_options() {
        let result = json!({
            "sessionId": "s1",
            "configOptions": [
                { "type": "select", "id": "model", "category": "model",
                  "currentValue": "kimi-code/k3-256k",
                  "options": [{ "value": "kimi-code/k3", "name": "K3" }] },
                { "type": "select", "id": "thinking", "category": "thought_level",
                  "currentValue": "high",
                  "options": [
                    { "value": "low", "name": "Low" },
                    { "value": "high", "name": "High" },
                    { "value": "max", "name": "Max" }
                  ] },
                { "type": "select", "id": "mode", "category": "mode",
                  "currentValue": "default",
                  "options": [{ "value": "default", "name": "Default" }] }
            ]
        });
        let (options, current) = extract_acp_reasoning(&result);
        assert_eq!(
            options.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(),
            vec!["low", "high", "max"]
        );
        assert_eq!(options[1].label, "High");
        assert_eq!(current.as_deref(), Some("high"));
    }

    /// kimi always_thinking 模型（K2.7 Coding）ACP 只给 `options:[{on,On}]`——不能当 effort 显示。
    #[test]
    fn reasoning_options_skip_boolean_on_only_toggle() {
        let result = json!({
            "configOptions": [
                { "type": "select", "id": "thinking", "category": "thought_level",
                  "currentValue": "on",
                  "options": [{ "value": "on", "name": "On" }] }
            ]
        });
        let (options, current) = extract_acp_reasoning(&result);
        assert!(
            options.is_empty(),
            "On-only 不应成为 effort 档位: {options:?}"
        );
        assert!(current.is_none(), "On 当前值也不该回填: {current:?}");
    }

    #[test]
    fn reasoning_options_are_empty_when_the_cli_exposes_none() {
        // cursor/grok 那种只给 model+mode 的形态：不得凭空造出档位。
        let result = json!({
            "configOptions": [
                { "id": "model", "category": "model", "options": [] },
                { "id": "mode", "category": "mode", "options": [] }
            ]
        });
        let (options, current) = extract_acp_reasoning(&result);
        assert!(options.is_empty(), "不该造出档位：{options:?}");
        assert!(current.is_none());
    }

    #[test]
    fn reasoning_options_survive_a_missing_options_array() {
        // 结构变形（有 currentValue 但没 options 列表）时不 panic，当前值仍要能读出来。
        let result = json!({
            "configOptions": [
                { "id": "effort", "category": "reasoning", "currentValue": "medium" }
            ]
        });
        let (options, current) = extract_acp_reasoning(&result);
        assert!(options.is_empty());
        assert_eq!(current.as_deref(), Some("medium"));
    }

    #[test]
    fn acp_models_skip_banner_and_parse_json() {
        // 首行 banner（非 JSON）后跟 session/new 结果，仍应解析出模型。
        let lines = [
            "cursor-agent v1.2.3 starting…",
            "",
            r#"{"id":2,"result":{"models":{"availableModels":[{"modelId":"gpt-5","name":"GPT-5"},{"modelId":"o3"}]}}}"#,
        ];
        let models = collect_acp_models_from_lines(&lines).expect("models parsed past banner");
        assert!(models.iter().any(|m| m.id == "gpt-5"));
        assert!(models.iter().any(|m| m.id == "o3"));
        assert_eq!(models[0].id, "default");
    }

    #[test]
    fn acp_models_merge_async_session_update_push() {
        // session/new 结果给出一个模型，随后异步 session/update 推送更多模型 → 合并去重。
        let lines = [
            r#"{"id":2,"result":{"models":{"availableModels":[{"modelId":"gpt-5"}]}}}"#,
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"configOptions":[{"id":"model","options":[{"value":"claude-sonnet","name":"Claude Sonnet"},{"value":"gpt-5"}]}]}}}"#,
        ];
        let models = collect_acp_models_from_lines(&lines).expect("merged models");
        assert!(models.iter().any(|m| m.id == "gpt-5"));
        assert!(models.iter().any(|m| m.id == "claude-sonnet"));
        // gpt-5 出现在两处，去重后仅一次。
        assert_eq!(models.iter().filter(|m| m.id == "gpt-5").count(), 1);
    }

    #[test]
    fn acp_models_default_slot_enriched_by_current_model_id() {
        // grok 形态：session/new 结果带 models.currentModelId + availableModels（含 _meta 窗口）。
        // default 占位应富化为 "Default (Grok 4.5)" 并继承窗口，真实模型进列表。
        let lines = [
            r#"{"id":2,"result":{"sessionId":"s1","models":{"currentModelId":"grok-4.5","availableModels":[{"modelId":"grok-4.5","name":"Grok 4.5","_meta":{"totalContextTokens":500000}}]}}}"#,
        ];
        let models = collect_acp_models_from_lines(&lines).expect("models");
        assert_eq!(models[0].id, "default");
        assert_eq!(models[0].label, "Default (Grok 4.5)");
        assert_eq!(models[0].context_window_tokens, Some(500_000));
        let real = models
            .iter()
            .find(|m| m.id == "grok-4.5")
            .expect("real model");
        assert_eq!(real.context_window_tokens, Some(500_000));
    }

    #[test]
    fn acp_models_none_when_no_models_present() {
        let lines = [
            "banner line",
            r#"{"id":1,"result":{}}"#,
            r#"{"id":2,"result":{}}"#,
        ];
        assert!(collect_acp_models_from_lines(&lines).is_none());
    }

    #[test]
    fn acp_prompt_blocks_text_only_when_no_images() {
        let blocks = acp_prompt_blocks("hello", &[]);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0], json!({ "type": "text", "text": "hello" }));
    }

    #[test]
    fn acp_prompt_blocks_appends_image_block() {
        let img = crate::external_agents::attachments::ImageBlock {
            data_base64: "AAAA".to_string(),
            mime: "image/png".to_string(),
            path: std::path::PathBuf::from("/tmp/a.png"),
        };
        let blocks = acp_prompt_blocks("look", std::slice::from_ref(&img));
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], json!("text"));
        assert_eq!(
            blocks[1],
            json!({ "type": "image", "data": "AAAA", "mimeType": "image/png" })
        );
    }

    /// Live cross-turn continuity over ACP: connect once to `cursor-agent acp`, run two prompt
    /// turns on the SAME process, and confirm turn 2 recalls a fact from turn 1 — proving the ACP
    /// session persists between turns (Phase 2). Requires a logged-in `cursor-agent` + network.
    #[tokio::test]
    #[ignore = "requires live cursor-agent login + network"]
    async fn acp_persistent_session_remembers_across_turns() {
        use crate::external_agents::session::live::SessionCommand;
        use tokio::sync::{mpsc, oneshot};

        let bin = which_bin("cursor-agent").expect("cursor-agent on PATH");
        let cwd = std::env::temp_dir();
        let session = AcpSession::connect(&bin, &["acp".to_string()], &cwd, None, None, &[], None)
            .await
            .expect("connect cursor-agent acp");
        let sid = session.session_id().to_string();
        assert!(!sid.is_empty());
        let control = spawn_acp_session_actor(session);

        async fn one_turn(control: &mpsc::Sender<SessionCommand>, prompt: &str) -> String {
            let (etx, mut erx) = mpsc::channel::<UnifiedAgentEvent>(64);
            let (dtx, drx) = oneshot::channel();
            control
                .send(SessionCommand::RunTurn {
                    prompt: prompt.to_string(),
                    model: None,
                    reasoning: None,
                    images: vec![],
                    extra_writable_roots: vec![],
                    events: etx,
                    done: dtx,
                    approvals: None,
                })
                .await
                .unwrap();
            let mut text = String::new();
            let mut drx = drx;
            loop {
                tokio::select! {
                    biased;
                    r = &mut drx => {
                        while let Ok(e) = erx.try_recv() {
                            if let UnifiedAgentEvent::TextDelta { delta } = e { text.push_str(&delta); }
                        }
                        r.unwrap().unwrap();
                        break;
                    }
                    ev = erx.recv() => {
                        if let Some(UnifiedAgentEvent::TextDelta { delta }) = ev { text.push_str(&delta); }
                    }
                }
            }
            text
        }

        let _t1 = one_turn(&control, "Remember this secret number: 42. Just reply OK.").await;
        let t2 = one_turn(
            &control,
            "What was the secret number I just gave you? Reply with only the digits.",
        )
        .await;
        eprintln!("acp turn2 reply: {t2:?}");
        assert!(
            t2.contains("42"),
            "turn 2 should recall 42 from turn 1, got: {t2:?}"
        );
        let _ = control.send(SessionCommand::Close).await;
    }

    fn which_bin(name: &str) -> Option<std::path::PathBuf> {
        let out = std::process::Command::new("which")
            .arg(name)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if p.is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(p))
        }
    }

    #[test]
    fn apply_acp_session_update_emits_tool_use_and_result() {
        let started = serde_json::Map::from_iter([
            ("sessionUpdate".to_string(), json!("tool_call")),
            ("toolCallId".to_string(), json!("acp-1")),
            ("title".to_string(), json!("Write")),
        ]);
        let finished = serde_json::Map::from_iter([
            ("sessionUpdate".to_string(), json!("tool_call_update")),
            ("toolCallId".to_string(), json!("acp-1")),
            ("title".to_string(), json!("Write")),
            ("status".to_string(), json!("completed")),
            ("content".to_string(), json!("done")),
        ]);
        let mut emitted = HashSet::new();
        let mut events = Vec::new();
        assert!(apply_acp_session_update(
            &started,
            &mut emitted,
            &mut |event| {
                events.push(event);
            }
        ));
        assert!(apply_acp_session_update(
            &finished,
            &mut emitted,
            &mut |event| {
                events.push(event);
            }
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            UnifiedAgentEvent::ToolUse { id, name, .. } if id == "acp-1" && name == "Write"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            UnifiedAgentEvent::ToolResult { tool_use_id, content, is_error }
                if tool_use_id == "acp-1" && content == "done" && !*is_error
        )));
    }

    // ---- AcpTextAssembler / shared update-dedup (Step 2) ----

    fn msg_chunk(text: &str) -> serde_json::Map<String, Value> {
        serde_json::Map::from_iter([
            ("sessionUpdate".to_string(), json!("agent_message_chunk")),
            ("content".to_string(), json!({ "text": text })),
        ])
    }

    fn thought_chunk(text: &str) -> serde_json::Map<String, Value> {
        serde_json::Map::from_iter([
            ("sessionUpdate".to_string(), json!("agent_thought_chunk")),
            ("content".to_string(), json!({ "text": text })),
        ])
    }

    fn tool_call_update(id: &str) -> serde_json::Map<String, Value> {
        serde_json::Map::from_iter([
            ("sessionUpdate".to_string(), json!("tool_call")),
            ("toolCallId".to_string(), json!(id)),
            ("title".to_string(), json!("Write")),
        ])
    }

    fn run_updates(updates: &[serde_json::Map<String, Value>]) -> (String, Vec<UnifiedAgentEvent>) {
        let mut state = AcpUpdateState::default();
        let mut events = Vec::new();
        for u in updates {
            acp_apply_session_update(u, &mut state, &mut |e| events.push(e));
        }
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                UnifiedAgentEvent::TextDelta { delta } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        (text, events)
    }

    #[test]
    fn assembler_passes_through_incremental_deltas() {
        let mut a = AcpTextAssembler::default();
        assert_eq!(a.push_chunk("Hello"), Some("Hello".to_string()));
        assert_eq!(a.push_chunk(" world"), Some(" world".to_string()));
    }

    #[test]
    fn assembler_trims_per_message_accumulated_snapshots() {
        let mut a = AcpTextAssembler::default();
        assert_eq!(a.push_chunk("Hel"), Some("Hel".to_string()));
        assert_eq!(a.push_chunk("Hello"), Some("lo".to_string()));
        // 重复快照无新增内容。
        assert_eq!(a.push_chunk("Hello"), None);
    }

    #[test]
    fn assembler_resets_on_boundary_for_new_message() {
        let mut a = AcpTextAssembler::default();
        assert_eq!(a.push_chunk("Hello"), Some("Hello".to_string()));
        a.on_boundary();
        // 新消息累积快照,不以旧全文为前缀 → 视为新消息重置游标。
        assert_eq!(a.push_chunk("Bye"), Some("Bye".to_string()));
        assert_eq!(a.push_chunk("Byebye"), Some("bye".to_string()));
    }

    #[test]
    fn assembler_whole_turn_snapshot_stays_backward_compatible() {
        let mut a = AcpTextAssembler::default();
        assert_eq!(a.push_chunk("Hello"), Some("Hello".to_string()));
        a.on_boundary();
        // 整轮累积语义:边界后的快照仍以旧全文开头 → 不重置,只裁出增量(不重不漏)。
        assert_eq!(a.push_chunk("Hello world"), Some(" world".to_string()));
    }

    #[test]
    fn driver_incremental_no_duplication() {
        let (text, _) = run_updates(&[msg_chunk("Hello"), msg_chunk(" there")]);
        assert_eq!(text, "Hello there");
    }

    #[test]
    fn driver_per_message_snapshots_with_tool_call_no_dup() {
        // msg1 按消息累积 → tool_call(边界)→ msg2 按消息累积。
        let (text, events) = run_updates(&[
            msg_chunk("Loo"),
            msg_chunk("Looking"),
            tool_call_update("t1"),
            msg_chunk("Don"),
            msg_chunk("Done"),
        ]);
        assert_eq!(text, "LookingDone");
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, UnifiedAgentEvent::ToolUse { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn driver_whole_turn_snapshots_with_tool_call_no_dup_no_loss() {
        // 整轮累积:tool_call 后的快照仍以前一段正文为前缀,只应发出新尾巴。
        let (text, _) = run_updates(&[
            msg_chunk("Looking"),
            tool_call_update("t1"),
            msg_chunk("LookingDone"),
        ]);
        assert_eq!(text, "LookingDone");
    }

    #[test]
    fn driver_thought_then_message_are_separate_streams() {
        // 思考块与正文块互为边界,各自独立累积,互不裁剪对方内容。
        let (text, events) = run_updates(&[thought_chunk("plan"), msg_chunk("plan answer")]);
        // 正文以 "plan answer" 起头,思考游标不影响正文;正文应完整发出。
        assert_eq!(text, "plan answer");
        assert!(events
            .iter()
            .any(|e| matches!(e, UnifiedAgentEvent::ThinkingDelta { delta } if delta == "plan")));
    }

    // ---- ACP usage：usage_update（官方 RFD）与 PromptResponse.usage ----

    fn usage_update(fields: Value) -> serde_json::Map<String, Value> {
        let mut map =
            serde_json::Map::from_iter([("sessionUpdate".to_string(), json!("usage_update"))]);
        if let Some(obj) = fields.as_object() {
            for (k, v) in obj {
                map.insert(k.clone(), v.clone());
            }
        }
        map
    }

    #[test]
    fn usage_update_parses_flat_used_and_size() {
        // opencode 本机实测原文（字段平铺在 update 下，不嵌套在 usage 对象里）。
        let update = usage_update(json!({
            "used": 13477,
            "size": 200000,
            "cost": { "amount": 0, "currency": "USD" }
        }));
        let usage = parse_acp_usage_update(&update).expect("usage_update 应被解析");
        // `used` 是「上下文里现有的全部 token」，落在 input_tokens（下游分子读这里）。
        assert_eq!(usage.input_tokens, Some(13_477));
        assert_eq!(usage.context_window_tokens, Some(200_000));
    }

    #[test]
    fn usage_update_tolerates_missing_fields() {
        let only_used = usage_update(json!({ "used": 42 }));
        let parsed = parse_acp_usage_update(&only_used).expect("只有 used 也该解析");
        assert_eq!(parsed.input_tokens, Some(42));
        assert_eq!(parsed.context_window_tokens, None);

        let only_size = usage_update(json!({ "size": 262144 }));
        let parsed = parse_acp_usage_update(&only_size).expect("只有 size 也该解析");
        assert_eq!(parsed.input_tokens, Some(0));
        assert_eq!(parsed.context_window_tokens, Some(262_144));

        // 两个都缺 → None，不 panic。
        assert!(parse_acp_usage_update(&usage_update(json!({}))).is_none());
        // 非数值/负数等异常形态也不得 panic。
        assert!(
            parse_acp_usage_update(&usage_update(json!({ "used": "many", "size": -1 }))).is_none()
        );
    }

    #[test]
    fn driver_emits_usage_event_for_usage_update() {
        let (_, events) = run_updates(&[usage_update(json!({ "used": 13477, "size": 200000 }))]);
        assert!(events.iter().any(|e| matches!(
            e,
            UnifiedAgentEvent::Usage { usage }
                if usage.input_tokens == Some(13_477)
                    && usage.context_window_tokens == Some(200_000)
        )));
    }

    #[test]
    fn usage_update_does_not_mark_a_message_boundary() {
        // 直接盯游标状态：`on_boundary()` 只是**置位**，是否真清空要等下一条 chunk 的
        // starts_with 裁定，所以只看输出会漏掉这个回归——必须断言标志位本身。
        // 做法：在 usage_update 前后各取一次快照，断言这条通知没有改动任何游标状态。
        let mut state = AcpUpdateState::default();
        let mut events = Vec::new();
        let mut feed = |state: &mut AcpUpdateState, u: serde_json::Map<String, Value>| {
            acp_apply_session_update(&u, state, &mut |e| events.push(e));
        };
        feed(&mut state, thought_chunk("t"));
        feed(&mut state, msg_chunk("abc"));
        let before = (
            state.text.current.clone(),
            state.text.boundary_pending,
            state.thought.current.clone(),
            state.thought.boundary_pending,
        );

        feed(
            &mut state,
            usage_update(json!({ "used": 100, "size": 200000 })),
        );

        assert_eq!(
            (
                state.text.current.clone(),
                state.text.boundary_pending,
                state.thought.current.clone(),
                state.thought.boundary_pending,
            ),
            before,
            "usage_update 不是消息边界，不得改动任何游标状态"
        );
    }

    #[test]
    fn usage_update_between_chunks_never_duplicates_text() {
        // 若 usage_update 被当成边界，游标会在下一条不同前缀的 chunk 处被清空，
        // 之后到来的整轮累积快照就不再以游标为前缀 → 整段正文重发一遍。
        let (text, _) = run_updates(&[
            msg_chunk("abc"),
            usage_update(json!({ "used": 100, "size": 200000 })),
            msg_chunk("xyz"),
            msg_chunk("abcxyzEND"),
        ]);
        assert_eq!(text, "abcxyzEND", "usage_update 不得重置正文游标");
    }

    #[test]
    fn usage_update_between_chunks_never_duplicates_thought() {
        let mut state = AcpUpdateState::default();
        let mut events = Vec::new();
        for u in [
            thought_chunk("abc"),
            usage_update(json!({ "used": 100 })),
            thought_chunk("xyz"),
            thought_chunk("abcxyzEND"),
        ] {
            acp_apply_session_update(&u, &mut state, &mut |e| events.push(e));
        }
        let thoughts: String = events
            .iter()
            .filter_map(|e| match e {
                UnifiedAgentEvent::ThinkingDelta { delta } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(thoughts, "abcxyzEND", "usage_update 不得重置思考游标");
        // 用量事件仍要发出来。
        assert!(events
            .iter()
            .any(|e| matches!(e, UnifiedAgentEvent::Usage { .. })));
    }

    #[test]
    fn one_shot_driver_also_emits_usage_update() {
        // 一次性驱动（run_acp_session）走 apply_acp_session_update，必须同样覆盖。
        let update = usage_update(json!({ "used": 13477, "size": 200000 }));
        let mut emitted = HashSet::new();
        let mut events = Vec::new();
        assert!(apply_acp_session_update(&update, &mut emitted, &mut |e| {
            events.push(e);
        }));
        assert!(events.iter().any(|e| matches!(
            e,
            UnifiedAgentEvent::Usage { usage }
                if usage.input_tokens == Some(13_477)
                    && usage.context_window_tokens == Some(200_000)
        )));
    }

    #[test]
    fn prompt_response_usage_counts_cache_and_thought_tokens() {
        // opencode 本机实测样本；11685 + 4 + 11 + 1792 = 13492 = 其自报的 totalTokens。
        let usage = format_acp_usage(&json!({
            "inputTokens": 11685,
            "outputTokens": 4,
            "totalTokens": 13492,
            "thoughtTokens": 11,
            "cachedReadTokens": 1792
        }))
        .expect("有非零字段应产出用量");
        assert_eq!(usage.input_tokens, Some(11_685));
        assert_eq!(usage.output_tokens, Some(4));
        assert_eq!(usage.cached_input_tokens, Some(1_792));
        assert_eq!(usage.reasoning_tokens, Some(11));
        assert_eq!(usage.total_tokens, Some(13_492));
        // PromptResponse.usage 不带窗口——留 None 交给 run.rs 的窗口粘滞。
        assert_eq!(usage.context_window_tokens, None);
    }

    #[test]
    fn prompt_response_usage_counts_cache_write() {
        let usage = format_acp_usage(&json!({ "cachedWriteTokens": 4096 }))
            .expect("只有 cache write 也是真实占用");
        assert_eq!(usage.cache_creation_input_tokens, Some(4_096));
        assert_eq!(usage.total_tokens, Some(4_096));
    }

    #[test]
    fn prompt_response_usage_is_none_when_empty_or_all_zero() {
        // ACP 标记 UNSTABLE，缺失/全零一律 None（不得报错，也不得造出 0 用量覆盖真实值）。
        assert!(format_acp_usage(&json!({})).is_none());
        assert!(format_acp_usage(&json!({
            "inputTokens": 0, "outputTokens": 0,
            "cachedReadTokens": 0, "cachedWriteTokens": 0, "thoughtTokens": 0
        }))
        .is_none());
    }

    /// grok 0.2.114 本机实测原文（2026-07-30）：`result` 里**没有** `usage`，用量全在 `_meta`。
    #[test]
    fn prompt_result_reads_grok_meta_usage() {
        let result = json!({
            "stopReason": "end_turn",
            "_meta": {
                "sessionId": "019fb3a1-b5d8-7a30-9a49-56c47a1c8df6",
                "totalTokens": 14893, "modelId": "grok-4.5",
                "inputTokens": 14870, "outputTokens": 23,
                "cachedReadTokens": 128, "reasoningTokens": 21,
                "usage": { "inputTokens": 14870, "outputTokens": 23, "totalTokens": 14893,
                           "cachedReadTokens": 128, "reasoningTokens": 21, "numTurns": 1 }
            }
        });
        let usage = usage_from_prompt_result(&result).expect("grok 的 _meta 必须被读到");
        // 信 grok 自报的 total：它的 cachedRead ⊆ input（14870 + 23 = 14893），
        // 按 opencode 那套并列口径累加会变成 15021（cache 双算）。
        assert_eq!(usage.total_tokens, Some(14_893));
        assert_eq!(usage.input_tokens, Some(14_870));
        assert_eq!(usage.cached_input_tokens, Some(128));
    }

    /// 带工具调用的一轮：`_meta` 顶层 = 最后一次模型往返，`_meta.usage` = 本 prompt 累计。
    /// 上下文占用取前者，取后者会随 `numTurns` 虚涨（实测 15137 vs 30113）。
    #[test]
    fn prompt_result_prefers_last_call_over_cumulative_usage() {
        let result = json!({
            "stopReason": "end_turn",
            "_meta": {
                "totalTokens": 15137, "inputTokens": 15108, "outputTokens": 29,
                "cachedReadTokens": 14848, "reasoningTokens": 24,
                "usage": { "inputTokens": 30034, "outputTokens": 79, "totalTokens": 30113,
                           "cachedReadTokens": 29696, "modelCalls": 2, "numTurns": 2 }
            }
        });
        let usage = usage_from_prompt_result(&result).expect("应读到用量");
        assert_eq!(usage.total_tokens, Some(15_137));
    }

    /// 官方 `result.usage` 优先于 `_meta`：opencode 那条路不能被 grok 分支改口径。
    #[test]
    fn prompt_result_prefers_official_usage_field() {
        let result = json!({
            "usage": { "inputTokens": 11685, "outputTokens": 4,
                       "thoughtTokens": 11, "cachedReadTokens": 1792 },
            "_meta": { "totalTokens": 999_999 }
        });
        let usage = usage_from_prompt_result(&result).expect("应读官方字段");
        assert_eq!(usage.total_tokens, Some(13_492)); // 四项并列相加，不是 _meta 的 999999
    }

    #[test]
    fn prompt_result_without_usage_anywhere_is_none() {
        assert!(usage_from_prompt_result(&json!({ "stopReason": "end_turn" })).is_none());
        // `_meta` 存在但只有非用量字段（cursor/opencode 的 result 形状）——不得造出假用量。
        assert!(usage_from_prompt_result(&json!({
            "stopReason": "end_turn", "_meta": { "sessionId": "s1" }
        }))
        .is_none());
        // totalTokens 为 0 的空轮（斜杠命令）：不得覆盖真实值。
        assert!(usage_from_prompt_result(&json!({ "_meta": { "totalTokens": 0 } })).is_none());
    }

    #[test]
    fn normalize_models_from_available() {
        let result = json!({
            "models": {
                "availableModels": [
                    { "modelId": "grok-4.3", "name": "Grok 4.3" }
                ]
            }
        });
        let models = normalize_models(&result);
        assert!(models.iter().any(|m| m.id == "grok-4.3"));
    }

    #[test]
    fn cursor_model_id_params_carry_context_window() {
        // 本机实测 cursor 的 session/new 返回样本（2026-07-26）：窗口写在方括号参数里，
        // 且这些模型**没有** _meta.totalContextTokens。
        assert_eq!(
            context_window_from_model_id_params(
                "claude-opus-5[thinking=true,context=300k,effort=high,fast=false]"
            ),
            Some(300_000)
        );
        assert_eq!(
            context_window_from_model_id_params(
                "gpt-5.6-sol[context=272k,reasoning=medium,fast=false]"
            ),
            Some(272_000)
        );
        assert_eq!(
            context_window_from_model_id_params(
                "claude-sonnet-4-6[thinking=true,context=200k,effort=high,fast=false]"
            ),
            Some(200_000)
        );
        // 无 context= 提示 → None，不猜。
        assert_eq!(
            context_window_from_model_id_params("composer-2.5[fast=true]"),
            None
        );
        assert_eq!(context_window_from_model_id_params("default[]"), None);
        // 完全没有方括号（grok / kimi 形态）也不该 panic。
        assert_eq!(context_window_from_model_id_params("grok-4.5"), None);
        // 有 context= 但值解析不出来 → None。
        assert_eq!(
            context_window_from_model_id_params("weird[context=abc]"),
            None
        );
    }

    #[test]
    fn normalize_models_reads_cursor_context_hint_from_model_id() {
        let result = json!({
            "models": {
                "currentModelId": "composer-2.5[fast=true]",
                "availableModels": [
                    { "modelId": "claude-opus-5[thinking=true,context=300k,effort=high,fast=false]",
                      "name": "claude-opus-5" },
                    { "modelId": "gpt-5.6-sol[context=272k,reasoning=medium,fast=false]",
                      "name": "gpt-5.6-sol" },
                    { "modelId": "composer-2.5[fast=true]", "name": "composer-2.5" }
                ]
            }
        });
        let models = normalize_models(&result);
        let window_of = |id: &str| {
            models
                .iter()
                .find(|m| m.id == id)
                .unwrap_or_else(|| panic!("missing {id}"))
                .context_window_tokens
        };
        assert_eq!(
            window_of("claude-opus-5[thinking=true,context=300k,effort=high,fast=false]"),
            Some(300_000)
        );
        assert_eq!(
            window_of("gpt-5.6-sol[context=272k,reasoning=medium,fast=false]"),
            Some(272_000)
        );
        // 无提示的模型窗口留空——不许兜底成 200K。
        assert_eq!(window_of("composer-2.5[fast=true]"), None);
    }

    /// cursor 的模型实际是从 **configOptions** 分支出来的，不是 `models.availableModels`
    /// ——那条分支命中后会提前 return。真机实测 33 个模型全走 configOptions，
    /// 所以只给 availableModels 加窗口解析在生产里等于没做（实测 0/33 有窗口）。
    /// 这条测试钉住 configOptions 分支，防止再次回归。
    #[test]
    fn normalize_models_reads_context_hint_from_config_options_branch() {
        // 真机 `cursor-agent acp` 的 session/new 返回形状（含 mode 项，模型项排第二）。
        let result = json!({
            "configOptions": [
                { "id": "mode", "category": "mode", "options": [
                    { "value": "agent", "name": "Agent" }
                ]},
                { "id": "model", "category": "model",
                  "currentValue": "claude-opus-5[thinking=true,context=300k,effort=high,fast=false]",
                  "options": [
                    { "value": "default[]", "name": "Auto" },
                    { "value": "claude-opus-5[thinking=true,context=300k,effort=high,fast=false]",
                      "name": "claude-opus-5" },
                    { "value": "gpt-5.6-sol[context=272k,reasoning=medium,fast=false]",
                      "name": "gpt-5.6-sol" },
                    { "value": "composer-2.5[fast=true]", "name": "composer-2.5" }
                ]}
            ]
        });
        let models = normalize_models(&result);
        let window_of = |id: &str| {
            models
                .iter()
                .find(|m| m.id == id)
                .unwrap_or_else(|| panic!("missing {id}"))
                .context_window_tokens
        };
        assert_eq!(
            window_of("claude-opus-5[thinking=true,context=300k,effort=high,fast=false]"),
            Some(300_000)
        );
        assert_eq!(
            window_of("gpt-5.6-sol[context=272k,reasoning=medium,fast=false]"),
            Some(272_000)
        );
        // `default[]` 与无 context= 的模型都留空，不猜。
        assert_eq!(window_of("composer-2.5[fast=true]"), None);
        assert_eq!(window_of("default[]"), None);
    }

    #[test]
    fn normalize_models_prefers_meta_window_over_model_id_hint() {
        // 同时有 _meta.totalContextTokens 与 modelId 的 context= 时取 _meta（显式字段更可靠）。
        let result = json!({
            "models": {
                "availableModels": [
                    { "modelId": "x-model[context=300k]", "name": "X",
                      "_meta": { "totalContextTokens": 500000 } }
                ]
            }
        });
        let models = normalize_models(&result);
        assert_eq!(
            models
                .iter()
                .find(|m| m.id == "x-model[context=300k]")
                .expect("model")
                .context_window_tokens,
            Some(500_000)
        );
    }

    #[test]
    fn extract_acp_current_reads_current_model_and_reasoning() {
        // grok 形态：currentModelId + 当前模型的 _meta.reasoningEfforts 里 default=true。
        let result = json!({
            "models": {
                "currentModelId": "grok-4.5",
                "availableModels": [
                    { "modelId": "grok-4.5", "name": "Grok 4.5", "_meta": {
                        "reasoningEfforts": [
                            { "id": "low", "default": false },
                            { "id": "high", "default": true }
                        ]
                    }},
                    { "modelId": "grok-4-fast", "name": "Grok 4 Fast" }
                ]
            }
        });
        let (model, reasoning) = extract_acp_current(&result);
        assert_eq!(model.as_deref(), Some("grok-4.5"));
        assert_eq!(reasoning.as_deref(), Some("high"));
    }

    #[test]
    fn extract_acp_current_none_without_current() {
        let result = json!({
            "models": { "availableModels": [ { "modelId": "gpt-5" } ] }
        });
        let (model, reasoning) = extract_acp_current(&result);
        assert!(model.is_none());
        assert!(reasoning.is_none());
    }

    // ---- N3: per-turn model / reasoning change decisions (Step 5) ----

    #[test]
    fn model_set_rpc_uses_config_option_when_id_present() {
        let (method, params) = model_set_rpc("sess-1", Some("model"), "grok-4.5");
        assert_eq!(method, "session/set_config_option");
        assert_eq!(params["sessionId"], json!("sess-1"));
        assert_eq!(params["configId"], json!("model"));
        assert_eq!(params["value"], json!("grok-4.5"));
    }

    #[test]
    fn model_set_rpc_falls_back_to_set_model() {
        let (method, params) = model_set_rpc("sess-1", None, "sonnet-4");
        assert_eq!(method, "session/set_model");
        assert_eq!(params["sessionId"], json!("sess-1"));
        assert_eq!(params["modelId"], json!("sonnet-4"));
    }

    #[test]
    fn reasoning_action_no_change_when_equal() {
        let cur = Some("high".to_string());
        let want = Some("high".to_string());
        assert_eq!(
            reasoning_action(&cur, &want, &Some("reasoning".to_string())),
            ReasoningAction::NoChange
        );
    }

    #[test]
    fn reasoning_action_sets_config_when_option_available() {
        let cur = Some("low".to_string());
        let want = Some("high".to_string());
        assert_eq!(
            reasoning_action(&cur, &want, &Some("reasoning_effort".to_string())),
            ReasoningAction::SetConfig {
                config_id: "reasoning_effort".to_string(),
                value: "high".to_string(),
            }
        );
    }

    #[test]
    fn reasoning_action_reconnects_when_launch_flag_only() {
        // grok: reasoning is a launch flag → no config id → change forces a reconnect.
        let cur = Some("low".to_string());
        let want = Some("high".to_string());
        assert_eq!(
            reasoning_action(&cur, &want, &None),
            ReasoningAction::Reconnect
        );
    }

    #[test]
    fn find_config_ids_picks_model_and_reasoning() {
        let result = json!({
            "configOptions": [
                { "id": "model", "options": [] },
                { "id": "reasoning_effort", "options": [] },
            ]
        });
        let (model_id, reasoning_id) = find_config_ids(&result);
        assert_eq!(model_id.as_deref(), Some("model"));
        assert_eq!(reasoning_id.as_deref(), Some("reasoning_effort"));
    }

    #[test]
    fn find_config_ids_none_when_absent() {
        let (model_id, reasoning_id) = find_config_ids(&json!({}));
        assert!(model_id.is_none());
        assert!(reasoning_id.is_none());
    }

    fn event_variant(event: &UnifiedAgentEvent) -> &'static str {
        match event {
            UnifiedAgentEvent::TextDelta { .. } => "TextDelta",
            UnifiedAgentEvent::ThinkingDelta { .. } => "ThinkingDelta",
            UnifiedAgentEvent::ToolUse { .. } => "ToolUse",
            UnifiedAgentEvent::ToolResult { .. } => "ToolResult",
            UnifiedAgentEvent::Usage { .. } => "Usage",
            UnifiedAgentEvent::Error { .. } => "Error",
            UnifiedAgentEvent::Raw { .. } => "Raw",
            UnifiedAgentEvent::SlashCommands { .. } => "SlashCommands",
            UnifiedAgentEvent::CliCompacted { .. } => "CliCompacted",
            UnifiedAgentEvent::UserSteer { .. } => "UserSteer",
            UnifiedAgentEvent::UserFollowUp { .. } => "UserFollowUp",
            UnifiedAgentEvent::StatusNote { .. } => "StatusNote",
            UnifiedAgentEvent::BackgroundTask { .. } => "BackgroundTask",
            UnifiedAgentEvent::TodoWrite { .. } => "TodoWrite",
            UnifiedAgentEvent::SubagentProgress { .. } => "SubagentProgress",
        }
    }

    /// 一轮真机 ACP 对话，**走生产用的常驻 actor**（`AcpSession::connect` +
    /// `spawn_acp_session_actor`）。
    ///
    /// 此前这条测试驱动的是 `run_acp_session` —— 一个只被它自己吊着命的一次性驱动，
    /// 也就是同一个协议的第二份实现。改成驱动生产代码后那份实现被删掉了。
    #[tokio::test]
    #[ignore = "requires live cursor-agent login + network"]
    async fn cursor_acp_smoke() {
        let Some(bin) = which_bin("cursor-agent") else {
            eprintln!("SKIP: cursor-agent 不在 PATH 上");
            return;
        };
        let cwd = std::env::temp_dir();
        let session = match AcpSession::connect(
            &bin,
            &["acp".to_string()],
            &cwd,
            None,
            None,
            &[],
            None,
        )
        .await
        {
            Ok(session) => session,
            Err(err) => {
                eprintln!("SKIP: 连接失败（未登录 / 网络？）：{err}");
                return;
            }
        };
        let control = spawn_acp_session_actor(session);
        let captured = run_one_live_turn(
            &control,
            "Reply with exactly the token SMOKE_OK and nothing else.",
        )
        .await;

        eprintln!("=== cursor ACP smoke: {} events ===", captured.len());
        for (i, ev) in captured.iter().enumerate() {
            eprintln!("[{i}] {ev:?}");
        }
        let seq: Vec<&str> = captured.iter().map(event_variant).collect();
        eprintln!("cursor sequence: {seq:?}");

        let got_text = captured
            .iter()
            .any(|e| matches!(e, UnifiedAgentEvent::TextDelta { .. }));
        let got_error = captured
            .iter()
            .any(|e| matches!(e, UnifiedAgentEvent::Error { .. }));
        assert!(
            got_text || got_error,
            "expected at least one TextDelta or an Error, got: {seq:?}"
        );
    }

    /// 把一轮跑完并收齐事件。90s 墙钟上限 —— 挂住就是失败，不是慢。
    async fn run_one_live_turn(
        control: &tokio::sync::mpsc::Sender<SessionCommand>,
        prompt: &str,
    ) -> Vec<UnifiedAgentEvent> {
        let (events_tx, mut events_rx) = tokio::sync::mpsc::channel::<UnifiedAgentEvent>(256);
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        control
            .send(SessionCommand::RunTurn {
                prompt: prompt.to_string(),
                model: None,
                reasoning: None,
                images: Vec::new(),
                extra_writable_roots: Vec::new(),
                events: events_tx,
                done: done_tx,
                approvals: None,
            })
            .await
            .expect("actor alive");
        let collector = tokio::spawn(async move {
            let mut out = Vec::new();
            while let Some(event) = events_rx.recv().await {
                out.push(event);
            }
            out
        });
        match timeout(Duration::from_secs(90), done_rx).await {
            Ok(Ok(Ok(()))) => eprintln!("turn: Ok"),
            Ok(Ok(Err(err))) => eprintln!("turn: Err({err})"),
            Ok(Err(_)) => eprintln!("turn: actor dropped the done channel"),
            Err(_) => panic!("session HUNG past the 90s wall-clock guard"),
        }
        collector.await.expect("collector task")
    }

    /// Live proof that L6 recovers cursor's context window from the modelId params.
    ///
    /// cursor 是唯一把窗口写在 modelId 方括号里的 CLI（实测 32 个模型中 13 个带
    /// `context=`，且全都没有 `_meta.totalContextTokens`）。这条跑真实 `cursor-agent acp`
    /// 探测，断言确实有模型解析出了窗口——改动前这些全是 None，分母只能吃 200K 兜底。
    #[tokio::test]
    #[ignore = "requires live cursor-agent login + network"]
    async fn cursor_models_expose_context_window_from_model_id() {
        let bin = which_bin("cursor-agent").expect("cursor-agent on PATH");
        let cwd = std::env::temp_dir();
        let probe = detect_acp_models(&bin, &["acp"], &cwd, 60)
            .await
            .expect("cursor acp model probe");

        let with_window: Vec<_> = probe
            .models
            .iter()
            .filter(|m| m.context_window_tokens.is_some())
            .collect();
        eprintln!(
            "cursor models: {} total, {} with a window",
            probe.models.len(),
            with_window.len()
        );
        for m in with_window.iter().take(8) {
            eprintln!("  {} -> {:?}", m.id, m.context_window_tokens);
        }
        assert!(
            !with_window.is_empty(),
            "no cursor model exposed a context window; modelId parsing regressed"
        );
        // 实测样本里 claude-opus-5 是 300k、gpt-5.6-sol 是 272k，都应落在合理区间。
        for m in &with_window {
            let w = m.context_window_tokens.unwrap();
            assert!(
                (100_000..=2_000_000).contains(&w),
                "implausible window {w} for {}",
                m.id
            );
        }
    }

    /// Live end-to-end proof that the ACP usage channel (L1/L2) works against a real CLI.
    ///
    /// opencode 是实测唯一会发 `usage_update` 的已装 CLI（kimi/cursor 均不发），所以它是
    /// 这条链路唯一能真机验证的入口。断言两件单测无法覆盖的事：
    /// 1. **分母真的来自 CLI**——`context_window_tokens` 非空（`usage_update.size`）。
    /// 2. **分子计入了 cache**——`total_tokens > input + output`，即 `cachedReadTokens`
    ///    确实被 `format_acp_usage` 读到了（旧代码只读 input/output，实测偏低 13%）。
    /// 同时打印真实数字，供人工对照 `research/cli-wire-facts.md` 里记录的样本。
    #[tokio::test]
    #[ignore = "requires live opencode login + network"]
    async fn opencode_acp_reports_usage_and_context_window() {
        let Some(bin) = which_bin("opencode") else {
            eprintln!("SKIP: opencode 不在 PATH 上");
            return;
        };
        let cwd = std::env::temp_dir();
        // 走生产用的常驻 actor（此前驱动的是只被真机测试吊着命的一次性 `run_acp_session`）。
        let session = match AcpSession::connect(
            &bin,
            &["acp".to_string()],
            &cwd,
            None,
            None,
            &[],
            None,
        )
        .await
        {
            Ok(session) => session,
            Err(err) => {
                eprintln!("SKIP: 连接失败（未登录 / 网络？）：{err}");
                return;
            }
        };
        let control = spawn_acp_session_actor(session);
        // 这里不用 `run_one_live_turn`：它的墙钟超时是 panic，而 opencode 密钥失效导致的
        // 挂起是**环境**问题不是本层回归，必须按 SKIP 处理。
        let (events_tx, mut events_rx) = tokio::sync::mpsc::channel::<UnifiedAgentEvent>(256);
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        control
            .send(SessionCommand::RunTurn {
                prompt: "Reply with exactly the token USAGE_OK and nothing else.".to_string(),
                model: None,
                reasoning: None,
                images: Vec::new(),
                extra_writable_roots: Vec::new(),
                events: events_tx,
                done: done_tx,
                approvals: None,
            })
            .await
            .expect("actor alive");
        let collector = tokio::spawn(async move {
            let mut out = Vec::new();
            while let Some(event) = events_rx.recv().await {
                out.push(event);
            }
            out
        });
        let result = timeout(Duration::from_secs(120), done_rx).await;
        let captured = collector.await.expect("collector task");
        if result.is_err() {
            eprintln!("SKIP: opencode ACP 会话在 120s 内没有完成（多为密钥失效/网络问题）。先跑 `opencode run hi` 确认 CLI 本身可用。");
            return;
        }

        let usages: Vec<&crate::chat::model::ModelUsage> = captured
            .iter()
            .filter_map(|e| match e {
                UnifiedAgentEvent::Usage { usage } => Some(usage),
                _ => None,
            })
            .collect();
        for u in &usages {
            eprintln!(
                "opencode usage: input={:?} output={:?} cache_read={:?} reasoning={:?} \
                 total={:?} window={:?}",
                u.input_tokens,
                u.output_tokens,
                u.cached_input_tokens,
                u.reasoning_tokens,
                u.total_tokens,
                u.context_window_tokens,
            );
        }
        assert!(
            !usages.is_empty(),
            "opencode reported no usage at all — the ACP usage channel regressed \
             (若 `opencode run hi` 也报 Invalid API Key，先修密钥再看这条)"
        );

        // 分母：来自 usage_update.size。
        let window = usages.iter().find_map(|u| u.context_window_tokens);
        assert!(
            window.is_some_and(|w| w > 0),
            "no context window from usage_update.size; usages={usages:?}"
        );

        // 分子：cache token 必须计入 total，否则就是修复前那个低估的老口径。
        let counts_cache = usages.iter().any(|u| {
            let cache = u.cached_input_tokens.unwrap_or(0);
            let plain = u.input_tokens.unwrap_or(0) + u.output_tokens.unwrap_or(0);
            cache > 0 && u.total_tokens.unwrap_or(0) > plain
        });
        assert!(
            counts_cache,
            "no usage report counted cache tokens into total; usages={usages:?}"
        );
    }
}

#[cfg(test)]
mod live_reasoning_tests {
    use super::*;

    /// 真机跑 `kimi acp`，确认档位选项与当前值都能探到。
    /// 单测喂构造样本，这条证明真实 CLI 也给 —— 而且是 defs 静态表拿不到的
    /// （`acp_def` 把 reasoning_options 写死成空，所有 ACP CLI 共用）。
    #[tokio::test]
    #[ignore = "requires a real kimi CLI on this machine"]
    async fn live_kimi_exposes_reasoning_options() {
        // kimi 常装在 ~/.kimi-code/bin（不一定在 PATH 上），两处都找。
        let bin = crate::external_agents::spawn::resolve_binary(
            crate::external_agents::registry::get_agent_def("kimi").expect("kimi def"),
        )
        .await
        .or_else(|| {
            let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
            let p = home.join(".kimi-code").join("bin").join("kimi");
            p.exists().then_some(p)
        })
        .expect("kimi on PATH or in ~/.kimi-code/bin");
        let probe = detect_acp_models(&bin, &["acp"], &std::env::temp_dir(), 60)
            .await
            .expect("kimi acp probe");
        eprintln!("current_model     = {:?}", probe.current_model);
        eprintln!("current_reasoning = {:?}", probe.current_reasoning);
        eprintln!(
            "reasoning_options = {:?}",
            probe
                .reasoning_options
                .iter()
                .map(|o| format!("{}/{}", o.id, o.label))
                .collect::<Vec<_>>()
        );
        assert!(
            !probe.reasoning_options.is_empty(),
            "kimi 应上报推理档位（实测 low/high/max），拿到空说明发现侧回归了"
        );
        assert!(
            probe.current_reasoning.is_some(),
            "kimi 的 configOptions 带 currentValue，当前档位不该是 None"
        );
    }
}

// -------------------------------------------------------------------------------------------
// 导入用：ACP 会话枚举
// -------------------------------------------------------------------------------------------

/// `session/list` 返回的一条会话（只取导入列表要用的字段）。
#[derive(Debug, Clone, PartialEq)]
pub struct AcpSessionSummary {
    pub session_id: String,
    pub cwd: String,
    pub title: Option<String>,
}

/// ACP `session/list` 探针：起进程 → `initialize` → 按 `cwd` 翻页拉会话 → 关掉。
///
/// **只在导入列表里用**，不参与聊天路径。
///
/// 返回 `None` 表示**该 agent 不支持导入**——要么 `initialize` 没声明 `loadSession`
/// （导进来也续不了聊，本机 gemini 就是这样），要么 `session/list` 回 `Method not found`。
/// 返回 `Some(vec![])` 是"支持但这个目录下没有会话"，两者含义不同，前端展示也不同。
///
/// `cwd` 必须传给 agent：不传的话它返回的是**全局**最近会话，翻页上限会在够到本目录的会话
/// 之前就截断（这个坑 paseo 也踩过并写在注释里）。
pub async fn probe_acp_sessions(
    bin: &Path,
    args: &[&str],
    cwd: &Path,
    timeout_secs: u64,
) -> Option<Vec<AcpSessionSummary>> {
    // ponytail: 每次列表都起一次进程握手（实测 opencode ~2s）。导入入口是低频动作，
    // 不做连接复用；真觉得慢再挂到 detection 的缓存上。
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

    let deadline = Duration::from_secs(timeout_secs);
    let started = std::time::Instant::now();

    write_rpc(
        &mut stdin,
        1,
        "initialize",
        json!({
            "protocolVersion": ACP_PROTOCOL_VERSION,
            "clientCapabilities": { "terminal": false },
            "clientInfo": { "name": "kivio", "version": "external-agents" },
        }),
    )
    .await
    .ok()?;

    let init = read_rpc_response(&mut reader, 1, started, deadline).await?;
    let supports_load = init
        .get("result")
        .and_then(|r| r.get("agentCapabilities"))
        .and_then(|c| c.get("loadSession"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !supports_load {
        let _ = child.start_kill();
        return None;
    }

    let cwd_text = cwd.to_string_lossy().to_string();
    let mut sessions = Vec::new();
    let mut cursor: Option<String> = None;
    let mut next_id: u64 = 2;
    // 翻页上限：防止 agent 返回一个永不为空的游标把我们困在这里。
    for _ in 0..20 {
        let mut params = json!({ "cwd": cwd_text });
        if let Some(c) = cursor.as_ref() {
            params["cursor"] = json!(c);
        }
        let id = next_id;
        next_id += 1;
        if write_rpc(&mut stdin, id, "session/list", params)
            .await
            .is_err()
        {
            break;
        }
        let Some(response) = read_rpc_response(&mut reader, id, started, deadline).await else {
            break;
        };
        if response.get("error").is_some() {
            // `Method not found` ⇒ 这个 agent 根本没有会话枚举，按"不支持导入"处理。
            let _ = child.start_kill();
            return None;
        }
        let Some(result) = response.get("result") else {
            break;
        };
        if let Some(items) = result.get("sessions").and_then(|v| v.as_array()) {
            for item in items {
                let Some(session_id) = item.get("sessionId").and_then(|v| v.as_str()) else {
                    continue;
                };
                sessions.push(AcpSessionSummary {
                    session_id: session_id.to_string(),
                    cwd: item
                        .get("cwd")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&cwd_text)
                        .to_string(),
                    title: item
                        .get("title")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                });
            }
        }
        cursor = result
            .get("nextCursor")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }

    let _ = child.start_kill();
    Some(sessions)
}

/// 读到指定 id 的 JSON-RPC 响应为止，途中的通知一律丢弃。超时或流结束返回 `None`。
async fn read_rpc_response(
    reader: &mut Lines<BufReader<ChildStdout>>,
    want_id: u64,
    started: std::time::Instant,
    deadline: Duration,
) -> Option<Value> {
    loop {
        if started.elapsed() > deadline {
            return None;
        }
        let line = match timeout(Duration::from_millis(200), reader.next_line()).await {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) | Ok(Err(_)) => return None,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if value.get("id").and_then(|v| v.as_u64()) == Some(want_id) {
            return Some(value);
        }
    }
}

/// 用 ACP `session/load` 把一条既有会话的历史重放出来，收集期间推送的 `session/update`。
///
/// 只在**导入**时用。返回的是原始 update 负载，交给 `import_history::parse_acp_updates`
/// 转成 `ChatMessage`——协议细节留在本模块，消息形状的事归那边。
///
/// 返回 `None` = 加载失败或该 agent 不支持；`Some(vec![])` = 加载成功但**不重放历史**
/// （kimi 实测就是这样）。两者调用方要区别对待。
pub async fn probe_acp_session_history(
    bin: &Path,
    args: &[&str],
    cwd: &Path,
    session_id: &str,
    timeout_secs: u64,
) -> Option<Vec<Value>> {
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

    let deadline = Duration::from_secs(timeout_secs);
    let started = std::time::Instant::now();

    write_rpc(
        &mut stdin,
        1,
        "initialize",
        json!({
            "protocolVersion": ACP_PROTOCOL_VERSION,
            "clientCapabilities": { "terminal": false },
            "clientInfo": { "name": "kivio", "version": "external-agents" },
        }),
    )
    .await
    .ok()?;
    read_rpc_response(&mut reader, 1, started, deadline).await?;

    write_rpc(
        &mut stdin,
        2,
        "session/load",
        json!({
            "sessionId": session_id,
            "cwd": cwd.to_string_lossy(),
            "mcpServers": [],
        }),
    )
    .await
    .ok()?;

    // 重放的通知**大部分在响应之前**到达，但不能只读到响应就停——实测部分 agent 会在
    // 响应之后再补推。所以收到响应后再多听一小段（tail window），否则会漏掉尾巴上的消息。
    let mut updates = Vec::new();
    let mut got_response = false;
    let mut tail_deadline: Option<std::time::Instant> = None;
    loop {
        if started.elapsed() > deadline {
            break;
        }
        if let Some(tail) = tail_deadline {
            if std::time::Instant::now() > tail {
                break;
            }
        }
        let line = match timeout(Duration::from_millis(200), reader.next_line()).await {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) | Ok(Err(_)) => break,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if value.get("id").and_then(|v| v.as_u64()) == Some(2) {
            if value.get("error").is_some() {
                let _ = child.start_kill();
                return None;
            }
            got_response = true;
            tail_deadline = Some(std::time::Instant::now() + Duration::from_secs(3));
            continue;
        }
        if value.get("method").and_then(|v| v.as_str()) == Some("session/update") {
            if let Some(update) = value.get("params").and_then(|p| p.get("update")) {
                updates.push(update.clone());
            }
        }
    }

    let _ = child.start_kill();
    got_response.then_some(updates)
}
