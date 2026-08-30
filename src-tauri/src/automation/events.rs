use tauri::{AppHandle, Emitter};

use super::types::AutomationRunEvent;

pub const EVENT: &str = "automation-run";

pub fn emit(app: &AppHandle, event: AutomationRunEvent) {
    if let Err(err) = app.emit(EVENT, &event) {
        eprintln!("automation-run emit failed: {err}");
    }
}

pub fn run_started(app: &AppHandle, automation_id: &str, run_id: &str) {
    emit(
        app,
        AutomationRunEvent {
            kind: "run_started".into(),
            automation_id: automation_id.into(),
            run_id: run_id.into(),
            node_id: None,
            status: Some("running".into()),
            output: None,
            error: None,
        },
    );
}

pub fn node_started(app: &AppHandle, automation_id: &str, run_id: &str, node_id: &str) {
    emit(
        app,
        AutomationRunEvent {
            kind: "node_started".into(),
            automation_id: automation_id.into(),
            run_id: run_id.into(),
            node_id: Some(node_id.into()),
            status: Some("running".into()),
            output: None,
            error: None,
        },
    );
}

pub fn node_finished(
    app: &AppHandle,
    automation_id: &str,
    run_id: &str,
    node_id: &str,
    status: &str,
    output: Option<String>,
    error: Option<String>,
) {
    emit(
        app,
        AutomationRunEvent {
            kind: "node_finished".into(),
            automation_id: automation_id.into(),
            run_id: run_id.into(),
            node_id: Some(node_id.into()),
            status: Some(status.into()),
            output,
            error,
        },
    );
}

pub fn run_finished(
    app: &AppHandle,
    automation_id: &str,
    run_id: &str,
    status: &str,
    error: Option<String>,
) {
    emit(
        app,
        AutomationRunEvent {
            kind: "run_finished".into(),
            automation_id: automation_id.into(),
            run_id: run_id.into(),
            node_id: None,
            status: Some(status.into()),
            output: None,
            error,
        },
    );
}
