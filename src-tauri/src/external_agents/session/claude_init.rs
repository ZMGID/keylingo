use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use uuid::Uuid;

use crate::external_agents::registry::get_agent_def;
use crate::external_agents::slash::is_claude_init;
use crate::external_agents::spawn::{parse_json_line, spawn_agent, write_probe_stdin};
use crate::external_agents::types::{RuntimeBuildOptions, RuntimeContext, RuntimeModelOption};

/// Built-in Claude Code model catalog — **ported from desktop-cc-gui**
/// (`engine/status.rs::get_builtin_claude_models`).
///
/// Claude CLI 没有 list-models RPC。cc-gui 的做法是：
/// 1. 只暴露 4 个家族档位（Fable / Opus / Sonnet / Haiku），catalog id 钉死当前代
/// 2. `~/.claude/settings.json` 的 `env.ANTHROPIC_DEFAULT_*` / 进程 env **改写**
///    每档的 runtime model + 展示名（tier id 不变，所以四行不会因映射到同一模型而折叠）
/// 3. **不起进程**探测模型列表
///
/// 这里 id / label 与 cc-gui 字面一致；Kivio 额外在列表头保留 `default`（Auto / 不传
/// `--model`），这是本应用胶囊语义，cc-gui 没有这一行。
const CLAUDE_BUILTIN_TIERS: &[(&str, &str)] = &[
    ("claude-fable-5", "Fable 5"),
    ("claude-opus-5", "Opus 5"),
    ("claude-sonnet-5", "Sonnet 5"),
    ("claude-haiku-4-5-20251001", "Haiku 4.5"),
];

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

/// 某个 `--model` **别名**对应的上下文窗口。
///
/// **只有带 `[1m]` / `[1M]` 标记的别名能给出窗口**；裸别名（`opus` / `sonnet` / `haiku` /
/// `fable`）一律 `None`。
///
/// 改动原因（claude 2.1.220 本机 init 探测实测，2026-07-29）：
/// ```text
/// --model opus   -> claude-opus-4-8[1M]
/// --model sonnet -> claude-sonnet-5[1M]
/// --model fable  -> claude-fable-5[1M]
/// --model haiku  -> claude-sonnet-5
/// ```
/// 即**裸别名解析出什么模型完全由 CLI 当时的版本决定**，4 个里有 3 个是 1M。
/// 旧规则「在别名白名单里 ⇒ 200K」于是给出的是**小 5 倍的假分母**：`usage_ratio` 虚高 5 倍，
/// 压缩阈值在真实占用只有 20% 时就触发。spec 第 14e 条：假分母比没有分母更有害。
///
/// **为什么不改成硬编码「opus/sonnet/fable = 1M」**：那只是把同一张会过期的表换个值——
/// 下一个模型代次、`CLAUDE_CODE_MAX_CONTEXT_TOKENS` 环境覆盖、第三方 router 模型都会让它再错。
/// 真正的分母来源是 CLI 自己实报的 `result.modelUsage[model].contextWindow`
/// （见 `stream/claude.rs::context_window_from_model_usage`），它在**第一条回复之后**
/// 就成为 `context_window_for_external_model` 的最高优先级来源。首轮之前显示「满度未知」
/// 是诚实的代价，比一个错 5 倍的百分比好。
pub fn context_window_from_claude_model_alias(alias: &str) -> Option<u32> {
    let alias = alias.trim();
    if alias.is_empty() || alias == "default" {
        return None;
    }
    if alias.to_ascii_lowercase().contains("[1m]") {
        return Some(1_000_000);
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
    // 探测是一次性子进程，只为读 `system/init`。不加 `--no-session-persistence` 的话
    // claude 会把它记成一个真实会话，用户的会话列表里就多一个只含 `"."` 的空壳
    // （真机核实见 `defs::claude::ephemeral_probe_args` 的注释）。
    let args = crate::external_agents::defs::claude::ephemeral_probe_args(&(def.build_args)(
        &runtime_ctx,
        &build_options,
        None,
    ));
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

/// 构建 Claude 模型选单（对齐 desktop-cc-gui `get_claude_models`）。
///
/// - **不起进程**（`resolved_bin` / `cwd` 仅保留签名兼容；slash 探测另走 `probe_claude_init`）
/// - 4 档 builtin + settings/env 覆盖改写 label（展示名）
/// - 头上多一个 `default`（Kivio Auto：不传 `--model`）
/// - `current_model`：读 `settings.json` 的 `model` / `ANTHROPIC_MODEL`（不 spawn）
pub async fn detect_claude_models(
    _resolved_bin: &Path,
    _cwd: &Path,
) -> Option<(Vec<RuntimeModelOption>, Option<String>)> {
    Some(build_claude_model_catalog())
}

/// 同步版，单测与 `detect_claude_models` 共用。
fn build_claude_model_catalog() -> (Vec<RuntimeModelOption>, Option<String>) {
    let overrides = read_claude_model_overrides();
    let mut models = get_builtin_claude_models();
    apply_claude_model_overrides(&mut models, &overrides);

    // Kivio Auto 行：不传 `--model`，让 CLI 用自己的默认 / settings。
    let mut out = vec![RuntimeModelOption {
        id: "default".to_string(),
        label: "Default".to_string(),
        context_window_tokens: None,
    }];
    out.extend(models);

    let current_model = claude_configured_current_model(&overrides);
    (out, current_model)
}

/// 与 cc-gui `get_builtin_claude_models` 同形：catalog id + 展示名。
/// runtime model 默认 = catalog id；有覆盖时由 `apply_claude_model_overrides` 改 label，
/// 真正传给 CLI 的值在 `resolve_claude_cli_model` 里再解析一次。
fn get_builtin_claude_models() -> Vec<RuntimeModelOption> {
    CLAUDE_BUILTIN_TIERS
        .iter()
        .map(|(id, label)| RuntimeModelOption {
            id: (*id).to_string(),
            label: (*label).to_string(),
            context_window_tokens: None,
        })
        .collect()
}

#[derive(Default, Clone, Debug)]
struct ClaudeModelOverrides {
    main: Option<String>,
    fable: Option<String>,
    sonnet: Option<String>,
    opus: Option<String>,
    haiku: Option<String>,
}

fn normalize_non_empty(input: Option<String>) -> Option<String> {
    input.and_then(|value| {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

/// 读进程 env + `~/.claude/settings.json` 的 `env`（文件覆盖 env，与 cc-gui 一致）。
fn read_claude_model_overrides() -> ClaudeModelOverrides {
    let mut overrides = ClaudeModelOverrides {
        main: normalize_non_empty(std::env::var("ANTHROPIC_MODEL").ok()),
        fable: normalize_non_empty(std::env::var("ANTHROPIC_DEFAULT_FABLE_MODEL").ok()),
        sonnet: normalize_non_empty(std::env::var("ANTHROPIC_DEFAULT_SONNET_MODEL").ok()),
        opus: normalize_non_empty(std::env::var("ANTHROPIC_DEFAULT_OPUS_MODEL").ok()),
        haiku: normalize_non_empty(std::env::var("ANTHROPIC_DEFAULT_HAIKU_MODEL").ok()),
    };

    if let Some(file_overrides) = read_claude_model_overrides_from_settings() {
        if file_overrides.main.is_some() {
            overrides.main = file_overrides.main;
        }
        if file_overrides.fable.is_some() {
            overrides.fable = file_overrides.fable;
        }
        if file_overrides.sonnet.is_some() {
            overrides.sonnet = file_overrides.sonnet;
        }
        if file_overrides.opus.is_some() {
            overrides.opus = file_overrides.opus;
        }
        if file_overrides.haiku.is_some() {
            overrides.haiku = file_overrides.haiku;
        }
    }

    overrides
}

fn read_claude_model_overrides_from_settings() -> Option<ClaudeModelOverrides> {
    let path = claude_config_dir()?.join("settings.json");
    let content = std::fs::read_to_string(path).ok()?;
    let root = serde_json::from_str::<Value>(&content).ok()?;
    let env = root.get("env")?;
    Some(ClaudeModelOverrides {
        main: normalize_non_empty(
            env.get("ANTHROPIC_MODEL")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        ),
        fable: normalize_non_empty(
            env.get("ANTHROPIC_DEFAULT_FABLE_MODEL")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        ),
        sonnet: normalize_non_empty(
            env.get("ANTHROPIC_DEFAULT_SONNET_MODEL")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        ),
        opus: normalize_non_empty(
            env.get("ANTHROPIC_DEFAULT_OPUS_MODEL")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        ),
        haiku: normalize_non_empty(
            env.get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        ),
    })
}

/// Infer Claude model family for ANTHROPIC_DEFAULT_* slot resolution (cc-gui).
fn claude_model_family_key(model_id: &str) -> Option<&'static str> {
    let normalized = model_id.to_ascii_lowercase();
    if normalized.contains("fable") {
        return Some("fable");
    }
    if normalized.contains("haiku") {
        return Some("haiku");
    }
    if normalized.contains("sonnet") {
        return Some("sonnet");
    }
    if normalized.contains("opus") {
        return Some("opus");
    }
    None
}

fn resolve_override_for_family<'a>(
    family: &str,
    overrides: &'a ClaudeModelOverrides,
) -> Option<&'a str> {
    let tier = match family {
        "fable" => overrides.fable.as_deref(),
        "haiku" => overrides.haiku.as_deref(),
        "sonnet" => overrides.sonnet.as_deref(),
        "opus" => overrides.opus.as_deref(),
        _ => None,
    };
    tier.or(overrides.main.as_deref())
}

/// 把 settings/env 映射写到展示名上（cc-gui：改 `model` + `name`，tier id 不变）。
/// Kivio 的 `RuntimeModelOption` 只有 id/label：id 保持 catalog，label 改成映射后的 runtime id。
fn apply_claude_model_overrides(
    models: &mut [RuntimeModelOption],
    overrides: &ClaudeModelOverrides,
) {
    let has_any = overrides.main.is_some()
        || overrides.fable.is_some()
        || overrides.sonnet.is_some()
        || overrides.opus.is_some()
        || overrides.haiku.is_some();
    if !has_any {
        return;
    }

    for model in models.iter_mut() {
        let Some(family) = claude_model_family_key(&model.id) else {
            continue;
        };
        let Some(mapped) = resolve_override_for_family(family, overrides) else {
            continue;
        };
        // 展示映射后的 runtime id（与 cc-gui 把 name 改成 mapped 一致）。
        model.label = mapped.to_string();
    }
}

/// 选单里存的是 catalog id（如 `claude-sonnet-5`）；真正传给 `--model` 的是
/// 映射后的 runtime id。无映射时原样返回。
///
/// 对齐 cc-gui：picker 身份与 CLI 透传值分离，四档映射到同一 runtime 也不会折叠。
pub fn resolve_claude_cli_model(selected: &str) -> String {
    let selected = selected.trim();
    if selected.is_empty() || selected == "default" {
        return selected.to_string();
    }
    // 只对 builtin catalog id 做家族映射；用户手填 / 旧会话里的自定义 id 原样透传。
    let is_catalog = CLAUDE_BUILTIN_TIERS.iter().any(|(id, _)| *id == selected);
    if !is_catalog {
        return selected.to_string();
    }
    let overrides = read_claude_model_overrides();
    if let Some(family) = claude_model_family_key(selected) {
        if let Some(mapped) = resolve_override_for_family(family, &overrides) {
            return mapped.to_string();
        }
    }
    selected.to_string()
}

/// Wire model for Claude CLI: `None` = omit `--model` / skip set_model (Auto / empty).
///
/// Catalog ids go through settings/env mapping; non-catalog values pass through unchanged.
/// Call this at **every** boundary that hands a model to the CLI (launch argv, set_model,
/// reconnect) — not only at first spawn.
pub fn claude_wire_model(selected: Option<&str>) -> Option<String> {
    let selected = selected.map(str::trim).filter(|s| !s.is_empty())?;
    if selected == "default" {
        return None;
    }
    let runtime = resolve_claude_cli_model(selected);
    if runtime.is_empty() || runtime == "default" {
        None
    } else {
        Some(runtime)
    }
}

/// Whether a live session must `set_model` to match `selected`.
///
/// Returns `Some(runtime_id)` when a change is needed, `None` when already on that runtime
/// (or when Auto/empty — leave session model alone).
pub fn needs_set_model(active: Option<&str>, selected: &str) -> Option<String> {
    let runtime = claude_wire_model(Some(selected))?;
    if active.map(str::trim) == Some(runtime.as_str()) {
        None
    } else {
        Some(runtime)
    }
}

/// Map Claude settings / CLI alias strings onto a curated catalog id for picker backfill.
///
/// Returns `None` for free-form gateway ids that are not a known family (display-only).
pub fn map_claude_config_to_catalog_id(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw == "default" {
        return None;
    }
    if CLAUDE_BUILTIN_TIERS.iter().any(|(id, _)| *id == raw) {
        return Some(raw.to_string());
    }
    // Bare aliases + concrete ids → current curated tier.
    match claude_model_family_key(raw) {
        Some("fable") => Some("claude-fable-5".to_string()),
        Some("opus") => Some("claude-opus-5".to_string()),
        Some("sonnet") => Some("claude-sonnet-5".to_string()),
        Some("haiku") => Some("claude-haiku-4-5-20251001".to_string()),
        _ => None,
    }
}

/// 胶囊「当前模型」：settings.json 顶层 `model`，否则 `ANTHROPIC_MODEL` / main 覆盖。
/// 能映射到 catalog 的返回 catalog id（便于 RuntimePicker 回填）；否则返回 raw 仅展示。
/// 不 spawn CLI（cc-gui 同样不起进程）。
fn claude_configured_current_model(overrides: &ClaudeModelOverrides) -> Option<String> {
    let raw = if let Some(path) = claude_config_dir().map(|d| d.join("settings.json")) {
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Ok(root) = serde_json::from_str::<Value>(&content) {
                if let Some(model) = root
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    Some(model.to_string())
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    }
    .or_else(|| overrides.main.clone())?;

    Some(map_claude_config_to_catalog_id(&raw).unwrap_or(raw))
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

/// 用户在 Claude Code 里配的推理档位（`settings.json` 的 `effortLevel` / `ultracode`，
/// 或进程环境的 `CLAUDE_CODE_EFFORT_LEVEL`）。
///
/// 与 codex/pi/kimi 的「读本地配置回填当前档位」是同一件事——claude 此前漏了这条，
/// 于是胶囊上恒显示「自动」，哪怕用户明明配了 `effortLevel: "high"`。
///
/// 优先级（与 CLI 自身一致，2.1.220 二进制核实）：
/// `CLAUDE_CODE_EFFORT_LEVEL` 环境变量 > `settings.json` 的 `ultracode: true`
/// （CLI 里 `if(settings.ultracode===true) return "xhigh"`，我们回填成 `ultracode` 档）
/// > `settings.json` 的 `effortLevel`。
///
/// **环境变量名此前是错的**：原来读的是 `CLAUDE_EFFORT`，但二进制里那是个**输出**变量——
/// CLI 把「本轮实际生效的档位」导给 hook / Bash 用（zod 描述原文："Also exposed to hook
/// commands and Bash as the CLAUDE_EFFORT env var"）。真正的输入覆盖是
/// `CLAUDE_CODE_EFFORT_LEVEL`（`ait()` 只读它，且 `unset` / `auto` 视为「没配」）。
/// 读错的后果不是「读不到」而是**读到别人的**：Kivio 若从 Claude Code 内启动，
/// `CLAUDE_EFFORT` 会被继承进来（本机 `env | grep CLAUDE` 可见，且它不在
/// `spawn::PARENT_SESSION_ENV_VARS` 的剥离清单里），胶囊显示的就是宿主那一轮的档位，
/// 与用户的 claude 配置无关。

///
/// 返回值必须落在 `defs/claude.rs` 的 `REASONING` 选项 id 集合内，否则前端选不中；
/// 认不出的值一律 `None`（显示「自动」），不猜、不 panic。
pub fn claude_config_effort() -> Option<String> {
    const KNOWN: &[&str] = &["low", "medium", "high", "xhigh", "max", "ultracode"];
    let normalize = |raw: &str| -> Option<String> {
        let value = raw.trim().to_ascii_lowercase();
        // CLI 自己把这两个当「没配」（`ait()`），我们跟随，否则会显示一个不存在的档位。
        if value == "unset" || value == "auto" {
            return None;
        }
        KNOWN.contains(&value.as_str()).then_some(value)
    };

    if let Ok(raw) = std::env::var("CLAUDE_CODE_EFFORT_LEVEL") {
        if let Some(effort) = normalize(&raw) {
            return Some(effort);
        }
    }

    let text = claude_config_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join("settings.json")).ok())?;
    let value = serde_json::from_str::<Value>(&text).ok()?;
    if value.get("ultracode").and_then(|v| v.as_bool()) == Some(true) {
        return Some("ultracode".to_string());
    }
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

    /// `CLAUDE_CONFIG_DIR` / `CLAUDE_CODE_EFFORT_LEVEL` 是**进程级**环境变量，而 cargo 默认
    /// 并发跑测试 —— 两个都改它们的用例并行时会读到对方的 settings.json（实测：一条用例
    /// 期望 `ultracode` 却拿到另一条写的 `high`）。用一把锁串起来，别靠运气。
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

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
        // 裸别名一律 None。本机实测（claude 2.1.220）`--model sonnet` 解析为
        // `claude-sonnet-5[1M]`、`opus` → `claude-opus-4-8[1M]`、`fable` → `claude-fable-5[1M]`，
        // 只有 `haiku` → `claude-sonnet-5`。旧规则「白名单里 ⇒ 200K」于是 4 个里错 3 个，
        // 且是**小 5 倍的假分母**（压缩阈值在 20% 就触发）。真正的分母走 CLI 实报的
        // `result.modelUsage[model].contextWindow`。
        assert_eq!(context_window_from_claude_model_alias("sonnet"), None);
        assert_eq!(context_window_from_claude_model_alias("opus"), None);
        assert_eq!(context_window_from_claude_model_alias("haiku"), None);
        assert_eq!(context_window_from_claude_model_alias("fable"), None);
        assert_eq!(context_window_from_claude_model_alias("default"), None);
        // 大写 `[1M]`（CLI 实际输出的形态）同样要认。
        assert_eq!(
            context_window_from_claude_model_alias("opus[1M]"),
            Some(1_000_000)
        );
    }

    #[test]
    fn builtin_catalog_matches_cc_gui() {
        // 与 desktop-cc-gui `get_builtin_claude_models` 字面一致。
        assert_eq!(CLAUDE_BUILTIN_TIERS.len(), 4);
        assert_eq!(
            CLAUDE_BUILTIN_TIERS,
            &[
                ("claude-fable-5", "Fable 5"),
                ("claude-opus-5", "Opus 5"),
                ("claude-sonnet-5", "Sonnet 5"),
                ("claude-haiku-4-5-20251001", "Haiku 4.5"),
            ]
        );
        // 不提供裸别名 / 冷门 / 历史版本堆。
        for noise in [
            "sonnet",
            "opus",
            "haiku",
            "fable",
            "best",
            "opusplan",
            "sonnet[1m]",
            "claude-opus-4-8",
            "claude-mythos-5",
        ] {
            assert!(
                !CLAUDE_BUILTIN_TIERS.iter().any(|(id, _)| *id == noise),
                "{noise} 不应出现在 builtin catalog"
            );
        }
    }

    /// 把 CLAUDE_CONFIG_DIR 指到空目录，避免读到本机真实 `~/.claude/settings.json`。
    fn isolate_claude_config() -> (PathBuf, Option<String>) {
        let dir = std::env::temp_dir().join(format!(
            "kivio-claude-cfg-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("CLAUDE_CONFIG_DIR").ok();
        std::env::set_var("CLAUDE_CONFIG_DIR", &dir);
        for key in [
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_FABLE_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        ] {
            std::env::remove_var(key);
        }
        (dir, prev)
    }

    fn restore_claude_config(dir: PathBuf, prev: Option<String>) {
        match prev {
            Some(v) => std::env::set_var("CLAUDE_CONFIG_DIR", v),
            None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn catalog_is_default_plus_four_tiers_without_spawn() {
        let _guard = env_lock();
        let (dir, prev) = isolate_claude_config();
        let (models, _) = build_claude_model_catalog();
        assert_eq!(models.len(), 5, "default + 4 tiers: {:?}", models);
        assert_eq!(models[0].id, "default");
        for (i, (id, label)) in CLAUDE_BUILTIN_TIERS.iter().enumerate() {
            assert_eq!(models[i + 1].id, *id);
            assert_eq!(models[i + 1].label, *label);
        }
        // 裸别名不得出现。
        assert!(!models.iter().any(|m| m.id == "sonnet" || m.id == "opus"));
        restore_claude_config(dir, prev);
    }

    #[test]
    fn settings_overrides_rewrite_labels_keep_catalog_ids() {
        let _guard = env_lock();
        let dir = std::env::temp_dir().join(format!(
            "kivio-claude-override-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{
              "env": {
                "ANTHROPIC_MODEL": "MiniMax-M1[1m]",
                "ANTHROPIC_DEFAULT_FABLE_MODEL": "kimi-k3",
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "GLM-5.1",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "MiniMax-M4[1m]",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "deepseek-v4-flash"
              }
            }"#,
        )
        .unwrap();
        let prev = std::env::var("CLAUDE_CONFIG_DIR").ok();
        std::env::set_var("CLAUDE_CONFIG_DIR", &dir);
        for key in [
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_FABLE_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        ] {
            std::env::remove_var(key);
        }

        let (models, _) = build_claude_model_catalog();
        // tier id 稳定；label 变成映射后的 runtime id（cc-gui 同款）。
        let fable = models.iter().find(|m| m.id == "claude-fable-5").unwrap();
        assert_eq!(fable.label, "kimi-k3");
        let opus = models.iter().find(|m| m.id == "claude-opus-5").unwrap();
        assert_eq!(opus.label, "MiniMax-M4[1m]");
        let sonnet = models.iter().find(|m| m.id == "claude-sonnet-5").unwrap();
        assert_eq!(sonnet.label, "GLM-5.1");
        let haiku = models
            .iter()
            .find(|m| m.id == "claude-haiku-4-5-20251001")
            .unwrap();
        assert_eq!(haiku.label, "deepseek-v4-flash");
        // 四档都在，不会因映射折叠。
        assert_eq!(
            models
                .iter()
                .filter(|m| m.id.starts_with("claude-"))
                .count(),
            4
        );

        // 传给 CLI 的是 runtime，不是 catalog id。
        assert_eq!(resolve_claude_cli_model("claude-fable-5"), "kimi-k3");
        assert_eq!(resolve_claude_cli_model("claude-sonnet-5"), "GLM-5.1");
        assert_eq!(resolve_claude_cli_model("claude-opus-5"), "MiniMax-M4[1m]");
        // 非 catalog id 原样透传。
        assert_eq!(resolve_claude_cli_model("already-custom"), "already-custom");
        assert_eq!(resolve_claude_cli_model("default"), "default");
        // resolve 幂等：runtime id 再 resolve 不变。
        assert_eq!(resolve_claude_cli_model("GLM-5.1"), "GLM-5.1");
        // wire / needs_set_model：多轮已在 runtime 上时不再 set_model。
        assert_eq!(
            claude_wire_model(Some("claude-sonnet-5")).as_deref(),
            Some("GLM-5.1")
        );
        assert_eq!(
            needs_set_model(Some("GLM-5.1"), "claude-sonnet-5"),
            None,
            "active already on mapped runtime — no set_model"
        );
        assert_eq!(
            needs_set_model(Some("kimi-k3"), "claude-sonnet-5").as_deref(),
            Some("GLM-5.1"),
            "switching tiers must emit mapped runtime id"
        );
        assert_eq!(claude_wire_model(Some("default")), None);
        assert_eq!(claude_wire_model(None), None);

        match prev {
            Some(v) => std::env::set_var("CLAUDE_CONFIG_DIR", v),
            None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn map_config_aliases_to_catalog_for_picker_backfill() {
        assert_eq!(
            map_claude_config_to_catalog_id("opus").as_deref(),
            Some("claude-opus-5")
        );
        assert_eq!(
            map_claude_config_to_catalog_id("sonnet").as_deref(),
            Some("claude-sonnet-5")
        );
        assert_eq!(
            map_claude_config_to_catalog_id("claude-sonnet-5").as_deref(),
            Some("claude-sonnet-5")
        );
        assert_eq!(
            map_claude_config_to_catalog_id("claude-opus-4-8").as_deref(),
            Some("claude-opus-5")
        );
        assert_eq!(map_claude_config_to_catalog_id("my-gateway-x"), None);
        assert_eq!(map_claude_config_to_catalog_id("default"), None);
    }

    #[test]
    fn all_tiers_map_to_same_runtime_without_collapse() {
        let _guard = env_lock();
        let (dir, prev) = isolate_claude_config();
        std::env::set_var("ANTHROPIC_DEFAULT_FABLE_MODEL", "kimi-k3");
        std::env::set_var("ANTHROPIC_DEFAULT_SONNET_MODEL", "kimi-k3");
        std::env::set_var("ANTHROPIC_DEFAULT_OPUS_MODEL", "kimi-k3");
        std::env::set_var("ANTHROPIC_DEFAULT_HAIKU_MODEL", "kimi-k3");

        let (models, _) = build_claude_model_catalog();
        let tiers: Vec<_> = models
            .iter()
            .filter(|m| m.id.starts_with("claude-"))
            .collect();
        assert_eq!(tiers.len(), 4);
        assert!(tiers.iter().all(|m| m.label == "kimi-k3"));
        // id 互不相同 —— 选中哪一档仍然可区分。
        let mut ids = std::collections::HashSet::new();
        for m in &tiers {
            assert!(ids.insert(m.id.as_str()), "duplicate catalog id {}", m.id);
        }

        for key in [
            "ANTHROPIC_DEFAULT_FABLE_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        ] {
            std::env::remove_var(key);
        }
        restore_claude_config(dir, prev);
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

    /// `effortLevel` 必须能被读出来并落在 `defs/claude.rs` 的 REASONING id 集合内，
    /// 否则前端选不中、胶囊恒显示「自动」——这正是修复前的症状。
    #[test]
    fn reads_effort_level_from_settings_json() {
        let _guard = env_lock();
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
        let prev_effort = std::env::var("CLAUDE_CODE_EFFORT_LEVEL").ok();
        std::env::set_var("CLAUDE_CONFIG_DIR", &dir);
        std::env::remove_var("CLAUDE_CODE_EFFORT_LEVEL");

        assert_eq!(claude_config_effort().as_deref(), Some("high"));

        // 环境变量优先于 settings.json（与 CLI 自身的优先级一致）。
        std::env::set_var("CLAUDE_CODE_EFFORT_LEVEL", "max");
        assert_eq!(claude_config_effort().as_deref(), Some("max"));

        // 认不出的值一律 None（显示「自动」），不猜。
        std::env::set_var("CLAUDE_CODE_EFFORT_LEVEL", "turbo");
        assert_eq!(
            claude_config_effort().as_deref(),
            Some("high"),
            "环境变量非法时应回落 settings.json，而不是整个放弃"
        );

        std::fs::write(dir.join("settings.json"), r#"{"model":"opus"}"#).unwrap();
        std::env::remove_var("CLAUDE_CODE_EFFORT_LEVEL");
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
            Some(v) => std::env::set_var("CLAUDE_CODE_EFFORT_LEVEL", v),
            None => std::env::remove_var("CLAUDE_CODE_EFFORT_LEVEL"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `settings.json` 的 `ultracode: true` 要回填成 `ultracode` 档，而不是「自动」。
    /// 另外 `unset` / `auto` 是 CLI 自己认的「没配」，不能当成一个真实档位显示。
    #[test]
    fn reads_ultracode_and_treats_unset_auto_as_unconfigured() {
        let _guard = env_lock();
        let dir = std::env::temp_dir().join(format!(
            "kivio-claude-ultracode-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let prev_dir = std::env::var("CLAUDE_CONFIG_DIR").ok();
        let prev_effort = std::env::var("CLAUDE_CODE_EFFORT_LEVEL").ok();
        std::env::set_var("CLAUDE_CONFIG_DIR", &dir);
        std::env::remove_var("CLAUDE_CODE_EFFORT_LEVEL");

        // ultracode 优先于 effortLevel（CLI 里也是这个顺序）。
        std::fs::write(
            dir.join("settings.json"),
            r#"{"ultracode":true,"effortLevel":"low"}"#,
        )
        .unwrap();
        assert_eq!(claude_config_effort().as_deref(), Some("ultracode"));

        // ultracode:false 不算配置，继续看 effortLevel。
        std::fs::write(
            dir.join("settings.json"),
            r#"{"ultracode":false,"effortLevel":"low"}"#,
        )
        .unwrap();
        assert_eq!(claude_config_effort().as_deref(), Some("low"));

        // 环境变量的 unset / auto = 「没配」，要落回文件而不是显示一个假档位。
        std::env::set_var("CLAUDE_CODE_EFFORT_LEVEL", "auto");
        assert_eq!(claude_config_effort().as_deref(), Some("low"));
        std::env::set_var("CLAUDE_CODE_EFFORT_LEVEL", "unset");
        assert_eq!(claude_config_effort().as_deref(), Some("low"));
        // 环境变量也认 ultracode。
        std::env::set_var("CLAUDE_CODE_EFFORT_LEVEL", "ultracode");
        assert_eq!(claude_config_effort().as_deref(), Some("ultracode"));

        match prev_dir {
            Some(v) => std::env::set_var("CLAUDE_CONFIG_DIR", v),
            None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
        }
        match prev_effort {
            Some(v) => std::env::set_var("CLAUDE_CODE_EFFORT_LEVEL", v),
            None => std::env::remove_var("CLAUDE_CODE_EFFORT_LEVEL"),
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
        // 清掉环境变量，专门验证 settings.json 这条路。
        let prev = std::env::var("CLAUDE_CODE_EFFORT_LEVEL").ok();
        std::env::remove_var("CLAUDE_CODE_EFFORT_LEVEL");
        let from_file = claude_config_effort();
        match prev.clone() {
            Some(v) => std::env::set_var("CLAUDE_CODE_EFFORT_LEVEL", v),
            None => std::env::remove_var("CLAUDE_CODE_EFFORT_LEVEL"),
        }
        let with_env = claude_config_effort();
        eprintln!("settings.json 的 effortLevel -> {from_file:?}");
        eprintln!("含 CLAUDE_CODE_EFFORT_LEVEL 环境变量   -> {with_env:?}");
        eprintln!("（None 表示会显示「自动」）");
    }
}

/// 真机验收：探测残渣与思考档位这两类「效果对不对」的改动，单测只能证明 flag 拼进了命令行，
/// 证明不了 CLI 真的照做（spec 第 15 条）。这里断言可证伪的量：探测前后的会话文件数、
/// 同一 prompt 下 thinking 块的个数。
#[cfg(test)]
mod live_probe_hygiene_tests {
    use super::*;
    use crate::external_agents::spawn::{
        resolve_binary, spawn_agent, stream_json_user_line, write_probe_stdin,
    };

    /// claude 把会话落在 `~/.claude/projects/<cwd 编码>/<session-id>.jsonl`。
    /// 编码规则：cwd 里每个**非字母数字**字符逐个换成 `-`（不折叠连续分隔符）。
    /// 本机核验：`C:\Users\11028\AppData\Roaming\com.zmair.kivio\chat-workspaces\__global__`
    /// → `C--Users-11028-AppData-Roaming-com-zmair-kivio-chat-workspaces---global--`。
    fn claude_project_dir_for(cwd: &Path) -> PathBuf {
        let encoded: String = cwd
            .to_string_lossy()
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect();
        claude_config_dir()
            .expect("claude config dir")
            .join("projects")
            .join(encoded)
    }

    fn count_session_files(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| {
                        e.path()
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    /// 起一次探测子进程，读到 `system/init` 后**再多活 5 秒**才收尾。
    ///
    /// 那 5 秒是关键：CLI 是在处理完第一条用户消息之后才把会话写盘的（本机实测 init 约
    /// 3.3s 到达、写盘还要再晚一点）。生产代码读到 init 就立刻 kill，往往抢在写盘之前，
    /// 于是残渣是**概率性**出现的 —— 若不加这段等待，对照组可能一个文件都不落，测试就
    /// 变成了一条永远绿的假验证。
    async fn probe_once(bin: &Path, cwd: &Path, ephemeral: bool) -> bool {
        let def = get_agent_def("claude").expect("claude def");
        let base = (def.build_args)(
            &RuntimeContext {
                extra_allowed_dirs: Vec::new(),
                resume_session_id: None,
                new_session_id: Some(Uuid::new_v4().to_string()),
                include_partial_messages: false,
            },
            &RuntimeBuildOptions {
                model: None,
                reasoning: None,
                sandbox: None,
            },
            None,
        );
        let args = if ephemeral {
            crate::external_agents::defs::claude::ephemeral_probe_args(&base)
        } else {
            base
        };
        let Ok(mut spawned) =
            spawn_agent(def, bin, &args, cwd, &std::collections::HashMap::new()).await
        else {
            return false;
        };
        if write_probe_stdin(&mut spawned.child).await.is_err() {
            return false;
        }
        let got_init = read_claude_init_value(&mut spawned.child, Duration::from_secs(30))
            .await
            .is_some();
        tokio::time::sleep(Duration::from_secs(5)).await;
        let _ = spawned.child.start_kill();
        let _ = spawned.child.wait().await;
        // 给 CLI 收尾写盘留一点时间。
        tokio::time::sleep(Duration::from_millis(1500)).await;
        got_init
    }

    /// **探测不得在用户的 claude 会话列表里留下空壳会话。**
    /// 对照组（不带 flag）必须真的落一个文件，否则这条测试证明不了任何事。
    #[tokio::test]
    #[ignore = "spawns the real claude CLI and inspects ~/.claude/projects"]
    async fn live_probe_leaves_no_shell_session() {
        let def = get_agent_def("claude").expect("claude def");
        let Some(bin) = resolve_binary(def).await else {
            eprintln!("skip: claude 不在 PATH 上");
            return;
        };

        let cwd = std::env::temp_dir().join(format!(
            "kivio-probe-hygiene-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&cwd).unwrap();
        let project_dir = claude_project_dir_for(&cwd);
        eprintln!("探测 cwd      : {}", cwd.display());
        eprintln!("会话落盘目录  : {}", project_dir.display());

        // 对照组：不带 --no-session-persistence。
        if !probe_once(&bin, &cwd, false).await {
            eprintln!(
                "skip: 探测没能拿到 system/init（未登录 / 网络问题？先手动跑一次 `claude -p hi`）"
            );
            let _ = std::fs::remove_dir_all(&cwd);
            return;
        }
        let baseline = count_session_files(&project_dir);
        eprintln!("不带 flag 探测一次后：{baseline} 个会话文件");
        assert!(
            baseline >= 1,
            "对照组没落下任何会话文件，这条测试无法证伪 —— 要么 claude 改了落盘时机，\
             要么等待时间不够。看 {}",
            project_dir.display()
        );

        // 实验组：带上 flag，文件数**不得增加**。
        assert!(
            probe_once(&bin, &cwd, true).await,
            "带 --no-session-persistence 后探测拿不到 init 了 —— 这个 flag 不能用"
        );
        let after = count_session_files(&project_dir);
        eprintln!("带 flag 再探一次后：{after} 个会话文件");
        assert_eq!(
            after, baseline,
            "带了 --no-session-persistence 还是落了会话文件（{baseline} → {after}）"
        );

        let _ = std::fs::remove_dir_all(&project_dir);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// 跑完一整轮，返回 assistant 帧里 `thinking` 块的个数（`None` = 这一轮没跑起来）。
    async fn count_thinking_blocks(bin: &Path, cwd: &Path, reasoning: Option<&str>) -> Option<u32> {
        use tokio::io::AsyncBufReadExt;

        let def = get_agent_def("claude").expect("claude def");
        let args = crate::external_agents::defs::claude::ephemeral_probe_args(&(def.build_args)(
            &RuntimeContext {
                extra_allowed_dirs: Vec::new(),
                resume_session_id: None,
                new_session_id: Some(Uuid::new_v4().to_string()),
                include_partial_messages: false,
            },
            &RuntimeBuildOptions {
                model: None,
                reasoning: reasoning.map(str::to_string),
                sandbox: None,
            },
            None,
        ));
        eprintln!("  args: {}", args.join(" "));
        let mut spawned = spawn_agent(def, bin, &args, cwd, &std::collections::HashMap::new())
            .await
            .ok()?;
        // 一道需要推导的题：思考开着时模型基本一定会思考，关掉后必须一个块都没有。
        let prompt = "A bat and a ball cost 1.10 in total. The bat costs 1.00 more than the ball. \
                      How much does the ball cost? Think it through carefully before answering.";
        {
            use tokio::io::AsyncWriteExt;
            let mut stdin = spawned.child.stdin.take()?;
            stdin
                .write_all(stream_json_user_line(prompt, &[]).ok()?.as_bytes())
                .await
                .ok()?;
        }
        let stdout = spawned.child.stdout.take()?;
        let mut reader = tokio::io::BufReader::new(stdout).lines();
        let mut thinking = 0u32;
        let mut ok = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(240);
        while tokio::time::Instant::now() < deadline {
            let line = match tokio::time::timeout(Duration::from_secs(5), reader.next_line()).await
            {
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) | Ok(Err(_)) => break,
                Err(_) => continue,
            };
            let Some(value) = parse_json_line(&line) else {
                continue;
            };
            match value.get("type").and_then(|v| v.as_str()) {
                Some("assistant") => {
                    if let Some(blocks) =
                        value.pointer("/message/content").and_then(|v| v.as_array())
                    {
                        thinking += blocks
                            .iter()
                            .filter(|b| b.get("type").and_then(|v| v.as_str()) == Some("thinking"))
                            .count() as u32;
                    }
                }
                Some("result") => {
                    ok = value.get("is_error").and_then(|v| v.as_bool()) != Some(true);
                    if !ok {
                        eprintln!(
                            "  result 报错：{}",
                            value.get("result").unwrap_or(&Value::Null)
                        );
                    }
                    break;
                }
                _ => {}
            }
        }
        let _ = spawned.child.start_kill();
        let _ = spawned.child.wait().await;
        ok.then_some(thinking)
    }

    /// **「关闭思考」这一档必须真的关掉思考。** 单测只能证明 `--thinking disabled` 拼进了
    /// 命令行；这条断言同一 prompt 下 thinking 块个数从「有」变成 0。
    #[tokio::test]
    #[ignore = "spawns the real claude CLI and burns two full turns"]
    async fn live_thinking_off_really_stops_thinking() {
        let def = get_agent_def("claude").expect("claude def");
        let Some(bin) = resolve_binary(def).await else {
            eprintln!("skip: claude 不在 PATH 上");
            return;
        };
        let cwd = std::env::temp_dir();

        eprintln!("--- 档位 high（对照组）---");
        let Some(with_thinking) = count_thinking_blocks(&bin, &cwd, Some("high")).await else {
            eprintln!("skip: 对照组这一轮没跑成（未登录 / 网络问题？）");
            return;
        };
        eprintln!("--- 档位 off ---");
        let Some(without) = count_thinking_blocks(&bin, &cwd, Some("off")).await else {
            eprintln!("skip: off 这一轮没跑成（未登录 / 网络问题？）");
            return;
        };
        eprintln!("thinking 块个数：high={with_thinking} off={without}");
        assert!(
            with_thinking > 0,
            "对照组一个 thinking 块都没有，这条测试无法证伪（换个更需要推导的 prompt）"
        );
        assert_eq!(without, 0, "选了「关闭思考」却仍在思考");
    }

    /// `--effort ultracode` 必须被 CLI 认下来。判据是**可证伪的**：CLI 对认不出的
    /// `--effort` 会在 stderr 打 `Warning: Unknown --effort value …` 然后按默认档跑，
    /// 所以「胡编的值有这句 warning、ultracode 没有」就证明了它是合法取值。
    #[tokio::test]
    #[ignore = "spawns the real claude CLI"]
    async fn live_ultracode_is_a_recognized_effort_value() {
        let def = get_agent_def("claude").expect("claude def");
        let Some(bin) = resolve_binary(def).await else {
            eprintln!("skip: claude 不在 PATH 上");
            return;
        };

        async fn stderr_of(bin: &Path, effort: &str) -> String {
            let out = crate::external_agents::spawn::cli_command(bin)
                .args(["-p", "--effort", effort, "--no-session-persistence"])
                .current_dir(std::env::temp_dir())
                .stdin(std::process::Stdio::null())
                .output()
                .await;
            out.map(|o| String::from_utf8_lossy(&o.stderr).to_string())
                .unwrap_or_default()
        }

        let bogus = stderr_of(&bin, "totallybogus").await;
        eprintln!("bogus stderr: {}", bogus.trim());
        if !bogus.to_lowercase().contains("unknown --effort value") {
            eprintln!("skip: CLI 不再打这句 warning，判据失效——换判据前别改结论");
            return;
        }
        let ultracode = stderr_of(&bin, "ultracode").await;
        eprintln!("ultracode stderr: {}", ultracode.trim());
        assert!(
            !ultracode.to_lowercase().contains("unknown --effort value"),
            "CLI 不认 `--effort ultracode` 了，这一档会静默降级成默认档"
        );
    }
}
