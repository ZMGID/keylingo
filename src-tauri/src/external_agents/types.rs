use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::chat::model::ModelUsage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamFormat {
    ClaudeStreamJson,
    PiRpc,
    AcpJsonRpc,
    CodexAppServer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptInputFormat {
    Text,
    StreamJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelProbeStrategy {
    Acp,
    ClaudeInit,
}

/// How a CLI's `/commands` are discovered for the slash popover. We only advertise commands
/// that the CLI genuinely honors in our (headless) invocation, so most one-shot CLIs are
/// `None` rather than carrying a fabricated list the CLI would ignore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashStrategy {
    /// Probe the Claude `system/init` event — yields built-ins + the user's custom commands
    /// and skills, exactly as the `claude` CLI resolves them for this cwd.
    ClaudeInit,
    /// Discover commands natively over ACP: run `initialize` → `session/new`, then read
    /// `session/update` notifications for the `available_commands_update` payload the agent
    /// pushes. Works for any ACP-speaking CLI (cursor / gemini / opencode).
    Acp,
    /// Discover via codex `app-server` `skills/list` merged with a curated built-in command set.
    CodexAppServer,
    /// Discover via the Pi RPC `get_commands` request.
    PiRpc,
    /// No discoverable slash commands for this CLI in headless mode.
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeModelOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u32>,
}

/// 模型下拉列表的来源：真实探测得到，还是探测失败后降级到静态表（fallback）。
/// 前端据此显示"默认列表"角标 + 重试。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    Probed,
    Fallback,
}

impl ModelSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ModelSource::Probed => "probed",
            ModelSource::Fallback => "fallback",
        }
    }
}

/// 模型探测缓存条目：带来源，供 state 层按来源应用不同 TTL（probed 长、fallback 短负缓存）。
/// 同时缓存 CLI 当前配置的模型/推理等级（current_*）与**推理档位列表**，使缓存命中也能回填胶囊。
///
/// `reasoning_options` 必须一起缓存：kimi 等 ACP CLI 的档位来自探测（`configOptions`），
/// `acp_def` 静态表是空的。若缓存只存 models 而命中时回落 def 表，effort 胶囊会在第二次
/// 打开时消失（bac8f53 修了探测路径，但漏了这一步）。
#[derive(Debug, Clone)]
pub struct CachedAgentModels {
    pub models: Vec<RuntimeModelOption>,
    pub source: ModelSource,
    /// 探测得到的推理档位列表（kimi ACP thinking: low/high/max 等）。空 = 无档位或回落 def。
    pub reasoning_options: Vec<RuntimeModelOption>,
    /// 按模型的 effort 列表（kimi support_efforts）。缓存命中时前端按所选模型切换档位。
    pub reasoning_by_model: std::collections::HashMap<String, Vec<RuntimeModelOption>>,
    /// CLI 自己当前配置的模型 id（codex config.toml / ACP currentModelId / claude resolved model）。
    /// None = 该 CLI 无「当前模型」概念，前端显示「自动」。
    pub current_model: Option<String>,
    /// CLI 当前配置的推理等级 id（codex model_reasoning_effort / ACP default reasoning effort）。
    pub current_reasoning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedAgent {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub models: Vec<RuntimeModelOption>,
    pub reasoning_options: Vec<RuntimeModelOption>,
    #[serde(default)]
    pub sandbox_options: Vec<RuntimeModelOption>,
    pub auth_status: Option<String>,
    /// 用户在设置页停用了它。**不进可用性缓存**——出缓存时才盖章（`commands.rs`），
    /// 否则改一次开关要等 `AVAILABILITY_CACHE_TTL`（600s）才生效。
    #[serde(default)]
    pub disabled: bool,
    /// 协议是否支持运行中注入（见 `RuntimeAgentDef::supports_steering`）。
    /// 前端据此决定排队条上给不给「立刻引导」。
    #[serde(default)]
    pub supports_steering: bool,
}

#[derive(Debug, Clone)]
pub struct RuntimeBuildOptions {
    pub model: Option<String>,
    pub reasoning: Option<String>,
    /// Sandbox/permission level id (native flag value, e.g. claude "bypassPermissions" / codex "workspace-write").
    pub sandbox: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RuntimeContext {
    pub extra_allowed_dirs: Vec<String>,
    pub resume_session_id: Option<String>,
    pub new_session_id: Option<String>,
    pub include_partial_messages: bool,
}

#[derive(Debug, Clone)]
pub struct RuntimeAgentDef {
    pub id: &'static str,
    pub name: &'static str,
    pub bin: &'static str,
    pub fallback_bins: &'static [&'static str],
    pub version_args: &'static [&'static str],
    pub auth_probe_args: Option<&'static [&'static str]>,
    pub fallback_models: &'static [(&'static str, &'static str)],
    pub reasoning_options: &'static [(&'static str, &'static str)],
    pub list_models_args: Option<&'static [&'static str]>,
    pub list_models_timeout_secs: Option<u64>,
    pub models_from_stderr: bool,
    pub model_probe: Option<ModelProbeStrategy>,
    pub model_probe_args: Option<&'static [&'static str]>,
    pub slash_strategy: SlashStrategy,
    pub env: &'static [(&'static str, &'static str)],
    pub max_prompt_arg_bytes: Option<usize>,
    pub prompt_via_stdin: bool,
    pub prompt_input_format: PromptInputFormat,
    pub stream_format: StreamFormat,
    pub resumes_session_via_cli: bool,
    /// 该 CLI 是否能通过其协议原生接收图片（Claude base64 / ACP image / Codex localImage）。
    /// false（pi/kimi）时图片降级为在 prompt 文本里写出路径。
    pub supports_native_image: bool,
    /// 该 CLI 的协议能否往**在飞的轮次**里追加一条用户输入（「立刻引导」）。
    ///
    /// 目前只有 codex 能：`turn/steer` + `expectedTurnId`（真机验证见
    /// `session::codex_app_server` 的 `codex_turn_steer_injects_into_the_running_turn`）。
    /// claude 的 stream-json 输入是**顺序**处理的、没有注入用的 control_request；
    /// ACP 只有 `session/prompt` 与 `session/cancel`。这些一律 false —— 前端据此
    /// 不显示引导入口，排队消息照旧在轮末自动发出。
    pub supports_steering: bool,
    /// 允许原生注入的图片 MIME 白名单；空 = 不限。Claude stream-json 仅认 jpeg/png/gif/webp，
    /// 超出的图片降级为路径文本（不静默丢弃）。
    pub image_mime_whitelist: &'static [&'static str],
    pub build_args: fn(&RuntimeContext, &RuntimeBuildOptions, Option<&str>) -> Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCliSlashCommand {
    pub name: String,
    pub slash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UnifiedAgentEvent {
    TextDelta {
        delta: String,
    },
    ThinkingDelta {
        delta: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    Usage {
        usage: ModelUsage,
    },
    Error {
        message: String,
    },
    /// A stdout line the stream handler could not parse as a known protocol event. Kept so an
    /// otherwise-silent run (a CLI that printed a plain-text error/usage message instead of JSON)
    /// surfaces its output instead of looking like it produced nothing.
    Raw {
        line: String,
    },
    SlashCommands {
        commands: Vec<ExternalCliSlashCommand>,
    },
    /// 用户在这一轮跑着的时候插了一句话（「立刻引导」），且 CLI 的协议**已受理**注入。
    ///
    /// 只在受理成功后发 —— 这条事件会被 `run.rs` 变成时间线上那张 `user_steer` 卡，
    /// 而卡片的语义是「这句话确实进了模型的输入」。受理失败时不发，前端那条排队消息
    /// 就留在队列里等轮末自动发送。
    UserSteer {
        /// 前端生成的 id，回到卡片的 `structured_content.steer_id` 供前端对账出队。
        id: String,
        text: String,
    },
    /// CLI 在**自己内部**完成了一次上下文压缩（claude 的
    /// `{"type":"system","subtype":"compact_boundary"}`）。
    ///
    /// 与 Kivio 主动发 `/compact`（`external_agents/compact.rs`）不同：那是用户点的、
    /// Kivio 知情；这条是 CLI 自动触发的，Kivio 只能被动收到通知。不接的话
    /// 用户会看到「对话突然变短了但没有任何提示」。
    ///
    /// 字段取自 `compact_metadata`，claude 2.1.220 反查二进制核实的构造处为
    /// `{ trigger, pre_tokens, post_tokens?, cumulative_dropped_tokens?, duration_ms?,
    ///    user_context?, messages_summarized? }`。
    ///
    /// **历史注记**：此处曾写「**没有** post_tokens，压缩后的占用由下一条
    /// `message_start.message.usage` 上报，不需要也不该猜」——那是错的，`post_tokens`
    /// 确实存在。照那条注释走的结果是 `run.rs::emit_cli_compaction` 把
    /// `token_estimate_after` 硬编码 0，前端分隔线上的「→ N」永远不显示。
    CliCompacted {
        /// `manual`（CLI 内用户敲的 /compact）| `auto`（CLI 自动触发）
        trigger: String,
        /// 压缩**前**的上下文占用；CLI 未提供时为 `None`。
        pre_tokens: Option<u64>,
        /// 压缩**后**的上下文占用；CLI 未提供时为 `None`（此时前端不显示「→ N」）。
        post_tokens: Option<u64>,
        /// 本会话累计被丢弃的 token（`cumulative_dropped_tokens`），仅用于诊断日志。
        dropped_tokens: Option<u64>,
        /// 压缩耗时，仅用于诊断日志。
        duration_ms: Option<u64>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalAgentSession {
    pub conversation_id: String,
    pub agent_id: String,
    pub session_id: String,
    #[serde(default)]
    pub stable_prompt_hash: Option<String>,
    /// Model this native session was created with. When the user's currently-selected model
    /// differs, we start a fresh session instead of resuming (some CLIs — notably Claude — bake
    /// the model into the session at create time and ignore `--model` on `--resume`). `None`
    /// means "let the CLI use its default" (i.e. `--model` was not passed).
    #[serde(default)]
    pub model: Option<String>,
}

pub fn default_model_option() -> RuntimeModelOption {
    RuntimeModelOption {
        id: "default".to_string(),
        label: "Default".to_string(),
        context_window_tokens: None,
    }
}

pub fn fallback_models_from_pairs(pairs: &[(&str, &str)]) -> Vec<RuntimeModelOption> {
    let mut out = vec![default_model_option()];
    for (id, label) in pairs {
        if *id == "default" {
            continue;
        }
        out.push(RuntimeModelOption {
            id: (*id).to_string(),
            label: (*label).to_string(),
            context_window_tokens: None,
        });
    }
    out
}

pub fn reasoning_options_from_pairs(pairs: &[(&str, &str)]) -> Vec<RuntimeModelOption> {
    pairs
        .iter()
        .map(|(id, label)| RuntimeModelOption {
            id: (*id).to_string(),
            label: (*label).to_string(),
            context_window_tokens: None,
        })
        .collect()
}
