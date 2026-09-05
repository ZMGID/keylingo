//! Agent type definitions for the multi-agent / sub-agent system (P3).
//!
//! An `AgentDefinition` is the sub-agent counterpart of `ChatAssistantSnapshot`:
//! a named persona with an optional system-prompt prefix, an optional model
//! override, and an allow-list of tools. Definitions load in three layers
//! (built-in → user `~/<app_data>/agents/*.md` → project `.kivio/agents/*.md`),
//! later layers overriding earlier ones by id, mirroring the Skill loader and
//! Claude Code's `agents/` convention.
//!
//! Unlike clawspring's reference implementation (whose `tools` field was never
//! enforced), Kivio enforces the tool allow-list at spawn time via
//! `chat::agent::filter::filter_tools_for_agent`, which also strips the `agent`
//! tool itself so a sub-agent can never recursively spawn.

pub mod parse;
pub mod types;

pub use types::{builtin_agent_definitions, AgentDefinition};

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

/// User-level agents directory: `<app_data>/agents`. Created on demand.
pub fn user_agents_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("app_data_dir unavailable: {err}"))?
        .join("agents");
    std::fs::create_dir_all(&dir).map_err(|err| format!("create agents dir failed: {err}"))?;
    Ok(dir)
}

/// Load the merged agent registry: built-ins, then `<app_data>/agents/*.md`
/// (source `user`), then `<project_root>/.kivio/agents/*.md` (source
/// `project`) when a project root is provided. Same-id later layers override
/// earlier ones. Parse failures are skipped (never fatal) so a malformed file
/// can't break spawning.
pub fn load_agent_definitions(
    app: &AppHandle,
    project_root: Option<&Path>,
) -> Vec<AgentDefinition> {
    let mut defs = builtin_agent_definitions();
    for plugin in crate::plugins::packages::active() {
        for path in crate::plugins::packages::markdown_files(&plugin.agents) {
            if let Ok(raw) = crate::plugins::packages::component_markdown(&path) {
                let fallback = path.file_stem().and_then(|s| s.to_str()).unwrap_or("agent");
                if let Some(mut def) = parse::parse_agent_markdown(
                    fallback,
                    &raw,
                    "plugin",
                    Some(path.display().to_string()),
                ) {
                    def.id = format!("pkg-{}-{}", plugin.package.id, def.id);
                    def.system_prompt = plugin.expand_body(&def.system_prompt);
                    def.tools = def
                        .tools
                        .iter()
                        .map(|name| plugin_tool_name(name))
                        .collect();
                    def.disallowed_tools = def
                        .disallowed_tools
                        .iter()
                        .map(|name| plugin_tool_name(name))
                        .collect();
                    if def.model.as_deref() == Some("inherit") {
                        def.model = None;
                    }
                    def.name = format!("{}:{}", plugin.package.name, def.name);
                    upsert(&mut defs, def);
                }
            }
        }
    }

    if let Ok(dir) = user_agents_dir(app) {
        merge_dir(&mut defs, &dir, "user");
    }
    if let Some(root) = project_root {
        merge_dir(&mut defs, &root.join(".kivio").join("agents"), "project");
    }
    defs
}

fn plugin_tool_name(name: &str) -> String {
    match name {
        "Bash" => "bash",
        "Read" => "read",
        "Write" => "write",
        "Edit" => "edit",
        "Glob" => "glob",
        "Grep" => "grep",
        "Agent" | "Task" => "agent",
        other => other,
    }
    .into()
}

/// Find a definition by id or (case-insensitive) name.
pub fn find_definition<'a>(
    defs: &'a [AgentDefinition],
    name_or_id: &str,
) -> Option<&'a AgentDefinition> {
    let needle = name_or_id.trim();
    let lower = needle.to_ascii_lowercase();
    defs.iter()
        .find(|d| d.id == needle || d.name == needle)
        .or_else(|| {
            defs.iter()
                .find(|d| d.id.eq_ignore_ascii_case(&lower) || d.name.eq_ignore_ascii_case(&lower))
        })
}

fn merge_dir(defs: &mut Vec<AgentDefinition>, dir: &Path, source: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let fallback_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("agent")
            .to_string();
        let path_str = path.to_string_lossy().to_string();
        if let Some(def) = parse::parse_agent_markdown(&fallback_id, &raw, source, Some(path_str)) {
            upsert(defs, def);
        }
    }
}

fn upsert(defs: &mut Vec<AgentDefinition>, def: AgentDefinition) {
    if let Some(existing) = defs.iter_mut().find(|d| d.id == def.id) {
        *existing = def;
    } else {
        defs.push(def);
    }
}
