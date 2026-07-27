use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use uuid::Uuid;

use crate::external_agents::registry::get_agent_def;
use crate::external_agents::slash::is_claude_init;
use crate::external_agents::spawn::{parse_json_line, spawn_agent, write_probe_stdin};
use crate::external_agents::types::{RuntimeBuildOptions, RuntimeContext, RuntimeModelOption};

/// `--model` aliases accepted by Claude Code, used to build a static model catalog with
/// labels + context windows (no per-alias process probe). The CLI validates the alias at
/// run time, so an unsupported alias simply fails that turn rather than the picker load.
const CLAUDE_MODEL_ALIASES: &[&str] = &[
    "opus",
    "sonnet",
    "sonnet[1m]",
    "opus[1m]",
    "haiku",
    "fable",
    "fable[1m]",
];

/// `env.*` keys in `~/.claude/settings.json` (and the matching process env vars) that
/// point Claude Code at a custom/third-party model. We surface these as extra `--model`
/// targets so a user's gateway/bedrock setup shows up in the picker. These are the
/// Claude CLI's own public env interface — not paseo's code.
const CLAUDE_ENV_MODEL_KEYS: &[&str] = &[
    "ANTHROPIC_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
];

/// Upper bound on the single best-effort "default" probe. Discovery no longer spawns a
/// process per alias — the alias catalog is static — so this is the only spawn, kept short
/// so the model picker stays responsive even when the CLI is slow to emit its init event.
const DEFAULT_PROBE_TIMEOUT_SECS: u64 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeInitInfo {
    pub resolved_model: String,
    pub context_window_tokens: Option<u32>,
}

pub fn context_window_from_claude_resolved_model(resolved: &str) -> Option<u32> {
    let trimmed = resolved.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.to_ascii_lowercase().ends_with("[1m]") {
        return Some(1_000_000);
    }
    if trimmed.to_ascii_lowercase().contains("claude-") {
        return Some(200_000);
    }
    None
}

pub fn context_window_from_claude_model_alias(alias: &str) -> Option<u32> {
    let alias = alias.trim();
    if alias.is_empty() || alias == "default" {
        return None;
    }
    if alias.to_ascii_lowercase().contains("[1m]") {
        return Some(1_000_000);
    }
    if CLAUDE_MODEL_ALIASES.contains(&alias) {
        return Some(200_000);
    }
    None
}

pub fn label_for_claude_model(alias: &str, resolved: &str) -> String {
    let human = humanize_claude_resolved_model(resolved);
    if alias == "default" {
        format!("Default ({human})")
    } else if alias == "sonnet[1m]" {
        format!("Sonnet (1M context)")
    } else {
        human
    }
}

fn humanize_claude_resolved_model(resolved: &str) -> String {
    let mut base = resolved.trim().to_string();
    let has_1m = base.to_ascii_lowercase().ends_with("[1m]");
    if has_1m {
        base.truncate(base.len().saturating_sub(4));
    }
    if let Some(rest) = base.strip_prefix("claude-") {
        base = rest.to_string();
    }
    let parts: Vec<&str> = base.split('-').filter(|part| !part.is_empty()).collect();
    let label = if parts.is_empty() {
        base
    } else {
        let family = title_case_token(parts[0]);
        if parts.len() >= 3
            && parts[1].chars().all(|ch| ch.is_ascii_digit())
            && parts[2].chars().all(|ch| ch.is_ascii_digit())
        {
            format!("{family} {}.{}", parts[1], parts[2])
        } else if parts.len() >= 2 && parts[1].chars().all(|ch| ch.is_ascii_digit()) {
            format!("{family} {}", parts[1])
        } else {
            parts
                .iter()
                .map(|part| title_case_token(part))
                .collect::<Vec<_>>()
                .join(" ")
        }
    };
    if has_1m {
        format!("{label} (1M context)")
    } else {
        label
    }
}

fn title_case_token(token: &str) -> String {
    let lower = token.to_ascii_lowercase();
    if lower.is_empty() {
        return lower;
    }
    let mut chars = lower.chars();
    let first = chars.next().unwrap().to_ascii_uppercase().to_string();
    first + chars.as_str()
}

pub async fn probe_claude_init(
    resolved_bin: &Path,
    cwd: &Path,
    model_alias: Option<&str>,
) -> Option<ClaudeInitInfo> {
    let def = get_agent_def("claude")?;
    let runtime_ctx = RuntimeContext {
        extra_allowed_dirs: Vec::new(),
        resume_session_id: None,
        new_session_id: Some(Uuid::new_v4().to_string()),
        include_partial_messages: false,
    };
    let build_options = RuntimeBuildOptions {
        model: model_alias
            .filter(|value| !value.is_empty() && *value != "default")
            .map(str::to_string),
        reasoning: None,
        sandbox: None,
    };
    let args = (def.build_args)(&runtime_ctx, &build_options, None);
    let extra_env = std::collections::HashMap::new();
    let mut spawned = spawn_agent(def, resolved_bin, &args, cwd, &extra_env)
        .await
        .ok()?;
    write_probe_stdin(&mut spawned.child).await.ok()?;

    let init = read_claude_init_value(&mut spawned.child, Duration::from_secs(20)).await?;
    let _ = spawned.child.start_kill();
    let _ = spawned.child.wait().await;

    parse_claude_init_info(&init)
}

/// 返回 (模型目录, 当前解析模型)。当前模型 = CLI 对 "default" 实际解析出的模型（如
/// "claude-fable-5[1m]"），供胶囊展示 CLI 当前配置；探不到时为 None。
pub async fn detect_claude_models(
    resolved_bin: &Path,
    cwd: &Path,
) -> Option<(Vec<RuntimeModelOption>, Option<String>)> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    // 1. Default option — one best-effort probe (short timeout). The CLI reports the model
    //    it actually resolves "default" to, which gives an accurate label + context window.
    //    Failure is non-fatal: we still ship a generic "Default" entry.
    let default_info = tokio::time::timeout(
        Duration::from_secs(DEFAULT_PROBE_TIMEOUT_SECS),
        probe_claude_init(resolved_bin, cwd, None),
    )
    .await
    .ok()
    .flatten();
    let current_model = default_info
        .as_ref()
        .map(|info| info.resolved_model.clone());
    out.push(RuntimeModelOption {
        id: "default".to_string(),
        label: match &default_info {
            Some(info) => label_for_claude_model("default", &info.resolved_model),
            None => "Default".to_string(),
        },
        context_window_tokens: default_info
            .as_ref()
            .and_then(|info| info.context_window_tokens),
    });
    seen.insert("default".to_string());

    // 2. Built-in alias catalog — entirely static, no process spawn.
    for &alias in CLAUDE_MODEL_ALIASES {
        if seen.insert(alias.to_string()) {
            out.push(catalog_model_option(alias));
        }
    }

    // 3. Custom models configured via ~/.claude/settings.json `env.*` + process env.
    for model in claude_config_models() {
        if seen.insert(model.clone()) {
            out.push(RuntimeModelOption {
                context_window_tokens: context_window_from_claude_resolved_model(&model),
                label: model.clone(),
                id: model,
            });
        }
    }

    Some((out, current_model))
}

/// Static catalog entry for a Claude `--model` alias — label + context window with no probe.
fn catalog_model_option(alias: &str) -> RuntimeModelOption {
    let is_1m = alias.to_ascii_lowercase().ends_with("[1m]");
    let base = alias
        .get(..alias.len().saturating_sub(if is_1m { 4 } else { 0 }))
        .unwrap_or(alias);
    let family = title_case_token(base);
    RuntimeModelOption {
        id: alias.to_string(),
        label: if is_1m {
            format!("{family} (1M context)")
        } else {
            family
        },
        context_window_tokens: context_window_from_claude_model_alias(alias),
    }
}

/// Config dir Claude Code reads: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
fn claude_config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    directories::BaseDirs::new().map(|base| base.home_dir().join(".claude"))
}

/// Extra model ids the user configured for Claude Code via settings.json `env.*` and process
/// env vars (e.g. a gateway/bedrock model). Returns deduped, non-empty ids in discovery order.
fn claude_config_models() -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let push = |raw: &str, out: &mut Vec<String>, seen: &mut HashSet<String>| {
        let model = raw.trim();
        if !model.is_empty() && seen.insert(model.to_string()) {
            out.push(model.to_string());
        }
    };

    if let Some(text) =
        claude_config_dir().and_then(|dir| std::fs::read_to_string(dir.join("settings.json")).ok())
    {
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            if let Some(env) = value.get("env").and_then(|v| v.as_object()) {
                for key in CLAUDE_ENV_MODEL_KEYS {
                    if let Some(model) = env.get(*key).and_then(|v| v.as_str()) {
                        push(model, &mut out, &mut seen);
                    }
                }
            }
        }
    }

    for key in CLAUDE_ENV_MODEL_KEYS {
        if let Ok(model) = std::env::var(key) {
            push(&model, &mut out, &mut seen);
        }
    }

    out
}

/// 用户在 Claude Code 里配的推理档位（`settings.json` 的 `effortLevel`，
/// 或进程环境的 `CLAUDE_EFFORT`）。
///
/// 与 codex/pi/kimi 的「读本地配置回填当前档位」是同一件事——claude 此前漏了这条，
/// 于是胶囊上恒显示「自动」，哪怕用户明明配了 `effortLevel: "high"`。
///
/// 优先级：`CLAUDE_EFFORT` 环境变量 > `settings.json` 的 `effortLevel`。
/// 环境变量更贴近「本次启动实际生效的值」（CLI 自身也是这个优先级）。
///
/// 返回值必须落在 `defs/claude.rs` 的 `REASONING` 选项 id 集合内，否则前端选不中；
/// 认不出的值一律 `None`（显示「自动」），不猜、不 panic。
pub fn claude_config_effort() -> Option<String> {
    const KNOWN: &[&str] = &["low", "medium", "high", "xhigh", "max"];
    let normalize = |raw: &str| -> Option<String> {
        let value = raw.trim().to_ascii_lowercase();
        KNOWN.contains(&value.as_str()).then_some(value)
    };

    if let Ok(raw) = std::env::var("CLAUDE_EFFORT") {
        if let Some(effort) = normalize(&raw) {
            return Some(effort);
        }
    }

    let text = claude_config_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join("settings.json")).ok())?;
    let value = serde_json::from_str::<Value>(&text).ok()?;
    normalize(value.get("effortLevel")?.as_str()?)
}

pub fn parse_claude_init_info(value: &Value) -> Option<ClaudeInitInfo> {
    if !is_claude_init(value) {
        return None;
    }
    let resolved_model = value.get("model").and_then(|v| v.as_str())?.trim();
    if resolved_model.is_empty() {
        return None;
    }
    Some(ClaudeInitInfo {
        resolved_model: resolved_model.to_string(),
        context_window_tokens: context_window_from_claude_resolved_model(resolved_model),
    })
}

async fn read_claude_init_value(
    child: &mut tokio::process::Child,
    timeout: Duration,
) -> Option<Value> {
    let stdout = child.stdout.as_mut()?;
    let mut reader = tokio::io::BufReader::new(stdout).lines();
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), reader.next_line()).await {
            Ok(Ok(Some(line))) => {
                if line.trim().is_empty() {
                    continue;
                }
                if let Some(value) = parse_json_line(&line) {
                    if is_claude_init(&value) {
                        return Some(value);
                    }
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn context_window_from_resolved_model() {
        assert_eq!(
            context_window_from_claude_resolved_model("claude-opus-4-8[1m]"),
            Some(1_000_000)
        );
        assert_eq!(
            context_window_from_claude_resolved_model("claude-sonnet-4-6"),
            Some(200_000)
        );
    }

    #[test]
    fn context_window_from_alias() {
        assert_eq!(
            context_window_from_claude_model_alias("sonnet[1m]"),
            Some(1_000_000)
        );
        assert_eq!(
            context_window_from_claude_model_alias("sonnet"),
            Some(200_000)
        );
        assert_eq!(context_window_from_claude_model_alias("default"), None);
    }

    #[test]
    fn catalog_options_have_labels_and_windows() {
        let opus = catalog_model_option("opus");
        assert_eq!(opus.id, "opus");
        assert_eq!(opus.label, "Opus");
        assert_eq!(opus.context_window_tokens, Some(200_000));

        let sonnet_1m = catalog_model_option("sonnet[1m]");
        assert_eq!(sonnet_1m.id, "sonnet[1m]");
        assert_eq!(sonnet_1m.label, "Sonnet (1M context)");
        assert_eq!(sonnet_1m.context_window_tokens, Some(1_000_000));
    }

    #[test]
    fn full_catalog_covers_every_alias_without_spawn() {
        // Every alias must yield a catalog entry — discovery no longer probes per alias.
        for &alias in CLAUDE_MODEL_ALIASES {
            let option = catalog_model_option(alias);
            assert_eq!(option.id, alias);
            assert!(!option.label.is_empty());
            assert!(option.context_window_tokens.is_some());
        }
    }

    #[test]
    fn labels_match_cli_picker() {
        assert_eq!(
            label_for_claude_model("default", "claude-opus-4-8[1m]"),
            "Default (Opus 4.8 (1M context))"
        );
        assert_eq!(
            label_for_claude_model("sonnet[1m]", "claude-sonnet-4-6[1m]"),
            "Sonnet (1M context)"
        );
    }

    #[test]
    fn parse_init_info() {
        let init = json!({
            "type": "system",
            "subtype": "init",
            "model": "claude-opus-4-8[1m]"
        });
        let info = parse_claude_init_info(&init).unwrap();
        assert_eq!(info.resolved_model, "claude-opus-4-8[1m]");
        assert_eq!(info.context_window_tokens, Some(1_000_000));
    }

    #[test]
    fn parse_context_window_label_still_works() {
        use crate::external_agents::context::parse_context_window_label;
        assert_eq!(parse_context_window_label("1m"), Some(1_000_000));
        assert_eq!(parse_context_window_label("200K"), Some(200_000));
    }

    #[tokio::test]
    #[ignore = "requires local claude CLI on PATH"]
    async fn live_detect_claude_models_from_cli() {
        use crate::external_agents::detection::detect_single_agent;
        use crate::external_agents::registry::get_agent_def;

        let def = get_agent_def("claude").expect("claude agent def");
        let detected = detect_single_agent(def, &std::env::temp_dir()).await;
        assert!(detected.available, "claude CLI should be available on PATH");
        for model in &detected.models {
            println!(
                "  {} -> {} ({} tokens)",
                model.id,
                model.label,
                model
                    .context_window_tokens
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".to_string())
            );
        }
        assert!(
            detected.models.len() >= 4,
            "expected multiple probed models, got {:?}",
            detected.models
        );

        let default = detected
            .models
            .iter()
            .find(|model| model.id == "default")
            .expect("default model option");
        assert_eq!(
            default.context_window_tokens,
            Some(1_000_000),
            "default should resolve to 1M context"
        );

        let sonnet = detected
            .models
            .iter()
            .find(|model| model.id == "sonnet")
            .expect("sonnet model option");
        assert_eq!(
            sonnet.context_window_tokens,
            Some(200_000),
            "standard sonnet should be 200K"
        );

        let sonnet_1m = detected
            .models
            .iter()
            .find(|model| model.id == "sonnet[1m]")
            .expect("sonnet[1m] model option");
        assert_eq!(
            sonnet_1m.context_window_tokens,
            Some(1_000_000),
            "sonnet[1m] should be 1M"
        );
    }

    /// `effortLevel` 必须能被读出来并落在 `defs/claude.rs` 的 REASONING id 集合内，
    /// 否则前端选不中、胶囊恒显示「自动」——这正是修复前的症状。
    #[test]
    fn reads_effort_level_from_settings_json() {
        let dir = std::env::temp_dir().join(format!(
            "kivio-claude-effort-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // 复刻本机真实 settings.json 的相关片段。
        std::fs::write(
            dir.join("settings.json"),
            r#"{"effortLevel":"high","model":"opus","env":{"ANTHROPIC_AUTH_TOKEN":"sk-x"}}"#,
        )
        .unwrap();

        let prev_dir = std::env::var("CLAUDE_CONFIG_DIR").ok();
        let prev_effort = std::env::var("CLAUDE_EFFORT").ok();
        std::env::set_var("CLAUDE_CONFIG_DIR", &dir);
        std::env::remove_var("CLAUDE_EFFORT");

        assert_eq!(claude_config_effort().as_deref(), Some("high"));

        // 环境变量优先于 settings.json（与 CLI 自身的优先级一致）。
        std::env::set_var("CLAUDE_EFFORT", "max");
        assert_eq!(claude_config_effort().as_deref(), Some("max"));

        // 认不出的值一律 None（显示「自动」），不猜。
        std::env::set_var("CLAUDE_EFFORT", "turbo");
        assert_eq!(
            claude_config_effort().as_deref(),
            Some("high"),
            "环境变量非法时应回落 settings.json，而不是整个放弃"
        );

        std::fs::write(dir.join("settings.json"), r#"{"model":"opus"}"#).unwrap();
        std::env::remove_var("CLAUDE_EFFORT");
        assert_eq!(
            claude_config_effort(),
            None,
            "没配 effortLevel 时不得编一个出来"
        );

        match prev_dir {
            Some(v) => std::env::set_var("CLAUDE_CONFIG_DIR", v),
            None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
        }
        match prev_effort {
            Some(v) => std::env::set_var("CLAUDE_EFFORT", v),
            None => std::env::remove_var("CLAUDE_EFFORT"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod live_effort_tests {
    use super::*;

    /// 读本机真实 `~/.claude/settings.json`，打印实际解析出的档位。
    /// 单测喂的是构造样本，这条证明真实配置也能读到（本机 effortLevel = "high"）。
    #[test]
    #[ignore = "reads the real ~/.claude/settings.json on this machine"]
    fn live_reads_real_effort_level() {
        // 清掉环境变量，专门验证 settings.json 这条路（本机 CLAUDE_EFFORT 也是 high，
        // 不清的话分不清读到的是哪一个）。
        let prev = std::env::var("CLAUDE_EFFORT").ok();
        std::env::remove_var("CLAUDE_EFFORT");
        let from_file = claude_config_effort();
        match prev.clone() {
            Some(v) => std::env::set_var("CLAUDE_EFFORT", v),
            None => std::env::remove_var("CLAUDE_EFFORT"),
        }
        let with_env = claude_config_effort();
        eprintln!("settings.json 的 effortLevel -> {from_file:?}");
        eprintln!("含 CLAUDE_EFFORT 环境变量   -> {with_env:?}");
        eprintln!("（None 表示会显示「自动」）");
    }
}
