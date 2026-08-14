use crate::chat::types::{AgentPlanMode, AgentPlanState, AgentPlanStatus};

pub fn mode_from_str(value: &str) -> Result<AgentPlanMode, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "act" => Ok(AgentPlanMode::Act),
        "plan" => Ok(AgentPlanMode::Plan),
        "orchestrate" => Ok(AgentPlanMode::Orchestrate),
        other => Err(format!("Unknown agent plan mode: {other}")),
    }
}

pub fn is_plan_mode(state: &AgentPlanState) -> bool {
    state.mode == AgentPlanMode::Plan
}

pub fn is_orchestrate_mode(state: &AgentPlanState) -> bool {
    state.mode == AgentPlanMode::Orchestrate
}

/// System-prompt section for the dedicated Kivio Chat runtime (not an agent plan mode).
/// Kept in English like other internal runtime sections (`format_prompt` Act/Plan/Orchestrate).
pub fn chat_runtime_prompt() -> String {
    "Chat runtime (internal runtime mode): this conversation uses Kivio Chat, not the full agent. You are a conversational assistant with light research tools only. Prefer clear, direct answers. You may use web search, web_fetch, knowledge_search, ask_user, memory_read/memory_search, and read-only MCP tools when they improve factual accuracy or cite the user's knowledge bases. Do not edit files, run shell commands, spawn sub-agents, activate skills that mutate the workspace, modify memory, or claim to have performed side-effecting work. If the user needs implementation, multi-file changes, or terminal work, say they should switch to Kivio Agent (Act / Plan / Orchestrate). When you use knowledge_search, cite sources with [n] markers.".to_string()
}

/// Same text for all languages — settings preview and runtime share one English source.
pub fn chat_runtime_prompt_for_lang(_language: &str) -> String {
    chat_runtime_prompt()
}

pub fn with_mode(current: &AgentPlanState, mode: AgentPlanMode) -> AgentPlanState {
    let mut next = current.clone();
    if next.mode != mode {
        next.mode = mode;
        next.updated_at = chrono::Local::now().timestamp();
    }
    next
}

pub fn approve(current: &AgentPlanState) -> AgentPlanState {
    let mut next = current.clone();
    next.mode = AgentPlanMode::Act;
    next.status = if executable_plan_text(current).is_some() {
        AgentPlanStatus::Approved
    } else {
        AgentPlanStatus::Empty
    };
    next.updated_at = chrono::Local::now().timestamp();
    next
}

pub fn capture_draft_from_reply(current: &AgentPlanState, content: &str) -> AgentPlanState {
    let plan = content.trim();
    if !is_executable_plan_text(plan) {
        return current.clone();
    }
    AgentPlanState {
        mode: AgentPlanMode::Plan,
        status: AgentPlanStatus::Draft,
        plan: Some(plan.to_string()),
        updated_at: chrono::Local::now().timestamp(),
    }
}

pub fn format_prompt(state: &AgentPlanState) -> String {
    let status = status_name(&state.status);
    let current_plan = current_plan_text(state)
        .map(|plan| plan.to_string())
        .unwrap_or_else(|| "No current saved plan.".to_string());

    if state.mode == AgentPlanMode::Plan {
        format!(
            "Agent plan mode (internal runtime mode): current mode is plan and status is {status}. Plan mode is read-only: research, read, search, and analyze before producing a plan. Do not perform or claim side-effecting work such as editing files, running commands, mutating memory, or implementing changes unless Kivio returned an actual tool result. Ask clarifying questions when needed.\n\nInvestigate first (required): before writing the plan, you MUST use the read-only tools to understand the current state — do not guess or conclude from just one or two files. Investigate systematically along these dimensions, and back every claim with a file/code you actually read (cite the source path):\n- Current state: how the relevant feature/module works today and where its entry points are;\n- Scope: which files/functions this change will touch, plus their upstream/downstream callers;\n- Existing conventions: the naming, patterns, error handling, and testing style already used in this area — new code must align;\n- External references (required unless this is a purely internal, trivial change): whenever the task involves an external standard/protocol, a third-party library/framework API, or any architecture decision, you MUST use web_search to check how official docs and representative open-source projects do it — their architecture, flow, conventions, and pitfalls — then web_fetch to read the key pages. Do not conclude from memory or prior knowledge alone. Confirm the established industry approach before planning rather than building blindly (skip only if the web search tool is genuinely unavailable, and say so in your findings);\n- Risks & unknowns: edge cases, things that could break, and open points that still need confirmation.\nOnly start writing the plan once these dimensions are covered well enough to support an actionable plan; if information is insufficient, keep investigating or ask the user rather than rushing to a plan.\n\nFinal reply structure: first a \"## Findings\" section giving the key findings along the dimensions above (each citing the source file), immediately followed by a \"## Plan\" section with actionable steps/todos (clearly numbered or checkboxed) so the user can immediately tell this is a Plan. Put background and risks after the plan, and make clear that implementation waits for Act / execute plan.\n\nCurrent saved plan:\n{current_plan}"
        )
    } else if state.mode == AgentPlanMode::Orchestrate {
        format!(
            "Agent orchestrate mode (internal runtime mode): current mode is orchestrate and plan status is {status}. You are the ORCHESTRATOR. Your job is limited to: understanding the request, planning, decomposing work, dispatching sub-agents, integrating their results, and making final decisions. All hands-on execution — researching / reading / browsing sources, investigating code, writing code or documents, running tests or commands, processing files — must be delegated to sub-agent workers via the `agent` tool. Do not do a worker's job yourself.\n\nHard rule: whenever a task can be split into 2 or more independent / parallelizable / separable parts, dispatch one sub-agent per part. Independent parts MUST be dispatched in the SAME round (multiple `agent` calls in one message) so they run in parallel; only serialize when one part truly depends on another's output.\n\nHow to write each dispatch: a worker runs in isolation and sees NOTHING outside its prompt. Each `agent` prompt must be self-contained — include the goal, the necessary context (paths, URLs, constraints, prior findings it needs), and the exact deliverable you expect back (format + scope). Vague prompts produce vague results.\n\nRequired flow for multi-step tasks: (1) use `todo_write` to lay out the task plan first; (2) for each independent subtask, set the matching todo's `owner` to the sub-agent name, mark it `in_progress`, and dispatch with the `agent` tool; (3) when a worker returns, check its result against what you asked for — if insufficient, dispatch a follow-up worker with sharper instructions instead of doing it yourself, then mark the todo `completed`; (4) integrate all results into your own final reply: reconcile conflicts, drop redundancy, and answer the user's actual question rather than pasting raw worker outputs.\n\nThe only exception: a genuinely indivisible single-step small task (e.g. a one-line translation, a simple factual question) may be answered directly. Everything else goes through the flow above. If the user asks to continue or execute the plan, use the saved plan below.\n\nCurrent saved plan:\n{current_plan}"
        )
    } else {
        format!(
            "Agent plan context (internal runtime state): current mode is act and plan status is {status}. If the user asks to continue or execute the plan, use the saved plan below as context; if the user changes requirements, follow the latest user message and note that the plan needs adjustment. Do not treat the plan as a user-editable todo list, and do not create reminders or calendar items.\n\nCurrent saved plan:\n{current_plan}"
        )
    }
}

pub fn current_plan_text(state: &AgentPlanState) -> Option<&str> {
    state
        .plan
        .as_deref()
        .map(str::trim)
        .filter(|plan| !plan.is_empty())
}

pub fn executable_plan_text(state: &AgentPlanState) -> Option<&str> {
    current_plan_text(state).filter(|plan| is_executable_plan_text(plan))
}

pub fn is_executable_plan_text(content: &str) -> bool {
    let text = content.trim();
    if text.is_empty() {
        return false;
    }

    let meaningful_lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if meaningful_lines.len() < 2 {
        return false;
    }

    let step_lines = meaningful_lines
        .iter()
        .filter(|line| is_step_like_line(line))
        .count();
    if step_lines >= 2 {
        return true;
    }

    has_plan_keyword(text) && step_lines >= 1
}

fn is_step_like_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    starts_with_markdown_step(trimmed)
        || starts_with_chinese_step(trimmed)
        || starts_with_todo_keyword(trimmed)
}

fn starts_with_markdown_step(line: &str) -> bool {
    if line.starts_with("- [ ]")
        || line.starts_with("- [x]")
        || line.starts_with("- [X]")
        || line.starts_with("* [ ]")
        || line.starts_with("* [x]")
        || line.starts_with("* [X]")
        || line.starts_with("- ")
        || line.starts_with("* ")
        || line.starts_with("+ ")
        || line.starts_with("• ")
    {
        return true;
    }

    let bytes = line.as_bytes();
    let mut digit_count = 0;
    while digit_count < bytes.len() && bytes[digit_count].is_ascii_digit() {
        digit_count += 1;
    }
    if digit_count == 0 || digit_count > 3 {
        return false;
    }
    line[digit_count..]
        .chars()
        .next()
        .is_some_and(|ch| matches!(ch, '.' | ')' | '、'))
}

fn starts_with_chinese_step(line: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "第1步",
        "第2步",
        "第3步",
        "第4步",
        "第5步",
        "第6步",
        "第7步",
        "第8步",
        "第9步",
        "第一步",
        "第二步",
        "第三步",
        "第四步",
        "第五步",
        "第六步",
        "第七步",
        "第八步",
        "第九步",
        "步骤1",
        "步骤2",
        "步骤3",
        "步骤4",
        "步骤5",
        "步骤6",
        "步骤7",
        "步骤8",
        "步骤9",
        "一、",
        "二、",
        "三、",
        "四、",
        "五、",
        "六、",
        "七、",
        "八、",
        "九、",
    ];
    PREFIXES.iter().any(|prefix| line.starts_with(prefix))
}

fn starts_with_todo_keyword(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.starts_with("todo:")
        || lower.starts_with("todo ")
        || lower.starts_with("step ")
        || lower.starts_with("步骤：")
        || lower.starts_with("步骤:")
        || lower.starts_with("任务：")
        || lower.starts_with("任务:")
}

fn has_plan_keyword(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("plan")
        || lower.contains("todo")
        || lower.contains("step")
        || text.contains("计划")
        || text.contains("步骤")
        || text.contains("待办")
        || text.contains("任务")
}

fn status_name(status: &AgentPlanStatus) -> &'static str {
    match status {
        AgentPlanStatus::Empty => "empty",
        AgentPlanStatus::Draft => "draft",
        AgentPlanStatus::Approved => "approved",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_state_defaults_to_act_empty() {
        let state: AgentPlanState = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(state.mode, AgentPlanMode::Act);
        assert_eq!(state.status, AgentPlanStatus::Empty);
        assert_eq!(state.plan, None);
    }

    #[test]
    fn capture_draft_keeps_plan_mode_and_trims_reply() {
        let state =
            capture_draft_from_reply(&AgentPlanState::default(), "  1. Read code\n2. Edit  ");
        assert_eq!(state.mode, AgentPlanMode::Plan);
        assert_eq!(state.status, AgentPlanStatus::Draft);
        assert_eq!(state.plan.as_deref(), Some("1. Read code\n2. Edit"));
        assert!(state.updated_at > 0);
    }

    #[test]
    fn capture_draft_ignores_non_plan_fragment() {
        let current = AgentPlanState::default();
        let state = capture_draft_from_reply(&current, "没问题！积萌,");

        assert_eq!(state, current);
    }

    #[test]
    fn executable_plan_requires_real_steps() {
        assert!(is_executable_plan_text(
            "计划：\n1. Read code\n2. Implement fix"
        ));
        assert!(is_executable_plan_text("- [ ] 调研\n- [ ] 修改"));
        assert!(!is_executable_plan_text("没问题！积萌,"));
        assert!(!is_executable_plan_text("计划：我会处理这个问题。"));
    }

    #[test]
    fn approve_without_plan_stays_empty_act() {
        let mut state = AgentPlanState::default();
        state.mode = AgentPlanMode::Plan;
        let approved = approve(&state);
        assert_eq!(approved.mode, AgentPlanMode::Act);
        assert_eq!(approved.status, AgentPlanStatus::Empty);
    }

    #[test]
    fn approve_with_plan_marks_approved() {
        let mut state = AgentPlanState::default();
        state.plan = Some("1. Read code\n2. Edit".to_string());
        state.status = AgentPlanStatus::Draft;
        let approved = approve(&state);
        assert_eq!(approved.mode, AgentPlanMode::Act);
        assert_eq!(approved.status, AgentPlanStatus::Approved);
    }

    #[test]
    fn mode_from_str_accepts_orchestrate() {
        assert_eq!(
            mode_from_str("orchestrate").unwrap(),
            AgentPlanMode::Orchestrate
        );
        assert_eq!(
            mode_from_str("Orchestrate").unwrap(),
            AgentPlanMode::Orchestrate
        );
        assert_eq!(mode_from_str("act").unwrap(), AgentPlanMode::Act);
        assert_eq!(mode_from_str("plan").unwrap(), AgentPlanMode::Plan);
        assert!(mode_from_str("bogus").is_err());
    }

    #[test]
    fn is_orchestrate_mode_detects_mode() {
        let mut state = AgentPlanState::default();
        assert!(!is_orchestrate_mode(&state));
        state.mode = AgentPlanMode::Orchestrate;
        assert!(is_orchestrate_mode(&state));
        assert!(!is_plan_mode(&state));
    }

    #[test]
    fn format_prompt_emits_orchestrate_section() {
        let mut state = AgentPlanState::default();
        state.mode = AgentPlanMode::Orchestrate;
        let en = format_prompt(&state);
        assert!(en.contains("orchestrate mode"));
        assert!(en.contains("sub-agents"));
    }

    #[test]
    fn chat_runtime_prompt_mentions_research_tools() {
        let en = chat_runtime_prompt();
        assert!(en.contains("Chat runtime"));
        assert!(en.contains("knowledge_search"));
        assert!(en.contains("read-only MCP"));
    }
}
