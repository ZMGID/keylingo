use std::{path::PathBuf, time::Instant};

use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::chat::agent::execute::truncate_chars;
use crate::chat::agent::prepare as agent_prepare;
use crate::chat::attachments::{
    compose_text_attachments_for_api, text_attachments_from_attachments,
};
use crate::chat::{
    chat_missing_model_error, format_chat_missing_api_key_error, session_model_for_conversation,
    Conversation, ToolCallStatus,
};
use crate::chat::model_metadata::{
    chat_max_output_tokens_for_model, model_can_generate_images_directly,
};
use crate::chat::storage::live_set_system_prompt;
use crate::chat::vision::{
    analyze_chat_images_with_auxiliary_model, auxiliary_vision_model_for_images,
    auxiliary_vision_tool_record, finish_auxiliary_vision_tool_record,
    user_content_with_auxiliary_vision_result,
};
use crate::skills;
use crate::state::AppState;

#[cfg(debug_assertions)]
use super::agent_host::ProbeAgentHost;
use super::agent_host::{ChatAgentHost, RegistryToolExecutor};
use super::catalog::{
    chat_memory_prompt_for_request, is_builder_conversation, project_prompt_context_for,
};
use super::context::{build_chat_api_messages, resolve_usage_anchor};
use super::direct_image::complete_direct_image_generation_reply;
use super::interaction::{emit_chat_stream_delta, emit_chat_tool_record, wait_for_chat_cancel};
use super::messages::{
    auxiliary_tool_segments, build_assistant_message, capture_agent_plan_draft_if_needed,
    push_assistant_message, tool_segment_for_record,
};
use super::reply_runtime::{ArmReplyOutcome, ChatReplyGuard, ReplyArm};
use super::resolve_thinking;
use super::tooling::{
    append_agent_ask_user_tools, append_agent_todo_tools, apply_agent_plan_tool_filter,
    apply_chat_mode_tool_filter, apply_inline_code_request_tool_filter,
    apply_web_search_mode_tool_filter, list_tools_for_chat, resolve_forced_skill_id,
};

pub(super) async fn complete_assistant_reply(
    app: &AppHandle,
    state: &State<'_, AppState>,
    conversation: &mut Conversation,
    title_from_first_user: Option<&str>,
    last_user_api_content: Option<&str>,
    last_user_image_paths: &[PathBuf],
    active_skill_id: Option<&str>,
    entry: crate::chat::agent::AgentRunEntry,
) -> Result<(), String> {
    complete_assistant_reply_inner(
        app,
        state,
        conversation,
        title_from_first_user,
        last_user_api_content,
        last_user_image_paths,
        active_skill_id,
        entry,
        None,
        false,
    )
    .await
    .map(|_| ())
}

/// 共享实现：`arm = None` 为单模型现状（直接落盘，返回 `Ok(())` 语义不变）；
/// `arm = Some(..)` 为多模型臂（用臂的 provider/model、自动批准工具、**不落盘**，
/// 把产出的 assistant 消息通过 `ArmReplyOutcome.message` 返回给协调者）。
pub(super) async fn complete_assistant_reply_inner(
    app: &AppHandle,
    state: &State<'_, AppState>,
    conversation: &mut Conversation,
    title_from_first_user: Option<&str>,
    last_user_api_content: Option<&str>,
    last_user_image_paths: &[PathBuf],
    active_skill_id: Option<&str>,
    entry: crate::chat::agent::AgentRunEntry,
    arm: Option<&ReplyArm>,
    probe: bool,
) -> Result<ArmReplyOutcome, String> {
    if conversation.agent_runtime.is_external() {
        // 外部 CLI 路径在 run.rs 内自带 generation；这里登记一条 per-run 回复槽位，
        // 让 `conversation_has_active_reply` 在外部回复期间也能拒绝并发新发送（防回归）。
        let ext_generation = state.next_chat_generation(&conversation.id);
        let ext_run_id = format!("chat-run-ext-{}-{}", ext_generation, Uuid::new_v4());
        let _ext_reply_guard =
            ChatReplyGuard::try_new(state.inner(), &conversation.id, &ext_run_id, ext_generation);
        let latest_user = conversation
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user");
        // 虚拟文本附件（memory://）正文只存在附件记录里、不在 message.content 中：
        // 在此重建内联正文，外部 CLI 才能看到粘贴的长文本。磁盘附件仍走
        // file_paths → file_attachments_note 的路径说明（run.rs），不进正文。
        let latest_user_text = latest_user
            .map(|m| {
                let text_attachments = text_attachments_from_attachments(&m.attachments);
                if text_attachments.is_empty() {
                    m.content.clone()
                } else {
                    compose_text_attachments_for_api(&m.content, &text_attachments)
                }
            })
            .unwrap_or_default();
        // 外部 CLI 也要带附件：图片走各协议原生块 / 降级，文件走路径说明。图片路径已由调用方
        // 算好（last_user_image_paths）；文件路径从最后一条 user 消息现解析（best-effort）。
        let latest_user_file_paths = latest_user
            .map(|m| {
                crate::chat::attachments::stored_file_paths_for_attachments(
                    app,
                    &conversation.id,
                    &m.attachments,
                )
                .unwrap_or_default()
            })
            .unwrap_or_default();
        return crate::external_agents::run_external_cli_reply(
            app,
            state,
            conversation,
            title_from_first_user,
            &latest_user_text,
            last_user_image_paths,
            &latest_user_file_paths,
            active_skill_id,
            entry,
        )
        .await
        .map(|_| ArmReplyOutcome {
            message: None,
            run_id: None,
            error: None,
        });
    }

    let settings = state.settings_read().clone();
    // 多模型臂用自己的 provider/model；单模型用会话级（行为不变）。
    // 提前转成 owned，避免对 `conversation` 的长期不可变借用挡住后续的 `&mut conversation`。
    let resolved_provider_id = arm
        .map(|a| a.provider_id.clone())
        .unwrap_or_else(|| conversation.provider_id.clone());
    let resolved_model = arm
        .map(|a| a.model.clone())
        .unwrap_or_else(|| conversation.model.clone());
    let provider = settings
        .get_provider(&resolved_provider_id)
        .ok_or_else(|| "Chat provider not found".to_string())?
        .clone();
    if provider.api_keys.is_empty() {
        return Err(format_chat_missing_api_key_error(&provider.name));
    }
    if resolved_model.trim().is_empty() {
        return Err(chat_missing_model_error());
    }

    let last_user_idx = conversation.messages.iter().rposition(|m| m.role == "user");
    let language = crate::settings::resolve_chat_language(&settings);
    // 思考：每对话等级覆盖全局开关。None=跟随全局（现状）；"off"=强制关；low/medium/high=按家族注入。
    let (thinking_enabled, thinking_level) = resolve_thinking(
        conversation.thinking_level.as_deref(),
        settings.chat.thinking_enabled,
        Some(&provider),
        &resolved_model,
    );
    let retry_attempts = if settings.retry_enabled {
        settings.retry_attempts as usize
    } else {
        1
    };
    let chat_mode = conversation.agent_runtime.is_chat();
    // Plan/Orchestrate only apply to the full Kivio Agent runtime, not Chat.
    let plan_mode = !chat_mode && crate::chat::plan::is_plan_mode(&conversation.agent_plan_state);
    let orchestrate_mode =
        !chat_mode && crate::chat::plan::is_orchestrate_mode(&conversation.agent_plan_state);
    let direct_image_model =
        !plan_mode && model_can_generate_images_directly(&provider, &resolved_model);
    let run_generation = state.next_chat_generation(&conversation.id);
    let run_id = format!("chat-run-{}-{}", run_generation, Uuid::new_v4());
    let assistant_message_id = format!("msg_{}", Uuid::new_v4());
    let recovery = arm.map(|arm| crate::chat::protocol::ChatRunRecoveryMetadata {
        group_id: arm.group_id.clone(),
        group_size: arm.group_size as u32,
        arm_index: arm.arm_index as u32,
        provider_id: arm.provider_id.clone(),
        model: arm.model.clone(),
    });
    crate::chat::protocol::register_run_with_recovery(
        app,
        &conversation.id,
        &run_id,
        &assistant_message_id,
        conversation.revision,
        recovery,
    );
    let mut protocol_guard =
        crate::chat::protocol::RegisteredRunGuard::new(app, &run_id, conversation.revision);
    // per-run 回复槽位 + 活跃 generation 守卫：本函数任意退出路径（含早返回的直接生图 /
    // 辅助视觉分支）都会 drop 它，释放该 run 的槽位并退役其 generation。同会话多模型并发时
    // 每条 run 各持一个守卫，互不影响。`next_chat_generation` 已登记 generation，这里仅补登
    // run_id 槽位；run_id 由 generation + uuid 拼成，必不重复，try_new 不会返回 None。
    let _reply_guard =
        ChatReplyGuard::try_new(state.inner(), &conversation.id, &run_id, run_generation);
    if direct_image_model && arm.is_some() {
        protocol_guard.defer_terminal();
        return Ok(ArmReplyOutcome {
            message: None,
            run_id: Some(run_id),
            error: Some(
                "多模型并行回答暂不支持直接生图模型，请在多答选择中移除该模型。".to_string(),
            ),
        });
    }
    if direct_image_model {
        return complete_direct_image_generation_reply(
            app,
            state,
            &settings,
            &provider,
            conversation,
            title_from_first_user,
            last_user_api_content,
            last_user_image_paths,
            active_skill_id,
            &run_id,
            assistant_message_id,
            run_generation,
            retry_attempts,
            entry,
        )
        .await
        .map(|_| ArmReplyOutcome {
            message: None,
            run_id: None,
            error: None,
        });
    }
    let session = session_model_for_conversation(conversation);
    let auxiliary_vision_model = auxiliary_vision_model_for_images(
        &settings,
        Some(&provider),
        &resolved_model,
        last_user_image_paths,
        Some(session),
    );
    let mut auxiliary_tool_records = Vec::new();
    let auxiliary_vision_result = if let Some(auxiliary_vision_model) = auxiliary_vision_model {
        let mut record = auxiliary_vision_tool_record(
            &settings,
            &auxiliary_vision_model,
            last_user_image_paths.len(),
        );
        let started = Instant::now();
        emit_chat_stream_delta(
            app,
            &run_id,
            "",
            None,
            Some(&tool_segment_for_record(&record, 100, None)),
        );
        emit_chat_tool_record(app, &run_id, &record);
        let analysis = tokio::select! {
            result = analyze_chat_images_with_auxiliary_model(
                state,
                &settings,
                &auxiliary_vision_model,
                &conversation.id,
                &assistant_message_id,
                last_user_api_content,
                last_user_image_paths,
                retry_attempts,
                &language,
            ) => result,
            _ = wait_for_chat_cancel(state.inner(), &conversation.id, run_generation) => {
                finish_auxiliary_vision_tool_record(
                    &mut record,
                    ToolCallStatus::Cancelled,
                    started,
                    None,
                    Some("Mixer vision analysis cancelled".to_string()),
                );
                emit_chat_tool_record(app, &run_id, &record);
                auxiliary_tool_records.push(record);
                if arm.is_some() {
                    protocol_guard.defer_terminal();
                    return Ok(ArmReplyOutcome {
                        message: None,
                        run_id: Some(run_id),
                        error: Some("cancelled".to_string()),
                    });
                }
                crate::chat::protocol::finish_run(
                    app,
                    &run_id,
                    "cancelled",
                    "",
                    conversation.revision,
                );
                return Err("cancelled".to_string());
            }
        };
        match analysis {
            Ok(result) => {
                finish_auxiliary_vision_tool_record(
                    &mut record,
                    ToolCallStatus::Success,
                    started,
                    Some(truncate_chars(result.content.trim(), 1000)),
                    None,
                );
                emit_chat_tool_record(app, &run_id, &record);
                auxiliary_tool_records.push(record);
                Some(result)
            }
            Err(err) => {
                finish_auxiliary_vision_tool_record(
                    &mut record,
                    ToolCallStatus::Error,
                    started,
                    None,
                    Some(err.clone()),
                );
                emit_chat_tool_record(app, &run_id, &record);
                auxiliary_tool_records.push(record);
                if arm.is_some() {
                    protocol_guard.defer_terminal();
                    return Ok(ArmReplyOutcome {
                        message: None,
                        run_id: Some(run_id),
                        error: Some(err),
                    });
                }
                return Err(err);
            }
        }
    } else {
        None
    };
    let empty_image_paths: &[PathBuf] = &[];
    let main_image_paths = if auxiliary_vision_result.is_some() {
        empty_image_paths
    } else {
        last_user_image_paths
    };
    let augmented_last_user_content = auxiliary_vision_result.as_ref().map(|result| {
        user_content_with_auxiliary_vision_result(last_user_api_content, result, &language)
    });
    let last_user_content_for_main = augmented_last_user_content
        .as_deref()
        .or(last_user_api_content);
    let skill_cwd = crate::chat::storage::resolve_conversation_working_directory(
        app,
        conversation,
        &settings.chat_tools.native_tools.working_directory,
    )
    .ok();
    let skill_registry = skills::build_registry_in(
        app,
        &settings.chat_tools.skill_scan_paths,
        skill_cwd.as_deref(),
    )
    .unwrap_or_default();
    let requested_skill_id = active_skill_id.or(conversation.active_skill_id.as_deref());
    let skill_id = resolve_forced_skill_id(
        &settings.chat_tools,
        conversation.assistant_snapshot.as_ref(),
        &skill_registry,
        requested_skill_id,
        crate::settings::obsidian_connector_configured(&settings.obsidian_vault_path),
    );
    if skill_id.is_none() && conversation.active_skill_id.is_some() {
        conversation.active_skill_id = None;
    }
    let active_skill_detail = skill_id.as_deref().and_then(|id| {
        skills::read_skill_detail_in(
            app,
            &settings.chat_tools.skill_scan_paths,
            id,
            skill_cwd.as_deref(),
        )
        .ok()
    });
    let mut effective_chat_tools = settings.chat_tools.clone();
    if arm.is_some() || probe {
        // 多答 fan-out（决策 D1 注）：N 条并行 run 若各自弹工具审批会产生 N 倍弹窗、
        // 且无法对应到具体列。多模型臂内一律自动批准（静默执行）。单模型保持原审批策略。
        // probe（无头测试通道）同理：无 GUI 可应答审批，必须自动放行，否则挂起。
        effective_chat_tools.approval_policy = "auto".to_string();
    }
    let (memory_prompt, memory_warning) = chat_memory_prompt_for_request(app, &settings);
    if let Some(warning) = memory_warning.as_ref() {
        conversation.context_state.warning = Some(warning.clone());
    }
    let tools_capable = agent_prepare::chat_tools_capable(
        &effective_chat_tools,
        settings.chat_memory.enabled,
        crate::settings::chat_image_generation_enabled_for_session(
            &settings,
            Some(session_model_for_conversation(conversation)),
        ),
    );
    let tool_list = list_tools_for_chat(
        app,
        state.inner(),
        &settings,
        Some(session_model_for_conversation(conversation)),
    )
    .await;
    let unavailable_mcp_servers = tool_list.unavailable_mcp_servers;
    let mut tools = tool_list.tools;
    agent_prepare::apply_assistant_mcp_restrictions(
        &mut tools,
        conversation.assistant_snapshot.as_ref(),
    );
    let builder_mode = is_builder_conversation(conversation);
    if builder_mode {
        // 搭建会话只暴露 save_assistant,屏蔽文件/命令/MCP/技能等,保持聚焦。
        tools.clear();
        tools.push(crate::mcp::types::native_save_assistant_tool());
    }
    apply_inline_code_request_tool_filter(&mut tools, last_user_api_content);
    let blocked_tool_calls = if chat_mode {
        apply_chat_mode_tool_filter(&mut tools, true, &settings.chat.chat_mode)
    } else {
        apply_agent_plan_tool_filter(&mut tools, plan_mode)
    };
    // 会话级三态联网搜索（任务 07-23）：按有效模式收敛第三方 `search_web` 的暴露；
    // 内置搜索走 `config.web_search_mode` → 各适配器请求体注入，不在工具列表里。
    // builder 会话已清空工具（只留 save_assistant），不参与搜索门控。
    let web_search_mode =
        crate::chat::types::WebSearchMode::resolve(conversation.web_search_mode, &settings);
    if !builder_mode {
        apply_web_search_mode_tool_filter(&mut tools, web_search_mode, &settings);
    }
    let user_tools_available = tools_capable && !tools.is_empty();
    agent_prepare::apply_skill_fallback_when_tools_unavailable(
        &mut effective_chat_tools,
        skill_id.as_deref(),
        user_tools_available,
    );
    let ask_user_tools_available = append_agent_ask_user_tools(&mut tools);
    let todo_tools_available = if chat_mode {
        false
    } else {
        append_agent_todo_tools(&mut tools)
    };
    // Resolved here (rather than further down with the other prompt context) so
    // the sub-agent role registry below can reuse the project root instead of
    // resolving the conversation's project a second time.
    let project_prompt_context = project_prompt_context_for(app, conversation);
    // Multi-agent spawn tool (P3): exposure is mode-controlled. Act and
    // Orchestrate both expose the `agent` tool; Plan / Chat exclude it (spawn is a
    // side-effecting, non-read-only capability).
    if !plan_mode && !chat_mode && !builder_mode {
        // Load the role registry (built-in + user + project) so the `agent`
        // tool's schema lists the roles that actually exist for this
        // conversation — the model must not have to guess role names.
        let project_root = project_prompt_context
            .as_ref()
            .and_then(|context| context.root_path.as_deref())
            .map(std::path::Path::new);
        let agent_defs = crate::agents::load_agent_definitions(app, project_root);
        crate::chat::sub_agent::append_tool_definitions(&mut tools, true, &agent_defs);
    }
    // Orchestrate mode raises the autonomy budget: a single user message may
    // need more tool rounds to plan, fan out sub-agents, and aggregate. We lift
    // max_tool_rounds to max(configured, ORCHESTRATE_MIN_TOOL_ROUNDS) but keep
    // unlimited (None) as-is rather than forcing a cap.
    if orchestrate_mode {
        effective_chat_tools.max_tool_rounds = effective_chat_tools
            .max_tool_rounds
            .map(|rounds| rounds.max(crate::settings::ORCHESTRATE_MIN_TOOL_ROUNDS));
    }
    let runtime_tools_available = !tools.is_empty();
    let available_builtin_tools = agent_prepare::available_builtin_tool_names(&tools);
    let agent_todo_prompt = if chat_mode {
        None
    } else {
        Some(crate::chat::todo::format_prompt(
            &conversation.agent_todo_state,
            todo_tools_available,
        ))
    };
    let agent_ask_user_prompt = crate::chat::ask_user::format_prompt(ask_user_tools_available);
    let runtime_prompts = agent_prepare::resolve_runtime_prompt_sources(
        chat_mode,
        settings.chat.system_prompt.as_str(),
        settings.chat.chat_mode.system_prompt.as_str(),
        &conversation.agent_plan_state,
    );
    // Default workbench surfaced to the model. It is an ergonomic default, not
    // a sandbox; explicit user paths continue to take precedence.
    let workbench_dir = crate::chat::storage::resolve_conversation_working_directory(
        app,
        conversation,
        &settings.chat_tools.native_tools.working_directory,
    )
    .ok()
    .map(|path| path.display().to_string());
    // 集的系统提示词：按对话 set_id 实时取（不冻结），随集编辑立即对集内对话生效。
    let set_system_prompt = live_set_system_prompt(app, conversation);
    let obsidian_vault_path = (!settings.obsidian_vault_path.trim().is_empty())
        .then_some(settings.obsidian_vault_path.as_str());
    let knowledge_base_prompt = crate::chat::knowledge_base::mount_system_prompt(
        app,
        &conversation.knowledge_base_ids,
        conversation.force_knowledge_search,
    );
    let system_prompt = agent_prepare::build_chat_system_prompt(
        &language,
        !main_image_paths.is_empty(),
        thinking_enabled,
        &skill_registry,
        &effective_chat_tools,
        runtime_tools_available,
        &available_builtin_tools,
        skill_id.as_deref(),
        active_skill_detail.as_ref(),
        conversation.assistant_snapshot.as_ref(),
        set_system_prompt.as_deref(),
        runtime_prompts.custom_system_prompt.as_str(),
        runtime_prompts.is_chat_runtime,
        memory_prompt.as_deref(),
        runtime_prompts.agent_plan_prompt.as_deref(),
        Some(&agent_ask_user_prompt),
        agent_todo_prompt.as_deref(),
        project_prompt_context.as_ref(),
        workbench_dir.as_deref(),
        knowledge_base_prompt.as_deref(),
        obsidian_vault_path,
    );
    // 从未成功连接的 MCP server：工具没法降级进列表，注一行说明让模型知道
    // "配置了但连不上"，而不是回答"没有这个工具"。
    let system_prompt =
        match crate::mcp::registry::unavailable_mcp_servers_note(&unavailable_mcp_servers) {
            Some(note) => format!("{system_prompt}\n\n{note}"),
            None => system_prompt,
        };

    let runtime_messages = match build_chat_api_messages(
        Some(app),
        &system_prompt,
        conversation,
        last_user_idx,
        last_user_content_for_main,
        main_image_paths,
    ) {
        Ok(messages) => messages,
        Err(error) => {
            if arm.is_some() {
                protocol_guard.defer_terminal();
                return Ok(ArmReplyOutcome {
                    message: None,
                    run_id: Some(run_id),
                    error: Some(error),
                });
            }
            return Err(error);
        }
    };
    let mut fallback_chat_tools = effective_chat_tools.clone();
    if skill_id.is_some() && fallback_chat_tools.skill_fallback_mode == "progressive" {
        fallback_chat_tools.skill_fallback_mode = "skill_md_only".to_string();
    }
    let provider_tools_fallback_system_prompt = agent_prepare::build_chat_system_prompt(
        &language,
        !main_image_paths.is_empty(),
        thinking_enabled,
        &skill_registry,
        &fallback_chat_tools,
        false,
        &[],
        skill_id.as_deref(),
        active_skill_detail.as_ref(),
        conversation.assistant_snapshot.as_ref(),
        set_system_prompt.as_deref(),
        runtime_prompts.custom_system_prompt.as_str(),
        runtime_prompts.is_chat_runtime,
        memory_prompt.as_deref(),
        runtime_prompts.agent_plan_prompt.as_deref(),
        Some(&crate::chat::ask_user::format_prompt(false)),
        if chat_mode {
            None
        } else {
            Some(crate::chat::todo::format_prompt(
                &conversation.agent_todo_state,
                false,
            ))
        }
        .as_deref(),
        project_prompt_context.as_ref(),
        workbench_dir.as_deref(),
        knowledge_base_prompt.as_deref(),
        obsidian_vault_path,
    );

    let chat_host = ChatAgentHost {
        app: app.clone(),
        state: state.inner(),
        run_id: run_id.clone(),
        // 多模型臂不直接落盘（最终由协调者统一 upsert + save），因此抑制 loop 的
        // mid-run 部分快照写盘，避免 N 条并发 run 同写 conversations/{id}.json 的竞态。
        suppress_partial_persist: arm.is_some(),
        // 生命周期 Hooks：无启用条目时为 None，loop 完全不感知。先用 `any_enabled`
        // 短路，没配 Hook 时连下面这几个 id / model 字符串都不分配（验收 6）。
        hooks: crate::chat::hooks::HookDispatcher::any_enabled(&settings.chat_tools.hooks)
            .then(|| {
                crate::chat::hooks::HookDispatcher::new(
                    app.clone(),
                    &settings.chat_tools.hooks,
                    conversation.id.clone(),
                    run_id.clone(),
                    assistant_message_id.clone(),
                    workbench_dir
                        .as_deref()
                        .map(PathBuf::from)
                        .unwrap_or_else(std::env::temp_dir),
                    format!("{}:{}", provider.id, resolved_model),
                )
            })
            .flatten(),
    };
    // probe（无头测试通道，仅 debug）：换用自动放行审批/consent/ask_user 的 host，
    // 否则模型调用敏感工具或 ask_user 会 await GUI 应答而永久挂起。
    #[cfg(debug_assertions)]
    let probe_host = ProbeAgentHost {
        state: state.inner(),
    };
    let host: &dyn crate::chat::agent::AgentHost = {
        #[cfg(debug_assertions)]
        {
            if probe {
                &probe_host
            } else {
                &chat_host
            }
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = probe;
            &chat_host
        }
    };
    let executor = RegistryToolExecutor {
        app: app.clone(),
        state: state.inner(),
    };
    let max_output_tokens = chat_max_output_tokens_for_model(
        Some(&provider),
        &resolved_model,
        settings.chat.max_output_tokens,
    );
    // 真实用量锚点：run 首次压缩检查前，用上一轮落盘 usage 把上下文占用锚定到 provider 实报值
    // （对齐 pi/opencode 的 ground-truth 口径，避免字符估算低估导致压缩过晚/超窗）。
    let (initial_anchor_total_tokens, initial_anchor_trailing_estimate) =
        resolve_usage_anchor(conversation, Some(&provider));
    let result = crate::chat::agent::run_agent_loop(
        crate::chat::agent::AgentRunConfig {
            state: state.inner(),
            conversation_id: conversation.id.clone(),
            tool_conversation_id: conversation.id.clone(),
            depth: 0,
            run_id: run_id.clone(),
            message_id: assistant_message_id.clone(),
            generation: run_generation,
            provider,
            model: resolved_model.clone(),
            runtime_messages,
            tools,
            blocked_tool_calls,
            settings: settings.clone(),
            effective_chat_tools,
            language,
            thinking_enabled,
            thinking_level,
            web_search_mode,
            max_output_tokens,
            retry_attempts,
            assistant_snapshot: conversation.assistant_snapshot.clone(),
            provider_tools_fallback_system_prompt,
            initial_anchor_total_tokens,
            initial_anchor_trailing_estimate,
            skill_project_cwd: skill_cwd.clone(),
        },
        host,
        &executor,
    )
    .await;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            if arm.is_some() {
                protocol_guard.defer_terminal();
                return Ok(ArmReplyOutcome {
                    message: None,
                    run_id: Some(run_id),
                    error: Some(error),
                });
            }
            let (terminal_content, terminal_revision) =
                match crate::chat::repository::repository(app)
                    .get(app, &conversation.id)
                    .await
                {
                    Ok(latest) => {
                        let content = latest
                            .messages
                            .iter()
                            .find(|message| message.id == assistant_message_id)
                            .map(|message| message.content.clone())
                            .unwrap_or_default();
                        let revision = latest.revision;
                        *conversation = latest;
                        (content, revision)
                    }
                    Err(_) => (String::new(), conversation.revision),
                };
            crate::chat::protocol::finish_run(
                app,
                &run_id,
                if error == "cancelled" {
                    "cancelled"
                } else {
                    "error"
                },
                &terminal_content,
                terminal_revision,
            );
            return Err(error);
        }
    };

    let refreshed = crate::chat::repository::repository(app)
        .get(app, &conversation.id)
        .await
        .map_err(crate::chat::repository::repository_error);
    *conversation = match refreshed {
        Ok(conversation) => conversation,
        Err(error) => {
            if arm.is_some() {
                protocol_guard.defer_terminal();
                return Ok(ArmReplyOutcome {
                    message: None,
                    run_id: Some(run_id),
                    error: Some(error),
                });
            }
            return Err(error);
        }
    };
    let message_plan = capture_agent_plan_draft_if_needed(
        conversation,
        plan_mode,
        &result.content,
        result.stream_outcome.as_str(),
    );
    let mut segments = auxiliary_tool_segments(&auxiliary_tool_records);
    segments.extend(result.segments);
    let mut tool_records = auxiliary_tool_records;
    tool_records.extend(result.tool_records);
    // 模型原生生成的图片（Gemini native image gen，任务 07-24）→ assistant 消息级 artifacts，
    // 前端答案下方图片画廊直接展示（消费 message.artifacts）。空则无开销。
    let artifacts = generated_image_artifacts(&result.images);
    let run_entry = agent_run_entry_label(entry);
    if let Some(arm) = arm {
        // 多模型臂：构造 assistant 消息但**不落盘**，交协调者统一合并 + 一次性 save。
        let message = build_assistant_message(
            assistant_message_id,
            result.content,
            result.reasoning,
            artifacts,
            tool_records,
            result.api_messages,
            segments,
            skill_id.as_deref(),
            Some(run_entry),
            Some(result.stream_outcome.as_str()),
            result.usage,
            result.last_step_usage,
            message_plan,
            Some((
                arm.group_id.clone(),
                resolved_provider_id.clone(),
                resolved_model.clone(),
            )),
        );
        // 降级兜底的结构化描述挂到消息上，前端渲染成独立错误卡片（正文不再混故障文案）。
        let mut message = message;
        message.degraded = result.degraded;
        protocol_guard.defer_terminal();
        return Ok(ArmReplyOutcome {
            message: Some(message),
            run_id: Some(run_id),
            error: None,
        });
    }
    if let Some(boundary) = result.compaction_boundary.clone() {
        conversation
            .context_state
            .compaction_boundaries
            .push(boundary);
    }
    // L2 压缩对齐落盘路径：run 结束时把 L2 产出的 summary 写回 context_state.summary +
    // compression_count（不再只 push boundary）。质量兜底已在 compaction 核心拦截，此处直接采用。
    if let Some(mut summary) = result.compaction_summary.clone() {
        // L2 产出的 summary.source_message_ids 为空（运行时侧拿不到完整 UI id 列表）——
        // 在此按 source_until_message_id 从 conversation 累积（含旧 summary 覆盖范围），
        // 与落盘路径 compact_conversation_inner 口径一致。必须在替换 summary **之前**读旧 S1。
        summary.source_message_ids = crate::chat::agent::compaction::accumulate_source_ids(
            conversation,
            &summary.source_until_message_id,
        );
        // Files-touched ledger: recompute cumulatively here (this L2 summary was
        // built on the runtime Value stream, which lacks the Conversation). Same
        // recompute as the disk path, so neither clobbers the other's ledger.
        let ledger = crate::chat::agent::file_ledger::build_for_boundary(
            conversation,
            &summary.source_until_message_id,
        );
        summary.file_ledger = (!ledger.is_empty()).then_some(ledger);
        conversation.context_state.last_compressed_at = Some(summary.created_at);
        conversation.context_state.compressed_message_count = summary.source_message_ids.len();
        conversation.context_state.compression_count = conversation
            .context_state
            .compression_count
            .saturating_add(1);
        conversation.context_state.summary = Some(summary);
        // R-4：多次链式压缩后提示准确性下降（与 compact_conversation 口径一致）。
        conversation.context_state.warning = crate::chat::agent::compaction::decay_warning_for(
            conversation.context_state.compression_count,
        );
    }
    let terminal_content = result.content.clone();
    let terminal_outcome = result.stream_outcome.clone();
    let run_plan_update = message_plan.clone();
    push_assistant_message(
        app,
        state,
        &settings,
        conversation,
        assistant_message_id,
        result.content,
        result.reasoning,
        artifacts,
        tool_records,
        result.api_messages,
        segments,
        skill_id.as_deref(),
        title_from_first_user,
        Some(run_entry),
        Some(result.stream_outcome.as_str()),
        result.usage,
        result.last_step_usage,
        message_plan,
        result.degraded,
    )
    .await?;
    if let Some(plan_state) = run_plan_update {
        crate::chat::protocol::emit_run_event(
            app,
            &run_id,
            crate::chat::protocol::ChatRunEvent::PlanUpdated {
                plan_state: (&plan_state).into(),
            },
        );
    }
    crate::chat::protocol::finish_run(
        app,
        &run_id,
        &terminal_outcome,
        &terminal_content,
        conversation.revision,
    );
    Ok(ArmReplyOutcome {
        message: None,
        run_id: None,
        error: None,
    })
}

pub(super) fn agent_run_entry_label(entry: crate::chat::agent::AgentRunEntry) -> &'static str {
    match entry {
        crate::chat::agent::AgentRunEntry::Send => "send",
        crate::chat::agent::AgentRunEntry::Regenerate => "regenerate",
    }
}

/// 把模型原生生成的图片（`GenerateOutput.images`，任务 07-24）转成 assistant 消息级
/// `ChatToolArtifact`：`data:{mime};base64,{data}` 内联，name `generated-image-{i}.{ext}`
/// （1 基）。复用 image_generation 的 `extension_for_mime` / `decoded_base64_len`，与 Mixer
/// 出图工具的 artifact 形态保持一致。空输入 → 空 Vec（无开销）。
fn generated_image_artifacts(
    images: &[crate::chat::model::GeneratedImageData],
) -> Vec<crate::mcp::types::ChatToolArtifact> {
    use crate::chat::image_generation::{decoded_base64_len, extension_for_mime};
    images
        .iter()
        .enumerate()
        .map(|(idx, image)| {
            let extension = extension_for_mime(&image.mime_type);
            crate::mcp::types::ChatToolArtifact {
                id: None,
                name: format!("generated-image-{}.{}", idx + 1, extension),
                mime_type: image.mime_type.clone(),
                data_url: format!("data:{};base64,{}", image.mime_type, image.data),
                size_bytes: decoded_base64_len(&image.data),
                path: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::model::GeneratedImageData;

    /// 任务 07-24：Gemini 原生出图 → assistant 消息级 artifacts。转换产出 data_url 前缀 +
    /// size_bytes，穿过 build_assistant_message 后落在 ChatMessage.artifacts。
    #[test]
    fn native_image_becomes_assistant_message_artifact() {
        let images = vec![GeneratedImageData {
            mime_type: "image/png".to_string(),
            // base64("hello") = "aGVsbG8=" → 解码 5 字节。
            data: "aGVsbG8=".to_string(),
        }];
        let artifacts = generated_image_artifacts(&images);

        let message = build_assistant_message(
            "msg_test".to_string(),
            "这是图片。".to_string(),
            None,
            artifacts,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            Some("send"),
            Some("completed"),
            None,
            None,
            None,
            None,
        );

        assert_eq!(message.artifacts.len(), 1);
        let artifact = &message.artifacts[0];
        assert_eq!(artifact.name, "generated-image-1.png");
        assert_eq!(artifact.mime_type, "image/png");
        assert!(artifact.data_url.starts_with("data:image/png;base64,"));
        assert!(artifact.data_url.ends_with("aGVsbG8="));
        assert_eq!(artifact.size_bytes, Some(5));
    }
}
