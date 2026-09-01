//! In-memory provider request capture for the developer "Request Debug" panel.
//!
//! When `settings.chat_tools.request_debug_enabled` is on, each model adapter
//! (`openai` / `anthropic` / `responses`) records the full request (url + sanitized
//! headers + body) and the aggregated response (text / tool_calls / finish_reason /
//! usage / error) into a bounded in-memory ring buffer on `AppState`. Nothing is
//! written to disk and the buffer is cleared on process exit.
//!
//! **Zero overhead when disabled**: adapters must check `state.request_debug_enabled()`
//! BEFORE constructing any body/headers/record — this module never runs otherwise.
//!
//! **No secret leak**: [`sanitize_headers`] masks `Authorization` / `x-api-key` style
//! headers to a short preview (`Bearer sk-x…`); the request body never carries the key.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::chat::model::{GenerateOutput, GenerateRequest, ModelUsage, PendingToolCall};
use crate::settings::ModelProvider;
use crate::state::AppState;
use crate::usage::{chat_usage_source_for_label, operation_from_label};

/// Max records kept in the ring buffer. Oldest are evicted first.
pub const REQUEST_DEBUG_CAPACITY: usize = 50;

/// Cap on the recorded response text (chars) so a long answer can't blow up memory.
const MAX_RESPONSE_TEXT_CHARS: usize = 4000;

/// Header keys (lower-cased) whose values carry credentials and must be masked.
const SENSITIVE_HEADER_KEYS: &[&str] = &[
    "authorization",
    "x-api-key",
    "api-key",
    "x-goog-api-key",
    "x-api-token",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestDebugRecord {
    pub id: String,
    pub created_at: i64,
    pub duration_ms: u64,
    pub provider_id: String,
    pub provider_name: String,
    pub model: String,
    pub api_format: String,
    pub operation: String,
    pub source: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub message_id: Option<String>,
    pub status: String,
    pub request: RequestDebugRequest,
    pub response: RequestDebugResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestDebugRequest {
    pub url: String,
    /// Sanitized headers — credential values are already masked.
    pub headers: BTreeMap<String, String>,
    pub body: Value,
    pub stream: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestDebugResponse {
    #[serde(default)]
    pub status_code: Option<u16>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub reasoning: Option<String>,
    /// `null` when there were no tool calls, otherwise an array of call summaries.
    #[serde(default)]
    pub tool_calls: Value,
    #[serde(default)]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub usage: Option<ModelUsage>,
    /// 内置联网搜索解析结果（任务 07-23）：从响应里提取到的 queries + citations；
    /// `null` = 未解析到任何搜索痕迹（模型没搜 / 渠道没返回结构化字段）。
    #[serde(default)]
    pub web_search: Value,
    #[serde(default)]
    pub error: Option<String>,
}

impl RequestDebugResponse {
    /// Summarize a successful [`GenerateOutput`] (text truncated, tool calls + usage kept).
    pub fn from_output(output: &GenerateOutput, status_code: Option<u16>) -> Self {
        Self {
            status_code,
            text: truncate_opt(&output.text),
            reasoning: output.reasoning.clone(),
            tool_calls: tool_calls_to_value(&output.tool_calls),
            finish_reason: output.finish_reason.clone(),
            usage: output.usage.clone(),
            web_search: output
                .web_search
                .as_ref()
                .and_then(|ws| serde_json::to_value(ws).ok())
                .unwrap_or(Value::Null),
            error: None,
        }
    }

    /// Summarize a failed call from its error string + parsed HTTP status (if any).
    pub fn from_error(error: &str, status_code: Option<u16>) -> Self {
        Self {
            status_code,
            tool_calls: Value::Null,
            web_search: Value::Null,
            error: Some(error.to_string()),
            ..Default::default()
        }
    }
}

/// Inputs an adapter hands to [`build_debug_record`]. The adapter owns url/headers/body
/// (built from the SAME functions it uses to send, so the record can't drift) and the
/// response summary; this module derives operation/source and assembles the record.
pub struct DebugRecordArgs<'a> {
    pub provider: &'a ModelProvider,
    pub request: &'a GenerateRequest,
    pub label: &'a str,
    pub started_at: i64,
    pub duration_ms: u64,
    pub status: &'a str,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Value,
    pub stream: bool,
    pub response: RequestDebugResponse,
}

/// Assemble a [`RequestDebugRecord`] from adapter-provided request/response parts.
pub fn build_debug_record(args: DebugRecordArgs<'_>) -> RequestDebugRecord {
    let source = args
        .request
        .metadata
        .usage_source
        .clone()
        .unwrap_or_else(|| chat_usage_source_for_label(args.label));
    let operation = args
        .request
        .metadata
        .usage_operation
        .clone()
        .unwrap_or_else(|| operation_from_label(args.label));
    RequestDebugRecord {
        id: format!("dbg_{}", Uuid::new_v4()),
        created_at: args.started_at,
        duration_ms: args.duration_ms,
        provider_id: args.provider.id.clone(),
        provider_name: args.provider.name.clone(),
        model: args.request.model.clone(),
        api_format: args.provider.api_format.clone(),
        operation,
        source,
        conversation_id: args.request.metadata.conversation_id.clone(),
        message_id: args.request.metadata.message_id.clone(),
        status: args.status.to_string(),
        request: RequestDebugRequest {
            url: args.url,
            headers: args.headers,
            body: args.body,
            stream: args.stream,
        },
        response: args.response,
    }
}

/// Push a record into the ring buffer, evicting the oldest entries past capacity.
/// Also mirrors the whole buffer to disk (newest-first JSONL) so the last
/// [`REQUEST_DEBUG_CAPACITY`] requests survive a restart and can be inspected
/// out-of-band. ponytail: full-buffer rewrite per call (≤50 capped records) —
/// switch to append+rotate only if this ever shows up in a profile.
pub fn record(state: &AppState, record: RequestDebugRecord) {
    let mut buffer = state
        .request_debug
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    while buffer.len() >= REQUEST_DEBUG_CAPACITY {
        buffer.pop_front();
    }
    buffer.push_back(record);
    let disk_view: Vec<RequestDebugRecord> = buffer.iter().rev().cloned().collect();
    drop(buffer);
    persist_to_disk(state, &disk_view);
}

/// Path of the on-disk mirror: `<app_data>/request_debug/records.jsonl`.
fn debug_log_path(state: &AppState) -> PathBuf {
    let root = state
        .usage_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| state.usage_dir.clone());
    root.join("request_debug").join("records.jsonl")
}

/// Rewrite the on-disk mirror with `records` (newest-first, one JSON per line).
/// Best-effort: a debug convenience must never break the request path, so all
/// IO errors are swallowed.
fn persist_to_disk(state: &AppState, records: &[RequestDebugRecord]) {
    let path = debug_log_path(state);
    if let Some(dir) = path.parent() {
        if fs::create_dir_all(dir).is_err() {
            return;
        }
    }
    let mut out = String::with_capacity(records.len() * 512);
    for record in records {
        if let Ok(line) = serde_json::to_string(record) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    let _ = fs::write(&path, out);
}

/// Snapshot the buffer, newest first.
pub fn snapshot(state: &AppState) -> Vec<RequestDebugRecord> {
    let buffer = state
        .request_debug
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    buffer.iter().rev().cloned().collect()
}

/// Empty the buffer and remove the on-disk mirror.
pub fn clear(state: &AppState) {
    state
        .request_debug
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    let _ = fs::remove_file(debug_log_path(state));
}

/// Return a copy of `headers` with credential values masked. Non-sensitive headers pass
/// through unchanged. Comparison is case-insensitive on the header name.
pub fn sanitize_headers(headers: BTreeMap<String, String>) -> BTreeMap<String, String> {
    headers
        .into_iter()
        .map(|(name, value)| {
            if SENSITIVE_HEADER_KEYS.contains(&name.to_ascii_lowercase().as_str()) {
                (name, mask_secret(&value))
            } else {
                (name, value)
            }
        })
        .collect()
}

/// Mask a credential to a short preview. Keeps an optional `Bearer ` scheme prefix and the
/// first 4 chars of the secret (matching the capture-proxy convention `sk-xxxx…`).
pub fn mask_secret(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let (prefix, secret) = match trimmed.split_once(' ') {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("bearer") => {
            (format!("{scheme} "), rest)
        }
        _ => (String::new(), trimmed),
    };
    let preview: String = secret.chars().take(4).collect();
    format!("{prefix}{preview}…")
}

fn truncate_opt(text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    Some(truncate_chars(text, MAX_RESPONSE_TEXT_CHARS))
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str("…[truncated]");
    out
}

fn tool_calls_to_value(calls: &[PendingToolCall]) -> Value {
    if calls.is_empty() {
        return Value::Null;
    }
    Value::Array(
        calls
            .iter()
            .map(|call| {
                serde_json::json!({
                    "id": call.id,
                    "name": call.function_name,
                    "arguments": call.arguments,
                    "argumentsRaw": call.arguments_raw,
                    "parseError": call.arguments_parse_error,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record(id_seed: &str) -> RequestDebugRecord {
        RequestDebugRecord {
            id: format!("dbg_{id_seed}"),
            created_at: 0,
            duration_ms: 0,
            provider_id: "p".into(),
            provider_name: "P".into(),
            model: "m".into(),
            api_format: "openai_chat".into(),
            operation: "chat".into(),
            source: "chat".into(),
            conversation_id: None,
            message_id: None,
            status: "success".into(),
            request: RequestDebugRequest {
                url: "https://x/v1/chat/completions".into(),
                headers: BTreeMap::new(),
                body: serde_json::json!({}),
                stream: false,
            },
            response: RequestDebugResponse::default(),
        }
    }

    #[test]
    fn ring_buffer_evicts_oldest_past_capacity() {
        let state =
            AppState::new_headless(crate::settings::Settings::default(), std::env::temp_dir());
        // Push one past capacity; the first record must have been evicted.
        for i in 0..(REQUEST_DEBUG_CAPACITY + 1) {
            record(&state, sample_record(&i.to_string()));
        }
        let snap = snapshot(&state);
        assert_eq!(snap.len(), REQUEST_DEBUG_CAPACITY);
        // Newest first: the last pushed (id = capacity) is at the front.
        assert_eq!(
            snap.first().unwrap().id,
            format!("dbg_{REQUEST_DEBUG_CAPACITY}")
        );
        // The very first record (id = 0) was evicted; oldest kept is id = 1.
        assert_eq!(snap.last().unwrap().id, "dbg_1");
        assert!(snap.iter().all(|r| r.id != "dbg_0"));
    }

    #[test]
    fn clear_empties_the_buffer() {
        let state =
            AppState::new_headless(crate::settings::Settings::default(), std::env::temp_dir());
        record(&state, sample_record("a"));
        assert_eq!(snapshot(&state).len(), 1);
        clear(&state);
        assert!(snapshot(&state).is_empty());
    }

    #[test]
    fn record_mirrors_to_disk_and_clear_removes_it() {
        // Isolated usage_dir so the on-disk mirror lands in a unique place.
        let base = std::env::temp_dir().join(format!("kivio-dbg-test-{}", Uuid::new_v4()));
        let usage_dir = base.join("usage");
        std::fs::create_dir_all(&usage_dir).unwrap();
        let state = AppState::new_headless(crate::settings::Settings::default(), usage_dir);
        let path = debug_log_path(&state);

        record(&state, sample_record("a"));
        record(&state, sample_record("b"));

        let contents = std::fs::read_to_string(&path).expect("mirror file written");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2, "one JSON line per record");
        // Newest-first: "b" precedes "a".
        assert!(lines[0].contains("dbg_b"));
        assert!(lines[1].contains("dbg_a"));
        // Each line is a standalone parseable record.
        let first: RequestDebugRecord = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first.id, "dbg_b");

        clear(&state);
        assert!(!path.exists(), "clear removes the mirror file");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn sanitize_masks_credentials_and_keeps_the_rest() {
        let mut headers = BTreeMap::new();
        headers.insert("Authorization".into(), "Bearer sk-abcdef123456".into());
        headers.insert("x-api-key".into(), "sk-ant-secret-key".into());
        headers.insert("Accept-Encoding".into(), "identity".into());
        headers.insert("x-session-id".into(), "conv_123".into());

        let sanitized = sanitize_headers(headers);

        assert_eq!(sanitized["Authorization"], "Bearer sk-a…");
        assert_eq!(sanitized["x-api-key"], "sk-a…");
        // Non-sensitive headers are untouched.
        assert_eq!(sanitized["Accept-Encoding"], "identity");
        assert_eq!(sanitized["x-session-id"], "conv_123");
        // No full secret survives anywhere in the sanitized map.
        assert!(sanitized
            .values()
            .all(|v| !v.contains("abcdef123456") && !v.contains("ant-secret-key")));
    }

    #[test]
    fn build_debug_record_produces_sanitized_complete_record() {
        let provider = ModelProvider {
            id: "test".into(),
            name: "Test".into(),
            api_keys: vec!["sk-abcdef123456".into()],
            api_key_legacy: None,
            base_url: "https://api.openai.com/v1".into(),
            available_models: vec!["gpt-5".into()],
            enabled_models: vec!["gpt-5".into()],
            enabled: true,
            api_format: "openai_chat".into(),
            model_overrides: Default::default(),
            compress_request_body: false,
            request: Default::default(),
            active_key_index: 0,
        };
        let request = GenerateRequest {
            model: "gpt-5".into(),
            system: "sys".into(),
            messages: Vec::new(),
            tools: Vec::new(),
            options: Default::default(),
            metadata: crate::chat::model::RequestMetadata {
                label: "Chat API".into(),
                conversation_id: Some("conv_1".into()),
                ..Default::default()
            },
        };
        let output = GenerateOutput {
            text: "hello".into(),
            reasoning: None,
            tool_calls: vec![PendingToolCall {
                id: "call_1".into(),
                function_name: "web_search".into(),
                arguments: serde_json::json!({ "q": "rust" }),
                arguments_raw: "{\"q\":\"rust\"}".into(),
                arguments_parse_error: None,
                signature: None,
            }],
            usage: Some(ModelUsage {
                total_tokens: Some(42),
                ..Default::default()
            }),
            finish_reason: Some("tool_calls".into()),
            provider_messages: Vec::new(),
            cancelled: false,
            web_search: None,
            images: Vec::new(),
            reasoning_items: Vec::new(),
        };

        let mut headers = BTreeMap::new();
        headers.insert(
            "Authorization".into(),
            format!("Bearer {}", provider.api_keys[0]),
        );
        let headers = sanitize_headers(headers);

        let record = build_debug_record(DebugRecordArgs {
            provider: &provider,
            request: &request,
            label: "Chat API",
            started_at: 100,
            duration_ms: 250,
            status: "success",
            url: "https://api.openai.com/v1/chat/completions".into(),
            headers,
            body: serde_json::json!({ "model": "gpt-5" }),
            stream: false,
            response: RequestDebugResponse::from_output(&output, Some(200)),
        });

        // Metadata derivation.
        assert!(record.id.starts_with("dbg_"));
        assert_eq!(record.provider_id, "test");
        assert_eq!(record.model, "gpt-5");
        assert_eq!(record.source, "chat");
        assert_eq!(record.operation, "chat_api");
        assert_eq!(record.conversation_id.as_deref(), Some("conv_1"));
        assert_eq!(record.status, "success");
        // Header is sanitized — no full key.
        assert_eq!(record.request.headers["Authorization"], "Bearer sk-a…");
        assert!(!serde_json::to_string(&record)
            .unwrap()
            .contains("abcdef123456"));
        // Response summary carries tool calls + usage + finish reason.
        assert_eq!(record.response.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(record.response.text.as_deref(), Some("hello"));
        let calls = record
            .response
            .tool_calls
            .as_array()
            .expect("tool calls array");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["name"], "web_search");
        assert_eq!(record.response.usage.and_then(|u| u.total_tokens), Some(42));
    }
}
