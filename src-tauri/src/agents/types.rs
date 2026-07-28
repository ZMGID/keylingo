//! `AgentDefinition` data model and the built-in agent types.

use serde::Serialize;

/// A sub-agent persona. `system_prompt` is prepended to the base chat system
/// prompt; `model` empty means inherit the parent's model; `tools` empty means
/// "all tools available to the parent except the `agent` tool" (the allow-list
/// is enforced at spawn time, see `chat::agent::filter::filter_tools_for_agent`).
///
/// `disallowed_tools` is the denylist counterpart (industry convention:
/// `disallowedTools`), applied BEFORE `tools` so "inherit everything except X"
/// is expressible. `skills` is a preload list: the listed skills' full bodies
/// are injected into the sub-agent's launch context — it does NOT narrow which
/// skills the sub-agent may activate later.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub system_prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub tools: Vec<String>,
    #[serde(default)]
    pub disallowed_tools: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    pub source: String,
}

/// The built-in agent types, ported from clawspring's `subagent.py` (general /
/// researcher / coder / reviewer). Prompts are concise and bilingual-neutral
/// (English persona text the model adapts to the conversation language). The
/// `general-purpose` type is the default: empty prompt, no tool restriction.
pub fn builtin_agent_definitions() -> Vec<AgentDefinition> {
    vec![
        AgentDefinition {
            id: "general-purpose".to_string(),
            name: "general-purpose".to_string(),
            description: "General-purpose agent for researching complex questions and executing multi-step tasks.".to_string(),
            system_prompt: String::new(),
            model: None,
            tools: Vec::new(),
            disallowed_tools: Vec::new(),
            skills: Vec::new(),
            source: "builtin".to_string(),
        },
        AgentDefinition {
            id: "researcher".to_string(),
            name: "researcher".to_string(),
            description: "Read-only research agent: searches the web and reads files to gather and synthesize information. Cannot modify files.".to_string(),
            system_prompt: "You are a research sub-agent. Investigate thoroughly using read-only tools, then return a concise, well-organized synthesis of your findings with concrete references. Do not attempt to modify files or run commands.".to_string(),
            model: None,
            tools: vec![
                "read".to_string(),
                "grep".to_string(),
                "glob".to_string(),
                "web_search".to_string(),
                "web_fetch".to_string(),
            ],
            disallowed_tools: Vec::new(),
            skills: Vec::new(),
            source: "builtin".to_string(),
        },
        AgentDefinition {
            id: "coder".to_string(),
            name: "coder".to_string(),
            description: "Implementation agent: reads, edits, and writes code to complete a focused engineering task.".to_string(),
            system_prompt: "You are a coding sub-agent. Implement the requested change precisely. Read the relevant files first, make targeted edits, and report exactly what you changed. Keep the change scoped to the task you were given.".to_string(),
            model: None,
            tools: vec![
                "read".to_string(),
                "grep".to_string(),
                "glob".to_string(),
                "edit".to_string(),
                "write".to_string(),
            ],
            disallowed_tools: Vec::new(),
            skills: Vec::new(),
            source: "builtin".to_string(),
        },
        AgentDefinition {
            id: "reviewer".to_string(),
            name: "reviewer".to_string(),
            description: "Read-only review agent: inspects code for correctness, clarity, and risk, then reports findings. Cannot modify files.".to_string(),
            system_prompt: "You are a code-review sub-agent. Inspect the relevant code using read-only tools and report concrete findings: bugs, risks, and concise improvement suggestions with file references. Do not modify files.".to_string(),
            model: None,
            tools: vec![
                "read".to_string(),
                "grep".to_string(),
                "glob".to_string(),
            ],
            disallowed_tools: Vec::new(),
            skills: Vec::new(),
            source: "builtin".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_have_unique_ids_and_general_purpose_first() {
        let defs = builtin_agent_definitions();
        assert_eq!(defs[0].id, "general-purpose");
        let mut ids: Vec<&str> = defs.iter().map(|d| d.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), defs.len(), "built-in agent ids must be unique");
    }

    #[test]
    fn general_purpose_has_no_tool_restriction() {
        let defs = builtin_agent_definitions();
        let gp = defs.iter().find(|d| d.id == "general-purpose").unwrap();
        assert!(gp.tools.is_empty());
        assert!(gp.system_prompt.is_empty());
    }

    #[test]
    fn researcher_is_read_only_set() {
        let defs = builtin_agent_definitions();
        let r = defs.iter().find(|d| d.id == "researcher").unwrap();
        assert!(r.tools.contains(&"read".to_string()));
        assert!(r.tools.contains(&"web_search".to_string()));
        assert!(!r.tools.contains(&"write".to_string()));
        assert!(!r.tools.contains(&"edit".to_string()));
    }
}
