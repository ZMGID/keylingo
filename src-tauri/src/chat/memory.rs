use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager};
use tauri_plugin_shell::ShellExt;

use crate::mcp::types::McpToolCallResult;

pub const L1_MAX_BYTES: usize = 5_000;

const MEMORY_DIR: &str = "chat-memory";
const L1_FILE: &str = "L1.md";
const L2_FILE: &str = "L2.md";

const DEFAULT_L1: &str = "# L1 Online Memory\n\n";
const DEFAULT_L2: &str = "# L2 Long-Term Memory\n\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryLayer {
    L1,
    L2,
}

impl MemoryLayer {
    pub fn from_str(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "l1" => Ok(Self::L1),
            "l2" => Ok(Self::L2),
            _ => Err("Memory layer must be \"l1\" or \"l2\"".to_string()),
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Self::L1 => L1_FILE,
            Self::L2 => L2_FILE,
        }
    }

    fn default_content(self) -> &'static str {
        match self {
            Self::L1 => DEFAULT_L1,
            Self::L2 => DEFAULT_L2,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::L1 => "L1",
            Self::L2 => "L2",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryLayerContent {
    pub layer: String,
    pub content: String,
    pub bytes: usize,
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMemoryState {
    pub success: bool,
    pub l1: MemoryLayerContent,
    pub l2: MemoryLayerContent,
    pub dir: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryModifyArgs {
    pub layer: String,
    pub operation: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub old_text: Option<String>,
    #[serde(default)]
    pub heading: Option<String>,
    #[serde(default)]
    pub archive_mode: Option<String>,
}

/// Resolve (and create) the `chat-memory` dir under an explicit app-data base.
/// This is the AppHandle-free core so non-Tauri callers (e.g. the kivio-code
/// terminal agent, which has no `AppHandle`) can read/write the SAME memory the
/// GUI writes, by passing their own resolved app-data dir.
pub fn memory_dir_at(base: &Path) -> Result<PathBuf, String> {
    let dir = base.join(MEMORY_DIR);
    fs::create_dir_all(&dir).map_err(|e| format!("create memory dir: {e}"))?;
    Ok(dir)
}

pub fn memory_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir unavailable: {e}"))?;
    memory_dir_at(&base)
}

fn memory_file_path_at(dir: &Path, layer: MemoryLayer) -> PathBuf {
    dir.join(layer.file_name())
}

fn ensure_memory_file_at(dir: &Path, layer: MemoryLayer) -> Result<PathBuf, String> {
    let path = memory_file_path_at(dir, layer);
    if !path.exists() {
        crate::chat::storage::atomic_write(&path, layer.default_content(), "memory")?;
    }
    Ok(path)
}

pub fn read_layer_at(dir: &Path, layer: MemoryLayer) -> Result<MemoryLayerContent, String> {
    let path = ensure_memory_file_at(dir, layer)?;
    let content =
        fs::read_to_string(&path).map_err(|e| format!("read {} memory: {e}", layer.label()))?;
    Ok(MemoryLayerContent {
        layer: layer.label().to_ascii_lowercase(),
        bytes: content.len(),
        max_bytes: (layer == MemoryLayer::L1).then_some(L1_MAX_BYTES),
        content,
    })
}

pub fn read_layer(app: &AppHandle, layer: MemoryLayer) -> Result<MemoryLayerContent, String> {
    read_layer_at(&memory_dir(app)?, layer)
}

pub fn read_all(app: &AppHandle) -> Result<ChatMemoryState, String> {
    let dir = memory_dir(app)?;
    Ok(ChatMemoryState {
        success: true,
        l1: read_layer_at(&dir, MemoryLayer::L1)?,
        l2: read_layer_at(&dir, MemoryLayer::L2)?,
        dir: dir.display().to_string(),
    })
}

pub fn save_layer_at(
    dir: &Path,
    layer: MemoryLayer,
    content: &str,
) -> Result<MemoryLayerContent, String> {
    validate_memory_content(layer, content)?;
    let path = memory_file_path_at(dir, layer);
    crate::chat::storage::atomic_write(&path, content, "memory")?;
    read_layer_at(dir, layer)
}

pub fn save_layer(
    app: &AppHandle,
    layer: MemoryLayer,
    content: &str,
) -> Result<MemoryLayerContent, String> {
    save_layer_at(&memory_dir(app)?, layer, content)
}

pub fn l1_prompt_block_at(dir: &Path) -> Result<Option<String>, String> {
    let memory = read_layer_at(dir, MemoryLayer::L1)?;
    let trimmed = memory.content.trim();
    if trimmed.is_empty() || trimmed == "# L1 Online Memory" {
        return Ok(None);
    }
    if memory.bytes > L1_MAX_BYTES {
        return Err(format!(
            "L1 memory is {} bytes, over the {L1_MAX_BYTES} byte limit; it was not injected.",
            memory.bytes
        ));
    }
    Ok(Some(format!(
        "Kivio Memory (L1 online memory; user-editable; persistent across chats):\n\n{}",
        trimmed
    )))
}

pub fn l1_prompt_block(app: &AppHandle) -> Result<Option<String>, String> {
    l1_prompt_block_at(&memory_dir(app)?)
}

#[tauri::command]
pub(crate) fn chat_memory_get(app: AppHandle) -> Result<ChatMemoryState, String> {
    read_all(&app)
}

#[tauri::command]
pub(crate) fn chat_memory_save(
    app: AppHandle,
    layer: String,
    content: String,
) -> Result<MemoryLayerContent, String> {
    save_layer(&app, MemoryLayer::from_str(&layer)?, &content)
}

#[tauri::command]
#[allow(deprecated)]
pub(crate) fn chat_memory_open_folder(app: AppHandle) -> Result<serde_json::Value, String> {
    let dir = memory_dir(&app)?;
    let dir_str = dir.display().to_string();
    app.shell()
        .open(dir_str.clone(), None)
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "success": true,
        "path": dir_str,
    }))
}

pub fn tool_read(app: &AppHandle, arguments: &Value) -> Result<McpToolCallResult, String> {
    tool_read_at(&memory_dir(app)?, arguments)
}

pub fn tool_read_at(dir: &Path, arguments: &Value) -> Result<McpToolCallResult, String> {
    let layer = arguments
        .get("layer")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "memory_read requires layer".to_string())
        .and_then(MemoryLayer::from_str)?;
    let query = arguments
        .get("query")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let max_bytes = arguments
        .get("maxBytes")
        .or_else(|| arguments.get("max_bytes"))
        .and_then(|value| value.as_u64())
        .map(|value| value.clamp(200, 50_000) as usize)
        .unwrap_or(match layer {
            MemoryLayer::L1 => L1_MAX_BYTES,
            MemoryLayer::L2 => 8_000,
        });

    let memory = read_layer_at(dir, layer)?;
    let content = if layer == MemoryLayer::L2 {
        l2_read_slice(&memory.content, query, max_bytes)
    } else {
        truncate_bytes(&memory.content, max_bytes)
    };

    Ok(McpToolCallResult {
        content: format!(
            "{} memory ({} bytes total):\n\n{}",
            layer.label(),
            memory.bytes,
            content.trim()
        ),
        is_error: false,
        raw: serde_json::to_value(&memory).unwrap_or(Value::Null),
        artifacts: Vec::new(),
        structured_content: None,
        follow_up_user_messages: Vec::new(),
    })
}

const DEFAULT_SEARCH_RESULTS: usize = 5;
const MAX_SEARCH_RESULTS: usize = 20;
const SNIPPET_MAX_BYTES: usize = 1_200;
const HEADING_HIT_WEIGHT: usize = 2;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemorySearchMatch {
    heading: String,
    snippet: String,
    score: usize,
}

pub fn tool_search(app: &AppHandle, arguments: &Value) -> Result<McpToolCallResult, String> {
    tool_search_at(&memory_dir(app)?, arguments)
}

pub fn tool_search_at(dir: &Path, arguments: &Value) -> Result<McpToolCallResult, String> {
    let query = arguments
        .get("query")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "memory_search requires a non-empty query".to_string())?;
    let max_results = arguments
        .get("maxResults")
        .or_else(|| arguments.get("max_results"))
        .and_then(|value| value.as_u64())
        .map(|value| (value as usize).clamp(1, MAX_SEARCH_RESULTS))
        .unwrap_or(DEFAULT_SEARCH_RESULTS);
    let layer = arguments
        .get("layer")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(MemoryLayer::from_str)
        .transpose()?
        .unwrap_or(MemoryLayer::L2);

    let memory = read_layer_at(dir, layer)?;
    let tokens = tokenize(query);
    let matches = search_sections(&memory.content, &tokens, max_results);

    if matches.is_empty() {
        return Ok(McpToolCallResult {
            content: format!("No {} memory entries matched: {query}", layer.label()),
            is_error: false,
            raw: Value::Null,
            artifacts: Vec::new(),
            structured_content: Some(serde_json::json!({
                "query": query,
                "layer": layer.label().to_ascii_lowercase(),
                "matches": [],
            })),
            follow_up_user_messages: Vec::new(),
        });
    }

    let mut body = String::new();
    for (idx, entry) in matches.iter().enumerate() {
        if idx > 0 {
            body.push_str("\n\n");
        }
        body.push_str(&format!(
            "{}. {}\n{}",
            idx + 1,
            entry.heading,
            entry.snippet
        ));
    }

    let structured = serde_json::json!({
        "query": query,
        "layer": layer.label().to_ascii_lowercase(),
        "matches": matches,
    });

    Ok(McpToolCallResult {
        content: format!(
            "{} memory search for \"{query}\" ({} match(es)):\n\n{}",
            layer.label(),
            matches.len(),
            body.trim()
        ),
        is_error: false,
        raw: structured.clone(),
        artifacts: Vec::new(),
        structured_content: Some(structured),
        follow_up_user_messages: Vec::new(),
    })
}

/// Lowercase token split on whitespace and common punctuation. Empty tokens
/// are dropped so scoring only counts real words.
fn tokenize(text: &str) -> Vec<String> {
    text.to_ascii_lowercase()
        .split(|c: char| c.is_whitespace() || c.is_ascii_punctuation())
        .filter(|token| !token.is_empty())
        .map(|token| token.to_string())
        .collect()
}

/// Split markdown into `#`-led sections (heading line + body) and score each
/// by query-token overlap. Heading hits are weighted; 0-score sections are
/// dropped; result is sorted by score (desc) then original order, top-N.
fn search_sections(content: &str, tokens: &[String], max_results: usize) -> Vec<MemorySearchMatch> {
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(usize, usize, MemorySearchMatch)> = Vec::new();
    for (order, (heading, body)) in split_sections(content).into_iter().enumerate() {
        let heading_lower = heading.to_ascii_lowercase();
        let body_lower = body.to_ascii_lowercase();
        let mut score = 0usize;
        for token in tokens {
            if heading_lower.contains(token.as_str()) {
                score += HEADING_HIT_WEIGHT;
            }
            if body_lower.contains(token.as_str()) {
                score += 1;
            }
        }
        if score == 0 {
            continue;
        }
        let snippet = truncate_bytes(body.trim(), SNIPPET_MAX_BYTES);
        scored.push((
            score,
            order,
            MemorySearchMatch {
                heading: if heading.is_empty() {
                    "(untitled)".to_string()
                } else {
                    heading.to_string()
                },
                snippet,
                score,
            },
        ));
    }

    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored
        .into_iter()
        .take(max_results)
        .map(|(_, _, m)| m)
        .collect()
}

/// Break content into `(heading_line, body)` pairs. A new section starts at
/// any line beginning with `#`. Content before the first heading becomes a
/// leading section with an empty heading.
fn split_sections(content: &str) -> Vec<(String, String)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current_heading = String::new();
    let mut current_body = String::new();
    let mut started = false;

    for line in content.lines() {
        if line.trim_start().starts_with('#') {
            if started || !current_body.trim().is_empty() {
                sections.push((current_heading, current_body.trim_end().to_string()));
            }
            current_heading = line.trim().to_string();
            current_body = String::new();
            started = true;
        } else {
            current_body.push_str(line);
            current_body.push('\n');
        }
    }
    if started || !current_body.trim().is_empty() {
        sections.push((current_heading, current_body.trim_end().to_string()));
    }
    sections
}

pub fn tool_modify(app: &AppHandle, arguments: &Value) -> Result<McpToolCallResult, String> {
    tool_modify_at(&memory_dir(app)?, arguments)
}

pub fn tool_modify_at(dir: &Path, arguments: &Value) -> Result<McpToolCallResult, String> {
    let args: MemoryModifyArgs = serde_json::from_value(arguments.clone())
        .map_err(|e| format!("Invalid memory_modify arguments: {e}"))?;
    let layer = MemoryLayer::from_str(&args.layer)?;
    let operation = args.operation.trim().to_ascii_lowercase();
    let result = match operation.as_str() {
        "append" => modify_append(dir, layer, args.content.as_deref(), args.heading.as_deref())?,
        "replace" => modify_replace(
            dir,
            layer,
            args.old_text.as_deref(),
            args.content.as_deref(),
        )?,
        "remove" => modify_remove(dir, layer, args.old_text.as_deref())?,
        "archive" => {
            if layer != MemoryLayer::L1 {
                return Err("memory_modify archive must use layer=\"l1\" because archive moves or copies L1 content into L2".to_string());
            }
            modify_archive(dir, &args)?
        }
        _ => {
            return Err(
                "memory_modify operation must be append, replace, remove, or archive".to_string(),
            )
        }
    };

    Ok(McpToolCallResult {
        content: format!(
            "{} memory updated by {operation}. Current size: {} bytes.",
            layer.label(),
            result.bytes
        ),
        is_error: false,
        raw: serde_json::to_value(&result).unwrap_or(Value::Null),
        artifacts: Vec::new(),
        structured_content: None,
        follow_up_user_messages: Vec::new(),
    })
}

fn modify_append(
    dir: &Path,
    layer: MemoryLayer,
    content: Option<&str>,
    heading: Option<&str>,
) -> Result<MemoryLayerContent, String> {
    let addition = require_content(content)?;
    let current = read_layer_at(dir, layer)?.content;
    let next = if layer == MemoryLayer::L2 {
        append_to_heading_or_end(&current, addition, heading)
    } else {
        append_block(&current, addition)
    };
    save_layer_at(dir, layer, &next)
}

fn modify_replace(
    dir: &Path,
    layer: MemoryLayer,
    old_text: Option<&str>,
    content: Option<&str>,
) -> Result<MemoryLayerContent, String> {
    let old_text = require_old_text(old_text)?;
    let replacement = require_content(content)?;
    let current = read_layer_at(dir, layer)?.content;
    ensure_unique_match(&current, old_text)?;
    save_layer_at(dir, layer, &current.replacen(old_text, replacement, 1))
}

fn modify_remove(
    dir: &Path,
    layer: MemoryLayer,
    old_text: Option<&str>,
) -> Result<MemoryLayerContent, String> {
    let old_text = require_old_text(old_text)?;
    let current = read_layer_at(dir, layer)?.content;
    ensure_unique_match(&current, old_text)?;
    save_layer_at(dir, layer, &current.replacen(old_text, "", 1))
}

fn modify_archive(dir: &Path, args: &MemoryModifyArgs) -> Result<MemoryLayerContent, String> {
    let old_text = require_old_text(args.old_text.as_deref())?;
    let l1 = read_layer_at(dir, MemoryLayer::L1)?.content;
    ensure_unique_match(&l1, old_text)?;
    let l2 = read_layer_at(dir, MemoryLayer::L2)?.content;
    let archived = args.content.as_deref().unwrap_or(old_text);
    validate_memory_content(MemoryLayer::L2, archived)?;
    let next_l2 = append_to_heading_or_end(&l2, archived, args.heading.as_deref());
    save_layer_at(dir, MemoryLayer::L2, &next_l2)?;

    let mode = args
        .archive_mode
        .as_deref()
        .unwrap_or("move")
        .trim()
        .to_ascii_lowercase();
    if mode == "copy" {
        read_layer_at(dir, MemoryLayer::L1)
    } else {
        save_layer_at(dir, MemoryLayer::L1, &l1.replacen(old_text, "", 1))
    }
}

fn require_content(content: Option<&str>) -> Result<&str, String> {
    let content = content
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "memory_modify content cannot be empty".to_string())?;
    validate_secret_free(content)?;
    Ok(content)
}

fn require_old_text(old_text: Option<&str>) -> Result<&str, String> {
    old_text
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "memory_modify oldText cannot be empty".to_string())
}

fn ensure_unique_match(content: &str, needle: &str) -> Result<(), String> {
    let count = content.matches(needle).count();
    match count {
        1 => Ok(()),
        0 => Err("oldText was not found in memory; provide an exact unique snippet".to_string()),
        _ => Err(format!(
            "oldText matched {count} places in memory; provide a more specific snippet"
        )),
    }
}

fn validate_memory_content(layer: MemoryLayer, content: &str) -> Result<(), String> {
    validate_secret_free(content)?;
    if layer == MemoryLayer::L1 && content.len() > L1_MAX_BYTES {
        return Err(format!(
            "L1 memory is {} bytes, over the {L1_MAX_BYTES} byte limit. Move details to L2 and keep only short active facts in L1.",
            content.len()
        ));
    }
    Ok(())
}

fn validate_secret_free(content: &str) -> Result<(), String> {
    let lower = content.to_ascii_lowercase();
    // 只看字段名/前缀的裸 contains 会误伤正常笔记："sk-" 命中 task-/desk-/risk-，
    // "bearer"/"password" 是普通英文词，「不要把 apikey 写进 settings」是合法偏好。
    // 词边界 + 后面紧跟一段 key 长度的 token 才像真凭据。报错点名命中的模式——
    // 不点名模型只会盲改重试（实测连拦三次死循环）。
    let key_like = [
        ("sk-", 16),
        ("ghp_", 20),
        ("github_pat_", 20),
        ("bearer", 16),
        ("api_key", 16),
        ("apikey", 16),
        ("secret_key", 16),
        ("password", 6),
        ("passwd", 6),
    ];
    for (marker, min_len) in key_like {
        if contains_credential(&lower, marker, min_len) {
            return Err(format!(
                "Memory content looks like it contains a credential (`{marker}` followed by a token); save a redacted summary instead."
            ));
        }
    }
    if lower.contains("-----begin") {
        return Err(
            "Memory content looks like it contains a private key block; save a redacted summary instead."
                .to_string(),
        );
    }
    // 「system prompt」不在列：讨论系统提示词的笔记是合法内容，注入标志是祈使句本身。
    let injection_like = [
        "ignore previous instructions",
        "ignore all previous instructions",
        "disregard previous instructions",
    ];
    for needle in injection_like {
        if lower.contains(needle) {
            return Err(format!(
                "Memory content looks like prompt-injection instructions (matched `{needle}`); save neutral facts instead."
            ));
        }
    }
    Ok(())
}

/// `marker` 出现在词边界上、且其后（跳过至多 3 个 `: = " '` 空格类分隔符）
/// 紧跟一段 ≥ `min_len` 的 token（字母数字 `_` `-`）才判为凭据。
fn contains_credential(lower: &str, marker: &str, min_len: usize) -> bool {
    let bytes = lower.as_bytes();
    let mut from = 0;
    while let Some(pos) = lower[from..].find(marker) {
        let at = from + pos;
        let boundary = at == 0
            || !(bytes[at - 1].is_ascii_alphanumeric()
                || bytes[at - 1] == b'_'
                || bytes[at - 1] == b'-');
        let mut i = at + marker.len();
        let mut skipped = 0;
        while i < bytes.len() && skipped < 3 && matches!(bytes[i], b' ' | b':' | b'=' | b'"' | b'\'')
        {
            i += 1;
            skipped += 1;
        }
        let run = bytes[i..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_' || **b == b'-')
            .count();
        if boundary && run >= min_len {
            return true;
        }
        from = at + marker.len();
    }
    false
}

fn append_block(current: &str, addition: &str) -> String {
    let mut next = current.trim_end().to_string();
    if !next.is_empty() {
        next.push_str("\n\n");
    }
    next.push_str(addition.trim());
    next.push('\n');
    next
}

fn append_to_heading_or_end(current: &str, addition: &str, heading: Option<&str>) -> String {
    let Some(heading) = heading.map(str::trim).filter(|value| !value.is_empty()) else {
        return append_block(current, addition);
    };
    let Some(start) = current.find(heading) else {
        let mut next = current.trim_end().to_string();
        if !next.is_empty() {
            next.push_str("\n\n");
        }
        next.push_str(heading);
        next.push_str("\n\n");
        next.push_str(addition.trim());
        next.push('\n');
        return next;
    };

    let after_heading = start + heading.len();
    let rest = &current[after_heading..];
    let next_heading_offset = rest
        .find("\n#")
        .map(|offset| after_heading + offset)
        .unwrap_or(current.len());
    let mut next = String::new();
    next.push_str(current[..next_heading_offset].trim_end());
    next.push_str("\n\n");
    next.push_str(addition.trim());
    next.push('\n');
    next.push_str(&current[next_heading_offset..]);
    if !next.ends_with('\n') {
        next.push('\n');
    }
    next
}

fn l2_read_slice(content: &str, query: Option<&str>, max_bytes: usize) -> String {
    if let Some(query) = query {
        let lower_content = content.to_ascii_lowercase();
        let lower_query = query.to_ascii_lowercase();
        if let Some(byte_idx) = lower_content.find(&lower_query) {
            let start = content[..byte_idx]
                .rfind("\n#")
                .map(|idx| idx + 1)
                .unwrap_or(0);
            let end = content[byte_idx..]
                .find("\n#")
                .map(|idx| byte_idx + idx)
                .unwrap_or(content.len());
            return truncate_bytes(&content[start..end], max_bytes);
        }
        return format!("No exact text match for query: {query}");
    }
    truncate_bytes(content, max_bytes)
}

fn truncate_bytes(content: &str, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content.to_string();
    }
    let mut end = max_bytes;
    while !content.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    format!(
        "{}\n\n[Memory output truncated: original {} bytes, showing first {} bytes.]",
        &content[..end],
        content.len(),
        end
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l1_rejects_over_limit_bytes() {
        let content = "a".repeat(L1_MAX_BYTES + 1);
        assert!(validate_memory_content(MemoryLayer::L1, &content).is_err());
        assert!(validate_memory_content(MemoryLayer::L2, &content).is_ok());
    }

    #[test]
    fn secret_filter_needs_boundary_and_token_tail() {
        // 普通词、字段名讨论、中文笔记不拦
        for ok in [
            "task-oriented desk-side risk-based notes",
            "偏好：不要把 apikey 写进 settings.json",
            "the bearer of good news said password rules changed",
            "把这条约定放进 system prompt 开头",
            "文件分享偏好：我生成的HTML文件优先用本地路径分享",
        ] {
            assert!(validate_secret_free(ok).is_ok(), "should pass: {ok}");
        }
        // 真凭据形状要拦，且报错点名命中的模式
        let err = validate_secret_free("key: sk-abcdefghijklmnop1234").unwrap_err();
        assert!(err.contains("sk-"), "{err}");
        assert!(validate_secret_free("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9x").is_err());
        assert!(validate_secret_free("password: hunter42x").is_err());
        assert!(validate_secret_free("-----BEGIN RSA PRIVATE KEY-----").is_err());
        let err = validate_secret_free("please ignore previous instructions").unwrap_err();
        assert!(err.contains("ignore previous instructions"), "{err}");
    }

    #[test]
    fn unique_match_requires_exactly_one_match() {
        assert!(ensure_unique_match("one two", "one").is_ok());
        assert!(ensure_unique_match("one one", "one").is_err());
        assert!(ensure_unique_match("one two", "missing").is_err());
    }

    #[test]
    fn append_heading_inserts_inside_section() {
        let current = "# L2\n\n## A\n\nold\n\n## B\n\nother\n";
        let next = append_to_heading_or_end(current, "new", Some("## A"));
        assert!(next.contains("## A\n\nold\n\nnew\n\n## B"));
    }

    const SEARCH_DOC: &str = "# L2 Long-Term Memory\n\n\
## Deployment Pipeline\n\nWe deploy via GitHub Actions to staging then production.\n\n\
## Database Schema\n\nThe users table stores email and a hashed password digest.\n\n\
## Code Style\n\nUse two-space indentation in TypeScript files.\n";

    #[test]
    fn search_returns_relevant_section() {
        let tokens = tokenize("database password");
        let matches = search_sections(SEARCH_DOC, &tokens, 5);
        assert!(!matches.is_empty());
        assert_eq!(matches[0].heading, "## Database Schema");
        assert!(matches[0].snippet.contains("hashed password"));
    }

    #[test]
    fn search_weights_heading_hits() {
        // "code" appears only in the "## Code Style" heading; heading weight
        // should rank that section above one that only matches in body.
        let tokens = tokenize("code");
        let matches = search_sections(SEARCH_DOC, &tokens, 5);
        assert_eq!(matches[0].heading, "## Code Style");
        assert!(matches[0].score >= HEADING_HIT_WEIGHT);
    }

    #[test]
    fn search_respects_top_n_limit() {
        // Every section mentions a token shared with the query.
        let doc = "# Title\n\n## One\n\nshared alpha\n\n## Two\n\nshared beta\n\n## Three\n\nshared gamma\n";
        let tokens = tokenize("shared");
        let matches = search_sections(doc, &tokens, 2);
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn search_no_match_yields_empty() {
        let tokens = tokenize("kubernetes helm chart");
        let matches = search_sections(SEARCH_DOC, &tokens, 5);
        assert!(matches.is_empty());
    }

    #[test]
    fn search_missing_query_errors() {
        // tokenize of empty/whitespace yields no tokens -> empty result, but
        // tool_search rejects before that; emulate the arg check here.
        let args = serde_json::json!({ "maxResults": 3 });
        let query = args
            .get("query")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        assert!(query.is_none());
        assert!(tokenize("   ").is_empty());
    }
}
