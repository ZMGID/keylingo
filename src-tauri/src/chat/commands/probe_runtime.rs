use std::time::Duration;

use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::chat::probe::ProbeRequest;
use crate::chat::storage::{create_project, get_projects, load_conversation, update_project};
use crate::chat::{ChatMessage, ConversationContextState};
use crate::state::AppState;

use super::catalog::create_chat_conversation_internal;
use super::complete_assistant_reply_inner;

/// 一次 probe 生成的产物。
#[cfg(debug_assertions)]
pub(crate) struct ProbeRun {
    pub(crate) conversation_id: String,
    pub(crate) message: ChatMessage,
    /// 仅当请求带 `computeContextStats` 时才有：轮末主动算一次（见 `run_chat_probe`）。
    pub(crate) context_state: Option<ConversationContextState>,
}

/// 生成失败。会话 id 尽量带上——失败往往正是要在同一个会话里继续追查的对象
/// （例如「取消之后这个会话还能不能用」）。
#[cfg(debug_assertions)]
pub(crate) struct ProbeRunError {
    pub(crate) conversation_id: Option<String>,
    pub(crate) error: String,
}

/// 无头测试通道的一次生成编排（仅 debug）：把 scratch 会话绑到一个**固定复用**的
/// 「Chat Probe」项目（根为请求的 cwd，使文件工具相对路径可解析）→ 推入 user 消息 →
/// 走与 GUI 完全相同的生成核心（`complete_assistant_reply_inner`，probe=true 自动放行）→
/// 取回生成的 assistant 消息。**会话与项目都保留**（不删除），以便在会话列表里观察调试。
///
/// `conversationId = Some(..)` 续聊已有会话（不新建、不改标题/项目/运行时）。**多轮场景
/// 只有这条路能测**：跨轮记忆、外部 CLI 常驻会话复用、压缩边界，都需要同一个会话连发多轮。
///
/// `externalAgentId = Some(..)` 把新建的会话钉到外部 CLI 运行时。没有它就只能去改
/// `settings.chat.defaultAgentRuntime`——那是全局副作用，且改错嵌套层级不会报错、只会静默
/// 跑回内置路径（真机验收时踩过）。
///
/// `externalModel` / `externalReasoning` / `externalSandbox` **新建与续聊都生效**（spec 第 3b
/// 条只禁切 kind/agent，这三项放行）：它们是 claude 的启动参数，中途改动会触发 `LaunchConfig`
/// 指纹不匹配 ⇒ 换进程 + 原生 resume（spec 第 26 条）。配上 result 里的 `liveSession.childPid`
/// 就能断言「pid 变了但上下文还在」。
///
/// `cancelAfterMs` 到点触发一次取消，**走生产那条路**（见 `schedule_probe_cancel`）。
///
/// `computeContextStats` 在轮末复用 `chat_get_context_stats` 再算一次上下文状态：这个字段
/// 平时只在用户点开用量条时才算，不主动算的话 result 里的分子/分母全是空的。
#[cfg(debug_assertions)]
pub(crate) async fn run_chat_probe(
    app: &AppHandle,
    state: &State<'_, AppState>,
    req: &ProbeRequest,
) -> Result<ProbeRun, ProbeRunError> {
    const PROBE_PROJECT_ID: &str = "proj_kivio_probe";
    let resume_id = req
        .conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());

    let fail = |conversation_id: Option<String>, error: String| ProbeRunError {
        conversation_id,
        error,
    };

    let mut conversation = match resume_id {
        Some(id) => load_conversation(app, id).map_err(|e| fail(Some(id.to_string()), e))?,
        None => new_probe_conversation(app, state, req, PROBE_PROJECT_ID)
            .await
            .map_err(|e| fail(None, e))?,
    };
    let conversation_id = conversation.id.clone();
    let fail = move |error: String| ProbeRunError {
        conversation_id: Some(conversation_id.clone()),
        error,
    };

    // 外部 CLI 的 model / reasoning / sandbox：新建与续聊都生效（见函数文档）。
    // 只在会话确实绑了外部运行时的情况下写，避免给内置会话留下无意义的残留配置。
    if conversation.agent_runtime.is_external() {
        apply_external_runtime_overrides(&mut conversation.agent_runtime, req);
    }
    // 可选运行模式（act/plan/orchestrate）：验证模式提示词用。非法值报错而非静默回落。
    if let Some(mode) = req.mode.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        let mode = crate::chat::plan::mode_from_str(mode).map_err(&fail)?;
        conversation.agent_plan_state =
            crate::chat::plan::with_mode(&conversation.agent_plan_state, mode);
    }
    // 可选会话级联网搜索模式（off/builtin/third_party）：验证内置搜索链路用（任务 07-23）。
    if let Some(ws) = req
        .web_search_mode
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        use crate::chat::types::WebSearchMode;
        conversation.web_search_mode = Some(match ws {
            "off" => WebSearchMode::Off,
            "builtin" => WebSearchMode::Builtin,
            "third_party" => WebSearchMode::ThirdParty,
            other => return Err(fail(format!("invalid webSearchMode: {other}"))),
        });
    }
    let user_message = ChatMessage {
        id: format!("msg_{}", Uuid::new_v4()),
        role: "user".to_string(),
        content: req.prompt.clone(),
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
    let runtime = conversation.agent_runtime.clone();
    let plan_state = conversation.agent_plan_state.clone();
    let web_search_mode = conversation.web_search_mode;
    conversation = crate::chat::repository::repository(app)
        .mutate_expected(
            app,
            &conversation.id,
            Some(conversation.revision),
            |latest| {
                latest.agent_runtime = runtime;
                latest.agent_plan_state = plan_state;
                latest.web_search_mode = web_search_mode;
                latest.messages.push(user_message);
                Ok(())
            },
        )
        .await
        .map_err(crate::chat::repository::repository_error)
        .map_err(&fail)?;

    if let Some(delay_ms) = req.cancel_after_ms {
        schedule_probe_cancel(app, &conversation.id, Duration::from_millis(delay_ms));
    }

    let gen_result = complete_assistant_reply_inner(
        app,
        state,
        &mut conversation,
        None,
        Some(req.prompt.as_str()),
        &[],
        req.skill_id.as_deref(),
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

    gen_result.map_err(&fail)?;
    let message =
        assistant.ok_or_else(|| fail("probe: no assistant message produced".to_string()))?;

    let context_state = if req.compute_context_stats {
        compute_context_state_for_probe(app, &conversation.id).await
    } else {
        None
    };

    Ok(ProbeRun {
        conversation_id: conversation.id.clone(),
        message,
        context_state,
    })
}

/// 把请求里的外部运行时覆盖写进会话配置。空串按「不覆盖」处理（JSON 里省略字段与写空串
/// 应当等价，否则脚本里一个手滑的 `""` 会静默把模型清成默认值）。
#[cfg(debug_assertions)]
fn apply_external_runtime_overrides(
    runtime: &mut crate::chat::AgentRuntimeConfig,
    req: &ProbeRequest,
) {
    let pick = |value: &Option<String>| -> Option<String> {
        value
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    if let Some(model) = pick(&req.external_model) {
        runtime.external_model = Some(model);
    }
    if let Some(reasoning) = pick(&req.external_reasoning) {
        runtime.external_reasoning = Some(reasoning);
    }
    if let Some(sandbox) = pick(&req.external_sandbox) {
        runtime.external_sandbox = Some(sandbox);
    }
}

/// 轮末算一次上下文状态，**复用 `chat_get_context_stats` 那条生产路径**（用户点开用量条时
/// 走的就是它），而不是另写一份计算——两份口径迟早分叉（spec 第 2 条）。
///
/// 算不出来时返回 `None` 而不是报错：上下文状态是观测项，不该把一轮成功的生成判成失败。
#[cfg(debug_assertions)]
async fn compute_context_state_for_probe(
    app: &AppHandle,
    conversation_id: &str,
) -> Option<ConversationContextState> {
    let state = app.state::<AppState>();
    let value =
        super::context::chat_get_context_stats(app.clone(), state, conversation_id.to_string())
            .await
            .map_err(|err| {
                eprintln!("[chat-probe] computeContextStats failed: {err}");
                err
            })
            .ok()?;
    serde_json::from_value(value.get("contextState")?.clone()).ok()
}

/// 到点触发一次取消。
///
/// **必须与用户点「停止」是同一条路**：`chat_cancel_stream` 这条 Tauri 命令的全部内容就是
/// `state.cancel_chat_generation(&conversation_id)`，这里调的就是它。另造一条取消路径测到的
/// 就不是生产行为了。
///
/// 计时从「本轮 generation 真正登记」起算：`cancel_chat_generation` 只是把该会话的活跃
/// generation 集合清空，在登记之前调用是**空操作**，取消会静默失效（而测试会看到一轮正常
/// 完成的回答，最难排查的那种假绿）。
#[cfg(debug_assertions)]
fn schedule_probe_cancel(app: &AppHandle, conversation_id: &str, delay: Duration) {
    /// 等待 generation 登记的上限。超时就放弃并明确打日志——静默不取消是最坏的结果。
    const ARM_TIMEOUT: Duration = Duration::from_secs(60);
    const ARM_POLL: Duration = Duration::from_millis(50);

    let app = app.clone();
    let conversation_id = conversation_id.to_string();
    tauri::async_runtime::spawn(async move {
        let armed = tokio::time::timeout(ARM_TIMEOUT, async {
            loop {
                let active = {
                    let state = app.state::<AppState>();
                    let map = state
                        .chat_active_generations
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    map.get(&conversation_id).is_some_and(|set| !set.is_empty())
                };
                if active {
                    return;
                }
                tokio::time::sleep(ARM_POLL).await;
            }
        })
        .await
        .is_ok();
        if !armed {
            eprintln!(
                "[chat-probe] cancelAfterMs: {ARM_TIMEOUT:?} 内没等到活跃 generation，放弃取消"
            );
            return;
        }
        tokio::time::sleep(delay).await;
        app.state::<AppState>()
            .cancel_chat_generation(&conversation_id);
        eprintln!("[chat-probe] cancelAfterMs: cancelled {conversation_id}");
    });
}

/// 新建一次 probe 会话：绑到固定复用的「Chat Probe」项目（根为 cwd，使文件工具相对路径可解析），
/// 标题取自 prompt 便于在列表里识别，可选钉到外部 CLI 运行时。
#[cfg(debug_assertions)]
async fn new_probe_conversation(
    app: &AppHandle,
    state: &State<'_, AppState>,
    req: &ProbeRequest,
    probe_project_id: &str,
) -> Result<crate::chat::Conversation, String> {
    // cwd → 固定复用的「Chat Probe」项目：根设为 cwd，使文件工具（read/glob/grep）相对路径
    // 从此解析（非项目会话是 global workspace 无根，与真实 GUI 一致）。复用同一项目避免污染
    // 列表；不删除，方便在会话列表里点开观察每次 probe 的完整轨迹。
    let project_id = if let Some(cwd) = req.cwd.as_deref().filter(|c| !c.trim().is_empty()) {
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
            )
            .await;
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
        req.provider.clone(),
        req.model.clone(),
        None,
        project_id,
        None,
        None,
    )
    .await?;
    conversation.title = {
        let head: String = req.prompt.chars().take(60).collect();
        format!("🔬 {head}")
    };
    // 外部 CLI 运行时：只在新建时钉一次。续聊会话的运行时不许改——有消息的外部会话禁切
    // kind/agent（spec 第 3b 条），改了会被后端校验拒绝。
    if let Some(agent) = req
        .external_agent_id
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

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;

    fn req_with(
        model: Option<&str>,
        reasoning: Option<&str>,
        sandbox: Option<&str>,
    ) -> ProbeRequest {
        ProbeRequest {
            external_model: model.map(str::to_string),
            external_reasoning: reasoning.map(str::to_string),
            external_sandbox: sandbox.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn external_overrides_only_touch_the_fields_that_were_sent() {
        let mut runtime = crate::chat::AgentRuntimeConfig {
            kind: crate::chat::types::AgentRuntimeKind::External,
            external_agent_id: Some("claude".to_string()),
            external_model: Some("sonnet".to_string()),
            external_reasoning: Some("high".to_string()),
            external_sandbox: Some("bypassPermissions".to_string()),
            external_agent_preset: None,
        };
        apply_external_runtime_overrides(&mut runtime, &req_with(Some("opus"), None, None));
        assert_eq!(runtime.external_model.as_deref(), Some("opus"));
        // 没发的字段保持原样——否则「只换 sandbox」会顺手把模型清掉，测出来的就不是那条路。
        assert_eq!(runtime.external_reasoning.as_deref(), Some("high"));
        assert_eq!(
            runtime.external_sandbox.as_deref(),
            Some("bypassPermissions")
        );
        assert_eq!(runtime.external_agent_id.as_deref(), Some("claude"));
    }

    /// 空串等同于「没发」：脚本里一个手滑的 `""` 不该静默把模型清成默认值。
    #[test]
    fn blank_external_overrides_are_ignored() {
        let mut runtime = crate::chat::AgentRuntimeConfig {
            kind: crate::chat::types::AgentRuntimeKind::External,
            external_agent_id: Some("claude".to_string()),
            external_model: Some("sonnet".to_string()),
            ..Default::default()
        };
        apply_external_runtime_overrides(&mut runtime, &req_with(Some("   "), Some(""), None));
        assert_eq!(runtime.external_model.as_deref(), Some("sonnet"));
        assert!(runtime.external_reasoning.is_none());
    }
}
