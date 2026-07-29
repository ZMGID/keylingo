use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::external_agents::slash::parse_slash_commands_from_init;
use crate::external_agents::stream::{usage_from_parts, CliUsageParts};
use crate::external_agents::types::UnifiedAgentEvent;

/// 从 claude 的一个 usage 对象里抽出四个分量。
///
/// `result.usage` 与 `stream_event → message_start → message.usage` 是同一形状，故共用此函数。
/// 实测 `result.usage` 键位（值可为 0，但键都在）：
/// `input_tokens` / `cache_creation_input_tokens` / `cache_read_input_tokens` / `output_tokens`。
///
/// 两个 cache 字段必须计入——缓存命中的 token 照样占上下文窗口，只是不重复计费。
/// 只读 input+output 会在长会话里低估一个数量级。
fn claude_usage_parts(usage: &Value) -> CliUsageParts {
    let field = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
    CliUsageParts {
        input: field("input_tokens"),
        output: field("output_tokens"),
        cache_read: field("cache_read_input_tokens"),
        cache_creation: field("cache_creation_input_tokens"),
        // Anthropic 口径：`input_tokens` 是**非缓存**部分，与两个 cache 字段不相交，
        // 三者相加才是全量输入。与内置路径 `context_estimate::anchor_total_tokens`
        // 的 `anthropic_messages` 分支同口径。（codex 相反，cache ⊆ input，别照抄。）
        cache_included_in_input: false,
        ..Default::default()
    }
}

fn usage_parts_all_zero(parts: &CliUsageParts) -> bool {
    parts.input == 0 && parts.output == 0 && parts.cache_read == 0 && parts.cache_creation == 0
}

/// 从 `result.usage` 里选出代表**当前上下文占用**的那份快照。
///
/// `usage.iterations[]` 是一轮内多次 LLM 往返的序列，每一项都是**独立快照**（不是增量）：
/// 当前上下文占用 = **末项**。累加各项得到的是本轮的计费总量，不是窗口占用，
/// 用它当分子会让进度条持续虚高。取首项则会漏掉本轮后续往返累积的上下文。
///
/// **顶层回退带闸门**（`completed_result_turns == 0`，即本会话的第一个 `result`）：
/// `usage` 顶层是本轮的**计费总量**（= 各 iteration 之和）。第一轮只有一次往返时它恰好
/// 等于上下文占用；从第二轮起（同一进程内）它既不是快照、也漏掉前面轮次已在窗口里的历史，
/// 拿来当分子会持续虚高。实测 `iterations` 在**不调工具的轮次里恒为 `[]`**
/// （claude 2.1.220），所以这个回退很常走 —— 正因为常走，才必须只在第一轮放行。
/// 闸门关上时返回 `None`：那一轮的真实占用由 `message_start.message.usage` 上报
/// （服务端算的、每次请求都有），比顶层计费总量准。
///
/// 注：B1（一个会话一个常驻进程）落地后这个闸门**真正开始生效** —— 同一个
/// `ClaudeStreamState` 跨轮存活（`session/claude_stream.rs` 持有它），计数会真的往上涨。
/// 也正因如此，`--include-partial-messages` 必须**始终开着**（`run.rs` 传
/// `include_partial_messages: true`）：第 2 轮起 `iterations` 恒为 `[]` + 顶层被闸门拦住
/// ⇒ 分子**完全依赖** `message_start.message.usage`，关掉那个 flag 用量条就没有分子了。
fn claude_result_usage_snapshot(usage: &Value, completed_result_turns: u32) -> Option<&Value> {
    if let Some(last) = usage
        .get("iterations")
        .and_then(|v| v.as_array())
        .and_then(|items| items.iter().rev().find(|item| item.is_object()))
    {
        return Some(last);
    }
    (completed_result_turns == 0).then_some(usage)
}

/// 从 `result.modelUsage` 取**当前模型的上下文窗口**（分母的最高优先级来源）。
///
/// 形状（claude 2.1.220 本机实测原文）：
/// ```json
/// "modelUsage": { "claude-opus-4-8[1M]": {
///   "inputTokens": 7037, "outputTokens": 4, "cacheReadInputTokens": 31792,
///   "cacheCreationInputTokens": 182, "webSearchRequests": 0, "costUSD": 0.0523,
///   "contextWindow": 1000000, "maxOutputTokens": 64000,
///   "canonicalModel": "claude-opus-4-8", "provider": "firstParty" } }
/// ```
///
/// **为什么必须读它**：CLI 给 `contextWindow` 赋值时依次考虑
/// `CLAUDE_CODE_MAX_CONTEXT_TOKENS` 环境覆盖、`context-1m-2025-08-07` beta、per-model 表 ——
/// 任何外部静态表都复现不了（尤其 env 覆盖与第三方 router 模型）。这是 CLI 唯一权威的分母。
///
/// **两条实测红线**：
/// - `contextWindow` **可以是 0**（本机 `~/.claude/stats-cache.json` 里 8 个模型全是 0），
///   必须 `> 0` 才采用；
/// - entry 里的 **token 数是进程累计**（CLI 内部 `+=`），只有 `contextWindow` /
///   `maxOutputTokens` 是静态属性。实测同一轮 `usage.input_tokens=2` 而
///   `modelUsage.inputTokens=7041`（三轮累加）—— 绝不能拿它当分子。
///
/// 取值优先级：按当前 resolved model **精确匹配** key（`system/init` 的 `model` 实测就是
/// 带 `[1M]` 后缀的那个形态）→ 匹配 entry 的 `canonicalModel`（不带后缀）→ 退「取最大值」。
/// 为什么不能直接取最大：`modelUsage` 是**会话累计 map**，会话里换过模型、或跑过用 haiku 的
/// 子 agent 时，最大值可能是历史上那个更大的窗口而不是当前模型的。
///
/// 被中断的轮次 `modelUsage` 实测为 `{}` —— 一律当「可能缺失」处理，返回 `None`。
fn context_window_from_model_usage(model_usage: &Value, resolved_model: Option<&str>) -> Option<u64> {
    let entries = model_usage.as_object()?;
    let window_of = |entry: &Value| {
        entry
            .get("contextWindow")
            .and_then(|v| v.as_u64())
            .filter(|value| *value > 0)
    };
    if let Some(model) = resolved_model.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(window) = entries.get(model).and_then(window_of) {
            return Some(window);
        }
        if let Some(window) = entries
            .values()
            .filter(|entry| {
                entry.get("canonicalModel").and_then(|v| v.as_str()) == Some(model)
            })
            .find_map(window_of)
        {
            return Some(window);
        }
    }
    entries.values().filter_map(window_of).max()
}

/// 四种 `result` 失败子型（官方 `SDKResultError.subtype`）→ 中文原因。
///
/// 返回 `Some` 即「这个 subtype 本身就意味着失败」——成员判定与文案共用这一张表，
/// 避免出现「列表里加了子型但忘了给文案」的分叉。
fn result_error_subtype_reason(subtype: &str) -> Option<&'static str> {
    match subtype {
        "error_during_execution" => Some("执行过程中出错"),
        "error_max_turns" => Some("达到最大轮次上限"),
        "error_max_budget_usd" => Some("达到预算上限"),
        "error_max_structured_output_retries" => Some("结构化输出重试次数耗尽"),
        _ => None,
    }
}

/// 判断一条 `result` 是否代表失败，若是则给出**交给 `errors::classify` 的原始文案**。
///
/// 两个判据都要看，缺一不可：
/// - `subtype` ∈ 四种 `error_*`（官方 `SDKResultError`）；
/// - `is_error == true`，**即使 `subtype` 是 `success`**。本机未登录时的真实样本正是
///   `{"type":"result","subtype":"success","is_error":true,`
///   `"result":"Not logged in · Please run /login"}`——只判 subtype 会把它当成功，
///   这一轮于是被标记为「已完成」，用户只拿到一句裸英文、没有任何可操作提示。
///
/// 文案优先级 `errors[]` > `result`：前者是 error 子型独有的结构化原因列表，后者是
/// success 子型带 `is_error` 时错误文案的落点。两者都缺才用 subtype 兜底（仍带上
/// subtype，`<details>` 里能看出到底是哪种失败）。
fn claude_result_error_message(obj: &serde_json::Map<String, Value>) -> Option<String> {
    let subtype = obj.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
    let is_error = obj
        .get("is_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let subtype_reason = result_error_subtype_reason(subtype);
    if !is_error && subtype_reason.is_none() {
        return None;
    }

    let joined_errors = obj
        .get("errors")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("; ")
        })
        .filter(|joined| !joined.is_empty());
    if let Some(joined) = joined_errors {
        return Some(joined);
    }

    let result_text = obj
        .get("result")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(text) = result_text {
        return Some(text.to_string());
    }

    Some(match subtype_reason {
        Some(reason) => format!("claude 本轮失败：{reason}（subtype={subtype}）"),
        None => format!("claude 本轮报告失败（subtype={subtype}，is_error=true）"),
    })
}

/// 这条 `result` 是不是**用户中断**造成的收尾（而不是真的失败）。
///
/// **协议事实**（claude 2.1.220 本机实测，见 `session/claude_persist_probe_tests.rs::
/// claude_interrupt_ends_the_round_but_not_the_process`）：往 stdin 写
/// `{"type":"control_request","request":{"subtype":"interrupt"}}` 之后，被打断的那一轮
/// **仍然吐一条 `result`**，形态是
/// `is_error: true` / `subtype: "error_during_execution"` / `terminal_reason:
/// "aborted_streaming"` / `result: null` / `modelUsage: {}` / `errors: ["[ede_diagnostic] …"]`。
///
/// 常驻会话下用户每点一次「停止」都会走到这里，若不识别就会在「已取消」之后再叠一个
/// 假错误气泡（且把 `stream_outcome` 从 cancelled 翻成 error）。
///
/// 判据**只认 `terminal_reason`** 这个明确的机器码：它是在 `claude_result_error_message`
/// **之前**的一道豁免，而不是削弱后者的 `is_error` 判据（spec 第 20 条——未登录的真实样本
/// 正是 `subtype:"success"` + `is_error:true`，那条判据必须保留）。
pub fn result_is_user_abort(value: &Value) -> bool {
    value
        .get("terminal_reason")
        .and_then(|v| v.as_str())
        .map(str::trim)
        == Some("aborted_streaming")
}

/// `result.permission_denials[]` → 一条 markdown 引用块提示。
///
/// 被权限规则拒掉的工具调用在流里**没有** tool_use/tool_result 帧，不接的话对用户完全
/// 不可见（"CLI 好像什么都没做"）。走 `TextDelta` 而非新增事件变体：同一条提示模式
/// 已被 `run.rs::CONTEXT_RESET_NOTICE` 用过，无需改动任何前端契约。
fn permission_denials_note(obj: &serde_json::Map<String, Value>) -> Option<String> {
    let denials = obj.get("permission_denials")?.as_array()?;
    if denials.is_empty() {
        return None;
    }
    // 同名工具被连续拒多次只列一次——提示要的是「哪些工具被拦了」，不是流水账。
    let mut names: Vec<&str> = Vec::new();
    for denial in denials {
        if let Some(name) = denial
            .get("tool_name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    Some(if names.is_empty() {
        // 结构变形（拿不到 tool_name）时也不能把「有调用被拒」这件事整个丢掉。
        format!(
            "> ⚠️ 有 {} 个工具调用被权限规则拒绝，未执行。\n\n",
            denials.len()
        )
    } else {
        format!(
            "> ⚠️ 以下工具调用被权限规则拒绝，未执行：{}\n\n",
            names.join("、")
        )
    })
}

/// `system/task_notification`（官方 `SDKTaskNotificationMessage`）→ 提示文案。
///
/// 只对 `failed` / `stopped` 产出：后台任务**失败**在流里没有别的痕迹，不提示等于静默丢失；
/// `completed` 的产出由该任务自己的 tool_result 承载，再发一条只是重复。
fn task_notification_note(obj: &serde_json::Map<String, Value>) -> Option<String> {
    let label = match obj.get("status").and_then(|v| v.as_str()) {
        Some("failed") => "失败",
        Some("stopped") => "被中止",
        _ => return None,
    };
    let summary = obj
        .get("summary")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    Some(match summary {
        Some(text) => format!("> ⚠️ 后台任务{label}：{text}\n\n"),
        None => format!("> ⚠️ 后台任务{label}。\n\n"),
    })
}

/// `assistant` 帧上的 `error` 值 → (是否算「本轮失败」, 提示文案)。
///
/// **协议事实**（claude 2.1.220 反查二进制核实）：`error` 与 `aborted` 挂在 assistant 帧的
/// **顶层**（与 `message` 平级），构造处形如
/// `{type:"assistant", message, ..., error: n.error, ...isAbortedMidStream && {aborted:!0}}`。
/// 取值集合：`authentication_failed` / `oauth_org_not_allowed` / `billing_error` /
/// `rate_limit` / `overloaded` / `invalid_request` / `model_not_found` / `server_error` /
/// `unknown` / `max_output_tokens`。
///
/// **为什么必须接**：`max_output_tokens` 最典型 —— 回答在**中途硬截断**，而 `result` 那边
/// 可能仍是成功（错误只写在 assistant 帧上）。不接的话，一段被砍断的回答被标成「完成」，
/// 用户不知道为什么答案说到一半就停了。
///
/// 返回的第一项决定出口：`true` 走 `UnifiedAgentEvent::Error`（交给 `errors::classify`
/// 给可操作中文，spec 第 5 条），`false` 走 `TextDelta` 提示（回答本身仍然有效，只是不完整）。
fn assistant_error_report(kind: &str) -> Option<(bool, String)> {
    let note = |text: &str| Some((false, format!("> ⚠️ {text}\n\n")));
    match kind {
        // 截断 / 中止：正文有效但不完整，用提示而不是错误——把这一轮判成失败会连带
        // 丢掉已经流出来的半截回答。
        "max_output_tokens" => note("回答达到模型的输出上限被截断，内容可能不完整。"),
        "aborted" => note("本轮回答被中止。"),
        // 认证 / 计费 / 上游故障：交给 errors::classify 出可操作中文（Auth 附登录命令）。
        "authentication_failed" | "oauth_org_not_allowed" | "billing_error" | "rate_limit"
        | "overloaded" | "invalid_request" | "model_not_found" | "server_error" => {
            Some((true, format!("claude 报告错误：{kind}")))
        }
        // `unknown` 与未来新增值：仍要可见（静默是最坏的），但不足以判定整轮失败。
        "" => None,
        other => note(&format!("claude 报告了一个错误：{other}。")),
    }
}

/// `system/status` 的压缩终态（`compact_result` / `compact_error`）→ 提示文案。
///
/// **协议事实**（claude 2.1.220 反查二进制的 zod schema）：
/// `{type:"system", subtype:"status", status, permissionMode?, compact_result:"success"|"failed",
/// compact_error?:string}`。
///
/// **为什么必须接**：压缩**失败**时 claude 发的是带 `compact_result:"failed"` 的 status，
/// **不会**发 `compact_boundary`。我们在 `status:"compacting"` 时插的
/// 「⏳ 正在压缩上下文…」于是永远停在那里，用户以为还在跑。
fn compact_status_note(obj: &serde_json::Map<String, Value>) -> Option<String> {
    match obj.get("compact_result").and_then(|v| v.as_str())? {
        "failed" => {
            let reason = obj
                .get("compact_error")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            Some(match reason {
                Some(text) => format!("> ⚠️ claude 压缩上下文失败：{text}\n\n"),
                None => "> ⚠️ claude 压缩上下文失败。\n\n".to_string(),
            })
        }
        "success" => Some("> ✅ claude 已完成上下文压缩。\n\n".to_string()),
        // 未来新增取值：安全忽略（spec 第 10 条）。
        _ => None,
    }
}

struct PendingContentBlock {
    block_type: String,
    id: Option<String>,
    name: Option<String>,
    input_json: String,
    input_value: Option<Value>,
}

#[derive(Default)]
pub struct ClaudeStreamState {
    text_streamed: bool,
    /// **per-turn**：本轮是否已经发出过任何「正文」（模型回答 / 本地命令输出）。
    ///
    /// 与 `text_streamed` **不是一回事**，别复用（spec 第 3 条）：后者是 **per-message** 的
    /// 「这条消息已流式发过」标志，在 `message_start` 复位；本标志跨消息累积、在 `result`
    /// （轮次边界）复位，用来判断「本轮到底有没有给用户任何正文」——A4 的 `result.result`
    /// 兜底就靠它，否则一轮里流过正文之后再兜底一次会把回答重复一遍。
    ///
    /// 只有真正的正文置位；系统提示（压缩中 / 权限被拒 / 任务失败等 blockquote）不算，
    /// 否则一条「正在压缩」提示就会把 `/cost` 的报告吞掉。
    any_text_emitted: bool,
    /// 本会话已经收完的 `result` 轮次数。给 `claude_result_usage_snapshot` 的顶层回退开闸门。
    completed_result_turns: u32,
    /// 最近一个 `result` 是否是**用户中断**的收尾（`terminal_reason == "aborted_streaming"`）。
    /// 常驻会话据此把这一轮送去「已取消」出口而不是错误出口（见 `result_is_user_abort`）。
    last_result_aborted: bool,
    /// `system/init` 报的 resolved model（如 `claude-opus-4-8[1M]`）。用于在 `modelUsage`
    /// 这张**会话累计 map** 里精确定位当前模型的 `contextWindow`。
    ///
    /// 注意：`system/init` 实测**每轮都会发**（不是只在开头），所以这里是「最近一次」
    /// 而不是「首轮」——正好是需要的语义（会话中途换模型也能跟上）。
    resolved_model: Option<String>,
    /// 本轮已经报过的 assistant 错误值，用于去重：同一次失败常在多条 assistant 帧上
    /// 重复出现，且 `result` 往往还会再报一次。
    reported_assistant_errors: HashSet<String>,
    current_message_id: Option<String>,
    blocks: HashMap<String, PendingContentBlock>,
    streamed_tool_use_ids: HashSet<String>,
}

impl ClaudeStreamState {
    /// 发一段**正文**（模型回答 / 本地命令输出），并置位 per-turn 的 `any_text_emitted`。
    /// 系统提示类的 blockquote 不要走这里——它们不该抑制 A4 的 `result.result` 兜底。
    fn emit_text(&mut self, sink: &mut dyn FnMut(UnifiedAgentEvent), delta: String) {
        if delta.is_empty() {
            return;
        }
        self.any_text_emitted = true;
        sink(UnifiedAgentEvent::TextDelta { delta });
    }

    /// 本会话已收完的 `result` 轮次数（>0 即读到过协议层完成标志）。
    pub fn completed_result_turns(&self) -> u32 {
        self.completed_result_turns
    }

    /// 最近一个 `result` 是否是用户中断的收尾。常驻会话在读到轮次边界后立刻查询它。
    pub fn last_result_aborted(&self) -> bool {
        self.last_result_aborted
    }

    pub fn handle_value(&mut self, value: &Value, sink: &mut dyn FnMut(UnifiedAgentEvent)) {        let obj = match value.as_object() {
            Some(o) => o,
            None => return,
        };
        let kind = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "system" => {
                match obj.get("subtype").and_then(|v| v.as_str()) {
                    Some("init") => {
                        // resolved model 供 `modelUsage` 精确定位当前模型的窗口（A8）。
                        // init 每轮都发，这里存「最近一次」而非「首轮」。
                        if let Some(model) = obj
                            .get("model")
                            .and_then(|v| v.as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                        {
                            self.resolved_model = Some(model.to_string());
                        }
                        let commands = parse_slash_commands_from_init(value);
                        if !commands.is_empty() {
                            sink(UnifiedAgentEvent::SlashCommands { commands });
                        }
                    }
                    // claude 自己触发的上下文压缩。`compact_metadata` 在 claude 2.1.220 里是
                    // `{ trigger, pre_tokens, post_tokens?, cumulative_dropped_tokens?,
                    //    duration_ms?, user_context?, messages_summarized? }`
                    // （反查二进制的构造处：`{trigger, pre_tokens, ...postTokens!==void 0 &&
                    // {post_tokens}, ...cumulativeDroppedTokens!==void 0 && {...}}`）。
                    //
                    // 这里曾写着「官方 SDK 类型只有 trigger 与 pre_tokens，没有 post_tokens，
                    // 别去读一个不存在的字段」——**那条注释是错的**，照着走就永远不会去查，
                    // 于是前端分隔线上的「→ N」永远显示不出来。
                    // 三个数字都是可选的：缺失时留 `None`，压缩这件事本身照发。
                    Some("compact_boundary") => {
                        let metadata = obj.get("compact_metadata");
                        let number = |key: &str| {
                            metadata
                                .and_then(|m| m.get(key))
                                .and_then(|v| v.as_u64())
                        };
                        let trigger = metadata
                            .and_then(|m| m.get("trigger"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("auto")
                            .to_string();
                        sink(UnifiedAgentEvent::CliCompacted {
                            trigger,
                            pre_tokens: number("pre_tokens"),
                            post_tokens: number("post_tokens"),
                            dropped_tokens: number("cumulative_dropped_tokens"),
                            duration_ms: number("duration_ms"),
                        });
                    }
                    // `status`（官方 `SDKStatusMessage`）：claude 2.1.220 的 zod schema 是
                    // `{status, permissionMode?, compact_result?:"success"|"failed", compact_error?}`，
                    // `SDKStatus` 含 `compacting` / `requesting` / `null`。
                    // 仍**按开放字符串处理、不做白名单**——这是对未来新增取值的防御，
                    // 而不是（此前注释所称的）「`requesting` 不在官方类型里」：2.1.220 的类型里有它。
                    //
                    // 三种情况转成事件：
                    //   - `compacting` —— 压缩**开始**端（`compact_boundary` 是结束端），
                    //     长压缩里唯一的可见信号。用 `CliCompacted` 会在前端插入一条假的分隔线
                    //     （压缩尚未完成），所以走 `TextDelta`，不新增事件变体。
                    //   - `compact_result: failed` —— 压缩失败时 claude **不发**
                    //     `compact_boundary`，不接就会让「⏳ 正在压缩…」永远停住。
                    //   - `compact_result: success` —— 与 boundary 互为佐证，收尾提示。
                    // 其余 status 值是纯进度噪音（一轮内多条），无展示位，显式忽略。
                    Some("status") => {
                        if let Some(note) = compact_status_note(obj) {
                            sink(UnifiedAgentEvent::TextDelta { delta: note });
                        } else if obj.get("status").and_then(|v| v.as_str()) == Some("compacting") {
                            sink(UnifiedAgentEvent::TextDelta {
                                delta: "> ⏳ claude 正在压缩上下文…\n\n".to_string(),
                            });
                        }
                    }
                    // 客户端执行的斜杠命令（`/cost` `/usage` `/context` `/status` `/doctor`）的
                    // 输出通道之一。官方 schema：
                    // `{type:"system", subtype:"local_command_output", content:string}`
                    // （zod 描述原文："Output from a local slash command (e.g. /voice, /usage).
                    // Displayed as assistant-style text in the transcript."）。
                    //
                    // 这类命令**没有模型往返**，报告正文只在这里和 `result.result` 里。
                    // 两条都不接的话，用户点了 `/cost` 看到的是 run.rs 的兜底
                    // 「claude 命令已执行」——报告内容 100% 丢失。而我们是**主动**把 claude
                    // 的斜杠命令列表暴露给用户的（`slash.rs` 从 init 解析），这条路一定会被走到。
                    Some("local_command_output") => {
                        if let Some(text) = obj
                            .get("content")
                            .and_then(|v| v.as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                        {
                            self.emit_text(sink, format!("{text}\n"));
                        }
                    }
                    // 后台任务终态（官方 `SDKTaskNotificationMessage`）。
                    Some("task_notification") => {
                        if let Some(note) = task_notification_note(obj) {
                            sink(UnifiedAgentEvent::TextDelta { delta: note });
                        }
                    }
                    // ---- 以下 subtype **有意不接**（不是漏了）----
                    //
                    // `hook_started` / `hook_progress` / `hook_response`：hook 是**用户自己**
                    // 在 claude 侧配的钩子，与本轮回答无关。实测一次最简调用（未登录、零工具）
                    // 就有 4 条，且 `hook_response.stdout` 会把整个 SessionStart 注入内容
                    // （本机实测 ~4KB 提示词全文）搬进流里——落进气泡就是刷屏。
                    // hook **失败**也无需在这里补：`outcome: "error"` 时 claude 自己会在
                    // 后续帧里反映影响，而 Kivio 无法对别人的 hook 做任何有意义的处置。
                    //
                    // `files_persisted`（`SDKFilesPersistedEvent`）：SDK 的文件上传通道产物，
                    // Kivio 走本地 cwd + `--add-dir`，不用该通道，恒不出现。
                    //
                    // 未知 / 未来新增 subtype 一律安全忽略：不 panic、不中断流（spec 第 10 条）。
                    _ => {}
                }
            }
            "stream_event" => {
                if let Some(event) = obj.get("event").and_then(|v| v.as_object()) {
                    self.handle_stream_event(event, sink);
                }
            }
            "assistant" => {
                // A5：`error` / `aborted` 挂在**帧的顶层**（与 `message` 平级），不在
                // `message` 里面。先处理它们——`max_output_tokens` 这类会让下面的正文
                // 只是**半截回答**，提示必须跟着这一条一起出去。
                let error_kind = obj
                    .get("error")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .or_else(|| {
                        (obj.get("aborted").and_then(|v| v.as_bool()) == Some(true))
                            .then(|| "aborted".to_string())
                    });
                if let Some(kind) = error_kind {
                    // 同一次失败常在多条 assistant 帧上重复出现，且 result 往往还会再报一次。
                    if self.reported_assistant_errors.insert(kind.clone()) {
                        if let Some((fatal, message)) = assistant_error_report(&kind) {
                            if fatal {
                                sink(UnifiedAgentEvent::Error { message });
                            } else {
                                // 提示不算正文：不置位 any_text_emitted，别抑制 A4 的兜底。
                                sink(UnifiedAgentEvent::TextDelta { delta: message });
                            }
                        }
                    }
                }
                if let Some(message) = obj.get("message").and_then(|v| v.as_object()) {
                    if let Some(content) = message.get("content").and_then(|v| v.as_array()) {
                        for block in content {
                            let block = match block.as_object() {
                                Some(b) => b,
                                None => continue,
                            };
                            match block.get("type").and_then(|v| v.as_str()) {
                                Some("text") => {
                                    if !self.text_streamed {
                                        if let Some(text) =
                                            block.get("text").and_then(|v| v.as_str())
                                        {
                                            self.emit_text(sink, text.to_string());
                                        }
                                    }
                                }
                                Some("tool_use") => {
                                    let id = block
                                        .get("id")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("tool")
                                        .to_string();
                                    if self.streamed_tool_use_ids.contains(&id) {
                                        continue;
                                    }
                                    self.streamed_tool_use_ids.insert(id.clone());
                                    sink(UnifiedAgentEvent::ToolUse {
                                        id,
                                        name: block
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("tool")
                                            .to_string(),
                                        input: block.get("input").cloned().unwrap_or(Value::Null),
                                    });
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            "user" => {
                if let Some(message) = obj.get("message").and_then(|v| v.as_object()) {
                    if let Some(content) = message.get("content").and_then(|v| v.as_array()) {
                        for block in content {
                            let block = match block.as_object() {
                                Some(b) => b,
                                None => continue,
                            };
                            if block.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                                sink(UnifiedAgentEvent::ToolResult {
                                    tool_use_id: block
                                        .get("tool_use_id")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    content: block
                                        .get("content")
                                        .map(|v| {
                                            if let Some(s) = v.as_str() {
                                                s.to_string()
                                            } else {
                                                v.to_string()
                                            }
                                        })
                                        .unwrap_or_default(),
                                    is_error: block
                                        .get("is_error")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false),
                                });
                            }
                        }
                    }
                }
            }
            "result" => {
                // 用户中断的收尾必须**先**豁免：被打断的轮次同样带 `is_error: true`
                // （见 `result_is_user_abort` 的实测形态），不豁免就会在「已取消」之后
                // 再叠一个假错误气泡。豁免只看 `terminal_reason`，不削弱 spec 第 20 条。
                let aborted = result_is_user_abort(value);
                self.last_result_aborted = aborted;
                // usage 解析**先于**错误分支——失败轮次的用量同样要计入用量条。
                //
                // 分子：`iterations` 末项（多次往返的独立快照），顶层回退带「仅第一个 result」
                // 闸门（见 `claude_result_usage_snapshot`）。
                // 分母：`modelUsage[当前模型].contextWindow`（CLI 唯一权威的窗口来源，A8）。
                let context_window = obj
                    .get("modelUsage")
                    .and_then(|mu| context_window_from_model_usage(mu, self.resolved_model.as_deref()));
                let mut parts = obj
                    .get("usage")
                    .and_then(|usage| {
                        claude_result_usage_snapshot(usage, self.completed_result_turns)
                    })
                    .map(claude_usage_parts)
                    .unwrap_or_default();
                parts.context_window = context_window;
                // **全零守卫**：没有 LLM 往返的轮次（未登录 / `/help` / 未知斜杠命令 /
                // 我们自己发的 `/compact`）返回 `iterations: []` + 顶层四字段全 0。
                // 不守就会产出 `total_tokens: Some(0)` 并落盘到那条 assistant 消息上，
                // 而 `context.rs` 挑「最近一条带 usage 的消息」用的是 `is_some()` 判据
                // ——`Some(0)` 会命中 ⇒ 用量条从 47K 掉到 0，直到下一轮真实回复才恢复。
                //
                // 窗口是**静态属性**，即使本轮零用量也值得上报（它是分母、与分子无关），
                // 所以「全零但拿到窗口」仍然发；分子不被清零由 `run.rs::merge_cli_usage`
                // 的全零不覆盖规则保证。
                if !usage_parts_all_zero(&parts) || parts.context_window.is_some() {
                    sink(UnifiedAgentEvent::Usage {
                        usage: usage_from_parts(parts),
                    });
                }
                // 被权限规则拒掉的工具调用（R4）：无论本轮成功与否都提示。
                if let Some(note) = permission_denials_note(obj) {
                    sink(UnifiedAgentEvent::TextDelta { delta: note });
                }
                // R1：`subtype` 与 `is_error` 任一指示失败都要产出 Error。裸文案交给
                // `run.rs` 的 `errors::classify`（spec 第 5 条）——它会把 "Not logged in"
                // 归成 Auth 并附上 `claude /login`，而不是把英文原句直接落进气泡。
                let error_message = if aborted {
                    None
                } else {
                    claude_result_error_message(obj)
                };
                // A4：本轮成功但**没有模型输出**（客户端执行的斜杠命令：`/cost` `/usage`
                // `/context` `/status` `/doctor`）时，报告正文只在 `result` 字段里。
                // 三条判据同时成立才兜底，否则会把正常回答重复一遍：
                //   成功 + `usage.output_tokens == 0` + 本轮还没发过任何正文。
                if error_message.is_none() && !self.any_text_emitted {
                    let output_tokens = obj
                        .get("usage")
                        .and_then(|u| u.get("output_tokens"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    if output_tokens == 0 {
                        if let Some(text) = obj
                            .get("result")
                            .and_then(|v| v.as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                        {
                            self.emit_text(sink, text.to_string());
                        }
                    }
                }
                if let Some(message) = error_message {
                    sink(UnifiedAgentEvent::Error { message });
                }
                // 轮次边界：`result` 之后进入下一轮（常驻进程下同一个 state 会跨轮存活）。
                // per-turn 状态在这里复位，per-session 计数在这里递增。
                self.completed_result_turns = self.completed_result_turns.saturating_add(1);
                self.any_text_emitted = false;
                self.reported_assistant_errors.clear();
            }
            "error" => {
                sink(UnifiedAgentEvent::Error {
                    message: obj
                        .get("error")
                        .and_then(|v| v.as_str())
                        .or_else(|| obj.get("message").and_then(|v| v.as_str()))
                        .unwrap_or("unknown error")
                        .to_string(),
                });
            }
            // ---- 以下顶层 type **有意不接**（不是漏了）----
            //
            // `tool_progress`（`SDKToolProgressMessage`：tool_use_id / tool_name /
            // elapsed_time_seconds）：长工具的进度心跳，但**前端没有承载位**。外部 CLI 的工具卡
            // 走 `ToolCallBlock.tsx` 的 `DefaultToolCallBlock`，它整个不渲染耗时
            // （`getDuration` 只被 SubAgent/Advisor/Knowledge/Python 四张专用卡调用），
            // 运行中状态已由 `status: Running` 的 shimmer 表达。而 `ToolCallRecord` 上没有
            // 「进度」字段，要让这个数字可见就得**新增 `UnifiedAgentEvent` 变体**
            // ——那会牵动全部 CLI 的 match（上一轮加 `CliCompacted` 动了 3 处），
            // 换来的是零可见收益。等前端真有「工具已运行 Ns」的位置再接。
            //
            // `tool_use_summary`（`SDKToolUseSummaryMessage`：summary /
            // preceding_tool_use_ids）：同样无承载位，且**没有无损的映射**——复用
            // `ToolResult` 会把该工具的真实结果覆盖成摘要、并把失败的工具改判成 Success；
            // 走 `TextDelta` 则把它混进回答正文（它不是模型的回答）。两种都比不接更糟。
            //
            // `auth_status` / `keep_alive`：SDK 的 WebSocket/交互式登录通道产物，
            // Kivio 走一次性子进程 + stdio，实测不出现。
            //
            // 未知 / 未来新增 type 一律安全忽略：不 panic、不中断流（spec 第 10 条）。
            _ => {}
        }
    }

    fn block_key(&self, index: &Value) -> String {
        format!(
            "{}:{}",
            self.current_message_id.as_deref().unwrap_or("anon"),
            index.as_u64().unwrap_or(0)
        )
    }

    fn handle_stream_event(
        &mut self,
        event: &serde_json::Map<String, Value>,
        sink: &mut dyn FnMut(UnifiedAgentEvent),
    ) {
        let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match event_type {
            "message_start" => {
                // 新 assistant 消息开始:复位 `text_streamed`(N7)。否则上一条消息经 delta
                // 流式发出后此标志一直为真,导致后续整块交付的 assistant 消息正文被永久跳过。
                self.text_streamed = false;
                self.current_message_id = event
                    .get("message")
                    .and_then(|v| v.get("id"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                // `message.usage` 是服务端算出的本次请求真实上下文占用（系统提示 + 工具定义 +
                // 全历史 + cache），且在**回答开始前**就到——用它让用量条在生成过程中就准确,
                // 而不是等 turn 结束的 `result`。一轮内会多次 message_start,
                // `run.rs` 后到覆盖先到 = 取最新快照,正是所需语义。
                if let Some(usage) = event.get("message").and_then(|v| v.get("usage")) {
                    let parts = claude_usage_parts(usage);
                    if !usage_parts_all_zero(&parts) {
                        sink(UnifiedAgentEvent::Usage {
                            usage: usage_from_parts(parts),
                        });
                    }
                }
            }
            "content_block_start" => {
                let Some(block) = event.get("content_block").and_then(|v| v.as_object()) else {
                    return;
                };
                let block_type = block
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let key = self.block_key(event.get("index").unwrap_or(&Value::Null));
                self.blocks.insert(
                    key,
                    PendingContentBlock {
                        block_type,
                        id: block.get("id").and_then(|v| v.as_str()).map(str::to_string),
                        name: block
                            .get("name")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                        input_json: String::new(),
                        input_value: block.get("input").cloned(),
                    },
                );
            }
            "content_block_delta" => {
                if let Some(delta) = event.get("delta").and_then(|v| v.as_object()) {
                    match delta.get("type").and_then(|v| v.as_str()) {
                        Some("text_delta") => {
                            if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                                self.text_streamed = true;
                                self.emit_text(sink, text.to_string());
                            }
                        }
                        Some("thinking_delta") => {
                            if let Some(text) = delta.get("thinking").and_then(|v| v.as_str()) {
                                sink(UnifiedAgentEvent::ThinkingDelta {
                                    delta: text.to_string(),
                                });
                            }
                        }
                        Some("input_json_delta") => {
                            let key = self.block_key(event.get("index").unwrap_or(&Value::Null));
                            if let Some(state) = self.blocks.get_mut(&key) {
                                if let Some(partial) =
                                    delta.get("partial_json").and_then(|v| v.as_str())
                                {
                                    state.input_json.push_str(partial);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            "content_block_stop" => {
                let key = self.block_key(event.get("index").unwrap_or(&Value::Null));
                let Some(state) = self.blocks.remove(&key) else {
                    return;
                };
                if state.block_type != "tool_use" {
                    return;
                }
                let id = state.id.unwrap_or_else(|| "tool".to_string());
                if self.streamed_tool_use_ids.contains(&id) {
                    return;
                }
                let name = state.name.unwrap_or_else(|| "tool".to_string());
                let input = if !state.input_json.trim().is_empty() {
                    serde_json::from_str(&state.input_json)
                        .unwrap_or_else(|_| Value::String(state.input_json.clone()))
                } else {
                    state.input_value.unwrap_or(Value::Null)
                };
                self.streamed_tool_use_ids.insert(id.clone());
                sink(UnifiedAgentEvent::ToolUse { id, name, input });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_delta_from_stream_event() {
        let raw = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}}}"#;
        let value: Value = serde_json::from_str(raw).unwrap();
        let mut events = Vec::new();
        ClaudeStreamState::default().handle_value(&value, &mut |e| events.push(e));
        assert!(matches!(
            events.first(),
            Some(UnifiedAgentEvent::TextDelta { delta }) if delta == "hi"
        ));
    }

    #[test]
    fn parses_streamed_tool_use_from_content_blocks() {
        let chunks = [
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg-1"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu-1","name":"Write"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"file_path\":\"page.html\"}"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_stop","index":0}}"#,
        ];
        let mut state = ClaudeStreamState::default();
        let mut events = Vec::new();
        for raw in chunks {
            let value: Value = serde_json::from_str(raw).unwrap();
            state.handle_value(&value, &mut |e| events.push(e));
        }
        assert!(events.iter().any(|event| matches!(
            event,
            UnifiedAgentEvent::ToolUse { id, name, .. }
                if id == "toolu-1" && name == "Write"
        )));
    }

    #[test]
    fn text_streamed_resets_per_message() {
        // msg1 经 delta 流式发出;msg2 只以整块 assistant 帧交付。复位后两条正文都应发出。
        let chunks = [
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg-1"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"first"}}}"#,
            r#"{"type":"assistant","message":{"id":"msg-1","content":[{"type":"text","text":"first"}]}}"#,
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg-2"}}}"#,
            r#"{"type":"assistant","message":{"id":"msg-2","content":[{"type":"text","text":"second"}]}}"#,
        ];
        let mut state = ClaudeStreamState::default();
        let mut events = Vec::new();
        for raw in chunks {
            let value: Value = serde_json::from_str(raw).unwrap();
            state.handle_value(&value, &mut |e| events.push(e));
        }
        let texts: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                UnifiedAgentEvent::TextDelta { delta } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["first", "second"]);
    }

    #[test]
    fn parses_slash_commands_from_init() {
        let raw = r#"{"type":"system","subtype":"init","slash_commands":["compact","clear"]}"#;
        let value: Value = serde_json::from_str(raw).unwrap();
        let mut events = Vec::new();
        ClaudeStreamState::default().handle_value(&value, &mut |e| events.push(e));
        assert!(events.iter().any(|event| matches!(
            event,
            UnifiedAgentEvent::SlashCommands { commands }
                if commands.len() == 2 && commands.iter().any(|c| c.slash == "/compact")
        )));
    }

    /// CLI 自己触发的压缩必须被看见：否则用户只见「对话突然变短」而无任何解释。
    /// 字段按 claude 2.1.220 的 `compact_metadata` 构造（反查二进制核实的构造处：
    /// `{trigger, pre_tokens, ...postTokens!==void 0 && {post_tokens},
    ///   ...cumulativeDroppedTokens!==void 0 && {cumulative_dropped_tokens}, ...}`）。
    #[test]
    fn parses_cli_triggered_compaction() {
        let raw = r#"{"type":"system","subtype":"compact_boundary",
            "compact_metadata":{"trigger":"auto","pre_tokens":152340,"post_tokens":18420,
              "cumulative_dropped_tokens":133920,"duration_ms":4210},
            "uuid":"u-1","session_id":"s-1"}"#;
        let value: Value = serde_json::from_str(raw).unwrap();
        let mut events = Vec::new();
        ClaudeStreamState::default().handle_value(&value, &mut |e| events.push(e));
        assert!(
            events.iter().any(|event| matches!(
                event,
                UnifiedAgentEvent::CliCompacted { trigger, pre_tokens, post_tokens, dropped_tokens, duration_ms }
                    if trigger == "auto"
                        && *pre_tokens == Some(152_340)
                        // 这一项是本轮修复的核心：旧注释断言「没有 post_tokens」，
                        // 于是 run.rs 把 token_estimate_after 硬编码 0、前端永远不显示「→ N」。
                        && *post_tokens == Some(18_420)
                        && *dropped_tokens == Some(133_920)
                        && *duration_ms == Some(4_210)
            )),
            "compact_boundary 未产出带 post_tokens 的 CliCompacted：{events:?}"
        );
    }

    #[test]
    fn compaction_without_metadata_still_reports_the_event() {
        // 元数据缺失（字段可选/未来变形）时也不能把压缩这件事整个丢掉——
        // trigger 退 auto、其余数字留 None，事件照发。
        let raw = r#"{"type":"system","subtype":"compact_boundary"}"#;
        let value: Value = serde_json::from_str(raw).unwrap();
        let mut events = Vec::new();
        ClaudeStreamState::default().handle_value(&value, &mut |e| events.push(e));
        assert!(events.iter().any(|event| matches!(
            event,
            UnifiedAgentEvent::CliCompacted { trigger, pre_tokens, post_tokens, .. }
                if trigger == "auto" && pre_tokens.is_none() && post_tokens.is_none()
        )));
    }

    /// `post_tokens` 可选：只给 pre 的旧形态仍要工作（`post_tokens` 留 None ⇒ 前端不显示「→ N」）。
    #[test]
    fn compaction_without_post_tokens_keeps_pre_tokens() {
        let events = run(&[
            r#"{"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"manual","pre_tokens":900}}"#,
        ]);
        assert!(events.iter().any(|event| matches!(
            event,
            UnifiedAgentEvent::CliCompacted { trigger, pre_tokens, post_tokens, .. }
                if trigger == "manual" && *pre_tokens == Some(900) && post_tokens.is_none()
        )));
    }

    #[test]
    fn other_system_subtypes_do_not_look_like_compaction() {
        // `system` 下还有 turn_duration / stop_hook_summary 等一堆 subtype
        // （本机 claude 历史里实测到 7 种），不得被误判成压缩。
        for raw in [
            r#"{"type":"system","subtype":"turn_duration","ms":12}"#,
            r#"{"type":"system","subtype":"stop_hook_summary"}"#,
            r#"{"type":"system","subtype":"api_error"}"#,
        ] {
            let value: Value = serde_json::from_str(raw).unwrap();
            let mut events = Vec::new();
            ClaudeStreamState::default().handle_value(&value, &mut |e| events.push(e));
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, UnifiedAgentEvent::CliCompacted { .. })),
                "{raw} 被误判为压缩"
            );
        }
    }

    /// 压缩后用量条的自愈路径：压缩后的真实占用靠紧随其后的
    /// `message_start.message.usage`（服务端算的）上报。
    /// `compact_boundary` 自己的 `post_tokens` 是给**分隔线文案**用的，不是分子来源
    /// （它是 CLI 自己的估算，而 message_start 是服务端对下一次请求的真实计量）。
    /// 这条钉住「压缩后能拿到新数字」。
    #[test]
    fn usage_recovers_from_message_start_after_compaction() {
        let events = run(&[
            r#"{"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"auto","pre_tokens":152340,"post_tokens":8900}}"#,
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m-2","usage":{"input_tokens":900,"cache_read_input_tokens":8000,"cache_creation_input_tokens":0,"output_tokens":0}}}}"#,
        ]);
        let usage = events
            .iter()
            .rev()
            .find_map(|e| match e {
                UnifiedAgentEvent::Usage { usage } => Some(usage),
                _ => None,
            })
            .expect("压缩后应有新的用量上报");
        // 900 + 8000 = 8900，远低于压缩前的 152340 —— 说明分母/分子已跟上压缩。
        assert_eq!(usage.total_tokens, Some(8_900));
    }

    // ---- usage：cache 计入 + iterations 末项 + message_start 实时上报 ----

    fn run(chunks: &[&str]) -> Vec<UnifiedAgentEvent> {
        let mut state = ClaudeStreamState::default();
        let mut events = Vec::new();
        for raw in chunks {
            let value: Value = serde_json::from_str(raw).unwrap();
            state.handle_value(&value, &mut |e| events.push(e));
        }
        events
    }

    fn usages(events: &[UnifiedAgentEvent]) -> Vec<crate::chat::model::ModelUsage> {
        events
            .iter()
            .filter_map(|e| match e {
                UnifiedAgentEvent::Usage { usage } => Some(usage.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn result_usage_counts_cache_tokens() {
        // 实测 result.usage 的真实键位（本机样本值为 0，此处按同结构填非零数字）。
        let events = run(&[
            r#"{"type":"result","usage":{"input_tokens":1200,"cache_creation_input_tokens":300,
                "cache_read_input_tokens":45000,"output_tokens":800,"iterations":[],
                "service_tier":"standard"}}"#,
        ]);
        let usage = usages(&events).pop().expect("result 应产出 Usage");
        assert_eq!(usage.input_tokens, Some(1200));
        assert_eq!(usage.output_tokens, Some(800));
        assert_eq!(usage.cached_input_tokens, Some(45_000));
        assert_eq!(usage.cache_creation_input_tokens, Some(300));
        // 1200 + 800 + 45000 + 300：漏掉 cache 会得到 2000，差一个数量级。
        assert_eq!(usage.total_tokens, Some(47_300));
    }

    #[test]
    fn result_usage_takes_last_iteration_not_first_nor_sum() {
        // iterations[] 是一轮内多次 LLM 往返的**独立快照**序列，当前上下文占用 = 末项。
        let events = run(&[
            r#"{"type":"result","usage":{"input_tokens":1,"output_tokens":1,
                "cache_read_input_tokens":0,"cache_creation_input_tokens":0,
                "iterations":[
                  {"input_tokens":100,"output_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},
                  {"input_tokens":500,"output_tokens":20,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},
                  {"input_tokens":900,"output_tokens":30,"cache_read_input_tokens":2000,"cache_creation_input_tokens":0}
                ]}}"#,
        ]);
        let usage = usages(&events).pop().expect("result 应产出 Usage");
        assert_eq!(usage.input_tokens, Some(900), "取末项，不是首项(100)");
        assert_eq!(usage.output_tokens, Some(30));
        assert_eq!(usage.cached_input_tokens, Some(2000));
        // 末项 900+30+2000=2930；累加三项会是 1500+60+2000=3560（计费口径，不是上下文占用）。
        assert_eq!(usage.total_tokens, Some(2930));
    }

    #[test]
    fn result_usage_falls_back_to_top_level_when_iterations_empty_or_absent() {
        for raw in [
            r#"{"type":"result","usage":{"input_tokens":7,"output_tokens":3,"iterations":[]}}"#,
            r#"{"type":"result","usage":{"input_tokens":7,"output_tokens":3}}"#,
        ] {
            let usage = usages(&run(&[raw])).pop().expect("result 应产出 Usage");
            assert_eq!(usage.input_tokens, Some(7));
            assert_eq!(usage.output_tokens, Some(3));
            assert_eq!(usage.total_tokens, Some(10));
        }
    }

    #[test]
    fn message_start_with_usage_emits_realtime_usage() {
        let events = run(&[
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg-1",
                "usage":{"input_tokens":1500,"cache_read_input_tokens":62000,
                "cache_creation_input_tokens":900,"output_tokens":2}}}}"#,
        ]);
        let usage = usages(&events).pop().expect("message_start 应产出 Usage");
        assert_eq!(usage.input_tokens, Some(1500));
        assert_eq!(usage.cached_input_tokens, Some(62_000));
        assert_eq!(usage.total_tokens, Some(64_402));
    }

    #[test]
    fn message_start_without_usage_emits_nothing_and_keeps_per_message_reset() {
        // 不带 usage / 全零 usage 都不应产出事件；且 message_id 与 text_streamed 复位行为不变。
        let chunks = [
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg-1"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"first"}}}"#,
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg-2",
                "usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}"#,
            r#"{"type":"assistant","message":{"id":"msg-2","content":[{"type":"text","text":"second"}]}}"#,
        ];
        let events = run(&chunks);
        assert!(usages(&events).is_empty(), "无有效 usage 时不得产出事件");
        let texts: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                UnifiedAgentEvent::TextDelta { delta } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            vec!["first", "second"],
            "per-message 复位行为不得改变"
        );
    }

    // ---- result 的错误必须进错误通道（R1 / AC1-AC3）----

    fn errors(events: &[UnifiedAgentEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                UnifiedAgentEvent::Error { message } => Some(message.clone()),
                _ => None,
            })
            .collect()
    }

    fn texts(events: &[UnifiedAgentEvent]) -> String {
        events
            .iter()
            .filter_map(|e| match e {
                UnifiedAgentEvent::TextDelta { delta } => Some(delta.as_str()),
                _ => None,
            })
            .collect()
    }

    /// **本机实测原样本**（2026-07-27，嵌套 claude 未登录，
    /// `claude -p "say hi" --output-format stream-json --include-partial-messages --verbose`）。
    /// 逐字保留：`subtype` 是 `success` 而 `is_error` 为 true，错误文案在 `result` 字段里。
    const REAL_NOT_LOGGED_IN_RESULT: &str = r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":null,"duration_ms":63,"duration_api_ms":0,"num_turns":1,"result":"Not logged in · Please run /login","stop_reason":"stop_sequence","session_id":"9de6398f-b124-494f-94e2-b716733270cb","total_cost_usd":0,"usage":{"input_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":0,"server_tool_use":{"web_search_requests":0,"web_fetch_requests":0},"service_tier":"standard","cache_creation":{"ephemeral_1h_input_tokens":0,"ephemeral_5m_input_tokens":0},"inference_geo":"","iterations":[],"speed":"standard"},"modelUsage":{},"permission_denials":[],"terminal_reason":"api_error","fast_mode_state":"off","uuid":"048761d6-ca03-4a01-8ae8-526c7e06e08a"}"#;

    /// AC1：`subtype: "success"` + `is_error: true` 必须产出 Error，文案取 `result` 字段。
    /// 只判 subtype 会把这一轮当成功 ⇒ 用户拿到空回复且零提示（本轮修复的核心 bug）。
    #[test]
    fn real_not_logged_in_result_becomes_an_error_event() {
        let events = run(&[REAL_NOT_LOGGED_IN_RESULT]);
        let messages = errors(&events);
        assert_eq!(
            messages.len(),
            1,
            "未登录样本应恰好产出一条 Error：{events:?}"
        );
        assert!(
            messages[0].contains("Not logged in · Please run /login"),
            "错误文案应取 result 字段，实际：{}",
            messages[0]
        );
    }

    /// 该文案必须能被 `errors::classify` 归成 Auth 并附上登录命令——
    /// spec 第 5 条要求裸串不得直接落气泡。这条钉住「进了分类器且分对了」。
    #[test]
    fn not_logged_in_error_classifies_as_auth_with_login_hint() {
        let message = errors(&run(&[REAL_NOT_LOGGED_IN_RESULT]))
            .pop()
            .expect("应有 Error");
        let classified = crate::external_agents::errors::classify(&message, None, "", "claude");
        assert_eq!(
            classified.kind,
            crate::external_agents::errors::ExternalAgentErrorKind::Auth
        );
        assert!(classified.user_message.contains("claude /login"));
        // 原始英文串只进 `<details>`，不做气泡主文案。
        assert!(!classified.user_message.contains("Not logged in"));
        assert!(classified.detail.contains("Not logged in"));
    }

    /// AC2：四种 `error_*` subtype 各自产出 Error，且 `errors[]` 优先于 `result`。
    #[test]
    fn all_error_subtypes_produce_an_error_and_prefer_the_errors_array() {
        for subtype in [
            "error_during_execution",
            "error_max_turns",
            "error_max_budget_usd",
            "error_max_structured_output_retries",
        ] {
            let raw = format!(
                r#"{{"type":"result","subtype":"{subtype}","is_error":true,
                   "errors":["rate limited by upstream"],"result":"ignored fallback text",
                   "permission_denials":[],"usage":{{"input_tokens":5,"output_tokens":1}}}}"#
            );
            let messages = errors(&run(&[&raw]));
            assert_eq!(messages.len(), 1, "{subtype} 应产出一条 Error");
            assert_eq!(
                messages[0], "rate limited by upstream",
                "{subtype} 应优先用 errors[]，而不是 result 字段"
            );
        }
    }

    /// `is_error` 缺失时，error 子型**本身**就足以判定失败（官方类型里 is_error 恒 true，
    /// 但不能依赖对端一定带上这个字段）。
    #[test]
    fn error_subtype_alone_is_enough_even_without_is_error() {
        let messages = errors(&run(&[
            r#"{"type":"result","subtype":"error_max_turns","errors":["max turns"]}"#,
        ]));
        assert_eq!(messages, vec!["max turns".to_string()]);
    }

    /// 文案兜底链：`errors[]` 为空数组 / 全空串时退到 `result`；两者都没有才用 subtype 造句。
    #[test]
    fn error_message_falls_back_from_errors_to_result_then_subtype() {
        let from_result = errors(&run(&[
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,
               "errors":["","  "],"result":"tool crashed"}"#,
        ]));
        assert_eq!(from_result, vec!["tool crashed".to_string()]);

        let from_subtype = errors(&run(&[
            r#"{"type":"result","subtype":"error_max_budget_usd","is_error":true}"#,
        ]));
        assert_eq!(from_subtype.len(), 1);
        assert!(
            from_subtype[0].contains("预算上限")
                && from_subtype[0].contains("error_max_budget_usd"),
            "兜底文案应含中文原因与 subtype：{}",
            from_subtype[0]
        );
    }

    /// AC3：正常成功轮次**不得**产出 Error，且 usage 解析不受新增分支影响。
    #[test]
    fn successful_result_produces_no_error_and_keeps_usage() {
        let events = run(&[r#"{"type":"result","subtype":"success","is_error":false,
               "result":"done","permission_denials":[],
               "usage":{"input_tokens":1200,"cache_creation_input_tokens":300,
               "cache_read_input_tokens":45000,"output_tokens":800,"iterations":[]}}"#]);
        assert!(errors(&events).is_empty(), "成功轮次不得产出 Error");
        assert!(texts(&events).is_empty(), "成功轮次不得插入任何提示文本");
        let usage = usages(&events).pop().expect("仍应产出 Usage");
        assert_eq!(usage.total_tokens, Some(47_300));
    }

    /// 失败轮次的 usage 同样要上报——错误分支不能把用量吞掉（用量条不该因失败而停更）。
    #[test]
    fn failed_result_still_reports_usage() {
        let events = run(&[
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,
               "errors":["boom"],"usage":{"input_tokens":900,"output_tokens":30,
               "cache_read_input_tokens":2000,"cache_creation_input_tokens":0}}"#,
        ]);
        assert_eq!(errors(&events).len(), 1);
        let usage = usages(&events).pop().expect("失败轮次也应产出 Usage");
        assert_eq!(usage.total_tokens, Some(2930));
    }

    // ---- R4：permission_denials 可见 ----

    #[test]
    fn permission_denials_are_surfaced_to_the_user() {
        let events = run(&[
            r#"{"type":"result","subtype":"success","is_error":false,"result":"ok",
               "permission_denials":[
                 {"tool_name":"Bash","tool_use_id":"toolu-1","tool_input":{"command":"rm -rf /"}},
                 {"tool_name":"Write","tool_use_id":"toolu-2","tool_input":{"file_path":"/etc/hosts"}},
                 {"tool_name":"Bash","tool_use_id":"toolu-3","tool_input":{"command":"curl x"}}
               ],"usage":{"input_tokens":5,"output_tokens":1}}"#,
        ]);
        let note = texts(&events);
        assert!(note.contains("被权限规则拒绝"), "应有可见提示：{note}");
        assert!(note.contains("Bash") && note.contains("Write"));
        // 同名工具去重：Bash 被拒两次也只列一次。
        assert_eq!(note.matches("Bash").count(), 1, "同名工具不应重复列出");
        // 提示不得吃掉这一轮的成功语义。
        assert!(errors(&events).is_empty());
    }

    #[test]
    fn empty_permission_denials_add_no_noise() {
        let events = run(&[
            r#"{"type":"result","subtype":"success","is_error":false,"result":"ok",
               "permission_denials":[],"usage":{"input_tokens":1,"output_tokens":1}}"#,
        ]);
        assert!(texts(&events).is_empty(), "空 denials 不得产生提示");
    }

    #[test]
    fn permission_denials_without_tool_name_still_report_the_count() {
        // 结构变形（拿不到 tool_name）时也不能把「有调用被拒」整个丢掉。
        let events =
            run(&[r#"{"type":"result","subtype":"success","permission_denials":[{},{}]}"#]);
        let note = texts(&events);
        assert!(
            note.contains('2') && note.contains("被权限规则拒绝"),
            "{note}"
        );
    }

    // ---- R2：system 子类型 ----

    /// `status: "compacting"` 是压缩的**开始**端（`compact_boundary` 是结束端），
    /// 长压缩里这是唯一的可见信号。注意它不得被误报成 `CliCompacted`——
    /// 那会让前端在压缩尚未完成时就插一条分隔线。
    #[test]
    fn compacting_status_is_reported_as_a_notice_not_a_compaction_boundary() {
        let events = run(&[
            r#"{"type":"system","subtype":"status","status":"compacting","uuid":"u-1","session_id":"s-1"}"#,
        ]);
        assert!(texts(&events).contains("正在压缩上下文"), "{events:?}");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, UnifiedAgentEvent::CliCompacted { .. })),
            "compacting 状态不是压缩边界"
        );
    }

    /// 其余 status 值是纯进度噪音（一轮内多条），不得落进正文。
    /// `requesting` 是本机实测到的真实值，claude 2.1.220 的 `SDKStatus` 类型里**有**它
    /// （此前注释称「官方类型里没有」已过期）。仍刻意不做白名单校验——那是对未来新增
    /// 取值的防御（开放字符串），只保证不刷屏。
    #[test]
    fn other_status_values_stay_silent() {
        for raw in [
            r#"{"type":"system","subtype":"status","status":"requesting","uuid":"u-1","session_id":"s-1"}"#,
            r#"{"type":"system","subtype":"status","status":null}"#,
            r#"{"type":"system","subtype":"status","status":"compacting_x"}"#,
        ] {
            let events = run(&[raw]);
            assert!(events.is_empty(), "{raw} 不应产出任何事件：{events:?}");
        }
    }

    // ---- A6：压缩终态（compact_result / compact_error）----

    /// 压缩**失败**时 claude 发的是带 `compact_result:"failed"` 的 status，**不会**发
    /// `compact_boundary`。不接的话「⏳ 正在压缩…」永远停在那里，用户以为还在跑。
    #[test]
    fn failed_compaction_status_is_surfaced_instead_of_hanging_on_compacting() {
        let events = run(&[
            r#"{"type":"system","subtype":"status","status":null,"compact_result":"failed",
               "compact_error":"context too large to summarize","uuid":"u","session_id":"s"}"#,
        ]);
        let note = texts(&events);
        assert!(note.contains("压缩上下文失败"), "{note}");
        assert!(
            note.contains("context too large to summarize"),
            "应带上 compact_error 原因：{note}"
        );
        // 失败不是压缩边界：不得插分隔线。
        assert!(!events
            .iter()
            .any(|e| matches!(e, UnifiedAgentEvent::CliCompacted { .. })));
    }

    #[test]
    fn failed_compaction_without_reason_still_reports_the_failure() {
        let events = run(&[
            r#"{"type":"system","subtype":"status","status":"compacting","compact_result":"failed"}"#,
        ]);
        let note = texts(&events);
        assert!(note.contains("压缩上下文失败"), "{note}");
        // 同一条 status 不能既报「正在压缩」又报「失败」。
        assert!(!note.contains("正在压缩"), "{note}");
    }

    #[test]
    fn successful_compaction_status_is_reported_once() {
        let events = run(&[
            r#"{"type":"system","subtype":"status","status":null,"compact_result":"success","permissionMode":"bypassPermissions"}"#,
        ]);
        assert!(texts(&events).contains("已完成上下文压缩"), "{events:?}");
    }

    /// 未来新增的 compact_result 取值一律安全忽略（spec 第 10 条）。
    #[test]
    fn unknown_compact_result_value_is_ignored() {
        let events = run(&[
            r#"{"type":"system","subtype":"status","status":"requesting","compact_result":"partial"}"#,
        ]);
        assert!(events.is_empty(), "{events:?}");
    }

    // ---- A4：客户端斜杠命令的输出不许被吞掉 ----

    /// `/cost` `/usage` `/context` `/status` `/doctor` 由 claude **客户端执行**，没有模型往返。
    /// 输出走 `system/local_command_output`；不接的话用户看到的是 run.rs 的兜底
    /// 「claude 命令已执行」，报告内容 100% 丢失。
    #[test]
    fn local_command_output_becomes_answer_text() {
        let events = run(&[
            r#"{"type":"system","subtype":"local_command_output",
               "content":"Total cost: $1.23\nTotal duration: 5m 12s","uuid":"u","session_id":"s"}"#,
        ]);
        let text = texts(&events);
        assert!(text.contains("Total cost: $1.23"), "{text}");
        assert!(text.contains("Total duration"), "{text}");
    }

    /// 另一条通道：报告正文落在 `result.result` 上（成功 + `output_tokens == 0`）。
    #[test]
    fn zero_output_result_text_is_surfaced_when_no_body_was_streamed() {
        let events = run(&[
            r#"{"type":"result","subtype":"success","is_error":false,
               "result":"Context usage: 42k/1M tokens",
               "usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,
                        "cache_creation_input_tokens":0,"iterations":[]}}"#,
        ]);
        assert!(
            texts(&events).contains("Context usage: 42k/1M tokens"),
            "{events:?}"
        );
    }

    /// **不得重复**：本轮已经流过正文时，`result.result`（常是回答的完整副本）不再发一遍。
    #[test]
    fn result_text_is_not_duplicated_when_the_answer_already_streamed() {
        let events = run(&[
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m-1"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"hello",
               "usage":{"input_tokens":10,"output_tokens":0}}"#,
        ]);
        assert_eq!(texts(&events), "hello", "正文被重复发了一遍：{events:?}");
    }

    /// 有真实模型输出（`output_tokens > 0`）时同样不兜底。
    #[test]
    fn result_text_is_not_surfaced_when_the_model_produced_output() {
        let events = run(&[
            r#"{"type":"result","subtype":"success","is_error":false,"result":"the answer",
               "usage":{"input_tokens":10,"output_tokens":7}}"#,
        ]);
        assert!(texts(&events).is_empty(), "{events:?}");
    }

    /// 系统提示（压缩中 / 权限被拒）**不算正文**：它们不该抑制 A4 的兜底，
    /// 否则一条「正在压缩」提示就能把 `/cost` 的报告吞掉。
    #[test]
    fn notices_do_not_suppress_the_zero_output_result_fallback() {
        let events = run(&[
            r#"{"type":"system","subtype":"status","status":"compacting"}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Cost report",
               "usage":{"input_tokens":0,"output_tokens":0}}"#,
        ]);
        assert!(texts(&events).contains("Cost report"), "{events:?}");
    }

    /// 失败轮次的 `result` 文案只走错误通道（`errors::classify`），不得**同时**当正文发出去
    /// ——否则未登录时用户会看到裸英文 + 分类提示两份。
    #[test]
    fn failed_result_text_goes_to_the_error_channel_only() {
        let events = run(&[REAL_NOT_LOGGED_IN_RESULT]);
        assert_eq!(errors(&events).len(), 1);
        assert!(
            !texts(&events).contains("Not logged in"),
            "错误文案不得同时落进正文：{events:?}"
        );
    }

    // ---- A5：assistant 帧上的 error / aborted ----

    /// `max_output_tokens`：回答在**中途硬截断**，而 result 那边可能仍是成功
    /// （错误只写在 assistant 帧上）。不接就会把半截回答标成「完成」。
    #[test]
    fn max_output_tokens_truncation_is_surfaced_as_a_notice_not_a_failure() {
        let events = run(&[
            r#"{"type":"assistant","error":"max_output_tokens","uuid":"u","session_id":"s",
               "message":{"id":"m-1","role":"assistant","content":[{"type":"text","text":"half an ans"}]}}"#,
        ]);
        let text = texts(&events);
        assert!(text.contains("输出上限"), "{text}");
        // 正文仍要发出——把这一轮判成失败会连带丢掉已经流出来的半截回答。
        assert!(text.contains("half an ans"), "{text}");
        assert!(
            errors(&events).is_empty(),
            "截断不是「整轮失败」：{events:?}"
        );
    }

    /// 认证 / 计费 / 上游故障类走 Error 通道，由 `errors::classify` 给可操作中文（spec 第 5 条）。
    #[test]
    fn assistant_auth_error_goes_to_the_error_channel() {
        let events = run(&[
            r#"{"type":"assistant","error":"authentication_failed",
               "message":{"id":"m-1","role":"assistant","content":[]}}"#,
        ]);
        let messages = errors(&events);
        assert_eq!(messages.len(), 1, "{events:?}");
        let classified =
            crate::external_agents::errors::classify(&messages[0], None, "", "claude");
        assert_eq!(
            classified.kind,
            crate::external_agents::errors::ExternalAgentErrorKind::Auth
        );
    }

    /// 各类值都要能到出口（哪怕只是提示）——静默是最坏的结果。
    #[test]
    fn every_documented_assistant_error_value_is_visible() {
        for kind in [
            "oauth_org_not_allowed",
            "billing_error",
            "rate_limit",
            "overloaded",
            "invalid_request",
            "model_not_found",
            "server_error",
            "unknown",
        ] {
            let raw = format!(
                r#"{{"type":"assistant","error":"{kind}","message":{{"id":"m","role":"assistant","content":[]}}}}"#
            );
            let events = run(&[&raw]);
            assert!(
                !errors(&events).is_empty() || !texts(&events).is_empty(),
                "{kind} 被静默吞掉：{events:?}"
            );
        }
    }

    #[test]
    fn aborted_assistant_message_is_surfaced() {
        let events = run(&[
            r#"{"type":"assistant","aborted":true,
               "message":{"id":"m-1","role":"assistant","content":[{"type":"text","text":"partial"}]}}"#,
        ]);
        assert!(texts(&events).contains("被中止"), "{events:?}");
    }

    /// 同一次失败常在多条 assistant 帧上重复出现——只报一次。
    #[test]
    fn repeated_assistant_errors_are_reported_once_per_turn() {
        let events = run(&[
            r#"{"type":"assistant","error":"rate_limit","message":{"id":"m-1","role":"assistant","content":[]}}"#,
            r#"{"type":"assistant","error":"rate_limit","message":{"id":"m-2","role":"assistant","content":[]}}"#,
        ]);
        assert_eq!(errors(&events).len(), 1, "{events:?}");
    }

    /// 正常 assistant 帧（无 error / aborted）一个提示都不该多。
    #[test]
    fn clean_assistant_message_adds_no_notice() {
        let events = run(&[
            r#"{"type":"assistant","message":{"id":"m-1","role":"assistant","content":[{"type":"text","text":"hi"}]}}"#,
        ]);
        assert_eq!(texts(&events), "hi");
        assert!(errors(&events).is_empty());
    }

    // ---- A2：零用量的 result 不许把分子清零 ----

    /// **本轮修复的核心**：没有 LLM 往返的轮次（未登录 / `/help` / 未知斜杠命令 /
    /// Kivio 自己发的 `/compact`）返回 `iterations: []` + 顶层四字段全 0。
    /// 不守就会产出 `total_tokens: Some(0)` 并落盘，`context.rs` 的 `is_some()` 判据命中它
    /// ⇒ 用量条从 47K 掉到 0。
    #[test]
    fn all_zero_result_usage_emits_no_usage_event() {
        let events = run(&[
            r#"{"type":"result","subtype":"success","is_error":false,"result":"ok",
               "modelUsage":{},
               "usage":{"input_tokens":0,"cache_creation_input_tokens":0,
                        "cache_read_input_tokens":0,"output_tokens":0,"iterations":[]}}"#,
        ]);
        assert!(
            usages(&events).is_empty(),
            "全零 usage 不得产出事件（会把分子清零）：{events:?}"
        );
    }

    /// 同一轮里 message_start 已报过真实占用时，随后的全零 result 不得把它盖掉。
    #[test]
    fn zero_usage_result_does_not_erase_the_realtime_usage_of_the_same_turn() {
        let events = run(&[
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m-1",
               "usage":{"input_tokens":1200,"cache_read_input_tokens":45000,
                        "cache_creation_input_tokens":300,"output_tokens":0}}}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,
               "usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,
                        "cache_creation_input_tokens":0,"iterations":[]}}"#,
        ]);
        let all = usages(&events);
        assert_eq!(all.len(), 1, "只应有 message_start 那一条：{events:?}");
        assert_eq!(all[0].total_tokens, Some(46_500));
    }

    /// 顶层回退的闸门：同一个 state（= 常驻进程下的同一个会话）第二个 result 起，
    /// `iterations` 为空时不再拿顶层「计费总量」当上下文快照。
    #[test]
    fn top_level_usage_fallback_is_gated_to_the_first_result_of_a_session() {
        let flat = serde_json::json!({"input_tokens": 7, "output_tokens": 3});
        assert!(
            claude_result_usage_snapshot(&flat, 0).is_some(),
            "第一个 result 仍要用顶层回退"
        );
        assert!(
            claude_result_usage_snapshot(&flat, 1).is_none(),
            "第二个 result 起顶层是计费总量，不是上下文快照"
        );

        // iterations 非空时不受闸门影响——那才是真正的快照序列。
        let with_iterations = serde_json::json!({
            "input_tokens": 1,
            "iterations": [{"input_tokens": 900, "output_tokens": 30}]
        });
        assert_eq!(
            claude_result_usage_snapshot(&with_iterations, 5)
                .and_then(|v| v.get("input_tokens")),
            Some(&serde_json::json!(900))
        );
    }

    /// 端到端形态：同一个 state 连跑两轮，第二轮的空 `iterations` 不再产出 usage
    /// （分子交给该轮的 message_start）。
    #[test]
    fn second_result_in_one_session_does_not_report_top_level_usage() {
        let events = run(&[
            r#"{"type":"result","subtype":"success","usage":{"input_tokens":100,"output_tokens":5,"iterations":[]}}"#,
            r#"{"type":"result","subtype":"success","usage":{"input_tokens":900,"output_tokens":40,"iterations":[]}}"#,
        ]);
        let all = usages(&events);
        assert_eq!(all.len(), 1, "第二个 result 不该再报顶层用量：{events:?}");
        assert_eq!(all[0].total_tokens, Some(105));
    }

    // ---- A8：分母来自 CLI 实报的 modelUsage.contextWindow ----

    /// **本机实测原文**（claude 2.1.220，`--model opus`，一轮真实回复）。
    /// 逐字保留：key 带 `[1M]` 后缀、`canonicalModel` 不带、`contextWindow` 是 1M。
    const REAL_MODEL_USAGE: &str = r#"{"claude-opus-4-8[1M]":{"inputTokens":7037,"outputTokens":4,
        "cacheReadInputTokens":31792,"cacheCreationInputTokens":182,"webSearchRequests":0,
        "costUSD":0.052318500000000004,"contextWindow":1000000,"maxOutputTokens":64000,
        "canonicalModel":"claude-opus-4-8","provider":"firstParty"}}"#;

    #[test]
    fn context_window_comes_from_the_real_model_usage_payload() {
        let model_usage: Value = serde_json::from_str(REAL_MODEL_USAGE).unwrap();
        // init 报的 resolved model 实测就是带后缀的那个形态 → key 精确命中。
        assert_eq!(
            context_window_from_model_usage(&model_usage, Some("claude-opus-4-8[1M]")),
            Some(1_000_000)
        );
        // canonicalModel（不带后缀）也要能匹上。
        assert_eq!(
            context_window_from_model_usage(&model_usage, Some("claude-opus-4-8")),
            Some(1_000_000)
        );
        // 认不出的模型退「取最大值」，不报错。
        assert_eq!(
            context_window_from_model_usage(&model_usage, Some("something-else")),
            Some(1_000_000)
        );
        assert_eq!(
            context_window_from_model_usage(&model_usage, None),
            Some(1_000_000)
        );
    }

    /// `modelUsage` 是**会话累计 map**：会话里换过模型 / 跑过 haiku 子 agent 时，
    /// 最大值可能是历史上那个更大的窗口而不是当前模型的 —— 精确匹配必须赢过 max。
    #[test]
    fn exact_model_match_beats_the_largest_window_in_the_map() {
        let model_usage = serde_json::json!({
            "claude-opus-4-8[1M]": {"contextWindow": 1_000_000, "canonicalModel": "claude-opus-4-8"},
            "claude-haiku-4-5": {"contextWindow": 200_000, "canonicalModel": "claude-haiku-4-5"},
        });
        assert_eq!(
            context_window_from_model_usage(&model_usage, Some("claude-haiku-4-5")),
            Some(200_000),
            "当前模型是 haiku 时不该拿子 agent/历史模型的 1M 当分母"
        );
    }

    /// `contextWindow` **可以是 0**（本机 `~/.claude/stats-cache.json` 里 8 个模型全是 0）。
    /// 0 不是窗口，必须当「没报」处理，否则分母为 0 会算出无穷大的占用比。
    #[test]
    fn zero_context_window_is_not_a_denominator() {
        let model_usage = serde_json::json!({
            "m-a": {"contextWindow": 0},
            "m-b": {"contextWindow": 0},
        });
        assert_eq!(context_window_from_model_usage(&model_usage, Some("m-a")), None);
        assert_eq!(context_window_from_model_usage(&model_usage, None), None);
        // 空 map（被中断的轮次实测就是 `{}`）同样是 None。
        assert_eq!(
            context_window_from_model_usage(&serde_json::json!({}), None),
            None
        );
        // 形状异常不得 panic。
        assert_eq!(
            context_window_from_model_usage(&serde_json::json!("nope"), None),
            None
        );
        assert_eq!(
            context_window_from_model_usage(&serde_json::json!({"m": {"contextWindow": "1M"}}), None),
            None
        );
    }

    /// 端到端：`system/init` → `result` 一路把窗口带到 `ModelUsage.context_window_tokens`，
    /// 下游 `context.rs::context_window_for_external_model` 的最高优先级来源就是它。
    #[test]
    fn init_model_plus_result_model_usage_yields_the_window() {
        let events = run(&[
            r#"{"type":"system","subtype":"init","model":"claude-opus-4-8[1M]","slash_commands":[]}"#,
            &format!(
                r#"{{"type":"result","subtype":"success","is_error":false,
                   "modelUsage":{REAL_MODEL_USAGE},
                   "usage":{{"input_tokens":7037,"output_tokens":4,
                     "cache_read_input_tokens":31792,"cache_creation_input_tokens":182,
                     "iterations":[]}}}}"#
            ),
        ]);
        let usage = usages(&events).pop().expect("result 应产出 Usage");
        assert_eq!(usage.context_window_tokens, Some(1_000_000));
        // 分子仍取本轮的 `usage`，**不是** modelUsage 里的进程累计数字。
        assert_eq!(usage.total_tokens, Some(39_015));
    }

    /// 零用量但拿到窗口时仍上报（窗口是静态属性，与分子无关）；
    /// 分子不被清零由 `run.rs::merge_cli_usage` 的「全零不覆盖」保证。
    #[test]
    fn window_is_reported_even_when_the_turn_used_no_tokens() {
        let events = run(&[&format!(
            r#"{{"type":"result","subtype":"success","modelUsage":{REAL_MODEL_USAGE},
               "usage":{{"input_tokens":0,"output_tokens":0,"iterations":[]}}}}"#
        )]);
        let usage = usages(&events).pop().expect("窗口应照常上报");
        assert_eq!(usage.context_window_tokens, Some(1_000_000));
    }

    // ---- B1：用户中断的 result 不是失败 ----

    /// **本机实测原样本**（claude 2.1.220，2026-07-29，常驻进程 + `control_request`
    /// `interrupt`）。逐字保留形态：`is_error: true` + `subtype: "error_during_execution"`
    /// + `terminal_reason: "aborted_streaming"` + `result: null` + `modelUsage: {}`。
    const REAL_ABORTED_RESULT: &str = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"duration_ms":3120,"num_turns":1,"result":null,"session_id":"s-1","total_cost_usd":0.004,"usage":{"input_tokens":4,"cache_creation_input_tokens":182,"cache_read_input_tokens":19334,"output_tokens":210,"iterations":[]},"modelUsage":{},"permission_denials":[],"errors":["[ede_diagnostic] aborted"],"terminal_reason":"aborted_streaming"}"#;

    /// 中断的轮次不得产出 Error —— 否则每点一次「停止」都多一个假错误气泡。
    #[test]
    fn user_aborted_result_is_not_a_failure() {
        let events = run(&[REAL_ABORTED_RESULT]);
        assert!(
            errors(&events).is_empty(),
            "中断被判成失败了：{events:?}（会在「已取消」后再叠一个错误气泡）"
        );
        assert!(
            !texts(&events).contains("[ede_diagnostic]"),
            "内部诊断串不得落进正文：{events:?}"
        );
    }

    /// 判据只认 `terminal_reason`：spec 第 20 条的 `is_error` 判据必须保持不变——
    /// 未登录的真实样本正是 `subtype:"success"` + `is_error:true`，它仍要报错。
    #[test]
    fn abort_exemption_does_not_weaken_the_is_error_rule() {
        assert!(result_is_user_abort(
            &serde_json::from_str::<Value>(REAL_ABORTED_RESULT).unwrap()
        ));
        assert!(!result_is_user_abort(
            &serde_json::from_str::<Value>(REAL_NOT_LOGGED_IN_RESULT).unwrap()
        ));
        // 未登录（terminal_reason = "api_error"）仍必须是一条 Error。
        assert_eq!(errors(&run(&[REAL_NOT_LOGGED_IN_RESULT])).len(), 1);
        // 其它 terminal_reason / 缺失该键都不算中断。
        for raw in [
            r#"{"type":"result","subtype":"success","terminal_reason":"end_turn"}"#,
            r#"{"type":"result","subtype":"success"}"#,
            r#"{"type":"result","subtype":"success","terminal_reason":42}"#,
        ] {
            assert!(!result_is_user_abort(
                &serde_json::from_str::<Value>(raw).unwrap()
            ));
        }
    }

    /// 常驻会话据此把这一轮送去「已取消」出口；下一个正常 `result` 必须把标志复位，
    /// 否则取消过一次之后每一轮都被当成取消。
    #[test]
    fn last_result_aborted_tracks_the_most_recent_result() {
        let mut state = ClaudeStreamState::default();
        let feed = |state: &mut ClaudeStreamState, raw: &str| {
            let value: Value = serde_json::from_str(raw).unwrap();
            state.handle_value(&value, &mut |_| {});
        };
        assert!(!state.last_result_aborted());
        feed(&mut state, REAL_ABORTED_RESULT);
        assert!(state.last_result_aborted());
        feed(
            &mut state,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","terminal_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":2}}"#,
        );
        assert!(
            !state.last_result_aborted(),
            "取消标志没复位 ⇒ 之后每一轮都会被当成取消"
        );
    }

    /// **常驻的跨轮状态契约**（spec 第 3 条）：同一个 state 连跑两轮，
    /// per-turn 的 `any_text_emitted` 必须在 `result` 复位，否则 A4 的 `result.result`
    /// 兜底（`/cost` 之类客户端命令的唯一输出通道）从第 2 轮起永久失效。
    #[test]
    fn per_turn_state_resets_at_the_round_boundary_so_the_fallback_survives_turn_two() {
        let events = run(&[
            // 第 1 轮：正常流式回答 + 结束。
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m-1"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"hello","usage":{"input_tokens":10,"output_tokens":4}}"#,
            // 第 2 轮：客户端斜杠命令，报告只在 result.result 里。
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Total cost: $1.23","usage":{"input_tokens":0,"output_tokens":0,"iterations":[]}}"#,
        ]);
        assert!(
            texts(&events).contains("Total cost: $1.23"),
            "第 2 轮的 result 兜底失效了（any_text_emitted 没在 result 复位）：{events:?}"
        );
    }

    /// 同理，`reported_assistant_errors` 也是 per-turn：第 2 轮的同类错误仍要能报出来。
    #[test]
    fn assistant_error_dedup_is_per_turn_not_per_session() {
        let events = run(&[
            r#"{"type":"assistant","error":"rate_limit","message":{"id":"m-1","role":"assistant","content":[]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":1,"output_tokens":1}}"#,
            r#"{"type":"assistant","error":"rate_limit","message":{"id":"m-2","role":"assistant","content":[]}}"#,
        ]);
        assert_eq!(
            errors(&events).len(),
            2,
            "第 2 轮的同类错误被上一轮的去重表吞掉了：{events:?}"
        );
    }

    /// **常驻下分子的唯一来源**（spec 第 14h 条的闸门 + `--include-partial-messages`）：
    /// 第 2 轮起 `iterations` 恒为 `[]`、顶层回退被闸门拦住 ⇒ 分子完全依赖
    /// `message_start.message.usage`。这条钉住那条链路在同一个 state 上跨轮成立。
    #[test]
    fn second_turn_numerator_comes_from_message_start_because_the_top_level_fallback_is_gated() {
        let events = run(&[
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m-1","usage":{"input_tokens":100,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}"#,
            r#"{"type":"result","subtype":"success","usage":{"input_tokens":100,"output_tokens":5,"iterations":[]}}"#,
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m-2","usage":{"input_tokens":900,"output_tokens":0,"cache_read_input_tokens":8000,"cache_creation_input_tokens":0}}}}"#,
            r#"{"type":"result","subtype":"success","usage":{"input_tokens":950,"output_tokens":40,"iterations":[]}}"#,
        ]);
        let all = usages(&events);
        // 第 1 轮：message_start(100) + result 顶层回退(105)；第 2 轮：只有 message_start(8900)。
        assert_eq!(
            all.len(),
            3,
            "第 2 轮的 result 不该再报顶层计费总量：{events:?}"
        );
        assert_eq!(
            all.last().and_then(|u| u.total_tokens),
            Some(8_900),
            "第 2 轮分子必须来自 message_start（--include-partial-messages 必须常开）"
        );
    }

    #[test]
    fn task_notification_reports_only_failure_and_stop() {        let failed = run(&[
            r#"{"type":"system","subtype":"task_notification","task_id":"t-1","status":"failed",
               "output_file":"/tmp/t1.log","summary":"build 脚本退出码 2"}"#,
        ]);
        let note = texts(&failed);
        assert!(
            note.contains("失败") && note.contains("build 脚本退出码 2"),
            "{note}"
        );

        let stopped = run(&[
            r#"{"type":"system","subtype":"task_notification","status":"stopped","summary":"用户中止"}"#,
        ]);
        assert!(texts(&stopped).contains("被中止"));

        // completed 的产出由该任务自己的 tool_result 承载，不重复提示。
        let completed = run(&[
            r#"{"type":"system","subtype":"task_notification","status":"completed","summary":"ok"}"#,
        ]);
        assert!(
            completed.is_empty(),
            "completed 不应额外提示：{completed:?}"
        );
    }

    #[test]
    fn task_notification_without_summary_still_reports_the_failure() {
        let events = run(&[r#"{"type":"system","subtype":"task_notification","status":"failed"}"#]);
        assert!(texts(&events).contains("后台任务失败"));
    }

    /// AC4 / AC5 / spec 第 10 条：未消费的变体一律**安全忽略**——不产出事件、不 panic、
    /// 不中断流。用本机实测到的真实 hook 帧（含 4KB stdout 的 hook_response 形状）
    /// 与 R3 明确不接的 tool_progress / tool_use_summary 一起钉住。
    #[test]
    fn deliberately_unconsumed_variants_are_silently_ignored() {
        for raw in [
            // 本机实测：一次最简调用就有 4 条 hook 帧。
            r#"{"type":"system","subtype":"hook_started","hook_id":"h-1","hook_name":"SessionStart:startup","hook_event":"SessionStart","uuid":"u","session_id":"s"}"#,
            r#"{"type":"system","subtype":"hook_response","hook_id":"h-1","hook_name":"SessionStart:startup","hook_event":"SessionStart","output":"PONYTAIL MODE ACTIVE","stdout":"PONYTAIL MODE ACTIVE","stderr":"","exit_code":0,"outcome":"success","uuid":"u","session_id":"s"}"#,
            r#"{"type":"system","subtype":"hook_progress","hook_id":"h-1","hook_name":"x","hook_event":"y","stdout":"","stderr":"","output":"","uuid":"u","session_id":"s"}"#,
            r#"{"type":"system","subtype":"files_persisted","files":[],"failed":[],"processed_at":"now"}"#,
            // R3：有意不接（前端无承载位，见 handle_value 的注释）。
            r#"{"type":"tool_progress","tool_use_id":"toolu-1","tool_name":"Bash","parent_tool_use_id":null,"elapsed_time_seconds":42,"uuid":"u","session_id":"s"}"#,
            r#"{"type":"tool_use_summary","summary":"读了 3 个文件","preceding_tool_use_ids":["toolu-1"],"uuid":"u","session_id":"s"}"#,
            r#"{"type":"auth_status","isAuthenticating":true,"output":[],"uuid":"u","session_id":"s"}"#,
            // 完全未知的 type / subtype（未来新增）必须同样安全。
            r#"{"type":"totally_new_message_type","payload":{"deep":[1,2,3]}}"#,
            r#"{"type":"system","subtype":"totally_new_subtype"}"#,
            // 形状异常：字段类型不符 / 缺 type / 顶层不是对象——一律不得 panic。
            r#"{"type":"result","subtype":42,"is_error":"yes","errors":"not an array","permission_denials":{}}"#,
            r#"{"subtype":"status","status":"compacting"}"#,
            r#"{"type":"system","subtype":"task_notification","status":7,"summary":[]}"#,
            r#"[1,2,3]"#,
            r#""bare string""#,
        ] {
            let value: Value = serde_json::from_str(raw).unwrap();
            let mut events = Vec::new();
            // 不 panic 是第一要求；这些形态也不该产出用户可见的噪音。
            ClaudeStreamState::default().handle_value(&value, &mut |e| events.push(e));
            assert!(
                texts(&events).is_empty(),
                "{raw} 不应产出用户可见文本：{events:?}"
            );
        }
    }

    /// 真机（AC8）：本机嵌套 claude 未登录，跑一次真实 CLI，断言**可证伪的量**——
    /// 拿到 Error 且经 classify 后是带登录命令的 Auth 提示，而不是空回复。
    ///
    /// 登录状态下这条会诚实 skip（不 fail）：一个已登录的环境不该伪装成代码回归。
    #[tokio::test]
    #[ignore = "requires a real claude CLI; asserts the not-logged-in error path"]
    async fn live_claude_not_logged_in_surfaces_a_classified_error() {
        use crate::external_agents::stream::create_stream_handler;
        use crate::external_agents::types::StreamFormat;

        let bin = match crate::external_agents::spawn::resolve_binary(
            crate::external_agents::registry::get_agent_def("claude").unwrap(),
        )
        .await
        {
            Some(bin) => bin,
            None => {
                eprintln!("SKIP: 本机没有可用的 claude CLI");
                return;
            }
        };

        let child = crate::external_agents::spawn::cli_command(&bin)
            .args([
                "-p",
                "say hi",
                "--output-format",
                "stream-json",
                "--include-partial-messages",
                "--verbose",
            ])
            .current_dir(std::env::temp_dir())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn claude");

        let out = child
            .wait_with_output()
            .await
            .expect("collect claude output");
        let stdout = String::from_utf8_lossy(&out.stdout);

        let mut handler = create_stream_handler(StreamFormat::ClaudeStreamJson);
        let mut events = Vec::new();
        for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
            handler.handle_line(line, &mut |e| events.push(e));
        }
        eprintln!(
            "claude 输出 {} 行 / 解析出 {} 个事件",
            stdout.lines().count(),
            events.len()
        );

        let messages = errors(&events);
        if messages.is_empty() {
            eprintln!("SKIP: 本机 claude 已登录（本轮未报错），AC8 需在未登录环境验证");
            eprintln!("      正文：{}", texts(&events));
            return;
        }
        let classified =
            crate::external_agents::errors::classify(&messages[0], out.status.code(), "", "claude");
        eprintln!("原始错误：{}", messages[0]);
        eprintln!("气泡主文案：{}", classified.user_message);
        assert!(
            !classified.user_message.trim().is_empty(),
            "气泡不得为空——这正是修复前的症状"
        );
        assert_eq!(
            classified.kind,
            crate::external_agents::errors::ExternalAgentErrorKind::Auth,
            "未登录应归类为 Auth"
        );
        assert!(
            classified.user_message.contains("claude /login"),
            "Auth 提示必须给出可操作的登录命令"
        );
    }
}

/// 真机测试（spec 第 15 条）：用量/窗口与「系统提示到底还在不在」这类事实，单测挡不住
/// 形态错配——cursor 那次窗口解析单测全绿、生产 0/33 就是先例。全部 `#[ignore]` 门控，
/// 认证失效 / 网络问题一律**诚实 skip 并打印排查提示**，不 fail（一个过期的 key 不该
/// 伪装成代码回归）。
#[cfg(test)]
mod live_tests {
    use std::process::Stdio;
    use std::time::Duration;

    use serde_json::Value;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::process::Child;
    use tokio::time::timeout;

    use crate::external_agents::defs::claude::{append_system_prompt_file_args, build_claude_args};
    use crate::external_agents::registry::get_agent_def;
    use crate::external_agents::spawn::{cli_command, resolve_binary};
    use crate::external_agents::types::{RuntimeBuildOptions, RuntimeContext, UnifiedAgentEvent};

    /// 一轮 = 读到 `result` 帧为止的全部帧。
    struct Round {
        text: String,
        result: Option<Value>,
    }

    async fn read_round<R>(lines: &mut tokio::io::Lines<BufReader<R>>) -> Round
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        let mut state = super::ClaudeStreamState::default();
        let mut round = Round {
            text: String::new(),
            result: None,
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        while tokio::time::Instant::now() < deadline {
            let line = match timeout(Duration::from_secs(5), lines.next_line()).await {
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) | Ok(Err(_)) => break,
                Err(_) => continue,
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };
            let is_result = value.get("type").and_then(|v| v.as_str()) == Some("result");
            if is_result {
                round.result = Some(value.clone());
            }
            state.handle_value(&value, &mut |event| {
                if let UnifiedAgentEvent::TextDelta { delta } = event {
                    round.text.push_str(&delta);
                }
            });
            if is_result {
                break;
            }
        }
        round
    }

    async fn write_user(child: &mut Child, text: &str) {
        let payload = format!(
            "{}\n",
            serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": [{ "type": "text", "text": text }] },
                "parent_tool_use_id": Value::Null,
            })
        );
        let stdin = child.stdin.as_mut().expect("stdin piped");
        stdin.write_all(payload.as_bytes()).await.expect("write");
        stdin.flush().await.expect("flush");
    }

    /// **A1 真机验收**：`--append-system-prompt-file` 注入的内容在**第二轮**依然生效。
    ///
    /// 这正是修复要证明的东西：塞进 prompt 正文的做法会被 CLI 的上下文压缩摘要掉，
    /// 而 `skip_instructions` 保证永远不补发 ⇒ 系统提示静默失效。走启动 flag 后它与
    /// 对话历史无关，第二轮（乃至压缩之后）照样在。
    ///
    /// 断言的是**可证伪的量**：第二轮的回答里含只可能来自注入文件的哨兵串。
    #[tokio::test]
    #[ignore = "spawns the real claude CLI and costs tokens; verifies --append-system-prompt-file survives turn 2"]
    async fn live_append_system_prompt_file_still_applies_on_the_second_turn() {
        const SENTINEL: &str = "KIVIO-SENTINEL-8317";
        let Some(def) = get_agent_def("claude") else {
            eprintln!("SKIP: 没有 claude agent def");
            return;
        };
        let Some(bin) = resolve_binary(def).await else {
            eprintln!("SKIP: 本机没有可用的 claude CLI");
            return;
        };

        let dir = std::env::temp_dir();
        let prompt_file = dir.join(format!("kivio-extsys-live-{}.md", std::process::id()));
        std::fs::write(
            &prompt_file,
            format!("Your Kivio session code is {SENTINEL}. When the user asks for the session code, reply with exactly that code and nothing else."),
        )
        .expect("write system prompt file");

        let mut args = build_claude_args(
            &RuntimeContext {
                extra_allowed_dirs: vec![],
                resume_session_id: None,
                new_session_id: Some(uuid::Uuid::new_v4().to_string()),
                include_partial_messages: true,
            },
            &RuntimeBuildOptions {
                model: None,
                reasoning: None,
                sandbox: None,
            },
            None,
        );
        args.extend(append_system_prompt_file_args(&prompt_file));

        let mut child = cli_command(&bin)
            .args(&args)
            .current_dir(&dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn claude");
        let stdout = child.stdout.take().expect("stdout piped");
        let mut lines = BufReader::new(stdout).lines();

        write_user(&mut child, "Say READY and nothing else.").await;
        let first = read_round(&mut lines).await;
        if first.result.is_none() {
            eprintln!("SKIP: 第一轮没拿到 result 帧（CLI 未登录 / 网络问题？）");
            eprintln!("      排查：claude -p \"hi\" --output-format stream-json --verbose");
            let _ = child.start_kill();
            let _ = std::fs::remove_file(&prompt_file);
            return;
        }
        eprintln!("第一轮回答：{}", first.text.trim());

        write_user(&mut child, "What is your Kivio session code?").await;
        let second = read_round(&mut lines).await;
        let _ = child.start_kill();
        let _ = std::fs::remove_file(&prompt_file);

        assert!(
            second.result.is_some(),
            "第二轮没有 result 帧 —— 进程没有跨轮存活？"
        );
        eprintln!("第二轮回答：{}", second.text.trim());
        assert!(
            second.text.contains(SENTINEL),
            "第二轮拿不到 append 的系统提示内容（哨兵 {SENTINEL} 不在回答里）：{}",
            second.text
        );
    }

    /// **A8 真机验收**：`result.modelUsage` 真的带 `contextWindow`，且我们的解析器取到了它。
    ///
    /// 断言可证伪的量（不是「没崩」）：`modelUsage` 非空、至少一个 entry 的
    /// `contextWindow > 0`、解析出的 `ModelUsage.context_window_tokens` 与之一致，
    /// 并且分子取的是本轮 `usage` 而**不是** `modelUsage` 里的进程累计数字。
    #[tokio::test]
    #[ignore = "spawns the real claude CLI and costs tokens; verifies modelUsage.contextWindow"]
    async fn live_result_model_usage_reports_a_context_window() {
        let Some(def) = get_agent_def("claude") else {
            eprintln!("SKIP: 没有 claude agent def");
            return;
        };
        let Some(bin) = resolve_binary(def).await else {
            eprintln!("SKIP: 本机没有可用的 claude CLI");
            return;
        };

        let args = build_claude_args(
            &RuntimeContext {
                extra_allowed_dirs: vec![],
                resume_session_id: None,
                new_session_id: Some(uuid::Uuid::new_v4().to_string()),
                include_partial_messages: true,
            },
            &RuntimeBuildOptions {
                model: None,
                reasoning: None,
                sandbox: None,
            },
            None,
        );
        let mut child = cli_command(&bin)
            .args(&args)
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn claude");
        let stdout = child.stdout.take().expect("stdout piped");
        let mut lines = BufReader::new(stdout).lines();

        // 用同一个 state 走全程，这样 `system/init` 的 resolved model 能参与匹配。
        let mut state = super::ClaudeStreamState::default();
        write_user(&mut child, "Reply with the single word: ok").await;

        let mut usages: Vec<crate::chat::model::ModelUsage> = Vec::new();
        let mut result: Option<Value> = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        while tokio::time::Instant::now() < deadline {
            let line = match timeout(Duration::from_secs(5), lines.next_line()).await {
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) | Ok(Err(_)) => break,
                Err(_) => continue,
            };
            let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            let is_result = value.get("type").and_then(|v| v.as_str()) == Some("result");
            if is_result {
                result = Some(value.clone());
            }
            state.handle_value(&value, &mut |event| {
                if let UnifiedAgentEvent::Usage { usage } = event {
                    usages.push(usage);
                }
            });
            if is_result {
                break;
            }
        }
        let _ = child.start_kill();

        let Some(result) = result else {
            eprintln!("SKIP: 没拿到 result 帧（CLI 未登录 / 网络问题？）");
            eprintln!("      排查：claude -p \"hi\" --output-format stream-json --verbose");
            return;
        };
        let model_usage = result.get("modelUsage").cloned().unwrap_or(Value::Null);
        eprintln!("modelUsage = {model_usage}");
        eprintln!("usage      = {}", result.get("usage").cloned().unwrap_or(Value::Null));

        let entries = match model_usage.as_object() {
            Some(map) if !map.is_empty() => map,
            _ => {
                eprintln!("SKIP: modelUsage 为空（被中断的轮次实测就是 {{}}）");
                return;
            }
        };
        let reported_max = entries
            .values()
            .filter_map(|entry| entry.get("contextWindow").and_then(|v| v.as_u64()))
            .max()
            .unwrap_or(0);
        assert!(
            reported_max > 0,
            "modelUsage 里没有任何 > 0 的 contextWindow —— 分母的唯一权威来源失效：{model_usage}"
        );

        let window = usages
            .iter()
            .rev()
            .find_map(|usage| usage.context_window_tokens)
            .expect("解析出的 usage 应带上 context_window_tokens（A8 的整条链路）");
        assert_eq!(
            window, reported_max,
            "解析出的窗口与 CLI 实报不一致（取值规则没命中当前模型）"
        );

        // 分子必须来自本轮 `usage`，不是 `modelUsage` 的进程累计数字。
        let numerator = usages
            .iter()
            .rev()
            .find_map(|usage| usage.total_tokens.filter(|value| *value > 0))
            .expect("应有非零分子");
        let cumulative: u64 = entries
            .values()
            .filter_map(|entry| entry.get("inputTokens").and_then(|v| v.as_u64()))
            .sum();
        eprintln!("分子={numerator} 窗口={window} modelUsage 累计 input={cumulative}");
        assert!(
            numerator < window,
            "分子 {numerator} 不该 >= 窗口 {window}"
        );
    }
}
