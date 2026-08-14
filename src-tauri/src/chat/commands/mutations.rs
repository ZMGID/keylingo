use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::chat::agent::execute::truncate_chars;
use crate::chat::attachments::{
    compose_text_attachments_for_api, compose_user_content_for_api,
    stored_image_paths_for_attachments, text_attachments_from_attachments,
};
use crate::state::AppState;

use super::super::storage::{
    assistant_snapshot, conversation_attachments_dir, find_project_by_id, find_project_by_name,
    find_set_by_id, load_conversation,
};
use super::super::{
    AgentPlanState, AgentTodoState, ChatMessage, Conversation, ConversationContextState, ForkOrigin,
};
use super::catalog::{reconcile_conversation_orphan_tool_segments, strip_transcripts_for_frontend};
use super::context::{
    compute_context_state, emit_chat_context_state, mark_summary_stale_if_needed,
};
use super::messages::replace_final_text_segments_for_edit;
use super::{
    complete_assistant_reply, ChatSendReservation, CHAT_REPLY_BUSY_ERROR, MAX_REPLY_MODELS,
};

fn find_message_index(conversation: &Conversation, message_id: &str) -> Result<usize, String> {
    conversation
        .messages
        .iter()
        .position(|m| m.id == message_id)
        .ok_or_else(|| "消息不存在".to_string())
}

/// 更新单条消息（仅助手回复）
#[tauri::command]
pub(crate) async fn chat_update_message(
    app: AppHandle,
    state: State<'_, AppState>,
    conversation_id: String,
    message_id: String,
    content: String,
) -> Result<serde_json::Value, String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err("消息内容不能为空".to_string());
    }
    let repository = crate::chat::repository::repository(&app);
    let mut snapshot = repository
        .get(&app, &conversation_id)
        .await
        .map_err(crate::chat::repository::repository_error)?;
    let mut attempt = 0;
    let (mut conversation, context_state) = loop {
        let mut candidate = snapshot.clone();
        let idx = find_message_index(&candidate, &message_id)?;
        if candidate.messages[idx].role != "assistant" {
            return Err("仅支持编辑助手回复".to_string());
        }
        mark_summary_stale_if_needed(&mut candidate, idx);
        replace_final_text_segments_for_edit(&mut candidate.messages[idx], trimmed);
        candidate.messages[idx].timestamp = chrono::Local::now().timestamp();
        let context_state = compute_context_state(&app, &state, &candidate, None, &[]).await?;
        let edited_message = candidate.messages[idx].clone();
        match repository
            .mutate_expected(&app, &conversation_id, Some(snapshot.revision), |latest| {
                let latest_idx = find_message_index(latest, &message_id)?;
                if latest.messages[latest_idx].role != "assistant" {
                    return Err("仅支持编辑助手回复".to_string());
                }
                mark_summary_stale_if_needed(latest, latest_idx);
                latest.messages[latest_idx] = edited_message;
                latest.context_state = context_state.clone();
                Ok(())
            })
            .await
        {
            Ok(latest) => break (latest, context_state),
            Err(crate::chat::repository::ConversationRepositoryError::Conflict { .. })
                if attempt == 0 =>
            {
                attempt += 1;
                snapshot = repository
                    .get(&app, &conversation_id)
                    .await
                    .map_err(crate::chat::repository::repository_error)?;
            }
            Err(err) => return Err(crate::chat::repository::repository_error(err)),
        }
    };
    emit_chat_context_state(
        &app,
        &conversation.id,
        conversation.revision,
        &context_state,
    );

    strip_transcripts_for_frontend(&mut conversation);
    Ok(serde_json::json!({
        "success": true,
        "conversation": conversation,
    }))
}

/// `chat_regenerate_message` 的截断/编辑核心（纯函数，便于单测）：
/// - assistant：截到它之前（`new_content` 无意义 → 报错）。
/// - user + `new_content`：trim 校验非空 → 替换内容（编辑提问；附件保留）→ 保留该条截掉其后。
///   摘要失效用 `idx`（内容变了，覆盖到该条的摘要即失效）。
/// - user 无 `new_content`：孤儿重试，摘要失效用 `idx + 1`，保留该条截掉其后。
pub(super) fn apply_regenerate_truncation(
    conversation: &mut Conversation,
    idx: usize,
    new_content: Option<String>,
) -> Result<(), String> {
    match conversation.messages[idx].role.as_str() {
        "assistant" => {
            if new_content.is_some() {
                return Err("编辑内容仅支持用户消息".to_string());
            }
            mark_summary_stale_if_needed(conversation, idx);
            conversation.messages.truncate(idx);
        }
        "user" => {
            if let Some(content) = new_content {
                let trimmed = content.trim();
                if trimmed.is_empty() {
                    return Err("消息内容不能为空".to_string());
                }
                mark_summary_stale_if_needed(conversation, idx);
                conversation.messages[idx].content = trimmed.to_string();
                conversation.messages[idx].timestamp = chrono::Local::now().timestamp();
            } else {
                mark_summary_stale_if_needed(conversation, idx + 1);
            }
            conversation.messages.truncate(idx + 1);
        }
        _ => return Err("仅支持重新生成助手回复或重试用户消息".to_string()),
    }
    Ok(())
}

/// 重新生成助手回复（移除该条及之后的消息，再基于此前上下文请求新回复）。
/// `new_content`：编辑用户提问并重新生成——仅当目标是 user 消息时有效，先替换其内容
/// 再走截断+重生成（附件保留；一个原子命令，避免"改了历史但不重生成"的不一致状态）。
#[tauri::command]
pub(crate) async fn chat_regenerate_message(
    app: AppHandle,
    state: State<'_, AppState>,
    conversation_id: String,
    message_id: String,
    new_content: Option<String>,
) -> Result<serde_json::Value, String> {
    // Busy 拒绝：该会话仍有任意一条 run 在跑时不允许再触发重新生成。
    // 原子哨兵预留关闭 TOCTOU 窗口；per-run 槽位 / generation 在 `complete_assistant_reply` 内注册。
    let Some(_send_reservation) = ChatSendReservation::try_acquire(state.inner(), &conversation_id)
    else {
        return Ok(serde_json::json!({
            "success": false,
            "error": CHAT_REPLY_BUSY_ERROR,
        }));
    };

    let repository = crate::chat::repository::repository(&app);
    let mut snapshot = repository
        .get(&app, &conversation_id)
        .await
        .map_err(crate::chat::repository::repository_error)?;
    let mut attempt = 0;
    let mut conversation = loop {
        let next_content = new_content.clone();
        match repository
            .mutate_expected(&app, &conversation_id, Some(snapshot.revision), |latest| {
                let idx = find_message_index(latest, &message_id)?;
                apply_regenerate_truncation(latest, idx, next_content)?;
                if latest.messages.last().map(|message| message.role.as_str()) != Some("user") {
                    return Err("缺少对应的用户消息，无法重新生成".to_string());
                }
                let existing_ids: std::collections::HashSet<String> = latest
                    .messages
                    .iter()
                    .map(|message| message.id.clone())
                    .collect();
                latest
                    .group_selections
                    .retain(|_, selected| existing_ids.contains(selected));
                Ok(())
            })
            .await
        {
            Ok(latest) => break latest,
            Err(crate::chat::repository::ConversationRepositoryError::Conflict { .. })
                if attempt == 0 =>
            {
                attempt += 1;
                snapshot = repository
                    .get(&app, &conversation_id)
                    .await
                    .map_err(crate::chat::repository::repository_error)?;
            }
            Err(err) => return Err(crate::chat::repository::repository_error(err)),
        }
    };

    let last_user_api_content = conversation
        .messages
        .last()
        .filter(|message| message.role == "user")
        .map(|message| {
            let attachment_dir = if message.attachments.is_empty() {
                None
            } else {
                conversation_attachments_dir(&app, &conversation_id).ok()
            };
            let mut api_content = compose_user_content_for_api(
                &message.content,
                &message.attachments,
                attachment_dir.as_deref(),
            );
            // 虚拟文本附件（memory://）正文只存在附件记录里、不在 message.content 中：
            // 重新生成时同样要内联，否则模型只会看到原始短提示词或空内容。
            let text_attachments = text_attachments_from_attachments(&message.attachments);
            if !text_attachments.is_empty() {
                api_content = compose_text_attachments_for_api(&api_content, &text_attachments);
            }
            api_content
        });
    let last_user_image_paths = conversation
        .messages
        .last()
        .filter(|message| message.role == "user")
        .map(|message| {
            stored_image_paths_for_attachments(&app, &conversation_id, &message.attachments)
        })
        .transpose()?
        .unwrap_or_default();
    for attempt in 0..2 {
        let context_state = match compute_context_state(
            &app,
            &state,
            &conversation,
            last_user_api_content.as_deref(),
            &last_user_image_paths,
        )
        .await
        {
            Ok(state) => state,
            Err(err) => {
                eprintln!("Context usage estimate failed before regenerate: {err}");
                break;
            }
        };
        match repository
            .update_context(
                &app,
                &conversation_id,
                conversation.revision,
                context_state.clone(),
            )
            .await
        {
            Ok(latest) => {
                conversation = latest;
                emit_chat_context_state(
                    &app,
                    &conversation.id,
                    conversation.revision,
                    &context_state,
                );
                break;
            }
            Err(crate::chat::repository::ConversationRepositoryError::Conflict { .. })
                if attempt == 0 =>
            {
                conversation = repository
                    .get(&app, &conversation_id)
                    .await
                    .map_err(crate::chat::repository::repository_error)?;
            }
            Err(err) => {
                eprintln!("Context state commit failed before regenerate: {err}");
                break;
            }
        }
    }
    let reply_outcome = complete_assistant_reply(
        &app,
        &state,
        &mut conversation,
        None,
        last_user_api_content.as_deref(),
        &last_user_image_paths,
        None,
        crate::chat::agent::AgentRunEntry::Regenerate,
    )
    .await;
    strip_transcripts_for_frontend(&mut conversation);
    match reply_outcome {
        Ok(()) => Ok(serde_json::json!({
            "success": true,
            "conversation": conversation,
        })),
        Err(err) if err == "cancelled" => Ok(serde_json::json!({
            "success": true,
            "conversation": conversation,
        })),
        Err(err) => Ok(serde_json::json!({
            "success": false,
            "error": err,
        })),
    }
}

/// 删除单条消息
#[tauri::command]
pub(crate) async fn chat_delete_message(
    app: AppHandle,
    state: State<'_, AppState>,
    conversation_id: String,
    message_id: String,
) -> Result<serde_json::Value, String> {
    let repository = crate::chat::repository::repository(&app);
    let mut snapshot = repository
        .get(&app, &conversation_id)
        .await
        .map_err(crate::chat::repository::repository_error)?;
    let mut attempt = 0;
    let (mut conversation, context_state) = loop {
        let mut candidate = snapshot.clone();
        let idx = find_message_index(&candidate, &message_id)?;
        if candidate.messages[idx].role != "assistant" {
            return Err("仅支持删除助手回复".to_string());
        }
        mark_summary_stale_if_needed(&mut candidate, idx);
        let removed = candidate.messages.remove(idx);
        if let Some(group_id) = removed.group_id.as_deref() {
            if candidate.group_selections.get(group_id).map(String::as_str)
                == Some(removed.id.as_str())
            {
                candidate.group_selections.remove(group_id);
            }
        }
        let context_state = compute_context_state(&app, &state, &candidate, None, &[]).await?;
        match repository
            .mutate_expected(&app, &conversation_id, Some(snapshot.revision), |latest| {
                let latest_idx = find_message_index(latest, &message_id)?;
                if latest.messages[latest_idx].role != "assistant" {
                    return Err("仅支持删除助手回复".to_string());
                }
                mark_summary_stale_if_needed(latest, latest_idx);
                let removed = latest.messages.remove(latest_idx);
                if let Some(group_id) = removed.group_id.as_deref() {
                    if latest.group_selections.get(group_id).map(String::as_str)
                        == Some(removed.id.as_str())
                    {
                        latest.group_selections.remove(group_id);
                    }
                }
                latest.context_state = context_state.clone();
                Ok(())
            })
            .await
        {
            Ok(latest) => break (latest, context_state),
            Err(crate::chat::repository::ConversationRepositoryError::Conflict { .. })
                if attempt == 0 =>
            {
                attempt += 1;
                snapshot = repository
                    .get(&app, &conversation_id)
                    .await
                    .map_err(crate::chat::repository::repository_error)?;
            }
            Err(err) => return Err(crate::chat::repository::repository_error(err)),
        }
    };
    emit_chat_context_state(
        &app,
        &conversation.id,
        conversation.revision,
        &context_state,
    );

    // 删消息是附件引用真正消失的时机：清掉不再被任何消息引用的 GC 托管附件
    // （`msgimg-*` / `artifact-*`）。用户上传的 `att_*` 永不自动删。
    // 传 id 不传对象：GC 自己从磁盘重读完整转录算引用集合，不受下面 strip 的影响。
    let (removed_files, freed) =
        crate::chat::gc::sweep_conversation_attachments(&app, &conversation_id);
    if removed_files > 0 {
        eprintln!(
            "[chat-gc] 删除消息后回收 {removed_files} 个孤儿附件（{:.1} MB）",
            freed as f64 / (1024.0 * 1024.0)
        );
    }

    strip_transcripts_for_frontend(&mut conversation);
    Ok(serde_json::json!({
        "success": true,
        "conversation": conversation,
    }))
}

/// 一键 rewind（「回到这里」）：删掉这条用户提问**及其之后**的所有消息，把原文回传给前端塞进输入框。
/// 与 `chat_regenerate_message` 的区别：不自动重生成，用户可以改完再自己发。
/// ponytail: 用户上传的附件（`att_*`）留在会话附件目录里 —— 原文回输入框但附件不回，
/// 删掉就找不回来了，清理留给会话删除时的整目录清空。GC 托管的图（`msgimg-*` /
/// `artifact-*`）在下面按引用对账回收。
#[tauri::command]
pub(crate) async fn chat_rewind_to_message(
    app: AppHandle,
    state: State<'_, AppState>,
    conversation_id: String,
    message_id: String,
) -> Result<serde_json::Value, String> {
    // 与 regenerate 同一把哨兵：会话里还有 run 在跑时不许截历史。
    let Some(_send_reservation) = ChatSendReservation::try_acquire(state.inner(), &conversation_id)
    else {
        return Ok(serde_json::json!({
            "success": false,
            "error": CHAT_REPLY_BUSY_ERROR,
        }));
    };

    // ponytail: 故意不在这里跑 compute_context_state —— 它要列 MCP 工具/扫技能，能拖几秒，
    // 而 rewind 只是删消息。前端拿到结果立刻更新，随后 fire-and-forget 调 chat_get_context_stats
    // 补算上下文用量。
    let repository = crate::chat::repository::repository(&app);
    let mut snapshot = repository
        .get(&app, &conversation_id)
        .await
        .map_err(crate::chat::repository::repository_error)?;
    let mut attempt = 0;
    let (mut conversation, content) = loop {
        let idx = find_message_index(&snapshot, &message_id)?;
        if snapshot.messages[idx].role != "user" {
            return Err("仅支持回到用户提问".to_string());
        }
        let content = snapshot.messages[idx].content.clone();
        match repository
            .mutate_expected(&app, &conversation_id, Some(snapshot.revision), |latest| {
                let latest_idx = find_message_index(latest, &message_id)?;
                if latest.messages[latest_idx].role != "user" {
                    return Err("仅支持回到用户提问".to_string());
                }
                mark_summary_stale_if_needed(latest, latest_idx);
                latest.messages.truncate(latest_idx);
                let existing_ids: std::collections::HashSet<String> = latest
                    .messages
                    .iter()
                    .map(|message| message.id.clone())
                    .collect();
                latest
                    .group_selections
                    .retain(|_, selected| existing_ids.contains(selected));
                Ok(())
            })
            .await
        {
            Ok(latest) => break (latest, content),
            Err(crate::chat::repository::ConversationRepositoryError::Conflict { .. })
                if attempt == 0 =>
            {
                attempt += 1;
                snapshot = repository
                    .get(&app, &conversation_id)
                    .await
                    .map_err(crate::chat::repository::repository_error)?;
            }
            Err(err) => return Err(crate::chat::repository::repository_error(err)),
        }
    };

    // rewind 一次砍掉一整段历史，是孤儿附件最容易堆积的地方。GC 托管的图（`msgimg-*` /
    // `artifact-*`）没人引用就回收；用户上传的 `att_*` 一律保留（原文回输入框但附件不回，
    // 删掉就找不回来了）。传 id 不传对象：GC 自己从磁盘重读完整转录算引用集合。
    let (removed_files, freed) =
        crate::chat::gc::sweep_conversation_attachments(&app, &conversation_id);
    if removed_files > 0 {
        eprintln!(
            "[chat-gc] rewind 后回收 {removed_files} 个孤儿附件（{:.1} MB）",
            freed as f64 / (1024.0 * 1024.0)
        );
    }

    strip_transcripts_for_frontend(&mut conversation);
    Ok(serde_json::json!({
        "success": true,
        "conversation": conversation,
        "content": content,
    }))
}

/// 组装分叉分支的消息前缀（纯函数，便于单测）：
/// - 取 `messages[0..=anchor_idx]`（含锚点）。
/// - 若锚点属于某多模型多答组（`group_id = Some(g)`，决策 Q2）：只保留锚点那一列，
///   移除前缀内其余同组兄弟列（组内更晚的列已被切片排除），并把锚点的 `group_id` 置 None
///   转为普通单答，使新对话该轮成为干净的线性单答。
/// - 保留原 message id（跨对话无需唯一，`group_selections` 引用因此仍有效）。
pub(super) fn build_fork_messages(messages: &[ChatMessage], anchor_idx: usize) -> Vec<ChatMessage> {
    let anchor_id = messages[anchor_idx].id.clone();
    let anchor_group = messages[anchor_idx].group_id.clone();
    let mut out: Vec<ChatMessage> = messages[..=anchor_idx].to_vec();

    if let Some(group) = anchor_group {
        // 丢弃同组的其它兄弟列，仅留锚点那条。
        out.retain(|m| m.group_id.as_deref() != Some(group.as_str()) || m.id == anchor_id);
        // 锚点转普通单答。
        if let Some(anchor) = out.iter_mut().find(|m| m.id == anchor_id) {
            anchor.group_id = None;
        }
    }

    out
}

/// 分叉成新对话（方案 B）：把源对话某消息及其之前的消息复制进一个全新对话，之后独立继续。
/// 纯复制 + 打开，不自动发送（决策 Q1）。新对话继承源的会话级配置、深拷被引用的附件/图片
/// artifact，并记录 `forked_from` 供 UI 面包屑回跳。源对话完全只读、不受影响。
#[tauri::command]
pub(crate) async fn chat_fork_conversation(
    app: AppHandle,
    conversation_id: String,
    message_id: String,
) -> Result<serde_json::Value, String> {
    let source = load_conversation(&app, &conversation_id)?;
    let anchor_idx = find_message_index(&source, &message_id)?;

    let now = chrono::Local::now().timestamp();
    let new_id = format!("conv_{}", Uuid::new_v4());
    let messages = build_fork_messages(&source.messages, anchor_idx);

    // 深拷被复制消息引用的附件 / 图片 artifact 文件到新对话目录（裸文件名同名拷贝，路径保持有效）。
    // Missing attachment files are warnings rather than fork blockers. Workbench outputs are not copied when forking.
    copy_forked_conversation_files(&app, &conversation_id, &new_id, &messages);

    // 清理 group_selections：仅保留其选中 message_id 仍存在于新 messages、且该消息仍带 group_id 的条目。
    let existing_groups: std::collections::HashMap<&str, &str> = messages
        .iter()
        .filter_map(|m| m.group_id.as_deref().map(|g| (m.id.as_str(), g)))
        .collect();
    let mut group_selections = source.group_selections.clone();
    group_selections.retain(|group_id, sel_msg_id| {
        existing_groups.get(sel_msg_id.as_str()) == Some(&group_id.as_str())
    });

    // 标题加「（分支）」后缀（R7）。先把源标题截到留出后缀空间，保证后缀在 40 字上限内始终可见。
    const FORK_SUFFIX: &str = "（分支）";
    let base = truncate_chars(&source.title, 40 - FORK_SUFFIX.chars().count());
    let title = format!("{base}{FORK_SUFFIX}");

    let conversation = Conversation {
        id: new_id,
        revision: 0,
        title,
        provider_id: source.provider_id.clone(),
        model: source.model.clone(),
        messages,
        agent_runtime: source.agent_runtime.clone(),
        active_skill_id: source.active_skill_id.clone(),
        assistant_id: source.assistant_id.clone(),
        assistant_snapshot: source.assistant_snapshot.clone(),
        created_at: now,
        updated_at: now,
        pinned: false,
        archived: false,
        folder: source.folder.clone(),
        project_id: source.project_id.clone(),
        set_id: source.set_id.clone(),
        context_state: ConversationContextState::default(),
        agent_todo_state: AgentTodoState::default(),
        agent_plan_state: AgentPlanState::default(),
        knowledge_base_ids: source.knowledge_base_ids.clone(),
        force_knowledge_search: source.force_knowledge_search,
        thinking_level: source.thinking_level.clone(),
        web_search_mode: source.web_search_mode,
        reply_models: source.reply_models.clone(),
        group_selections,
        forked_from: Some(ForkOrigin {
            conversation_id: source.id.clone(),
            message_id: source.messages[anchor_idx].id.clone(),
            title: source.title.clone(),
        }),
    };

    let conversation = crate::chat::repository::repository(&app)
        .create(&app, conversation)
        .await
        .map_err(crate::chat::repository::repository_error)?;

    let mut conversation = conversation;
    // 与 chat_get_conversation 一致：返回前做读时孤立工具分段对账（必须在 strip 之前——
    // strip 会清掉已完成消息的 api_messages，而回捞工具名依赖它）。源存量文件若有孤立分段，
    // 分叉后的即时展示也能正常显示工具卡（tool-segment-record-reconcile 契约）。
    reconcile_conversation_orphan_tool_segments(&mut conversation);
    strip_transcripts_for_frontend(&mut conversation);
    Ok(serde_json::json!({
        "success": true,
        "conversation": conversation,
    }))
}

/// 深拷分叉消息引用的对话目录文件（附件 + 图片 artifact）到新对话附件目录。
/// path 为裸文件名（相对源对话附件目录），同名拷贝到新目录后引用保持有效。
fn copy_forked_conversation_files(
    app: &AppHandle,
    source_id: &str,
    new_id: &str,
    messages: &[ChatMessage],
) {
    // 收集所有被引用的裸文件名（附件 + 消息级 artifact + 各 tool_call 内 artifact）。
    let mut names: Vec<&str> = Vec::new();
    for message in messages {
        for att in &message.attachments {
            if !att.path.is_empty() {
                names.push(att.path.as_str());
            }
        }
        for artifact in &message.artifacts {
            if let Some(p) = artifact.path.as_deref().filter(|p| !p.is_empty()) {
                names.push(p);
            }
        }
        for tool_call in &message.tool_calls {
            for artifact in &tool_call.artifacts {
                if let Some(p) = artifact.path.as_deref().filter(|p| !p.is_empty()) {
                    names.push(p);
                }
            }
        }
    }
    if names.is_empty() {
        return;
    }
    names.sort_unstable();
    names.dedup();

    let (Ok(src_dir), Ok(dst_dir)) = (
        conversation_attachments_dir(app, source_id),
        conversation_attachments_dir(app, new_id),
    ) else {
        eprintln!("fork: 无法解析附件目录，跳过附件深拷（source={source_id} new={new_id}）");
        return;
    };

    for name in names {
        // 只接受裸文件名，拒绝任何路径分隔符（防越目录）。
        if name.contains('/') || name.contains('\\') {
            eprintln!("fork: 跳过非法附件路径 {name}");
            continue;
        }
        let src = src_dir.join(name);
        let dst = dst_dir.join(name);
        if !src.is_file() {
            eprintln!("fork: 源附件不存在，跳过 {name}");
            continue;
        }
        if let Err(e) = std::fs::copy(&src, &dst) {
            eprintln!("fork: 拷贝附件失败 {name}: {e}");
        }
    }
}

/// 删除对话
#[tauri::command]
pub(crate) async fn chat_delete_conversation(
    app: AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
    conversation_id: String,
) -> Result<serde_json::Value, String> {
    // 删对话即终止其持久外部 CLI 会话（actor 关闭子进程）并清掉跨重启 resume 句柄。
    state.remove_external_live_session(&conversation_id);
    crate::external_agents::session::clear_live_handle(&app, &conversation_id);
    // 该对话起的后台命令也一并收掉。它们本来只在退出应用时统一清（跨轮存活是有意的），
    // 但一个还在跑的 dev server 会把 cwd 钉在对话工作区里，Windows 上直接导致工作区
    // 删不掉。删对话时这些进程已经没有归属，先杀掉再清目录。
    let killed = state.kill_background_commands_for_conversation(&conversation_id);
    if killed > 0 {
        eprintln!("Deleted conversation {conversation_id}: killed {killed} background command(s)");
    }
    // 顺手清掉该对话在内存里按 conversation_id 累积的运行态小 map（stream 代际计数 /
    // 会话级工具同意），它们只插不删、严格无界——对话删了便永远不会再被引用。
    state.forget_chat_conversation_runtime(&conversation_id);
    let warnings = crate::chat::repository::repository(&app)
        .delete(&app, &conversation_id)
        .await
        .map_err(crate::chat::repository::repository_error)?;
    Ok(serde_json::json!({
        "success": true,
        "warnings": warnings,
    }))
}

/// 更新对话（标题、置顶、归档、文件夹等）
#[tauri::command]
pub(crate) async fn chat_update_conversation(
    app: AppHandle,
    conversation_id: String,
    title: Option<String>,
    pinned: Option<bool>,
    archived: Option<bool>,
    folder: Option<String>,
    project_id: Option<String>,
    set_id: Option<String>,
    provider_id: Option<String>,
    model: Option<String>,
    active_skill_id: Option<String>,
    assistant_id: Option<String>,
    knowledge_base_ids: Option<Vec<String>>,
    force_knowledge_search: Option<bool>,
    thinking_level: Option<String>,
    web_search_mode: Option<String>,
    reply_models: Option<Vec<crate::chat::ModelRef>>,
) -> Result<serde_json::Value, String> {
    let mut conversation = crate::chat::repository::repository(&app)
        .mutate(&app, &conversation_id, |conversation| {
            if let Some(t) = title {
                conversation.title = t;
            }
            if let Some(p) = pinned {
                conversation.pinned = p;
            }
            if let Some(a) = archived {
                conversation.archived = a;
            }
            if let Some(folder) = folder {
                let trimmed = folder.trim();
                conversation.folder = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
                conversation.project_id = match conversation.folder.as_deref() {
                    Some(folder) => find_project_by_name(&app, folder)?.map(|project| project.id),
                    None => None,
                };
            }
            if let Some(project_id) = project_id {
                let trimmed = project_id.trim();
                if trimmed.is_empty() {
                    conversation.project_id = None;
                    conversation.folder = None;
                } else {
                    let project = find_project_by_id(&app, trimmed)?;
                    conversation.project_id = Some(project.id);
                    conversation.folder = Some(project.name);
                    conversation.set_id = None; // 集与项目互斥
                }
            }
            if let Some(set_id) = set_id {
                let trimmed = set_id.trim();
                if trimmed.is_empty() {
                    conversation.set_id = None;
                } else {
                    let set = find_set_by_id(&app, trimmed)?;
                    conversation.set_id = Some(set.id);
                    // 集与项目互斥：归入集即移出项目/文件夹
                    conversation.project_id = None;
                    conversation.folder = None;
                }
            }
            if let Some(provider_id) = provider_id {
                conversation.provider_id = provider_id;
            }
            if let Some(model) = model {
                conversation.model = model;
            }
            if let Some(skill_id) = active_skill_id {
                let trimmed = skill_id.trim();
                conversation.active_skill_id = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
            }
            if let Some(assistant_id) = assistant_id {
                let trimmed = assistant_id.trim();
                if trimmed.is_empty() {
                    conversation.assistant_id = None;
                    conversation.assistant_snapshot = None;
                    conversation.active_skill_id = None;
                } else {
                    let snapshot = assistant_snapshot(&app, trimmed)?;
                    // 切换助手不再强制激活默认技能;skill_ids 仅作白名单。
                    conversation.active_skill_id = None;
                    conversation.assistant_id = Some(snapshot.id.clone());
                    conversation.assistant_snapshot = Some(snapshot);
                }
            }
            if let Some(ids) = knowledge_base_ids {
                // Drop blanks/dups; order preserved.
                let mut seen = std::collections::HashSet::new();
                conversation.knowledge_base_ids = ids
                    .into_iter()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty() && seen.insert(s.clone()))
                    .collect();
            }
            if let Some(force) = force_knowledge_search {
                conversation.force_knowledge_search = force;
            }
            if let Some(level) = thinking_level {
                // 仅接受已知值；空串/未知 → 清除（回到「跟随全局」）。
                conversation.thinking_level = match level.trim() {
                    "off" | "low" | "medium" | "high" | "xhigh" | "max" => {
                        Some(level.trim().to_string())
                    }
                    _ => None,
                };
            }
            if let Some(mode) = web_search_mode {
                // 会话级三态联网搜索（任务 07-23）。空/未知 → 清除（回退全局 nativeTools.webSearch）。
                use crate::chat::types::WebSearchMode;
                conversation.web_search_mode = match mode.trim() {
                    "off" => Some(WebSearchMode::Off),
                    "builtin" => Some(WebSearchMode::Builtin),
                    "third_party" => Some(WebSearchMode::ThirdParty),
                    _ => None,
                };
            }
            if let Some(reply_models) = reply_models {
                // 多模型一问多答（决策 D2/D4）：持久化会话级多答模型集。去重（provider+model）、
                // 丢空、保序、上限 MAX_REPLY_MODELS（超出报错，前端应已禁选）。
                if reply_models.len() > MAX_REPLY_MODELS {
                    return Err(format!(
                        "多模型并行回答最多同时选择 {MAX_REPLY_MODELS} 个模型。"
                    ));
                }
                let mut seen = std::collections::HashSet::new();
                conversation.reply_models = reply_models
                    .into_iter()
                    .filter_map(|m| {
                        let provider_id = m.provider_id.trim().to_string();
                        let model = m.model.trim().to_string();
                        if provider_id.is_empty() || model.is_empty() {
                            return None;
                        }
                        let key = format!("{provider_id}\u{0}{model}");
                        if seen.insert(key) {
                            Some(crate::chat::ModelRef { provider_id, model })
                        } else {
                            None
                        }
                    })
                    .collect();
            }

            Ok(())
        })
        .await
        .map_err(crate::chat::repository::repository_error)?;

    strip_transcripts_for_frontend(&mut conversation);
    Ok(serde_json::json!({
        "success": true,
        "conversation": conversation,
    }))
}

/// 对话库批量更新：收藏/归档/移入集或项目。一次 IPC，走 repository bulk_mutate。
#[tauri::command]
pub(crate) async fn chat_bulk_update_conversations(
    app: AppHandle,
    ids: Vec<String>,
    pinned: Option<bool>,
    archived: Option<bool>,
    project_id: Option<String>,
    set_id: Option<String>,
) -> Result<serde_json::Value, String> {
    if ids.is_empty() {
        return Ok(serde_json::json!({ "success": true, "updated": 0 }));
    }
    if ids.len() > 500 {
        return Err("单次最多批量更新 500 条对话".into());
    }
    let id_set: std::collections::HashSet<String> = ids.into_iter().collect();

    // 预解析归属目标（避免在 mutate 闭包里反复查）
    let project_patch = match project_id.as_deref().map(str::trim) {
        None => None,
        Some("") => Some((None, None)),
        Some(pid) => {
            let p = find_project_by_id(&app, pid)?;
            Some((Some(p.id), Some(p.name)))
        }
    };
    let set_patch = match set_id.as_deref().map(str::trim) {
        None => None,
        Some("") => Some(None),
        Some(sid) => {
            let s = find_set_by_id(&app, sid)?;
            Some(Some(s.id))
        }
    };

    let updated = crate::chat::repository::repository(&app)
        .bulk_mutate(&app, |conversation| {
            if !id_set.contains(&conversation.id) {
                return Ok(false);
            }
            let mut changed = false;
            if let Some(p) = pinned {
                if conversation.pinned != p {
                    conversation.pinned = p;
                    changed = true;
                }
            }
            if let Some(a) = archived {
                if conversation.archived != a {
                    conversation.archived = a;
                    changed = true;
                }
            }
            // 集与项目互斥：若两者都传，set 优先（与单条 update 一致的产品语义可调整；
            // 前端批量条一次只发一种归属）。
            if let Some(set_id_opt) = &set_patch {
                let next = set_id_opt.clone();
                if conversation.set_id != next {
                    conversation.set_id = next;
                    if conversation.set_id.is_some() {
                        conversation.project_id = None;
                        conversation.folder = None;
                    }
                    changed = true;
                }
            } else if let Some((proj_id, folder)) = &project_patch {
                if conversation.project_id != *proj_id || conversation.folder != *folder {
                    conversation.project_id = proj_id.clone();
                    conversation.folder = folder.clone();
                    if conversation.project_id.is_some() {
                        conversation.set_id = None;
                    }
                    changed = true;
                }
            }
            if changed {
                conversation.updated_at = chrono::Local::now().timestamp();
            }
            Ok(changed)
        })
        .await
        .map_err(crate::chat::repository::repository_error)?;

    Ok(serde_json::json!({
        "success": true,
        "updated": updated,
    }))
}

/// 对话库批量删除。
#[tauri::command]
pub(crate) async fn chat_bulk_delete_conversations(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<serde_json::Value, String> {
    if ids.is_empty() {
        return Ok(serde_json::json!({
            "success": true,
            "deleted": 0,
            "warnings": [],
        }));
    }
    if ids.len() > 500 {
        return Err("单次最多批量删除 500 条对话".into());
    }
    let mut deleted = 0usize;
    let mut warnings: Vec<String> = Vec::new();
    for conversation_id in ids {
        // 与单条删除一致：外部 CLI 会话 / 后台命令 / 运行态小 map 都先清，
        // 否则工作区被占着时副产物清理会失败，用户体感「删不掉」。
        state.remove_external_live_session(&conversation_id);
        crate::external_agents::session::clear_live_handle(&app, &conversation_id);
        let killed = state.kill_background_commands_for_conversation(&conversation_id);
        if killed > 0 {
            eprintln!(
                "Bulk-deleted conversation {conversation_id}: killed {killed} background command(s)"
            );
        }
        state.forget_chat_conversation_runtime(&conversation_id);
        match crate::chat::repository::repository(&app)
            .delete(&app, &conversation_id)
            .await
        {
            Ok(ws) => {
                deleted += 1;
                for w in ws {
                    warnings.push(format!("{conversation_id}: {w}"));
                }
            }
            Err(err) => {
                warnings.push(format!(
                    "{conversation_id}: {}",
                    crate::chat::repository::repository_error(err)
                ));
            }
        }
    }
    Ok(serde_json::json!({
        "success": true,
        "deleted": deleted,
        "warnings": warnings,
    }))
}

/// 设置某个多答组（task 06-30）的「选中条」（决策 D5）：用户点选某一列后续聊以它为准。
/// `message_id` 必须是属于 `group_id` 这组的某条 assistant 消息；写入
/// `conversation.group_selections[group_id] = message_id`，下一轮历史拼装据此只保留该条。
#[tauri::command]
pub(crate) async fn chat_set_group_selection(
    app: AppHandle,
    conversation_id: String,
    group_id: String,
    message_id: String,
) -> Result<serde_json::Value, String> {
    let group_id = group_id.trim().to_string();
    let message_id = message_id.trim().to_string();
    if group_id.is_empty() || message_id.is_empty() {
        return Err("group_id 与 message_id 不能为空".to_string());
    }
    let mut conversation = crate::chat::repository::repository(&app)
        .mutate(&app, &conversation_id, |conversation| {
            let valid = conversation.messages.iter().any(|message| {
                message.id == message_id
                    && message.role == "assistant"
                    && message.group_id.as_deref() == Some(group_id.as_str())
            });
            if !valid {
                return Err("选中的回答不属于该多答组".to_string());
            }
            conversation.group_selections.insert(group_id, message_id);
            Ok(())
        })
        .await
        .map_err(crate::chat::repository::repository_error)?;

    strip_transcripts_for_frontend(&mut conversation);
    Ok(serde_json::json!({
        "success": true,
        "conversation": conversation,
    }))
}
