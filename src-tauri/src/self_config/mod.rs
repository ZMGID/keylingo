//! Small, live-state operations for the bundled Kivio configuration skills.
//! No generic settings replacement, CLI execution, or credential dumping.
mod skill_install;
#[cfg(test)]
mod tests;

use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tauri::{Emitter, Manager};

use crate::mcp::native_registry::{text_tool_result, NativeCallCtx, NativeToolFuture};
use crate::mcp::types::{ChatToolDefinition, McpToolCallResult};
use crate::settings::{ChatMcpServer, Settings};

pub fn inspect_definition() -> ChatToolDefinition {
    definition("kivio_inspect", "Read Kivio's current configuration, skills, plugin packages, MCP summaries or hook status without exposing credentials or connecting servers. Use the kivio-configuration-guide skill for self-configuration requests. Disk/registry presence does not prove a capability is usable in this conversation.", json!({
        "type":"object", "properties":{"topic":{"type":"string","enum":["status","skills","plugins","mcp","hooks"]}}, "additionalProperties":false
    }), false)
}

pub fn configure_definition() -> ChatToolDefinition {
    definition("kivio_configure", "Install or configure Kivio extensions using live application services. First load the relevant kivio configuration skill and inspect current state. Acts on the user's requested resources only. New skills/plugins/MCP tools are usable on the next turn. MCP tests may launch a process/connect a service. Config JSON is read from config_path; no whole-settings replacement. Requires the host command tool toggle and normal session consent.", json!({
        "type":"object", "properties":{
            "action":{"type":"string","enum":["skill_install","skill_set_enabled","skill_settings","plugin_import","plugin_set_enabled","plugin_remove","mcp_upsert","mcp_remove","mcp_test","hooks_save"]},
            "source":{"type":"string","description":"skill_install: local skill directory or one-skill ZIP; plugin_import: local package root or HTTPS Git URL"},
            "scope":{"type":"string","enum":["user","project"],"description":"skill_install only; default user. project requires a project conversation."},
            "replace":{"type":"boolean","description":"skill_install only; explicit replacement keeps a recoverable backup outside scan roots"},
            "id":{"type":"string","description":"Actual skill/package/MCP id returned by inspection. mcp_upsert: omit to create, pass an existing id to patch."},
            "enabled":{"type":"boolean"},
            "subdirectory":{"type":"string","description":"plugin_import only, relative to source repository"},
            "config_path":{"type":"string","description":"UTF-8 JSON file: mcp_upsert uses ChatMcpServer fields (partial update for existing id); hooks_save uses {enabled,hooks}; skill_settings uses skillScanPaths/skillAutoMatch/skillRuntime. Never an entire settings.json."}
        },"required":["action"],"additionalProperties":false
    }), true)
}

fn definition(
    name: &str,
    description: &str,
    input_schema: Value,
    sensitive: bool,
) -> ChatToolDefinition {
    ChatToolDefinition {
        id: format!("native__{name}"),
        name: name.into(),
        description: description.into(),
        source: "native".into(),
        server_id: None,
        server_name: Some("Kivio".into()),
        input_schema,
        sensitive,
        annotations: None,
        output_schema: None,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectArgs {
    #[serde(default = "status_topic")]
    topic: String,
}
fn status_topic() -> String {
    "status".into()
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    SkillInstall {
        source: String,
        #[serde(default)]
        scope: Scope,
        #[serde(default)]
        replace: bool,
    },
    SkillSetEnabled {
        id: String,
        enabled: bool,
    },
    SkillSettings {
        config_path: String,
    },
    PluginImport {
        source: String,
        subdirectory: Option<String>,
    },
    PluginSetEnabled {
        id: String,
        enabled: bool,
    },
    PluginRemove {
        id: String,
    },
    McpUpsert {
        id: Option<String>,
        config_path: String,
    },
    McpRemove {
        id: String,
    },
    McpTest {
        id: String,
    },
    HooksSave {
        config_path: String,
    },
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Scope {
    #[default]
    User,
    Project,
}

fn cwd(ctx: &NativeCallCtx<'_>) -> Option<PathBuf> {
    ctx.workspace
        .project
        .as_ref()
        .and_then(|p| p.root_path.clone())
        .or_else(|| ctx.workspace.default_directory.clone())
}

pub fn inspect(ctx: NativeCallCtx<'_>) -> NativeToolFuture<'_> {
    Box::pin(async move {
        if !ctx.state.settings_read().chat_tools.native_tools.read_file {
            return Err("Kivio inspection requires the read tool toggle".into());
        }
        let args: InspectArgs =
            serde_json::from_value(ctx.arguments.clone()).map_err(|e| e.to_string())?;
        let settings = ctx.state.settings_read().clone();
        let value = match args.topic.as_str() {
            "status" => status_summary(&settings, cwd(&ctx).as_deref()),
            "skills" => {
                let registry = crate::skills::build_registry_metadata_in(
                    ctx.app,
                    &settings.chat_tools.skill_scan_paths,
                    cwd(&ctx).as_deref(),
                )?;
                json!({"skills":registry.metas().iter().map(|m| json!({
                    "id":m.id,"name":m.name,"source":m.source,"path":m.path,
                    "globallyAvailable":crate::settings::skill_globally_available(&settings.chat_tools, &m.id, crate::settings::obsidian_connector_configured(&settings.obsidian_vault_path)),
                    "disableModelInvocation":m.disable_model_invocation
                })).collect::<Vec<_>>(),"warnings":registry.warnings,
                "note":"Registry and global gates only; the conversation assistant allow-list and model tools can still restrict activation."})
            }
            "plugins" => json!({"packages":crate::plugins::packages::plugin_packages_list()?,
                "catalog":crate::plugins::PLUGIN_CATALOG.iter().map(|p| json!({"id":p.id,"name":p.name,"enabled":crate::plugins::is_enabled(p.id)})).collect::<Vec<_>>(),
                "note":"Catalog installation detection is not run by this read-only tool; catalog plugins use the Plugins page."}),
            "mcp" => {
                json!({"servers":settings.chat_tools.servers.iter().map(server_summary).collect::<Vec<_>>(),"connected": "not tested"})
            }
            "hooks" => {
                let config = crate::plugins::packages::workflow_hooks_get()?;
                json!({"workflow":hook_summary(&config),
                    "notificationHooks":settings.chat_tools.hooks.iter().map(|h| json!({"id":h.id,"name":h.name,"event":h.event,"enabled":h.enabled,"type":h.kind})).collect::<Vec<_>>(),
                    "workflowPath":crate::plugins::plugins_root().map(|p|p.join("workflow-hooks.json")),
                    "note":"Notification hooks are fire-and-forget; workflow hooks can affect execution. Scripts and credentials are not included."})
            }
            _ => return Err("Unknown inspection topic".into()),
        };
        result(&settings, value)
    })
}

fn status_summary(settings: &Settings, cwd: Option<&Path>) -> Value {
    json!({"application":"Kivio","version":env!("CARGO_PKG_VERSION"),"os":std::env::consts::OS,
        "appData":crate::app_data::app_data_dir(),"userSkills":crate::skills::kivio_skills_dir(),"cwd":cwd,
        "skillRuntime":settings.chat_tools.native_tools.skill_runtime,"skillAutoMatch":settings.chat_tools.skill_auto_match,
        "skillScanPaths":settings.chat_tools.skill_scan_paths,"disabledSkillIds":settings.chat_tools.disabled_skill_ids,
        "configurationTools":{"inspect":settings.chat_tools.native_tools.read_file,"configure":settings.chat_tools.native_tools.run_command},
        "providers":settings.providers.iter().map(|p|json!({"id":p.id,"name":p.name,"apiFormat":p.api_format,"hasCredentials":p.has_credentials()})).collect::<Vec<_>>(),
        "defaultModels":settings.default_models,
        "note":"Application defaults, not a claim about this conversation's selected model/runtime. Changes to tool catalogs take effect next turn."})
}

fn server_summary(server: &ChatMcpServer) -> Value {
    json!({"id":server.id,"name":server.name,"enabled":server.enabled,"transport":server.transport,
        "hasCommand":!server.command.is_empty(),"argCount":server.args.len(),"hasUrl":!server.url.is_empty(),
        "envNames":server.env.keys().collect::<Vec<_>>(),"headerNames":server.headers.keys().collect::<Vec<_>>(),
        "enabledTools":server.enabled_tools,"managed":managed_server(server),"hasAuth":server.auth.is_some()})
}

fn hook_summary(config: &Value) -> Value {
    json!({"enabled":config.get("enabled").and_then(Value::as_bool),
        "events":config.get("hooks").and_then(Value::as_object).map(|events| events.iter().map(|(k,v)|(k.clone(),json!(v.as_array().map(Vec::len)))).collect::<serde_json::Map<_,_>>())})
}

pub fn configure(ctx: NativeCallCtx<'_>) -> NativeToolFuture<'_> {
    Box::pin(async move {
        if !ctx
            .state
            .settings_read()
            .chat_tools
            .native_tools
            .run_command
        {
            return Err("Kivio configuration requires the host command tool toggle".into());
        }
        if ctx.native_ctx.is_none_or(|c| c.depth > 0) {
            return Err("Configure Kivio from the main conversation, not a sub-agent".into());
        }
        let action: Action =
            serde_json::from_value(ctx.arguments.clone()).map_err(|e| e.to_string())?;
        let before = ctx.state.settings_read().clone();
        let operation = configure_action(&ctx, action).await;
        if operation.is_ok() {
            // Notify webviews without broadcasting settings or credentials.
            let _ = ctx.app.emit("kivio-configuration-changed", ());
        }
        match operation {
            Ok(value) => result(
                &before,
                json!({"result":value,"refresh":"Use the next turn for newly installed skills, commands, hooks and MCP tools; current run snapshots are unchanged."}),
            ),
            Err(error) => Err(redact_text(&before, error)),
        }
    })
}

async fn configure_action(ctx: &NativeCallCtx<'_>, action: Action) -> Result<Value, String> {
    match action {
        Action::SkillInstall {
            source,
            scope,
            replace,
        } => {
            let source = crate::native_tools::resolve_tool_read_path(ctx.workspace, &source)?;
            let root = match scope {
                Scope::User => crate::skills::kivio_skills_dir().ok_or("Home directory unavailable")?,
                Scope::Project => ctx.workspace.project.as_ref().and_then(|p|p.root_path.clone())
                    .ok_or("Project installation requires a project conversation with a root directory")?.join(".kivio/skills"),
            };
            tokio::task::spawn_blocking(move || skill_install::install(&source, &root, replace))
                .await
                .map_err(|e| e.to_string())?
        }
        Action::SkillSetEnabled { id, enabled } => {
            let settings = ctx.state.settings_read().clone();
            let registry = crate::skills::build_registry_metadata_in(
                ctx.app,
                &settings.chat_tools.skill_scan_paths,
                cwd(ctx).as_deref(),
            )?;
            let skill = registry
                .records
                .iter()
                .find(|r| r.meta.id == id)
                .ok_or("Skill id not found in current registry")?;
            if skill.meta.source == "plugin" || crate::plugins::skill_owned_by_plugin(&id).is_some()
            {
                return Err("Manage plugin-owned skills through the plugin enable switch".into());
            }
            update_settings(ctx, |next| {
                next.chat_tools.disabled_skill_ids.retain(|s| s != &id);
                if !enabled {
                    next.chat_tools.disabled_skill_ids.push(id.clone());
                }
                Ok(())
            })?;
            Ok(json!({"id":id,"enabled":enabled}))
        }
        Action::SkillSettings { config_path } => {
            let config = read_config(ctx, &config_path)?;
            update_settings(ctx, |next| apply_skill_settings(next, &config))?;
            Ok(json!({"saved":true}))
        }
        Action::PluginImport {
            source,
            subdirectory,
        } => {
            let source = if source.trim().starts_with("https://") {
                source
            } else {
                crate::native_tools::resolve_tool_read_path(ctx.workspace, &source)?
                    .display()
                    .to_string()
            };
            Ok(json!(
                crate::plugins::packages::plugin_packages_import(source, subdirectory).await?
            ))
        }
        Action::PluginSetEnabled { id, enabled } => Ok(json!(
            crate::plugins::packages::plugin_packages_set_enabled(
                ctx.app.clone(),
                ctx.app.state(),
                id,
                enabled
            )
            .await?
        )),
        Action::PluginRemove { id } => {
            crate::plugins::packages::plugin_packages_remove(
                ctx.app.clone(),
                ctx.app.state(),
                id.clone(),
            )
            .await?;
            Ok(json!({"removed":id}))
        }
        Action::McpUpsert { id, config_path } => {
            let config = read_config(ctx, &config_path)?;
            let mut server_id = String::new();
            update_settings(ctx, |next| {
                server_id = upsert_server(next, id.as_deref(), &config)?;
                Ok(())
            })?;
            ctx.state.mcp_disconnect_server(&server_id).await;
            Ok(json!({"id":server_id,"saved":true,"tested":false}))
        }
        Action::McpRemove { id } => {
            update_settings(ctx, |next| {
                let server = next
                    .chat_tools
                    .servers
                    .iter()
                    .find(|s| s.id == id)
                    .ok_or("Unknown MCP id")?;
                if managed_server(server) {
                    return Err("Manage this server through its plugin or connector".into());
                }
                next.chat_tools.servers.retain(|s| s.id != id);
                Ok(())
            })?;
            ctx.state.mcp_disconnect_server(&id).await;
            Ok(json!({"removed":id}))
        }
        Action::McpTest { id } => {
            let server = ctx
                .state
                .settings_read()
                .chat_tools
                .servers
                .iter()
                .find(|s| s.id == id)
                .cloned()
                .ok_or("Unknown MCP id")?;
            if !crate::mcp::registry::mcp_server_is_runtime_eligible(&server) {
                return Err("Enable the MCP server before testing".into());
            }
            let listed = crate::mcp::registry::chat_mcp_list_tool_defs(
                ctx.app.clone(),
                ctx.app.state(),
                id.clone(),
            )
            .await?;
            Ok(
                json!({"id":id,"success":true,"tools":listed.iter().map(|t| &t.name).collect::<Vec<_>>()}),
            )
        }
        Action::HooksSave { config_path } => {
            let config = read_config(ctx, &config_path)?;
            crate::plugins::packages::workflow_hooks_save(config.clone())?;
            Ok(json!({"saved":true,"workflow":hook_summary(&config)}))
        }
    }
}

fn read_config(ctx: &NativeCallCtx<'_>, path: &str) -> Result<Value, String> {
    let path = crate::native_tools::resolve_tool_read_path(ctx.workspace, path)?;
    if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 1024 * 1024 {
        return Err("Config exceeds 1 MiB".into());
    }
    // Do not print parse input or the entire settings store on malformed JSON.
    serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?).map_err(|e| {
        format!(
            "Invalid config JSON at line {}, column {}",
            e.line(),
            e.column()
        )
    })
}

fn update_settings(
    ctx: &NativeCallCtx<'_>,
    change: impl FnOnce(&mut Settings) -> Result<(), String>,
) -> Result<(), String> {
    // Read-modify-persist under the application's settings lock, just like package activation.
    let mut current = ctx.state.settings_write();
    let mut next = current.clone();
    change(&mut next)?;
    crate::settings::persist_settings(ctx.app, &next)?;
    *current = next;
    Ok(())
}

fn apply_skill_settings(settings: &mut Settings, config: &Value) -> Result<(), String> {
    let object = config.as_object().ok_or("Expected skill settings object")?;
    if object.is_empty() {
        return Err("No skill settings supplied".into());
    }
    for (key, value) in object {
        match key.as_str() {
            "skillAutoMatch" => {
                settings.chat_tools.skill_auto_match =
                    value.as_bool().ok_or("skillAutoMatch must be boolean")?
            }
            "skillRuntime" => {
                settings.chat_tools.native_tools.skill_runtime =
                    value.as_bool().ok_or("skillRuntime must be boolean")?
            }
            "skillScanPaths" => {
                let paths: Vec<String> = serde_json::from_value(value.clone())
                    .map_err(|_| "skillScanPaths must be an array of absolute directories")?;
                if paths
                    .iter()
                    .any(|p| !Path::new(p).is_absolute() || !Path::new(p).is_dir())
                {
                    return Err("Scan paths must be existing absolute directories; expand home/env variables first".into());
                }
                let mut unique = Vec::new();
                for path in paths {
                    if !unique.contains(&path) {
                        unique.push(path);
                    }
                }
                settings.chat_tools.skill_scan_paths = unique;
            }
            _ => return Err(format!("Unsupported skill setting: {key}")),
        }
    }
    Ok(())
}

fn managed_server(server: &ChatMcpServer) -> bool {
    server.connector_id.is_some() || server.id.starts_with("plugin-")
}

fn upsert_server(
    settings: &mut Settings,
    id: Option<&str>,
    patch: &Value,
) -> Result<String, String> {
    let object = patch
        .as_object()
        .ok_or("Expected one MCP server object, not a server map")?;
    const FIELDS: &[&str] = &[
        "name",
        "enabled",
        "transport",
        "url",
        "command",
        "args",
        "env",
        "headers",
        "cwd",
        "enabledTools",
    ];
    if object.is_empty() || object.keys().any(|k| !FIELDS.contains(&k.as_str())) {
        return Err("MCP config accepts only name/enabled/transport/url/command/args/env/headers/cwd/enabledTools; id comes from the tool argument".into());
    }
    let mut server = match id {
        Some(id) => settings
            .chat_tools
            .servers
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .ok_or("Unknown MCP id; omit id to create")?,
        None => ChatMcpServer {
            id: uuid::Uuid::new_v4().to_string(),
            ..Default::default()
        },
    };
    if managed_server(&server) {
        return Err("Manage this server through its plugin or connector".into());
    }
    let mut merged = serde_json::to_value(&server).map_err(|e| e.to_string())?;
    for (key, value) in object {
        merged[key] = value.clone();
    }
    server = serde_json::from_value(merged).map_err(|_| "Invalid MCP field type")?;
    if server.name.trim().is_empty() {
        return Err("MCP name required".into());
    }
    if id.is_none()
        && settings
            .chat_tools
            .servers
            .iter()
            .any(|s| s.name == server.name)
    {
        return Err("MCP name already exists; inspect and update its id instead".into());
    }
    match server.transport.as_str() {
        "stdio" if !server.command.trim().is_empty() => {}
        "streamable_http" => {
            let url = url::Url::parse(&server.url).map_err(|_| "Invalid MCP URL")?;
            if !["http", "https"].contains(&url.scheme())
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err("MCP URL must be HTTP(S) without embedded credentials".into());
            }
        }
        _ => return Err("Use stdio with command, or streamable_http with url".into()),
    }
    if server
        .cwd
        .as_ref()
        .is_some_and(|p| !p.is_empty() && (!Path::new(p).is_absolute() || !Path::new(p).is_dir()))
    {
        return Err("MCP cwd must be an existing absolute directory".into());
    }
    let id = server.id.clone();
    settings.chat_tools.servers.retain(|s| s.id != id);
    if server.enabled {
        settings.chat_tools.enabled = true;
    }
    settings.chat_tools.servers.push(server);
    Ok(id)
}

fn result(settings: &Settings, mut value: Value) -> Result<McpToolCallResult, String> {
    // Redact string values before serialization: env values such as "1" must
    // never corrupt booleans, numbers or the JSON structure of the tool result.
    redact_value(settings, &mut value);
    Ok(text_tool_result(
        serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?,
    ))
}

fn redact_value(settings: &Settings, value: &mut Value) {
    match value {
        Value::String(text) => *text = redact_text(settings, text.clone()),
        Value::Array(items) => items.iter_mut().for_each(|v| redact_value(settings, v)),
        Value::Object(object) => object.values_mut().for_each(|v| redact_value(settings, v)),
        _ => {}
    }
}

fn redact_text(settings: &Settings, mut text: String) -> String {
    let mut secrets = Vec::new();
    for p in &settings.providers {
        secrets.extend(p.api_keys.iter().cloned());
        if let Some(key) = &p.api_key_legacy {
            secrets.push(key.clone());
        }
    }
    for s in &settings.chat_tools.servers {
        secrets.extend(s.env.values().cloned());
        secrets.extend(s.headers.values().cloned());
        // MCP startup errors can echo arguments as well as env/header values.
        // Keep option names, redact positional arguments and values after '='.
        for arg in &s.args {
            if let Some((_, value)) = arg.split_once('=') {
                secrets.push(value.to_string());
            } else if !arg.starts_with('-') {
                secrets.push(arg.clone());
            }
        }
        if let Some(auth) = &s.auth {
            secrets.push(auth.access_token.clone());
            if let Some(token) = &auth.refresh_token {
                secrets.push(token.clone());
            }
        }
    }
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        if secret.len() < 4 {
            if let Ok(pattern) = regex::Regex::new(&format!(r"\b{}\b", regex::escape(secret))) {
                text = pattern.replace_all(&text, "[redacted]").into_owned();
            }
        } else {
            text = text.replace(secret, "[redacted]");
        }
    }
    // Package sources and diagnostics may contain URL credentials/query tokens.
    if let Ok(pattern) = regex::Regex::new(r#"https?://[^\s"<>]+"#) {
        text = pattern
            .replace_all(&text, |captures: &regex::Captures<'_>| {
                let raw = &captures[0];
                match url::Url::parse(raw) {
                    Ok(mut url) => {
                        let _ = url.set_username("");
                        let _ = url.set_password(None);
                        url.set_query(None);
                        url.set_fragment(None);
                        url.to_string()
                    }
                    Err(_) => "[redacted URL]".into(),
                }
            })
            .into_owned();
    }
    text
}
