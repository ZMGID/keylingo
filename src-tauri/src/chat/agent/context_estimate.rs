//! 真实用量锚点：把 provider 实报的 `usage` 作为上下文占用的 ground-truth 锚点，
//! 只对「锚点响应**之后**新增的消息」做字符估算叠加（对齐 pi/opencode 的分层口径：
//! API usage 为主，chars 启发式仅兜底）。
//!
//! 口径对齐 pi 的 `contextTokens = calculateContextTokens(lastUsage) + Σ estimate(其后消息)`：
//! - `anchor_total_tokens` = 上次调用「整个 prompt + 该次响应」的真实 token 数（= 下一次请求
//!   input 的主体，含 output，因为响应已成为历史）；
//! - trailing 估算只覆盖锚点响应**之后**新增的消息（响应本身用真实 output 计入锚点，不重复估）；
//! - 最终 `max(effective, 纯字符估算)` 作保守下限，保证引入锚点永不比现状更乐观。

use crate::chat::model::ModelUsage;

/// provider 实报 `usage` → 「上次整个 prompt + 该次响应」的 token 总数（对齐 pi 的
/// `calculateContextTokens = totalTokens || input+output+cacheRead+cacheWrite`），按 provider
/// 家族消歧缓存计数：
/// - `anthropic_messages`：`input_tokens` 是**非缓存**部分，全量 = `input + output + cache_read +
///   cache_creation`（四者不相交）。**不**用 Kivio 的 `total_tokens`——它对 Anthropic 只存
///   `input+output`、漏了 cache（见 `usage::model_usage_from_anthropic_value`）。
/// - 其它（`openai_*` / responses）：优先 `total_tokens`（= prompt+completion，prompt 已含 cached，
///   无双算）；缺失则 `input(=prompt,含cached) + output`。**不**再叠加 cached（子集，叠加即双算，
///   opencode #4416 踩过）。
///
/// 所需字段全缺（provider 未报）→ `None`，调用方回落纯估算。
pub(crate) fn anchor_total_tokens(usage: &ModelUsage, api_format: &str) -> Option<u64> {
    if api_format == "anthropic_messages" {
        let input = usage.input_tokens?;
        Some(
            input
                .saturating_add(usage.output_tokens.unwrap_or(0))
                .saturating_add(usage.cached_input_tokens.unwrap_or(0))
                .saturating_add(usage.cache_creation_input_tokens.unwrap_or(0)),
        )
    } else {
        usage.total_tokens.or_else(|| {
            usage
                .input_tokens
                .map(|input| input.saturating_add(usage.output_tokens.unwrap_or(0)))
        })
    }
}

/// 只算 **prompt 侧**（不含本次输出）的 token —— `anchor_total_tokens` 减掉 output。
///
/// 静默超窗检测（pi `isContextOverflow` case 2/3）要比的是「我们发过去多少」和
/// 「窗口多大」，把响应的 output 算进来会让判定虚高。分家族口径与
/// `anchor_total_tokens` 一致：
/// - `anthropic_messages`：`input_tokens` 是**非缓存**部分，全量 prompt =
///   `input + cache_read + cache_creation`（三者不相交，对齐 pi 的 `input + cacheRead`）。
/// - 其它（`openai_*` / responses）：`input_tokens`(=prompt) 已含 cached，**不**再叠加。
pub(crate) fn prompt_tokens(usage: &ModelUsage, api_format: &str) -> Option<u64> {
    let input = usage.input_tokens?;
    if api_format == "anthropic_messages" {
        Some(
            input
                .saturating_add(usage.cached_input_tokens.unwrap_or(0))
                .saturating_add(usage.cache_creation_input_tokens.unwrap_or(0)),
        )
    } else {
        Some(input)
    }
}

/// 计算上下文有效占用与是否采用了真实锚点。
///
/// - `anchor_total`：`Some` = 有可用锚点（上次「prompt+响应」真实 token 总数）；`None` = 无锚点。
/// - `trailing_estimate`：锚点响应**之后**新增消息的字符估算。
/// - `estimate_full`：整段对话的纯字符估算（含工具 schema，现状口径），作保守下限。
///
/// 返回 `(tokens, anchored)`：`anchored == true` 仅当真实锚点值确实被采用（≥ 纯估算），
/// 供 footer 决定标注「模型实报」还是「估算」。锚点值反而更小时（极少）取纯估算并标 `false`。
pub(crate) fn effective_context_tokens(
    anchor_total: Option<u64>,
    trailing_estimate: usize,
    estimate_full: usize,
) -> (usize, bool) {
    match anchor_total {
        Some(total) => {
            let anchored = (total as usize).saturating_add(trailing_estimate);
            if anchored >= estimate_full {
                (anchored, true)
            } else {
                // 锚点+增量竟小于纯估算（罕见，如锚点来自被压缩过的更小 prompt）——
                // 取更保守的纯估算，且不标记为真实锚点。
                (estimate_full, false)
            }
        }
        None => (estimate_full, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: Option<u64>, cached: Option<u64>, cache_create: Option<u64>) -> ModelUsage {
        ModelUsage {
            input_tokens: input,
            output_tokens: Some(100),
            total_tokens: None,
            cached_input_tokens: cached,
            cache_creation_input_tokens: cache_create,
            reasoning_tokens: None,
            context_window_tokens: None,
        }
    }

    #[test]
    fn anthropic_sums_disjoint_parts_including_output() {
        // Anthropic: input 非缓存 + output + cache_read + cache_creation = 全量（含响应）。
        let u = usage(Some(100_000), Some(50_000), Some(20_000));
        assert_eq!(
            anchor_total_tokens(&u, "anthropic_messages"),
            Some(170_100) // 100000 + 100(out) + 50000 + 20000
        );
    }

    #[test]
    fn anthropic_ignores_kivio_total_tokens_missing_cache() {
        // 即便 total_tokens 存在（Kivio 对 Anthropic 只填 input+output、漏 cache），也不能用它，
        // 必须显式加 cache。
        let mut u = usage(Some(100_000), Some(50_000), None);
        u.total_tokens = Some(100_100); // = input+output，漏了 cache
        assert_eq!(
            anchor_total_tokens(&u, "anthropic_messages"),
            Some(150_100) // 100000 + 100 + 50000（cache 必须算进去）
        );
    }

    #[test]
    fn openai_prefers_total_tokens_no_double_count() {
        // OpenAI: 有 total_tokens 直接用（prompt 已含 cached，无双算）。
        let mut u = usage(Some(100_000), Some(50_000), None);
        u.total_tokens = Some(100_500);
        assert_eq!(anchor_total_tokens(&u, "openai_chat"), Some(100_500));
    }

    #[test]
    fn openai_falls_back_to_input_plus_output() {
        // 无 total_tokens：input(=prompt,含cached) + output，绝不叠加 cached。
        let u = usage(Some(100_000), Some(50_000), None);
        assert_eq!(anchor_total_tokens(&u, "openai_chat"), Some(100_100));
        assert_eq!(anchor_total_tokens(&u, "openai_responses"), Some(100_100));
    }

    #[test]
    fn missing_fields_yield_none() {
        let mut u = usage(None, Some(10), None);
        u.total_tokens = None;
        assert_eq!(anchor_total_tokens(&u, "anthropic_messages"), None);
        assert_eq!(anchor_total_tokens(&u, "openai_chat"), None);
    }

    #[test]
    fn effective_prefers_anchor_when_not_smaller() {
        assert_eq!(
            effective_context_tokens(Some(240_000), 2_000, 90_000),
            (242_000, true)
        );
    }

    #[test]
    fn effective_falls_back_to_estimate_when_anchor_smaller() {
        assert_eq!(
            effective_context_tokens(Some(10_000), 1_000, 50_000),
            (50_000, false)
        );
    }

    #[test]
    fn effective_no_anchor_uses_estimate() {
        assert_eq!(effective_context_tokens(None, 0, 42_000), (42_000, false));
    }

    #[test]
    fn prompt_tokens_excludes_output_and_sums_anthropic_cache() {
        // Anthropic: input 是非缓存部分，cache 必须补上；output(100) 不能算进来。
        let u = usage(Some(100_000), Some(50_000), Some(20_000));
        assert_eq!(prompt_tokens(&u, "anthropic_messages"), Some(170_000));
        // OpenAI 系：input 已含 cached，不再叠加，也不含 output。
        assert_eq!(prompt_tokens(&u, "openai_chat"), Some(100_000));
        assert_eq!(prompt_tokens(&u, "openai_responses"), Some(100_000));
    }

    #[test]
    fn prompt_tokens_needs_input() {
        let u = usage(None, Some(10), Some(10));
        assert_eq!(prompt_tokens(&u, "anthropic_messages"), None);
        assert_eq!(prompt_tokens(&u, "openai_chat"), None);
    }
}
