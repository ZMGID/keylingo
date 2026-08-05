//! 第三方供应商（中转站）的落地层：把设置页里选中的供应商变成**子进程能看见的东西**。
//!
//! 三条通道，全部只作用于 Kivio 自己拉起的进程，**绝不改用户的 `~/.claude` / `~/.codex`**：
//!
//! 1. **环境变量** —— claude / gemini / 其余 env 系直接注入 `provider.env`
//!    （出口是 `overrides::env_for` → `spawn::agent_cli_command` / `cli_command`）。
//! 2. **claude 的 `--settings` 压制** —— 光注入环境变量**不够**：Claude Code 会把
//!    `~/.claude/settings.json` 的 `env` 段注入自己进程，盖掉继承来的同名变量。用户那份
//!    文件通常已被 cc-switch 写满了 `ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN`，
//!    于是「在 Kivio 里选了供应商却还是走老中转站」。所以额外物化一份只含 `{"env": …}`
//!    的文件用 `--settings` 传进去，并把本供应商**没设的路由键补成空串**显式压掉。
//! 3. **codex 的私有 `CODEX_HOME`** —— codex 的 base_url 只能来自 `config.toml`，没有
//!    环境变量通道。物化一个私有 home（config.toml + auth.json）后注入 `CODEX_HOME`，
//!    用户自己的 `~/.codex` 一个字节不动。
//!
//! 物化时机是**保存 / 切换供应商那一次**（`commands::chat_external_cli_provider_apply`），
//! 不是每轮。ccgui 用的是 per-turn 临时目录 + `Drop` 删除，那套在 Kivio 会把常驻 claude
//! 会话中途要读的文件删掉。代价是文件长期留在 app data 里（0600，与 settings.json 里
//! 本来就明文存 key 同一威胁模型）。
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::settings::ExternalCliProvider;

/// claude 的「路由键」：决定请求打到哪、用哪个模型的那些环境变量。
///
/// 用途有二：物化 `--settings` 时把**没设的**补成空串（否则用户 `~/.claude/settings.json`
/// 里的同名键会漏进来，出现「base_url 是新的、模型名还是旧供应商的」这种半切换）；
/// 以及注入前先从子进程环境里删一遍，清掉父进程残留。
pub const CLAUDE_ROUTING_ENV_KEYS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME",
    "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME",
    "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
];

fn profiles_dir() -> Option<PathBuf> {
    crate::app_data::app_data_dir().map(|dir| dir.join("external-cli-providers"))
}

/// 供应商 id 直接当文件/目录名用，必须消毒。照 ccgui 的 `sanitize_provider_path_segment`：
/// 拒绝路径分隔符、`..`、控制字符与 Windows 保留名，越界一律返回 None 而不是「尽量修复」。
fn sanitize_segment(id: &str) -> Option<String> {
    let id = id.trim();
    if id.is_empty() || id == "." || id == ".." || id.ends_with('.') {
        return None;
    }
    if id.chars().any(|ch| {
        ch.is_control() || matches!(ch, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
    }) {
        return None;
    }
    let upper = id.to_ascii_uppercase();
    const RESERVED: &[&str] = &["CON", "PRN", "AUX", "NUL"];
    if RESERVED.contains(&upper.as_str())
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.as_bytes()[3].is_ascii_digit()
            && upper.as_bytes()[3] != b'0')
    {
        return None;
    }
    Some(id.to_string())
}

fn codex_home_for(provider_id: &str) -> Option<PathBuf> {
    Some(profiles_dir()?.join(format!("codex-{}", sanitize_segment(provider_id)?)))
}

fn claude_settings_path_for(provider_id: &str) -> Option<PathBuf> {
    Some(profiles_dir()?.join(format!("claude-{}.json", sanitize_segment(provider_id)?)))
}

/// 要注入这个 CLI 子进程的供应商环境变量。没选供应商 = 空表（保持原样，不托管）。
pub fn provider_env(agent_id: &str) -> HashMap<String, String> {
    let Some(provider) = super::overrides::active_provider(agent_id) else {
        return HashMap::new();
    };
    if agent_id == "codex" {
        // codex 读不到 base_url 环境变量，只认 config.toml；私有 home 是唯一通道。
        return match codex_home_for(&provider.id) {
            Some(home) => HashMap::from([("CODEX_HOME".to_string(), home.to_string_lossy().into_owned())]),
            None => HashMap::new(),
        };
    }
    provider
        .env
        .into_iter()
        .map(|pair| (pair.key, pair.value))
        .collect()
}

/// claude 启动时要追加的 `--settings <path>`；无供应商 / 文件没物化成功时返回 None。
pub fn claude_settings_override(agent_id: &str) -> Option<PathBuf> {
    if agent_id != "claude" {
        return None;
    }
    let provider = super::overrides::active_provider(agent_id)?;
    let path = claude_settings_path_for(&provider.id)?;
    path.is_file().then_some(path)
}

/// 给所有 CLI 物化一遍当前生效的供应商。由 `persist_settings` 在同步完镜像后调用 ——
/// 保存设置就等于落地，前端不需要记得多调一个命令。
/// 单个失败只记日志：一个 CLI 的坏 TOML 不该拦住整次设置保存。
pub fn materialize_all() {
    for def in crate::external_agents::registry::AGENT_DEFS {
        if let Err(err) = materialize(def.id) {
            eprintln!("[external-agent] 供应商落地失败（{}）：{err}", def.id);
        }
    }
}

/// 把当前生效的供应商写到盘上（claude 的 settings 覆盖文件 / codex 的私有 home）。
/// 无供应商时是 no-op —— 「切回官方」不需要恢复任何东西，因为从没改过用户的文件。
pub fn materialize(agent_id: &str) -> Result<(), String> {
    let Some(provider) = super::overrides::active_provider(agent_id) else {
        return Ok(());
    };
    match agent_id {
        "claude" => materialize_claude(&provider),
        "codex" => materialize_codex(&provider),
        // 其余 CLI 纯靠环境变量，没有要落盘的东西。
        _ => Ok(()),
    }
}

fn materialize_claude(provider: &ExternalCliProvider) -> Result<(), String> {
    let path = claude_settings_path_for(&provider.id)
        .ok_or_else(|| format!("供应商 id 不能作为文件名：{}", provider.id))?;
    let mut env: serde_json::Map<String, serde_json::Value> = provider
        .env
        .iter()
        .map(|pair| (pair.key.clone(), serde_json::Value::String(pair.value.clone())))
        .collect();
    // 没设的路由键补空串：settings.json 的 env 段是「显式赋值」而不是「没写就沿用」，
    // 不补的话用户 ~/.claude/settings.json 里的旧值会从另一边漏进来。
    for key in CLAUDE_ROUTING_ENV_KEYS {
        env.entry((*key).to_string())
            .or_insert_with(|| serde_json::Value::String(String::new()));
    }
    let body = serde_json::json!({ "env": env });
    write_private(&path, &serde_json::to_string_pretty(&body).map_err(|e| e.to_string())?)
}

fn materialize_codex(provider: &ExternalCliProvider) -> Result<(), String> {
    let home = codex_home_for(&provider.id)
        .ok_or_else(|| format!("供应商 id 不能作为目录名：{}", provider.id))?;
    // 写之前先验一遍：坏 TOML 会让 codex 整个起不来，报的错还跟供应商八竿子打不着。
    toml::from_str::<toml::Value>(&provider.config_toml)
        .map_err(|e| format!("config.toml 解析失败：{e}"))?;
    std::fs::create_dir_all(&home).map_err(|e| format!("创建 {} 失败：{e}", home.display()))?;
    write_private(&home.join("config.toml"), &provider.config_toml)?;
    let auth = provider.auth_json.trim();
    if auth.is_empty() {
        let _ = std::fs::remove_file(home.join("auth.json"));
    } else {
        serde_json::from_str::<serde_json::Value>(auth)
            .map_err(|e| format!("auth.json 解析失败：{e}"))?;
        write_private(&home.join("auth.json"), auth)?;
    }
    Ok(())
}

/// 删除供应商时清掉它物化出来的文件。失败只记日志：残留一个读不到的旧文件不影响正确性。
pub fn cleanup(agent_id: &str, provider_id: &str) {
    match agent_id {
        "claude" => {
            if let Some(path) = claude_settings_path_for(provider_id) {
                let _ = std::fs::remove_file(path);
            }
        }
        "codex" => {
            if let Some(home) = codex_home_for(provider_id) {
                let _ = std::fs::remove_dir_all(home);
            }
        }
        _ => {}
    }
}

/// 文件里有 API key，权限收到 0600（Windows 无 unix 权限位，靠 app data 目录本身的 ACL）。
fn write_private(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建 {} 失败：{e}", parent.display()))?;
    }
    std::fs::write(path, content).map_err(|e| format!("写入 {} 失败：{e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rejects_path_escapes() {
        assert_eq!(sanitize_segment("loki-claude").as_deref(), Some("loki-claude"));
        assert!(sanitize_segment("../../etc/passwd").is_none());
        assert!(sanitize_segment("a/b").is_none());
        assert!(sanitize_segment("a\\b").is_none());
        assert!(sanitize_segment("..").is_none());
        assert!(sanitize_segment("").is_none());
        assert!(sanitize_segment("CON").is_none());
        assert!(sanitize_segment("COM1").is_none());
        // COM0 不是保留名，别误伤。
        assert_eq!(sanitize_segment("COM0").as_deref(), Some("COM0"));
    }
}
