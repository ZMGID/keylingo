use super::super::types::{
    ModelProbeStrategy, PromptInputFormat, RuntimeAgentDef, RuntimeBuildOptions, RuntimeContext,
    StreamFormat,
};

const FALLBACK_MODELS: &[(&str, &str)] = &[("default", "Default")];

/// Claude Code 的思考档位。大部分走 `--effort <level>`，两个例外见 `claude_thinking_args`。
///
/// 全部取值均为 claude 2.1.220 本机核实（spec 第 12 条）：
/// - `--effort` 的 `--help` 明写「(low, medium, high, xhigh, max)」；
/// - `off` 走 `--thinking disabled`（**隐藏 flag**，见 `claude_thinking_args` 的说明）；
/// - `ultracode` 是 `--effort` 认的一个别名，实测 CLI 自己的 `/effort` 用法串就是
///   `Usage: /effort <low|medium|high|xhigh|max|ultracode|auto>`。
const REASONING: &[(&str, &str)] = &[
    ("default", "Default"),
    ("off", "Off (no thinking)"),
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("xhigh", "Extra high"),
    ("max", "Max"),
    ("ultracode", "Ultracode"),
];

/// 一个思考档位对应的启动参数。纯函数，便于单测拼装结果。
///
/// 三条分支，每条的语义都在 claude 2.1.220 上核实过（spec 第 12 条：传 flag 前先核实）：
///
/// 1. **`default` / 空 ⇒ 不传任何东西**，让 CLI 用它自己的默认档（每个模型的
///    `default_effort` 不同，我们不该替它决定）。
///
/// 2. **`off` ⇒ `--thinking disabled`**。`--thinking <mode>` 是**隐藏 flag**（`--help`
///    里没有），零副作用探针确认存在：不给值报 `error: option '--thinking <mode>'
///    argument missing`，而胡编的 flag 报 `error: unknown option '--totally-bogus-flag-xyz'`。
///    二进制里的 commander 定义是
///    `new id("--thinking <mode>", "Thinking mode: enabled (equivalent to adaptive), disabled")
///     .choices(["enabled","adaptive","disabled"]).hideHelp()`，
///    官方 Agent SDK 也是用 `{type:"disabled"}` → `--thinking disabled`。
///    真机验收（同一 prompt、同一模型）：`--thinking enabled` 得到 2 个 thinking 块 / 3 条
///    thinking delta，`--thinking disabled` 得到 **0 / 0**。
///    **这一档不传 `--effort`**：语义是「不要思考」，再叠一个思考强度自相矛盾。
///
/// 3. **其余 ⇒ `--effort <值>`**，含 `ultracode`。
///    `ultracode` 不是独立 flag（`--ultracode` 实测不存在），而是 `--effort` 的一个取值：
///    二进制里 `cUc={ultracode:"xhigh"}` 把它映射成 xhigh 强度，同时
///    `ultracode:_Jn(cli.effort)` 用同一个字符串把会话的 ultracode 状态
///    （xhigh + 常驻 dynamic-workflow 编排）置位。
///    真机验收：`--effort totallybogus` 会在 stderr 打
///    `Warning: Unknown --effort value 'totallybogus' — ignoring it…`，
///    而 `--effort ultracode` / `xhigh` / `max` **一句警告都没有**。
///    **所以不需要 `--settings`**：官方文档式的做法是往 `--settings` 传 `{"ultracode":true}`，
///    但那要落一个临时配置文件、还要论证它不会盖掉用户 `~/.claude/settings.json`；
///    `--effort ultracode` 是 CLI 自己认的同义写法，零额外文件、零配置污染面。
pub fn claude_thinking_args(reasoning: &str) -> Vec<String> {
    let value = reasoning.trim().to_ascii_lowercase();
    match value.as_str() {
        "" | "default" => Vec::new(),
        "off" => vec!["--thinking".to_string(), "disabled".to_string()],
        _ => vec!["--effort".to_string(), value],
    }
}

/// 把探测用的启动参数改成「不落盘会话」：追加 `--no-session-persistence`。
///
/// **为什么必须有**：查模型列表 / 斜杠命令时我们会起一个一次性 claude 子进程，只为读它的
/// `system/init`。claude 会把这次启动当成一个真实会话记到
/// `~/.claude/projects/<cwd 编码>/<session-id>.jsonl` 里，于是用户自己的 claude 会话列表被
/// 一堆只含一条 `"."` 的空壳会话污染（kimi 侧实测某目录下 53 个会话有 52 个是这种残渣，
/// 见 spec 第 11b / 14f 条）。
///
/// **真机核实**（claude 2.1.220，探测 cwd = `chat-workspaces/__global__`，
/// 数 `~/.claude/projects/C--…-chat-workspaces---global--/` 下的文件数）：
/// 不带这个 flag、探测存活到 init 之后 5s 再收尾 ⇒ 目录里**多出一个**以我们的 session id
/// 命名的 `.jsonl`；带上它 ⇒ **一个都不多**，而 `system/init` 照样按时到达（两次都是 ~3.3s）
/// 且 `slash_commands` 字段完整（斜杠命令探测不受影响）。
/// （`--help` 标注「only works with --print」，我们的探测本来就带 `-p`。）
///
/// **绝不能加到真实回复的路径上**：那会让用户的对话不落盘、`--resume` 直接失效。
/// 所以做成一个只在探测入口调用的独立函数，而不是塞进 `build_claude_args`。
pub fn ephemeral_probe_args(args: &[String]) -> Vec<String> {
    let mut out = args.to_vec();
    if !out.iter().any(|arg| arg == NO_SESSION_PERSISTENCE) {
        out.push(NO_SESSION_PERSISTENCE.to_string());
    }
    out
}

const NO_SESSION_PERSISTENCE: &str = "--no-session-persistence";

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
    // 档位 → flag 的映射交给 `claude_thinking_args`：并非每一档都是 `--effort`
    // （「关闭思考」走 `--thinking disabled`）。
    if let Some(reasoning) = options
        .reasoning
        .as_ref()
        .filter(|r| *r != "default" && !r.is_empty())
    {
        args.extend(claude_thinking_args(reasoning));
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
    claude_args_with_session_flag(args, "--resume", session_id)
}

/// 把启动参数改写成「开一个**全新**会话」：摘掉已有的 `--session-id <x>` / `--resume <x>`，
/// 换成 `--session-id <session_id>`。
///
/// **用途只有一个**：`--resume` 的目标会话在 claude 那边已经不存在了
/// （`No conversation found with session ID`，见 `stream::claude::is_missing_session_error`）。
/// 这时拿同一份参数重连是必然再失败一次，正确处置是换个新 id 从空上下文继续，
/// 并告诉用户上下文已重置。
///
/// 新 id 必须是**没用过的**（调用方给一个新 UUID）：claude 对 `--session-id` 的要求是
/// 「这个 id 还不存在」，把那个刚刚被判定为「找不到」的 id 再拿来用属于赌它的失败原因
/// 一定是「文件真的没了」而不是「文件在但读不了」——不值得赌。
pub fn claude_args_fresh_session(args: &[String], session_id: &str) -> Vec<String> {
    claude_args_with_session_flag(args, "--session-id", session_id)
}

/// 摘掉全部会话 flag（连值一起）再追加 `flag value`。
///
/// 两个改写口（续接 / 开新）共用这一份：值本身可能长得像别的东西，只摘 flag 会把裸 id
/// 留成非法位置参数，这个坑不该有第二份实现（spec 第 2 条）。
fn claude_args_with_session_flag(args: &[String], flag: &str, session_id: &str) -> Vec<String> {
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
    out.push(flag.to_string());
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

    /// 「关闭思考」这一档必须走 `--thinking disabled`，而**不是** `--effort off`
    /// （`off` 不是 `--effort` 的合法取值，CLI 会打一句 warning 后按默认档跑 —— 也就是
    /// 用户点了「关闭思考」却照样在思考，且没有任何可见信号）。
    #[test]
    fn thinking_off_uses_the_thinking_flag_not_effort() {
        let args = claude_thinking_args("off");
        assert_eq!(args, vec!["--thinking".to_string(), "disabled".to_string()]);
        // 这一档绝不能同时带 `--effort`：「不要思考」再叠思考强度自相矛盾。
        assert!(!args.contains(&"--effort".to_string()));

        let full = build_claude_args(
            &RuntimeContext {
                extra_allowed_dirs: vec![],
                resume_session_id: None,
                new_session_id: None,
                include_partial_messages: false,
            },
            &RuntimeBuildOptions {
                model: None,
                reasoning: Some("off".to_string()),
                sandbox: None,
            },
            None,
        );
        assert!(full.windows(2).any(|w| w == ["--thinking", "disabled"]));
        assert!(!full.contains(&"--effort".to_string()));
    }

    /// `ultracode` 是 `--effort` 的取值（CLI 自己的 `/effort` 用法串里就有它），
    /// 不是独立 flag、也不需要 `--settings` 传配置键。
    #[test]
    fn ultracode_is_an_effort_value() {
        assert_eq!(
            claude_thinking_args("ultracode"),
            vec!["--effort".to_string(), "ultracode".to_string()]
        );
        // 不得凭空引入 `--ultracode`（实测不存在）或 `--settings`（会牵扯用户配置层）。
        let args = claude_thinking_args("ultracode");
        assert!(!args.iter().any(|a| a == "--ultracode" || a == "--settings"));
    }

    /// 档位表里每一项都必须能拼出合法参数，且 REASONING 的 id 与拼装分支不会分叉。
    #[test]
    fn every_reasoning_option_maps_to_flags() {
        for (id, label) in REASONING {
            assert!(!label.is_empty(), "{id} 缺 label");
            let args = claude_thinking_args(id);
            match *id {
                "default" => assert!(args.is_empty(), "default 不该传 flag：{args:?}"),
                "off" => assert_eq!(args[0], "--thinking"),
                _ => {
                    assert_eq!(args[0], "--effort");
                    assert_eq!(args[1], *id);
                }
            }
            // flag 与值必须成对，不能出现落单的 flag。
            assert!(args.len() % 2 == 0, "{id} 参数不成对：{args:?}");
        }
    }

    /// 大小写 / 空白不该让档位掉进「未知取值」——CLI 对未知 `--effort` 只打一句 warning，
    /// 静默降级成默认档。
    #[test]
    fn reasoning_values_are_normalized() {
        assert_eq!(
            claude_thinking_args("  Off  "),
            vec!["--thinking".to_string(), "disabled".to_string()]
        );
        assert_eq!(
            claude_thinking_args("XHigh"),
            vec!["--effort".to_string(), "xhigh".to_string()]
        );
        assert!(claude_thinking_args("   ").is_empty());
    }

    /// 探测参数必须带 `--no-session-persistence`（否则每次探测都在用户的 claude
    /// 会话列表里留一个空壳会话），且**只有探测**带 —— `build_claude_args` 自身绝不能带，
    /// 那会让真实对话不落盘、`--resume` 直接失效。
    #[test]
    fn probe_args_disable_session_persistence_but_reply_args_do_not() {
        let reply = build_claude_args(
            &RuntimeContext {
                extra_allowed_dirs: vec![],
                resume_session_id: None,
                new_session_id: Some("sess-1".to_string()),
                include_partial_messages: true,
            },
            &RuntimeBuildOptions {
                model: None,
                reasoning: None,
                sandbox: None,
            },
            None,
        );
        assert!(
            !reply.contains(&"--no-session-persistence".to_string()),
            "回复路径带上了不落盘 flag —— 用户的对话会消失、--resume 失效：{reply:?}"
        );

        let probe = ephemeral_probe_args(&reply);
        assert!(probe.contains(&"--no-session-persistence".to_string()));
        // 其余参数原样保留（探测靠 `-p` + stream-json 才能读到 system/init）。
        for expected in &reply {
            assert!(probe.contains(expected), "探测参数丢了 {expected}");
        }
        // 幂等：重复套用不得出现两份。
        let twice = ephemeral_probe_args(&probe);
        assert_eq!(
            twice
                .iter()
                .filter(|a| *a == "--no-session-persistence")
                .count(),
            1,
            "{twice:?}"
        );
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
