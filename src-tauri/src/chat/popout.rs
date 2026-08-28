use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

use crate::chat::protocol;
use crate::state::AppState;
use crate::windows;

pub const POPOUT_LABEL_PREFIX: &str = "chat-popout-";
pub const MAX_POPOUT_WINDOWS: usize = 3;
pub const POPOUTS_CHANGED_EVENT: &str = "chat-popouts-changed";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PopoutsChangedPayload {
    conversation_ids: Vec<String>,
}

pub fn is_popout_label(label: &str) -> bool {
    label.starts_with(POPOUT_LABEL_PREFIX)
}

pub fn conversation_id_from_label(label: &str) -> Option<&str> {
    label.strip_prefix(POPOUT_LABEL_PREFIX).filter(|id| !id.is_empty())
}

pub fn popout_label(conversation_id: &str) -> String {
    format!("{POPOUT_LABEL_PREFIX}{conversation_id}")
}

fn valid_popout_conversation_id(id: &str) -> bool {
    id.starts_with("conv_")
        && id.len() > "conv_".len()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub fn list_popout_conversation_ids(app: &AppHandle) -> Vec<String> {
    let mut ids: Vec<String> = app
        .webview_windows()
        .into_iter()
        .filter_map(|(label, _)| conversation_id_from_label(&label).map(str::to_string))
        .collect();
    ids.sort();
    ids
}

pub fn has_open_popouts(app: &AppHandle) -> bool {
    app.webview_windows()
        .into_iter()
        .any(|(label, _)| is_popout_label(&label))
}

pub fn first_visible_popout(app: &AppHandle) -> Option<WebviewWindow> {
    app.webview_windows().into_iter().find_map(|(label, window)| {
        if is_popout_label(&label) && window.is_visible().ok().unwrap_or(false) {
            Some(window)
        } else {
            None
        }
    })
}

fn sync_registered_popouts(app: &AppHandle) {
    let ids: std::collections::HashSet<String> =
        list_popout_conversation_ids(app).into_iter().collect();
    *app.state::<AppState>()
        .chat_popout_conversations
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = ids;
}

fn emit_popouts_changed(app: &AppHandle) {
    sync_registered_popouts(app);
    let conversation_ids = list_popout_conversation_ids(app);
    let _ = app.emit(
        POPOUTS_CHANGED_EVENT,
        PopoutsChangedPayload { conversation_ids },
    );
}

pub fn on_popout_destroyed(app: &AppHandle, label: &str) {
    protocol::unsubscribe_label(&app.state::<AppState>(), label);
    emit_popouts_changed(app);
}

#[cfg(target_os = "macos")]
pub fn sync_macos_activation_policy(app: &AppHandle) {
    let chat_visible = app
        .get_webview_window("chat")
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false);
    if chat_visible || has_open_popouts(app) {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    } else {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
}

pub fn close_popout_for_conversation(app: &AppHandle, conversation_id: &str) {
    let label = popout_label(conversation_id);
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.destroy();
    }
}

fn reveal_popout(app: &AppHandle, window: &WebviewWindow) {
    crate::windows::apply_chat_window_chrome(window);
    crate::windows::normalize_chat_window_behavior(window);
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

#[tauri::command]
pub fn chat_open_conversation_popout(
    app: AppHandle,
    conversation_id: String,
) -> Result<(), String> {
    if !valid_popout_conversation_id(&conversation_id) {
        return Err(format!("Invalid conversation id: {conversation_id}"));
    }
    let label = popout_label(&conversation_id);
    if let Some(window) = app.get_webview_window(&label) {
        reveal_popout(&app, &window);
        return Ok(());
    }
    let open = list_popout_conversation_ids(&app);
    if open.len() >= MAX_POPOUT_WINDOWS {
        return Err(format!(
            "最多同时打开 {MAX_POPOUT_WINDOWS} 个独立对话窗口"
        ));
    }
    let window = windows::ensure_chat_popout_window(&app, &label, &conversation_id)?;
    sync_registered_popouts(&app);
    emit_popouts_changed(&app);
    crate::windows::apply_chat_window_chrome(&window);
    crate::windows::normalize_chat_window_behavior(&window);
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    }
    Ok(())
}

#[tauri::command]
pub fn chat_focus_conversation_popout(
    app: AppHandle,
    conversation_id: String,
) -> Result<bool, String> {
    let Some(window) = app.get_webview_window(&popout_label(&conversation_id)) else {
        return Ok(false);
    };
    reveal_popout(&app, &window);
    Ok(true)
}

#[tauri::command]
pub fn chat_list_conversation_popouts(app: AppHandle) -> Vec<String> {
    list_popout_conversation_ids(&app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_roundtrip() {
        let id = "conv_abc-123";
        let label = popout_label(id);
        assert_eq!(label, "chat-popout-conv_abc-123");
        assert!(is_popout_label(&label));
        assert_eq!(conversation_id_from_label(&label), Some(id));
        assert!(!is_popout_label("chat"));
        assert!(conversation_id_from_label("chat").is_none());
    }

    #[test]
    fn rejects_unsafe_conversation_ids() {
        assert!(valid_popout_conversation_id("conv_a1-b2"));
        assert!(!valid_popout_conversation_id("conv_"));
        assert!(!valid_popout_conversation_id("../etc"));
        assert!(!valid_popout_conversation_id("chat-popout-x"));
    }
}
