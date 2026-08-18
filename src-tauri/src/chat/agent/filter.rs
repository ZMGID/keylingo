//! Sub-agent tool-table filtering (P3).
//!
//! Narrows a tool list to an `AgentDefinition`'s allow/deny lists and ALWAYS
//! strips the `agent` spawn tool itself (second recursion guard alongside the
//! depth check). The sub-agent definition is an explicit settings-level policy:
//! it keeps skill-runtime tools while enforcing the agent's configured tool list.
//!
//! Entry syntax follows the industry convention (Claude Code / Cursor), which
//! matches Kivio's own MCP tool ids (`mcp__<server>__<tool>`) verbatim:
//! `*` (everything), `mcp__*` (every MCP tool), `mcp__<server>` /
//! `mcp__<server>__*` (one server's tools), any other `<prefix>*`, or an exact
//! tool name (legacy aliases included). Composition order is spec-mandated:
//! apply `disallowed_tools` FIRST, then resolve `tools` on what remains.

use crate::agents::AgentDefinition;
use crate::mcp::ChatToolDefinition;

use super::prepare::tool_matches_recommended_name;

/// Whether one allow/deny entry matches `tool`. Suffix `*` only — the spec's
/// wildcard forms are all prefixes, so a glob engine would be dead weight.
/// Public so the spawn path can tell "the denylist emptied the pool" apart from
/// "an allow entry was misspelled" when refusing a zero-tool launch.
pub fn entry_matches(tool: &ChatToolDefinition, entry: &str) -> bool {
    let entry = entry.trim();
    if entry.is_empty() {
        return false;
    }
    if entry == "*" {
        return true;
    }
    // `mcp__*` ⇒ every MCP tool; `mcp__<server>` / `mcp__<server>__*` ⇒ that
    // server's whole tool set (spec semantics: a bare server name authorizes it).
    if let Some(rest) = entry.strip_prefix("mcp__") {
        let server = rest.trim_end_matches('*').trim_end_matches("__");
        if server.is_empty() {
            return tool.source == "mcp";
        }
        if tool.server_id.as_deref() == Some(server) {
            return true;
        }
    }
    if let Some(prefix) = entry.strip_suffix('*') {
        return [
            tool.name.as_str(),
            tool.id.as_str(),
            tool.openai_tool_name().as_str(),
        ]
        .iter()
        .any(|candidate| candidate.starts_with(prefix));
    }
    tool_matches_recommended_name(tool, entry)
}

/// Filter `tools` in place for a sub-agent run. Returns the removed tools (for
/// transparency/logging), mirroring `apply_agent_plan_tool_filter`.
pub fn filter_tools_for_agent(
    tools: &mut Vec<ChatToolDefinition>,
    def: &AgentDefinition,
) -> Vec<ChatToolDefinition> {
    let mut removed = Vec::new();
    let allow = &def.tools;
    let deny = &def.disallowed_tools;
    tools.retain(|tool| {
        // The `agent` spawn tool is never available inside a sub-agent: a worker
        // must not spawn sibling agents (recursion is top-down only). This is the
        // second recursion guard alongside the depth check.
        if is_sub_agent_control_tool(tool) {
            removed.push(tool.clone());
            return false;
        }
        // Phase 1 (spec order): the denylist wins over everything below.
        if deny.iter().any(|entry| entry_matches(tool, entry)) {
            removed.push(tool.clone());
            return false;
        }
        // Empty allow-list ⇒ no narrowing (all remaining tools available).
        if allow.is_empty() {
            return true;
        }
        // Always keep skill-source tools and skill-runtime meta-tools so the
        // sub-agent can still read/run skills when present.
        if tool.source == "skill" || super::prepare::is_native_skill_tool_name(&tool.name) {
            return true;
        }
        // Phase 2: resolve the allow-list on the post-deny pool.
        let allowed = allow.iter().any(|entry| entry_matches(tool, entry));
        // Keep Kivio housekeeping built-ins (todo, etc.) that the agent did not
        // explicitly exclude — they are appended separately and are harmless.
        if allowed {
            true
        } else {
            removed.push(tool.clone());
            false
        }
    });
    removed
}

/// Allow-list entries that match nothing in `pool` once the `agent` tool and the
/// denylist are taken out. The spawn path uses this to refuse a launch whose
/// `tools` resolved to the empty set (spec: report the unresolved entries rather
/// than silently starting a zero-tool sub-agent). `pool` is the UNFILTERED
/// catalog; empty result when `def.tools` is empty (no narrowing requested).
pub fn unresolved_allow_entries(pool: &[ChatToolDefinition], def: &AgentDefinition) -> Vec<String> {
    def.tools
        .iter()
        .filter(|entry| {
            !pool.iter().any(|tool| {
                !is_sub_agent_control_tool(tool)
                    && !def
                        .disallowed_tools
                        .iter()
                        .any(|deny| entry_matches(tool, deny))
                    && entry_matches(tool, entry)
            })
        })
        .cloned()
        .collect()
}

/// Whether `tool` is the `agent` spawn tool. It is stripped from a sub-agent's
/// table: a worker cannot spawn sibling agents.
fn is_sub_agent_control_tool(tool: &ChatToolDefinition) -> bool {
    tool.source == "native" && crate::chat::sub_agent::is_sub_agent_tool_name(&tool.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentDefinition;

    fn native(name: &str) -> ChatToolDefinition {
        ChatToolDefinition {
            id: format!("native__{name}"),
            name: name.to_string(),
            description: String::new(),
            source: "native".to_string(),
            server_id: None,
            server_name: Some("Kivio".to_string()),
            input_schema: serde_json::json!({}),
            sensitive: false,
            annotations: None,
            output_schema: None,
        }
    }

    fn mcp(server: &str, name: &str) -> ChatToolDefinition {
        ChatToolDefinition {
            // Same id shape as `mcp::types::tool_definition_from_mcp`, which is
            // exactly the industry `mcp__<server>__<tool>` convention.
            id: format!("mcp__{server}__{name}"),
            name: name.to_string(),
            description: String::new(),
            source: "mcp".to_string(),
            server_id: Some(server.to_string()),
            server_name: Some(server.to_string()),
            input_schema: serde_json::json!({}),
            sensitive: false,
            annotations: None,
            output_schema: None,
        }
    }

    fn def(tools: Vec<&str>) -> AgentDefinition {
        AgentDefinition {
            id: "t".to_string(),
            name: "t".to_string(),
            description: String::new(),
            system_prompt: String::new(),
            model: None,
            tools: tools.into_iter().map(String::from).collect(),
            disallowed_tools: Vec::new(),
            skills: Vec::new(),
            source: "builtin".to_string(),
        }
    }

    #[test]
    fn always_strips_agent_tool_even_with_empty_allow_list() {
        let mut tools = vec![native("agent"), native("read_file")];
        let removed = filter_tools_for_agent(&mut tools, &def(vec![]));
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "read_file");
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].name, "agent");
    }

    #[test]
    fn narrows_to_allow_list_and_strips_agent() {
        let mut tools = vec![
            native("agent"),
            native("read_file"),
            native("write_file"),
            native("web_search"),
        ];
        let removed = filter_tools_for_agent(&mut tools, &def(vec!["read_file", "web_search"]));
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["read_file", "web_search"]);
        // agent + write_file removed
        assert_eq!(removed.len(), 2);
    }

    #[test]
    fn default_subagent_keeps_non_agent_tools() {
        // A default (general-purpose) sub-agent has an empty allow-list, so all
        // tools except `agent` survive — write_file must NOT be stripped.
        let mut tools = vec![native("agent"), native("write_file"), native("read_file")];
        let removed = filter_tools_for_agent(&mut tools, &def(vec![]));
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"write_file"), "write_file must remain");
        assert!(removed.iter().all(|t| t.name != "write_file"));
    }

    #[test]
    fn filtering_is_idempotent() {
        let mut tools = vec![native("agent"), native("read_file"), native("write_file")];
        let d = def(vec!["read_file"]);
        filter_tools_for_agent(&mut tools, &d);
        let after_first: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        filter_tools_for_agent(&mut tools, &d);
        let after_second: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        assert_eq!(after_first, after_second);
        assert_eq!(after_first, vec!["read_file".to_string()]);
    }

    // -----------------------------------------------------------------------
    // Spec wildcard syntax (`mcp__<server>` / `mcp__<server>__*` / `mcp__*` / `*`)
    // -----------------------------------------------------------------------

    fn names(tools: &[ChatToolDefinition]) -> Vec<&str> {
        tools.iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn mcp_server_entry_and_explicit_wildcard_are_equivalent() {
        for entry in ["mcp__notion", "mcp__notion__*"] {
            let mut tools = vec![
                native("agent"),
                native("read_file"),
                mcp("notion", "search"),
                mcp("notion", "create_page"),
                mcp("linear", "list_issues"),
            ];
            filter_tools_for_agent(&mut tools, &def(vec![entry]));
            assert_eq!(
                names(&tools),
                vec!["search", "create_page"],
                "entry {entry} must pass the whole notion server and block others"
            );
        }
    }

    #[test]
    fn mcp_star_passes_every_mcp_tool_and_blocks_native() {
        let mut tools = vec![
            native("agent"),
            native("read_file"),
            mcp("notion", "search"),
            mcp("linear", "list_issues"),
        ];
        filter_tools_for_agent(&mut tools, &def(vec!["mcp__*"]));
        assert_eq!(names(&tools), vec!["search", "list_issues"]);
    }

    #[test]
    fn star_passes_everything_except_the_agent_tool() {
        let mut tools = vec![
            native("agent"),
            native("read_file"),
            native("write_file"),
            mcp("notion", "search"),
        ];
        filter_tools_for_agent(&mut tools, &def(vec!["*"]));
        assert_eq!(names(&tools), vec!["read_file", "write_file", "search"]);
    }

    #[test]
    fn entries_without_wildcards_still_match_exactly() {
        let mut tools = vec![
            native("read_file"),
            native("read_file_extra"),
            mcp("notion", "search"),
        ];
        filter_tools_for_agent(&mut tools, &def(vec!["read_file"]));
        assert_eq!(names(&tools), vec!["read_file"]);
    }

    #[test]
    fn disallowed_tools_alone_narrows_by_subtraction() {
        let mut tools = vec![
            native("agent"),
            native("read_file"),
            native("write"),
            native("edit"),
            mcp("notion", "search"),
        ];
        let mut d = def(vec![]);
        d.disallowed_tools = vec!["write".to_string(), "edit".to_string()];
        filter_tools_for_agent(&mut tools, &d);
        // Everything is inherited except the two denied tools (and `agent`).
        assert_eq!(names(&tools), vec!["read_file", "search"]);
    }

    #[test]
    fn deny_is_applied_before_allow_so_overlaps_are_removed() {
        let mut tools = vec![
            native("read_file"),
            mcp("notion", "search"),
            mcp("notion", "create_page"),
        ];
        let mut d = def(vec!["mcp__notion__*", "read_file"]);
        // `create_page` is in BOTH lists ⇒ removed (spec: deny first, then allow).
        d.disallowed_tools = vec!["create_page".to_string()];
        filter_tools_for_agent(&mut tools, &d);
        assert_eq!(names(&tools), vec!["read_file", "search"]);
    }

    #[test]
    fn deny_star_removes_everything() {
        let mut tools = vec![native("read_file"), mcp("notion", "search")];
        let mut d = def(vec![]);
        d.disallowed_tools = vec!["*".to_string()];
        filter_tools_for_agent(&mut tools, &d);
        assert!(tools.is_empty());
    }

    #[test]
    fn unresolved_allow_entries_lists_only_misses() {
        let pool = vec![
            native("agent"),
            native("read_file"),
            mcp("notion", "search"),
        ];
        let d = def(vec!["read_file", "reed_file", "mcp__notionn__*"]);
        assert_eq!(
            unresolved_allow_entries(&pool, &d),
            vec!["reed_file".to_string(), "mcp__notionn__*".to_string()]
        );
        // No narrowing requested ⇒ nothing unresolved.
        assert!(unresolved_allow_entries(&pool, &def(vec![])).is_empty());
        // A tool the denylist already removed does not count as resolved.
        let mut denied = def(vec!["read_file"]);
        denied.disallowed_tools = vec!["read_file".to_string()];
        assert_eq!(
            unresolved_allow_entries(&pool, &denied),
            vec!["read_file".to_string()]
        );
        // The stripped `agent` tool can never resolve an entry either.
        assert_eq!(
            unresolved_allow_entries(&pool, &def(vec!["agent"])),
            vec!["agent".to_string()]
        );
    }
}
