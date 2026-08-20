use serde_json::Value;

use crate::chat::model::WORKBENCH_LOCATION_PROMPT_HEAD;
use crate::chat::types::{ChatAssistantSnapshot, ContextUsageSegment};
use crate::mcp::ChatToolDefinition;
use crate::settings::{
    chat_no_think_instruction, default_chat_system_prompt, ChatToolsConfig,
};
use crate::skills;

pub fn chat_tools_capable(
    chat_tools: &ChatToolsConfig,
    memory_enabled: bool,
    image_generation_enabled: bool,
) -> bool {
    chat_tools.enabled
        || crate::settings::chat_native_tools_enabled(chat_tools)
        || memory_enabled
        || image_generation_enabled
}

/// Apply the active assistant snapshot's explicit MCP server policy.
/// Native and Skill tools are unaffected; `None` means unrestricted, while an
/// empty `mcp_server_ids` list intentionally disables every MCP tool.
pub fn apply_assistant_mcp_restrictions(
    tools: &mut Vec<ChatToolDefinition>,
    assistant_snapshot: Option<&ChatAssistantSnapshot>,
) {
    let Some(assistant) = assistant_snapshot else {
        return;
    };
    tools.retain(|tool| {
        if tool.source != "mcp" {
            return true;
        }
        match tool.server_id.as_deref() {
            Some(server_id) => assistant.mcp_server_ids.iter().any(|id| id == server_id),
            None => false,
        }
    });
}

/// 某技能在当前对话是否可用：全局已启用 **且**（无助手 = 不限；有助手 = 在其 skill_ids 白名单内）。
/// 空 skill_ids = 该助手不可用任何技能。
pub fn skill_allowed_for_conversation(
    chat_tools: &crate::settings::ChatToolsConfig,
    assistant_snapshot: Option<&ChatAssistantSnapshot>,
    skill_id: &str,
    obsidian_vault_configured: bool,
) -> bool {
    if !crate::settings::skill_globally_available(
        chat_tools,
        skill_id,
        obsidian_vault_configured,
    ) {
        return false;
    }
    match assistant_snapshot {
        Some(assistant) => assistant.skill_ids.iter().any(|id| id == skill_id),
        None => true,
    }
}

pub fn apply_skill_fallback_when_tools_unavailable(
    chat_tools: &mut ChatToolsConfig,
    active_skill_id: Option<&str>,
    tools_available: bool,
) {
    if !tools_available
        && active_skill_id
            .map(|id| !id.trim().is_empty())
            .unwrap_or(false)
        && chat_tools.skill_fallback_mode == "progressive"
    {
        chat_tools.skill_fallback_mode = "skill_md_only".to_string();
    }
}

pub fn available_builtin_tool_names(tools: &[ChatToolDefinition]) -> Vec<String> {
    let mut names = tools
        .iter()
        .filter(|tool| is_kivio_builtin_tool(tool))
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

pub fn disabled_builtin_tool_feedback(function_name: &str) -> Option<String> {
    // Builtin name set = static native registry (17 native + todo/ask_user)
    // plus the non-native builtin sources listed here.
    const EXTRA_BUILTIN_NAMES: &[&str] = &["mixer_generate_image"];
    // 模型按 wire 名（保留名别名）调用——反查回内部名再比对注册表。
    let function_name = crate::mcp::types::resolve_reserved_wire_alias(function_name);
    let is_builtin = crate::mcp::native_registry::find_entry(function_name).is_some()
        || EXTRA_BUILTIN_NAMES.contains(&function_name);
    if is_builtin {
        Some(format!(
            "Kivio tool `{function_name}` is not enabled for this chat. Do not call it again; answer using the available context and enabled tools only."
        ))
    } else {
        None
    }
}

pub fn is_native_skill_tool_name(name: &str) -> bool {
    // 兼容旧名 skill_activate（现规整为 skill）。
    matches!(name, "skill" | "skill_activate")
}

pub fn is_kivio_builtin_tool(tool: &ChatToolDefinition) -> bool {
    matches!(tool.source.as_str(), "native" | "mixer")
        && !is_native_skill_tool_name(&tool.name)
        && !crate::chat::todo::is_agent_todo_tool_name(&tool.name)
}

pub fn builtin_tool_bypasses_approval(tool: &ChatToolDefinition) -> bool {
    if tool.source == "skill" && is_native_skill_tool_name(&tool.name) {
        return true;
    }
    tool.source == "native"
        && crate::mcp::native_registry::find_entry(&tool.name)
            .is_some_and(|entry| entry.bypasses_approval)
}

/// True for the native file/shell tools gated by one-time per-conversation
/// session consent (read/write/edit/bash/grep/glob). See
/// `native_registry::native_tool_requires_session_consent`.
pub fn tool_requires_session_consent(tool: &ChatToolDefinition) -> bool {
    tool.source == "native"
        && crate::mcp::native_registry::native_tool_requires_session_consent(&tool.name)
}

pub fn build_chat_system_prompt(
    language: &str,
    has_image: bool,
    thinking_enabled: bool,
    registry: &skills::SkillRegistry,
    chat_tools: &ChatToolsConfig,
    tools_available: bool,
    available_builtin_tools: &[String],
    active_skill_id: Option<&str>,
    active_skill_detail: Option<&skills::SkillDetail>,
    assistant_snapshot: Option<&ChatAssistantSnapshot>,
    set_system_prompt: Option<&str>,
    custom_system_prompt: &str,
    is_chat_runtime: bool,
    memory_prompt: Option<&str>,
    agent_plan_prompt: Option<&str>,
    agent_ask_user_prompt: Option<&str>,
    agent_todo_prompt: Option<&str>,
    project_context: Option<&ProjectPromptContext>,
    workbench_dir: Option<&str>,
    knowledge_base_prompt: Option<&str>,
    obsidian_vault_path: Option<&str>,
) -> String {
    build_chat_system_prompt_with_segments(
        language,
        has_image,
        thinking_enabled,
        registry,
        chat_tools,
        tools_available,
        available_builtin_tools,
        active_skill_id,
        active_skill_detail,
        assistant_snapshot,
        set_system_prompt,
        custom_system_prompt,
        is_chat_runtime,
        memory_prompt,
        agent_plan_prompt,
        agent_ask_user_prompt,
        agent_todo_prompt,
        project_context,
        workbench_dir,
        knowledge_base_prompt,
        obsidian_vault_path,
    )
    .0
}

/// Chat vs Agent identity sources. Chat must not inherit the Agent system
/// prompt or the Act/Plan/Orchestrate overlay — settings already keeps those
/// fields separate (`chat.systemPrompt` vs `chat.chatMode.systemPrompt`).
pub struct RuntimePromptSources {
    pub custom_system_prompt: String,
    pub agent_plan_prompt: Option<String>,
    pub is_chat_runtime: bool,
}

pub fn resolve_runtime_prompt_sources(
    chat_mode: bool,
    agent_system_prompt: &str,
    chat_mode_system_prompt: &str,
    plan_state: &crate::chat::types::AgentPlanState,
) -> RuntimePromptSources {
    if chat_mode {
        RuntimePromptSources {
            custom_system_prompt: chat_mode_system_prompt.trim().to_string(),
            agent_plan_prompt: None,
            is_chat_runtime: true,
        }
    } else {
        RuntimePromptSources {
            custom_system_prompt: agent_system_prompt.to_string(),
            agent_plan_prompt: Some(crate::chat::plan::format_prompt(plan_state)),
            is_chat_runtime: false,
        }
    }
}

/// Project binding facts injected into the system prompt so the model knows
/// the default path base before generating file tool arguments.
#[derive(Debug, Clone)]
pub struct ProjectPromptContext {
    pub name: String,
    pub root_path: Option<String>,
}

const CHAT_WORK_STYLE: &str = "Be concise. Address only the current request — no filler preamble, no \"here's what I'll do next\". Match length to the task.";

const CHAT_ASK_USER_PROMPT: &str = "Use ask_user when a preference or A/B choice would block this reply. Do not list options in assistant text for the user to type back.";

const CHAT_TOOLS_RUNTIME: &str = "When a request needs a live fact, a URL, a knowledge-base passage, or memory, call the matching enabled tool. Only claim a tool was used after Kivio returns a tool result. Answer date questions from the system date above without tools.";

fn work_style_prompt(available_builtin_tools: &[String]) -> String {
    let can_edit_files = available_builtin_tools
        .iter()
        .any(|tool| matches!(tool.as_str(), "write" | "edit" | "bash"));
    let file_clause = if can_edit_files {
        " after editing files you don't need to restate what changed (the user can see it)."
    } else {
        ""
    };
    format!(
        "How you work: address only the current request — no filler preamble, no wrap-up postamble, no \"here's what I'll do next\" narration;{file_clause} Match length to the task: answer simple questions in a sentence or two, and expand into structured output only for complex or report-style tasks — don't pad to look thorough. When the user only asks how to do something or whether it's possible, answer first; don't jump to making changes, and don't do work they didn't ask for."
    )
}

fn project_context_prompt(project: &ProjectPromptContext) -> String {
    match &project.root_path {
        // Do not interpolate the folder path here. It changes per project (and
        // Chat Probe rebinds the same project to a new cwd) and would sit in
        // the static system prefix, breaking tool-schema cache. The absolute
        // path lives only in the trailing workbench paragraph, which is peeled
        // onto the first user message.
        Some(_) => format!(
            "This is a project conversation. Project \"{}\" is bound to a folder. Relative paths in file/command tools resolve from the current default workbench (that project root); writing an explicit absolute or ~/ path (e.g. ~/Desktop/x.html) targets that global location outside the project.",
            project.name
        ),
        None => format!(
            "This is a project conversation, but project \"{}\" has no bound folder; file/command tools are unavailable until the user binds one from the project menu.",
            project.name
        ),
    }
}

pub fn build_chat_system_prompt_with_segments(
    language: &str,
    has_image: bool,
    thinking_enabled: bool,
    registry: &skills::SkillRegistry,
    chat_tools: &ChatToolsConfig,
    tools_available: bool,
    available_builtin_tools: &[String],
    active_skill_id: Option<&str>,
    active_skill_detail: Option<&skills::SkillDetail>,
    assistant_snapshot: Option<&ChatAssistantSnapshot>,
    set_system_prompt: Option<&str>,
    custom_system_prompt: &str,
    is_chat_runtime: bool,
    memory_prompt: Option<&str>,
    agent_plan_prompt: Option<&str>,
    agent_ask_user_prompt: Option<&str>,
    agent_todo_prompt: Option<&str>,
    project_context: Option<&ProjectPromptContext>,
    workbench_dir: Option<&str>,
    knowledge_base_prompt: Option<&str>,
    obsidian_vault_path: Option<&str>,
) -> (String, Vec<ContextUsageSegment>) {
    let mut prompt = String::new();
    let mut segments = Vec::new();
    if is_chat_runtime {
        // 自定义人设叠在合同之上，不替换。合同放在专家/集之后，避免被后写指令盖掉。
        if !custom_system_prompt.trim().is_empty() {
            append_context_segment(
                &mut prompt,
                &mut segments,
                "system_prompt",
                "System prompt",
                &format!(
                    "Additional instructions:\n{}",
                    custom_system_prompt.trim()
                ),
            );
        }
        append_context_segment(
            &mut prompt,
            &mut segments,
            "system_prompt",
            "System prompt",
            CHAT_WORK_STYLE,
        );
    } else {
        let base_prompt = if custom_system_prompt.trim().is_empty() {
            default_chat_system_prompt(has_image)
        } else {
            custom_system_prompt.trim().to_string()
        };
        append_context_segment(
            &mut prompt,
            &mut segments,
            "system_prompt",
            "System prompt",
            &base_prompt,
        );
        // 工作方式纪律（始终附加，独立于可被自定义人设覆盖的基座）。
        // 改文件那句只在真有 write/edit/bash 时出现。
        append_context_segment(
            &mut prompt,
            &mut segments,
            "system_prompt",
            "System prompt",
            &work_style_prompt(available_builtin_tools),
        );
    }
    if let Some(assistant) = assistant_snapshot {
        let assistant_prompt = assistant_prompt_segment(assistant);
        if !assistant_prompt.trim().is_empty() {
            append_context_segment(
                &mut prompt,
                &mut segments,
                "assistant",
                "Assistant",
                &assistant_prompt,
            );
        }
    }
    // 集的系统提示词：实时注入（不冻结），随集编辑对集内所有对话立即生效。作为独立段落，
    // 与助手段并存（助手段提供人设/工具白名单，集段是这一组对话的统一指令）。
    // 带与助手相同的标题，避免原文被埋进超长 system prompt 后模型当旁注忽略。
    if let Some(set_prompt) = set_system_prompt {
        let set_prompt = set_prompt.trim();
        if !set_prompt.is_empty() {
            append_context_segment(
                &mut prompt,
                &mut segments,
                "set",
                "Set instructions",
                &format!("Set instructions:\n{set_prompt}"),
            );
        }
    }
    if is_chat_runtime {
        let has_knowledge_search = available_builtin_tools
            .iter()
            .any(|tool| tool.as_str() == "knowledge_search");
        let mut contract = crate::chat::plan::chat_capability_contract(has_knowledge_search);
        if has_image {
            contract.push_str(" You can use images the user provides.");
        }
        append_context_segment(
            &mut prompt,
            &mut segments,
            "system_prompt",
            "System prompt",
            &contract,
        );
    }
    append_context_segment(
        &mut prompt,
        &mut segments,
        "runtime_context",
        "Runtime context",
        &crate::settings::chat_current_datetime_context(language),
    );

    if !is_chat_runtime {
        if let Some(project) = project_context {
            append_context_segment(
                &mut prompt,
                &mut segments,
                "runtime_context",
                "Runtime context",
                &project_context_prompt(project),
            );
        }
    }

    if let Some(memory) = memory_prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        append_context_segment(
            &mut prompt,
            &mut segments,
            "memory_l1",
            "Memory / L1",
            memory,
        );
    }

    if let Some(kb) = knowledge_base_prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        append_context_segment(
            &mut prompt,
            &mut segments,
            "knowledge_base",
            "Knowledge base",
            kb,
        );
    }

    // Chat 没有文件/shell/skill 激活：这些段会教模型去 list_dir / run_command，
    // 和「写文件 / Shell 只在 Agent 可用」矛盾，故整段跳过。
    if !is_chat_runtime {
        if let Some(path) = obsidian_vault_path
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let text = format!(
                "Obsidian vault path: {path}\n\
                 This is a local Obsidian markdown vault. Use the native file tools: \
                 list_dir to browse (entries include modified time), glob_files to find *.md by name, \
                 search_files to search by content/keyword, read_file to read a note; \
                 notes cross-reference each other via [[wikilink]].\n\
                 For Obsidian syntax or file-format details, activate the obsidian-markdown / \
                 obsidian-bases / json-canvas / obsidian-cli skills."
            );
            append_context_segment(
                &mut prompt,
                &mut segments,
                "runtime_context",
                "Runtime context",
                &text,
            );
        }

        // 能力插件：仅「已安装且启用」时注入短 systemHint；关闭则零注入。
        if let Some(text) = crate::plugins::enabled_system_prompt()
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            append_context_segment(
                &mut prompt,
                &mut segments,
                "runtime_context",
                "Runtime context",
                text,
            );
        }
    }

    if let Some(plan) = agent_plan_prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        append_context_segment(&mut prompt, &mut segments, "agent_plan", "Agent plan", plan);
    }

    if is_chat_runtime {
        if available_builtin_tools
            .iter()
            .any(|tool| tool.as_str() == crate::chat::ask_user::ASK_USER_TOOL_NAME)
        {
            append_context_segment(
                &mut prompt,
                &mut segments,
                "agent_ask_user",
                "Agent ask_user",
                CHAT_ASK_USER_PROMPT,
            );
        }
    } else if let Some(ask_user) = agent_ask_user_prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        append_context_segment(
            &mut prompt,
            &mut segments,
            "agent_ask_user",
            "Agent ask_user",
            ask_user,
        );
    }

    if let Some(todo) = agent_todo_prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        append_context_segment(&mut prompt, &mut segments, "agent_todo", "Agent todo", todo);
    }

    // Per-conversation workbench path (`…/conv_xxx`) is the only system-prompt
    // bit that changes between ordinary chats. Compute it once, inject the
    // static tool rules without it, then append the path as the last paragraph
    // so prefix-cache matching can share role / L1 / skills / tool schemas
    // across conversations. `generate_request_from_openai_messages` peels this
    // paragraph off `system` and parks it on the first user message (after tools
    // in the token stream).
    let workbench_text = if tools_available {
        workbench_location_prompt(workbench_dir, available_builtin_tools)
    } else {
        None
    };

    if tools_available {
        if is_chat_runtime {
            append_context_segment(
                &mut prompt,
                &mut segments,
                "runtime_context",
                "Runtime context",
                CHAT_TOOLS_RUNTIME,
            );
        } else {
            let mut action_examples = Vec::new();
            if available_builtin_tools
                .iter()
                .any(|tool| tool.as_str() == crate::chat::ask_user::ASK_USER_TOOL_NAME)
            {
                action_examples.push("asking the user a blocking clarification");
            }
            if available_builtin_tools
                .iter()
                .any(|tool| matches!(tool.as_str(), "read" | "grep" | "glob"))
            {
                action_examples.push("reading or searching project files");
            }
            if available_builtin_tools
                .iter()
                .any(|tool| tool.as_str() == "bash")
            {
                action_examples.push("running code or a command");
            }
            if available_builtin_tools
                .iter()
                .any(|tool| matches!(tool.as_str(), "web_search" | "web_fetch"))
            {
                action_examples.push("using the web");
            }
            if available_builtin_tools
                .iter()
                .any(|tool| tool.as_str() == "mixer_generate_image")
            {
                action_examples.push("generating an image");
            }
            if action_examples.is_empty() {
                action_examples.push("using an enabled tool");
            }
            let mut runtime = format!(
                "You have access to tools (functions). When the user's request requires action—such as {}—YOU MUST call the appropriate enabled tool instead of describing what to do. Never say \"I cannot run commands\" or \"you can do it yourself\" when an enabled tool is available for that action. Do not call tools that are not listed as enabled.",
                action_examples.join(", ")
            );
            runtime.push_str(
                " Only claim that a tool was used, a script was run, a file was read, or the web was searched after Kivio returns an actual tool result in the conversation.",
            );
            runtime.push_str(
                " If the user only asks for today/tomorrow/weekday derivable from the system date above, answer directly without calling tools.",
            );
            append_context_segment(
                &mut prompt,
                &mut segments,
                "runtime_context",
                "Runtime context",
                &runtime,
            );
        }
        if let Some(native_prompt) =
            native_tools_prompt(available_builtin_tools, workbench_text.is_some())
        {
            append_context_segment(
                &mut prompt,
                &mut segments,
                "native_tools",
                "Native tools",
                &native_prompt,
            );
        }
        // Sub-agent delegation rules — only when the `agent` spawn tool is
        // available. The `agent` call is BLOCKING + single-result (Claude Code
        // Task model); to run sub-agents in parallel, emit MULTIPLE `agent` calls
        // in ONE message — they execute concurrently and each returns its result.
        // No polling/collection tool exists. Concise on purpose.
        if available_builtin_tools
            .iter()
            .any(|tool| tool.as_str() == crate::chat::sub_agent::AGENT_TOOL_NAME)
        {
            let background_prompt =
                "Delegating to sub-agents: each agent call BLOCKS, waits for the sub-agent to finish, and returns its full result directly. To run sub-agents in PARALLEL, emit MULTIPLE agent tool calls in a SINGLE message — they execute concurrently and each returns its own result. There is no polling or collection tool; do not look for one.";
            append_context_segment(
                &mut prompt,
                &mut segments,
                "native_tools",
                "Native tools",
                background_prompt,
            );
            // Roles are data, not code: the available ones are listed in the
            // `agent` tool's `subagent_type` description, and a new permanent
            // role is just a `.md` file the model can write with its own tools.
            let agents_dir = crate::app_data::app_data_dir()
                .map(|dir| dir.join("agents").display().to_string())
                .unwrap_or_else(|| "<app_data>/agents".to_string());
            let roles_prompt = format!(
                "Sub-agent roles: the roles available right now are listed in the agent tool's subagent_type description. For a one-off role, skip subagent_type and pass system_prompt/tools inline. To add a PERMANENT role, write a Markdown file to {agents_dir} with YAML frontmatter (name, description, tools, disallowedTools, skills, model) whose body is the role's system prompt; tools/disallowedTools accept `mcp__<server>__*`, `mcp__*` and `*`, and disallowedTools is applied before tools."
            );
            append_context_segment(
                &mut prompt,
                &mut segments,
                "native_tools",
                "Native tools",
                &roles_prompt,
            );
        }
        // 工作目录卫生只在真能写文件/跑命令时有意义；Chat 检索工具集不要这段。
        let needs_hygiene = available_builtin_tools.iter().any(|tool| {
            matches!(tool.as_str(), "write" | "edit" | "bash")
        });
        if needs_hygiene {
            let tool_hygiene = "Working directory hygiene:\n\
- Keep disposable batch/job descriptor JSONs, review screenshots, and scratch drafts in the system temp directory rather than cluttering the default workbench.\n\
- Before finishing a multi-step task, delete intermediate files you created so the workbench keeps only useful outputs.\n\
- When passing file paths to MCP tools (stdio servers), always use absolute paths; the server's working directory is unpredictable.";
            append_context_segment(
                &mut prompt,
                &mut segments,
                "native_tools",
                "Native tools",
                tool_hygiene,
            );
        }
    }

    // Chat 不能激活 skill / 改工作区，目录和「去调 skill 工具」会直接教错。
    let include_catalog = !is_chat_runtime
        && (chat_tools.skill_auto_match
            || active_skill_id.is_some()
            || chat_tools.skill_fallback_mode != "legacy_full_body");
    if include_catalog {
        let obsidian_vault_configured = obsidian_vault_path
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
        let catalog =
            skills::format_catalog(registry, active_skill_id, tools_available, |skill_id| {
                skill_allowed_for_conversation(
                    chat_tools,
                    assistant_snapshot,
                    skill_id,
                    obsidian_vault_configured,
                )
            });
        if !catalog.is_empty() {
            append_context_segment(&mut prompt, &mut segments, "skills", "Skills", &catalog);
        }
    }

    if !is_chat_runtime {
        if !chat_tools.skill_auto_match {
            append_context_segment(
                &mut prompt,
                &mut segments,
                "skills",
                "Skills",
                "Only activate skills that are enabled in Settings (listed in the catalog below).",
            );
        }

        let fallback = chat_tools.skill_fallback_mode.as_str();
        if let Some(skill_id) = active_skill_id.filter(|id| !id.trim().is_empty()) {
            let mut skill_prompt = format!("User pinned skill for this message: {skill_id}");
            if tools_available {
                skill_prompt.push_str(
                    ". Activate it with the skill tool to load its full instructions for this message.",
                );
            } else if matches!(fallback, "skill_md_only" | "legacy_full_body") {
                skill_prompt.push_str(". Follow the Active Skill instructions below.");
            } else {
                skill_prompt.push_str(
                    ". Progressive skill loading requires tool support; switch provider or set fallback to SKILL.md only.",
                );
            }
            append_context_segment(
                &mut prompt,
                &mut segments,
                "skills",
                "Skills",
                &skill_prompt,
            );
        } else if tools_available && chat_tools.skill_auto_match {
            append_context_segment(
                &mut prompt,
                &mut segments,
                "skills",
                "Skills",
                "When the task matches a skill's description, call the skill tool for it proactively — you don't need the user to name it; a description match is enough. Activating loads that skill's full step-by-step instructions, which beat improvising. Only skip a skill whose description clearly doesn't fit the current task.",
            );
        }

        if matches!(fallback, "skill_md_only" | "legacy_full_body") {
            if let Some(skill) = active_skill_detail {
                if !skill.body.trim().is_empty() {
                    append_context_segment(
                        &mut prompt,
                        &mut segments,
                        "skills",
                        "Skills",
                        &format!("Active Skill:\n{}", skill.body),
                    );
                }
            }
        }
    }

    if !thinking_enabled && !tools_available {
        append_context_segment(
            &mut prompt,
            &mut segments,
            "runtime_context",
            "Runtime context",
            chat_no_think_instruction(),
        );
    }
    if let Some(text) = workbench_text.as_deref() {
        append_context_segment(
            &mut prompt,
            &mut segments,
            "native_tools",
            "Native tools",
            text,
        );
    }
    (prompt, merge_context_segments(segments))
}

fn append_context_segment(
    prompt: &mut String,
    segments: &mut Vec<ContextUsageSegment>,
    id: &str,
    label: &str,
    content: &str,
) {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return;
    }
    if !prompt.is_empty() {
        prompt.push_str("\n\n");
    }
    prompt.push_str(trimmed);
    segments.push(ContextUsageSegment {
        id: id.to_string(),
        label: label.to_string(),
        estimated_tokens: estimate_tokens(trimmed),
        color: context_segment_color(id).map(str::to_string),
    });
}

fn assistant_prompt_segment(assistant: &ChatAssistantSnapshot) -> String {
    let mut parts = vec![format!("Active assistant: {}", assistant.name)];
    if !assistant.description.trim().is_empty() {
        parts.push(format!(
            "Assistant purpose: {}",
            assistant.description.trim()
        ));
    }
    let assistant_system_prompt = assistant.system_prompt.trim();
    if !assistant_system_prompt.is_empty() {
        parts.push(format!(
            "Assistant instructions:\n{assistant_system_prompt}"
        ));
    }
    parts.join("\n\n")
}

pub fn merge_context_segments(segments: Vec<ContextUsageSegment>) -> Vec<ContextUsageSegment> {
    let mut merged: Vec<ContextUsageSegment> = Vec::new();
    for segment in segments {
        if segment.estimated_tokens == 0 {
            continue;
        }
        if let Some(existing) = merged.iter_mut().find(|item| item.id == segment.id) {
            existing.estimated_tokens += segment.estimated_tokens;
        } else {
            merged.push(segment);
        }
    }
    merged
}

pub fn context_segment_color(id: &str) -> Option<&'static str> {
    match id {
        "system_prompt" => Some("#7A7A7A"),
        "assistant" => Some("#8A6FBD"),
        "set" => Some("#5C9A8B"),
        "runtime_context" => Some("#3E8B60"),
        "memory_l1" => Some("#4F9A9A"),
        "agent_plan" => Some("#8A724C"),
        "agent_todo" => Some("#5F7C5A"),
        "tool_definitions" => Some("#7553CF"),
        "skills" => Some("#BD8A3E"),
        "mcp" => Some("#B04B8D"),
        "native_tools" => Some("#4E7FB8"),
        "summarized_conversation" => Some("#BF3F66"),
        "conversation" => Some("#D07652"),
        "attachments" => Some("#6A8FBD"),
        _ => None,
    }
}

pub fn estimate_tokens(text: &str) -> usize {
    let mut ascii = 0usize;
    let mut non_ascii = 0usize;
    for ch in text.chars() {
        if ch.is_ascii() {
            ascii += 1;
        } else {
            non_ascii += 1;
        }
    }
    ascii.div_ceil(4) + non_ascii
}

/// content-part `type` 值：图片部件（估算记 0 token——图片按 provider 的 tile 计费，
/// 而非 base64 体积；把 base64 长度算进 token 会把估算打爆几个数量级）。
/// **务必保持 0**：上下文用量条（`compute_context_state`）已用
/// `estimate_image_attachment_tokens`（按图片真实尺寸/tile）**另行**累加图片 token，
/// `count_tokens_in_value` 委托本函数、对内联图片返回 0 正是为了**不重复计**。
/// 若在此给图片一个非 0 常量，用量条会双重计数；而 L2 循环内估算对内联图片的欠计
/// 由 auto 触发路径（usage_ratio 已含图片）兜住，无需在此 hedge。
pub(crate) const IMAGE_PART_TYPES: [&str; 3] = ["image_url", "input_image", "image"];
/// content-part `type` 值：文本部件（按其 `text` 字段估算）。
pub(crate) const TEXT_PART_TYPES: [&str; 2] = ["text", "input_text"];

/// 估算任意 `Value`（含多模态数组 content）的 token 数。**图片部件记 0**、文本部件按文本、
/// 对象按 key+value 递归、字符串按 `estimate_tokens`。压缩侧（estimate_message_tokens /
/// serialize）与上下文用量条（commands.rs::count_tokens_in_value 委托本函数）**共用同一口径**，
/// 防止 base64 图片把 token 估算打爆导致徒劳压缩 / anti-thrashing 误收尾。
pub(crate) fn estimate_value_tokens(value: &Value) -> usize {
    match value {
        Value::String(text) => estimate_tokens(text),
        Value::Array(items) => items.iter().map(estimate_value_tokens).sum(),
        Value::Object(map) => {
            if let Some(kind) = map.get("type").and_then(Value::as_str) {
                if IMAGE_PART_TYPES.contains(&kind) {
                    return 0;
                }
                if TEXT_PART_TYPES.contains(&kind) {
                    return map.get("text").map(estimate_value_tokens).unwrap_or(0);
                }
            }
            map.iter()
                .map(|(key, value)| estimate_tokens(key) + estimate_value_tokens(value))
                .sum()
        }
        _ => estimate_tokens(&value.to_string()),
    }
}

pub(crate) fn tool_matches_recommended_name(tool: &ChatToolDefinition, recommended: &str) -> bool {
    let recommended = recommended.trim();
    if recommended.is_empty() {
        return false;
    }
    // 旧名归一化：persona/skill 白名单里写的旧工具名（find/ls/todo_update/list_background）
    // 规整到现名，避免改名后被静默剔除。
    let recommended = crate::mcp::types::canonical_tool_name(recommended);
    tool.name == recommended
        || tool.id == recommended
        || tool.openai_tool_name() == recommended
        || tool
            .server_id
            .as_deref()
            .map(|server_id| format!("{server_id}:{}", tool.name) == recommended)
            .unwrap_or(false)
}

fn workbench_location_prompt(
    workbench_dir: Option<&str>,
    available_builtin_tools: &[String],
) -> Option<String> {
    let has = |name: &str| available_builtin_tools.iter().any(|tool| tool.as_str() == name);
    let dir = workbench_dir.map(str::trim).filter(|dir| {
        !dir.is_empty() && (has("write") || has("edit") || has("bash"))
    })?;
    Some(format!(
        "{WORKBENCH_LOCATION_PROMPT_HEAD} `{dir}`. When the user does not specify a location, use relative paths or the default cwd so files and basic work land here. This is NOT a sandbox or access restriction: if the user names Desktop, an absolute path, `~/...`, or another directory, use that exact location instead."
    ))
}

fn native_tools_prompt(
    available_builtin_tools: &[String],
    _has_workbench: bool,
) -> Option<String> {
    let native_tool_names = available_builtin_tools
        .iter()
        .filter(|tool| tool.as_str() != crate::chat::ask_user::ASK_USER_TOOL_NAME)
        .cloned()
        .collect::<Vec<_>>();
    if native_tool_names.is_empty() {
        return None;
    }
    // 提示词展示 wire 名（保留名规避后的别名）：模型必须按请求里声明的函数名调用，
    // 提示词与 tools 声明不一致会诱发未知工具调用。逻辑判断仍用内部名。
    let list = native_tool_names
        .iter()
        .map(|name| crate::mcp::types::apply_reserved_wire_alias(name))
        .collect::<Vec<_>>()
        .join(", ");
    let has = |name: &str| native_tool_names.iter().any(|tool| tool.as_str() == name);
    let has_web_search = has("web_search");
    let has_web_fetch = has("web_fetch");
    let has_image_generation = has("mixer_generate_image");
    let has_advisor = has("advisor");
    let has_present_artifacts = has("present_artifacts");
    let has_write = has("write");
    let has_edit = has("edit");
    let has_bash = has("bash");
    let has_memory = has("memory_read") || has("memory_search") || has("memory_modify");
    let has_file_cwd = has_write
        || has_edit
        || has_bash
        || has("read")
        || has("grep")
        || has("glob");
    let has_host_side_effects = has_write || has_edit || has_bash;

    let mut bullets: Vec<String> = Vec::new();
    if has_file_cwd {
        bullets.push(
            "Relative file paths and omitted command cwd resolve from the current default workbench (the bound project root for project conversations, or the per-conversation workbench otherwise). Explicit absolute or ~/ paths remain unrestricted and always take precedence.".to_string(),
        );
    }
    if has_write || has_edit {
        bullets.push(
            "Touch files only when the user explicitly asks to save/modify/delete local files or gives a target path: edit for small edits, write for new files or whole-file overwrites. If asked for a code block without saving, answer inline. After a write, state the path briefly; do not repeat the file content.".to_string(),
        );
    }
    if has_write || has_edit || has_bash || has_memory {
        bullets.push(if has_write || has_edit || has_bash {
            if has_memory {
                "Write/edit tools and bash may need user approval; memory_read (L2 on demand; L1 is auto-injected), memory_search (keyword search over L2; prefer it when you are unsure of the exact heading), and memory_modify do not.".to_string()
            } else {
                "Write/edit tools and bash may need user approval.".to_string()
            }
        } else {
            "memory_read (L2 on demand; L1 is auto-injected) and memory_search (keyword search over L2; prefer it when you are unsure of the exact heading) do not require approval.".to_string()
        });
    }
    if has_bash {
        // 运行时取值,让同一份 prompt 在不同平台都说真话。Windows 上 bash 实际选哪个
        // shell 是运行期探测的(见 native_tools::find_git_bash / run_command_shell_hint),
        // 这里用同一个探测结果分支措辞,保证系统提示词与 run_command 工具描述(R4,
        // mcp/types.rs::native_run_command_tool)永远一致——不会出现提示词说 PowerShell、
        // 工具描述说 Git Bash 的自相矛盾。
        let windows_git_bash =
            cfg!(target_os = "windows") && !crate::native_tools::run_command_shell_hint().is_empty();
        let (os_name, shell_name) = if windows_git_bash {
            ("Windows", "Git Bash")
        } else if cfg!(target_os = "windows") {
            ("Windows", "PowerShell")
        } else if cfg!(target_os = "macos") {
            ("macOS", "sh")
        } else {
            ("Linux", "sh")
        };
        let shell_syntax_hint = if windows_git_bash {
            "Windows via Git Bash: use bash syntax (pipes, heredoc, `$VAR`, `$(seq ...)`), NOT PowerShell cmdlets; write Windows paths with forward slashes (C:/Users/...) — backslashes are escape characters in bash"
        } else if cfg!(target_os = "windows") {
            "Windows is PowerShell: use full cmdlet names like `Get-ChildItem`/`Get-Content`, environment variables as `$env:VAR`, chain commands with `;`, do NOT use the removed `wmic`, and do NOT `-Recurse` the whole drive"
        } else {
            "Unix: `$VAR`, `ls`, `/`"
        };
        bullets.push(format!(
            "Runtime environment: {os_name}; bash runs via {shell_name}. Match that shell's syntax ({shell_syntax_hint}). Each bash call is a fresh process — cwd does NOT persist across calls; switch directories with the `cwd` parameter, not a prior `cd`. To run multi-line or quoted code, write it to a file with write and run that — do not cram it into inline commands like `python -c \"...\"` (inline quotes are fragile across shells). When a tool returns a hard rejection, change strategy instead of retrying variants of the same action; never re-run a failed command unchanged; don't drop one-off probe or cleanup scripts into the project."
        ));
        bullets.push(
            "bash runs on the host shell from the current default workbench; non-zero exit means failure. Paths with spaces must use the `cwd` parameter—never `cd path && command`; do not combine `cwd` with a leading `cd ... &&` prefix. Long-running dev commands such as `npm run dev`, `tauri dev`, and `vite` start in the background automatically and return a job_id immediately; do not start the same dev server twice. Explain and get confirmation before destructive, network, or environment-changing commands. Run a skill's bundled scripts with run_command; never use host pip unless the user explicitly asked for a host Python install.".to_string(),
        );
        bullets.push(
            "Background commands (bash with background:true, or auto-detected dev servers): the call returns a job_id immediately and hands control back to you — keep working, do NOT poll right away. Read incremental output and exit status with bash_output (pass the job_id; use the returned next_offset for the next read), list all tracked jobs by calling bash_output with no job_id, and stop one with kill_background. Keep polling bounded (≤20 checks); status in history may be stale, so refresh once with bash_output before reporting a background command's result. Background commands survive across turns until you kill them or the app exits, so kill_background a dev server when you no longer need it.".to_string(),
        );
    }
    if has_web_search || has_web_fetch {
        let live_access_hint = match (has_web_search, has_web_fetch, has_host_side_effects) {
            (true, true, true) => {
                "Use search_web/web_fetch or the relevant Skill script for live web/API access."
            }
            (true, true, false) => "Use search_web/web_fetch for live web/API access.",
            (true, false, true) => {
                "Use search_web or the relevant Skill script for live web/API access."
            }
            (true, false, false) => "Use search_web for live web/API access.",
            (false, true, true) => {
                "Use web_fetch or the relevant Skill script for web page access."
            }
            (false, true, false) => "Use web_fetch for web page access.",
            (false, false, _) => unreachable!(),
        };
        bullets.push(live_access_hint.to_string());
    }
    if has_present_artifacts {
        bullets.push(
            "When the user asks to show, preview, attach, or send a local file or image in the chat, you MUST call present_artifacts at the exact display point. Use artifact_ids for generated files and paths for existing local files. Reading or analyzing a file does NOT display it.".to_string(),
        );
    }
    if has_image_generation {
        bullets.push(
            "When the user asks to create, generate, or draw an image, call mixer_generate_image; do not merely describe it.".to_string(),
        );
    }
    if has_advisor {
        bullets.push(
            "A stronger advisor model is available via the advisor tool. Consult it when you are stuck, have failed the same approach repeatedly, or face a significant design/architecture decision — pass a specific question plus the relevant context. Do not call it for routine steps you can handle yourself.".to_string(),
        );
    }
    if has_write || has_edit || has_bash {
        bullets.push(
            "Before changing code, read neighboring files and existing conventions — mimic the current style, naming, and the libraries/frameworks already in use; never assume a library is available without confirming the project already uses it. Do not add code comments unless asked. After code changes, verify when you can (run existing tests, lint/typecheck); never git commit/push unless the user explicitly asks. Reference code locations as `file_path:line_number`. When several independent lookups or commands are needed, call multiple tools in parallel in one message instead of serially.".to_string(),
        );
    }

    let mut prompt = format!("Built-in tools enabled: {list}. Only call tools in this list.");
    for bullet in bullets {
        prompt.push_str("\n- ");
        prompt.push_str(&bullet);
    }
    Some(prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_assistant_snapshot(
        mcp_server_ids: Vec<&str>,
        skill_ids: Vec<&str>,
    ) -> ChatAssistantSnapshot {
        ChatAssistantSnapshot {
            id: "asst_test".to_string(),
            name: "Test Assistant".to_string(),
            description: String::new(),
            source: "user".to_string(),
            system_prompt: String::new(),
            provider_id: String::new(),
            model: String::new(),
            mcp_server_ids: mcp_server_ids.into_iter().map(str::to_string).collect(),
            skill_ids: skill_ids.into_iter().map(str::to_string).collect(),
        }
    }

    fn test_mcp_tool() -> ChatToolDefinition {
        ChatToolDefinition {
            id: "mcp__demo__search".to_string(),
            name: "search".to_string(),
            description: "Search demo".to_string(),
            source: "mcp".to_string(),
            server_id: Some("demo".to_string()),
            server_name: Some("Demo".to_string()),
            input_schema: serde_json::json!({ "type": "object", "properties": {} }),
            sensitive: false,
            annotations: None,
            output_schema: None,
        }
    }

    #[test]
    fn is_native_skill_tool_name_matches_runtime_tools() {
        assert!(is_native_skill_tool_name("skill"));
        assert!(is_native_skill_tool_name("skill_activate")); // 旧名兼容
        assert!(!is_native_skill_tool_name("web_search"));
    }

    #[test]
    fn chat_prompt_omits_disabled_web_tools() {
        let registry = skills::SkillRegistry::default();
        let mut chat_tools = crate::settings::ChatToolsConfig::default();
        chat_tools.native_tools.skill_runtime = true;
        chat_tools.native_tools.run_command = true;
        chat_tools.native_tools.web_search = false;
        chat_tools.native_tools.web_fetch = false;

        let prompt = build_chat_system_prompt(
            "zh-CN",
            false,
            false,
            &registry,
            &chat_tools,
            true,
            &["bash".to_string()],
            None,
            None,
            None,
            None,
            "",
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );

        assert!(prompt.contains("bash"));
        assert!(!prompt.contains("web_search"));
        assert!(!prompt.contains("web_fetch"));
    }

    #[test]
    fn chat_prompt_surfaces_default_workbench_without_confinement() {
        let registry = skills::SkillRegistry::default();
        let mut chat_tools = crate::settings::ChatToolsConfig::default();
        chat_tools.native_tools.write_file = true;

        let prompt = build_chat_system_prompt(
            "zh-CN",
            false,
            false,
            &registry,
            &chat_tools,
            true,
            &["write".to_string()],
            None,
            None,
            None,
            None,
            "",
            false,
            None,
            None,
            None,
            None,
            None,
            Some("/Users/me/Kivio/workspace/conv_abc"),
            None,
            None,
        );

        // Workbench + write: surface the absolute workbench path
        // and keep explicit external paths allowed.
        assert!(prompt.contains("/Users/me/Kivio/workspace/conv_abc"));
        assert!(prompt.contains("Current default workbench"));
        assert!(prompt.contains("NOT a sandbox or access restriction"));
        assert!(prompt.contains("use that exact location instead"));
        assert!(!prompt.contains("run_python"));
        // The removed deliver_file tool must not appear anywhere.
        assert!(!prompt.contains("deliver_file"));
        // Per-conversation path must sit after the static tool/skill rules so
        // prefix cache can share those across conversations.
        let path_at = prompt
            .find("/Users/me/Kivio/workspace/conv_abc")
            .expect("workbench path");
        assert!(
            prompt
                .find("Built-in tools enabled")
                .expect("native tool list")
                < path_at
        );
        assert!(
            prompt
                .find("Working directory hygiene")
                .expect("hygiene rules")
                < path_at
        );
        let last = prompt
            .trim()
            .rsplit("\n\n")
            .next()
            .expect("last paragraph");
        assert!(
            last.starts_with(WORKBENCH_LOCATION_PROMPT_HEAD),
            "workbench path must be the last system-prompt paragraph, got: {last}"
        );
    }

    #[test]
    fn project_folder_path_stays_out_of_static_system_prefix() {
        let registry = skills::SkillRegistry::default();
        let mut chat_tools = crate::settings::ChatToolsConfig::default();
        chat_tools.native_tools.write_file = true;
        let tools = ["write".to_string()];
        let build = |root: &str| {
            let project = ProjectPromptContext {
                name: "Chat Probe".to_string(),
                root_path: Some(root.to_string()),
            };
            build_chat_system_prompt(
                "zh-CN",
                false,
                false,
                &registry,
                &chat_tools,
                true,
                &tools,
                None,
                None,
                None,
                None,
                "",
                false,
                None,
                None,
                None,
                None,
                Some(&project),
                Some(root),
                None,
                None,
            )
        };

        let prompt_a = build("/tmp/workbench-alpha");
        let prompt_b = build("/tmp/workbench-beta");
        assert!(prompt_a.contains("/tmp/workbench-alpha"));
        assert!(prompt_b.contains("/tmp/workbench-beta"));
        assert!(
            !prompt_a.contains("bound to folder:"),
            "project paragraph must not embed the absolute path"
        );

        let idx_a = prompt_a
            .rfind("\n\n")
            .expect("workbench paragraph separator");
        let idx_b = prompt_b
            .rfind("\n\n")
            .expect("workbench paragraph separator");
        let (prefix_a, suffix_a) = (&prompt_a[..idx_a], &prompt_a[idx_a + 2..]);
        let (prefix_b, suffix_b) = (&prompt_b[..idx_b], &prompt_b[idx_b + 2..]);
        assert_eq!(
            prefix_a, prefix_b,
            "static system prefix must not change when only the project folder path changes"
        );
        assert!(suffix_a.starts_with(WORKBENCH_LOCATION_PROMPT_HEAD));
        assert!(suffix_a.contains("/tmp/workbench-alpha"));
        assert!(!prefix_a.contains("/tmp/workbench-alpha"));
        assert!(!prefix_a.contains("/tmp/workbench-beta"));
        assert!(suffix_b.contains("/tmp/workbench-beta"));
        assert_ne!(suffix_a, suffix_b);
    }

    #[test]
    fn chat_prompt_requires_present_artifacts_for_explicit_display() {
        let registry = skills::SkillRegistry::default();
        let chat_tools = crate::settings::ChatToolsConfig::default();

        let prompt = build_chat_system_prompt(
            "zh-CN",
            false,
            false,
            &registry,
            &chat_tools,
            true,
            &["present_artifacts".to_string(), "read".to_string()],
            None,
            None,
            None,
            None,
            "",
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );

        assert!(prompt.contains("MUST call present_artifacts"));
        assert!(prompt.contains("paths for existing local files"));
        assert!(prompt.contains("Reading or analyzing a file does NOT display it"));
    }

    #[test]
    fn chat_prompt_prevents_write_file_for_inline_code_requests() {
        let registry = skills::SkillRegistry::default();
        let mut chat_tools = crate::settings::ChatToolsConfig::default();
        chat_tools.native_tools.write_file = true;

        let prompt = build_chat_system_prompt(
            "zh-CN",
            false,
            false,
            &registry,
            &chat_tools,
            true,
            &["write".to_string()],
            None,
            None,
            None,
            None,
            "",
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );

        assert!(prompt.contains("code block"));
        assert!(prompt.contains("answer inline"));
        assert!(prompt.contains("do not repeat the file content"));
    }

    #[test]
    fn chat_prompt_includes_obsidian_vault_path() {
        let registry = skills::SkillRegistry::default();
        let chat_tools = crate::settings::ChatToolsConfig::default();

        let prompt = build_chat_system_prompt(
            "zh-CN",
            false,
            false,
            &registry,
            &chat_tools,
            false,
            &[],
            None,
            None,
            None,
            None,
            "",
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("/Users/me/Obsidian/MyVault"),
        );

        assert!(prompt.contains("Obsidian vault path: /Users/me/Obsidian/MyVault"));
    }

    #[test]
    fn chat_prompt_labels_live_set_instructions() {
        let registry = skills::SkillRegistry::default();
        let chat_tools = crate::settings::ChatToolsConfig::default();

        let prompt = build_chat_system_prompt(
            "zh-CN",
            false,
            false,
            &registry,
            &chat_tools,
            false,
            &[],
            None,
            None,
            None,
            Some("Always answer in 文言文."),
            "",
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );

        assert!(
            prompt.contains("Set instructions:\nAlways answer in 文言文."),
            "{prompt}"
        );

        let blank = build_chat_system_prompt(
            "zh-CN",
            false,
            false,
            &registry,
            &chat_tools,
            false,
            &[],
            None,
            None,
            None,
            Some("   "),
            "",
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(!blank.contains("Set instructions:"), "{blank}");
    }

    #[test]
    fn assistant_mcp_restrictions_keep_only_allowed_servers() {
        let assistant = test_assistant_snapshot(vec!["demo"], vec![]);
        let mut other = test_mcp_tool();
        other.server_id = Some("other".to_string());
        let mut tools = vec![
            crate::mcp::types::native_skill_activate_tool(),
            crate::mcp::types::native_web_fetch_tool(),
            test_mcp_tool(), // server_id = "demo"
            other,           // server_id = "other"
        ];

        apply_assistant_mcp_restrictions(&mut tools, Some(&assistant));

        // 原生工具保留,只有 allow-list 内的 MCP 工具保留。
        assert!(tools.iter().any(|t| t.name == "skill"));
        assert!(tools.iter().any(|t| t.name == "web_fetch"));
        assert_eq!(tools.iter().filter(|t| t.source == "mcp").count(), 1);
        assert!(tools
            .iter()
            .any(|t| t.source == "mcp" && t.server_id.as_deref() == Some("demo")));
    }

    #[test]
    fn assistant_empty_mcp_list_drops_all_mcp_tools() {
        let assistant = test_assistant_snapshot(vec![], vec![]);
        let mut tools = vec![crate::mcp::types::native_web_fetch_tool(), test_mcp_tool()];

        apply_assistant_mcp_restrictions(&mut tools, Some(&assistant));

        assert!(tools.iter().all(|t| t.source != "mcp"));
        assert!(tools.iter().any(|t| t.name == "web_fetch"));
    }

    #[test]
    fn no_assistant_does_not_restrict_mcp() {
        let mut tools = vec![test_mcp_tool()];
        apply_assistant_mcp_restrictions(&mut tools, None);
        assert_eq!(tools.len(), 1);
    }

    #[test]
    fn skill_allowed_respects_assistant_allow_list() {
        let chat_tools = crate::settings::ChatToolsConfig::default(); // 默认无禁用技能
        let assistant = test_assistant_snapshot(vec![], vec!["doc"]);

        assert!(skill_allowed_for_conversation(
            &chat_tools,
            Some(&assistant),
            "doc",
            false,
        ));
        // 不在白名单内的技能被拒。
        assert!(!skill_allowed_for_conversation(
            &chat_tools,
            Some(&assistant),
            "pdf",
            false,
        ));
        // 无助手 = 不限(只看全局 enable)。
        assert!(skill_allowed_for_conversation(
            &chat_tools,
            None,
            "pdf",
            false
        ));
    }

    #[test]
    fn skill_allowed_hides_obsidian_skill_without_vault() {
        let chat_tools = crate::settings::ChatToolsConfig::default();
        // No vault → Obsidian skills are unavailable at the conversation level.
        assert!(!skill_allowed_for_conversation(
            &chat_tools,
            None,
            "obsidian-markdown",
            false,
        ));
        // Vault configured → available.
        assert!(skill_allowed_for_conversation(
            &chat_tools,
            None,
            "obsidian-markdown",
            true,
        ));
    }

    #[test]
    fn skill_fallback_switches_to_markdown_when_assistant_disables_tools() {
        let mut chat_tools = crate::settings::ChatToolsConfig::default();

        apply_skill_fallback_when_tools_unavailable(&mut chat_tools, Some("doc"), false);

        assert_eq!(chat_tools.skill_fallback_mode, "skill_md_only");
    }

    #[test]
    fn disabled_builtin_tool_feedback_is_hidden_model_feedback() {
        let feedback = disabled_builtin_tool_feedback("web_search")
            .expect("disabled builtin tools should produce model feedback");

        assert!(feedback.contains("not enabled"));
        assert!(feedback.contains("web_search"));
        assert!(disabled_builtin_tool_feedback("mcp__server__tool").is_none());
        // 模型按 wire 别名调用时同样识别为内置工具（保留名规避）。
        let alias_feedback = disabled_builtin_tool_feedback("search_web")
            .expect("wire alias resolves to the builtin tool");
        assert!(alias_feedback.contains("not enabled"));
    }

    #[test]
    fn native_tools_prompt_renders_wire_alias_for_web_search() {
        // 提示词必须展示 wire 名（search_web）——与 tools 声明一致，否则模型会调用
        // 未声明的 web_search（且该名会被 Cursor 系上游吞掉）。
        let names = vec!["web_fetch".to_string(), "web_search".to_string()];
        let prompt = native_tools_prompt(&names, false).expect("prompt");
        assert!(prompt.contains("search_web"), "{prompt}");
        assert!(!prompt.contains("web_search"), "{prompt}");
    }

    #[test]
    fn native_tools_prompt_omits_shell_essay_for_research_tools() {
        let names = vec![
            "web_search".to_string(),
            "web_fetch".to_string(),
            "knowledge_search".to_string(),
            "memory_read".to_string(),
            "memory_search".to_string(),
        ];
        let prompt = native_tools_prompt(&names, false).expect("prompt");
        assert!(prompt.contains("search_web"), "{prompt}");
        assert!(prompt.contains("web_fetch"), "{prompt}");
        assert!(prompt.contains("knowledge_search"), "{prompt}");
        assert!(prompt.contains("memory_read"), "{prompt}");
        assert!(!prompt.contains("bash runs"), "{prompt}");
        assert!(!prompt.contains("Git Bash"), "{prompt}");
        assert!(!prompt.contains("run_python"), "{prompt}");
        assert!(!prompt.contains("Touch files"), "{prompt}");
        assert!(!prompt.contains("npm run dev"), "{prompt}");
        assert!(!prompt.contains("Skill script"), "{prompt}");
    }

    #[test]
    fn chat_runtime_uses_chat_prompt_not_agent_identity() {
        let registry = skills::SkillRegistry::default();
        let chat_tools = crate::settings::ChatToolsConfig::default();
        let research_tools = [
            "web_search".to_string(),
            "web_fetch".to_string(),
            "knowledge_search".to_string(),
            "memory_read".to_string(),
            "memory_search".to_string(),
        ];
        let prompt = build_chat_system_prompt(
            "zh-CN",
            false,
            true,
            &registry,
            &chat_tools,
            true,
            &research_tools,
            None,
            None,
            None,
            None,
            "",
            true,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );

        assert!(prompt.contains("Kivio Chat"), "{prompt}");
        assert!(prompt.contains("conversational research assistant"), "{prompt}");
        assert!(prompt.contains("These limits override"), "{prompt}");
        assert!(prompt.contains("cite sources with [n]"), "{prompt}");
        assert!(!prompt.contains("internal runtime mode"), "{prompt}");
        assert!(!prompt.contains("I cannot run commands"), "{prompt}");
        assert!(
            !prompt.contains("run code for calculations, edit files"),
            "{prompt}"
        );
        assert!(!prompt.contains("after editing files"), "{prompt}");
        assert!(!prompt.contains("bash runs"), "{prompt}");
        assert!(!prompt.contains("Git Bash"), "{prompt}");
        assert!(!prompt.contains("run_python"), "{prompt}");
        assert!(!prompt.contains("<available_skills>"), "{prompt}");
        assert!(!prompt.contains("Agent plan"), "{prompt}");
    }

    #[test]
    fn chat_runtime_stacks_custom_and_drops_kb_cite_when_tool_off() {
        let registry = skills::SkillRegistry::default();
        let chat_tools = crate::settings::ChatToolsConfig::default();
        let prompt = build_chat_system_prompt(
            "zh-CN",
            false,
            true,
            &registry,
            &chat_tools,
            true,
            &["web_search".to_string(), "web_fetch".to_string()],
            None,
            None,
            None,
            None,
            "Speak like a careful editor.",
            true,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(prompt.contains("Additional instructions:"), "{prompt}");
        assert!(prompt.contains("Speak like a careful editor."), "{prompt}");
        assert!(prompt.contains("These limits override"), "{prompt}");
        assert!(!prompt.contains("cite sources with [n]"), "{prompt}");
    }

    #[test]
    fn chat_runtime_injects_knowledge_base_prompt() {
        let registry = skills::SkillRegistry::default();
        let chat_tools = crate::settings::ChatToolsConfig::default();
        let prompt = build_chat_system_prompt(
            "zh-CN",
            false,
            true,
            &registry,
            &chat_tools,
            true,
            &["knowledge_search".to_string()],
            None,
            None,
            None,
            None,
            "",
            true,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("This conversation has knowledge bases attached: Docs."),
            None,
        );
        assert!(
            prompt.contains("This conversation has knowledge bases attached: Docs."),
            "{prompt}"
        );
    }

    #[test]
    fn resolve_runtime_prompt_sources_keeps_chat_and_agent_apart() {
        let plan = crate::chat::types::AgentPlanState::default();
        let chat = resolve_runtime_prompt_sources(true, "AGENT IDENTITY", "  ", &plan);
        assert!(chat.is_chat_runtime);
        assert!(chat.custom_system_prompt.is_empty());
        assert!(chat.agent_plan_prompt.is_none());

        let chat_custom =
            resolve_runtime_prompt_sources(true, "AGENT IDENTITY", "only research", &plan);
        assert_eq!(chat_custom.custom_system_prompt, "only research");
        assert!(chat_custom.agent_plan_prompt.is_none());

        let agent = resolve_runtime_prompt_sources(false, "AGENT IDENTITY", "only research", &plan);
        assert!(!agent.is_chat_runtime);
        assert_eq!(agent.custom_system_prompt, "AGENT IDENTITY");
        assert!(agent
            .agent_plan_prompt
            .is_some_and(|text| text.contains("act") || text.contains("plan")));
    }

    #[test]
    fn native_tools_prompt_gates_code_discipline_on_file_or_bash_tools() {
        // 代码工作纪律只在具备 write/edit/bash 时注入；纯只读/无这些工具时不出现，
        // 避免污染纯聊天场景。
        let with_bash = vec!["bash".to_string(), "read".to_string()];
        let p = native_tools_prompt(&with_bash, false).expect("prompt");
        assert!(
            p.contains("file_path:line_number"),
            "bash present should add discipline: {p}"
        );

        // 只有只读工具（无 write/edit/bash）时不注入。
        let read_only = vec!["read".to_string(), "glob".to_string()];
        let p2 = native_tools_prompt(&read_only, false).expect("prompt");
        assert!(
            !p2.contains("file_path:line_number"),
            "read-only should omit discipline: {p2}"
        );
    }

    #[test]
    fn estimate_tokens_counts_ascii_and_cjk() {
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens("你好ab"), 3);
    }
}
