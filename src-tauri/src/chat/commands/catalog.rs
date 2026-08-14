use std::path::Path;

use serde_json::Value;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_shell::ShellExt;
use uuid::Uuid;

use crate::chat::agent::prepare as agent_prepare;
use crate::chat::attachments::{save_message_attachments, title_source_for_user_message};
use crate::chat::storage::{
    archive_assistant, assistant_snapshot, create_assistant, create_set, delete_project,
    delete_set, duplicate_assistant, find_project_by_id, find_project_by_name, find_set_by_id,
    get_assistants, get_projects, get_sets, update_assistant, update_project, update_set,
};
use crate::chat::{
    AgentPlanState, AgentTodoState, Attachment, ChatAssistant, ChatMessage, Conversation,
    ConversationContextState,
};
use crate::settings::Settings;
use crate::skills;
use crate::state::AppState;

use super::messages::reconcile_orphan_tool_segments;

/// 外部入口（如 Lens 交接）预置会话历史时的一条消息。
#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct ExternalConversationMessage {
    pub role: String,
    pub content: String,
}

pub(super) fn chat_memory_prompt_for_request(
    app: &AppHandle,
    settings: &Settings,
) -> (Option<String>, Option<String>) {
    if !settings.chat_memory.enabled {
        return (None, None);
    }
    match crate::chat::memory::l1_prompt_block(app) {
        Ok(prompt) => (prompt, None),
        Err(err) => (None, Some(err)),
    }
}

/// Resolves the conversation's project binding into prompt context so the
/// model knows the path base before generating file tool arguments.
pub(super) fn project_prompt_context_for(
    app: &AppHandle,
    conversation: &Conversation,
) -> Option<agent_prepare::ProjectPromptContext> {
    let project = crate::chat::storage::resolve_conversation_project(app, conversation)
        .ok()
        .flatten()?;
    Some(agent_prepare::ProjectPromptContext {
        name: project.name,
        root_path: project
            .root_path
            .map(|root| root.trim().to_string())
            .filter(|root| !root.is_empty()),
    })
}

/// 获取对话列表
#[tauri::command]
pub(crate) async fn chat_get_conversations(
    app: AppHandle,
    offset: usize,
    limit: usize,
    folder: Option<String>,
    project_id: Option<String>,
    set_id: Option<String>,
) -> Result<serde_json::Value, String> {
    let conversations = crate::chat::repository::repository(&app)
        .list(&app, offset, limit, folder, project_id, set_id)
        .await
        .map_err(crate::chat::repository::repository_error)?;
    Ok(serde_json::json!({
        "success": true,
        "conversations": conversations,
    }))
}

/// 全量搜索对话（不止侧栏默认加载的前 N 个）。先按标题/预览/文件夹匹配元数据，
/// 未命中再全文扫消息正文（content + reasoning），让搜索能找到深埋在对话中间的内容。
#[tauri::command]
pub(crate) async fn chat_search_conversations(
    app: AppHandle,
    query: String,
    limit: usize,
) -> Result<serde_json::Value, String> {
    let conversations = crate::chat::repository::repository(&app)
        .search(&app, &query, limit)
        .await
        .map_err(crate::chat::repository::repository_error)?;
    Ok(serde_json::json!({
        "success": true,
        "conversations": conversations,
    }))
}

/// 对话库统一查询（扩展 → 对话库）。返回 `{ items, total }`。
#[tauri::command]
pub(crate) async fn chat_query_conversations(
    app: AppHandle,
    offset: Option<usize>,
    limit: Option<usize>,
    sort: Option<String>,
    order: Option<String>,
    q: Option<String>,
    full_text: Option<bool>,
    shelf: Option<String>,
    project_id: Option<String>,
    set_id: Option<String>,
    assistant_id: Option<String>,
    provider_id: Option<String>,
    runtime_kind: Option<String>,
) -> Result<serde_json::Value, String> {
    let query = crate::chat::storage::ConversationLibraryQuery {
        offset: offset.unwrap_or(0),
        limit: limit.unwrap_or(80),
        sort: sort.unwrap_or_else(|| "updated".into()),
        order: order.unwrap_or_else(|| "desc".into()),
        q,
        full_text: full_text.unwrap_or(true),
        shelf: shelf.unwrap_or_else(|| "all".into()),
        project_id,
        set_id,
        assistant_id,
        provider_id,
        runtime_kind,
    };
    let page = crate::chat::repository::repository(&app)
        .query_library(&app, query)
        .await
        .map_err(crate::chat::repository::repository_error)?;
    Ok(serde_json::json!({
        "success": true,
        "items": page.items,
        "total": page.total,
    }))
}

/// 获取对话详情
#[tauri::command]
pub(crate) async fn chat_get_conversation(
    app: AppHandle,
    conversation_id: String,
) -> Result<serde_json::Value, String> {
    let repository = crate::chat::repository::repository(&app);
    // 存量迁移：老会话的 `model_messages` 里可能还躺着图片 base64（外置是后来才补的）。
    // 打开时顺手外置一次，之后这份 JSON 就回到 KB 量级。谓词是廉价扫描，无图零开销；
    // 迁移失败只记警告——它是优化，绝不该挡住打开会话。
    let mut conversation = match repository
        .externalize_stored_images(&app, &conversation_id)
        .await
    {
        Ok(Some(migrated)) => migrated,
        Ok(None) => repository
            .get(&app, &conversation_id)
            .await
            .map_err(crate::chat::repository::repository_error)?,
        Err(err) => {
            eprintln!("externalize stored images failed ({conversation_id}): {err}");
            repository
                .get(&app, &conversation_id)
                .await
                .map_err(crate::chat::repository::repository_error)?
        }
    };
    reconcile_conversation_orphan_tool_segments(&mut conversation);
    strip_transcripts_for_frontend(&mut conversation);
    Ok(serde_json::json!({
        "success": true,
        "conversation": conversation,
    }))
}

/// 读取时对账(只读、不写盘):对每条 assistant 消息补齐孤立工具分段为中断态记录,
/// 使**存量**坏会话(改动前落库、含「有分段无记录」的消息)打开即正常显示,而不必等
/// 该消息被重新落库自愈。必须在 [`strip_transcripts_for_frontend`] **之前**跑——它会清空
/// 已完成消息的 `api_messages`,而这里要靠 `api_messages` 回捞被删工具的 name/arguments。
/// 中断草稿的 `api_messages` 不被剥,所以最常见的中断场景仍能拿到真名。
pub(super) fn reconcile_conversation_orphan_tool_segments(conversation: &mut Conversation) {
    for message in conversation.messages.iter_mut() {
        if message.role != "assistant" {
            continue;
        }
        reconcile_orphan_tool_segments(
            &mut message.tool_calls,
            &message.segments,
            &message.api_messages,
        );
    }
}

/// 剥离发给前端的 Conversation 副本里两份转录：`api_messages`（OpenAI 线格式）和
/// `model_messages`（provider 无关回放转录，含全部工具结果原文，是单条消息里最重的字段）。
///
/// 前端两份都从不读（全仓 grep 零引用，回放/编辑全在后端），但它们照样整本序列化进 IPC
/// 白占渲染器 JS heap，且随对话历史线性增长——大对话里 `model_messages` 是前端堆头号占用。
/// 这里**只动发给前端的内存副本，不写盘**——磁盘仍保留完整转录，后端回放读的是独立
/// `load_conversation` 的盘上副本（见 `build_chat_api_messages`），不受此处影响。
///
/// ⚠️ 中断草稿（`stream_outcome == Some("interrupted")`）的转录是「继续」恢复工具上下文
/// 所必需的（见 commit 9d247b0），**绝不剥**。仅剥已完成的 assistant 消息（至多保留最后
/// 一条中断草稿的转录，体积有界）。
pub(super) fn strip_transcripts_for_frontend(conversation: &mut Conversation) {
    for message in conversation.messages.iter_mut() {
        if message.role != "assistant" {
            continue;
        }
        // 中断草稿的转录是「继续」恢复工具上下文所必需的，绝不剥。但**图片 base64 要剥**：
        // 一张截图 2.5MB，整份灌进渲染器 JS heap 纯属白占（前端两份转录都从不读）。
        // 中断草稿是唯一同时持有两份转录的形态（persist_partial_assistant_snapshot 两个都填），
        // 所以两份都要剥——落盘副本已经外置成引用，这里兜住"还没落盘就发出去"的运行期形态。
        if message.stream_outcome.as_deref() == Some("interrupted") {
            strip_image_payloads_from_model_messages(&mut message.model_messages);
            strip_image_payloads_from_api_messages(&mut message.api_messages);
            continue;
        }
        // 两份转录前端都从不读；后端回放走盘上独立副本（build_chat_api_messages 经
        // load_conversation 读盘）。完成态一律剥——含 legacy 老对话（其唯一转录是 api_messages，
        // 但那是磁盘的事，发给前端的副本不需要保留）。legacy 历史转录正是冷加载时最重的一块。
        message.model_messages = Vec::new();
        message.api_messages = Vec::new();
    }
}

/// 清掉 `model_messages` 里图片部件的 base64（保留 mime / path，转录结构不变）。
/// 用于中断草稿：它的转录整份要留给「继续」用，但图片字节没必要过 IPC。
fn strip_image_payloads_from_model_messages(messages: &mut [crate::chat::model::ModelMessage]) {
    for model_message in messages.iter_mut() {
        for part in model_message.content.iter_mut() {
            if let crate::chat::model::MessagePart::Image { data, .. } = part {
                data.clear();
            }
        }
    }
}

/// `strip_image_payloads_from_model_messages` 的 wire-JSON 版：把 `api_messages` 里
/// 图片部件的 `data:` URL 换成短占位串。**只动发给前端的内存副本**，磁盘上是
/// `kivio-attachment://` 哨兵（见 `attachments::externalize_api_message_images`）。
fn strip_image_payloads_from_api_messages(messages: &mut [serde_json::Value]) {
    for message in messages.iter_mut() {
        let Some(parts) = message.get_mut("content").and_then(|c| c.as_array_mut()) else {
            continue;
        };
        for part in parts.iter_mut() {
            let Some(image_url) = part.get_mut("image_url") else {
                continue;
            };
            // 对象形（`image_url.url`）与字符串形（Responses 的 `input_image`）都要覆盖。
            let slot = if image_url.is_object() {
                image_url.get_mut("url")
            } else if image_url.is_string() {
                Some(image_url)
            } else {
                None
            };
            let Some(slot) = slot else { continue };
            if slot
                .as_str()
                .is_some_and(|url| url.starts_with("data:image/"))
            {
                *slot = serde_json::Value::String(String::new());
            }
        }
    }
}

/// 创建新对话
#[tauri::command]
pub(crate) async fn chat_create_conversation(
    app: AppHandle,
    state: State<'_, AppState>,
    provider_id: Option<String>,
    model: Option<String>,
    folder: Option<String>,
    project_id: Option<String>,
    set_id: Option<String>,
    assistant_id: Option<String>,
) -> Result<serde_json::Value, String> {
    let conversation = create_chat_conversation_internal(
        &app,
        state.inner(),
        provider_id,
        model,
        folder,
        project_id,
        set_id,
        assistant_id,
    )
    .await?;

    Ok(serde_json::json!({
        "success": true,
        "conversation": conversation,
    }))
}

pub(crate) async fn create_chat_conversation_internal(
    app: &AppHandle,
    state: &AppState,
    provider_id: Option<String>,
    model: Option<String>,
    folder: Option<String>,
    project_id: Option<String>,
    set_id: Option<String>,
    assistant_id: Option<String>,
) -> Result<Conversation, String> {
    let settings = state.settings_read().clone();
    let set_id = set_id.and_then(non_empty_string);
    // 归属互斥：集与项目至多一个。在创建边界强制（防直连 API 同时传两者）——集优先，清掉项目/文件夹。
    let project_id = if set_id.is_some() { None } else { project_id };
    let folder = if set_id.is_some() { None } else { folder };
    let mut assistant_snapshot = assistant_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| assistant_snapshot(&app, id))
        .transpose()?;
    // 在集下新建且未显式指定助手时，用集的默认助手（创建时冻结进快照，与现有助手行为一致）。
    // 默认助手不可用（归档/停用/不存在）则静默回退为无助手，不阻断建对话。
    if assistant_snapshot.is_none() {
        if let Some(set_id) = set_id.as_deref() {
            if let Some(default_assistant_id) = find_set_by_id(&app, set_id)
                .ok()
                .and_then(|set| set.default_assistant_id)
                .filter(|id| !id.trim().is_empty())
            {
                assistant_snapshot =
                    crate::chat::storage::assistant_snapshot(&app, default_assistant_id.trim())
                        .ok();
            }
        }
    }

    // 使用提供的 provider/model，或者回退到默认模型配置。
    let (default_provider_id, default_model) = settings.effective_chat_model();
    let provider_id = provider_id
        .and_then(non_empty_string)
        .or_else(|| {
            assistant_snapshot
                .as_ref()
                .and_then(|assistant| non_empty_string(assistant.provider_id.clone()))
        })
        .unwrap_or(default_provider_id);
    let model = model
        .and_then(non_empty_string)
        .or_else(|| {
            assistant_snapshot
                .as_ref()
                .and_then(|assistant| non_empty_string(assistant.model.clone()))
        })
        .unwrap_or(default_model);
    let requested_project_id = project_id.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    let folder = folder.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    let project = match requested_project_id.as_deref() {
        Some(project_id) => Some(find_project_by_id(app, project_id)?),
        None => match folder.as_deref() {
            Some(folder) => find_project_by_name(app, folder)?,
            None => None,
        },
    };
    let project_id = project.as_ref().map(|project| project.id.clone());
    let folder = project
        .as_ref()
        .map(|project| project.name.clone())
        .or(folder);
    let assistant_id_for_reuse = assistant_snapshot
        .as_ref()
        .map(|assistant| assistant.id.clone());

    let conversation = {
        let _create_guard = state.chat_create_conversation_lock.lock().await;
        if let Some(conversation) = crate::chat::repository::repository(app)
            .find_reusable_blank(
                app,
                &provider_id,
                &model,
                folder.as_deref(),
                project_id.as_deref(),
                set_id.as_deref(),
                assistant_id_for_reuse.as_deref(),
            )
            .await
            .map_err(crate::chat::repository::repository_error)?
        {
            conversation
        } else {
            let now = chrono::Local::now().timestamp();
            let conversation = Conversation {
                id: format!("conv_{}", Uuid::new_v4()),
                revision: 0,
                title: "新对话".to_string(),
                provider_id,
                model,
                messages: vec![],
                // 助手不再有「默认单技能」;skill_ids 只是白名单,不强制激活某个技能。
                active_skill_id: None,
                assistant_id: assistant_snapshot
                    .as_ref()
                    .map(|assistant| assistant.id.clone()),
                assistant_snapshot,
                created_at: now,
                updated_at: now,
                pinned: false,
                archived: false,
                folder,
                project_id,
                set_id,
                context_state: ConversationContextState::default(),
                agent_todo_state: AgentTodoState::default(),
                agent_plan_state: AgentPlanState::default(),
                knowledge_base_ids: Vec::new(),
                force_knowledge_search: false,
                thinking_level: None,
                web_search_mode: None,
                reply_models: Vec::new(),
                group_selections: std::collections::HashMap::new(),
                forked_from: None,
                agent_runtime: settings.chat.default_agent_runtime.clone(),
            };

            crate::chat::repository::repository(app)
                .create(app, conversation)
                .await
                .map_err(crate::chat::repository::repository_error)?
        }
    };

    Ok(conversation)
}

/// 用一段预置的多轮历史 + 截图创建一个新会话（不触发回复）。
/// Lens「在 AI 客户端继续」按钮经由 external-send 管道走这条路径：
/// 把 Lens 浮窗内已有的 user/assistant 历史搬到客户端成为真正的对话历史，截图挂在首个 user 轮，
/// 用户落地后可直接继续输入。
#[tauri::command]
pub(crate) async fn chat_import_external_conversation(
    app: AppHandle,
    state: State<'_, AppState>,
    messages: Vec<ExternalConversationMessage>,
    attachments: Vec<String>,
    provider_id: Option<String>,
    model: Option<String>,
    project_id: Option<String>,
) -> Result<serde_json::Value, String> {
    let history: Vec<ExternalConversationMessage> = messages
        .into_iter()
        .filter(|m| !m.content.trim().is_empty())
        .collect();
    if history.is_empty() {
        return Err("Missing conversation history".to_string());
    }

    // 始终新建会话（不复用空白会话）：历史预置需要干净的容器。
    let mut conversation = create_chat_conversation_internal(
        &app,
        state.inner(),
        provider_id,
        model,
        None,
        project_id,
        None,
        None,
    )
    .await?;
    // create 可能复用了一个空白会话；这里清空以确保从干净状态写入历史。
    conversation.messages.clear();

    // 截图等附件存入会话目录，只挂在首个 user 轮。
    let stored_attachments = save_message_attachments(&app, &conversation.id, attachments)?;

    let now = chrono::Local::now().timestamp();
    let mut first_user_seen = false;
    let mut title_set = false;
    for entry in history {
        let role = if entry.role == "assistant" {
            "assistant"
        } else {
            "user"
        };
        let mut message_attachments: Vec<Attachment> = Vec::new();
        if role == "user" && !first_user_seen {
            first_user_seen = true;
            message_attachments = stored_attachments.clone();
        }
        if role == "user" && !title_set {
            let title_source = title_source_for_user_message(&entry.content, &message_attachments);
            if !title_source.is_empty() {
                conversation.title = title_source.chars().take(40).collect();
                title_set = true;
            }
        }
        conversation.messages.push(ChatMessage {
            id: format!("msg_{}", Uuid::new_v4()),
            role: role.to_string(),
            content: entry.content,
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
            group_id: None,
            provider_id: None,
            model: None,
            timestamp: now,
            degraded: None,
        });
    }

    let imported_title = conversation.title.clone();
    let imported_messages = conversation.messages.clone();
    conversation = crate::chat::repository::repository(&app)
        .mutate_expected(
            &app,
            &conversation.id,
            Some(conversation.revision),
            |latest| {
                latest.title = imported_title;
                latest.messages = imported_messages;
                Ok(())
            },
        )
        .await
        .map_err(crate::chat::repository::repository_error)?;

    strip_transcripts_for_frontend(&mut conversation);
    Ok(serde_json::json!({
        "success": true,
        "conversation": conversation,
    }))
}

fn non_empty_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[tauri::command]
pub(crate) fn chat_get_assistants(app: AppHandle) -> Result<serde_json::Value, String> {
    let assistants = get_assistants(&app, false)?;
    Ok(serde_json::json!({
        "success": true,
        "assistants": assistants,
    }))
}

#[tauri::command]
pub(crate) fn chat_create_assistant(
    app: AppHandle,
    assistant: ChatAssistant,
) -> Result<serde_json::Value, String> {
    let assistant = create_assistant(&app, assistant)?;
    Ok(serde_json::json!({
        "success": true,
        "assistant": assistant,
    }))
}

#[tauri::command]
pub(crate) fn chat_update_assistant(
    app: AppHandle,
    assistant: ChatAssistant,
) -> Result<serde_json::Value, String> {
    let assistant = update_assistant(&app, assistant)?;
    Ok(serde_json::json!({
        "success": true,
        "assistant": assistant,
    }))
}

/// 对话搭建专家的会话哨兵 id:挂在 assistant_snapshot 上,既注入搭建系统提示词,
/// 又作为「搭建模式」标记(仅此类会话暴露 save_assistant 工具)。
const BUILDER_ASSISTANT_ID: &str = "asst_builder";

pub(super) fn is_builder_conversation(conversation: &Conversation) -> bool {
    conversation
        .assistant_snapshot
        .as_ref()
        .map(|a| a.id.as_str())
        == Some(BUILDER_ASSISTANT_ID)
}

/// 把 `save_assistant` 的工具参数解析成一个待落库的 ChatAssistant(纯函数,便于单测)。
/// 校验/裁剪交给 storage::normalize_assistant;这里只做必填检查与字段提取。
pub(super) fn assistant_from_builder_args(arguments: &Value) -> Result<ChatAssistant, String> {
    let name = arguments
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "save_assistant 需要非空的 name".to_string())?;
    let system_prompt = arguments
        .get("system_prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if system_prompt.is_empty() {
        return Err("save_assistant 需要非空的 system_prompt".to_string());
    }
    let str_field = |key: &str| -> String {
        arguments
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let str_arr = |key: &str| -> Vec<String> {
        arguments
            .get(key)
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    let now = chrono::Local::now().timestamp();
    Ok(ChatAssistant {
        id: format!("asst_{}", Uuid::new_v4()),
        name: name.to_string(),
        description: str_field("description"),
        icon: str_field("icon"),
        color: str_field("color"),
        source: "user".to_string(),
        system_prompt: system_prompt.to_string(),
        provider_id: String::new(),
        model: String::new(),
        mcp_server_ids: str_arr("mcp_server_ids"),
        skill_ids: str_arr("skill_ids"),
        enabled: true,
        installed: true,
        archived: false,
        built_in: false,
        created_at: now,
        updated_at: now,
    })
}

/// 由对话搭建流程的 `save_assistant` 工具调用:把工具参数组装成一个新专家并落库。
/// 返回给模型的成功摘要。校验/字段裁剪交给 storage::normalize_assistant。
pub(crate) fn create_assistant_via_builder(
    app: &AppHandle,
    arguments: &Value,
) -> Result<String, String> {
    let assistant = assistant_from_builder_args(arguments)?;
    let saved = create_assistant(app, assistant)?;
    let _ = app.emit("chat-assistants-changed", &saved.id);
    Ok(format!(
        "已创建专家「{}」(MCP {} 个 / 技能 {} 个)。可在「专家套件」里查看、编辑或开始对话。",
        saved.name,
        saved.mcp_server_ids.len(),
        saved.skill_ids.len()
    ))
}

/// 构造搭建助手的系统提示词:固定流程指令 + 当前可用的 MCP 服务器与技能目录(供模型选 id)。
fn builder_system_prompt(app: &AppHandle, settings: &Settings) -> String {
    let mcp_block = {
        let items: Vec<String> = settings
            .chat_tools
            .servers
            .iter()
            .filter(|s| s.enabled)
            .map(|s| format!("- {} ({})", s.id, s.name))
            .collect();
        if items.is_empty() {
            "（无已启用的 MCP 服务器）".to_string()
        } else {
            items.join("\n")
        }
    };
    let skills_block = match skills::build_registry(app, &settings.chat_tools.skill_scan_paths) {
        Ok(registry) => {
            let items: Vec<String> = registry
                .records
                .iter()
                .filter(|r| crate::settings::is_skill_enabled(&settings.chat_tools, &r.meta.id))
                .map(|r| format!("- {} ({})", r.meta.id, r.meta.name))
                .collect();
            if items.is_empty() {
                "（无可用技能）".to_string()
            } else {
                items.join("\n")
            }
        }
        Err(_) => "（无可用技能）".to_string(),
    };
    format!(
        "你是「专家搭建助手」。任务:通过对话帮用户创建一个新的 Kivio 专家(assistant),最后调用 save_assistant 工具落库。回答语言跟随用户。\n\n\
流程:\n\
1. 先用一两个问题问清这个专家「要做什么、面向什么场景、语气/风格、有没有边界或禁忌」。一次只问一两个,别一次性列一堆。\n\
2. 据此为它撰写 system_prompt(这是该专家自己的系统指令,用第二人称写给它)。\n\
3. 判断它需要哪些 MCP 服务器和技能,只能从下面「可用」列表里选并给出精确 id;用不到就留空。\n\
4. 调用 save_assistant 前,先把完整配置(名称/描述/系统提示词要点/选用的 MCP/技能)复述给用户,得到明确确认后再调用;确认前不要调用工具。\n\
5. save_assistant 成功后,简短告诉用户已创建、可在「专家套件」查看。\n\n\
可用 MCP 服务器(格式 id (名称)):\n{mcp_block}\n\n\
可用技能(格式 id (名称)):\n{skills_block}\n\n\
注意:mcp_server_ids / skill_ids 必须使用上面列出的精确 id,不要编造;name 与 system_prompt 必填。"
    )
}

#[tauri::command]
pub(crate) async fn chat_create_builder_conversation(
    app: AppHandle,
    state: State<'_, AppState>,
    provider_id: Option<String>,
    model: Option<String>,
    project_id: Option<String>,
) -> Result<serde_json::Value, String> {
    let settings = state.settings_read().clone();
    let (default_provider_id, default_model) = settings.effective_chat_model();
    let provider_id = provider_id
        .and_then(non_empty_string)
        .unwrap_or(default_provider_id);
    let model = model.and_then(non_empty_string).unwrap_or(default_model);

    let project = match project_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(pid) => Some(find_project_by_id(&app, pid)?),
        None => None,
    };
    let resolved_project_id = project.as_ref().map(|p| p.id.clone());
    let folder = project.as_ref().map(|p| p.name.clone());

    let snapshot = crate::chat::types::ChatAssistantSnapshot {
        id: BUILDER_ASSISTANT_ID.to_string(),
        name: "专家搭建助手".to_string(),
        description: "通过对话帮你创建一个新专家。".to_string(),
        source: "builtin".to_string(),
        system_prompt: builder_system_prompt(&app, &settings),
        provider_id: String::new(),
        model: String::new(),
        mcp_server_ids: Vec::new(),
        skill_ids: Vec::new(),
    };

    let now = chrono::Local::now().timestamp();
    let conversation = Conversation {
        id: format!("conv_{}", Uuid::new_v4()),
        revision: 0,
        title: "搭建新专家".to_string(),
        provider_id,
        model,
        messages: vec![],
        active_skill_id: None,
        assistant_id: None,
        assistant_snapshot: Some(snapshot),
        created_at: now,
        updated_at: now,
        pinned: false,
        archived: false,
        folder,
        project_id: resolved_project_id,
        set_id: None,
        context_state: ConversationContextState::default(),
        agent_todo_state: AgentTodoState::default(),
        agent_plan_state: AgentPlanState::default(),
        knowledge_base_ids: Vec::new(),
        force_knowledge_search: false,
        thinking_level: None,
        web_search_mode: None,
        reply_models: Vec::new(),
        group_selections: std::collections::HashMap::new(),
        forked_from: None,
        agent_runtime: crate::chat::AgentRuntimeConfig::default(),
    };
    let conversation = crate::chat::repository::repository(&app)
        .create(&app, conversation)
        .await
        .map_err(crate::chat::repository::repository_error)?;
    Ok(serde_json::json!({
        "success": true,
        "conversation": conversation,
    }))
}

#[tauri::command]
pub(crate) fn chat_duplicate_assistant(
    app: AppHandle,
    assistant_id: String,
) -> Result<serde_json::Value, String> {
    let assistant = duplicate_assistant(&app, &assistant_id)?;
    Ok(serde_json::json!({
        "success": true,
        "assistant": assistant,
    }))
}

#[tauri::command]
pub(crate) fn chat_delete_assistant(
    app: AppHandle,
    assistant_id: String,
) -> Result<serde_json::Value, String> {
    archive_assistant(&app, &assistant_id)?;
    Ok(serde_json::json!({
        "success": true,
    }))
}

#[tauri::command]
pub(crate) fn chat_get_projects(app: AppHandle) -> Result<serde_json::Value, String> {
    let projects = get_projects(&app)?;
    Ok(serde_json::json!({
        "success": true,
        "projects": projects,
    }))
}

#[tauri::command]
pub(crate) fn chat_get_conversation_pins(app: AppHandle) -> Result<serde_json::Value, String> {
    let pins = crate::chat::storage::load_conversation_pins(&app)?;
    Ok(serde_json::json!({ "success": true, "pins": pins }))
}

/// `group_id` 是集或项目的 id（前缀不同，不会撞）。空 pins 表示清掉该分组的钉子。
#[tauri::command]
pub(crate) fn chat_set_conversation_pins(
    app: AppHandle,
    group_id: String,
    pins: Vec<crate::chat::ConversationPin>,
) -> Result<serde_json::Value, String> {
    crate::chat::storage::set_conversation_pins(&app, &group_id, pins)?;
    Ok(serde_json::json!({ "success": true }))
}

#[tauri::command]
pub(crate) fn chat_reorder_projects(
    app: AppHandle,
    ids: Vec<String>,
) -> Result<serde_json::Value, String> {
    let projects = crate::chat::storage::reorder_projects(&app, &ids)?;
    Ok(serde_json::json!({
        "success": true,
        "projects": projects,
    }))
}

#[tauri::command]
pub(crate) fn chat_create_project(
    app: AppHandle,
    name: String,
    description: Option<String>,
    color: Option<String>,
    root_path: Option<String>,
    // 为 true 时：root_path 不存在则在父目录下创建该文件夹（新建空白项目）。
    ensure_root_dir: Option<bool>,
) -> Result<serde_json::Value, String> {
    let now = chrono::Local::now().timestamp();
    let project = crate::chat::storage::create_project_with_options(
        &app,
        crate::chat::ChatProject {
            id: format!("proj_{}", Uuid::new_v4()),
            name,
            description,
            color,
            root_path,
            created_at: now,
            updated_at: now,
        },
        ensure_root_dir.unwrap_or(false),
    )?;

    Ok(serde_json::json!({
        "success": true,
        "project": project,
    }))
}

#[tauri::command]
pub(crate) async fn chat_update_project(
    app: AppHandle,
    project_id: String,
    name: Option<String>,
    description: Option<String>,
    description_set: Option<bool>,
    color: Option<String>,
    color_set: Option<bool>,
    root_path: Option<String>,
    root_path_set: Option<bool>,
) -> Result<serde_json::Value, String> {
    let description_has_value = description.is_some();
    let color_has_value = color.is_some();
    let root_path_has_value = root_path.is_some();
    let project = update_project(
        &app,
        &project_id,
        name,
        description,
        description_set.unwrap_or(description_has_value),
        color,
        color_set.unwrap_or(color_has_value),
        root_path,
        root_path_set.unwrap_or(root_path_has_value),
    )
    .await?;
    Ok(serde_json::json!({
        "success": true,
        "project": project,
    }))
}

#[tauri::command]
pub(crate) async fn chat_delete_project(
    app: AppHandle,
    project_id: String,
) -> Result<serde_json::Value, String> {
    delete_project(&app, &project_id).await?;
    Ok(serde_json::json!({
        "success": true,
    }))
}

#[tauri::command]
#[allow(deprecated)]
pub(crate) fn chat_project_open_folder(
    app: AppHandle,
    project_id: String,
) -> Result<serde_json::Value, String> {
    let project = find_project_by_id(&app, &project_id)?;
    let Some(root_path) = project
        .root_path
        .as_ref()
        .map(|path| path.trim())
        .filter(|path| !path.is_empty())
    else {
        return Err("该项目尚未配置文件夹".to_string());
    };
    let path = Path::new(root_path);
    if !path.is_dir() {
        return Err("项目文件夹不存在或无法访问".to_string());
    }
    app.shell()
        .open(root_path.to_string(), None)
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "success": true,
        "path": root_path,
    }))
}

// ===== Chat 集(Set) 命令：仿 project 命令 =====

#[tauri::command]
pub(crate) fn chat_get_sets(app: AppHandle) -> Result<serde_json::Value, String> {
    let sets = get_sets(&app)?;
    Ok(serde_json::json!({ "success": true, "sets": sets }))
}

#[tauri::command]
pub(crate) fn chat_reorder_sets(
    app: AppHandle,
    ids: Vec<String>,
) -> Result<serde_json::Value, String> {
    let sets = crate::chat::storage::reorder_sets(&app, &ids)?;
    Ok(serde_json::json!({ "success": true, "sets": sets }))
}

#[tauri::command]
pub(crate) fn chat_create_set(
    app: AppHandle,
    name: String,
    system_prompt: Option<String>,
    default_assistant_id: Option<String>,
    color: Option<String>,
) -> Result<serde_json::Value, String> {
    let now = chrono::Local::now().timestamp();
    let set = create_set(
        &app,
        crate::chat::ChatSet {
            id: format!("set_{}", Uuid::new_v4()),
            name,
            system_prompt: system_prompt.unwrap_or_default(),
            default_assistant_id: default_assistant_id.filter(|id| !id.trim().is_empty()),
            color,
            created_at: now,
            updated_at: now,
        },
    )?;
    Ok(serde_json::json!({ "success": true, "set": set }))
}

#[tauri::command]
pub(crate) fn chat_update_set(
    app: AppHandle,
    set_id: String,
    name: Option<String>,
    system_prompt: Option<String>,
    system_prompt_set: Option<bool>,
    default_assistant_id: Option<String>,
    default_assistant_id_set: Option<bool>,
    color: Option<String>,
    color_set: Option<bool>,
) -> Result<serde_json::Value, String> {
    let system_prompt_has_value = system_prompt.is_some();
    let default_assistant_has_value = default_assistant_id.is_some();
    let color_has_value = color.is_some();
    let set = update_set(
        &app,
        &set_id,
        name,
        system_prompt,
        system_prompt_set.unwrap_or(system_prompt_has_value),
        default_assistant_id,
        default_assistant_id_set.unwrap_or(default_assistant_has_value),
        color,
        color_set.unwrap_or(color_has_value),
    )?;
    Ok(serde_json::json!({ "success": true, "set": set }))
}

#[tauri::command]
pub(crate) async fn chat_delete_set(
    app: AppHandle,
    set_id: String,
) -> Result<serde_json::Value, String> {
    delete_set(&app, &set_id).await?;
    Ok(serde_json::json!({ "success": true }))
}
