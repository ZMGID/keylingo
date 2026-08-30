use std::fs;
use std::path::PathBuf;

use tauri::AppHandle;

use super::storage::{automations_dir, validate_id};
use super::types::{AutomationRun, AutomationRunSummary};

const MAX_RUNS_PER_AUTOMATION: usize = 50;

fn runs_dir(app: &AppHandle, automation_id: &str) -> Result<PathBuf, String> {
    validate_id(automation_id)?;
    let dir = automations_dir(app)?.join("runs").join(automation_id);
    fs::create_dir_all(&dir).map_err(|err| format!("create runs dir failed: {err}"))?;
    Ok(dir)
}

fn run_path(app: &AppHandle, automation_id: &str, run_id: &str) -> Result<PathBuf, String> {
    validate_id(run_id)?;
    Ok(runs_dir(app, automation_id)?.join(format!("{run_id}.json")))
}

pub(crate) fn write_run(app: &AppHandle, run: &AutomationRun) -> Result<(), String> {
    let path = run_path(app, &run.automation_id, &run.id)?;
    let json = serde_json::to_string_pretty(run).map_err(|err| err.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|err| format!("write run failed: {err}"))?;
    fs::rename(&tmp, path).map_err(|err| format!("commit run failed: {err}"))?;
    prune(app, &run.automation_id)?;
    Ok(())
}

fn prune(app: &AppHandle, automation_id: &str) -> Result<(), String> {
    let mut summaries = list(app, automation_id)?;
    if summaries.len() <= MAX_RUNS_PER_AUTOMATION {
        return Ok(());
    }
    summaries.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    let extra = summaries.len() - MAX_RUNS_PER_AUTOMATION;
    for summary in summaries.into_iter().take(extra) {
        let path = run_path(app, automation_id, &summary.id)?;
        let _ = fs::remove_file(path);
    }
    Ok(())
}

pub(crate) fn list(app: &AppHandle, automation_id: &str) -> Result<Vec<AutomationRunSummary>, String> {
    let dir = runs_dir(app, automation_id)?;
    let mut items = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|err| format!("list runs failed: {err}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        match read_run_file(&path) {
            Ok(run) => items.push(AutomationRunSummary {
                id: run.id,
                origin: run.origin,
                status: run.status,
                started_at: run.started_at,
                finished_at: run.finished_at,
                error: run.error,
            }),
            Err(err) => eprintln!("skip automation run {}: {err}", path.display()),
        }
    }
    items.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(items)
}

pub(crate) fn get(app: &AppHandle, automation_id: &str, run_id: &str) -> Result<AutomationRun, String> {
    read_run_file(&run_path(app, automation_id, run_id)?)
}

fn read_run_file(path: &PathBuf) -> Result<AutomationRun, String> {
    let raw = fs::read_to_string(path).map_err(|err| format!("read run failed: {err}"))?;
    serde_json::from_str(&raw).map_err(|err| format!("parse run failed: {err}"))
}
