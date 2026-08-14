use std::collections::HashMap;

use serde_json::Value;
use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::chat::model::{
    model_messages_from_openai_messages, MessagePart, ModelMessage, ModelRole,
};
use crate::mcp::types::ChatToolArtifact;
use crate::settings::Settings;
use crate::state::AppState;

use super::context::{
    compute_context_state, emit_chat_context_state, try_auto_compress_context_after_update,
};
use super::interaction::emit_chat_plan_state;
use super::title::{generate_title, resolve_conversation_title};
use super::{
    AgentPlanState, ChatMessage, ChatMessageSegment, ChatMessageSegmentKind,
    ChatMessageSegmentPhase, Conversation, ToolCallRecord, ToolCallStatus,
};

/// 多答组的列标识：(group_id, provider_id, model)。单模型为 None（字段写 None）。
type AssistantGroupMeta = (String, String, String);

/// 反向对账:给「有工具分段但无对应记录」的孤立 `tool_call_id` 合成一条中断态
/// (`Cancelled`)占位记录,追加进 `tool_calls`。
///
/// 场景:工具分段在 planning 阶段(解析出调用即)创建并流式推送,记录在 execution
/// 阶段(工具执行时)创建。若一轮在两者之间被中断(网关掐流/400/取消/超时),落库消息
/// 就有分段无记录 → 前端渲染「工具记录缺失」。`normalize_assistant_segments` 只补
/// 「有记录没分段」的正向;此函数补反向,消除困惑呈现,并保留「模型确实发起过该工具」
/// 的痕迹。能从 `api_messages`(OpenAI 线格式 assistant `tool_calls[]`)按 id 回捞
/// name/arguments 就用真值,捞不到留空(前端兜底显示「工具调用」)。对无孤立分段的
/// 消息零副作用(空转)。
pub(super) fn reconcile_orphan_tool_segments(
    tool_calls: &mut Vec<ToolCallRecord>,
    segments: &[ChatMessageSegment],
    api_messages: &[Value],
) {
    use std::collections::HashSet;
    let record_ids: HashSet<&str> = tool_calls.iter().map(|record| record.id.as_str()).collect();

    // 孤立工具分段的 (id, round),去重保序。
    let mut orphan_ids: Vec<(String, u32)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for segment in segments {
        if segment.kind != ChatMessageSegmentKind::Tool {
            continue;
        }
        let Some(id) = segment.tool_call_id.as_deref() else {
            continue;
        };
        if id.is_empty() || record_ids.contains(id) || !seen.insert(id.to_string()) {
            continue;
        }
        orphan_ids.push((id.to_string(), segment.round.unwrap_or(0)));
    }
    if orphan_ids.is_empty() {
        return;
    }

    let now = chrono::Local::now().timestamp();
    for (id, round) in orphan_ids {
        let (name, arguments) = tool_call_meta_from_api_messages(api_messages, &id);
        tool_calls.push(ToolCallRecord {
            id,
            name,
            source: String::new(),
            server_id: None,
            arguments,
            status: ToolCallStatus::Cancelled,
            result_preview: None,
            error: Some("工具调用未完成（会话中断）".to_string()),
            duration_ms: Some(0),
            started_at: Some(now),
            completed_at: Some(now),
            round,
            sensitive: false,
            artifacts: Vec::new(),
            trace_id: None,
            span_id: None,
            structured_content: None,
        });
    }
}

/// 从 `api_messages`(OpenAI 线格式)里按 `tool_call_id` 回捞工具调用的
/// `(name, arguments)`。扫每条消息的 `tool_calls[]`,匹配 `id` 命中即返回;未命中
/// 返回 `(空, 空)`。
fn tool_call_meta_from_api_messages(api_messages: &[Value], id: &str) -> (String, String) {
    for message in api_messages {
        let Some(calls) = message.get("tool_calls").and_then(Value::as_array) else {
            continue;
        };
        for call in calls {
            if call.get("id").and_then(Value::as_str) == Some(id) {
                let function = call.get("function");
                let name = function
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let arguments = function
                    .and_then(|f| f.get("arguments"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                return (name, arguments);
            }
        }
    }
    (String::new(), String::new())
}

/// 构造一条 assistant `ChatMessage`（含 segment 归一、model_messages 计算）。
/// `push_assistant_message`（落盘路径）与多模型臂（返回消息交协调者落盘）共用此函数，
/// 保证两条路径生成的消息形态一致。`group_meta = Some(..)` 时写入 group_id/provider_id/model。
#[allow(clippy::too_many_arguments)]
pub(super) fn build_assistant_message(
    message_id: String,
    content: String,
    reasoning: Option<String>,
    artifacts: Vec<ChatToolArtifact>,
    mut tool_calls: Vec<ToolCallRecord>,
    api_messages: Vec<Value>,
    segments: Vec<ChatMessageSegment>,
    active_skill_id: Option<&str>,
    run_entry: Option<&str>,
    stream_outcome: Option<&str>,
    usage: Option<crate::chat::model::ModelUsage>,
    anchor_usage: Option<crate::chat::model::ModelUsage>,
    agent_plan: Option<AgentPlanState>,
    group_meta: Option<AssistantGroupMeta>,
) -> ChatMessage {
    // 反向对账:补齐「有工具分段无记录」的孤立调用为中断态记录，避免前端显示
    // 「工具记录缺失」。在 normalize（正向补段）之前跑，使新记录与既有分段自然对上。
    reconcile_orphan_tool_segments(&mut tool_calls, &segments, &api_messages);
    let segments =
        normalize_assistant_segments(&content, reasoning.as_deref(), &tool_calls, segments);
    let stored_content = content_from_segments(&segments).unwrap_or_else(|| content.clone());
    let stored_reasoning = reasoning_from_segments(&segments).or(reasoning);

    // model_messages 是规范回放源（build_chat_api_messages 优先用它）。算好后，若它
    // 非空就丢弃冗余的 api_messages（OpenAI 线格式）——回放/编辑路径仅在 model_messages
    // 为空时才回落 api_messages，前端更是从不读它。省 RAM/磁盘/IPC。为空兜底（罕见：
    // 转换产出空）才保留 api_messages，避免丢工具上下文。中断草稿走另一条路
    // (persist_partial_assistant_snapshot)，那里仍保留 api_messages 以保「继续」可回放。
    let model_messages = assistant_model_messages_for_storage(
        &stored_content,
        stored_reasoning.as_deref(),
        &api_messages,
        &tool_calls,
    );
    let api_messages = if model_messages.is_empty() {
        api_messages
    } else {
        Vec::new()
    };

    let (group_id, provider_id, model) = match group_meta {
        Some((g, p, m)) => (Some(g), Some(p), Some(m)),
        None => (None, None, None),
    };

    ChatMessage {
        id: message_id,
        role: "assistant".to_string(),
        content: stored_content,
        attachments: vec![],
        reasoning: stored_reasoning,
        artifacts,
        model_messages,
        tool_calls,
        segments,
        agent_plan,
        api_messages,
        active_skill_id: active_skill_id.map(|id| id.to_string()),
        run_entry: run_entry.map(str::to_string),
        stream_outcome: stream_outcome.map(str::to_string),
        usage,
        anchor_usage,
        group_id,
        provider_id,
        model,
        timestamp: chrono::Local::now().timestamp(),
        degraded: None,
    }
}

/// 多答 fan-out 中某个臂报错时，合成一条「错误列」assistant 消息（不落盘由调用者 upsert）。
/// 复用 build_assistant_message 保证列形态一致：带 group_id/provider/model，content 为错误信息，
/// stream_outcome 标 "error"。这样报错的模型仍保留为一列，而不是被整列吞掉。
pub(super) fn build_error_arm_message(
    group_id: &str,
    provider_id: String,
    model: String,
    error: String,
    run_entry: &str,
    active_skill_id: Option<&str>,
) -> ChatMessage {
    build_assistant_message(
        format!("msg_{}", Uuid::new_v4()),
        error,
        None,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        active_skill_id,
        Some(run_entry),
        Some("error"),
        None,
        None,
        None,
        Some((group_id.to_string(), provider_id, model)),
    )
}

pub(crate) async fn push_assistant_message(
    app: &AppHandle,
    state: &State<'_, AppState>,
    settings: &Settings,
    conversation: &mut Conversation,
    message_id: String,
    content: String,
    reasoning: Option<String>,
    artifacts: Vec<ChatToolArtifact>,
    tool_calls: Vec<ToolCallRecord>,
    api_messages: Vec<Value>,
    segments: Vec<ChatMessageSegment>,
    active_skill_id: Option<&str>,
    title_from_first_user: Option<&str>,
    run_entry: Option<&str>,
    stream_outcome: Option<&str>,
    usage: Option<crate::chat::model::ModelUsage>,
    anchor_usage: Option<crate::chat::model::ModelUsage>,
    agent_plan: Option<AgentPlanState>,
    // 降级兜底的结构化描述；Some 时前端渲染成独立错误卡片。
    degraded: Option<crate::chat::agent::recovery::DegradedAnswer>,
) -> Result<(), String> {
    let message = build_assistant_message(
        message_id,
        content.clone(),
        reasoning,
        artifacts,
        tool_calls,
        api_messages,
        segments,
        active_skill_id,
        run_entry,
        stream_outcome,
        usage,
        anchor_usage,
        agent_plan,
        // 单模型落盘路径不带 group 信息（行为不变）。
        None,
    );
    let mut message = message;
    message.degraded = degraded;
    let stored_content = message.content.clone();

    // 标题还是"自动生成的样子"吗——`"新对话"`，或者等于第一句用户消息的启发式结果。
    // 用户手动重命名过的标题不会等于启发式结果，所以据此判断不会覆盖用户的命名。
    let title_looks_auto = conversation.title == "新对话"
        || title_from_first_user.is_some_and(|first| conversation.title == generate_title(first));
    let is_external =
        conversation.agent_runtime.kind == crate::chat::types::AgentRuntimeKind::External;

    // 外部 CLI 对话优先用 CLI 自己生成的标题：那是用户在 CLI 里看到的那个，而且不花模型调用。
    //
    // **每轮都试一次**（一次文件读，很便宜）：CLI 是异步写标题的（claude 的 `ai-title`），
    // 首轮回复落盘时往往还没有，只在第一轮试就永远拿不到。
    let external_cli_title = if is_external && title_looks_auto {
        crate::external_agents::import::cli_session_title(app, &conversation.id)
            .filter(|title| !title.trim().is_empty() && *title != conversation.title)
    } else {
        None
    };

    let generated_title = if external_cli_title.is_some() {
        external_cli_title
    } else if let Some(user_content) = title_from_first_user {
        // 外部对话放宽首轮门槛：它的标题可能已经被兜底成"第一句用户消息"，
        // 卡在 `== "新对话"` 上的话，标题模型这一步压根不会触发。
        // 工具型首轮会在生成中落 assistant 草稿快照，不能用 messages.len() == 1
        // 判断首轮，否则只要首轮用了工具就会永远跳过标题请求。
        let is_first_user_turn = conversation
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .count()
            == 1;
        let wants_generation = is_first_user_turn
            && if is_external {
                title_looks_auto
            } else {
                conversation.title == "新对话"
            };
        if wants_generation {
            // 被取消的首条回复不值得花一次模型调用生成标题（标题生成是一次
            // 带 30s 超时的 LLM 请求，会显著拖慢"停止"后 invoke 的返回 / 输入框解锁）。
            // 用本地启发式标题兜底；下一条正常回复或重命名仍可得到更好的标题。
            if stream_outcome == Some("cancelled") {
                Some(generate_title(user_content))
            } else {
                // 外部对话的 `provider_id`/`model` 是空的（ADR-0001），`resolve_mixer_side_model`
                // 会因此跳过"继承会话模型"，落到设置里的标题模型、再兜到全局 Chat 模型。
                Some(
                    resolve_conversation_title(
                        settings,
                        state.inner(),
                        conversation,
                        user_content,
                        &stored_content,
                    )
                    .await,
                )
            }
        } else {
            None
        }
    } else {
        None
    };

    let first_user = title_from_first_user.map(str::to_string);
    let message_plan = message.agent_plan.clone();
    let plan_update = message_plan.clone();
    let conversation_id = conversation.id.clone();
    let mut persisted = crate::chat::repository::repository(app)
        .mutate(app, &conversation_id, |latest| {
            upsert_assistant_message(latest, message);
            if let Some(title) = generated_title {
                let title_is_still_auto = latest.title == "新对话"
                    || first_user
                        .as_deref()
                        .is_some_and(|first| latest.title == generate_title(first));
                if title_is_still_auto {
                    latest.title = title;
                }
            }
            if let Some(plan) = message_plan {
                latest.agent_plan_state = plan;
            }
            Ok(())
        })
        .await
        .map_err(crate::chat::repository::repository_error)?;

    // Context is derived from the full conversation. Commit it with CAS and
    // recompute once if an unrelated operation advanced the revision while the
    // potentially expensive estimate/compression was running.
    for attempt in 0..2 {
        match compute_context_state(app, state, &persisted, None, &[]).await {
            Ok(context_state) => {
                persisted.context_state = context_state;
                try_auto_compress_context_after_update(app, state, &mut persisted, None, &[]).await;
                let next_context = persisted.context_state.clone();
                match crate::chat::repository::repository(app)
                    .update_context(app, &conversation_id, persisted.revision, next_context)
                    .await
                {
                    Ok(latest) => {
                        persisted = latest;
                        emit_chat_context_state(
                            app,
                            &conversation_id,
                            persisted.revision,
                            &persisted.context_state,
                        );
                        break;
                    }
                    Err(crate::chat::repository::ConversationRepositoryError::Conflict {
                        ..
                    }) if attempt == 0 => {
                        persisted = crate::chat::repository::repository(app)
                            .get(app, &conversation_id)
                            .await
                            .map_err(crate::chat::repository::repository_error)?;
                    }
                    Err(err) => {
                        eprintln!("Context state commit failed after assistant reply: {err}");
                        break;
                    }
                }
            }
            Err(err) => {
                eprintln!("Context usage estimate failed after assistant reply: {err}");
                break;
            }
        }
    }

    *conversation = persisted;
    if let Some(plan) = plan_update {
        emit_chat_plan_state(app, &conversation_id, conversation.revision, &plan);
    }
    Ok(())
}

/// Insert an assistant message, replacing any existing message that already
/// carries the same id. The agent loop's per-round crash-safety checkpoint
/// writes a draft assistant message under the run's `message_id`; both that
/// draft path and the final write go through here so a completed run cleanly
/// overwrites its own draft instead of appending a duplicate.
pub(super) fn upsert_assistant_message(conversation: &mut Conversation, message: ChatMessage) {
    if let Some(pos) = conversation
        .messages
        .iter()
        .position(|existing| existing.id == message.id)
    {
        conversation.messages[pos] = message;
    } else {
        conversation.messages.push(message);
    }
}

/// Write a best-effort snapshot of the in-progress assistant turn to disk so a
/// mid-run crash / forced exit doesn't discard the whole reply. Reloads the
/// conversation (to pick up todo/plan/user state already persisted by other
/// paths), upserts a draft assistant message keyed by `message_id`, and saves.
/// The draft is marked `interrupted`; the loop's final write replaces it with
/// the completed message. `api_messages` carries the loop's accumulated
/// provider messages (assistant tool_calls + tool results) so the draft stays
/// replayable on a later "continue" — `model_messages` are derived from them
/// exactly as the final write does, keeping the storage shape consistent. No-op
/// when nothing has been produced yet.
pub(super) async fn persist_partial_assistant_snapshot(
    app: &AppHandle,
    conversation_id: &str,
    message_id: &str,
    tool_records: &[ToolCallRecord],
    segments: &[ChatMessageSegment],
    api_messages: &[Value],
) -> Result<(), String> {
    if tool_records.is_empty() && segments.is_empty() {
        return Ok(());
    }
    let segments = segments.to_vec();
    // 中断草稿是「永不完成」run 的最终存档，最易出孤立工具分段——同样反向对账补齐。
    let mut tool_records = tool_records.to_vec();
    reconcile_orphan_tool_segments(&mut tool_records, &segments, api_messages);
    let content = content_from_segments(&segments).unwrap_or_default();
    let reasoning = reasoning_from_segments(&segments);
    let model_messages = assistant_model_messages_for_storage(
        &content,
        reasoning.as_deref(),
        api_messages,
        &tool_records,
    );
    let draft = ChatMessage {
        id: message_id.to_string(),
        role: "assistant".to_string(),
        content,
        attachments: Vec::new(),
        reasoning,
        artifacts: Vec::new(),
        tool_calls: tool_records,
        segments,
        agent_plan: None,
        api_messages: api_messages.to_vec(),
        model_messages,
        active_skill_id: None,
        run_entry: None,
        stream_outcome: Some("interrupted".to_string()),
        usage: None,
        anchor_usage: None,
        group_id: None,
        provider_id: None,
        model: None,
        timestamp: chrono::Local::now().timestamp(),
        degraded: None,
    };
    crate::chat::repository::repository(app)
        .upsert_message(app, conversation_id, draft)
        .await
        .map(|_| ())
        .map_err(crate::chat::repository::repository_error)
}

pub(super) fn normalize_assistant_segments(
    content: &str,
    reasoning: Option<&str>,
    tool_calls: &[ToolCallRecord],
    mut segments: Vec<ChatMessageSegment>,
) -> Vec<ChatMessageSegment> {
    if segments.is_empty() {
        segments = synthesize_assistant_segments(content, reasoning, tool_calls);
    }

    let mut next_order = next_segment_order(&segments);
    if !content.trim().is_empty() && content_from_segments(&segments).is_none() {
        segments.push(ChatMessageSegment {
            id: format!("seg_{}_synthesis_text", next_order),
            kind: ChatMessageSegmentKind::Text,
            phase: if tool_calls.is_empty() {
                ChatMessageSegmentPhase::Plain
            } else {
                ChatMessageSegmentPhase::Synthesis
            },
            order: next_order,
            step_number: None,
            round: None,
            text: Some(content.to_string()),
            tool_call_id: None,
        });
        next_order = next_order.saturating_add(1);
    }

    if reasoning_from_segments(&segments).is_none() {
        if let Some(reasoning) = reasoning.map(str::trim).filter(|value| !value.is_empty()) {
            segments.push(ChatMessageSegment {
                id: format!("seg_{}_reasoning", next_order),
                kind: ChatMessageSegmentKind::Reasoning,
                phase: ChatMessageSegmentPhase::Synthesis,
                order: next_order,
                step_number: None,
                round: None,
                text: Some(reasoning.to_string()),
                tool_call_id: None,
            });
        }
    }

    let existing_tool_segment_ids = segments
        .iter()
        .filter_map(|segment| {
            if segment.kind == ChatMessageSegmentKind::Tool {
                segment.tool_call_id.clone()
            } else {
                None
            }
        })
        .collect::<std::collections::HashSet<_>>();
    let mut missing_records: Vec<&ToolCallRecord> = tool_calls
        .iter()
        .filter(|record| !existing_tool_segment_ids.contains(&record.id))
        .collect();
    missing_records.sort_by_key(|record| record.started_at.unwrap_or(0));
    if !missing_records.is_empty() {
        let synthesis_start = segments
            .iter()
            .filter(|segment| segment.phase == ChatMessageSegmentPhase::Synthesis)
            .map(|segment| segment.order)
            .min();
        for record in missing_records {
            let insert_at = segments
                .iter()
                .filter(|segment| synthesis_start.is_none_or(|start| segment.order < start))
                .map(|segment| segment.order)
                .max()
                .unwrap_or(0)
                .saturating_add(1);
            for segment in segments.iter_mut() {
                if segment.order >= insert_at {
                    segment.order = segment.order.saturating_add(1);
                }
            }
            segments.push(tool_segment_for_record(record, insert_at, None));
        }
    }

    segments.sort_by_key(|segment| segment.order);
    segments
}

fn synthesize_assistant_segments(
    content: &str,
    reasoning: Option<&str>,
    tool_calls: &[ToolCallRecord],
) -> Vec<ChatMessageSegment> {
    let mut segments = Vec::new();
    let mut order = 1000u32;
    for record in tool_calls {
        segments.push(tool_segment_for_record(record, order, None));
        order = order.saturating_add(1);
    }
    if let Some(reasoning) = reasoning.map(str::trim).filter(|value| !value.is_empty()) {
        segments.push(ChatMessageSegment {
            id: format!("seg_{}_reasoning", order),
            kind: ChatMessageSegmentKind::Reasoning,
            phase: if tool_calls.is_empty() {
                ChatMessageSegmentPhase::Plain
            } else {
                ChatMessageSegmentPhase::Synthesis
            },
            order,
            step_number: None,
            round: None,
            text: Some(reasoning.to_string()),
            tool_call_id: None,
        });
        order = order.saturating_add(1);
    }
    if !content.trim().is_empty() {
        segments.push(ChatMessageSegment {
            id: format!("seg_{}_text", order),
            kind: ChatMessageSegmentKind::Text,
            phase: if tool_calls.is_empty() {
                ChatMessageSegmentPhase::Plain
            } else {
                ChatMessageSegmentPhase::Synthesis
            },
            order,
            step_number: None,
            round: None,
            text: Some(content.to_string()),
            tool_call_id: None,
        });
    }
    segments
}

pub(super) fn auxiliary_tool_segments(records: &[ToolCallRecord]) -> Vec<ChatMessageSegment> {
    records
        .iter()
        .enumerate()
        .map(|(index, record)| tool_segment_for_record(record, 100 + index as u32, None))
        .collect()
}

pub(super) fn tool_segment_for_record(
    record: &ToolCallRecord,
    order: u32,
    step_number: Option<u8>,
) -> ChatMessageSegment {
    ChatMessageSegment {
        id: format!("seg_{}_tool_{}", order, record.id),
        kind: ChatMessageSegmentKind::Tool,
        phase: if record.round == 0 || record.source == "mixer" {
            ChatMessageSegmentPhase::Auxiliary
        } else {
            ChatMessageSegmentPhase::ToolLoop
        },
        order,
        step_number,
        round: Some(record.round),
        text: None,
        tool_call_id: Some(record.id.clone()),
    }
}

pub(super) fn plain_text_segment(order: u32, text: &str) -> ChatMessageSegment {
    ChatMessageSegment {
        id: format!("seg_{}_plain_text", order),
        kind: ChatMessageSegmentKind::Text,
        phase: ChatMessageSegmentPhase::Plain,
        order,
        step_number: None,
        round: None,
        text: Some(text.to_string()),
        tool_call_id: None,
    }
}

pub(super) fn content_from_segments(segments: &[ChatMessageSegment]) -> Option<String> {
    let content = joined_segment_text(segments, |segment| {
        segment.kind == ChatMessageSegmentKind::Text
            && matches!(
                segment.phase,
                ChatMessageSegmentPhase::Plain | ChatMessageSegmentPhase::Synthesis
            )
    });
    if content.trim().is_empty() {
        None
    } else {
        Some(content)
    }
}

pub(super) fn reasoning_from_segments(segments: &[ChatMessageSegment]) -> Option<String> {
    let reasoning = joined_segment_text(segments, |segment| {
        segment.kind == ChatMessageSegmentKind::Reasoning
    });
    if reasoning.trim().is_empty() {
        None
    } else {
        Some(reasoning)
    }
}

fn joined_segment_text(
    segments: &[ChatMessageSegment],
    predicate: impl Fn(&ChatMessageSegment) -> bool,
) -> String {
    let mut parts = segments
        .iter()
        .filter(|segment| predicate(segment))
        .filter_map(|segment| {
            let text = segment.text.as_deref()?.trim();
            if text.is_empty() {
                None
            } else {
                Some((segment.order, text.to_string()))
            }
        })
        .collect::<Vec<_>>();
    parts.sort_by_key(|(order, _)| *order);
    parts
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn next_segment_order(segments: &[ChatMessageSegment]) -> u32 {
    segments
        .iter()
        .map(|segment| segment.order)
        .max()
        .unwrap_or(999)
        .saturating_add(1)
}

pub(super) fn replace_final_text_segments_for_edit(message: &mut ChatMessage, content: &str) {
    let mut segments = if message.segments.is_empty() {
        synthesize_assistant_segments(
            &message.content,
            message.reasoning.as_deref(),
            &message.tool_calls,
        )
    } else {
        std::mem::take(&mut message.segments)
    };
    segments.retain(|segment| {
        !(segment.kind == ChatMessageSegmentKind::Text
            && matches!(
                segment.phase,
                ChatMessageSegmentPhase::Plain | ChatMessageSegmentPhase::Synthesis
            ))
    });
    let order = next_segment_order(&segments);
    segments.push(ChatMessageSegment {
        id: format!("seg_{}_edited_synthesis", order),
        kind: ChatMessageSegmentKind::Text,
        phase: if message.tool_calls.is_empty() {
            ChatMessageSegmentPhase::Plain
        } else {
            ChatMessageSegmentPhase::Synthesis
        },
        order,
        step_number: None,
        round: None,
        text: Some(content.to_string()),
        tool_call_id: None,
    });
    segments.sort_by_key(|segment| segment.order);
    message.segments = segments;
    message.content =
        content_from_segments(&message.segments).unwrap_or_else(|| content.to_string());
    message.reasoning = reasoning_from_segments(&message.segments);
    message.model_messages = edited_assistant_model_messages(message);
    message.api_messages = Vec::new();
}

fn edited_assistant_model_messages(message: &ChatMessage) -> Vec<ModelMessage> {
    let mut replay = message.model_messages.clone();
    if replay.is_empty() && !message.api_messages.is_empty() {
        replay = model_messages_from_openai_messages(message.api_messages.clone());
    }

    let edited_answer = assistant_model_messages_for_storage(
        &message.content,
        message.reasoning.as_deref(),
        &[],
        &[],
    );
    if edited_answer.is_empty() {
        return Vec::new();
    }

    if let Some(final_answer_idx) = replay.iter().rposition(|model_message| {
        model_message.role == ModelRole::Assistant
            && !model_message
                .content
                .iter()
                .any(|part| matches!(part, MessagePart::ToolCall { .. }))
    }) {
        replay.truncate(final_answer_idx);
        replay.extend(edited_answer);
        replay
    } else if replay.is_empty() {
        edited_answer
    } else {
        replay.extend(edited_answer);
        replay
    }
}

pub(super) fn capture_agent_plan_draft_if_needed(
    conversation: &mut Conversation,
    original_plan_mode: bool,
    content: &str,
    stream_outcome: &str,
) -> Option<AgentPlanState> {
    if stream_outcome != "completed"
        || !original_plan_mode
        || !crate::chat::plan::is_plan_mode(&conversation.agent_plan_state)
    {
        return None;
    }
    let next_state =
        crate::chat::plan::capture_draft_from_reply(&conversation.agent_plan_state, content);
    if next_state == conversation.agent_plan_state {
        return if crate::chat::plan::executable_plan_text(&next_state)
            .is_some_and(|plan| plan == content.trim())
        {
            Some(next_state)
        } else {
            None
        };
    }
    conversation.agent_plan_state = next_state.clone();
    Some(next_state)
}

pub(super) fn assistant_model_messages_for_storage(
    content: &str,
    reasoning: Option<&str>,
    api_messages: &[Value],
    tool_calls: &[ToolCallRecord],
) -> Vec<ModelMessage> {
    if !api_messages.is_empty() {
        let mut canonical = model_messages_from_openai_messages(api_messages.to_vec());
        mark_tool_result_errors(&mut canonical, tool_calls);
        if !canonical.is_empty() {
            return canonical;
        }
    }

    let mut parts = Vec::new();
    if !content.trim().is_empty() {
        parts.push(MessagePart::Text {
            text: content.to_string(),
        });
    }
    if let Some(reasoning) = reasoning.map(str::trim).filter(|value| !value.is_empty()) {
        parts.push(MessagePart::Reasoning {
            text: reasoning.to_string(),
        });
    }

    if parts.is_empty() {
        Vec::new()
    } else {
        vec![ModelMessage {
            role: ModelRole::Assistant,
            content: parts,
        }]
    }
}

fn mark_tool_result_errors(messages: &mut [ModelMessage], tool_calls: &[ToolCallRecord]) {
    let error_by_id: HashMap<&str, bool> = tool_calls
        .iter()
        .map(|record| {
            (
                record.id.as_str(),
                matches!(record.status, ToolCallStatus::Error),
            )
        })
        .collect();
    if error_by_id.is_empty() {
        return;
    }

    for message in messages {
        for part in &mut message.content {
            if let MessagePart::ToolResult {
                tool_call_id,
                is_error,
                ..
            } = part
            {
                if let Some(failed) = error_by_id.get(tool_call_id.as_str()) {
                    *is_error = *failed;
                }
            }
        }
    }
}
