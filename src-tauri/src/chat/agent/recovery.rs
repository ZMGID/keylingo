//! 框架级:模型调用失败的统一分类 + 恢复策略中枢。
//!
//! **职责边界(对标 PI 的 agent-session)**:所有「模型调用失败后怎么办」的语义级决策
//! 只在这里表达——`classify`(失败归类)+ `decide`(策略选择)。synthesis / planning 的
//! 失败路径统一调用恢复入口(`synthesis::recover_synthesis`)执行本模块给出的动作,
//! 不再各自散写 overflow / 去敏 / 兜底 if-else。
//! 传输层重试(429 / 5xx / 网络退避、换 key)归 `api.rs::send_with_retry` / `send_with_failover`,
//! 与本模块的语义级恢复(overflow 压缩重试 / 去敏 / 确定性兜底)互不重叠。
//!
//! 设计目标:一种失败 = 一条分类(`classify`)+ 一条策略(`decide`),所有模型调用阶段
//! 共用;并保证「产生过工具结果的轮次永不空手而归」这一不变式只在此处定义
//! (`DegradeToGathered` → `assemble_results_from_tool_records`)。
//!
//! 不重复造轮子:沿用 `api::extract_status_code` 从错误串里取 HTTP 状态码(failover 逻辑
//! 也是这么做的),内容审核 / 超长靠 body 关键词判定。错误既可能是流式 `ModelError`,
//! 也可能是非流式 `String`,统一按消息文本分类即可,无需改动适配器返回类型。

use crate::chat::types::{ToolCallRecord, ToolCallStatus};

/// 降级回答的结构化描述，落到 `ChatMessage.degraded` 供前端渲染成独立卡片。
///
/// 为什么不只发拼好的文本：降级信息此前是一段纯文本塞进 assistant 正文，看起来和
/// 模型的真实回答一模一样，会被复制、会进对话历史再发回模型。结构化后正文归正文、
/// 故障归卡片。`text` 保留同样的纯文本，供旧前端与外部 CLI 回落。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DegradedAnswer {
    /// 失败归类的稳定标识（`rate_limited` / `context_overflow` / `moderation` /
    /// `empty_response` / `unknown`），前端据此选图标与措辞，不解析文案。
    pub kind: String,
    /// 一行人读的失败原因（已按 kind 精确化，不再笼统说"可能上下文过长"）。
    pub reason: String,
    /// 本轮已完成的工具调用摘要（名称 + 结果首行），让用户知道哪些工作没白做。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_summaries: Vec<DegradedToolSummary>,
    /// 与 `tool_summaries` 同源的纯文本版本；旧前端 / 外部 CLI 直接显示这个。
    pub text: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DegradedToolSummary {
    pub name: String,
    pub preview: String,
}

impl FailureKind {
    /// 稳定的 wire 标识。前端按它分支，不依赖本地化文案。
    pub(crate) fn wire_kind(self) -> &'static str {
        match self {
            FailureKind::ContentModeration => "moderation",
            FailureKind::ContextOverflow => "context_overflow",
            FailureKind::Empty => "empty_response",
            FailureKind::Exhausted => "rate_limited",
            FailureKind::Other => "unknown",
        }
    }
}

/// 模型调用失败的归类(只列出我们会**区别处置**的类型)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureKind {
    /// 供应商内容审核拒绝(典型:400 + "content/risk/policy/safety/审核")。
    ContentModeration,
    /// 上下文超长(400/413 + "context/maximum/token length")。
    ContextOverflow,
    /// 模型调用成功但产出为空。
    Empty,
    /// 限流 / 鉴权 / 5xx / 网络等——底层 api.rs 已重试或换 key,升到这层即已耗尽。
    Exhausted,
    /// 其它(无法归类)。
    Other,
}

/// 对一次失败应采取的恢复动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryAction {
    /// 上下文超长:先压缩一次历史(maybe_compact_send_view),再用压缩后的消息重发一次。
    /// 对标 PI 的 `_overflowRecoveryAttempted` 单次守门——只压缩-重试一次,避免「压完仍超」死循环。
    CompactAndRetry,
    /// 用"去敏 + 精简"的输入重做一次合成(可能产出真正的总结)。
    Remediate,
    /// 直接用已收集到的工具结果确定性兜底(不经模型 → 不被审核)。
    DegradeToGathered,
    /// 无可恢复(且没有工具结果)——交回上层用静态文案。
    Surface,
}

/// 上下文超长信号:覆盖 OpenAI / Anthropic / Gemini / 各代理 / DeepSeek 及国内供应商
/// 常见文案(对标 PI `ai/src/utils/overflow.ts` 的 `OVERFLOW_PATTERNS` 子集)。
/// 命中其一即认为是「上下文撑爆」类失败,应走压缩重试而非快速失败。
const OVERFLOW_PATTERNS: &[&str] = &[
    // OpenAI / 通用
    "maximum context",
    "context length",
    "context_length_exceeded",
    "context window",
    "too many tokens",
    "reduce the length",
    "reduce your prompt",
    "string too long",
    "max_tokens",
    "input is too long",
    "input too long",
    // Anthropic
    "prompt is too long",
    "request too large",
    "request_too_large",
    // Gemini / 其它代理
    "exceeds the maximum",
    "exceeds the limit",
    "exceeds the context",
    "token limit",
    "tokens limit",
    "exceed the context",
    "exceed context",
    // 国内供应商 / 中文文案
    "上下文长度",
    "上下文过长",
    "超出最大",
    "超过最大",
    "输入过长",
    "tokens 超",
];

/// 排除项:这些文案虽含「limit/token」等词,实为限流/配额而非上下文超长,
/// 命中则**不**判为 overflow(对标 PI `NON_OVERFLOW_PATTERNS`,防止把限流误判成 overflow)。
const NON_OVERFLOW_PATTERNS: &[&str] = &[
    "rate limit",
    "rate_limit",
    "too many requests",
    "requests per",
    "quota",
    "insufficient",
    "billing",
    "usage limit",
    "available balance",
];

/// 判断错误文本是否为「上下文超长」(overflow)。先排除限流/配额误判,再匹配 overflow 文案。
fn is_context_overflow(lower: &str) -> bool {
    if NON_OVERFLOW_PATTERNS.iter().any(|n| lower.contains(n)) {
        return false;
    }
    OVERFLOW_PATTERNS.iter().any(|n| lower.contains(n))
}

/// 把错误消息文本归类。`message` 为空视为 `Empty`(调用方在"成功但空响应"时传空串)。
pub(crate) fn classify(message: &str) -> FailureKind {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return FailureKind::Empty;
    }
    let status = crate::api::extract_status_code(trimmed);
    let lower = trimmed.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));

    // 内容审核:供应商措辞不一,关键词覆盖中英常见形态。
    if has(&[
        "content exists risk",
        "content policy",
        "content_policy",
        "content filter",
        "moderation",
        "safety",
        "sensitive",
        "审核",
        "违规",
        "敏感",
    ]) {
        return FailureKind::ContentModeration;
    }
    // 上下文超长。先于 status 判定:overflow 多以 400 返回,若不在此截获会被下面
    // 的 `_ => Other` 当成普通 BadRequest。命中 NON_OVERFLOW(限流/配额)则不算 overflow。
    if is_context_overflow(&lower) {
        return FailureKind::ContextOverflow;
    }
    match status {
        // 审核常以 400 返回但措辞没命中上面的词:仍按 BadRequest→Other 处理,交给
        // Remediate 兜一手(去敏精简后重试),不会更糟。
        Some(429) | Some(401) | Some(402) | Some(403) => FailureKind::Exhausted,
        Some(code) if (500..600).contains(&code) => FailureKind::Exhausted,
        _ => FailureKind::Other,
    }
}

/// 策略:给定失败类型 + 上下文,决定动作。集中表达,取代各阶段散落判断。
///
/// `has_tool_results`:本轮是否已产生工具结果(决定能否兜底)。
/// `already_remediated`:是否已经做过一次 Remediate(避免无限重试)。
/// `overflow_recovery_attempted`:是否已经做过一次 CompactAndRetry(单次守门,避免
/// 「压缩后仍超 → 再压缩」死循环,对标 PI `_overflowRecoveryAttempted`)。
pub(crate) fn decide(
    kind: FailureKind,
    has_tool_results: bool,
    already_remediated: bool,
    overflow_recovery_attempted: bool,
) -> RecoveryAction {
    if !has_tool_results {
        // 没有可兜底的素材:只能交回上层(静态文案 / 向上传播错误)。
        return RecoveryAction::Surface;
    }
    if already_remediated {
        // 去敏重试都失败了,确定性兜底,保证有结果。
        return RecoveryAction::DegradeToGathered;
    }
    match kind {
        // 上下文超长:走「压缩一次 → 重发一次」专用通道(而非去敏重试)。压缩+重试已经
        // 试过一次仍失败 → 降级到确定性兜底,避免死循环(单次守门)。
        FailureKind::ContextOverflow => {
            if overflow_recovery_attempted {
                RecoveryAction::DegradeToGathered
            } else {
                RecoveryAction::CompactAndRetry
            }
        }
        // 请求因内容被拒,或措辞没命中的 400(归到 Other)→ 用去敏精简的输入重做
        // 一次,通常能产出真正的总结;失败了下一轮 already_remediated 会兜底,不会更糟。
        FailureKind::ContentModeration | FailureKind::Other => RecoveryAction::Remediate,
        // 空响应 / 限流耗尽:重做无意义(同样的输入只会再失败),直接用已收集结果兜底。
        FailureKind::Empty | FailureKind::Exhausted => RecoveryAction::DegradeToGathered,
    }
}

/// 不变式实现:合成失败后,确定性拼出一条**简短**的降级消息(不经模型,不会被审核)。
///
/// 形态:一行真实原因(由 `kind` 决定,例如限流 vs 上下文超长——不再笼统甩"可能上下文过长"
/// 误导用户)+ 已完成工具的**裁剪后**摘要(每条仅首行、限条数)。完整结果已在上方以工具卡片
/// 呈现,这里再 dump 全文只会刷出几百行,故只留指针式短摘要。没有任何成功 preview → 返回空串
/// (调用方据此退回静态文案)。
pub(crate) fn assemble_results_from_tool_records(
    records: &[ToolCallRecord],
    language: &str,
    kind: FailureKind,
) -> String {
    assemble_degraded_answer(records, language, kind)
        .map(|d| d.text)
        .unwrap_or_default()
}

/// 结构化版本：同一套裁剪规则，但把「原因 / 工具摘要 / 纯文本」分开返回，
/// 让前端能渲染成独立卡片而不是把故障混进 assistant 正文。
/// 没有任何成功 preview → None（调用方退回静态文案，与文本版语义一致）。
pub(crate) fn assemble_degraded_answer(
    records: &[ToolCallRecord],
    language: &str,
    kind: FailureKind,
) -> Option<DegradedAnswer> {
    /// 摘要里最多列几条工具结果(其余折叠为计数)。
    const MAX_BLOCKS: usize = 8;
    /// 每条工具结果保留的最大字符数(只取首行)。
    const MAX_PREVIEW_CHARS: usize = 200;

    let zh = language.starts_with("zh");
    let mut blocks: Vec<String> = Vec::new();
    let mut summaries: Vec<DegradedToolSummary> = Vec::new();
    let mut overflow = 0usize;
    for record in records {
        if record.status != ToolCallStatus::Success {
            continue;
        }
        let preview = record
            .result_preview
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty());
        let Some(preview) = preview else {
            continue;
        };
        if blocks.len() >= MAX_BLOCKS {
            overflow += 1;
            continue;
        }
        // 只取首个非空行并裁剪——避免把整份工具输出再糊一遍。
        let first_line = preview
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or(preview);
        let clipped: String = if first_line.chars().count() > MAX_PREVIEW_CHARS {
            format!(
                "{}…",
                first_line
                    .chars()
                    .take(MAX_PREVIEW_CHARS)
                    .collect::<String>()
            )
        } else {
            first_line.to_string()
        };
        summaries.push(DegradedToolSummary {
            name: record.name.clone(),
            preview: clipped.clone(),
        });
        blocks.push(format!("• {} — {}", record.name, clipped));
    }
    if blocks.is_empty() {
        return None;
    }
    let reason = failure_reason_line(kind, zh);
    let note = if zh {
        format!(
            "本轮已完成 {} 个工具调用,完整结果见上方卡片:",
            blocks.len() + overflow
        )
    } else {
        format!(
            "Completed {} tool call(s) this round — full results are shown above:",
            blocks.len() + overflow
        )
    };
    let mut out = format!("{reason}\n\n{note}\n{}", blocks.join("\n"));
    if overflow > 0 {
        out.push_str(&if zh {
            format!("\n…(另有 {overflow} 条见上方)")
        } else {
            format!("\n… ({overflow} more above)")
        });
    }
    Some(DegradedAnswer {
        kind: kind.wire_kind().to_string(),
        reason: reason.to_string(),
        tool_summaries: summaries,
        text: out,
    })
}

/// 上下文超长的静态说明文案（Gap 2 anti-thrashing 兜底用）：当压缩反复失败、且没有任何
/// 工具结果可降级时，至少给用户一条明确的上下文超长提示，而不是空字符串/笼统报错。
/// 复用 `failure_reason_line` 的 ContextOverflow 文案，保持口径一致。
pub(crate) fn overflow_static_message(language: &str) -> String {
    failure_reason_line(FailureKind::ContextOverflow, language.starts_with("zh")).to_string()
}

/// 一行人读的失败原因(按 `kind`)。这是修复「降级消息误导」的核心:429 就说限流,
/// 上下文超长就说上下文,绝不再把限流说成"可能上下文过长"。
fn failure_reason_line(kind: FailureKind, zh: bool) -> &'static str {
    match kind {
        FailureKind::Exhausted => {
            if zh {
                "⚠️ 模型调用被限流或配额耗尽(HTTP 429/5xx),多次退避重试后仍失败 —— 这是请求频率/配额问题,与上下文长度无关。请稍后重试,或为该供应商添加备用 key、更换供应商。"
            } else {
                "⚠️ The model was rate-limited or quota-exhausted (HTTP 429/5xx) and still failed after several backoff retries. This is a request-rate/quota issue, unrelated to context length. Retry later, add a backup key, or switch providers."
            }
        }
        FailureKind::ContextOverflow => {
            if zh {
                "⚠️ 上下文超出模型窗口,压缩后重试仍失败。请改用更大上下文的模型,或精简对话后重试。"
            } else {
                "⚠️ The context exceeded the model's window and still failed after compaction. Switch to a larger-context model or trim the conversation."
            }
        }
        FailureKind::ContentModeration => {
            if zh {
                "⚠️ 请求被供应商内容审核拦截。换个措辞,或更换供应商后重试。"
            } else {
                "⚠️ The request was blocked by the provider's content moderation. Rephrase or switch providers."
            }
        }
        FailureKind::Empty => {
            if zh {
                "⚠️ 模型返回了空响应。请重试,或更换模型。"
            } else {
                "⚠️ The model returned an empty response. Retry or switch models."
            }
        }
        FailureKind::Other => {
            if zh {
                "⚠️ 模型调用失败。请重试,或更换供应商。"
            } else {
                "⚠️ The model call failed. Retry or switch providers."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(name: &str, status: ToolCallStatus, preview: Option<&str>) -> ToolCallRecord {
        ToolCallRecord {
            id: "t".into(),
            name: name.into(),
            source: "native".into(),
            server_id: None,
            arguments: String::new(),
            status,
            result_preview: preview.map(|p| p.to_string()),
            error: None,
            duration_ms: None,
            started_at: None,
            completed_at: None,
            round: 0,
            sensitive: false,
            artifacts: Vec::new(),
            trace_id: None,
            span_id: None,
            structured_content: None,
        }
    }

    #[test]
    fn classify_detects_moderation_overflow_empty() {
        assert_eq!(
            classify("Chat stream Error: 400 Bad Request - {\"error\":{\"message\":\"Content Exists Risk\"}}"),
            FailureKind::ContentModeration
        );
        assert_eq!(
            classify("Error: 400 - This model's maximum context length is 8192 tokens"),
            FailureKind::ContextOverflow
        );
        assert_eq!(classify(""), FailureKind::Empty);
        assert_eq!(
            classify("Chat stream Error: 429 Too Many Requests"),
            FailureKind::Exhausted
        );
        assert_eq!(
            classify("Chat API Error: 500 Internal Server Error"),
            FailureKind::Exhausted
        );
    }

    #[test]
    fn classify_detects_broad_overflow_provider_wording() {
        // 各供应商的超长文案都应归到 ContextOverflow(多以 400 返回)。
        for msg in [
            "Error: 400 - prompt is too long: 250000 tokens > 200000 maximum",
            "Chat API Error: 413 - request too large",
            "Error: 400 context_length_exceeded",
            "Error: 400 - input is too long for requested model",
            "Error: 400 - This request exceeds the maximum token limit",
            "Error: 400 reduce the length of the messages",
            "错误:400 - 上下文长度超出最大限制",
            "错误:400 - 输入过长",
        ] {
            assert_eq!(
                classify(msg),
                FailureKind::ContextOverflow,
                "should classify as overflow: {msg}"
            );
        }
    }

    #[test]
    fn classify_excludes_rate_limit_from_overflow() {
        // 限流/配额文案虽含 limit/token 等词,绝不能被当成 overflow。核心断言:不是 ContextOverflow。
        for msg in [
            "Chat stream Error: 429 Too Many Requests - rate limit exceeded, too many tokens per minute",
            "Chat API Error: 429 - quota exceeded for this token limit",
            "Chat API Error: 402 - insufficient available balance",
        ] {
            let kind = classify(msg);
            assert_ne!(kind, FailureKind::ContextOverflow, "must not be overflow: {msg}");
            assert_eq!(kind, FailureKind::Exhausted, "should be exhausted: {msg}");
        }
    }

    #[test]
    fn decide_upholds_invariant() {
        // 无工具结果 → 交回上层
        assert_eq!(
            decide(FailureKind::ContentModeration, false, false, false),
            RecoveryAction::Surface
        );
        // 审核 + 有结果 + 未补救 → 先去敏重试
        assert_eq!(
            decide(FailureKind::ContentModeration, true, false, false),
            RecoveryAction::Remediate
        );
        // 补救后仍失败 → 确定性兜底
        assert_eq!(
            decide(FailureKind::ContentModeration, true, true, false),
            RecoveryAction::DegradeToGathered
        );
        // 已耗尽(限流/5xx)+ 有结果 → 直接兜底
        assert_eq!(
            decide(FailureKind::Exhausted, true, false, false),
            RecoveryAction::DegradeToGathered
        );
        // 措辞没命中的 400(Other)+ 有结果 + 未补救 → 也先去敏重试(与 classify 注释一致)
        assert_eq!(
            decide(FailureKind::Other, true, false, false),
            RecoveryAction::Remediate
        );
        // 空响应重做无意义 → 直接兜底
        assert_eq!(
            decide(FailureKind::Empty, true, false, false),
            RecoveryAction::DegradeToGathered
        );
    }

    #[test]
    fn decide_overflow_compacts_once_then_degrades() {
        // overflow + 有结果 + 未尝试压缩重试 → 走压缩重试通道
        assert_eq!(
            decide(FailureKind::ContextOverflow, true, false, false),
            RecoveryAction::CompactAndRetry
        );
        // overflow + 已尝试过一次压缩重试 → 单次守门:降级兜底,不再压缩(防死循环)
        assert_eq!(
            decide(FailureKind::ContextOverflow, true, false, true),
            RecoveryAction::DegradeToGathered
        );
        // overflow + 无工具结果 → 仍交回上层(没素材可兜底)
        assert_eq!(
            decide(FailureKind::ContextOverflow, false, false, false),
            RecoveryAction::Surface
        );
    }

    #[test]
    fn assemble_uses_successful_previews_only() {
        let records = vec![
            rec("web_search", ToolCallStatus::Success, Some("标题A\n标题B")),
            rec("web_search", ToolCallStatus::Error, Some("不该出现")),
            rec("noop", ToolCallStatus::Success, None),
        ];
        let out = assemble_results_from_tool_records(&records, "zh-CN", FailureKind::Exhausted);
        assert!(out.contains("标题A"));
        assert!(!out.contains("不该出现"));
        assert!(out.contains("web_search"));
        // 真实原因行出现(限流),不再是误导的"可能上下文过长"。
        assert!(out.contains("限流") || out.contains("429"));

        assert!(
            assemble_results_from_tool_records(&[], "zh-CN", FailureKind::Exhausted).is_empty()
        );
    }

    #[test]
    fn structured_degraded_answer_carries_kind_reason_and_summaries() {
        // 结构化版本是错误卡片的数据源：kind 供前端选图标（不解析文案）、
        // reason 是人读原因、tool_summaries 让用户知道哪些工作没白做。
        let records = vec![
            rec(
                "mixer_generate_image",
                ToolCallStatus::Success,
                Some("Generated 1 image."),
            ),
            rec(
                "present_artifacts",
                ToolCallStatus::Success,
                Some("Displayed 1 file."),
            ),
            rec("failed_tool", ToolCallStatus::Error, Some("不该出现")),
        ];
        let d = assemble_degraded_answer(&records, "zh-CN", FailureKind::Exhausted)
            .expect("有成功工具结果 ⇒ 应产出结构化降级描述");
        assert_eq!(d.kind, "rate_limited");
        assert!(d.reason.contains("限流") || d.reason.contains("配额"));
        assert_eq!(d.tool_summaries.len(), 2, "失败的工具不进摘要");
        assert_eq!(d.tool_summaries[0].name, "mixer_generate_image");
        assert_eq!(d.tool_summaries[0].preview, "Generated 1 image.");
        // text 仍是同一段纯文本，供旧前端 / 外部 CLI 回落。
        assert!(d.text.contains("mixer_generate_image"));
        assert!(!d.text.contains("不该出现"));

        // 无成功结果 ⇒ None（调用方退回静态文案），与文本版 is_empty 语义一致。
        assert!(assemble_degraded_answer(&[], "zh-CN", FailureKind::Exhausted).is_none());
    }

    #[test]
    fn wire_kind_is_stable_per_failure_kind() {
        // 前端按这些字符串分支，改动即破坏契约。
        assert_eq!(FailureKind::Exhausted.wire_kind(), "rate_limited");
        assert_eq!(FailureKind::ContextOverflow.wire_kind(), "context_overflow");
        assert_eq!(FailureKind::ContentModeration.wire_kind(), "moderation");
        assert_eq!(FailureKind::Empty.wire_kind(), "empty_response");
        assert_eq!(FailureKind::Other.wire_kind(), "unknown");
    }

    #[test]
    fn text_and_structured_stay_in_sync() {
        // 文本版是结构化版的投影，二者不能漂移（文本版仍被旧路径调用）。
        let records = vec![rec("t", ToolCallStatus::Success, Some("preview"))];
        for kind in [
            FailureKind::Exhausted,
            FailureKind::ContextOverflow,
            FailureKind::ContentModeration,
            FailureKind::Empty,
            FailureKind::Other,
        ] {
            let text = assemble_results_from_tool_records(&records, "zh-CN", kind);
            let structured = assemble_degraded_answer(&records, "zh-CN", kind).expect("some");
            assert_eq!(text, structured.text, "kind={kind:?}");
        }
    }

    #[test]
    fn assemble_caps_and_clips_to_avoid_wall_of_text() {
        // 单条超长 + 多行只保留首行裁剪;>8 条折叠为计数 —— 不再刷几百行。
        let long = "x".repeat(5000);
        let mut records: Vec<ToolCallRecord> = (0..12)
            .map(|i| {
                rec(
                    "read",
                    ToolCallStatus::Success,
                    Some(Box::leak(format!("行{i}首行\n{long}").into_boxed_str())),
                )
            })
            .collect();
        records.push(rec(
            "read",
            ToolCallStatus::Success,
            Some(Box::leak(long.clone().into_boxed_str())),
        ));
        let out = assemble_results_from_tool_records(&records, "en", FailureKind::Other);
        let lines = out.lines().count();
        assert!(
            lines < 20,
            "degrade message must stay compact, got {lines} lines"
        );
        assert!(!out.contains(&long), "must not dump the full long preview");
        assert!(
            out.contains("more above"),
            "overflow tools should be folded into a count"
        );
    }
}
