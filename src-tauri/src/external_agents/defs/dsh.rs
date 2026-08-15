//! DeepSeek Harness (`dsh`) —— 通过 SDK JSON-RPC stdio 协议驱动。
//!
//! # 为什么不是 ACP 家族的一条数据行
//!
//! dsh 确实有 ACP 服务端（`@deepseek-ai/dsh-acp`），但它的 README 与本机实测都指向同一个
//! 结论：那是 automation-only 的桥，**只发 committed 的 `agent_message_chunk`**。实测
//! （0.1.0-rc.6）确认它没有 token 级 delta、没有 reasoning、没有 tool call，
//! `session/load` 直接回 `-32601 Method not found`。接进 Kivio 的话工具卡与思考块永远是空的，
//! 等于把一个 agent harness 退化成问答框。
//!
//! 所以走 `@deepseek-ai/dsh-sdk-jsonrpc-server`（`StreamFormat::DshJsonRpc`）：同一个 CLI，
//! 另一条协议，把**整条会话事件流**推过来 —— 本机实测拿到的事件类型是
//! `turn/start` `step/start` `user/message` `request/header` `request/context`
//! `assistant/chunk`(text-delta / reasoning-delta / usage / finish) `assistant/message`
//! `tool/call` `tool/result` `turn/end`，正好覆盖 `UnifiedAgentEvent` 的全部出口。
//!
//! # 启动形状
//!
//! dsh 没有「一条命令直接出流式 JSON」的模式，它**只能 boot profile**。所以启动参数是
//! `--profile <kivio profile>`，而那个 profile 由 `dsh_profile.rs` 生成并维护（Kivio 自己的
//! 目录，绝不改用户的 `profiles/web` 或家目录 `cordis.patch.yml`）。
//!
//! 模型 / 推理档位不是启动 flag，而是 `initialize` 的 RPC 参数（模型）与 Kivio profile
//! `cordis.patch.yml` 中的 `llm-deepseek.reasoningEffort`（档位）—— 见 `dsh_profile.rs`。

use super::super::types::{
    ExternalCliSlashCommand, PromptInputFormat, RuntimeAgentDef, RuntimeBuildOptions,
    RuntimeContext, SlashStrategy, StreamFormat,
};

/// Commands the Kivio bridge can actually execute via `ctx.commands.execute`.
/// Official web-only entries (`export` / `model` / `permission` / `plan`) stay
/// off the menu — Kivio already has model / permission / mode pills.
const DSH_SLASH_COMMANDS: &[(&str, &str, Option<&str>)] = &[
    ("compact", "Compact older conversation history", None),
    (
        "feedback",
        "record feedback about this session",
        Some("<text>"),
    ),
    (
        "goal",
        "set or view the goal for a long-running task",
        Some("[<objective>|clear|edit <objective>|pause|resume]"),
    ),
];

pub fn builtin_slash_commands() -> Vec<ExternalCliSlashCommand> {
    DSH_SLASH_COMMANDS
        .iter()
        .map(|(name, description, hint)| ExternalCliSlashCommand {
            slash: format!("/{name}"),
            name: (*name).to_string(),
            description: Some((*description).to_string()),
            argument_hint: hint.map(|value| (*value).to_string()),
        })
        .collect()
}

/// dsh 自带 DeepSeek 适配器（`deepseek-official` 路由）的两个模型。真实列表由
/// `detection::read_dsh_settings_models` 从 `~/.dsh/settings.yaml` 读出（含用户在
/// `llm-pi-ai.providers` 里配的中转路由）；这张表只是读不到时的兜底，前端标「默认列表」。
const FALLBACK_MODELS: &[(&str, &str)] = &[
    ("default", "Default"),
    ("deepseek-v4-flash", "DeepSeek-V4-Flash"),
    ("deepseek-v4-pro", "DeepSeek-V4-Pro"),
];

/// dsh 的推理档位：`off | high | max`（`llm-deepseek` 适配器自报，实测
/// `resolveModelInfo` 返回 `efforts:[off,high,max], defaultEffort:high`）。
///
/// **注意它不是启动 flag，也不在 `initialize` 参数里** —— 唯一入口是 profile patch 里
/// `llm-deepseek.reasoningEffort`。因此换档位要重写 patch + 换进程，由
/// `session::dsh_jsonrpc::DshLaunchExtras` 折进启动指纹。
const REASONING: &[(&str, &str)] = &[
    ("default", "Default"),
    ("off", "Off"),
    ("high", "High"),
    ("max", "Max"),
];

/// `dsh --profile <name>`。
///
/// profile 名固定（`dsh_profile::KIVIO_PROFILE`）：它是 Kivio 私有的一份装配，与用户自己的
/// `web` / `tui` profile 并存互不影响。模型/档位都不进 argv，见模块注释。
pub fn build_dsh_args(
    _ctx: &RuntimeContext,
    _options: &RuntimeBuildOptions,
    _prompt: Option<&str>,
) -> Vec<String> {
    vec![
        "--profile".to_string(),
        crate::external_agents::dsh_profile::KIVIO_PROFILE.to_string(),
    ]
}

pub const DSH_AGENT_DEF: RuntimeAgentDef = RuntimeAgentDef {
    id: "dsh",
    name: "DeepSeek Harness",
    bin: "dsh",
    fallback_bins: &[],
    version_args: &["--version"],
    // `dsh` 没有登录态概念：凭据是 `~/.dsh/.credentials.yaml` / `.env` / 环境变量里的
    // `DEEPSEEK_API_KEY`，没有任何「查询登录状态」的子命令。探测认证只会白起一个进程。
    auth_probe_args: None,
    fallback_models: FALLBACK_MODELS,
    reasoning_options: REASONING,
    // 模型列表读配置文件，不起进程（见 `detection::probe_models` 的 dsh 分支）。
    list_models_args: None,
    list_models_timeout_secs: None,
    models_from_stderr: false,
    model_probe: None,
    model_probe_args: None,
    // 官方 `/` 菜单：只列能走 `session/command` 的条目；run_turn 拦截后交给 registry。
    slash_strategy: SlashStrategy::Dsh,
    // 遥测默认关：任何非空值都算关（上游的隐私开关刻意「误关优于误开」）。用户想开就
    // 在 `~/.dsh/.env` 里自己设 —— 那份 env 由 dsh 自己加载，覆盖不到这里。
    env: &[("DSH_TELEMETRY_DISABLED", "1")],
    max_prompt_arg_bytes: None,
    // prompt 走 JSON-RPC 的 `contentBlocks`，既不进 argv 也不进裸 stdin。
    prompt_via_stdin: false,
    prompt_input_format: PromptInputFormat::Text,
    stream_format: StreamFormat::DshJsonRpc,
    // 原生 session id 由 Kivio bridge 的 `session/open` 创建/恢复，不进 argv；因此这里仍不是
    // `resumes_session_via_cli`（该字段只描述 `--resume` 一类 CLI 参数形状）。
    resumes_session_via_cli: false,
    // `contentBlocks` 的 image 块要求先经 `ctx.attachments` 落库拿引用（`{type:"image",
    // attachment:{attachmentId,…}}`），裸 base64 上不了线，而那个服务在这条 RPC 上没有出口。
    // 于是与 pi / kimi 同路：降级成 prompt 文本里的路径说明。
    supports_native_image: false,
    // Bridge 暴露 cancel/resume，但没有追加输入的 `steer` RPC。
    supports_steering: false,
    image_mime_whitelist: &[],
    build_args: build_dsh_args,
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

    fn opts() -> RuntimeBuildOptions {
        RuntimeBuildOptions {
            model: None,
            reasoning: None,
            sandbox: None,
        }
    }

    #[test]
    fn launches_the_kivio_profile() {
        assert_eq!(
            (DSH_AGENT_DEF.build_args)(&ctx(), &opts(), None),
            vec!["--profile".to_string(), "kivio".to_string()]
        );
    }

    /// 模型与推理档位**不进 argv**：模型是 `initialize` 的 RPC 参数，档位是 profile patch 里的
    /// `llm-deepseek.reasoningEffort`。硬塞进 argv 会被 dsh 的启动器当作「要转交给 app 的
    /// 内部参数」原样传下去（`args.ts` 的 passthrough 语义），既不生效也不报错。
    #[test]
    fn model_and_reasoning_never_reach_argv() {
        let args = (DSH_AGENT_DEF.build_args)(
            &ctx(),
            &RuntimeBuildOptions {
                model: Some("deepseek-v4-pro".to_string()),
                reasoning: Some("max".to_string()),
                sandbox: Some("danger-full-access".to_string()),
            },
            Some("hello"),
        );
        assert_eq!(args, vec!["--profile".to_string(), "kivio".to_string()]);
    }

    /// 这条线的能力边界是**实测**出来的，不是保守估计。改任一项之前先按模块注释复验协议。
    #[test]
    fn declares_the_measured_protocol_limits() {
        assert!(matches!(
            DSH_AGENT_DEF.stream_format,
            StreamFormat::DshJsonRpc
        ));
        assert!(matches!(DSH_AGENT_DEF.slash_strategy, SlashStrategy::Dsh));
        assert!(!DSH_AGENT_DEF.supports_steering);
        assert!(!DSH_AGENT_DEF.supports_native_image);
        assert!(!DSH_AGENT_DEF.resumes_session_via_cli);
        assert!(DSH_AGENT_DEF.model_probe.is_none());
        assert!(DSH_AGENT_DEF.auth_probe_args.is_none());
    }

    #[test]
    fn lists_the_official_slash_menu() {
        let commands = builtin_slash_commands();
        let names: Vec<&str> = commands.iter().map(|command| command.name.as_str()).collect();
        assert_eq!(names, ["compact", "feedback", "goal"]);
        assert!(commands.iter().all(|command| command.slash.starts_with('/')));
    }
}
