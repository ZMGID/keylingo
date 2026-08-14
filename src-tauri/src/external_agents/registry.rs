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

    /// 「运行中注入」是**逐协议**的能力，不是可以顺手打开的开关。
    ///
    /// 目前只有 codex 有对应原语（`turn/steer` + `expectedTurnId`，真机验证见
    /// `session::codex_app_server::tests::codex_turn_steer_injects_into_the_running_turn`）。
    /// 把别的 CLI 标成 true 会让前端亮出一个永远失败的入口；claude 那条更糟 —— 它的
    /// stream-json 输入是**顺序**处理的，硬写进去会让 CLI 在本轮之后自己再起一轮，
    /// 把轮次边界搞乱（见 `session::claude_stream` 里那段注释）。
    /// 要新增一个，先在该协议里做出注入并补一条真机测试，再改这里。
    #[test]
    fn only_codex_claims_mid_turn_steering() {
        let steerable: Vec<&str> = AGENT_DEFS
            .iter()
            .filter(|def| def.supports_steering)
            .map(|def| def.id)
            .collect();
        assert_eq!(steerable, vec!["codex"]);
    }
}
