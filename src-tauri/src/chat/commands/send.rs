use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::chat::attachments::{
    compose_text_attachments_for_api, compose_user_content_for_api, save_message_attachments,
    stored_image_paths_for_attachments, title_source_for_user_message, TextAttachmentInput,
};
use crate::chat::storage::{conversation_attachments_dir, load_conversation};
use crate::chat::Attachment;
use crate::chat::ChatMessage;
use crate::skills;
use crate::state::AppState;

use super::catalog::strip_transcripts_for_frontend;
use super::complete_assistant_reply;
use super::context::{
    compress_conversation_context, compute_context_state, context_likely_over_limit,
    emit_chat_context_state, rollback_user_message_after_failed_send, should_auto_compress_context,
};
use super::fan_out::run_reply_fan_out;
use super::reply_runtime::{resolve_reply_arms, ChatSendReservation, CHAT_REPLY_BUSY_ERROR};
use super::tooling::try_apply_skill_slash_trigger;

/// 发送消息
#[tauri::command]
pub(crate) async fn chat_send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    conversation_id: String,
    content: String,
    attachments: Vec<String>,
    text_attachments: Option<Vec<TextAttachmentInput>>,
    active_skill_id: Option<String>,
) -> Result<serde_json::Value, String> {
    // 内存文本附件（粘贴长文本虚拟 txt）：前端总传；缺省为空数组以兼容旧调用。
    let text_attachments = text_attachments.unwrap_or_default();
    // Busy 拒绝：该会话仍有任意一条 run 在跑（含多模型并发组）时不允许再发新消息。
    // 用原子的哨兵预留替代「先 check 后 register」，关闭并发发送同时通过 busy 检查的 TOCTOU 窗口。
    // 哨兵在本命令返回前一直存活；实际的 per-run 槽位 / generation 在 `complete_assistant_reply`
    // 内 run_id 生成处额外注册，与哨兵按不同 run_id 共存。
    let Some(_send_reservation) = ChatSendReservation::try_acquire(state.inner(), &conversation_id)
    else {
        return Ok(serde_json::json!({
            "success": false,
            "error": CHAT_REPLY_BUSY_ERROR,
        }));
    };

    let mut conversation = load_conversation(&app, &conversation_id)?;

    // Backend slash-trigger preprocessing (承重路径): plain text `/commit msg`
    // pins the skill and rewrites the body even without the front-end popover
    // (also covers paste / external API / mobile entry points).
    // External CLI conversations pass slash commands straight through to the agent.
    let (content, active_skill_id) = if conversation.agent_runtime.is_external() {
        (content, active_skill_id)
    } else {
        let settings = state.settings_read().clone();
        let skill_cwd = crate::chat::storage::resolve_conversation_working_directory(
            &app,
            &conversation,
            &settings.chat_tools.native_tools.working_directory,
        )
        .ok();
        let registry = skills::build_registry_in(
            &app,
            &settings.chat_tools.skill_scan_paths,
            skill_cwd.as_deref(),
        )
        .unwrap_or_default();
        match try_apply_skill_slash_trigger(
            &registry,
            &settings.chat_tools,
            conversation.assistant_snapshot.as_ref(),
            &content,
            crate::settings::obsidian_connector_configured(&settings.obsidian_vault_path),
        ) {
            Some((skill_id, rewritten)) => (rewritten, Some(skill_id)),
            None => (content, active_skill_id),
        }
    };

    let mut message_attachments = save_message_attachments(&app, &conversation_id, attachments)?;
    // 虚拟文本附件（memory:// 标记、不落盘）：持久化附件记录（含正文，随对话消息保存，
    // 不生成独立磁盘文件），使回复完成后已发送消息的气泡仍能展示并可点击查看内容。
    for text_attachment in &text_attachments {
        message_attachments.push(Attachment {
            id: format!("att-mem-{}", Uuid::new_v4()),
            attachment_type: "file".to_string(),
            name: text_attachment.name.clone(),
            path: format!("memory://{}", Uuid::new_v4()),
            content: Some(text_attachment.content.clone()),
        });
    }
    let attachments_dir = if message_attachments.is_empty() {
        None
    } else {
        Some(conversation_attachments_dir(&app, &conversation_id)?)
    };
    let mut api_content =
        compose_user_content_for_api(&content, &message_attachments, attachments_dir.as_deref());
    // 内存文本附件（虚拟 txt）正文直接内联进 API 内容，不落盘、不持久化。
    if !text_attachments.is_empty() {
        api_content = compose_text_attachments_for_api(&api_content, &text_attachments);
    }
    let title_source = title_source_for_user_message(&content, &message_attachments);
    let last_user_image_paths =
        stored_image_paths_for_attachments(&app, &conversation_id, &message_attachments)?;

    // 多模型一问多答（任务 06-30）：从会话级 reply_models 解析本次要并行的「臂」。
    // 0/1 个有效臂 → 单模型现状路径（行为完全不变，防回归 AC5）。≥2 → fan-out。
    // 仅普通（Act）模式生效（R11）：plan / orchestrate 模式下不 fan-out。
    let reply_arms = {
        let settings = state.settings_read();
        resolve_reply_arms(&settings, &conversation.reply_models)?
    };
    let plan_or_orchestrate = crate::chat::plan::is_plan_mode(&conversation.agent_plan_state)
        || crate::chat::plan::is_orchestrate_mode(&conversation.agent_plan_state);
    let fan_out = reply_arms.len() >= 2 && !plan_or_orchestrate;
    // fan-out 时所有臂共享一个 group_id；用户消息也打上它，便于前端把这一问的 N 答聚成一组。
    let group_id = if fan_out {
        Some(format!("grp_{}", Uuid::new_v4()))
    } else {
        None
    };

    // 创建用户消息
    let user_message = ChatMessage {
        id: format!("msg_{}", Uuid::new_v4()),
        role: "user".to_string(),
        content: content.clone(),
        attachments: message_attachments,
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
        group_id: group_id.clone(),
        provider_id: None,
        model: None,
        timestamp: chrono::Local::now().timestamp(),
        degraded: None,
    };

    conversation.messages.push(user_message.clone());
    conversation.updated_at = chrono::Local::now().timestamp();
    conversation = crate::chat::repository::repository(&app)
        .append_message(&app, &conversation_id, user_message.clone())
        .await
        .map_err(crate::chat::repository::repository_error)?;

    match compute_context_state(
        &app,
        &state,
        &conversation,
        Some(api_content.as_str()),
        &last_user_image_paths,
    )
    .await
    {
        Ok(context_state) => {
            conversation.context_state = context_state;
            if should_auto_compress_context(&conversation.context_state, &conversation) {
                match compress_conversation_context(&state, &mut conversation, "auto").await {
                    Ok(()) => {
                        let refreshed = compute_context_state(
                            &app,
                            &state,
                            &conversation,
                            Some(api_content.as_str()),
                            &last_user_image_paths,
                        )
                        .await?;
                        conversation.context_state = refreshed.clone();
                        conversation.updated_at = chrono::Local::now().timestamp();
                        conversation = crate::chat::repository::repository(&app)
                            .mutate(&app, &conversation_id, |latest| {
                                latest.context_state = refreshed.clone();
                                Ok(())
                            })
                            .await
                            .map_err(crate::chat::repository::repository_error)?;
                        emit_chat_context_state(
                            &app,
                            &conversation.id,
                            conversation.revision,
                            &refreshed,
                        );
                    }
                    Err(err) => {
                        eprintln!("Auto context compression failed: {err}");
                        if context_likely_over_limit(&conversation.context_state) {
                            rollback_user_message_after_failed_send(
                                &app,
                                &state,
                                &mut conversation,
                                &user_message.id,
                            )
                            .await?;
                            strip_transcripts_for_frontend(&mut conversation);
                            return Ok(serde_json::json!({
                                "success": false,
                                "conversation": conversation,
                                "error": format!(
                                    "Context is likely over the model limit and automatic compression failed: {err}. Please compress manually or switch to a larger-context model."
                                ),
                            }));
                        }
                        conversation.context_state.warning = Some(format!(
                            "Automatic compression failed: {err}. The uncompressed request was sent because the estimate is still within the model window."
                        ));
                        let next_context_state = conversation.context_state.clone();
                        conversation = crate::chat::repository::repository(&app)
                            .mutate(&app, &conversation_id, |latest| {
                                latest.context_state = next_context_state;
                                Ok(())
                            })
                            .await
                            .map_err(crate::chat::repository::repository_error)?;
                        emit_chat_context_state(
                            &app,
                            &conversation.id,
                            conversation.revision,
                            &conversation.context_state,
                        );
                    }
                }
            } else {
                let context_state = conversation.context_state.clone();
                conversation = crate::chat::repository::repository(&app)
                    .mutate(&app, &conversation_id, |latest| {
                        latest.context_state = context_state.clone();
                        Ok(())
                    })
                    .await
                    .map_err(crate::chat::repository::repository_error)?;
                emit_chat_context_state(
                    &app,
                    &conversation.id,
                    conversation.revision,
                    &context_state,
                );
            }
        }
        Err(err) => {
            eprintln!("Context usage estimate failed before send: {err}");
        }
    }

    let forced_skill_id = active_skill_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);

    if fan_out {
        let group_id = group_id.expect("fan_out implies group_id set");
        let fan_out_outcome = run_reply_fan_out(
            &app,
            &state,
            &mut conversation,
            &reply_arms,
            &group_id,
            Some(api_content.as_str()),
            &last_user_image_paths,
            forced_skill_id.as_deref(),
        )
        .await;
        strip_transcripts_for_frontend(&mut conversation);
        return match fan_out_outcome {
            Ok(()) => Ok(serde_json::json!({
                "success": true,
                "conversation": conversation,
            })),
            // 全部臂都失败（非取消）才算硬失败；部分成功在 run_reply_fan_out 内已合并落盘并返回 Ok。
            Err(err) if err == "cancelled" => Ok(serde_json::json!({
                "success": true,
                "conversation": conversation,
            })),
            Err(err) => Ok(serde_json::json!({
                "success": false,
                "conversation": conversation,
                "error": err,
            })),
        };
    }

    let reply_outcome = complete_assistant_reply(
        &app,
        &state,
        &mut conversation,
        Some(title_source.as_str()),
        Some(api_content.as_str()),
        &last_user_image_paths,
        forced_skill_id.as_deref(),
        crate::chat::agent::AgentRunEntry::Send,
    )
    .await;
    // 剥离按臂做、且在各臂最后一次写盘之后。发送前超上下文那条提前返回的分支会先 rollback
    // 再持久化，若在 match 前统一剥，就会把剥光的对话写回磁盘、永久丢掉盘上转录。
    match reply_outcome {
        Ok(()) => {
            strip_transcripts_for_frontend(&mut conversation);
            Ok(serde_json::json!({
                "success": true,
                "conversation": conversation,
            }))
        }
        Err(err) if err == "cancelled" => {
            strip_transcripts_for_frontend(&mut conversation);
            Ok(serde_json::json!({
                "success": true,
                "conversation": conversation,
            }))
        }
        Err(err) => {
            // 生成中途硬失败（403 / 空响应 等）发生在用户消息已落盘之后。**不要回滚**——
            // 把问题留在线程里，用户可一键重试而无需重打（与 chat_regenerate_message 的
            // 错误路径一致：那条路径报错时也保留用户消息）。盘上已是「用户消息、无 assistant」
            // 的干净状态（run_agent_loop 的 Err 在 push_assistant_message 之前冒泡），直接返回即可。
            strip_transcripts_for_frontend(&mut conversation);
            Ok(serde_json::json!({
                "success": false,
                "conversation": conversation,
                "error": err,
            }))
        }
    }
}
