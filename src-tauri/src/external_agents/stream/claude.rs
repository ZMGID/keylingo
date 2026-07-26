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
                if obj.get("subtype").and_then(|v| v.as_str()) == Some("init") {
                    let commands = parse_slash_commands_from_init(value);
                    if !commands.is_empty() {
                        sink(UnifiedAgentEvent::SlashCommands { commands });
                    }
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
                let parts = obj
                    .get("usage")
                    .map(|usage| claude_usage_parts(claude_result_usage_snapshot(usage)))
                    .unwrap_or_default();
                sink(UnifiedAgentEvent::Usage {
                    usage: usage_from_parts(parts),
                });
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
}
