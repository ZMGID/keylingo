//! Desktop notification for a completed chat reply.
//!
//! The notification does not include model output, which keeps potentially
//! sensitive reply text off the lock screen.

use crate::chat::Conversation;
use crate::state::AppState;

pub(crate) fn notify_reply_completed(
    app: &tauri::AppHandle,
    state: &AppState,
    conversation: &Conversation,
) {
    let settings = state.settings_read();
    let language = crate::settings::resolve_chat_language(&settings);
    let (title, body) = completion_copy(&language, &conversation.title);
    crate::automation::notify::show(app, &title, &body);
}

fn completion_copy(language: &str, conversation_title: &str) -> (String, String) {
    let is_chinese = language.trim().to_ascii_lowercase().starts_with("zh");
    let conversation_title = conversation_title.trim();

    if is_chinese {
        let body = if conversation_title.is_empty() {
            "你的回复已经生成完成。".to_string()
        } else {
            format!("「{conversation_title}」的回复已经生成完成。")
        };
        ("Kivio · 回复已完成".to_string(), body)
    } else {
        let body = if conversation_title.is_empty() {
            "Your reply is ready.".to_string()
        } else {
            format!("Your reply in “{conversation_title}” is ready.")
        };
        ("Kivio · Reply ready".to_string(), body)
    }
}

#[cfg(test)]
mod tests {
    use super::completion_copy;

    #[test]
    fn builds_chinese_copy_without_exposing_reply_content() {
        let (title, body) = completion_copy("zh-CN", "季度总结");
        assert_eq!(title, "Kivio · 回复已完成");
        assert_eq!(body, "「季度总结」的回复已经生成完成。");
    }

    #[test]
    fn builds_english_fallback_for_an_untitled_conversation() {
        let (title, body) = completion_copy("en", "  ");
        assert_eq!(title, "Kivio · Reply ready");
        assert_eq!(body, "Your reply is ready.");
    }
}
