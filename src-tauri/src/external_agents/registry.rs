use crate::external_agents::defs::{acp, claude, codex, dsh, grok, pi};
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
    dsh::DSH_AGENT_DEF,
];

pub fn get_agent_def(id: &str) -> Option<&'static RuntimeAgentDef> {
    AGENT_DEFS.iter().find(|def| def.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_ten_agents() {
        assert_eq!(AGENT_DEFS.len(), 10);
        assert!(get_agent_def("claude").is_some());
        assert!(get_agent_def("opencode").is_some());
        assert!(get_agent_def("pi").is_some());
        assert!(get_agent_def("hermes").is_some());
        assert!(get_agent_def("grok").is_some());
        assert!(get_agent_def("dsh").is_some());
        assert!(get_agent_def("unknown").is_none());
    }

    /// 「运行中注入」是逐协议能力。Codex 使用 `turn/steer`，Pi 使用 RPC `steer`，
    /// dsh 使用 bridge 的 `session/steer` → `agent.steer()`；都必须等对端成功响应后
    /// 才确认前端队列并发出 `UserSteer`。
    #[test]
    fn codex_pi_and_dsh_claim_mid_turn_steering() {
        let steerable: Vec<&str> = AGENT_DEFS
            .iter()
            .filter(|def| def.supports_steering)
            .map(|def| def.id)
            .collect();
        assert_eq!(steerable, vec!["codex", "pi", "dsh"]);
    }

    /// 原生 follow-up（当前轮结束后自动开下一轮）目前只有 Pi RPC 与 dsh `session/prompt`。
    #[test]
    fn pi_and_dsh_claim_native_follow_up() {
        let follow_up: Vec<&str> = AGENT_DEFS
            .iter()
            .filter(|def| def.supports_follow_up)
            .map(|def| def.id)
            .collect();
        assert_eq!(follow_up, vec!["pi", "dsh"]);
    }
}
