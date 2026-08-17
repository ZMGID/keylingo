//! Shared definition for the ACP-family external agents (cursor / gemini / opencode / hermes / kimi).
//!
//! All launch over the Agent Client Protocol (`StreamFormat::AcpJsonRpc`), probe models via
//! `ModelProbeStrategy::Acp`, discover slash commands via `SlashStrategy::Acp`, and build a
//! constant launch-arg vec. They differ only in id / name / binary / auth-probe / fallback models
//! / launch args / env — so a single [`acp_def`] const constructor + data rows replaces several
//! near-identical struct literals. `kimi` joins this family via `kimi acp` (its own ACP server),
//! inheriting persistent sessions, native resume, mid-turn model switch, and error-classified
//! reconnect instead of the old per-turn `-p` + `stream-json` spawn.

use super::super::types::{
    ModelProbeStrategy, PromptInputFormat, RuntimeAgentDef, RuntimeBuildOptions, RuntimeContext,
    SlashStrategy, StreamFormat,
};

const fn acp_def(
    id: &'static str,
    name: &'static str,
    bin: &'static str,
    fallback_bins: &'static [&'static str],
    auth_probe_args: Option<&'static [&'static str]>,
    fallback_models: &'static [(&'static str, &'static str)],
    launch_args: &'static [&'static str],
    env: &'static [(&'static str, &'static str)],
    build_args: fn(&RuntimeContext, &RuntimeBuildOptions, Option<&str>) -> Vec<String>,
) -> RuntimeAgentDef {
    RuntimeAgentDef {
        id,
        name,
        bin,
        fallback_bins,
        version_args: &["--version"],
        auth_probe_args,
        fallback_models,
        reasoning_options: &[],
        list_models_args: None,
        list_models_timeout_secs: Some(15),
        models_from_stderr: false,
        model_probe: Some(ModelProbeStrategy::Acp),
        model_probe_args: Some(launch_args),
        slash_strategy: SlashStrategy::Acp,
        env,
        max_prompt_arg_bytes: None,
        prompt_via_stdin: false,
        prompt_input_format: PromptInputFormat::Text,
        stream_format: StreamFormat::AcpJsonRpc,
        resumes_session_via_cli: false,
        supports_native_image: true,
        supports_steering: false,
        supports_follow_up: false,
        image_mime_whitelist: &[],
        build_args,
    }
}

// ACP launch: the model is set via `session/set_model` inside run_acp_session, not flags.
fn build_acp_args(_c: &RuntimeContext, _o: &RuntimeBuildOptions, _p: Option<&str>) -> Vec<String> {
    vec!["acp".to_string()]
}

fn build_gemini_args(
    _c: &RuntimeContext,
    _o: &RuntimeBuildOptions,
    _p: Option<&str>,
) -> Vec<String> {
    vec!["--experimental-acp".to_string()]
}

fn build_hermes_args(
    _c: &RuntimeContext,
    _o: &RuntimeBuildOptions,
    _p: Option<&str>,
) -> Vec<String> {
    vec!["acp".to_string(), "--accept-hooks".to_string()]
}

const CURSOR_MODELS: &[(&str, &str)] = &[
    ("default", "Default"),
    ("auto", "auto"),
    ("sonnet-4", "sonnet-4"),
    ("gpt-5", "gpt-5"),
];

const GEMINI_MODELS: &[(&str, &str)] = &[
    ("default", "Default"),
    ("gemini-3-pro-preview", "gemini-3-pro-preview"),
    ("gemini-3-flash-preview", "gemini-3-flash-preview"),
    ("gemini-2.5-pro", "gemini-2.5-pro"),
    ("gemini-2.5-flash", "gemini-2.5-flash"),
    ("gemini-2.5-flash-lite", "gemini-2.5-flash-lite"),
];

const OPENCODE_MODELS: &[(&str, &str)] = &[
    ("default", "Default"),
    ("anthropic/claude-sonnet-4-5", "anthropic/claude-sonnet-4-5"),
    ("openai/gpt-5", "openai/gpt-5"),
    ("google/gemini-2.5-pro", "google/gemini-2.5-pro"),
];

const HERMES_MODELS: &[(&str, &str)] = &[
    ("default", "Default"),
    ("grok-4.3", "grok-4.3 (xAI · default)"),
    ("grok-4.20-reasoning", "grok-4.20-reasoning (xAI · deep)"),
    (
        "grok-4.20-0309-non-reasoning",
        "grok-4.20-non-reasoning (xAI · fast)",
    ),
    (
        "grok-4.20-multi-agent-0309",
        "grok-4.20-multi-agent (xAI · orchestration)",
    ),
    ("openai-codex:gpt-5.5", "gpt-5.5 (openai-codex:gpt-5.5)"),
    ("openai-codex:gpt-5.4", "gpt-5.4 (openai-codex:gpt-5.4)"),
    (
        "openai-codex:gpt-5.4-mini",
        "gpt-5.4-mini (openai-codex:gpt-5.4-mini)",
    ),
];

const GEMINI_ENV: &[(&str, &str)] = &[("GEMINI_CLI_TRUST_WORKSPACE", "true")];

// Kimi Code models used only when the ACP session/new probe reports none (offline / not logged in).
// The real catalog now comes from the ACP `availableModels` / `configOptions` probe like the other
// ACP agents — this table is the fallback the picker labels as "默认列表".
const KIMI_MODELS: &[(&str, &str)] = &[
    ("default", "Default"),
    ("kimi-code/k3", "K3 (kimi-code/k3)"),
    (
        "kimi-code/kimi-for-coding",
        "K2.7 Coding (kimi-code/kimi-for-coding)",
    ),
];

pub const CURSOR_AGENT_DEF: RuntimeAgentDef = acp_def(
    "cursor-agent",
    "Cursor Agent",
    "cursor-agent",
    &[],
    Some(&["status"]),
    CURSOR_MODELS,
    &["acp"],
    &[],
    build_acp_args,
);

pub const GEMINI_AGENT_DEF: RuntimeAgentDef = acp_def(
    "gemini",
    "Gemini CLI",
    "gemini",
    &[],
    None,
    GEMINI_MODELS,
    &["--experimental-acp"],
    GEMINI_ENV,
    build_gemini_args,
);

pub const OPENCODE_AGENT_DEF: RuntimeAgentDef = acp_def(
    "opencode",
    "OpenCode",
    "opencode-cli",
    &["opencode"],
    None,
    OPENCODE_MODELS,
    &["acp"],
    &[],
    build_acp_args,
);

pub const HERMES_AGENT_DEF: RuntimeAgentDef = acp_def(
    "hermes",
    "Hermes",
    "hermes",
    &[],
    None,
    HERMES_MODELS,
    &["acp", "--accept-hooks"],
    &[],
    build_hermes_args,
);

pub const KIMI_AGENT_DEF: RuntimeAgentDef = acp_def(
    "kimi",
    "Kimi CLI",
    "kimi",
    // 刻意不加 `kimi-cli` 别名：那是已停止维护的旧 Python 版（版本号 1.4x），协议与模型列表
    // 都和现在的 TypeScript 版 Kimi Code（0.x，命令名就是 `kimi`）不同。回退过去会静默跑错
    // 目标，不如老实报「未安装」。
    &[],
    None,
    KIMI_MODELS,
    &["acp"],
    &[],
    build_acp_args,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acp_defs_build_expected_launch_args() {
        let ctx = RuntimeContext {
            extra_allowed_dirs: vec![],
            resume_session_id: None,
            new_session_id: None,
            include_partial_messages: false,
        };
        let opts = RuntimeBuildOptions {
            model: None,
            reasoning: None,
            sandbox: None,
        };
        let cases: &[(&RuntimeAgentDef, &[&str])] = &[
            (&CURSOR_AGENT_DEF, &["acp"]),
            (&GEMINI_AGENT_DEF, &["--experimental-acp"]),
            (&OPENCODE_AGENT_DEF, &["acp"]),
            (&HERMES_AGENT_DEF, &["acp", "--accept-hooks"]),
            (&KIMI_AGENT_DEF, &["acp"]),
        ];
        for (def, expected) in cases {
            let args = (def.build_args)(&ctx, &opts, None);
            let expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
            assert_eq!(args, expected, "launch args for {}", def.id);
            assert!(matches!(def.model_probe, Some(ModelProbeStrategy::Acp)));
            assert!(matches!(def.stream_format, StreamFormat::AcpJsonRpc));
        }
    }
}
