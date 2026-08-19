use std::sync::{atomic::AtomicBool, Arc};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, State};
use tokio::sync::{mpsc, oneshot};

use crate::external_agents::defs::pi::PI_AGENT_DEF;
use crate::external_agents::session::live::{
    LaunchConfig, LiveSession, PiSessionRequest, PiSessionRpcResult, SessionCommand,
};
use crate::external_agents::session::{
    find_live_binding_by_native_path, load_live_handle, load_session, save_live_handle,
    save_session, update_stored_session_id, LiveSessionHandle,
};
use crate::external_agents::types::{ExternalAgentSession, RuntimeBuildOptions, RuntimeContext};
use crate::state::AppState;

const PI_SESSION_REPLY_TIMEOUT: Duration = Duration::from_secs(18);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiSessionTreeSnapshot {
    pub tree: Value,
    pub leaf_id: Option<String>,
    pub session_id: String,
    pub session_file: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiSessionMutationResult {
    pub cancelled: bool,
    pub text: Option<String>,
    pub session_id: String,
    pub session_file: Option<String>,
    pub previous_session_id: String,
    pub previous_session_file: Option<String>,
    pub conversation_id: Option<String>,
    pub conversation: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiSessionSwitchResult {
    pub conversation_id: String,
}

async fn ensure_pi_control(
    app: &AppHandle,
    state: &AppState,
    conversation_id: &str,
) -> Result<mpsc::Sender<SessionCommand>, String> {
    if let Some(control) = state.external_pi_live_session_control(conversation_id) {
        return Ok(control);
    }
    let conversation = crate::chat::storage::load_conversation(app, conversation_id)?;
    if !conversation.agent_runtime.is_external()
        || conversation.agent_runtime.external_agent_id.as_deref() != Some("pi")
    {
        return Err("当前对话未绑定 Pi 外部 CLI".to_string());
    }

    let stored = load_session(app, conversation_id).filter(|session| session.agent_id == "pi");
    let handle = load_live_handle(app, conversation_id)
        .filter(|handle| handle.agent_id == "pi" && handle.protocol == "pi_rpc");
    let native_id = handle
        .as_ref()
        .map(|handle| handle.native_id.clone())
        .or_else(|| stored.as_ref().map(|session| session.session_id.clone()))
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| "这条对话还没有 Pi 原生会话；请先发送一条消息".to_string())?;
    let cwd = if let Some(handle) = handle
        .as_ref()
        .filter(|handle| !handle.cwd.trim().is_empty())
    {
        std::path::PathBuf::from(&handle.cwd)
    } else {
        crate::external_agents::workspace::ensure_effective_cwd(
            app,
            conversation_id,
            conversation.project_id.as_deref(),
        )?
    };
    if !cwd.is_dir() {
        return Err(format!("Pi 会话工作目录不存在：{}", cwd.display()));
    }

    let bin = crate::external_agents::spawn::resolve_binary(&PI_AGENT_DEF)
        .await
        .ok_or_else(|| "未找到可用的 Pi CLI".to_string())?;
    let runtime = RuntimeContext {
        extra_allowed_dirs: Vec::new(),
        resume_session_id: Some(native_id.clone()),
        new_session_id: None,
        include_partial_messages: false,
    };
    let model = conversation.agent_runtime.external_model.clone();
    let reasoning = conversation.agent_runtime.external_reasoning.clone();
    let options = RuntimeBuildOptions {
        model: model.clone(),
        reasoning: reasoning.clone(),
        sandbox: None,
    };
    let args = (PI_AGENT_DEF.build_args)(&runtime, &options, None);
    let session = crate::external_agents::session::pi_rpc::PiRpcSession::connect(
        &bin,
        &args,
        &cwd,
        Some(&native_id),
    )
    .await?;
    let child_pid = session.child_pid();
    let actual_id = session.session_id().to_string();
    let control = crate::external_agents::session::pi_rpc::spawn_pi_rpc_session_actor(session);
    let cwd_string = cwd.to_string_lossy().to_string();
    save_live_handle(
        app,
        conversation_id,
        &LiveSessionHandle {
            agent_id: "pi".to_string(),
            protocol: "pi_rpc".to_string(),
            native_id: actual_id,
            native_path: handle.and_then(|handle| handle.native_path),
            cwd: cwd_string.clone(),
        },
    )?;
    state.register_external_live_session(
        conversation_id.to_string(),
        LiveSession {
            control: control.clone(),
            agent_id: "pi".to_string(),
            cwd: cwd_string,
            launch_config: LaunchConfig::for_pi(model.as_deref(), reasoning.as_deref()),
            last_activity: Instant::now(),
            child_pid,
            turns_served: 0,
            busy: Arc::new(AtomicBool::new(false)),
        },
    );
    Ok(control)
}

async fn call_pi_session_locked(
    app: &AppHandle,
    state: &AppState,
    conversation_id: &str,
    request: PiSessionRequest,
) -> Result<(PiSessionRpcResult, mpsc::Sender<SessionCommand>), String> {
    let control = ensure_pi_control(app, state, conversation_id).await?;
    let _busy = state.try_mark_external_live_session_busy(conversation_id)?;
    let (reply_tx, reply_rx) = oneshot::channel();
    control
        .send(SessionCommand::PiSession {
            request,
            reply: reply_tx,
        })
        .await
        .map_err(|_| "Pi session actor is unavailable".to_string())?;
    let result = match tokio::time::timeout(PI_SESSION_REPLY_TIMEOUT, reply_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("Pi session actor closed before replying".to_string()),
        Err(_) => {
            state.remove_external_live_session(conversation_id);
            let _ = control.send(SessionCommand::Close).await;
            Err("Pi session command timed out; the live session was closed".to_string())
        }
    }?;
    Ok((result, control))
}

async fn call_pi_session(
    app: &AppHandle,
    state: &AppState,
    conversation_id: &str,
    request: PiSessionRequest,
) -> Result<PiSessionRpcResult, String> {
    let operation_lock = state.pi_session_control_lock_for(conversation_id);
    let _operation = operation_lock.lock().await;
    call_pi_session_locked(app, state, conversation_id, request)
        .await
        .map(|(result, _)| result)
}

fn state_identity(state: &Value) -> Result<(String, Option<String>), String> {
    let session_id = state
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Pi get_state did not return sessionId".to_string())?
        .to_string();
    let session_file = state
        .get("sessionFile")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Ok((session_id, session_file))
}

fn persist_observed_identity(
    app: &AppHandle,
    conversation_id: &str,
    state: &Value,
) -> Result<(String, Option<String>), String> {
    let (session_id, session_file) = state_identity(state)?;
    let mut handle = load_live_handle(app, conversation_id)
        .ok_or_else(|| "Pi live session handle disappeared".to_string())?;
    handle.native_id = session_id.clone();
    handle.native_path = session_file.clone();
    save_live_handle(app, conversation_id, &handle)?;
    update_stored_session_id(app, conversation_id, "pi", &session_id)?;
    Ok((session_id, session_file))
}

fn close_mutated_actor(
    state: &AppState,
    source_conversation_id: &str,
    control: &mpsc::Sender<SessionCommand>,
) {
    state.remove_external_live_session(source_conversation_id);
    let _ = control.try_send(SessionCommand::Close);
}

async fn resolve_fork_anchor(
    app: &AppHandle,
    state: &AppState,
    conversation_id: &str,
    entry_id: &str,
) -> Result<(String, String), String> {
    let (result, _) = call_pi_session_locked(
        app,
        state,
        conversation_id,
        PiSessionRequest::GetForkMessages,
    )
    .await?;
    let messages = result
        .data
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "Pi get_fork_messages returned no messages".to_string())?;
    let conversation = crate::chat::storage::load_conversation(app, conversation_id)?;
    let user_messages: Vec<(&str, &str)> = conversation
        .messages
        .iter()
        .filter(|message| message.role == "user")
        .map(|message| (message.id.as_str(), message.content.as_str()))
        .collect();
    kivio_anchor_for_fork_entry(messages, entry_id, &user_messages)
}

fn kivio_anchor_for_fork_entry(
    pi_messages: &[Value],
    entry_id: &str,
    user_messages: &[(&str, &str)],
) -> Result<(String, String), String> {
    let selected_index = pi_messages
        .iter()
        .position(|message| message.get("entryId").and_then(Value::as_str) == Some(entry_id))
        .ok_or_else(|| "Pi fork entry is no longer available".to_string())?;
    let selected_text = pi_messages[selected_index]
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let occurrence = pi_messages[..selected_index]
        .iter()
        .filter(|message| {
            message
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                == selected_text
        })
        .count();
    user_messages
        .iter()
        .copied()
        .filter(|(_, content)| content.trim() == selected_text)
        .nth(occurrence)
        .map(|(id, _)| (id.to_string(), selected_text.to_string()))
        .ok_or_else(|| "无法把这个 Pi 节点可靠映射到 Kivio 消息；源对话未被修改".to_string())
}

fn clone_anchor(app: &AppHandle, conversation_id: &str) -> Result<String, String> {
    crate::chat::storage::load_conversation(app, conversation_id)?
        .messages
        .last()
        .map(|message| message.id.clone())
        .ok_or_else(|| "空对话不能克隆 Pi 分支".to_string())
}

fn mutation_identity(result: &PiSessionRpcResult) -> Result<(String, Option<String>), String> {
    result
        .state
        .as_ref()
        .ok_or_else(|| "Pi session mutation returned no state".to_string())
        .and_then(state_identity)
}

async fn create_destination_conversation(
    app: &AppHandle,
    source_conversation_id: &str,
    anchor_message_id: &str,
    exclude_anchor: bool,
) -> Result<(String, Value), String> {
    let result = crate::chat::commands::mutations::chat_fork_conversation(
        app.clone(),
        source_conversation_id.to_string(),
        anchor_message_id.to_string(),
        Some(exclude_anchor),
    )
    .await?;
    let conversation = result
        .get("conversation")
        .cloned()
        .ok_or_else(|| "Kivio fork did not return a conversation".to_string())?;
    let conversation_id = conversation
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "Kivio fork did not return a conversation id".to_string())?
        .to_string();
    Ok((conversation_id, conversation))
}

async fn cleanup_destination(app: &AppHandle, conversation_id: &str) {
    let _ = crate::chat::repository::repository(app)
        .delete(app, conversation_id)
        .await;
    let _ = crate::external_agents::session::remove_all_bindings(app, conversation_id);
}

async fn mutate_to_new_conversation(
    app: &AppHandle,
    state: &AppState,
    source_conversation_id: &str,
    anchor_message_id: &str,
    exclude_anchor: bool,
    draft: Option<String>,
    request: PiSessionRequest,
) -> Result<PiSessionMutationResult, String> {
    let (result, control) =
        match call_pi_session_locked(app, state, source_conversation_id, request).await {
            Ok(result) => result,
            Err(error) if error.contains("session is busy") => return Err(error),
            Err(error) => {
                if let Some(control) =
                    state.external_pi_live_session_control(source_conversation_id)
                {
                    close_mutated_actor(state, source_conversation_id, &control);
                }
                return Err(error);
            }
        };
    let previous = load_live_handle(app, source_conversation_id)
        .filter(|handle| handle.agent_id == "pi")
        .ok_or_else(|| "Pi live session handle is missing".to_string())?;
    let cancelled = result.data.get("cancelled").and_then(Value::as_bool) == Some(true);
    let text = result
        .data
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_string);
    if cancelled {
        return Ok(PiSessionMutationResult {
            cancelled: true,
            text,
            session_id: previous.native_id.clone(),
            session_file: previous.native_path.clone(),
            previous_session_id: previous.native_id,
            previous_session_file: previous.native_path,
            conversation_id: None,
            conversation: None,
        });
    }
    let (session_id, session_file) = match mutation_identity(&result) {
        Ok(identity) => identity,
        Err(error) => {
            close_mutated_actor(state, source_conversation_id, &control);
            return Err(error);
        }
    };
    let (destination_id, conversation) = match create_destination_conversation(
        app,
        source_conversation_id,
        anchor_message_id,
        exclude_anchor,
    )
    .await
    {
        Ok(destination) => destination,
        Err(error) => {
            close_mutated_actor(state, source_conversation_id, &control);
            return Err(format!("Pi created the native branch, but Kivio could not create its conversation: {error}"));
        }
    };

    let destination_handle = LiveSessionHandle {
        agent_id: "pi".to_string(),
        protocol: "pi_rpc".to_string(),
        native_id: session_id.clone(),
        native_path: session_file.clone(),
        cwd: previous.cwd.clone(),
    };
    let mut destination_session =
        load_session(app, source_conversation_id).unwrap_or(ExternalAgentSession {
            conversation_id: destination_id.clone(),
            agent_id: "pi".to_string(),
            session_id: session_id.clone(),
            stable_prompt_hash: None,
            model: None,
        });
    destination_session.conversation_id = destination_id.clone();
    destination_session.session_id = session_id.clone();
    if let Err(error) = save_live_handle(app, &destination_id, &destination_handle)
        .and_then(|_| save_session(app, &destination_session))
        .and_then(|_| state.move_external_live_session(source_conversation_id, &destination_id))
    {
        close_mutated_actor(state, source_conversation_id, &control);
        cleanup_destination(app, &destination_id).await;
        return Err(format!("Pi branch binding failed: {error}"));
    }

    Ok(PiSessionMutationResult {
        cancelled: false,
        text: draft.or(text),
        session_id,
        session_file,
        previous_session_id: previous.native_id,
        previous_session_file: previous.native_path,
        conversation_id: Some(destination_id),
        conversation: Some(conversation),
    })
}

#[tauri::command]
pub async fn chat_pi_session_tree(
    app: AppHandle,
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<PiSessionTreeSnapshot, String> {
    let result = call_pi_session(&app, &state, &conversation_id, PiSessionRequest::GetTree).await?;
    let rpc_state = result
        .state
        .as_ref()
        .ok_or_else(|| "Pi get_tree returned no state".to_string())?;
    let (session_id, session_file) = persist_observed_identity(&app, &conversation_id, rpc_state)?;
    Ok(PiSessionTreeSnapshot {
        tree: result
            .data
            .get("tree")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
        leaf_id: result
            .data
            .get("leafId")
            .and_then(Value::as_str)
            .map(str::to_string),
        session_id,
        session_file,
    })
}

#[tauri::command]
pub async fn chat_pi_session_entries(
    app: AppHandle,
    state: State<'_, AppState>,
    conversation_id: String,
    since: Option<String>,
) -> Result<Value, String> {
    Ok(call_pi_session(
        &app,
        &state,
        &conversation_id,
        PiSessionRequest::GetEntries { since },
    )
    .await?
    .data)
}

#[tauri::command]
pub async fn chat_pi_fork_messages(
    app: AppHandle,
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<Value, String> {
    Ok(call_pi_session(
        &app,
        &state,
        &conversation_id,
        PiSessionRequest::GetForkMessages,
    )
    .await?
    .data)
}

#[tauri::command]
pub async fn chat_pi_session_fork(
    app: AppHandle,
    state: State<'_, AppState>,
    conversation_id: String,
    entry_id: String,
) -> Result<PiSessionMutationResult, String> {
    let entry_id = entry_id.trim();
    if entry_id.is_empty() {
        return Err("Pi fork requires an entry id".to_string());
    }
    let operation_lock = state.pi_session_control_lock_for(&conversation_id);
    let _operation = operation_lock.lock().await;
    let (anchor, draft) = resolve_fork_anchor(&app, &state, &conversation_id, entry_id).await?;
    mutate_to_new_conversation(
        &app,
        &state,
        &conversation_id,
        &anchor,
        true,
        Some(draft),
        PiSessionRequest::Fork {
            entry_id: entry_id.to_string(),
        },
    )
    .await
}

#[tauri::command]
pub async fn chat_pi_session_clone(
    app: AppHandle,
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<PiSessionMutationResult, String> {
    let operation_lock = state.pi_session_control_lock_for(&conversation_id);
    let _operation = operation_lock.lock().await;
    let anchor = clone_anchor(&app, &conversation_id)?;
    mutate_to_new_conversation(
        &app,
        &state,
        &conversation_id,
        &anchor,
        false,
        None,
        PiSessionRequest::Clone,
    )
    .await
}

#[tauri::command]
pub async fn chat_pi_session_switch(
    app: AppHandle,
    conversation_id: String,
    session_path: String,
) -> Result<PiSessionSwitchResult, String> {
    let path = std::path::Path::new(session_path.trim());
    if !path.is_file() {
        return Err("Pi session file does not exist".to_string());
    }
    let (bound_conversation_id, handle) = find_live_binding_by_native_path(&app, path)
        .ok_or_else(|| "只能导航到已由 Kivio 创建或绑定的 Pi session".to_string())?;
    if handle.agent_id != "pi" || handle.protocol != "pi_rpc" {
        return Err("目标文件不是已绑定的 Pi RPC 会话".to_string());
    }
    if bound_conversation_id == conversation_id {
        return Ok(PiSessionSwitchResult {
            conversation_id: bound_conversation_id,
        });
    }
    let target = crate::chat::storage::load_conversation(&app, &bound_conversation_id)?;
    if !target.agent_runtime.is_external()
        || target.agent_runtime.external_agent_id.as_deref() != Some("pi")
    {
        return Err("目标 Kivio 对话不再绑定 Pi".to_string());
    }
    Ok(PiSessionSwitchResult {
        conversation_id: bound_conversation_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(entry_id: &str, text: &str) -> Value {
        json!({ "entryId": entry_id, "text": text })
    }

    #[test]
    fn duplicate_user_prompts_map_to_the_matching_occurrence() {
        let pi = [msg("e1", "Same prompt"), msg("e2", "Same prompt")];
        let users = [("u1", "Same prompt"), ("u2", "Same prompt")];
        assert_eq!(
            kivio_anchor_for_fork_entry(&pi, "e1", &users).unwrap(),
            ("u1".to_string(), "Same prompt".to_string())
        );
        assert_eq!(
            kivio_anchor_for_fork_entry(&pi, "e2", &users).unwrap(),
            ("u2".to_string(), "Same prompt".to_string())
        );
    }

    #[test]
    fn extra_kivio_user_message_still_maps_by_occurrence() {
        let pi = [msg("e1", "hello"), msg("e2", "hello")];
        let users = [
            ("u0", "hello"),
            ("u-other", "something else"),
            ("u1", "hello"),
        ];
        assert_eq!(
            kivio_anchor_for_fork_entry(&pi, "e2", &users).unwrap(),
            ("u1".to_string(), "hello".to_string())
        );
    }

    #[test]
    fn extra_pi_unmatched_turn_maps_second_duplicate_to_last_match() {
        let pi = [
            msg("e1", "A"),
            msg("e-extra", "side"),
            msg("e3", "A"),
        ];
        let users = [("u1", "A"), ("u2", "A"), ("u3", "A")];
        assert_eq!(
            kivio_anchor_for_fork_entry(&pi, "e3", &users).unwrap(),
            ("u2".to_string(), "A".to_string())
        );
    }

    #[test]
    fn missing_entry_is_an_error() {
        let pi = [msg("e1", "hello")];
        let users = [("u1", "hello")];
        assert!(kivio_anchor_for_fork_entry(&pi, "missing", &users).is_err());
    }
}
