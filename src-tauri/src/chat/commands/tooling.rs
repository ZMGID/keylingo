use tauri::AppHandle;

use crate::chat::agent::prepare as agent_prepare;
use crate::mcp::{self, ChatToolDefinition};
use crate::settings::{SessionModel, Settings};
use crate::skills;
use crate::state::AppState;

/// Detect a leading `/skill <args>` slash trigger in a user message and, when it
/// matches an enabled skill, rewrite the message body to pin that skill.
///
/// Returns `(skill_id, rewritten_content)` on a match. The rewrite is
/// `"[Skill: name]\n\n{body}"` where `body` is the skill body with `$ARGUMENTS`
/// / `$ARG_NAME` substituted from the trailing words. The resolved id then flows
/// through the existing pin chain (`resolve_forced_skill_id` -> active Skill
/// catalog/prompt injection). Skill activation never changes enabled tools.
///
/// `disable_model_invocation` only gates *model* auto-invocation, so it is
/// intentionally ignored here — an explicit user slash command may still trigger
/// such a skill. Availability is gated by `skill_allowed_for_conversation`
/// (Settings enable list, connector prerequisites, assistant allow-list).
pub(super) fn try_apply_skill_slash_trigger(
    registry: &skills::SkillRegistry,
    chat_tools: &crate::settings::ChatToolsConfig,
    assistant_snapshot: Option<&crate::chat::types::ChatAssistantSnapshot>,
    content: &str,
    obsidian_vault_configured: bool,
) -> Option<(String, String)> {
    let trimmed = content.trim_start();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let first_word = parts.next().unwrap_or_default();
    if !first_word.starts_with('/') {
        return None;
    }
    let args_raw = parts.next().unwrap_or_default();

    let record = registry.find_by_trigger(first_word)?;
    if !agent_prepare::skill_allowed_for_conversation(
        chat_tools,
        assistant_snapshot,
        &record.meta.id,
        obsidian_vault_configured,
    ) {
        // A disabled or out-of-allow-list skill's slash command is left as ordinary text.
        return None;
    }
    if crate::mcp::native_registry::find_entry(first_word.trim_start_matches('/')).is_some() {
        // A skill id colliding with a built-in tool name would shadow it on the
        // backend trigger path. The front-end intercepts built-in slash commands
        // before send, so this is low risk — just note it.
        eprintln!(
            "[skill-slash] trigger {first_word} matches a built-in tool name; pinning skill {}",
            record.meta.id
        );
    }

    let rendered = skills::substitute_arguments(&record.body, args_raw, &record.meta.arguments);
    let rewritten = format!("[Skill: {}]\n\n{}", record.meta.name, rendered);
    Some((record.meta.id.clone(), rewritten))
}

pub(super) fn resolve_forced_skill_id(
    chat_tools: &crate::settings::ChatToolsConfig,
    assistant_snapshot: Option<&crate::chat::types::ChatAssistantSnapshot>,
    registry: &skills::SkillRegistry,
    requested: Option<&str>,
    obsidian_vault_configured: bool,
) -> Option<String> {
    let requested = requested.map(str::trim).filter(|id| !id.is_empty())?;
    let enabled = registry
        .records
        .iter()
        .filter(|record| {
            agent_prepare::skill_allowed_for_conversation(
                chat_tools,
                assistant_snapshot,
                &record.meta.id,
                obsidian_vault_configured,
            )
        })
        .any(|record| {
            record.meta.id == requested
                || record.meta.name == requested
                || skills::slugify(requested) == record.meta.id
        });
    if enabled {
        Some(requested.to_string())
    } else {
        None
    }
}

#[derive(Default)]
pub(super) struct ChatToolList {
    pub tools: Vec<ChatToolDefinition>,
    pub unavailable_mcp_servers: Vec<String>,
}

pub(super) async fn list_tools_for_chat(
    app: &AppHandle,
    state: &AppState,
    settings: &Settings,
    session: Option<SessionModel<'_>>,
) -> ChatToolList {
    if !(settings.chat_tools.enabled
        || crate::settings::chat_native_tools_enabled(&settings.chat_tools)
        || crate::settings::chat_memory_tools_enabled(settings)
        || crate::settings::chat_image_generation_enabled_for_session(settings, session)
        || settings.advisor_model().is_some())
    {
        return ChatToolList::default();
    }
    let catalog = mcp::registry::list_enabled_tool_catalog(app, state).await;
    let mut tools = catalog.tools;
    if let Some((provider_id, model)) =
        crate::chat::model_metadata::image_generation_model_for_session(settings, session)
    {
        if !tools.iter().any(|tool| tool.name == "mixer_generate_image") {
            let mut tool = mcp::types::mixer_generate_image_tool();
            let provider_name = settings
                .get_provider(&provider_id)
                .map(|provider| {
                    if provider.name.trim().is_empty() {
                        provider.id.clone()
                    } else {
                        provider.name.clone()
                    }
                })
                .unwrap_or(provider_id);
            tool.server_id = Some(format!("{provider_name} / {model}"));
            tools.push(tool);
        }
    }
    ChatToolList {
        tools,
        unavailable_mcp_servers: catalog.unavailable_mcp_servers,
    }
}

pub(super) fn append_agent_todo_tools(tools: &mut Vec<ChatToolDefinition>) -> bool {
    crate::chat::todo::append_tool_definitions(tools);
    true
}

pub(super) fn append_agent_ask_user_tools(tools: &mut Vec<ChatToolDefinition>) -> bool {
    crate::chat::ask_user::append_tool_definitions(tools);
    true
}

pub(super) fn apply_agent_plan_tool_filter(
    tools: &mut Vec<ChatToolDefinition>,
    plan_mode: bool,
) -> Vec<ChatToolDefinition> {
    if !plan_mode {
        return Vec::new();
    }
    let mut blocked = Vec::new();
    tools.retain(|tool| {
        let allowed = agent_plan_allows_tool(tool);
        if !allowed {
            blocked.push(tool.clone());
        }
        allowed
    });
    blocked
}

/// Chat mode: conversational research tools only — gated by `ChatModeConfig` toggles.
/// Blocks local fs mutation, shell, sub-agents, skills, todos, and write-capable MCP.
pub(super) fn apply_chat_mode_tool_filter(
    tools: &mut Vec<ChatToolDefinition>,
    chat_mode: bool,
    config: &crate::settings::ChatModeConfig,
) -> Vec<ChatToolDefinition> {
    if !chat_mode {
        return Vec::new();
    }
    let mut blocked = Vec::new();
    tools.retain(|tool| {
        let allowed = chat_mode_allows_tool(tool, config);
        if !allowed {
            blocked.push(tool.clone());
        }
        allowed
    });
    blocked
}

fn chat_mode_allows_tool(
    tool: &ChatToolDefinition,
    config: &crate::settings::ChatModeConfig,
) -> bool {
    if tool.source == "native" && crate::chat::ask_user::is_ask_user_tool_name(&tool.name) {
        return true;
    }
    if tool.source == "mcp" {
        return config.mcp_read_only && tool.is_read_only_tool();
    }
    if tool.source != "native" {
        return false;
    }
    match tool.name.as_str() {
        "web_search" | "search_web" => config.web_search,
        "web_fetch" => config.web_fetch,
        "knowledge_search" => config.knowledge_search,
        "memory_read" | "memory_search" => config.memory_tools,
        _ => false,
    }
}

fn agent_plan_allows_tool(tool: &ChatToolDefinition) -> bool {
    if tool.source == "native" && crate::chat::ask_user::is_ask_user_tool_name(&tool.name) {
        return true;
    }
    if tool.source == "native" && crate::chat::todo::is_agent_todo_tool_name(&tool.name) {
        return true;
    }
    if tool.source == "native" {
        return tool.is_read_only_tool();
    }
    if tool.source == "mcp" {
        return tool.is_read_only_tool();
    }
    tool.source == "skill" && tool.name == "skill"
}

/// 会话级三态联网搜索（任务 07-23）：按有效模式收敛第三方 `search_web` 工具的暴露。
/// - `ThirdParty`：保证 `search_web` 存在（列表因全局开关缺席时，若已配置搜索 key 则补回）。
/// - `Builtin` / `Off`：移除 `search_web`（内置走请求体注入，Off 完全不联网）。
///
/// 内置注入不在此处——由 `AgentRunConfig.builtin_web_search_active()` 经 `GenerateOptions`
/// 驱动各适配器；此函数只管客户端工具的去留。
pub(super) fn apply_web_search_mode_tool_filter(
    tools: &mut Vec<ChatToolDefinition>,
    mode: crate::chat::types::WebSearchMode,
    settings: &crate::settings::Settings,
) {
    const SEARCH_WEB_ID: &str = "native__web_search";
    match mode {
        crate::chat::types::WebSearchMode::ThirdParty => {
            let present = tools.iter().any(|t| t.id == SEARCH_WEB_ID);
            if !present && crate::mcp::registry::web_search_configured(settings) {
                tools.push(crate::mcp::types::native_web_search_tool());
            }
        }
        crate::chat::types::WebSearchMode::Builtin | crate::chat::types::WebSearchMode::Off => {
            tools.retain(|t| t.id != SEARCH_WEB_ID);
        }
    }
}

pub(super) fn apply_inline_code_request_tool_filter(
    tools: &mut Vec<ChatToolDefinition>,
    last_user_api_content: Option<&str>,
) {
    if !should_answer_inline_without_file_write(last_user_api_content) {
        return;
    }
    tools.retain(|tool| !(tool.source == "native" && tool.name == "write"));
}

pub(super) fn should_answer_inline_without_file_write(last_user_api_content: Option<&str>) -> bool {
    let Some(content) = last_user_api_content else {
        return false;
    };
    let user_text = content
        .split("[已添加附件]")
        .next()
        .unwrap_or(content)
        .trim();
    if user_text.is_empty() {
        return false;
    }
    let normalized = user_text.to_ascii_lowercase();
    if has_explicit_file_write_intent(user_text, &normalized) {
        return false;
    }
    has_inline_code_request_intent(user_text, &normalized)
}

fn has_explicit_file_write_intent(text: &str, normalized: &str) -> bool {
    const ZH_MARKERS: &[&str] = &[
        "保存",
        "写入",
        "写到",
        "写进",
        "输出到",
        "导出",
        "创建文件",
        "生成文件",
        "另存为",
        "存成",
        "落盘",
    ];
    const EN_MARKERS: &[&str] = &[
        "save",
        "create file",
        "output file",
        "output to",
        "export",
        "save as",
        "write to",
        "file named",
    ];
    ZH_MARKERS.iter().any(|marker| text.contains(marker))
        || EN_MARKERS.iter().any(|marker| normalized.contains(marker))
}

fn has_inline_code_request_intent(text: &str, normalized: &str) -> bool {
    const ZH_MARKERS: &[&str] = &["```", "代码块", "代码框", "围栏代码"];
    const EN_MARKERS: &[&str] = &["```", "code block", "fenced code"];
    ZH_MARKERS.iter().any(|marker| text.contains(marker))
        || EN_MARKERS.iter().any(|marker| normalized.contains(marker))
}

#[cfg(test)]
mod web_search_filter_tests {
    use super::*;
    use crate::chat::types::WebSearchMode;

    #[test]
    fn off_and_builtin_remove_search_web() {
        let settings = crate::settings::Settings::default();
        for mode in [WebSearchMode::Off, WebSearchMode::Builtin] {
            let mut tools = vec![
                crate::mcp::types::native_web_search_tool(),
                crate::mcp::types::native_web_fetch_tool(),
            ];
            apply_web_search_mode_tool_filter(&mut tools, mode, &settings);
            assert!(
                !tools.iter().any(|t| t.id == "native__web_search"),
                "search_web should be stripped in {mode:?}"
            );
            assert!(
                tools.iter().any(|t| t.id == "native__web_fetch"),
                "unrelated tools must survive in {mode:?}"
            );
        }
    }

    #[test]
    fn third_party_keeps_existing_search_web() {
        let settings = crate::settings::Settings::default();
        let mut tools = vec![crate::mcp::types::native_web_search_tool()];
        apply_web_search_mode_tool_filter(&mut tools, WebSearchMode::ThirdParty, &settings);
        assert!(tools.iter().any(|t| t.id == "native__web_search"));
    }
}
