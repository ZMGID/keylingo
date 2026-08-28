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

/// 弹出窗以窗口 label 为协议过滤的权威来源。前端漏传 `conversationId` 时也不能订成 All，
/// 否则独占路由会把高频 token 丢掉，多窗口同时生成时弹出窗表现为直接断。
pub fn protocol_filter_for_window(
    label: &str,
    conversation_id: Option<String>,
) -> protocol::ChatProtocolFilter {
    if let Some(id) = conversation_id_from_label(label) {
        protocol::ChatProtocolFilter::Conversation(id.to_string())
    } else {
        match conversation_id {
            Some(id) if !id.trim().is_empty() => protocol::ChatProtocolFilter::Conversation(id),
            _ => protocol::ChatProtocolFilter::All,
        }
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

/// 打开一条对话的独立窗口。
///
/// **必须是 async command。** Windows 上同步 IPC 命令里调用
/// `WebviewWindowBuilder::build()` 会和 WebView2 的 `WebMessageReceived`
/// 死锁：前端 `invoke` 永不返回，聊天窗整窗卡住、点击像没反应。
/// 见 `tauri::WebviewWindowBuilder` Known issues / wry#583。
#[tauri::command]
pub async fn chat_open_conversation_popout(
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

/// 把已打开的弹出窗提到前台。同样走 async：`show`/`set_focus` 在 Windows
/// 上也不该从同步 IPC 回调里调。
#[tauri::command]
pub async fn chat_focus_conversation_popout(
    app: AppHandle,
    conversation_id: String,
) -> Result<bool, String> {
    let Some(window) = app.get_webview_window(&popout_label(&conversation_id)) else {
        return Ok(false);
    };
    reveal_popout(&app, &window);
    Ok(true)
}

/// 关掉独立窗口，把对话收回主聊天。`destroy` 走 async，避免 Windows 上
/// 同步 IPC 里动 WebView 窗口卡住（同 open/focus）。
#[tauri::command]
pub async fn chat_close_conversation_popout(
    app: AppHandle,
    conversation_id: String,
) -> Result<(), String> {
    close_popout_for_conversation(&app, &conversation_id);
    if let Some(chat) = app.get_webview_window("chat") {
        let _ = chat.unminimize();
        let _ = chat.show();
        let _ = chat.set_focus();
    }
    Ok(())
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
    fn popout_label_forces_conversation_filter_even_without_js_id() {
        assert_eq!(
            protocol_filter_for_window("chat-popout-conv_abc", None),
            protocol::ChatProtocolFilter::Conversation("conv_abc".into()),
        );
        assert_eq!(
            protocol_filter_for_window("chat", None),
            protocol::ChatProtocolFilter::All,
        );
        assert_eq!(
            protocol_filter_for_window("chat", Some("conv_abc".into())),
            protocol::ChatProtocolFilter::Conversation("conv_abc".into()),
        );
    }

    #[test]
    fn rejects_unsafe_conversation_ids() {
        assert!(valid_popout_conversation_id("conv_a1-b2"));
        assert!(!valid_popout_conversation_id("conv_"));
        assert!(!valid_popout_conversation_id("../etc"));
        assert!(!valid_popout_conversation_id("chat-popout-x"));
    }
}
