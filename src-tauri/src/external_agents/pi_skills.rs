use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::external_agents::pi_extensions::{
    agent_dir, chat_pi_extensions_inventory, lock_settings_file, open_path,
    package_source as package_source_from_value, read_settings, write_settings,
};
use crate::skills::parse::split_frontmatter;

const SKILL_READ_MAX_BYTES: u64 = 1_048_576;
const WALK_ENTRY_LIMIT: usize = 20_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiSkillInventory {
    pub agent_dir: String,
    pub pi_skills_dir: String,
    pub agents_skills_dir: String,
    pub skill_commands_enabled: bool,
    pub configured_paths: Vec<PiSkillConfiguredPath>,
    pub skills: Vec<PiSkillEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiSkillConfiguredPath {
    pub path: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiSkillEntry {
    pub name: String,
    pub description: Option<String>,
    pub path: String,
    pub source_kind: String,
    pub package_source: Option<String>,
    pub package_root: Option<String>,
    pub enabled: bool,
    pub can_toggle: bool,
    pub can_remove: bool,
}

fn home_dir() -> Result<PathBuf, String> {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .ok_or_else(|| "无法解析用户主目录".to_string())
}

fn agents_skills_dir() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".agents").join("skills"))
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn resolve_setting_path(value: &str, agent_dir: &Path) -> Result<PathBuf, String> {
    if value == "~" {
        return home_dir();
    }
    if let Some(relative) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        return Ok(home_dir()?.join(relative));
    }
    let path = PathBuf::from(value);
    Ok(if path.is_absolute() {
        path
    } else {
        agent_dir.join(path)
    })
}
fn normalized_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn normalized_pattern(pattern: &str) -> String {
    pattern
        .strip_prefix("./")
        .or_else(|| pattern.strip_prefix(".\\"))
        .unwrap_or(pattern)
        .replace('\\', "/")
}

fn is_pattern(value: &str) -> bool {
    value.starts_with(['!', '+', '-']) || value.contains(['*', '?'])
}

fn skill_settings(root: &Map<String, Value>) -> Result<Vec<String>, String> {
    match root.get("skills") {
        None => Ok(Vec::new()),
        Some(Value::Array(entries)) => entries
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| "Pi settings.json 的 skills 必须是字符串数组".to_string())
            })
            .collect(),
        Some(_) => Err("Pi settings.json 的 skills 必须是数组".to_string()),
    }
}

fn glob_regex(pattern: &str) -> Option<Regex> {
    let pattern = normalized_pattern(pattern);
    let mut output = String::from("^");
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                output.push_str(".*");
            }
            '*' => output.push_str("[^/]*"),
            '?' => output.push_str("[^/]"),
            '/' => output.push('/'),
            other => output.push_str(&regex::escape(&other.to_string())),
        }
    }
    output.push('$');
    Regex::new(&output).ok()
}

fn pattern_matches(path: &Path, pattern: &str, base_dir: &Path) -> bool {
    let rel = path
        .strip_prefix(base_dir)
        .map(normalized_path)
        .unwrap_or_else(|_| normalized_path(path));
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let absolute = normalized_path(path);
    let mut candidates = vec![rel, file_name, absolute];
    if path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
        if let Some(parent) = path.parent() {
            candidates.push(
                parent
                    .strip_prefix(base_dir)
                    .map(normalized_path)
                    .unwrap_or_else(|_| normalized_path(parent)),
            );
            candidates.push(
                parent
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default(),
            );
            candidates.push(normalized_path(parent));
        }
    }
    let Some(regex) = glob_regex(pattern) else {
        return false;
    };
    candidates.iter().any(|candidate| regex.is_match(candidate))
}

fn exact_pattern_matches(path: &Path, pattern: &str, base_dir: &Path) -> bool {
    let pattern = normalized_pattern(pattern);
    let rel = path
        .strip_prefix(base_dir)
        .map(normalized_path)
        .unwrap_or_else(|_| normalized_path(path));
    if pattern == rel || pattern == normalized_path(path) {
        return true;
    }
    if path.file_name().and_then(|name| name.to_str()) != Some("SKILL.md") {
        return false;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    pattern
        == parent
            .strip_prefix(base_dir)
            .map(normalized_path)
            .unwrap_or_else(|_| normalized_path(parent))
        || pattern == normalized_path(parent)
}

fn enabled_by_overrides(path: &Path, patterns: &[String], base_dir: &Path) -> bool {
    let mut enabled = true;
    for pattern in patterns.iter().filter(|entry| entry.starts_with('!')) {
        if pattern_matches(path, &pattern[1..], base_dir) {
            enabled = false;
        }
    }
    for pattern in patterns.iter().filter(|entry| entry.starts_with('+')) {
        if exact_pattern_matches(path, &pattern[1..], base_dir) {
            enabled = true;
        }
    }
    for pattern in patterns.iter().filter(|entry| entry.starts_with('-')) {
        if exact_pattern_matches(path, &pattern[1..], base_dir) {
            enabled = false;
        }
    }
    enabled
}

fn enabled_by_patterns(path: &Path, patterns: &[String], base_dir: &Path) -> bool {
    let includes: Vec<&str> = patterns
        .iter()
        .filter(|entry| !entry.starts_with(['!', '+', '-']))
        .map(String::as_str)
        .collect();
    let mut enabled = includes.is_empty()
        || includes
            .iter()
            .any(|pattern| pattern_matches(path, pattern, base_dir));
    for pattern in patterns.iter().filter(|entry| entry.starts_with('!')) {
        if pattern_matches(path, &pattern[1..], base_dir) {
            enabled = false;
        }
    }
    for pattern in patterns.iter().filter(|entry| entry.starts_with('+')) {
        if exact_pattern_matches(path, &pattern[1..], base_dir) {
            enabled = true;
        }
    }
    for pattern in patterns.iter().filter(|entry| entry.starts_with('-')) {
        if exact_pattern_matches(path, &pattern[1..], base_dir) {
            enabled = false;
        }
    }
    enabled
}

fn package_skill_enabled(path: &Path, filter: Option<&[String]>, package_root: &Path) -> bool {
    match filter {
        None => true,
        Some([]) => false,
        Some(patterns) => enabled_by_patterns(path, patterns, package_root),
    }
}
fn read_skill_meta(path: &Path) -> (String, Option<String>) {
    let fallback = if path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
        path.parent()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "skill".to_string())
    } else {
        path.file_stem()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "skill".to_string())
    };
    if std::fs::metadata(path)
        .map(|metadata| metadata.len() > SKILL_READ_MAX_BYTES)
        .unwrap_or(true)
    {
        return (fallback, None);
    }
    let Ok(raw) = std::fs::read_to_string(path) else {
        return (fallback, None);
    };
    let (frontmatter, _) = split_frontmatter(&raw);
    let name = frontmatter
        .get("name")
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or(fallback);
    let description = frontmatter
        .get("description")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    (name, description)
}

fn collect_skill_entries(root: &Path) -> Vec<PathBuf> {
    let mut output = Vec::new();
    let mut visited = 0usize;
    collect_skill_entries_inner(root, true, &mut visited, &mut output);
    output.sort();
    output.dedup();
    output
}

fn collect_skill_entries_inner(
    dir: &Path,
    include_root_markdown: bool,
    visited: &mut usize,
    output: &mut Vec<PathBuf>,
) {
    if *visited >= WALK_ENTRY_LIMIT || !dir.is_dir() {
        return;
    }
    let skill_file = dir.join("SKILL.md");
    if skill_file.is_file() {
        output.push(skill_file);
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        *visited += 1;
        if *visited >= WALK_ENTRY_LIMIT {
            return;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(name.as_ref(), ".git" | "node_modules") {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_skill_entries_inner(&path, false, visited, output);
        } else if include_root_markdown
            && file_type.is_file()
            && path.extension().and_then(|ext| ext.to_str()) == Some("md")
        {
            output.push(path);
        }
    }
}

fn collect_path_skill_entries(path: &Path) -> Vec<PathBuf> {
    if path.is_dir() {
        collect_skill_entries(path)
    } else if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("md") {
        vec![path.to_path_buf()]
    } else {
        Vec::new()
    }
}

fn walk_paths(root: &Path, output: &mut Vec<PathBuf>, visited: &mut usize) {
    if *visited >= WALK_ENTRY_LIMIT || !root.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        *visited += 1;
        if *visited >= WALK_ENTRY_LIMIT {
            return;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(name.as_ref(), ".git" | "node_modules") {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        output.push(path.clone());
        if file_type.is_dir() {
            walk_paths(&path, output, visited);
        }
    }
}

fn manifest_skill_entries(package_root: &Path) -> Vec<PathBuf> {
    let manifest = std::fs::read_to_string(package_root.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let configured = manifest
        .as_ref()
        .and_then(|value| value.get("pi"))
        .and_then(|value| value.get("skills"))
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        });
    let Some(entries) = configured.filter(|entries| !entries.is_empty()) else {
        return collect_skill_entries(&package_root.join("skills"));
    };

    let mut files = Vec::new();
    for entry in entries
        .iter()
        .filter(|entry| !entry.starts_with(['!', '+', '-']))
    {
        if entry.contains(['*', '?']) {
            let mut candidates = Vec::new();
            let mut visited = 0usize;
            walk_paths(package_root, &mut candidates, &mut visited);
            for candidate in candidates
                .into_iter()
                .filter(|candidate| pattern_matches(candidate, entry, package_root))
            {
                files.extend(collect_path_skill_entries(&candidate));
            }
        } else {
            files.extend(collect_path_skill_entries(&package_root.join(entry)));
        }
    }
    files.sort();
    files.dedup();
    let overrides: Vec<String> = entries
        .into_iter()
        .filter(|entry| entry.starts_with(['!', '+', '-']))
        .collect();
    files.retain(|path| enabled_by_patterns(path, &overrides, package_root));
    files
}

fn package_skill_filter<'a>(settings: &'a Map<String, Value>, source: &str) -> Option<&'a Value> {
    settings
        .get("packages")
        .and_then(Value::as_array)
        .and_then(|packages| {
            packages
                .iter()
                .find(|entry| package_source_from_value(entry) == Some(source))
        })
}

fn path_pattern(path: &Path, base_dir: &Path) -> String {
    path.strip_prefix(base_dir)
        .map(normalized_path)
        .unwrap_or_else(|_| normalized_path(path))
}

fn local_skill_can_remove(path: &Path, root: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) != Some("SKILL.md")
        || path.parent() != Some(root)
}
fn add_skill(
    output: &mut Vec<PiSkillEntry>,
    path: PathBuf,
    source_kind: &str,
    package_source_value: Option<String>,
    package_root_value: Option<String>,
    enabled: bool,
    can_toggle: bool,
    can_remove: bool,
) {
    let (name, description) = read_skill_meta(&path);
    output.push(PiSkillEntry {
        name,
        description,
        path: path_text(&path),
        source_kind: source_kind.to_string(),
        package_source: package_source_value,
        package_root: package_root_value,
        enabled,
        can_toggle,
        can_remove,
    });
}

async fn inventory_at(agent_dir: &Path) -> Result<PiSkillInventory, String> {
    let settings = read_settings(agent_dir)?;
    let setting_entries = skill_settings(&settings)?;
    let pi_dir = agent_dir.join("skills");
    let agents_dir = agents_skills_dir()?;
    let agents_base = agents_dir
        .parent()
        .ok_or_else(|| "无法解析 .agents 目录".to_string())?;
    let mut skills = Vec::new();

    for path in collect_skill_entries(&pi_dir) {
        add_skill(
            &mut skills,
            path.clone(),
            "pi",
            None,
            None,
            enabled_by_overrides(&path, &setting_entries, agent_dir),
            true,
            local_skill_can_remove(&path, &pi_dir),
        );
    }
    for path in collect_skill_entries(&agents_dir) {
        add_skill(
            &mut skills,
            path.clone(),
            "agents",
            None,
            None,
            enabled_by_overrides(&path, &setting_entries, agents_base),
            true,
            local_skill_can_remove(&path, &agents_dir),
        );
    }

    let configured_values: Vec<String> = setting_entries
        .iter()
        .filter(|entry| !is_pattern(entry))
        .cloned()
        .collect();
    let configured_paths: Vec<PiSkillConfiguredPath> = configured_values
        .iter()
        .map(|entry| {
            let path = resolve_setting_path(entry, agent_dir)?;
            Ok(PiSkillConfiguredPath {
                path: path_text(&path),
                exists: path.exists(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let custom_patterns: Vec<String> = setting_entries
        .iter()
        .filter(|entry| is_pattern(entry))
        .cloned()
        .collect();
    let mut configured_files = Vec::new();
    for configured in &configured_paths {
        configured_files.extend(collect_path_skill_entries(Path::new(&configured.path)));
    }
    configured_files.sort();
    configured_files.dedup();
    for path in configured_files {
        add_skill(
            &mut skills,
            path.clone(),
            "configured",
            None,
            None,
            enabled_by_patterns(&path, &custom_patterns, agent_dir),
            true,
            false,
        );
    }

    let package_inventory = chat_pi_extensions_inventory().await?;
    for package in package_inventory.packages {
        let Some(package_path) = package.path.as_deref().map(PathBuf::from) else {
            continue;
        };
        let Some(setting) = package_skill_filter(&settings, &package.source) else {
            continue;
        };
        let autoload_disabled = setting
            .as_object()
            .and_then(|object| object.get("autoload"))
            .and_then(Value::as_bool)
            == Some(false);
        let filter = setting
            .as_object()
            .and_then(|object| object.get("skills"))
            .map(|value| {
                value
                    .as_array()
                    .ok_or_else(|| "Pi Package 的 skills 过滤必须是数组".to_string())?
                    .iter()
                    .map(|entry| {
                        entry
                            .as_str()
                            .map(ToOwned::to_owned)
                            .ok_or_else(|| "Pi Package 的 skills 过滤必须是字符串数组".to_string())
                    })
                    .collect::<Result<Vec<_>, String>>()
            })
            .transpose()?;
        for path in manifest_skill_entries(&package_path) {
            let enabled = package_skill_enabled(&path, filter.as_deref(), &package_path);
            add_skill(
                &mut skills,
                path,
                "package",
                Some(package.source.clone()),
                Some(path_text(&package_path)),
                enabled,
                !autoload_disabled,
                false,
            );
        }
    }

    let mut seen = HashSet::new();
    skills.retain(|entry| seen.insert((entry.path.clone(), entry.package_source.clone())));
    skills.sort_by(|left, right| {
        left.source_kind
            .cmp(&right.source_kind)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });

    Ok(PiSkillInventory {
        agent_dir: path_text(agent_dir),
        pi_skills_dir: path_text(&pi_dir),
        agents_skills_dir: path_text(&agents_dir),
        skill_commands_enabled: settings
            .get("enableSkillCommands")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        configured_paths,
        skills,
    })
}

fn update_exact_pattern(entries: &mut Vec<String>, pattern: &str, enabled: bool) {
    let normalized = normalized_pattern(pattern);
    entries.retain(|entry| {
        if !entry.starts_with(['+', '-']) {
            return true;
        }
        normalized_pattern(&entry[1..]) != normalized
    });
    entries.push(format!("{}{pattern}", if enabled { '+' } else { '-' }));
}

fn package_root_for_skill(entry: &PiSkillEntry) -> Result<PathBuf, String> {
    entry
        .package_root
        .as_deref()
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .ok_or_else(|| "无法解析 Package Skill 安装目录".to_string())
}

async fn find_inventory_skill(
    path: &str,
    package_source_value: Option<&str>,
) -> Result<PiSkillEntry, String> {
    let inventory = inventory_at(&agent_dir()?).await?;
    inventory
        .skills
        .into_iter()
        .find(|entry| entry.path == path && entry.package_source.as_deref() == package_source_value)
        .ok_or_else(|| "该 Pi Skill 已不存在".to_string())
}

#[tauri::command]
pub async fn chat_pi_skills_inventory() -> Result<PiSkillInventory, String> {
    inventory_at(&agent_dir()?).await
}

#[tauri::command]
pub async fn chat_pi_skill_set_enabled(
    path: String,
    package_source: Option<String>,
    enabled: bool,
) -> Result<(), String> {
    let skill = find_inventory_skill(&path, package_source.as_deref()).await?;
    if !skill.can_toggle {
        return Err("该 Package 使用 autoload:false，请用 pi config 管理".to_string());
    }
    let agent_dir = agent_dir()?;
    let settings_path = agent_dir.join("settings.json");
    let _lock = lock_settings_file(&settings_path)?;
    let mut settings = read_settings(&agent_dir)?;

    if skill.source_kind == "package" {
        let source = skill
            .package_source
            .as_deref()
            .ok_or_else(|| "Package Skill 缺少来源".to_string())?;
        let packages = settings
            .entry("packages".to_string())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| "Pi settings.json 的 packages 必须是数组".to_string())?;
        let package = packages
            .iter_mut()
            .find(|entry| package_source_from_value(entry) == Some(source))
            .ok_or_else(|| "该 Pi Package 已不在 settings.json 中".to_string())?;
        if let Value::String(value) = package {
            let mut object = Map::new();
            object.insert("source".to_string(), Value::String(value.clone()));
            *package = Value::Object(object);
        }
        let object = package
            .as_object_mut()
            .ok_or_else(|| "Pi Package 配置必须是字符串或对象".to_string())?;
        let had_filter = object.contains_key("skills");
        let mut filters = match object.remove("skills") {
            Some(Value::Array(entries)) => entries
                .into_iter()
                .map(|entry| {
                    entry
                        .as_str()
                        .map(ToOwned::to_owned)
                        .ok_or_else(|| "Pi Package 的 skills 必须是字符串数组".to_string())
                })
                .collect::<Result<Vec<_>, String>>()?,
            Some(_) => return Err("Pi Package 的 skills 必须是数组".to_string()),
            None => Vec::new(),
        };
        let package_root = package_root_for_skill(&skill)?;
        let pattern = path_pattern(Path::new(&skill.path), &package_root);
        if had_filter && filters.is_empty() {
            if enabled {
                filters.push(pattern);
            }
        } else {
            update_exact_pattern(&mut filters, &pattern, enabled);
        }
        object.insert(
            "skills".to_string(),
            Value::Array(filters.into_iter().map(Value::String).collect()),
        );
    } else {
        let mut entries = skill_settings(&settings)?;
        let base_dir = if skill.source_kind == "agents" {
            agents_skills_dir()?
                .parent()
                .ok_or_else(|| "无法解析 .agents 目录".to_string())?
                .to_path_buf()
        } else {
            agent_dir.clone()
        };
        let pattern = path_pattern(Path::new(&skill.path), &base_dir);
        update_exact_pattern(&mut entries, &pattern, enabled);
        settings.insert(
            "skills".to_string(),
            Value::Array(entries.into_iter().map(Value::String).collect()),
        );
    }
    write_settings(&agent_dir, &settings)
}

#[tauri::command]
pub fn chat_pi_skill_commands_set_enabled(enabled: bool) -> Result<(), String> {
    let agent_dir = agent_dir()?;
    let _lock = lock_settings_file(&agent_dir.join("settings.json"))?;
    let mut settings = read_settings(&agent_dir)?;
    settings.insert("enableSkillCommands".to_string(), Value::Bool(enabled));
    write_settings(&agent_dir, &settings)
}

#[tauri::command]
pub fn chat_pi_skill_add_path(path: String) -> Result<(), String> {
    if path.trim().is_empty() || path.contains(['\r', '\n', '\0']) {
        return Err("Skill 扫描路径无效".to_string());
    }
    let candidate = PathBuf::from(path.trim());
    if !candidate.is_absolute() || (!candidate.is_dir() && !candidate.is_file()) {
        return Err("请选择存在的绝对目录或 Markdown 文件".to_string());
    }
    if candidate.is_file()
        && candidate
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("md")
    {
        return Err("Skill 文件必须是 Markdown".to_string());
    }
    let canonical = std::fs::canonicalize(&candidate)
        .map_err(|err| format!("解析 Skill 扫描路径失败：{err}"))?;
    let agent_dir = agent_dir()?;
    let _lock = lock_settings_file(&agent_dir.join("settings.json"))?;
    let mut settings = read_settings(&agent_dir)?;
    let mut entries = skill_settings(&settings)?;
    let canonical_text = path_text(&canonical);
    if !entries.iter().any(|entry| {
        !is_pattern(entry)
            && resolve_setting_path(entry, &agent_dir)
                .ok()
                .and_then(|path| std::fs::canonicalize(path).ok())
                .is_some_and(|existing| existing == canonical)
    }) {
        entries.push(canonical_text);
    }
    settings.insert(
        "skills".to_string(),
        Value::Array(entries.into_iter().map(Value::String).collect()),
    );
    write_settings(&agent_dir, &settings)
}

#[tauri::command]
pub fn chat_pi_skill_remove_path(path: String) -> Result<(), String> {
    let agent_dir = agent_dir()?;
    let _lock = lock_settings_file(&agent_dir.join("settings.json"))?;
    let mut settings = read_settings(&agent_dir)?;
    let mut entries = skill_settings(&settings)?;
    let before = entries.len();
    entries.retain(|entry| {
        if is_pattern(entry) {
            return true;
        }
        let resolved =
            resolve_setting_path(entry, &agent_dir).unwrap_or_else(|_| agent_dir.join(entry));
        path_text(&resolved) != path
    });
    if entries.len() == before {
        return Err("该 Skill 扫描路径已不存在".to_string());
    }
    settings.insert(
        "skills".to_string(),
        Value::Array(entries.into_iter().map(Value::String).collect()),
    );
    write_settings(&agent_dir, &settings)
}

fn ensure_under(path: &Path, root: &Path) -> Result<PathBuf, String> {
    let canonical =
        std::fs::canonicalize(path).map_err(|err| format!("解析 Skill 路径失败：{err}"))?;
    let canonical_root =
        std::fs::canonicalize(root).map_err(|err| format!("解析 Skill 根目录失败：{err}"))?;
    if !canonical.starts_with(&canonical_root)
        || canonical
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("Skill 路径超出允许的全局目录".to_string());
    }
    Ok(canonical)
}

#[tauri::command]
pub async fn chat_pi_skill_remove(
    path: String,
    package_source: Option<String>,
) -> Result<(), String> {
    let skill = find_inventory_skill(&path, package_source.as_deref()).await?;
    if !skill.can_remove || !matches!(skill.source_kind.as_str(), "pi" | "agents") {
        return Err("只能删除全局本地 Skill".to_string());
    }
    let root = if skill.source_kind == "pi" {
        agent_dir()?.join("skills")
    } else {
        agents_skills_dir()?
    };
    let skill_file = ensure_under(Path::new(&skill.path), &root)?;
    let target = if skill_file.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
        skill_file
            .parent()
            .ok_or_else(|| "Skill 目录无效".to_string())?
            .to_path_buf()
    } else {
        skill_file
    };
    let canonical_root =
        std::fs::canonicalize(&root).map_err(|err| format!("解析 Skill 根目录失败：{err}"))?;
    if target == canonical_root {
        return Err("不能删除 Pi 的全局 Skill 根目录".to_string());
    }
    if target.is_dir() {
        std::fs::remove_dir_all(&target).map_err(|err| format!("删除 Skill 目录失败：{err}"))?;
    } else {
        std::fs::remove_file(&target).map_err(|err| format!("删除 Skill 文件失败：{err}"))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn chat_pi_skill_open(
    path: String,
    package_source: Option<String>,
) -> Result<(), String> {
    let skill = find_inventory_skill(&path, package_source.as_deref()).await?;
    let file = PathBuf::from(skill.path);
    open_path(file.parent().unwrap_or(&file))
}

#[tauri::command]
pub fn chat_pi_skills_open_dir(kind: String) -> Result<(), String> {
    let dir = match kind.as_str() {
        "pi" => agent_dir()?.join("skills"),
        "agents" => agents_skills_dir()?,
        _ => return Err("未知的 Pi Skill 目录类型".to_string()),
    };
    std::fs::create_dir_all(&dir).map_err(|err| format!("创建 Skill 目录失败：{err}"))?;
    open_path(&dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kivio-pi-skills-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn discovery_matches_pi_root_markdown_and_recursive_skill_dirs() {
        let root = temp_dir("discover");
        std::fs::write(
            root.join("root.md"),
            "---\nname: root\ndescription: root\n---\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("nested/demo")).unwrap();
        std::fs::write(
            root.join("nested/demo/SKILL.md"),
            "---\nname: demo\ndescription: demo\n---\n",
        )
        .unwrap();
        std::fs::write(root.join("nested/ignored.md"), "ignored").unwrap();

        let entries = collect_skill_entries(&root);
        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&root.join("root.md")));
        assert!(entries.contains(&root.join("nested/demo/SKILL.md")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn root_skill_manifest_cannot_remove_the_global_skills_directory() {
        let root = temp_dir("root-delete");
        assert!(!local_skill_can_remove(&root.join("SKILL.md"), &root));
        assert!(local_skill_can_remove(&root.join("demo/SKILL.md"), &root));
        assert!(local_skill_can_remove(&root.join("demo.md"), &root));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn override_matching_accepts_skill_file_and_parent_directory() {
        let root = temp_dir("patterns");
        let skill = root.join("skills/demo/SKILL.md");
        std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
        std::fs::write(&skill, "---\nname: demo\ndescription: demo\n---\n").unwrap();

        assert!(!enabled_by_overrides(
            &skill,
            &["-skills/demo".to_string()],
            &root
        ));
        assert!(enabled_by_overrides(
            &skill,
            &[
                "!skills/**".to_string(),
                "+skills/demo/SKILL.md".to_string()
            ],
            &root
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn exact_pattern_update_preserves_scan_paths_and_other_overrides() {
        let mut entries = vec![
            "C:/shared/skills".to_string(),
            "!skills/legacy/**".to_string(),
            "-skills/demo/SKILL.md".to_string(),
        ];
        update_exact_pattern(&mut entries, "skills/demo/SKILL.md", true);
        assert_eq!(entries[0], "C:/shared/skills");
        assert_eq!(entries[1], "!skills/legacy/**");
        assert_eq!(entries[2], "+skills/demo/SKILL.md");
    }

    #[test]
    fn package_patterns_can_enable_one_skill_from_an_include_filter() {
        let root = temp_dir("package-filter");
        let demo = root.join("skills/demo/SKILL.md");
        let other = root.join("skills/other/SKILL.md");
        std::fs::create_dir_all(demo.parent().unwrap()).unwrap();
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&demo, "demo").unwrap();
        std::fs::write(&other, "other").unwrap();
        let patterns = vec!["skills/other/**".to_string(), "+skills/demo".to_string()];
        assert!(enabled_by_patterns(&demo, &patterns, &root));
        assert!(enabled_by_patterns(&other, &patterns, &root));
        let empty = Vec::<String>::new();
        assert!(!package_skill_enabled(&demo, Some(&empty), &root));
        assert!(package_skill_enabled(&demo, None, &root));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn package_manifest_skill_directory_is_discovered() {
        let root = temp_dir("package-manifest");
        let skill = root.join("skills/demo/SKILL.md");
        std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
        std::fs::write(&skill, "---\nname: demo\ndescription: demo\n---\n").unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"pi":{"skills":["./skills"]}}"#,
        )
        .unwrap();

        assert_eq!(manifest_skill_entries(&root), vec![skill]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn setting_paths_expand_the_home_prefix() {
        let root = temp_dir("setting-path");
        let resolved = resolve_setting_path("~/.codex/skills", &root).unwrap();
        assert_eq!(resolved, home_dir().unwrap().join(".codex/skills"));
        let _ = std::fs::remove_dir_all(root);
    }
}
