use serde_json::Value;

use crate::chat::model::ModelUsage;
use crate::external_agents::types::{StreamFormat, UnifiedAgentEvent};

pub mod claude;

pub fn create_stream_handler(format: StreamFormat) -> StreamHandler {
    match format {
        StreamFormat::ClaudeStreamJson => StreamHandler(claude::ClaudeStreamState::default()),
        // PiRpc / AcpJsonRpc / CodexAppServer / DshJsonRpc are driven by dedicated session
        // runners in run.rs and never reach this factory.
        StreamFormat::PiRpc
        | StreamFormat::AcpJsonRpc
        | StreamFormat::CodexAppServer
        | StreamFormat::DshJsonRpc => {
            unreachable!("{format:?} uses a dedicated session runner, not create_stream_handler")
        }
    }
}

pub struct StreamHandler(claude::ClaudeStreamState);

impl StreamHandler {
    pub fn handle_line(&mut self, line: &str, sink: &mut dyn FnMut(UnifiedAgentEvent)) {
        let value = match serde_json::from_str::<Value>(line.trim()) {
            Ok(v) => v,
            Err(_) => {
                // Not JSON — surface it as a raw line rather than dropping it, so a CLI that
                // prints a plain-text error/notice doesn't leave the run looking empty.
                sink(UnifiedAgentEvent::Raw {
                    line: line.to_string(),
                });
                return;
            }
        };
        self.handle_value(&value, sink);
    }

    /// 喂一帧**已解析**的 JSON。
    ///
    /// 常驻会话（`session/claude_stream.rs`）必须先看一眼帧类型才能决定这一帧是「交给解析器」
    /// 还是「控制通道的事，要回一条 `control_response`」，所以它在读循环里就已经把这一行解析成
    /// `Value` 了。给它一个入口比让它把同一行 JSON 再交给 `handle_line` 解析第二遍好（spec 第 2
    /// 条：不要出现两份），也避免把控制帧误喂给流解析器。
    pub fn handle_value(&mut self, value: &Value, sink: &mut dyn FnMut(UnifiedAgentEvent)) {
        self.0.handle_value(value, sink);
    }

    /// 本轮是否读到了协议层明确的**完成标志**（claude 的 `result` 帧）。
    ///
    /// 出口用它豁免「非零退出码 = 失败」规则（spec 第 8b 条）：进程被我们主动杀掉时
    /// 退出码不可信（Windows `TerminateProcess` 恒为 1），而「CLI 说本轮结束了」是可信的。
    /// 真实的协议层失败走 `result.is_error` → `UnifiedAgentEvent::Error`，不受此豁免影响。
    pub fn saw_protocol_completion(&self) -> bool {
        self.0.completed_result_turns() > 0
    }

    /// 本会话（= 常驻进程下的整个对话）已经收完的 `result` 轮次数。
    ///
    /// 常驻会话用它做**轮次边界检测**：喂一行前后各取一次，数字涨了就说明本轮结束
    /// （claude 每轮恰好一个 `result`，被中断的轮次也有）。这样就不用为了找边界把
    /// 同一行 JSON 再解析一遍。
    pub fn completed_result_turns(&self) -> u32 {
        self.0.completed_result_turns()
    }

    /// 最近一个 `result` 是不是用户中断的收尾（`terminal_reason == "aborted_streaming"`）。
    /// 常驻会话据此把这一轮送去「已取消」出口而不是错误出口。
    pub fn last_result_aborted(&self) -> bool {
        self.0.last_result_aborted()
    }

    /// 最近一次 `system/init` 报的模型（claude 常驻会话跨轮记忆）。唤醒消息的模型归属用。
    pub fn resolved_model(&self) -> Option<&str> {
        self.0.resolved_model()
    }
}

/// 一次 CLI 用量上报的原始分量。各 CLI 只填其中几项，其余走 `..Default::default()`。
///
/// 用具名结构体而非位置参数：五个同类型 `u64` 连排极易传错序，而 cache 分量传错
/// 不会报错、只会让用量条静默失真——正是本次修复要根除的那类 bug。
#[derive(Debug, Clone, Default)]
pub struct CliUsageParts {
    /// 输入 token。是否已含 `cache_read` 由 `cache_included_in_input` 决定。
    pub input: u64,
    /// 输出 token（含推理 token——pi/codex 实测均已含在其 output 内，不要重复累加）。
    pub output: u64,
    /// 缓存命中读取的 token。**照样占上下文窗口**，只是不重复计费。
    pub cache_read: u64,
    /// 写入缓存的 token。
    pub cache_creation: u64,
    /// 推理 token，仅当 CLI 把它与 output **并列**上报时才填（ACP `thoughtTokens`）。
    pub reasoning: u64,
    /// **cache 与 input 的包含关系**，各 CLI 不同，必须逐个实测确认，不能想当然。
    ///
    /// - `false`（默认，Anthropic 口径）：`cache_read` 与 `input` **不相交**，两者相加才是
    ///   全量输入。claude / pi / ACP `PromptResponse.usage` 均属此类（实测对账见各调用点注释）。
    /// - `true`（OpenAI 口径）：`cache_read` 是 `input` 的**子集**，再加一遍就是双算。
    ///   codex 实测属此类：`inputTokens 16865 + outputTokens 7 = 16872 = totalTokens`，
    ///   而 `cachedInputTokens` 是其中的 3456（2026-07-26 本机 codex-cli 0.145.0 实测）。
    ///
    /// 这个区分与内置路径的 `chat::agent::context_estimate::anchor_total_tokens`
    /// 按 `api_format` 分家族消歧是同一件事——别在这里统一，会算错一整类 provider。
    pub cache_included_in_input: bool,
    /// CLI 自报的上下文窗口总大小（ACP `usage_update.size`）。
    pub context_window: Option<u64>,
}

/// 由分量构造 `ModelUsage`。
///
/// `total_tokens` 是**上下文占用**口径（不是计费口径）：缓存命中的 token 依然占据窗口，
/// 所以在 cache 与 input 不相交时必须相加——这**不是笔误**，与内置路径
/// `chat::agent::context_estimate::anchor_total_tokens` 的 `anthropic_messages` 分支一致。
/// 漏掉 cache 会让长会话的用量低估一个数量级（实测 kimi 侧 cache 占 97.6%、pi 62%）。
///
/// 反过来，cache 已含在 input 里时（codex 实测）再加一遍就是双算，进度条会虚高——
/// 见 `CliUsageParts::cache_included_in_input`。
pub fn usage_from_parts(parts: CliUsageParts) -> ModelUsage {
    let mut total = parts
        .input
        .saturating_add(parts.output)
        .saturating_add(parts.cache_creation)
        .saturating_add(parts.reasoning);
    if !parts.cache_included_in_input {
        total = total.saturating_add(parts.cache_read);
    }
    ModelUsage {
        input_tokens: Some(parts.input),
        output_tokens: Some(parts.output),
        total_tokens: Some(total),
        cached_input_tokens: (parts.cache_read > 0).then_some(parts.cache_read),
        cache_creation_input_tokens: (parts.cache_creation > 0).then_some(parts.cache_creation),
        reasoning_tokens: (parts.reasoning > 0).then_some(parts.reasoning),
        context_window_tokens: parts.context_window,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_from_numbers_totals_input_and_output() {
        let usage = usage_from_parts(CliUsageParts {
            input: 5,
            output: 7,
            ..Default::default()
        });
        assert_eq!(usage.input_tokens, Some(5));
        assert_eq!(usage.output_tokens, Some(7));
        assert_eq!(usage.total_tokens, Some(12));
        // 没有 cache 分量时不应凭空造出 Some(0)——下游用 is_some 判断「CLI 报了没有」。
        assert_eq!(usage.cached_input_tokens, None);
        assert_eq!(usage.cache_creation_input_tokens, None);
        assert_eq!(usage.context_window_tokens, None);
    }

    #[test]
    fn total_counts_cache_tokens_because_they_occupy_the_window() {
        // opencode 实测样本：11685 + 4 + 11 + 1792 = 13492 = 其自报的 totalTokens。
        let usage = usage_from_parts(CliUsageParts {
            input: 11_685,
            output: 4,
            cache_read: 1_792,
            reasoning: 11,
            ..Default::default()
        });
        assert_eq!(usage.total_tokens, Some(13_492));
        assert_eq!(usage.cached_input_tokens, Some(1_792));
        assert_eq!(usage.reasoning_tokens, Some(11));
    }

    #[test]
    fn context_window_rides_along_with_usage() {
        let usage = usage_from_parts(CliUsageParts {
            input: 13_477,
            context_window: Some(200_000),
            ..Default::default()
        });
        assert_eq!(usage.context_window_tokens, Some(200_000));
    }

    #[test]
    fn cache_already_inside_input_is_not_double_counted() {
        // codex 本机实测（codex-cli 0.145.0, 2026-07-26）：
        // inputTokens 16865 + outputTokens 7 = 16872 = totalTokens，
        // 而 cachedInputTokens 3456 是 inputTokens 的**子集**。再加一遍会得到 20328（虚高 20%）。
        let usage = usage_from_parts(CliUsageParts {
            input: 16_865,
            output: 7,
            cache_read: 3_456,
            cache_included_in_input: true,
            ..Default::default()
        });
        assert_eq!(usage.total_tokens, Some(16_872));
        // cache 仍要如实记录（成本面板与「缓存命中率」要用），只是不重复计入 total。
        assert_eq!(usage.cached_input_tokens, Some(3_456));
    }

    #[test]
    fn disjoint_cache_is_added_on_top_of_input() {
        // 同样的数字，在 Anthropic 口径（cache 与 input 不相交）下必须相加。
        let usage = usage_from_parts(CliUsageParts {
            input: 16_865,
            output: 7,
            cache_read: 3_456,
            cache_included_in_input: false,
            ..Default::default()
        });
        assert_eq!(usage.total_tokens, Some(20_328));
    }
}
