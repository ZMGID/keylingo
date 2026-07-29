use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::chat::storage::{
    create_project, get_projects, load_conversation, save_conversation, update_project,
};
use crate::chat::ChatMessage;
use crate::state::AppState;

use super::catalog::create_chat_conversation_internal;
use super::complete_assistant_reply_inner;
/// 无头测试通道的一次生成编排（仅 debug）：把 scratch 会话绑到一个**固定复用**的
/// 「Chat Probe」项目（根为请求的 cwd，使文件工具相对路径可解析）→ 推入 user 消息 →
/// 走与 GUI 完全相同的生成核心（`complete_assistant_reply_inner`，probe=true 自动放行）→
/// 取回生成的 assistant 消息。**会话与项目都保留**（不删除），以便在会话列表里观察调试。
/// 返回 `(会话 id, assistant 消息)`——会话 id 回传给调用方，供下一次请求续聊。
///
/// `conversation_id = Some(..)` 续聊已有会话（不新建、不改标题/项目/运行时）。**多轮场景
/// 只有这条路能测**：跨轮记忆、外部 CLI 常驻会话复用、压缩边界，都需要同一个会话连发多轮。
///
/// `external_agent_id = Some(..)` 把新建的会话钉到外部 CLI 运行时。没有它就只能去改
/// `settings.chat.defaultAgentRuntime`——那是全局副作用，且改错嵌套层级不会报错、只会静默
/// 跑回内置路径（真机验收时踩过）。
#[cfg(debug_assertions)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_chat_probe(
    app: &AppHandle,
    state: &State<'_, AppState>,
    prompt: String,
    provider: Option<String>,
    model: Option<String>,
    skill_id: Option<String>,
    mode: Option<String>,
    cwd: Option<String>,
    web_search_mode: Option<String>,
    conversation_id: Option<String>,
    external_agent_id: Option<String>,
) -> Result<(String, ChatMessage), String> {
    const PROBE_PROJECT_ID: &str = "proj_kivio_probe";
    let resume_id = conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());

    let mut conversation = match resume_id {
        Some(id) => load_conversation(app, id)?,
        None => new_probe_conversation(
            app,
            state,
            &prompt,
            provider,
            model,
            cwd,
            external_agent_id,
            PROBE_PROJECT_ID,
        )?,
    };
    // 可选运行模式（act/plan/orchestrate）：验证模式提示词用。非法值报错而非静默回落。
    if let Some(mode) = mode.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        let mode = crate::chat::plan::mode_from_str(mode)?;
        conversation.agent_plan_state =
            crate::chat::plan::with_mode(&conversation.agent_plan_state, mode);
    }
    // 可选会话级联网搜索模式（off/builtin/third_party）：验证内置搜索链路用（任务 07-23）。
    if let Some(ws) = web_search_mode
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        use crate::chat::types::WebSearchMode;
        conversation.web_search_mode = Some(match ws {
            "off" => WebSearchMode::Off,
            "builtin" => WebSearchMode::Builtin,
            "third_party" => WebSearchMode::ThirdParty,
            other => return Err(format!("invalid webSearchMode: {other}")),
        });
    }
    let user_message = ChatMessage {
        id: format!("msg_{}", Uuid::new_v4()),
        role: "user".to_string(),
        content: prompt.clone(),
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
        timestamp: chrono::Local::now().timestamp(),
        degraded: None,
    };
    conversation.messages.push(user_message);
    conversation.updated_at = chrono::Local::now().timestamp();
    save_conversation(app, &conversation)?;

    let gen_result = complete_assistant_reply_inner(
        app,
        state,
        &mut conversation,
        None,
        Some(prompt.as_str()),
        &[],
        skill_id.as_deref(),
        crate::chat::agent::AgentRunEntry::Send,
        None,
        /* probe */ true,
    )
    .await;

    // 拿到最后一条 assistant 消息（complete_assistant_reply_inner 已 push+save 到会话）。
    // 会话与项目都保留在列表里，供观察调试——不删除。
    let assistant = conversation
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "assistant")
        .cloned();

    gen_result?;
    assistant
        .map(|message| (conversation.id.clone(), message))
        .ok_or_else(|| "probe: no assistant message produced".to_string())
}

/// 新建一次 probe 会话：绑到固定复用的「Chat Probe」项目（根为 cwd，使文件工具相对路径可解析），
/// 标题取自 prompt 便于在列表里识别，可选钉到外部 CLI 运行时。
#[cfg(debug_assertions)]
#[allow(clippy::too_many_arguments)]
fn new_probe_conversation(
    app: &AppHandle,
    state: &State<'_, AppState>,
    prompt: &str,
    provider: Option<String>,
    model: Option<String>,
    cwd: Option<String>,
    external_agent_id: Option<String>,
    probe_project_id: &str,
) -> Result<crate::chat::Conversation, String> {
    // cwd → 固定复用的「Chat Probe」项目：根设为 cwd，使文件工具（read/glob/grep）相对路径
    // 从此解析（非项目会话是 global workspace 无根，与真实 GUI 一致）。复用同一项目避免污染
    // 列表；不删除，方便在会话列表里点开观察每次 probe 的完整轨迹。
    let project_id = if let Some(cwd) = cwd.as_deref().filter(|c| !c.trim().is_empty()) {
        let now = chrono::Local::now().timestamp();
        let exists = get_projects(app)?
            .into_iter()
            .any(|p| p.id == probe_project_id);
        if exists {
            // 更新根到本次 cwd（其余字段不动）。
            let _ = update_project(
                app,
                probe_project_id,
                None,
                None,
                false,
                None,
                false,
                Some(cwd.to_string()),
                true,
            );
        } else {
            create_project(
                app,
                crate::chat::types::ChatProject {
                    id: probe_project_id.to_string(),
                    name: "Chat Probe".to_string(),
                    description: Some(
                        "无头测试通道（debug）的会话都在这里，可点开观察".to_string(),
                    ),
                    color: None,
                    root_path: Some(cwd.to_string()),
                    created_at: now,
                    updated_at: now,
                },
            )?;
        }
        Some(probe_project_id.to_string())
    } else {
        None
    };

    let mut conversation = create_chat_conversation_internal(
        app,
        state.inner(),
        provider,
        model,
        None,
        project_id,
        None,
        None,
    )?;
    conversation.title = {
        let head: String = prompt.chars().take(60).collect();
        format!("🔬 {head}")
    };
    // 外部 CLI 运行时：只在新建时钉一次。续聊会话的运行时不许改——有消息的外部会话禁切
    // kind/agent（spec 第 3b 条），改了会被后端校验拒绝。
    if let Some(agent) = external_agent_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        conversation.agent_runtime = crate::chat::AgentRuntimeConfig {
            kind: crate::chat::types::AgentRuntimeKind::External,
            external_agent_id: Some(agent.to_string()),
            ..Default::default()
        };
    }
    Ok(conversation)
}
