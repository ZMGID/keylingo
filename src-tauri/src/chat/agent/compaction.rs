use serde_json::{json, Value};

use crate::chat::model_metadata::{chat_max_output_tokens_for_model, context_window_for_model};
use crate::chat::types::{
    ChatMessage, CompactionBoundaryRecord, Conversation, ConversationContextSummary,
};
use crate::settings::Settings;
use crate::state::AppState;

use super::loop_::{LoopEnv, RunState};
use super::planning::call_chat_completion_message_streamed;
use super::prepare::{estimate_tokens, estimate_value_tokens, IMAGE_PART_TYPES, TEXT_PART_TYPES};
use crate::chat::model::{MessagePart, ModelMessage};

/// 近期窗口（tokens）：从尾部往前累积整条消息，~该预算内的为受保护近期窗口、原样保留，
/// 其余旧段才进摘要。取代旧的固定 `KEEP_RECENT_RAW_MESSAGES` 条数（R7）。对齐 Codex
/// `COMPACT_USER_MESSAGE_MAX_TOKENS ≈ 20_000` 量级（含 user+assistant 整条）。
pub(crate) const RECENT_KEEP_TOKENS: usize = 20_000;
/// 估算占用超过**裸**窗口的该比例才触发自动压缩。对齐 Codex `AUTO_COMPACT_RATIO = 0.90`
/// （用裸窗口而非 safe_window 折扣，统一落盘 / L2 / 手动三处触发基准）。
pub(crate) const AUTO_COMPACT_RATIO: f32 = 0.90;
/// 摘要内容字符数下限（质量兜底）：低于此值视为烂摘要，拒绝覆盖旧 summary。
/// 修复「收到 ✅」式过短摘要污染落盘 context_state.summary 的问题。
const MIN_SUMMARY_CHARS: usize = 200;
/// 链式重摘衰减告警阈值：累计压缩次数达到此值后，给用户一条 `context_state.warning`
/// 提示多次压缩可能降低准确性（对齐 Codex 的 WarningEvent）。摘要是有损的，反复 summary-of-summary
/// 会累积漂移/细节流失。
const DECAY_WARNING_COMPRESSION_COUNT: usize = 3;
/// 序列化喂给摘要模型时，单条 `[Tool result]` / `[Tool error]` 的字符上限（R5）。
/// 仅截工具输出——用户/助手/推理/工具入参全文保留。对齐 OpenCode `TOOL_OUTPUT_MAX_CHARS = 2_000`。
const TOOL_OUTPUT_SUMMARY_MAX_CHARS: usize = 2_000;
/// Microcompact（R-1）降级旧工具结果时替换成的短标记。触发压缩时先把 old_segment 里的工具结果
/// 换成此标记、重估预算；够了就跳过昂贵的 LLM 摘要（对齐 Claude Code 的 microcompact）。
const MICROCOMPACT_TOOL_MARKER: &str = "[earlier tool result omitted to save context]";
/// 摘要调用允许产生的最大输出 token 数（R9）。Claude Code 9 段 prompt 先吐 `<analysis>`
/// 再吐 `<summary>`，真实大旧段时二者合计易超 4096 被截——提到 8192 以容纳完整产出
/// （仍远小于窗口，安全）。`summary_output_tokens` 保持 `min()`：真实上限更小的模型不受影响。
const SUMMARY_OUTPUT_TOKENS: u32 = 8_192;

/// 摘要**输入** token 预算占窗口的比例（R1）。摘要请求自身**绝不能**超窗——否则就是
/// "用超窗的请求去救超窗"，每次压缩都失败、降级返回原始视图，最终主调用仍超窗报错。
/// 取保守的 0.5 而非更高比例的理由：
/// - 模型窗口元数据常偏乐观（实测标称 128k 的模型真实可用 ~100k）；
/// - Responses API 等有额外 token 计数开销（工具 schema、system、role 标注）；
/// - 近期窗口本就在摘要请求之外逐字保留（`RECENT_KEEP_TOKENS` ≈ 20k），预算只需覆盖旧段摘要本身。
///
/// 0.5×window 给足余量，保证摘要请求恒定放得进窗口。
const SUMMARY_INPUT_BUDGET_RATIO: f32 = 0.5;

/// 当窗口未知（`window == 0`）时，摘要输入预算的兜底值（tokens）。
/// 取一个保守常见窗口的一半，既不跳过封顶又不会过度裁剪。
const SUMMARY_INPUT_BUDGET_FALLBACK_TOKENS: usize = 64_000;

/// 头尾裁剪时插入到中段的省略标记——告知摘要模型此处有更早历史被省略以放进摘要请求。
const HEAD_TAIL_OMISSION_MARKER: &str =
    "\n\n[... older history omitted to fit the summary request ...]\n\n";

/// 头尾裁剪保留预算偏向尾部的比例：头 ~40% / 尾 ~60%。近期工作（尾部）比早期意图更关键，
/// 但开头的任务目标/早期意图也需保留，故仍给头部留 ~40%。
const HEAD_BUDGET_FRACTION: f32 = 0.4;

/// 由 `replace_with_summary` 插入的摘要锚点前缀；anchored 链式摘要（R8）据此识别历史里已存在的
/// 上一份摘要，把它作为 `previous_summary` 让模型合并更新，而非从头再摘。
const SUMMARY_MARKER_PREFIX: &str = "[context summary]";

/// `build_chat_api_messages`（commands/context.rs）从落盘 `context_state.summary` 注入的
/// **system** 消息前缀。生成方（`commands/context.rs::summary_message`）与识别方
/// （`extract_previous_summary`）**共用同一常量**，防止格式漂移——历史上二者不一致
/// 曾导致 L2 压缩认不出注入的旧摘要，链式摘要退化为「只摘本轮 old_segment」，
/// run 结束整体覆盖旧 summary，跨轮静默丢掉早期上下文。
pub(crate) const PERSISTED_SUMMARY_PREFIX: &str = "Previous conversation summary:";

/// `replace_with_summary` 插入的摘要锚点后紧跟的固定 assistant ack 文案。抽成常量供
/// 插入方与 `summarize_history` 的 head 剔除方共用——否则链式重摘时上一轮的 ack 会作为
/// `[Assistant]:` 噪声行进入摘要输入。
const SUMMARY_ACK_TEXT: &str = "已了解早前对话的摘要，继续当前任务。";

/// 摘要模型调用的 system prompt（逐字对齐 pi `SUMMARIZATION_SYSTEM_PROMPT`，
/// coding-agent/src/core/compaction/utils.ts）。「不要续写对话」的约束配合
/// `<conversation>` 标签包裹，是 pi 防止摘要模型把历史当对话接着聊的两道锁。
const SUMMARY_SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.

Do NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";

/// 首次摘要 prompt（逐字对齐 pi `SUMMARIZATION_PROMPT`）：固定分节的「上下文检查点」。
/// 取代早前的 Claude Code 9 段 `<analysis>`/`<summary>` 双段 prompt——pi 的分节格式为
/// **增量更新**而设计（`UPDATE_SUMMARIZATION_PROMPT` 按节合并、In Progress→Done 迁移），
/// 链式压缩不再是「摘要的摘要」而是同一份检查点的持续演进，衰减显著更慢；且无需
/// analysis 前言，输出预算全花在正文上。
pub(crate) const SUMMARIZATION_PROMPT: &str = "The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or \"(none)\" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Any data, examples, or references needed to continue]
- [Or \"(none)\" if not applicable]

Keep each section concise. Preserve exact file paths, function names, and error messages.";

/// 链式压缩的**更新**prompt（逐字对齐 pi `UPDATE_SUMMARIZATION_PROMPT`）：存在上一份摘要时
/// 用它替代首次 prompt——PRESERVE 全部旧信息、按节合并新进展、In Progress→Done 迁移。
pub(crate) const UPDATE_SUMMARIZATION_PROMPT: &str = "The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.

Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from \"In Progress\" to \"Done\" when completed
- UPDATE \"Next Steps\" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it

Use this EXACT format:

## Goal
[Preserve existing goals, add new ones if the task expanded]

## Constraints & Preferences
- [Preserve existing, add new ones discovered]

## Progress
### Done
- [x] [Include previously done items AND newly completed items]

### In Progress
- [ ] [Current work - update based on progress]

### Blocked
- [Current blockers - remove if resolved]

## Key Decisions
- **[Decision]**: [Brief rationale] (preserve all previous, add new)

## Next Steps
1. [Update based on current state]

## Critical Context
- [Preserve important context, add new if needed]

Keep each section concise. Preserve exact file paths, function names, and error messages.";

/// split-turn 前缀摘要 prompt（逐字对齐 pi `TURN_PREFIX_SUMMARIZATION_PROMPT`）：token 尾窗
/// 把一轮从中间切开时，被裁掉的**本轮前半**用它单独摘要，与历史摘要拼接（历史摘要看不见
/// 这半轮的细节，直接丢会让模型不知道保留的后半是在干什么）。
pub(crate) const TURN_PREFIX_SUMMARIZATION_PROMPT: &str =
    "This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained.

Summarize the prefix to provide context for the retained suffix:

## Original Request
[What did the user ask for in this turn?]

## Early Progress
- [Key decisions and work done in the prefix]

## Context for Suffix
- [Information needed to understand the retained recent work]

Be concise. Focus on what's needed to understand the kept suffix.";

/// 估算消息序列的 token 数：逐条把 content 字符串（以及非字符串 content / tool_calls
/// 等结构化字段的 JSON 序列化）喂给 chars 启发式累加。
pub(crate) fn estimate_messages_tokens(messages: &[Value]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

/// 单条消息的 token 估算（与 `estimate_messages_tokens` 的逐条逻辑一致，供近期窗口选取复用）。
/// content 为多模态数组时走 `estimate_value_tokens`（图片记 0，不把 base64 体积算进 token）；
/// reasoning_content 计入（与 `serialize_message` 口径一致）。
fn estimate_message_tokens(message: &Value) -> usize {
    let tool_calls = message
        .get("tool_calls")
        .map(|calls| estimate_tokens(&calls.to_string()))
        .unwrap_or(0);
    let reasoning = message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .map(estimate_tokens)
        .unwrap_or(0);
    let content = match message.get("content") {
        Some(Value::String(text)) => estimate_tokens(text),
        Some(other) => estimate_value_tokens(other),
        None => 0,
    };
    content + tool_calls + reasoning + 4
}

/// 图片部件在摘要序列化中的占位符（不灌 base64）。
const IMAGE_PART_PLACEHOLDER: &str = "[image attachment omitted]";

/// 重复图片被去重后留在原位的占位文本。
const DUPLICATE_IMAGE_PLACEHOLDER: &str = "[与后文同一张图片，此处省略，不重复上传]";

/// 发送视图里所有图片 base64 的总字节预算。超出后从**最旧**的图片开始换占位符。
///
/// 4MB base64 ≈ 3MB 原始字节，够放四五张全分辨率截图；再多的历史图片对当前一步几乎
/// 没有价值，却每轮都要重传。参照 Codex 的实测事故：图片 base64 反复重放把请求打到
/// 8.34MB，多家 OpenAI 兼容中转直接 502/524 或返回空流。
///
/// ponytail: 固定常量，不做设置项。真有人需要不同额度再提成 `chat_tools` 配置。
const IMAGE_BYTES_BUDGET: usize = 4 * 1024 * 1024;

/// 收敛发送视图里的图片体积：**倒序**（新→旧）遍历，重复的图片只留最新那份，
/// 累计 base64 字节超过 [`IMAGE_BYTES_BUDGET`] 后把更早的图片换成占位文本。
/// 返回省下的字节数。
///
/// **为什么非做不可**：`read` 读到图片会把整张 base64 作为 follow-up user 消息永久留在
/// 历史里（`vision.rs::read_image_as_tool_result`）。同一张图被读两次就是两份 84KB
/// base64——实测占了请求体的 74%，而且每个 planning 轮都完整重传一次，几十轮下来几 MB，
/// 第三方中转直接在传输途中把流掐断。
///
/// 而 `estimate_message_tokens` 是**故意**不计图片 base64 的（token 口径上一张图约
/// 千把 token，不是 8 万字符），所以压缩层根本看不见这份体积，不会触发。字节层面的
/// 重复与堆积只能在这里单独处理。
///
/// 倒序遍历一次同时表达了两条规则：重复图留最新那份（模型当下在看的就是它），
/// 超预算时淘汰最旧的（对当前一步价值最低）。对齐 Claude Code 用户在要的
/// 「按年龄丢图」与 Strands Agents 的「图片换带元信息占位符」。
///
/// ponytail: 指纹直接用 part 的 JSON 串哈希——同一张图序列化必然逐字节相同，
/// 不用解 data URL、不用管 `image_url` / `input_image` / `image` 三种形状的差异。
fn prune_image_parts(messages: &mut [Value], budget: usize) -> usize {
    use std::collections::HashSet;
    use std::hash::{DefaultHasher, Hash, Hasher};

    let mut seen: HashSet<u64> = HashSet::new();
    let mut kept_bytes = 0usize;
    let mut saved = 0usize;
    for message in messages.iter_mut().rev() {
        let Some(parts) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        for part in parts.iter_mut() {
            let Some(kind) = part.get("type").and_then(Value::as_str) else {
                continue;
            };
            if !IMAGE_PART_TYPES.contains(&kind) {
                continue;
            }
            let serialized = part.to_string();
            let bytes = serialized.len();
            let mut hasher = DefaultHasher::new();
            serialized.hash(&mut hasher);
            if !seen.insert(hasher.finish()) {
                saved += bytes;
                *part = json!({ "type": "text", "text": DUPLICATE_IMAGE_PLACEHOLDER });
                continue;
            }
            if kept_bytes.saturating_add(bytes) > budget {
                saved += bytes;
                *part = json!({
                    "type": "text",
                    "text": format!(
                        "[较早的图片已从上下文移除以控制请求体积（约 {}KB）。需要时请重新读取该文件。]",
                        bytes / 1024
                    ),
                });
                continue;
            }
            kept_bytes += bytes;
        }
    }
    saved
}

/// 把多模态 content（数组 parts / 单对象）渲染成摘要文本：文本部件取全文、图片部件换占位符、
/// 未知部件退回其 JSON（保守不丢信息）。
fn render_multimodal_content(content: &Value) -> String {
    match content {
        Value::Array(parts) => parts
            .iter()
            .map(render_content_part)
            .collect::<Vec<_>>()
            .join(" "),
        other => render_content_part(other),
    }
}

fn render_content_part(part: &Value) -> String {
    if let Some(kind) = part.get("type").and_then(Value::as_str) {
        if IMAGE_PART_TYPES.contains(&kind) {
            return IMAGE_PART_PLACEHOLDER.to_string();
        }
        if TEXT_PART_TYPES.contains(&kind) {
            return part
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
        }
    }
    part.to_string()
}

/// 把单条消息渲染成角色标注文本行（R5）。用户/助手/推理/工具入参**全文保留**；仅
/// `[Tool result]` / `[Tool error]` 的内容截到 `TOOL_OUTPUT_SUMMARY_MAX_CHARS`（尾部加 `[truncated]`）。
/// 一条 assistant 消息可能同时带文本 + reasoning + 多个 tool_calls，全部展开为多行。
fn serialize_message(message: &Value) -> String {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut lines: Vec<String> = Vec::new();

    match role {
        "system" => {
            if let Some(text) = message.get("content").and_then(Value::as_str) {
                if !text.trim().is_empty() {
                    lines.push(format!("[System]: {text}"));
                }
            }
        }
        "user" => {
            if let Some(text) = message.get("content").and_then(Value::as_str) {
                lines.push(format!("[User]: {text}"));
            } else if let Some(content) = message.get("content") {
                // 非字符串 content（多模态 parts）：文本部件取全文、图片部件换占位符，
                // 不把 base64 灌进摘要输入（既污染摘要又打爆 token）。
                lines.push(format!("[User]: {}", render_multimodal_content(content)));
            }
        }
        "assistant" => {
            if let Some(text) = message.get("content").and_then(Value::as_str) {
                if !text.trim().is_empty() {
                    lines.push(format!("[Assistant]: {text}"));
                }
            } else if let Some(content) = message.get("content") {
                // 非字符串 content（罕见的多模态 assistant）：渲染文本 + 图片占位，不整段丢弃、不泄漏 base64。
                let rendered = render_multimodal_content(content);
                if !rendered.trim().is_empty() {
                    lines.push(format!("[Assistant]: {rendered}"));
                }
            }
            if let Some(reasoning) = message.get("reasoning_content").and_then(Value::as_str) {
                if !reasoning.trim().is_empty() {
                    lines.push(format!("[Assistant reasoning]: {reasoning}"));
                }
            }
            if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    let function = call.get("function");
                    let name = function
                        .and_then(|f| f.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    let args = function
                        .and_then(|f| f.get("arguments"))
                        .map(|a| match a.as_str() {
                            Some(s) => s.to_string(),
                            None => a.to_string(),
                        })
                        .unwrap_or_default();
                    // 工具入参全文保留（不截断）。
                    lines.push(format!("[Assistant tool call]: {name}({args})"));
                }
            }
        }
        "tool" => {
            let content = match message.get("content").and_then(Value::as_str) {
                Some(text) => text.to_string(),
                // 非字符串 content（多模态工具结果，如 MCP 返回的图片块）：走 render_multimodal_content
                // 而非 to_string()，避免 base64 泄漏进摘要输入（与 user 分支同处理）。
                None => match message.get("content") {
                    Some(content) => render_multimodal_content(content),
                    None => String::new(),
                },
            };
            let is_error = message
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let clipped = clip_tool_output(&content);
            if is_error {
                lines.push(format!("[Tool error]: {clipped}"));
            } else {
                lines.push(format!("[Tool result]: {clipped}"));
            }
        }
        other => {
            // 未知角色：退回 JSON，保证不丢信息（极罕见）。
            lines.push(format!("[{other}]: {message}"));
        }
    }

    lines.join("\n")
}

/// `[Tool result]` / `[Tool error]` 的内容截断到 `TOOL_OUTPUT_SUMMARY_MAX_CHARS`，
/// 超出时尾部加 `[truncated]` 标记（复用现有 head-tail 风格的 `truncate_chars`）。
fn clip_tool_output(content: &str) -> String {
    if content.chars().count() <= TOOL_OUTPUT_SUMMARY_MAX_CHARS {
        return content.to_string();
    }
    let head: String = content
        .chars()
        .take(TOOL_OUTPUT_SUMMARY_MAX_CHARS)
        .collect();
    format!("{head}\n[truncated]")
}

/// 把旧段消息序列化成喂给摘要模型的角色标注文本（R5）。每条消息一段，用空行分隔。
fn serialize_for_summary(messages: &[Value]) -> String {
    messages
        .iter()
        .map(serialize_message)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 把字符串按字符数前缀截取（不切多字节字符）。
fn take_chars(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// 把字符串按字符数后缀截取（保留末尾 n 个字符）。
fn take_chars_tail(text: &str, n: usize) -> String {
    let total = text.chars().count();
    if n >= total {
        return text.to_string();
    }
    text.chars().skip(total - n).collect()
}

/// 把序列化后的旧段文本头尾裁剪到 `budget_tokens`（R2）：保留开头（任务目标/早期意图）+
/// 结尾（最近工作），中间替换为 `HEAD_TAIL_OMISSION_MARKER`。偏向保留更多尾部（头 ~40% / 尾 ~60%）。
/// 未超预算则原样返回（R5，零行为变化）。
///
/// 在 token 预算上工作，但裁剪以字符为粒度：用 `estimate_tokens` 的 ASCII≈4 chars/token
/// 启发式把 token 预算换算成字符预算（保守按 4 倍），裁剪后再用 `estimate_tokens` 校验，
/// 若仍超预算则迭代收紧——返回结果 `estimate_tokens <= budget_tokens`（R2 兜底），
/// 唯一例外是 `budget_tokens` 小于省略标记自身 token 数的不可达极端（`char_budget <= 1` 退出，
/// 保留头尾各 <1 字符 + 标记）；实际 `head_budget` 有 `summary_input_budget/4` 下限，触达不到。
fn clip_serialized_to_budget(serialized: &str, budget_tokens: usize) -> String {
    if estimate_tokens(serialized) <= budget_tokens {
        return serialized.to_string();
    }
    // 给省略标记留出 token 预算。
    let marker_tokens = estimate_tokens(HEAD_TAIL_OMISSION_MARKER);
    let content_budget = budget_tokens.saturating_sub(marker_tokens);

    // token 预算换算成字符预算：ASCII 约 4 chars/token，按 4 倍作为初始字符预算上限。
    let mut char_budget = content_budget.saturating_mul(4).max(1);

    // 迭代收紧：头尾按比例切，校验 estimate_tokens，超了就收紧字符预算重切。
    loop {
        let head_chars = ((char_budget as f32) * HEAD_BUDGET_FRACTION) as usize;
        let tail_chars = char_budget.saturating_sub(head_chars);
        let head = take_chars(serialized, head_chars);
        let tail = take_chars_tail(serialized, tail_chars);
        let clipped = format!("{head}{HEAD_TAIL_OMISSION_MARKER}{tail}");
        if estimate_tokens(&clipped) <= budget_tokens || char_budget <= 1 {
            return clipped;
        }
        // 仍超预算（多字节字符占 1 token/char 时换算偏乐观）——收紧字符预算重试。
        char_budget = char_budget * 3 / 4;
    }
}

/// 一条 assistant 消息是否携带 tool_calls（其后的 role=="tool" 结果不能与它拆到摘要/保留两侧）。
#[cfg(test)]
fn has_tool_calls(message: &Value) -> bool {
    message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| !calls.is_empty())
        .unwrap_or(false)
}

/// 一条消息是否为 tool 结果（role=="tool"）。
fn is_tool_result(message: &Value) -> bool {
    message.get("role").and_then(Value::as_str) == Some("tool")
}

/// 按 token 选取受保护的近期窗口（R7）：在系统前缀之后的消息里，从尾部往前累积**整条**消息的
/// `estimate_message_tokens` 直到 ~`keep_tokens`，这些为原样保留的近期窗口；更早的为旧段、进摘要。
///
/// 约束：
/// - **不切断单条消息**（保 JSON 合法）——按整条累积，越过预算的那条整体归入旧段（除非配对保护）。
/// - **不拆 tool_call↔tool 配对**——若边界落在一条 assistant(tool_calls) 与其后的 tool 结果之间，
///   把成对的一组整体拉进近期窗口（往前移动边界，使旧段不以孤立的 tool 结果开头）。
///
/// 返回 `(system_prefix, old_segment, recent)`：系统前缀 = 开头连续 role=="system"；
/// old_segment = 系统前缀之后、近期窗口之前的旧段；recent = 受保护近期窗口。
fn select_recent_by_tokens(
    messages: &[Value],
    keep_tokens: usize,
) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let system_end = messages
        .iter()
        .position(|m| m.get("role").and_then(Value::as_str) != Some("system"))
        .unwrap_or(messages.len());

    // 从尾部往前累积整条消息，直到超过 keep_tokens。`split` 是近期窗口的起始下标（含）。
    let mut total = 0usize;
    let mut split = messages.len();
    let mut idx = messages.len();
    while idx > system_end {
        idx -= 1;
        let next = total + estimate_message_tokens(&messages[idx]);
        if next > keep_tokens && idx + 1 < messages.len() {
            // 越过预算：保留 [idx+1..] 为近期窗口（不切断当前条，当前条归旧段）。
            split = idx + 1;
            break;
        }
        total = next;
        split = idx;
    }

    // 配对保护：若近期窗口以孤立的 tool 结果开头（其 assistant(tool_calls) 落在旧段尾），
    // 把边界往前移，使整组 tool_call↔tool 一起进近期窗口（不拆配对）。
    while split > system_end && is_tool_result(&messages[split]) {
        split -= 1;
    }
    // split 现在指向一条 assistant(tool_calls) 或一条普通消息；若它是 assistant(tool_calls)，
    // 它已被包含进近期窗口，其后续 tool 结果也都在窗口内——配对完整。

    (
        messages[..system_end].to_vec(),
        messages[system_end..split].to_vec(),
        messages[split..].to_vec(),
    )
}

/// `extract_previous_summary` 的返回：上一份摘要正文 + 它的来源，供 `summarize_history`
/// 决定如何从压缩后视图里剔除它（锚点在 old_segment→head，注入在 system 前缀→prefix）。
struct PreviousSummary {
    text: String,
    from_injected: bool,
}

/// 探测上一份摘要（anchored 链式摘要，R8），供作为 `previous_summary` 让模型合并更新。
/// 两种形态，**锚点优先**（它是同一 run 内更晚的 L2 产物，已含合并结果）：
/// - **锚点摘要**：`replace_with_summary` 插入 old_segment 的一条 content 以
///   `SUMMARY_MARKER_PREFIX` 开头的 **user** 消息；
/// - **注入摘要**：`build_chat_api_messages` 从落盘 summary 注入 system 前缀的一条 content 以
///   `PERSISTED_SUMMARY_PREFIX` 开头的 **system** 消息。
///
/// 无锚点时才回退到注入摘要——否则同 run 第二次 L2 压缩会回退到已过期的落盘 S1。
fn extract_previous_summary(
    system_prefix: &[Value],
    old_segment: &[Value],
) -> Option<PreviousSummary> {
    // 锚点优先：old_segment 里的 user + SUMMARY_MARKER_PREFIX。
    let anchor = old_segment.iter().find_map(|message| {
        if message.get("role").and_then(Value::as_str) != Some("user") {
            return None;
        }
        let content = message.get("content").and_then(Value::as_str)?;
        let trimmed = content.trim_start();
        if !trimmed.starts_with(SUMMARY_MARKER_PREFIX) {
            return None;
        }
        Some(summary_body_after_first_line(trimmed))
    });
    if let Some(text) = anchor {
        return Some(PreviousSummary {
            text,
            from_injected: false,
        });
    }

    // 回退：system 前缀里的 system + PERSISTED_SUMMARY_PREFIX（注入摘要）。
    system_prefix.iter().find_map(|message| {
        if message.get("role").and_then(Value::as_str) != Some("system") {
            return None;
        }
        let content = message.get("content").and_then(Value::as_str)?;
        let trimmed = content.trim_start();
        if !trimmed.starts_with(PERSISTED_SUMMARY_PREFIX) {
            return None;
        }
        Some(PreviousSummary {
            text: summary_body_after_first_line(trimmed),
            from_injected: true,
        })
    })
}

/// 取摘要正文：摘要消息形如 "<前缀引导语>：\n<summary>"。找第一个换行后的内容；
/// 无换行则退回整条 trim。
fn summary_body_after_first_line(trimmed: &str) -> String {
    trimmed
        .split_once('\n')
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

/// 是否为 `replace_with_summary` 插入的锚点摘要（user + `SUMMARY_MARKER_PREFIX`）。
fn is_anchor_summary(message: &Value) -> bool {
    message.get("role").and_then(Value::as_str) == Some("user")
        && message
            .get("content")
            .and_then(Value::as_str)
            .map(|c| c.trim_start().starts_with(SUMMARY_MARKER_PREFIX))
            .unwrap_or(false)
}

/// 是否为锚点摘要配对的固定 ack（assistant + `SUMMARY_ACK_TEXT`）。
fn is_summary_ack(message: &Value) -> bool {
    message.get("role").and_then(Value::as_str) == Some("assistant")
        && message
            .get("content")
            .and_then(Value::as_str)
            .map(str::trim)
            == Some(SUMMARY_ACK_TEXT)
}

/// 是否为 `build_chat_api_messages` 注入的落盘摘要（system + `PERSISTED_SUMMARY_PREFIX`）。
fn is_injected_summary(message: &Value) -> bool {
    message.get("role").and_then(Value::as_str) == Some("system")
        && message
            .get("content")
            .and_then(Value::as_str)
            .map(|c| c.trim_start().starts_with(PERSISTED_SUMMARY_PREFIX))
            .unwrap_or(false)
}

/// 用摘要替换旧段，返回新的消息序列：系统前缀 + summary(user)/ack(assistant) 对 + 尾段。
/// user/assistant 成对插入保证 role 交替对严格 provider 合法。摘要 user 消息以
/// `SUMMARY_MARKER_PREFIX` 开头，供后续轮的 anchored 链式摘要识别。
fn replace_with_summary(
    system_prefix: Vec<Value>,
    summary: &str,
    recent: Vec<Value>,
) -> Vec<Value> {
    let mut out = system_prefix;
    out.push(json!({
        "role": "user",
        "content": format!(
            "{SUMMARY_MARKER_PREFIX} 以下是本次任务早前对话的压缩摘要（原始消息已省略以节省上下文）：\n{summary}"
        ),
    }));
    out.push(json!({
        "role": "assistant",
        "content": SUMMARY_ACK_TEXT,
    }));
    out.extend(recent);
    out
}

/// Microcompact 增量降级（R-1）：触发压缩时、发起 LLM 摘要**之前**的轻量兜底。
/// 把 old_segment（近期 `keep_tokens` 尾窗**之前**的段）里的 `role=="tool"` 结果内容换成
/// `MICROCOMPACT_TOOL_MARKER`，组回完整视图并重估 token。
///
/// **仅当降级足以把整体压回 `budget` 内才返回 `Some(view)`（= 可跳过昂贵摘要）**；否则 `None`，
/// 调用方落到既有 LLM 摘要路径。近期尾窗原样保留（近期工具结果不动）；不拆 tool_call↔tool 配对
/// （复用 `select_recent_by_tokens` 的切分与配对保护）；已是标记的工具结果不再重复降级（幂等）。
fn microcompact_send_view(
    messages: &[Value],
    keep_tokens: usize,
    budget: usize,
) -> Option<Vec<Value>> {
    let (system_prefix, old_segment, recent) = select_recent_by_tokens(messages, keep_tokens);
    if old_segment.is_empty() {
        return None;
    }
    let mut degraded_any = false;
    let degraded_old: Vec<Value> = old_segment
        .into_iter()
        .map(|message| {
            if !is_tool_result(&message) {
                return message;
            }
            let already_marker =
                message.get("content").and_then(Value::as_str) == Some(MICROCOMPACT_TOOL_MARKER);
            if already_marker {
                return message;
            }
            let mut degraded = message;
            if let Some(obj) = degraded.as_object_mut() {
                obj.insert(
                    "content".to_string(),
                    Value::String(MICROCOMPACT_TOOL_MARKER.to_string()),
                );
                degraded_any = true;
            }
            degraded
        })
        .collect();
    if !degraded_any {
        // old_segment 里没有可降级的工具结果——microcompact 无能为力，交给摘要。
        return None;
    }
    let mut view = system_prefix;
    view.extend(degraded_old);
    view.extend(recent);
    if estimate_messages_tokens(&view) <= budget {
        Some(view)
    } else {
        None
    }
}

/// 摘要调用的种类：决定用哪份 prompt 与输出预算。
#[derive(Clone, Copy, PartialEq, Eq)]
enum SummaryKind {
    /// 历史摘要（首次 / 链式更新，按 `previous_summary` 有无自动选 prompt）。
    History,
    /// split-turn 前缀摘要（更小的输出预算，无 previous/focus）。
    TurnPrefix,
}

/// 构造摘要请求的 user 指令体（对齐 pi `generateSummaryWithUsage` 的拼装顺序）：
/// 序列化历史包进 `<conversation>` 标签（防模型续写对话）→ `<previous-summary>`（链式更新时）
/// → 基底 prompt（有上一份摘要用 UPDATE、否则首次；TurnPrefix 用前缀 prompt）
/// → `focus`（手动 `/compact <focus>`）以 `Additional focus:` 追加。
fn build_summary_user_content(
    serialized_history: &str,
    previous_summary: Option<&str>,
    focus: Option<&str>,
    kind: SummaryKind,
) -> String {
    let mut prompt = format!("<conversation>\n{serialized_history}\n</conversation>\n\n");
    if kind == SummaryKind::TurnPrefix {
        prompt.push_str(TURN_PREFIX_SUMMARIZATION_PROMPT);
        return prompt;
    }
    if let Some(previous) = previous_summary {
        prompt.push_str(&format!(
            "<previous-summary>\n{previous}\n</previous-summary>\n\n"
        ));
    }
    let base = if previous_summary.is_some() {
        UPDATE_SUMMARIZATION_PROMPT
    } else {
        SUMMARIZATION_PROMPT
    };
    prompt.push_str(base);
    if let Some(focus) = focus {
        let focus = focus.trim();
        if !focus.is_empty() {
            prompt.push_str(&format!("\n\nAdditional focus: {focus}"));
        }
    }
    prompt
}

/// 从摘要模型的回复里抽取摘要正文：取 `<summary>...</summary>` 内文（如有），否则整体 trim。
fn extract_summary_text(response: &str) -> String {
    if let Some(start) = response.find("<summary>") {
        let after = &response[start + "<summary>".len()..];
        if let Some(end) = after.find("</summary>") {
            return after[..end].trim().to_string();
        }
        return after.trim().to_string();
    }
    response.trim().to_string()
}

/// 摘要调用的最大输出 token：`min(config.max_output_tokens, SUMMARY_OUTPUT_TOKENS)`（R9）。
fn summary_output_tokens(config_max: u32) -> u32 {
    config_max.min(SUMMARY_OUTPUT_TOKENS)
}

/// 把消息序列压缩成 system 前缀 + 摘要对 + 近期窗口（R5–R9 的共享核心）。
/// `focus` 为手动 `/compact <focus>` 透传的聚焦指令（自动路径为 None）。
/// 成功返回压缩后的完整消息序列；空摘要 / 失败 / 无可摘要旧段时返回 None（调用方据此降级）。
///
/// 自动路径（`maybe_compact_send_view`）走这里，避免重复摘要逻辑。
/// `keep_tokens`：受保护近期窗口大小——自动路径传 `min(RECENT_KEEP_TOKENS, budget)`（窗口比
/// `RECENT_KEEP_TOKENS` 还小的模型上，近期窗口不能大过压缩预算，否则压完仍超窗），手动路径传 `RECENT_KEEP_TOKENS`。
/// `window`：模型上下文窗口（tokens）——据此把**摘要请求自身的输入**封顶到
/// `window * SUMMARY_INPUT_BUDGET_RATIO`（R1/R2），保证摘要调用绝不超窗（"用超窗请求救超窗"的根因）。
/// `window == 0`（未知）时用 `SUMMARY_INPUT_BUDGET_FALLBACK_TOKENS` 兜底。
/// `cancel`：进行中取消的 future——自动路径传 host 的取消等待，手动路径传 `None`（强制压缩不取消）。
/// runtime 消息上的来源 UI 消息 id 标注（由 `commands/context.rs::build_chat_api_messages` 注入；
/// 一条 UI 消息展开出的多条 runtime 消息共享同一 id）。发给 provider 前该字段会被
/// `model_message_from_openai_message` 剥离，绝不进 wire 请求。
pub(crate) const UI_MESSAGE_ID_KEY: &str = "_ui_message_id";

fn ui_message_id_of(message: &Value) -> Option<&str> {
    message.get(UI_MESSAGE_ID_KEY).and_then(Value::as_str)
}

/// 把 runtime 切分映射回 UI 消息：返回「其 runtime 展开**完全**落入旧段」的最后一条
/// UI 消息 id。旧实现按 user|assistant 条数当 `ui_message_order` 下标推算，工具多轮
/// 展开/多答组剔除/摘要锚点都会错位（错位的 boundary 落盘后会静默丢上下文）；现改为
/// 读 `_ui_message_id` 标注精确映射。
///
/// 若旧段末尾的 UI 消息有展开条残留在近期窗口（横跨边界），回退到旧段内上一个不同的
/// 完整 id。旧段无任何带标注消息（只有摘要锚点/系统注入）→ None（调用方不落盘
/// boundary，运行时压缩视图照常生效）。
pub(crate) fn source_until_message_id_for_split(
    runtime_messages: &[Value],
    keep_tokens: usize,
) -> Option<String> {
    let (_system_prefix, old_segment, recent) =
        select_recent_by_tokens(runtime_messages, keep_tokens);
    if old_segment.is_empty() {
        return None;
    }
    // 近期窗口里出现过的 id：这些 UI 消息有展开条不在旧段里，不能作为 boundary。
    let ids_in_recent: std::collections::HashSet<&str> =
        recent.iter().filter_map(ui_message_id_of).collect();
    old_segment
        .iter()
        .rev()
        .filter_map(ui_message_id_of)
        .find(|id| !ids_in_recent.contains(id))
        .map(str::to_string)
}

/// 直接按 `MessagePart` 估算 `model_messages` 的 token 数，避免
/// `openai_messages_from_model_messages` 每次调用把整段工具转录克隆成 `Vec<Value>`
/// （`estimate_chat_message_tokens` 在 token 尾窗循环里逐条调用、且近满窗每轮都跑）。
/// 口径与 `estimate_message_tokens` 一致：文本/推理/工具入参/工具结果按字符估算，
/// 图片部件记 0，每条消息 +4 固定开销。
fn estimate_model_messages_tokens(messages: &[ModelMessage]) -> usize {
    messages
        .iter()
        .map(|message| {
            let parts: usize = message
                .content
                .iter()
                .map(|part| match part {
                    MessagePart::Text { text } | MessagePart::Reasoning { text } => {
                        estimate_tokens(text)
                    }
                    MessagePart::ToolCall {
                        name,
                        arguments_raw,
                        ..
                    } => estimate_tokens(name) + estimate_tokens(arguments_raw),
                    MessagePart::ToolResult { content, .. } => estimate_tokens(content),
                    // 图片部件记 0（与 estimate_value_tokens 同口径，不把 base64 算进 token）。
                    MessagePart::Image { .. } | MessagePart::ImageUrl { .. } => 0,
                })
                .sum();
            parts + 4
        })
        .sum()
}

/// 把单条 UI `ChatMessage` 估算成 token 数。**优先按展开形态**（model_messages /
/// api_messages）估算——真实 replay 发给模型的是这些里的完整工具转录，而非截断的
/// `result_preview`；分支顺序与 `build_chat_api_messages` 的展开路径同源对齐。
/// 无展开数据时退回 content + reasoning + 工具入参 + 结果预览口径。
pub(crate) fn estimate_chat_message_tokens(message: &ChatMessage) -> usize {
    if !message.model_messages.is_empty() {
        return estimate_model_messages_tokens(&message.model_messages);
    }
    if !message.api_messages.is_empty() {
        return message
            .api_messages
            .iter()
            .map(estimate_message_tokens)
            .sum();
    }
    let mut total = estimate_tokens(&message.content);
    if let Some(reasoning) = message.reasoning.as_deref() {
        total += estimate_tokens(reasoning);
    }
    for tool in &message.tool_calls {
        total += estimate_tokens(&tool.name);
        total += estimate_tokens(&tool.arguments);
        if let Some(preview) = tool.result_preview.as_deref() {
            total += estimate_tokens(preview);
        }
        if let Some(err) = tool.error.as_deref() {
            total += estimate_tokens(err);
        }
    }
    total + 4
}

/// 落盘路径的 token 切点：在 `[summary_start, len)` 区间内，从尾部往前累积整条
/// `ChatMessage` 的 `estimate_chat_message_tokens`，直到 ~`keep_tokens` 预算用尽。
/// 返回 old_segment 末尾下标（含）；越界或无旧段返回 None。
///
/// 与 L2 `select_recent_by_tokens` 同语义：不切断单条消息；越预算的那条整体归入旧段。
/// 内部委托 `token_split_over_indices`——落盘路径过滤多答排除臂后走同一切分核心。
/// 非过滤（连续区间）调用保留给直接单测 / commands.rs 测试用。
#[cfg(test)]
pub(crate) fn token_split_chat_messages(
    messages: &[ChatMessage],
    summary_start: usize,
    keep_tokens: usize,
) -> Option<usize> {
    if summary_start >= messages.len() {
        return None;
    }
    let indices: Vec<usize> = (summary_start..messages.len()).collect();
    token_split_over_indices(messages, &indices, keep_tokens)
}

/// token 切分核心：在**升序原始下标序列** `indices`（已按 summary_start 过滤、可选再排除
/// 多答臂）上，从尾部往前累积整条消息 token 到 ~`keep_tokens`。返回 old_segment 末尾的
/// **原始下标**（含）；全部落入近期窗口 / 空序列返回 None。
/// 落盘 `compact_conversation_inner` 与 `has_compressible_old_segment` 共用，防止口径分叉。
fn token_split_over_indices(
    messages: &[ChatMessage],
    indices: &[usize],
    keep_tokens: usize,
) -> Option<usize> {
    let n = indices.len();
    let mut total = 0usize;
    let mut split = n; // recent 起始（indices 内位置）；split-1 = old_segment 末尾
    let mut i = n;
    while i > 0 {
        i -= 1;
        let next = total + estimate_chat_message_tokens(&messages[indices[i]]);
        if next > keep_tokens && i + 1 < n {
            split = i + 1;
            break;
        }
        total = next;
        split = i;
    }
    if split == 0 {
        // 整段都进了近期窗口——没有可摘要旧段。
        return None;
    }
    Some(indices[split - 1])
}

/// 把 UI `ChatMessage` 序列化成喂给摘要模型的角色标注文本。对齐 L2 `serialize_message`：
/// user/assistant/reasoning/工具入参**全文保留**；`result_preview` / `error` 截到
/// `TOOL_OUTPUT_SUMMARY_MAX_CHARS`（尾部加 `[truncated]`）。这是修复「收到 ✅」式烂摘要的根因——
/// 旧落盘路径只发 UI 文本 + 500 字工具预览，工具入参完全丢失；现在与 L2 等价。
fn serialize_chat_message_for_summary(message: &ChatMessage) -> String {
    let role = if message.role == "assistant" {
        "assistant"
    } else {
        "user"
    };
    let mut lines: Vec<String> = Vec::new();

    let text = message.content.trim();
    if !text.is_empty() {
        lines.push(format!("[{}]: {text}", capitalize_role(role)));
    }
    if let Some(reasoning) = message.reasoning.as_deref() {
        if !reasoning.trim().is_empty() {
            lines.push(format!("[Assistant reasoning]: {reasoning}"));
        }
    }
    for tool in &message.tool_calls {
        // 工具入参全文保留（不截断）——让摘要模型能看到具体读了哪个文件 / 跑了什么命令。
        lines.push(format!(
            "[Assistant tool call]: {}({})",
            tool.name, tool.arguments
        ));
        let output = tool
            .result_preview
            .clone()
            .or_else(|| tool.error.clone())
            .unwrap_or_default();
        if !output.trim().is_empty() {
            let clipped = clip_tool_output(&output);
            if tool.error.is_some() {
                lines.push(format!("[Tool error]: {clipped}"));
            } else {
                lines.push(format!("[Tool result]: {clipped}"));
            }
        }
    }

    lines.join("\n")
}

fn capitalize_role(role: &str) -> &'static str {
    match role {
        "assistant" => "Assistant",
        "user" => "User",
        _ => "User",
    }
}

/// 把旧段 `ChatMessage` 序列化成角色标注文本（每条一段，空行分隔）。
/// 取 `&[&ChatMessage]`：落盘 old_segment 过滤多答排除臂后是非连续引用集合。
fn serialize_chat_messages_for_summary(messages: &[&ChatMessage]) -> String {
    messages
        .iter()
        .map(|m| serialize_chat_message_for_summary(m))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// `compact_with_summary_model` 的三态返回，区分**取消**与**失败**：取消是用户主动行为，
/// 不该被 anti-thrashing 计为「压缩未解决」（缺陷 5.4）。
enum CompactAttempt {
    Summary(String),
    /// 进行中被取消（仅 `cancel` future 触发时可达；force/落盘路径 cancel=None 时不可达）。
    Cancelled,
    /// 请求错误 / 空摘要 / 质量兜底拒绝——真正的失败。
    Failed,
}

/// 统一摘要调用核心（落盘 / L2 / 手动三处共用）：
/// 1. 把序列化后的旧段头尾裁剪到摘要**输入**预算（`window * SUMMARY_INPUT_BUDGET_RATIO`），
///    保证摘要请求自身绝不超窗（R1/R2）；`window == 0` 用兜底常量。
/// 2. 拼 Claude 9 段 prompt + anchored `<previous-summary>` + `focus`。
/// 3. **流式**调用压缩模型（`call_chat_completion_message_streamed`），抽 `<summary>` 正文。
/// 4. 质量兜底：空 / 过短 / 相对旧 summary 显著劣化 → `Failed`，**不覆盖**旧 summary。
///
/// 返回 `Summary(text)`（已 trim）/ `Cancelled` / `Failed`（调用方据此降级）。
#[allow(clippy::too_many_arguments)]
async fn compact_with_summary_model(
    state: &AppState,
    provider: &crate::settings::ModelProvider,
    model: &str,
    serialized_old_segment: &str,
    previous_summary: Option<&str>,
    focus: Option<&str>,
    kind: SummaryKind,
    window: usize,
    config_max_output_tokens: u32,
    retry_attempts: usize,
    conversation_id: &str,
    message_id: &str,
    cancel: Option<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>>>,
) -> CompactAttempt {
    // 摘要**输入**预算（R1）：window * ratio，未知窗口用兜底常量。
    let summary_input_budget = if window == 0 {
        SUMMARY_INPUT_BUDGET_FALLBACK_TOKENS
    } else {
        ((window as f32) * SUMMARY_INPUT_BUDGET_RATIO) as usize
    };
    // 为固定开销（system prompt + 基底 prompt + anchored previous_summary + focus）
    // 预留预算（R4：previous_summary 不被裁掉），剩余给序列化旧段 head。
    let base_prompt = match kind {
        SummaryKind::TurnPrefix => TURN_PREFIX_SUMMARIZATION_PROMPT,
        SummaryKind::History if previous_summary.is_some() => UPDATE_SUMMARIZATION_PROMPT,
        SummaryKind::History => SUMMARIZATION_PROMPT,
    };
    let fixed_overhead = estimate_tokens(SUMMARY_SYSTEM_PROMPT)
        + estimate_tokens(base_prompt)
        + previous_summary.map(estimate_tokens).unwrap_or(0)
        + focus.map(estimate_tokens).unwrap_or(0);
    // head 预算 = 总输入预算 - 固定开销；至少保留一点，避免开销吃光预算时退化为 0。
    let head_budget = summary_input_budget
        .saturating_sub(fixed_overhead)
        .max(summary_input_budget / 4);

    // 序列化旧段，超 head 预算时头尾裁剪（R2）；未超则原样（R5）。
    let serialized = clip_serialized_to_budget(serialized_old_segment, head_budget);
    let user_content = build_summary_user_content(&serialized, previous_summary, focus, kind);
    let summary_request = vec![
        json!({ "role": "system", "content": SUMMARY_SYSTEM_PROMPT }),
        json!({ "role": "user", "content": user_content }),
    ];

    // 前缀摘要预算减半（对齐 pi：turn prefix 用 0.5×，历史摘要 0.8×——它只需覆盖半轮）。
    let max_output = match kind {
        SummaryKind::History => summary_output_tokens(config_max_output_tokens),
        SummaryKind::TurnPrefix => summary_output_tokens(config_max_output_tokens).min(4_096),
    };
    // 流式调用：部分 provider（如 openai_responses 代理）只可靠服务流式，非流式会失败。
    let call = call_chat_completion_message_streamed(
        state,
        provider,
        model,
        summary_request,
        None,
        retry_attempts,
        false,
        max_output,
        conversation_id,
        message_id,
        "Chat context compaction",
    );

    let summary = match cancel {
        Some(cancel) => {
            tokio::select! {
                result = call => result,
                _ = cancel => {
                    // 取消进行中：放弃压缩，让后续 planning 自己检测取消并正常收尾。
                    // 区别于失败——不计入 anti-thrashing 未解决轮数。
                    return CompactAttempt::Cancelled;
                }
            }
        }
        None => call.await,
    };

    let raw = match summary {
        Ok(message) => super::stop::assistant_content_from_api_message(&message),
        Err(err) => {
            eprintln!("Chat context compaction failed: {err}; keeping raw view");
            return CompactAttempt::Failed;
        }
    };
    let text = extract_summary_text(&raw);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        eprintln!("Chat context compaction returned empty summary; keeping raw view");
        return CompactAttempt::Failed;
    }
    // 质量兜底（修复「收到 ✅」）：过短 / 相对旧 summary 显著劣化 → 拒绝覆盖。
    match summary_quality_guard(trimmed, previous_summary) {
        SummaryQuality::Ok => CompactAttempt::Summary(trimmed.to_string()),
        SummaryQuality::Truncated => {
            eprintln!(
                "Chat context compaction summary truncated (<analysis> without <summary>); rejecting"
            );
            CompactAttempt::Failed
        }
        SummaryQuality::TooShort => {
            eprintln!(
                "Chat context compaction summary too short ({} chars < {MIN_SUMMARY_CHARS}); rejecting",
                trimmed.chars().count()
            );
            CompactAttempt::Failed
        }
        SummaryQuality::Degraded => {
            eprintln!(
                "Chat context compaction summary degraded ({} < 30% of previous {}); keeping previous",
                trimmed.chars().count(),
                previous_summary.map(|p| p.trim().chars().count()).unwrap_or(0)
            );
            CompactAttempt::Failed
        }
    }
}

/// 摘要质量判定结果（`compact_with_summary_model` 的质量兜底）。
#[derive(Debug, PartialEq, Eq)]
enum SummaryQuality {
    Ok,
    TooShort,
    Truncated,
    Degraded,
}

/// 纯函数质量兜底：
/// - 截断（含 `<analysis>` 却无 `<summary>`）：Claude 9 段格式先吐 `<analysis>` 再吐 `<summary>`，
///   流被截在两者之间时 `extract_summary_text` 回退返回整段 analysis 前言，可能 >200 字骗过长度闸 →
///   拒绝（`Truncated`）。非 9 段格式的纯摘要不含 `<analysis>`，不受影响。
/// - 过短（< `MIN_SUMMARY_CHARS`）→ `TooShort`。
/// - 相对旧 summary 显著劣化（< 旧 summary 长度 30%，且旧 summary 本身达标）→ `Degraded`。
///
/// 抽成纯函数便于单元测试「截断拒绝」「过短拒绝」「劣化拒绝」「链式合并达标通过」等用例。
fn summary_quality_guard(trimmed: &str, previous_summary: Option<&str>) -> SummaryQuality {
    if trimmed.contains("<analysis>") && !trimmed.contains("<summary>") {
        return SummaryQuality::Truncated;
    }
    let trimmed_len = trimmed.chars().count();
    if trimmed_len < MIN_SUMMARY_CHARS {
        return SummaryQuality::TooShort;
    }
    if let Some(previous) = previous_summary {
        let prev_len = previous.trim().chars().count();
        // 仅当旧 summary 本身达标时才用「30%」门槛——避免用一份烂旧 summary 卡死新摘要。
        if prev_len >= MIN_SUMMARY_CHARS && trimmed_len * 10 < prev_len * 3 {
            return SummaryQuality::Degraded;
        }
    }
    SummaryQuality::Ok
}

/// 链式重摘衰减告警（R-4）：累计压缩次数达 `DECAY_WARNING_COMPRESSION_COUNT` 时返回一条英文
/// `context_state.warning`（沿用现有告警都是后端原始英文串的惯例），否则 None。压缩成功后由
/// 两条落盘路径（`compact_conversation` + L2 写回）调用，替代无条件的 `warning = None`。
pub(crate) fn decay_warning_for(compression_count: usize) -> Option<String> {
    if compression_count >= DECAY_WARNING_COMPRESSION_COUNT {
        Some(format!(
            "This conversation has been compressed {compression_count} times; repeated compression can reduce accuracy. Consider starting a new conversation."
        ))
    } else {
        None
    }
}

async fn summarize_history(
    state: &crate::state::AppState,
    provider: &crate::settings::ModelProvider,
    model: &str,
    messages: &[Value],
    keep_tokens: usize,
    window: usize,
    config_max_output_tokens: u32,
    retry_attempts: usize,
    conversation_id: &str,
    message_id: &str,
    focus: Option<&str>,
    cancel: Option<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>>>,
) -> CompactOutcome {
    let (system_prefix, old_segment, recent) = select_recent_by_tokens(messages, keep_tokens);
    if old_segment.is_empty() {
        // 没有可摘要的旧段（全在受保护近期窗口里）——压缩无能为力。
        return CompactOutcome::Failed;
    }

    // anchored 链式摘要（R8）：若历史含上一份摘要（old_segment 锚点 或 system 前缀注入），
    // 作为 previous_summary 合并更新，且不重复进摘要输入 / 压缩后视图。
    let previous_summary = extract_previous_summary(&system_prefix, &old_segment);

    // head：从 old_segment 剔除锚点摘要及其配对 ack（避免上一轮摘要/ack 作为噪声再进摘要输入）。
    let head: Vec<Value> = if previous_summary.is_some() {
        old_segment
            .iter()
            .filter(|m| !is_anchor_summary(m) && !is_summary_ack(m))
            .cloned()
            .collect()
    } else {
        old_segment.clone()
    };

    // 注入摘要来自 system 前缀——它已并入新摘要，从压缩后视图的 system 前缀里剔除，避免重复。
    let system_prefix: Vec<Value> = if previous_summary
        .as_ref()
        .map(|p| p.from_injected)
        .unwrap_or(false)
    {
        system_prefix
            .into_iter()
            .filter(|m| !is_injected_summary(m))
            .collect()
    } else {
        system_prefix
    };

    // split-turn 检测（对齐 pi `findCutPoint` 的 `isSplitTurn`）：近期窗口不以 user 消息
    // 开头（`select_recent_by_tokens` 已保证不以 tool 结果开头，此时只能是 assistant）⇒
    // token 切点落在一轮中间。head 尾部的「本轮前半」（该轮起始 user 消息到切点）不进
    // 历史摘要——历史 prompt 关注任务级进展，会把这半轮的操作细节抹平，保留的后半就
    // 成了没头的尾巴——而是单独用前缀 prompt 摘要，再与历史摘要拼接。
    let (history_head, turn_prefix) = split_history_turn_prefix(&head, &recent);

    // 历史摘要（预算封顶 + 流式调用 + 质量兜底都在 compact_with_summary_model 内）。
    // head 为空（整段 old_segment 都是本轮前半 / 只剩锚点+ack）时没有历史可重摘——
    // **但上一份摘要必须随身携带**：它已被上面的过滤从视图中剔除，这里不带上就会随
    // `replace_with_summary` 整段替换而永久丢失（审查发现的数据丢失回归：每个压缩
    // 周期只有一次长 agentic 请求的常见用法必命中）。无旧摘要才是真·"No prior history."。
    let mut cancel = cancel;
    let history_attempt = if history_head.is_empty() {
        CompactAttempt::Summary(empty_history_fallback_text(previous_summary.as_ref()))
    } else {
        compact_with_summary_model(
            state,
            provider,
            model,
            &serialize_for_summary(history_head),
            previous_summary.as_ref().map(|p| p.text.as_str()),
            focus,
            SummaryKind::History,
            window,
            config_max_output_tokens,
            retry_attempts,
            conversation_id,
            message_id,
            cancel.take(),
        )
        .await
    };
    let summary_text = match (history_attempt, turn_prefix.is_empty()) {
        (CompactAttempt::Summary(history_text), false) => {
            // 前缀摘要第二跳。history 跳没消费 cancel（head 为空走了 fallback）时由
            // 这一跳消费——否则 turn-prefix-only 压缩期间点「停止」要白等整个摘要调用。
            match compact_with_summary_model(
                state,
                provider,
                model,
                &serialize_for_summary(turn_prefix),
                None,
                None,
                SummaryKind::TurnPrefix,
                window,
                config_max_output_tokens,
                retry_attempts,
                conversation_id,
                message_id,
                cancel.take(),
            )
            .await
            {
                CompactAttempt::Summary(prefix_text) => CompactAttempt::Summary(format!(
                    "{history_text}\n\n---\n\n**Turn Context (split turn):**\n\n{prefix_text}"
                )),
                other => other,
            }
        }
        (attempt, _) => attempt,
    };

    match summary_text {
        CompactAttempt::Summary(text) => {
            CompactOutcome::Compacted(replace_with_summary(system_prefix, &text, recent), text)
        }
        CompactAttempt::Cancelled => CompactOutcome::Cancelled,
        CompactAttempt::Failed => CompactOutcome::Failed,
    }
}

/// `summarize_history` 的三态返回：成功（压缩后视图 + 摘要正文）/ 取消 / 失败。
enum CompactOutcome {
    Compacted(Vec<Value>, String),
    Cancelled,
    Failed,
}

/// split-turn 切分（pi `findCutPoint` 的 `isSplitTurn` 判定）：近期窗口不以 user 消息开头
/// ⇒ token 切点劈开了一轮，把 head 从「本轮起始 user 消息」处切成（历史，本轮前半）。
/// 近期窗口以 user 开头、或 head 里根本没有 user 消息（找不到轮起点）时前半为空。
fn split_history_turn_prefix<'a>(
    head: &'a [Value],
    recent: &[Value],
) -> (&'a [Value], &'a [Value]) {
    let split_turn = recent
        .first()
        .map(|m| m.get("role").and_then(Value::as_str) != Some("user"))
        .unwrap_or(false);
    if !split_turn {
        return (head, &[]);
    }
    match head
        .iter()
        .rposition(|m| m.get("role").and_then(Value::as_str) == Some("user"))
    {
        Some(pos) => (&head[..pos], &head[pos..]),
        None => (head, &[]),
    }
}

/// history_head 为空（无历史可重摘）时历史段的替身文本：有上一份摘要就**原文携带**
/// （它已被剔出视图，不带上就随整段替换永久丢失——链式压缩的旧摘要绝不许丢）；
/// 没有才是 pi 的 "No prior history."。
fn empty_history_fallback_text(previous_summary: Option<&PreviousSummary>) -> String {
    match previous_summary {
        Some(previous) => previous.text.clone(),
        None => "No prior history.".to_string(),
    }
}

/// 循环内上下文治理入口。返回本步应发送的消息视图：
/// - 未超限：原样 clone（零行为变化）。
/// - 超限：模型摘要——把系统前缀与受保护近期窗口之外的旧段压成一条结构化摘要（R5–R9），
///   成功后**写回 state.runtime_messages**（工作副本）并置 `state.compacted = true`
///   （供 finalize 把压缩后历史回传给跨轮调用方）；失败或取消则降级返回原始 clone
///   ——压缩是优化，绝不让它失败掉整轮。
///
/// `generated_api_messages`（持久化镜像）在任何分支都不被触碰。
pub(crate) async fn maybe_compact_send_view(env: &LoopEnv<'_>, state: &mut RunState) -> Vec<Value> {
    let config = env.config;
    // 先做无条件的图片收敛：与 token 预算无关，重复上传同一张图、以及无上限堆积的历史
    // 图片，任何情况下都是纯浪费，而 token 估算看不见它们（详见 `prune_image_parts`）。
    let saved_bytes = prune_image_parts(&mut state.runtime_messages, IMAGE_BYTES_BUDGET);
    if saved_bytes > 0 {
        eprintln!("Chat context: pruned {saved_bytes} bytes of image data from the send view");
    }
    // 统一基准：裸窗口 × AUTO_COMPACT_RATIO（0.90），对齐 Codex。去掉 safe_window 折扣——
    // 触发 / 摘要输入封顶都用同一个裸窗口，三处触发（落盘 / L2 / 手动）口径一致。
    let window = context_window_for_model(Some(&config.provider), &config.model).0;
    if window == 0 {
        return state.runtime_messages.clone();
    }
    // 真实用量锚点口径（对齐 pi/opencode 的 ground-truth 优先）：有锚点时用 provider 实报的
    // 上次 prompt token 数 + 锚点响应起往后新增消息的字符估算；无锚点回落纯字符估算。
    // 取 `max(纯估算)` 作保守下限，绝不因锚点偏小比现状更乐观。
    let budget = (window as f32 * AUTO_COMPACT_RATIO) as usize;
    // 纯字符估算 = 消息 + **工具 schema**（对齐 pi/footer 的兜底口径：pi 兜底含 system+每工具+消息；
    // Kivio footer 也含 `estimate_tool_segments`）。工具定义随每次请求发送、provider 会计入，漏算会
    // 让无锚点的首轮低估数千 token、压缩过晚——故这里补上（与 footer `count_tokens_in_value` 同口径，
    // 都基于 `estimate_value_tokens(tool.to_openai_tool())`）。
    let tool_schema_tokens: usize = state
        .tools
        .iter()
        .map(|tool| estimate_value_tokens(&tool.to_openai_tool()))
        .sum();
    let estimate_full =
        estimate_messages_tokens(&state.runtime_messages).saturating_add(tool_schema_tokens);
    let (anchor_prompt, trailing) = if let Some(usage) = &state.last_step_usage {
        // 本轮已发生过模型调用：锚点 = 上次调用 usage（含 output）；trailing = 那次**响应之后**新增。
        let start = state
            .runtime_len_at_last_call
            .min(state.runtime_messages.len());
        (
            super::context_estimate::anchor_total_tokens(usage, &config.provider.api_format),
            estimate_messages_tokens(&state.runtime_messages[start..]),
        )
    } else if state.initial_anchor_valid {
        // 本轮尚未调用模型（首次压缩检查）：用上一轮落盘 usage 组成的 config 锚点。
        (
            config.initial_anchor_total_tokens,
            config.initial_anchor_trailing_estimate,
        )
    } else {
        (None, 0)
    };
    let (estimated, _anchored) =
        super::context_estimate::effective_context_tokens(anchor_prompt, trailing, estimate_full);
    // **内置路径的实时用量通道**：本函数每个 planning 轮都跑一次，且这两个数就是权威口径
    // （`compute_context_state` 用的是同一对函数 `anchor_total_tokens` +
    // `effective_context_tokens`，分母同样是 `context_window_for_model`）—— 白捡的实时来源，
    // 零额外计算。粒度是「每轮一次」而不是每个 token：内置路径的分子来自 provider 的
    // usage，只有一次模型调用结束才有新数，中途没有更细的真实来源。
    // 子 agent 的 host 走默认 no-op，用量不会混进主对话。
    env.host.emit_context_usage_live(
        &config.conversation_id,
        estimated as u64,
        Some(window as u64),
    );
    if estimated <= budget {
        // 未超预算：本步无需压缩。重置 anti-thrashing 计数（Gap 2）——上下文已回到预算内。
        state.compaction_unresolved_rounds = 0;
        return state.runtime_messages.clone();
    }

    eprintln!(
        "Chat context compaction: est {estimated} tokens over budget {budget} (window {window}); summarizing old history"
    );

    env.host
        .emit_compaction_status(&config.conversation_id, "started", Some("agent_loop"), None);

    // 受保护近期窗口默认 20k token，但不得超过压缩预算——否则小窗口模型上整段历史会被近期窗口
    // 吞掉，没有可摘要的旧段，压缩永远救不了超窗。
    let keep_tokens = RECENT_KEEP_TOKENS.min(budget);

    // Microcompact（R-1）：先尝试把旧段工具结果降级成标记，够了就跳过昂贵的 LLM 摘要
    // （对齐 Claude Code "能拖就拖、便宜优先"）。仅当降级足以回到预算内才走此分支。
    if let Some(degraded) = microcompact_send_view(&state.runtime_messages, keep_tokens, budget) {
        let after = estimate_messages_tokens(&degraded);
        eprintln!(
            "Chat context microcompaction: est {estimated} -> {after} tokens (skipped summary)"
        );
        state.runtime_messages = degraded.clone();
        state.compacted = true;
        state.compaction_unresolved_rounds = 0;
        // 压缩后消息序列已变，旧锚点失真——清空，回落纯估算直到下次模型调用产生新 usage。
        state.last_step_usage = None;
        state.initial_anchor_valid = false;
        env.host.emit_compaction_status(
            &config.conversation_id,
            "microcompacted",
            Some("agent_loop"),
            None,
        );
        return degraded;
    }

    // 降级不足以回到预算内——走重型 LLM 摘要。取消 future 只在这条路径需要。
    let cancel = env
        .host
        .wait_for_generation_inactive(&config.conversation_id, config.generation);
    let runtime_before_compact = state.runtime_messages.clone();
    let compacted = summarize_history(
        config.state,
        &config.provider,
        &config.model,
        &state.runtime_messages,
        keep_tokens,
        window,
        // 用模型真实 max output（而非 run 的 config.max_output_tokens），与持久化路径
        // compact_conversation 口径统一——否则 run 配的小输出会把摘要卡短、9 段产出被截。
        chat_max_output_tokens_for_model(
            Some(&config.provider),
            &config.model,
            config.max_output_tokens,
        ),
        config.retry_attempts,
        &config.conversation_id,
        &config.message_id,
        None,
        Some(cancel),
    )
    .await;

    match compacted {
        CompactOutcome::Compacted(compacted, summary_text) => {
            let after = estimate_messages_tokens(&compacted);
            eprintln!("Chat context compaction: est {estimated} -> {after} tokens");
            state.runtime_messages = compacted.clone();
            state.compacted = true;
            // 压缩后消息序列已变，旧锚点失真——清空，回落纯估算直到下次模型调用产生新 usage。
            state.last_step_usage = None;
            state.initial_anchor_valid = false;
            if after <= budget {
                state.compaction_unresolved_rounds = 0;
            } else {
                state.compaction_unresolved_rounds =
                    state.compaction_unresolved_rounds.saturating_add(1);
            }
            if let Some(source_until_message_id) =
                source_until_message_id_for_split(&runtime_before_compact, keep_tokens)
            {
                let created_at = chrono::Local::now().timestamp();
                let summary_record = ConversationContextSummary {
                    id: format!("ctxsum_{}", uuid::Uuid::new_v4()),
                    content: summary_text.clone(),
                    source_message_ids: Vec::new(),
                    source_until_message_id: source_until_message_id.clone(),
                    token_estimate_before: estimated,
                    token_estimate_after: estimate_tokens(&summary_text),
                    created_at,
                    provider_id: config.provider.id.clone(),
                    model: config.model.clone(),
                    stale: false,
                    // Populated at the reply.rs persist site, which has the
                    // Conversation (this L2 path only holds runtime Values).
                    file_ledger: None,
                };
                let boundary = CompactionBoundaryRecord {
                    id: format!("ctxbd_{}", uuid::Uuid::new_v4()),
                    source_until_message_id,
                    // 时间线锚点：触发压缩时 runtime 里最后一条可映射的 UI 消息（run 进行中
                    // assistant 尚未落库，即最后一条 user）——divider 标记压缩发生的时刻。
                    display_after_message_id: runtime_before_compact
                        .iter()
                        .rev()
                        .find_map(|m| m.get(UI_MESSAGE_ID_KEY).and_then(Value::as_str))
                        .map(str::to_string),
                    token_estimate_before: estimated,
                    token_estimate_after: after,
                    summary_content: summary_text,
                    trigger: "agent_loop".to_string(),
                    created_at,
                };
                env.host.emit_compaction_status(
                    &config.conversation_id,
                    "completed",
                    Some("agent_loop"),
                    Some(&boundary),
                );
                state.pending_compaction_boundary = Some(boundary);
                state.pending_compaction_summary = Some(summary_record);
            } else {
                // 压缩视图已生效但无法可靠映射回 UI 消息（旧段只有摘要锚点/系统注入）——
                // 不落盘 boundary，但必须发终止事件让前端"压缩中"归位。
                env.host.emit_compaction_status(
                    &config.conversation_id,
                    "completed",
                    Some("agent_loop"),
                    None,
                );
            }
            compacted
        }
        CompactOutcome::Cancelled => {
            // 用户主动取消进行中的 run：不计入 anti-thrashing（取消 ≠ 压缩无能为力），
            // 让后续 planning 自己检测取消并正常收尾。仍发终止事件让前端"压缩中"归位。
            env.host.emit_compaction_status(
                &config.conversation_id,
                "failed",
                Some("agent_loop"),
                None,
            );
            state.runtime_messages.clone()
        }
        CompactOutcome::Failed => {
            // Gap 2: 需要压缩（超预算）但压缩没能减小上下文（摘要调用失败/为空/过短/无旧段）——
            // 计为一次「未解决」。连续达到 COMPACTION_THRASH_LIMIT 次时，规划循环会据此优雅收尾，
            // 而不是反复触发压缩并失败 6+ 次后才报错。
            state.compaction_unresolved_rounds =
                state.compaction_unresolved_rounds.saturating_add(1);
            // started 已发——失败也必须发终止事件，否则前端"压缩中"状态永久卡死。
            env.host.emit_compaction_status(
                &config.conversation_id,
                "failed",
                Some("agent_loop"),
                None,
            );
            state.runtime_messages.clone()
        }
    }
}

/// 手动压缩的保底切分（R4）：token 尾窗覆盖全部消息（无旧段）时，`/compact` 不该直接报
/// "没有足够的旧消息可以压缩"——旧行为（≤v2.7 落盘路径）小对话也可压。仅 `trigger == "manual"`
/// 且 `summary_start..len` 区间 UI 消息数 > 4 时生效：保留最后一条 user 及其后消息为近期窗口，
/// 其余进 old_segment；返回 old_segment 末尾下标。区间太短或末尾无可切点 → None（保持原报错）。
/// auto / agent_loop 触发条件不受影响（它们要超 90% 窗口才会走到这里）。
/// 内部委托 `manual_fallback_split_over_indices`——落盘路径过滤多答排除臂后走同一逻辑。
/// 非过滤（连续区间）调用保留给直接单测用。
#[cfg(test)]
fn manual_fallback_split(
    messages: &[ChatMessage],
    summary_start: usize,
    trigger: &str,
) -> Option<usize> {
    if summary_start >= messages.len() {
        return None;
    }
    let indices: Vec<usize> = (summary_start..messages.len()).collect();
    manual_fallback_split_over_indices(messages, &indices, trigger)
}

/// `manual_fallback_split` 的 index-based 核心：在升序原始下标序列 `indices` 上找最后一条
/// user，其之前的那条 included 消息为 old_segment 末尾。indices 数 ≤ 4 或 user 在首位 → None。
fn manual_fallback_split_over_indices(
    messages: &[ChatMessage],
    indices: &[usize],
    trigger: &str,
) -> Option<usize> {
    if trigger != "manual" {
        return None;
    }
    if indices.len() <= 4 {
        return None;
    }
    let last_user_pos = indices
        .iter()
        .rposition(|&idx| messages[idx].role == "user")?;
    // 最后一条 user 之前必须还有可摘要内容（不在 included 首位）。
    if last_user_pos == 0 {
        return None;
    }
    Some(indices[last_user_pos - 1])
}

/// 落盘路径「参与压缩」的原始下标（升序）：`summary_start` 之后、且**未被多答组排除**
/// （`group_answer_excluded_from_context`，与 build_chat_api_messages 同谓词）的消息。
/// 排除臂不进 replay，也不该进摘要输入 / token 预算。
fn context_included_indices(conversation: &Conversation, summary_start: usize) -> Vec<usize> {
    (summary_start..conversation.messages.len())
        .filter(|&idx| {
            !crate::chat::commands::context::group_answer_excluded_from_context(
                conversation,
                &conversation.messages[idx],
            )
        })
        .collect()
}

/// 累积 `source_message_ids`：上一份未过期 summary 的 ids ∪ 其 boundary 之后至 `until_id`
///（含）的全部消息 id（**按原始序列**，含多答排除臂）。落盘路径 `compact_conversation_inner`
/// 与 L2 run 结束写回（commands.rs）**共用**，防止两处累积口径分叉。
/// 找不到 `until_id` → 仅返回旧 ids（防御，不 panic）。
pub(crate) fn accumulate_source_ids(conversation: &Conversation, until_id: &str) -> Vec<String> {
    let prev = conversation
        .context_state
        .summary
        .as_ref()
        .filter(|s| !s.stale);
    let mut ids = prev
        .map(|s| s.source_message_ids.clone())
        .unwrap_or_default();
    let summary_start = prev
        .and_then(|s| {
            conversation
                .messages
                .iter()
                .position(|m| m.id == s.source_until_message_id)
        })
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let Some(until_idx) = conversation.messages.iter().position(|m| m.id == until_id) else {
        return ids;
    };
    if until_idx < summary_start {
        return ids;
    }
    ids.extend(
        conversation.messages[summary_start..=until_idx]
            .iter()
            .map(|m| m.id.clone()),
    );
    ids
}

/// 落盘压缩统一入口（手动 `chat_compress_context` / 自动发送前 / L2 run 结束三处共用）。
/// 按 token 尾窗切 old_segment / recent_tail，序列化 old_segment（含完整工具转录，工具结果截 2000 字），
/// 调统一核心 `compact_with_summary_model`（Claude 9 段 prompt + 流式 + 质量兜底），写回
/// `context_state.summary` + `compaction_boundaries` + `compression_count`，发协议压缩更新。
///
/// `trigger`: `"manual"` | `"auto"`。`focus`：手动 `/compact <focus>` 聚焦指令（自动为 None）。
/// 失败 / 无可摘要旧段 / 摘要质量不达标 → `Err`，**不覆盖**旧 summary。
pub(crate) async fn compact_conversation(
    state: &AppState,
    settings: &Settings,
    conversation: &mut Conversation,
    trigger: &str,
    focus: Option<&str>,
) -> Result<(), String> {
    compact_conversation_inner(state, settings, conversation, trigger, focus).await
}

async fn compact_conversation_inner(
    state: &AppState,
    settings: &Settings,
    conversation: &mut Conversation,
    trigger: &str,
    focus: Option<&str>,
) -> Result<(), String> {
    // 上一份落盘 summary 之后才进 old_segment；其之前已被摘要覆盖，不重复进。
    let summary_start = conversation
        .context_state
        .summary
        .as_ref()
        .filter(|s| !s.stale)
        .and_then(|s| {
            conversation
                .messages
                .iter()
                .position(|m| m.id == s.source_until_message_id)
        })
        .map(|idx| idx + 1)
        .unwrap_or(0);

    // 参与压缩的消息：summary_start 之后、且**未被多答组排除**的原始下标（与
    // build_chat_api_messages 同谓词——排除臂不进 replay，也不该进摘要输入/token 预算）。
    let included = context_included_indices(conversation, summary_start);
    let split = token_split_over_indices(&conversation.messages, &included, RECENT_KEEP_TOKENS)
        .or_else(|| manual_fallback_split_over_indices(&conversation.messages, &included, trigger))
        .ok_or_else(|| "没有足够的旧消息可以压缩".to_string())?;
    // old_segment = included 中原始下标 ≤ split 的消息（过滤后可能非连续）。
    let old_segment: Vec<&ChatMessage> = included
        .iter()
        .copied()
        .take_while(|&idx| idx <= split)
        .map(|idx| &conversation.messages[idx])
        .collect();
    if old_segment.is_empty() {
        return Err("没有足够的旧消息可以压缩".to_string());
    }
    let source_until_message_id = old_segment
        .last()
        .map(|m| m.id.clone())
        .ok_or_else(|| "没有足够的旧消息可以压缩".to_string())?;

    // 摘要模型：mixer 选 auto 时跟随当前会话主模型（effective_compression_model_for_session）。
    let (provider_id, model) =
        settings.effective_compression_model_for_session(Some(session_model_for(conversation)));
    let provider = settings
        .get_provider(&provider_id)
        .ok_or_else(|| "Compression provider not found".to_string())?
        .clone();
    if provider.api_keys.is_empty() {
        return Err(format_chat_missing_api_key_error(&provider.name));
    }
    if model.trim().is_empty() {
        return Err(chat_missing_model_error());
    }
    let retry_attempts = if settings.retry_enabled {
        settings.retry_attempts as usize
    } else {
        1
    };
    let window = context_window_for_model(Some(&provider), &model).0;

    let previous_summary = conversation
        .context_state
        .summary
        .as_ref()
        .filter(|s| !s.stale)
        .map(|s| s.content.clone());
    let serialized_head = serialize_chat_messages_for_summary(&old_segment);
    let message_id = source_until_message_id.clone();
    let summary_text = match compact_with_summary_model(
        state,
        &provider,
        &model,
        &serialized_head,
        previous_summary.as_deref(),
        focus,
        // ChatMessage 粒度的切分以整条 UI 消息为单位——一条 assistant UI 消息就是完整
        // 一轮，边界天然不劈开 turn，无需 TurnPrefix。
        SummaryKind::History,
        window,
        chat_max_output_tokens_for_model(Some(&provider), &model, settings.chat.max_output_tokens),
        retry_attempts,
        &conversation.id,
        &message_id,
        None,
    )
    .await
    {
        // 落盘/手动路径 cancel=None → Cancelled 不可达；与 Failed 同样报错、不覆盖旧 summary。
        CompactAttempt::Summary(text) => text,
        CompactAttempt::Cancelled | CompactAttempt::Failed => {
            return Err("Compression model returned an overly short or empty summary".to_string())
        }
    };

    let created_at = chrono::Local::now().timestamp();
    let token_estimate_before = previous_summary
        .as_deref()
        .map(estimate_tokens)
        .unwrap_or(0)
        + estimate_tokens(&serialized_head);
    let token_estimate_after = estimate_tokens(&summary_text);

    // 累积覆盖范围的全部消息 id（含多答排除臂——它们同样被 boundary 覆盖、不再 replay）。
    // 用共享 helper 而非 `old_segment.iter()`：批次 C 后 old_segment 会过滤掉排除臂，
    // 但 source_message_ids 必须按原始序列收集，两处口径由 helper 统一。
    let source_message_ids = accumulate_source_ids(conversation, &source_until_message_id);
    let compressed_message_count = source_message_ids.len();

    // Deterministic files-touched ledger over the covered history (see file_ledger).
    let file_ledger =
        super::file_ledger::build_for_boundary(conversation, &source_until_message_id);

    conversation.context_state.summary = Some(ConversationContextSummary {
        id: format!("ctxsum_{}", uuid::Uuid::new_v4()),
        content: summary_text.clone(),
        source_message_ids,
        source_until_message_id: source_until_message_id.clone(),
        token_estimate_before,
        token_estimate_after,
        created_at,
        provider_id,
        model,
        stale: false,
        file_ledger: (!file_ledger.is_empty()).then_some(file_ledger),
    });
    conversation.context_state.last_compressed_at = Some(created_at);
    conversation.context_state.compressed_message_count = compressed_message_count;
    conversation.context_state.compression_count = conversation
        .context_state
        .compression_count
        .saturating_add(1);

    let boundary_record = CompactionBoundaryRecord {
        id: format!("ctxbd_{}", uuid::Uuid::new_v4()),
        source_until_message_id,
        // 时间线锚点：divider 显示在「触发压缩时的最后一条消息」之后——标记压缩发生的
        // 时刻，而非 token 切分落点（切分落点在长对话里远高于触发点，观感是"横线跑上面去"）。
        display_after_message_id: conversation.messages.last().map(|m| m.id.clone()),
        token_estimate_before,
        token_estimate_after,
        summary_content: summary_text,
        trigger: trigger.to_string(),
        created_at,
    };
    conversation
        .context_state
        .compaction_boundaries
        .push(boundary_record);
    // R-4：多次链式压缩后提示准确性下降；未达阈值则清空告警（清掉上一轮"压缩失败但已发送"等旧警告）。
    conversation.context_state.warning =
        decay_warning_for(conversation.context_state.compression_count);

    Ok(())
}

/// 由当前会话解析出主模型（供 compression/title 等 auxiliary 任务在 mixer 选 auto 时跟随）。
fn session_model_for(conversation: &Conversation) -> crate::settings::SessionModel<'_> {
    crate::settings::SessionModel {
        provider_id: &conversation.provider_id,
        model: &conversation.model,
    }
}

/// 落盘路径「是否有可压缩旧段」判定（供 `should_auto_compress_context` 用）：
/// 在上一份未过期 summary 之后、按 `RECENT_KEEP_TOKENS` 尾窗切分后，是否还存在 old_segment。
pub(crate) fn has_compressible_old_segment(conversation: &Conversation) -> bool {
    let summary_start = conversation
        .context_state
        .summary
        .as_ref()
        .filter(|s| !s.stale)
        .and_then(|s| {
            conversation
                .messages
                .iter()
                .position(|m| m.id == s.source_until_message_id)
        })
        .map(|idx| idx + 1)
        .unwrap_or(0);
    // 与 compact_conversation_inner 同口径：过滤多答排除臂后再判断是否还有可摘要旧段。
    let included = context_included_indices(conversation, summary_start);
    token_split_over_indices(&conversation.messages, &included, RECENT_KEEP_TOKENS).is_some()
}

fn format_chat_missing_api_key_error(provider_name: &str) -> String {
    format!(
        "Provider 「{provider_name}」未配置 API Key，无法执行上下文压缩。请在设置中添加该 Provider 的密钥。"
    )
}

fn chat_missing_model_error() -> String {
    "未选择压缩模型，请在设置中指定或保持 auto 跟随当前会话模型。".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::model::ModelRole;
    use crate::chat::types::{ToolCallRecord, ToolCallStatus};

    fn chat_msg(id: &str, role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            attachments: Vec::new(),
            reasoning: None,
            artifacts: Vec::new(),
            tool_calls: Vec::new(),
            segments: Vec::new(),
            agent_plan: None,
            api_messages: Vec::new(),
            model_messages: Vec::new(),
            active_skill_id: None,
            run_entry: None,
            stream_outcome: None,
            usage: None,
            anchor_usage: None,
            group_id: None,
            provider_id: None,
            model: None,
            timestamp: 0,
            degraded: None,
        }
    }

    fn test_conversation(messages: Vec<ChatMessage>) -> Conversation {
        Conversation {
            id: "conv_test".to_string(),
            revision: 0,
            title: "t".to_string(),
            provider_id: "p".to_string(),
            model: "m".to_string(),
            messages,
            agent_runtime: Default::default(),
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
            context_state: Default::default(),
            agent_todo_state: Default::default(),
            agent_plan_state: Default::default(),
            knowledge_base_ids: Vec::new(),
            force_knowledge_search: false,
            thinking_level: None,
            web_search_mode: None,
            reply_models: Vec::new(),
            group_selections: Default::default(),
            forked_from: None,
        }
    }

    #[test]
    fn estimate_chat_message_tokens_counts_content_and_tools() {
        let mut m = chat_msg("m1", "assistant", &"abcd".repeat(100));
        m.reasoning = Some("r".repeat(40));
        m.tool_calls.push(ToolCallRecord {
            id: "c1".to_string(),
            name: "read".to_string(),
            source: String::new(),
            server_id: None,
            arguments: "{\"path\":\"/tmp/x\"}".to_string(),
            status: ToolCallStatus::Success,
            result_preview: Some("p".repeat(80)),
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
        });
        let tokens = estimate_chat_message_tokens(&m);
        // content 100/4=25, reasoning 40/4=10, tool name+args+preview ~ 25+, +1
        assert!(tokens > 60);
    }

    #[test]
    fn estimate_message_tokens_ignores_image_base64() {
        // 缺陷 2：图片 base64 不能打爆估算。带 1MB 级 base64 的多模态 user 消息，
        // 估算应与同文本纯文字消息同数量级（图片部件记 0）。
        let big_b64 = "A".repeat(1_400_000);
        let image_msg = json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "描述这张图" },
                { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{big_b64}") } }
            ]
        });
        let text_only = json!({ "role": "user", "content": "描述这张图" });
        let image_tokens = estimate_message_tokens(&image_msg);
        let text_tokens = estimate_message_tokens(&text_only);
        // 同数量级：差值不超过几十 token（结构 key 开销），绝不因 base64 膨胀到十万级。
        assert!(
            image_tokens < text_tokens + 50,
            "image msg est {image_tokens} must stay near text-only {text_tokens}"
        );
    }

    #[test]
    fn estimate_message_tokens_counts_reasoning() {
        // 缺陷 4(a)：reasoning_content 必须计入（与 serialize 口径一致）。
        let with_reasoning = json!({
            "role": "assistant",
            "content": "answer",
            "reasoning_content": "x".repeat(400)
        });
        let without = json!({ "role": "assistant", "content": "answer" });
        assert!(
            estimate_message_tokens(&with_reasoning) > estimate_message_tokens(&without) + 90,
            "reasoning (~100 tok) must be counted"
        );
    }

    #[test]
    fn prune_image_parts_keeps_only_the_last_copy() {
        // 真实故障复现：同一张图被 `read` 读了两次，两份逐字节相同的 base64 各占请求体
        // 一大半，每个 planning 轮重传一次，中转在传输途中断流。
        let image = |b64: &str| json!({ "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{b64}") } });
        let dup = "A".repeat(2000);
        let mut messages = vec![
            json!({ "role": "user", "content": [{ "type": "text", "text": "看这张图" }, image(&dup)] }),
            json!({ "role": "assistant", "content": "好的" }),
            json!({ "role": "user", "content": [image("DIFFERENT")] }),
            json!({ "role": "user", "content": [image(&dup)] }),
        ];

        let saved = prune_image_parts(&mut messages, IMAGE_BYTES_BUDGET);
        assert!(saved > 2000, "should report the dropped bytes, got {saved}");

        // 第一条里的重复图 → 占位文本
        let first = &messages[0]["content"][1];
        assert_eq!(first["type"], "text");
        assert_eq!(first["text"], DUPLICATE_IMAGE_PLACEHOLDER);
        // 文本部件不受影响
        assert_eq!(messages[0]["content"][0]["text"], "看这张图");
        // 不同的图原样保留
        assert_eq!(messages[2]["content"][0]["type"], "image_url");
        assert!(messages[2]["content"][0]["image_url"]["url"]
            .as_str()
            .unwrap()
            .contains("DIFFERENT"));
        // 最后一份重复图保留原图（模型当下在看的就是它）
        assert_eq!(messages[3]["content"][0]["type"], "image_url");
        assert!(messages[3]["content"][0]["image_url"]["url"]
            .as_str()
            .unwrap()
            .contains(&dup));

        // 幂等：再跑一次不再有可省的字节，也不会误删仅剩的那份
        assert_eq!(prune_image_parts(&mut messages, IMAGE_BYTES_BUDGET), 0);
        assert_eq!(messages[3]["content"][0]["type"], "image_url");
    }

    #[test]
    fn prune_image_parts_evicts_oldest_images_over_budget() {
        // 全都是**不同**的图（去重救不了），靠字节预算从最旧的开始淘汰。
        // 对应 Codex #28316：不同截图反复堆积，请求打到 8MB 后中转 502。
        let image = |tag: &str| {
            json!({
                "type": "image_url",
                "image_url": { "url": format!("data:image/png;base64,{tag}{}", "X".repeat(1000)) }
            })
        };
        let mut messages: Vec<Value> = ["oldest", "middle", "newest"]
            .iter()
            .map(|tag| json!({ "role": "user", "content": [image(tag)] }))
            .collect();

        // 预算只够放两张（每张 ~1KB 序列化后略多）。
        let saved = prune_image_parts(&mut messages, 2400);
        assert!(
            saved > 1000,
            "oldest image should be evicted, saved {saved}"
        );

        // 最旧的被换成带体积说明的占位文本
        assert_eq!(messages[0]["content"][0]["type"], "text");
        let note = messages[0]["content"][0]["text"].as_str().unwrap();
        assert!(note.contains("已从上下文移除"), "note was: {note}");
        assert!(note.contains("KB"), "占位符要带体积，便于用户理解: {note}");
        // 较新的两张保留
        assert_eq!(messages[1]["content"][0]["type"], "image_url");
        assert_eq!(messages[2]["content"][0]["type"], "image_url");

        // 预算充足时一张都不动
        let mut untouched: Vec<Value> = ["a", "b", "c"]
            .iter()
            .map(|tag| json!({ "role": "user", "content": [image(tag)] }))
            .collect();
        assert_eq!(prune_image_parts(&mut untouched, IMAGE_BYTES_BUDGET), 0);
        assert!(untouched
            .iter()
            .all(|m| m["content"][0]["type"] == "image_url"));
    }

    #[test]
    fn serialize_message_replaces_image_parts_with_placeholder() {
        // 缺陷 2：多模态 user 序列化——文本全文 + 图片占位符，绝不含 base64。
        let msg = json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "看这张截图" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAABBBB" } }
            ]
        });
        let s = serialize_message(&msg);
        assert!(s.contains("看这张截图"), "text part preserved");
        assert!(s.contains(IMAGE_PART_PLACEHOLDER), "image → placeholder");
        assert!(
            !s.contains(";base64,"),
            "no base64 leaks into summary input"
        );
    }

    #[test]
    fn estimate_chat_message_tokens_uses_expanded_api_messages() {
        // 缺陷 4(b)：带完整工具转录的 api_messages 应按展开形态估算，
        // 远大于截断 result_preview 口径。
        let mut m = chat_msg("m1", "assistant", "short visible text");
        m.tool_calls.push(ToolCallRecord {
            id: "c1".to_string(),
            name: "read".to_string(),
            source: "native".to_string(),
            server_id: None,
            arguments: "{}".to_string(),
            status: ToolCallStatus::Success,
            result_preview: Some("p".repeat(80)), // 截断预览（旧口径只算这个）
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
        });
        // 完整工具输出（真实 replay 内容）远大于 preview。
        m.api_messages = vec![
            json!({ "role": "assistant", "content": "", "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "read", "arguments": "{}" } }] }),
            json!({ "role": "tool", "tool_call_id": "c1", "content": "T".repeat(40_000) }),
        ];
        let tokens = estimate_chat_message_tokens(&m);
        let expanded_sum: usize = m.api_messages.iter().map(estimate_message_tokens).sum();
        assert_eq!(tokens, expanded_sum, "must estimate expanded api_messages");
        assert!(
            tokens > 9_000,
            "40k-char tool output ~10k tokens, far above preview"
        );
    }

    #[test]
    fn estimate_model_messages_tokens_ignores_image_base64() {
        // #2 修复：model_messages 分支直接按 MessagePart 估算，不把 base64 图片算进 token
        //（也不再克隆整段转录成 Vec<Value>）。
        let big_b64 = "A".repeat(1_400_000);
        let mut m = chat_msg("m1", "assistant", "ignored (model_messages wins)");
        m.model_messages = vec![
            ModelMessage {
                role: ModelRole::Assistant,
                content: vec![
                    MessagePart::Text {
                        text: "看这张图".to_string(),
                    },
                    MessagePart::Image {
                        mime_type: "image/png".to_string(),
                        data: big_b64.clone(),
                        path: None,
                    },
                ],
            },
            ModelMessage {
                role: ModelRole::Tool,
                content: vec![MessagePart::ToolResult {
                    tool_call_id: "c1".to_string(),
                    content: "T".repeat(4_000),
                    is_error: false,
                    artifacts: Vec::new(),
                }],
            },
        ];
        let tokens = estimate_chat_message_tokens(&m);
        // 文本(~2 CJK*? ) + 工具结果 4000/4=1000 + 2*4 开销 ≈ 1010 量级；绝不含 base64 的 35 万级。
        assert!(
            tokens < 2_000,
            "image base64 must not inflate estimate (was {tokens})"
        );
        assert!(
            tokens > 900,
            "tool result content must be counted (was {tokens})"
        );
    }

    #[test]
    fn serialize_message_tool_result_multimodal_no_base64_leak() {
        // #1 修复：工具结果为多模态数组（如 MCP 图片块）时，序列化走占位符而非 to_string() 泄漏 base64。
        let msg = json!({
            "role": "tool",
            "tool_call_id": "c1",
            "content": [
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAABBBBCCCC" } },
                { "type": "text", "text": "图里是一只猫" }
            ]
        });
        let s = serialize_message(&msg);
        assert!(s.starts_with("[Tool result]:"));
        assert!(s.contains("图里是一只猫"), "text part preserved");
        assert!(s.contains(IMAGE_PART_PLACEHOLDER), "image → placeholder");
        assert!(
            !s.contains(";base64,"),
            "no base64 leaks into summary input"
        );
    }

    #[test]
    fn serialize_message_assistant_multimodal_no_base64_leak() {
        // #1 修复：assistant 数组 content 不再被整段丢弃，且图片换占位符。
        let msg = json!({
            "role": "assistant",
            "content": [
                { "type": "text", "text": "看这个" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,ZZZZ" } }
            ]
        });
        let s = serialize_message(&msg);
        assert!(
            s.contains("[Assistant]: 看这个"),
            "assistant text preserved, not dropped"
        );
        assert!(s.contains(IMAGE_PART_PLACEHOLDER));
        assert!(!s.contains(";base64,"));
    }

    #[test]
    fn token_split_chat_messages_keeps_recent_tail_in_budget() {
        // 每条 5004 tokens（20000 chars/4 + 4 每条开销）；3 条总 15012 ≤ 20000 → 全在尾窗 → None。
        let small: Vec<ChatMessage> = (0..3)
            .map(|i| chat_msg(&format!("s{i}"), "user", &"a".repeat(20_000)))
            .collect();
        assert!(token_split_chat_messages(&small, 0, RECENT_KEEP_TOKENS).is_none());

        // 5 条各 5004 tokens。从尾累积 3 条(15012) 后第 4 条 → 20016 > 20000 越预算 →
        // old_segment=[b0,b1], recent=[b2..b4], boundary=1。
        let big: Vec<ChatMessage> = (0..5)
            .map(|i| chat_msg(&format!("b{i}"), "user", &"a".repeat(20_000)))
            .collect();
        let split = token_split_chat_messages(&big, 0, RECENT_KEEP_TOKENS).expect("split");
        assert_eq!(split, 1);
    }

    #[test]
    fn token_split_chat_messages_respects_summary_start() {
        // summary_start=2：index 0/1 属于上一份 summary、不参与本次；只从 index 2 起往尾扫。
        // 6 条各 ~20004 tokens（80000 chars），尾窗 20000 只容 1 条 → 其余进 old_segment，
        // boundary 落在倒数第 2 条(index 4)，且必然 > summary_start。
        let msgs: Vec<ChatMessage> = (0..6)
            .map(|i| chat_msg(&format!("m{i}"), "user", &"a".repeat(80_000)))
            .collect();
        let split = token_split_chat_messages(&msgs, 2, RECENT_KEEP_TOKENS).expect("split");
        assert_eq!(split, msgs.len() - 2);
        assert!(split > 2);
    }

    #[test]
    fn serialize_chat_message_includes_full_tool_args_and_clipped_result() {
        let mut m = chat_msg("m1", "assistant", "let me read it");
        m.tool_calls.push(ToolCallRecord {
            id: "c1".to_string(),
            name: "read".to_string(),
            source: "native".to_string(),
            server_id: None,
            arguments: "{\"path\":\"/tmp/important.txt\"}".to_string(),
            status: ToolCallStatus::Success,
            result_preview: Some("T".repeat(10_000)),
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
        });
        let s = serialize_chat_message_for_summary(&m);
        // 工具入参全文保留（修复「收到 ✅」根因）。
        assert!(s.contains("\"/tmp/important.txt\""));
        // 工具结果截到 2000 字（[truncated] 标记）。
        assert!(s.contains("[truncated]"));
        assert!(!s.contains(&"T".repeat(TOOL_OUTPUT_SUMMARY_MAX_CHARS + 1)));
    }

    #[test]
    fn summary_quality_guard_rejects_too_short() {
        // 「收到 ✅」式过短摘要 → TooShort，不覆盖旧 summary。
        let short = "收到 ✅";
        assert_eq!(summary_quality_guard(short, None), SummaryQuality::TooShort);
    }

    #[test]
    fn summary_output_tokens_caps_at_8192() {
        // R-3：容纳 9 段 analysis+summary，上限 8192；min() 语义保留。
        assert_eq!(SUMMARY_OUTPUT_TOKENS, 8_192);
        assert_eq!(summary_output_tokens(20_000), 8_192);
        assert_eq!(summary_output_tokens(200_000), 8_192);
        assert_eq!(summary_output_tokens(4_096), 4_096); // 真实上限更小的模型不受影响
    }

    #[test]
    fn decay_warning_for_fires_at_threshold() {
        // R-4：阈值 3；未达 → None；达到/超过 → Some 且含实际次数。
        assert_eq!(DECAY_WARNING_COMPRESSION_COUNT, 3);
        assert_eq!(decay_warning_for(0), None);
        assert_eq!(decay_warning_for(2), None);
        let w3 = decay_warning_for(3).expect("warning at threshold");
        assert!(w3.contains('3'));
        let w5 = decay_warning_for(5).expect("warning above threshold");
        assert!(w5.contains('5'));
    }

    #[test]
    fn summary_quality_guard_rejects_truncated_analysis() {
        // 截断的 9 段输出：吐了 <analysis> 前言但流断在 <summary> 之前，
        // extract_summary_text 回退返回整段 analysis（>200 字，能骗过长度闸）→ Truncated。
        let truncated = format!("<analysis>\n{}", "分析前言细节".repeat(60));
        assert!(truncated.chars().count() >= MIN_SUMMARY_CHARS);
        assert_eq!(
            summary_quality_guard(&truncated, None),
            SummaryQuality::Truncated
        );
    }

    #[test]
    fn summary_quality_guard_accepts_long_fresh_summary() {
        let long = "x".repeat(MIN_SUMMARY_CHARS + 10);
        assert_eq!(summary_quality_guard(&long, None), SummaryQuality::Ok);
    }

    #[test]
    fn summary_quality_guard_rejects_degraded_vs_previous() {
        // 新 summary 必须先过 TooShort 闸（≥ MIN_SUMMARY_CHARS=200）才可能命中 Degraded：
        // 旧 summary 1000 字达标，新 summary 250 字（≥200 但 < 30%×1000=300）→ Degraded。
        let previous = "p".repeat(1_000);
        let degraded = "n".repeat(250);
        assert_eq!(
            summary_quality_guard(&degraded, Some(&previous)),
            SummaryQuality::Degraded
        );
    }

    #[test]
    fn summary_quality_guard_accepts_chain_merge_when_comparable() {
        // 链式合并：新 summary 与旧 summary 长度相当 → Ok（允许覆盖更新）。
        let previous = "p".repeat(300);
        let merged = "m".repeat(280);
        assert_eq!(
            summary_quality_guard(&merged, Some(&previous)),
            SummaryQuality::Ok
        );
    }

    #[test]
    fn summary_quality_guard_skips_30pct_gate_when_previous_short() {
        // 旧 summary 本身不达标 → 不用 30% 门槛，只要新 summary 达标即 Ok
        // （避免一份烂旧 summary 卡死新摘要）。
        let previous = "p".repeat(50);
        let fresh = "n".repeat(MIN_SUMMARY_CHARS + 5);
        assert_eq!(
            summary_quality_guard(&fresh, Some(&previous)),
            SummaryQuality::Ok
        );
    }

    /// 构造带 `_ui_message_id` 标注的 runtime 消息（模拟 build_chat_api_messages 的注入）。
    fn tagged(ui_id: &str, role: &str, content: &str) -> Value {
        json!({ "role": role, "content": content, UI_MESSAGE_ID_KEY: ui_id })
    }

    #[test]
    fn source_until_maps_by_ui_tag_with_tool_expansion() {
        // UI 消息 m2（assistant）展开成 3 条 runtime（tool_calls / tool / 最终答复），
        // 全部落在旧段 → boundary 精确落在 m2；旧的条数推算会把展开的每条都计数而错位。
        let messages = vec![
            json!({ "role": "system", "content": "sys" }),
            tagged("m1", "user", &"a".repeat(4_000)),
            {
                let mut m = json!({ "role": "assistant", "content": "", "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "read", "arguments": "{}" } }] });
                m.as_object_mut()
                    .unwrap()
                    .insert(UI_MESSAGE_ID_KEY.into(), json!("m2"));
                m
            },
            {
                let mut m =
                    json!({ "role": "tool", "tool_call_id": "c1", "content": "b".repeat(4_000) });
                m.as_object_mut()
                    .unwrap()
                    .insert(UI_MESSAGE_ID_KEY.into(), json!("m2"));
                m
            },
            tagged("m2", "assistant", &"c".repeat(4_000)),
            tagged("m3", "user", "recent"),
        ];
        let until = source_until_message_id_for_split(&messages, 500);
        assert_eq!(until.as_deref(), Some("m2"));
    }

    #[test]
    fn source_until_skips_ui_message_straddling_boundary() {
        // m2 的展开条横跨边界（一部分在近期窗口）→ 不能作为 boundary，回退到 m1。
        let messages = vec![
            json!({ "role": "system", "content": "sys" }),
            tagged("m1", "user", &"a".repeat(8_000)),
            tagged("m2", "assistant", &"b".repeat(8_000)),
            tagged("m2", "assistant", "tail piece in recent"),
            tagged("m3", "user", "recent"),
        ];
        // keep=1000：recent 从尾部起 ~2 条小消息（m2 尾块 + m3），m2 首块在旧段 → 跨边界。
        let until = source_until_message_id_for_split(&messages, 1_000);
        assert_eq!(until.as_deref(), Some("m1"));
    }

    #[test]
    fn source_until_none_when_old_segment_untagged() {
        // 旧段只有摘要锚点/系统注入（无 _ui_message_id）→ None，调用方不落盘 boundary。
        let messages = vec![
            json!({ "role": "system", "content": "sys" }),
            json!({ "role": "user", "content": format!("{SUMMARY_MARKER_PREFIX} 摘要：\n{}", "s".repeat(8_000)) }),
            json!({ "role": "assistant", "content": "已了解早前对话的摘要，继续当前任务。" }),
            tagged("m9", "user", "recent question"),
        ];
        assert!(source_until_message_id_for_split(&messages, 1_000).is_none());
    }

    #[test]
    fn source_until_none_when_no_old_segment() {
        let messages = vec![
            json!({ "role": "system", "content": "sys" }),
            tagged("m1", "user", "hi"),
            tagged("m2", "assistant", "hello"),
        ];
        assert!(source_until_message_id_for_split(&messages, 8_000).is_none());
    }

    #[test]
    fn manual_fallback_split_keeps_last_user_pair() {
        // 6 条小消息（token 尾窗覆盖全部）：manual 保底切到最后一条 user 之前。
        let msgs: Vec<ChatMessage> = [
            ("m0", "user"),
            ("m1", "assistant"),
            ("m2", "user"),
            ("m3", "assistant"),
            ("m4", "user"),
            ("m5", "assistant"),
        ]
        .iter()
        .map(|(id, role)| chat_msg(id, role, "short"))
        .collect();
        assert!(token_split_chat_messages(&msgs, 0, RECENT_KEEP_TOKENS).is_none());
        // 最后一条 user 是 m4(index 4) → old_segment 末尾 = index 3。
        assert_eq!(manual_fallback_split(&msgs, 0, "manual"), Some(3));
        // 非手动触发不放宽。
        assert_eq!(manual_fallback_split(&msgs, 0, "auto"), None);
    }

    #[test]
    fn manual_fallback_split_rejects_short_conversations() {
        let msgs: Vec<ChatMessage> = [
            ("m0", "user"),
            ("m1", "assistant"),
            ("m2", "user"),
            ("m3", "assistant"),
        ]
        .iter()
        .map(|(id, role)| chat_msg(id, role, "short"))
        .collect();
        // ≤ 4 条 → None（保持"没有足够的旧消息可以压缩"报错）。
        assert_eq!(manual_fallback_split(&msgs, 0, "manual"), None);
        // summary_start 之后区间太短同样拒绝。
        let six: Vec<ChatMessage> = (0..6)
            .map(|i| {
                chat_msg(
                    &format!("m{i}"),
                    if i % 2 == 0 { "user" } else { "assistant" },
                    "s",
                )
            })
            .collect();
        assert_eq!(manual_fallback_split(&six, 2, "manual"), None);
    }

    #[test]
    fn estimate_counts_content_and_structured_fields() {
        let messages = vec![
            json!({ "role": "user", "content": "abcd".repeat(10) }),
            json!({ "role": "assistant", "content": "", "tool_calls": [{"id": "x", "function": {"name": "f", "arguments": "{}"}}] }),
        ];
        assert!(estimate_messages_tokens(&messages) > 10);
    }

    #[test]
    fn serialize_keeps_user_and_assistant_full() {
        let big_user = "U".repeat(5_000);
        let big_assistant = "A".repeat(5_000);
        let messages = vec![
            json!({ "role": "user", "content": big_user.clone() }),
            json!({ "role": "assistant", "content": big_assistant.clone() }),
        ];
        let serialized = serialize_for_summary(&messages);
        // 用户/助手消息全文保留（不截断）。
        assert!(serialized.contains(&big_user));
        assert!(serialized.contains(&big_assistant));
        assert!(serialized.contains("[User]:"));
        assert!(serialized.contains("[Assistant]:"));
    }

    #[test]
    fn serialize_clips_tool_result_to_cap() {
        let huge = "T".repeat(10_000);
        let messages = vec![json!({ "role": "tool", "tool_call_id": "c1", "content": huge })];
        let serialized = serialize_for_summary(&messages);
        assert!(serialized.starts_with("[Tool result]:"));
        assert!(serialized.contains("[truncated]"));
        // The clipped tool output keeps at most the cap chars (+ marker), far less than 10k.
        let t_run = "T".repeat(TOOL_OUTPUT_SUMMARY_MAX_CHARS + 1);
        assert!(
            !serialized.contains(&t_run),
            "tool output must be clipped to the cap"
        );
        // But it does keep the cap-sized prefix.
        assert!(serialized.contains(&"T".repeat(TOOL_OUTPUT_SUMMARY_MAX_CHARS)));
    }

    #[test]
    fn serialize_renders_tool_error_and_tool_call() {
        let messages = vec![
            json!({
                "role": "assistant",
                "content": "let me read it",
                "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": { "name": "read_file", "arguments": "{\"path\":\"main.rs\"}" }
                }]
            }),
            json!({ "role": "tool", "tool_call_id": "c1", "content": "boom", "is_error": true }),
        ];
        let serialized = serialize_for_summary(&messages);
        assert!(serialized.contains("[Assistant]: let me read it"));
        assert!(serialized.contains("[Assistant tool call]: read_file({\"path\":\"main.rs\"})"));
        assert!(serialized.contains("[Tool error]: boom"));
    }

    #[test]
    fn select_recent_by_tokens_splits_near_boundary() {
        let mut messages = vec![json!({ "role": "system", "content": "sys" })];
        // Each message ~ 250 tokens (1000 chars / 4). 40 messages ~ 10k tokens.
        for i in 0..40 {
            messages.push(json!({
                "role": if i % 2 == 0 { "user" } else { "assistant" },
                "content": "x".repeat(1_000)
            }));
        }
        let (sys, old, recent) = select_recent_by_tokens(&messages, 8_000);
        assert_eq!(sys.len(), 1, "system prefix protected");
        assert!(!old.is_empty(), "older messages go to the summary");
        assert!(!recent.is_empty(), "a recent tail is preserved");
        // The recent tail is bounded near 8000 tokens (whole messages, never split).
        let recent_tokens = estimate_messages_tokens(&recent);
        assert!(
            recent_tokens <= 8_000 + 300,
            "recent window ~8000 tokens (was {recent_tokens})"
        );
        // No message was split: every recent/old message is a full object from the input.
        assert_eq!(sys.len() + old.len() + recent.len(), messages.len());
        // Order preserved: old then recent reconstruct the post-system messages.
        assert_eq!(old[0]["content"], messages[1]["content"]);
        assert_eq!(recent.last().unwrap()["content"], messages[40]["content"]);
    }

    #[test]
    fn microcompact_reclaims_old_tool_results_and_skips_summary() {
        // old_segment 由两条大工具结果主导；降级后应回到预算内 → Some（可跳过摘要）。
        let messages = vec![
            json!({ "role": "system", "content": "sys" }),
            json!({ "role": "assistant", "content": "", "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "read", "arguments": "{}" } }] }),
            json!({ "role": "tool", "tool_call_id": "c1", "content": "T".repeat(40_000) }),
            json!({ "role": "assistant", "content": "", "tool_calls": [{ "id": "c2", "type": "function", "function": { "name": "read", "arguments": "{}" } }] }),
            json!({ "role": "tool", "tool_call_id": "c2", "content": "T".repeat(40_000) }),
            json!({ "role": "user", "content": "recent question" }),
            json!({ "role": "assistant", "content": "recent answer" }),
        ];
        // 总 ~20000 tok，budget 12000 → 超；keep 8000 → recent = 末尾两条小消息，两条大工具结果进 old。
        let view =
            microcompact_send_view(&messages, 8_000, 12_000).expect("microcompact should suffice");
        assert!(
            estimate_messages_tokens(&view) <= 12_000,
            "degraded view within budget"
        );
        let markers = view
            .iter()
            .filter(|m| m.get("content").and_then(Value::as_str) == Some(MICROCOMPACT_TOOL_MARKER))
            .count();
        assert_eq!(markers, 2, "both old tool results degraded");
        assert_eq!(view[view.len() - 2]["content"], "recent question");
        assert_eq!(view[view.len() - 1]["content"], "recent answer");
    }

    #[test]
    fn microcompact_returns_none_when_insufficient() {
        // old 段含可降级工具结果 + 一条无法降级的大 user 文本；降级工具后仍超 budget → None。
        // keep_tokens 取小，确保大 user + 工具结果都落在 old（不被拉进近期窗口）。
        let messages = vec![
            json!({ "role": "system", "content": "sys" }),
            json!({ "role": "user", "content": "U".repeat(80_000) }), // ~20000 tok，无法降级
            json!({ "role": "tool", "tool_call_id": "c1", "content": "T".repeat(4_000) }), // 可降级
            json!({ "role": "assistant", "content": "recent answer" }),
        ];
        assert!(microcompact_send_view(&messages, 100, 12_000).is_none());
    }

    #[test]
    fn microcompact_returns_none_when_no_old_segment() {
        // 全在近期窗口内（无旧段）→ None。
        let messages = vec![
            json!({ "role": "system", "content": "sys" }),
            json!({ "role": "user", "content": "hi" }),
            json!({ "role": "assistant", "content": "hello" }),
        ];
        assert!(microcompact_send_view(&messages, 8_000, 12_000).is_none());
    }

    #[test]
    fn microcompact_leaves_recent_tool_results_untouched() {
        // 近期窗口里的工具结果不降级——只降 old_segment。
        let messages = vec![
            json!({ "role": "system", "content": "sys" }),
            json!({ "role": "tool", "tool_call_id": "old", "content": "T".repeat(40_000) }),
            json!({ "role": "user", "content": "q" }),
            json!({ "role": "tool", "tool_call_id": "recent", "content": "recent tool output kept" }),
            json!({ "role": "assistant", "content": "a" }),
        ];
        let view = microcompact_send_view(&messages, 8_000, 12_000).expect("suffices");
        assert!(view
            .iter()
            .any(|m| m.get("content").and_then(Value::as_str) == Some(MICROCOMPACT_TOOL_MARKER)));
        assert!(view
            .iter()
            .any(|m| m.get("content").and_then(Value::as_str) == Some("recent tool output kept")));
    }

    #[test]
    fn select_recent_never_splits_tool_call_pair() {
        // Build: system, then many small messages, then an assistant(tool_calls)
        // immediately followed by a large tool result that lands on the boundary.
        let mut messages = vec![json!({ "role": "system", "content": "sys" })];
        for _ in 0..10 {
            messages.push(json!({ "role": "user", "content": "x".repeat(1_000) }));
        }
        messages.push(json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "read", "arguments": "{}" } }]
        }));
        // A big tool result that nudges the recent window to start right after it.
        messages
            .push(json!({ "role": "tool", "tool_call_id": "c1", "content": "y".repeat(30_000) }));
        // A trailing user message keeps the tail non-trivial.
        messages.push(json!({ "role": "user", "content": "done?" }));

        let (_sys, old, recent) = select_recent_by_tokens(&messages, 8_000);
        // The recent window must never START with an orphan tool result whose
        // assistant(tool_calls) got left in `old`.
        if let Some(first) = recent.first() {
            assert!(
                !is_tool_result(first),
                "recent window must not start with an orphan tool result"
            );
        }
        // And old must never END with an assistant(tool_calls) whose tool result was pulled away.
        if let Some(last) = old.last() {
            assert!(
                !has_tool_calls(last),
                "old segment must not end with a dangling tool_call whose result moved to recent"
            );
        }
    }

    #[test]
    fn extract_previous_summary_detects_anchored_marker() {
        let old = vec![
            json!({ "role": "user", "content": format!("{SUMMARY_MARKER_PREFIX} 引导语：\n1. Primary Request: build X") }),
            json!({ "role": "assistant", "content": SUMMARY_ACK_TEXT }),
            json!({ "role": "user", "content": "next question" }),
        ];
        let previous = extract_previous_summary(&[], &old).expect("prior summary detected");
        assert!(previous.text.contains("Primary Request: build X"));
        assert!(!previous.from_injected, "anchor is not an injected summary");
        // No marker present → None.
        let fresh = vec![json!({ "role": "user", "content": "just a question" })];
        assert!(extract_previous_summary(&[], &fresh).is_none());
    }

    #[test]
    fn extract_previous_summary_detects_injected_system_summary() {
        // 落盘 summary 被 build_chat_api_messages 注入为 system 前缀消息（PERSISTED_SUMMARY_PREFIX）。
        let system_prefix = vec![
            json!({ "role": "system", "content": "you are a helpful assistant" }),
            json!({
                "role": "system",
                "content": format!("{PERSISTED_SUMMARY_PREFIX}\n1. Primary Request: earlier goal")
            }),
        ];
        let old_segment = vec![json!({ "role": "user", "content": "later question" })];
        let previous = extract_previous_summary(&system_prefix, &old_segment)
            .expect("injected summary detected");
        assert!(previous.text.contains("Primary Request: earlier goal"));
        assert!(
            previous.from_injected,
            "must be flagged as injected for prefix stripping"
        );
    }

    #[test]
    fn chained_summary_head_excludes_anchor_and_ack() {
        // 缺陷 5.2 / AC5.1：链式重摘时，上一轮的锚点摘要 user 及其配对 ack assistant
        // 都要从摘要输入 head 剔除（同 summarize_history 的 filter），只留真正的早前消息。
        let old = vec![
            json!({ "role": "user", "content": format!("{SUMMARY_MARKER_PREFIX} 引导语：\nPREVIOUS_SUMMARY_BODY") }),
            json!({ "role": "assistant", "content": SUMMARY_ACK_TEXT }),
            json!({ "role": "user", "content": "REAL_EARLIER_MESSAGE" }),
        ];
        let head: Vec<Value> = old
            .iter()
            .filter(|m| !is_anchor_summary(m) && !is_summary_ack(m))
            .cloned()
            .collect();
        let serialized = serialize_for_summary(&head);
        assert!(
            !serialized.contains("PREVIOUS_SUMMARY_BODY"),
            "old anchor dropped"
        );
        assert!(!serialized.contains(SUMMARY_ACK_TEXT), "paired ack dropped");
        assert!(
            serialized.contains("REAL_EARLIER_MESSAGE"),
            "real message kept"
        );
    }

    #[test]
    fn extract_previous_summary_prefers_anchor_over_injected() {
        // 同 run 内既有注入摘要（system 前缀，过期 S1）又有锚点摘要（old_segment，本轮更新）
        // → 取锚点，避免回退到已过期的落盘 summary。
        let system_prefix = vec![json!({
            "role": "system",
            "content": format!("{PERSISTED_SUMMARY_PREFIX}\nSTALE injected summary")
        })];
        let old_segment = vec![
            json!({ "role": "user", "content": format!("{SUMMARY_MARKER_PREFIX} 引导语：\nFRESH anchor summary") }),
            json!({ "role": "assistant", "content": SUMMARY_ACK_TEXT }),
        ];
        let previous =
            extract_previous_summary(&system_prefix, &old_segment).expect("summary detected");
        assert!(previous.text.contains("FRESH anchor summary"));
        assert!(!previous.text.contains("STALE"));
        assert!(!previous.from_injected);
    }

    #[test]
    fn injected_summary_stripped_from_view_and_head() {
        // 注入摘要识别后：head 序列化不含其正文，压缩后视图的 system 前缀不再含注入消息。
        let injected = json!({
            "role": "system",
            "content": format!("{PERSISTED_SUMMARY_PREFIX}\nEARLIER_SUMMARY_BODY")
        });
        let system_prefix = vec![json!({ "role": "system", "content": "sys" }), injected];
        assert!(is_injected_summary(&system_prefix[1]));
        // 剔除后前缀只剩真正的 system prompt。
        let stripped: Vec<Value> = system_prefix
            .iter()
            .filter(|m| !is_injected_summary(m))
            .cloned()
            .collect();
        assert_eq!(stripped.len(), 1);
        assert_eq!(stripped[0]["content"], "sys");
    }

    #[test]
    fn accumulate_source_ids_unions_previous_and_new_range() {
        use crate::chat::types::ConversationContextState;
        let messages: Vec<ChatMessage> = ["m0", "m1", "m2", "m3", "m4"]
            .iter()
            .map(|id| chat_msg(id, "user", "x"))
            .collect();
        let mut conversation = test_conversation(messages);
        // 已有 S1 覆盖 m0..=m1（source_until = m1，ids = [m0, m1]）。
        conversation.context_state = ConversationContextState {
            summary: Some(ConversationContextSummary {
                id: "ctxsum_prev".to_string(),
                content: "prev".to_string(),
                source_message_ids: vec!["m0".to_string(), "m1".to_string()],
                source_until_message_id: "m1".to_string(),
                token_estimate_before: 0,
                token_estimate_after: 0,
                created_at: 0,
                provider_id: "p".to_string(),
                model: "m".to_string(),
                stale: false,
                file_ledger: None,
            }),
            ..Default::default()
        };
        // 新压缩覆盖到 m3 → ids = S1.ids ∪ (m2, m3)。
        let ids = accumulate_source_ids(&conversation, "m3");
        assert_eq!(ids, vec!["m0", "m1", "m2", "m3"]);
        // until_id 未找到 → 仅旧 ids。
        assert_eq!(
            accumulate_source_ids(&conversation, "nope"),
            vec!["m0", "m1"]
        );
    }

    #[test]
    fn accumulate_source_ids_no_previous_summary() {
        let messages: Vec<ChatMessage> = ["a", "b", "c"]
            .iter()
            .map(|id| chat_msg(id, "user", "x"))
            .collect();
        let conversation = test_conversation(messages);
        // 无旧 summary → 从头累积到 until_id。
        assert_eq!(accumulate_source_ids(&conversation, "b"), vec!["a", "b"]);
    }

    /// 构造带一个多答组（选中臂 A、排除臂 B）的会话，供批次 C 排除测试复用。
    fn conversation_with_excluded_arm() -> Conversation {
        let mut arm_a = chat_msg("a_sel", "assistant", "SELECTED_ARM_ANSWER");
        arm_a.group_id = Some("g1".to_string());
        let mut arm_b = chat_msg("b_excl", "assistant", "EXCLUDED_ARM_ANSWER");
        arm_b.group_id = Some("g1".to_string());
        let messages = vec![
            chat_msg("u1", "user", "question one"),
            arm_a,
            arm_b,
            chat_msg("u2", "user", "question two"),
            chat_msg("a2", "assistant", "second answer"),
        ];
        let mut conversation = test_conversation(messages);
        conversation
            .group_selections
            .insert("g1".to_string(), "a_sel".to_string());
        conversation
    }

    #[test]
    fn persisted_compaction_excludes_unselected_group_arms() {
        // 缺陷 3 / AC3.1：排除臂 B 不进 included、摘要序列化含 A 不含 B。
        let conversation = conversation_with_excluded_arm();
        let included = context_included_indices(&conversation, 0);
        assert!(included.contains(&1), "selected arm A kept");
        assert!(!included.contains(&2), "excluded arm B dropped");
        assert_eq!(included, vec![0, 1, 3, 4]);

        let old: Vec<&ChatMessage> = included
            .iter()
            .map(|&i| &conversation.messages[i])
            .collect();
        let serialized = serialize_chat_messages_for_summary(&old);
        assert!(serialized.contains("SELECTED_ARM_ANSWER"));
        assert!(!serialized.contains("EXCLUDED_ARM_ANSWER"));
    }

    #[test]
    fn source_ids_include_excluded_arms() {
        // 缺陷 3 / R3.2：被排除臂 id 仍计入 source_message_ids（被 boundary 覆盖、不再 replay）。
        let conversation = conversation_with_excluded_arm();
        let ids = accumulate_source_ids(&conversation, "a2");
        assert!(
            ids.contains(&"b_excl".to_string()),
            "excluded arm id covered"
        );
        assert_eq!(ids, vec!["u1", "a_sel", "b_excl", "u2", "a2"]);
    }

    #[test]
    fn context_included_indices_keeps_all_without_groups() {
        // AC3.3：无多答组 → 全量下标，行为不变。
        let messages: Vec<ChatMessage> = (0..4)
            .map(|i| {
                chat_msg(
                    &format!("m{i}"),
                    if i % 2 == 0 { "user" } else { "assistant" },
                    "x",
                )
            })
            .collect();
        let conversation = test_conversation(messages);
        assert_eq!(context_included_indices(&conversation, 0), vec![0, 1, 2, 3]);
        assert_eq!(context_included_indices(&conversation, 2), vec![2, 3]);
    }

    #[test]
    fn token_split_over_indices_matches_prefiltered_sequence() {
        // AC3.2：over_indices(included) 的 boundary == 「先物理滤除排除臂、连续切分」的
        // boundary（映射回原始下标）。构造大消息使切分真正发生，index 3 为排除臂。
        let big = "a".repeat(20_000); // ~5004 tok/条
        let messages: Vec<ChatMessage> = (0..6)
            .map(|i| {
                chat_msg(
                    &format!("m{i}"),
                    if i % 2 == 0 { "user" } else { "assistant" },
                    &big,
                )
            })
            .collect();
        // included 跳过 index 3。
        let included = vec![0usize, 1, 2, 4, 5];
        let via_indices =
            token_split_over_indices(&messages, &included, RECENT_KEEP_TOKENS).expect("split");

        // 参照：物理滤除 index 3 后连续切分，boundary 位置映射回原始下标。
        let filtered: Vec<ChatMessage> = included.iter().map(|&i| messages[i].clone()).collect();
        let pos = token_split_chat_messages(&filtered, 0, RECENT_KEEP_TOKENS).expect("split");
        assert_eq!(
            via_indices, included[pos],
            "boundary maps back to original index"
        );
        // 排除臂 (index 3) 绝不会成为 boundary。
        assert_ne!(via_indices, 3);
    }

    #[test]
    fn build_summary_prompt_carries_previous_summary_and_focus() {
        let content = build_summary_user_content(
            "[User]: hi\n\n[Assistant]: hello",
            Some("## Goal\nbuild X"),
            Some("focus on tests"),
            SummaryKind::History,
        );
        // 链式更新：走 UPDATE prompt + <previous-summary> 块（pi 语义）。
        assert!(content.contains("PRESERVE all existing information"));
        assert!(content.contains("<previous-summary>"));
        assert!(content.contains("## Goal\nbuild X"));
        // Focus 以 pi 的 Additional focus 形式追加。
        assert!(content.contains("Additional focus: focus on tests"));
        // 序列化历史包在 <conversation> 标签里（防止摘要模型续写对话）。
        assert!(content.contains("<conversation>\n[User]: hi"));
        assert!(content.contains("</conversation>"));
    }

    #[test]
    fn build_summary_prompt_fresh_has_no_previous_block() {
        let content = build_summary_user_content("[User]: hi", None, None, SummaryKind::History);
        // 首次摘要：走 SUMMARIZATION prompt，无 previous 块、无 focus。
        assert!(content.contains("Create a structured context checkpoint summary"));
        assert!(!content.contains("<previous-summary>"));
        assert!(!content.contains("Additional focus:"));
    }

    // 链式压缩的旧摘要绝不许丢（审查发现的数据丢失回归）：head 为空时历史段必须
    // 原文携带上一份摘要——它已被剔出视图，不带上就随整段替换永久消失。
    #[test]
    fn empty_history_carries_previous_summary_forward() {
        let previous = PreviousSummary {
            text: "## Goal\n重构认证模块".to_string(),
            from_injected: false,
        };
        assert_eq!(
            empty_history_fallback_text(Some(&previous)),
            "## Goal\n重构认证模块"
        );
        assert_eq!(empty_history_fallback_text(None), "No prior history.");
    }

    #[test]
    fn split_turn_prefix_extraction() {
        let user = |text: &str| json!({ "role": "user", "content": text });
        let assistant = |text: &str| json!({ "role": "assistant", "content": text });
        let tool = |text: &str| json!({ "role": "tool", "tool_call_id": "c", "content": text });

        // 近期窗口以 user 开头：一轮没被劈开，无前缀。
        let head = vec![user("q1"), assistant("a1")];
        let recent = vec![user("q2"), assistant("draft")];
        let (history, prefix) = split_history_turn_prefix(&head, &recent);
        assert_eq!(history.len(), 2);
        assert!(prefix.is_empty());

        // 近期窗口以 assistant 开头：切点劈开了 q2 那一轮——前缀从 q2 起到 head 末尾。
        let head = vec![
            user("q1"),
            assistant("a1"),
            user("q2"),
            assistant("work"),
            tool("r"),
        ];
        let recent = vec![assistant("more work"), tool("r2")];
        let (history, prefix) = split_history_turn_prefix(&head, &recent);
        assert_eq!(
            history.len(),
            2,
            "history ends before the split turn's user message"
        );
        assert_eq!(prefix.len(), 3, "prefix spans user q2 → cut point");
        assert_eq!(prefix[0]["content"], "q2");

        // head 里没有 user（找不到轮起点，对应 pi turnStartIndex == -1）：不切。
        let head = vec![assistant("a1"), tool("r")];
        let recent = vec![assistant("tail")];
        let (history, prefix) = split_history_turn_prefix(&head, &recent);
        assert_eq!(history.len(), 2);
        assert!(prefix.is_empty());
    }

    #[test]
    fn build_summary_prompt_turn_prefix_uses_prefix_prompt_only() {
        let content = build_summary_user_content(
            "[User]: do the thing\n\n[Assistant tool call]: read({\"path\":\"a.rs\"})",
            Some("ignored"),
            Some("ignored"),
            SummaryKind::TurnPrefix,
        );
        assert!(content.contains("This is the PREFIX of a turn"));
        // 前缀摘要不带 previous/focus（它只解释半轮，不承接链式更新）。
        assert!(!content.contains("<previous-summary>"));
        assert!(!content.contains("Additional focus:"));
    }

    #[test]
    fn summary_prompts_match_pi_structure() {
        // 对齐 pi coding-agent 的分节格式：首次/更新两份 prompt 都带同一组小节头。
        for header in [
            "## Goal",
            "## Constraints & Preferences",
            "## Progress",
            "### Done",
            "### In Progress",
            "### Blocked",
            "## Key Decisions",
            "## Next Steps",
            "## Critical Context",
        ] {
            assert!(
                SUMMARIZATION_PROMPT.contains(header),
                "summarization prompt missing section: {header}"
            );
            assert!(
                UPDATE_SUMMARIZATION_PROMPT.contains(header),
                "update prompt missing section: {header}"
            );
        }
        // 更新 prompt 的核心语义：保留旧信息 + In Progress→Done 迁移。
        assert!(UPDATE_SUMMARIZATION_PROMPT.contains("PRESERVE all existing information"));
        assert!(UPDATE_SUMMARIZATION_PROMPT
            .contains("move items from \"In Progress\" to \"Done\" when completed"));
        // system prompt 是 pi 的 context summarization assistant（含防续写约束）。
        assert!(SUMMARY_SYSTEM_PROMPT.starts_with("You are a context summarization assistant."));
        assert!(SUMMARY_SYSTEM_PROMPT.contains("Do NOT continue the conversation."));
    }

    #[test]
    fn extract_summary_text_prefers_summary_tag() {
        let resp = "<analysis>thinking…</analysis>\n<summary>\n1. Primary Request: X\n</summary>";
        assert_eq!(extract_summary_text(resp), "1. Primary Request: X");
        // No tag → whole response trimmed.
        assert_eq!(extract_summary_text("  just text  "), "just text");
    }

    #[test]
    fn extract_summary_text_handles_unclosed_and_missing_tags() {
        // Open tag without a closing tag → everything after the open tag, trimmed.
        assert_eq!(
            extract_summary_text("<analysis>x</analysis>\n<summary>\n1. Request: Y\n"),
            "1. Request: Y"
        );
        // Multiple <summary> tags: takes the first opening and the first closing
        // after it (greedy on the prefix is fine — first complete block wins).
        assert_eq!(
            extract_summary_text("<summary>first</summary>\n<summary>second</summary>"),
            "first"
        );
        // Empty content between tags collapses to empty (caller treats as failure).
        assert_eq!(extract_summary_text("<summary></summary>"), "");
    }

    #[test]
    fn recent_window_all_tool_results_yields_empty_old_segment() {
        // Pathological: after the system prefix the entire (small) tail is tool
        // results. The pair-protection walk would slide the boundary back to
        // system_end, so there is no old segment to summarize → callers degrade
        // gracefully (summarize_history returns None). Verify no orphan tool ends
        // up at the START of old_segment, and old is empty here.
        let mut messages = vec![json!({ "role": "system", "content": "sys" })];
        for i in 0..3 {
            messages
                .push(json!({ "role": "tool", "tool_call_id": format!("c{i}"), "content": "ok" }));
        }
        let (sys, old, recent) = select_recent_by_tokens(&messages, 8_000);
        assert_eq!(sys.len(), 1);
        assert!(
            old.is_empty(),
            "all-tool tail leaves nothing summarizable (old must be empty, was {old:?})"
        );
        // Whatever lands in old must never START with an orphan tool result.
        if let Some(first) = old.first() {
            assert!(!is_tool_result(first));
        }
        assert_eq!(recent.len(), 3);
    }

    #[test]
    fn summary_output_tokens_respects_model_cap() {
        // 大 config_max → 封顶到 SUMMARY_OUTPUT_TOKENS；小 config_max → 保留 min() 语义。
        assert_eq!(summary_output_tokens(100_000), SUMMARY_OUTPUT_TOKENS);
        assert_eq!(summary_output_tokens(1_000), 1_000);
    }

    #[test]
    fn replace_with_summary_keeps_role_alternation_legal() {
        let sys = vec![json!({ "role": "system", "content": "sys" })];
        let recent = vec![
            json!({ "role": "user", "content": "latest question" }),
            json!({ "role": "assistant", "content": "latest answer" }),
        ];
        let out = replace_with_summary(sys, "the summary", recent);
        let roles: Vec<&str> = out
            .iter()
            .map(|message| message["role"].as_str().unwrap())
            .collect();
        assert_eq!(
            roles,
            vec!["system", "user", "assistant", "user", "assistant"]
        );
        assert!(out[1]["content"].as_str().unwrap().contains("the summary"));
        // The inserted summary carries the anchor marker for future chained summaries.
        assert!(out[1]["content"]
            .as_str()
            .unwrap()
            .starts_with(SUMMARY_MARKER_PREFIX));
    }

    #[test]
    fn budget_ratio_halves_the_window() {
        // R1: summary input budget is window * 0.5.
        assert_eq!(SUMMARY_INPUT_BUDGET_RATIO, 0.5);
        let window = 128_000usize;
        let budget = ((window as f32) * SUMMARY_INPUT_BUDGET_RATIO) as usize;
        assert_eq!(budget, 64_000);
    }

    #[test]
    fn clip_keeps_small_serialized_unchanged() {
        // R5: a serialized old segment already under budget is returned verbatim.
        let serialized = "[User]: hi\n\n[Assistant]: hello there";
        let out = clip_serialized_to_budget(serialized, 10_000);
        assert_eq!(out, serialized);
        assert!(!out.contains("omitted to fit"));
    }

    #[test]
    fn clip_head_tail_fits_budget_and_keeps_both_ends() {
        // R2: a serialized old segment far exceeding the budget is clipped HEAD+TAIL
        // to fit, keeping a recognizable beginning and end with a middle marker.
        let head_marker = "BEGINNING_TASK_GOAL";
        let tail_marker = "MOST_RECENT_WORK";
        let mut serialized = String::new();
        serialized.push_str(head_marker);
        serialized.push(' ');
        serialized.push_str(&"filler ".repeat(50_000)); // ~350k chars
        serialized.push_str(tail_marker);

        let budget = 4_000usize;
        let clipped = clip_serialized_to_budget(&serialized, budget);

        // Hard guarantee: result fits the budget.
        assert!(
            estimate_tokens(&clipped) <= budget,
            "clipped est {} must be <= budget {budget}",
            estimate_tokens(&clipped)
        );
        // Both ends survive.
        assert!(clipped.contains(head_marker), "head must survive");
        assert!(clipped.contains(tail_marker), "tail must survive");
        // The omission marker is present in the middle.
        assert!(clipped.contains("older history omitted to fit"));
        // Tail bias: tail budget (~60%) >= head budget (~40%).
        let marker_pos = clipped.find("older history omitted").unwrap();
        let head_part_len = marker_pos;
        let tail_part_len = clipped.len() - (marker_pos + "older history omitted".len());
        assert!(
            tail_part_len >= head_part_len,
            "tail ({tail_part_len}) should keep at least as much as head ({head_part_len})"
        );
    }

    #[test]
    fn clip_with_unicode_fits_budget() {
        // Multi-byte (CJK) content costs ~1 token/char; clipping must still fit the budget.
        let serialized = "开头任务".to_string() + &"上下文".repeat(40_000) + "最近工作";
        let budget = 2_000usize;
        let clipped = clip_serialized_to_budget(&serialized, budget);
        assert!(
            estimate_tokens(&clipped) <= budget,
            "unicode clipped est {} must be <= budget {budget}",
            estimate_tokens(&clipped)
        );
        assert!(clipped.contains("开头任务"));
        assert!(clipped.contains("最近工作"));
    }

    #[test]
    fn budget_fallback_when_window_unknown() {
        // window == 0 → use the fallback budget constant (no panic, capping still applies).
        let serialized = "x".repeat(SUMMARY_INPUT_BUDGET_FALLBACK_TOKENS * 4 * 2);
        // Mirror summarize_history's budget calc for window == 0.
        let budget = SUMMARY_INPUT_BUDGET_FALLBACK_TOKENS;
        let clipped = clip_serialized_to_budget(&serialized, budget);
        assert!(estimate_tokens(&clipped) <= budget);
        assert!(SUMMARY_INPUT_BUDGET_FALLBACK_TOKENS > 0);
    }
}
