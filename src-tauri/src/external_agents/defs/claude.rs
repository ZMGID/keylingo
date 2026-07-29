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
) -> Vec<String> {    let mut args = vec![
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

/// `--append-system-prompt-file <path>`：把 Kivio 的会话级系统指令**追加**到 claude 原生
/// 系统提示之后（不替换）。
///
/// **隐藏 flag**（`--help` 里没有），claude 2.1.220 本机零副作用探针确认存在：
/// 不给值时报 `error: option '--append-system-prompt-file <file>' argument missing`，
/// 而对照的胡编 flag 报 `error: unknown option '--totally-bogus-flag-xyz'`。
///
/// **为什么用 file 形式而不是内联字符串**：Windows 命令行有 32767 字符上限，含 Memory 块的
/// instructions 可能超；npm 安装的用户拿到的是 `claude.cmd`，长参数在批处理转义那层还有风险。
///
/// 独立成函数（而不是给 `RuntimeContext` 加字段）：那是所有 CLI 共用的结构，为一个 claude
/// 专属 flag 加字段要牵动全部 def 的构造点与单测；`build_args` 的签名也一样共用。
pub fn append_system_prompt_file_args(path: &std::path::Path) -> Vec<String> {
    vec![
        "--append-system-prompt-file".to_string(),
        path.to_string_lossy().to_string(),
    ]
}

/// 把 claude 的启动参数改写成「续接 `session_id` 这个原生会话」：先摘掉已有的
/// `--session-id <x>` / `--resume <x>`，再追加 `--resume <session_id>`。
///
/// **为什么需要**：claude 的会话 id 走**启动 flag**（不像 codex/ACP 在握手 RPC 里传），
/// 所以常驻会话的重连必须改参数。而且**只能在首次创建时用 `--session-id`**：同一个 id
/// 再 `--session-id` 一次会被 claude 以「id 已存在」拒绝启动，`--resume` 才是「接着聊」。
/// 常驻改造前每轮都新起进程、每轮都由 `resolve_agent_resume_context` 决定用哪个 flag，
/// 所以这个改写点此前不存在。
pub fn claude_args_resuming(args: &[String], session_id: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(args.len() + 2);
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--session-id" || arg == "--resume" {
            // 成对出现，值一起摘掉（值本身可能长得像别的东西，绝不能只摘 flag）。
            skip_next = true;
            continue;
        }
        out.push(arg.clone());
    }
    out.push("--resume".to_string());
    out.push(session_id.to_string());
    out
}

/// 从启动参数里读回 claude 的原生 session id（`--session-id <id>` 或 `--resume <id>`）。
///
/// codex / ACP 的 native id 是握手响应给的；claude 的是我们自己在参数里放进去的，
/// 所以重连时只能从参数读回来（`system/init` / `result` 的 `session_id` 会在第一轮覆盖它）。
pub fn claude_session_id_from_args(args: &[String]) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == "--session-id" || pair[0] == "--resume")
        .map(|pair| pair[1].clone())
        .filter(|id| !id.is_empty())
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

    /// `--append-system-prompt-file` 必须是「flag 后紧跟路径」的成对形式，
    /// 且是 append 语义（不替换 claude 原生系统提示）。
    #[test]
    fn append_system_prompt_file_args_pair_the_flag_with_the_path() {
        let args = append_system_prompt_file_args(std::path::Path::new("/tmp/kivio-extsys-c1.md"));
        assert_eq!(
            args,
            vec![
                "--append-system-prompt-file".to_string(),
                "/tmp/kivio-extsys-c1.md".to_string(),
            ]
        );
        // 拼到 build_args 之后仍是相邻的一对（顺序无关，但成对不可拆）。
        let mut full = build_claude_args(
            &RuntimeContext {
                extra_allowed_dirs: vec![],
                resume_session_id: None,
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
        full.extend(append_system_prompt_file_args(std::path::Path::new(
            "/tmp/x.md",
        )));
        assert!(full
            .windows(2)
            .any(|w| w == ["--append-system-prompt-file", "/tmp/x.md"]));
    }

    /// Windows 路径（含反斜杠与空格）必须原样传，不做任何转义/引号包裹——
    /// 它是 argv 里的一个独立元素，加引号反而会让 CLI 找不到文件。
    #[test]
    fn append_system_prompt_file_keeps_windows_paths_verbatim() {
        let args = append_system_prompt_file_args(std::path::Path::new(
            r"C:\Users\a b\AppData\Local\Temp\kivio-extsys-c1.md",
        ));
        assert_eq!(args[1], r"C:\Users\a b\AppData\Local\Temp\kivio-extsys-c1.md");
    }

    #[test]
    fn claude_build_args_permission_mode_from_sandbox() {        let mk = |sandbox: Option<&str>| {            build_claude_args(
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

    // ---- B1：常驻会话的重连参数改写 ----

    fn args_with(session_flag: &str, id: &str) -> Vec<String> {
        build_claude_args(
            &RuntimeContext {
                extra_allowed_dirs: vec!["/skills".to_string()],
                resume_session_id: (session_flag == "--resume").then(|| id.to_string()),
                new_session_id: (session_flag == "--session-id").then(|| id.to_string()),
                include_partial_messages: true,
            },
            &RuntimeBuildOptions {
                model: Some("opus".to_string()),
                reasoning: None,
                sandbox: None,
            },
            None,
        )
    }

    /// 首次是 `--session-id <new>`；重连必须改成 `--resume <同一个 id>`——
    /// 同一个 id 再 `--session-id` 一次会被 claude 以「id 已存在」拒绝启动。
    #[test]
    fn reconnect_args_turn_session_id_into_resume() {
        let first = args_with("--session-id", "sess-1");
        let again = claude_args_resuming(&first, "sess-1");
        assert!(
            !again.contains(&"--session-id".to_string()),
            "重连仍带 --session-id，claude 会拒绝启动：{again:?}"
        );
        assert!(again.windows(2).any(|w| w == ["--resume", "sess-1"]));
        // 其余参数必须原样保留（模型 / allowed dir / partial messages / permission mode）。
        for expected in [
            "--include-partial-messages",
            "--model",
            "opus",
            "--add-dir",
            "/skills",
            "--permission-mode",
        ] {
            assert!(again.contains(&expected.to_string()), "丢了 {expected}");
        }
    }

    /// 已经是 `--resume` 时不得出现两份（第二次重连也走同一个函数）。
    #[test]
    fn reconnect_args_do_not_duplicate_resume() {
        let again = claude_args_resuming(&args_with("--resume", "sess-1"), "sess-1");
        assert_eq!(
            again.iter().filter(|a| *a == "--resume").count(),
            1,
            "{again:?}"
        );
        assert!(again.windows(2).any(|w| w == ["--resume", "sess-1"]));
    }

    /// 值必须跟着 flag 一起摘掉：只摘 flag 会把裸 id 留成一个非法位置参数。
    #[test]
    fn reconnect_args_drop_the_old_id_value_too() {
        let again = claude_args_resuming(&args_with("--session-id", "old-id"), "new-id");
        assert!(!again.contains(&"old-id".to_string()), "{again:?}");
        assert!(again.windows(2).any(|w| w == ["--resume", "new-id"]));
    }

    #[test]
    fn session_id_is_read_back_from_args() {
        assert_eq!(
            claude_session_id_from_args(&args_with("--session-id", "sess-a")).as_deref(),
            Some("sess-a")
        );
        assert_eq!(
            claude_session_id_from_args(&args_with("--resume", "sess-b")).as_deref(),
            Some("sess-b")
        );
        // 没有任何会话 flag（探测路径可能这样）时不得编一个出来。
        let bare = build_claude_args(
            &RuntimeContext {
                extra_allowed_dirs: vec![],
                resume_session_id: None,
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
        assert_eq!(claude_session_id_from_args(&bare), None);
    }
}
