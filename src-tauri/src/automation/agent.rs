//! Isolated `run_agent_loop` host for `action.agent` nodes.
//! Auto-approves tools (unattended schedule/hotkey cannot prompt). Does not
//! write a chat conversation.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::chat::agent::{
    run_agent_loop, AgentHost, AgentHostFuture, AgentRunConfig, ToolExecutionContext, ToolExecutor,
    ToolExecutorFuture,
};
use crate::chat::agent::prepare::{available_builtin_tool_names, build_chat_system_prompt};
use crate::chat::ask_user::{AskUserPromptPayload, AskUserResponseResult};
use crate::chat::types::{ChatMessageSegment, ToolCallRecord, WebSearchMode};
use crate::mcp::ChatToolDefinition;
use crate::skills;
use crate::state::AppState;

use super::types::NodeOutput;
use super::workspace;

struct WorkflowAgentHost {
    app: AppHandle,
    text: Mutex<String>,
}

impl AgentHost for WorkflowAgentHost {
    fn emit_stream_delta(
        &self,
        _conversation_id: &str,
        _run_id: &str,
        _message_id: &str,
        delta: &str,
        _reasoning_delta: Option<&str>,
        _segment: Option<&ChatMessageSegment>,
    ) {
        if !delta.is_empty() {
            if let Ok(mut guard) = self.text.lock() {
                guard.push_str(delta);
            }
        }
    }

    fn emit_tool_record(
        &self,
        _conversation_id: &str,
        _run_id: &str,
        _message_id: &str,
        _record: &ToolCallRecord,
    ) {
    }

    fn request_tool_approval<'a>(
        &'a self,
        _ctx: &'a ToolExecutionContext<'a>,
        _record: &'a ToolCallRecord,
    ) -> AgentHostFuture<'a, bool> {
        Box::pin(async { true })
    }

    fn request_session_consent<'a>(
        &'a self,
        _ctx: &'a ToolExecutionContext<'a>,
    ) -> AgentHostFuture<'a, bool> {
        Box::pin(async { true })
    }

    fn request_user_response<'a>(
        &'a self,
        _ctx: &'a ToolExecutionContext<'a>,
        _record: &'a ToolCallRecord,
        _prompt: AskUserPromptPayload,
    ) -> AgentHostFuture<'a, AskUserResponseResult> {
        Box::pin(async {
            AskUserResponseResult {
                phase: "cancelled".to_string(),
                answers: HashMap::new(),
            }
        })
    }

    fn is_generation_active(&self, conversation_id: &str, generation: u64) -> bool {
        self.app
            .state::<AppState>()
            .is_chat_generation_active(conversation_id, generation)
    }

    fn wait_for_generation_inactive<'a>(
        &'a self,
        conversation_id: &'a str,
        generation: u64,
    ) -> AgentHostFuture<'a, ()> {
        Box::pin(async move {
            loop {
                if !self.is_generation_active(conversation_id, generation) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
    }
}

struct WorkflowToolExecutor {
    app: AppHandle,
}

impl ToolExecutor for WorkflowToolExecutor {
    fn call<'a>(
        &'a self,
        ctx: &'a ToolExecutionContext<'a>,
        tool: &'a ChatToolDefinition,
        arguments: Value,
        skill_cache: Option<&'a mut skills::SkillRunCache>,
    ) -> ToolExecutorFuture<'a> {
        Box::pin(async move {
            let native_ctx = crate::mcp::registry::NativeToolContext {
                conversation_id: ctx.tool_conversation_id.to_string(),
                message_id: ctx.message_id.to_string(),
                tool_call_id: Some(ctx.tool_call_id.to_string()),
                run_id: ctx.run_id.to_string(),
                generation: ctx.generation,
                depth: ctx.depth,
            };
            crate::mcp::registry::call_tool(
                &self.app,
                &self.app.state::<AppState>(),
                tool,
                arguments,
                skill_cache,
                Some(native_ctx),
            )
            .await
        })
    }
}

pub(crate) async fn run_agent_node(
    app: &AppHandle,
    automation_id: &str,
    run_id: &str,
    prompt: &str,
    skill_id: Option<&str>,
) -> Result<NodeOutput, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("Agent prompt is empty".to_string());
    }

    let state = app.state::<AppState>();
    let state: &AppState = &state;
    let settings = state.settings_read().clone();
    let (provider_id, model) = settings.effective_chat_model();
    let provider = settings
        .get_provider(&provider_id)
        .filter(|p| p.enabled && !p.api_keys.is_empty())
        .cloned()
        .ok_or_else(|| {
            "Configure a chat provider and model in Settings before running an Agent step"
                .to_string()
        })?;

    let conversation_id = workspace::conversation_id(automation_id);
    state.grant_chat_consent(&conversation_id);
    let generation = state.next_chat_generation(&conversation_id);
    let message_id = format!("auto-msg-{run_id}");

    let catalog = crate::mcp::registry::list_enabled_tool_catalog(app, state).await;
    let tools = catalog.tools;
    let builtin_names = available_builtin_tool_names(&tools);

    let workdir = workspace::workbench_dir(
        &settings.chat_tools.native_tools.working_directory,
        automation_id,
    );
    let workdir_str = workdir
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned());
    let registry = skills::build_registry_in(
        app,
        &settings.chat_tools.skill_scan_paths,
        workdir.as_deref(),
    )
    .unwrap_or_default();
    let active_skill_detail = skill_id.and_then(|id| {
        skills::read_skill_detail_in(
            app,
            &settings.chat_tools.skill_scan_paths,
            id,
            workdir.as_deref(),
        )
        .ok()
    });

    let mut effective_chat_tools = settings.chat_tools.clone();
    effective_chat_tools.approval_policy = "auto".to_string();
    let tools_available = crate::chat::agent::prepare::chat_tools_capable(
        &effective_chat_tools,
        settings.chat_memory.enabled,
        settings.image_generation_model().is_some(),
    );
    let language = settings
        .settings_language
        .clone()
        .unwrap_or_else(|| "zh".to_string());
    let custom_system_prompt = settings.chat.system_prompt.clone();
    let obsidian_vault_path = (!settings.obsidian_vault_path.trim().is_empty())
        .then_some(settings.obsidian_vault_path.as_str());
    let additional_directories: [crate::chat::types::AdditionalDirectory; 0] = [];
    let system_prompt = build_chat_system_prompt(
        &language,
        false,
        true,
        &registry,
        &effective_chat_tools,
        tools_available,
        &builtin_names,
        skill_id,
        active_skill_detail.as_ref(),
        None,
        None,
        &custom_system_prompt,
        false,
        None,
        None,
        None,
        None,
        None,
        workdir_str.as_deref(),
        None,
        obsidian_vault_path,
        &additional_directories,
    );

    let runtime_messages = vec![
        serde_json::json!({ "role": "system", "content": system_prompt }),
        serde_json::json!({ "role": "user", "content": prompt }),
    ];

    let host = WorkflowAgentHost {
        app: app.clone(),
        text: Mutex::new(String::new()),
    };
    let executor = WorkflowToolExecutor { app: app.clone() };
    let retry_attempts = if settings.retry_enabled {
        settings.retry_attempts as usize
    } else {
        1
    };
    let max_output_tokens = settings.chat.max_output_tokens;
    let web_search_mode = WebSearchMode::resolve(None, &settings);

    let config = AgentRunConfig {
        state,
        conversation_id: conversation_id.clone(),
        tool_conversation_id: conversation_id.clone(),
        depth: 0,
        run_id: format!("auto-run-{run_id}"),
        message_id,
        generation,
        provider,
        model,
        runtime_messages,
        tools,
        blocked_tool_calls: Vec::new(),
        settings,
        effective_chat_tools,
        language,
        thinking_enabled: true,
        thinking_level: None,
        web_search_mode,
        max_output_tokens,
        retry_attempts,
        assistant_snapshot: None,
        provider_tools_fallback_system_prompt: system_prompt,
        initial_anchor_total_tokens: None,
        initial_anchor_trailing_estimate: 0,
        skill_project_cwd: workdir,
    };

    let outcome = run_agent_loop(config, &host, &executor).await;
    state.end_chat_generation(&conversation_id, generation);
    match outcome {
        Ok(result) => {
            let text = if result.content.trim().is_empty() {
                host.text.lock().ok().map(|g| g.clone()).unwrap_or_default()
            } else {
                result.content
            };
            if text.trim().is_empty() {
                Err("Agent returned an empty response".to_string())
            } else {
                Ok(NodeOutput::from_text(text))
            }
        }
        Err(err) if err == "cancelled" => Err("cancelled".to_string()),
        Err(err) => Err(err),
    }
}
