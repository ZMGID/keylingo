use std::{collections::HashMap, time::Duration};

use serde_json::Value;
use tauri::{AppHandle, State};
use tokio::time::{sleep, timeout};

use crate::chat::agent::execute::truncate_chars;
use crate::chat::attachments::{compose_text_attachments_for_api, TextAttachmentInput};
use crate::chat::{AgentPlanState, ChatMessageSegment, Conversation, ToolCallRecord};
use crate::mcp::types::ChatToolArtifact;
use crate::state::AppState;

use super::catalog::strip_transcripts_for_frontend;

/// 取走外部入口排队给 Chat 前端发送的消息。
#[tauri::command]
pub(crate) fn chat_take_external_sends(
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let requests = {
        let mut pending = state
            .pending_chat_external_sends
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *pending)
    };

    Ok(serde_json::json!({
        "success": true,
        "requests": requests,
    }))
}

#[tauri::command]
pub(crate) async fn chat_set_agent_plan_mode(
    app: AppHandle,
    conversation_id: String,
    mode: String,
) -> Result<serde_json::Value, String> {
    let mode = crate::chat::plan::mode_from_str(&mode)?;
    let mut conversation = crate::chat::repository::repository(&app)
        .mutate(&app, &conversation_id, |conversation| {
            conversation.agent_plan_state =
                crate::chat::plan::with_mode(&conversation.agent_plan_state, mode);
            Ok(())
        })
        .await
        .map_err(crate::chat::repository::repository_error)?;
    emit_chat_plan_state(
        &app,
        &conversation.id,
        conversation.revision,
        &conversation.agent_plan_state,
    );

    strip_transcripts_for_frontend(&mut conversation);
    Ok(serde_json::json!({
        "success": true,
        "conversation": conversation,
        "planState": conversation.agent_plan_state,
    }))
}

#[tauri::command]
pub(crate) async fn chat_execute_agent_plan(
    app: AppHandle,
    conversation_id: String,
    message_id: Option<String>,
) -> Result<serde_json::Value, String> {
    let mut conversation = crate::chat::repository::repository(&app)
        .mutate(&app, &conversation_id, |conversation| {
            approve_agent_plan_for_execution(conversation, message_id.as_deref())
        })
        .await
        .map_err(crate::chat::repository::repository_error)?;
    emit_chat_plan_state(
        &app,
        &conversation.id,
        conversation.revision,
        &conversation.agent_plan_state,
    );

    strip_transcripts_for_frontend(&mut conversation);
    Ok(serde_json::json!({
        "success": true,
        "conversation": conversation,
        "planState": conversation.agent_plan_state,
    }))
}

pub(super) fn approve_agent_plan_for_execution(
    conversation: &mut Conversation,
    message_id: Option<&str>,
) -> Result<(), String> {
    let selected_plan =
        if let Some(message_id) = message_id.map(str::trim).filter(|id| !id.is_empty()) {
            Some({
                let message = conversation
                    .messages
                    .iter_mut()
                    .find(|message| message.id == message_id && message.role == "assistant")
                    .ok_or_else(|| "计划消息不存在".to_string())?;
                let plan_state = message
                    .agent_plan
                    .as_ref()
                    .ok_or_else(|| "该消息不是可执行计划".to_string())?;
                if crate::chat::plan::executable_plan_text(plan_state).is_none() {
                    return Err("该消息不是可执行计划".to_string());
                }
                let approved = crate::chat::plan::approve(plan_state);
                message.agent_plan = Some(approved.clone());
                approved
            })
        } else {
            None
        };
    conversation.agent_plan_state =
        selected_plan.unwrap_or_else(|| crate::chat::plan::approve(&conversation.agent_plan_state));
    Ok(())
}

/// 取消指定对话的当前 Chat 生成或工具执行。
#[tauri::command]
pub(crate) fn chat_cancel_stream(
    state: State<AppState>,
    conversation_id: String,
) -> Result<(), String> {
    state.cancel_chat_generation(&conversation_id);
    Ok(())
}

/// 响应敏感工具调用确认。`always=true` 时额外把「本对话内该工具不再询问」落进
/// `chat_tool_always_allow`——决策通道本身仍是 bool，三态只存在于这一层。
#[tauri::command]
pub(crate) fn chat_confirm_tool_call(
    app: AppHandle,
    state: State<AppState>,
    tool_call_id: String,
    approved: bool,
    always: Option<bool>,
    // 计划批准（`ExitPlanMode`）三选一里用户选的那一档，决定批准后把 CLI 切到哪个权限
    // 模式。普通审批不传。
    permission_mode: Option<String>,
) -> Result<(), String> {
    let pending = state
        .pending_chat_tool_approvals
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&tool_call_id);
    if let Some(pending) = pending {
        if approved && always.unwrap_or(false) {
            state.grant_tool_always_allow(&pending.conversation_id, &pending.tool_name);
        }
        let _ = pending.sender.send(crate::state::ToolApprovalOutcome {
            approved,
            permission_mode: permission_mode
                .map(|mode| mode.trim().to_string())
                .filter(|mode| !mode.is_empty()),
        });
        crate::chat::protocol::withdraw_tool_approval(&app, &tool_call_id);
    }
    Ok(())
}

/// 返回开发者「请求调试」缓冲快照（最新在前）。仅内存，未开启开关时通常为空。
#[tauri::command]
pub(crate) fn get_request_debug_records(
    state: State<AppState>,
) -> Vec<crate::chat::request_debug::RequestDebugRecord> {
    crate::chat::request_debug::snapshot(&state)
}

/// 清空开发者「请求调试」缓冲。
#[tauri::command]
pub(crate) fn clear_request_debug_records(state: State<AppState>) {
    crate::chat::request_debug::clear(&state);
}

/// Background tasks 面板的统一列表：内置 `run_command background:true` 作业 +
/// 外部 CLI（claude）自报的后台任务（后台 Bash / 后台子代理）。Running 与终态都返回，
/// 前端分 Running / Finished 两段渲染；`startedAtMs` 供排序，`elapsedSecs` 只对
/// running 有意义（终态不显示时长，Desktop 面板同款语义）。
///
/// **按对话隔离**：面板挂在会话视图里，只展示当前对话自己的任务（换对话/新建对话
/// 不能看到别人的）。`conversation_id` 为 `None` 时不过滤（诊断用）；内置作业里
/// 无会话归属的旧条目在过滤模式下不展示。
///
/// 外部任务的对账：常驻 CLI 会话没了（回收 / 重连 / 崩溃）⇒ 它的后台任务随进程一起
/// 消失，但 `task_notification` 永远不会来。列表是唯一的读取口，就在这里把这类
/// 孤儿 running 条目翻成 stopped（单点收口，不用追每个会话关闭路径）。
#[tauri::command]
pub(crate) fn chat_list_background_tasks(
    state: State<AppState>,
    conversation_id: Option<String>,
) -> Vec<serde_json::Value> {
    let wanted = conversation_id.as_deref();
    let mut out: Vec<(std::time::SystemTime, serde_json::Value)> = Vec::new();

    {
        let map = state.background_commands_handle();
        let map = map.lock().unwrap_or_else(|e| e.into_inner());
        for j in map.values() {
            if wanted.is_some() && j.conversation_id.as_deref() != wanted {
                continue;
            }
            use crate::native_tools::BackgroundCommandStatus as S;
            let (status, exit_code) = match &j.status {
                S::Running => ("running", None),
                S::Exited { code: Some(0) } => ("completed", Some(0)),
                S::Exited { code } => ("failed", *code),
                S::Killed => ("stopped", None),
                S::Error { .. } => ("failed", None),
            };
            let value = serde_json::json!({
                "id": j.job_id,
                "source": "builtin",
                "kind": "bash",
                "title": j.command,
                "status": status,
                "exitCode": exit_code,
                "pid": j.pid,
                "cwd": j.cwd,
                "elapsedSecs": j.started_at.elapsed().map(|d| d.as_secs()).unwrap_or(0),
                "startedAtMs": epoch_ms(j.started_at),
            });
            out.push((j.started_at, value));
        }
    }

    {
        let mut map = state
            .external_background_tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for t in map.values_mut() {
            if wanted.is_some() && Some(t.conversation_id.as_str()) != wanted {
                continue;
            }
            if t.status == "running"
                && state
                    .external_live_session_control_any(&t.conversation_id)
                    .is_none()
            {
                t.status = "stopped".to_string();
                t.ended_at = Some(std::time::SystemTime::now());
            }
            let value = serde_json::json!({
                "id": t.task_id,
                "source": "external",
                "kind": t.kind,
                "title": t.description,
                "status": t.status,
                "summary": t.summary,
                "conversationId": t.conversation_id,
                "elapsedSecs": t.started_at.elapsed().map(|d| d.as_secs()).unwrap_or(0),
                "startedAtMs": epoch_ms(t.started_at),
            });
            out.push((t.started_at, value));
        }
    }

    out.sort_by_key(|(started_at, _)| *started_at);
    out.into_iter().map(|(_, value)| value).collect()
}

fn epoch_ms(t: std::time::SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 面板的「清空 Finished」：两个注册表里**本对话**的终态条目一并移除（`None` = 不过滤）。
/// 运行中的不动。注意被清的内置作业其 `bash_output` 会开始报「no such job」——用户显式
/// 清理，可接受；日志文件仍在磁盘（启动 GC 兜底）。
#[tauri::command]
pub(crate) fn chat_clear_finished_background_tasks(
    state: State<AppState>,
    conversation_id: Option<String>,
) {
    let wanted = conversation_id.as_deref();
    {
        let map = state.background_commands_handle();
        let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, j| {
            matches!(
                j.status,
                crate::native_tools::BackgroundCommandStatus::Running
            ) || (wanted.is_some() && j.conversation_id.as_deref() != wanted)
        });
    }
    state
        .external_background_tasks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|_, t| {
            t.status == "running"
                || (wanted.is_some() && Some(t.conversation_id.as_str()) != wanted)
        });
}

/// 面板停止一条外部 CLI 后台任务：往常驻会话 actor 送 `StopTask`（claude 写
/// `stop_task` 控制请求；dsh 写 `session/stop-job`）。乐观置 stopped —— 确认帧
/// 要到下一次读 stdout 才到，等它面板会像没反应；真没停掉的话下一轮的帧会把状态改回来。
#[tauri::command]
pub(crate) async fn chat_stop_external_background_task(
    state: State<'_, AppState>,
    conversation_id: String,
    task_id: String,
) -> Result<(), String> {
    if let Some(control) = state.external_live_session_control_any(&conversation_id) {
        let _ = control
            .send(
                crate::external_agents::session::live::SessionCommand::StopTask {
                    task_id: task_id.clone(),
                },
            )
            .await;
    }
    // 会话已没了 ⇒ 任务随进程消失，同样落到 stopped。
    state.upsert_external_background_task(&conversation_id, &task_id, "stopped", None, None, None);
    Ok(())
}

/// 从 UI 终止一个后台命令。复用 agent 的 `kill_background`（整组杀 + 标记 Killed）。
/// 用户从 UI 面板显式操作（面板已按对话过滤过），不再二次传会话过滤。
#[tauri::command]
pub(crate) fn chat_kill_background_command(
    state: State<AppState>,
    job_id: String,
) -> Result<(), String> {
    crate::native_tools::kill_background(&state, &serde_json::json!({ "job_id": job_id }), None)
        .map(|_| ())
}

/// 响应会话级文件/命令工具授权请求(按 conversation_id)。
#[tauri::command]
pub(crate) fn chat_respond_session_consent(
    app: AppHandle,
    state: State<AppState>,
    conversation_id: String,
    granted: bool,
) -> Result<(), String> {
    let pending = state
        .pending_chat_session_consents
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&conversation_id);
    if let Some(pending) = pending {
        crate::chat::protocol::resolve_session_consent(&app, &pending.run_id);
        let _ = pending.sender.send(granted);
    }
    Ok(())
}

/// 回答 ask_user 澄清卡片。
#[tauri::command]
pub(crate) fn chat_submit_user_choice(
    app: AppHandle,
    state: State<AppState>,
    tool_call_id: String,
    answers: HashMap<String, crate::chat::ask_user::AskUserAnswer>,
    skipped: bool,
) -> Result<(), String> {
    let response = {
        let pending = state
            .pending_chat_user_prompts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(pending) = pending.get(&tool_call_id) else {
            return Err("Clarification is no longer awaiting a response".to_string());
        };
        if skipped {
            crate::chat::ask_user::skipped_response()
        } else {
            crate::chat::ask_user::validate_response(
                &pending.prompt,
                crate::chat::ask_user::AskUserResponseResult {
                    phase: crate::chat::ask_user::ASK_USER_PHASE_ANSWERED.to_string(),
                    answers,
                },
            )?
        }
    };
    let pending = state
        .pending_chat_user_prompts
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&tool_call_id);
    let Some(pending) = pending else {
        return Err("Clarification is no longer awaiting a response".to_string());
    };
    crate::chat::protocol::resolve_user_prompt(&app, &pending.run_id, &tool_call_id);
    let _ = pending.sender.send(response);
    Ok(())
}

/// 运行中「立刻引导」：把用户这条消息注入正在跑的这一轮。
///
/// 两条运行时各走各的通道：
///   - **内置 agent 循环** → 放进 `pending_chat_steering` 信箱，`run_agent_loop` 在下一个轮次
///     边界取走（`chat::agent::steering`）。
///   - **外部 CLI** → 把 `SessionCommand::Steer` 送进常驻会话的 actor，由各协议自己决定：
///     codex 走 `turn/steer`（真注入，不中断）；claude 的 stream-json 输入是**顺序**处理的、
///     没有注入原语，ACP 也没有 —— 那两条会回 false。
///
/// 返回 `false` = 没进去（此刻没有在跑的轮次 / 该协议不支持 / 对端拒绝），前端据此把这条留在
/// 队列里，按普通消息在轮末发出去。注意「进去了」不等于「已生效」：真正生效的信号是那张
/// `user_steer` 卡的事件，前端收到才把队列里那条出队。
#[tauri::command]
pub(crate) async fn chat_steer_message(
    state: State<'_, AppState>,
    conversation_id: String,
    steer_id: String,
    content: String,
    text_attachments: Option<Vec<TextAttachmentInput>>,
) -> Result<bool, String> {
    let content = compose_text_attachments_for_api(&content, &text_attachments.unwrap_or_default());
    let Some(message) = crate::chat::agent::SteeringMessage::new(steer_id, &content) else {
        return Ok(false);
    };
    // 常驻 CLI 会话优先：这条对话由外部 CLI 在跑时，内置信箱根本没人来取。
    if let Some(control) = state.external_live_session_control_any(&conversation_id) {
        let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
        let sent = control
            .send(
                crate::external_agents::session::live::SessionCommand::Steer {
                    id: message.id,
                    text: message.text,
                    accepted: accepted_tx,
                },
            )
            .await
            .is_ok();
        if !sent {
            return Ok(false);
        }
        // actor 每条命令都会答复；真丢了（actor 中途没了）按未受理处理。
        return Ok(accepted_rx.await.unwrap_or(false));
    }
    Ok(state.push_chat_steering(&conversation_id, message))
}

/// 前端 Pyodide 执行完成后回传结果。
#[tauri::command]
pub(crate) fn chat_python_complete(
    app: AppHandle,
    state: State<AppState>,
    run_id: String,
    content: String,
    is_error: bool,
    artifacts: Option<Vec<ChatToolArtifact>>,
) -> Result<(), String> {
    let pending = state
        .pending_python_runs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&run_id);
    if let Some(pending) = pending {
        crate::chat::protocol::detach_python_request(&app, &run_id);
        let _ = pending.sender.send(crate::mcp::types::PythonRunResult {
            content,
            is_error,
            artifacts: artifacts.unwrap_or_default(),
        });
    }
    Ok(())
}

pub(super) fn emit_chat_plan_state(
    app: &AppHandle,
    conversation_id: &str,
    revision: u64,
    plan_state: &AgentPlanState,
) {
    crate::chat::protocol::emit_conversation_event(
        app,
        conversation_id,
        revision,
        crate::chat::protocol::ChatConversationEvent::PlanUpdated {
            plan_state: plan_state.into(),
        },
    );
}

pub(super) async fn request_session_consent(
    app: &AppHandle,
    state: &AppState,
    conversation_id: &str,
    run_id: &str,
    generation: u64,
) -> bool {
    // Already granted for this conversation — no prompt.
    if state.has_chat_consent(conversation_id) {
        return true;
    }
    // Serialize prompts so concurrent first-round tools (read/grep/find/ls run
    // in parallel) don't each insert a pending sender and clobber one another.
    // Whoever wins the lock prompts once; the rest re-check consent and reuse
    // the grant without a second dialog.
    let _prompt_guard = state.chat_consent_prompt_lock.lock().await;
    if state.has_chat_consent(conversation_id) {
        return true;
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let mut pending = state
            .pending_chat_session_consents
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Only one outstanding consent prompt per conversation.
        pending.insert(
            conversation_id.to_string(),
            crate::state::PendingSessionConsent {
                run_id: run_id.to_string(),
                sender: tx,
            },
        );
    }
    crate::chat::protocol::emit_run_event(
        app,
        run_id,
        crate::chat::protocol::ChatRunEvent::SessionConsentRequested,
    );
    let result = tokio::select! {
        result = timeout(Duration::from_secs(60), rx) => result,
        _ = wait_for_chat_cancel(state, conversation_id, generation) => {
            state
                .pending_chat_session_consents
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(conversation_id);
            crate::chat::protocol::resolve_session_consent(app, run_id);
            return false;
        }
    };
    crate::chat::protocol::resolve_session_consent(app, run_id);
    match result {
        Ok(Ok(true)) => {
            state.grant_chat_consent(conversation_id);
            true
        }
        _ => {
            state
                .pending_chat_session_consents
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(conversation_id);
            false
        }
    }
}

pub(crate) async fn request_tool_approval(
    app: &AppHandle,
    state: &AppState,
    conversation_id: &str,
    run_id: &str,
    generation: u64,
    record: &ToolCallRecord,
) -> bool {
    request_tool_approval_outcome(app, state, conversation_id, run_id, generation, record)
        .await
        .approved
}

/// 同 `request_tool_approval`，但把用户选的那一档也带回来（计划批准的三选一）。
pub(crate) async fn request_tool_approval_outcome(
    app: &AppHandle,
    state: &AppState,
    conversation_id: &str,
    run_id: &str,
    generation: u64,
    record: &ToolCallRecord,
) -> crate::state::ToolApprovalOutcome {
    // 用户此前对该工具按过「总是允许」→ 本对话内直接放行，不弹卡、不占挂起表。
    // 内置 agent 与外部 CLI 都走这个函数，所以一处判断两条路同时生效。
    if state.has_tool_always_allow(conversation_id, &record.name) {
        return crate::state::ToolApprovalOutcome {
            approved: true,
            permission_mode: None,
        };
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let mut pending = state
            .pending_chat_tool_approvals
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        pending.insert(
            record.id.clone(),
            crate::state::PendingToolApproval {
                conversation_id: conversation_id.to_string(),
                tool_name: record.name.clone(),
                sender: tx,
            },
        );
    }
    let summary = format_tool_approval_summary(record);
    crate::chat::protocol::emit_run_event(
        app,
        run_id,
        crate::chat::protocol::ChatRunEvent::ToolApprovalRequested {
            tool_call_id: record.id.clone(),
            name: record.name.clone(),
            source: record.source.clone(),
            server_id: record.server_id.clone(),
            target: summary.target,
            arguments_preview: summary.detail,
            sensitivity: "sensitive".to_string(),
        },
    );
    let result = tokio::select! {
        result = timeout(Duration::from_secs(60), rx) => result,
        _ = wait_for_chat_cancel(state, conversation_id, generation) => {
            let mut pending = state
                .pending_chat_tool_approvals
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            pending.remove(&record.id);
            drop(pending);
            withdraw_tool_confirm(app, &record.id);
            return crate::state::ToolApprovalOutcome::default();
        }
    };
    match result {
        Ok(Ok(value)) => value,
        _ => {
            let mut pending = state
                .pending_chat_tool_approvals
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            pending.remove(&record.id);
            drop(pending);
            // 超时/通道断开 ⇒ 这条已经按拒绝处理了。必须把卡片撤掉，否则用户回来点
            // 「允许」是个静默空操作（`chat_confirm_tool_call` 找不到条目就直接 Ok），
            // 他会以为自己批准了，而工具早就被拒了。
            withdraw_tool_confirm(app, &record.id);
            crate::state::ToolApprovalOutcome::default()
        }
    }
}

/// 通知前端撤掉某条审批卡（已超时 / 已取消 / 询问方已经不在了，答复不再有意义）。
pub(crate) fn withdraw_tool_confirm(app: &AppHandle, tool_call_id: &str) {
    crate::chat::protocol::withdraw_tool_approval(app, tool_call_id);
}

pub(crate) async fn request_user_response(
    app: &AppHandle,
    state: &AppState,
    conversation_id: &str,
    run_id: &str,
    generation: u64,
    record: &ToolCallRecord,
    prompt: crate::chat::ask_user::AskUserPromptPayload,
) -> crate::chat::ask_user::AskUserResponseResult {
    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let mut pending = state
            .pending_chat_user_prompts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        pending.insert(
            record.id.clone(),
            crate::chat::ask_user::PendingAskUserPrompt {
                run_id: run_id.to_string(),
                prompt: prompt.clone(),
                sender: tx,
            },
        );
    }

    let empty_answers = HashMap::new();
    let structured_content = crate::chat::ask_user::structured_content(
        &prompt,
        crate::chat::ask_user::ASK_USER_PHASE_AWAITING,
        &empty_answers,
    );
    crate::chat::protocol::emit_run_event(
        app,
        run_id,
        crate::chat::protocol::ChatRunEvent::UserPromptRequested {
            tool_call_id: record.id.clone(),
            name: record.name.clone(),
            source: record.source.clone(),
            prompt: (&prompt).into(),
            structured_content: Some(structured_content),
        },
    );

    let result = tokio::select! {
        result = timeout(Duration::from_secs(600), rx) => result,
        _ = wait_for_chat_cancel(state, conversation_id, generation) => {
            let mut pending = state
                .pending_chat_user_prompts
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            pending.remove(&record.id);
            crate::chat::protocol::resolve_user_prompt(app, run_id, &record.id);
            return crate::chat::ask_user::cancelled_response();
        }
    };
    let response = match result {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => {
            let mut pending = state
                .pending_chat_user_prompts
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            pending.remove(&record.id);
            crate::chat::ask_user::cancelled_response()
        }
        Err(_) => {
            let mut pending = state
                .pending_chat_user_prompts
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            pending.remove(&record.id);
            crate::chat::ask_user::timeout_response()
        }
    };
    crate::chat::protocol::resolve_user_prompt(app, run_id, &record.id);
    response
}

pub(super) async fn wait_for_chat_cancel(state: &AppState, conversation_id: &str, generation: u64) {
    while state.is_chat_generation_active(conversation_id, generation) {
        sleep(Duration::from_millis(100)).await;
    }
}

pub(crate) fn emit_chat_tool_record(app: &AppHandle, run_id: &str, record: &ToolCallRecord) {
    crate::chat::protocol::emit_run_event(
        app,
        run_id,
        crate::chat::protocol::ChatRunEvent::ToolUpdated {
            tool: crate::chat::protocol::ChatToolPayload::from_record(
                record,
                truncate_chars(&record.arguments, 800),
            ),
        },
    );
}

/// 决定一次 `emit_chat_stream_delta` 要发哪几条事件：`(发 reasoning, 发 text)`。
///
/// 空 delta + 有 segment 是「宣告段位置」的专用调用（工具卡槽位、内置搜索卡预留槽、段
/// phase 改写）。`ChatToolPayload` 不带 segment/order，工具事件自己说不清该插在哪儿，这条
/// 空 delta 事件就是唯一的位置载体——丢掉它，工具卡在流式期间根本不渲染，内置搜索卡会掉到
/// 答案末尾。delta 和 segment 都空才是真的没内容，那才一条都不发。
pub(super) fn stream_delta_event_kinds(
    delta: &str,
    reasoning_delta: Option<&str>,
    has_segment: bool,
) -> (bool, bool) {
    let emit_reasoning = reasoning_delta.is_some_and(|value| !value.is_empty() || has_segment);
    let emit_text = !delta.is_empty() || (has_segment && !emit_reasoning);
    (emit_reasoning, emit_text)
}

pub(crate) fn emit_chat_stream_delta(
    app: &AppHandle,
    run_id: &str,
    delta: &str,
    reasoning_delta: Option<&str>,
    segment: Option<&ChatMessageSegment>,
) {
    let segment = segment.map(crate::chat::protocol::ChatSegmentPayload::from);
    let (emit_reasoning, emit_text) =
        stream_delta_event_kinds(delta, reasoning_delta, segment.is_some());
    if emit_reasoning {
        crate::chat::protocol::emit_run_event(
            app,
            run_id,
            crate::chat::protocol::ChatRunEvent::ReasoningDelta {
                delta: reasoning_delta.unwrap_or_default().to_string(),
                segment: segment.clone(),
            },
        );
    }
    if emit_text {
        crate::chat::protocol::emit_run_event(
            app,
            run_id,
            crate::chat::protocol::ChatRunEvent::TextDelta {
                delta: delta.to_string(),
                segment,
            },
        );
    }
}

/// 审批卡要展示的两样东西：`target` 是这次操作的对象（文件路径 / 命令），用来在前端拼
/// 「允许写入 xxx.md？」这种自然语言标题；`detail` 是代码块正文。
pub(super) struct ToolApprovalSummary {
    pub target: Option<String>,
    pub detail: String,
}

pub(super) fn format_tool_approval_summary(record: &ToolCallRecord) -> ToolApprovalSummary {
    let parsed = serde_json::from_str::<Value>(&record.arguments).ok();
    let field = |names: &[&str]| -> Option<String> {
        names.iter().find_map(|name| {
            parsed
                .as_ref()
                .and_then(|value| value.get(*name))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
    };
    let mut target = None;
    let mut lines = Vec::new();
    // 工具名归一化：内置 agent 用小写 snake_case（`bash` / `write`），而**外部 CLI 报的是
    // 自己的原名**（claude 的 `Bash` / `Write` / `Edit` / `Read` 是 PascalCase）。不归一化
    // 的话外部 CLI 的审批卡永远落进 `_` 分支、只剩一坨截断的 JSON（与 spec 第 23 条前端
    // 工具卡踩过的是同一个坑）。字段名同理：claude 用 `file_path`，我们用 `path`。
    match record.name.to_ascii_lowercase().as_str() {
        "bash" | "run_command" => {
            if let Some(command) = field(&["command"]) {
                target = Some(truncate_chars(
                    command.lines().next().unwrap_or(&command),
                    120,
                ));
                lines.push(command);
            }
            if let Some(cwd) = field(&["cwd", "working_directory"]) {
                lines.push(format!("Working directory: {cwd}"));
            }
        }
        "write" | "edit" | "read" | "write_file" | "edit_file" | "read_file" | "notebookedit" => {
            if let Some(path) = field(&["path", "file_path", "notebook_path"]) {
                target = Some(path.clone());
                lines.push(path);
            }
            if record.name.eq_ignore_ascii_case("edit")
                || record.name.eq_ignore_ascii_case("edit_file")
            {
                // Current shape: edits: [{old_string, new_string}, ...]. Preview the
                // first edit's old_string; fall back to the legacy single-edit field.
                let first_old = parsed
                    .as_ref()
                    .and_then(|value| value.get("edits"))
                    .and_then(|value| value.as_array())
                    .and_then(|edits| edits.first())
                    .and_then(|edit| edit.get("old_string"))
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .or_else(|| field(&["old_string", "old"]));
                if let Some(old) = first_old {
                    lines.push(format!("Replace: {}", truncate_chars(&old, 180)));
                }
            }
        }
        // claude 的「计划写完了，批准我照着做」。卡片上要看到的是**计划正文**本身
        // —— 用户批的是这份计划，不是一个工具名。
        //
        // 留得比别的分支长得多（20k 字符）有两个理由：卡片上那块灰框自己会滚
        // （`max-h-40 overflow-auto`），而**右侧栏会拿同一份文本渲染整份计划**
        // （`requestDockMarkdownPreview`）—— 在这里截短就等于右边也读不全。
        "exitplanmode" => {
            if let Some(plan) = field(&["plan"]) {
                lines.push(truncate_chars(&plan, 20_000));
            }
        }
        // claude 自己要求进入计划档。这个工具**没有入参**，标题已经把事情说完了；
        // 不给 detail，否则会退到下面的裸 JSON 分支、卡片上蹲一个 `{}`。
        "enterplanmode" => {
            return ToolApprovalSummary {
                target: None,
                detail: String::new(),
            };
        }
        _ => {}
    }

    // ponytail: 只有认不出操作对象时才退回裸 JSON。认出来了还把 `Raw arguments:` 追在后面，
    // 卡片就退化成一坨截断的 JSON（旧版本正是如此），用户看不出自己在批准什么。
    if lines.is_empty() {
        ToolApprovalSummary {
            target: None,
            detail: truncate_chars(&record.arguments, 800),
        }
    } else {
        ToolApprovalSummary {
            target,
            detail: lines.join("\n"),
        }
    }
}
