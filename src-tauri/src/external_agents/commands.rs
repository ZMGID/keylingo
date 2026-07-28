use tauri::AppHandle;

use crate::chat::storage::{load_conversation, save_conversation};
use crate::chat::types::AgentRuntimeConfig;
use crate::external_agents::detection::{
    detect_agent_models, detect_availability_all, AVAILABILITY_CACHE_KEY, AVAILABILITY_CACHE_TTL,
    EXTERNAL_AGENT_MODELS_CACHE_TTL, EXTERNAL_AGENT_MODELS_FALLBACK_TTL,
};
use crate::external_agents::registry::get_agent_def;
use crate::external_agents::slash::{cache_key, list_external_cli_slash_commands};
use crate::external_agents::types::{CachedAgentModels, ModelSource};
use crate::external_agents::workspace::resolve_detection_cwd;
use crate::state::AppState;

#[tauri::command]
pub async fn chat_detect_external_agents(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    force_refresh: Option<bool>,
    conversation_id: Option<String>,
) -> Result<serde_json::Value, String> {
    let _ = (&app, &conversation_id); // 可用性与 cwd 无关；参数保留为兼容前端签名。
    let force = force_refresh.unwrap_or(false);
    if !force {
        if let Some(agents) =
            state.get_cached_detected_agents(AVAILABILITY_CACHE_KEY, AVAILABILITY_CACHE_TTL)
        {
            return Ok(serde_json::json!({
                "success": true,
                "agents": agents,
                "cached": true,
            }));
        }
    }

    // single-flight：并发调用只实跑一次；后到者持锁后复查缓存即命中。
    let _guard = state.availability_probe_lock.lock().await;
    if !force {
        if let Some(agents) =
            state.get_cached_detected_agents(AVAILABILITY_CACHE_KEY, AVAILABILITY_CACHE_TTL)
        {
            return Ok(serde_json::json!({
                "success": true,
                "agents": agents,
                "cached": true,
            }));
        }
    }
    let agents = detect_availability_all().await;
    state.set_cached_detected_agents(AVAILABILITY_CACHE_KEY.to_string(), agents.clone());
    Ok(serde_json::json!({
        "success": true,
        "agents": agents,
        "cached": false,
    }))
}

/// 懒查：只探一个指定 agent 的模型（cwd-scoped），single-flight + 缓存。前端在选中该 agent /
/// 打开其模型下拉时调用，避免列表阶段对所有 CLI 跑昂贵的模型探测（claude 达 25s）。
#[tauri::command]
pub async fn chat_detect_external_agent_models(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    agent_id: String,
    conversation_id: Option<String>,
    force: Option<bool>,
) -> Result<serde_json::Value, String> {
    let force = force.unwrap_or(false);
    let def = get_agent_def(&agent_id).ok_or_else(|| format!("未知外部 Agent: {agent_id}"))?;
    let cwd = resolve_detection_cwd(&app, conversation_id.as_deref())?;
    let cwd_key = cwd.to_string_lossy().into_owned();
    let key = cache_key(&agent_id, &cwd_key);

    if !force {
        if let Some(cached) = state.get_cached_external_agent_models(
            &key,
            EXTERNAL_AGENT_MODELS_CACHE_TTL,
            EXTERNAL_AGENT_MODELS_FALLBACK_TTL,
        ) {
            return Ok(cached_models_payload(def, &cached, true));
        }
    }

    let lock = state.model_probe_lock_for(&key);
    let _guard = lock.lock().await;
    if !force {
        if let Some(cached) = state.get_cached_external_agent_models(
            &key,
            EXTERNAL_AGENT_MODELS_CACHE_TTL,
            EXTERNAL_AGENT_MODELS_FALLBACK_TTL,
        ) {
            return Ok(cached_models_payload(def, &cached, true));
        }
    }
    let probe = detect_agent_models(def, &cwd).await;
    if !probe.models.is_empty() {
        // probed 长 TTL，fallback 短 TTL 负缓存——由 get 侧按 source 分别裁定过期。
        // reasoning_options 必须一并写入：ACP/kimi 档位只来自探测，def 静态表为空。
        state.set_cached_external_agent_models(
            key,
            CachedAgentModels {
                models: probe.models.clone(),
                source: probe.source,
                reasoning_options: probe.reasoning_options.clone(),
                reasoning_by_model: probe.reasoning_by_model.clone(),
                current_model: probe.current_model.clone(),
                current_reasoning: probe.current_reasoning.clone(),
            },
        );
    }
    Ok(models_payload(
        &probe.models,
        &probe.reasoning_options,
        &probe.reasoning_by_model,
        probe.source,
        probe.current_model.as_deref(),
        probe.current_reasoning.as_deref(),
        false,
        probe.probe_error.as_deref(),
    ))
}

fn models_payload(
    models: &[crate::external_agents::types::RuntimeModelOption],
    reasoning_options: &[crate::external_agents::types::RuntimeModelOption],
    reasoning_by_model: &std::collections::HashMap<
        String,
        Vec<crate::external_agents::types::RuntimeModelOption>,
    >,
    source: ModelSource,
    current_model: Option<&str>,
    current_reasoning: Option<&str>,
    cached_flag: bool,
    probe_error: Option<&str>,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "success": true,
        "models": models,
        "reasoningOptions": reasoning_options,
        "reasoningByModel": reasoning_by_model,
        "source": source.as_str(),
        "cached": cached_flag,
    });
    if let Some(model) = current_model {
        payload["currentModel"] = serde_json::Value::String(model.to_string());
    }
    if let Some(reasoning) = current_reasoning {
        payload["currentReasoning"] = serde_json::Value::String(reasoning.to_string());
    }
    if source == ModelSource::Fallback {
        if let Some(err) = probe_error {
            payload["probeError"] = serde_json::Value::String(err.to_string());
        }
    }
    payload
}

/// 组装缓存命中的返回 JSON：模型 + 缓存的 reasoning 选项 + 来源 + CLI 当前配置。
/// 缓存里档位为空时回落 def 静态表（claude/codex/pi/grok 有表；ACP 系仍为空）。
fn cached_models_payload(
    def: &crate::external_agents::types::RuntimeAgentDef,
    cached: &CachedAgentModels,
    cached_flag: bool,
) -> serde_json::Value {
    let reasoning_options = if cached.reasoning_options.is_empty() {
        crate::external_agents::types::reasoning_options_from_pairs(def.reasoning_options)
    } else {
        cached.reasoning_options.clone()
    };
    models_payload(
        &cached.models,
        &reasoning_options,
        &cached.reasoning_by_model,
        cached.source,
        cached.current_model.as_deref(),
        cached.current_reasoning.as_deref(),
        cached_flag,
        None,
    )
}

#[tauri::command]
pub async fn chat_list_external_cli_slash_commands(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    agent_id: String,
    conversation_id: Option<String>,
) -> Result<serde_json::Value, String> {
    let (supports, commands, message) =
        list_external_cli_slash_commands(&app, &state, &agent_id, conversation_id.as_deref())
            .await?;
    Ok(serde_json::json!({
        "success": true,
        "supportsSlashCommands": supports,
        "commands": commands,
        "message": message,
    }))
}

#[tauri::command]
pub fn chat_set_agent_runtime(
    app: AppHandle,
    conversation_id: String,
    agent_runtime: AgentRuntimeConfig,
) -> Result<serde_json::Value, String> {
    let mut conversation = load_conversation(&app, &conversation_id)?;

    // Session ↔ CLI binding (R3): once an external CLI has produced a message, its native session
    // is bound to this conversation — switching CLI or dropping back to the builtin loop would
    // orphan that session's history. Reject a runtime *source* change (kind / external_agent_id)
    // on a non-empty conversation whose current runtime is external; model / reasoning / sandbox
    // tweaks stay allowed. Builtin conversations are never locked (multi-model switching is fine).
    check_runtime_switch_allowed(
        &conversation.agent_runtime,
        conversation.messages.is_empty(),
        &agent_runtime,
    )?;

    conversation.agent_runtime = agent_runtime;
    conversation.updated_at = chrono::Local::now().timestamp();
    save_conversation(&app, &conversation)?;
    Ok(serde_json::json!({
        "success": true,
        "conversation": conversation,
    }))
}

/// Pure binding rule for `chat_set_agent_runtime` (extracted so it is unit-testable without a Tauri
/// `AppHandle`). Returns `Err` with a user-facing message when the switch is forbidden.
fn check_runtime_switch_allowed(
    current: &AgentRuntimeConfig,
    messages_is_empty: bool,
    next: &AgentRuntimeConfig,
) -> Result<(), String> {
    if !current.is_external() || messages_is_empty {
        return Ok(());
    }
    let normalize_id = |id: &Option<String>| {
        id.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let kind_changed = current.kind != next.kind;
    let id_changed =
        normalize_id(&current.external_agent_id) != normalize_id(&next.external_agent_id);
    if kind_changed || id_changed {
        let bound_name = current
            .external_agent_id
            .as_deref()
            .and_then(get_agent_def)
            .map(|d| d.name.to_string())
            .or_else(|| normalize_id(&current.external_agent_id))
            .unwrap_or_else(|| "当前 CLI".to_string());
        return Err(format!("会话已绑定 {bound_name}，新建会话可切换 CLI"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::types::AgentRuntimeKind;

    fn external(id: &str, model: &str) -> AgentRuntimeConfig {
        AgentRuntimeConfig {
            kind: AgentRuntimeKind::External,
            external_agent_id: Some(id.to_string()),
            external_model: Some(model.to_string()),
            external_reasoning: None,
            external_sandbox: None,
        }
    }

    #[test]
    fn empty_conversation_allows_any_switch() {
        let current = external("claude", "default");
        let next = AgentRuntimeConfig::default(); // builtin
        assert!(check_runtime_switch_allowed(&current, true, &next).is_ok());
        let next2 = external("codex", "default");
        assert!(check_runtime_switch_allowed(&current, true, &next2).is_ok());
    }

    #[test]
    fn non_empty_external_rejects_agent_and_kind_change() {
        let current = external("claude", "default");
        // Switch to a different CLI.
        let to_other = external("codex", "default");
        assert!(check_runtime_switch_allowed(&current, false, &to_other).is_err());
        // Switch back to builtin.
        let to_builtin = AgentRuntimeConfig::default();
        assert!(check_runtime_switch_allowed(&current, false, &to_builtin).is_err());
    }

    #[test]
    fn non_empty_external_allows_model_and_reasoning_change() {
        let current = external("claude", "default");
        let same_agent_new_model = external("claude", "sonnet");
        assert!(check_runtime_switch_allowed(&current, false, &same_agent_new_model).is_ok());
        let mut with_reasoning = external("claude", "default");
        with_reasoning.external_reasoning = Some("high".to_string());
        assert!(check_runtime_switch_allowed(&current, false, &with_reasoning).is_ok());
    }

    #[test]
    fn non_empty_builtin_is_never_locked() {
        let current = AgentRuntimeConfig::default(); // builtin
        let to_external = external("claude", "default");
        assert!(check_runtime_switch_allowed(&current, false, &to_external).is_ok());
    }

    /// 回归：缓存命中必须带回探测到的 reasoning_options。
    /// kimi 等 ACP CLI 的 def.reasoning_options 是空的；若命中时只回落 def 表，
    /// effort 胶囊在第二次打开时消失（bac8f53 漏的那步）。
    #[test]
    fn cached_payload_preserves_probed_reasoning_options() {
        use crate::external_agents::registry::get_agent_def;
        use crate::external_agents::types::{CachedAgentModels, ModelSource, RuntimeModelOption};

        let def = get_agent_def("kimi").expect("kimi def");
        assert!(
            def.reasoning_options.is_empty(),
            "precondition: acp_def 静态档位表为空"
        );

        let cached = CachedAgentModels {
            models: vec![RuntimeModelOption {
                id: "kimi-code/kimi-for-coding".into(),
                label: "K2.7 Coding".into(),
                context_window_tokens: None,
            }],
            source: ModelSource::Probed,
            reasoning_options: vec![
                RuntimeModelOption {
                    id: "low".into(),
                    label: "Low".into(),
                    context_window_tokens: None,
                },
                RuntimeModelOption {
                    id: "high".into(),
                    label: "High".into(),
                    context_window_tokens: None,
                },
                RuntimeModelOption {
                    id: "max".into(),
                    label: "Max".into(),
                    context_window_tokens: None,
                },
            ],
            reasoning_by_model: Default::default(),
            current_model: Some("kimi-code/kimi-for-coding".into()),
            current_reasoning: Some("high".into()),
        };

        let payload = cached_models_payload(def, &cached, true);
        let opts = payload["reasoningOptions"]
            .as_array()
            .expect("reasoningOptions array");
        assert_eq!(opts.len(), 3, "缓存命中应保留探测档位，不能回落空 def 表");
        assert_eq!(opts[0]["id"], "low");
        assert_eq!(opts[2]["id"], "max");
        assert_eq!(payload["currentReasoning"], "high");
    }
}
