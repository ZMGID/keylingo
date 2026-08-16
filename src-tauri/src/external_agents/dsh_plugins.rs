//! dsh 官方设置页「插件」两栏的本地适配，以及官方 DeepSeek 密钥。
//!
//! 官方 web 把可配项写进 `$DSH_HOME/settings.yaml` 的三个 namespace
//!（`shell` / `agent-loop` / `web-search-deepseek`），密钥进 `.credentials.yaml`
//!（官方 DeepSeek 模型与网页搜索共用 `DEEPSEEK_API_KEY`），
//! 清单则来自已 compose 的 Loader 树。Kivio 不改 `profiles/kivio/cordis.patch.yml`
//!（那份由 `dsh_profile::ensure_profile_ready` 每轮重写），所以：
//!
//! - **官方密钥**：写入 `.credentials.yaml` 的 `DEEPSEEK_API_KEY`，与官方网页版首次
//!   填密钥同一条路径；不进 `cordis.patch.yml`。就绪判定对齐官方分层：进程环境 >
//!   `.credentials.yaml` > 项目 `.env` > `$DSH_HOME/.env`。
//! - **插件配置**：读写 `settings.yaml` + 可选写凭据，热重载后 kivio / web 共用。
//! - **插件列表**：优先 `dsh --profile kivio --dump-config`（boot-free）解析
//!   id / name / disabled；失败则读安装包里的 `dsh-base` / `dsh-agent-presets`
//!   patch + Kivio 自己的 `cordis.patch.yml`。没有 live host，fiber 相位一律未知；
//!   启用态以 compose 后的 `disabled` 为准。
//! - **settings.yaml 里的第三方供应商**：设置页「所有供应商」会列出
//!   `llm-pi-ai.providers` 摘要。用户点删除时才从这份文件移除该条——这不是把
//!   Kivio 管理的供应商同步进 web，只处理页面上已经显示的那一行。
//! - **模型图片 / 推理档位**：官方 web 只认 `settings.yaml` 里的 `input` /
//!   `defaultInput` / `reasoningEfforts`。Kivio 保存供应商时，只把已存在路由上的
//!   这几项写回去，不新建供应商、不写密钥。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_yaml::Mapping;
use tauri::AppHandle;
use tauri_plugin_shell::ShellExt;
use tokio::time::timeout;

use crate::external_agents::dsh_profile::{profile_dir, KIVIO_PROFILE};
use crate::external_agents::registry::get_agent_def;
use crate::external_agents::spawn::{agent_cli_command, resolve_binary};
use crate::proc::NoConsoleWindow;
use crate::settings::ExternalCliProvider;

const SHELL_NS: &str = "shell";
const AGENT_LOOP_NS: &str = "agent-loop";
const WEB_SEARCH_NS: &str = "web-search-deepseek";
const LLM_PI_AI_NS: &str = "llm-pi-ai";
const CREDENTIALS_FILENAME: &str = ".credentials.yaml";
const DOTENV_FILENAME: &str = ".env";
const DEFAULT_API_KEY_ENV: &str = "DEEPSEEK_API_KEY";
const DSH_OFFICIAL_PROVIDER_ID: &str = "deepseek-official";

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_MAX_OUTPUT_BYTES: u64 = 64_000;
const DEFAULT_MAX_PARALLEL: u64 = 10;
const DEFAULT_MAX_USES: u64 = 5;
const DEFAULT_SEARCH_BASE_URL: &str = "https://api.deepseek.com/anthropic/v1";

const DUMP_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshPluginSettingsSnapshot {
    pub settings_path: String,
    pub shell: DshShellSettings,
    pub agent_loop: DshAgentLoopSettings,
    pub web_search: DshWebSearchSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshShellSettings {
    pub timeout_ms: Option<u64>,
    pub max_output_bytes: Option<u64>,
    pub timeout_ms_default: u64,
    pub max_output_bytes_default: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshAgentLoopSettings {
    pub max_parallel_tool_calls: Option<u64>,
    pub max_parallel_tool_calls_default: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshWebSearchSettings {
    pub base_url: Option<String>,
    pub max_uses: Option<u64>,
    pub api_key_env: String,
    pub api_key_configured: bool,
    pub api_key_writable: bool,
    pub base_url_default: String,
    pub max_uses_default: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshPluginSettingsPatch {
    pub shell: Option<DshShellPatch>,
    pub agent_loop: Option<DshAgentLoopPatch>,
    pub web_search: Option<DshWebSearchPatch>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshShellPatch {
    /// `Some(None)` = 恢复默认（从 yaml 删掉该键）。
    pub timeout_ms: Option<Option<u64>>,
    pub max_output_bytes: Option<Option<u64>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshAgentLoopPatch {
    pub max_parallel_tool_calls: Option<Option<u64>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshWebSearchPatch {
    pub base_url: Option<Option<String>>,
    pub max_uses: Option<Option<u64>>,
    /// 非空才写入 `.credentials.yaml`；空字符串 = 保持现有密钥。
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DshPluginEntry {
    pub id: String,
    pub module_name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshOfficialCredential {
    pub configured: bool,
    pub writable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DshNativeProviderModel {
    pub id: String,
    pub name: String,
}

/// `settings.yaml` 里一条 `llm-pi-ai` 供应商的编辑稿。探测摘要不带密钥；
/// 这个命令只在用户点「修改」时才回读 `.credentials.yaml`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DshNativeProviderDetail {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api: String,
    pub api_key: String,
    pub api_key_env: String,
    pub models: Vec<DshNativeProviderModel>,
    pub default_model: String,
}

fn dsh_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("DSH_HOME") {
        let path = PathBuf::from(home);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    directories::BaseDirs::new().map(|base| base.home_dir().join(".dsh"))
}

fn settings_path() -> Result<PathBuf, String> {
    dsh_home()
        .map(|home| home.join("settings.yaml"))
        .ok_or_else(|| "无法定位 dsh 配置目录".to_string())
}

fn credentials_path() -> Result<PathBuf, String> {
    dsh_home()
        .map(|home| home.join(CREDENTIALS_FILENAME))
        .ok_or_else(|| "无法定位 dsh 配置目录".to_string())
}

fn mapping_u64(map: &Mapping, key: &str) -> Option<u64> {
    let value = map.get(serde_yaml::Value::String(key.to_string()))?;
    match value {
        serde_yaml::Value::Number(n) => n.as_u64().or_else(|| n.as_i64().and_then(|v| u64::try_from(v).ok())),
        serde_yaml::Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn mapping_string(map: &Mapping, key: &str) -> Option<String> {
    map.get(serde_yaml::Value::String(key.to_string()))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn namespace_map<'a>(root: &'a Mapping, key: &str) -> Option<&'a Mapping> {
    root.get(serde_yaml::Value::String(key.to_string()))
        .and_then(serde_yaml::Value::as_mapping)
}

fn credential_status(api_key_env: &str) -> (bool, bool) {
    credential_status_with_files(
        api_key_env,
        credentials_path().ok().as_deref(),
        user_dotenv_path().as_deref(),
        None,
    )
}

fn credential_status_with_files(
    api_key_env: &str,
    credentials: Option<&Path>,
    user_env: Option<&Path>,
    project_env: Option<&Path>,
) -> (bool, bool) {
    if std::env::var_os(api_key_env).is_some_and(|value| !value.is_empty()) {
        return (true, false);
    }
    if credentials.is_some_and(|path| credentials_contain(path, api_key_env)) {
        return (true, true);
    }
    // Official layering: managed yaml, then project `.env`, then `$DSH_HOME/.env`.
    let configured = project_env.is_some_and(|path| dotenv_contains(path, api_key_env))
        || user_env.is_some_and(|path| dotenv_contains(path, api_key_env));
    (configured, true)
}

fn user_dotenv_path() -> Option<PathBuf> {
    dsh_home().map(|home| home.join(DOTENV_FILENAME))
}

fn credentials_contain(path: &Path, api_key_env: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(&text) else {
        return false;
    };
    value
        .as_mapping()
        .and_then(|map| mapping_string(map, api_key_env))
        .is_some()
}

fn dotenv_contains(path: &Path, api_key_env: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    dotenv_has_key(&text, api_key_env)
}

fn dotenv_has_key(text: &str, api_key_env: &str) -> bool {
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line
            .strip_prefix("export ")
            .map(str::trim)
            .unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != api_key_env {
            continue;
        }
        let value = value.trim();
        let unquoted = if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            &value[1..value.len() - 1]
        } else {
            value
        };
        if !unquoted.is_empty() {
            return true;
        }
    }
    false
}

fn pi_ai_api_key_env_from_settings(text: &str, provider_id: &str) -> Option<String> {
    let value = serde_yaml::from_str::<serde_yaml::Value>(text).ok()?;
    namespace_map(value.as_mapping()?, LLM_PI_AI_NS)
        .and_then(|pi| namespace_map(pi, "providers"))
        .and_then(|providers| {
            providers
                .get(serde_yaml::Value::String(provider_id.to_string()))
                .and_then(serde_yaml::Value::as_mapping)
        })
        .and_then(|provider| mapping_string(provider, "apiKeyEnv"))
}

fn native_pi_ai_api_key_env(provider_id: &str) -> Option<String> {
    let path = settings_path().ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    pi_ai_api_key_env_from_settings(&text, provider_id)
}

/// Connect 前的凭据门：Kivio 托管供应商由调用方另行判断；这里覆盖官方 DeepSeek
/// 与 `settings.yaml` 里的 `llm-pi-ai` 路由（含 `.env` 兜底）。
pub(crate) fn credentials_ready_for_provider(provider: &str, cwd: Option<&Path>) -> bool {
    let api_key_env = if provider.trim().is_empty() || provider == DSH_OFFICIAL_PROVIDER_ID {
        DEFAULT_API_KEY_ENV.to_string()
    } else if let Some(env) = native_pi_ai_api_key_env(provider) {
        env
    } else {
        return false;
    };
    let project_env = cwd.map(|path| path.join(DOTENV_FILENAME));
    credential_status_with_files(
        &api_key_env,
        credentials_path().ok().as_deref(),
        user_dotenv_path().as_deref(),
        project_env.as_deref(),
    )
    .0
}

fn parse_settings_snapshot(text: &str, settings_path: &Path) -> Result<DshPluginSettingsSnapshot, String> {
    let root = if text.trim().is_empty() {
        Mapping::new()
    } else {
        match serde_yaml::from_str::<serde_yaml::Value>(text)
            .map_err(|err| format!("解析 dsh settings.yaml 失败：{err}"))?
        {
            serde_yaml::Value::Mapping(map) => map,
            serde_yaml::Value::Null => Mapping::new(),
            _ => return Err("dsh settings.yaml 必须是 namespace 映射".to_string()),
        }
    };
    let shell = namespace_map(&root, SHELL_NS);
    let agent_loop = namespace_map(&root, AGENT_LOOP_NS);
    let web_search = namespace_map(&root, WEB_SEARCH_NS);
    let api_key_env = web_search
        .and_then(|map| mapping_string(map, "apiKeyEnv"))
        .unwrap_or_else(|| DEFAULT_API_KEY_ENV.to_string());
    let (api_key_configured, api_key_writable) = credential_status(&api_key_env);
    Ok(DshPluginSettingsSnapshot {
        settings_path: settings_path.display().to_string(),
        shell: DshShellSettings {
            timeout_ms: shell.and_then(|map| mapping_u64(map, "timeoutMs")),
            max_output_bytes: shell.and_then(|map| mapping_u64(map, "maxOutputBytes")),
            timeout_ms_default: DEFAULT_TIMEOUT_MS,
            max_output_bytes_default: DEFAULT_MAX_OUTPUT_BYTES,
        },
        agent_loop: DshAgentLoopSettings {
            max_parallel_tool_calls: agent_loop.and_then(|map| mapping_u64(map, "maxParallelToolCalls")),
            max_parallel_tool_calls_default: DEFAULT_MAX_PARALLEL,
        },
        web_search: DshWebSearchSettings {
            base_url: web_search.and_then(|map| mapping_string(map, "baseURL")),
            max_uses: web_search.and_then(|map| mapping_u64(map, "maxUses")),
            api_key_env,
            api_key_configured,
            api_key_writable,
            base_url_default: DEFAULT_SEARCH_BASE_URL.to_string(),
            max_uses_default: DEFAULT_MAX_USES,
        },
    })
}

fn load_root_mapping(path: &Path) -> Result<Mapping, String> {
    if !path.exists() {
        return Ok(Mapping::new());
    }
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("读取 dsh settings.yaml 失败：{err}"))?;
    if text.trim().is_empty() {
        return Ok(Mapping::new());
    }
    match serde_yaml::from_str::<serde_yaml::Value>(&text)
        .map_err(|err| format!("解析 dsh settings.yaml 失败：{err}"))?
    {
        serde_yaml::Value::Mapping(map) => Ok(map),
        serde_yaml::Value::Null => Ok(Mapping::new()),
        _ => Err("dsh settings.yaml 必须是 namespace 映射".to_string()),
    }
}

fn set_optional_number(map: &mut Mapping, key: &str, value: Option<Option<u64>>) {
    let Some(next) = value else {
        return;
    };
    let yaml_key = serde_yaml::Value::String(key.to_string());
    match next {
        Some(number) => {
            map.insert(yaml_key, serde_yaml::Value::Number(number.into()));
        }
        None => {
            map.remove(&yaml_key);
        }
    }
}

fn set_optional_string(map: &mut Mapping, key: &str, value: Option<Option<String>>) {
    let Some(next) = value else {
        return;
    };
    let yaml_key = serde_yaml::Value::String(key.to_string());
    match next.and_then(|text| {
        let trimmed = text.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }) {
        Some(text) => {
            map.insert(yaml_key, serde_yaml::Value::String(text));
        }
        None => {
            map.remove(&yaml_key);
        }
    }
}

fn upsert_namespace(root: &mut Mapping, key: &str, fields: Mapping) {
    let yaml_key = serde_yaml::Value::String(key.to_string());
    if fields.is_empty() {
        root.remove(&yaml_key);
    } else {
        root.insert(yaml_key, serde_yaml::Value::Mapping(fields));
    }
}

fn apply_patch(mut root: Mapping, patch: &DshPluginSettingsPatch) -> Mapping {
    if let Some(shell) = &patch.shell {
        let mut fields = namespace_map(&root, SHELL_NS).cloned().unwrap_or_default();
        set_optional_number(&mut fields, "timeoutMs", shell.timeout_ms);
        set_optional_number(&mut fields, "maxOutputBytes", shell.max_output_bytes);
        upsert_namespace(&mut root, SHELL_NS, fields);
    }
    if let Some(agent_loop) = &patch.agent_loop {
        let mut fields = namespace_map(&root, AGENT_LOOP_NS)
            .cloned()
            .unwrap_or_default();
        set_optional_number(
            &mut fields,
            "maxParallelToolCalls",
            agent_loop.max_parallel_tool_calls,
        );
        upsert_namespace(&mut root, AGENT_LOOP_NS, fields);
    }
    if let Some(web_search) = &patch.web_search {
        let mut fields = namespace_map(&root, WEB_SEARCH_NS)
            .cloned()
            .unwrap_or_default();
        set_optional_string(&mut fields, "baseURL", web_search.base_url.clone());
        set_optional_number(&mut fields, "maxUses", web_search.max_uses);
        upsert_namespace(&mut root, WEB_SEARCH_NS, fields);
    }
    root
}

fn write_credential(api_key_env: &str, api_key: &str) -> Result<(), String> {
    let key = api_key.trim();
    if key.is_empty() {
        return Ok(());
    }
    if !std::env::var_os(api_key_env).is_none_or(|value| value.is_empty()) {
        return Err(format!("{api_key_env} 已由环境变量提供，不能写进凭据文件"));
    }
    write_credential_at(&credentials_path()?, api_key_env, key)
}

fn write_credential_at(path: &Path, api_key_env: &str, key: &str) -> Result<(), String> {
    let mut root = if path.exists() {
        let text = std::fs::read_to_string(path)
            .map_err(|err| format!("读取 dsh 凭据失败：{err}"))?;
        match serde_yaml::from_str::<serde_yaml::Value>(&text)
            .map_err(|err| format!("解析 dsh 凭据失败：{err}"))?
        {
            serde_yaml::Value::Mapping(map) => map,
            serde_yaml::Value::Null => Mapping::new(),
            _ => return Err("dsh .credentials.yaml 必须是映射".to_string()),
        }
    } else {
        Mapping::new()
    };
    root.insert(
        serde_yaml::Value::String(api_key_env.to_string()),
        serde_yaml::Value::String(key.to_string()),
    );
    let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(root))
        .map_err(|err| format!("序列化 dsh 凭据失败：{err}"))?;
    crate::external_agents::provider_profile::write_private_atomic(path, &yaml)
}

fn persist_settings(path: &Path, root: Mapping) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建 dsh 配置目录失败：{err}"))?;
    }
    let yaml = if root.is_empty() {
        String::new()
    } else {
        serde_yaml::to_string(&serde_yaml::Value::Mapping(root))
            .map_err(|err| format!("序列化 dsh settings.yaml 失败：{err}"))?
    };
    crate::chat::storage::atomic_write(path, &yaml, "dsh settings.yaml")
}

/// `!!js` 是 dump 方言，serde 不必求值；清掉后按字面 `disabled: true` 判断启用态。
fn neutralize_js_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if let Some(index) = line.find("!!js") {
            out.push_str(&line[..index]);
            out.push_str("null");
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

fn is_explicitly_disabled(map: &Mapping) -> bool {
    matches!(
        map.get(serde_yaml::Value::String("disabled".to_string())),
        Some(serde_yaml::Value::Bool(true))
    )
}

fn collect_entries(value: &serde_yaml::Value, out: &mut BTreeMap<String, DshPluginEntry>) {
    match value {
        serde_yaml::Value::Sequence(items) => {
            for item in items {
                collect_entries(item, out);
            }
        }
        serde_yaml::Value::Mapping(map) => {
            let id = mapping_string(map, "id");
            let module_name = mapping_string(map, "name");
            match (id, module_name) {
                (Some(id), Some(module_name)) => {
                    out.insert(
                        id.clone(),
                        DshPluginEntry {
                            id,
                            module_name,
                            enabled: !is_explicitly_disabled(map),
                        },
                    );
                }
                (Some(id), None) if is_explicitly_disabled(map) => {
                    if let Some(existing) = out.get_mut(&id) {
                        existing.enabled = false;
                    }
                }
                _ => {}
            }
            for key in ["config", "insert"] {
                if let Some(nested) = map.get(serde_yaml::Value::String(key.to_string())) {
                    collect_entries(nested, out);
                }
            }
        }
        _ => {}
    }
}

fn parse_plugin_manifest(
    text: &str,
    out: &mut BTreeMap<String, DshPluginEntry>,
) -> Result<(), String> {
    let neutralized = neutralize_js_tags(text);
    let value: serde_yaml::Value = serde_yaml::from_str(&neutralized)
        .map_err(|err| format!("解析插件清单失败：{err}"))?;
    collect_entries(&value, out);
    Ok(())
}

fn dump_chunks(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        if line.trim_start().starts_with("# ==") {
            if !current.trim().is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() && !text.trim().is_empty() {
        chunks.push(text.to_string());
    }
    chunks
}

fn parse_inventory_dump(text: &str) -> Result<Vec<DshPluginEntry>, String> {
    let neutralized = neutralize_js_tags(text);
    let mut entries = BTreeMap::new();
    for chunk in dump_chunks(&neutralized) {
        if let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(&chunk) {
            collect_entries(&value, &mut entries);
        }
    }
    if entries.is_empty() {
        let stripped: String = neutralized
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .flat_map(|line| [line, "\n"])
            .collect();
        if let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(&stripped) {
            collect_entries(&value, &mut entries);
        }
    }
    if entries.is_empty() {
        return Err("解析 dsh --dump-config 失败：没有识别到插件条目".to_string());
    }
    Ok(entries.into_values().collect())
}

fn find_bundle_patch(start: &Path, package: &str) -> Option<PathBuf> {
    let relative = package.trim_start_matches('@');
    let (scope, name) = relative.split_once('/')?;
    let scoped = PathBuf::from(format!("@{scope}"))
        .join(name)
        .join("cordis.patch.yml");
    let mut cursor = if start.is_file() {
        start.parent()
    } else {
        Some(start)
    };
    for _ in 0..8 {
        let dir = cursor?;
        let direct = dir.join("node_modules").join(&scoped);
        if direct.is_file() {
            return Some(direct);
        }
        let nested = dir
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh")
            .join("node_modules")
            .join(&scoped);
        if nested.is_file() {
            return Some(nested);
        }
        cursor = dir.parent();
    }
    None
}

fn merge_manifest_file(path: &Path, out: &mut BTreeMap<String, DshPluginEntry>) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("读取 {} 失败：{err}", path.display()))?;
    parse_plugin_manifest(&text, out)
}

async fn inventory_from_installed_patches() -> Result<Vec<DshPluginEntry>, String> {
    let def = get_agent_def("dsh").ok_or_else(|| "未注册 dsh".to_string())?;
    let bin = resolve_binary(def).await;
    let mut entries = BTreeMap::new();
    let mut found_base = false;
    if let Some(bin) = bin.as_deref() {
        if let Some(path) = find_bundle_patch(bin, "@deepseek-ai/dsh-base") {
            merge_manifest_file(&path, &mut entries)?;
            found_base = true;
        }
        if let Some(path) = find_bundle_patch(bin, "@deepseek-ai/dsh-agent-presets") {
            let _ = merge_manifest_file(&path, &mut entries);
        }
    }
    if let Some(profile) = profile_dir() {
        if !found_base {
            if let Some(path) = find_bundle_patch(&profile, "@deepseek-ai/dsh-base") {
                merge_manifest_file(&path, &mut entries)?;
                found_base = true;
            }
        }
        let patch = profile.join("cordis.patch.yml");
        if patch.is_file() {
            merge_manifest_file(&patch, &mut entries)?;
        }
    }
    if entries.is_empty() {
        return Err(if found_base {
            "本地插件清单为空".to_string()
        } else {
            "未找到 dsh-base 插件清单".to_string()
        });
    }
    Ok(entries.into_values().collect())
}

fn inventory_fallback_error(dump_result: &Result<String, String>, file_err: &str) -> String {
    match dump_result {
        Err(dump_err) => format!("{dump_err}（已回退本地清单：{file_err}）"),
        Ok(_) => format!("无法从 dsh --dump-config 解析插件（已回退本地清单：{file_err}）"),
    }
}

async fn dump_kivio_config() -> Result<String, String> {
    let def = get_agent_def("dsh").ok_or_else(|| "未注册 dsh".to_string())?;
    let bin = resolve_binary(def)
        .await
        .ok_or_else(|| "未找到 dsh 可执行文件".to_string())?;
    let mut command = agent_cli_command(def, &bin);
    command
        .args(["--profile", KIVIO_PROFILE, "--dump-config"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .no_console_window()
        .kill_on_drop(true);
    if let Some(dir) = profile_dir() {
        command.current_dir(dir);
    }
    let child = command
        .spawn()
        .map_err(|err| format!("启动 dsh --dump-config 失败：{err}"))?;
    let output = timeout(DUMP_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| "dsh --dump-config 超时".to_string())?
        .map_err(|err| format!("读取 dsh --dump-config 失败：{err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.chars().rev().take(800).collect::<String>();
        let detail: String = detail.chars().rev().collect();
        let detail = detail.trim();
        return Err(if detail.is_empty() {
            "dsh --dump-config 失败".to_string()
        } else {
            format!("dsh --dump-config 失败：{detail}")
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if stdout.trim().is_empty() {
        return Err("dsh --dump-config 没有输出".to_string());
    }
    Ok(stdout)
}

fn open_settings_file(app: &AppHandle, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建 dsh 配置目录失败：{err}"))?;
    }
    if !path.exists() {
        crate::chat::storage::atomic_write(path, "", "dsh settings.yaml")?;
    }
    app.shell()
        .open(path.display().to_string(), None)
        .map_err(|err| format!("打开配置文件失败：{err}"))
}

#[tauri::command]
pub fn chat_dsh_plugin_settings_get() -> Result<DshPluginSettingsSnapshot, String> {
    let path = settings_path()?;
    let text = if path.exists() {
        std::fs::read_to_string(&path).map_err(|err| format!("读取 dsh settings.yaml 失败：{err}"))?
    } else {
        String::new()
    };
    parse_settings_snapshot(&text, &path)
}

#[tauri::command]
pub fn chat_dsh_plugin_settings_save(
    patch: DshPluginSettingsPatch,
) -> Result<DshPluginSettingsSnapshot, String> {
    let path = settings_path()?;
    let root = apply_patch(load_root_mapping(&path)?, &patch);
    persist_settings(&path, root)?;
    if let Some(web_search) = &patch.web_search {
        if let Some(api_key) = web_search.api_key.as_deref() {
            let snapshot = parse_settings_snapshot(
                &std::fs::read_to_string(&path).unwrap_or_default(),
                &path,
            )?;
            write_credential(&snapshot.web_search.api_key_env, api_key)?;
        }
    }
    parse_settings_snapshot(
        &std::fs::read_to_string(&path).unwrap_or_default(),
        &path,
    )
}

#[tauri::command]
pub async fn chat_dsh_plugin_inventory() -> Result<Vec<DshPluginEntry>, String> {
    let dump_result = dump_kivio_config().await;
    if let Ok(dump) = &dump_result {
        if let Ok(entries) = parse_inventory_dump(dump) {
            if !entries.is_empty() {
                return Ok(entries);
            }
        }
    }
    match inventory_from_installed_patches().await {
        Ok(entries) if !entries.is_empty() => Ok(entries),
        Ok(_) => Err(inventory_fallback_error(&dump_result, "本地插件清单为空")),
        Err(file_err) => Err(inventory_fallback_error(&dump_result, &file_err)),
    }
}

#[tauri::command]
pub fn chat_dsh_open_settings_file(app: AppHandle) -> Result<(), String> {
    open_settings_file(&app, &settings_path()?)
}

fn reject_managed_official_provider(id: &str) -> Result<(), String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("供应商 id 不能为空".to_string());
    }
    if id == DSH_OFFICIAL_PROVIDER_ID {
        return Err("官方 DeepSeek 不能按第三方供应商编辑".to_string());
    }
    Ok(())
}

fn credential_value(path: &Path, api_key_env: &str) -> Option<String> {
    if api_key_env.is_empty() {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let value = serde_yaml::from_str::<serde_yaml::Value>(&text).ok()?;
    mapping_string(value.as_mapping()?, api_key_env)
}

fn parse_native_models(value: Option<&serde_yaml::Value>) -> Vec<DshNativeProviderModel> {
    let Some(serde_yaml::Value::Sequence(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            serde_yaml::Value::String(id) => {
                let id = id.trim();
                (!id.is_empty()).then(|| DshNativeProviderModel {
                    id: id.to_string(),
                    name: id.to_string(),
                })
            }
            serde_yaml::Value::Mapping(map) => {
                let id = mapping_string(map, "id")?;
                let name = mapping_string(map, "name").unwrap_or_else(|| id.clone());
                Some(DshNativeProviderModel { id, name })
            }
            _ => None,
        })
        .collect()
}

fn selected_default_model(root: &Mapping) -> (Option<String>, Option<String>) {
    for key in ["agent-default-model", "api-gateway"] {
        if let Some(map) = namespace_map(root, key) {
            return (mapping_string(map, "provider"), mapping_string(map, "model"));
        }
    }
    (None, None)
}

fn native_provider_detail_at(
    settings_path: &Path,
    credentials_path: &Path,
    id: &str,
) -> Result<DshNativeProviderDetail, String> {
    reject_managed_official_provider(id)?;
    let root = load_root_mapping(settings_path)?;
    let provider = namespace_map(&root, LLM_PI_AI_NS)
        .and_then(|pi| namespace_map(pi, "providers"))
        .and_then(|providers| {
            providers
                .get(serde_yaml::Value::String(id.to_string()))
                .and_then(serde_yaml::Value::as_mapping)
        })
        .ok_or_else(|| format!("未找到供应商 {id}"))?;
    let name = mapping_string(provider, "displayName").unwrap_or_else(|| id.to_string());
    let base_url = mapping_string(provider, "baseURL").unwrap_or_default();
    let api = mapping_string(provider, "api").unwrap_or_default();
    let api_key_env = mapping_string(provider, "apiKeyEnv").unwrap_or_default();
    let models = parse_native_models(provider.get(serde_yaml::Value::String("models".to_string())));
    let (default_provider, default_model) = selected_default_model(&root);
    let default_model = if default_provider.as_deref() == Some(id) {
        default_model
            .filter(|model| models.iter().any(|entry| entry.id == *model))
            .or_else(|| models.first().map(|model| model.id.clone()))
            .unwrap_or_default()
    } else {
        models
            .first()
            .map(|model| model.id.clone())
            .unwrap_or_default()
    };
    Ok(DshNativeProviderDetail {
        id: id.to_string(),
        name,
        base_url,
        api,
        api_key: credential_value(credentials_path, &api_key_env).unwrap_or_default(),
        api_key_env,
        models,
        default_model,
    })
}

fn clear_default_if_matches(root: &mut Mapping, id: &str) {
    for key in ["agent-default-model", "api-gateway"] {
        let yaml_key = serde_yaml::Value::String(key.to_string());
        let matches = root
            .get(&yaml_key)
            .and_then(serde_yaml::Value::as_mapping)
            .and_then(|map| mapping_string(map, "provider"))
            .as_deref()
            == Some(id);
        if matches {
            root.remove(&yaml_key);
        }
    }
}

fn delete_native_provider_in(root: &mut Mapping, id: &str) {
    let pi_key = serde_yaml::Value::String(LLM_PI_AI_NS.to_string());
    let Some(serde_yaml::Value::Mapping(mut pi)) = root.remove(&pi_key) else {
        return;
    };
    let providers_key = serde_yaml::Value::String("providers".to_string());
    if let Some(serde_yaml::Value::Mapping(mut providers)) = pi.remove(&providers_key) {
        providers.remove(&serde_yaml::Value::String(id.to_string()));
        if !providers.is_empty() {
            pi.insert(providers_key, serde_yaml::Value::Mapping(providers));
        }
    }
    if !pi.is_empty() {
        root.insert(pi_key, serde_yaml::Value::Mapping(pi));
    }
    clear_default_if_matches(root, id);
}

fn delete_native_provider_at(settings_path: &Path, id: &str) -> Result<(), String> {
    reject_managed_official_provider(id)?;
    let mut root = load_root_mapping(settings_path)?;
    delete_native_provider_in(&mut root, id);
    persist_settings(settings_path, root)
}

fn modality_list(has_image: bool) -> serde_yaml::Value {
    let mut items = vec![serde_yaml::Value::String("text".to_string())];
    if has_image {
        items.push(serde_yaml::Value::String("image".to_string()));
    }
    serde_yaml::Value::Sequence(items)
}

fn modality_matches(value: Option<&serde_yaml::Value>, has_image: bool) -> bool {
    let Some(serde_yaml::Value::Sequence(items)) = value else {
        return false;
    };
    let has_text = items.iter().any(|item| item.as_str() == Some("text"));
    let image = items.iter().any(|item| item.as_str() == Some("image"));
    has_text && image == has_image
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReasoningDecl {
    Disabled,
    Levels(BTreeMap<String, Option<String>>),
}

#[derive(Debug, Clone)]
struct ModelCaps {
    has_image: bool,
    reasoning: Option<ReasoningDecl>,
}

fn parse_reasoning_decl(value: &serde_json::Value) -> Option<ReasoningDecl> {
    if value.as_bool() == Some(false) {
        return Some(ReasoningDecl::Disabled);
    }
    let object = value.as_object()?;
    if object.is_empty() {
        return None;
    }
    let mut levels = BTreeMap::new();
    for (key, item) in object {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let wire = item
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        levels.insert(key.to_string(), wire);
    }
    Some(ReasoningDecl::Levels(levels))
}

fn reasoning_yaml(decl: &ReasoningDecl) -> serde_yaml::Value {
    match decl {
        ReasoningDecl::Disabled => serde_yaml::Value::Bool(false),
        ReasoningDecl::Levels(levels) => {
            let mut map = Mapping::new();
            for (key, wire) in levels {
                map.insert(
                    serde_yaml::Value::String(key.clone()),
                    match wire {
                        Some(value) => serde_yaml::Value::String(value.clone()),
                        None => serde_yaml::Value::Null,
                    },
                );
            }
            serde_yaml::Value::Mapping(map)
        }
    }
}

fn reasoning_matches(existing: Option<&serde_yaml::Value>, decl: &ReasoningDecl) -> bool {
    match (existing, decl) {
        (Some(serde_yaml::Value::Bool(false)), ReasoningDecl::Disabled) => true,
        (Some(serde_yaml::Value::Mapping(map)), ReasoningDecl::Levels(levels)) => {
            if map.len() != levels.len() {
                return false;
            }
            levels.iter().all(|(key, wire)| {
                match map.get(serde_yaml::Value::String(key.clone())) {
                    Some(serde_yaml::Value::Null) => wire.is_none(),
                    Some(serde_yaml::Value::String(value)) => wire.as_deref() == Some(value.as_str()),
                    _ => false,
                }
            })
        }
        _ => false,
    }
}

fn model_caps_from_config_json(config_json: &str) -> BTreeMap<String, ModelCaps> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(config_json) else {
        return BTreeMap::new();
    };
    let Some(models) = value.get("models").and_then(serde_json::Value::as_array) else {
        return BTreeMap::new();
    };
    let mut caps = BTreeMap::new();
    for model in models {
        let Some(id) = model
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        let has_image = model
            .get("input")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|items| items.iter().any(|item| item.as_str() == Some("image")));
        let reasoning = model.get("reasoningEfforts").and_then(parse_reasoning_decl);
        caps.insert(id.to_string(), ModelCaps { has_image, reasoning });
    }
    caps
}

fn write_model_caps(map: &mut Mapping, caps: &ModelCaps) -> bool {
    let mut changed = false;
    if !modality_matches(
        map.get(serde_yaml::Value::String("input".to_string())),
        caps.has_image,
    ) {
        map.insert(
            serde_yaml::Value::String("input".to_string()),
            modality_list(caps.has_image),
        );
        changed = true;
    }
    if let Some(reasoning) = &caps.reasoning {
        if !reasoning_matches(
            map.get(serde_yaml::Value::String("reasoningEfforts".to_string())),
            reasoning,
        ) {
            map.insert(
                serde_yaml::Value::String("reasoningEfforts".to_string()),
                reasoning_yaml(reasoning),
            );
            changed = true;
        }
    }
    changed
}

fn apply_model_caps(models: &mut serde_yaml::Value, caps: &BTreeMap<String, ModelCaps>) -> bool {
    let Some(items) = models.as_sequence_mut() else {
        return false;
    };
    let mut changed = false;
    for item in items.iter_mut() {
        match item {
            serde_yaml::Value::String(id) => {
                let id = id.trim();
                let Some(model) = caps.get(id) else {
                    continue;
                };
                let mut map = Mapping::new();
                map.insert(
                    serde_yaml::Value::String("id".to_string()),
                    serde_yaml::Value::String(id.to_string()),
                );
                write_model_caps(&mut map, model);
                *item = serde_yaml::Value::Mapping(map);
                changed = true;
            }
            serde_yaml::Value::Mapping(map) => {
                let Some(id) = mapping_string(map, "id") else {
                    continue;
                };
                let Some(model) = caps.get(&id) else {
                    continue;
                };
                changed |= write_model_caps(map, model);
            }
            _ => {}
        }
    }
    changed
}

fn apply_provider_caps(root: &mut Mapping, route: &str, caps: &BTreeMap<String, ModelCaps>) -> bool {
    let Some(pi) = root
        .get_mut(serde_yaml::Value::String(LLM_PI_AI_NS.to_string()))
        .and_then(serde_yaml::Value::as_mapping_mut)
    else {
        return false;
    };
    let Some(providers) = pi
        .get_mut(serde_yaml::Value::String("providers".to_string()))
        .and_then(serde_yaml::Value::as_mapping_mut)
    else {
        return false;
    };
    let Some(provider) = providers
        .get_mut(serde_yaml::Value::String(route.to_string()))
        .and_then(serde_yaml::Value::as_mapping_mut)
    else {
        return false;
    };
    let mut changed = false;
    if let Some(models) = provider.get_mut(serde_yaml::Value::String("models".to_string())) {
        changed |= apply_model_caps(models, caps);
    }
    let default_has_image = caps.values().any(|model| model.has_image);
    if !modality_matches(
        provider.get(serde_yaml::Value::String("defaultInput".to_string())),
        default_has_image,
    ) {
        provider.insert(
            serde_yaml::Value::String("defaultInput".to_string()),
            modality_list(default_has_image),
        );
        changed = true;
    }
    changed
}

fn sync_kivio_model_capabilities_at(
    settings_path: &Path,
    providers: &[ExternalCliProvider],
) -> Result<(), String> {
    let mut root = load_root_mapping(settings_path)?;
    let mut changed = false;
    for provider in providers {
        let route = provider.native_provider_id.trim();
        if route.is_empty() || route == DSH_OFFICIAL_PROVIDER_ID {
            continue;
        }
        let caps = model_caps_from_config_json(&provider.config_json);
        if caps.is_empty() {
            continue;
        }
        changed |= apply_provider_caps(&mut root, route, &caps);
    }
    if changed {
        persist_settings(settings_path, root)?;
    }
    Ok(())
}

/// 把 Kivio 已保存的模型 `input` / `reasoningEfforts` 写回已存在的 `settings.yaml` 路由。
/// 不新建供应商，也不写密钥；官方 web 的贴图和 effort 选择只认这些字段。
pub(crate) fn sync_kivio_model_capabilities(providers: &[ExternalCliProvider]) -> Result<(), String> {
    let Ok(path) = settings_path() else {
        return Ok(());
    };
    if !path.exists() {
        return Ok(());
    }
    sync_kivio_model_capabilities_at(&path, providers)
}

#[tauri::command]
pub fn chat_dsh_native_provider_get(id: String) -> Result<DshNativeProviderDetail, String> {
    native_provider_detail_at(&settings_path()?, &credentials_path()?, id.trim())
}

#[tauri::command]
pub fn chat_dsh_native_provider_delete(id: String) -> Result<(), String> {
    delete_native_provider_at(&settings_path()?, id.trim())
}

#[tauri::command]
pub fn chat_dsh_official_credential_status() -> Result<DshOfficialCredential, String> {
    let (configured, writable) = credential_status(DEFAULT_API_KEY_ENV);
    Ok(DshOfficialCredential {
        configured,
        writable,
    })
}

pub(crate) fn official_deepseek_key_ready() -> bool {
    credential_status(DEFAULT_API_KEY_ENV).0
}

#[tauri::command]
pub fn chat_dsh_official_credential_save(api_key: String) -> Result<DshOfficialCredential, String> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err("请输入 DeepSeek API 密钥".to_string());
    }
    write_credential(DEFAULT_API_KEY_ENV, key)?;
    chat_dsh_official_credential_status()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_settings_inherit_schema_defaults() {
        let snapshot = parse_settings_snapshot("{}", Path::new("/tmp/settings.yaml")).unwrap();
        assert_eq!(snapshot.shell.timeout_ms, None);
        assert_eq!(snapshot.shell.timeout_ms_default, 120_000);
        assert_eq!(snapshot.agent_loop.max_parallel_tool_calls_default, 10);
        assert_eq!(snapshot.web_search.max_uses_default, 5);
        assert_eq!(snapshot.web_search.api_key_env, "DEEPSEEK_API_KEY");
    }

    #[test]
    fn reads_overridden_namespaces() {
        let snapshot = parse_settings_snapshot(
            r#"
ui-onboarding:
  welcomeNoticeVersion: keep-me
shell:
  timeoutMs: 90000
  maxOutputBytes: 32000
agent-loop:
  maxParallelToolCalls: 4
web-search-deepseek:
  baseURL: https://relay.example/anthropic/v1
  maxUses: 2
  apiKeyEnv: SEARCH_KEY
"#,
            Path::new("/tmp/settings.yaml"),
        )
        .unwrap();
        assert_eq!(snapshot.shell.timeout_ms, Some(90_000));
        assert_eq!(snapshot.shell.max_output_bytes, Some(32_000));
        assert_eq!(snapshot.agent_loop.max_parallel_tool_calls, Some(4));
        assert_eq!(
            snapshot.web_search.base_url.as_deref(),
            Some("https://relay.example/anthropic/v1")
        );
        assert_eq!(snapshot.web_search.max_uses, Some(2));
        assert_eq!(snapshot.web_search.api_key_env, "SEARCH_KEY");
    }

    #[test]
    fn patch_updates_one_namespace_and_keeps_neighbors() {
        let root = serde_yaml::from_str::<serde_yaml::Value>(
            "ui-onboarding:\n  keep: 1\nshell:\n  timeoutMs: 1000\n",
        )
        .unwrap()
        .as_mapping()
        .unwrap()
        .clone();
        let patched = apply_patch(
            root,
            &DshPluginSettingsPatch {
                agent_loop: Some(DshAgentLoopPatch {
                    max_parallel_tool_calls: Some(Some(3)),
                }),
                ..DshPluginSettingsPatch::default()
            },
        );
        assert!(patched.contains_key(serde_yaml::Value::String("ui-onboarding".into())));
        assert_eq!(
            mapping_u64(namespace_map(&patched, SHELL_NS).unwrap(), "timeoutMs"),
            Some(1000)
        );
        assert_eq!(
            mapping_u64(
                namespace_map(&patched, AGENT_LOOP_NS).unwrap(),
                "maxParallelToolCalls"
            ),
            Some(3)
        );
    }

    #[test]
    fn reset_removes_namespace_when_empty() {
        let root = serde_yaml::from_str::<serde_yaml::Value>("shell:\n  timeoutMs: 1000\n")
            .unwrap()
            .as_mapping()
            .unwrap()
            .clone();
        let patched = apply_patch(
            root,
            &DshPluginSettingsPatch {
                shell: Some(DshShellPatch {
                    timeout_ms: Some(None),
                    max_output_bytes: Some(None),
                }),
                ..DshPluginSettingsPatch::default()
            },
        );
        assert!(!patched.contains_key(serde_yaml::Value::String(SHELL_NS.into())));
    }

    #[test]
    fn inventory_reads_composed_dump_and_nested_groups() {
        let dump = r#"
# == cordis.yml
- id: include
  name: '@deepseek-ai/cordis-plugin-include'
# == @deepseek-ai/dsh-base
- id: timer
  name: '@deepseek-ai/dsh-timer'
- id: tool-bash
  name: '@deepseek-ai/dsh-tool-bash'
  disabled: true
- id: group-tools
  name: '@deepseek-ai/dsh-group'
  group: true
  config:
    - id: tool-web
      name: '@deepseek-ai/dsh-tool-web'
      disabled: !!js process.platform === 'win32'
"#;
        let entries = parse_inventory_dump(dump).unwrap();
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0].id, "group-tools");
        assert!(entries.iter().any(|entry| entry.id == "include" && entry.enabled));
        assert!(entries.iter().any(|entry| entry.id == "tool-bash" && !entry.enabled));
        // !!js 清成 null：不当成 disabled。
        assert!(entries.iter().any(|entry| entry.id == "tool-web" && entry.enabled));
    }

    #[test]
    fn inventory_skips_unparseable_dump_preamble() {
        let dump = r#"
node: warning this is not yaml
# == cordis.yml
- id: timer
  name: '@deepseek-ai/dsh-timer'
# == broken
:::::
# == @deepseek-ai/dsh-base
- id: tool-bash
  name: '@deepseek-ai/dsh-tool-bash'
  disabled: true
"#;
        let entries = parse_inventory_dump(dump).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|entry| entry.id == "timer" && entry.enabled));
        assert!(entries.iter().any(|entry| entry.id == "tool-bash" && !entry.enabled));
    }

    #[test]
    fn inventory_reads_patch_insert_and_disabled_overlay() {
        let mut entries = BTreeMap::new();
        parse_plugin_manifest(
            r#"
- insert:
    - id: timer
      name: '@deepseek-ai/dsh-timer'
    - id: tool-bash
      name: '@deepseek-ai/dsh-tool-bash'
    - id: group-tools
      name: '@deepseek-ai/dsh-group'
      config:
        - id: tool-web
          name: '@deepseek-ai/dsh-tool-web'
"#,
            &mut entries,
        )
        .unwrap();
        parse_plugin_manifest(
            r#"
- id: tool-bash
  disabled: true
- insert:
    - id: kivio-dsh-jsonrpc-bridge
      name: './kivio-dsh-bridge.mjs'
"#,
            &mut entries,
        )
        .unwrap();
        assert!(entries.get("timer").is_some_and(|entry| entry.enabled));
        assert!(entries.get("tool-bash").is_some_and(|entry| !entry.enabled));
        assert_eq!(
            entries
                .get("kivio-dsh-jsonrpc-bridge")
                .map(|entry| entry.module_name.as_str()),
            Some("./kivio-dsh-bridge.mjs")
        );
        assert!(entries.contains_key("tool-web"));
    }

    #[test]
    fn write_credential_skips_empty_key() {
        assert!(write_credential(DEFAULT_API_KEY_ENV, "  ").is_ok());
    }

    #[test]
    fn official_credential_writes_deepseek_key_and_keeps_neighbors() {
        let dir = std::env::temp_dir().join(format!("kivio-dsh-cred-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".credentials.yaml");
        std::fs::write(&path, "OTHER_KEY: keep-me\n").unwrap();
        write_credential_at(&path, DEFAULT_API_KEY_ENV, "sk-test").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("DEEPSEEK_API_KEY"));
        assert!(text.contains("sk-test"));
        assert!(text.contains("OTHER_KEY"));
        assert!(credentials_contain(&path, DEFAULT_API_KEY_ENV));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn native_provider_detail_reads_models_and_key_without_touching_neighbors() {
        let dir = std::env::temp_dir().join(format!("kivio-dsh-native-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let settings = dir.join("settings.yaml");
        let credentials = dir.join(".credentials.yaml");
        std::fs::write(
            &settings,
            r#"
ui-onboarding:
  welcomeNoticeVersion: keep-me
agent-default-model:
  provider: gpt
  model: gpt-5.6-sol
llm-pi-ai:
  providers:
    gpt:
      displayName: gpt
      apiKeyEnv: GPT_API_KEY
      api: openai-responses
      baseURL: https://relay.example/v1
      models:
        - id: gpt-5.6-luna
          name: gpt-5.6-luna
        - id: gpt-5.6-sol
          name: gpt-5.6-sol
        - id: ""
        - {}
"#,
        )
        .unwrap();
        std::fs::write(&credentials, "GPT_API_KEY: sk-test\nOTHER_KEY: keep-me\n").unwrap();

        let detail = native_provider_detail_at(&settings, &credentials, "gpt").unwrap();
        assert_eq!(detail.id, "gpt");
        assert_eq!(detail.name, "gpt");
        assert_eq!(detail.base_url, "https://relay.example/v1");
        assert_eq!(detail.api, "openai-responses");
        assert_eq!(detail.api_key, "sk-test");
        assert_eq!(detail.api_key_env, "GPT_API_KEY");
        assert_eq!(detail.default_model, "gpt-5.6-sol");
        assert_eq!(
            detail.models,
            vec![
                DshNativeProviderModel {
                    id: "gpt-5.6-luna".into(),
                    name: "gpt-5.6-luna".into(),
                },
                DshNativeProviderModel {
                    id: "gpt-5.6-sol".into(),
                    name: "gpt-5.6-sol".into(),
                },
            ]
        );
        assert!(native_provider_detail_at(&settings, &credentials, DSH_OFFICIAL_PROVIDER_ID).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn native_provider_delete_removes_route_and_stale_default() {
        let dir = std::env::temp_dir().join(format!("kivio-dsh-del-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let settings = dir.join("settings.yaml");
        std::fs::write(
            &settings,
            r#"
ui-onboarding:
  welcomeNoticeVersion: keep-me
agent-default-model:
  provider: gpt
  model: gpt-5.6-sol
llm-pi-ai:
  providers:
    gpt:
      displayName: gpt
      api: openai-responses
    keep:
      displayName: Keep
"#,
        )
        .unwrap();

        delete_native_provider_at(&settings, "gpt").unwrap();
        let text = std::fs::read_to_string(&settings).unwrap();
        assert!(text.contains("welcomeNoticeVersion"));
        assert!(text.contains("Keep"));
        assert!(!text.contains("displayName: gpt"));
        assert!(!text.contains("agent-default-model"));
        assert!(delete_native_provider_at(&settings, DSH_OFFICIAL_PROVIDER_ID).is_err());
        delete_native_provider_at(&settings, "missing").unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn kivio_model_caps_write_existing_settings_route_only() {
        let dir = std::env::temp_dir().join(format!("kivio-dsh-caps-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let settings = dir.join("settings.yaml");
        std::fs::write(
            &settings,
            r#"
ui-onboarding:
  welcomeNoticeVersion: keep-me
llm-pi-ai:
  providers:
    gpt:
      displayName: gpt
      api: openai-responses
      models:
        - id: gpt-5.6-sol
          name: gpt-5.6-sol
        - id: gpt-image-2
          name: gpt-image-2
    keep:
      displayName: Keep
"#,
        )
        .unwrap();

        sync_kivio_model_capabilities_at(
            &settings,
            &[ExternalCliProvider {
                id: "p-dsh-gpt".into(),
                name: "gpt".into(),
                native_provider_id: "gpt".into(),
                config_json: r#"{
                    "models": [
                        {
                            "id":"gpt-5.6-sol",
                            "input":["text","image"],
                            "reasoningEfforts":{"off":null,"low":"low","high":"high","xhigh":"xhigh","max":"max"}
                        },
                        {
                            "id":"gpt-image-2",
                            "input":["text","image"],
                            "reasoningEfforts":false
                        }
                    ]
                }"#
                .into(),
                ..Default::default()
            }],
        )
        .unwrap();

        let text = std::fs::read_to_string(&settings).unwrap();
        assert!(text.contains("welcomeNoticeVersion"));
        assert!(text.contains("Keep"));
        assert!(text.contains("defaultInput"));
        assert!(text.contains("gpt-5.6-sol"));
        assert!(text.contains("reasoningEfforts"));
        assert!(text.contains("xhigh"));
        assert!(text.matches("image").count() >= 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dotenv_parser_accepts_export_quotes_and_skips_empty() {
        assert!(dotenv_has_key("DEEPSEEK_API_KEY=sk-test\n", "DEEPSEEK_API_KEY"));
        assert!(dotenv_has_key(
            "export DEEPSEEK_API_KEY=sk-test\n",
            "DEEPSEEK_API_KEY"
        ));
        assert!(dotenv_has_key(
            "DEEPSEEK_API_KEY=\"sk-test\"\n",
            "DEEPSEEK_API_KEY"
        ));
        assert!(dotenv_has_key(
            "DEEPSEEK_API_KEY='sk-test'\n",
            "DEEPSEEK_API_KEY"
        ));
        assert!(!dotenv_has_key("DEEPSEEK_API_KEY=\n", "DEEPSEEK_API_KEY"));
        assert!(!dotenv_has_key("DEEPSEEK_API_KEY=\"\"\n", "DEEPSEEK_API_KEY"));
        assert!(!dotenv_has_key(
            "# DEEPSEEK_API_KEY=sk-test\n",
            "DEEPSEEK_API_KEY"
        ));
        assert!(!dotenv_has_key("OTHER=sk-test\n", "DEEPSEEK_API_KEY"));
    }

    #[test]
    fn pi_ai_api_key_env_reads_settings_route() {
        let yaml = r#"
llm-pi-ai:
  providers:
    gpt:
      displayName: gpt
      apiKeyEnv: OPENAI_API_KEY
"#;
        assert_eq!(
            pi_ai_api_key_env_from_settings(yaml, "gpt").as_deref(),
            Some("OPENAI_API_KEY")
        );
        assert_eq!(pi_ai_api_key_env_from_settings(yaml, "missing"), None);
        assert_eq!(pi_ai_api_key_env_from_settings("{}", "gpt"), None);
    }

    #[test]
    fn credential_status_accepts_dotenv_when_yaml_missing() {
        let dir = std::env::temp_dir().join(format!("kivio-dsh-env-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let user_env = dir.join(".env");
        std::fs::write(&user_env, "DEEPSEEK_API_KEY=sk-from-home\n").unwrap();
        let (configured, writable) =
            credential_status_with_files("DEEPSEEK_API_KEY", None, Some(&user_env), None);
        assert!(configured);
        assert!(writable);

        let project_env = dir.join("project.env");
        std::fs::write(&project_env, "GPT_KEY=sk-project\n").unwrap();
        let (configured, writable) =
            credential_status_with_files("GPT_KEY", None, None, Some(&project_env));
        assert!(configured);
        assert!(writable);

        let credentials = dir.join(".credentials.yaml");
        std::fs::write(&credentials, "DEEPSEEK_API_KEY: sk-yaml\n").unwrap();
        let (configured, writable) =
            credential_status_with_files("DEEPSEEK_API_KEY", Some(&credentials), None, None);
        assert!(configured);
        assert!(writable);

        let (configured, _) = credential_status_with_files(
            "MISSING_KEY",
            Some(&credentials),
            Some(&user_env),
            Some(&project_env),
        );
        assert!(!configured);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn official_deepseek_key_ready_matches_credential_status() {
        assert_eq!(
            official_deepseek_key_ready(),
            credential_status(DEFAULT_API_KEY_ENV).0
        );
    }
}
