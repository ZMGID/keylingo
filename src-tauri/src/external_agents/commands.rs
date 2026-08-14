use tauri::{AppHandle, Emitter, Manager};

use crate::chat::types::AgentRuntimeConfig;
use crate::external_agents::detection::{
    detect_agent_models, detect_availability_all, AVAILABILITY_CACHE_KEY, AVAILABILITY_CACHE_TTL,
    EXTERNAL_AGENT_MODELS_CACHE_TTL, EXTERNAL_AGENT_MODELS_FALLBACK_TTL,
};
use crate::external_agents::registry::get_agent_def;
use crate::external_agents::slash::{cache_key, list_external_cli_slash_commands};
use crate::external_agents::types::{CachedAgentModels, DetectedAgent, ModelSource};
use crate::external_agents::workspace::resolve_detection_cwd;
use crate::state::AppState;

/// 上次探测到的可用性快照落盘位置。内存缓存只活一个进程，重启后第一次打开设置页 / 运行时
/// 选择器要干等一轮全量探测（9 个 CLI × version+auth 子进程）才有内容——快照就是为了让那一
/// 眼先有东西看。
fn availability_snapshot_path(state: &AppState) -> std::path::PathBuf {
    state.usage_dir.join("external-agent-availability.json")
}

fn load_availability_snapshot(state: &AppState) -> Option<Vec<DetectedAgent>> {
    let content = std::fs::read_to_string(availability_snapshot_path(state)).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_availability_snapshot(state: &AppState, agents: &[DetectedAgent]) {
    let Ok(content) = serde_json::to_string(agents) else {
        return;
    };
    let path = availability_snapshot_path(state);
    if let Err(err) = crate::chat::storage::atomic_write(&path, &content, "外部 CLI 可用性快照")
    {
        eprintln!("[external-agent] 保存可用性快照失败: {err}");
    }
}

#[tauri::command]
pub async fn chat_detect_external_agents(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    force_refresh: Option<bool>,
    conversation_id: Option<String>,
) -> Result<serde_json::Value, String> {
    let _ = &conversation_id; // 可用性与 cwd 无关；参数保留为兼容前端签名。
    let force = force_refresh.unwrap_or(false);
    if !force {
        if let Some(agents) =
            state.get_cached_detected_agents(AVAILABILITY_CACHE_KEY, AVAILABILITY_CACHE_TTL)
        {
            return Ok(serde_json::json!({
                "success": true,
                "agents": stamp_disabled(agents),
                "cached": true,
            }));
        }
        // 内存没有（刚启动 / TTL 过期）就先把上次的快照端上去，真探测丢到后台，
        // 探完通过 `external-agents-updated` 推给前端。用户看到的是「立刻有列表，
        // 几秒后自己刷新」，而不是空列表 + 手动点重新扫描。
        if let Some(agents) = load_availability_snapshot(&state) {
            spawn_availability_refresh(app);
            return Ok(serde_json::json!({
                "success": true,
                "agents": stamp_disabled(agents),
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
                "agents": stamp_disabled(agents),
                "cached": true,
            }));
        }
    }
    let agents = detect_availability_all().await;
    state.set_cached_detected_agents(AVAILABILITY_CACHE_KEY.to_string(), agents.clone());
    save_availability_snapshot(&state, &agents);
    Ok(serde_json::json!({
        "success": true,
        "agents": stamp_disabled(agents),
        "cached": false,
    }))
}

/// 后台重探可用性并广播结果。single-flight 锁用 `try_lock`：已经有人在探就直接算了，
/// 那一轮探完自己会广播。
fn spawn_availability_refresh(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let Ok(_guard) = state.availability_probe_lock.try_lock() else {
            return;
        };
        let agents = detect_availability_all().await;
        state.set_cached_detected_agents(AVAILABILITY_CACHE_KEY.to_string(), agents.clone());
        save_availability_snapshot(&state, &agents);
        let _ = app.emit(
            "external-agents-updated",
            serde_json::json!({ "agents": stamp_disabled(agents) }),
        );
    });
}

/// 删除供应商时清掉它物化出来的文件（不影响任何设置，纯清理）。
///
/// 「保存设置 → 物化 + 清缓存」那条路在 `settings::persist_settings` 里，不需要前端调命令；
/// 删除是唯一一个保存后**没有任何东西会去动那些文件**的操作，所以单独留一个口。
#[tauri::command]
pub async fn chat_external_cli_provider_cleanup(
    agent_id: String,
    provider_id: String,
    native_provider_id: Option<String>,
    provider_name: Option<String>,
) -> Result<(), String> {
    crate::external_agents::provider_profile::cleanup(
        &agent_id,
        &provider_id,
        native_provider_id.as_deref(),
        provider_name.as_deref(),
    );
    Ok(())
}

/// 解析 Pi 实际使用的 agent 目录（认 `PI_CODING_AGENT_DIR`）。
#[tauri::command]
pub fn chat_external_cli_pi_agent_dir() -> Option<String> {
    crate::external_agents::provider_profile::pi_agent_dir()
        .map(|path| path.to_string_lossy().into_owned())
}

/// 供应商弹窗的「获取模型」：拿填好的 base_url + key 去中转站问模型列表。
/// 只作建议用，拉不到就报错文案，不影响手填。
#[tauri::command]
pub async fn chat_external_cli_fetch_relay_models(
    state: tauri::State<'_, AppState>,
    base_url: String,
    api_key: String,
) -> Result<Vec<String>, String> {
    crate::external_agents::relay_models::fetch(&state.http, &base_url, &api_key).await
}

/// 扫描本机 cc-switch 的库，列出可导入的供应商。只读，不写任何东西。
#[tauri::command]
pub async fn chat_external_cli_scan_cc_switch() -> Result<serde_json::Value, String> {
    let scan = crate::external_agents::cc_switch::scan()?;
    serde_json::to_value(scan).map_err(|e| e.to_string())
}

/// 把设置页的「停用」开关盖到（可能来自缓存的）可用性结果上。
fn stamp_disabled(
    mut agents: Vec<crate::external_agents::types::DetectedAgent>,
) -> Vec<crate::external_agents::types::DetectedAgent> {
    for agent in agents.iter_mut() {
        agent.disabled = crate::external_agents::overrides::is_disabled(&agent.id);
    }
    agents
}

/// 用户手填的模型追加到探测结果后面；id 撞车时保留探测出来的那条。
fn merge_custom_models(
    agent_id: &str,
    models: &[crate::external_agents::types::RuntimeModelOption],
) -> Vec<crate::external_agents::types::RuntimeModelOption> {
    let mut merged = models.to_vec();
    for custom in crate::external_agents::overrides::custom_models(agent_id) {
        if merged.iter().any(|m| m.id == custom.id) {
            continue;
        }
        let label = if custom.label.is_empty() {
            custom.id.clone()
        } else {
            custom.label
        };
        merged.push(crate::external_agents::types::RuntimeModelOption {
            id: custom.id,
            label,
            context_window_tokens: None,
        });
    }
    merged
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
        &merge_custom_models(&agent_id, &probe.models),
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
        &merge_custom_models(def.id, &cached.models),
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
pub async fn chat_set_agent_runtime(
    app: AppHandle,
    conversation_id: String,
    agent_runtime: AgentRuntimeConfig,
) -> Result<serde_json::Value, String> {
    let conversation = crate::chat::repository::repository(&app)
        .mutate(&app, &conversation_id, |conversation| {
            check_runtime_switch_allowed(
                &conversation.agent_runtime,
                conversation.messages.is_empty(),
                &agent_runtime,
            )?;
            conversation.agent_runtime = agent_runtime;
            Ok(())
        })
        .await
        .map_err(crate::chat::repository::repository_error)?;
    Ok(serde_json::json!({
        "success": true,
        "conversation": conversation,
    }))
}

/// Pure binding rule for `chat_set_agent_runtime` (extracted so it is unit-testable without a Tauri
/// `AppHandle`). Returns `Err` with a user-facing message when the switch is forbidden.
///
/// Rule: **one agent per conversation**. Empty conversations may switch freely; once any message
/// exists, `kind` and `external_agent_id` are frozen (covers both local CLI *and* Kivio builtin).
/// Same-agent model / reasoning / sandbox updates remain allowed.
fn check_runtime_switch_allowed(
    current: &AgentRuntimeConfig,
    messages_is_empty: bool,
    next: &AgentRuntimeConfig,
) -> Result<(), String> {
    if messages_is_empty {
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
        let bound_name = if current.is_external() {
            current
                .external_agent_id
                .as_deref()
                .and_then(get_agent_def)
                .map(|d| d.name.to_string())
                .or_else(|| normalize_id(&current.external_agent_id))
                .unwrap_or_else(|| "当前 CLI".to_string())
        } else if current.is_chat() {
            "Kivio Chat".to_string()
        } else {
            "Kivio Agent".to_string()
        };
        return Err(format!("会话已绑定 {bound_name}，新建会话可切换 Agent"));
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
    fn non_empty_builtin_rejects_switch_to_external() {
        let current = AgentRuntimeConfig::default(); // builtin
        let to_external = external("claude", "default");
        assert!(check_runtime_switch_allowed(&current, false, &to_external).is_err());
    }

    #[test]
    fn non_empty_builtin_allows_same_kind_noop() {
        let current = AgentRuntimeConfig::default();
        let same = AgentRuntimeConfig::default();
        assert!(check_runtime_switch_allowed(&current, false, &same).is_ok());
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

// ---------------------------------------------------------------------------------------------
// 从本地 CLI 导入对话
// ---------------------------------------------------------------------------------------------

/// 列出某个项目下可导入的原生会话（按项目工作目录过滤，已导入的带标记）。
#[tauri::command]
pub async fn chat_list_importable_cli_sessions(
    app: AppHandle,
    project_id: String,
) -> Result<serde_json::Value, String> {
    let sessions =
        crate::external_agents::import::list_importable_for_project(&app, &project_id).await?;
    Ok(serde_json::json!({ "success": true, "sessions": sessions }))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSessionRequest {
    pub agent_id: String,
    pub session_id: String,
}

/// 批量导入。**单条失败不影响其它条**——一次勾十条，不该因为其中一条的文件损坏就全军覆没。
#[tauri::command]
pub async fn chat_import_cli_sessions(
    app: AppHandle,
    project_id: String,
    items: Vec<ImportSessionRequest>,
) -> Result<serde_json::Value, String> {
    let mut imported = Vec::new();
    let mut failures = Vec::new();
    for item in items {
        match crate::external_agents::import::import_one_session(
            &app,
            &project_id,
            &item.agent_id,
            &item.session_id,
        )
        .await
        {
            Ok(conversation_id) => imported.push(serde_json::json!({
                "agentId": item.agent_id,
                "sessionId": item.session_id,
                "conversationId": conversation_id,
            })),
            Err(error) => failures.push(serde_json::json!({
                "agentId": item.agent_id,
                "sessionId": item.session_id,
                "error": error,
            })),
        }
    }
    Ok(serde_json::json!({
        "success": failures.is_empty(),
        "imported": imported,
        "failures": failures,
    }))
}

/// 这条导入的对话，在 CLI 那边是不是已经有新内容了（ADR-0002 的过期提示）。
#[tauri::command]
pub fn chat_imported_history_stale(app: AppHandle, conversation_id: String) -> bool {
    crate::external_agents::import::imported_history_is_stale(&app, &conversation_id)
}
