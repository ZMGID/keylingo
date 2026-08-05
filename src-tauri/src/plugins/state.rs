//! 插件本地状态：目录布局、enabled 元数据、PATH / Skill / 提示注入查询。

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::catalog::{catalog_plugin, CatalogPlugin, PLUGIN_CATALOG};
use crate::proc::NoConsoleWindow;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginMeta {
    pub id: String,
    pub version: Option<String>,
    /// 装完默认 false；仅 true 时注入能力
    #[serde(default)]
    pub enabled: bool,
    pub installed_at: Option<String>,
    pub binary_name: Option<String>,
}

impl Default for PluginMeta {
    fn default() -> Self {
        Self {
            id: String::new(),
            version: None,
            enabled: false,
            installed_at: None,
            binary_name: None,
        }
    }
}

pub fn plugins_root() -> Option<PathBuf> {
    crate::app_data::app_data_dir().map(|dir| dir.join("plugins"))
}

pub fn plugin_dir(id: &str) -> Option<PathBuf> {
    plugins_root().map(|root| root.join(id))
}

pub fn meta_path(id: &str) -> Option<PathBuf> {
    plugin_dir(id).map(|dir| dir.join("meta.json"))
}

/// 插件附属 skill 目录：`plugins/<plugin_id>/skills/<skill_id>/`
pub fn skill_dir(plugin_id: &str, skill_id: &str) -> Option<PathBuf> {
    plugin_dir(plugin_id).map(|dir| dir.join("skills").join(skill_id))
}

pub fn read_meta(id: &str) -> Option<PluginMeta> {
    let path = meta_path(id)?;
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn write_meta(meta: &PluginMeta) -> Result<(), String> {
    let path = meta_path(&meta.id).ok_or_else(|| "app data directory unavailable".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create plugin dir: {e}"))?;
    }
    let raw = serde_json::to_string_pretty(meta).map_err(|e| format!("serialize meta: {e}"))?;
    std::fs::write(&path, raw).map_err(|e| format!("write meta: {e}"))
}

fn binary_filename(catalog: &CatalogPlugin) -> String {
    #[cfg(windows)]
    {
        format!("{}.exe", catalog.binary)
    }
    #[cfg(not(windows))]
    {
        catalog.binary.to_string()
    }
}

/// Kivio 托管安装目录中的二进制（不论是否启用）。
pub fn kivio_binary_path(id: &str) -> Option<PathBuf> {
    let catalog = catalog_plugin(id)?;
    let dir = plugin_dir(id)?;
    let name = read_meta(id)
        .and_then(|m| m.binary_name)
        .unwrap_or_else(|| binary_filename(catalog));
    let path = dir.join(name);
    path.is_file().then_some(path)
}

/// 列表/检测前刷新进程 PATH（Windows 注册表用户 Path 可能在 AI 安装后才更新，
/// GUI 进程启动时的快照不含新目录）。
pub fn refresh_process_path_for_detection() {
    #[cfg(target_os = "windows")]
    {
        crate::path_env::enrich_path_windows();
    }
    #[cfg(target_os = "macos")]
    {
        crate::path_env::enrich_path_macos();
    }
}

fn which_on_path(binary: &str) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        let output = Command::new("sh")
            .arg("-c")
            .arg(format!("command -v {binary}"))
            .no_console_window()
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!path.is_empty()).then(|| PathBuf::from(path))
    }
    #[cfg(windows)]
    {
        // 同时试 bare name 与 .exe（where 对无扩展名有时不稳）
        for name in [binary, &format!("{binary}.exe")] {
            let output = Command::new("where")
                .arg(name)
                .no_console_window()
                .output()
                .ok();
            let Some(output) = output else { continue };
            if !output.status.success() {
                continue;
            }
            let path = String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from);
            if let Some(p) = path.filter(|p| p.is_file()) {
                return Some(p);
            }
        }
        None
    }
}

/// 展开 `%VAR%` 并检查文件是否存在。
fn expand_known_path(template: &str) -> Option<PathBuf> {
    let expanded = expand_env_in_path(template);
    let path = PathBuf::from(expanded);
    path.is_file().then_some(path)
}

fn expand_env_in_path(input: &str) -> String {
    // Windows-style %VAR%；非 Windows 上也可用于 %USERPROFILE% 等
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(end) = input[i + 1..].find('%') {
                let name = &input[i + 1..i + 1 + end];
                if !name.is_empty() {
                    if let Ok(val) = std::env::var(name) {
                        out.push_str(&val);
                        i += 1 + end + 1;
                        continue;
                    }
                }
            }
        }
        let ch_len = input[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        out.push_str(&input[i..i + ch_len]);
        i += ch_len;
    }
    // Unix-style $HOME / ${HOME}
    if out.contains('$') {
        if let Ok(home) = std::env::var("HOME") {
            out = out.replace("$HOME", &home).replace("${HOME}", &home);
        }
        if let Ok(userprofile) = std::env::var("USERPROFILE") {
            out = out
                .replace("$USERPROFILE", &userprofile)
                .replace("${USERPROFILE}", &userprofile);
        }
    }
    out
}

fn known_binary_path(catalog: &CatalogPlugin) -> Option<PathBuf> {
    for template in catalog.known_binary_paths {
        if let Some(path) = expand_known_path(template) {
            return Some(path);
        }
    }
    None
}

/// 解析可用二进制：Kivio 托管 → 刷新后 PATH → 官方常见安装路径。
pub fn resolve_binary(id: &str) -> Option<PathBuf> {
    if let Some(path) = kivio_binary_path(id) {
        return Some(path);
    }
    let catalog = catalog_plugin(id)?;
    if let Some(path) = which_on_path(catalog.binary) {
        return Some(path);
    }
    known_binary_path(catalog)
}

/// 列表检测用：先刷新 PATH 再 resolve。
pub fn resolve_binary_for_status(id: &str) -> Option<PathBuf> {
    refresh_process_path_for_detection();
    resolve_binary(id)
}

pub fn is_installed(id: &str) -> bool {
    resolve_binary(id).is_some()
}

/// 插件 meta 中的启用开关（未安装也可能有 meta=false）。
pub fn is_enabled(id: &str) -> bool {
    read_meta(id).map(|m| m.enabled).unwrap_or(false)
}

/// 仅 **已安装且启用** 时返回 bin 目录，供 PATH prepend。
pub fn enabled_bin_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for plugin in PLUGIN_CATALOG {
        if !is_enabled(plugin.id) {
            continue;
        }
        if let Some(bin) = resolve_binary(plugin.id) {
            if let Some(parent) = bin.parent() {
                let p = parent.to_path_buf();
                if !dirs.iter().any(|d| d == &p) {
                    dirs.push(p);
                }
            }
        }
    }
    dirs
}

/// 合并已启用插件目录到当前进程 PATH 值。
pub fn enabled_path_env() -> Option<(String, std::ffi::OsString)> {
    let dirs = enabled_bin_dirs();
    if dirs.is_empty() {
        return None;
    }
    let key = shell_path_env_key();
    let mut next = std::ffi::OsString::new();
    for (i, dir) in dirs.iter().enumerate() {
        if i > 0 {
            next.push(path_sep());
        }
        next.push(dir.as_os_str());
    }
    let existing =
        std::env::var_os(if cfg!(windows) { "Path" } else { "PATH" }).unwrap_or_default();
    if !existing.is_empty() {
        next.push(path_sep());
        next.push(existing);
    }
    Some((key, next))
}

fn shell_path_env_key() -> String {
    #[cfg(windows)]
    {
        "Path".to_string()
    }
    #[cfg(not(windows))]
    {
        "PATH".to_string()
    }
}

fn path_sep() -> &'static str {
    #[cfg(windows)]
    {
        ";"
    }
    #[cfg(not(windows))]
    {
        ":"
    }
}

/// 已启用插件的附属 Skill 扫描根（每个 skill 目录内含 SKILL.md）。
/// 关闭插件后不再返回，registry 下次构建即消失。
///
/// **兼容性：** 不在每次扫描时全量 `load_skill`（OfficeCLI 十几个 skill 会反复拉起 CLI、拖慢工具列表）。
/// 仅当声明的 skill 文件不齐时才补同步；完整同步发生在插件「启用」时。
pub fn enabled_skill_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for plugin in PLUGIN_CATALOG {
        if !is_enabled(plugin.id) || !is_installed(plugin.id) {
            continue;
        }
        if !plugin_skill_files_complete(plugin) {
            if let Err(err) = super::install::write_skill_files(plugin) {
                eprintln!("[plugins] sync skills for {}: {err}", plugin.id);
            }
        }
        for skill_id in plugin.skill_ids {
            if let Some(dir) = skill_dir(plugin.id, skill_id) {
                if dir.join("SKILL.md").is_file() {
                    roots.push(dir);
                }
            }
        }
    }
    roots
}

fn plugin_skill_files_complete(plugin: &super::catalog::CatalogPlugin) -> bool {
    if plugin.skill_ids.is_empty() {
        return true;
    }
    plugin.skill_ids.iter().all(|skill_id| {
        skill_dir(plugin.id, skill_id)
            .map(|d| d.join("SKILL.md").is_file())
            .unwrap_or(false)
    })
}

/// 已启用插件的 system 适配提示：统一外壳 + 各插件一段（仅已安装且启用）。
/// 多插件时按 catalog 顺序拼接；关闭则零注入。
pub fn enabled_system_prompt() -> Option<String> {
    let mut parts = Vec::new();
    for plugin in PLUGIN_CATALOG {
        if !is_enabled(plugin.id) {
            continue;
        }
        if !is_installed(plugin.id) {
            continue;
        }
        let hint = plugin.system_hint.trim();
        if !hint.is_empty() {
            parts.push(hint.to_string());
        }
    }
    if parts.is_empty() {
        None
    } else {
        let mut out = String::from(
            "[Kivio Plugins]\n\
The following capability plugins are enabled. Prefer their declared entry points over ad-hoc alternatives. \
Do not re-install them or write MCP config for third-party IDEs.",
        );
        out.push_str("\n\n");
        out.push_str(&parts.join("\n\n"));
        Some(out)
    }
}

pub fn probe_version(binary: &Path) -> Option<String> {
    let output = Command::new(binary)
        .arg("--version")
        .no_console_window()
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
}

pub fn default_binary_filename(id: &str) -> Option<String> {
    catalog_plugin(id).map(binary_filename)
}
