use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use base64::{engine::general_purpose, Engine as _};
use serde_json::Value;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::chat::agent::stop as agent_stop;
use crate::chat::model_metadata::{model_supports_image_generation, model_supports_vision};
use crate::mcp;
use crate::settings::{ModelProvider, SessionModel, Settings};
use crate::state::AppState;

use super::mcp_image_feedback::{
    append_tool_result_note, data_url_image_part, image_extension_for_mime,
    select_image_artifacts_for_attach,
};
use super::storage::load_conversation;
use super::{
    chat_missing_model_error, format_chat_missing_api_key_error, session_model_for_conversation,
    ToolCallRecord, ToolCallStatus,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AuxiliaryVisionModel {
    pub(super) provider_id: String,
    pub(super) provider_name: String,
    pub(super) model: String,
}

pub(super) fn auxiliary_vision_model_for_images(
    settings: &Settings,
    main_provider: Option<&ModelProvider>,
    main_model: &str,
    image_paths: &[PathBuf],
    session: Option<SessionModel<'_>>,
) -> Option<AuxiliaryVisionModel> {
    if image_paths.is_empty() {
        return None;
    }

    // 主模型自身支持视觉时，图片永远直接交给主模型——即便配置了独立视觉模型。
    // 独立视觉模型只是给「纯文本主模型」补视觉的兜底，不应把会看图的主模型降级走 mixer。
    if model_supports_vision(main_provider, main_model) == Some(true) {
        return None;
    }

    if settings.has_explicit_vision_model() {
        let (provider_id, model) = settings.effective_vision_model_for_session(session);
        return auxiliary_vision_model_from_selection(settings, &provider_id, &model);
    }

    if model_supports_vision(main_provider, main_model) != Some(false) {
        return None;
    }

    settings
        .providers
        .iter()
        .filter(|provider| provider.enabled)
        .flat_map(|provider| {
            provider
                .enabled_models
                .iter()
                .map(move |model| (provider, model))
        })
        .find_map(|(provider, model)| {
            if provider.id
                == main_provider
                    .map(|provider| provider.id.as_str())
                    .unwrap_or("")
                && model == main_model
            {
                return None;
            }
            if model_supports_vision(Some(provider), model) == Some(true)
                && model_supports_image_generation(Some(provider), model) != Some(true)
            {
                Some(AuxiliaryVisionModel {
                    provider_id: provider.id.clone(),
                    provider_name: provider.name.clone(),
                    model: model.clone(),
                })
            } else {
                None
            }
        })
}

fn auxiliary_vision_model_from_selection(
    settings: &Settings,
    provider_id: &str,
    model: &str,
) -> Option<AuxiliaryVisionModel> {
    let model = model.trim();
    if model.is_empty() {
        return None;
    }
    settings
        .get_provider(provider_id)
        .map(|provider| AuxiliaryVisionModel {
            provider_id: provider.id.clone(),
            provider_name: provider.name.clone(),
            model: model.to_string(),
        })
}

pub(super) struct AuxiliaryVisionResult {
    pub(super) provider_name: String,
    pub(super) model: String,
    pub(super) content: String,
}

pub(super) fn auxiliary_vision_tool_record(
    settings: &Settings,
    auxiliary_model: &AuxiliaryVisionModel,
    image_count: usize,
) -> ToolCallRecord {
    let provider_name = if auxiliary_model.provider_name.trim().is_empty() {
        auxiliary_model.provider_id.clone()
    } else {
        auxiliary_model.provider_name.clone()
    };
    ToolCallRecord {
        id: format!("call_mixer_vision_{}", Uuid::new_v4()),
        name: "mixer_vision".to_string(),
        source: "mixer".to_string(),
        server_id: Some(format!("{provider_name} / {}", auxiliary_model.model)),
        arguments: serde_json::json!({
            "task": "vision",
            "provider": provider_name,
            "model": auxiliary_model.model,
            "images": image_count,
            "auto": !settings.has_explicit_vision_model(),
        })
        .to_string(),
        status: ToolCallStatus::Running,
        result_preview: None,
        error: None,
        duration_ms: None,
        started_at: Some(chrono::Local::now().timestamp()),
        completed_at: None,
        round: 0,
        sensitive: false,
        artifacts: Vec::new(),
        trace_id: None,
        span_id: None,
        structured_content: None,
    }
}

pub(super) fn finish_auxiliary_vision_tool_record(
    record: &mut ToolCallRecord,
    status: ToolCallStatus,
    started: Instant,
    result_preview: Option<String>,
    error: Option<String>,
) {
    record.status = status;
    record.duration_ms = Some(started.elapsed().as_millis() as u64);
    record.completed_at = Some(chrono::Local::now().timestamp());
    record.result_preview = result_preview;
    record.error = error;
}

/// 会话里最后一条用户消息的正文（去空白，空则 None）。工具产出的图片走辅助视觉分析时，
/// 用它补上"用户到底想问什么"的上下文，与直接附图路径（reply.rs 传 last_user_api_content）对齐。
fn last_user_question(conversation: &super::Conversation) -> Option<&str> {
    conversation
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.trim())
        .filter(|c| !c.is_empty())
}

pub(super) async fn analyze_chat_images_with_auxiliary_model(
    state: &State<'_, AppState>,
    settings: &Settings,
    auxiliary_model: &AuxiliaryVisionModel,
    conversation_id: &str,
    message_id: &str,
    last_user_api_content: Option<&str>,
    image_paths: &[PathBuf],
    retry_attempts: usize,
    language: &str,
) -> Result<AuxiliaryVisionResult, String> {
    if image_paths.is_empty() {
        return Err("No image attachments to analyze".to_string());
    }
    let provider = settings
        .get_provider(&auxiliary_model.provider_id)
        .ok_or_else(|| "Vision auxiliary provider not found".to_string())?
        .clone();
    if provider.api_keys.is_empty() {
        return Err(format_chat_missing_api_key_error(&provider.name));
    }
    if auxiliary_model.model.trim().is_empty() {
        return Err(chat_missing_model_error());
    }

    let mut parts = image_paths
        .iter()
        .map(image_content_part)
        .collect::<Result<Vec<_>, _>>()?;
    parts.push(serde_json::json!({
        "type": "text",
        "text": auxiliary_vision_user_prompt(last_user_api_content, language),
    }));
    let messages = vec![
        serde_json::json!({
            "role": "system",
            "content": auxiliary_vision_system_prompt(language),
        }),
        serde_json::json!({
            "role": "user",
            "content": parts,
        }),
    ];
    // 走流式（见 planning.rs 上 call_chat_completion_output_with_usage / *_streamed 的注释）：
    // 非流式在部分 openai_responses 代理上直接报 "Unknown Responses API error"。
    let message = crate::chat::agent::planning::call_chat_completion_message_streamed(
        state.inner(),
        &provider,
        &auxiliary_model.model,
        messages,
        None,
        retry_attempts,
        // 不发 reasoning effort，交给端点默认（`false` 的语义是显式下发 "none"，不是不发；
        // 而 none 不在 xAI 的档位里，会 400）。
        true,
        4096,
        conversation_id,
        message_id,
        "Chat auxiliary vision analysis",
    )
    .await?;
    let content = agent_stop::assistant_content_from_api_message(&message);
    if content.trim().is_empty() {
        return Err("Vision auxiliary model returned an empty analysis".to_string());
    }
    Ok(AuxiliaryVisionResult {
        provider_name: provider.name,
        model: auxiliary_model.model.clone(),
        content,
    })
}

/// `read` 工具读到图片文件时的三级策略，复用对话级图片附件那套现成实现：
/// ① 主模型支持视觉 → 直喂原图（作为 follow-up user 消息，因为工具结果本身只能
/// 回文本）；② 纯文本主模型 → 辅助视觉模型出客观文字描述；③ 兜底 → OCR。
/// 失败/无视觉模型时逐级降级，始终返回一个可读的文本结果。
pub(super) async fn read_image_as_tool_result(
    app: &AppHandle,
    settings: &Settings,
    conversation_id: &str,
    message_id: &str,
    path: &Path,
) -> Result<mcp::types::McpToolCallResult, String> {
    use crate::mcp::native_registry::text_tool_result;
    // 直传 base64 不压缩；超大图片会撑爆上下文，故设上限兜底。
    // ponytail: 不压缩直传，12MB 上限兜底；上下文吃紧再加 resize helper。
    const MAX_IMAGE_BYTES: u64 = 12 * 1024 * 1024;

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image")
        .to_string();

    if let Ok(meta) = fs::metadata(path) {
        if meta.len() > MAX_IMAGE_BYTES {
            return Ok(text_tool_result(format!(
                "图片 {name} 过大（{} 字节，上限 {MAX_IMAGE_BYTES} 字节），未读取。请压缩后重试。",
                meta.len()
            )));
        }
    }

    let state = app.state::<AppState>();
    let conversation = load_conversation(app, conversation_id)?;
    let provider = settings.get_provider(&conversation.provider_id);
    let model = conversation.model.as_str();
    let path_buf = path.to_path_buf();

    // ① 主模型支持视觉 → 直喂原图。工具结果只能回文本，所以真正的图片作为紧随
    // 其后的一条 user 消息追加（rounds::push_tool_execution_result 负责排在 tool
    // 结果之后；Anthropic 侧会与 tool_result 合并进同一 user turn）。
    if model_supports_vision(provider, model) == Some(true) {
        let part = image_content_part(&path_buf)?;
        let follow_up = serde_json::json!({ "role": "user", "content": [part] });
        return Ok(mcp::types::McpToolCallResult {
            content: format!("已读取图片 {name}，已作为图片直接提供给你查看（见下一条消息）。"),
            is_error: false,
            raw: Value::Null,
            artifacts: Vec::new(),
            structured_content: None,
            follow_up_user_messages: vec![follow_up],
        });
    }

    // ② 纯文本主模型 → 辅助视觉模型出客观文字描述（复用对话级图片那套）。
    if let Some(aux) = auxiliary_vision_model_for_images(
        settings,
        provider,
        model,
        std::slice::from_ref(&path_buf),
        Some(session_model_for_conversation(&conversation)),
    ) {
        let language = crate::settings::resolve_chat_language(settings);
        let retry_attempts = if settings.retry_enabled {
            settings.retry_attempts as usize
        } else {
            1
        };
        if let Ok(result) = analyze_chat_images_with_auxiliary_model(
            &state,
            settings,
            &aux,
            conversation_id,
            message_id,
            last_user_question(&conversation),
            std::slice::from_ref(&path_buf),
            retry_attempts,
            &language,
        )
        .await
        {
            return Ok(text_tool_result(format!(
                "图片 {name} 的视觉分析（{} / {}）：\n\n{}",
                result.provider_name, result.model, result.content
            )));
        }
    }

    // ③ 兜底 OCR。
    match crate::chat::knowledge_base::process::process_document(
        state.inner(),
        &settings.document_processing,
        path,
    )
    .await
    {
        Ok(doc) => Ok(text_tool_result(format!(
            "图片 {name} 的 OCR 文本：\n\n{}",
            doc.text
        ))),
        Err(err) => Ok(text_tool_result(format!(
            "图片 {name}：当前模型不支持视觉，且无可用视觉模型，OCR 也未成功（{err}）。如需识别请在设置启用视觉模型或 OCR 引擎。"
        ))),
    }
}

/// R1：MCP 工具结果里的图片 artifact「直达模型」。通用于所有 MCP server（非
/// officecli 专属），复用 `read_image_as_tool_result` 已验证的两级策略：
/// ① 主模型支持视觉 → 把图片作为 follow-up user 消息直喂（`data_url_image_part`，
/// 不落盘）；② 纯文本主模型 → 落临时文件 `kivio-mcpimg-<uuid>.<ext>` 走辅助视觉
/// 模型做审查向分析（R2），把分析文字追加进 tool 结果的 content，随后删除临时
/// 文件。全程尽力而为：拿不到会话上下文、无可用视觉模型、分析失败等任何一步
/// 出错都原样保留 `[image: <mime>]` 占位符，不影响 MCP 工具调用本身的成败。
/// 仅对当前这一轮工具结果生效，不回填历史轮（调用方每轮都会重新执行）。
pub(super) async fn attach_image_artifacts_for_model(
    app: &AppHandle,
    settings: &Settings,
    conversation_id: &str,
    message_id: &str,
    result: &mut mcp::types::McpToolCallResult,
) {
    // 单图 8MB / 单结果 4 张，见 PRD R1 护栏约束。
    const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
    const MAX_IMAGES: usize = 4;

    let (accepted, guard_note) =
        select_image_artifacts_for_attach(&result.artifacts, MAX_IMAGE_BYTES, MAX_IMAGES);
    if accepted.is_empty() {
        if let Some(note) = guard_note {
            append_tool_result_note(result, &note);
        }
        return;
    }

    let conversation = match load_conversation(app, conversation_id) {
        Ok(conversation) => conversation,
        Err(_) => return, // 拿不到会话上下文，静默保留占位符
    };
    let provider = settings.get_provider(&conversation.provider_id);
    let model = conversation.model.as_str();

    // ① 主模型支持视觉 → 直喂原图（工具结果只能回文本，图片作为紧随其后的 user
    // 消息追加；rounds::push_tool_execution_result 负责排序，Anthropic 侧会与
    // tool_result 合并进同一 user turn，与 read 工具走的管子完全一致）。
    if model_supports_vision(provider, model) == Some(true) {
        let parts: Vec<Value> = accepted
            .iter()
            .filter_map(|(artifact, _)| data_url_image_part(&artifact.data_url).ok())
            .collect();
        if !parts.is_empty() {
            result
                .follow_up_user_messages
                .push(serde_json::json!({ "role": "user", "content": parts }));
            // 已直喂模型的图属于审查材料而非交付物：清掉 artifacts，
            // 聊天画廊不再重复展示（成品预览走 live preview / 交付目录）。
            result.artifacts.clear();
        }
        if let Some(note) = guard_note {
            append_tool_result_note(result, &note);
        }
        return;
    }

    // ② 纯文本主模型 → 落临时文件走辅助视觉模型（审查向分析，见 R2）。
    let mut temp_paths: Vec<PathBuf> = Vec::new();
    for (artifact, bytes) in &accepted {
        let ext = image_extension_for_mime(&artifact.mime_type);
        let path = std::env::temp_dir().join(format!("kivio-mcpimg-{}.{ext}", Uuid::new_v4()));
        if fs::write(&path, bytes).is_ok() {
            temp_paths.push(path);
        }
    }
    if temp_paths.is_empty() {
        if let Some(note) = guard_note {
            append_tool_result_note(result, &note);
        }
        return;
    }
    let cleanup_temp_paths = |paths: &[PathBuf]| {
        for path in paths {
            let _ = fs::remove_file(path);
        }
    };

    let Some(aux) = auxiliary_vision_model_for_images(
        settings,
        provider,
        model,
        &temp_paths,
        Some(session_model_for_conversation(&conversation)),
    ) else {
        cleanup_temp_paths(&temp_paths);
        if let Some(note) = guard_note {
            append_tool_result_note(result, &note);
        }
        return;
    };

    let state = app.state::<AppState>();
    let language = crate::settings::resolve_chat_language(settings);
    let retry_attempts = if settings.retry_enabled {
        settings.retry_attempts as usize
    } else {
        1
    };
    let analysis = analyze_chat_images_with_auxiliary_model(
        &state,
        settings,
        &aux,
        conversation_id,
        message_id,
        last_user_question(&conversation),
        &temp_paths,
        retry_attempts,
        &language,
    )
    .await;
    cleanup_temp_paths(&temp_paths);

    match analysis {
        Ok(analysis) => {
            let mut note = format!(
                "图片视觉分析（{} / {}）：\n\n{}",
                analysis.provider_name, analysis.model, analysis.content
            );
            if let Some(guard_note) = guard_note {
                note.push('\n');
                note.push_str(&guard_note);
            }
            append_tool_result_note(result, &note);
            // 同 vision 分支：审查材料不进聊天画廊。
            result.artifacts.clear();
        }
        Err(_) => {
            // 辅助视觉模型也失败：保留原占位符，只附上护栏提示（如果有）。
            if let Some(note) = guard_note {
                append_tool_result_note(result, &note);
            }
        }
    }
}

// R2：从纯「客观描述」升级为「审查+描述」——纯文本主模型读图（含 R1 的 MCP 图片
// 兜底路径）全靠这个函数出的文字判断画面对不对，之前只描述内容会漏掉字面 \n、
// 文字溢出等一看就是缺陷的问题（Gate 3 视觉审查假 PASS 的根因）。中英文都要求
// 逐条列出发现的问题，确无问题才允许写「未见视觉缺陷」。
fn auxiliary_vision_system_prompt(language: &str) -> &'static str {
    if language.starts_with("zh") {
        "你是 Kivio 的视觉副任务模型。你的任务是审查并描述用户提供的图片，输出给另一个主对话模型使用的文字观察。除描述图片中可见的信息、文字、结构、对象、界面状态和与用户问题相关的细节外，必须显式检查并逐条报告以下缺陷（如存在）：文字被截断或溢出容器、元素重叠、字面转义符（如按文字原样出现的 \\n、\\t）、明显的位置或对齐错位、对比度过低导致文字不可读。确认不存在以上问题才写「未见视觉缺陷」。不要回答最终问题，不要编造不可见内容。"
    } else {
        "You are Kivio's auxiliary vision model. Read the user's images and produce textual observations for another main chat model, combining description with review. Beyond describing visible information, text, layout, objects, UI state, and details relevant to the user's request, you must explicitly check and list any of the following defects if present: text truncated or overflowing its container, overlapping elements, literal escape sequences appearing as text (e.g. \\n, \\t), obvious position or alignment misalignment, and low-contrast unreadable text. Only state \"no visual defects observed\" once you have confirmed none of these are present. Do not answer the final question and do not invent unseen content."
    }
}

fn auxiliary_vision_user_prompt(last_user_api_content: Option<&str>, language: &str) -> String {
    let content = last_user_api_content
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default();
    if language.starts_with("zh") {
        if content.is_empty() {
            "请分析这些图片，输出主对话模型回答用户时需要知道的视觉事实。".to_string()
        } else {
            format!(
                "用户原始消息如下。请结合图片提取主对话模型回答时需要知道的视觉事实。\n\n{content}"
            )
        }
    } else if content.is_empty() {
        "Analyze these images and output the visual facts the main chat model needs.".to_string()
    } else {
        format!(
            "The user's original message is below. Extract the visual facts the main chat model needs to answer it.\n\n{content}"
        )
    }
}

pub(super) fn user_content_with_auxiliary_vision_result(
    last_user_api_content: Option<&str>,
    result: &AuxiliaryVisionResult,
    language: &str,
) -> String {
    let original = last_user_api_content
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default();
    let aux_block = if language.starts_with("zh") {
        format!(
            "[混音器视觉副任务结果]\n图片附件已由视觉模型（{} - {}）预先分析。主对话模型不能直接访问图片，请基于以下视觉观察回答用户：\n{}",
            result.provider_name,
            result.model,
            result.content.trim()
        )
    } else {
        format!(
            "[Mixer vision auxiliary result]\nThe image attachments were pre-analyzed by the vision model ({} - {}). The main chat model cannot access the images directly; answer using the visual observations below:\n{}",
            result.provider_name,
            result.model,
            result.content.trim()
        )
    };
    if original.is_empty() {
        aux_block
    } else {
        format!("{original}\n\n{aux_block}")
    }
}

pub(super) fn image_content_part(path: &PathBuf) -> Result<serde_json::Value, String> {
    let bytes = fs::read(path).map_err(|e| format!("读取图片附件失败: {e}"))?;
    let base64 = general_purpose::STANDARD.encode(bytes);
    let mime = image_mime_for_path(path);
    Ok(serde_json::json!({
        "type": "image_url",
        "image_url": { "url": format!("data:{mime};base64,{base64}") },
    }))
}

fn image_mime_for_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tiff" | "tif" => "image/tiff",
        "heic" => "image/heic",
        "heif" => "image/heif",
        _ => "image/png",
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::PathBuf};

    use crate::settings::{ModelProvider, Settings};

    use super::auxiliary_vision_model_for_images;

    fn test_provider(id: &str, name: &str, enabled_models: Vec<&str>) -> ModelProvider {
        ModelProvider {
            id: id.to_string(),
            name: name.to_string(),
            api_keys: vec!["sk-test".to_string()],
            api_key_legacy: None,
            base_url: "https://api.example.com/v1".to_string(),
            available_models: Vec::new(),
            enabled_models: enabled_models.into_iter().map(str::to_string).collect(),
            enabled: true,
            api_format: "openai_chat".to_string(),
            model_overrides: HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        }
    }

    #[test]
    fn auto_auxiliary_vision_picks_enabled_vision_model_when_main_is_text_only() {
        let mut settings = Settings::default();
        let main_provider = test_provider("main", "Main", vec!["deepseek-v4-flash"]);
        let vision_provider = test_provider("vision", "Vision", vec!["gpt-4o"]);
        settings.providers = vec![main_provider.clone(), vision_provider];

        let selected = auxiliary_vision_model_for_images(
            &settings,
            Some(&main_provider),
            "deepseek-v4-flash",
            &[PathBuf::from("image.png")],
            None,
        )
        .expect("auto should select a vision-capable model");

        assert_eq!(selected.provider_id, "vision");
        assert_eq!(selected.model, "gpt-4o");
    }

    #[test]
    fn auto_auxiliary_vision_keeps_images_on_main_when_main_supports_vision() {
        let mut settings = Settings::default();
        let main_provider = test_provider("main", "Main", vec!["gpt-4o"]);
        let vision_provider = test_provider("vision", "Vision", vec!["gemini-2.0-flash"]);
        settings.providers = vec![main_provider.clone(), vision_provider];

        assert_eq!(
            auxiliary_vision_model_for_images(
                &settings,
                Some(&main_provider),
                "gpt-4o",
                &[PathBuf::from("image.png")],
                None,
            ),
            None
        );
    }

    #[test]
    fn explicit_vision_model_does_not_hijack_vision_capable_main_model() {
        // 用户给主模型在 model_overrides 里手动开了 vision=true，同时设置里又配了独立视觉模型。
        // 期望：图片直接发给会看图的主模型，不走 mixer。回归 #vision-mixer-hijack。
        use crate::settings::{ModelCapabilities, ModelInfo};

        let mut main_provider = test_provider("main", "Main", vec!["models/gemini-3.1-flash-lite"]);
        main_provider.model_overrides.insert(
            "models/gemini-3.1-flash-lite".to_string(),
            ModelInfo {
                capabilities: Some(ModelCapabilities {
                    vision: Some(true),
                    ..ModelCapabilities::default()
                }),
                ..ModelInfo::default()
            },
        );
        let vision_provider = test_provider("vision", "Vision", vec!["gpt-4o"]);

        let mut settings = Settings::default();
        settings.providers = vec![main_provider.clone(), vision_provider];
        // 显式配置一个独立视觉模型（旧逻辑会因此把所有图片都劫持到 mixer）。
        settings.default_models.vision.provider_id = "vision".to_string();
        settings.default_models.vision.model = "gpt-4o".to_string();

        assert_eq!(
            auxiliary_vision_model_for_images(
                &settings,
                Some(&main_provider),
                "models/gemini-3.1-flash-lite",
                &[PathBuf::from("image.png")],
                None,
            ),
            None,
            "vision-capable main model should keep images, not route to the mixer"
        );
    }
}
