use std::path::Path;

use crate::chat::agent::prepare as agent_prepare;
use crate::chat::model::ModelUsage;
use crate::chat::model_metadata::context_window_for_model;
use crate::chat::types::{ContextUsageSegment, Conversation, ConversationContextState};
use crate::external_agents::detection::detect_single_agent;
use crate::external_agents::kimi_usage;
use crate::external_agents::registry::get_agent_def;
use crate::external_agents::session::claude_init::{
    context_window_from_claude_model_alias, context_window_from_claude_resolved_model,
};
use crate::external_agents::types::RuntimeModelOption;

pub const CONTEXT_SOURCE_BUILTIN: &str = "kivio_builtin";
pub const CONTEXT_SOURCE_EXTERNAL: &str = "external_cli";
pub const TOKEN_COUNT_CLI: &str = "cli_reported";
pub const TOKEN_COUNT_ESTIMATED: &str = "estimated";
/// 内置路径把 provider 实报 `usage` 作为真实用量锚点时的口径标记（对齐 CLI 的 `cli_reported`）。
/// 前端 ContextIndicator 据此把 footer 标为「模型实报」（精确值，不带 `~` 前缀）。
pub const TOKEN_COUNT_PROVIDER_REPORTED: &str = "provider_reported";

pub fn parse_context_window_label(label: &str) -> Option<u32> {
    let s = label.trim().to_ascii_uppercase();
    if s.is_empty() {
        return None;
    }
    let (num_part, multiplier) = if let Some(rest) = s.strip_suffix('M') {
        (rest, 1_000_000u32)
    } else if let Some(rest) = s.strip_suffix('K') {
        (rest, 1_000u32)
    } else {
        (s.as_str(), 1u32)
    };
    num_part
        .parse::<f64>()
        .ok()
        .filter(|value| *value > 0.0)
        .map(|value| (value * multiplier as f64).round() as u32)
}

/// kimi 的上下文窗口静态映射。
///
/// **为什么只能静态**：kimi 的 ACP 上游确实什么都不给——实测 `session/new` 返回的
/// `configOptions` 里只有模型 id/名字，无任何 token 字段；`session/prompt` 结果只有
/// `{"stopReason":"end_turn"}`；也不发 ACP 官方的 `usage_update`。
///
/// 表来源：opencode `models --verbose` 实测 + kimi 官方文档，2026-07-26 核对。
/// ACP 报的 id 带 `kimi-code/` 前缀，用户在设置里手填可能只写后半段，两种都要认。
///
/// 注意：**不要**为了让 `k3` 匹到 modelDatabase 的 `kimi-k3` 而放宽
/// `chat::model_metadata::model_database_entry` 的通用匹配算法——它服务所有 provider，
/// 放宽会在别的 provider 上误命中。
fn context_window_from_kimi_model(model: &str) -> Option<u32> {
    let id = model.trim().to_ascii_lowercase();
    let id = id.strip_prefix("kimi-code/").unwrap_or(id.as_str());
    match id {
        "k3" => Some(1_048_576),
        "k3-256k" | "kimi-for-coding" | "kimi-for-coding-highspeed" => Some(262_144),
        _ => None,
    }
}

/// 外部 CLI 会话的上下文窗口（分母）。
///
/// 优先级（可靠 → 不可靠）：
/// 1. **CLI 本轮实报** —— `message.usage.context_window_tokens`（ACP `usage_update.size`）。
///    由调用方通过 `reported_window` 传入。比任何静态表都准：模型可能中途切换。
/// 2. 模型探测上报 —— `RuntimeModelOption.context_window_tokens`
///    （codex `debug models` / grok `_meta` / cursor modelId 的 `context=` / pi 模型表）。
/// 3. claude 别名表。
/// 4. kimi 静态映射。
/// 5. 模型名数据库 + 关键词启发。
/// 6. `None` —— **不再兜底 200K**。
///
/// 返回 `(窗口, 是否为启发式估算)`。窗口为 `None` 时上层不算百分比、前端不显示阈值刻度。
/// 编造一个假 200K 的代价不是"显示不准"，而是 `usage_ratio` 会在错误的点触发压缩阈值。
pub fn context_window_for_external_model(
    agent_id: &str,
    model: &str,
    detected_models: Option<&[RuntimeModelOption]>,
    reported_window: Option<u64>,
) -> (Option<usize>, bool) {
    let model = model.trim();
    let lookup_id = if model.is_empty() || model == "default" {
        "default"
    } else {
        model
    };

    if let Some(tokens) = reported_window.filter(|value| *value > 0) {
        return (Some(tokens as usize), false);
    }

    if let Some(models) = detected_models {
        if let Some(found) = models.iter().find(|item| item.id == lookup_id) {
            if let Some(tokens) = found.context_window_tokens {
                return (Some(tokens as usize), false);
            }
        }
    }

    if agent_id == "claude" {
        if let Some(tokens) = context_window_from_claude_model_alias(lookup_id) {
            return (Some(tokens as usize), false);
        }
        if lookup_id != "default" {
            if let Some(tokens) = context_window_from_claude_resolved_model(lookup_id) {
                return (Some(tokens as usize), false);
            }
        }
    }

    if agent_id == "kimi" {
        if let Some(tokens) = context_window_from_kimi_model(lookup_id) {
            return (Some(tokens as usize), false);
        }
    }

    if lookup_id == "default" {
        // 模型都不知道是哪个，任何按名字猜窗口的做法都是编造。
        return (None, true);
    }

    // `context_window_for_model` 自带 200K 兜底（内置路径那里 provider 元数据可靠，兜底合理）。
    // 外部 CLI 这条路必须把兜底剥掉：`estimated == true` 且落在兜底值上时视为「没查到」。
    let (tokens, estimated) = context_window_for_model(None, lookup_id);
    if estimated && tokens == FALLBACK_CONTEXT_WINDOW_TOKENS {
        return (None, true);
    }
    (Some(tokens), estimated)
}

/// 与 `chat::model_metadata` 内部的兜底常量保持一致。该常量不对外导出，这里复述一份并由
/// `fallback_window_constant_matches_metadata` 单测钉住——它一旦变动，测试会红。
const FALLBACK_CONTEXT_WINDOW_TOKENS: usize = 200_000;

pub struct ExternalSessionUsage {
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub token_count_source: &'static str,
    /// CLI 随用量一并上报的上下文窗口（ACP `usage_update.size`）。分母的最高优先级来源。
    pub reported_context_window: Option<u64>,
}

/// 汇总外部 CLI 会话的已用 token（分子）。
///
/// 来源顺序：
/// 1. **CLI 实报** —— 最近一条带 usage 的 assistant 消息（claude/codex/pi/ACP 各自的通道）。
/// 2. **kimi 落盘日志** —— kimi 的 ACP 上游什么都不报，只能读它自己的 wire.jsonl
///    （见 `kimi_usage` 模块头的说明）。只对 `agent_id == "kimi"` 且给了 cwd 时生效。
/// 3. **字符估算** —— 兜底，系统性偏低（不含 CLI 自己的 system prompt / 工具定义 / 附件）。
///
/// 热路径检查（spec 第 9 条）：本函数只读会话对象与磁盘文件，**不起子进程**；
/// 调用方是 `chat_get_context_stats`（用户点开用量条）与压缩后的重算，不在回复热路径。
/// 从一条 CLI 实报的 `usage` 里取出**上下文占用**口径的已用 token。
///
/// 用 `total_tokens` 而不是 `input_tokens`：各 CLI 解析层（`stream::usage_from_parts`）
/// 已按各家的 cache 包含关系把 cache 正确汇总进 `total_tokens`，而 `input_tokens` 在
/// Anthropic 口径（claude / pi / ACP）下只是**非缓存**部分——只读它等于把前面几层补的
/// cache 全丢在半路（实测 kimi cache 占 97.6%、pi 62%、opencode 13%）。
///
/// `total_tokens` 缺失时退回 `input + output`（改动前落盘的旧会话没有 total）。
///
/// **分子口径只有这一个真源**：轮末的权威计算走它。曾经那条实时通道也走它（正是为了不让
/// 生成过程中的数字与轮末的权威值分叉），那条通道已删 —— 外部 CLI 的占用现在一轮只更新一次。
pub(crate) fn cli_reported_context_tokens(usage: &ModelUsage) -> usize {
    usage
        .total_tokens
        .unwrap_or_else(|| {
            usage
                .input_tokens
                .unwrap_or(0)
                .saturating_add(usage.output_tokens.unwrap_or(0))
        })
        .min(usize::MAX as u64) as usize
}

/// 这条 CLI 上报是否**真的带了分子**（任一 token 数 > 0）。
///
/// 判据不能是 `input_tokens.is_some()`：全零上报（未登录 / `/help` / 未知斜杠命令 /
/// Kivio 自己发的 `/compact` —— 这些轮次没有 LLM 往返）落盘的是 `Some(0)`，
/// `is_some()` 会命中它并把用量条清零。
fn usage_has_token_numbers(usage: &ModelUsage) -> bool {
    // 必须与 `run.rs::usage_tokens_all_zero` **完全互为反面**（含 `reasoning_tokens`）。
    // 不一致的后果：只报 reasoning tokens 的那一轮，`merge_cli_usage` 认为它是真数据、
    // 拿它替换上一条好数据，而这里的分子挑选器认为它是空的、继续往前找更旧的消息 ⇒
    // 分子与落库用量描述的是**不同轮次**。
    !crate::external_agents::run::usage_tokens_all_zero(usage)
}

pub fn collect_external_session_usage(
    conversation: &Conversation,
    agent_id: &str,
    work_dir: Option<&Path>,
) -> ExternalSessionUsage {
    let mut latest: Option<&ModelUsage> = None;
    let mut latest_index: Option<usize> = None;
    for (index, message) in conversation.messages.iter().enumerate().rev() {
        if message.role != "assistant" {
            continue;
        }
        if let Some(usage) = message.usage.as_ref() {
            // **跳过全零上报**：没有 LLM 往返的轮次（未登录 / `/help` / 未知斜杠命令 /
            // 我们自己发的 `/compact`）会落下 `Some(0)` 的用量。判据若只看 `is_some()`，
            // `Some(0)` 会命中 ⇒ 挑到这条 ⇒ 用量条从 47K 掉到 0，直到下一轮真实回复才恢复。
            // 窗口（分母）不参与这个判断——它是静态属性，缺分子的那条上报不该被当分子来源。
            if usage_has_token_numbers(usage) {
                latest = Some(usage);
                latest_index = Some(index);
                break;
            }
        }
    }

    // 分母单独找：一条「token 数全零但带 context_window_tokens」的上报（斜杠命令、被中断的
    // 轮次）对分子无用，但它的窗口是**最新**的权威值。跟着分子一起跳过会白丢分母 ——
    // 换到小窗口模型后进度条会一直按旧模型的分母算。
    let reported_window = conversation
        .messages
        .iter()
        .rev()
        .filter(|message| message.role == "assistant")
        .filter_map(|message| message.usage.as_ref())
        .find_map(|usage| usage.context_window_tokens);

    // 压缩之后的真实占用**只在 boundary 的 post_tokens 上**：用量上报里的数字是压缩
    // **前**的。不看它的话，压完进度条还钉在 95%，用户会接着压第二次、第三次，直到
    // 下一轮真实生成才纠正。
    //
    // 只有当这条 boundary 比挑中的用量更新时才用它 —— 锚点消息在用量消息之后（或压根
    // 没挑到用量）才算新。
    let newest_boundary = conversation
        .context_state
        .compaction_boundaries
        .iter()
        .filter(|boundary| boundary.token_estimate_after > 0)
        .max_by_key(|boundary| boundary.created_at);
    if let Some(boundary) = newest_boundary {
        let anchor_index = boundary
            .display_after_message_id
            .as_deref()
            .and_then(|anchor| {
                conversation
                    .messages
                    .iter()
                    .position(|message| message.id == anchor)
            });
        let boundary_is_newer = match (anchor_index, latest_index) {
            (Some(anchor), Some(usage_at)) => anchor >= usage_at,
            (_, None) => true,
            (None, Some(_)) => false,
        };
        if boundary_is_newer {
            return ExternalSessionUsage {
                input_tokens: boundary.token_estimate_after,
                output_tokens: 0,
                token_count_source: TOKEN_COUNT_CLI,
                reported_context_window: reported_window,
            };
        }
    }

    if let Some(usage) = latest {
        return ExternalSessionUsage {
            input_tokens: cli_reported_context_tokens(usage),
            output_tokens: usage.output_tokens.unwrap_or(0) as usize,
            token_count_source: TOKEN_COUNT_CLI,
            reported_context_window: reported_window.or(usage.context_window_tokens),
        };
    }

    if agent_id == "kimi" {
        if let Some(usage) = work_dir.and_then(kimi_usage::latest_turn_usage) {
            return ExternalSessionUsage {
                // kimi 的 input_tokens 已含 cache（inputOther + CacheRead + CacheCreation），
                // 加上 output 才与上面 CLI 实报分支的「上下文占用」口径一致。
                input_tokens: usage
                    .input_tokens
                    .saturating_add(usage.output_tokens)
                    .min(usize::MAX as u64) as usize,
                output_tokens: usage.output_tokens as usize,
                token_count_source: TOKEN_COUNT_CLI,
                reported_context_window: None,
            };
        }
    }

    let transcript = conversation
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    ExternalSessionUsage {
        input_tokens: agent_prepare::estimate_tokens(&transcript),
        output_tokens: 0,
        token_count_source: TOKEN_COUNT_ESTIMATED,
        reported_context_window: None,
    }
}

pub fn compute_external_context_state(
    conversation: &Conversation,
    agent_id: &str,
    model: &str,
    detected_models: Option<&[RuntimeModelOption]>,
    compact_usage: Option<&ModelUsage>,
    work_dir: Option<&Path>,
) -> ConversationContextState {
    let usage = compact_usage
        .map(|item| ExternalSessionUsage {
            // 与 collect_external_session_usage 同口径（含 cache），否则压缩后那一次刷新
            // 会显示一个比平时低的数，看起来像"压缩效果特别好"。
            input_tokens: cli_reported_context_tokens(item),
            output_tokens: item.output_tokens.unwrap_or(0) as usize,
            token_count_source: TOKEN_COUNT_CLI,
            reported_context_window: item.context_window_tokens,
        })
        .unwrap_or_else(|| collect_external_session_usage(conversation, agent_id, work_dir));

    let (context_window_tokens, context_window_estimated) = context_window_for_external_model(
        agent_id,
        model,
        detected_models,
        usage.reported_context_window,
    );
    // **CLI 还没报过任何数 ⇒ 什么都不显示**（分子分母全空，前端就地退回「—」）。
    //
    // 走到这里的只有首轮（`collect_external_session_usage` 的最后一条兜底：拿会话转录估算
    // 的那个分支 —— 第二轮起总有上一条 assistant 消息的实报值）。此前那一档显示的是
    // 「0% · ~19 / 200.0K」：分子是转录估算（不含 CLI 的系统提示 / 工具表 / 读过的文件，
    // 差一个数量级），分母是静态表编的（`claude-sonnet-5` 表里 200K，而 CLI 实报 1M
    // ——`[1m]` beta）。两个数都是假的，凑出来的百分比也是假的。
    //
    // 首轮答完就有真值了（轮末权威计算 = CLI 实报），空一小会儿比给个错的强。代价：**从不
    // 上报用量的 CLI** 会一直显示「—」；那也比一个差一个数量级的估算诚实。
    if usage.token_count_source != TOKEN_COUNT_CLI {
        return ConversationContextState {
            estimated_input_tokens: 0,
            context_window_tokens: None,
            context_window_estimated: false,
            usage_ratio: None,
            status: "unknown".to_string(),
            segments: Vec::new(),
            last_measured_at: chrono::Local::now().timestamp(),
            last_compressed_at: conversation.context_state.last_compressed_at,
            compressed_message_count: 0,
            compression_count: conversation.context_state.compression_count,
            summary: None,
            compaction_boundaries: conversation.context_state.compaction_boundaries.clone(),
            warning: conversation.context_state.warning.clone(),
            context_source: Some(CONTEXT_SOURCE_EXTERNAL.to_string()),
            token_count_source: None,
            session_input_tokens: None,
            session_output_tokens: None,
            external_agent_id: Some(agent_id.to_string()),
            external_model: normalized_external_model(model),
        };
    }
    let usage_ratio = context_window_tokens
        .filter(|window| *window > 0)
        .map(|window| usage.input_tokens as f32 / window as f32);
    let status = external_context_status(usage_ratio);
    let segments = external_context_segments(&usage);
    let last_compressed_at = conversation.context_state.last_compressed_at;
    let compression_count = conversation.context_state.compression_count;

    ConversationContextState {
        estimated_input_tokens: usage.input_tokens,
        context_window_tokens,
        context_window_estimated,
        usage_ratio,
        status,
        segments,
        last_measured_at: chrono::Local::now().timestamp(),
        last_compressed_at,
        compressed_message_count: 0,
        compression_count,
        summary: None,
        compaction_boundaries: conversation.context_state.compaction_boundaries.clone(),
        warning: conversation.context_state.warning.clone(),
        context_source: Some(CONTEXT_SOURCE_EXTERNAL.to_string()),
        token_count_source: Some(usage.token_count_source.to_string()),
        session_input_tokens: Some(usage.input_tokens),
        session_output_tokens: Some(usage.output_tokens),
        external_agent_id: Some(agent_id.to_string()),
        external_model: normalized_external_model(model),
    }
}

/// `default` / 空串都当「没指定模型」（不是一个叫 default 的模型）。
fn normalized_external_model(model: &str) -> Option<String> {
    if model.trim().is_empty() || model == "default" {
        None
    } else {
        Some(model.to_string())
    }
}

fn external_context_status(usage_ratio: Option<f32>) -> String {
    let Some(ratio) = usage_ratio else {
        return "unknown".to_string();
    };
    if ratio >= 0.95 {
        "critical".to_string()
    } else if ratio >= 0.70 {
        "warning".to_string()
    } else {
        "normal".to_string()
    }
}

fn external_context_segments(usage: &ExternalSessionUsage) -> Vec<ContextUsageSegment> {
    if usage.input_tokens == 0 {
        return Vec::new();
    }
    let label = if usage.token_count_source == TOKEN_COUNT_CLI {
        "CLI session context".to_string()
    } else {
        "Estimated transcript".to_string()
    };
    vec![ContextUsageSegment {
        id: "external-session".to_string(),
        label,
        estimated_tokens: usage.input_tokens,
        color: Some("#4A7FD7".to_string()),
    }]
}

/// `probe_cwd` = 模型探测用的 cwd；`work_dir` = 会话的**执行** cwd
/// （`workspace::resolve_effective_cwd`），只用于按 workDir 关联 kimi 的落盘日志。
/// 两者语义不同（见 spec 第 11b 条），故分开传，不复用同一个参数。
pub async fn compute_external_context_state_with_probe(
    conversation: &Conversation,
    probe_models: bool,
    compact_usage: Option<&ModelUsage>,
    cached_models: Option<&[RuntimeModelOption]>,
    probe_cwd: Option<&Path>,
    work_dir: Option<&Path>,
) -> ConversationContextState {
    let agent_id = conversation
        .agent_runtime
        .external_agent_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .unwrap_or("");
    let model = conversation
        .agent_runtime
        .external_model
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("default");
    let detected_models = if probe_models {
        if let Some(def) = get_agent_def(agent_id) {
            let fallback_cwd = std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir());
            Some(
                detect_single_agent(def, probe_cwd.unwrap_or(fallback_cwd.as_path()))
                    .await
                    .models,
            )
        } else {
            None
        }
    } else {
        cached_models.map(|models| models.to_vec())
    };
    compute_external_context_state(
        conversation,
        agent_id,
        model,
        detected_models.as_ref().map(|models| models.as_slice()),
        compact_usage,
        work_dir,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::types::{
        AgentPlanState, AgentRuntimeConfig, AgentRuntimeKind, AgentTodoState, ChatMessage,
        Conversation, ConversationContextState,
    };

    fn empty_conversation() -> Conversation {
        Conversation {
            id: "c1".to_string(),
            revision: 0,
            title: "t".to_string(),
            provider_id: "p".to_string(),
            model: "m".to_string(),
            messages: vec![],
            agent_runtime: AgentRuntimeConfig {
                kind: AgentRuntimeKind::External,
                external_agent_id: Some("pi".to_string()),
                external_model: Some("anthropic/claude-sonnet-4-5".to_string()),
                external_reasoning: None,
                external_sandbox: None,
                external_agent_preset: None,
            },
            active_skill_id: None,
            assistant_id: None,
            assistant_snapshot: None,
            created_at: 0,
            updated_at: 0,
            pinned: false,
            archived: false,
            folder: None,
            project_id: None,
            set_id: None,
            context_state: ConversationContextState::default(),
            agent_plan_state: AgentPlanState::default(),
            agent_todo_state: AgentTodoState::default(),
            knowledge_base_ids: Vec::new(),
            force_knowledge_search: false,
            thinking_level: None,
            web_search_mode: None,
            reply_models: Vec::new(),
            group_selections: std::collections::HashMap::new(),
            forked_from: None,
        }
    }

    fn message(id: &str, role: &str, content: &str, usage: Option<ModelUsage>) -> ChatMessage {
        ChatMessage {
            id: id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            attachments: vec![],
            reasoning: None,
            artifacts: vec![],
            tool_calls: vec![],
            segments: vec![],
            agent_plan: None,
            api_messages: vec![],
            model_messages: vec![],
            active_skill_id: None,
            run_entry: None,
            stream_outcome: None,
            usage,
            anchor_usage: None,
            group_id: None,
            provider_id: None,
            model: None,
            timestamp: 1,
            degraded: None,
        }
    }

    #[test]
    fn parse_context_window_labels() {
        assert_eq!(parse_context_window_label("200K"), Some(200_000));
        assert_eq!(parse_context_window_label("128k"), Some(128_000));
        assert_eq!(parse_context_window_label("1M"), Some(1_000_000));
    }

    /// 钉住本文件复述的兜底常量与 `chat::model_metadata` 内部保持一致——那边一旦改值，
    /// 「未知模型不编造窗口」的剥离逻辑会静默失效，本测试会红。
    #[test]
    fn fallback_window_constant_matches_metadata() {
        let (tokens, estimated) = context_window_for_model(None, "totally-unknown-model-xyz");
        assert!(estimated);
        assert_eq!(tokens, FALLBACK_CONTEXT_WINDOW_TOKENS);
    }

    /// 回归：`claude-sonnet-5` 静态表只知道 200K，而 CLI 实报 1M（`[1m]` beta）。实报永远
    /// 压过静态表 —— 生成过程中的分母干脆不下发（`ContextUsageTicker`），由前端沿用上一轮
    /// 这里算出的值，否则就是「回答时 89%、答完 18%」。
    #[test]
    fn cli_reported_window_beats_the_claude_static_table() {
        assert_eq!(
            context_window_for_external_model("claude", "claude-sonnet-5", None, Some(1_000_000)).0,
            Some(1_000_000)
        );
        assert_eq!(
            context_window_for_external_model("claude", "claude-sonnet-5", None, None).0,
            Some(200_000)
        );
    }

    #[test]
    fn kimi_static_window_map() {
        assert_eq!(
            context_window_from_kimi_model("kimi-code/k3"),
            Some(1_048_576)
        );
        assert_eq!(context_window_from_kimi_model("k3"), Some(1_048_576));
        assert_eq!(
            context_window_from_kimi_model("kimi-code/k3-256k"),
            Some(262_144)
        );
        assert_eq!(context_window_from_kimi_model("k3-256k"), Some(262_144));
        assert_eq!(
            context_window_from_kimi_model("kimi-code/kimi-for-coding"),
            Some(262_144)
        );
        assert_eq!(
            context_window_from_kimi_model("kimi-for-coding-highspeed"),
            Some(262_144)
        );
        assert_eq!(context_window_from_kimi_model("unknown-kimi"), None);
    }

    #[test]
    fn kimi_window_reaches_context_state() {
        let (window, estimated) =
            context_window_for_external_model("kimi", "kimi-code/k3-256k", None, None);
        assert_eq!(window, Some(262_144));
        assert!(!estimated);
        // 同名模型在别的 agent 下不该走 kimi 映射（那是 agent 专属表）。
        // 但名字里明写 256k，L8 的关键词表照样能捞到——这是另一条来源，不是 kimi 表。
        let (other, _) =
            context_window_for_external_model("cursor", "kimi-code/k3-256k", None, None);
        assert_eq!(other, Some(262_144));
    }

    #[test]
    fn unknown_agent_and_model_yields_no_window_instead_of_fake_200k() {
        // 模型库收录后，composer-2.5 有官方 200K 上下文（Cursor docs pricing catalog）。
        let (window, estimated) =
            context_window_for_external_model("cursor", "composer-2.5", None, None);
        assert_eq!(window, Some(200_000));
        assert!(!estimated);

        // 完全未知的模型 id 仍不能编造 200K 兜底（会触发假百分比 / 错位压缩阈值）。
        let (window, estimated) = context_window_for_external_model(
            "cursor",
            "totally-unknown-cursor-model-xyz",
            None,
            None,
        );
        assert_eq!(window, None);
        assert!(estimated);

        // 连模型都不知道（default）时同样不猜。
        let (window, _) = context_window_for_external_model("cursor", "default", None, None);
        assert_eq!(window, None);
    }

    #[test]
    fn cli_reported_window_outranks_static_tables() {
        // 别名表给 1M（`sonnet[1m]`）；CLI 本轮实报 300K 必须赢——模型可能中途切换。
        let (window, estimated) =
            context_window_for_external_model("claude", "sonnet[1m]", None, Some(300_000));
        assert_eq!(window, Some(300_000));
        assert!(!estimated);
        // 探测上报也必须让位给实报。
        let detected = vec![RuntimeModelOption {
            id: "gpt-5".to_string(),
            label: "GPT-5".to_string(),
            context_window_tokens: Some(272_000),
        }];
        let (window, _) =
            context_window_for_external_model("codex", "gpt-5", Some(&detected), Some(400_000));
        assert_eq!(window, Some(400_000));
        // 没有实报时才轮到探测值。
        let (window, _) =
            context_window_for_external_model("codex", "gpt-5", Some(&detected), None);
        assert_eq!(window, Some(272_000));
        // 实报 0 视为无效，不当分母。
        let (window, _) =
            context_window_for_external_model("codex", "gpt-5", Some(&detected), Some(0));
        assert_eq!(window, Some(272_000));
    }

    #[test]
    fn agents_with_window_sources_are_unaffected() {
        // claude 别名表：只有显式带 `[1m]` 标记的别名给窗口。裸别名（sonnet / opus / …）
        // 解析成什么模型由 CLI 版本决定（本机实测 3/4 是 1M），编一个 200K 会让分母小 5 倍
        // ⇒ 一律 None，靠 CLI 实报的 `modelUsage.contextWindow` 补上（见下一条）。
        let (window, _) = context_window_for_external_model("claude", "sonnet[1m]", None, None);
        assert_eq!(window, Some(1_000_000));
        let (window, estimated) = context_window_for_external_model("claude", "sonnet", None, None);
        assert_eq!(window, None, "裸别名不得编造 200K 分母");
        assert!(estimated);
        // codex：探测上报（debug models 的 context_window）
        let detected = vec![RuntimeModelOption {
            id: "gpt-5-codex".to_string(),
            label: "GPT-5 Codex".to_string(),
            context_window_tokens: Some(272_000),
        }];
        let (window, _) =
            context_window_for_external_model("codex", "gpt-5-codex", Some(&detected), None);
        assert_eq!(window, Some(272_000));
        // pi：--list-models 表同样走探测上报这一级
        let pi_models = vec![RuntimeModelOption {
            id: "anthropic/claude-sonnet-4-5".to_string(),
            label: "Sonnet".to_string(),
            context_window_tokens: Some(128_000),
        }];
        let (window, _) = context_window_for_external_model(
            "pi",
            "anthropic/claude-sonnet-4-5",
            Some(&pi_models),
            None,
        );
        assert_eq!(window, Some(128_000));
    }

    #[test]
    fn unknown_window_produces_unknown_status_and_no_ratio() {
        let mut conversation = empty_conversation();
        conversation.agent_runtime.external_agent_id = Some("cursor".to_string());
        // composer-2.5 已入库；这里要的是「完全未知模型 → 无窗口」路径。
        conversation.agent_runtime.external_model =
            Some("totally-unknown-cursor-model-xyz".to_string());
        conversation.messages.push(message(
            "a1",
            "assistant",
            "hi",
            Some(ModelUsage {
                input_tokens: Some(5000),
                output_tokens: Some(10),
                total_tokens: Some(5010),
                ..Default::default()
            }),
        ));

        let state = compute_external_context_state(
            &conversation,
            "cursor",
            "totally-unknown-cursor-model-xyz",
            None,
            None,
            None,
        );
        assert_eq!(state.context_window_tokens, None);
        assert_eq!(state.usage_ratio, None);
        assert_eq!(state.status, "unknown");
        // 分子仍然照常显示。
        assert_eq!(state.estimated_input_tokens, 5010);
    }

    /// **首轮什么都不显示**：CLI 一个数都没报过时，分子分母全空（前端退回「—」），
    /// 而不是拿转录估算 + 静态表窗口凑一个「0% · ~19 / 200.0K」——两个数都是假的。
    #[test]
    fn no_cli_report_yet_shows_nothing_instead_of_an_estimate() {
        let mut conversation = empty_conversation();
        conversation.agent_runtime.external_agent_id = Some("claude".to_string());
        conversation.agent_runtime.external_model = Some("claude-sonnet-5".to_string());
        // 首轮：只有用户消息，还没有任何带用量的 assistant 消息。
        conversation.messages.push(message(
            "u1",
            "user",
            "帮我看一下这个仓库的上下文用量是怎么算的",
            None,
        ));

        let state = compute_external_context_state(
            &conversation,
            "claude",
            "claude-sonnet-5",
            None,
            None,
            None,
        );
        assert_eq!(state.estimated_input_tokens, 0, "分子不该是转录估算");
        assert_eq!(
            state.context_window_tokens, None,
            "分母不该来自静态表（表里 200K，而 CLI 实报 1M）"
        );
        assert_eq!(state.usage_ratio, None);
        assert_eq!(state.status, "unknown");
        assert!(
            state.segments.is_empty(),
            "不该有「Estimated transcript」那一条"
        );
        assert_eq!(state.token_count_source, None, "没有数就别声明口径");
        // 仍然要标明这是外部 CLI 会话（前端据此选压缩按钮的文案等）。
        assert_eq!(
            state.context_source.as_deref(),
            Some(CONTEXT_SOURCE_EXTERNAL)
        );
        assert_eq!(state.external_agent_id.as_deref(), Some("claude"));
    }

    #[test]
    fn reported_window_from_message_usage_becomes_denominator() {
        // L1 把 ACP usage_update.size 存进 message.usage.context_window_tokens；
        // 这里验证它真的被当成分母消费（改动前无人读这个字段）。
        let mut conversation = empty_conversation();
        conversation.agent_runtime.external_agent_id = Some("opencode".to_string());
        conversation.messages.push(message(
            "a1",
            "assistant",
            "hi",
            Some(ModelUsage {
                input_tokens: Some(13_477),
                output_tokens: Some(4),
                total_tokens: Some(13_481),
                context_window_tokens: Some(200_000),
                ..Default::default()
            }),
        ));

        let state = compute_external_context_state(
            &conversation,
            "opencode",
            "opencode/big-pickle",
            None,
            None,
            None,
        );
        assert_eq!(state.context_window_tokens, Some(200_000));
        assert_eq!(state.estimated_input_tokens, 13_481);
        let ratio = state.usage_ratio.expect("ratio");
        assert!((ratio - 13_481.0 / 200_000.0).abs() < 1e-6);
        assert_eq!(state.status, "normal");
    }

    #[test]
    fn external_context_uses_latest_assistant_usage() {
        let mut conversation = empty_conversation();
        conversation
            .messages
            .push(message("u1", "user", "hi", None));
        conversation.messages.push(message(
            "a1",
            "assistant",
            "hello",
            Some(ModelUsage {
                input_tokens: Some(1439),
                output_tokens: Some(28),
                total_tokens: Some(1467),
                ..Default::default()
            }),
        ));

        let state = compute_external_context_state(
            &conversation,
            "pi",
            "anthropic/claude-sonnet-4-5",
            None,
            None,
            None,
        );
        assert_eq!(state.estimated_input_tokens, 1467);
        assert_eq!(state.token_count_source.as_deref(), Some(TOKEN_COUNT_CLI));
        assert_eq!(
            state.context_source.as_deref(),
            Some(CONTEXT_SOURCE_EXTERNAL)
        );
        assert!(state.summary.is_none());
        assert!(state.usage_ratio.unwrap() > 0.0);
    }

    #[test]
    fn kimi_falls_back_to_wire_log_before_character_estimate() {
        // kimi 的 ACP 上游不报任何用量 → 消息上没有 usage。改动前只能字符估算（把 23605 估成 ~24）。
        let home = std::env::temp_dir().join(format!(
            "kivio-ctx-kimi-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let work_dir = home.join("ws").join("conv-1");
        let session_dir = home.join("sessions").join("session-a");
        std::fs::create_dir_all(session_dir.join("agents").join("main")).unwrap();
        std::fs::create_dir_all(&work_dir).unwrap();
        std::fs::write(
            session_dir.join("agents").join("main").join("wire.jsonl"),
            "{\"type\":\"usage.record\",\"model\":\"kimi-code/k3-256k\",\"usage\":{\"inputOther\":565,\"output\":228,\"inputCacheRead\":23040,\"inputCacheCreation\":0},\"usageScope\":\"turn\",\"time\":1}\n",
        )
        .unwrap();
        std::fs::write(
            home.join("session_index.jsonl"),
            format!(
                "{}\n",
                serde_json::json!({
                    "sessionId": "session-a",
                    "sessionDir": session_dir.to_string_lossy(),
                    "workDir": work_dir.to_string_lossy(),
                })
            ),
        )
        .unwrap();

        let original = std::env::var("KIMI_CODE_HOME").ok();
        std::env::set_var("KIMI_CODE_HOME", &home);

        let mut conversation = empty_conversation();
        conversation.agent_runtime.external_agent_id = Some("kimi".to_string());
        conversation.agent_runtime.external_model = Some("kimi-code/k3-256k".to_string());
        conversation
            .messages
            .push(message("u1", "user", "帮我看一下", None));
        conversation
            .messages
            .push(message("a1", "assistant", "好的", None));

        let usage = collect_external_session_usage(&conversation, "kimi", Some(&work_dir));
        // wire.jsonl 的 input 已含 cache（565 + 23040），再加 output 228 = 上下文占用。
        assert_eq!(usage.input_tokens, 23_833);
        assert_eq!(usage.output_tokens, 228);
        assert_eq!(usage.token_count_source, TOKEN_COUNT_CLI);

        let state = compute_external_context_state(
            &conversation,
            "kimi",
            "kimi-code/k3-256k",
            None,
            None,
            Some(&work_dir),
        );
        assert_eq!(state.estimated_input_tokens, 23_833);
        assert_eq!(state.context_window_tokens, Some(262_144));

        // 不给 work_dir 时退回字符估算（几个字，绝不会是万级）。
        let fallback = collect_external_session_usage(&conversation, "kimi", None);
        assert_eq!(fallback.token_count_source, TOKEN_COUNT_ESTIMATED);
        assert!(fallback.input_tokens < 100);

        // 其它 agent 即便给了同一个 work_dir 也不该走 kimi 日志。
        let other = collect_external_session_usage(&conversation, "cursor", Some(&work_dir));
        assert_eq!(other.token_count_source, TOKEN_COUNT_ESTIMATED);

        match original {
            Some(value) => std::env::set_var("KIMI_CODE_HOME", value),
            None => std::env::remove_var("KIMI_CODE_HOME"),
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    /// 分子必须把 cache 计入——这是整个任务的 R0 目标。
    ///
    /// 各 CLI 解析层已把 cache 填进 `cached_input_tokens` / `cache_creation_input_tokens`
    /// 并汇总进 `total_tokens`，但用量条真正显示的是本函数返回的 `input_tokens`。
    /// 若这里只读 `usage.input_tokens`（Anthropic 口径下那是**非缓存**部分），
    /// 前面几层补的 cache 就全被丢在半路——实测 kimi cache 占 97.6%、pi 62%、opencode 13%。
    #[test]
    fn cli_reported_numerator_counts_cache_tokens() {
        let mut conversation = empty_conversation();
        conversation.agent_runtime.external_agent_id = Some("claude".to_string());
        conversation.messages.push(message(
            "a1",
            "assistant",
            "hi",
            Some(ModelUsage {
                input_tokens: Some(1_200),
                output_tokens: Some(800),
                cached_input_tokens: Some(45_000),
                cache_creation_input_tokens: Some(300),
                total_tokens: Some(47_300),
                ..Default::default()
            }),
        ));

        let usage = collect_external_session_usage(&conversation, "claude", None);
        // 只读 input_tokens 会得到 1200，把 45300 个真实占用窗口的 cache token 丢掉。
        assert_eq!(usage.input_tokens, 47_300);
        assert_eq!(usage.token_count_source, TOKEN_COUNT_CLI);
    }

    /// ACP 的 `usage_update.used` 已经是「上下文里现有的全部 token」，被塞进 `input_tokens`
    /// 且 `usage_from_parts` 把它原样求进 `total_tokens`——两条路必须得出同一个数。
    #[test]
    fn acp_usage_update_numerator_is_unchanged_by_total_semantics() {
        use crate::external_agents::stream::{usage_from_parts, CliUsageParts};
        // 直接用生产构造器，避免手写 ModelUsage 时凭空造出一个不可能的组合。
        let reported = usage_from_parts(CliUsageParts {
            input: 13_477,
            context_window: Some(200_000),
            ..Default::default()
        });
        let mut conversation = empty_conversation();
        conversation.agent_runtime.external_agent_id = Some("opencode".to_string());
        conversation
            .messages
            .push(message("a1", "assistant", "hi", Some(reported)));
        let usage = collect_external_session_usage(&conversation, "opencode", None);
        assert_eq!(usage.input_tokens, 13_477);
        assert_eq!(usage.reported_context_window, Some(200_000));
    }

    /// **零用量的上报不许把分子清零**（A2 的第三道防线）。
    ///
    /// 没有 LLM 往返的轮次（未登录 / `/help` / 未知斜杠命令 / Kivio 自己发的 `/compact`）
    /// 会落下一条全 0 的 usage。判据若是 `input_tokens.is_some()`，`Some(0)` 会命中它 ⇒
    /// 用量条从 47K 掉到 0，直到下一轮真实回复才恢复。必须跳过它、继续往前找。
    #[test]
    fn all_zero_usage_is_skipped_so_the_numerator_does_not_drop_to_zero() {
        let mut conversation = empty_conversation();
        conversation.agent_runtime.external_agent_id = Some("claude".to_string());
        conversation.messages.push(message(
            "a1",
            "assistant",
            "真实回复",
            Some(ModelUsage {
                input_tokens: Some(1_200),
                output_tokens: Some(800),
                cached_input_tokens: Some(45_000),
                cache_creation_input_tokens: Some(300),
                total_tokens: Some(47_300),
                ..Default::default()
            }),
        ));
        conversation
            .messages
            .push(message("u2", "user", "/help", None));
        // `/help` 这轮：全零 usage（但可能带着窗口）。
        conversation.messages.push(message(
            "a2",
            "assistant",
            "claude 命令已执行",
            Some(ModelUsage {
                input_tokens: Some(0),
                output_tokens: Some(0),
                total_tokens: Some(0),
                context_window_tokens: Some(1_000_000),
                ..Default::default()
            }),
        ));

        let usage = collect_external_session_usage(&conversation, "claude", None);
        assert_eq!(
            usage.input_tokens, 47_300,
            "全零上报把分子清零了（修复前的症状）"
        );
        assert_eq!(usage.token_count_source, TOKEN_COUNT_CLI);
    }

    /// 只有全零上报时也不能把它当分子——退到字符估算（而不是显示 0）。
    #[test]
    fn only_zero_usage_falls_back_to_the_character_estimate() {
        let mut conversation = empty_conversation();
        conversation.messages.push(message(
            "u1",
            "user",
            "/help 这是一段足够长的用户消息",
            None,
        ));
        conversation.messages.push(message(
            "a1",
            "assistant",
            "claude 命令已执行",
            Some(ModelUsage {
                input_tokens: Some(0),
                output_tokens: Some(0),
                total_tokens: Some(0),
                ..Default::default()
            }),
        ));
        let usage = collect_external_session_usage(&conversation, "claude", None);
        assert_eq!(usage.token_count_source, TOKEN_COUNT_ESTIMATED);
        assert!(usage.input_tokens > 0);
    }

    /// 旧会话（改动前落盘）的 `usage` 只有 input/output，没有 `total_tokens`。
    /// 必须退回 input+output，不能因为 total 缺失就显示 0。
    #[test]
    fn legacy_usage_without_total_falls_back_to_input_plus_output() {
        let mut conversation = empty_conversation();
        conversation.messages.push(message(
            "a1",
            "assistant",
            "hi",
            Some(ModelUsage {
                input_tokens: Some(1439),
                output_tokens: Some(28),
                total_tokens: None,
                ..Default::default()
            }),
        ));
        let usage = collect_external_session_usage(&conversation, "pi", None);
        assert_eq!(usage.input_tokens, 1467);
    }

    #[test]
    fn cli_reported_usage_still_beats_kimi_wire_log() {
        // 消息上已有 CLI 实报时不该去读日志（日志是"上游什么都不报"时的兜底）。
        let mut conversation = empty_conversation();
        conversation.agent_runtime.external_agent_id = Some("kimi".to_string());
        conversation.messages.push(message(
            "a1",
            "assistant",
            "hi",
            Some(ModelUsage {
                input_tokens: Some(777),
                output_tokens: Some(3),
                total_tokens: Some(780),
                ..Default::default()
            }),
        ));
        let usage = collect_external_session_usage(
            &conversation,
            "kimi",
            Some(Path::new("/nonexistent/workdir")),
        );
        assert_eq!(usage.input_tokens, 780);
        assert_eq!(usage.token_count_source, TOKEN_COUNT_CLI);
    }

    /// 这条删掉后 `cli_reported_context_tokens` 的 kimi 分支就没人钉了。
    #[test]
    fn kimi_wire_usage_struct_sums_cache_into_input() {
        let usage = kimi_usage::KimiTurnUsage {
            input_tokens: 23_605,
            output_tokens: 228,
            cache_read_tokens: 23_040,
            cache_creation_tokens: 0,
            model: None,
        };
        // input 已含 cache：565 + 23040 = 23605，远大于非缓存的 565。
        assert!(usage.input_tokens > 40 * 565);
        assert_eq!(usage.input_tokens + usage.output_tokens, 23_833);
    }
}
