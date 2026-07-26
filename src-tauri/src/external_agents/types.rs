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
/// 同时缓存 CLI 当前配置的模型/推理等级（current_*），使缓存命中也能回填胶囊。
#[derive(Debug, Clone)]
pub struct CachedAgentModels {
    pub models: Vec<RuntimeModelOption>,
    pub source: ModelSource,
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
    /// CLI 在**自己内部**完成了一次上下文压缩（claude 的
    /// `{"type":"system","subtype":"compact_boundary"}`）。
    ///
    /// 与 Kivio 主动发 `/compact`（`external_agents/compact.rs`）不同：那是用户点的、
    /// Kivio 知情；这条是 CLI 自动触发的，Kivio 只能被动收到通知。不接的话
    /// 用户会看到「对话突然变短了但没有任何提示」。
    ///
    /// 字段取自官方 SDK 的 `SDKCompactBoundaryMessage.compact_metadata`
    /// （`@anthropic-ai/claude-agent-sdk` 的 sdk.d.ts）：**只有** `trigger` 与
    /// `pre_tokens`，**没有** post_tokens——压缩后的真实占用由下一条
    /// `message_start.message.usage` 上报（服务端算的），不需要也不该猜。
    CliCompacted {
        /// `manual`（CLI 内用户敲的 /compact）| `auto`（CLI 自动触发）
        trigger: String,
        /// 压缩**前**的上下文占用；CLI 未提供时为 `None`。
        pre_tokens: Option<u64>,
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
