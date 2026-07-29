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
///
/// 取值来自 claude 2.1.220 二进制里的别名白名单
/// `["sonnet","opus","haiku","fable","best","sonnet[1m]","opus[1m]","fable[1m]","opusplan"]`
/// （spec 第 21 条：把本机装的二进制当文档查阅）。`haiku[1m]` **有意不在**里面 —— 白名单里
/// 就没有它，尽管 haiku 的 catalog 条目标着 `supports_1m_suffix`。
const CLAUDE_MODEL_ALIASES: &[&str] = &[
    "opus",
    "sonnet",
    "sonnet[1m]",
    "opus[1m]",
    "haiku",
    "fable",
    "fable[1m]",
    "best",
    "opusplan",
];

/// 具体版本（钉死某一代模型，别名解析到哪一代由 CLI 版本决定，用户想退回上一版时需要这个）。
///
/// **数据来源**：claude 2.1.220 二进制里烘进去的模型目录（`{id,family,display_name,…}` 数组，
/// 共 17 条），原样抄出 `id` + `display_name`，一个字没编。spec 第 21 条认可这种「把本机
/// 二进制当文档查」的做法。
///
/// **有意不带上下文窗口**：catalog 里确实有 `context:{window,…}`，但那正是 spec 第 14g 条
/// 明令不许再建的那张表 —— 窗口会被 `CLAUDE_CODE_MAX_CONTEXT_TOKENS`、1M beta、第三方 router
/// 改写，只有 CLI 自己每轮实报的 `result.modelUsage[model].contextWindow` 准。这里只回答
/// 「有哪些可选」。
///
/// **为什么不做「每个模型支持哪些 effort 档」的门控**：catalog 里有
/// `capabilities:["effort","max_effort","xhigh_effort",…]`（如 Sonnet 4.6 就**没有**
/// `xhigh_effort`），但真机核实过 CLI 对超出能力的档位是**静默降级**而非报错：
/// 未知 `--effort` 只在 stderr 打一句 warning 后按默认档跑，`--thinking disabled` 撞上
/// `rejects_disabled_thinking` 的 Fable 5 时被直接忽略（实测仍产出 2 个 thinking 块、
/// 不报错）。也就是说门控纯属观感，代价却是再养一张会过期的能力表 —— 不划算。
const CLAUDE_CONCRETE_MODELS: &[(&str, &str)] = &[
    ("claude-opus-5", "Opus 5"),
    ("claude-opus-4-8", "Opus 4.8"),
    ("claude-opus-4-7", "Opus 4.7"),
    ("claude-opus-4-6", "Opus 4.6"),
    ("claude-opus-4-5", "Opus 4.5"),
    ("claude-opus-4-1", "Opus 4.1"),
    ("claude-opus-4-0", "Opus 4"),
    ("claude-sonnet-5", "Sonnet 5"),
    ("claude-sonnet-4-6", "Sonnet 4.6"),
    ("claude-sonnet-4-5", "Sonnet 4.5"),
    ("claude-sonnet-4-0", "Sonnet 4"),
    ("claude-3-7-sonnet", "Sonnet 3.7"),
    ("claude-3-5-sonnet", "Sonnet 3.5"),
    ("claude-fable-5", "Fable 5"),
    ("claude-mythos-5", "Mythos 5"),
    ("claude-haiku-4-5", "Haiku 4.5"),
    ("claude-3-5-haiku", "Haiku 3.5"),
];

/// 上面那张具体版本表是从**哪个** CLI 版本的二进制里读出来的。
///
/// 这是版本门控的判据：装的 CLI 比它旧时**不提供**具体版本条目，只留别名 + 用户自配模型
/// （= 改动前的行为，严格不退步）。理由是我们只有这一个版本的样本 —— 更旧的 CLI 烘的是更短
/// 的一张目录表，里面到底有哪些 id 我们**没有证据**，而 claude 对 `--model` 是**原样透传**的
/// （真机核实：`--model claude-bogus-9` 照样启动、`system/init` 原样回报），错误要等到真正
/// 发请求时才炸。宁可少给几个选项，也不要给一个「点了才发现不行」的选项。
///
/// 版本号从探活缓存里读（`spawn::cached_cli_version`），**不额外起进程**（spec 第 9 条）。
/// 读不到版本 ⇒ 不做门控（照常提供），未知不该等于「功能消失」。
const CONCRETE_MODELS_MIN_CLI_VERSION: (u32, u32, u32) = (2, 1, 220);

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

    // 3. 具体版本。版本号取自探活缓存（`resolve_binary` 刚刚跑过 `--version`），
    //    **不新起进程**；CLI 太旧就整组不提供（见 CONCRETE_MODELS_MIN_CLI_VERSION）。
    let version_line = crate::external_agents::spawn::cached_cli_version(resolved_bin);
    for option in concrete_model_options(version_line.as_deref()) {
        if seen.insert(option.id.clone()) {
            out.push(option);
        }
    }

    // 4. Custom models configured via ~/.claude/settings.json `env.*` + process env.
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

/// 从 `claude --version` 的输出行里抠出 `(major, minor, patch)`。
///
/// 本机实测输出形如 `2.1.220 (Claude Code)`。刻意只认「行首的 `x.y.z`」这一种形态：
/// 认得越宽，越容易把某个日志行里的数字当成版本号，从而**误判成旧版**而悄悄少给选项。
/// 认不出 ⇒ `None` ⇒ 不做门控（见 `CONCRETE_MODELS_MIN_CLI_VERSION`）。
pub fn parse_claude_cli_version(line: &str) -> Option<(u32, u32, u32)> {
    let head = line.trim().split_whitespace().next()?;
    let mut parts = head.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    // patch 可能带后缀（如 `220-rc1`），只取前导数字。
    let patch_raw = parts.next().unwrap_or("0");
    let digits: String = patch_raw.chars().take_while(char::is_ascii_digit).collect();
    let patch = digits.parse::<u32>().unwrap_or(0);
    Some((major, minor, patch))
}

/// 当前装的 CLI 是否新到可以提供「具体版本」这一组模型选项。
/// 版本读不到（`None`）时一律放行 —— 未知不该等于功能消失。
pub fn cli_offers_concrete_models(version_line: Option<&str>) -> bool {
    match version_line.and_then(parse_claude_cli_version) {
        Some(installed) => installed >= CONCRETE_MODELS_MIN_CLI_VERSION,
        None => true,
    }
}

/// 具体版本的模型选项。窗口一律 `None`（见 `CLAUDE_CONCRETE_MODELS` 的说明）；
/// CLI 太旧时返回空列表，调用方就只剩别名 + 用户自配模型（= 改动前的行为）。
fn concrete_model_options(version_line: Option<&str>) -> Vec<RuntimeModelOption> {
    if !cli_offers_concrete_models(version_line) {
        return Vec::new();
    }
    CLAUDE_CONCRETE_MODELS
        .iter()
        .map(|(id, label)| RuntimeModelOption {
            id: (*id).to_string(),
            label: (*label).to_string(),
            context_window_tokens: None,
        })
        .collect()
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
    fn catalog_options_have_labels_and_windows() {
        let opus = catalog_model_option("opus");
        assert_eq!(opus.id, "opus");
        assert_eq!(opus.label, "Opus");
        // 裸别名的窗口未知（见 context_window_from_claude_model_alias 的实测说明）：
        // 编一个 200K 会让 `usage_ratio` 虚高 5 倍。
        assert_eq!(opus.context_window_tokens, None);

        let sonnet_1m = catalog_model_option("sonnet[1m]");
        assert_eq!(sonnet_1m.id, "sonnet[1m]");
        assert_eq!(sonnet_1m.label, "Sonnet (1M context)");
        assert_eq!(sonnet_1m.context_window_tokens, Some(1_000_000));
    }

    #[test]
    fn full_catalog_covers_every_alias_without_spawn() {
        // Every alias must yield a catalog entry — discovery no longer probes per alias.
        // 窗口**不强制有值**：裸别名的窗口只有 CLI 自己知道（首轮 result 的 modelUsage 才报）。
        for &alias in CLAUDE_MODEL_ALIASES {
            let option = catalog_model_option(alias);
            assert_eq!(option.id, alias);
            assert!(!option.label.is_empty());
            if alias.to_ascii_lowercase().contains("[1m]") {
                assert_eq!(option.context_window_tokens, Some(1_000_000), "{alias}");
            } else {
                assert_eq!(option.context_window_tokens, None, "{alias}");
            }
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
            sonnet.context_window_tokens, None,
            "裸别名的窗口只有 CLI 自己知道（本机 `--model sonnet` 解析为 claude-sonnet-5[1M]），\
             静态编一个 200K 会让分母小 5 倍"
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

        // 具体版本必须真的出现在列表里（本机 CLI 版本足够新），且不带静态窗口。
        let concrete = detected
            .models
            .iter()
            .find(|model| model.id == "claude-opus-4-8")
            .expect("具体版本 claude-opus-4-8 应可选（本机 CLI 版本 >= 2.1.220）");
        assert_eq!(concrete.context_window_tokens, None);
        assert!(detected
            .models
            .iter()
            .any(|model| model.id == "claude-sonnet-4-5"));
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

    // ---- 具体版本模型 + 版本门控 ----

    /// 版本行解析：只认行首的 `x.y.z`。认得越宽越容易把日志里的数字当版本 ⇒ 误判成旧版
    /// ⇒ 悄悄少给选项，而这种错没有任何可观测信号。
    #[test]
    fn parses_the_cli_version_line() {
        // 本机实测输出。
        assert_eq!(
            parse_claude_cli_version("2.1.220 (Claude Code)"),
            Some((2, 1, 220))
        );
        assert_eq!(parse_claude_cli_version("  3.0.0  "), Some((3, 0, 0)));
        // patch 带后缀时只取前导数字。
        assert_eq!(parse_claude_cli_version("2.2.10-rc1"), Some((2, 2, 10)));
        // 两段版本号补 0。
        assert_eq!(parse_claude_cli_version("2.1"), Some((2, 1, 0)));
        // 认不出的一律 None（⇒ 不做门控）。
        assert_eq!(parse_claude_cli_version(""), None);
        assert_eq!(parse_claude_cli_version("Claude Code 2.1.220"), None);
        assert_eq!(parse_claude_cli_version("v2.1.220"), None);
    }

    #[test]
    fn concrete_models_are_gated_on_the_installed_cli_version() {
        // 本机版本：给出全部具体版本条目。
        let current = concrete_model_options(Some("2.1.220 (Claude Code)"));
        assert_eq!(current.len(), CLAUDE_CONCRETE_MODELS.len());
        assert!(current.iter().any(|m| m.id == "claude-opus-4-8"));
        assert!(current.iter().any(|m| m.id == "claude-sonnet-5"));
        // 具体版本一律不带窗口（窗口只认 CLI 每轮实报，spec 第 14g 条）。
        for m in &current {
            assert_eq!(m.context_window_tokens, None, "{} 不该带静态窗口", m.id);
            assert!(!m.label.is_empty());
        }

        // 更新的版本照样给。
        assert!(!concrete_model_options(Some("2.2.0 (Claude Code)")).is_empty());
        assert!(!concrete_model_options(Some("3.0.0")).is_empty());

        // 更旧的版本：整组不提供（回到改动前的行为，只剩别名 + 用户自配）。
        assert!(concrete_model_options(Some("2.1.219 (Claude Code)")).is_empty());
        assert!(concrete_model_options(Some("2.0.9")).is_empty());
        assert!(concrete_model_options(Some("1.9.999")).is_empty());

        // 版本读不到 ⇒ 不做门控。未知不该等于功能消失。
        assert!(!concrete_model_options(None).is_empty());
        assert!(!concrete_model_options(Some("weird output")).is_empty());
    }

    /// 具体版本表本身的卫生：id 唯一、不与别名冲突、形态是 `claude-…`（会被原样当
    /// `--model` 传出去，一个手误在真机上只会表现为「跑到一半才报模型不存在」）。
    #[test]
    fn concrete_model_table_is_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for (id, label) in CLAUDE_CONCRETE_MODELS {
            assert!(seen.insert(*id), "重复的模型 id：{id}");
            assert!(id.starts_with("claude-"), "{id} 不像具体模型 id");
            assert!(!id.contains(' '), "{id} 含空格");
            assert!(!label.is_empty(), "{id} 缺 label");
            assert!(
                !CLAUDE_MODEL_ALIASES.contains(id),
                "{id} 与别名重复，会在下拉里出现两次"
            );
        }
        // 别名表补齐了 CLI 白名单里的 best / opusplan。
        assert!(CLAUDE_MODEL_ALIASES.contains(&"best"));
        assert!(CLAUDE_MODEL_ALIASES.contains(&"opusplan"));
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
            eprintln!("skip: 探测没能拿到 system/init（未登录 / 网络问题？先手动跑一次 `claude -p hi`）");
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
            let line = match tokio::time::timeout(Duration::from_secs(5), reader.next_line()).await {
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) | Ok(Err(_)) => break,
                Err(_) => continue,
            };
            let Some(value) = parse_json_line(&line) else {
                continue;
            };
            match value.get("type").and_then(|v| v.as_str()) {
                Some("assistant") => {
                    if let Some(blocks) = value
                        .pointer("/message/content")
                        .and_then(|v| v.as_array())
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
                        eprintln!("  result 报错：{}", value.get("result").unwrap_or(&Value::Null));
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
