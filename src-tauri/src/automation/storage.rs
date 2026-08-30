use std::fs;
use std::path::PathBuf;

use chrono::{SecondsFormat, Utc};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use super::types::{is_allowed_node_type, Automation, AutomationMeta, SCHEMA_VERSION};

pub(crate) fn automations_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("app_data_dir unavailable: {err}"))?
        .join("automations");
    fs::create_dir_all(&dir).map_err(|err| format!("create automations dir failed: {err}"))?;
    Ok(dir)
}

pub(crate) fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err("invalid automation id".to_string());
    }
    Ok(())
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn file_path(app: &AppHandle, id: &str) -> Result<PathBuf, String> {
    validate_id(id)?;
    Ok(automations_dir(app)?.join(format!("{id}.json")))
}

fn read_one(path: &PathBuf) -> Result<Automation, String> {
    let raw = fs::read_to_string(path).map_err(|err| format!("read automation failed: {err}"))?;
    let automation: Automation =
        serde_json::from_str(&raw).map_err(|err| format!("parse automation failed: {err}"))?;
    if automation.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "unsupported automation schemaVersion {}",
            automation.schema_version
        ));
    }
    Ok(automation)
}

fn validate_graph(automation: &Automation) -> Result<(), String> {
    for node in &automation.nodes {
        validate_id(&node.id)?;
        if !is_allowed_node_type(&node.node_type) {
            return Err(format!("unknown node type: {}", node.node_type));
        }
    }
    for edge in &automation.edges {
        validate_id(&edge.id)?;
        validate_id(&edge.source)?;
        validate_id(&edge.target)?;
    }
    Ok(())
}

fn write_atomic(path: &PathBuf, automation: &Automation) -> Result<(), String> {
    let json = serde_json::to_string_pretty(automation).map_err(|err| err.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|err| format!("write automation failed: {err}"))?;
    fs::rename(&tmp, path).map_err(|err| format!("commit automation failed: {err}"))?;
    Ok(())
}

pub(crate) fn list(app: &AppHandle) -> Result<Vec<AutomationMeta>, String> {
    let dir = automations_dir(app)?;
    let mut metas = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|err| format!("list automations failed: {err}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        match read_one(&path) {
            Ok(automation) => metas.push(automation.meta()),
            Err(err) => {
                eprintln!("skip automation {}: {err}", path.display());
            }
        }
    }
    metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(metas)
}

pub(crate) fn get(app: &AppHandle, id: &str) -> Result<Automation, String> {
    read_one(&file_path(app, id)?)
}

pub(crate) fn save(app: &AppHandle, mut automation: Automation) -> Result<Automation, String> {
    if automation.id.trim().is_empty() {
        automation.id = Uuid::new_v4().to_string();
    }
    validate_id(&automation.id)?;
    automation.schema_version = SCHEMA_VERSION;
    if automation.created_at.trim().is_empty() {
        automation.created_at = now_iso();
    }
    automation.updated_at = now_iso();
    validate_graph(&automation)?;
    write_atomic(&file_path(app, &automation.id)?, &automation)?;
    Ok(automation)
}

pub(crate) fn delete(app: &AppHandle, id: &str) -> Result<(), String> {
    let path = file_path(app, id)?;
    if path.exists() {
        fs::remove_file(&path).map_err(|err| format!("delete automation failed: {err}"))?;
    }
    Ok(())
}

pub(crate) fn set_enabled(app: &AppHandle, id: &str, enabled: bool) -> Result<Automation, String> {
    let mut automation = get(app, id)?;
    automation.enabled = enabled;
    save(app, automation)
}

#[cfg(test)]
mod tests {
    use super::validate_id;

    #[test]
    fn rejects_path_ids() {
        assert!(validate_id("ok").is_ok());
        assert!(validate_id("../x").is_err());
        assert!(validate_id("a/b").is_err());
        assert!(validate_id("").is_err());
    }
}
