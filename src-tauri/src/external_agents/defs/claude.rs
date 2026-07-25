use super::super::types::{
    ModelProbeStrategy, PromptInputFormat, RuntimeAgentDef, RuntimeBuildOptions, RuntimeContext,
    StreamFormat,
};

const FALLBACK_MODELS: &[(&str, &str)] = &[("default", "Default")];

/// Claude Code thinking levels, passed via the CLI's `--effort <level>` flag.
const REASONING: &[(&str, &str)] = &[
    ("default", "Default"),
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("xhigh", "Extra high"),
    ("max", "Max"),
];

pub fn build_claude_args(
    ctx: &RuntimeContext,
    options: &RuntimeBuildOptions,
    _prompt: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        "--input-format".to_string(),
        "stream-json".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
    ];
    if ctx.include_partial_messages {
        args.push("--include-partial-messages".to_string());
    }
    if let Some(model) = options
        .model
        .as_ref()
        .filter(|m| *m != "default" && !m.is_empty())
    {
        args.push("--model".to_string());
        args.push(model.clone());
    }
    if let Some(effort) = options
        .reasoning
        .as_ref()
        .filter(|r| *r != "default" && !r.is_empty())
    {
        args.push("--effort".to_string());
        args.push(effort.clone());
    }
    for dir in &ctx.extra_allowed_dirs {
        if !dir.is_empty() {
            args.push("--add-dir".to_string());
            args.push(dir.clone());
        }
    }
    if let Some(session_id) = ctx.resume_session_id.as_ref().filter(|s| !s.is_empty()) {
        args.push("--resume".to_string());
        args.push(session_id.clone());
    } else if let Some(session_id) = ctx.new_session_id.as_ref().filter(|s| !s.is_empty()) {
        args.push("--session-id".to_string());
        args.push(session_id.clone());
    }
    args.push("--permission-mode".to_string());
    args.push(
        options
            .sandbox
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("bypassPermissions")
            .to_string(),
    );
    args
}

pub const CLAUDE_AGENT_DEF: RuntimeAgentDef = RuntimeAgentDef {
    id: "claude",
    name: "Claude Code",
    bin: "claude",
    fallback_bins: &["openclaude"],
    version_args: &["--version"],
    auth_probe_args: Some(&["auth", "status"]),
    fallback_models: FALLBACK_MODELS,
    reasoning_options: REASONING,
    list_models_args: None,
    list_models_timeout_secs: Some(10),
    models_from_stderr: false,
    model_probe: Some(ModelProbeStrategy::ClaudeInit),
    model_probe_args: None,
    slash_strategy: super::super::types::SlashStrategy::ClaudeInit,
    env: &[],
    max_prompt_arg_bytes: None,
    prompt_via_stdin: true,
    prompt_input_format: PromptInputFormat::StreamJson,
    stream_format: StreamFormat::ClaudeStreamJson,
    resumes_session_via_cli: true,
    supports_native_image: true,
    image_mime_whitelist: &["image/jpeg", "image/png", "image/gif", "image/webp"],
    build_args: build_claude_args,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_build_args_includes_resume_and_add_dir() {
        let args = build_claude_args(
            &RuntimeContext {
                extra_allowed_dirs: vec!["/skills".to_string()],
                resume_session_id: Some("sess-1".to_string()),
                new_session_id: None,
                include_partial_messages: true,
            },
            &RuntimeBuildOptions {
                model: Some("sonnet".to_string()),
                reasoning: None,
                sandbox: None,
            },
            None,
        );
        assert!(args.contains(&"--resume".to_string()));
        assert!(args.contains(&"sess-1".to_string()));
        assert!(args.contains(&"--add-dir".to_string()));
        assert!(args.contains(&"/skills".to_string()));
        assert!(args.contains(&"--model".to_string()));
    }

    #[test]
    fn claude_build_args_passes_effort_when_set() {
        let mk = |reasoning: Option<&str>| {
            build_claude_args(
                &RuntimeContext {
                    extra_allowed_dirs: vec![],
                    resume_session_id: None,
                    new_session_id: None,
                    include_partial_messages: false,
                },
                &RuntimeBuildOptions {
                    model: None,
                    reasoning: reasoning.map(str::to_string),
                    sandbox: None,
                },
                None,
            )
        };
        let high = mk(Some("high"));
        assert!(high.windows(2).any(|w| w == ["--effort", "high"]));
        // "default" / none → no --effort (Claude uses its own default).
        assert!(!mk(Some("default")).contains(&"--effort".to_string()));
        assert!(!mk(None).contains(&"--effort".to_string()));
    }

    #[test]
    fn claude_build_args_permission_mode_from_sandbox() {
        let mk = |sandbox: Option<&str>| {
            build_claude_args(
                &RuntimeContext {
                    extra_allowed_dirs: vec![],
                    resume_session_id: None,
                    new_session_id: None,
                    include_partial_messages: false,
                },
                &RuntimeBuildOptions {
                    model: None,
                    reasoning: None,
                    sandbox: sandbox.map(str::to_string),
                },
                None,
            )
        };
        assert!(mk(Some("plan"))
            .windows(2)
            .any(|w| w == ["--permission-mode", "plan"]));
        // Unset → defaults to bypassPermissions so headless tools still work.
        assert!(mk(None)
            .windows(2)
            .any(|w| w == ["--permission-mode", "bypassPermissions"]));
    }
}
