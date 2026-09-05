use super::super::types::{
    PromptInputFormat, RuntimeAgentDef, RuntimeBuildOptions, RuntimeContext, SlashStrategy,
    StreamFormat,
};

pub fn build_antigravity_args(
    ctx: &RuntimeContext,
    options: &RuntimeBuildOptions,
    _prompt: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
    ];
    // Resume is supplied by the persisted live handle in connect(), never --continue.
    // agy allocates its own conversation id; a Kivio-generated UUID is not a new-session flag.
    for (flag, value) in [
        ("--model", &options.model),
        ("--effort", &options.reasoning),
    ] {
        if let Some(value) = value
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty() && *v != "default")
        {
            args.extend([flag.into(), value.into()]);
        }
    }
    match options.sandbox.as_deref() {
        Some(mode @ ("plan" | "accept-edits")) => args.extend(["--mode".into(), mode.into()]),
        Some("sandbox") => args.push("--sandbox".into()),
        Some("always-proceed") => args.push("--dangerously-skip-permissions".into()),
        _ => {} // Inherit native permissions; do not silently bypass them.
    }
    for dir in &ctx.extra_allowed_dirs {
        args.extend(["--add-dir".into(), dir.clone()]);
    }
    args
}

pub const ANTIGRAVITY_AGENT_DEF: RuntimeAgentDef = RuntimeAgentDef {
    id: "antigravity",
    name: "Antigravity CLI",
    bin: "agy",
    fallback_bins: &[],
    version_args: &["--version"],
    auth_probe_args: None,
    fallback_models: &[],
    reasoning_options: &[
        ("default", "Default"),
        ("low", "Low"),
        ("medium", "Medium"),
        ("high", "High"),
    ],
    list_models_args: Some(&["models"]),
    // Fetches the remote catalog; the desktop's first probe can exceed 30s.
    list_models_timeout_secs: Some(60),
    models_from_stderr: false,
    model_probe: None,
    model_probe_args: None,
    slash_strategy: SlashStrategy::Antigravity,
    env: &[],
    max_prompt_arg_bytes: None,
    prompt_via_stdin: true,
    prompt_input_format: PromptInputFormat::Text,
    stream_format: StreamFormat::AntigravityStreamJson,
    resumes_session_via_cli: true,
    supports_native_image: false,
    supports_steering: false,
    supports_follow_up: false,
    image_mime_whitelist: &[],
    build_args: build_antigravity_args,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_flags_preserve_paths_and_never_invent_session_ids_or_permissions() {
        let ctx = RuntimeContext {
            extra_allowed_dirs: vec![r"C:\my project".into()],
            resume_session_id: Some("old".into()),
            new_session_id: Some("new".into()),
            include_partial_messages: true,
        };
        let mut options = RuntimeBuildOptions {
            model: Some("my-model".into()),
            reasoning: Some("high".into()),
            sandbox: None,
        };
        let args = build_antigravity_args(&ctx, &options, Some("must go to stdin"));
        assert!(args
            .windows(2)
            .any(|w| w == ["--add-dir", r"C:\my project"]));
        assert!(args.windows(2).any(|w| w == ["--model", "my-model"]));
        assert!(!args.iter().any(|v| [
            "old",
            "new",
            "--continue",
            "--dangerously-skip-permissions",
            "must go to stdin"
        ]
        .contains(&v.as_str())));
        options.sandbox = Some("always-proceed".into());
        assert!(build_antigravity_args(&ctx, &options, None)
            .contains(&"--dangerously-skip-permissions".into()));
    }
}
