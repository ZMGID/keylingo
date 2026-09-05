//! Manifest-based, user-scoped plugins. Package files are copied into Kivio ownership;
//! importing never executes an installer. Existing curated CLI plugins are independent.
use crate::{
    chat::workflow_hooks::{self, Hook},
    settings::ChatMcpServer,
    state::AppState,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};
use tauri::{AppHandle, State};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Package {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub format: String,
    pub source: String,
    pub revision: Option<String>,
    pub enabled: bool,
    pub components: BTreeMap<String, usize>,
    pub diagnostics: Vec<String>,
}
pub struct Resolved {
    pub package: Package,
    pub root: PathBuf,
    pub skills: Vec<PathBuf>,
    pub commands: Vec<PathBuf>,
    pub agents: Vec<PathBuf>,
    pub hooks: Vec<Hook>,
    pub servers: Vec<ChatMcpServer>,
}
fn packages_root() -> Result<PathBuf, String> {
    super::plugins_root()
        .map(|p| p.join("packages"))
        .ok_or("Application data directory unavailable".into())
}
fn package_dir(id: &str) -> Result<PathBuf, String> {
    uuid::Uuid::parse_str(id).map_err(|_| "Invalid package id")?;
    Ok(packages_root()?.join(id))
}
fn read_json(path: &Path) -> Result<Value, String> {
    if fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .len()
        > 1024 * 1024
    {
        return Err("JSON file exceeds 1 MiB".into());
    }
    serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("{}: {e}", path.display()))
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    fs::write(
        &tmp,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}
pub fn contained(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("Component path must stay inside the plugin".into());
    }
    let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let target = fs::canonicalize(root.join(relative)).map_err(|e| e.to_string())?;
    if !target.starts_with(&root) {
        return Err("Plugin path escapes its root".into());
    }
    Ok(target)
}
fn paths(root: &Path, value: Option<&Value>, default: &str) -> Result<Vec<PathBuf>, String> {
    let values = match value {
        None => {
            return Ok(if root.join(default).exists() {
                vec![contained(root, default)?]
            } else {
                vec![]
            })
        }
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| v.as_str().ok_or("Component paths must be strings"))
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("Component must be a path or array of paths".into()),
    };
    values.into_iter().map(|v| contained(root, v)).collect()
}
fn configs(root: &Path, value: Option<&Value>, default: &str) -> Result<Vec<Value>, String> {
    match value {
        None => {
            if root.join(default).exists() {
                Ok(vec![read_json(&contained(root, default)?)?])
            } else {
                Ok(vec![])
            }
        }
        Some(Value::String(path)) => Ok(vec![read_json(&contained(root, path)?)?]),
        Some(Value::Array(items)) => {
            let mut out = Vec::new();
            for item in items {
                out.extend(configs(root, Some(item), default)?);
            }
            Ok(out)
        }
        Some(v) if v.is_object() => Ok(vec![v.clone()]),
        _ => Err("Configuration must be a path, object, or array".into()),
    }
}
/// Native v1 deliberately uses path declarations, keeping executable configuration
/// in component files and rejecting misspelled or unsupported manifest fields.
fn validate_native_manifest(value: &Value) -> Result<(), String> {
    let fields = value
        .as_object()
        .ok_or("Kivio manifest must be an object")?;
    for key in fields.keys() {
        if ![
            "$schema",
            "schemaVersion",
            "name",
            "version",
            "description",
            "metadata",
            "skills",
            "commands",
            "agents",
            "hooks",
            "mcpServers",
        ]
        .contains(&key.as_str())
        {
            return Err(format!("Unknown Kivio v1 manifest field: {key}"));
        }
    }
    if value.get("schemaVersion").and_then(Value::as_u64) != Some(1) {
        return Err("Kivio schemaVersion must be 1".into());
    }
    let name = value.get("name").and_then(Value::as_str).unwrap_or("");
    if name.is_empty()
        || name.len() > 64
        || !name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
    {
        return Err(
            "Kivio name must be 1–64 lowercase letters/digits separated by single hyphens".into(),
        );
    }
    if value
        .get("version")
        .and_then(Value::as_str)
        .is_none_or(|s| s.trim().is_empty())
    {
        return Err("Kivio version must be a nonempty string".into());
    }
    for key in ["$schema", "description"] {
        if value.get(key).is_some_and(|v| !v.is_string()) {
            return Err(format!("{key} must be a string"));
        }
    }
    if value.get("metadata").is_some_and(|v| !v.is_object()) {
        return Err("metadata must be an object".into());
    }
    for key in ["skills", "commands", "agents", "hooks", "mcpServers"] {
        if let Some(v) = value.get(key) {
            let entries = if let Some(items) = v.as_array() {
                items.iter().collect()
            } else {
                vec![v]
            };
            for entry in entries {
                let path = entry
                    .as_str()
                    .ok_or_else(|| format!("{key} must be a path or array of paths"))?;
                if !path.starts_with("./")
                    || path.len() <= 2
                    || path.contains('\\')
                    || path.contains(':')
                    || path.split('/').any(|p| p == "..")
                {
                    return Err(format!("{key} paths must start with ./ and stay inside the plugin using forward slashes"));
                }
            }
        }
    }
    Ok(())
}

pub fn resolve(root: &Path, mut package: Package, data: &Path) -> Result<Resolved, String> {
    let (format, manifest) = if root.join(".kivio-plugin/plugin.json").is_file() {
        ("kivio", ".kivio-plugin/plugin.json")
    } else if root.join(".codex-plugin/plugin.json").is_file() {
        ("codex", ".codex-plugin/plugin.json")
    } else if root.join(".claude-plugin/plugin.json").is_file() {
        ("claude", ".claude-plugin/plugin.json")
    } else {
        return Err(
            "No .kivio-plugin/plugin.json, .codex-plugin/plugin.json or .claude-plugin/plugin.json at the selected root"
                .into(),
        );
    };
    let manifest = read_json(&contained(root, manifest)?)?;
    if format == "kivio" {
        validate_native_manifest(&manifest)?;
    }
    package.format = format.into();
    package.name = manifest
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("Plugin name is required")?
        .into();
    if !package
        .name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("Plugin name must contain only letters, numbers and hyphens".into());
    }
    package.description = manifest
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .into();
    package.version = manifest
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_owned);
    package.diagnostics.clear();
    for key in [
        "apps",
        "lspServers",
        "outputStyles",
        "dependencies",
        "experimental",
    ] {
        if manifest.get(key).is_some() {
            package
                .diagnostics
                .push(format!("Unsupported component: {key}"));
        }
    }
    for file in [".app.json", ".lsp.json", "settings.json"] {
        if root.join(file).exists() {
            package
                .diagnostics
                .push(format!("Unsupported host configuration: {file}"));
        }
    }
    let mut skills = paths(root, manifest.get("skills"), "skills")?;
    if format == "claude" && manifest.get("skills").is_some() && root.join("skills").exists() {
        let default = contained(root, "skills")?;
        if !skills.contains(&default) {
            skills.insert(0, default);
        }
    }
    let commands = paths(root, manifest.get("commands"), "commands")?;
    let agents = paths(root, manifest.get("agents"), "agents")?;
    let mut unsupported_agents = agents.clone();
    unsupported_agents.extend(component_files_bounded(&agents, 0, "toml"));
    unsupported_agents.sort();
    unsupported_agents.dedup();
    for path in &unsupported_agents {
        if path.is_file() && path.extension().is_some_and(|e| e != "md") {
            package.diagnostics.push(format!(
                "Only Markdown agent definitions are supported: {}",
                path.display()
            ));
        }
    }
    let mut component_files = markdown_files(&commands);
    component_files.extend(markdown_files(&agents));
    component_files.extend(
        markdown_files(&skills)
            .into_iter()
            .filter(|p| p.file_name().is_some_and(|n| n == "SKILL.md")),
    );
    for file in component_files {
        if let Err(error) = component_markdown(&file) {
            package
                .diagnostics
                .push(format!("{}: {error}", file.display()));
        }
    }
    let mut hooks = Vec::new();
    let mut hook_configs = configs(root, manifest.get("hooks"), "hooks/hooks.json")?;
    if format == "claude"
        && manifest.get("hooks").is_some()
        && root.join("hooks/hooks.json").exists()
    {
        let default = read_json(&contained(root, "hooks/hooks.json")?)?;
        if !hook_configs.contains(&default) {
            hook_configs.insert(0, default);
        }
    }
    for value in hook_configs {
        match workflow_hooks::parse(&value, &package.id, root, data) {
            Ok(mut h) => {
                if format == "kivio" {
                    for hook in &mut h {
                        hook.foreign_tool_names = false;
                    }
                }
                hooks.extend(h);
            }
            Err(e) => package.diagnostics.push(e),
        }
    }
    let mut env: BTreeMap<String, String> = std::env::vars().collect();
    for key in ["PLUGIN_ROOT", "CLAUDE_PLUGIN_ROOT"] {
        env.insert(key.into(), root.display().to_string());
    }
    for key in ["PLUGIN_DATA", "CLAUDE_PLUGIN_DATA"] {
        env.insert(key.into(), data.display().to_string());
    }
    let mut servers = Vec::new();
    let mut mcp_configs = configs(root, manifest.get("mcpServers"), ".mcp.json")?;
    if format == "claude" && manifest.get("mcpServers").is_some() && root.join(".mcp.json").exists()
    {
        let default = read_json(&contained(root, ".mcp.json")?)?;
        if !mcp_configs.contains(&default) {
            mcp_configs.insert(0, default);
        }
    }
    for config in mcp_configs {
        for (name, raw) in config
            .get("mcpServers")
            .or_else(|| config.get("mcp_servers"))
            .unwrap_or(&config)
            .as_object()
            .ok_or("mcpServers must be an object")?
        {
            let raw = expand_json(raw, &env);
            if raw.to_string().contains("${") {
                package
                    .diagnostics
                    .push(format!("MCP {name}: unresolved environment variable"));
                continue;
            }
            let transport = raw
                .get("type")
                .or_else(|| raw.get("transport"))
                .and_then(Value::as_str)
                .unwrap_or(if raw.get("url").is_some() {
                    "http"
                } else {
                    "stdio"
                });
            if !["stdio", "http", "streamable-http", "streamable_http"].contains(&transport) {
                package
                    .diagnostics
                    .push(format!("Unsupported MCP transport: {transport}"));
                continue;
            }
            let mut server: ChatMcpServer =
                serde_json::from_value(raw.clone()).map_err(|e| format!("MCP {name}: {e}"))?;
            server.id = format!(
                "plugin-package-{}-{}",
                package.id,
                crate::skills::slugify(name)
            );
            server.name = format!("{} / {name}", package.name);
            server.enabled = true;
            server.transport = if transport == "stdio" {
                "stdio"
            } else {
                "streamable_http"
            }
            .into();
            server.connector_id = Some(format!("plugin:package:{}", package.id));
            server.cwd = Some(root.display().to_string());
            if transport == "stdio" && server.command.trim().is_empty() {
                return Err(format!("MCP {name} requires command"));
            }
            if transport != "stdio" && server.url.trim().is_empty() {
                return Err(format!("MCP {name} requires url"));
            }
            for (key, value) in &env {
                if key.starts_with("PLUGIN_") || key.starts_with("CLAUDE_PLUGIN_") {
                    server.env.insert(key.clone(), value.clone());
                }
            }
            servers.retain(|s: &ChatMcpServer| s.id != server.id);
            servers.push(server);
        }
    }
    package.components = [
        (
            "skills",
            markdown_files(&skills)
                .iter()
                .filter(|p| p.file_name().is_some_and(|n| n == "SKILL.md"))
                .count(),
        ),
        ("commands", markdown_files(&commands).len()),
        ("agents", markdown_files(&agents).len()),
        ("hooks", hooks.len()),
        ("mcp", servers.len()),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v))
    .collect();
    Ok(Resolved {
        package,
        root: root.into(),
        skills,
        commands,
        agents,
        hooks,
        servers,
    })
}
fn expand_json(value: &Value, env: &BTreeMap<String, String>) -> Value {
    match value {
        Value::String(s) => Value::String(workflow_hooks::expand(s, env)),
        Value::Array(a) => Value::Array(a.iter().map(|v| expand_json(v, env)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), expand_json(v, env)))
                .collect(),
        ),
        _ => value.clone(),
    }
}
fn load(id: &str) -> Result<Resolved, String> {
    let dir = package_dir(id)?;
    let package: Package =
        serde_json::from_value(read_json(&dir.join("record.json"))?).map_err(|e| e.to_string())?;
    if package.id != id {
        return Err("Plugin record identity mismatch".into());
    }
    resolve(&dir.join("content"), package, &dir.join("data"))
}
pub fn active() -> Vec<Resolved> {
    let Ok(root) = packages_root() else {
        return vec![];
    };
    let Ok(entries) = fs::read_dir(root) else {
        return vec![];
    };
    let mut packages = entries
        .flatten()
        .filter_map(|entry| {
            let id = entry.file_name().to_string_lossy().into_owned();
            if !owner_enabled(&id) {
                return None;
            }
            load(&id).ok().filter(|p| p.package.diagnostics.is_empty())
        })
        .collect::<Vec<_>>();
    packages.sort_by(|a, b| a.package.id.cmp(&b.package.id));
    packages
}
pub fn owner_enabled(id: &str) -> bool {
    package_dir(id)
        .ok()
        .and_then(|dir| read_json(&dir.join("record.json")).ok())
        .is_some_and(|v| v["enabled"] == true)
}
fn list() -> Result<Vec<Package>, String> {
    let root = packages_root()?;
    if !root.exists() {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(root).map_err(|e| e.to_string())?.flatten() {
        let id = entry.file_name().to_string_lossy().into_owned();
        if uuid::Uuid::parse_str(&id).is_err() {
            continue;
        }
        match load(&id) {
            Ok(p) => out.push(p.package),
            Err(e) => {
                if let Ok(value) = read_json(&entry.path().join("record.json")) {
                    if let Ok(mut p) = serde_json::from_value::<Package>(value) {
                        p.diagnostics = vec![e];
                        out.push(p);
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(out)
}
fn copy_tree(source: &Path, target: &Path, budget: &mut u64, depth: usize) -> Result<(), String> {
    if depth > 32 {
        return Err("Package nesting exceeds 32 directories".into());
    }
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name() == ".git" || entry.file_name() == "node_modules" {
            continue;
        }
        let meta = fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() {
            return Err("Plugin imports cannot contain symlinks".into());
        }
        let canonical = fs::canonicalize(entry.path()).map_err(|e| e.to_string())?;
        if !canonical.starts_with(fs::canonicalize(source).map_err(|e| e.to_string())?) {
            return Err("Plugin junction escapes source".into());
        }
        let dest = target.join(entry.file_name());
        if meta.is_dir() {
            copy_tree(&entry.path(), &dest, budget, depth + 1)?;
        } else if meta.is_file() {
            *budget = budget
                .checked_sub(meta.len().max(1))
                .ok_or("Plugin exceeds 100 MiB import limit")?;
            fs::copy(entry.path(), dest).map_err(|e| e.to_string())?;
        } else {
            return Err("Unsupported package file type".into());
        }
    }
    Ok(())
}
#[tauri::command]
pub fn plugin_packages_list() -> Result<Vec<Package>, String> {
    list()
}

#[tauri::command]
pub async fn plugin_packages_import(
    source: String,
    subdirectory: Option<String>,
) -> Result<Package, String> {
    let root = packages_root()?;
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    let staging = root.join(format!(".staging-{id}"));
    fs::create_dir(&staging).map_err(|e| e.to_string())?;
    let result = async {
        let local = PathBuf::from(source.trim());
        let mut revision = None;
        if local.is_dir() {
            let local = fs::canonicalize(&local).map_err(|e| e.to_string())?;
            if fs::canonicalize(&root)
                .map_err(|e| e.to_string())?
                .starts_with(&local)
            {
                return Err("Source cannot contain Kivio's plugin storage".into());
            }
            let selected = match subdirectory.as_deref().filter(|s| !s.trim().is_empty()) {
                Some(relative) => contained(&local, relative)?,
                None => local,
            };
            copy_tree(
                &selected,
                &staging.join("content"),
                &mut (100 * 1024 * 1024),
                0,
            )?;
        } else {
            let url = url::Url::parse(source.trim())
                .map_err(|_| "Enter a local plugin directory or HTTPS Git repository URL")?;
            if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
                return Err("Git source must be HTTPS without embedded credentials".into());
            }
            let clone = staging.join("clone");
            let mut cmd = tokio::process::Command::new("git");
            cmd.args([
                "-c",
                "core.hooksPath=",
                "-c",
                "protocol.file.allow=never",
                "clone",
                "--depth",
                "1",
                "--",
                source.trim(),
            ])
            .arg(&clone)
            .env("GIT_TERMINAL_PROMPT", "0")
            .kill_on_drop(true);
            #[cfg(windows)]
            {
                use crate::proc::NoConsoleWindow;
                cmd.no_console_window();
            }
            let output = tokio::time::timeout(std::time::Duration::from_secs(120), cmd.output())
                .await
                .map_err(|_| "Git clone timed out")?
                .map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err(format!(
                    "Git clone failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            let mut rev = tokio::process::Command::new("git");
            rev.arg("-C")
                .arg(&clone)
                .args(["rev-parse", "HEAD"])
                .kill_on_drop(true);
            #[cfg(windows)]
            {
                use crate::proc::NoConsoleWindow;
                rev.no_console_window();
            }
            let output = rev.output().await.map_err(|e| e.to_string())?;
            if output.status.success() {
                revision = Some(String::from_utf8_lossy(&output.stdout).trim().into());
            }
            let selected = match subdirectory.as_deref().filter(|s| !s.trim().is_empty()) {
                Some(relative) => contained(&clone, relative)?,
                None => clone.clone(),
            };
            copy_tree(
                &selected,
                &staging.join("content"),
                &mut (100 * 1024 * 1024),
                0,
            )?;
            fs::remove_dir_all(&clone).map_err(|e| e.to_string())?;
        }
        let source = match subdirectory.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(dir) => format!("{} :: {dir}", source.trim()),
            None => source.trim().into(),
        };
        let package = Package {
            id: id.clone(),
            name: String::new(),
            description: String::new(),
            version: None,
            format: String::new(),
            source,
            revision,
            enabled: false,
            components: BTreeMap::new(),
            diagnostics: vec![],
        };
        let resolved = resolve(&staging.join("content"), package, &staging.join("data"))?;
        write_json(&staging.join("record.json"), &resolved.package)?;
        fs::rename(&staging, root.join(&id)).map_err(|e| e.to_string())?;
        Ok(resolved.package)
    }
    .await;
    if result.is_err() && staging.is_dir() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

#[tauri::command]
pub async fn plugin_packages_set_enabled(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<Package, String> {
    let _guard = mutation_lock().lock().await;
    set_enabled(&app, &state, &id, enabled).await
}
fn mutation_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(Default::default)
}
async fn set_enabled(
    app: &AppHandle,
    state: &AppState,
    id: &str,
    enabled: bool,
) -> Result<Package, String> {
    // Broken manifests must still be removable and disableable.
    let mut resolved = match load(id) {
        Ok(resolved) => resolved,
        Err(error) if !enabled => {
            let package: Package =
                serde_json::from_value(read_json(&package_dir(id)?.join("record.json"))?)
                    .map_err(|e| e.to_string())?;
            Resolved {
                package,
                root: package_dir(id)?.join("content"),
                skills: vec![],
                commands: vec![],
                agents: vec![],
                hooks: vec![],
                servers: vec![],
            }
            .with_diagnostic(error)
        }
        Err(error) => return Err(error),
    };
    if enabled && !resolved.package.diagnostics.is_empty() {
        return Err(format!(
            "Cannot enable plugin with unsupported or invalid components: {}",
            resolved.package.diagnostics.join("; ")
        ));
    }
    if enabled
        && active()
            .iter()
            .any(|p| p.package.id != id && p.package.name == resolved.package.name)
    {
        return Err(format!(
            "Another version/source of {} is enabled; disable it first to avoid command conflicts",
            resolved.package.name
        ));
    }
    let prefix = format!("plugin-package-{id}-");
    // Snapshot + persist while holding the settings lock: unrelated user changes are preserved.
    let old_package = resolved.package.clone();
    resolved.package.enabled = enabled;
    let disconnect;
    {
        let mut settings = state.settings_write();
        disconnect = settings
            .chat_tools
            .servers
            .iter()
            .filter(|s| s.id.starts_with(&prefix))
            .map(|s| s.id.clone())
            .collect::<Vec<_>>();
        let mut next = settings.clone();
        next.chat_tools
            .servers
            .retain(|s| !s.id.starts_with(&prefix));
        if enabled {
            next.chat_tools.enabled = true;
            next.chat_tools.native_tools.skill_runtime = true;
            next.chat_tools.servers.extend(resolved.servers);
        }
        write_json(&package_dir(&id)?.join("record.json"), &resolved.package)?;
        if let Err(e) = crate::settings::persist_settings(&app, &next) {
            let _ = write_json(&package_dir(&id)?.join("record.json"), &old_package);
            return Err(e);
        }
        *settings = next;
    }
    // IDs are deterministic; disconnect both enabled and disabled snapshots.
    for server in disconnect {
        state.mcp_disconnect_server(&server).await;
    }
    Ok(resolved.package)
}

#[tauri::command]
pub async fn plugin_packages_remove(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let _guard = mutation_lock().lock().await;
    set_enabled(&app, &state, &id, false).await?;
    let dir = package_dir(&id)?;
    let root = fs::canonicalize(packages_root()?).map_err(|e| e.to_string())?;
    let target = fs::canonicalize(&dir).map_err(|e| e.to_string())?;
    if target.parent() != Some(root.as_path()) {
        return Err("Invalid managed plugin directory".into());
    }
    fs::remove_dir_all(target).map_err(|e| e.to_string())
}

impl Resolved {
    fn with_diagnostic(mut self, error: String) -> Self {
        self.package.diagnostics = vec![error];
        self
    }
    pub fn expand_body(&self, body: &str) -> String {
        let data = self.root.parent().unwrap_or(&self.root).join("data");
        let env = [
            ("PLUGIN_ROOT", &self.root),
            ("CLAUDE_PLUGIN_ROOT", &self.root),
            ("PLUGIN_DATA", &data),
            ("CLAUDE_PLUGIN_DATA", &data),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.display().to_string()))
        .collect();
        workflow_hooks::expand(body, &env)
    }
}

fn hooks_path() -> Result<PathBuf, String> {
    super::plugins_root()
        .map(|p| p.join("workflow-hooks.json"))
        .ok_or("Application data unavailable".into())
}
#[tauri::command]
pub fn workflow_hooks_get() -> Result<Value, String> {
    let path = hooks_path()?;
    if path.exists() {
        read_json(&path)
    } else {
        Ok(json!({"enabled": false, "hooks": {}}))
    }
}
#[tauri::command]
pub fn workflow_hooks_save(config: Value) -> Result<(), String> {
    let path = hooks_path()?;
    let root = path.parent().ok_or("No hooks root")?;
    workflow_hooks::parse(
        config.get("hooks").ok_or("hooks object required")?,
        "user",
        root,
        &root.join("hook-data"),
    )?;
    if config.get("enabled").and_then(Value::as_bool).is_none() {
        return Err("enabled boolean required".into());
    }
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    write_json(&path, &config)
}
pub fn hook_runtime(
    cwd: PathBuf,
    agent_type: String,
    prompt: Option<String>,
) -> workflow_hooks::Runtime {
    let mut hooks = active()
        .into_iter()
        .flat_map(|p| p.hooks)
        .collect::<Vec<_>>();
    if let Ok(value) = workflow_hooks_get() {
        if value.get("enabled").and_then(Value::as_bool) == Some(true) {
            if let Ok(path) = hooks_path() {
                if let Some(root) = path.parent() {
                    if let Ok(user) =
                        workflow_hooks::parse(&value, "user", root, &root.join("hook-data"))
                    {
                        hooks.extend(user);
                    }
                }
            }
        }
    }
    workflow_hooks::Runtime {
        hooks,
        cwd,
        agent_type,
        prompt,
    }
}

pub fn markdown_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_file() && path.extension().is_some_and(|e| e == "md") {
            files.push(path.clone());
        } else if let Ok(entries) = fs::read_dir(path) {
            let children = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| fs::symlink_metadata(p).is_ok_and(|m| !m.file_type().is_symlink()))
                .collect::<Vec<_>>();
            files.extend(component_files_bounded(&children, 1, "md"));
        }
    }
    files.sort();
    files.dedup();
    files
}

fn component_files_bounded(paths: &[PathBuf], depth: usize, extension: &str) -> Vec<PathBuf> {
    if depth > 6 {
        return vec![];
    }
    let mut out = Vec::new();
    for path in paths {
        if path.is_file() && path.extension().is_some_and(|e| e == extension) {
            out.push(path.clone());
        } else if let Ok(entries) = fs::read_dir(path) {
            let children = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| fs::symlink_metadata(p).is_ok_and(|m| !m.file_type().is_symlink()))
                .collect::<Vec<_>>();
            out.extend(component_files_bounded(&children, depth + 1, extension));
        }
    }
    out
}

/// Adapt YAML metadata without relocating component files or their relative resources.
pub fn component_markdown(path: &Path) -> Result<String, String> {
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let trimmed = raw.trim_start();
    let (mut fields, body) = if let Some(rest) = trimmed.strip_prefix("---") {
        let end = rest.find("\n---").ok_or("Unclosed component frontmatter")?;
        let fields: BTreeMap<String, serde_yaml::Value> = serde_yaml::from_str(&rest[..end])
            .map_err(|e| format!("Invalid component YAML: {e}"))?;
        (fields, &rest[end + 4..])
    } else {
        (BTreeMap::new(), raw.as_str())
    };
    if fields.contains_key("hooks")
        || fields.get("context").and_then(serde_yaml::Value::as_str) == Some("fork")
    {
        return Err("Component-local hooks and context: fork are not supported".into());
    }
    let fallback = if path.file_name().is_some_and(|n| n == "SKILL.md") {
        path.parent().and_then(Path::file_name)
    } else {
        path.file_stem()
    }
    .and_then(|s| s.to_str())
    .unwrap_or("component");
    fields
        .entry("name".into())
        .or_insert_with(|| serde_yaml::Value::String(fallback.into()));
    fields.entry("description".into()).or_insert_with(|| {
        serde_yaml::Value::String(format!("Run the {fallback} plugin component"))
    });
    let mut header = Vec::new();
    for (key, value) in fields {
        let text = match value {
            serde_yaml::Value::String(s) => s.replace(['\n', '\r'], " "),
            serde_yaml::Value::Bool(b) => b.to_string(),
            serde_yaml::Value::Sequence(items) => items
                .iter()
                .filter_map(serde_yaml::Value::as_str)
                .collect::<Vec<_>>()
                .join(","),
            _ => continue,
        };
        header.push(format!("{key}: {text}"));
    }
    Ok(format!("---\n{}\n---\n{body}", header.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_manifest_rejects_unknown_versions_fields_and_bad_paths() {
        let base = json!({"schemaVersion":1,"name":"native-example","version":"1.0.0"});
        validate_native_manifest(&base).unwrap();
        for (key, value) in [
            ("schemaVersion", json!(2)),
            ("schemaVersion", json!("1")),
            ("name", json!("Bad_Name")),
            ("version", json!(" ")),
            ("metadata", json!(false)),
            ("skills", json!("../escape")),
            ("hooks", json!({})),
            ("commands", json!(["./ok", 42])),
            ("mcpServers", json!("./../escape")),
            ("hook", json!("./hooks.json")),
        ] {
            let mut invalid = base.clone();
            invalid[key] = value;
            assert!(
                validate_native_manifest(&invalid).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[tokio::test]
    async fn native_fixture_has_priority_and_hooks_use_native_tool_input() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("tests/fixtures/plugins/kivio-example");
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path().join("content");
        copy_tree(&source, &content, &mut (1024 * 1024), 0).unwrap();
        fs::create_dir(content.join(".codex-plugin")).unwrap();
        fs::write(
            content.join(".codex-plugin/plugin.json"),
            "invalid ignored alternative",
        )
        .unwrap();
        let mut resolved = resolve(&content, fixture(), dir.path()).unwrap();
        assert_eq!(resolved.package.format, "kivio");
        assert!(
            resolved.package.diagnostics.is_empty(),
            "{:?}",
            resolved.package.diagnostics
        );
        assert_eq!(resolved.package.components["commands"], 1);
        assert_eq!(resolved.package.components["agents"], 1);
        for hook in &mut resolved.hooks {
            assert!(!hook.foreign_tool_names);
            hook.owner = "fixture".into();
        }
        let runtime = workflow_hooks::Runtime {
            hooks: resolved.hooks,
            cwd: content,
            ..Default::default()
        };
        let mut input = runtime.input("PreToolUse", "native-session");
        input["tool_name"] = json!("write");
        input["tool_input"] = json!({"path":"protected/example.txt", "content":"hello"});
        assert!(runtime
            .run("PreToolUse", input.clone())
            .await
            .unwrap()
            .denied
            .is_some());
        input["tool_input"]["path"] = json!("ordinary/example.txt");
        assert!(runtime
            .run("PreToolUse", input)
            .await
            .unwrap()
            .denied
            .is_none());
    }

    #[test]
    fn native_empty_components_override_defaults_and_codex_mcp_wrapper_loads() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".kivio-plugin")).unwrap();
        fs::create_dir(dir.path().join("skills")).unwrap();
        fs::write(
            dir.path().join(".kivio-plugin/plugin.json"),
            r#"{"schemaVersion":1,"name":"example","version":"1.0.0","skills":[],"hooks":[]}"#,
        )
        .unwrap();
        fs::create_dir(dir.path().join("hooks")).unwrap();
        fs::write(
            dir.path().join("hooks/hooks.json"),
            "invalid disabled hooks",
        )
        .unwrap();
        fs::write(
            dir.path().join(".mcp.json"),
            r#"{"mcp_servers":{"helper":{"command":"node"}}}"#,
        )
        .unwrap();
        let resolved = resolve(dir.path(), fixture(), dir.path()).unwrap();
        assert!(resolved.skills.is_empty());
        assert!(resolved.hooks.is_empty());
        assert_eq!(resolved.servers.len(), 1);
        fs::write(
            dir.path().join(".kivio-plugin/plugin.json"),
            r#"{"schemaVersion":2}"#,
        )
        .unwrap();
        assert!(resolve(dir.path(), fixture(), dir.path()).is_err());
    }

    #[tokio::test]
    async fn portable_fixture_loads_commands_skills_and_executes_its_hook() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("tests/fixtures/plugins/portable-example");
        let dir = tempfile::Builder::new()
            .prefix("plugin spaces ")
            .tempdir()
            .unwrap();
        let content = dir.path().join("content");
        copy_tree(&source, &content, &mut (1024 * 1024), 0).unwrap();
        let mut resolved = resolve(&content, fixture(), &dir.path().join("data")).unwrap();
        assert!(
            resolved.package.diagnostics.is_empty(),
            "{:?}",
            resolved.package.diagnostics
        );
        assert_eq!(resolved.skills.len(), 1);
        let command_path = markdown_files(&resolved.commands).pop().unwrap();
        let command = component_markdown(&command_path).unwrap();
        let detail = crate::skills::parse_skill_markdown(&command, "plugin", None, vec![]).unwrap();
        assert_eq!(detail.meta.name, "check");
        assert!(detail.meta.description.contains("portable plugin command"));
        assert!(detail.body.contains("$ARGUMENTS"));
        // Resolve without registering in the real application data directory.
        for hook in &mut resolved.hooks {
            hook.owner = "fixture".into();
        }
        let runtime = workflow_hooks::Runtime {
            hooks: resolved.hooks,
            cwd: content,
            ..Default::default()
        };
        let out = runtime
            .run(
                "SessionStart",
                runtime.input("SessionStart", "fixture-session"),
            )
            .await
            .unwrap();
        assert_eq!(
            out.context,
            ["Portable example loaded for session fixture-session."]
        );
    }

    #[test]
    fn multiple_mcp_servers_have_distinct_ids_and_relative_environment() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".codex-plugin")).unwrap();
        fs::write(dir.path().join(".codex-plugin/plugin.json"), r#"{"name":"multi","mcpServers":{"one":{"command":"node","args":["${PLUGIN_ROOT}/one.cjs"]},"two":{"type":"http","url":"https://example.com/mcp"}}}"#).unwrap();
        let resolved = resolve(dir.path(), fixture(), dir.path()).unwrap();
        assert_eq!(resolved.servers.len(), 2);
        assert_ne!(resolved.servers[0].id, resolved.servers[1].id);
        assert!(resolved
            .servers
            .iter()
            .any(|s| s.transport == "streamable_http"));
        assert!(resolved
            .servers
            .iter()
            .any(|s| s.args.first().is_some_and(|a| a
                .contains(&dir.path().display().to_string())
                && !a.contains("${"))));
    }
    fn fixture() -> Package {
        Package {
            id: uuid::Uuid::new_v4().to_string(),
            name: "".into(),
            description: "".into(),
            format: "".into(),
            source: "fixture".into(),
            revision: None,
            enabled: false,
            version: None,
            components: BTreeMap::new(),
            diagnostics: vec![],
        }
    }
    #[test]
    fn manifests_keep_custom_paths_and_do_not_double_load() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".codex-plugin")).unwrap();
        fs::create_dir(dir.path().join(".claude-plugin")).unwrap();
        fs::create_dir(dir.path().join("custom")).unwrap();
        fs::write(dir.path().join(".codex-plugin/plugin.json"), r#"{"name":"example","skills":"./custom","hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"echo hello"}]}]}}"#).unwrap();
        fs::write(
            dir.path().join(".claude-plugin/plugin.json"),
            "invalid ignored alternate manifest",
        )
        .unwrap();
        let resolved = resolve(dir.path(), fixture(), dir.path()).unwrap();
        assert_eq!(resolved.package.format, "codex");
        assert_eq!(resolved.hooks.len(), 1);
        assert!(resolved.skills[0].ends_with("custom"));
    }
    #[test]
    fn directory_components_are_counted_and_toml_agents_are_diagnosed() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".codex-plugin")).unwrap();
        fs::create_dir(dir.path().join("commands")).unwrap();
        fs::create_dir_all(dir.path().join("agents/nested")).unwrap();
        fs::write(
            dir.path().join(".codex-plugin/plugin.json"),
            r#"{"name":"example"}"#,
        )
        .unwrap();
        fs::write(dir.path().join("commands/one.md"), "First command").unwrap();
        fs::write(dir.path().join("commands/two.md"), "Second command").unwrap();
        fs::write(
            dir.path().join("agents/nested/reviewer.toml"),
            "name = 'reviewer'",
        )
        .unwrap();
        let resolved = resolve(dir.path(), fixture(), dir.path()).unwrap();
        assert_eq!(resolved.package.components["commands"], 2);
        assert_eq!(resolved.package.components["agents"], 0);
        assert!(resolved
            .package
            .diagnostics
            .iter()
            .any(|d| d.contains("reviewer.toml")));
    }
    #[test]
    fn path_escape_and_unknown_capabilities_are_diagnosed() {
        let dir = tempfile::tempdir().unwrap();
        assert!(contained(dir.path(), "../outside").is_err());
        fs::create_dir(dir.path().join(".claude-plugin")).unwrap();
        fs::write(
            dir.path().join(".claude-plugin/plugin.json"),
            r#"{"name":"example","lspServers":{}}"#,
        )
        .unwrap();
        assert!(!resolve(dir.path(), fixture(), dir.path())
            .unwrap()
            .package
            .diagnostics
            .is_empty());
    }
}
