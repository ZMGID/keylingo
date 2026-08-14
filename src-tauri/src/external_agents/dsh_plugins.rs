//! dsh 官方设置页「插件」两栏的本地适配。
//!
//! 官方 web 把可配项写进 `$DSH_HOME/settings.yaml` 的三个 namespace
//!（`shell` / `agent-loop` / `web-search-deepseek`），密钥进 `.credentials.yaml`，
//! 清单则来自已 compose 的 Loader 树。Kivio 不改 `profiles/kivio/cordis.patch.yml`
//!（那份由 `dsh_profile::ensure_profile_ready` 每轮重写），所以：
//!
//! - **插件配置**：读写 `settings.yaml` + 可选写凭据，热重载后 kivio / web 共用。
//! - **插件列表**：优先 `dsh --profile kivio --dump-config`（boot-free）解析
//!   id / name / disabled；失败则读安装包里的 `dsh-base` / `dsh-agent-presets`
//!   patch + Kivio 自己的 `cordis.patch.yml`。没有 live host，fiber 相位一律未知；
//!   启用态以 compose 后的 `disabled` 为准。

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

const SHELL_NS: &str = "shell";
const AGENT_LOOP_NS: &str = "agent-loop";
const WEB_SEARCH_NS: &str = "web-search-deepseek";
const CREDENTIALS_FILENAME: &str = ".credentials.yaml";
const DEFAULT_API_KEY_ENV: &str = "DEEPSEEK_API_KEY";

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
    if std::env::var_os(api_key_env)
        .is_some_and(|value| !value.is_empty())
    {
        return (true, false);
    }
    let Ok(path) = credentials_path() else {
        return (false, true);
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return (false, true);
    };
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(&text) else {
        return (false, true);
    };
    let configured = value
        .as_mapping()
        .and_then(|map| mapping_string(map, api_key_env))
        .is_some();
    (configured, true)
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
    let path = credentials_path()?;
    let mut root = if path.exists() {
        let text = std::fs::read_to_string(&path)
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
    crate::external_agents::provider_profile::write_private_atomic(&path, &yaml)
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
}
