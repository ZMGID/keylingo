//! 从本地 CLI 导入对话——历史解析。
//!
//! 把 CLI 自己的 transcript 解析成 Kivio 的 `ChatMessage`。契约见
//! [ADR-0002](../../../docs/adr/0002-imported-history-is-a-snapshot.md)：
//!
//! - 只认四类块：`text` / `tool_use` / `tool_result` / `thinking`，外加图片。
//! - 子 agent 分支（`isSidechain`）、hook 注入、`file-history-snapshot` 等内部账务一律丢弃。
//! - **`tool_result` 正文截断到 2KB**。这份快照只用于显示、不参与模型输入（续聊时模型读的是
//!   CLI 那边那份完整历史），所以截断零风险；不截的话对话 JSON 会跟着 transcript 一起膨胀到几 MB。
//!
//! 解析器是**纯函数**：不碰 `AppHandle`、不写盘。内联图片原样吐给调用方，由导入命令决定
//! 落到哪个对话的附件目录——那才是知道对话 id 的地方。

use serde_json::Value;
use uuid::Uuid;

use crate::chat::types::{
    ChatMessage, ChatMessageSegment, ChatMessageSegmentKind, ChatMessageSegmentPhase,
    ToolCallRecord, ToolCallStatus,
};

/// `tool_result` 正文保留的字节上限。
pub const TOOL_RESULT_CAP_BYTES: usize = 2048;

/// transcript 里的一张内联图片，尚未落盘。
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedImage {
    pub media_type: String,
    pub data_base64: String,
}

/// 一条解析出来的消息 + 它携带的、还没落盘的图片。
#[derive(Debug, Clone)]
pub struct ImportedMessage {
    pub message: ChatMessage,
    pub images: Vec<ImportedImage>,
}

/// 按字符边界截断，超出时追加省略说明。
///
/// 按**字节**上限但在**字符**边界切——直接 `&s[..cap]` 会在多字节字符中间切断导致 panic。
fn truncate_tool_result(text: &str) -> String {
    if text.len() <= TOOL_RESULT_CAP_BYTES {
        return text.to_string();
    }
    let mut end = TOOL_RESULT_CAP_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n\n…（导入时已截断，完整内容在 CLI 那边）", &text[..end])
}

/// 累积中的一条消息。claude 的一个回合会拆成多条 `assistant` 记录，要合并成一条 UI 消息。
#[derive(Default)]
struct Pending {
    role: String,
    text: String,
    reasoning: String,
    tool_calls: Vec<ToolCallRecord>,
    segments: Vec<ChatMessageSegment>,
    images: Vec<ImportedImage>,
    order: u32,
    /// transcript 里那条记录的时间，epoch **毫秒**；取不到就 0。
    /// 注意 `ChatMessage.timestamp` 用的是**秒**（前端 `nowSeconds()`），`finish()` 里换算。
    timestamp: i64,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
            && self.reasoning.trim().is_empty()
            && self.tool_calls.is_empty()
            && self.images.is_empty()
    }

    fn push_segment(
        &mut self,
        kind: ChatMessageSegmentKind,
        text: Option<String>,
        tool: Option<String>,
    ) {
        let order = self.order;
        self.order += 1;
        self.segments.push(ChatMessageSegment {
            id: Uuid::new_v4().to_string(),
            kind,
            phase: ChatMessageSegmentPhase::Plain,
            order,
            step_number: None,
            round: None,
            text,
            tool_call_id: tool,
        });
    }

    fn finish(self) -> Option<ImportedMessage> {
        if self.is_empty() {
            return None;
        }
        let reasoning =
            (!self.reasoning.trim().is_empty()).then(|| self.reasoning.trim().to_string());
        Some(ImportedMessage {
            message: ChatMessage {
                id: Uuid::new_v4().to_string(),
                role: self.role,
                content: self.text.trim().to_string(),
                attachments: Vec::new(),
                reasoning,
                artifacts: Vec::new(),
                tool_calls: self.tool_calls,
                segments: self.segments,
                agent_plan: None,
                api_messages: Vec::new(),
                // 快照不参与模型输入（ADR-0002）——`model_messages` 刻意留空。
                model_messages: Vec::new(),
                active_skill_id: None,
                run_entry: None,
                stream_outcome: None,
                degraded: None,
                usage: None,
                anchor_usage: None,
                group_id: None,
                // 导入的消息不归属任何 Kivio provider——续聊由 CLI 承担（ADR-0001）。
                provider_id: None,
                model: None,
                // 毫秒 → 秒：`ChatMessage.timestamp` 与前端 `nowSeconds()` 对齐，
                // 直接塞毫秒会让导入的消息显示成公元五万年。
                timestamp: self.timestamp / 1000,
            },
            images: self.images,
        })
    }
}

/// 解析 claude 的 `<session>.jsonl`。
///
/// claude 把一个回合拆成多条 `assistant` 记录（文本、思考、工具各一条），而 `tool_result`
/// 是以 `role: user` 的记录回来的——所以**不能**见到 `user` 就当成一条新的用户消息，
/// 否则每次工具调用都会在界面上凭空多出一条空白用户气泡。判据是这条 `user` 记录里
/// 有没有真正的文本块。
pub fn parse_claude_history(raw: &str) -> Vec<ImportedMessage> {
    let mut out: Vec<ImportedMessage> = Vec::new();
    let mut pending = Pending::default();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        // 子 agent 分支不还原（Kivio 没有"从历史重建嵌套卡片"的渲染路径）。
        if entry.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let entry_ms = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(crate::external_agents::import::parse_rfc3339_ms)
            .unwrap_or(0);
        let entry_type = entry
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if entry_type != "user" && entry_type != "assistant" {
            continue; // queue-operation / mode / ai-title / file-history-snapshot 等内部账务。
        }
        let Some(content) = entry.get("message").and_then(|m| m.get("content")) else {
            continue;
        };

        // `content` 是裸字符串时只可能是用户输入的纯文本。
        if let Some(text) = content.as_str() {
            if text.trim().is_empty() {
                continue;
            }
            // 只在**角色变化**时收尾。claude 会把 `<system-reminder>` 这类注入当成独立的
            // user 记录发出来，一个回合的输入常常横跨好几条——按条切会在界面上劈成一串气泡。
            if pending.role != "user" {
                if let Some(done) = std::mem::take(&mut pending).finish() {
                    out.push(done);
                }
                pending.role = "user".to_string();
                pending.timestamp = entry_ms;
            }
            if !pending.text.is_empty() {
                pending.text.push('\n');
            }
            pending.text.push_str(text.trim());
            pending.push_segment(
                ChatMessageSegmentKind::Text,
                Some(text.trim().to_string()),
                None,
            );
            continue;
        }

        let Some(blocks) = content.as_array() else {
            continue;
        };

        if entry_type == "user" {
            let has_text = blocks.iter().any(|b| {
                b.get("type").and_then(Value::as_str) == Some("text")
                    && !b
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .trim()
                        .is_empty()
            });
            if has_text && pending.role != "user" {
                if let Some(done) = std::mem::take(&mut pending).finish() {
                    out.push(done);
                }
                pending.role = "user".to_string();
                pending.timestamp = entry_ms;
            }
            for block in blocks {
                apply_user_block(block, &mut pending, &mut out);
            }
            continue;
        }

        // assistant：与前一条 assistant 合并；前面是用户消息则先收尾。
        if pending.role != "assistant" {
            if let Some(done) = std::mem::take(&mut pending).finish() {
                out.push(done);
            }
            pending.role = "assistant".to_string();
            pending.timestamp = entry_ms;
        }
        for block in blocks {
            apply_assistant_block(block, &mut pending);
        }
    }

    if let Some(done) = pending.finish() {
        out.push(done);
    }
    coalesce_adjacent(out)
}

fn apply_user_block(block: &Value, pending: &mut Pending, out: &mut [ImportedMessage]) {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => {
            let text = block
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !text.trim().is_empty() {
                if !pending.text.is_empty() {
                    pending.text.push('\n');
                }
                pending.text.push_str(text.trim());
                pending.push_segment(
                    ChatMessageSegmentKind::Text,
                    Some(text.trim().to_string()),
                    None,
                );
            }
        }
        Some("image") => {
            if let Some(image) = image_from_block(block) {
                pending.images.push(image);
            }
        }
        Some("tool_result") => {
            let Some(tool_use_id) = block.get("tool_use_id").and_then(Value::as_str) else {
                return;
            };
            let text = truncate_tool_result(&tool_result_text(block));
            let is_error = block.get("is_error").and_then(Value::as_bool) == Some(true);
            // 结果要回填到**已经收尾**的那条 assistant 消息上——tool_result 总是晚于发起它的
            // assistant 记录到达，此时那条消息多半已经被 flush 进 out 了。
            for record in pending.tool_calls.iter_mut().chain(
                out.iter_mut()
                    .rev()
                    .flat_map(|m| m.message.tool_calls.iter_mut()),
            ) {
                if record.id == tool_use_id {
                    record.status = if is_error {
                        ToolCallStatus::Error
                    } else {
                        ToolCallStatus::Success
                    };
                    if is_error {
                        record.error = Some(text.clone());
                    } else {
                        record.result_preview = Some(text.clone());
                    }
                    return;
                }
            }
        }
        _ => {}
    }
}

fn apply_assistant_block(block: &Value, pending: &mut Pending) {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => {
            let text = block
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !text.trim().is_empty() {
                if !pending.text.is_empty() {
                    pending.text.push('\n');
                }
                pending.text.push_str(text.trim());
                pending.push_segment(
                    ChatMessageSegmentKind::Text,
                    Some(text.trim().to_string()),
                    None,
                );
            }
        }
        Some("thinking") => {
            let text = block
                .get("thinking")
                .or_else(|| block.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !text.trim().is_empty() {
                if !pending.reasoning.is_empty() {
                    pending.reasoning.push('\n');
                }
                pending.reasoning.push_str(text.trim());
                pending.push_segment(
                    ChatMessageSegmentKind::Reasoning,
                    Some(text.trim().to_string()),
                    None,
                );
            }
        }
        Some("tool_use") => {
            let id = block
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if id.is_empty() {
                return;
            }
            let name = block
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let arguments = block
                .get("input")
                .map(|v| v.to_string())
                .unwrap_or_else(|| "{}".to_string());
            pending.push_segment(ChatMessageSegmentKind::Tool, None, Some(id.clone()));
            pending.tool_calls.push(ToolCallRecord {
                id,
                name,
                source: "external_cli".to_string(),
                server_id: None,
                arguments,
                // 先记成 Success；配对的 tool_result 到达时会改写成真实状态。
                // 没等到结果的（会话被中断）就停在这里——比标成 Running 更贴近"已经结束了"的事实。
                status: ToolCallStatus::Success,
                result_preview: None,
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
        }
        _ => {}
    }
}

/// `tool_result.content` 可能是字符串，也可能是块数组。
fn tool_result_text(block: &Value) -> String {
    let Some(content) = block.get("content") else {
        return String::new();
    };
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    let Some(items) = content.as_array() else {
        return String::new();
    };
    items
        .iter()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn image_from_block(block: &Value) -> Option<ImportedImage> {
    let source = block.get("source")?;
    let data = source.get("data").and_then(Value::as_str)?;
    if data.is_empty() {
        return None;
    }
    Some(ImportedImage {
        media_type: source
            .get("media_type")
            .and_then(Value::as_str)
            .unwrap_or("image/png")
            .to_string(),
        data_base64: data.to_string(),
    })
}

impl Pending {
    fn push_text(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        self.text.push_str(text);
        self.push_segment(ChatMessageSegmentKind::Text, Some(text.to_string()), None);
    }

    fn push_reasoning(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if !self.reasoning.is_empty() {
            self.reasoning.push('\n');
        }
        self.reasoning.push_str(text);
        self.push_segment(
            ChatMessageSegmentKind::Reasoning,
            Some(text.to_string()),
            None,
        );
    }

    fn push_tool(&mut self, id: String, name: String, arguments: String) {
        if id.is_empty() {
            return;
        }
        self.push_segment(ChatMessageSegmentKind::Tool, None, Some(id.clone()));
        self.tool_calls.push(ToolCallRecord {
            id,
            name,
            source: "external_cli".to_string(),
            server_id: None,
            arguments,
            status: ToolCallStatus::Success,
            result_preview: None,
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
    }
}

/// 把工具结果回填到对应的 `ToolCallRecord` 上。
///
/// 结果总是晚于发起它的 assistant 记录到达，那条消息可能已经被收进 `out` 了，所以要从
/// 当前累积的这条一路往前找。找不到就丢弃——宁可少显示一个结果，也不要凭空造一条工具卡片。
fn resolve_tool_result(
    pending: &mut Pending,
    out: &mut [ImportedMessage],
    call_id: &str,
    text: String,
    is_error: bool,
) {
    for record in pending.tool_calls.iter_mut().chain(
        out.iter_mut()
            .rev()
            .flat_map(|m| m.message.tool_calls.iter_mut()),
    ) {
        if record.id == call_id {
            record.status = if is_error {
                ToolCallStatus::Error
            } else {
                ToolCallStatus::Success
            };
            if is_error {
                record.error = Some(text);
            } else {
                record.result_preview = Some(text);
            }
            return;
        }
    }
}

/// 切到指定角色：角色变了就把当前这条收尾推进 `out`。
fn switch_role(pending: &mut Pending, out: &mut Vec<ImportedMessage>, role: &str, ts: i64) {
    if pending.role != role {
        if let Some(done) = std::mem::take(pending).finish() {
            out.push(done);
        }
        pending.role = role.to_string();
        pending.timestamp = ts;
    }
}

/// 合并相邻的同角色消息，并重排段落序号。
///
/// **为什么需要这一步**：光靠"角色变了才收尾"不够。transcript 里会出现**内容为空**的记录
/// （codex 在中断、上下文压缩后就有空的 `user_message`），它把角色切过去、又因为没内容而
/// 被丢弃，结果前后两条 assistant 在 `out` 里挨在了一起。实测 24MB 的真 codex 会话就是
/// 这么炸的。与其在每个解析器里各打一个补丁，不如在出口统一收一次。
fn coalesce_adjacent(messages: Vec<ImportedMessage>) -> Vec<ImportedMessage> {
    let mut out: Vec<ImportedMessage> = Vec::with_capacity(messages.len());
    for item in messages {
        let Some(last) = out.last_mut() else {
            out.push(item);
            continue;
        };
        if last.message.role != item.message.role {
            out.push(item);
            continue;
        }
        if !item.message.content.trim().is_empty() {
            if !last.message.content.is_empty() {
                last.message.content.push('\n');
            }
            last.message.content.push_str(item.message.content.trim());
        }
        if let Some(reasoning) = item.message.reasoning {
            match last.message.reasoning.as_mut() {
                Some(existing) => {
                    existing.push('\n');
                    existing.push_str(&reasoning);
                }
                None => last.message.reasoning = Some(reasoning),
            }
        }
        let base = last.message.segments.len() as u32;
        for mut segment in item.message.segments {
            segment.order += base;
            last.message.segments.push(segment);
        }
        last.message.tool_calls.extend(item.message.tool_calls);
        last.images.extend(item.images);
    }
    out
}

/// 从 `data:image/png;base64,XXXX` 取出图片。grok 用这种写法，claude 用 `source.data`，
/// 两边不能共用一个提取函数。
fn image_from_data_url(url: &str) -> Option<ImportedImage> {
    let rest = url.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    if data.is_empty() || !meta.contains("base64") {
        return None;
    }
    let media_type = meta.split(';').next().unwrap_or("image/png");
    Some(ImportedImage {
        media_type: if media_type.is_empty() {
            "image/png".to_string()
        } else {
            media_type.to_string()
        },
        data_base64: data.to_string(),
    })
}

/// 解析 grok 的 `<session>/chat_history.jsonl`。
///
/// 每行一个 `{type, ...}`：`user`（content 是块数组，text / image data URL）、`assistant`
/// （content 是字符串 + OpenAI 风格的 `tool_calls`）、`reasoning`（正文在 `summary[].text`，
/// 另有 `encrypted_content` 不用管）、`tool_result`（`tool_call_id` + `content`）。
/// `system` 和 `backend_tool_call`（内置联网搜索）跳过。
pub fn parse_grok_history(raw: &str) -> Vec<ImportedMessage> {
    let mut out: Vec<ImportedMessage> = Vec::new();
    let mut pending = Pending::default();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match entry.get("type").and_then(Value::as_str) {
            Some("user") => {
                switch_role(&mut pending, &mut out, "user", 0);
                match entry.get("content") {
                    Some(Value::String(text)) => pending.push_text(text),
                    Some(Value::Array(blocks)) => {
                        for block in blocks {
                            match block.get("type").and_then(Value::as_str) {
                                Some("text") => pending.push_text(
                                    block.get("text").and_then(Value::as_str).unwrap_or(""),
                                ),
                                Some("image") => {
                                    if let Some(image) = block
                                        .get("url")
                                        .and_then(Value::as_str)
                                        .and_then(image_from_data_url)
                                    {
                                        pending.images.push(image);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some("assistant") => {
                switch_role(&mut pending, &mut out, "assistant", 0);
                if let Some(text) = entry.get("content").and_then(Value::as_str) {
                    pending.push_text(text);
                }
                if let Some(calls) = entry.get("tool_calls").and_then(Value::as_array) {
                    for call in calls {
                        pending.push_tool(
                            call.get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            call.get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown")
                                .to_string(),
                            call.get("arguments")
                                .and_then(Value::as_str)
                                .unwrap_or("{}")
                                .to_string(),
                        );
                    }
                }
            }
            Some("reasoning") => {
                switch_role(&mut pending, &mut out, "assistant", 0);
                if let Some(items) = entry.get("summary").and_then(Value::as_array) {
                    for item in items {
                        pending
                            .push_reasoning(item.get("text").and_then(Value::as_str).unwrap_or(""));
                    }
                }
            }
            Some("tool_result") => {
                let Some(call_id) = entry.get("tool_call_id").and_then(Value::as_str) else {
                    continue;
                };
                let text = truncate_tool_result(
                    entry.get("content").and_then(Value::as_str).unwrap_or(""),
                );
                resolve_tool_result(&mut pending, &mut out, call_id, text, false);
            }
            _ => {}
        }
    }

    if let Some(done) = pending.finish() {
        out.push(done);
    }
    coalesce_adjacent(out)
}

/// 解析 codex 的 `rollout-*.jsonl`。
///
/// 用于显示的正文取 `event_msg` 的 `user_message` / `agent_message`——`response_item.message`
/// 里混着 `role: developer` 的权限说明等注入文本，不是对话内容。
///
/// 工具有两套：`function_call` / `function_call_output`（普通工具）和 `custom_tool_call` /
/// `custom_tool_call_output`（`apply_patch` 这类）。两套都要认，只认一套的话打补丁的那些
/// 回合在界面上会凭空少掉。
pub fn parse_codex_history(raw: &str) -> Vec<ImportedMessage> {
    let mut out: Vec<ImportedMessage> = Vec::new();
    let mut pending = Pending::default();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let ts = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(crate::external_agents::import::parse_rfc3339_ms)
            .unwrap_or(0);
        let Some(payload) = entry.get("payload") else {
            continue;
        };
        let outer = entry.get("type").and_then(Value::as_str).unwrap_or("");
        let inner = payload.get("type").and_then(Value::as_str).unwrap_or("");

        match (outer, inner) {
            ("event_msg", "user_message") => {
                switch_role(&mut pending, &mut out, "user", ts);
                pending.push_text(payload.get("message").and_then(Value::as_str).unwrap_or(""));
            }
            ("event_msg", "agent_message") => {
                switch_role(&mut pending, &mut out, "assistant", ts);
                pending.push_text(payload.get("message").and_then(Value::as_str).unwrap_or(""));
            }
            ("response_item", "reasoning") => {
                switch_role(&mut pending, &mut out, "assistant", ts);
                if let Some(items) = payload.get("summary").and_then(Value::as_array) {
                    for item in items {
                        pending
                            .push_reasoning(item.get("text").and_then(Value::as_str).unwrap_or(""));
                    }
                }
            }
            ("response_item", "function_call") => {
                switch_role(&mut pending, &mut out, "assistant", ts);
                pending.push_tool(
                    payload
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    payload
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                    payload
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}")
                        .to_string(),
                );
            }
            ("response_item", "custom_tool_call") => {
                switch_role(&mut pending, &mut out, "assistant", ts);
                pending.push_tool(
                    payload
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    payload
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                    payload
                        .get("input")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                );
            }
            ("response_item", "function_call_output")
            | ("response_item", "custom_tool_call_output") => {
                let Some(call_id) = payload.get("call_id").and_then(Value::as_str) else {
                    continue;
                };
                let text = truncate_tool_result(
                    payload.get("output").and_then(Value::as_str).unwrap_or(""),
                );
                resolve_tool_result(&mut pending, &mut out, call_id, text, false);
            }
            _ => {}
        }
    }

    if let Some(done) = pending.finish() {
        out.push(done);
    }
    coalesce_adjacent(out)
}

/// 把 ACP `session/load` 重放出来的 `session/update` 转成消息。
///
/// 只认这几种 `sessionUpdate`：`user_message_chunk` / `agent_message_chunk` /
/// `agent_thought_chunk` / `tool_call` / `tool_call_update`。其余（`available_commands_update`、
/// `plan` 等）是运行时状态，不是对话内容。
///
/// **chunk 是逐片来的**，同类连续片段要拼成一条消息——每片单独成条的话，一句话会在界面上
/// 碎成几十条气泡。角色切换靠 chunk 的类型变化驱动，出口再统一 `coalesce_adjacent` 兜一道。
pub fn parse_acp_updates(updates: &[Value]) -> Vec<ImportedMessage> {
    let mut out: Vec<ImportedMessage> = Vec::new();
    let mut pending = Pending::default();

    for update in updates {
        let kind = update
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .unwrap_or("");
        match kind {
            "user_message_chunk" => {
                switch_role(&mut pending, &mut out, "user", 0);
                append_acp_text(&mut pending, update, false);
            }
            "agent_message_chunk" => {
                switch_role(&mut pending, &mut out, "assistant", 0);
                append_acp_text(&mut pending, update, false);
            }
            "agent_thought_chunk" => {
                switch_role(&mut pending, &mut out, "assistant", 0);
                append_acp_text(&mut pending, update, true);
            }
            "tool_call" => {
                switch_role(&mut pending, &mut out, "assistant", 0);
                let id = update
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                // 没有 toolCallId 就自造一个：卡片仍然能显示，只是后续的 tool_call_update
                // 匹配不上——比整条工具调用消失好。
                let id = if id.is_empty() {
                    format!("acp-{}", Uuid::new_v4())
                } else {
                    id
                };
                let name = update
                    .get("title")
                    .and_then(Value::as_str)
                    .or_else(|| update.get("kind").and_then(Value::as_str))
                    .unwrap_or("tool")
                    .to_string();
                let arguments = update
                    .get("rawInput")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "{}".to_string());
                pending.push_tool(id, name, arguments);
                apply_acp_tool_status(&mut pending, &mut out, update);
            }
            "tool_call_update" => apply_acp_tool_status(&mut pending, &mut out, update),
            _ => {}
        }
    }

    if let Some(done) = pending.finish() {
        out.push(done);
    }
    coalesce_adjacent(out)
}

/// chunk 的 `content` 可能是 `{type:"text",text}`、块数组，或裸字符串。
fn append_acp_text(pending: &mut Pending, update: &Value, is_reasoning: bool) {
    let Some(content) = update.get("content") else {
        return;
    };
    let mut collected = String::new();
    match content {
        Value::String(text) => collected.push_str(text),
        Value::Object(_) => {
            if let Some(text) = content.get("text").and_then(Value::as_str) {
                collected.push_str(text);
            }
        }
        Value::Array(items) => {
            for item in items {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    collected.push_str(text);
                }
            }
        }
        _ => {}
    }
    if collected.trim().is_empty() {
        return;
    }
    // chunk 是逐片来的：直接续在末尾，不额外插换行，也不新起一个段落——
    // 每片一个段落会让一句话在界面上碎成一串。
    let target = if is_reasoning {
        &mut pending.reasoning
    } else {
        &mut pending.text
    };
    let fresh = target.is_empty();
    target.push_str(&collected);
    if fresh {
        let kind = if is_reasoning {
            ChatMessageSegmentKind::Reasoning
        } else {
            ChatMessageSegmentKind::Text
        };
        pending.push_segment(kind, Some(String::new()), None);
    }
    // 段落文本跟着累积的正文走，避免和 `content` 不一致。
    let latest = if is_reasoning {
        pending.reasoning.clone()
    } else {
        pending.text.clone()
    };
    let want = if is_reasoning {
        ChatMessageSegmentKind::Reasoning
    } else {
        ChatMessageSegmentKind::Text
    };
    if let Some(segment) = pending.segments.iter_mut().rev().find(|s| s.kind == want) {
        segment.text = Some(latest);
    }
}

fn apply_acp_tool_status(pending: &mut Pending, out: &mut Vec<ImportedMessage>, update: &Value) {
    let Some(id) = update.get("toolCallId").and_then(Value::as_str) else {
        return;
    };
    let status = update.get("status").and_then(Value::as_str).unwrap_or("");
    let text = update
        .get("content")
        .map(collect_acp_tool_output)
        .unwrap_or_default();
    if status.is_empty() && text.is_empty() {
        return;
    }
    let is_error = status == "failed" || status == "error";
    if text.is_empty() {
        // 只有状态没有正文：单独标一下，别把 result_preview 写成空串。
        for record in pending.tool_calls.iter_mut().chain(
            out.iter_mut()
                .rev()
                .flat_map(|m| m.message.tool_calls.iter_mut()),
        ) {
            if record.id == id {
                record.status = if is_error {
                    ToolCallStatus::Error
                } else {
                    ToolCallStatus::Success
                };
                return;
            }
        }
        return;
    }
    resolve_tool_result(pending, out, id, truncate_tool_result(&text), is_error);
}

fn collect_acp_tool_output(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                item.get("text").and_then(Value::as_str).or_else(|| {
                    item.get("content")
                        .and_then(|c| c.get("text"))
                        .and_then(Value::as_str)
                })
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(_) => content
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jsonl(lines: &[&str]) -> String {
        lines.join("\n")
    }

    #[test]
    fn grok_maps_assistant_tool_calls_and_results() {
        let raw = jsonl(&[
            r#"{"type":"system","content":"你是 Grok"}"#,
            r#"{"type":"user","content":[{"type":"text","text":"看下 git 状态"}]}"#,
            r#"{"type":"reasoning","summary":[{"type":"summary_text","text":"先跑一下 git status"}],"encrypted_content":"xxx"}"#,
            r#"{"type":"assistant","content":"我来看看","tool_calls":[{"id":"call-1","name":"run_terminal_command","arguments":"{\"command\":\"git status\"}"}]}"#,
            r#"{"type":"tool_result","tool_call_id":"call-1","content":"exit: 0"}"#,
            r#"{"type":"backend_tool_call","kind":{"tool_type":"web_search"}}"#,
        ]);
        let msgs = parse_grok_history(&raw);
        assert_eq!(msgs.len(), 2, "system / backend_tool_call 不该成条消息");
        assert_eq!(msgs[0].message.role, "user");
        let assistant = &msgs[1].message;
        assert_eq!(assistant.reasoning.as_deref(), Some("先跑一下 git status"));
        assert_eq!(assistant.content, "我来看看");
        assert_eq!(assistant.tool_calls.len(), 1);
        assert_eq!(assistant.tool_calls[0].name, "run_terminal_command");
        assert_eq!(
            assistant.tool_calls[0].result_preview.as_deref(),
            Some("exit: 0")
        );
    }

    #[test]
    fn grok_extracts_data_url_images() {
        let raw = jsonl(&[
            r#"{"type":"user","content":[{"type":"image","url":"data:image/jpeg;base64,QUJD"}]}"#,
        ]);
        let msgs = parse_grok_history(&raw);
        assert_eq!(msgs[0].images.len(), 1);
        assert_eq!(msgs[0].images[0].media_type, "image/jpeg");
        assert_eq!(msgs[0].images[0].data_base64, "QUJD");
    }

    #[test]
    fn codex_reads_both_tool_families() {
        // 只认 function_call 的话，apply_patch 那些回合会凭空少掉。
        let raw = jsonl(&[
            r#"{"type":"session_meta","payload":{"id":"019c","cwd":"C:\\p"}}"#,
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"改一下这个文件"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<permissions>"}]}}"#,
            r#"{"type":"event_msg","payload":{"type":"agent_message","message":"这就改"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call","name":"shell_command","arguments":"{\"command\":\"ls\"}","call_id":"call_a"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"call_a","output":"Exit code: 0"}}"#,
            r#"{"type":"response_item","payload":{"type":"custom_tool_call","call_id":"call_b","name":"apply_patch","input":"*** Begin Patch"}}"#,
            r#"{"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call_b","output":"Success."}}"#,
        ]);
        let msgs = parse_codex_history(&raw);
        assert_eq!(msgs.len(), 2);
        assert_eq!(
            msgs[0].message.content, "改一下这个文件",
            "developer 注入不该混进正文"
        );
        let assistant = &msgs[1].message;
        assert_eq!(assistant.content, "这就改");
        let names: Vec<&str> = assistant
            .tool_calls
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, vec!["shell_command", "apply_patch"]);
        assert_eq!(
            assistant.tool_calls[0].result_preview.as_deref(),
            Some("Exit code: 0")
        );
        assert_eq!(
            assistant.tool_calls[1].result_preview.as_deref(),
            Some("Success.")
        );
    }

    #[test]
    fn unmatched_tool_result_is_dropped_not_invented() {
        // 结果找不到对应的调用就丢掉，绝不凭空造一条工具卡片。
        let raw = jsonl(&[
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"你好"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"不存在","output":"孤儿结果"}}"#,
        ]);
        let msgs = parse_codex_history(&raw);
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].message.tool_calls.is_empty());
    }

    /// grok / codex 跑真实数据。
    /// `KIVIO_GROK_JSONL=<chat_history.jsonl> KIVIO_CODEX_JSONL=<rollout.jsonl> cargo test ... -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn smoke_parse_real_grok_and_codex() {
        for (label, var, parse) in [
            (
                "grok",
                "KIVIO_GROK_JSONL",
                parse_grok_history as fn(&str) -> Vec<ImportedMessage>,
            ),
            ("codex", "KIVIO_CODEX_JSONL", parse_codex_history),
        ] {
            let Ok(path) = std::env::var(var) else {
                println!("[{label}] 跳过（未设 {var}）");
                continue;
            };
            let raw = std::fs::read_to_string(&path).unwrap();
            let msgs = parse(&raw);
            let calls: usize = msgs.iter().map(|m| m.message.tool_calls.len()).sum();
            let resolved = msgs
                .iter()
                .flat_map(|m| &m.message.tool_calls)
                .filter(|c| c.result_preview.is_some() || c.error.is_some())
                .count();
            let biggest = msgs
                .iter()
                .flat_map(|m| &m.message.tool_calls)
                .filter_map(|c| c.result_preview.as_ref().map(|p| p.len()))
                .max()
                .unwrap_or(0);
            println!(
                "[{label}] {} 字节 → {} 条消息，工具 {calls} 个（{resolved} 个有结果），思考 {} 条，图片 {}，最大结果 {biggest} 字节",
                raw.len(),
                msgs.len(),
                msgs.iter().filter(|m| m.message.reasoning.is_some()).count(),
                msgs.iter().map(|m| m.images.len()).sum::<usize>(),
            );
            assert!(msgs.len() > 1, "[{label}] 至少该解析出多条消息");
            assert!(
                biggest < TOOL_RESULT_CAP_BYTES + 200,
                "[{label}] 截断没生效"
            );
            for pair in msgs.windows(2) {
                assert_ne!(
                    pair[0].message.role, pair[1].message.role,
                    "[{label}] 出现连续同角色消息"
                );
            }
        }
    }

    #[test]
    fn tool_result_only_user_entry_does_not_create_a_user_bubble() {
        // 这是最容易做错的地方：claude 的工具结果是以 role:user 回来的。
        // 当成新用户消息的话，每次工具调用都会在界面上多出一条空白用户气泡。
        let raw = jsonl(&[
            r#"{"type":"user","message":{"content":"帮我读个文件"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu_1","name":"Read","input":{"path":"a.txt"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu_1","content":"文件内容"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"读完了"}]}}"#,
        ]);
        let msgs = parse_claude_history(&raw);
        let roles: Vec<&str> = msgs.iter().map(|m| m.message.role.as_str()).collect();
        assert_eq!(roles, vec!["user", "assistant"], "工具结果不该单独成条消息");
        assert_eq!(msgs[1].message.tool_calls.len(), 1);
        assert_eq!(
            msgs[1].message.tool_calls[0].result_preview.as_deref(),
            Some("文件内容")
        );
        assert_eq!(msgs[1].message.content, "读完了");
    }

    #[test]
    fn thinking_goes_to_reasoning_not_content() {
        let raw = jsonl(&[
            r#"{"type":"user","message":{"content":"你好"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"先想想"},{"type":"text","text":"你好"}]}}"#,
        ]);
        let msgs = parse_claude_history(&raw);
        assert_eq!(msgs[1].message.reasoning.as_deref(), Some("先想想"));
        assert_eq!(msgs[1].message.content, "你好");
        // 段落顺序要保留：思考在前、正文在后。
        let kinds: Vec<_> = msgs[1]
            .message
            .segments
            .iter()
            .map(|s| s.kind.clone())
            .collect();
        assert_eq!(
            kinds,
            vec![
                ChatMessageSegmentKind::Reasoning,
                ChatMessageSegmentKind::Text
            ]
        );
    }

    #[test]
    fn sidechain_and_bookkeeping_entries_are_dropped() {
        let raw = jsonl(&[
            r#"{"type":"queue-operation","operation":"enqueue"}"#,
            r#"{"type":"user","message":{"content":"问题"}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"子 agent 的话"}]}}"#,
            r#"{"type":"file-history-snapshot"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"答案"}]}}"#,
        ]);
        let msgs = parse_claude_history(&raw);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].message.content, "答案");
    }

    #[test]
    fn tool_result_is_truncated_on_char_boundary() {
        let big = "中".repeat(4000); // 12000 字节，远超 2KB
        let raw = jsonl(&[
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu_1","name":"Bash","input":{}}]}}"#,
            &format!(
                r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"tu_1","content":{}}}]}}}}"#,
                serde_json::to_string(&big).unwrap()
            ),
        ]);
        let msgs = parse_claude_history(&raw);
        let preview = msgs[0].message.tool_calls[0]
            .result_preview
            .as_deref()
            .unwrap();
        assert!(preview.len() < TOOL_RESULT_CAP_BYTES + 100);
        assert!(preview.contains("已截断"));
        // 没在多字节字符中间切断 —— 能正常当 UTF-8 用就是证明。
        assert!(preview.chars().next().is_some());
    }

    #[test]
    fn error_tool_result_sets_error_status() {
        let raw = jsonl(&[
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu_1","name":"Bash","input":{}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu_1","is_error":true,"content":"炸了"}]}}"#,
        ]);
        let msgs = parse_claude_history(&raw);
        let call = &msgs[0].message.tool_calls[0];
        assert_eq!(call.status, ToolCallStatus::Error);
        assert_eq!(call.error.as_deref(), Some("炸了"));
        assert!(call.result_preview.is_none());
    }

    #[test]
    fn inline_images_are_returned_unwritten() {
        let raw = jsonl(&[
            r#"{"type":"user","message":{"content":[{"type":"text","text":"看这张图"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}]}}"#,
        ]);
        let msgs = parse_claude_history(&raw);
        assert_eq!(msgs[0].images.len(), 1);
        assert_eq!(msgs[0].images[0].media_type, "image/png");
        // 解析器不落盘：attachments 留空，由导入命令填。
        assert!(msgs[0].message.attachments.is_empty());
    }

    #[test]
    fn model_messages_stay_empty() {
        // ADR-0002：快照不参与模型输入。填了反而会让续聊时历史重复。
        let raw = jsonl(&[r#"{"type":"user","message":{"content":"你好"}}"#]);
        let msgs = parse_claude_history(&raw);
        assert!(msgs[0].message.model_messages.is_empty());
        assert!(msgs[0].message.api_messages.is_empty());
    }

    /// 解析本机真实会话。默认不跑。
    /// `KIVIO_CLAUDE_JSONL=<路径> cargo test --lib external_agents::import_history::tests::smoke -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn smoke_parse_real_claude_session() {
        let path = std::env::var("KIVIO_CLAUDE_JSONL").expect("需要 KIVIO_CLAUDE_JSONL");
        let raw = std::fs::read_to_string(&path).unwrap();
        let msgs = parse_claude_history(&raw);
        let users = msgs.iter().filter(|m| m.message.role == "user").count();
        let assistants = msgs
            .iter()
            .filter(|m| m.message.role == "assistant")
            .count();
        let calls: usize = msgs.iter().map(|m| m.message.tool_calls.len()).sum();
        let resolved = msgs
            .iter()
            .flat_map(|m| &m.message.tool_calls)
            .filter(|c| c.result_preview.is_some() || c.error.is_some())
            .count();
        let reasoning = msgs
            .iter()
            .filter(|m| m.message.reasoning.is_some())
            .count();
        let images: usize = msgs.iter().map(|m| m.images.len()).sum();
        let biggest = msgs
            .iter()
            .flat_map(|m| &m.message.tool_calls)
            .filter_map(|c| c.result_preview.as_ref().map(|p| p.len()))
            .max()
            .unwrap_or(0);
        println!(
            "原始 {} 字节 → {} 条消息（user {users} / assistant {assistants}）",
            raw.len(),
            msgs.len()
        );
        println!("工具调用 {calls} 个，其中 {resolved} 个配到了结果；带思考的消息 {reasoning} 条；图片 {images} 张");
        println!("最大 result_preview {biggest} 字节（上限 {TOOL_RESULT_CAP_BYTES} + 截断说明）");
        for m in msgs.iter().take(3) {
            let head: String = m.message.content.chars().take(50).collect();
            println!(
                "  [{}] {:?} tools={} seg={}",
                m.message.role,
                head,
                m.message.tool_calls.len(),
                m.message.segments.len()
            );
        }
        assert!(msgs.len() > 1, "真实会话至少该解析出多条消息");
        assert!(
            biggest < TOOL_RESULT_CAP_BYTES + 200,
            "截断没生效：{biggest} 字节"
        );
        // 相邻两条同角色消息说明合并逻辑漏了——claude 一个回合会拆成多条 assistant 记录。
        for pair in msgs.windows(2) {
            assert_ne!(
                pair[0].message.role, pair[1].message.role,
                "出现连续同角色消息，合并逻辑有问题"
            );
        }
    }
}
