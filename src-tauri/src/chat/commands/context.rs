use std::path::{Path, PathBuf};

use serde_json::Value;
use tauri::{AppHandle, State};

use crate::chat::agent::prepare as agent_prepare;
use crate::chat::model::openai_messages_from_model_messages;
use crate::chat::session_model_for_conversation;
use crate::chat::model_metadata::context_window_for_model;
use crate::chat::storage::{live_set_system_prompt, load_conversation};
use crate::chat::{
    ChatMessage, CompactionBoundaryRecord, ContextUsageSegment, Conversation,
    ConversationContextState, ConversationContextSummary,
};
use crate::external_agents::detection::{
    EXTERNAL_AGENT_MODELS_CACHE_TTL, EXTERNAL_AGENT_MODELS_FALLBACK_TTL,
};
use crate::mcp::ChatToolDefinition;
use crate::settings::{ModelProvider, ProviderApiFormat};
use crate::skills;
use crate::state::AppState;

use super::catalog::{
    chat_memory_prompt_for_request, is_builder_conversation, project_prompt_context_for,
    strip_transcripts_for_frontend,
};
use super::sanitization::{sanitize_api_message_for_model, sanitize_image_payloads_for_model};
use super::{
    append_agent_ask_user_tools, append_agent_todo_tools, apply_agent_plan_tool_filter,
    apply_chat_mode_tool_filter, apply_inline_code_request_tool_filter, image_content_part,
    list_tools_for_chat, resolve_forced_skill_id,
};
use crate::chat::vision::auxiliary_vision_model_for_images;

#[tauri::command]
pub(crate) async fn chat_get_context_stats(
    app: AppHandle,
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<serde_json::Value, String> {
    let mut conversation = load_conversation(&app, &conversation_id)?;
    let context_state = if conversation.agent_runtime.is_external() {
        let cwd = crate::external_agents::workspace::resolve_effective_cwd(
            &app,
            &conversation.id,
            conversation.project_id.as_deref(),
        )?;
        crate::external_agents::context::compute_external_context_state_with_probe(
            &conversation,
            true,
            None,
            None,
            Some(&cwd),
            Some(&cwd),
        )
        .await
    } else {
        compute_context_state(&app, &state, &conversation, None, &[]).await?
    };
    conversation = persist_context_state_best_effort(
        &app,
        &conversation_id,
        conversation,
        context_state.clone(),
    )
    .await?;
    strip_transcripts_for_frontend(&mut conversation);
    Ok(serde_json::json!({
        "success": true,
        "contextState": context_state,
        "conversation": conversation,
    }))
}

/// 把刚算出来的上下文状态落盘。**抢不到版本不算失败**。
///
/// 上面那次计算是个 async 的慢活（外部 CLI 那条还要探模型），期间生成中的那一轮每落一条
/// 消息就把会话推进一版，于是拿着旧 revision 去写必然撞 `Conflict`。而这个命令语义上是
/// **读**（用户点刷新问「我的上下文多满」），落盘只是顺手缓存 —— 把缓存失败变成用户可见的
/// 红字是纯粹的噪声：面板里那条「conversation revision conflict (conv_…): expected 2,
/// actual 6」就是这么来的（生成中打开面板必现）。
///
/// 撞了先用**最新**的 revision 重试一次（多数情况下这一次就成了）；再撞就放弃落盘，直接把
/// 算出来的状态返回给前端 —— 反正轮末的权威计算会自己写一次。
async fn persist_context_state_best_effort(
    app: &AppHandle,
    conversation_id: &str,
    conversation: crate::chat::Conversation,
    context_state: crate::chat::ConversationContextState,
) -> Result<crate::chat::Conversation, String> {
    let repository = crate::chat::repository::repository(app);
    match repository
        .update_context(
            app,
            conversation_id,
            conversation.revision,
            context_state.clone(),
        )
        .await
    {
        Ok(updated) => Ok(updated),
        Err(crate::chat::repository::ConversationRepositoryError::Conflict { .. }) => {
            let latest = repository
                .get(app, conversation_id)
                .await
                .map_err(crate::chat::repository::repository_error)?;
            let revision = latest.revision;
            match repository
                .update_context(app, conversation_id, revision, context_state)
                .await
            {
                Ok(updated) => Ok(updated),
                // 还在被推进（生成中）⇒ 不落盘，返回刚读到的会话 + 算出来的状态。
                Err(crate::chat::repository::ConversationRepositoryError::Conflict { .. }) => {
                    Ok(latest)
                }
                Err(err) => Err(crate::chat::repository::repository_error(err)),
            }
        }
        Err(err) => Err(crate::chat::repository::repository_error(err)),
    }
}

#[tauri::command]
pub(crate) async fn chat_compress_context(
    app: AppHandle,
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<serde_json::Value, String> {
    let mut conversation = load_conversation(&app, &conversation_id)?;
    if conversation.agent_runtime.is_external() {
        crate::external_agents::compact::request_external_compaction(
            &app,
            &state,
            &mut conversation,
        )
        .await?;
        let context_state_after_compact = conversation.context_state.clone();
        // 同 `chat_get_context_stats`：压缩**已经发生**了，落盘缓存抢不到版本不该报错。
        conversation = persist_context_state_best_effort(
            &app,
            &conversation_id,
            conversation,
            context_state_after_compact.clone(),
        )
        .await?;
        // 用**压缩后算出来的**那份，不能读回 `conversation.context_state`：落盘被让位时
        // 上面返回的是重新读到的会话，它身上还是压缩前的状态。
        let context_state = context_state_after_compact;
        emit_chat_context_state(
            &app,
            &conversation.id,
            conversation.revision,
            &context_state,
        );
        strip_transcripts_for_frontend(&mut conversation);
        return Ok(serde_json::json!({
            "success": true,
            "contextState": context_state,
            "conversation": conversation,
        }));
    }
    compress_conversation_context(&state, &mut conversation, "manual").await?;
    let context_state = compute_context_state(&app, &state, &conversation, None, &[]).await?;
    conversation.context_state = context_state.clone();
    conversation = crate::chat::repository::repository(&app)
        .update_context(
            &app,
            &conversation_id,
            conversation.revision,
            context_state.clone(),
        )
        .await
        .map_err(crate::chat::repository::repository_error)?;
    emit_chat_context_state(
        &app,
        &conversation.id,
        conversation.revision,
        &context_state,
    );
    strip_transcripts_for_frontend(&mut conversation);
    Ok(serde_json::json!({
        "success": true,
        "contextState": context_state,
        "conversation": conversation,
    }))
}

const CONTEXT_BLOCK_RATIO: f32 = 1.0;
const IMAGE_ATTACHMENT_TOKEN_ESTIMATE: usize = 1_600;
const AUXILIARY_VISION_RESULT_TOKEN_ESTIMATE: usize = 800;

fn active_summary(conversation: &Conversation) -> Option<&ConversationContextSummary> {
    conversation
        .context_state
        .summary
        .as_ref()
        .filter(|summary| !summary.stale)
        .filter(|summary| !summary.content.trim().is_empty())
        .filter(|summary| {
            conversation
                .messages
                .iter()
                .any(|message| message.id == summary.source_until_message_id)
        })
}

fn summary_boundary_index(conversation: &Conversation) -> Option<usize> {
    let summary = active_summary(conversation)?;
    conversation
        .messages
        .iter()
        .position(|message| message.id == summary.source_until_message_id)
}

fn summary_message(summary: &ConversationContextSummary) -> Value {
    let mut content = format!(
        "{}\n{}",
        crate::chat::agent::compaction::PERSISTED_SUMMARY_PREFIX,
        summary.content.trim()
    );
    // Deterministic files-touched ledger, rendered under the LLM summary as a
    // factual floor (budget-isolated; never fed to the summarizer).
    if let Some(ledger) = &summary.file_ledger {
        let block = crate::chat::agent::file_ledger::render_block(ledger);
        if !block.is_empty() {
            content.push_str("\n\n");
            content.push_str(&block);
        }
    }
    serde_json::json!({
        "role": "system",
        "content": content,
    })
}

pub(super) fn mark_summary_stale_if_needed(conversation: &mut Conversation, changed_index: usize) {
    let Some(summary) = conversation.context_state.summary.as_mut() else {
        return;
    };
    let boundary_index = conversation
        .messages
        .iter()
        .position(|message| message.id == summary.source_until_message_id);
    if boundary_index
        .map(|boundary| changed_index <= boundary)
        .unwrap_or(true)
    {
        summary.stale = true;
        conversation.context_state.status = "stale".to_string();
    }
}

pub(super) fn count_tokens_in_value(value: &Value) -> usize {
    // 口径统一：委托压缩侧共用的 estimate_value_tokens（图片部件记 0，文本按文本，
    // 对象递归）。曾经两处各写一份，压缩侧漏了图片归零导致 base64 打爆估算。
    agent_prepare::estimate_value_tokens(value)
}

fn ceil_div_u32(value: u32, divisor: u32) -> usize {
    value.div_ceil(divisor) as usize
}

fn estimate_openai_tile_image_tokens(
    width: u32,
    height: u32,
    base_tokens: usize,
    tile_tokens: usize,
) -> usize {
    let mut scaled_width = width.max(1) as f64;
    let mut scaled_height = height.max(1) as f64;
    let longest = scaled_width.max(scaled_height);
    if longest > 2048.0 {
        let scale = 2048.0 / longest;
        scaled_width *= scale;
        scaled_height *= scale;
    }
    let shortest = scaled_width.min(scaled_height);
    if shortest > 768.0 {
        let scale = 768.0 / shortest;
        scaled_width *= scale;
        scaled_height *= scale;
    }
    let tiles = (scaled_width / 512.0).ceil().max(1.0) as usize
        * (scaled_height / 512.0).ceil().max(1.0) as usize;
    base_tokens + tiles * tile_tokens
}

fn estimate_openai_patch_image_tokens(
    width: u32,
    height: u32,
    patch_budget: usize,
    multiplier: f64,
    max_dimension: u32,
) -> usize {
    let patch_budget = patch_budget.max(1);
    let width = width.max(1);
    let height = height.max(1);
    let original_patches = ceil_div_u32(width, 32) * ceil_div_u32(height, 32);
    let mut scale = 1.0_f64;
    let longest = width.max(height);
    if longest > max_dimension.max(1) {
        scale = scale.min(max_dimension.max(1) as f64 / longest as f64);
    }
    if original_patches > patch_budget {
        let pixel_budget = patch_budget as f64 * 32.0 * 32.0;
        let shrink_factor = (pixel_budget / (width as f64 * height as f64)).sqrt();
        let target_width_patches = (width as f64 * shrink_factor) / 32.0;
        let target_height_patches = (height as f64 * shrink_factor) / 32.0;
        let width_adjust = target_width_patches.floor().max(1.0) / target_width_patches.max(1.0);
        let height_adjust = target_height_patches.floor().max(1.0) / target_height_patches.max(1.0);
        scale = scale.min(shrink_factor * width_adjust.min(height_adjust));
    }
    let mut scaled_width = ((width as f64 * scale).floor() as u32).max(1);
    let mut scaled_height = ((height as f64 * scale).floor() as u32).max(1);
    while ceil_div_u32(scaled_width, 32) * ceil_div_u32(scaled_height, 32) > patch_budget
        || scaled_width.max(scaled_height) > max_dimension.max(1)
    {
        scaled_width = ((scaled_width as f64 * 0.99).floor() as u32).max(1);
        scaled_height = ((scaled_height as f64 * 0.99).floor() as u32).max(1);
    }
    let patches = ceil_div_u32(scaled_width, 32) * ceil_div_u32(scaled_height, 32);
    (patches as f64 * multiplier).ceil() as usize
}

fn estimate_anthropic_image_tokens(model: &str, width: u32, height: u32) -> usize {
    let lower = model.to_ascii_lowercase();
    let high_resolution_opus = lower.contains("opus")
        && (lower.contains("4.7")
            || lower.contains("4-7")
            || lower.contains("4.8")
            || lower.contains("4-8"));
    let cap = if high_resolution_opus { 4_784 } else { 1_600 };
    ((width.max(1) as f64 * height.max(1) as f64) / 750.0)
        .ceil()
        .min(cap as f64) as usize
}

fn estimate_gemini_image_tokens(width: u32, height: u32) -> usize {
    if width <= 384 && height <= 384 {
        return 258;
    }
    let tiles = ceil_div_u32(width.max(1), 768) * ceil_div_u32(height.max(1), 768);
    tiles.max(1) * 258
}

fn provider_image_estimator_descriptor(provider: Option<&ModelProvider>, model: &str) -> String {
    let Some(provider) = provider else {
        return model.to_ascii_lowercase();
    };
    format!(
        "{} {} {} {}",
        provider.name, provider.base_url, provider.api_format, model
    )
    .to_ascii_lowercase()
}

pub(super) fn estimate_image_tokens_for_dimensions(
    provider: Option<&ModelProvider>,
    model: &str,
    width: u32,
    height: u32,
) -> usize {
    // Provider docs meter image context by pixels/tiles, not by base64 payload bytes.
    let descriptor = provider_image_estimator_descriptor(provider, model);
    if provider
        .map(|provider| provider.api_format_kind() == ProviderApiFormat::AnthropicMessages)
        .unwrap_or(false)
        || descriptor.contains("anthropic")
        || descriptor.contains("claude")
    {
        return estimate_anthropic_image_tokens(model, width, height);
    }
    if descriptor.contains("gemini")
        || descriptor.contains("google")
        || descriptor.contains("generativelanguage.googleapis.com")
    {
        return estimate_gemini_image_tokens(width, height);
    }

    if descriptor.contains("gpt-5.4-mini")
        || descriptor.contains("gpt-5-4-mini")
        || descriptor.contains("gpt-4.1-mini")
        || descriptor.contains("gpt-4-1-mini")
        || descriptor.contains("gpt-5-mini")
    {
        return estimate_openai_patch_image_tokens(width, height, 1_536, 1.62, 2_048);
    }
    if descriptor.contains("gpt-5.4-nano")
        || descriptor.contains("gpt-5-4-nano")
        || descriptor.contains("gpt-4.1-nano")
        || descriptor.contains("gpt-4-1-nano")
        || descriptor.contains("gpt-5-nano")
    {
        return estimate_openai_patch_image_tokens(width, height, 1_536, 2.46, 2_048);
    }
    if descriptor.contains("o4-mini") {
        return estimate_openai_patch_image_tokens(width, height, 1_536, 1.72, 2_048);
    }
    if descriptor.contains("gpt-5.5") || descriptor.contains("gpt-5-5") {
        return estimate_openai_patch_image_tokens(width, height, 10_000, 1.0, 6_000);
    }
    if descriptor.contains("gpt-5.4") || descriptor.contains("gpt-5-4") {
        return estimate_openai_patch_image_tokens(width, height, 2_500, 1.0, 2_048);
    }
    if descriptor.contains("gpt-4o-mini") {
        return estimate_openai_tile_image_tokens(width, height, 2_833, 5_667);
    }
    if descriptor.contains("gpt-5") {
        return estimate_openai_tile_image_tokens(width, height, 70, 140);
    }
    if descriptor.contains("o1") || descriptor.contains("o3") {
        return estimate_openai_tile_image_tokens(width, height, 75, 150);
    }
    if descriptor.contains("computer-use") {
        return estimate_openai_tile_image_tokens(width, height, 65, 129);
    }
    estimate_openai_tile_image_tokens(width, height, 85, 170)
}

fn estimate_image_tokens_for_path(
    provider: Option<&ModelProvider>,
    model: &str,
    path: &Path,
) -> usize {
    match image::image_dimensions(path) {
        Ok((width, height)) => estimate_image_tokens_for_dimensions(provider, model, width, height),
        Err(_) => IMAGE_ATTACHMENT_TOKEN_ESTIMATE,
    }
}

fn estimate_image_attachment_tokens(
    provider: Option<&ModelProvider>,
    model: &str,
    image_paths: &[PathBuf],
) -> usize {
    image_paths
        .iter()
        .map(|path| estimate_image_tokens_for_path(provider, model, path))
        .sum()
}

fn push_estimated_segment(
    segments: &mut Vec<ContextUsageSegment>,
    id: &str,
    label: &str,
    tokens: usize,
) {
    if tokens == 0 {
        return;
    }
    segments.push(ContextUsageSegment {
        id: id.to_string(),
        label: label.to_string(),
        estimated_tokens: tokens,
        color: agent_prepare::context_segment_color(id).map(str::to_string),
    });
}

fn estimate_tool_segments(tools: &[ChatToolDefinition]) -> Vec<ContextUsageSegment> {
    let mut segments = Vec::new();
    for tool in tools {
        let tool_value = tool.to_openai_tool();
        let id = match tool.source.as_str() {
            "mcp" => "mcp",
            "native" | "mixer" => "native_tools",
            "skill" => "skills",
            _ => "tool_definitions",
        };
        let label = match id {
            "mcp" => "MCP",
            "native_tools" => "Native tools",
            "skills" => "Skills",
            _ => "Tool definitions",
        };
        push_estimated_segment(&mut segments, id, label, count_tokens_in_value(&tool_value));
    }
    agent_prepare::merge_context_segments(segments)
}

fn estimate_messages_segments(
    conversation: &Conversation,
    messages: &[Value],
    attachment_tokens: usize,
) -> Vec<ContextUsageSegment> {
    let mut segments = Vec::new();
    let summary_tokens = active_summary(conversation)
        .map(|summary| agent_prepare::estimate_tokens(&summary.content))
        .unwrap_or_default();
    push_estimated_segment(
        &mut segments,
        "summarized_conversation",
        "Summarized conversation",
        summary_tokens,
    );

    let conversation_tokens = messages
        .iter()
        .filter(|message| {
            message
                .get("role")
                .and_then(|role| role.as_str())
                .map(|role| role != "system")
                .unwrap_or(true)
        })
        .map(count_tokens_in_value)
        .sum::<usize>();
    push_estimated_segment(
        &mut segments,
        "conversation",
        "Conversation",
        conversation_tokens,
    );
    push_estimated_segment(
        &mut segments,
        "attachments",
        "Attachments",
        attachment_tokens,
    );
    agent_prepare::merge_context_segments(segments)
}

/// 解析会话的真实用量锚点：从尾部找最近一条带 `anchor_usage` 且 provider 与当前一致的 assistant。
/// 返回 `(anchor_total_tokens, trailing_estimate)`：
/// - `anchor_total_tokens` = 该 assistant 上次调用「整个 prompt + 该次响应」的真实 token 总数
///   （含 output，按 provider 家族消歧，见 `context_estimate::anchor_total_tokens`）；
/// - `trailing_estimate` = 该 assistant **之后**（不含它本身，其 output 已计入锚点）到末尾所有消息的估算。
///
/// provider 与 `conversation.provider_id` 不一致（切换过供应商，计数口径不可比）、锚点消息之后发生过
/// 压缩（消息序列已变，旧计数失真，R4）或无 usage → `(None, 0)`，调用方回落纯字符估算。
/// 对齐 `context_estimate::effective_context_tokens` 的锚点口径。
pub(super) fn resolve_usage_anchor(
    conversation: &Conversation,
    provider: Option<&ModelProvider>,
) -> (Option<u64>, usize) {
    let Some(provider) = provider else {
        return (None, 0);
    };
    let api_format = provider.api_format.as_str();
    // 压缩边界失效（R4）：锚点消息生成后若发生过压缩（自动/手动），其记录的 token 数反映的是压缩前的
    // 完整历史，与压缩后实际发送的 prompt 不再可比——锚点作废。取最晚一次压缩时刻，任何时间戳 ≤ 该时刻的
    // assistant 锚点都视为失真（run 内自动压缩后仍会生成更晚的 assistant，其 anchor_usage 是压缩后调用
    // 值、时间戳晚于边界，不受影响）。
    let latest_compaction_at = conversation
        .context_state
        .compaction_boundaries
        .iter()
        .map(|b| b.created_at)
        .max();
    let anchor = conversation
        .messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(idx, message)| {
            if message.role != "assistant" {
                return None;
            }
            let usage = message.anchor_usage.as_ref()?;
            // provider 切换后旧锚点作废：单模型回退会话级 provider_id；多模型每条自带 provider_id。
            let msg_provider = message
                .provider_id
                .as_deref()
                .unwrap_or(&conversation.provider_id);
            if msg_provider != provider.id {
                return None;
            }
            // 压缩后锚点失真（R4）：边界晚于锚点消息 → 作废（回落纯估算）。
            if let Some(compacted_at) = latest_compaction_at {
                if compacted_at > message.timestamp {
                    return None;
                }
            }
            crate::chat::agent::context_estimate::anchor_total_tokens(usage, api_format)
                .map(|total| (idx, total))
        });
    match anchor {
        Some((idx, total)) => {
            // trailing = 锚点消息**之后**的消息（锚点消息本身的 output 已计入 total，故 idx+1..）。
            let trailing = conversation.messages[idx + 1..]
                .iter()
                .map(crate::chat::agent::compaction::estimate_chat_message_tokens)
                .sum();
            (Some(total), trailing)
        }
        None => (None, 0),
    }
}

pub(super) async fn compute_context_state(
    app: &AppHandle,
    state: &State<'_, AppState>,
    conversation: &Conversation,
    last_user_api_content: Option<&str>,
    last_user_image_paths: &[PathBuf],
) -> Result<ConversationContextState, String> {
    if conversation.agent_runtime.is_external() {
        // 缓存 key 必须与写入方 chat_detect_external_agent_models 一致：探测 cwd
        // （resolve_detection_cwd，非项目会话 = __global__），否则该读取恒 miss。
        let model_cache_key =
            crate::external_agents::workspace::resolve_detection_cwd(app, Some(&conversation.id))
                .ok()
                .and_then(|cwd| {
                    conversation
                        .agent_runtime
                        .external_agent_id
                        .as_deref()
                        .map(|agent_id| {
                            crate::external_agents::slash::cache_key(
                                agent_id,
                                cwd.to_string_lossy().as_ref(),
                            )
                        })
                });
        let cached_models = model_cache_key.as_deref().and_then(|cache_key| {
            state.get_cached_external_agent_models(
                cache_key,
                EXTERNAL_AGENT_MODELS_CACHE_TTL,
                EXTERNAL_AGENT_MODELS_FALLBACK_TTL,
            )
        });
        // 执行 cwd（resolve_effective_cwd，每会话独立 workspace）与上面的探测 cwd 是两回事：
        // 它只用于按 workDir 关联 kimi 落盘的 wire.jsonl（见 kimi_usage 模块）。拿不到不影响其余。
        let work_dir = crate::external_agents::workspace::resolve_effective_cwd(
            app,
            &conversation.id,
            conversation.project_id.as_deref(),
        )
        .ok();
        return Ok(
            crate::external_agents::context::compute_external_context_state_with_probe(
                conversation,
                false,
                None,
                cached_models.as_ref().map(|c| c.models.as_slice()),
                None,
                work_dir.as_deref(),
            )
            .await,
        );
    }

    let settings = state.settings_read().clone();
    let provider = settings.get_provider(&conversation.provider_id).cloned();
    let provider_available = provider.is_some();
    let language = crate::settings::resolve_chat_language(&settings);
    let thinking_enabled = settings.chat.thinking_enabled;
    let skill_cwd = crate::chat::storage::resolve_conversation_working_directory(
        app,
        conversation,
        &settings.chat_tools.native_tools.working_directory,
    )
    .ok();
    let skill_registry = skills::build_registry_in(
        app,
        &settings.chat_tools.skill_scan_paths,
        skill_cwd.as_deref(),
    )
    .unwrap_or_default();
    let requested_skill_id = conversation.active_skill_id.as_deref();
    let active_skill_id = resolve_forced_skill_id(
        &settings.chat_tools,
        conversation.assistant_snapshot.as_ref(),
        &skill_registry,
        requested_skill_id,
        crate::settings::obsidian_connector_configured(&settings.obsidian_vault_path),
    );
    let active_skill_detail = active_skill_id.as_deref().and_then(|id| {
        skills::read_skill_detail_in(
            app,
            &settings.chat_tools.skill_scan_paths,
            id,
            skill_cwd.as_deref(),
        )
        .ok()
    });
    let mut effective_chat_tools = settings.chat_tools.clone();
    let (memory_prompt, memory_warning) = chat_memory_prompt_for_request(app, &settings);
    let tools_capable = provider_available
        && agent_prepare::chat_tools_capable(
            &effective_chat_tools,
            settings.chat_memory.enabled,
            crate::settings::chat_image_generation_enabled_for_session(
                &settings,
                Some(session_model_for_conversation(conversation)),
            ),
        );
    let mut tools = list_tools_for_chat(
        app,
        state.inner(),
        &settings,
        Some(session_model_for_conversation(conversation)),
    )
    .await
    .tools;
    agent_prepare::apply_assistant_mcp_restrictions(
        &mut tools,
        conversation.assistant_snapshot.as_ref(),
    );
    if is_builder_conversation(conversation) {
        tools.clear();
        tools.push(crate::mcp::types::native_save_assistant_tool());
    }
    apply_inline_code_request_tool_filter(&mut tools, last_user_api_content);
    let chat_mode = conversation.agent_runtime.is_chat();
    let plan_mode = !chat_mode && crate::chat::plan::is_plan_mode(&conversation.agent_plan_state);
    if chat_mode {
        apply_chat_mode_tool_filter(&mut tools, true, &settings.chat.chat_mode);
    } else {
        apply_agent_plan_tool_filter(&mut tools, plan_mode);
    }
    let user_tools_available = tools_capable && !tools.is_empty();
    agent_prepare::apply_skill_fallback_when_tools_unavailable(
        &mut effective_chat_tools,
        active_skill_id.as_deref(),
        user_tools_available,
    );
    let ask_user_tools_available = append_agent_ask_user_tools(&mut tools);
    let todo_tools_available = if chat_mode {
        false
    } else {
        append_agent_todo_tools(&mut tools)
    };
    let runtime_tools_available = !tools.is_empty();
    let available_builtin_tools = agent_prepare::available_builtin_tool_names(&tools);
    let runtime_prompts = agent_prepare::resolve_runtime_prompt_sources(
        chat_mode,
        settings.chat.system_prompt.as_str(),
        settings.chat.chat_mode.system_prompt.as_str(),
        &conversation.agent_plan_state,
    );

    let route_images_through_auxiliary_vision = auxiliary_vision_model_for_images(
        &settings,
        provider.as_ref(),
        &conversation.model,
        last_user_image_paths,
        Some(session_model_for_conversation(conversation)),
    )
    .is_some();
    let empty_image_paths: &[PathBuf] = &[];
    let main_image_paths = if route_images_through_auxiliary_vision {
        empty_image_paths
    } else {
        last_user_image_paths
    };
    let attachment_tokens = if route_images_through_auxiliary_vision {
        last_user_image_paths.len() * AUXILIARY_VISION_RESULT_TOKEN_ESTIMATE
    } else {
        estimate_image_attachment_tokens(provider.as_ref(), &conversation.model, main_image_paths)
    };

    let set_system_prompt = live_set_system_prompt(app, conversation);
    let knowledge_base_prompt = crate::chat::knowledge_base::mount_system_prompt(
        app,
        &conversation.knowledge_base_ids,
        conversation.force_knowledge_search,
    );
    let obsidian_vault_path = (!settings.obsidian_vault_path.trim().is_empty())
        .then_some(settings.obsidian_vault_path.as_str());
    let (system_prompt, mut segments) = agent_prepare::build_chat_system_prompt_with_segments(
        &language,
        !main_image_paths.is_empty(),
        thinking_enabled,
        &skill_registry,
        &effective_chat_tools,
        runtime_tools_available,
        &available_builtin_tools,
        active_skill_id.as_deref(),
        active_skill_detail.as_ref(),
        conversation.assistant_snapshot.as_ref(),
        set_system_prompt.as_deref(),
        runtime_prompts.custom_system_prompt.as_str(),
        runtime_prompts.is_chat_runtime,
        memory_prompt.as_deref(),
        runtime_prompts.agent_plan_prompt.as_deref(),
        Some(&crate::chat::ask_user::format_prompt(
            ask_user_tools_available,
        )),
        if chat_mode {
            None
        } else {
            Some(crate::chat::todo::format_prompt(
                &conversation.agent_todo_state,
                todo_tools_available,
            ))
        }
        .as_deref(),
        project_prompt_context_for(app, conversation).as_ref(),
        crate::chat::storage::resolve_conversation_working_directory(
            app,
            conversation,
            &settings.chat_tools.native_tools.working_directory,
        )
        .ok()
        .map(|path| path.display().to_string())
        .as_deref(),
        knowledge_base_prompt.as_deref(),
        obsidian_vault_path,
    );
    let last_user_idx = conversation.messages.iter().rposition(|m| m.role == "user");
    let request_messages = build_chat_api_messages(
        // 估算路径：不 rehydrate 图片（token 口径不计图片字节，读几 MB 纯浪费）。
        None,
        &system_prompt,
        conversation,
        last_user_idx,
        last_user_api_content,
        main_image_paths,
    )?;
    segments.extend(estimate_messages_segments(
        conversation,
        &request_messages,
        attachment_tokens,
    ));

    if !tools.is_empty() {
        segments.extend(estimate_tool_segments(&tools));
    }

    let segments = agent_prepare::merge_context_segments(segments);
    let estimate_full = segments
        .iter()
        .map(|segment| segment.estimated_tokens)
        .sum::<usize>();
    // 真实用量锚点（对齐 pi/opencode）：有锚点时 footer 显示 provider 实报值 + 锚点后新增估算，
    // 否则回落纯字符估算。`effective_context_tokens` 取 `max(纯估算)` 作保守下限。
    let (anchor_prompt, anchor_trailing) = resolve_usage_anchor(conversation, provider.as_ref());
    let (estimated_input_tokens, anchored) =
        crate::chat::agent::context_estimate::effective_context_tokens(
            anchor_prompt,
            anchor_trailing,
            estimate_full,
        );
    let (context_window_tokens, context_window_estimated) =
        context_window_for_model(provider.as_ref(), &conversation.model);
    let usage_ratio = if context_window_tokens == 0 {
        None
    } else {
        Some(estimated_input_tokens as f32 / context_window_tokens as f32)
    };
    let summary = conversation.context_state.summary.clone();
    let status = context_status(usage_ratio, summary.as_ref());
    let last_compressed_at = summary
        .as_ref()
        .filter(|summary| !summary.stale)
        .map(|summary| summary.created_at)
        .or(conversation.context_state.last_compressed_at);
    let compressed_message_count = summary
        .as_ref()
        .filter(|summary| !summary.stale)
        .map(|summary| summary.source_message_ids.len())
        .unwrap_or_default();
    let mut compression_count = conversation.context_state.compression_count;
    if compression_count == 0 && active_summary(conversation).is_some() {
        compression_count = 1;
    }

    Ok(ConversationContextState {
        estimated_input_tokens,
        context_window_tokens: Some(context_window_tokens),
        context_window_estimated,
        usage_ratio,
        status,
        segments,
        last_measured_at: chrono::Local::now().timestamp(),
        last_compressed_at,
        compressed_message_count,
        compression_count,
        summary,
        compaction_boundaries: conversation.context_state.compaction_boundaries.clone(),
        warning: memory_warning.or_else(|| conversation.context_state.warning.clone()),
        context_source: Some(crate::external_agents::context::CONTEXT_SOURCE_BUILTIN.to_string()),
        token_count_source: if anchored {
            Some(crate::external_agents::context::TOKEN_COUNT_PROVIDER_REPORTED.to_string())
        } else {
            None
        },
        session_input_tokens: if anchored {
            Some(estimated_input_tokens)
        } else {
            None
        },
        session_output_tokens: None,
        external_agent_id: None,
        external_model: None,
    })
}

pub(super) fn context_likely_over_limit(context_state: &ConversationContextState) -> bool {
    context_state
        .usage_ratio
        .map(|ratio| ratio >= CONTEXT_BLOCK_RATIO)
        .unwrap_or(false)
}

pub(super) async fn rollback_user_message_after_failed_send(
    app: &AppHandle,
    state: &State<'_, AppState>,
    conversation: &mut Conversation,
    user_message_id: &str,
) -> Result<(), String> {
    conversation
        .messages
        .retain(|message| message.id != user_message_id);
    conversation.updated_at = chrono::Local::now().timestamp();
    match compute_context_state(app, state, conversation, None, &[]).await {
        Ok(mut context_state) => {
            context_state.warning = None;
            conversation.context_state = context_state.clone();
        }
        Err(context_err) => {
            eprintln!("Context usage estimate failed after send rollback: {context_err}");
        }
    }
    let context_state = conversation.context_state.clone();
    let persisted = crate::chat::repository::repository(app)
        .mutate(app, &conversation.id, |latest| {
            latest
                .messages
                .retain(|message| message.id != user_message_id);
            latest.context_state = context_state;
            Ok(())
        })
        .await
        .map_err(crate::chat::repository::repository_error)?;
    // 落盘之后再发：revision 必须是权威的那个，否则前端 applyEvent 会按旧 revision 丢弃。
    emit_chat_context_state(
        app,
        &persisted.id,
        persisted.revision,
        &persisted.context_state,
    );
    *conversation = persisted;
    Ok(())
}

pub(super) fn should_auto_compress_context(
    context_state: &ConversationContextState,
    conversation: &Conversation,
) -> bool {
    if conversation.agent_runtime.is_external() {
        return false;
    }
    let Some(ratio) = context_state.usage_ratio else {
        return false;
    };
    if ratio < crate::chat::agent::compaction::AUTO_COMPACT_RATIO {
        return false;
    }
    crate::chat::agent::compaction::has_compressible_old_segment(conversation)
}

pub(super) async fn try_auto_compress_context_after_update(
    app: &AppHandle,
    state: &State<'_, AppState>,
    conversation: &mut Conversation,
    last_user_api_content: Option<&str>,
    last_user_image_paths: &[PathBuf],
) {
    if !should_auto_compress_context(&conversation.context_state, conversation) {
        return;
    }
    match compress_conversation_context(state, conversation, "auto").await {
        Ok(()) => {
            match compute_context_state(
                app,
                state,
                conversation,
                last_user_api_content,
                last_user_image_paths,
            )
            .await
            {
                Ok(refreshed) => {
                    // 不清 warning：compute_context_state 已保留 compact_conversation 设好的
                    // decay_warning_for(count)（R-4 多次压缩准确度提示）；此处若置 None 会把它抹掉，
                    // 与另一条自动压缩路径（见上方 auto-compress 分支）行为不一致。
                    conversation.context_state = refreshed;
                }
                Err(err) => {
                    eprintln!("Context usage estimate failed after auto compression: {err}");
                }
            }
        }
        Err(err) => {
            eprintln!("Auto context compression failed: {err}");
            conversation.context_state.warning =
                Some(format!("Automatic compression failed: {err}."));
        }
    }
}

pub(super) async fn compress_conversation_context(
    state: &State<'_, AppState>,
    conversation: &mut Conversation,
    trigger: &str,
) -> Result<(), String> {
    let settings = state.settings_read().clone();
    crate::chat::agent::compaction::compact_conversation(
        state.inner(),
        &settings,
        conversation,
        trigger,
        None,
    )
    .await
}

pub(super) fn emit_chat_context_state(
    app: &AppHandle,
    conversation_id: &str,
    revision: u64,
    context_state: &ConversationContextState,
) {
    crate::chat::protocol::emit_conversation_event(
        app,
        conversation_id,
        revision,
        crate::chat::protocol::ChatConversationEvent::ContextUpdated {
            context_state: context_state.into(),
        },
    );
}

/// **生成过程中**的上下文占用（分子 + 分母）。
///
/// 复用统一协议的 context update（不新造事件）：同一个主题、同一个订阅者、同一套
/// conversationId 路由。载荷用 `live` 与权威快照 `contextState` 区分——权威快照那条要读磁盘、
/// 连 MCP 列工具、算分段，绝不能放在每个增量上（spec 第 9 条的精神）；这条只带两个数，
/// 前端就地更新分子/分母与比例，其余字段（分段、压缩计数、来源标签）留给轮末的权威计算。
///
/// **唯一的生产者是内置 agent 的压缩检查**（`chat/agent/compaction.rs`），粒度是**每个
/// planning 轮一次**，两个数与轮末权威计算出自同一对函数（`anchor_total_tokens` +
/// `effective_context_tokens`，分母同样是 `context_window_for_model`），所以轮末不会跳。
///
/// 外部 CLI **不再走这条**：那边曾有一条 350ms 节流的通道，分子是单次请求快照（工具循环里
/// 每请求一变、压缩后还会掉）、分母是上一轮的粘滞值、分段是前端缩放出来的，看着就是在跳。
/// 现在外部 CLI 的占用一轮只更新一次，由轮末权威计算发出。别再给这个函数加第二个生产者
/// 之前先想清楚那三点。
///
/// `context_window_tokens` 为 `None` = 本次上报没带窗口，**前端必须保留已知的旧值**
/// （分母粘滞，见 `applyLiveContextUsage`）。
pub(crate) fn emit_chat_context_usage_live(
    app: &AppHandle,
    _conversation_id: &str,
    run_id: &str,
    used_tokens: u64,
    context_window_tokens: Option<u64>,
) {
    crate::chat::protocol::emit_run_event(
        app,
        run_id,
        crate::chat::protocol::ChatRunEvent::ContextUsageUpdated {
            usage: crate::chat::protocol::ChatContextUsagePayload {
                used_tokens,
                context_window_tokens,
            },
        },
    );
}

pub(super) fn emit_chat_compaction_state(
    app: &AppHandle,
    _conversation_id: &str,
    run_id: &str,
    phase: &str,
    trigger: Option<&str>,
    boundary: Option<&CompactionBoundaryRecord>,
) {
    crate::chat::protocol::emit_run_event(
        app,
        run_id,
        crate::chat::protocol::ChatRunEvent::CompactionUpdated {
            phase: phase.to_string(),
            trigger: trigger.map(str::to_string),
            boundary: boundary.map(Into::into),
        },
    );
}

fn context_status(
    usage_ratio: Option<f32>,
    summary: Option<&ConversationContextSummary>,
) -> String {
    if summary.is_some_and(|item| item.stale) {
        return "stale".to_string();
    }
    if summary.is_some() {
        return "compressed".to_string();
    }
    let Some(ratio) = usage_ratio else {
        return "unknown".to_string();
    };
    if ratio >= 0.95 {
        "critical".to_string()
    } else if ratio >= 0.70 {
        "warning".to_string()
    } else {
        "normal".to_string()
    }
}

// 任务 06-30 步骤 0 核对结论：token 估算与历史拼装**同源**——`compute_context_state`
// （commands.rs 内）直接调用本函数得到 `request_messages`，再用 `estimate_messages_segments`
// 在这份消息上估 token。因此后续步骤（步骤 4）在本函数循环里对「多答组只保留选中条」
// 做过滤后，token 估算会自动排除未选中条，**无需在 `compute_context_state` 另行过滤**。

/// 多答组（任务 06-30）历史过滤：判断某条带 `group_id` 的 assistant 消息是否应排除出上下文。
/// 规则（决策 D5）：同一 `group_id` 只保留「选中条」——
/// - `conversation.group_selections[group_id]` 指定的 message_id；
/// - 无记录则取该组在 `messages` 中**顺序第一条** assistant。
/// 其余答案仅保留展示、排除出发给模型的历史（R6）。非多答消息（无 group_id）一律保留。
/// `pub(crate)`：落盘压缩（compaction.rs）复用同一谓词，保证摘要输入与 replay 视图口径一致。
pub(crate) fn group_answer_excluded_from_context(
    conversation: &Conversation,
    message: &ChatMessage,
) -> bool {
    let Some(group_id) = message.group_id.as_deref() else {
        return false;
    };
    if message.role != "assistant" {
        return false;
    }
    let selected = conversation
        .group_selections
        .get(group_id)
        .map(String::as_str)
        .or_else(|| {
            // 无显式选择时，优先保留该组第一条「非错误」assistant 进上下文，跳过
            // stream_outcome == "error" 的失败臂——否则失败臂的错误文案会作为上一轮
            // 答案回灌给模型。全组皆 error（罕见）时才退回顺序第一条。
            let in_group =
                |m: &&ChatMessage| m.role == "assistant" && m.group_id.as_deref() == Some(group_id);
            conversation
                .messages
                .iter()
                .find(|m| in_group(m) && m.stream_outcome.as_deref() != Some("error"))
                .or_else(|| conversation.messages.iter().find(in_group))
                .map(|m| m.id.as_str())
        });
    selected != Some(message.id.as_str())
}

/// 给一条 runtime 消息标注来源 UI 消息 id（`_ui_message_id`）。
/// 该字段只存在于运行期视图：发给 provider 前会经 `model_message_from_openai_message`
/// 只抽取已知字段，未知字段天然被剥离，不会进任何 wire 请求。压缩落盘时
/// `compaction::source_until_message_id_for_split` 据此把 runtime 旧段精确映射回 UI 消息。
fn tag_ui_message_id(mut message: Value, ui_message_id: &str) -> Value {
    if let Some(obj) = message.as_object_mut() {
        obj.insert(
            "_ui_message_id".to_string(),
            Value::String(ui_message_id.to_string()),
        );
    }
    message
}

/// `app` 为 `None` 时跳过图片 rehydrate（纯估算调用方不需要真实 base64——token 口径
/// 本来就不计图片字节）。真正要发给模型的路径必须传 `Some`。
pub(super) fn build_chat_api_messages(
    app: Option<&AppHandle>,
    system_prompt: &str,
    conversation: &Conversation,
    last_user_idx: Option<usize>,
    last_user_api_content: Option<&str>,
    last_user_image_paths: &[PathBuf],
) -> Result<Vec<Value>, String> {
    let mut messages = vec![serde_json::json!({
        "role": "system",
        "content": system_prompt,
    })];

    // 有 active summary 时：注入一条 system role 的 `Previous conversation summary:`，
    // 之后只 replay boundary 之后的原文。boundary 由 token 预算决定（compaction::token_split_chat_messages，
    // recent tail ≤ RECENT_KEEP_TOKENS）；boundary 之前的原文已被摘要覆盖、不重发。
    // 当累计再增长到裸窗口 90% 时会触发再次压缩（auto / agent_loop）。
    let start_idx = if let Some(summary) = active_summary(conversation) {
        messages.push(summary_message(summary));
        summary_boundary_index(conversation)
            .map(|idx| idx + 1)
            .unwrap_or_default()
    } else {
        0
    };

    for (idx, message) in conversation.messages.iter().enumerate() {
        if idx < start_idx {
            continue;
        }
        // 多答组：仅保留选中条，其余答案不进发给模型的上下文（R6 / AC4）。
        if group_answer_excluded_from_context(conversation, message) {
            continue;
        }
        let content = if Some(idx) == last_user_idx {
            last_user_api_content.unwrap_or(message.content.as_str())
        } else {
            message.content.as_str()
        };
        let sanitized_content = sanitize_image_payloads_for_model(content);
        if Some(idx) == last_user_idx && !last_user_image_paths.is_empty() {
            let mut parts = last_user_image_paths
                .iter()
                .map(image_content_part)
                .collect::<Result<Vec<_>, _>>()?;
            parts.push(serde_json::json!({ "type": "text", "text": sanitized_content }));
            messages.push(tag_ui_message_id(
                serde_json::json!({
                    "role": message.role,
                    "content": parts,
                }),
                &message.id,
            ));
        } else {
            messages.push(tag_ui_message_id(
                serde_json::json!({
                    "role": message.role,
                    "content": sanitized_content,
                }),
                &message.id,
            ));
        }
        if message.role == "assistant" && !message.model_messages.is_empty() {
            messages.pop();
            // 落盘的图片部件只有 path，没 base64（见 attachments::externalize_model_message_images）。
            // 展开成 wire 格式之前先按 path 读盘填回来，模型看到的内容与当初一字不差。
            let mut model_messages = message.model_messages.clone();
            if let Some(app) = app {
                crate::chat::attachments::rehydrate_model_message_images(
                    app,
                    &conversation.id,
                    &mut model_messages,
                );
            }
            messages.extend(
                openai_messages_from_model_messages(&model_messages)
                    .iter()
                    .map(sanitize_api_message_for_model)
                    .map(|expanded| tag_ui_message_id(expanded, &message.id)),
            );
        } else if message.role == "assistant" && !message.api_messages.is_empty() {
            messages.pop();
            // 与上面 model_messages 同理：中断草稿的 `api_messages` 里图片已被外置成
            // `kivio-attachment://` 哨兵，发给模型前必须还原成 data URL。
            let mut api_messages = message.api_messages.clone();
            if let Some(app) = app {
                crate::chat::attachments::rehydrate_api_message_images(
                    app,
                    &conversation.id,
                    &mut api_messages,
                );
            }
            messages.extend(
                api_messages
                    .iter()
                    .map(sanitize_api_message_for_model)
                    .map(|expanded| tag_ui_message_id(expanded, &message.id)),
            );
        }
    }

    Ok(messages)
}
