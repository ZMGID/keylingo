use std::{
    future::Future,
    pin::Pin,
    time::{Duration, Instant},
};

use serde_json::Value;
use tokio::time::timeout;

use crate::chat::ask_user;
use crate::chat::model::PendingToolCall;
use crate::chat::types::{ToolCallRecord, ToolCallStatus};
use crate::mcp::types::{ChatToolArtifact, McpToolCallResult};
use crate::mcp::ChatToolDefinition;
use crate::settings::Settings;
use crate::skills;

use super::host::AgentHost;
use super::prepare::{builtin_tool_bypasses_approval, disabled_builtin_tool_feedback};

pub type ToolExecutorFuture<'a> =
    Pin<Box<dyn Future<Output = Result<McpToolCallResult, String>> + Send + 'a>>;

pub trait ToolExecutor: Send + Sync {
    fn call<'a>(
        &'a self,
        ctx: &'a ToolExecutionContext<'a>,
        tool: &'a ChatToolDefinition,
        arguments: Value,
        skill_cache: Option<&'a mut skills::SkillRunCache>,
    ) -> ToolExecutorFuture<'a>;
}

pub struct ToolExecutionContext<'a> {
    pub conversation_id: &'a str,
    pub run_id: &'a str,
    pub message_id: &'a str,
    pub generation: u64,
    pub round: u32,
    /// Sub-agent nesting depth: 0 for a top-level chat run, +1 per spawned
    /// sub-agent. Used to gate `agent` spawns (`MAX_SUB_AGENT_DEPTH`) and to
    /// reject sensitive tools inside sub-agents.
    pub depth: u8,
    /// Conversation that conversation-scoped tools (todo, native file
    /// workspace) operate on. Equals `conversation_id` for a top-level run; for
    /// a sub-agent it is the PARENT conversation so the sub-agent can claim the
    /// parent's todos and resolve the parent's project workspace, while
    /// `conversation_id` stays a synthetic id used only for generation/streaming
    /// isolation.
    pub tool_conversation_id: &'a str,
    /// The model-issued tool_call id of the call currently executing. Sub-agent
    /// progress is reported back onto the parent tool card addressed by this id.
    pub tool_call_id: &'a str,
}

pub fn match_tool_call<'a>(
    tools: &'a [ChatToolDefinition],
    function_name: &str,
) -> Option<&'a ChatToolDefinition> {
    if let Some(exact) = tools
        .iter()
        .find(|tool| tool.openai_tool_name() == function_name || tool.name == function_name)
    {
        return Some(exact);
    }
    // 旧名归一化：工具被移除/合并/改名后（find→glob、ls→read、list_background→bash_output、
    // todo_update→todo_write、skill_activate→skill），模型仍可能按旧名出牌。规整到现名后精确再比一次。
    let canonical = crate::mcp::types::canonical_tool_name(function_name);
    if canonical != function_name {
        if let Some(hit) = tools
            .iter()
            .find(|tool| tool.openai_tool_name() == canonical || tool.name == canonical)
        {
            return Some(hit);
        }
    }
    // 大小写不敏感兜底：Cursor 系模型（grok-composer 等）训练时烙着大写工具名
    // （Grep/Read/Bash），在别的工具集下会间歇性按原名出牌。仅当忽略大小写后
    // **唯一**命中才采用——MCP 服务器可能真有同名不同大小写的工具，多义时不猜。
    let lowered = function_name.to_ascii_lowercase();
    let mut case_insensitive = tools.iter().filter(|tool| {
        tool.openai_tool_name().to_ascii_lowercase() == lowered
            || tool.name.to_ascii_lowercase() == lowered
    });
    let candidate = case_insensitive.next()?;
    if case_insensitive.next().is_some() {
        return None; // 多义：不猜，走未知工具自愈路径。
    }
    Some(candidate)
}

pub fn unknown_tool_record(call: &PendingToolCall, round: u32, error: String) -> ToolCallRecord {
    let now = chrono::Local::now().timestamp();
    ToolCallRecord {
        id: call.id.clone(),
        name: call.function_name.clone(),
        source: "unknown".to_string(),
        server_id: None,
        arguments: call.arguments_raw.clone(),
        status: ToolCallStatus::Error,
        result_preview: None,
        error: Some(error),
        duration_ms: Some(0),
        started_at: Some(now),
        completed_at: Some(now),
        round,
        sensitive: false,
        artifacts: Vec::new(),
        trace_id: None,
        span_id: None,
        structured_content: None,
    }
}

pub fn invalid_tool_arguments_record(
    call: &PendingToolCall,
    tool: &ChatToolDefinition,
    round: u32,
    error: String,
) -> ToolCallRecord {
    let now = chrono::Local::now().timestamp();
    ToolCallRecord {
        id: call.id.clone(),
        name: tool.name.clone(),
        source: tool.source.clone(),
        server_id: tool.server_id.clone(),
        arguments: call.arguments_raw.clone(),
        status: ToolCallStatus::Error,
        result_preview: None,
        error: Some(error),
        duration_ms: Some(0),
        started_at: Some(now),
        completed_at: Some(now),
        round,
        sensitive: tool.sensitive,
        artifacts: Vec::new(),
        trace_id: None,
        span_id: None,
        structured_content: None,
    }
}

pub async fn execute_tool_call(
    host: &dyn AgentHost,
    executor: &dyn ToolExecutor,
    settings: &Settings,
    ctx: &ToolExecutionContext<'_>,
    tool: &ChatToolDefinition,
    call: PendingToolCall,
    skill_cache: Option<&mut skills::SkillRunCache>,
) -> (ToolCallRecord, String, Vec<Value>) {
    // 宽容 coercion（对齐 pi validateToolArguments 的 Value.Convert + coerceWithJsonSchema）：
    // 校验前先按 schema 把「差一层类型」的参数掰正（"5"→5、"true"→true、5→"5"），
    // 弱模型的这类口误不值得烧掉一整轮。`arguments_raw` 保持模型原话（展示/落盘/回放）。
    let call = {
        let mut call = call;
        call.arguments =
            coerce_tool_arguments(&tool.input_schema, std::mem::take(&mut call.arguments));
        call
    };
    let now = chrono::Local::now().timestamp();
    let mut record = ToolCallRecord {
        id: call.id.clone(),
        name: tool.name.clone(),
        source: tool.source.clone(),
        server_id: tool.server_id.clone(),
        arguments: call.arguments_raw.clone(),
        status: ToolCallStatus::Pending,
        result_preview: None,
        error: None,
        duration_ms: None,
        started_at: Some(now),
        completed_at: None,
        round: ctx.round,
        sensitive: tool.sensitive,
        artifacts: Vec::new(),
        trace_id: Some(ctx.run_id.to_string()),
        span_id: Some(tool_span_id(ctx.round, &call.id)),
        structured_content: None,
    };
    host.emit_tool_record(ctx.conversation_id, ctx.run_id, ctx.message_id, &record);

    if let Err(err) = validate_tool_arguments(tool, &call.arguments) {
        record.status = ToolCallStatus::Error;
        record.duration_ms = Some(0);
        record.completed_at = Some(chrono::Local::now().timestamp());
        record.error = Some(err.clone());
        host.emit_tool_record(ctx.conversation_id, ctx.run_id, ctx.message_id, &record);
        return (
            record,
            format!("Tool arguments failed schema validation: {err}. Retry this tool call with arguments that match the declared JSON schema."),
            Vec::new(),
        );
    }

    if tool.source == "native" && ask_user::is_ask_user_tool_name(&tool.name) {
        let (record, content) =
            execute_ask_user_call(host, settings, ctx, record, call.arguments.clone()).await;
        return (record, content, Vec::new());
    }

    // Gate. The file/shell tools (read/write/edit/bash/grep/find/ls) are
    // governed by a single per-conversation **session consent**: prompt once,
    // then run freely with full-disk access for the rest of the conversation.
    // `approval_policy`: "auto" implicitly consents (no prompt); "always_confirm"
    // confirms each call (the per-call prompt IS the gate — no separate session
    // consent); the default prompts once per conversation. Everything else (MCP
    // tools, etc.) keeps the existing per-call approval.
    let skip = |record: &mut ToolCallRecord, reason: &str| {
        record.status = ToolCallStatus::Skipped;
        record.completed_at = Some(chrono::Local::now().timestamp());
        record.error = Some(reason.to_string());
    };

    if super::prepare::tool_requires_session_consent(tool) {
        let policy = settings.chat_tools.approval_policy.as_str();
        if policy == "always_confirm" {
            if !host.request_tool_approval(ctx, &record).await {
                skip(&mut record, "Tool call was not approved");
                host.emit_tool_record(ctx.conversation_id, ctx.run_id, ctx.message_id, &record);
                let content = record.error.clone().unwrap_or_default();
                return (record, content, Vec::new());
            }
        } else if policy != "auto" && !host.request_session_consent(ctx).await {
            skip(&mut record, "用户未授权本会话使用文件 / 命令工具");
            host.emit_tool_record(ctx.conversation_id, ctx.run_id, ctx.message_id, &record);
            let content = record.error.clone().unwrap_or_default();
            return (record, content, Vec::new());
        }
    } else if tool_requires_approval(settings, tool) {
        let approved = host.request_tool_approval(ctx, &record).await;
        if !approved {
            skip(&mut record, "Tool call was not approved");
            host.emit_tool_record(ctx.conversation_id, ctx.run_id, ctx.message_id, &record);
            let content = record.error.clone().unwrap_or_default();
            return (record, content, Vec::new());
        }
    }

    record.status = ToolCallStatus::Running;
    host.emit_tool_record(ctx.conversation_id, ctx.run_id, ctx.message_id, &record);
    let started = Instant::now();
    let timeout_ms = effective_tool_timeout_ms(settings, tool, &call.arguments);
    let result = tokio::select! {
        result = timeout(
            Duration::from_millis(timeout_ms),
            executor.call(ctx, tool, call.arguments.clone(), skill_cache),
        ) => result,
        _ = host.wait_for_generation_inactive(ctx.conversation_id, ctx.generation) => {
            record.status = ToolCallStatus::Cancelled;
            record.duration_ms = Some(started.elapsed().as_millis() as u64);
            record.completed_at = Some(chrono::Local::now().timestamp());
            record.error = Some("Tool call cancelled".to_string());
            host.emit_tool_record(ctx.conversation_id, ctx.run_id, ctx.message_id, &record);
            let content = record.error.clone().unwrap_or_default();
            return (record, content, Vec::new());
        }
    };
    record.duration_ms = Some(started.elapsed().as_millis() as u64);
    record.completed_at = Some(chrono::Local::now().timestamp());
    let max_tool_output_chars = settings.chat_tools.max_tool_output_chars;
    let mut follow_ups: Vec<Value> = Vec::new();
    let tool_content = match result {
        Ok(Ok(mut output)) if !output.is_error => {
            assign_artifact_ids(&mut output.artifacts);
            if tool.name == "present_artifacts" {
                complete_artifact_presentation(&mut output);
            }
            record.status = ToolCallStatus::Success;
            record.artifacts = output.artifacts.clone();
            record.structured_content = output.structured_content.clone();
            follow_ups = output.follow_up_user_messages.clone();
            record.result_preview = Some(limit_tool_text_for_model(
                &format_tool_result_preview(&tool_content_with_structured_output(
                    &output,
                    &tool.source,
                )),
                max_tool_output_chars,
            ));
            limit_tool_text_for_model(
                &tool_content_with_structured_output(&output, &tool.source),
                max_tool_output_chars,
            )
        }
        Ok(Ok(output)) => {
            record.status = ToolCallStatus::Error;
            record.structured_content = output.structured_content.clone();
            record.error = Some(truncate_chars(&output.content, 1000));
            limit_tool_text_for_model(
                &tool_content_with_structured_output(&output, &tool.source),
                max_tool_output_chars,
            )
        }
        Ok(Err(err)) => {
            record.status = ToolCallStatus::Error;
            record.error = Some(err.clone());
            limit_tool_text_for_model(&err, max_tool_output_chars)
        }
        Err(_) => {
            record.status = ToolCallStatus::Error;
            let err = format_tool_timeout_error(tool, timeout_ms, &call.arguments);
            record.error = Some(err.clone());
            // 让模型在 tool result 里也能读到完整说明（不只在 error 字段）
            err
        }
    };
    host.emit_tool_record(ctx.conversation_id, ctx.run_id, ctx.message_id, &record);
    (record, tool_content, follow_ups)
}

async fn execute_ask_user_call(
    host: &dyn AgentHost,
    settings: &Settings,
    ctx: &ToolExecutionContext<'_>,
    mut record: ToolCallRecord,
    arguments: Value,
) -> (ToolCallRecord, String) {
    let prompt = match ask_user::normalize_prompt(arguments) {
        Ok(prompt) => prompt,
        Err(err) => {
            record.status = ToolCallStatus::Error;
            record.duration_ms = Some(0);
            record.completed_at = Some(chrono::Local::now().timestamp());
            record.error = Some(err.clone());
            host.emit_tool_record(ctx.conversation_id, ctx.run_id, ctx.message_id, &record);
            return (
                record,
                format!("Invalid ask_user prompt: {err}. Retry with a valid questions payload."),
            );
        }
    };

    record.status = ToolCallStatus::Running;
    record.structured_content = Some(ask_user::structured_content(
        &prompt,
        ask_user::ASK_USER_PHASE_AWAITING,
        &Default::default(),
    ));
    host.emit_tool_record(ctx.conversation_id, ctx.run_id, ctx.message_id, &record);

    let started = Instant::now();
    let response = host
        .request_user_response(ctx, &record, prompt.clone())
        .await;
    record.duration_ms = Some(started.elapsed().as_millis() as u64);
    record.completed_at = Some(chrono::Local::now().timestamp());
    record.structured_content = Some(ask_user::structured_content(
        &prompt,
        &response.phase,
        &response.answers,
    ));
    let content = ask_user::tool_result_content(&response);
    match response.phase.as_str() {
        ask_user::ASK_USER_PHASE_ANSWERED => {
            record.status = ToolCallStatus::Success;
            record.result_preview = Some("User answered clarification questions".to_string());
        }
        ask_user::ASK_USER_PHASE_SKIPPED => {
            record.status = ToolCallStatus::Skipped;
            record.result_preview = Some("User skipped clarification".to_string());
        }
        ask_user::ASK_USER_PHASE_TIMEOUT => {
            record.status = ToolCallStatus::Skipped;
            record.result_preview = Some("Clarification timed out".to_string());
        }
        ask_user::ASK_USER_PHASE_CANCELLED => {
            record.status = ToolCallStatus::Cancelled;
            record.error = Some("Clarification cancelled".to_string());
        }
        _ => {
            record.status = ToolCallStatus::Error;
            record.error = Some(format!(
                "Invalid ask_user response phase: {}",
                response.phase
            ));
        }
    }
    host.emit_tool_record(ctx.conversation_id, ctx.run_id, ctx.message_id, &record);
    (
        record,
        limit_tool_text_for_model(&content, settings.chat_tools.max_tool_output_chars),
    )
}

pub fn disabled_tool_content(call: &PendingToolCall) -> Option<String> {
    disabled_builtin_tool_feedback(&call.function_name)
}

pub fn tool_requires_approval(settings: &Settings, tool: &ChatToolDefinition) -> bool {
    if builtin_tool_bypasses_approval(tool) {
        return false;
    }
    match settings.chat_tools.approval_policy.as_str() {
        "auto" => false,
        "always_confirm" => true,
        // MCP annotations are self-reported by the server and cannot weaken approval.
        // The explicit user-side `auto` policy above remains the trusted opt-out.
        _ if tool.source == "mcp" => true,
        _ => tool.sensitive,
    }
}

fn tool_span_id(round: u32, tool_call_id: &str) -> String {
    format!("tool_round_{round}_{tool_call_id}")
}

pub fn validate_tool_arguments(tool: &ChatToolDefinition, arguments: &Value) -> Result<(), String> {
    validate_schema_value(&tool.input_schema, arguments, "arguments")
}

/// 按 JSON Schema 做宽容 coercion（pi `coerceWithJsonSchema` 的 Rust 移植）：
/// 类型差一层皮的值在校验前被掰正，语义不明的值原样保留交给校验报错。
/// 只做无损/明确的转换——字符串数字→数字、"true"/"false"→bool、数字/布尔→字符串、
/// 对 object/array 递归；anyOf/oneOf 先试原值、再逐成员试 coercion。
pub(crate) fn coerce_tool_arguments(schema: &Value, value: Value) -> Value {
    if schema.is_null() || schema.as_object().is_some_and(|object| object.is_empty()) {
        return value;
    }
    coerce_with_schema(schema, value)
}

fn coerce_with_schema(schema: &Value, mut value: Value) -> Value {
    if let Some(all_of) = schema.get("allOf").and_then(Value::as_array) {
        for nested in all_of {
            value = coerce_with_schema(nested, value);
        }
    }
    if let Some(any_of) = schema.get("anyOf").and_then(Value::as_array) {
        value = coerce_with_union(any_of, value);
    }
    if let Some(one_of) = schema.get("oneOf").and_then(Value::as_array) {
        value = coerce_with_union(one_of, value);
    }

    let types = schema_types(schema);
    let matches_union_member = types.len() > 1
        && types
            .iter()
            .any(|type_name| value_matches_schema_type(type_name, &value));
    if !types.is_empty() && !matches_union_member {
        for type_name in &types {
            let candidate = coerce_primitive_by_type(&value, type_name);
            if let Some(candidate) = candidate {
                value = candidate;
                break;
            }
        }
    }

    if types.iter().any(|t| t == "object") {
        if let Some(object) = value.as_object_mut() {
            let properties = schema.get("properties").and_then(Value::as_object);
            if let Some(properties) = properties {
                let required = schema
                    .get("required")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<std::collections::HashSet<_>>();
                for (key, child_schema) in properties {
                    if let Some(child) = object.remove(key) {
                        // 模型经常把未设置的 optional 参数显式写成 null。若 schema 本身
                        // 不接受 null，就应视为省略；转成 0/false/"" 会改变工具语义。
                        if child.is_null()
                            && !required.contains(key.as_str())
                            && validate_schema_value(child_schema, &child, "value").is_err()
                        {
                            continue;
                        }
                        object.insert(key.clone(), coerce_with_schema(child_schema, child));
                    }
                }
            }
            if let Some(additional) = schema
                .get("additionalProperties")
                .filter(|extra| extra.is_object())
            {
                let extra_keys: Vec<String> = object
                    .keys()
                    .filter(|key| {
                        properties
                            .map(|props| !props.contains_key(*key))
                            .unwrap_or(true)
                    })
                    .cloned()
                    .collect();
                for key in extra_keys {
                    if let Some(child) = object.remove(&key) {
                        object.insert(key, coerce_with_schema(additional, child));
                    }
                }
            }
        }
    }
    if types.iter().any(|t| t == "array") {
        if let (Some(items), Some(array)) = (schema.get("items"), value.as_array_mut()) {
            if items.is_object() {
                for item in array.iter_mut() {
                    let taken = std::mem::take(item);
                    *item = coerce_with_schema(items, taken);
                }
            }
        }
    }
    value
}

/// anyOf/oneOf：原值已匹配任一成员则不动；否则逐成员试 coercion，取第一个校验通过的。
fn coerce_with_union(options: &[Value], value: Value) -> Value {
    if options
        .iter()
        .any(|option| validate_schema_value(option, &value, "v").is_ok())
    {
        return value;
    }
    for option in options {
        let candidate = coerce_with_schema(option, value.clone());
        if validate_schema_value(option, &candidate, "v").is_ok() {
            return candidate;
        }
    }
    value
}

fn schema_types(schema: &Value) -> Vec<String> {
    match schema.get("type") {
        Some(Value::String(type_name)) => vec![type_name.clone()],
        Some(Value::Array(type_names)) => type_names
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// 单个标量的定向 coercion（pi `coercePrimitiveByType`）。返回 None = 不知道怎么转，原样保留。
fn coerce_primitive_by_type(value: &Value, type_name: &str) -> Option<Value> {
    match type_name {
        "number" => match value {
            Value::String(text) if !text.trim().is_empty() => text
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .map(Value::from),
            Value::Bool(flag) => Some(Value::from(if *flag { 1 } else { 0 })),
            _ => None,
        },
        "integer" => match value {
            Value::String(text) if !text.trim().is_empty() => {
                text.trim().parse::<i64>().ok().map(Value::from)
            }
            Value::Bool(flag) => Some(Value::from(if *flag { 1 } else { 0 })),
            _ => None,
        },
        "boolean" => match value {
            Value::String(text) if text == "true" => Some(Value::Bool(true)),
            Value::String(text) if text == "false" => Some(Value::Bool(false)),
            Value::Number(number) if number.as_i64() == Some(1) => Some(Value::Bool(true)),
            Value::Number(number) if number.as_i64() == Some(0) => Some(Value::Bool(false)),
            _ => None,
        },
        "string" => match value {
            Value::Number(number) => Some(Value::String(number.to_string())),
            Value::Bool(flag) => Some(Value::String(flag.to_string())),
            _ => None,
        },
        "null" => match value {
            Value::String(text) if text.is_empty() => Some(Value::Null),
            Value::Number(number) if number.as_i64() == Some(0) => Some(Value::Null),
            Value::Bool(false) => Some(Value::Null),
            _ => None,
        },
        _ => None,
    }
}

fn validate_schema_value(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    if schema.is_null() || schema.as_object().is_some_and(|object| object.is_empty()) {
        return Ok(());
    }
    if let Some(options) = schema.get("anyOf").and_then(Value::as_array) {
        if options
            .iter()
            .any(|option| validate_schema_value(option, value, path).is_ok())
        {
            return Ok(());
        }
        return Err(format!("{path} does not match any allowed schema"));
    }
    if let Some(options) = schema.get("oneOf").and_then(Value::as_array) {
        let matches = options
            .iter()
            .filter(|option| validate_schema_value(option, value, path).is_ok())
            .count();
        if matches == 1 {
            return Ok(());
        }
        return Err(format!("{path} must match exactly one allowed schema"));
    }
    if let Some(types) = schema.get("type") {
        validate_schema_type(types, value, path)?;
    }
    if let Some(enum_values) = schema.get("enum").and_then(Value::as_array) {
        if !enum_values.iter().any(|candidate| candidate == value) {
            return Err(format!("{path} must be one of the declared enum values"));
        }
    }
    if let Some(object) = value.as_object() {
        validate_object_schema(schema, object, path)?;
    }
    if let Some(items) = schema.get("items") {
        if let Some(array) = value.as_array() {
            for (idx, item) in array.iter().enumerate() {
                validate_schema_value(items, item, &format!("{path}[{idx}]"))?;
            }
        }
    }
    if let Some(max_items) = schema.get("maxItems").and_then(Value::as_u64) {
        if value
            .as_array()
            .is_some_and(|array| array.len() as u64 > max_items)
        {
            return Err(format!("{path} must contain at most {max_items} items"));
        }
    }
    validate_numeric_range(schema, value, path)?;
    Ok(())
}

fn validate_schema_type(types: &Value, value: &Value, path: &str) -> Result<(), String> {
    if let Some(type_name) = types.as_str() {
        if value_matches_schema_type(type_name, value) {
            return Ok(());
        }
        return Err(format!("{path} must be {type_name}"));
    }
    if let Some(type_names) = types.as_array() {
        if type_names
            .iter()
            .filter_map(Value::as_str)
            .any(|type_name| value_matches_schema_type(type_name, value))
        {
            return Ok(());
        }
        return Err(format!("{path} has an invalid type"));
    }
    Ok(())
}

fn value_matches_schema_type(type_name: &str, value: &Value) -> bool {
    match type_name {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        _ => true,
    }
}

fn validate_object_schema(
    schema: &Value,
    object: &serde_json::Map<String, Value>,
    path: &str,
) -> Result<(), String> {
    let properties = schema.get("properties").and_then(Value::as_object);
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for key in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(key) {
                return Err(format!("{path}.{key} is required"));
            }
        }
    }
    if let Some(properties) = properties {
        for (key, child_schema) in properties {
            if let Some(child_value) = object.get(key) {
                validate_schema_value(child_schema, child_value, &format!("{path}.{key}"))?;
            }
        }
    }
    if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false) {
        if let Some(extra_key) = object.keys().find(|key| match properties {
            Some(properties) => !properties.contains_key(*key),
            None => true,
        }) {
            return Err(format!("{path}.{extra_key} is not allowed"));
        }
    }
    Ok(())
}

fn validate_numeric_range(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    let Some(number) = value.as_f64() else {
        return Ok(());
    };
    if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64) {
        if number < minimum {
            return Err(format!("{path} must be at least {minimum}"));
        }
    }
    if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64) {
        if number > maximum {
            return Err(format!("{path} must be at most {maximum}"));
        }
    }
    Ok(())
}

/// MCP 契约要求 structuredContent 进入模型可见文本（除非 text 已含同样 JSON）。
/// native 工具不受此约束：它们的 content 都是为模型精心格式化的文本（read_file 的
/// cat -n、文件变更的 summary+diff、todo/memory 的排版文本），再拼整包 JSON 只会
/// 让 token 翻倍（read_file 行号化后 content 不再等于 JSON，旧逻辑会整包重复追加）。
fn assign_artifact_ids(artifacts: &mut [ChatToolArtifact]) {
    for artifact in artifacts {
        // IDs are a Kivio-owned display capability. Never trust or preserve an
        // ID supplied by an external tool server.
        artifact.id = Some(format!("art_{}", uuid::Uuid::new_v4().simple()));
    }
}

fn complete_artifact_presentation(output: &mut McpToolCallResult) {
    let Some(structured) = output.structured_content.as_mut() else {
        return;
    };
    let Some(ids) = structured
        .get_mut("artifactIds")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for artifact in &output.artifacts {
        let Some(id) = artifact.id.as_deref() else {
            continue;
        };
        if !ids.iter().any(|value| value.as_str() == Some(id)) {
            ids.push(Value::String(id.to_string()));
        }
    }
    output.raw = structured.clone();
}

fn artifact_presentation_hint(artifacts: &[ChatToolArtifact]) -> Option<String> {
    if artifacts.is_empty() {
        return None;
    }
    let items = artifacts
        .iter()
        .filter_map(|artifact| {
            artifact
                .id
                .as_deref()
                .map(|id| format!("- {id}: {} ({})", artifact.name, artifact.mime_type))
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        return None;
    }
    Some(format!(
        "Available artifacts (not shown automatically):\n{}\nCall present_artifacts with selected artifact_ids where the user should see them. Existing local files can be shown with paths.",
        items.join("\n")
    ))
}

fn tool_content_with_structured_output(output: &McpToolCallResult, source: &str) -> String {
    let mut content = if source == "native" {
        output.content.clone()
    } else if let Some(structured_content) = output.structured_content.as_ref() {
        let structured_json = serde_json::to_string(structured_content).unwrap_or_default();
        if structured_json.is_empty() || output.content.contains(&structured_json) {
            output.content.clone()
        } else if output.content.trim().is_empty() {
            structured_json
        } else {
            format!(
                "{}\n\nstructuredContent: {}",
                output.content, structured_json
            )
        }
    } else {
        output.content.clone()
    };
    let is_presentation = output
        .structured_content
        .as_ref()
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        == Some("artifact_presentation");
    if !is_presentation {
        if let Some(hint) = artifact_presentation_hint(&output.artifacts) {
            if !content.trim().is_empty() {
                content.push_str("\n\n");
            }
            content.push_str(&hint);
        }
    }
    content
}

fn effective_tool_timeout_ms(
    settings: &Settings,
    tool: &ChatToolDefinition,
    arguments: &Value,
) -> u64 {
    let default_timeout_ms = settings.chat_tools.tool_timeout_ms;
    if tool.source == "mixer" && tool.name == "mixer_generate_image" {
        return default_timeout_ms.max(crate::chat::image_generation::IMAGE_GENERATION_TIMEOUT_MS);
    }
    if tool.source == "native" && tool.name == "bash" {
        return arguments
            .get("timeout_ms")
            .and_then(|value| value.as_u64())
            .unwrap_or(default_timeout_ms)
            .clamp(1_000, 300_000)
            .max(default_timeout_ms);
    }
    // The `agent` spawn tool runs a whole sub-agent loop whose own budget
    // (SUB_AGENT_MAX_ATTEMPTS × the inner run) is far longer than the default
    // generic tool timeout (120s). Without a longer outer timeout, the generic
    // 120s would fire first and mis-kill a still-running sub-agent. Give the
    // outer call a large backstop and let the sub-agent's inner lifecycle +
    // cascade cancel govern it.
    if tool.source == "native" && tool.name == crate::chat::sub_agent::AGENT_TOOL_NAME {
        return crate::chat::sub_agent::SUB_AGENT_TOOL_TIMEOUT_MS.max(default_timeout_ms);
    }
    // MCP 与其它工具一样走设置里的「工具超时」，不单独封顶。
    default_timeout_ms
}

/// 超时错误给模型用：含工具名、时限、简短参数摘要与可执行建议（避免只回一句「超时」）。
fn format_tool_timeout_error(
    tool: &ChatToolDefinition,
    timeout_ms: u64,
    arguments: &Value,
) -> String {
    let secs = timeout_ms as f64 / 1000.0;
    let arg_summary = timeout_argument_summary(arguments);
    let mut msg = format!(
        "工具调用超时（已中止，限时 {secs:.0}s）。工具：`{}`（source={}）。",
        tool.name, tool.source
    );
    if !arg_summary.is_empty() {
        msg.push_str(&format!(" 参数摘要：{arg_summary}"));
    }
    msg.push_str(
        " 请勿原样重试同一条调用。建议：缩小单次任务（拆成更小的步骤）；\
核对路径与语法后换一版参数再试。需要更长等待可在设置中提高「工具超时」。",
    );
    msg
}

fn timeout_argument_summary(arguments: &Value) -> String {
    // 优先 command 字段（officecli MCP / bash）
    let raw = arguments
        .get("command")
        .map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .or_else(|| {
            arguments
                .get("cmd")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            let s = arguments.to_string();
            if s.len() > 240 {
                format!("{}…", s.chars().take(240).collect::<String>())
            } else {
                s
            }
        });
    let compact = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() > 220 {
        format!("{}…", compact.chars().take(220).collect::<String>())
    } else {
        compact
    }
}

pub fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut out = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

fn limit_tool_text_for_model(value: &str, max_chars: Option<usize>) -> String {
    match max_chars {
        Some(max_chars) => truncate_tool_content_for_model(value, max_chars),
        None => value.to_string(),
    }
}

/// 头+尾保留式截断：前 1/2 预算 + 后 1/4 预算，中间替换为截断说明。
/// 尾部保留是关键——编译错误、测试失败摘要通常在长输出的末尾。
fn truncate_tool_content_for_model(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_string();
    }
    let head_chars = max_chars / 2;
    let tail_chars = max_chars / 4;
    let head: String = value.chars().take(head_chars).collect();
    let tail: String = value
        .chars()
        .skip(char_count.saturating_sub(tail_chars))
        .collect();
    let omitted = char_count - head_chars - tail_chars;
    format!(
        "{head}\n\n[Tool output truncated: original {char_count} chars, showing first {head_chars} and last {tail_chars}; {omitted} chars omitted.]\n\n{tail}"
    )
}

fn format_tool_result_preview(content: &str) -> String {
    let trimmed = content.trim();
    let json_str = trimmed
        .strip_prefix("stdout:")
        .map(str::trim)
        .unwrap_or(trimmed);
    let Ok(value) = serde_json::from_str::<Value>(json_str) else {
        return content.to_string();
    };

    if let Some(answer) = value
        .get("answer")
        .and_then(|answer| answer.as_str())
        .map(str::trim)
        .filter(|answer| !answer.is_empty())
    {
        return format!("答: {answer}");
    }

    let Some(results) = value.get("results").and_then(|results| results.as_array()) else {
        return content.to_string();
    };

    if results.is_empty() {
        return "无搜索结果".to_string();
    }

    let query = value
        .get("query")
        .and_then(|query| query.as_str())
        .unwrap_or_default();
    let query_label = if query.is_empty() {
        String::new()
    } else {
        format!("「{query}」")
    };
    let first = &results[0];
    let title = first
        .get("title")
        .or_else(|| first.get("url"))
        .and_then(|title| title.as_str())
        .unwrap_or_default();
    let snippet = first
        .get("content")
        .or_else(|| first.get("raw_content"))
        .and_then(|content| content.as_str())
        .unwrap_or_default();
    let snippet: String = snippet.chars().take(80).collect();
    let head = format!("{} 条结果{query_label}", results.len());
    if title.is_empty() && snippet.is_empty() {
        return head;
    }
    if snippet.is_empty() {
        return format!("{head}: {title}");
    }
    format!("{head}: {title} - {snippet}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };

    use crate::chat::agent::host::AgentHostFuture;
    use crate::chat::types::{ChatMessageSegment, ToolCallStatus};
    use crate::mcp::types::native_skill_activate_tool;

    #[derive(Default)]
    struct ExecuteTestHost {
        approvals: AtomicUsize,
        consents: AtomicUsize,
        deny_consent: bool,
        records: Mutex<Vec<ToolCallRecord>>,
    }

    impl AgentHost for ExecuteTestHost {
        fn emit_stream_delta(
            &self,
            _conversation_id: &str,
            _run_id: &str,
            _message_id: &str,
            _delta: &str,
            _reasoning_delta: Option<&str>,
            _segment: Option<&ChatMessageSegment>,
        ) {
        }

        fn emit_tool_record(
            &self,
            _conversation_id: &str,
            _run_id: &str,
            _message_id: &str,
            record: &ToolCallRecord,
        ) {
            self.records
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .push(record.clone());
        }

        fn request_tool_approval<'a>(
            &'a self,
            _ctx: &'a ToolExecutionContext<'a>,
            _record: &'a ToolCallRecord,
        ) -> AgentHostFuture<'a, bool> {
            self.approvals.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { true })
        }

        fn request_session_consent<'a>(
            &'a self,
            _ctx: &'a ToolExecutionContext<'a>,
        ) -> AgentHostFuture<'a, bool> {
            self.consents.fetch_add(1, Ordering::SeqCst);
            let granted = !self.deny_consent;
            Box::pin(async move { granted })
        }

        fn request_user_response<'a>(
            &'a self,
            _ctx: &'a ToolExecutionContext<'a>,
            _record: &'a ToolCallRecord,
            _prompt: crate::chat::ask_user::AskUserPromptPayload,
        ) -> AgentHostFuture<'a, crate::chat::ask_user::AskUserResponseResult> {
            Box::pin(async { crate::chat::ask_user::skipped_response() })
        }

        fn is_generation_active(&self, _conversation_id: &str, _generation: u64) -> bool {
            true
        }

        fn wait_for_generation_inactive<'a>(
            &'a self,
            _conversation_id: &'a str,
            _generation: u64,
        ) -> AgentHostFuture<'a, ()> {
            Box::pin(async { std::future::pending::<()>().await })
        }
    }

    #[derive(Default)]
    struct ExecuteTestExecutor {
        calls: AtomicUsize,
        structured_content: Option<Value>,
    }

    impl ToolExecutor for ExecuteTestExecutor {
        fn call<'a>(
            &'a self,
            _ctx: &'a ToolExecutionContext<'a>,
            tool: &'a ChatToolDefinition,
            _arguments: Value,
            _skill_cache: Option<&'a mut skills::SkillRunCache>,
        ) -> ToolExecutorFuture<'a> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let content = format!("result:{}", tool.name);
            let structured_content = self.structured_content.clone();
            Box::pin(async move {
                Ok(McpToolCallResult {
                    content,
                    is_error: false,
                    raw: Value::Null,
                    artifacts: Vec::new(),
                    structured_content,
                    follow_up_user_messages: Vec::new(),
                })
            })
        }
    }

    fn test_execution_context() -> ToolExecutionContext<'static> {
        ToolExecutionContext {
            conversation_id: "conversation",
            run_id: "run",
            message_id: "message",
            generation: 1,
            round: 2,
            depth: 0,
            tool_conversation_id: "conversation",
            tool_call_id: "call",
        }
    }

    fn test_pending_call(id: &str, function_name: &str, arguments: Value) -> PendingToolCall {
        PendingToolCall {
            id: id.to_string(),
            function_name: function_name.to_string(),
            arguments_raw: serde_json::to_string(&arguments).expect("serialize test args"),
            arguments,
            arguments_parse_error: None,
            signature: None,
        }
    }

    fn sensitive_test_tool() -> ChatToolDefinition {
        ChatToolDefinition {
            id: "native__write_file".to_string(),
            name: "write".to_string(),
            description: "Write file".to_string(),
            source: "native".to_string(),
            server_id: None,
            server_name: Some("Kivio".to_string()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
            sensitive: true,
            annotations: None,
            output_schema: None,
        }
    }

    fn named_test_tool(name: &str) -> ChatToolDefinition {
        ChatToolDefinition {
            id: format!("native__{name}"),
            name: name.to_string(),
            description: String::new(),
            source: "native".to_string(),
            server_id: None,
            server_name: Some("Kivio".to_string()),
            input_schema: serde_json::json!({ "type": "object" }),
            sensitive: false,
            annotations: None,
            output_schema: None,
        }
    }

    #[test]
    fn assigns_fresh_kivio_owned_artifact_ids() {
        let mut artifacts = vec![
            ChatToolArtifact {
                id: None,
                name: "report.txt".to_string(),
                mime_type: "text/plain".to_string(),
                data_url: String::new(),
                size_bytes: None,
                path: None,
            },
            ChatToolArtifact {
                id: Some("art_existing".to_string()),
                name: "chart.png".to_string(),
                mime_type: "image/png".to_string(),
                data_url: String::new(),
                size_bytes: None,
                path: None,
            },
        ];

        assign_artifact_ids(&mut artifacts);

        let first = artifacts[0].id.as_deref().expect("first artifact id");
        let second = artifacts[1].id.as_deref().expect("second artifact id");
        assert!(first.starts_with("art_") && first.len() > 4);
        assert!(second.starts_with("art_") && second.len() > 4);
        assert_ne!(first, second);
        assert_ne!(second, "art_existing");
    }

    #[test]
    fn completes_path_based_presentation_with_assigned_ids() {
        let mut output = McpToolCallResult {
            content: "display".to_string(),
            is_error: false,
            raw: serde_json::json!({}),
            artifacts: vec![ChatToolArtifact {
                id: Some("art_local".to_string()),
                name: "local.txt".to_string(),
                mime_type: "text/plain".to_string(),
                data_url: String::new(),
                size_bytes: Some(5),
                path: Some("C:\\tmp\\local.txt".to_string()),
            }],
            structured_content: Some(serde_json::json!({
                "type": "artifact_presentation",
                "artifactIds": ["art_existing"]
            })),
            follow_up_user_messages: Vec::new(),
        };

        complete_artifact_presentation(&mut output);

        let expected = serde_json::json!({
            "type": "artifact_presentation",
            "artifactIds": ["art_existing", "art_local"]
        });
        assert_eq!(output.structured_content, Some(expected.clone()));
        assert_eq!(output.raw, expected);
        assert_eq!(
            tool_content_with_structured_output(&output, "native"),
            "display"
        );
    }

    #[test]
    fn artifact_hint_exposes_ids_without_claiming_automatic_display() {
        let artifacts = vec![ChatToolArtifact {
            id: Some("art_report".to_string()),
            name: "report.txt".to_string(),
            mime_type: "text/plain".to_string(),
            data_url: String::new(),
            size_bytes: None,
            path: None,
        }];

        let hint = artifact_presentation_hint(&artifacts).expect("artifact hint");

        assert!(hint.contains("art_report: report.txt (text/plain)"));
        assert!(hint.contains("not shown automatically"));
        assert!(hint.contains("present_artifacts"));
        assert!(hint.contains("Existing local files can be shown with paths"));
    }

    #[test]
    fn match_tool_call_falls_back_to_case_insensitive_unique_hit() {
        // Cursor 系模型按训练时的大写工具名出牌（Grep/Read/Bash）——唯一命中时采用。
        let tools = vec![named_test_tool("grep"), named_test_tool("read")];
        assert_eq!(match_tool_call(&tools, "grep").unwrap().name, "grep");
        assert_eq!(match_tool_call(&tools, "Grep").unwrap().name, "grep");
        assert_eq!(match_tool_call(&tools, "READ").unwrap().name, "read");
        assert!(match_tool_call(&tools, "Glob").is_none());
    }

    #[test]
    fn match_tool_call_routes_legacy_names_to_current_tools() {
        // 旧名（移除/合并/改名前的名字）经归一化路由到现工具。
        let tools = vec![
            named_test_tool("read"),
            named_test_tool("glob"),
            named_test_tool("bash_output"),
            named_test_tool("todo_write"),
        ];
        assert_eq!(match_tool_call(&tools, "ls").unwrap().name, "read");
        assert_eq!(match_tool_call(&tools, "find").unwrap().name, "glob");
        assert_eq!(
            match_tool_call(&tools, "list_background").unwrap().name,
            "bash_output"
        );
        assert_eq!(
            match_tool_call(&tools, "todo_update").unwrap().name,
            "todo_write"
        );
    }

    #[test]
    fn match_tool_call_refuses_ambiguous_case_variants() {
        // MCP 可能真有同名不同大小写的工具：精确匹配照常，忽略大小写多义时不猜。
        let tools = vec![named_test_tool("fetch"), named_test_tool("Fetch")];
        assert_eq!(match_tool_call(&tools, "fetch").unwrap().name, "fetch");
        assert_eq!(match_tool_call(&tools, "Fetch").unwrap().name, "Fetch");
        assert!(match_tool_call(&tools, "FETCH").is_none());
    }

    #[test]
    fn unknown_tool_record_is_error_metadata() {
        let call = PendingToolCall {
            id: "call_1".to_string(),
            function_name: "missing".to_string(),
            arguments: Value::Null,
            arguments_raw: "{}".to_string(),
            arguments_parse_error: None,
            signature: None,
        };

        let record = unknown_tool_record(&call, 2, "Unknown tool requested: missing".to_string());

        assert!(matches!(record.status, ToolCallStatus::Error));
        assert_eq!(record.round, 2);
        assert_eq!(record.source, "unknown");
    }

    #[tokio::test]
    async fn schema_validation_fails_before_approval_or_execution() {
        let host = ExecuteTestHost::default();
        let executor = ExecuteTestExecutor::default();
        let settings = Settings::default();
        let tool = sensitive_test_tool();
        let call = test_pending_call("call_invalid", "write", serde_json::json!({}));

        let (record, content, _) = execute_tool_call(
            &host,
            &executor,
            &settings,
            &test_execution_context(),
            &tool,
            call,
            None,
        )
        .await;

        assert!(matches!(record.status, ToolCallStatus::Error));
        assert!(record
            .error
            .as_deref()
            .is_some_and(|error| error.contains("arguments.path is required")));
        assert!(content.contains("schema validation"));
        assert_eq!(host.approvals.load(Ordering::SeqCst), 0);
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
        let records = host.records.lock().unwrap_or_else(|err| err.into_inner());
        assert_eq!(records.len(), 2);
        assert!(matches!(records[0].status, ToolCallStatus::Pending));
        assert!(matches!(records[1].status, ToolCallStatus::Error));
    }

    #[test]
    fn schema_validation_rejects_additional_properties_without_declared_properties() {
        let mut tool = sensitive_test_tool();
        tool.input_schema = serde_json::json!({
            "type": "object",
            "additionalProperties": false
        });

        let err = validate_tool_arguments(&tool, &serde_json::json!({ "extra": true }))
            .expect_err("extra properties should be rejected");

        assert!(err.contains("arguments.extra is not allowed"));
    }

    #[tokio::test]
    async fn successful_tool_records_trace_and_structured_content() {
        let host = ExecuteTestHost::default();
        let executor = ExecuteTestExecutor {
            calls: AtomicUsize::new(0),
            structured_content: Some(serde_json::json!({ "answer": 42 })),
        };
        let settings = Settings::default();
        let mut tool = sensitive_test_tool();
        tool.sensitive = false;
        let call = test_pending_call(
            "call_ok",
            "write",
            serde_json::json!({ "path": "/tmp/out.txt", "content": "hello" }),
        );

        let (record, content, _) = execute_tool_call(
            &host,
            &executor,
            &settings,
            &test_execution_context(),
            &tool,
            call,
            None,
        )
        .await;

        assert!(matches!(record.status, ToolCallStatus::Success));
        assert_eq!(record.trace_id.as_deref(), Some("run"));
        assert_eq!(record.span_id.as_deref(), Some("tool_round_2_call_ok"));
        assert_eq!(
            record.structured_content.as_ref(),
            Some(&serde_json::json!({ "answer": 42 }))
        );
        // native 工具的 content 是为模型格式化的文本，不再追加整包 structuredContent
        // JSON（record/前端仍持有完整 structured_content）。MCP 契约见下一个测试。
        assert!(!content.contains("structuredContent"));
        assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn file_tool_skipped_when_session_consent_denied() {
        let host = ExecuteTestHost {
            deny_consent: true,
            ..Default::default()
        };
        let executor = ExecuteTestExecutor::default();
        let mut settings = Settings::default();
        // Exercise the per-conversation consent gate (default policy is now "auto",
        // which bypasses it).
        settings.chat_tools.approval_policy = "readonly_auto_sensitive_confirm".to_string();
        let tool = sensitive_test_tool(); // native "write" — consent-gated
        let call = test_pending_call(
            "call_write",
            "write",
            serde_json::json!({ "path": "/tmp/out.txt", "content": "hi" }),
        );

        let (record, _content, _) = execute_tool_call(
            &host,
            &executor,
            &settings,
            &test_execution_context(),
            &tool,
            call,
            None,
        )
        .await;

        assert!(matches!(record.status, ToolCallStatus::Skipped));
        assert_eq!(host.consents.load(Ordering::SeqCst), 1);
        // No per-call approval under default policy, and the tool never ran.
        assert_eq!(host.approvals.load(Ordering::SeqCst), 0);
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn file_tool_uses_session_consent_not_per_call_approval() {
        let host = ExecuteTestHost::default(); // grants consent
        let executor = ExecuteTestExecutor::default();
        let mut settings = Settings::default();
        // Per-conversation consent policy (default is now "auto", which bypasses it).
        settings.chat_tools.approval_policy = "readonly_auto_sensitive_confirm".to_string();
        let tool = sensitive_test_tool();
        let call = test_pending_call(
            "call_write",
            "write",
            serde_json::json!({ "path": "/tmp/out.txt", "content": "hi" }),
        );

        let (record, _content, _) = execute_tool_call(
            &host,
            &executor,
            &settings,
            &test_execution_context(),
            &tool,
            call,
            None,
        )
        .await;

        assert!(matches!(record.status, ToolCallStatus::Success));
        assert_eq!(host.consents.load(Ordering::SeqCst), 1);
        assert_eq!(host.approvals.load(Ordering::SeqCst), 0);
        assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    }

    /// MCP structured-content 契约（agent-runtime spec）：MCP 结果的 structuredContent
    /// 必须进入模型可见文本（除非文本已含同样 JSON）。
    #[tokio::test]
    async fn mcp_tool_content_still_carries_structured_content() {
        let host = ExecuteTestHost::default();
        let executor = ExecuteTestExecutor {
            calls: AtomicUsize::new(0),
            structured_content: Some(serde_json::json!({ "answer": 42 })),
        };
        let settings = Settings::default();
        let mut tool = sensitive_test_tool();
        tool.sensitive = false;
        tool.source = "mcp".to_string();
        tool.server_id = Some("server-1".to_string());
        let call = test_pending_call(
            "call_mcp",
            "write",
            serde_json::json!({ "path": "/tmp/out.txt", "content": "hello" }),
        );

        let (record, content, _) = execute_tool_call(
            &host,
            &executor,
            &settings,
            &test_execution_context(),
            &tool,
            call,
            None,
        )
        .await;

        assert!(matches!(record.status, ToolCallStatus::Success));
        assert!(content.contains("structuredContent"));
        assert!(content.contains("\"answer\":42"));
    }

    #[test]
    fn untrusted_mcp_read_only_hint_does_not_bypass_approval() {
        let mut settings = Settings::default();
        settings.chat_tools.approval_policy = "readonly_auto_sensitive_confirm".to_string();
        let mut tool = sensitive_test_tool();
        tool.source = "mcp".to_string();
        tool.sensitive = false;
        tool.annotations = Some(serde_json::json!({ "readOnlyHint": true }));

        assert!(tool_requires_approval(&settings, &tool));
        settings.chat_tools.approval_policy = "auto".to_string();
        assert!(!tool_requires_approval(&settings, &tool));
    }

    #[test]
    fn native_skill_tools_match_by_openai_name() {
        let tool = native_skill_activate_tool();
        // 现名 skill 精确命中，旧名 skill_activate 经 canonical alias 归一化命中。
        assert!(match_tool_call(std::slice::from_ref(&tool), "skill").is_some());
        assert!(match_tool_call(&[tool], "skill_activate").is_some());
    }

    #[test]
    fn format_tool_result_preview_summarizes_tavily_search_json() {
        let raw = r#"stdout:
{
  "answer": null,
  "query": "吉林市 明天 天气",
  "results": [
    {
      "title": "吉林市天气预报",
      "content": "明天晴有时多云，最高33℃"
    }
  ]
}"#;
        let preview = format_tool_result_preview(raw);
        assert!(preview.contains("1 条结果"));
        assert!(preview.contains("吉林市天气预报"));
        assert!(preview.contains("33"));
        assert!(!preview.contains("\"answer\": null"));
    }

    #[test]
    fn truncate_tool_content_for_model_marks_truncated_output() {
        // 头+尾保留：12 字符预算 → 头 6 + 尾 3，中间替换为截断说明。
        let content = "HEADxxOMITTEDyyTAIL_abc";
        let truncated = truncate_tool_content_for_model(content, 12);

        assert!(truncated.starts_with("HEADxx"));
        assert!(truncated.ends_with("abc"));
        assert!(truncated.contains("Tool output truncated"));
        assert!(truncated.contains("original 23 chars"));
        assert!(truncated.contains("first 6 and last 3"));
        assert!(!truncated.contains("OMITTEDyy"));
    }

    #[test]
    fn truncate_tool_content_for_model_keeps_short_output_unchanged() {
        assert_eq!(truncate_tool_content_for_model("abc", 3), "abc");
    }

    #[test]
    fn mixer_image_generation_uses_extended_timeout() {
        let mut settings = Settings::default();
        settings.chat_tools.tool_timeout_ms = 60_000;
        let tool = crate::mcp::types::mixer_generate_image_tool();
        let arguments = serde_json::json!({ "prompt": "draw a quiet desktop assistant" });

        assert_eq!(
            effective_tool_timeout_ms(&settings, &tool, &arguments),
            crate::chat::image_generation::IMAGE_GENERATION_TIMEOUT_MS
        );
    }

    #[test]
    fn agent_spawn_uses_sub_agent_backstop_timeout() {
        // The `agent` spawn tool must outlast the sub-agent's own run budget so the
        // outer 120s default does not mis-kill a long sub-agent run.
        let mut settings = Settings::default();
        settings.chat_tools.tool_timeout_ms = 120_000;
        let tool = crate::chat::sub_agent::agent_tool(&[]);
        let arguments = serde_json::json!({ "prompt": "do a focused sub-task" });

        assert_eq!(
            effective_tool_timeout_ms(&settings, &tool, &arguments),
            crate::chat::sub_agent::SUB_AGENT_TOOL_TIMEOUT_MS
        );
        assert!(
            crate::chat::sub_agent::SUB_AGENT_TOOL_TIMEOUT_MS > 120_000,
            "agent timeout must exceed the default generic tool timeout"
        );
    }

    #[test]
    fn agent_spawn_respects_larger_user_default_timeout() {
        // If the user configured an even larger generic timeout, honor it.
        let mut settings = Settings::default();
        settings.chat_tools.tool_timeout_ms = crate::chat::sub_agent::SUB_AGENT_TOOL_TIMEOUT_MS + 1;
        let tool = crate::chat::sub_agent::agent_tool(&[]);
        let arguments = serde_json::json!({ "prompt": "do a focused sub-task" });

        assert_eq!(
            effective_tool_timeout_ms(&settings, &tool, &arguments),
            crate::chat::sub_agent::SUB_AGENT_TOOL_TIMEOUT_MS + 1
        );
    }

    #[test]
    fn five_minute_tool_timeout_applies_to_bash() {
        let mut settings = Settings::default();
        settings.chat_tools.tool_timeout_ms = 300_000;
        let bash = crate::mcp::types::native_run_command_tool();

        assert_eq!(
            effective_tool_timeout_ms(&settings, &bash, &serde_json::json!({})),
            300_000
        );
    }

    #[test]
    fn mcp_tool_timeout_follows_settings_like_other_tools() {
        let mut settings = Settings::default();
        settings.chat_tools.tool_timeout_ms = 300_000;
        let tool = ChatToolDefinition {
            id: "mcp__plugin-officecli__officecli".into(),
            name: "officecli".into(),
            description: String::new(),
            source: "mcp".into(),
            server_id: Some("plugin-officecli".into()),
            server_name: Some("OfficeCLI".into()),
            input_schema: serde_json::json!({}),
            sensitive: false,
            annotations: None,
            output_schema: None,
        };
        assert_eq!(
            effective_tool_timeout_ms(
                &settings,
                &tool,
                &serde_json::json!({ "command": "batch …" })
            ),
            300_000
        );
    }

    #[test]
    fn tool_timeout_error_mentions_tool_and_actionable_hint() {
        let tool = ChatToolDefinition {
            id: "mcp__plugin-officecli__officecli".into(),
            name: "officecli".into(),
            description: String::new(),
            source: "mcp".into(),
            server_id: Some("plugin-officecli".into()),
            server_name: Some("OfficeCLI".into()),
            input_schema: serde_json::json!({}),
            sensitive: false,
            annotations: None,
            output_schema: None,
        };
        let err = format_tool_timeout_error(
            &tool,
            300_000,
            &serde_json::json!({
                "command": "batch \"deck.pptx\" --commands \"[{...}]\""
            }),
        );
        assert!(err.contains("超时"));
        assert!(err.contains("officecli"));
        assert!(err.contains("300") || err.contains("300s") || err.contains("300.0"));
        assert!(err.contains("勿原样重试") || err.contains("缩小"));
    }

    #[test]
    fn coercion_fixes_string_number_and_bool() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "offset": { "type": "integer" },
                "ratio": { "type": "number" },
                "recursive": { "type": "boolean" },
                "name": { "type": "string" }
            }
        });
        let coerced = coerce_tool_arguments(
            &schema,
            serde_json::json!({
                "offset": "5",
                "ratio": "0.5",
                "recursive": "true",
                "name": 42
            }),
        );
        assert_eq!(
            coerced,
            serde_json::json!({ "offset": 5, "ratio": 0.5, "recursive": true, "name": "42" })
        );
        // coercion 后必须能过既有校验（这就是它存在的意义）。
        assert!(validate_schema_value(&schema, &coerced, "arguments").is_ok());
    }

    #[test]
    fn coercion_drops_null_optional_properties_instead_of_inventing_defaults() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "domain_type": { "type": "string" },
                "pub_year_min": { "type": "integer" },
                "pub_year_max": { "type": "integer" },
                "nullable_note": { "type": ["string", "null"] }
            },
            "required": ["query"]
        });
        let coerced = coerce_tool_arguments(
            &schema,
            serde_json::json!({
                "query": "Kivio",
                "domain_type": "web",
                "pub_year_min": null,
                "pub_year_max": null,
                "nullable_note": null
            }),
        );
        assert_eq!(
            coerced,
            serde_json::json!({
                "query": "Kivio",
                "domain_type": "web",
                "nullable_note": null
            })
        );
    }

    #[test]
    fn coercion_does_not_turn_required_null_into_a_zero_value() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "count": { "type": "integer" } },
            "required": ["count"]
        });
        let coerced = coerce_tool_arguments(&schema, serde_json::json!({ "count": null }));
        assert_eq!(coerced, serde_json::json!({ "count": null }));
        assert!(validate_schema_value(&schema, &coerced, "arguments").is_err());
    }

    #[test]
    fn coercion_recurses_into_arrays_and_nested_objects() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "edits": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": { "line": { "type": "integer" } }
                    }
                }
            }
        });
        let coerced = coerce_tool_arguments(
            &schema,
            serde_json::json!({ "edits": [ { "line": "12" }, { "line": 3 } ] }),
        );
        assert_eq!(
            coerced,
            serde_json::json!({ "edits": [ { "line": 12 }, { "line": 3 } ] })
        );
    }

    #[test]
    fn coercion_leaves_valid_and_hopeless_values_alone() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "count": { "type": "integer" } }
        });
        // 已合法：原样。
        let valid = serde_json::json!({ "count": 7 });
        assert_eq!(coerce_tool_arguments(&schema, valid.clone()), valid);
        // 掰不动的（非数字字符串）：原样保留，交给校验去报错。
        let hopeless = serde_json::json!({ "count": "many" });
        assert_eq!(coerce_tool_arguments(&schema, hopeless.clone()), hopeless);
        assert!(validate_schema_value(&schema, &hopeless, "arguments").is_err());
    }

    #[test]
    fn coercion_resolves_any_of_by_member_validation() {
        let schema = serde_json::json!({
            "anyOf": [
                { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] },
                { "type": "integer" }
            ]
        });
        // 原值已匹配成员一：不动。
        let object = serde_json::json!({ "path": "a.rs" });
        assert_eq!(coerce_tool_arguments(&schema, object.clone()), object);
        // 字符串数字：coerce 后命中成员二。
        assert_eq!(
            coerce_tool_arguments(&schema, serde_json::json!("12")),
            serde_json::json!(12)
        );
    }
}
