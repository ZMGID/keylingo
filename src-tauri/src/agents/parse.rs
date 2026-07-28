//! Parse an `AgentDefinition` from a Markdown file with YAML-ish frontmatter,
//! reusing the Skill frontmatter parser so the two systems stay consistent.
//!
//! ```text
//! ---
//! name: research-agent
//! description: Deep-dive research and source synthesis
//! tools: read_file, web_search, web_fetch
//! disallowedTools: write, edit
//! skills: pdf
//! model: gpt-4o
//! ---
//!
//! You are specialized in deep fact-checking research...
//! ```

use crate::skills::parse::{parse_list_value, split_frontmatter};
use crate::skills::slugify;

use super::types::AgentDefinition;

/// Parse one agent `.md`. Returns `None` only when there is no usable name
/// (every other field has a sensible default), so a partially-specified file
/// still loads. `name`/`description` fall back to the file stem.
pub fn parse_agent_markdown(
    fallback_id: &str,
    raw: &str,
    source: &str,
    path: Option<String>,
) -> Option<AgentDefinition> {
    let (frontmatter, body) = split_frontmatter(raw);

    let name = frontmatter
        .get("name")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback_id.to_string());
    if name.is_empty() {
        return None;
    }
    let id = slugify(&name);
    let description = frontmatter
        .get("description")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let model = frontmatter
        .get("model")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let tools = parse_list_value(frontmatter.get("tools"));
    // Industry convention spells the denylist `disallowedTools` (camelCase);
    // Kivio's own frontmatter style is snake_case, so accept both (camelCase wins).
    let disallowed_tools = match frontmatter.get("disallowedTools") {
        Some(value) => parse_list_value(Some(value)),
        None => parse_list_value(frontmatter.get("disallowed_tools")),
    };
    let skills = parse_list_value(frontmatter.get("skills"));
    let system_prompt = body.trim().to_string();

    let _ = path; // reserved for future "open definition" UX; kept for symmetry with skills
    Some(AgentDefinition {
        id,
        name,
        description,
        system_prompt,
        model,
        tools,
        disallowed_tools,
        skills,
        source: source.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_frontmatter_and_body() {
        let raw = "---\nname: research-agent\ndescription: Deep research\ntools: read_file, web_search, web_fetch\nmodel: gpt-4o\n---\n\nYou are a research specialist.\n";
        let def = parse_agent_markdown("fallback", raw, "user", None).unwrap();
        assert_eq!(def.id, "research-agent");
        assert_eq!(def.name, "research-agent");
        assert_eq!(def.description, "Deep research");
        assert_eq!(def.model.as_deref(), Some("gpt-4o"));
        assert_eq!(
            def.tools,
            vec!["read_file", "web_search", "web_fetch"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
        assert_eq!(def.system_prompt, "You are a research specialist.");
        assert_eq!(def.source, "user");
    }

    #[test]
    fn falls_back_to_file_stem_when_name_missing() {
        let raw = "Just a body, no frontmatter.";
        let def = parse_agent_markdown("my-agent", raw, "project", None).unwrap();
        assert_eq!(def.id, "my-agent");
        assert_eq!(def.name, "my-agent");
        assert!(def.tools.is_empty());
        assert_eq!(def.system_prompt, "Just a body, no frontmatter.");
    }

    #[test]
    fn parses_bracket_tool_list() {
        let raw = "---\nname: x\ntools: [read_file, edit_file]\n---\nbody";
        let def = parse_agent_markdown("x", raw, "user", None).unwrap();
        assert_eq!(
            def.tools,
            vec!["read_file".to_string(), "edit_file".to_string()]
        );
    }

    #[test]
    fn parses_skills_preload_list() {
        let raw = "---\nname: x\nskills: pdf, docx\n---\nbody";
        let def = parse_agent_markdown("x", raw, "user", None).unwrap();
        assert_eq!(def.skills, vec!["pdf".to_string(), "docx".to_string()]);
    }

    #[test]
    fn parses_disallowed_tools_both_key_spellings() {
        // Industry-standard camelCase.
        let camel = "---\nname: x\ndisallowedTools: write, edit\n---\nbody";
        let def = parse_agent_markdown("x", camel, "user", None).unwrap();
        assert_eq!(
            def.disallowed_tools,
            vec!["write".to_string(), "edit".to_string()]
        );
        // Kivio-style snake_case.
        let snake = "---\nname: x\ndisallowed_tools: bash\n---\nbody";
        let def = parse_agent_markdown("x", snake, "user", None).unwrap();
        assert_eq!(def.disallowed_tools, vec!["bash".to_string()]);
        // Both present ⇒ camelCase wins.
        let both = "---\nname: x\ndisallowedTools: write\ndisallowed_tools: bash\n---\nbody";
        let def = parse_agent_markdown("x", both, "user", None).unwrap();
        assert_eq!(def.disallowed_tools, vec!["write".to_string()]);
    }

    #[test]
    fn new_fields_default_to_empty() {
        let raw = "---\nname: x\ntools: read_file\n---\nbody";
        let def = parse_agent_markdown("x", raw, "user", None).unwrap();
        assert!(def.disallowed_tools.is_empty());
        assert!(def.skills.is_empty());
    }
}
