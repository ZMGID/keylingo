//! 第三方供应商（中转站）的落地层：把设置页里选中的供应商变成**子进程能看见的东西**。
//!
//! Claude / Codex 继续使用 Kivio 私有配置；OpenCode / Pi / Grok 按各自官方约定写入原生配置：
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
//! 4. **opencode / pi 的原生配置** —— 字段级合并 Kivio 管理的 provider、凭据与默认模型；
//!    其他 provider 和顶层设置原样保留。切回「CLI 自身配置」时恢复 Kivio 接管前的默认模型。
//! 5. **grok 的 `~/.grok/config.toml`** —— 与 cc-switch 一样落盘（Grok 没有 env 通道，
//!    base_url 只能写进 config.toml）。把供应商的 `config_toml` 里的 `[models]` /
//!    `[model.*]` 合并进现有文件，marketplace / ui / cli 等用户段原样保留；首次接管前
//!    整份备份，切回「CLI 自身配置」时还原。
//! 6. **kimi 的 `~/.kimi-code/config.toml`** —— Kimi Code CLI 凭证只认 config.toml（不读
//!    shell 环境变量）。把供应商片段里的 `default_model` / `[providers.*]` / `[models.*]`
//!    合并进现有文件；`managed:kimi-code` OAuth 段与其它用户配置原样保留。首次接管前
//!    整份备份，切回「CLI 自身配置」时还原。
//! 7. **dsh 的 Kivio 私有 profile** —— provider 本体由 `dsh_profile.rs` 写进
//!    `profiles/kivio/cordis.patch.yml` 的 `llm-pi-ai.providers`；本层把 `apiKeyEnv`
//!    指向的 Key 注入 Kivio 启动的进程。`settings.yaml` 只回写已存在路由上的模型
//!    `input` / `defaultInput` / `reasoningEfforts`（官方 web 贴图与 effort 门控），
//!    不新建供应商、不写密钥。
//!
//! 物化时机是**保存 / 切换供应商那一次**（`commands::chat_external_cli_provider_apply`），
//! 不是每轮。ccgui 用的是 per-turn 临时目录 + `Drop` 删除，那套在 Kivio 会把常驻 claude
//! 会话中途要读的文件删掉。代价是文件长期留在 app data 里（0600，与 settings.json 里
//! 本来就明文存 key 同一威胁模型）。
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::settings::{ExternalCliAgentConfig, ExternalCliProvider};

/// 设置保存与删除命令可能并发触发；三份原生文件必须作为一个逻辑事务串行合并。
static NATIVE_CONFIG_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Debug, Clone)]
struct NativePaths {
    config: PathBuf,
    alternate_configs: Vec<PathBuf>,
    auth: PathBuf,
    settings: Option<PathBuf>,
    state: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct NativeManagedState {
    managed_provider_ids: Vec<String>,
    defaults_managed: bool,
    previous_defaults: HashMap<String, BackedUpField>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct BackedUpField {
    present: bool,
    value: Value,
}

impl Default for BackedUpField {
    fn default() -> Self {
        Self {
            present: false,
            value: Value::Null,
        }
    }
}

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

fn nonempty_env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn opencode_paths() -> Option<NativePaths> {
    let base = directories::BaseDirs::new()?;
    let home = base.home_dir();
    let config_home = nonempty_env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
    let data_home = nonempty_env_path("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share"));
    let config_dir = config_home.join("opencode");
    let candidates: Vec<PathBuf> = ["config.json", "opencode.json", "opencode.jsonc"]
        .into_iter()
        .map(|name| config_dir.join(name))
        .collect();
    let config = candidates
        .iter()
        .rev()
        .find(|path| path.is_file())
        .cloned()
        .unwrap_or_else(|| config_dir.join("opencode.json"));
    let alternate_configs = candidates
        .into_iter()
        .filter(|path| path != &config && path.is_file())
        .collect();
    Some(NativePaths {
        config,
        alternate_configs,
        auth: data_home.join("opencode/auth.json"),
        settings: None,
        state: profiles_dir()?.join("opencode-native-state.json"),
    })
}

pub fn pi_agent_dir() -> Option<PathBuf> {
    let base = directories::BaseDirs::new()?;
    Some(
        nonempty_env_path("PI_CODING_AGENT_DIR")
            .unwrap_or_else(|| base.home_dir().join(".pi/agent")),
    )
}

fn pi_paths() -> Option<NativePaths> {
    let agent = pi_agent_dir()?;
    Some(NativePaths {
        config: agent.join("models.json"),
        alternate_configs: Vec::new(),
        auth: agent.join("auth.json"),
        settings: Some(agent.join("settings.json")),
        state: profiles_dir()?.join("pi-native-state.json"),
    })
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
            Some(home) => HashMap::from([(
                "CODEX_HOME".to_string(),
                home.to_string_lossy().into_owned(),
            )]),
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

/// 把供应商写到盘上。OpenCode / Pi / Grok / Kimi 即使当前未启用，也要同步（或恢复）原生配置。
pub fn materialize(agent_id: &str) -> Result<(), String> {
    if matches!(agent_id, "opencode" | "pi") {
        let config = super::overrides::agent_config(agent_id).unwrap_or_default();
        return materialize_native(agent_id, &config);
    }
    if agent_id == "grok" {
        let config = super::overrides::agent_config(agent_id).unwrap_or_default();
        return materialize_grok(&config);
    }
    if agent_id == "kimi" {
        let config = super::overrides::agent_config(agent_id).unwrap_or_default();
        return materialize_kimi(&config);
    }
    if agent_id == "dsh" {
        let config = super::overrides::agent_config(agent_id).unwrap_or_default();
        return crate::external_agents::dsh_plugins::sync_kivio_model_capabilities(&config.providers);
    }
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
        .map(|pair| {
            (
                pair.key.clone(),
                serde_json::Value::String(pair.value.clone()),
            )
        })
        .collect();
    // 没设的路由键补空串：settings.json 的 env 段是「显式赋值」而不是「没写就沿用」，
    // 不补的话用户 ~/.claude/settings.json 里的旧值会从另一边漏进来。
    for key in CLAUDE_ROUTING_ENV_KEYS {
        env.entry((*key).to_string())
            .or_insert_with(|| serde_json::Value::String(String::new()));
    }
    let body = serde_json::json!({ "env": env });
    write_private(
        &path,
        &serde_json::to_string_pretty(&body).map_err(|e| e.to_string())?,
    )
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

/// Grok 原生配置路径：`$GROK_HOME/config.toml`，否则 `~/.grok/config.toml`。
fn grok_config_path() -> Option<PathBuf> {
    if let Some(home) = nonempty_env_path("GROK_HOME") {
        return Some(home.join("config.toml"));
    }
    directories::BaseDirs::new().map(|base| base.home_dir().join(".grok").join("config.toml"))
}

fn grok_state_path() -> Option<PathBuf> {
    Some(profiles_dir()?.join("grok-native-state.json"))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct GrokManagedState {
    /// 是否已由 Kivio 接管过 `config.toml`（有备份可还原）。
    managed: bool,
    /// 接管前整份 `config.toml` 原文；切回「CLI 自身配置」时写回。
    previous_config: Option<String>,
}

fn read_grok_state(path: &Path) -> Result<GrokManagedState, String> {
    if !path.is_file() {
        return Ok(GrokManagedState::default());
    }
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(GrokManagedState::default());
    }
    serde_json::from_str(&text).map_err(|e| format!("解析 {} 失败：{e}", path.display()))
}

fn write_grok_state(path: &Path, state: &GrokManagedState) -> Result<(), String> {
    let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())? + "\n";
    write_private_atomic(path, &text)
}

fn read_text_or_empty(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(format!("读取 {} 失败：{err}", path.display())),
    }
}

/// 把供应商 `config_toml` 里的 `[models]` / `[model.*]` 合并进 base。
/// 其它段（marketplace / ui / cli / mcp…）一律保留 base 的——对齐「只换路由、不动用户偏好」。
fn merge_grok_provider_config(base: &str, provider_config: &str) -> Result<String, String> {
    let mut base_doc: toml::Table = if base.trim().is_empty() {
        toml::Table::new()
    } else {
        toml::from_str(base).map_err(|e| format!("现有 Grok config.toml 解析失败：{e}"))?
    };
    let provider_doc: toml::Table = if provider_config.trim().is_empty() {
        return Err("Grok 供应商缺少 config.toml".to_string());
    } else {
        toml::from_str(provider_config)
            .map_err(|e| format!("Grok 供应商 config.toml 解析失败：{e}"))?
    };

    // 至少要有 model 表或 models.default，否则落盘等于空操作。
    let has_models = provider_doc
        .get("models")
        .and_then(|v| v.as_table())
        .is_some_and(|t| t.contains_key("default"));
    let has_model = provider_doc
        .get("model")
        .and_then(|v| v.as_table())
        .is_some_and(|t| !t.is_empty());
    if !has_models && !has_model {
        return Err("Grok 供应商 config.toml 缺少 [models].default 或 [model.*] 段".to_string());
    }

    if let Some(models) = provider_doc.get("models").and_then(|v| v.as_table()) {
        let base_models = base_doc
            .entry("models".to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or_else(|| "Grok config.toml 的 [models] 不是表".to_string())?;
        for (key, value) in models {
            base_models.insert(key.clone(), value.clone());
        }
    }

    if let Some(model) = provider_doc.get("model").and_then(|v| v.as_table()) {
        let base_model = base_doc
            .entry("model".to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or_else(|| "Grok config.toml 的 [model] 不是表".to_string())?;
        for (key, value) in model {
            base_model.insert(key.clone(), value.clone());
        }
    }

    // toml crate 的 pretty 序列化对 dotted keys 友好；末尾补换行方便 diff。
    let mut rendered =
        toml::to_string_pretty(&base_doc).map_err(|e| format!("序列化 Grok config 失败：{e}"))?;
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    Ok(rendered)
}

fn materialize_grok(config: &ExternalCliAgentConfig) -> Result<(), String> {
    let path = grok_config_path().ok_or_else(|| "无法定位 Grok 配置目录".to_string())?;
    let state_path = grok_state_path().ok_or_else(|| "无法定位 Grok 状态文件".to_string())?;
    let _guard = NATIVE_CONFIG_LOCK
        .lock()
        .map_err(|_| "原生 CLI 配置写锁已损坏".to_string())?;
    materialize_grok_at(config, &path, &state_path)
}

fn materialize_grok_at(
    config: &ExternalCliAgentConfig,
    path: &Path,
    state_path: &Path,
) -> Result<(), String> {
    let mut state = read_grok_state(state_path)?;

    // 切回「CLI 自身配置」：把接管前的整份文件还原回去。
    if config.current_provider.trim().is_empty() {
        if state.managed {
            if let Some(prev) = state.previous_config.take() {
                if prev.is_empty() {
                    // 接管前文件不存在：删掉我们写过的，回到「无 config.toml」。
                    if path.is_file() {
                        std::fs::remove_file(path)
                            .map_err(|e| format!("删除 {} 失败：{e}", path.display()))?;
                    }
                } else {
                    write_private_atomic(path, &prev)?;
                }
            }
            state.managed = false;
            write_grok_state(state_path, &state)?;
        }
        return Ok(());
    }

    let provider = config
        .providers
        .iter()
        .find(|p| p.id == config.current_provider)
        .ok_or_else(|| format!("当前供应商 {} 不存在", config.current_provider))?;
    if provider.config_toml.trim().is_empty() {
        return Err(format!(
            "供应商 {} 缺少可落盘的 Grok config.toml",
            provider.name
        ));
    }

    // 首次接管：备份现有文件，之后切供应商始终基于这份备份合并，避免 A→B 残留 A 的 model 键。
    if !state.managed {
        state.previous_config = Some(read_text_or_empty(path)?);
        state.managed = true;
    }
    let base = state.previous_config.as_deref().unwrap_or("");
    let merged = merge_grok_provider_config(base, &provider.config_toml)?;
    write_private_atomic(path, &merged)?;
    write_grok_state(state_path, &state)
}

/// Kimi Code CLI 配置路径：`$KIMI_CODE_HOME/config.toml`，否则 `~/.kimi-code/config.toml`。
fn kimi_config_path() -> Option<PathBuf> {
    if let Some(home) = nonempty_env_path("KIMI_CODE_HOME") {
        return Some(home.join("config.toml"));
    }
    directories::BaseDirs::new().map(|base| base.home_dir().join(".kimi-code").join("config.toml"))
}

fn kimi_state_path() -> Option<PathBuf> {
    Some(profiles_dir()?.join("kimi-native-state.json"))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct KimiManagedState {
    managed: bool,
    previous_config: Option<String>,
}

fn read_kimi_state(path: &Path) -> Result<KimiManagedState, String> {
    if !path.is_file() {
        return Ok(KimiManagedState::default());
    }
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(KimiManagedState::default());
    }
    serde_json::from_str(&text).map_err(|e| format!("解析 {} 失败：{e}", path.display()))
}

fn write_kimi_state(path: &Path, state: &KimiManagedState) -> Result<(), String> {
    let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())? + "\n";
    write_private_atomic(path, &text)
}

/// 把供应商 `config_toml` 里的 `default_model` / `providers` / `models` 合并进 base。
/// 其它段（thinking / services / permission / managed OAuth…）一律保留 base 的。
fn merge_kimi_provider_config(base: &str, provider_config: &str) -> Result<String, String> {
    let mut base_doc: toml::Table = if base.trim().is_empty() {
        toml::Table::new()
    } else {
        toml::from_str(base).map_err(|e| format!("现有 Kimi config.toml 解析失败：{e}"))?
    };
    let provider_doc: toml::Table = if provider_config.trim().is_empty() {
        return Err("Kimi 供应商缺少 config.toml".to_string());
    } else {
        toml::from_str(provider_config)
            .map_err(|e| format!("Kimi 供应商 config.toml 解析失败：{e}"))?
    };

    let providers = provider_doc
        .get("providers")
        .and_then(|v| v.as_table())
        .ok_or_else(|| "Kimi 供应商 config.toml 缺少 [providers.*]".to_string())?;
    if providers.is_empty() {
        return Err("Kimi 供应商 config.toml 缺少 [providers.*]".to_string());
    }
    let models = provider_doc
        .get("models")
        .and_then(|v| v.as_table())
        .ok_or_else(|| "Kimi 供应商 config.toml 缺少 [models.*]".to_string())?;
    if models.is_empty() {
        return Err("Kimi 供应商 config.toml 缺少 [models.*]".to_string());
    }

    // 从备份合并时：先去掉本片段要接管的 provider id 对应的旧 models，避免残留。
    // 实际策略更简单——始终基于 previous_config 合并当前供应商，所以只需 overlay。
    let base_providers = base_doc
        .entry("providers".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or_else(|| "Kimi config.toml 的 [providers] 不是表".to_string())?;
    for (key, value) in providers {
        // 永不覆盖 managed:kimi-code（OAuth 托管账号）。
        if key == "managed:kimi-code" {
            continue;
        }
        base_providers.insert(key.clone(), value.clone());
    }

    let base_models = base_doc
        .entry("models".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or_else(|| "Kimi config.toml 的 [models] 不是表".to_string())?;
    for (key, value) in models {
        base_models.insert(key.clone(), value.clone());
    }

    if let Some(default_model) = provider_doc.get("default_model") {
        base_doc.insert("default_model".to_string(), default_model.clone());
    }

    let mut rendered =
        toml::to_string_pretty(&base_doc).map_err(|e| format!("序列化 Kimi config 失败：{e}"))?;
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    Ok(rendered)
}

fn materialize_kimi(config: &ExternalCliAgentConfig) -> Result<(), String> {
    let path = kimi_config_path().ok_or_else(|| "无法定位 Kimi 配置目录".to_string())?;
    let state_path = kimi_state_path().ok_or_else(|| "无法定位 Kimi 状态文件".to_string())?;
    let _guard = NATIVE_CONFIG_LOCK
        .lock()
        .map_err(|_| "原生 CLI 配置写锁已损坏".to_string())?;
    materialize_kimi_at(config, &path, &state_path)
}

fn materialize_kimi_at(
    config: &ExternalCliAgentConfig,
    path: &Path,
    state_path: &Path,
) -> Result<(), String> {
    let mut state = read_kimi_state(state_path)?;

    if config.current_provider.trim().is_empty() {
        if state.managed {
            if let Some(prev) = state.previous_config.take() {
                if prev.is_empty() {
                    if path.is_file() {
                        std::fs::remove_file(path)
                            .map_err(|e| format!("删除 {} 失败：{e}", path.display()))?;
                    }
                } else {
                    write_private_atomic(path, &prev)?;
                }
            }
            state.managed = false;
            write_kimi_state(state_path, &state)?;
        }
        return Ok(());
    }

    let provider = config
        .providers
        .iter()
        .find(|p| p.id == config.current_provider)
        .ok_or_else(|| format!("当前供应商 {} 不存在", config.current_provider))?;
    if provider.config_toml.trim().is_empty() {
        return Err(format!(
            "供应商 {} 缺少可落盘的 Kimi config.toml",
            provider.name
        ));
    }

    if !state.managed {
        state.previous_config = Some(read_text_or_empty(path)?);
        state.managed = true;
    }
    let base = state.previous_config.as_deref().unwrap_or("");
    let merged = merge_kimi_provider_config(base, &provider.config_toml)?;
    write_private_atomic(path, &merged)?;
    write_kimi_state(state_path, &state)
}

#[derive(Debug, Clone)]
struct NativeProviderEntry {
    source_id: String,
    native_id: String,
    config: Value,
    auth: Option<Value>,
    default_model: String,
    default_thinking_level: Option<String>,
}

fn materialize_native(agent_id: &str, config: &ExternalCliAgentConfig) -> Result<(), String> {
    let paths = match agent_id {
        "opencode" => opencode_paths(),
        "pi" => pi_paths(),
        _ => None,
    }
    .ok_or_else(|| format!("无法定位 {agent_id} 的原生配置目录"))?;
    let _guard = NATIVE_CONFIG_LOCK
        .lock()
        .map_err(|_| "原生 CLI 配置写锁已损坏".to_string())?;
    materialize_native_at(agent_id, config, &paths)
}

fn materialize_native_at(
    agent_id: &str,
    config: &ExternalCliAgentConfig,
    paths: &NativePaths,
) -> Result<(), String> {
    let entries = parse_native_entries(agent_id, &config.providers)?;
    let active = if config.current_provider.trim().is_empty() {
        None
    } else {
        let provider = config
            .providers
            .iter()
            .find(|provider| provider.id == config.current_provider)
            .ok_or_else(|| format!("当前供应商 {} 不存在", config.current_provider))?;
        if provider.config_json.trim().is_empty() {
            // 升级前的 env-only 条目不参与原生默认值接管，但仍是合法的当前供应商。
            None
        } else {
            Some(
                entries
                    .iter()
                    .find(|entry| entry.source_id == config.current_provider)
                    .ok_or_else(|| {
                        format!(
                            "当前供应商 {} 没有可落盘的 {agent_id} 原生配置",
                            config.current_provider
                        )
                    })?,
            )
        }
    };

    let mut state = read_managed_state(&paths.state)?;
    let previous_ids: HashSet<String> = state.managed_provider_ids.iter().cloned().collect();
    let next_ids: HashSet<String> = entries
        .iter()
        .map(|entry| entry.native_id.clone())
        .collect();

    match agent_id {
        "opencode" => {
            migrate_shadowed_opencode_configs(paths, &mut state, &previous_ids, &entries)?;
            let mut root = read_object_file(&paths.config, true, false, "OpenCode opencode.json")?;
            let before_root = root.clone();
            ensure_provider_id_available(&root, "provider", &previous_ids, &entries)?;
            sync_provider_map(agent_id, &mut root, "provider", &previous_ids, &entries)?;

            let mut auth = read_object_file(&paths.auth, false, true, "OpenCode auth.json")?;
            let before_auth = auth.clone();
            ensure_auth_id_available(&auth, &previous_ids, &entries)?;
            sync_auth_map(&mut auth, &previous_ids, &entries);

            apply_default_fields(
                &mut root,
                &mut state,
                &["model"],
                active.map(|entry| {
                    vec![(
                        "model",
                        Value::String(format!("{}/{}", entry.native_id, entry.default_model)),
                    )]
                }),
            );
            state.managed_provider_ids = sorted_ids(next_ids);
            prewrite_new_backup(&paths.state, &state)?;
            write_object_if_changed(&paths.config, before_root, &root)?;
            write_object_if_changed(&paths.auth, before_auth, &auth)?;
        }
        "pi" => {
            let mut models = read_object_file(&paths.config, false, false, "Pi models.json")?;
            let before_models = models.clone();
            ensure_provider_id_available(&models, "providers", &previous_ids, &entries)?;
            sync_provider_map(agent_id, &mut models, "providers", &previous_ids, &entries)?;

            let mut auth = read_object_file(&paths.auth, false, true, "Pi auth.json")?;
            let before_auth = auth.clone();
            sync_auth_map(&mut auth, &previous_ids, &entries);

            let settings_path = paths
                .settings
                .as_ref()
                .ok_or_else(|| "Pi settings.json 路径缺失".to_string())?;
            let mut settings = read_object_file(settings_path, false, false, "Pi settings.json")?;
            let before_settings = settings.clone();
            apply_default_fields(
                &mut settings,
                &mut state,
                &["defaultProvider", "defaultModel", "defaultThinkingLevel"],
                active.map(|entry| {
                    let mut values = vec![
                        ("defaultProvider", Value::String(entry.native_id.clone())),
                        ("defaultModel", Value::String(entry.default_model.clone())),
                    ];
                    if let Some(level) = &entry.default_thinking_level {
                        values.push(("defaultThinkingLevel", Value::String(level.clone())));
                    }
                    values
                }),
            );
            state.managed_provider_ids = sorted_ids(next_ids);
            prewrite_new_backup(&paths.state, &state)?;
            write_object_if_changed(&paths.config, before_models, &models)?;
            write_object_if_changed(&paths.auth, before_auth, &auth)?;
            write_object_if_changed(settings_path, before_settings, &settings)?;
        }
        _ => return Ok(()),
    }

    write_managed_state(&paths.state, &state)
}

fn migrate_shadowed_opencode_configs(
    paths: &NativePaths,
    state: &mut NativeManagedState,
    previous_ids: &HashSet<String>,
    entries: &[NativeProviderEntry],
) -> Result<(), String> {
    for path in &paths.alternate_configs {
        let root = read_object_file(path, true, false, "OpenCode shadowed config")?;
        ensure_provider_id_available(&root, "provider", previous_ids, entries)?;
    }
    for path in &paths.alternate_configs {
        let mut root = read_object_file(path, true, false, "OpenCode shadowed config")?;
        let before = root.clone();
        if state.defaults_managed
            && root
                .get("model")
                .and_then(Value::as_str)
                .and_then(|model| model.split_once('/'))
                .is_some_and(|(provider, _)| previous_ids.contains(provider))
        {
            apply_default_fields(&mut root, state, &["model"], None);
        }
        remove_managed_provider_ids(&mut root, "provider", previous_ids)?;
        write_object_if_changed(path, before, &root)?;
    }
    Ok(())
}

fn parse_native_entries(
    agent_id: &str,
    providers: &[ExternalCliProvider],
) -> Result<Vec<NativeProviderEntry>, String> {
    let mut entries = Vec::new();
    let mut native_ids = HashSet::new();
    for provider in providers {
        // 兼容升级前创建的 env-only 条目；编辑并保存后才变成原生配置。
        if provider.config_json.trim().is_empty() {
            continue;
        }
        let native_id = resolve_native_provider_id(provider)?;
        if !native_ids.insert(native_id.clone()) {
            return Err(format!("供应商 id 归一化后冲突：{native_id}"));
        }
        let mut config = parse_object_text(&provider.config_json, "provider configJson")?;
        // 旧 configJson 可能还没写 forceAdaptiveThinking；同步到 models.json 时补上，
        // 否则 pi-cache-optimizer 会对 opus≥4.6 / sonnet≥4.6 / fable≥5 弹告警。
        if agent_id == "pi" {
            ensure_pi_adaptive_thinking_compat(&mut config);
        }
        let auth = if provider.auth_json.trim().is_empty() {
            Map::new()
        } else {
            parse_object_text(&provider.auth_json, "provider authJson")?
        };
        let default_model = provider.default_model.trim().to_string();
        if default_model.is_empty() {
            return Err(format!("供应商 {} 缺少默认模型", provider.name));
        }
        validate_native_provider(agent_id, &config, &default_model, &provider.name)?;
        validate_native_auth(agent_id, &auth, &provider.name)?;
        let default_thinking_level = if agent_id == "pi" {
            let level = provider.default_reasoning.trim();
            if level.is_empty() {
                None
            } else if matches!(
                level,
                "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
            ) {
                Some(level.to_string())
            } else {
                return Err(format!("供应商 {} 的 Pi 默认推理等级无效", provider.name));
            }
        } else {
            None
        };
        if let Some(level) = default_thinking_level.as_deref() {
            validate_pi_thinking_level(&config, &default_model, level, &provider.name)?;
        }
        entries.push(NativeProviderEntry {
            source_id: provider.id.clone(),
            native_id,
            config: Value::Object(config),
            auth: (!auth.is_empty()).then(|| Value::Object(auth)),
            default_model,
            default_thinking_level,
        });
    }
    Ok(entries)
}

fn validate_pi_thinking_level(
    config: &Map<String, Value>,
    default_model: &str,
    level: &str,
    provider_name: &str,
) -> Result<(), String> {
    if level == "off" {
        return Ok(());
    }
    let model = config
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| {
            models
                .iter()
                .find(|model| model.get("id").and_then(Value::as_str) == Some(default_model))
        })
        .ok_or_else(|| format!("供应商 {provider_name} 的默认模型不在 models 列表中"))?;
    if model.get("reasoning").and_then(Value::as_bool) != Some(true) {
        return Err(format!(
            "供应商 {provider_name} 的默认模型未声明支持推理，仅可使用 off"
        ));
    }

    let mapping = model.get("thinkingLevelMap").and_then(Value::as_object);
    let supported = match mapping.and_then(|mapping| mapping.get(level)) {
        Some(value) => !value.is_null(),
        None => matches!(level, "minimal" | "low" | "medium" | "high"),
    };
    if supported {
        Ok(())
    } else {
        Err(format!(
            "供应商 {provider_name} 的默认模型不支持 Pi 推理等级 {level}"
        ))
    }
}

/// Anthropic adaptive-generation Claude 判定（对齐 pi-cache-optimizer / 前端
/// `isPiAdaptiveThinkingModel`）：opus ≥4.6 / sonnet ≥4.6 / fable ≥5。
fn is_pi_adaptive_thinking_model(model_id: &str) -> bool {
    static RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)(^|[/\s:_-])(opus-4[.-][6-9]|opus-4-[1-9][0-9]|opus-([5-9]|[1-9][0-9])|sonnet-4[.-][6-9]|sonnet-4-[1-9][0-9]|sonnet-([5-9]|[1-9][0-9])|fable-([5-9]|[1-9][0-9]))($|[-_.:/\s\[])",
        )
        .expect("adaptive thinking model pattern")
    });
    RE.is_match(model_id)
}

/// 给 pi 的 anthropic-messages 渠道里 adaptive 模型补 `compat.forceAdaptiveThinking`。
/// 已有 true 不覆盖；非 anthropic-messages / 非 adaptive 模型不动。
fn ensure_pi_adaptive_thinking_compat(config: &mut Map<String, Value>) {
    if config.get("api").and_then(Value::as_str) != Some("anthropic-messages") {
        return;
    }
    let Some(models) = config.get_mut("models").and_then(Value::as_array_mut) else {
        return;
    };
    for model in models {
        let Some(obj) = model.as_object_mut() else {
            continue;
        };
        let Some(id) = obj.get("id").and_then(Value::as_str).map(str::to_string) else {
            continue;
        };
        if !is_pi_adaptive_thinking_model(&id) {
            continue;
        }
        let compat = obj
            .entry("compat".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(compat_obj) = compat.as_object_mut() {
            if compat_obj
                .get("forceAdaptiveThinking")
                .and_then(Value::as_bool)
                != Some(true)
            {
                compat_obj.insert("forceAdaptiveThinking".to_string(), Value::Bool(true));
            }
        }
    }
}

fn validate_native_provider(
    agent_id: &str,
    config: &Map<String, Value>,
    default_model: &str,
    name: &str,
) -> Result<(), String> {
    let nonempty = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    };
    match agent_id {
        "opencode" => {
            if !nonempty(config.get("npm")) {
                return Err(format!("供应商 {name} 的 OpenCode 配置缺少 npm"));
            }
            if config.get("npm").and_then(Value::as_str) == Some("@ai-sdk/openai-compatible")
                && !config
                    .get("options")
                    .and_then(Value::as_object)
                    .is_some_and(|options| nonempty(options.get("baseURL")))
            {
                return Err(format!(
                    "供应商 {name} 使用 OpenAI Compatible 时必须填写 options.baseURL"
                ));
            }
        }
        "pi" => {
            const APIS: &[&str] = &[
                "openai-completions",
                "openai-responses",
                "anthropic-messages",
                "google-generative-ai",
            ];
            let api = config.get("api").and_then(Value::as_str).unwrap_or("");
            if !nonempty(config.get("baseUrl")) || !APIS.contains(&api) {
                return Err(format!("供应商 {name} 的 Pi baseUrl 或 api 无效"));
            }
        }
        _ => {}
    }
    let model_exists = match agent_id {
        "opencode" => config
            .get("models")
            .and_then(Value::as_object)
            .is_some_and(|models| models.contains_key(default_model)),
        "pi" => config
            .get("models")
            .and_then(Value::as_array)
            .is_some_and(|models| {
                models
                    .iter()
                    .any(|model| model.get("id").and_then(Value::as_str) == Some(default_model))
            }),
        _ => true,
    };
    if !model_exists {
        return Err(format!("供应商 {name} 的默认模型不在 models 列表中"));
    }
    Ok(())
}

fn validate_native_auth(
    agent_id: &str,
    auth: &Map<String, Value>,
    name: &str,
) -> Result<(), String> {
    if agent_id == "opencode" && auth.is_empty() {
        return Ok(());
    }
    let expected_type = if agent_id == "opencode" {
        "api"
    } else {
        "api_key"
    };
    let valid = auth.get("type").and_then(Value::as_str) == Some(expected_type)
        && auth
            .get("key")
            .and_then(Value::as_str)
            .is_some_and(|key| !key.trim().is_empty());
    if valid {
        Ok(())
    } else {
        Err(format!("供应商 {name} 的原生凭据格式无效"))
    }
}

/// 解析写入 CLI 原生配置的 provider id：显式 nativeProviderId → 显示名 slug → 内部 id slug。
fn resolve_native_provider_id(provider: &ExternalCliProvider) -> Result<String, String> {
    let configured = provider.native_provider_id.trim();
    if !configured.is_empty() {
        if !valid_explicit_native_provider_id(configured) {
            return Err(format!(
                "供应商 {} 的原生 provider id 无效：{}",
                provider.name, configured
            ));
        }
        return Ok(configured.to_string());
    }
    if let Some(slug) = slugify_provider_key(&provider.name) {
        return Ok(slug);
    }
    if let Some(slug) = slugify_provider_key(&provider.id) {
        return Ok(slug);
    }
    Err(format!(
        "供应商 id 无法生成原生 provider id：{}",
        provider.id
    ))
}

/// 把名字/内部 id 压成合法 provider key（小写 ascii + `.` `_` `-`），规则对齐前端 `nativeProviderIdFromName`。
fn slugify_provider_key(raw: &str) -> Option<String> {
    let mut slug = String::new();
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if matches!(ch, '.' | '_' | '-') {
            slug.push(ch);
        } else if slug.chars().last() != Some('-') {
            slug.push('-');
        }
    }
    let mut collapsed = String::with_capacity(slug.len());
    for ch in slug.chars() {
        if ch == '-' && collapsed.ends_with('-') {
            continue;
        }
        collapsed.push(ch);
    }
    let trimmed = collapsed.trim_matches(|c| matches!(c, '.' | '_' | '-'));
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn native_cleanup_ids(
    provider_id: &str,
    provider_name: Option<&str>,
    configured_native_id: Option<&str>,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut push = |id: String| {
        if !id.is_empty() && !ids.iter().any(|existing| existing == &id) {
            ids.push(id);
        }
    };
    if let Some(id) = configured_native_id
        .map(str::trim)
        .filter(|id| valid_explicit_native_provider_id(id))
    {
        push(id.to_string());
    }
    if let Some(name) = provider_name.map(str::trim).filter(|name| !name.is_empty()) {
        if let Some(slug) = slugify_provider_key(name) {
            push(format!("kivio-{slug}"));
            push(slug);
        }
    }
    if let Some(slug) = slugify_provider_key(provider_id) {
        push(format!("kivio-{slug}"));
        push(slug);
    }
    ids
}

fn valid_explicit_native_provider_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn ensure_provider_id_available(
    root: &Map<String, Value>,
    field: &str,
    previous_ids: &HashSet<String>,
    entries: &[NativeProviderEntry],
) -> Result<(), String> {
    let Some(existing) = root.get(field).and_then(Value::as_object) else {
        return Ok(());
    };
    if let Some(entry) = entries.iter().find(|entry| {
        existing.contains_key(&entry.native_id) && !previous_ids.contains(&entry.native_id)
    }) {
        return Err(format!(
            "原生配置中已存在非 Kivio 管理的供应商 id：{}",
            entry.native_id
        ));
    }
    Ok(())
}

fn ensure_auth_id_available(
    auth: &Map<String, Value>,
    previous_ids: &HashSet<String>,
    entries: &[NativeProviderEntry],
) -> Result<(), String> {
    if let Some(entry) = entries.iter().find(|entry| {
        auth.contains_key(&entry.native_id) && !previous_ids.contains(&entry.native_id)
    }) {
        return Err(format!(
            "原生 auth.json 中已存在非 Kivio 管理的供应商 id：{}",
            entry.native_id
        ));
    }
    Ok(())
}

fn merge_object(base: &mut Map<String, Value>, incoming: &Map<String, Value>) {
    for (key, value) in incoming {
        base.insert(key.clone(), value.clone());
    }
}

fn merge_opencode_model(
    mut existing: Map<String, Value>,
    incoming: &Map<String, Value>,
) -> Map<String, Value> {
    for (key, value) in incoming {
        if let (Some(base), Some(incoming)) = (
            existing.get(key).and_then(Value::as_object).cloned(),
            value.as_object(),
        ) {
            let mut merged = base;
            merge_object(&mut merged, incoming);
            existing.insert(key.clone(), Value::Object(merged));
        } else {
            existing.insert(key.clone(), value.clone());
        }
    }
    existing
}

fn merge_opencode_provider(existing: Value, incoming: &Value) -> Value {
    let (Some(mut existing), Some(incoming)) =
        (existing.as_object().cloned(), incoming.as_object())
    else {
        return incoming.clone();
    };

    for (key, value) in incoming {
        match key.as_str() {
            "options" => {
                let mut merged = existing
                    .get("options")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                if let Some(incoming) = value.as_object() {
                    merge_object(&mut merged, incoming);
                    if !incoming.contains_key("baseURL") {
                        merged.remove("baseURL");
                    }
                    existing.insert(key.clone(), Value::Object(merged));
                } else {
                    existing.insert(key.clone(), value.clone());
                }
            }
            "models" => {
                let old_models = existing.get("models").and_then(Value::as_object);
                let mut models = Map::new();
                if let Some(incoming_models) = value.as_object() {
                    for (model_id, model) in incoming_models {
                        let mut merged = old_models
                            .and_then(|models| models.get(model_id))
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default();
                        if let Some(incoming_model) = model.as_object() {
                            merged = merge_opencode_model(merged, incoming_model);
                            models.insert(model_id.clone(), Value::Object(merged));
                        } else {
                            models.insert(model_id.clone(), model.clone());
                        }
                    }
                    existing.insert(key.clone(), Value::Object(models));
                } else {
                    existing.insert(key.clone(), value.clone());
                }
            }
            _ => {
                existing.insert(key.clone(), value.clone());
            }
        }
    }
    Value::Object(existing)
}

fn remove_managed_provider_ids(
    root: &mut Map<String, Value>,
    field: &str,
    managed_ids: &HashSet<String>,
) -> Result<(), String> {
    let Some(value) = root.get_mut(field) else {
        return Ok(());
    };
    let providers = value
        .as_object_mut()
        .ok_or_else(|| format!("原生配置的 {field} 必须是对象"))?;
    for id in managed_ids {
        providers.remove(id);
    }
    Ok(())
}

fn sync_provider_map(
    agent_id: &str,
    root: &mut Map<String, Value>,
    field: &str,
    previous_ids: &HashSet<String>,
    entries: &[NativeProviderEntry],
) -> Result<(), String> {
    if previous_ids.is_empty() && entries.is_empty() {
        return Ok(());
    }
    let providers = root
        .entry(field.to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| format!("原生配置的 {field} 必须是对象"))?;
    let mut previous = HashMap::new();
    for id in previous_ids {
        if let Some(value) = providers.remove(id) {
            previous.insert(id.clone(), value);
        }
    }
    for entry in entries {
        let config = if agent_id == "opencode" {
            previous
                .remove(&entry.native_id)
                .map(|existing| merge_opencode_provider(existing, &entry.config))
                .unwrap_or_else(|| entry.config.clone())
        } else {
            entry.config.clone()
        };
        providers.insert(entry.native_id.clone(), config);
    }
    Ok(())
}

fn sync_auth_map(
    auth: &mut Map<String, Value>,
    previous_ids: &HashSet<String>,
    entries: &[NativeProviderEntry],
) {
    for id in previous_ids {
        auth.remove(id);
    }
    for entry in entries {
        if let Some(value) = &entry.auth {
            auth.insert(entry.native_id.clone(), value.clone());
        }
    }
}

fn apply_default_fields(
    root: &mut Map<String, Value>,
    state: &mut NativeManagedState,
    keys: &[&str],
    active_values: Option<Vec<(&str, Value)>>,
) {
    match active_values {
        Some(values) => {
            for key in keys {
                state
                    .previous_defaults
                    .entry((*key).to_string())
                    .or_insert_with(|| {
                        let value = root.get(*key).cloned();
                        BackedUpField {
                            present: value.is_some(),
                            value: value.unwrap_or(Value::Null),
                        }
                    });
            }
            state.defaults_managed = true;
            for (key, value) in values {
                root.insert(key.to_string(), value);
            }
        }
        None if state.defaults_managed => {
            for key in keys {
                match state.previous_defaults.get(*key) {
                    Some(backup) if backup.present => {
                        root.insert((*key).to_string(), backup.value.clone());
                    }
                    _ => {
                        root.remove(*key);
                    }
                }
            }
            state.defaults_managed = false;
            state.previous_defaults.clear();
        }
        None => {}
    }
}

fn sorted_ids(ids: HashSet<String>) -> Vec<String> {
    let mut ids: Vec<String> = ids.into_iter().collect();
    ids.sort();
    ids
}

/// 首次接管默认模型时先把备份状态落盘；即使进程在随后写 CLI 配置时退出，也不会丢恢复点。
fn prewrite_new_backup(path: &Path, state: &NativeManagedState) -> Result<(), String> {
    if state.defaults_managed {
        write_managed_state(path, state)?;
    }
    Ok(())
}

fn read_managed_state(path: &Path) -> Result<NativeManagedState, String> {
    if !path.is_file() {
        return Ok(NativeManagedState::default());
    }
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("解析 {} 失败：{e}", path.display()))
}

fn write_managed_state(path: &Path, state: &NativeManagedState) -> Result<(), String> {
    let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())? + "\n";
    if std::fs::read_to_string(path).ok().as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    write_private_atomic(path, &text)
}

fn parse_object_text(text: &str, label: &str) -> Result<Map<String, Value>, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| format!("{label} 解析失败：{e}"))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| format!("{label} 顶层必须是对象"))
}

fn read_object_file(
    path: &Path,
    jsonc: bool,
    empty_ok: bool,
    label: &str,
) -> Result<Map<String, Value>, String> {
    if !path.is_file() {
        return Ok(Map::new());
    }
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    if text.trim().is_empty() && empty_ok {
        return Ok(Map::new());
    }
    let value: Value = if jsonc {
        json5::from_str(&text).map_err(|e| format!("{label} 解析失败：{e}"))?
    } else {
        serde_json::from_str(&text).map_err(|e| format!("{label} 解析失败：{e}"))?
    };
    value
        .as_object()
        .cloned()
        .ok_or_else(|| format!("{label} 顶层必须是对象"))
}

fn write_object_if_changed(
    path: &Path,
    before: Map<String, Value>,
    after: &Map<String, Value>,
) -> Result<(), String> {
    if &before == after {
        return Ok(());
    }
    let text = serde_json::to_string_pretty(&Value::Object(after.clone()))
        .map_err(|e| e.to_string())?
        + "\n";
    write_private_atomic(path, &text)
}

/// 删除供应商时清掉它物化出来的文件。失败只记日志：残留一个读不到的旧文件不影响正确性。
pub fn cleanup(
    agent_id: &str,
    provider_id: &str,
    native_provider_id: Option<&str>,
    provider_name: Option<&str>,
) {
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
        "opencode" | "pi" => {
            if let Err(err) =
                cleanup_native(agent_id, provider_id, native_provider_id, provider_name)
            {
                eprintln!("[external-agent] 清理 {agent_id} 原生供应商失败：{err}");
            }
        }
        _ => {}
    }
}

fn cleanup_native(
    agent_id: &str,
    provider_id: &str,
    native_provider_id: Option<&str>,
    provider_name: Option<&str>,
) -> Result<(), String> {
    let paths = match agent_id {
        "opencode" => opencode_paths(),
        "pi" => pi_paths(),
        _ => None,
    }
    .ok_or_else(|| format!("无法定位 {agent_id} 的原生配置目录"))?;
    let _guard = NATIVE_CONFIG_LOCK
        .lock()
        .map_err(|_| "原生 CLI 配置写锁已损坏".to_string())?;
    cleanup_native_at(
        agent_id,
        provider_id,
        native_provider_id,
        provider_name,
        &paths,
    )
}

fn cleanup_native_at(
    agent_id: &str,
    provider_id: &str,
    configured_native_id: Option<&str>,
    provider_name: Option<&str>,
    paths: &NativePaths,
) -> Result<(), String> {
    let native_ids = native_cleanup_ids(provider_id, provider_name, configured_native_id);
    if native_ids.is_empty() {
        return Ok(());
    }
    let matches_id =
        |value: Option<&str>| value.is_some_and(|value| native_ids.iter().any(|id| id == value));
    let mut state = read_managed_state(&paths.state)?;
    match agent_id {
        "opencode" => {
            for config_path in std::iter::once(&paths.config).chain(&paths.alternate_configs) {
                let mut config = read_object_file(config_path, true, false, "原生 provider 配置")?;
                let before_config = config.clone();
                if state.defaults_managed
                    && config
                        .get("model")
                        .and_then(Value::as_str)
                        .and_then(|model| model.split_once('/'))
                        .is_some_and(|(provider, _)| matches_id(Some(provider)))
                {
                    apply_default_fields(&mut config, &mut state, &["model"], None);
                }
                if let Some(providers) = config.get_mut("provider").and_then(Value::as_object_mut) {
                    for id in &native_ids {
                        providers.remove(id);
                    }
                }
                write_object_if_changed(config_path, before_config, &config)?;
            }
        }
        "pi" => {
            let mut config = read_object_file(&paths.config, false, false, "原生 provider 配置")?;
            let before_config = config.clone();
            if let Some(providers) = config.get_mut("providers").and_then(Value::as_object_mut) {
                for id in &native_ids {
                    providers.remove(id);
                }
            }
            write_object_if_changed(&paths.config, before_config, &config)?;

            if let Some(settings_path) = paths.settings.as_ref() {
                let mut settings =
                    read_object_file(settings_path, false, false, "Pi settings.json")?;
                let before_settings = settings.clone();
                if state.defaults_managed
                    && matches_id(settings.get("defaultProvider").and_then(Value::as_str))
                {
                    apply_default_fields(
                        &mut settings,
                        &mut state,
                        &["defaultProvider", "defaultModel", "defaultThinkingLevel"],
                        None,
                    );
                }
                write_object_if_changed(settings_path, before_settings, &settings)?;
            }
        }
        _ => return Ok(()),
    }
    let mut auth = read_object_file(&paths.auth, false, true, "原生 auth.json")?;
    let before_auth = auth.clone();
    for id in &native_ids {
        auth.remove(id);
    }
    write_object_if_changed(&paths.auth, before_auth, &auth)?;
    state
        .managed_provider_ids
        .retain(|id| !native_ids.iter().any(|candidate| candidate == id));
    write_managed_state(&paths.state, &state)
}

/// 文件里有 API key，权限收到 0600（Windows 无 unix 权限位，靠 app data 目录本身的 ACL）。
fn write_private(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建 {} 失败：{e}", parent.display()))?;
    }
    std::fs::write(path, content).map_err(|e| format!("写入 {} 失败：{e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// 原生配置会被独立 CLI 同时读取：同目录临时文件 fsync 后 rename，避免读到半截 JSON。
pub(crate) fn write_private_atomic(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建 {} 失败：{e}", parent.display()))?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("无效文件名：{}", path.display()))?;
    let temp = path.with_file_name(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<(), std::io::Error> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    })();
    if let Err(err) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("原子写入 {} 失败：{err}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_paths(root: &Path, pi: bool) -> NativePaths {
        NativePaths {
            config: root.join(if pi { "models.json" } else { "opencode.json" }),
            alternate_configs: Vec::new(),
            auth: root.join("auth.json"),
            settings: pi.then(|| root.join("settings.json")),
            state: root.join("kivio-state.json"),
        }
    }

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("kivio-{label}-{}", uuid::Uuid::new_v4()))
    }

    fn native_provider(
        id: &str,
        name: &str,
        config_json: Value,
        auth_json: Value,
        default_model: &str,
    ) -> ExternalCliProvider {
        ExternalCliProvider {
            id: id.to_string(),
            name: name.to_string(),
            config_json: serde_json::to_string(&config_json).unwrap(),
            auth_json: serde_json::to_string(&auth_json).unwrap(),
            default_model: default_model.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn sanitize_rejects_path_escapes() {
        assert_eq!(
            sanitize_segment("loki-claude").as_deref(),
            Some("loki-claude")
        );
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

    #[test]
    fn slugify_provider_key_keeps_dot_and_underscore() {
        assert_eq!(
            slugify_provider_key("My.Relay").as_deref(),
            Some("my.relay")
        );
        assert_eq!(
            slugify_provider_key("hello_world").as_deref(),
            Some("hello_world")
        );
        assert_eq!(
            slugify_provider_key("Relay One").as_deref(),
            Some("relay-one")
        );
        assert_eq!(slugify_provider_key("a--b").as_deref(), Some("a-b"));
    }

    #[test]
    fn native_cleanup_ids_prefer_name_slug_and_keep_legacy_aliases() {
        let ids = native_cleanup_ids("p-msoeiznl", Some("Relay One"), None);
        assert!(ids.iter().any(|id| id == "relay-one"));
        assert!(ids.iter().any(|id| id == "kivio-relay-one"));
        assert!(ids.iter().any(|id| id == "p-msoeiznl"));
        assert!(ids.iter().any(|id| id == "kivio-p-msoeiznl"));
    }

    #[test]
    fn opencode_merges_jsonc_and_restores_previous_default() {
        let root = temp_root("opencode-native");
        let paths = test_paths(&root, false);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            &paths.config,
            r#"{
              // user comment: JSONC must parse
              "model": "anthropic/claude-old",
              "mcp": { "keep": true },
              "provider": { "user-relay": { "name": "Keep me" } },
            }"#,
        )
        .unwrap();
        std::fs::write(&paths.auth, r#"{"user-relay":{"type":"api","key":"keep"}}"#).unwrap();

        let provider = native_provider(
            "Relay One",
            "Relay One",
            serde_json::json!({
                "npm": "@ai-sdk/openai-compatible",
                "name": "Relay One",
                "options": { "baseURL": "https://relay.example/v1" },
                "models": { "gpt-test": { "name": "GPT Test" } }
            }),
            serde_json::json!({ "type": "api", "key": "sk-test" }),
            "gpt-test",
        );
        let mut config = ExternalCliAgentConfig {
            providers: vec![provider],
            current_provider: "Relay One".to_string(),
            ..Default::default()
        };

        materialize_native_at("opencode", &config, &paths).unwrap();
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.config).unwrap()).unwrap();
        assert_eq!(written["mcp"]["keep"], true);
        assert_eq!(written["provider"]["user-relay"]["name"], "Keep me");
        assert_eq!(
            written["provider"]["relay-one"]["models"]["gpt-test"]["name"],
            "GPT Test"
        );
        assert_eq!(written["model"], "relay-one/gpt-test");
        let auth: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.auth).unwrap()).unwrap();
        assert_eq!(auth["user-relay"]["key"], "keep");
        assert_eq!(auth["relay-one"]["key"], "sk-test");

        let mut user_edited = written.clone();
        user_edited["provider"]["relay-one"]["headerTimeout"] = serde_json::json!(12_000);
        user_edited["provider"]["relay-one"]["models"]["gpt-test"]["variants"] =
            serde_json::json!({ "high": { "reasoningEffort": "high" } });
        std::fs::write(
            &paths.config,
            serde_json::to_string_pretty(&user_edited).unwrap(),
        )
        .unwrap();
        materialize_native_at("opencode", &config, &paths).unwrap();
        let merged: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.config).unwrap()).unwrap();
        assert_eq!(merged["provider"]["relay-one"]["headerTimeout"], 12_000);
        assert_eq!(
            merged["provider"]["relay-one"]["models"]["gpt-test"]["variants"]["high"]
                ["reasoningEffort"],
            "high"
        );

        config.current_provider.clear();
        materialize_native_at("opencode", &config, &paths).unwrap();
        let restored: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.config).unwrap()).unwrap();
        assert_eq!(restored["model"], "anthropic/claude-old");
        assert!(restored["provider"]["relay-one"].is_object());

        cleanup_native_at("opencode", "p-msoeiznl", None, Some("Relay One"), &paths).unwrap();
        let cleaned: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.config).unwrap()).unwrap();
        assert!(cleaned["provider"].get("relay-one").is_none());
        assert!(cleaned["provider"]["user-relay"].is_object());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn opencode_uses_stable_id_and_removes_managed_auth_when_key_is_cleared() {
        let root = temp_root("opencode-stable-id");
        let paths = test_paths(&root, false);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&paths.config, "{}").unwrap();
        std::fs::write(&paths.auth, "{}").unwrap();
        let mut provider = native_provider(
            "internal-timestamp-id",
            "Relay",
            serde_json::json!({
                "npm": "@ai-sdk/openai-compatible",
                "name": "Relay",
                "options": { "baseURL": "https://relay.example/v1" },
                "models": { "claude-test": { "name": "Claude Test" } }
            }),
            serde_json::json!({ "type": "api", "key": "sk-test" }),
            "claude-test",
        );
        provider.native_provider_id = "team-relay".to_string();
        let mut config = ExternalCliAgentConfig {
            providers: vec![provider],
            current_provider: "internal-timestamp-id".to_string(),
            ..Default::default()
        };

        materialize_native_at("opencode", &config, &paths).unwrap();
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.config).unwrap()).unwrap();
        assert!(written["provider"]["team-relay"].is_object());
        assert_eq!(written["model"], "team-relay/claude-test");

        config.providers[0].auth_json.clear();
        config.providers[0].config_json = serde_json::to_string(&serde_json::json!({
            "npm": "@ai-sdk/anthropic",
            "name": "Relay",
            "options": {},
            "models": { "claude-test": { "name": "Claude Test" } }
        }))
        .unwrap();
        materialize_native_at("opencode", &config, &paths).unwrap();
        let switched: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.config).unwrap()).unwrap();
        assert_eq!(
            switched["provider"]["team-relay"]["npm"],
            "@ai-sdk/anthropic"
        );
        assert!(switched["provider"]["team-relay"]["options"]
            .get("baseURL")
            .is_none());
        let auth: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.auth).unwrap()).unwrap();
        assert!(auth.get("team-relay").is_none());

        cleanup_native_at(
            "opencode",
            "internal-timestamp-id",
            Some("team-relay"),
            Some("Relay"),
            &paths,
        )
        .unwrap();
        let cleaned: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.config).unwrap()).unwrap();
        assert!(cleaned["provider"].get("team-relay").is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn opencode_refuses_to_overwrite_unmanaged_provider_id() {
        let root = temp_root("opencode-provider-collision");
        let paths = test_paths(&root, false);
        std::fs::create_dir_all(&root).unwrap();
        let original = r#"{"provider":{"team-relay":{"name":"User managed"}}}"#;
        std::fs::write(&paths.config, original).unwrap();
        std::fs::write(&paths.auth, "{}").unwrap();
        let mut provider = native_provider(
            "internal-id",
            "Relay",
            serde_json::json!({
                "npm": "@ai-sdk/openai-compatible",
                "name": "Relay",
                "options": { "baseURL": "https://relay.example/v1" },
                "models": { "gpt-test": { "name": "GPT Test" } }
            }),
            serde_json::json!({ "type": "api", "key": "sk-test" }),
            "gpt-test",
        );
        provider.native_provider_id = "team-relay".to_string();
        let config = ExternalCliAgentConfig {
            providers: vec![provider],
            current_provider: "internal-id".to_string(),
            ..Default::default()
        };

        let err = materialize_native_at("opencode", &config, &paths).unwrap_err();
        assert!(err.contains("非 Kivio 管理"));
        assert_eq!(std::fs::read_to_string(&paths.config).unwrap(), original);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn opencode_moves_managed_entries_to_the_highest_priority_config() {
        let root = temp_root("opencode-config-precedence");
        std::fs::create_dir_all(&root).unwrap();
        let lower_path = root.join("opencode.json");
        let higher_path = root.join("opencode.jsonc");
        let lower_paths = test_paths(&root, false);
        std::fs::write(&lower_path, r#"{"model":"user/old"}"#).unwrap();
        std::fs::write(&lower_paths.auth, "{}").unwrap();
        let provider = native_provider(
            "Relay One",
            "Relay One",
            serde_json::json!({
                "npm": "@ai-sdk/openai-compatible",
                "name": "Relay One",
                "options": { "baseURL": "https://relay.example/v1" },
                "models": { "gpt-test": { "name": "GPT Test" } }
            }),
            serde_json::json!({ "type": "api", "key": "sk-test" }),
            "gpt-test",
        );
        let mut config = ExternalCliAgentConfig {
            providers: vec![provider],
            current_provider: "Relay One".to_string(),
            ..Default::default()
        };
        materialize_native_at("opencode", &config, &lower_paths).unwrap();

        std::fs::write(
            &higher_path,
            r#"{
              // Higher-priority user config.
              "model": "top/selected",
              "provider": { "top": { "name": "Top" } },
            }"#,
        )
        .unwrap();
        let higher_paths = NativePaths {
            config: higher_path.clone(),
            alternate_configs: vec![lower_path.clone()],
            auth: lower_paths.auth.clone(),
            settings: None,
            state: lower_paths.state.clone(),
        };
        materialize_native_at("opencode", &config, &higher_paths).unwrap();

        let lower: Value =
            serde_json::from_str(&std::fs::read_to_string(&lower_path).unwrap()).unwrap();
        assert_eq!(lower["model"], "user/old");
        assert!(lower["provider"].get("relay-one").is_none());
        let higher: Value =
            serde_json::from_str(&std::fs::read_to_string(&higher_path).unwrap()).unwrap();
        assert_eq!(higher["model"], "relay-one/gpt-test");
        assert!(higher["provider"]["top"].is_object());

        config.current_provider.clear();
        materialize_native_at("opencode", &config, &higher_paths).unwrap();
        let restored: Value =
            serde_json::from_str(&std::fs::read_to_string(&higher_path).unwrap()).unwrap();
        assert_eq!(restored["model"], "top/selected");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn opencode_cleanup_active_provider_restores_previous_default() {
        let root = temp_root("opencode-cleanup-active");
        let paths = test_paths(&root, false);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            &paths.config,
            r#"{"model":"user-relay/old","provider":{"user-relay":{"name":"User"}}}"#,
        )
        .unwrap();
        std::fs::write(&paths.auth, "{}").unwrap();
        let provider = native_provider(
            "Relay One",
            "Relay One",
            serde_json::json!({
                "npm": "@ai-sdk/openai-compatible",
                "name": "Relay One",
                "options": { "baseURL": "https://relay.example/v1" },
                "models": { "gpt-test": { "name": "GPT Test" } }
            }),
            serde_json::json!({ "type": "api", "key": "sk-test" }),
            "gpt-test",
        );
        let config = ExternalCliAgentConfig {
            providers: vec![provider],
            current_provider: "Relay One".to_string(),
            ..Default::default()
        };

        materialize_native_at("opencode", &config, &paths).unwrap();
        cleanup_native_at("opencode", "p-msoeiznl", None, Some("Relay One"), &paths).unwrap();
        let cleaned: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.config).unwrap()).unwrap();
        assert_eq!(cleaned["model"], "user-relay/old");
        assert!(cleaned["provider"].get("relay-one").is_none());
        let state = read_managed_state(&paths.state).unwrap();
        assert!(!state.defaults_managed);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pi_switches_managed_providers_without_overwriting_backup() {
        let root = temp_root("pi-native");
        let paths = test_paths(&root, true);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            &paths.config,
            r#"{"providers":{"user":{"name":"User","models":[]}}}"#,
        )
        .unwrap();
        std::fs::write(&paths.auth, "{}").unwrap();
        std::fs::write(
            paths.settings.as_ref().unwrap(),
            r#"{"theme":"light","defaultProvider":"user","defaultModel":"old","defaultThinkingLevel":"low"}"#,
        )
        .unwrap();
        let make = |id: &str, model: &str| {
            let mut provider = native_provider(
                id,
                id,
                serde_json::json!({
                    "name": id,
                    "baseUrl": "https://relay.example/v1",
                    "api": "openai-completions",
                    "models": [{ "id": model, "name": model, "reasoning": true }]
                }),
                serde_json::json!({ "type": "api_key", "key": format!("sk-{id}") }),
                model,
            );
            provider.default_reasoning = "high".to_string();
            provider
        };
        let mut config = ExternalCliAgentConfig {
            providers: vec![make("First", "m1"), make("Second", "m2")],
            current_provider: "First".to_string(),
            ..Default::default()
        };

        materialize_native_at("pi", &config, &paths).unwrap();
        config.current_provider = "Second".to_string();
        materialize_native_at("pi", &config, &paths).unwrap();
        let active: Value = serde_json::from_str(
            &std::fs::read_to_string(paths.settings.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(active["defaultProvider"], "second");
        assert_eq!(active["defaultModel"], "m2");
        assert_eq!(active["defaultThinkingLevel"], "high");
        assert_eq!(active["theme"], "light");

        config.current_provider.clear();
        materialize_native_at("pi", &config, &paths).unwrap();
        let restored: Value = serde_json::from_str(
            &std::fs::read_to_string(paths.settings.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(restored["defaultProvider"], "user");
        assert_eq!(restored["defaultModel"], "old");
        assert_eq!(restored["defaultThinkingLevel"], "low");
        let models: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.config).unwrap()).unwrap();
        assert!(models["providers"]["user"].is_object());
        assert!(models["providers"]["first"].is_object());
        assert!(models["providers"]["second"].is_object());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pi_cleanup_active_provider_restores_previous_default() {
        let root = temp_root("pi-cleanup-active");
        let paths = test_paths(&root, true);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            &paths.config,
            r#"{"providers":{"user":{"name":"User","models":[]}}}"#,
        )
        .unwrap();
        std::fs::write(&paths.auth, "{}").unwrap();
        std::fs::write(
            paths.settings.as_ref().unwrap(),
            r#"{"defaultProvider":"user","defaultModel":"old"}"#,
        )
        .unwrap();
        let mut provider = native_provider(
            "Relay One",
            "Relay One",
            serde_json::json!({
                "name": "Relay One",
                "baseUrl": "https://relay.example/v1",
                "api": "openai-completions",
                "models": [{ "id": "gpt-test", "name": "GPT Test", "reasoning": true }]
            }),
            serde_json::json!({ "type": "api_key", "key": "sk-test" }),
            "gpt-test",
        );
        provider.default_reasoning = "high".to_string();
        let config = ExternalCliAgentConfig {
            providers: vec![provider],
            current_provider: "Relay One".to_string(),
            ..Default::default()
        };

        materialize_native_at("pi", &config, &paths).unwrap();
        cleanup_native_at("pi", "p-msoeiznl", None, Some("Relay One"), &paths).unwrap();
        let settings: Value = serde_json::from_str(
            &std::fs::read_to_string(paths.settings.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(settings["defaultProvider"], "user");
        assert_eq!(settings["defaultModel"], "old");
        let models: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.config).unwrap()).unwrap();
        assert!(models["providers"].get("relay-one").is_none());
        let state = read_managed_state(&paths.state).unwrap();
        assert!(!state.defaults_managed);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cleanup_removes_legacy_kivio_alias_when_deleting_by_internal_id() {
        let root = temp_root("pi-cleanup-legacy");
        let paths = test_paths(&root, true);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            &paths.config,
            r#"{"providers":{"relay-one":{"name":"Relay"},"kivio-p-msoeiznl":{"name":"Legacy"}}}"#,
        )
        .unwrap();
        std::fs::write(
            &paths.auth,
            r#"{"relay-one":{"type":"api_key","key":"new"},"kivio-p-msoeiznl":{"type":"api_key","key":"old"}}"#,
        )
        .unwrap();
        std::fs::write(
            paths.settings.as_ref().unwrap(),
            r#"{"defaultProvider":"relay-one"}"#,
        )
        .unwrap();
        std::fs::write(
            &paths.state,
            r#"{"managedProviderIds":["relay-one","kivio-p-msoeiznl"],"defaultsManaged":false}"#,
        )
        .unwrap();

        cleanup_native_at("pi", "p-msoeiznl", None, Some("Relay One"), &paths).unwrap();
        let models: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.config).unwrap()).unwrap();
        assert!(models["providers"].get("relay-one").is_none());
        assert!(models["providers"].get("kivio-p-msoeiznl").is_none());
        let auth: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.auth).unwrap()).unwrap();
        assert!(auth.get("relay-one").is_none());
        assert!(auth.get("kivio-p-msoeiznl").is_none());
        let state = read_managed_state(&paths.state).unwrap();
        assert!(state.managed_provider_ids.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pi_upgrade_backs_up_existing_thinking_level_before_managing_it() {
        let root = temp_root("pi-thinking-upgrade");
        let paths = test_paths(&root, true);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&paths.config, r#"{"providers":{}}"#).unwrap();
        std::fs::write(&paths.auth, "{}").unwrap();
        std::fs::write(
            paths.settings.as_ref().unwrap(),
            r#"{"defaultProvider":"kivio-relay","defaultModel":"gpt-test","defaultThinkingLevel":"medium"}"#,
        )
        .unwrap();
        std::fs::write(
            &paths.state,
            r#"{
              "managedProviderIds": ["kivio-relay"],
              "defaultsManaged": true,
              "previousDefaults": {
                "defaultProvider": {"present": true, "value": "user"},
                "defaultModel": {"present": true, "value": "old"}
              }
            }"#,
        )
        .unwrap();
        let mut provider = native_provider(
            "Relay",
            "Relay",
            serde_json::json!({
                "name": "Relay",
                "baseUrl": "https://relay.example/v1",
                "api": "openai-responses",
                "models": [{ "id": "gpt-test", "reasoning": true }]
            }),
            serde_json::json!({ "type": "api_key", "key": "sk-test" }),
            "gpt-test",
        );
        provider.default_reasoning = "high".to_string();
        let mut config = ExternalCliAgentConfig {
            providers: vec![provider],
            current_provider: "Relay".to_string(),
            ..Default::default()
        };

        materialize_native_at("pi", &config, &paths).unwrap();
        let managed: Value = serde_json::from_str(
            &std::fs::read_to_string(paths.settings.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(managed["defaultThinkingLevel"], "high");

        config.current_provider.clear();
        materialize_native_at("pi", &config, &paths).unwrap();
        let restored: Value = serde_json::from_str(
            &std::fs::read_to_string(paths.settings.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(restored["defaultProvider"], "user");
        assert_eq!(restored["defaultModel"], "old");
        assert_eq!(restored["defaultThinkingLevel"], "medium");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pi_validates_default_thinking_level_against_model_mapping() {
        let make = |reasoning: bool, mapping: Value, level: &str| {
            let mut provider = native_provider(
                "Relay",
                "Relay",
                serde_json::json!({
                    "name": "Relay",
                    "baseUrl": "https://relay.example/v1",
                    "api": "openai-responses",
                    "models": [{
                        "id": "model",
                        "reasoning": reasoning,
                        "thinkingLevelMap": mapping
                    }]
                }),
                serde_json::json!({ "type": "api_key", "key": "sk-test" }),
                "model",
            );
            provider.default_reasoning = level.to_string();
            provider
        };

        for level in ["off", "minimal", "low", "medium", "high"] {
            assert!(
                parse_native_entries("pi", &[make(true, serde_json::json!({}), level)]).is_ok()
            );
        }
        for level in ["xhigh", "max"] {
            assert!(parse_native_entries(
                "pi",
                &[make(true, serde_json::json!({ level: level }), level)]
            )
            .is_ok());
        }

        assert!(parse_native_entries(
            "pi",
            &[make(
                true,
                serde_json::json!({ "low": null, "xhigh": null }),
                "low"
            )]
        )
        .is_err());
        assert!(parse_native_entries("pi", &[make(true, serde_json::json!({}), "xhigh")]).is_err());
        assert!(parse_native_entries(
            "pi",
            &[make(false, serde_json::json!({ "high": "high" }), "high")]
        )
        .is_err());
        assert!(parse_native_entries(
            "pi",
            &[make(false, serde_json::json!({ "off": null }), "off")]
        )
        .is_ok());
    }

    #[test]
    fn pi_legacy_active_provider_is_unmanaged_but_missing_active_is_rejected() {
        let root = temp_root("pi-legacy-active");
        let paths = test_paths(&root, true);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&paths.config, r#"{"providers":{}}"#).unwrap();
        std::fs::write(&paths.auth, "{}").unwrap();
        std::fs::write(
            paths.settings.as_ref().unwrap(),
            r#"{"defaultProvider":"user","defaultModel":"old"}"#,
        )
        .unwrap();
        let legacy = ExternalCliProvider {
            id: "legacy".to_string(),
            name: "Legacy".to_string(),
            ..Default::default()
        };
        let mut config = ExternalCliAgentConfig {
            providers: vec![legacy],
            current_provider: "legacy".to_string(),
            ..Default::default()
        };

        materialize_native_at("pi", &config, &paths).unwrap();
        let settings: Value = serde_json::from_str(
            &std::fs::read_to_string(paths.settings.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(settings["defaultProvider"], "user");
        assert_eq!(settings["defaultModel"], "old");

        config.current_provider = "missing".to_string();
        let error = materialize_native_at("pi", &config, &paths).unwrap_err();
        assert!(error.contains("不存在"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_opencode_root_is_rejected_without_overwrite() {
        let root = temp_root("opencode-malformed");
        let paths = test_paths(&root, false);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&paths.config, "[1, 2, 3]").unwrap();
        let config = ExternalCliAgentConfig {
            providers: vec![native_provider(
                "relay",
                "Relay",
                serde_json::json!({ "models": { "m": { "name": "M" } } }),
                serde_json::json!({ "type": "api", "key": "sk" }),
                "m",
            )],
            current_provider: "relay".to_string(),
            ..Default::default()
        };
        assert!(materialize_native_at("opencode", &config, &paths).is_err());
        assert_eq!(std::fs::read_to_string(&paths.config).unwrap(), "[1, 2, 3]");
        let _ = std::fs::remove_dir_all(root);
    }

    fn grok_provider(id: &str, name: &str, config_toml: &str) -> ExternalCliProvider {
        ExternalCliProvider {
            id: id.to_string(),
            name: name.to_string(),
            config_toml: config_toml.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn grok_merge_preserves_marketplace_and_sets_default_model() {
        let base = r#"[models]
default = "old"

[marketplace]
official_marketplace_auto_installed = true

[ui]
yolo = false
"#;
        let provider = r#"[models]
default = "grok-4.5"

[model."grok-4.5"]
model = "grok-4.5"
base_url = "https://relay.example/v1"
api_key = "sk-x"
api_backend = "responses"
context_window = 500000
"#;
        let merged = merge_grok_provider_config(base, provider).unwrap();
        let doc: toml::Table = toml::from_str(&merged).unwrap();
        assert_eq!(
            doc.get("models")
                .and_then(|v| v.get("default"))
                .and_then(|v| v.as_str()),
            Some("grok-4.5")
        );
        assert!(doc
            .get("marketplace")
            .and_then(|v| v.get("official_marketplace_auto_installed"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false));
        let model = doc
            .get("model")
            .and_then(|v| v.get("grok-4.5"))
            .and_then(|v| v.as_table())
            .expect("model entry");
        assert_eq!(
            model.get("base_url").and_then(|v| v.as_str()),
            Some("https://relay.example/v1")
        );
        assert_eq!(model.get("api_key").and_then(|v| v.as_str()), Some("sk-x"));
    }

    #[test]
    fn grok_materialize_restores_previous_config_on_clear() {
        let root = temp_root("grok-restore");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("config.toml");
        let state_path = root.join("state.json");
        let original = "[models]\ndefault = \"native\"\n\n[ui]\nyolo = true\n";
        std::fs::write(&path, original).unwrap();

        let mut config = ExternalCliAgentConfig {
            providers: vec![grok_provider(
                "relay",
                "Relay",
                r#"[models]
default = "relay-model"

[model."relay-model"]
model = "relay-model"
base_url = "https://relay.example/v1"
api_key = "sk"
"#,
            )],
            current_provider: "relay".to_string(),
            ..Default::default()
        };
        materialize_grok_at(&config, &path, &state_path).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("relay-model"));
        assert!(after.contains("yolo = true"));

        config.current_provider.clear();
        materialize_grok_at(&config, &path, &state_path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn grok_switch_providers_does_not_accumulate_old_model_keys() {
        let root = temp_root("grok-switch");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("config.toml");
        let state_path = root.join("state.json");
        std::fs::write(&path, "[cli]\nauto_update = true\n").unwrap();

        let mut config = ExternalCliAgentConfig {
            providers: vec![
                grok_provider(
                    "a",
                    "A",
                    r#"[models]
default = "model-a"

[model."model-a"]
model = "model-a"
base_url = "https://a.example/v1"
api_key = "ska"
"#,
                ),
                grok_provider(
                    "b",
                    "B",
                    r#"[models]
default = "model-b"

[model."model-b"]
model = "model-b"
base_url = "https://b.example/v1"
api_key = "skb"
"#,
                ),
            ],
            current_provider: "a".to_string(),
            ..Default::default()
        };
        materialize_grok_at(&config, &path, &state_path).unwrap();
        config.current_provider = "b".to_string();
        materialize_grok_at(&config, &path, &state_path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("model-b"));
        assert!(!text.contains("model-a"));
        assert!(text.contains("auto_update = true"));
        let _ = std::fs::remove_dir_all(root);
    }

    fn kimi_provider(id: &str, name: &str, config_toml: &str) -> ExternalCliProvider {
        ExternalCliProvider {
            id: id.to_string(),
            name: name.to_string(),
            config_toml: config_toml.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn kimi_merge_preserves_managed_oauth_and_sets_default_model() {
        let base = r#"default_model = "kimi-code/k3"

[thinking]
enabled = true

[providers."managed:kimi-code"]
type = "kimi"
base_url = "https://api.kimi.com/coding/v1"
api_key = ""

[models."kimi-code/k3"]
provider = "managed:kimi-code"
model = "k3"
max_context_size = 1048576
"#;
        let provider = r#"default_model = "relay/gpt-5"

[providers.relay]
type = "openai"
base_url = "https://relay.example/v1"
api_key = "sk-x"

[models."relay/gpt-5"]
provider = "relay"
model = "gpt-5"
max_context_size = 128000
display_name = "GPT 5"
"#;
        let merged = merge_kimi_provider_config(base, provider).unwrap();
        let doc: toml::Table = toml::from_str(&merged).unwrap();
        assert_eq!(
            doc.get("default_model").and_then(|v| v.as_str()),
            Some("relay/gpt-5")
        );
        assert!(doc
            .get("thinking")
            .and_then(|v| v.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false));
        let managed = doc
            .get("providers")
            .and_then(|v| v.get("managed:kimi-code"))
            .and_then(|v| v.as_table())
            .expect("managed provider kept");
        assert_eq!(managed.get("type").and_then(|v| v.as_str()), Some("kimi"));
        let relay = doc
            .get("providers")
            .and_then(|v| v.get("relay"))
            .and_then(|v| v.as_table())
            .expect("relay provider");
        assert_eq!(
            relay.get("base_url").and_then(|v| v.as_str()),
            Some("https://relay.example/v1")
        );
        let model = doc
            .get("models")
            .and_then(|v| v.get("relay/gpt-5"))
            .and_then(|v| v.as_table())
            .expect("model entry");
        assert_eq!(model.get("model").and_then(|v| v.as_str()), Some("gpt-5"));
        assert_eq!(
            model.get("max_context_size").and_then(|v| v.as_integer()),
            Some(128000)
        );
    }

    #[test]
    fn kimi_materialize_restores_previous_config_on_clear() {
        let root = temp_root("kimi-restore");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("config.toml");
        let state_path = root.join("state.json");
        let original = "default_model = \"kimi-code/k3\"\n\n[thinking]\nenabled = true\n";
        std::fs::write(&path, original).unwrap();

        let mut config = ExternalCliAgentConfig {
            providers: vec![kimi_provider(
                "relay",
                "Relay",
                r#"default_model = "relay/m1"

[providers.relay]
type = "openai"
base_url = "https://relay.example/v1"
api_key = "sk"

[models."relay/m1"]
provider = "relay"
model = "m1"
max_context_size = 128000
"#,
            )],
            current_provider: "relay".to_string(),
            ..Default::default()
        };
        materialize_kimi_at(&config, &path, &state_path).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("relay/m1"));
        assert!(after.contains("enabled = true"));

        config.current_provider.clear();
        materialize_kimi_at(&config, &path, &state_path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn kimi_switch_providers_starts_from_backup_not_previous_relay() {
        let root = temp_root("kimi-switch");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("config.toml");
        let state_path = root.join("state.json");
        std::fs::write(
            &path,
            "[providers.\"managed:kimi-code\"]\ntype = \"kimi\"\n",
        )
        .unwrap();

        let mut config = ExternalCliAgentConfig {
            providers: vec![
                kimi_provider(
                    "a",
                    "A",
                    r#"default_model = "a/m-a"

[providers.a]
type = "openai"
base_url = "https://a.example/v1"
api_key = "ska"

[models."a/m-a"]
provider = "a"
model = "m-a"
max_context_size = 128000
"#,
                ),
                kimi_provider(
                    "b",
                    "B",
                    r#"default_model = "b/m-b"

[providers.b]
type = "openai_responses"
base_url = "https://b.example/v1"
api_key = "skb"

[models."b/m-b"]
provider = "b"
model = "m-b"
max_context_size = 200000
"#,
                ),
            ],
            current_provider: "a".to_string(),
            ..Default::default()
        };
        materialize_kimi_at(&config, &path, &state_path).unwrap();
        config.current_provider = "b".to_string();
        materialize_kimi_at(&config, &path, &state_path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("m-b"));
        assert!(text.contains("openai_responses"));
        // 从备份合并：managed OAuth 还在；A 的 provider 不应残留
        assert!(text.contains("managed:kimi-code"));
        assert!(!text.contains("[providers.a]") && !text.contains("providers.a"));
        // toml pretty 可能写成 [providers.a] 或 nested table — 用解析确认
        let doc: toml::Table = toml::from_str(&text).unwrap();
        let providers = doc.get("providers").and_then(|v| v.as_table()).unwrap();
        assert!(providers.get("a").is_none());
        assert!(providers.get("b").is_some());
        assert!(providers.get("managed:kimi-code").is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pi_adaptive_thinking_model_detection() {
        for id in [
            "claude-opus-4-8",
            "claude-opus-4-6",
            "claude-opus-4.7",
            "claude-sonnet-5",
            "claude-fable-5",
            "claude-opus-4-6[1M]",
            "claude-sonnet-4-6-20250929",
            "claude-opus-5",
        ] {
            assert!(is_pi_adaptive_thinking_model(id), "expected adaptive: {id}");
        }
        for id in [
            "claude-sonnet-4-5",
            "claude-opus-4-1",
            "claude-3-5-sonnet",
            "gpt-5",
        ] {
            assert!(
                !is_pi_adaptive_thinking_model(id),
                "expected not adaptive: {id}"
            );
        }
    }

    #[test]
    fn ensure_pi_adaptive_thinking_compat_patches_anthropic_models() {
        let mut config = serde_json::json!({
            "api": "anthropic-messages",
            "baseUrl": "https://relay.example",
            "models": [
                { "id": "claude-opus-4-8", "reasoning": true },
                { "id": "claude-sonnet-4-5", "reasoning": true },
                { "id": "claude-fable-5", "reasoning": true, "compat": { "allowEmptySignature": true } }
            ]
        })
        .as_object()
        .cloned()
        .unwrap();
        ensure_pi_adaptive_thinking_compat(&mut config);
        let models = config.get("models").and_then(Value::as_array).unwrap();
        assert_eq!(
            models[0]
                .get("compat")
                .and_then(|c| c.get("forceAdaptiveThinking"))
                .and_then(Value::as_bool),
            Some(true)
        );
        assert!(models[1].get("compat").is_none());
        // 已有其它 compat 键时只补 forceAdaptiveThinking，不抹掉原字段。
        assert_eq!(
            models[2]
                .get("compat")
                .and_then(|c| c.get("forceAdaptiveThinking"))
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            models[2]
                .get("compat")
                .and_then(|c| c.get("allowEmptySignature"))
                .and_then(Value::as_bool),
            Some(true)
        );

        // OpenAI 线不动。
        let mut openai = serde_json::json!({
            "api": "openai-completions",
            "models": [{ "id": "claude-opus-4-8" }]
        })
        .as_object()
        .cloned()
        .unwrap();
        ensure_pi_adaptive_thinking_compat(&mut openai);
        assert!(openai.get("models").and_then(Value::as_array).unwrap()[0]
            .get("compat")
            .is_none());
    }
}
