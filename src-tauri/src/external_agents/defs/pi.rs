use super::super::types::{
    PromptInputFormat, RuntimeAgentDef, RuntimeBuildOptions, RuntimeContext, StreamFormat,
};

const FALLBACK_MODELS: &[(&str, &str)] = &[
    // Pi's real models are user-configured and discovered via `pi --list-models`; if that fails
    // we only offer Default rather than inventing provider models the user never set up.
    ("default", "Default"),
];

const REASONING: &[(&str, &str)] = &[
    ("default", "Default"),
    ("off", "Off"),
    ("minimal", "Minimal"),
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("xhigh", "XHigh"),
];

pub fn build_pi_args(
    ctx: &RuntimeContext,
    options: &RuntimeBuildOptions,
    _prompt: Option<&str>,
) -> Vec<String> {
    let mut args = vec!["--mode".to_string(), "rpc".to_string()];
    // pi 原生会话：`--session-id <id>` 语义天然幂等——不存在则创建、存在则续接。首轮
    // resolve_agent_resume_context 给出 new_session_id（新 uuid，落盘 external-agent-sessions），
    // 后续轮给出 resume_session_id（同一 id），两种情况都用这同一个 flag。pi 每轮 spawn，
    // 靠这个 id 让 CLI 自己从原生会话文件恢复历史，Kivio 不再重放 transcript。
    if let Some(session_id) = ctx
        .resume_session_id
        .as_ref()
        .or(ctx.new_session_id.as_ref())
        .filter(|s| !s.is_empty())
    {
        args.push("--session-id".to_string());
        args.push(session_id.clone());
    }
    if let Some(model) = options
        .model
        .as_ref()
        .filter(|m| *m != "default" && !m.is_empty())
    {
        args.push("--model".to_string());
        args.push(model.clone());
    }
    if let Some(reasoning) = options
        .reasoning
        .as_ref()
        .filter(|r| *r != "default" && !r.is_empty())
    {
        args.push("--thinking".to_string());
        args.push(reasoning.clone());
    }
    // 注意：pi 无「授权目录」flag（`--help` 无 --add-dir/allowed-dir 等价项，只有 --approve 信任
    // 项目本地文件）。此前把 extra_allowed_dirs 塞进 `--append-system-prompt` 是误用——该 flag
    // 是「向系统提示追加文本/文件内容」，会把目录路径当提示词写进去，既不授权也污染上下文。
    // 附件目录路径已在 prompt 文本的附件说明块里给出，pi 靠自身文件权限模型读取，无需此处注入。
    args
}

pub const PI_AGENT_DEF: RuntimeAgentDef = RuntimeAgentDef {
    id: "pi",
    name: "Pi",
    bin: "pi",
    fallback_bins: &[],
    version_args: &["--version"],
    auth_probe_args: None,
    fallback_models: FALLBACK_MODELS,
    reasoning_options: REASONING,
    list_models_args: Some(&["--list-models"]),
    list_models_timeout_secs: Some(20),
    models_from_stderr: true,
    model_probe: None,
    model_probe_args: None,
    slash_strategy: super::super::types::SlashStrategy::PiRpc,
    env: &[],
    max_prompt_arg_bytes: None,
    prompt_via_stdin: true,
    prompt_input_format: PromptInputFormat::Text,
    stream_format: StreamFormat::PiRpc,
    resumes_session_via_cli: true,
    supports_native_image: false,
    supports_steering: false,
    image_mime_whitelist: &[],
    build_args: build_pi_args,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_build_args_rpc_mode_and_thinking() {
        let args = build_pi_args(
            &RuntimeContext {
                extra_allowed_dirs: vec!["/skills".to_string()],
                resume_session_id: None,
                new_session_id: None,
                include_partial_messages: false,
            },
            &RuntimeBuildOptions {
                model: Some("anthropic/claude-sonnet-4-5".to_string()),
                reasoning: Some("high".to_string()),
                sandbox: None,
            },
            None,
        );
        assert!(args.contains(&"rpc".to_string()));
        assert!(args.contains(&"--thinking".to_string()));
        // extra_allowed_dirs 不再被塞进 --append-system-prompt（pi 无授权目录 flag）。
        assert!(!args.contains(&"--append-system-prompt".to_string()));
        assert!(!args.contains(&"/skills".to_string()));
        // 无 session id 时不带 --session-id。
        assert!(!args.contains(&"--session-id".to_string()));
    }

    #[test]
    fn pi_build_args_passes_new_session_id_on_first_turn() {
        let args = build_pi_args(
            &RuntimeContext {
                extra_allowed_dirs: vec![],
                resume_session_id: None,
                new_session_id: Some("sess-new".to_string()),
                include_partial_messages: false,
            },
            &RuntimeBuildOptions {
                model: None,
                reasoning: None,
                sandbox: None,
            },
            None,
        );
        assert!(args.windows(2).any(|w| w == ["--session-id", "sess-new"]));
    }

    #[test]
    fn pi_build_args_resumes_via_session_id_on_later_turn() {
        let args = build_pi_args(
            &RuntimeContext {
                extra_allowed_dirs: vec![],
                // A resume takes precedence over any new id, and both map to the same flag.
                resume_session_id: Some("sess-existing".to_string()),
                new_session_id: None,
                include_partial_messages: false,
            },
            &RuntimeBuildOptions {
                model: None,
                reasoning: None,
                sandbox: None,
            },
            None,
        );
        assert!(args
            .windows(2)
            .any(|w| w == ["--session-id", "sess-existing"]));
    }
}
