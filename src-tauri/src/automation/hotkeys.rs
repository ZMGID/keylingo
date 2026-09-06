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

pub(crate) fn fingerprint(automation: &super::types::Automation) -> String {
    format!(
        "{}:{}",
        automation.enabled,
        accelerator(automation).unwrap_or_default()
    )
}

fn accelerator(automation: &super::types::Automation) -> Option<String> {
    automation
        .nodes
        .iter()
        .find(|node| {
            node.node_type == "trigger.hotkey" && !super::interpolate::node_disabled(&node.data)
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabling_a_trigger_removes_its_binding_and_changes_registration_fingerprint() {
        let mut automation = serde_json::from_value(serde_json::json!({
            "id":"a", "enabled":true,
            "nodes":[{"id":"h","type":"trigger.hotkey","data":{"hotkey":{"accelerator":"Control+Shift+K"}}}]
        })).unwrap();
        let before = fingerprint(&automation);
        assert_eq!(accelerator(&automation).as_deref(), Some("Control+Shift+K"));
        automation.nodes[0].data["disabled"] = serde_json::json!(true);
        assert!(accelerator(&automation).is_none());
        assert_ne!(fingerprint(&automation), before);
    }
}
