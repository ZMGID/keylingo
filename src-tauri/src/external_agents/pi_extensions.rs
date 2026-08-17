use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::external_agents::registry::get_agent_def;
use crate::proc::NoConsoleWindow;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(300);
const OUTPUT_MAX_CHARS: usize = 24_000;
const SETTINGS_LOCK_ATTEMPTS: usize = 10;
const SETTINGS_LOCK_DELAY: Duration = Duration::from_millis(20);
const SETTINGS_LOCK_STALE: Duration = Duration::from_secs(10);
static SETTINGS_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiExtensionInventory {
    pub agent_dir: String,
    pub extensions_dir: String,
    pub packages: Vec<PiPackageEntry>,
    pub local_extensions: Vec<PiLocalExtensionEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiPackageEntry {
    pub source: String,
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub path: Option<String>,
    pub enabled: bool,
    pub can_toggle: bool,
    pub has_extensions: bool,
    pub extension_entries: usize,
    pub resources: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiLocalExtensionEntry {
    pub relative_path: String,
    pub name: String,
    pub path: String,
    pub enabled: bool,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiExtensionCommandResult {
    pub output: String,
}

pub(crate) fn agent_dir() -> Result<PathBuf, String> {
    crate::external_agents::provider_profile::pi_agent_dir()
        .ok_or_else(|| "无法解析 Pi 配置目录".to_string())
}

fn settings_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("settings.json")
}

pub(crate) struct PiSettingsFileLock {
    lock_dir: PathBuf,
}

impl Drop for PiSettingsFileLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.lock_dir);
    }
}

pub(crate) fn lock_settings_file(path: &Path) -> Result<PiSettingsFileLock, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建 Pi 设置目录失败：{err}"))?;
    }
    let lock_dir = PathBuf::from(format!("{}.lock", path.to_string_lossy()));
    for attempt in 0..SETTINGS_LOCK_ATTEMPTS {
        match std::fs::create_dir(&lock_dir) {
            Ok(()) => return Ok(PiSettingsFileLock { lock_dir }),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = std::fs::metadata(&lock_dir)
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|elapsed| elapsed > SETTINGS_LOCK_STALE);
                if stale {
                    let _ = std::fs::remove_dir(&lock_dir);
                    continue;
                }
                if attempt + 1 < SETTINGS_LOCK_ATTEMPTS {
                    std::thread::sleep(SETTINGS_LOCK_DELAY);
                    continue;
                }
                return Err("Pi settings.json 正被其他进程修改，请稍后重试".to_string());
            }
            Err(err) => return Err(format!("锁定 Pi settings.json 失败：{err}")),
        }
    }
    Err("锁定 Pi settings.json 失败".to_string())
}
pub(crate) fn read_settings(agent_dir: &Path) -> Result<Map<String, Value>, String> {
    let path = settings_path(agent_dir);
    if !path.exists() {
        return Ok(Map::new());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|err| format!("读取 Pi settings.json 失败：{err}"))?;
    match serde_json::from_str::<Value>(&text)
        .map_err(|err| format!("Pi settings.json 不是合法 JSON：{err}"))?
    {
        Value::Object(root) => Ok(root),
        _ => Err("Pi settings.json 顶层必须是对象".to_string()),
    }
}

pub(crate) fn write_settings(agent_dir: &Path, root: &Map<String, Value>) -> Result<(), String> {
    let text = serde_json::to_string_pretty(root)
        .map_err(|err| format!("序列化 Pi settings.json 失败：{err}"))?;
    crate::chat::storage::atomic_write(
        &settings_path(agent_dir),
        &format!("{text}\n"),
        "Pi settings.json",
    )
}

pub(crate) fn package_source(entry: &Value) -> Option<&str> {
    match entry {
        Value::String(source) => Some(source),
        Value::Object(object) => object.get("source").and_then(Value::as_str),
        _ => None,
    }
}

fn npm_package_name(source: &str) -> Option<&str> {
    let spec = source.strip_prefix("npm:")?;
    if spec.starts_with('@') {
        let slash = spec.find('/')?;
        let version = spec[slash + 1..].rfind('@').map(|index| slash + 1 + index);
        Some(version.map_or(spec, |index| &spec[..index]))
    } else {
        Some(spec.rfind('@').map_or(spec, |index| &spec[..index]))
    }
}

fn is_remote_source(source: &str) -> bool {
    source.starts_with("git:")
        || source.starts_with("https://")
        || source.starts_with("http://")
        || source.starts_with("ssh://")
        || source.starts_with("git://")
}

fn configured_path(
    agent_dir: &Path,
    source: &str,
    listed: &HashMap<String, PathBuf>,
) -> Option<PathBuf> {
    if let Some(path) = listed.get(source) {
        return Some(path.clone());
    }
    if let Some(name) = npm_package_name(source) {
        return Some(agent_dir.join("npm").join("node_modules").join(name));
    }
    if is_remote_source(source) {
        return None;
    }
    let path = PathBuf::from(source);
    Some(if path.is_absolute() {
        path
    } else {
        agent_dir.join(path)
    })
}

fn package_enabled(entry: &Value) -> (bool, bool) {
    let Value::Object(object) = entry else {
        return (true, true);
    };
    if object.get("autoload").and_then(Value::as_bool) == Some(false) {
        return (has_positive_filter(object.get("extensions")), false);
    }
    match object.get("extensions") {
        None => (true, true),
        Some(Value::Array(entries)) if entries.is_empty() => (false, true),
        Some(Value::Array(_)) => (true, false),
        Some(_) => (true, false),
    }
}

fn has_positive_filter(value: Option<&Value>) -> bool {
    value.and_then(Value::as_array).is_some_and(|entries| {
        entries
            .iter()
            .filter_map(Value::as_str)
            .any(|entry| !entry.starts_with('!') && !entry.starts_with('-'))
    })
}

fn package_manifest(path: Option<&Path>) -> (Option<Map<String, Value>>, bool) {
    let Some(path) = path else {
        return (None, false);
    };
    if path.is_file() {
        return (None, is_extension_file(path));
    }
    let manifest_path = path.join("package.json");
    let manifest = std::fs::read_to_string(manifest_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().cloned());
    (manifest, false)
}

fn manifest_resource_info(
    manifest: Option<&Map<String, Value>>,
    conventional_extensions_dir: Option<&Path>,
) -> (bool, usize, Vec<String>) {
    let pi = manifest
        .and_then(|root| root.get("pi"))
        .and_then(Value::as_object);
    let mut resources = Vec::new();
    let mut extension_entries = 0usize;
    for key in ["extensions", "skills", "prompts", "themes"] {
        let count = pi
            .and_then(|object| object.get(key))
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        if count > 0 {
            resources.push(key.to_string());
        }
        if key == "extensions" {
            extension_entries = count;
        }
    }
    if pi.is_none() {
        if let Some(dir) = conventional_extensions_dir {
            extension_entries = count_extension_entries(dir);
            if extension_entries > 0 {
                resources.push("extensions".to_string());
            }
        }
    }
    (extension_entries > 0, extension_entries, resources)
}

fn count_extension_entries(path: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            let path = entry.path();
            is_extension_file(&path)
                || (path.is_dir()
                    && ["index.ts", "index.js", "index.mts", "index.mjs"]
                        .iter()
                        .any(|name| path.join(name).is_file()))
        })
        .count()
}

fn is_extension_file(path: &Path) -> bool {
    path.is_file()
        && matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("ts" | "js" | "mts" | "mjs" | "cts" | "cjs")
        )
}

fn package_display(entry: &Value, path: Option<&Path>) -> PiPackageEntry {
    let source = package_source(entry).unwrap_or_default().to_string();
    let (enabled, can_toggle) = package_enabled(entry);
    let (manifest, local_extension_file) = package_manifest(path);
    let conventional_extensions_dir = path
        .filter(|value| value.join("extensions").is_dir())
        .map(|value| value.join("extensions"));
    let (mut has_extensions, mut extension_entries, mut resources) =
        manifest_resource_info(manifest.as_ref(), conventional_extensions_dir.as_deref());
    if local_extension_file {
        has_extensions = true;
        extension_entries = 1;
        resources.push("extensions".to_string());
    }
    resources.sort();
    resources.dedup();
    let name = manifest
        .as_ref()
        .and_then(|root| root.get("name"))
        .and_then(Value::as_str)
        .unwrap_or(&source)
        .to_string();
    let version = manifest
        .as_ref()
        .and_then(|root| root.get("version"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let description = manifest
        .as_ref()
        .and_then(|root| root.get("description"))
        .and_then(Value::as_str)
        .map(str::to_string);
    PiPackageEntry {
        source,
        name,
        version,
        description,
        path: path.map(|value| value.to_string_lossy().into_owned()),
        enabled,
        can_toggle: can_toggle && has_extensions,
        has_extensions,
        extension_entries,
        resources,
    }
}

fn extension_pattern(relative_path: &str) -> String {
    format!("extensions/{}", relative_path.replace('\\', "/"))
}

fn strip_filter_prefix(value: &str) -> &str {
    value.strip_prefix(['!', '+', '-']).unwrap_or(value)
}

fn local_extension_enabled(settings: &Map<String, Value>, relative_path: &str) -> bool {
    let pattern = extension_pattern(relative_path);
    let Some(entries) = settings.get("extensions").and_then(Value::as_array) else {
        return true;
    };
    let mut enabled = true;
    for entry in entries.iter().filter_map(Value::as_str) {
        if strip_filter_prefix(entry).replace('\\', "/") != pattern {
            continue;
        }
        enabled = !entry.starts_with('!') && !entry.starts_with('-');
    }
    enabled
}

fn local_extensions(agent_dir: &Path, settings: &Map<String, Value>) -> Vec<PiLocalExtensionEntry> {
    let dir = agent_dir.join("extensions");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let (relative_path, name, kind) = if is_extension_file(&path) {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let name = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or(&file_name)
                .to_string();
            (file_name, name, "file")
        } else if path.is_dir() {
            let Some(index_name) = ["index.ts", "index.js", "index.mts", "index.mjs"]
                .into_iter()
                .find(|name| path.join(name).is_file())
            else {
                continue;
            };
            let dir_name = entry.file_name().to_string_lossy().into_owned();
            (format!("{dir_name}/{index_name}"), dir_name, "directory")
        } else {
            continue;
        };
        result.push(PiLocalExtensionEntry {
            enabled: local_extension_enabled(settings, &relative_path),
            relative_path,
            name,
            path: path.to_string_lossy().into_owned(),
            kind: kind.to_string(),
        });
    }
    result.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    result
}

async fn listed_package_paths(agent_dir: &Path) -> HashMap<String, PathBuf> {
    let Ok(output) = run_pi_output(agent_dir, &["list", "--no-approve"]).await else {
        return HashMap::new();
    };
    parse_list_paths(&output)
}

fn parse_list_paths(output: &str) -> HashMap<String, PathBuf> {
    let mut result = HashMap::new();
    let mut source: Option<String> = None;
    for line in output.lines() {
        if line.starts_with("  ") && !line.starts_with("    ") {
            source = Some(line.trim().to_string());
        } else if line.starts_with("    ") {
            if let Some(source) = source.take() {
                result.insert(source, PathBuf::from(line.trim()));
            }
        }
    }
    result
}

async fn inventory_at(agent_dir: &Path) -> Result<PiExtensionInventory, String> {
    let settings = read_settings(agent_dir)?;
    let listed = listed_package_paths(agent_dir).await;
    let packages = settings
        .get("packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let source = package_source(entry)?;
            let path = configured_path(agent_dir, source, &listed);
            Some(package_display(entry, path.as_deref()))
        })
        .collect();
    Ok(PiExtensionInventory {
        agent_dir: agent_dir.to_string_lossy().into_owned(),
        extensions_dir: agent_dir.join("extensions").to_string_lossy().into_owned(),
        packages,
        local_extensions: local_extensions(agent_dir, &settings),
    })
}

fn validate_source(source: &str) -> Result<String, String> {
    let source = source.trim();
    if source.is_empty() {
        return Err("请输入 npm、git 或本地路径来源".to_string());
    }
    if source.len() > 4096 || source.contains(['\r', '\n', '\0']) {
        return Err("扩展来源格式无效".to_string());
    }
    Ok(source.to_string())
}

fn configured_entry<'a>(settings: &'a Map<String, Value>, source: &str) -> Option<&'a Value> {
    settings
        .get("packages")
        .and_then(Value::as_array)?
        .iter()
        .find(|entry| package_source(entry) == Some(source))
}

async fn resolve_pi_binary() -> Result<PathBuf, String> {
    let def = get_agent_def("pi").ok_or_else(|| "Pi CLI 定义不存在".to_string())?;
    crate::external_agents::spawn::resolve_binary(def)
        .await
        .ok_or_else(|| "未找到可用的 Pi CLI".to_string())
}

async fn run_pi_output(agent_dir: &Path, args: &[&str]) -> Result<String, String> {
    let def = get_agent_def("pi").ok_or_else(|| "Pi CLI 定义不存在".to_string())?;
    let binary = resolve_pi_binary().await?;
    let mut command = crate::external_agents::spawn::agent_cli_command(def, &binary);
    command
        .args(args)
        .current_dir(agent_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .no_console_window()
        .kill_on_drop(true);
    let output = tokio::time::timeout(COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| "Pi 扩展命令执行超时".to_string())?
        .map_err(|err| format!("启动 Pi 扩展命令失败：{err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let combined = truncate_chars(&combined, OUTPUT_MAX_CHARS);
    if output.status.success() {
        Ok(combined)
    } else if combined.is_empty() {
        Err(format!(
            "Pi 扩展命令失败（退出码 {:?}）",
            output.status.code()
        ))
    } else {
        Err(combined)
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let omitted = value.chars().count() - max_chars;
    format!(
        "{}\n...[省略 {omitted} 个字符]",
        value.chars().take(max_chars).collect::<String>()
    )
}

fn set_package_enabled_at(agent_dir: &Path, source: &str, enabled: bool) -> Result<(), String> {
    let _guard = SETTINGS_LOCK
        .lock()
        .map_err(|_| "Pi 设置写入锁不可用".to_string())?;
    let _file_lock = lock_settings_file(&settings_path(agent_dir))?;
    let mut settings = read_settings(agent_dir)?;
    let packages = settings
        .entry("packages".to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| "Pi settings.json 的 packages 必须是数组".to_string())?;
    let entry = packages
        .iter_mut()
        .find(|entry| package_source(entry) == Some(source))
        .ok_or_else(|| "该 Pi Package 已不在 settings.json 中".to_string())?;
    let (_, can_toggle) = package_enabled(entry);
    if !can_toggle {
        return Err("该 Package 使用了自定义资源过滤，请用 pi config 管理".to_string());
    }
    if enabled {
        if let Value::Object(object) = entry {
            object.remove("extensions");
            if object.len() == 1 && object.get("source").and_then(Value::as_str).is_some() {
                *entry = Value::String(source.to_string());
            }
        }
    } else {
        if let Value::String(value) = entry {
            let mut object = Map::new();
            object.insert("source".to_string(), Value::String(value.clone()));
            *entry = Value::Object(object);
        }
        if let Value::Object(object) = entry {
            object.insert("extensions".to_string(), Value::Array(Vec::new()));
        }
    }
    write_settings(agent_dir, &settings)
}

fn local_entry_path(agent_dir: &Path, relative_path: &str) -> Result<PathBuf, String> {
    if relative_path.is_empty() || relative_path.contains(['\r', '\n', '\0']) {
        return Err("本地扩展路径无效".to_string());
    }
    let path = Path::new(relative_path);
    if path.is_absolute()
        || !path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return Err("本地扩展路径无效".to_string());
    }
    let full = agent_dir.join("extensions").join(path);
    if !full.exists() {
        return Err("本地扩展已不存在".to_string());
    }
    Ok(full)
}

fn set_local_enabled_at(
    agent_dir: &Path,
    relative_path: &str,
    enabled: bool,
) -> Result<(), String> {
    let _full = local_entry_path(agent_dir, relative_path)?;
    let _guard = SETTINGS_LOCK
        .lock()
        .map_err(|_| "Pi 设置写入锁不可用".to_string())?;
    let _file_lock = lock_settings_file(&settings_path(agent_dir))?;
    let mut settings = read_settings(agent_dir)?;
    let pattern = extension_pattern(relative_path);
    let entries = settings
        .entry("extensions".to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| "Pi settings.json 的 extensions 必须是数组".to_string())?;
    entries.retain(|entry| {
        entry
            .as_str()
            .is_none_or(|value| strip_filter_prefix(value).replace('\\', "/") != pattern)
    });
    entries.push(Value::String(format!(
        "{}{pattern}",
        if enabled { "+" } else { "-" }
    )));
    write_settings(agent_dir, &settings)
}

pub(crate) fn open_path(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err("路径不存在".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::{w, PCWSTR};
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let target: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(target.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if result.0 as isize <= 32 {
            return Err(format!(
                "打开路径失败，Windows Shell 错误码：{}",
                result.0 as isize
            ));
        }
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        #[cfg(target_os = "macos")]
        let program = "open";
        #[cfg(not(target_os = "macos"))]
        let program = "xdg-open";
        std::process::Command::new(program)
            .arg(path)
            .no_console_window()
            .spawn()
            .map_err(|err| format!("打开路径失败：{err}"))?;
        Ok(())
    }
}

#[tauri::command]
pub async fn chat_pi_extensions_inventory() -> Result<PiExtensionInventory, String> {
    inventory_at(&agent_dir()?).await
}

#[tauri::command]
pub fn chat_pi_extension_set_enabled(
    kind: String,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    let agent_dir = agent_dir()?;
    match kind.as_str() {
        "package" => set_package_enabled_at(&agent_dir, &id, enabled),
        "local" => set_local_enabled_at(&agent_dir, &id, enabled),
        _ => Err("未知 Pi 扩展类型".to_string()),
    }
}

#[tauri::command]
pub async fn chat_pi_extension_install(source: String) -> Result<PiExtensionCommandResult, String> {
    let source = validate_source(&source)?;
    let agent_dir = agent_dir()?;
    let output = run_pi_output(&agent_dir, &["install", &source, "--no-approve"]).await?;
    Ok(PiExtensionCommandResult { output })
}

#[tauri::command]
pub async fn chat_pi_extension_update(
    source: Option<String>,
) -> Result<PiExtensionCommandResult, String> {
    let agent_dir = agent_dir()?;
    let output = if let Some(source) = source {
        let source = validate_source(&source)?;
        let settings = read_settings(&agent_dir)?;
        if configured_entry(&settings, &source).is_none() {
            return Err("该 Pi Package 已不在 settings.json 中".to_string());
        }
        run_pi_output(
            &agent_dir,
            &["update", "--extension", &source, "--no-approve"],
        )
        .await?
    } else {
        run_pi_output(&agent_dir, &["update", "--extensions", "--no-approve"]).await?
    };
    Ok(PiExtensionCommandResult { output })
}

#[tauri::command]
pub async fn chat_pi_extension_remove(source: String) -> Result<PiExtensionCommandResult, String> {
    let source = validate_source(&source)?;
    let agent_dir = agent_dir()?;
    let settings = read_settings(&agent_dir)?;
    if configured_entry(&settings, &source).is_none() {
        return Err("该 Pi Package 已不在 settings.json 中".to_string());
    }
    let output = run_pi_output(&agent_dir, &["remove", &source, "--no-approve"]).await?;
    Ok(PiExtensionCommandResult { output })
}

#[tauri::command]
pub async fn chat_pi_extension_open(kind: String, id: String) -> Result<(), String> {
    let agent_dir = agent_dir()?;
    match kind.as_str() {
        "package" => {
            let settings = read_settings(&agent_dir)?;
            let entry = configured_entry(&settings, &id)
                .ok_or_else(|| "该 Pi Package 已不在 settings.json 中".to_string())?;
            let listed = listed_package_paths(&agent_dir).await;
            let path = configured_path(
                &agent_dir,
                package_source(entry).unwrap_or_default(),
                &listed,
            )
            .ok_or_else(|| "无法解析该 Package 的安装目录".to_string())?;
            open_path(&path)
        }
        "local" => {
            let entry = local_entry_path(&agent_dir, &id)?;
            let parent = entry
                .parent()
                .ok_or_else(|| "本地扩展没有可打开的目录".to_string())?;
            open_path(parent)
        }
        _ => Err("未知 Pi 扩展类型".to_string()),
    }
}

#[tauri::command]
pub fn chat_pi_extensions_open_dir() -> Result<(), String> {
    let dir = agent_dir()?.join("extensions");
    std::fs::create_dir_all(&dir).map_err(|err| format!("创建 Pi 扩展目录失败：{err}"))?;
    open_path(&dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kivio-pi-extensions-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn npm_source_parsing_handles_scoped_and_versioned_names() {
        assert_eq!(npm_package_name("npm:plain@1.2.3"), Some("plain"));
        assert_eq!(npm_package_name("npm:@scope/pkg@1.2.3"), Some("@scope/pkg"));
        assert_eq!(npm_package_name("npm:@scope/pkg"), Some("@scope/pkg"));
    }

    #[test]
    fn package_toggle_preserves_unrelated_settings() {
        let dir = temp_dir();
        std::fs::write(
            settings_path(&dir),
            r#"{
  "defaultModel": "keep-me",
  "packages": ["npm:one", {"source":"npm:two","skills":[]}]
}"#,
        )
        .unwrap();

        set_package_enabled_at(&dir, "npm:one", false).unwrap();
        let disabled = read_settings(&dir).unwrap();
        assert_eq!(disabled["defaultModel"], "keep-me");
        assert_eq!(
            disabled["packages"][0]["extensions"],
            Value::Array(Vec::new())
        );

        set_package_enabled_at(&dir, "npm:one", true).unwrap();
        let enabled = read_settings(&dir).unwrap();
        assert_eq!(enabled["packages"][0], "npm:one");
        assert_eq!(enabled["packages"][1]["skills"], Value::Array(Vec::new()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn package_toggle_respects_pi_cross_process_lock() {
        let dir = temp_dir();
        std::fs::write(settings_path(&dir), r#"{"packages":["npm:one"]}"#).unwrap();
        std::fs::create_dir(format!("{}.lock", settings_path(&dir).to_string_lossy())).unwrap();

        let error = set_package_enabled_at(&dir, "npm:one", false).unwrap_err();
        assert!(error.contains("其他进程"));
        let settings = read_settings(&dir).unwrap();
        assert_eq!(settings["packages"][0], "npm:one");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn local_toggle_uses_pi_force_patterns() {
        let dir = temp_dir();
        std::fs::create_dir_all(dir.join("extensions")).unwrap();
        std::fs::write(dir.join("extensions/demo.ts"), "export default () => {};").unwrap();

        set_local_enabled_at(&dir, "demo.ts", false).unwrap();
        let disabled = read_settings(&dir).unwrap();
        assert_eq!(disabled["extensions"][0], "-extensions/demo.ts");
        assert!(!local_extension_enabled(&disabled, "demo.ts"));

        set_local_enabled_at(&dir, "demo.ts", true).unwrap();
        let enabled = read_settings(&dir).unwrap();
        assert_eq!(enabled["extensions"][0], "+extensions/demo.ts");
        assert!(local_extension_enabled(&enabled, "demo.ts"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn directory_extension_uses_its_index_entry_path() {
        let dir = temp_dir();
        let extension_dir = dir.join("extensions/workspace-helper");
        std::fs::create_dir_all(&extension_dir).unwrap();
        std::fs::write(extension_dir.join("index.ts"), "export default () => {};").unwrap();

        let entries = local_extensions(&dir, &Map::new());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].relative_path, "workspace-helper/index.ts");
        assert_eq!(entries[0].kind, "directory");

        set_local_enabled_at(&dir, &entries[0].relative_path, false).unwrap();
        let settings = read_settings(&dir).unwrap();
        assert_eq!(
            settings["extensions"][0],
            "-extensions/workspace-helper/index.ts"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn list_output_parser_keeps_source_to_path_pairs() {
        let parsed = parse_list_paths(
            "User packages:\n  npm:one\n    C:\\Users\\me\\.pi\\agent\\npm\\node_modules\\one\n",
        );
        assert_eq!(
            parsed.get("npm:one"),
            Some(&PathBuf::from(
                r"C:\Users\me\.pi\agent\npm\node_modules\one"
            ))
        );
    }
}
