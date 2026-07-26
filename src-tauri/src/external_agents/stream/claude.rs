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
/// `iterations` 缺失 / 为空数组 / 末项不是对象时，退回 `usage` 顶层字段。
fn claude_result_usage_snapshot(usage: &Value) -> &Value {
    usage
        .get("iterations")
        .and_then(|v| v.as_array())
        .and_then(|items| items.iter().rev().find(|item| item.is_object()))
        .unwrap_or(usage)
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
    current_message_id: Option<String>,
    blocks: HashMap<String, PendingContentBlock>,
    streamed_tool_use_ids: HashSet<String>,
}

impl ClaudeStreamState {
    pub fn handle_value(&mut self, value: &Value, sink: &mut dyn FnMut(UnifiedAgentEvent)) {
        let obj = match value.as_object() {
            Some(o) => o,
            None => return,
        };
        let kind = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "system" => {
                match obj.get("subtype").and_then(|v| v.as_str()) {
                    Some("init") => {
                        let commands = parse_slash_commands_from_init(value);
                        if !commands.is_empty() {
                            sink(UnifiedAgentEvent::SlashCommands { commands });
                        }
                    }
                    // claude 自己触发的上下文压缩。官方 SDK 类型
                    // `SDKCompactBoundaryMessage.compact_metadata` 只有 `trigger` 与
                    // `pre_tokens`——**没有 post_tokens**，别去读一个不存在的字段。
                    // 压缩后的真实占用会由紧随其后的 `message_start.message.usage`
                    // 上报（服务端算的），用量条靠那条自愈，这里只负责让压缩这件事**可见**。
                    Some("compact_boundary") => {
                        let metadata = obj.get("compact_metadata");
                        let trigger = metadata
                            .and_then(|m| m.get("trigger"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("auto")
                            .to_string();
                        let pre_tokens = metadata
                            .and_then(|m| m.get("pre_tokens"))
                            .and_then(|v| v.as_u64());
                        sink(UnifiedAgentEvent::CliCompacted {
                            trigger,
                            pre_tokens,
                        });
                    }
                    // `status`（官方 `SDKStatusMessage`）：`SDKStatus` 目前只有 `compacting`
                    // 与 `null`，但实测还会出现协议里未声明的值（本机见到 `requesting`），
                    // 故按开放字符串处理、不做白名单。
                    //
                    // 只有 `compacting` 转成事件：它与 `compact_boundary` 是同一件事的
                    // 「开始 / 结束」两端，而**开始**这一端在长压缩里是唯一的可见信号。
                    // 用 `CliCompacted` 会在前端插入一条假的压缩分隔线（压缩尚未完成），
                    // 所以走 `TextDelta` 提示，不新增事件变体。其余 status 值是纯进度噪音
                    // （一轮内多条），无展示位，显式忽略。
                    Some("status")
                        if obj.get("status").and_then(|v| v.as_str()) == Some("compacting") =>
                    {
                        sink(UnifiedAgentEvent::TextDelta {
                            delta: "> ⏳ claude 正在压缩上下文…\n\n".to_string(),
                        });
                    }
                    // 其余 status 值（含实测到的 `requesting` 与协议里的 `null`）是纯进度噪音，
                    // 一轮内多条且无展示位——显式忽略，不要落进下面的未知分支。
                    Some("status") => {}
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
                                            sink(UnifiedAgentEvent::TextDelta {
                                                delta: text.to_string(),
                                            });
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
                // usage 解析保持原样（上一轮刚修好：iterations 末项 + cache 四字段），
                // 且**先于**错误分支——失败轮次的用量同样要计入用量条。
                let parts = obj
                    .get("usage")
                    .map(|usage| claude_usage_parts(claude_result_usage_snapshot(usage)))
                    .unwrap_or_default();
                sink(UnifiedAgentEvent::Usage {
                    usage: usage_from_parts(parts),
                });
                // 被权限规则拒掉的工具调用（R4）：无论本轮成功与否都提示。
                if let Some(note) = permission_denials_note(obj) {
                    sink(UnifiedAgentEvent::TextDelta { delta: note });
                }
                // R1：`subtype` 与 `is_error` 任一指示失败都要产出 Error。裸文案交给
                // `run.rs` 的 `errors::classify`（spec 第 5 条）——它会把 "Not logged in"
                // 归成 Auth 并附上 `claude /login`，而不是把英文原句直接落进气泡。
                if let Some(message) = claude_result_error_message(obj) {
                    sink(UnifiedAgentEvent::Error { message });
                }
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
                                sink(UnifiedAgentEvent::TextDelta {
                                    delta: text.to_string(),
                                });
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
    /// 字段按官方 SDK 的 `SDKCompactBoundaryMessage.compact_metadata` 构造
    /// （`@anthropic-ai/claude-agent-sdk` 的 sdk.d.ts：只有 trigger + pre_tokens）。
    #[test]
    fn parses_cli_triggered_compaction() {
        let raw = r#"{"type":"system","subtype":"compact_boundary",
            "compact_metadata":{"trigger":"auto","pre_tokens":152340},
            "uuid":"u-1","session_id":"s-1"}"#;
        let value: Value = serde_json::from_str(raw).unwrap();
        let mut events = Vec::new();
        ClaudeStreamState::default().handle_value(&value, &mut |e| events.push(e));
        assert!(
            events.iter().any(|event| matches!(
                event,
                UnifiedAgentEvent::CliCompacted { trigger, pre_tokens }
                    if trigger == "auto" && *pre_tokens == Some(152_340)
            )),
            "compact_boundary 未产出 CliCompacted：{events:?}"
        );
    }

    #[test]
    fn compaction_without_metadata_still_reports_the_event() {
        // 元数据缺失（字段可选/未来变形）时也不能把压缩这件事整个丢掉——
        // trigger 退 auto、pre_tokens 留 None，事件照发。
        let raw = r#"{"type":"system","subtype":"compact_boundary"}"#;
        let value: Value = serde_json::from_str(raw).unwrap();
        let mut events = Vec::new();
        ClaudeStreamState::default().handle_value(&value, &mut |e| events.push(e));
        assert!(events.iter().any(|event| matches!(
            event,
            UnifiedAgentEvent::CliCompacted { trigger, pre_tokens }
                if trigger == "auto" && pre_tokens.is_none()
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

    /// 压缩后用量条的自愈路径：`compact_boundary` **不带** post_tokens
    /// （官方 SDK 只给 pre_tokens），压缩后的真实占用靠紧随其后的
    /// `message_start.message.usage` 上报。这条钉住「压缩后能拿到新数字」。
    #[test]
    fn usage_recovers_from_message_start_after_compaction() {
        let events = run(&[
            r#"{"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"auto","pre_tokens":152340}}"#,
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
    /// `requesting` 是本机实测到的真实值——官方 `SDKStatus` 类型里**没有**它，
    /// 所以这里刻意不做白名单校验，只保证不刷屏。
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

    #[test]
    fn task_notification_reports_only_failure_and_stop() {
        let failed = run(&[
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
