use std::time::Duration;

use tauri::{AppHandle, State};
use tokio::time::timeout;

use crate::chat::agent::{execute::truncate_chars, stop as agent_stop};
use crate::chat::model_metadata::model_can_generate_images_directly;
use crate::chat::ChatMessage;
use crate::settings::{SessionModel, Settings};
use crate::state::AppState;

const MAX_DRAFT_CHARS: usize = 4000;
const MAX_CONTEXT_CHARS: usize = 400;
const MAX_CONTEXT_MESSAGES: usize = 4;

/// Wire-level contract for the prompt-optimize side call.
/// Same two locks as title summary: must stream; must not send thinking-off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PromptOptimizeCallSpec {
    pub thinking_enabled: bool,
    pub max_output_tokens: u32,
    pub label: &'static str,
}

pub(super) const fn prompt_optimize_call_spec() -> PromptOptimizeCallSpec {
    PromptOptimizeCallSpec {
        thinking_enabled: true,
        max_output_tokens: 2048,
        label: "Chat prompt optimize",
    }
}

pub fn default_system_prompt(language: &str) -> &'static str {
    if language.starts_with("zh") {
        "你是提问优化助手。把用户的草稿改写成更清楚、具体、便于模型准确回答的问题。\n\
规则：\n\
- 只输出优化后的问题，不要解释、不要前缀、不要用引号或代码块包起来\n\
- 保留用户的意图、语言和人称；不要回答这个问题\n\
- 补全含糊处（目标、范围、约束、期望输出格式），但不要编造用户没给的事实\n\
- 已经写得足够清楚时只做轻微润色"
    } else {
        "You rewrite draft questions so a model can answer them accurately.\n\
Rules:\n\
- Output only the rewritten question: no explanation, prefix, quotes, or code fences\n\
- Keep the user's intent, language, and person; do not answer the question\n\
- Fill in vagueness (goal, scope, constraints, desired output) without inventing facts\n\
- If the draft is already clear, only lightly polish it"
    }
}

pub(super) fn build_optimize_user_prompt(
    draft: &str,
    recent_context: &str,
    language: &str,
) -> String {
    let text = truncate_chars(draft.trim(), MAX_DRAFT_CHARS);
    if language.starts_with("zh") {
        if recent_context.is_empty() {
            format!("请优化下面的提问：\n\n{text}")
        } else {
            format!("最近对话（供指代消解，不要回答）：\n{recent_context}\n\n请优化下面的提问：\n\n{text}")
        }
    } else if recent_context.is_empty() {
        format!("Rewrite this question:\n\n{text}")
    } else {
        format!(
            "Recent conversation (for resolving references; do not answer):\n{recent_context}\n\nRewrite this question:\n\n{text}"
        )
    }
}

pub(super) fn recent_conversation_context(messages: &[ChatMessage]) -> String {
    let mut turns: Vec<String> = Vec::new();
    for message in messages.iter().rev() {
        if turns.len() >= MAX_CONTEXT_MESSAGES {
            break;
        }
        let content = truncate_chars(message.content.trim(), MAX_CONTEXT_CHARS);
        if content.is_empty() {
            continue;
        }
        let label = if message.role == "assistant" {
            "助手"
        } else if message.role == "user" {
            "用户"
        } else {
            continue;
        };
        turns.push(format!("{label}：{content}"));
    }
    turns.reverse();
    turns.join("\n")
}

pub(super) fn sanitize_optimized_prompt(raw: &str) -> Option<String> {
    let mut text = raw.trim().to_string();
    if text.is_empty() {
        return None;
    }
    if text.starts_with("```") {
        let mut lines: Vec<&str> = text.lines().collect();
        if lines.first().is_some_and(|line| line.starts_with("```")) {
            lines.remove(0);
        }
        if lines.last().is_some_and(|line| line.trim() == "```") {
            lines.pop();
        }
        text = lines.join("\n").trim().to_string();
    }
    for prefix in [
        "优化后的问题：",
        "优化后的提问：",
        "优化后：",
        "Rewritten question:",
        "Rewritten:",
        "Optimized question:",
        "Optimized:",
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest.trim().to_string();
        }
    }
    text = text
        .trim_matches(['"', '\'', '`', '“', '”', '‘', '’'])
        .trim()
        .to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn localize(language: &str, zh: &str, en: &str) -> String {
    if language.starts_with("zh") {
        zh.to_string()
    } else {
        en.to_string()
    }
}

fn resolve_system_prompt(settings: &Settings, language: &str) -> String {
    let custom = settings.chat.prompt_optimize_prompt.trim();
    if custom.is_empty() {
        default_system_prompt(language).to_string()
    } else {
        custom.to_string()
    }
}

async fn optimize_prompt_with_model(
    settings: &Settings,
    state: &AppState,
    conversation_id: &str,
    session: Option<SessionModel<'_>>,
    draft: &str,
    recent_context: &str,
) -> Result<String, String> {
    let language = crate::settings::resolve_chat_language(settings);
    if draft.trim().is_empty() {
        return Err(localize(
            &language,
            "先输入要优化的问题",
            "Type a question to optimize",
        ));
    }
    if draft.trim().starts_with('/') {
        return Err(localize(
            &language,
            "斜杠命令无需优化",
            "Slash commands don't need optimizing",
        ));
    }

    let (provider_id, model) = settings.effective_prompt_optimize_model_for_session(session);
    let Some(provider) = settings.get_provider(&provider_id).cloned() else {
        return Err(localize(
            &language,
            "未找到可用模型，请先在混音器或模型设置里配置",
            "No usable model. Configure one in Mixer or Models first.",
        ));
    };
    if provider.api_keys.is_empty() || model.trim().is_empty() {
        return Err(localize(
            &language,
            "当前模型没有 API Key，请先在设置里填写",
            "This model has no API key. Add one in Settings.",
        ));
    }
    if model_can_generate_images_directly(&provider, &model) {
        return Err(localize(
            &language,
            "生图模型不能用于问题优化，请在混音器里另选模型",
            "Image models can't optimize questions. Pick another model in Mixer.",
        ));
    }

    let retry_attempts = if settings.retry_enabled {
        settings.retry_attempts as usize
    } else {
        1
    };
    let messages = vec![
        serde_json::json!({
            "role": "system",
            "content": resolve_system_prompt(settings, &language),
        }),
        serde_json::json!({
            "role": "user",
            "content": build_optimize_user_prompt(draft, recent_context, &language),
        }),
    ];
    let spec = prompt_optimize_call_spec();
    let message = crate::chat::agent::planning::call_chat_completion_message_streamed(
        state,
        &provider,
        &model,
        messages,
        None,
        retry_attempts,
        spec.thinking_enabled,
        spec.max_output_tokens,
        conversation_id,
        "",
        spec.label,
    )
    .await
    .map_err(|err| {
        localize(
            &language,
            &format!("问题优化失败：{err}"),
            &format!("Prompt optimize failed: {err}"),
        )
    })?;
    let raw = agent_stop::assistant_content_from_api_message(&message);
    sanitize_optimized_prompt(&raw).ok_or_else(|| {
        localize(
            &language,
            "模型没有返回可用的优化结果",
            "The model returned an empty rewrite",
        )
    })
}

/// Rewrite the composer draft into a clearer question. Does not send a chat turn.
#[tauri::command]
pub(crate) async fn chat_optimize_prompt(
    app: AppHandle,
    state: State<'_, AppState>,
    text: String,
    conversation_id: Option<String>,
) -> Result<String, String> {
    let settings = state.settings_read().clone();
    let mut recent_context = String::new();
    let mut session_owned: Option<(String, String)> = None;
    if let Some(id) = conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        if let Ok(snapshot) = crate::chat::repository::repository(&app)
            .get(&app, id)
            .await
        {
            recent_context = recent_conversation_context(&snapshot.messages);
            session_owned = Some((snapshot.provider_id.clone(), snapshot.model.clone()));
        }
    }
    let session = session_owned.as_ref().map(|(provider_id, model)| SessionModel {
        provider_id,
        model,
    });
    let conversation_id = conversation_id.unwrap_or_default();
    match timeout(
        Duration::from_secs(30),
        optimize_prompt_with_model(
            &settings,
            state.inner(),
            &conversation_id,
            session,
            &text,
            &recent_context,
        ),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err("问题优化超时，请重试".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ModelProvider;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    fn test_message(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: format!("msg_{}_{}", role, content.len()),
            role: role.to_string(),
            content: content.to_string(),
            attachments: Vec::new(),
            reasoning: None,
            artifacts: Vec::new(),
            tool_calls: Vec::new(),
            segments: Vec::new(),
            agent_plan: None,
            api_messages: Vec::new(),
            model_messages: Vec::new(),
            active_skill_id: None,
            run_entry: None,
            stream_outcome: None,
            usage: None,
            anchor_usage: None,
            group_id: None,
            provider_id: None,
            model: None,
            timestamp: 1,
            degraded: None,
        }
    }

    #[test]
    fn prompt_optimize_call_spec_stays_stream_friendly() {
        let spec = prompt_optimize_call_spec();
        assert!(spec.thinking_enabled);
        assert_eq!(spec.max_output_tokens, 2048);
        assert_eq!(spec.label, "Chat prompt optimize");
    }

    #[test]
    fn sanitize_strips_fences_and_prefixes() {
        assert_eq!(
            sanitize_optimized_prompt("```\n把这段代码按模块拆开\n```"),
            Some("把这段代码按模块拆开".to_string())
        );
        assert_eq!(
            sanitize_optimized_prompt("优化后的问题：明天北京下雨吗？"),
            Some("明天北京下雨吗？".to_string())
        );
        assert_eq!(sanitize_optimized_prompt("   "), None);
    }

    #[test]
    fn recent_context_keeps_last_user_and_assistant_turns() {
        let messages = vec![
            test_message("user", "第一问"),
            test_message("assistant", "第一答"),
            test_message("user", "那这个呢"),
        ];
        let context = recent_conversation_context(&messages);
        assert!(context.contains("用户：第一问"));
        assert!(context.contains("助手：第一答"));
        assert!(context.contains("用户：那这个呢"));
    }

    #[test]
    fn user_prompt_includes_draft_and_optional_context() {
        let plain = build_optimize_user_prompt("帮我看看这个", "", "zh");
        assert!(plain.contains("帮我看看这个"));
        assert!(!plain.contains("最近对话"));

        let with_ctx = build_optimize_user_prompt("那这个呢", "用户：第一问", "zh");
        assert!(with_ctx.contains("最近对话"));
        assert!(with_ctx.contains("那这个呢"));
    }

    fn test_app_state() -> AppState {
        let offline_models =
            crate::offline_models::OfflineModelManager::headless(reqwest::Client::new());
        AppState::base(
            Settings::default(),
            std::env::temp_dir().join(format!(
                "kivio-prompt-optimize-test-{}",
                uuid::Uuid::new_v4()
            )),
            reqwest::Client::new(),
            #[cfg(target_os = "macos")]
            crate::macos_ocr::MacOcrClient::disabled(),
            offline_models.clone(),
            crate::rapidocr::RapidOcrClient::new(offline_models),
        )
    }

    fn test_provider(base_url: &str) -> ModelProvider {
        ModelProvider {
            id: "optimize-provider".to_string(),
            name: "Optimize Provider".to_string(),
            api_keys: vec!["test-key".to_string()],
            api_key_legacy: None,
            base_url: base_url.to_string(),
            available_models: Vec::new(),
            enabled_models: Vec::new(),
            enabled: true,
            api_format: "openai_chat".to_string(),
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
            active_key_index: 0,
        }
    }

    fn start_sse_mock(events: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_thread = Arc::clone(&captured);
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .ok();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            let header_end = loop {
                let Ok(n) = stream.read(&mut chunk) else {
                    return;
                };
                if n == 0 {
                    return;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break pos + 4;
                }
            };
            let headers = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
            let content_length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            while buf.len() < header_end + content_length {
                let Ok(n) = stream.read(&mut chunk) else {
                    break;
                };
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let body = String::from_utf8_lossy(&buf[header_end..]).into_owned();
            captured_thread
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(body);

            let sse: String = events
                .iter()
                .map(|event| format!("data: {event}\n\n"))
                .collect();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse}",
                sse.len()
            );
            let _ = stream.write_all(response.as_bytes());
        });
        (format!("http://{addr}/v1"), captured)
    }

    #[tokio::test]
    async fn prompt_optimize_streams_and_keeps_thinking_on() {
        let (base_url, captured) = start_sse_mock(vec![
            r#"{"choices":[{"delta":{"content":"请根据模块边界拆分这段代码，并说明每处改动的原因。"}}]}"#.to_string(),
            "[DONE]".to_string(),
        ]);

        let state = test_app_state();
        let mut settings = Settings::default();
        settings.providers = vec![test_provider(&base_url)];
        settings.default_models.prompt_optimize.provider_id = "optimize-provider".into();
        settings.default_models.prompt_optimize.model = "optimize-model".into();
        settings.retry_enabled = false;

        let rewritten = optimize_prompt_with_model(
            &settings,
            &state,
            "conv_opt",
            None,
            "帮我看看这个",
            "",
        )
        .await
        .expect("rewrite from streamed model");

        assert!(rewritten.contains("拆分这段代码"));

        let bodies = captured.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(bodies.len(), 1, "exactly one optimize request");
        let body = &bodies[0];
        assert!(
            body.contains("\"stream\":true"),
            "prompt optimize must stream; body={body}"
        );
        assert!(
            !body.contains("\"reasoning_effort\":\"none\"")
                && !body.contains("\"effort\":\"none\""),
            "must not send explicit thinking-off; body={body}"
        );
        assert!(
            body.contains("optimize-model") && body.contains("提问优化助手"),
            "request should use the Chinese builtin prompt against optimize-model; body={body}"
        );
    }
}
