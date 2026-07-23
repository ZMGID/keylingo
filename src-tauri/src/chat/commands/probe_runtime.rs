use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::chat::storage::{create_project, get_projects, save_conversation, update_project};
use crate::chat::ChatMessage;
use crate::state::AppState;

use super::catalog::create_chat_conversation_internal;
use super::complete_assistant_reply_inner;
/// 无头测试通道的一次生成编排（仅 debug）：把 scratch 会话绑到一个**固定复用**的
/// 「Chat Probe」项目（根为请求的 cwd，使文件工具相对路径可解析）→ 推入 user 消息 →
/// 走与 GUI 完全相同的生成核心（`complete_assistant_reply_inner`，probe=true 自动放行）→
/// 取回生成的 assistant 消息。**会话与项目都保留**（不删除），以便在会话列表里观察调试。
/// 返回 assistant 消息（含 content + tool_calls + stream_outcome + usage）。
#[cfg(debug_assertions)]
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
) -> Result<ChatMessage, String> {
    const PROBE_PROJECT_ID: &str = "proj_kivio_probe";
    // cwd → 固定复用的「Chat Probe」项目：根设为 cwd，使文件工具（read/glob/grep）相对路径
    // 从此解析（非项目会话是 global workspace 无根，与真实 GUI 一致）。复用同一项目避免污染
    // 列表；不删除，方便在会话列表里点开观察每次 probe 的完整轨迹。
    let project_id = if let Some(cwd) = cwd.as_deref().filter(|c| !c.trim().is_empty()) {
        let now = chrono::Local::now().timestamp();
        let exists = get_projects(app)?
            .into_iter()
            .any(|p| p.id == PROBE_PROJECT_ID);
        if exists {
            // 更新根到本次 cwd（其余字段不动）。
            let _ = update_project(
                app,
                PROBE_PROJECT_ID,
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
                    id: PROBE_PROJECT_ID.to_string(),
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
        Some(PROBE_PROJECT_ID.to_string())
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
    // 会话标题取自 prompt（截断），便于在列表里识别。
    conversation.title = {
        let head: String = prompt.chars().take(60).collect();
        format!("🔬 {head}")
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
    assistant.ok_or_else(|| "probe: no assistant message produced".to_string())
}
