use tauri::AppHandle;

use super::storage;

/// Enabled automations that have a non-empty hotkey accelerator.
/// `shortcuts.rs` registers them after the built-in app hotkeys.
pub fn enabled_bindings(app: &AppHandle) -> Vec<(String, String)> {
    let Ok(metas) = storage::list(app) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for meta in metas {
        if !meta.enabled {
            continue;
        }
        let Ok(automation) = storage::get(app, &meta.id) else {
            continue;
        };
        let Some(acc) = accelerator(&automation) else {
            continue;
        };
        out.push((acc, automation.id));
    }
    out
}

fn accelerator(automation: &super::types::Automation) -> Option<String> {
    automation
        .nodes
        .iter()
        .find(|node| node.node_type == "trigger.hotkey")
        .and_then(|node| {
            node.data
                .get("hotkey")
                .and_then(|v| v.get("accelerator"))
                .and_then(|v| v.as_str())
        })
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}
