use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use tauri::{AppHandle, State};
use tokio::io::AsyncBufReadExt;
use uuid::Uuid;

use crate::external_agents::detection::detect_availability_single;
use crate::external_agents::registry::get_agent_def;
use crate::external_agents::session::acp::detect_acp_commands;
use crate::external_agents::spawn::{
    parse_json_line, resolve_binary, spawn_agent, write_probe_stdin,
};
use crate::external_agents::types::{
    ExternalCliSlashCommand, RuntimeBuildOptions, RuntimeContext, SlashStrategy, UnifiedAgentEvent,
};
use crate::external_agents::workspace::resolve_detection_cwd;
use crate::state::AppState;

pub const SLASH_COMMANDS_CACHE_TTL: Duration = Duration::from_secs(300);
/// 空结果负缓存 TTL（与模型探测 fallback 负缓存对齐）：CLI 不上报斜杠命令 / 探测超时得空列表时，
/// 短 TTL 内不再重探，避免切会话/切 agent 每次都重发 session/new（kimi 侧每次落一个空壳会话）。
pub const SLASH_COMMANDS_EMPTY_CACHE_TTL: Duration = Duration::from_secs(30);

pub fn cache_key(agent_id: &str, cwd: &str) -> String {
    format!("{agent_id}:{cwd}")
}

pub fn parse_slash_commands_from_init(value: &Value) -> Vec<ExternalCliSlashCommand> {
    let Some(items) = value.get("slash_commands").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in items {
        let parsed = parse_slash_command_item(item);
        if let Some(cmd) = parsed {
            if seen.insert(cmd.name.clone()) {
                out.push(cmd);
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn parse_slash_command_item(item: &Value) -> Option<ExternalCliSlashCommand> {
    if let Some(name) = item.as_str().filter(|s| !s.trim().is_empty()) {
        return Some(ExternalCliSlashCommand {
            slash: format!("/{name}"),
            name: name.to_string(),
            description: None,
            argument_hint: None,
        });
    }
    let obj = item.as_object()?;
    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?;
    Some(ExternalCliSlashCommand {
        slash: format!("/{name}"),
        name: name.to_string(),
        description: obj
            .get("description")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        argument_hint: obj
            .get("argument_hint")
            .or_else(|| obj.get("argumentHint"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

pub fn is_claude_init(value: &Value) -> bool {
    value.get("type").and_then(|v| v.as_str()) == Some("system")
        && value.get("subtype").and_then(|v| v.as_str()) == Some("init")
}

pub async fn probe_claude_slash_commands(
    resolved_bin: &Path,
    cwd: &Path,
    args: &[String],
) -> Result<Vec<ExternalCliSlashCommand>, String> {
    let def = get_agent_def("claude").ok_or_else(|| "Claude agent def missing".to_string())?;
    let extra_env = HashMap::new();
    let mut spawned = spawn_agent(def, resolved_bin, args, cwd, &extra_env).await?;
    write_probe_stdin(&mut spawned.child).await?;

    let mut init_value: Option<Value> = None;
    let stdout = spawned
        .child
        .stdout
        .take()
        .ok_or_else(|| "stdout unavailable".to_string())?;
    let mut reader = tokio::io::BufReader::new(stdout).lines();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), reader.next_line()).await {
            Ok(Ok(Some(line))) => {
                if line.trim().is_empty() {
                    continue;
                }
                if let Some(value) = parse_json_line(&line) {
                    if is_claude_init(&value) {
                        init_value = Some(value);
                        break;
                    }
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(e)) => return Err(e.to_string()),
            Err(_) => continue,
        }
    }
    let _ = spawned.child.start_kill();
    let _ = spawned.child.wait().await;

    let value = init_value.ok_or_else(|| "Claude slash 探测未收到 init".to_string())?;
    Ok(parse_slash_commands_from_init(&value))
}

pub async fn list_external_cli_slash_commands(
    app: &AppHandle,
    state: &State<'_, AppState>,
    agent_id: &str,
    conversation_id: Option<&str>,
) -> Result<(bool, Vec<ExternalCliSlashCommand>, Option<String>), String> {
    let def = get_agent_def(agent_id).ok_or_else(|| format!("未知外部 Agent: {agent_id}"))?;

    // Cheap, dependency-free strategies first — no CLI availability check needed.
    if def.slash_strategy == SlashStrategy::None {
        return Ok((
            false,
            Vec::new(),
            Some(format!(
                "{} 以一次性模式运行，斜杠命令仅在其交互式终端中可用；可直接输入指令。",
                def.name
            )),
        ));
    }
    if def.slash_strategy == SlashStrategy::Dsh {
        return Ok((
            true,
            crate::external_agents::defs::dsh::builtin_slash_commands(),
            None,
        ));
    }

    let cwd = resolve_slash_cwd(app, conversation_id)?;
    let key = cache_key(agent_id, &cwd);
    if let Some(cached) = state.get_cached_external_slash_commands(
        &key,
        SLASH_COMMANDS_CACHE_TTL,
        SLASH_COMMANDS_EMPTY_CACHE_TTL,
    ) {
        return Ok((true, cached, None));
    }

    // ClaudeInit / Acp / PiRpc all probe with an identical fresh runtime context + empty build
    // options; only the resolved binary + discovery call differ per arm.
    let runtime_ctx = RuntimeContext {
        extra_allowed_dirs: Vec::new(),
        resume_session_id: None,
        new_session_id: Some(Uuid::new_v4().to_string()),
        include_partial_messages: false,
    };
    let build_options = RuntimeBuildOptions {
        model: None,
        reasoning: None,
        sandbox: None,
    };

    let commands = match def.slash_strategy {
        SlashStrategy::ClaudeInit => {
            let detected = detect_availability_single(def).await;
            if !detected.available {
                return Err(format!(
                    "{} 未安装或不可用，请确认 CLI 在 PATH 中。",
                    def.name
                ));
            }
            let resolved_bin = resolve_binary(def)
                .await
                .ok_or_else(|| format!("无法定位 {} 可执行文件", def.bin))?;
            let args = crate::external_agents::defs::claude::ephemeral_probe_args(&(def
                .build_args)(
                &runtime_ctx,
                &build_options,
                None,
            ));
            probe_claude_slash_commands(&resolved_bin, Path::new(&cwd), &args).await?
        }
        SlashStrategy::Acp => {
            let detected = detect_availability_single(def).await;
            if !detected.available {
                return Err(format!(
                    "{} 未安装或不可用，请确认 CLI 在 PATH 中。",
                    def.name
                ));
            }
            let resolved_bin = resolve_binary(def)
                .await
                .ok_or_else(|| format!("无法定位 {} 可执行文件", def.bin))?;
            let args = (def.build_args)(&runtime_ctx, &build_options, None);
            let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
            detect_acp_commands(&resolved_bin, &args_ref, Path::new(&cwd), 10)
                .await
                .unwrap_or_default()
        }
        SlashStrategy::CodexAppServer => {
            let resolved_bin = resolve_binary(def)
                .await
                .ok_or_else(|| format!("无法定位 {} 可执行文件", def.bin))?;
            crate::external_agents::session::codex_app_server::detect_codex_commands(
                &resolved_bin,
                Path::new(&cwd),
                12,
            )
            .await
            .unwrap_or_default()
        }
        SlashStrategy::PiRpc => {
            let resolved_bin = resolve_binary(def)
                .await
                .ok_or_else(|| format!("无法定位 {} 可执行文件", def.bin))?;
            let args = (def.build_args)(&runtime_ctx, &build_options, None);
            let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
            crate::external_agents::session::pi_rpc::detect_pi_commands(
                &resolved_bin,
                &args_ref,
                Path::new(&cwd),
                10,
            )
            .await
            .unwrap_or_default()
        }
        SlashStrategy::Dsh | SlashStrategy::None => unreachable!("cheap strategies handled above"),
    };

    // R1: cache the result even when empty (negative cache) so切会话/切 agent 不再每次重探。
    // 空列表由 get 的 short TTL 兜底；非空走长 TTL。
    state.set_cached_external_slash_commands(key, commands.clone());

    if commands.is_empty()
        && matches!(
            def.slash_strategy,
            SlashStrategy::Acp | SlashStrategy::CodexAppServer | SlashStrategy::PiRpc
        )
    {
        return Ok((
            true,
            Vec::new(),
            Some(format!("{} 未上报任何斜杠命令。", def.name)),
        ));
    }

    Ok((true, commands, None))
}

/// 斜杠命令探测用 cwd：与模型探测对齐用全局 scope（`resolve_detection_cwd` → `__global__`），
/// 而非每会话独立 workspace。斜杠命令是 CLI 全局命令，与 cwd 无关；用全局 scope 后残渣不再散落
/// 各会话工作区、缓存键全 App 共享（切会话即命中）。只有绑定项目的会话落到项目根。
fn resolve_slash_cwd(app: &AppHandle, conversation_id: Option<&str>) -> Result<String, String> {
    let cwd = resolve_detection_cwd(app, conversation_id)?;
    Ok(cwd.to_string_lossy().into_owned())
}

pub fn slash_commands_from_event(
    event: &UnifiedAgentEvent,
) -> Option<Vec<ExternalCliSlashCommand>> {
    match event {
        UnifiedAgentEvent::SlashCommands { commands } => Some(commands.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_string_slash_commands_from_init() {
        let init = json!({
            "type": "system",
            "subtype": "init",
            "slash_commands": ["compact", "clear", "frontend-design:frontend-design"]
        });
        let commands = parse_slash_commands_from_init(&init);
        assert_eq!(commands.len(), 3);
        assert!(commands.iter().any(|c| c.slash == "/compact"));
        assert!(commands
            .iter()
            .any(|c| c.slash == "/frontend-design:frontend-design"));
    }

    #[test]
    fn parses_object_slash_commands_from_init() {
        let init = json!({
            "type": "system",
            "subtype": "init",
            "slash_commands": [
                {"name": "compact", "description": "Compact history", "argument_hint": "[instructions]"}
            ]
        });
        let commands = parse_slash_commands_from_init(&init);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].slash, "/compact");
        assert_eq!(commands[0].description.as_deref(), Some("Compact history"));
        assert_eq!(commands[0].argument_hint.as_deref(), Some("[instructions]"));
    }
}
