use super::super::types::{
    PromptInputFormat, RuntimeAgentDef, RuntimeBuildOptions, RuntimeContext, StreamFormat,
};

/// 探测彻底失败时的静态兜底 — 与 desktop-cc-gui `generatedModelCatalog.json` 四档一致。
const FALLBACK_MODELS: &[(&str, &str)] = &[
    ("default", "Default"),
    ("gpt-5.6-sol", "gpt-5.6-sol"),
    ("gpt-5.6-terra", "gpt-5.6-terra"),
    ("gpt-5.6-luna", "gpt-5.6-luna"),
    ("gpt-5.5", "gpt-5.5"),
];

const REASONING: &[(&str, &str)] = &[
    ("default", "Default"),
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("xhigh", "XHigh"),
    ("max", "Max"),
    ("ultra", "Ultra"),
];

pub fn build_codex_args(
    _ctx: &RuntimeContext,
    _options: &RuntimeBuildOptions,
    _prompt: Option<&str>,
) -> Vec<String> {
    // The app-server protocol negotiates cwd / model / sandbox / approval over JSON-RPC
    // (`Thread/start` + `Turn/start`), so no model / sandbox CLI flags are needed here.
    vec!["app-server".to_string()]
}

pub const CODEX_AGENT_DEF: RuntimeAgentDef = RuntimeAgentDef {
    id: "codex",
    name: "Codex CLI",
    bin: "codex",
    fallback_bins: &[],
    version_args: &["--version"],
    auth_probe_args: Some(&["login", "status"]),
    fallback_models: FALLBACK_MODELS,
    reasoning_options: REASONING,
    list_models_args: Some(&["debug", "models"]),
    // `codex debug models` cold-start can exceed 5s（首次要拉配置/鉴权）；给 15s 免误判失败（F4）。
    list_models_timeout_secs: Some(20),
    models_from_stderr: false,
    model_probe: None,
    model_probe_args: None,
    slash_strategy: super::super::types::SlashStrategy::CodexAppServer,
    env: &[],
    max_prompt_arg_bytes: None,
    prompt_via_stdin: false,
    prompt_input_format: PromptInputFormat::Text,
    stream_format: StreamFormat::CodexAppServer,
    resumes_session_via_cli: false,
    supports_native_image: true,
    supports_steering: true,
    supports_follow_up: false,
    image_mime_whitelist: &[],
    build_args: build_codex_args,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_build_args_uses_app_server() {
        let args = build_codex_args(
            &RuntimeContext {
                extra_allowed_dirs: vec![],
                resume_session_id: None,
                new_session_id: None,
                include_partial_messages: false,
            },
            &RuntimeBuildOptions {
                model: Some("gpt-5".to_string()),
                reasoning: Some("high".to_string()),
                sandbox: None,
            },
            None,
        );
        assert_eq!(args, vec!["app-server".to_string()]);
    }
}
