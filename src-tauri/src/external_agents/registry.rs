use crate::external_agents::defs::{acp, claude, codex, grok, pi};
use crate::external_agents::types::RuntimeAgentDef;

pub const AGENT_DEFS: &[RuntimeAgentDef] = &[
    claude::CLAUDE_AGENT_DEF,
    codex::CODEX_AGENT_DEF,
    acp::CURSOR_AGENT_DEF,
    acp::OPENCODE_AGENT_DEF,
    acp::GEMINI_AGENT_DEF,
    acp::KIMI_AGENT_DEF,
    pi::PI_AGENT_DEF,
    acp::HERMES_AGENT_DEF,
    grok::GROK_AGENT_DEF,
];

pub fn get_agent_def(id: &str) -> Option<&'static RuntimeAgentDef> {
    AGENT_DEFS.iter().find(|def| def.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_nine_agents() {
        assert_eq!(AGENT_DEFS.len(), 9);
        assert!(get_agent_def("claude").is_some());
        assert!(get_agent_def("opencode").is_some());
        assert!(get_agent_def("pi").is_some());
        assert!(get_agent_def("hermes").is_some());
        assert!(get_agent_def("grok").is_some());
        assert!(get_agent_def("unknown").is_none());
    }
}
