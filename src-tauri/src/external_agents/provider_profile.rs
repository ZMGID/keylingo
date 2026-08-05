//! 第三方供应商（中转站）的落地层：把设置页里选中的供应商变成**子进程能看见的东西**。
//!
//! Claude / Codex 继续使用 Kivio 私有配置；OpenCode / Pi 按各自官方约定写入原生全局配置：
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
    Some(NativePaths {
        config: config_home.join("opencode/opencode.json"),
        auth: data_home.join("opencode/auth.json"),
        settings: None,
        state: profiles_dir()?.join("opencode-native-state.json"),
    })
}

fn pi_paths() -> Option<NativePaths> {
    let base = directories::BaseDirs::new()?;
    let agent = nonempty_env_path("PI_CODING_AGENT_DIR")
        .unwrap_or_else(|| base.home_dir().join(".pi/agent"));
    Some(NativePaths {
        config: agent.join("models.json"),
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

/// 把供应商写到盘上。OpenCode / Pi 即使当前未启用，也要同步已保存列表并恢复原生默认值。
pub fn materialize(agent_id: &str) -> Result<(), String> {
    if matches!(agent_id, "opencode" | "pi") {
        let config = super::overrides::agent_config(agent_id).unwrap_or_default();
        return materialize_native(agent_id, &config);
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

#[derive(Debug, Clone)]
struct NativeProviderEntry {
    source_id: String,
    native_id: String,
    config: Value,
    auth: Value,
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
            let mut root = read_object_file(&paths.config, true, false, "OpenCode opencode.json")?;
            let before_root = root.clone();
            sync_provider_map(&mut root, "provider", &previous_ids, &entries)?;

            let mut auth = read_object_file(&paths.auth, false, true, "OpenCode auth.json")?;
            let before_auth = auth.clone();
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
            sync_provider_map(&mut models, "providers", &previous_ids, &entries)?;

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
        let native_id = native_provider_id(&provider.id)
            .ok_or_else(|| format!("供应商 id 无法生成原生 provider id：{}", provider.id))?;
        if !native_ids.insert(native_id.clone()) {
            return Err(format!("供应商 id 归一化后冲突：{native_id}"));
        }
        let config = parse_object_text(&provider.config_json, "provider configJson")?;
        let auth = parse_object_text(&provider.auth_json, "provider authJson")?;
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
            auth: Value::Object(auth),
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
            if !nonempty(config.get("npm"))
                || !config
                    .get("options")
                    .and_then(Value::as_object)
                    .is_some_and(|options| nonempty(options.get("baseURL")))
            {
                return Err(format!(
                    "供应商 {name} 的 OpenCode 配置缺少 npm 或 options.baseURL"
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

fn native_provider_id(id: &str) -> Option<String> {
    let mut slug = String::new();
    let mut last_hyphen = false;
    for ch in id.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_hyphen = false;
        } else if !last_hyphen && !slug.is_empty() {
            slug.push('-');
            last_hyphen = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    (!slug.is_empty()).then(|| format!("kivio-{slug}"))
}

fn sync_provider_map(
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
    for id in previous_ids {
        providers.remove(id);
    }
    for entry in entries {
        providers.insert(entry.native_id.clone(), entry.config.clone());
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
        auth.insert(entry.native_id.clone(), entry.auth.clone());
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
        "opencode" | "pi" => {
            if let Err(err) = cleanup_native(agent_id, provider_id) {
                eprintln!("[external-agent] 清理 {agent_id} 原生供应商失败：{err}");
            }
        }
        _ => {}
    }
}

fn cleanup_native(agent_id: &str, provider_id: &str) -> Result<(), String> {
    let paths = match agent_id {
        "opencode" => opencode_paths(),
        "pi" => pi_paths(),
        _ => None,
    }
    .ok_or_else(|| format!("无法定位 {agent_id} 的原生配置目录"))?;
    let _guard = NATIVE_CONFIG_LOCK
        .lock()
        .map_err(|_| "原生 CLI 配置写锁已损坏".to_string())?;
    cleanup_native_at(agent_id, provider_id, &paths)
}

fn cleanup_native_at(agent_id: &str, provider_id: &str, paths: &NativePaths) -> Result<(), String> {
    let Some(native_id) = native_provider_id(provider_id) else {
        return Ok(());
    };
    let mut state = read_managed_state(&paths.state)?;
    match agent_id {
        "opencode" => {
            let mut config = read_object_file(&paths.config, true, false, "原生 provider 配置")?;
            let before_config = config.clone();
            if state.defaults_managed
                && config
                    .get("model")
                    .and_then(Value::as_str)
                    .and_then(|model| model.split_once('/'))
                    .is_some_and(|(provider, _)| provider == native_id)
            {
                apply_default_fields(&mut config, &mut state, &["model"], None);
            }
            if let Some(providers) = config.get_mut("provider").and_then(Value::as_object_mut) {
                providers.remove(&native_id);
            }
            write_object_if_changed(&paths.config, before_config, &config)?;
        }
        "pi" => {
            let mut config = read_object_file(&paths.config, false, false, "原生 provider 配置")?;
            let before_config = config.clone();
            if let Some(providers) = config.get_mut("providers").and_then(Value::as_object_mut) {
                providers.remove(&native_id);
            }
            write_object_if_changed(&paths.config, before_config, &config)?;

            if let Some(settings_path) = paths.settings.as_ref() {
                let mut settings =
                    read_object_file(settings_path, false, false, "Pi settings.json")?;
                let before_settings = settings.clone();
                if state.defaults_managed
                    && settings.get("defaultProvider").and_then(Value::as_str)
                        == Some(native_id.as_str())
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
    auth.remove(&native_id);
    write_object_if_changed(&paths.auth, before_auth, &auth)?;
    state.managed_provider_ids.retain(|id| id != &native_id);
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
fn write_private_atomic(path: &Path, content: &str) -> Result<(), String> {
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
            written["provider"]["kivio-relay-one"]["models"]["gpt-test"]["name"],
            "GPT Test"
        );
        assert_eq!(written["model"], "kivio-relay-one/gpt-test");
        let auth: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.auth).unwrap()).unwrap();
        assert_eq!(auth["user-relay"]["key"], "keep");
        assert_eq!(auth["kivio-relay-one"]["key"], "sk-test");

        config.current_provider.clear();
        materialize_native_at("opencode", &config, &paths).unwrap();
        let restored: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.config).unwrap()).unwrap();
        assert_eq!(restored["model"], "anthropic/claude-old");
        assert!(restored["provider"]["kivio-relay-one"].is_object());

        cleanup_native_at("opencode", "Relay One", &paths).unwrap();
        let cleaned: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.config).unwrap()).unwrap();
        assert!(cleaned["provider"].get("kivio-relay-one").is_none());
        assert!(cleaned["provider"]["user-relay"].is_object());
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
        cleanup_native_at("opencode", "Relay One", &paths).unwrap();
        let cleaned: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.config).unwrap()).unwrap();
        assert_eq!(cleaned["model"], "user-relay/old");
        assert!(cleaned["provider"].get("kivio-relay-one").is_none());
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
        assert_eq!(active["defaultProvider"], "kivio-second");
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
        assert!(models["providers"]["kivio-first"].is_object());
        assert!(models["providers"]["kivio-second"].is_object());
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
        cleanup_native_at("pi", "Relay One", &paths).unwrap();
        let settings: Value = serde_json::from_str(
            &std::fs::read_to_string(paths.settings.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(settings["defaultProvider"], "user");
        assert_eq!(settings["defaultModel"], "old");
        let models: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.config).unwrap()).unwrap();
        assert!(models["providers"].get("kivio-relay-one").is_none());
        let state = read_managed_state(&paths.state).unwrap();
        assert!(!state.defaults_managed);
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
}
