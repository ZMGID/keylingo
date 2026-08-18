//! 设置页「本地 CLI Agent」里用户填的每-CLI 覆盖（自定义路径 / 环境变量 / 自定义模型 / 停用）
//! 的进程内镜像。
//!
//! 为什么是全局而不是参数传递：`spawn::resolve_binary` / `detection::*` 这一整条探测+拉起链
//! 都没有 `AppHandle`（它们也被无 Tauri 上下文的路径复用），要把覆盖顺着传下去得改十几个签名。
//!
//! ponytail: 只在 `persist_settings` / `load_settings` 两处同步；**故意不挂在 `sanitize_settings`
//! 上** —— 校验路径会用临时拼的 partial `Settings` 调它，挂上去会把真实覆盖清空。
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};

use crate::settings::{CliCustomModel, ExternalCliAgentConfig, ExternalCliProvider, Settings};

static OVERRIDES: LazyLock<RwLock<HashMap<String, ExternalCliAgentConfig>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn sync_from_settings(settings: &Settings) {
    if let Ok(mut guard) = OVERRIDES.write() {
        *guard = settings.chat.external_cli_agents.clone();
    }
}

fn get(id: &str) -> Option<ExternalCliAgentConfig> {
    OVERRIDES.read().ok()?.get(id).cloned()
}

/// 原生落盘层需要整份配置：不仅要看当前项，还要同步全部 Kivio 管理的 provider。
pub fn agent_config(id: &str) -> Option<ExternalCliAgentConfig> {
    get(id)
}

/// 用户是否停用了这个 CLI（不再出现在 Chat 的运行时选择器里）。
pub fn is_disabled(id: &str) -> bool {
    get(id).map(|cfg| cfg.disabled).unwrap_or(false)
}

/// 用户指定的可执行文件路径；空/未设 = 走 PATH 探测。
pub fn custom_path(id: &str) -> Option<PathBuf> {
    let path = get(id)?.path;
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

/// 当前生效的第三方供应商（设置页里选中的那条）。空 `current_provider` = 不托管。
pub fn active_provider(id: &str) -> Option<ExternalCliProvider> {
    let cfg = get(id)?;
    if cfg.current_provider.is_empty() {
        return None;
    }
    cfg.providers
        .into_iter()
        .find(|provider| provider.id == cfg.current_provider && !provider.disabled)
}

/// 注入该 CLI 子进程的环境变量（`ANTHROPIC_BASE_URL` 之类的中转配置）。
///
/// 供应商打底、用户手填的 env 覆盖在上面：手填是「我知道我在干什么」的逃生口，
/// 反过来会让用户改半天发现没生效。
pub fn env_for(id: &str) -> HashMap<String, String> {
    let provider_env = crate::external_agents::provider_profile::provider_env(id);
    match get(id) {
        Some(cfg) => merge_env(provider_env, &cfg.env),
        None => provider_env,
    }
}

/// 供应商 env 打底、手填 env 盖在上面。抽成纯函数是为了能不碰全局镜像地测优先级。
fn merge_env(
    mut base: HashMap<String, String>,
    manual: &[crate::settings::CliEnvVar],
) -> HashMap<String, String> {
    for pair in manual {
        base.insert(pair.key.clone(), pair.value.clone());
    }
    base
}

/// 用户手填的模型，合并进探测出来的下拉列表。
pub fn custom_models(id: &str) -> Vec<CliCustomModel> {
    get(id).map(|cfg| cfg.custom_models).unwrap_or_default()
}

/// 按二进制路径反查该 agent 的环境变量覆盖。
///
/// 为什么按路径而不是按 id：常驻会话 / 斜杠命令 / 导入这些 spawn 点散落在
/// `session/*.rs` 十几处，签名里都只有 `bin: &Path`，全部改成带 `def` 要顺带改三十多个
/// 调用点（大半是测试）。反查是纯函数，且新增 spawn 点自动覆盖，不会漏。
///
/// 只在**确实配了环境变量**的 agent 里找；各 CLI 的可执行名互不重复（claude / codex /
/// cursor-agent / opencode / gemini / kimi / pi / hermes / grok / dsh），所以按文件名匹配是唯一的。
pub fn env_for_bin(bin: &std::path::Path) -> HashMap<String, String> {
    // 先只在锁里定位 agent id，出锁再拼环境变量：`env_for` 自己还要读一次这把锁，
    // 持读锁再读会在有写者排队时死锁。
    let hit = {
        let guard = match OVERRIDES.read() {
            Ok(guard) => guard,
            Err(_) => return HashMap::new(),
        };
        guard
            .iter()
            .find(|(id, cfg)| {
                // dsh 的供应商可全部并存；即使 currentProvider 为空，显式选中其中一个模型时也要注入凭据。
                let has_coexisting_dsh_providers =
                    id.as_str() == "dsh" && cfg.providers.iter().any(|provider| !provider.disabled);
                if cfg.env.is_empty()
                    && cfg.current_provider.is_empty()
                    && !has_coexisting_dsh_providers
                {
                    return false;
                }
                if cfg.path.is_empty() {
                    crate::external_agents::registry::get_agent_def(id)
                        .is_some_and(|def| bin_name_matches(bin, def))
                } else {
                    std::path::Path::new(&cfg.path) == bin
                }
            })
            .map(|(id, _)| id.clone())
    };
    hit.map(|id| env_for(&id)).unwrap_or_default()
}

/// Windows 上 PATH 命中的可能是 `claude.cmd` / `claude.exe`，所以带扩展名和不带都比一遍。
fn bin_name_matches(
    bin: &std::path::Path,
    def: &crate::external_agents::types::RuntimeAgentDef,
) -> bool {
    let stem = bin.file_stem().and_then(|s| s.to_str());
    let name = bin.file_name().and_then(|s| s.to_str());
    std::iter::once(def.bin)
        .chain(def.fallback_bins.iter().copied())
        .any(|candidate| stem == Some(candidate) || name == Some(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::CliEnvVar;

    fn pair(key: &str, value: &str) -> CliEnvVar {
        CliEnvVar {
            key: key.to_string(),
            value: value.to_string(),
        }
    }

    #[test]
    fn manual_env_wins_over_provider_env() {
        let base = HashMap::from([
            (
                "ANTHROPIC_BASE_URL".to_string(),
                "https://provider".to_string(),
            ),
            (
                "ANTHROPIC_AUTH_TOKEN".to_string(),
                "sk-provider".to_string(),
            ),
        ]);
        let merged = merge_env(
            base,
            &[
                pair("ANTHROPIC_BASE_URL", "https://manual"),
                pair("EXTRA", "1"),
            ],
        );
        // 手填是「我知道我在干什么」的逃生口，必须压过供应商。
        assert_eq!(merged["ANTHROPIC_BASE_URL"], "https://manual");
        // 供应商里手填没提到的键要保留，否则换个键名就把整份供应商配置抹了。
        assert_eq!(merged["ANTHROPIC_AUTH_TOKEN"], "sk-provider");
        assert_eq!(merged["EXTRA"], "1");
    }
}
