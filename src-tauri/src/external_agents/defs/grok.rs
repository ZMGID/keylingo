//! Grok CLI (xAI "Grok Build") external agent definition.
//!
//! Grok speaks the Agent Client Protocol over `grok agent stdio` (verified against v0.2.103):
//! `initialize` → `session/new` (result carries `models.availableModels`) → `session/set_model`
//! → `session/prompt`, with `agent_thought_chunk` / `agent_message_chunk` / `tool_call` /
//! `tool_call_update` session updates and an async `available_commands_update` push — all
//! shapes the shared ACP driver already handles. It differs from the acp_def family in two
//! ways, which is why it gets its own def instead of another data row there:
//! - reasoning effort is a launch flag (`grok agent --reasoning-effort <level> stdio`), so it
//!   has non-empty `reasoning_options` and a dynamic `build_args`;
//! - the model can also be pinned at launch via `-m` (session/set_model still works and is
//!   harmless duplication for the fresh-session path).

use super::super::types::{
    ModelProbeStrategy, PromptInputFormat, RuntimeAgentDef, RuntimeBuildOptions, RuntimeContext,
    SlashStrategy, StreamFormat,
};

const FALLBACK_MODELS: &[(&str, &str)] = &[("default", "Default"), ("grok-4.5", "Grok 4.5")];

/// Grok reasoning-effort levels (`grok agent --reasoning-effort`). The CLI also accepts
/// none/minimal/xhigh/max, but the models it ships advertise low/medium/high — keep the
/// menu to what the server actually differentiates.
const REASONING: &[(&str, &str)] = &[
    ("default", "Default"),
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
];

pub fn build_grok_args(
    _ctx: &RuntimeContext,
    options: &RuntimeBuildOptions,
    _prompt: Option<&str>,
) -> Vec<String> {
    let mut args = vec!["agent".to_string()];
    if let Some(model) = options
        .model
        .as_ref()
        .filter(|m| *m != "default" && !m.is_empty())
    {
        args.push("-m".to_string());
        args.push(model.clone());
    }
    if let Some(effort) = options
        .reasoning
        .as_ref()
        .filter(|r| *r != "default" && !r.is_empty())
    {
        args.push("--reasoning-effort".to_string());
        args.push(effort.clone());
    }
    // Headless driver auto-answers permission requests anyway; --always-approve skips the
    // round-trips entirely (same spirit as claude's bypassPermissions default).
    args.push("--always-approve".to_string());
    args.push("stdio".to_string());
    args
}

pub const GROK_AGENT_DEF: RuntimeAgentDef = RuntimeAgentDef {
    id: "grok",
    name: "Grok CLI",
    bin: "grok",
    fallback_bins: &[],
    version_args: &["--version"],
    // `grok models` exits 0 when logged in (served from local cache), 1 when unauthenticated.
    auth_probe_args: Some(&["models"]),
    fallback_models: FALLBACK_MODELS,
    reasoning_options: REASONING,
    list_models_args: None,
    list_models_timeout_secs: Some(15),
    models_from_stderr: false,
    model_probe: Some(ModelProbeStrategy::Acp),
    model_probe_args: Some(&["agent", "stdio"]),
    slash_strategy: SlashStrategy::Acp,
    env: &[],
    max_prompt_arg_bytes: None,
    prompt_via_stdin: false,
    prompt_input_format: PromptInputFormat::Text,
    stream_format: StreamFormat::AcpJsonRpc,
    // Cross-turn resume goes through the ACP live-session registry (session/load;
    // grok advertises loadSession: true), same as the other ACP agents.
    resumes_session_via_cli: false,
    supports_native_image: true,
    image_mime_whitelist: &[],
    build_args: build_grok_args,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> RuntimeContext {
        RuntimeContext {
            extra_allowed_dirs: vec![],
            resume_session_id: None,
            new_session_id: None,
            include_partial_messages: false,
        }
    }

    #[test]
    fn grok_build_args_pins_model_and_effort_before_stdio() {
        let args = build_grok_args(
            &ctx(),
            &RuntimeBuildOptions {
                model: Some("grok-4.5".to_string()),
                reasoning: Some("high".to_string()),
                sandbox: None,
            },
            None,
        );
        assert_eq!(args.first().map(String::as_str), Some("agent"));
        assert_eq!(args.last().map(String::as_str), Some("stdio"));
        assert!(args.windows(2).any(|w| w == ["-m", "grok-4.5"]));
        assert!(args.windows(2).any(|w| w == ["--reasoning-effort", "high"]));
        assert!(args.contains(&"--always-approve".to_string()));
    }

    #[test]
    fn grok_build_args_defaults_omit_model_and_effort() {
        let args = build_grok_args(
            &ctx(),
            &RuntimeBuildOptions {
                model: Some("default".to_string()),
                reasoning: None,
                sandbox: None,
            },
            None,
        );
        assert_eq!(args, vec!["agent", "--always-approve", "stdio"]);
    }

    #[test]
    fn grok_def_uses_acp_protocol() {
        assert!(matches!(
            GROK_AGENT_DEF.stream_format,
            StreamFormat::AcpJsonRpc
        ));
        assert!(matches!(
            GROK_AGENT_DEF.model_probe,
            Some(ModelProbeStrategy::Acp)
        ));
        assert_eq!(
            GROK_AGENT_DEF.model_probe_args,
            Some(&["agent", "stdio"][..])
        );
        assert!(matches!(GROK_AGENT_DEF.slash_strategy, SlashStrategy::Acp));
    }

    /// Live end-to-end over the real grok CLI: detection (binary + auth + ACP model probe)
    /// then one ACP prompt turn through the shared driver. Requires a logged-in `grok` on
    /// PATH + network. Run: `cargo test grok_live_smoke -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "requires live grok login + network"]
    async fn grok_live_smoke() {
        use crate::external_agents::detection::detect_single_agent;
        use crate::external_agents::session::acp::run_acp_session;
        use crate::external_agents::spawn::{resolve_binary, spawn_agent};
        use crate::external_agents::types::UnifiedAgentEvent;

        let cwd = std::env::temp_dir();
        let detected = detect_single_agent(&GROK_AGENT_DEF, &cwd).await;
        eprintln!(
            "grok detected: available={} version={:?} auth={:?} models={:?}",
            detected.available,
            detected.version,
            detected.auth_status,
            detected.models.iter().map(|m| &m.id).collect::<Vec<_>>()
        );
        assert!(detected.available, "grok binary not found on PATH");
        assert_eq!(detected.auth_status.as_deref(), Some("ok"));
        assert!(
            detected.models.len() > 1,
            "ACP model probe returned nothing"
        );

        let bin = resolve_binary(&GROK_AGENT_DEF).await.expect("resolve grok");
        let args = build_grok_args(
            &ctx(),
            &RuntimeBuildOptions {
                model: None,
                reasoning: Some("low".to_string()),
                sandbox: None,
            },
            None,
        );
        let mut spawned = spawn_agent(&GROK_AGENT_DEF, &bin, &args, &cwd, &Default::default())
            .await
            .expect("spawn grok");
        let events = std::cell::RefCell::new(Vec::<UnifiedAgentEvent>::new());
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            run_acp_session(
                &mut spawned.child,
                "Reply with exactly the token GROK_SMOKE_OK and nothing else.",
                &cwd,
                None,
                &[],
                |event| events.borrow_mut().push(event),
                || false,
            ),
        )
        .await;
        let _ = spawned.child.start_kill();
        let captured = events.into_inner();
        eprintln!("grok smoke: {} events, result={result:?}", captured.len());
        let text: String = captured
            .iter()
            .filter_map(|e| match e {
                UnifiedAgentEvent::TextDelta { delta } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        eprintln!("grok smoke text: {text:?}");
        assert!(matches!(result, Ok(Ok(()))), "ACP turn failed: {result:?}");
        assert!(text.contains("GROK_SMOKE_OK"), "got: {text:?}");
    }
}
