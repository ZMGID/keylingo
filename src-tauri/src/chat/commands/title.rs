use std::time::Duration;

use tauri::State;
use tokio::time::timeout;

use crate::chat::agent::{execute::truncate_chars, stop as agent_stop};
use crate::chat::model_metadata::model_can_generate_images_directly;
use crate::chat::Conversation;
use crate::settings::{SessionModel, Settings};
use crate::state::AppState;


pub(super) async fn resolve_conversation_title(
    settings: &Settings,
    state: &State<'_, AppState>,
    conversation: &Conversation,
    user_content: &str,
    assistant_content: &str,
) -> String {
    let session = SessionModel {
        provider_id: conversation.provider_id.as_str(),
        model: conversation.model.as_str(),
    };
    match timeout(
        Duration::from_secs(8),
        generate_title_with_model(
            settings,
            state,
            &conversation.id,
            Some(session),
            user_content,
            assistant_content,
        ),
    )
    .await
    {
        Ok(Some(title)) => title,
        // 兜底必须留痕：这三条路都会安静地把标题变成「第一句用户消息」，用户只能看到
        // 「标题没总结」，而用量日志里连一条请求都没有（超时会把 future 丢掉、来不及记账）。
        Ok(None) => {
            eprintln!("[title] 跳过模型总结（provider/model 未解析或被门控），退回截断标题");
            generate_title(user_content)
        }
        Err(_) => {
            eprintln!("[title] 模型总结 8s 超时，退回截断标题");
            generate_title(user_content)
        }
    }
}

async fn generate_title_with_model(
    settings: &Settings,
    state: &State<'_, AppState>,
    conversation_id: &str,
    session: Option<SessionModel<'_>>,
    user_content: &str,
    assistant_content: &str,
) -> Option<String> {
    let (provider_id, model) = settings.effective_title_summary_model_for_session(session);
    let Some(provider) = settings.get_provider(&provider_id).cloned() else {
        eprintln!("[title] provider 未找到: {provider_id}");
        return None;
    };
    if provider.api_keys.is_empty() || model.trim().is_empty() {
        eprintln!("[title] provider 无 key 或 model 为空: {provider_id} / {model}");
        return None;
    }
    if model_can_generate_images_directly(&provider, &model) {
        eprintln!("[title] 生图模型不用于标题: {model}");
        return None;
    }

    let language = crate::settings::resolve_chat_language(settings);
    let prompt = build_title_summary_prompt(user_content, assistant_content, &language);
    let retry_attempts = if settings.retry_enabled {
        settings.retry_attempts as usize
    } else {
        1
    };
    let messages = vec![
        serde_json::json!({
            "role": "system",
            "content": title_summary_system_prompt(&language),
        }),
        serde_json::json!({
            "role": "user",
            "content": prompt,
        }),
    ];
    // **必须走流式**，不能用非流式 `generate`：部分 openai_responses 代理只可靠地服务流式
    // 请求，非流式调用直接报 "Unknown Responses API error"。压缩的摘要调用早就因为同一个原因
    // 改走流式了（见 planning.rs 上 call_chat_completion_message_streamed 的注释），当时的结论
    // 是「压缩是 agent 里唯一的非流式调用」—— 标题生成在 commands/ 里、不在 agent/ 里，被漏了。
    // 表现就是这类渠道上标题永远总结不出来，只能静默兜底成第一句用户消息。
    let message = crate::chat::agent::planning::call_chat_completion_message_streamed(
        state.inner(),
        &provider,
        &model,
        messages,
        None,
        retry_attempts,
        // **不发 reasoning effort**（`thinking_enabled: true` + level 不设 = 两个适配器都不写
        // 这个字段），交给端点自己的默认。
        //
        // 这里原来传 `false`，而 `false` 的语义是「显式下发关闭信号」：Responses 发
        // `reasoning.effort:"none"`、Chat 发 `reasoning_effort:"none"`。xAI 的档位只有
        // low/medium/high/xhigh，没有 none —— 实测两次真正发出去的标题请求全部失败
        // （grok-4.5：http_400 / http_503，用量日志里带着 reasoningEffort:"none"）。
        // 标题这种一次性小请求不值得为「关思考」去赌各家对 none 的支持度。
        true,
        // 标题本身十几个字就够，但思考型模型的 reasoning token 也吃这个预算，留点余量。
        2048,
        conversation_id,
        "",
        "Chat title summary",
    )
    .await
    .map_err(|err| {
        eprintln!("[title] 模型请求失败: {err}");
        err
    })
    .ok()?;
    let raw = agent_stop::assistant_content_from_api_message(&message);

    sanitize_generated_title(&raw)
}

fn title_summary_system_prompt(language: &str) -> &'static str {
    if language.starts_with("zh") {
        "你只负责为对话生成简洁标题。只输出标题本身，不要解释。"
    } else {
        "You only generate concise conversation titles. Output only the title, with no explanation."
    }
}

pub(super) fn build_title_summary_prompt(
    user_content: &str,
    assistant_content: &str,
    language: &str,
) -> String {
    let user = truncate_chars(user_content.trim(), 1200);
    let assistant = truncate_chars(assistant_content.trim(), 1200);
    if language.starts_with("zh") {
        format!(
            "请根据下面的首轮对话生成一个简洁中文标题。\n要求：只输出标题本身；不要引号；不要句号；不超过 14 个汉字，最多 20 个字符。\n\n用户：{user}\n\n助手：{assistant}"
        )
    } else {
        format!(
            "Create a concise English title for this first chat turn.\nRules: output only the title; no quotes; no period; 3-6 words.\n\nUser: {user}\n\nAssistant: {assistant}"
        )
    }
}

pub(super) fn sanitize_generated_title(raw: &str) -> Option<String> {
    let mut title = raw
        .trim()
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .to_string();

    title = title
        .trim_start_matches(['-', '*', '•', ' '])
        .trim_matches(['"', '\'', '`', '“', '”', '‘', '’', '。', '.', ' '])
        .to_string();
    for prefix in ["标题：", "标题:", "Title:", "Title：", "title:", "title："] {
        if let Some(rest) = title.strip_prefix(prefix) {
            title = rest.trim().to_string();
        }
    }
    title = title
        .trim_matches(['"', '\'', '`', '“', '”', '‘', '’', '。', '.', ' '])
        .to_string();
    if title.is_empty() {
        return None;
    }
    Some(generate_title(&title))
}

/// 生成对话标题（本地兜底截断）
pub(super) fn generate_title(content: &str) -> String {
    let trimmed = content.trim();
    let title = trimmed.chars().take(30).collect::<String>();
    if trimmed.chars().count() > 30 {
        format!("{title}...")
    } else {
        title
    }
}
