use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Condvar, Mutex, OnceLock},
    time::UNIX_EPOCH,
};

use serde::Serialize;
use serde_json::{json, Value};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use super::{
    resolve_tool_read_path, resolve_tool_write_path, workspace_display_path, NativeToolWorkspace,
    MAX_READ_FILE_BYTES, TOOL_OUTPUT_MAX_BYTES, TOOL_OUTPUT_MAX_LINES,
};

const MAX_LIST_ENTRIES: usize = 500;
const MAX_GLOB_RESULTS: usize = 500;
const MAX_SEARCH_FILES: usize = 5_000;
const MAX_SEARCH_MATCHES: usize = 1_000;
const MAX_SEARCH_FILE_BYTES: u64 = 1024 * 1024;
/// Cap each emitted grep line at this many chars; longer lines get a marker.
const MAX_GREP_LINE_CHARS: usize = 500;
const UTF8_BOM: &str = "\u{feff}";
const DEFAULT_IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".turbo",
    ".vite",
];

static FILE_MUTATION_LOCKS: OnceLock<FileMutationLocks> = OnceLock::new();

#[derive(Debug, Clone, Serialize)]
pub struct FileMutationResult {
    pub ok: bool,
    pub operation: String,
    pub target_touched: bool,
    pub resolved_path: Option<String>,
    pub files: Vec<FileMutationFile>,
    pub bytes_written: u64,
    pub additions: usize,
    pub removals: usize,
    pub diff: String,
    pub warnings: Vec<String>,
    pub diagnostics: Vec<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadFileResult {
    pub path: String,
    pub resolved_path: String,
    pub content: String,
    pub total_lines: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub truncated: bool,
    pub file_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileMutationFile {
    pub path: String,
    pub operation: String,
    pub bytes_written: u64,
    pub additions: usize,
    pub removals: usize,
    pub diff: String,
}

impl FileMutationResult {
    pub fn summary(&self) -> String {
        let stats = format!("+{} -{}", self.additions, self.removals);
        if let Some(file) = self.files.first().filter(|_| self.files.len() == 1) {
            return format!(
                "{} {} ({stats})",
                mutation_operation_label(&file.operation),
                file.path
            );
        }
        format!(
            "{} {} file(s) ({stats})",
            mutation_operation_label(&self.operation),
            self.files.len()
        )
    }
}

pub fn read_file(
    workspace: &NativeToolWorkspace,
    arguments: &Value,
) -> Result<ReadFileResult, String> {
    let path = arguments
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "read_file requires path".to_string())?;
    let full = resolve_tool_read_path(workspace, path)?;
    if !full.is_file() {
        return Err(format!("不是可读取的文件: {path}"));
    }
    let metadata = fs::metadata(&full).map_err(|err| format!("Read metadata failed: {err}"))?;

    let offset = arguments
        .get("offset")
        .and_then(|v| v.as_u64())
        .unwrap_or(1)
        .max(1) as usize;
    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    // 超过内存阈值 → 流式行窗口读，不再报错要求调用方自己传 offset/limit。
    // 输出上限两条路径一致（TOOL_OUTPUT_MAX_*），所以「整个文件多大」不再是失败条件，
    // 只决定用哪个读法（对齐 pi：read 永远给得出东西，给多少由 truncate 决定）。
    if metadata.len() > MAX_READ_FILE_BYTES {
        return read_file_window_streaming(workspace, &full, &metadata, offset, limit);
    }

    let content = fs::read_to_string(&full).map_err(|err| format!("Read file failed: {err}"))?;

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();
    let start = offset.saturating_sub(1).min(lines.len());
    let requested_end = limit
        .map(|lim| (start + lim).min(lines.len()))
        .unwrap_or(lines.len());
    // 照抄 pi `read.ts`：模型给的 limit 先切，**再无条件过一遍 truncate_head**。
    // limit 是模型的意图，上限是框架的地板——模型写多大都翻不过去。
    let (kept, capped) = truncate_head(&lines[start..requested_end]);
    let end = start + kept;
    let truncated = end < total_lines;
    let returned_content = if offset == 1 && limit.is_none() && capped.is_none() {
        content
    } else {
        lines[start..end].join("\n")
    };
    let display_path = workspace_display_path(workspace, &full);
    Ok(ReadFileResult {
        resolved_path: full.display().to_string(),
        content: returned_content,
        total_lines,
        start_line: if total_lines == 0 { 0 } else { start + 1 },
        end_line: end,
        truncated,
        file_size: metadata.len(),
        next_offset: truncated.then(|| next_offset_after(start, end, kept, capped)),
        warnings: cap_warnings(capped, &display_path, start, end, kept),
        path: display_path,
    })
}

/// 下一次续读的起始行。
///
/// 常规是 `end + 1`。唯一的例外是**怪物长行**（首行自身就超字节预算 → `kept == 0`
/// 且触顶原因是 `Bytes`）：那一行永远读不出来，`end + 1` 会把 next_offset 停在它
/// 自己身上，模型照做就是原地打转，所以跳到下一行，整行内容交给 warning 指的
/// `run_command`。
///
/// 判据必须同时看 `capped`，**不能只看 `kept == 0`**：`limit=0` 同样得到 `kept == 0`
/// 但 `capped` 是 `None`，那时必须老老实实指回 `end + 1`（= 原地），否则会静默跳过
/// 一行、且因为没触顶连 warning 都不给。
fn next_offset_after(start: usize, end: usize, kept: usize, capped: Option<TruncatedBy>) -> usize {
    if kept == 0 && capped == Some(TruncatedBy::Bytes) {
        start + 2
    } else {
        end + 1
    }
}

/// 触顶原因（照抄 pi `TruncationResult.truncatedBy`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TruncatedBy {
    Lines,
    Bytes,
}

/// 从头保留完整行，直到行数到 [`TOOL_OUTPUT_MAX_LINES`] 或字节到
/// [`TOOL_OUTPUT_MAX_BYTES`]（照抄 pi `truncateHead`：谁先到算谁，绝不返回半行）。
///
/// 返回 `(保留行数, 触顶原因)`；原因为 `None` = 整段都放得下。首行自身就超字节预算
/// → `(0, Some(Bytes))`，由调用方给出 `run_command` 兜底提示（pi 那边指向 bash）。
fn truncate_head(lines: &[&str]) -> (usize, Option<TruncatedBy>) {
    let total_bytes: usize =
        lines.iter().map(|line| line.len()).sum::<usize>() + lines.len().saturating_sub(1);
    if lines.len() <= TOOL_OUTPUT_MAX_LINES && total_bytes <= TOOL_OUTPUT_MAX_BYTES {
        return (lines.len(), None);
    }
    let mut used = 0usize;
    for (idx, line) in lines.iter().enumerate() {
        if idx >= TOOL_OUTPUT_MAX_LINES {
            return (idx, Some(TruncatedBy::Lines));
        }
        let cost = line.len() + usize::from(idx > 0);
        if used + cost > TOOL_OUTPUT_MAX_BYTES {
            return (idx, Some(TruncatedBy::Bytes));
        }
        used += cost;
    }
    (lines.len(), None)
}

/// 把触顶原因翻成模型能照做的一句话（对齐 pi 的 `[Showing lines … Use offset=N to
/// continue.]`）。没触顶就没有 warning——文件本来就读完了，不该吓唬模型。
fn cap_warnings(
    capped: Option<TruncatedBy>,
    path: &str,
    start: usize,
    end: usize,
    kept: usize,
) -> Vec<String> {
    let kb = TOOL_OUTPUT_MAX_BYTES / 1024;
    match capped {
        None => Vec::new(),
        // 怪物长行的逃生口。`head -c` 取的是 run_command 的**内联**上限而不是 50KB：
        // 超过那个数 run_command 会把输出转存成日志文件，模型还得回头 read 它，那一行
        // 依旧超 50KB —— 兜了一圈回到原点。命令给的是示例（Windows 上未必有 sed），
        // 路径是真路径，模型可以直接照抄。
        Some(TruncatedBy::Bytes) if kept == 0 => vec![format!(
            "第 {line} 行单行就超过 {kb}KB 的单次读取上限。用 run_command 单独取它，\
             例如 `sed -n '{line}p' {path} | head -c {inline}`；或用 offset={next} 跳过这一行继续。",
            line = start + 1,
            next = start + 2,
            inline = super::shell::MAX_INLINE_COMMAND_OUTPUT_BYTES,
        )],
        Some(TruncatedBy::Bytes) => vec![format!(
            "单次读取上限 {kb}KB 已触顶（不是文件结尾），用 offset={} 继续。",
            end + 1
        )],
        Some(TruncatedBy::Lines) => vec![format!(
            "单次读取上限 {TOOL_OUTPUT_MAX_LINES} 行已触顶（不是文件结尾），用 offset={} 继续。",
            end + 1
        )],
    }
}

/// Streams an oversized file line by line, keeping only the requested window in
/// memory. The window obeys the same [`TOOL_OUTPUT_MAX_LINES`] /
/// [`TOOL_OUTPUT_MAX_BYTES`] budget as the in-memory path.
fn read_file_window_streaming(
    workspace: &NativeToolWorkspace,
    full: &Path,
    metadata: &fs::Metadata,
    offset: usize,
    limit: Option<usize>,
) -> Result<ReadFileResult, String> {
    use std::io::BufRead;

    let limit = limit
        .unwrap_or(TOOL_OUTPUT_MAX_LINES)
        .clamp(1, TOOL_OUTPUT_MAX_LINES);
    let start = offset.saturating_sub(1);
    let end = start.saturating_add(limit);

    let file = fs::File::open(full).map_err(|err| format!("Read file failed: {err}"))?;
    let reader = std::io::BufReader::new(file);
    let mut window: Vec<String> = Vec::new();
    let mut window_bytes = 0usize;
    let mut window_byte_capped = false;
    let mut total_lines = 0usize;
    for line in reader.lines() {
        let line = line.map_err(|err| format!("Read file failed: {err}"))?;
        if total_lines >= start && total_lines < end && !window_byte_capped {
            // `+1` 是 join("\n") 会补回来的换行符——不算就会比预算多发 (行数-1) 字节，
            // 和非流式路径的 truncate_head 口径对不上。
            let cost = line.len() + usize::from(!window.is_empty());
            if window_bytes + cost > TOOL_OUTPUT_MAX_BYTES {
                window_byte_capped = true;
            } else {
                window_bytes += cost;
                window.push(line);
            }
        }
        total_lines += 1;
    }

    let start = start.min(total_lines);
    let kept = window.len();
    let end = (start + kept).min(total_lines);
    let truncated = end < total_lines;
    let capped = if window_byte_capped {
        Some(TruncatedBy::Bytes)
    } else if truncated && kept >= limit && limit == TOOL_OUTPUT_MAX_LINES {
        Some(TruncatedBy::Lines)
    } else {
        None
    };
    let display_path = workspace_display_path(workspace, full);
    Ok(ReadFileResult {
        resolved_path: full.display().to_string(),
        content: window.join("\n"),
        total_lines,
        start_line: if total_lines == 0 { 0 } else { start + 1 },
        end_line: end,
        truncated,
        file_size: metadata.len(),
        next_offset: truncated.then(|| next_offset_after(start, end, kept, capped)),
        warnings: cap_warnings(capped, &display_path, start, end, kept),
        path: display_path,
    })
}

pub fn write_file(
    workspace: &NativeToolWorkspace,
    arguments: &Value,
) -> Result<FileMutationResult, String> {
    let path = arguments
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "write_file requires path".to_string())?;
    let content = arguments
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "write_file requires content".to_string())?;
    let full = resolve_tool_write_path(workspace, path)?;
    let _guard = acquire_file_mutation_locks([full.clone()])?;
    let existed = full.is_file();
    // The existing content is only needed for the diff; degrade gracefully on non-UTF-8.
    let before = if existed {
        fs::read_to_string(&full).ok()
    } else {
        None
    };
    atomic_write_text(&full, content, before.as_deref())
        .map_err(|err| format!("Write file failed: {err}"))?;
    let operation = if existed { "overwrite" } else { "create" };
    let diff_omitted = existed && before.is_none();
    let file = if diff_omitted {
        FileMutationFile {
            path: workspace_display_path(workspace, &full),
            operation: operation.to_string(),
            bytes_written: content.len() as u64,
            additions: 0,
            removals: 0,
            diff: String::new(),
        }
    } else {
        planned_file_result(workspace, full, operation, before.as_deref(), Some(content))?
    };
    let mut result = file_mutation_result(operation, vec![file]);
    if diff_omitted {
        result
            .warnings
            .push("Existing file content is not valid UTF-8; diff omitted.".to_string());
    }
    Ok(result)
}

pub fn edit_file(
    workspace: &NativeToolWorkspace,
    arguments: &Value,
) -> Result<FileMutationResult, String> {
    let path = arguments
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "edit requires path".to_string())?;
    let edits_value = arguments
        .get("edits")
        .and_then(|v| v.as_array())
        .filter(|edits| !edits.is_empty())
        .ok_or_else(|| {
            "edit requires `edits`: a non-empty array of {old_string, new_string}".to_string()
        })?;
    // Parse the edits up front so a malformed entry fails before we touch disk.
    let parsed: Vec<(String, String)> = edits_value
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let old = e
                .get("old_string")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("edits[{i}] requires old_string"))?;
            let new = e
                .get("new_string")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("edits[{i}] requires new_string"))?;
            Ok((old.to_string(), new.to_string()))
        })
        .collect::<Result<_, String>>()?;

    let full = resolve_tool_write_path(workspace, path)?;
    let _guard = acquire_file_mutation_locks([full.clone()])?;
    if !full.is_file() {
        return Err(format!("不是可编辑的文件: {path}"));
    }

    let content = fs::read_to_string(&full).map_err(|err| format!("Read file failed: {err}"))?;
    // 行尾归一后再匹配：模型给的 old_string 通常是 LF，而文件可能是 CRLF（Windows 高频），
    // 直接字面 `contains` 会 0 命中。统一归一到 LF 做匹配/替换；写回时 atomic_write_text
    // 依据原文件把 LF 还原成 CRLF（并保留 BOM），磁盘行尾风格不变。
    let normalized_content = normalize_line_endings(&content, "\n");
    // Apply edits in order. Each old_string must occur exactly once in the
    // current working text (no replace_all): a model that wants to change every
    // occurrence lists them as separate, context-extended edits — same safety as
    // the old single-edit uniqueness check, now per edit.
    let mut working = normalized_content.clone();
    let mut warnings = Vec::new();
    let mut applied = 0usize;
    for (i, (old, new)) in parsed.iter().enumerate() {
        let normalized_old = normalize_line_endings(old, "\n");
        let normalized_new = normalize_line_endings(new, "\n");
        if normalized_old == normalized_new {
            warnings.push(format!(
                "edits[{i}]: old_string and new_string are identical; skipped."
            ));
            continue;
        }
        if normalized_old.is_empty() {
            return Err(format!("edits[{i}]: old_string is empty."));
        }
        let count = working.matches(&normalized_old).count();
        if count > 1 {
            return Err(format!(
                "edits[{i}]: old_string appears {count} times; extend it with surrounding context \
                 so it matches exactly one location (replace_all is no longer supported — list each \
                 occurrence as its own edit)."
            ));
        }
        if count == 1 {
            working = working.replacen(&normalized_old, &normalized_new, 1);
            applied += 1;
            continue;
        }

        // Exact match found nothing. Fall back to fuzzy matching: cosmetic unicode
        // differences (NFKC, smart quotes/dashes, whitespace runs) are normalized on
        // BOTH sides, the match is located in normalized space, then mapped back to an
        // ORIGINAL byte range via an alignment table so the replacement preserves the
        // file's real bytes everywhere except the matched span.
        match fuzzy_find_unique(&working, &normalized_old) {
            Ok(range) => {
                working.replace_range(range, &normalized_new);
                applied += 1;
            }
            Err(FuzzyMatchError::NotFound) => {
                return Err(format!(
                    "edits[{i}]: old_string not found in file. Re-read the file and copy an exact, \
                     contiguous snippet including its leading whitespace/indentation. Line endings are \
                     normalized automatically, so a CRLF vs LF mismatch is not the cause."
                ));
            }
            Err(FuzzyMatchError::Ambiguous(n)) => {
                return Err(format!(
                    "edits[{i}]: old_string appears {n} times; extend it with surrounding context \
                     so it matches exactly one location (replace_all is no longer supported — list each \
                     occurrence as its own edit)."
                ));
            }
        }
    }

    if applied == 0 {
        let display_path = workspace_display_path(workspace, &full);
        if warnings.is_empty() {
            warnings.push("No edits changed the file.".to_string());
        }
        return Ok(FileMutationResult {
            ok: true,
            operation: "edit".to_string(),
            target_touched: false,
            resolved_path: Some(display_path.clone()),
            files: vec![FileMutationFile {
                path: display_path,
                operation: "noop".to_string(),
                bytes_written: content.len() as u64,
                additions: 0,
                removals: 0,
                diff: String::new(),
            }],
            bytes_written: content.len() as u64,
            additions: 0,
            removals: 0,
            diff: String::new(),
            warnings,
            diagnostics: Vec::new(),
        });
    }

    atomic_write_text(&full, &working, Some(&content))
        .map_err(|err| format!("Write file failed: {err}"))?;
    let mut result = file_mutation_result(
        "edit",
        vec![planned_file_result(
            workspace,
            full,
            "edit",
            Some(&normalized_content),
            Some(&working),
        )?],
    );
    result.warnings.extend(warnings);
    Ok(result)
}

struct FileMutationLocks {
    active: Mutex<HashSet<PathBuf>>,
    ready: Condvar,
}

#[derive(Debug, PartialEq, Eq)]
enum FuzzyMatchError {
    NotFound,
    Ambiguous(usize),
}

/// One normalized character produced from the original text, plus the original
/// byte range it came from. Whitespace runs collapse to a single space whose
/// range spans the whole run, so a normalized-space match maps cleanly back to
/// the original byte offsets.
struct NormChar {
    ch: char,
    start: usize,
    end: usize,
}

/// Normalize a single original char for fuzzy matching: smart quotes/dashes →
/// ASCII, then NFKC. Returns the resulting (usually one) chars. Whitespace is
/// handled by the caller (run collapsing), so this never emits a space here for
/// non-space input.
fn fuzzy_normalize_char(ch: char) -> Vec<char> {
    let mapped = match ch {
        // Smart single quotes / prime → '
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => '\'',
        // Smart double quotes / double prime → "
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' => '"',
        // Dashes (hyphen/non-breaking-hyphen/figure/en/em/horizontal-bar/minus) → -
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
        | '\u{2212}' => '-',
        other => other,
    };
    mapped.to_string().nfkc().collect()
}

/// Treat any unicode whitespace (incl. NBSP, narrow NBSP, ideographic space) as
/// collapsible whitespace.
fn is_fuzzy_whitespace(ch: char) -> bool {
    ch.is_whitespace()
}

/// Build the normalized form of `text` alongside an alignment table back to the
/// ORIGINAL byte offsets. Whitespace runs collapse to one space spanning the run.
fn build_normalized(text: &str) -> (String, Vec<NormChar>) {
    let mut norm = String::new();
    let mut table: Vec<NormChar> = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((byte_idx, ch)) = chars.next() {
        if is_fuzzy_whitespace(ch) {
            let start = byte_idx;
            let mut end = byte_idx + ch.len_utf8();
            // Consume the rest of the whitespace run.
            while let Some(&(next_idx, next_ch)) = chars.peek() {
                if is_fuzzy_whitespace(next_ch) {
                    end = next_idx + next_ch.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            norm.push(' ');
            table.push(NormChar {
                ch: ' ',
                start,
                end,
            });
            continue;
        }
        let end = byte_idx + ch.len_utf8();
        for mapped in fuzzy_normalize_char(ch) {
            norm.push(mapped);
            table.push(NormChar {
                ch: mapped,
                start: byte_idx,
                end,
            });
        }
    }
    (norm, table)
}

/// Find a unique fuzzy match of `needle` inside `haystack` and return the
/// ORIGINAL byte range in `haystack` it corresponds to. Leading/trailing
/// whitespace of the needle (in normalized space) is trimmed so cosmetic edge
/// whitespace doesn't block the match; the mapped original range still covers
/// exactly the matched original bytes.
fn fuzzy_find_unique(
    haystack: &str,
    needle: &str,
) -> Result<std::ops::Range<usize>, FuzzyMatchError> {
    let (norm_hay, hay_table) = build_normalized(haystack);
    let (norm_needle, _needle_table) = build_normalized(needle);
    let needle_trimmed = norm_needle.trim();
    if needle_trimmed.is_empty() {
        return Err(FuzzyMatchError::NotFound);
    }

    // Collect all non-overlapping occurrences in normalized space.
    let mut occurrences: Vec<(usize, usize)> = Vec::new();
    let mut search_from = 0usize;
    while let Some(rel) = norm_hay[search_from..].find(needle_trimmed) {
        let start = search_from + rel;
        let end = start + needle_trimmed.len();
        occurrences.push((start, end));
        // Advance past this match so overlapping matches aren't double-counted.
        search_from = end;
    }

    match occurrences.len() {
        0 => Err(FuzzyMatchError::NotFound),
        1 => {
            let (norm_start, norm_end) = occurrences[0];
            Ok(map_norm_range_to_original(&hay_table, norm_start, norm_end))
        }
        n => Err(FuzzyMatchError::Ambiguous(n)),
    }
}

/// Map a byte range in the normalized string to the spanning original byte range
/// using the per-normalized-char alignment table.
fn map_norm_range_to_original(
    table: &[NormChar],
    norm_start: usize,
    norm_end: usize,
) -> std::ops::Range<usize> {
    let mut byte = 0usize;
    let mut first: Option<usize> = None;
    let mut last: Option<usize> = None;
    for (idx, nc) in table.iter().enumerate() {
        let ch_len = nc.ch.len_utf8();
        let ch_start = byte;
        let ch_end = byte + ch_len;
        // Overlap test against [norm_start, norm_end).
        if ch_start < norm_end && ch_end > norm_start {
            if first.is_none() {
                first = Some(idx);
            }
            last = Some(idx);
        }
        byte = ch_end;
        if byte >= norm_end {
            break;
        }
    }
    match (first, last) {
        (Some(f), Some(l)) => table[f].start..table[l].end,
        // Should not happen for a non-empty match; fall back to a no-op range.
        _ => 0..0,
    }
}

struct FileMutationLockGuard {
    paths: Vec<PathBuf>,
}

impl Drop for FileMutationLockGuard {
    fn drop(&mut self) {
        let Some(locks) = FILE_MUTATION_LOCKS.get() else {
            return;
        };
        let Ok(mut active) = locks.active.lock() else {
            return;
        };
        for path in &self.paths {
            active.remove(path);
        }
        locks.ready.notify_all();
    }
}

fn acquire_file_mutation_locks<I>(paths: I) -> Result<FileMutationLockGuard, String>
where
    I: IntoIterator<Item = PathBuf>,
{
    let locks = FILE_MUTATION_LOCKS.get_or_init(|| FileMutationLocks {
        active: Mutex::new(HashSet::new()),
        ready: Condvar::new(),
    });
    let mut paths = paths.into_iter().collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    let mut active = locks
        .active
        .lock()
        .map_err(|_| "File mutation lock is unavailable".to_string())?;
    while paths.iter().any(|path| active.contains(path)) {
        active = locks
            .ready
            .wait(active)
            .map_err(|_| "File mutation lock is unavailable".to_string())?;
    }
    for path in &paths {
        active.insert(path.clone());
    }
    Ok(FileMutationLockGuard { paths })
}

fn file_mutation_result(operation: &str, files: Vec<FileMutationFile>) -> FileMutationResult {
    let resolved_path = files
        .first()
        .filter(|_| files.len() == 1)
        .map(|file| file.path.clone());
    let bytes_written = files.iter().map(|file| file.bytes_written).sum();
    let additions = files.iter().map(|file| file.additions).sum();
    let removals = files.iter().map(|file| file.removals).sum();
    let diff = files
        .iter()
        .map(|file| file.diff.as_str())
        .filter(|diff| !diff.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    FileMutationResult {
        ok: true,
        operation: operation.to_string(),
        target_touched: true,
        resolved_path,
        files,
        bytes_written,
        additions,
        removals,
        diff,
        warnings: Vec::new(),
        diagnostics: Vec::new(),
    }
}

fn atomic_write_text(
    target: &Path,
    content: &str,
    existing_text: Option<&str>,
) -> Result<(), String> {
    let mut text = content.to_string();
    if let Some(existing) = existing_text {
        if existing.contains("\r\n") {
            text = normalize_line_endings(&text, "\r\n");
        }
        if existing.starts_with(UTF8_BOM) && !text.starts_with(UTF8_BOM) {
            text = format!("{UTF8_BOM}{text}");
        }
    }
    atomic_write_bytes(target, text.as_bytes(), existing_text)
}

fn normalize_line_endings(content: &str, target: &str) -> String {
    let lf = content.replace("\r\n", "\n").replace('\r', "\n");
    if target == "\r\n" {
        lf.replace('\n', "\r\n")
    } else {
        lf
    }
}

fn atomic_write_bytes(
    target: &Path,
    bytes: &[u8],
    existing_text: Option<&str>,
) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| "Target path has no parent directory".to_string())?;
    fs::create_dir_all(parent).map_err(|err| format!("Create parent dirs failed: {err}"))?;
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let tmp = parent.join(format!(
        ".kivio-tmp-{}-{file_name}",
        Uuid::new_v4().simple()
    ));
    {
        let mut file =
            fs::File::create(&tmp).map_err(|err| format!("Create temp file failed: {err}"))?;
        file.write_all(bytes)
            .map_err(|err| format!("Write temp file failed: {err}"))?;
        file.sync_all()
            .map_err(|err| format!("Sync temp file failed: {err}"))?;
    }
    #[cfg(unix)]
    if target.exists() {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(target) {
            let _ = fs::set_permissions(
                &tmp,
                fs::Permissions::from_mode(metadata.permissions().mode()),
            );
        }
    }
    let _ = existing_text;
    #[cfg(target_os = "windows")]
    if target.exists() {
        // std::fs::rename cannot replace an existing file on Windows. This
        // fallback has a tiny non-atomic gap, but still guarantees chunks are
        // never streamed directly into the target. A future Windows API
        // ReplaceFileW path can tighten this last commit step.
        fs::remove_file(target)
            .map_err(|err| format!("Remove existing target failed before replace: {err}"))?;
    }
    match fs::rename(&tmp, target) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            Err(format!("Rename temp file failed: {err}"))
        }
    }
}

fn planned_file_result(
    workspace: &NativeToolWorkspace,
    path: PathBuf,
    operation: &str,
    before: Option<&str>,
    after: Option<&str>,
) -> Result<FileMutationFile, String> {
    let display_path = workspace_display_path(workspace, &path);
    let (diff, additions, removals) = unified_diff(&display_path, before, after);
    Ok(FileMutationFile {
        path: display_path,
        operation: operation.to_string(),
        bytes_written: after.map(|content| content.len() as u64).unwrap_or(0),
        additions,
        removals,
        diff,
    })
}

/// LCS guard: above this many DP cells for the changed middle region, fall back
/// to a coarse single-hunk diff (whole middle as remove+add).
const DIFF_LCS_MAX_CELLS: usize = 250_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffOpKind {
    Equal,
    Remove,
    Add,
}

#[derive(Debug)]
struct DiffOp<'a> {
    kind: DiffOpKind,
    text: &'a str,
}

fn unified_diff(path: &str, before: Option<&str>, after: Option<&str>) -> (String, usize, usize) {
    let old_lines = diff_lines(before.unwrap_or(""));
    let new_lines = diff_lines(after.unwrap_or(""));
    if old_lines == new_lines {
        return (String::new(), 0, 0);
    }

    let mut prefix = 0usize;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }

    let mut old_end = old_lines.len();
    let mut new_end = new_lines.len();
    while old_end > prefix && new_end > prefix && old_lines[old_end - 1] == new_lines[new_end - 1] {
        old_end -= 1;
        new_end -= 1;
    }

    let ops = build_diff_ops(&old_lines, &new_lines, prefix, old_end, new_end);

    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.kind != DiffOpKind::Equal)
        .map(|(idx, _)| idx)
        .collect();
    if changed.is_empty() {
        return (String::new(), 0, 0);
    }

    // Prefix sums of old/new lines consumed before each op index.
    let mut old_consumed = vec![0usize; ops.len() + 1];
    let mut new_consumed = vec![0usize; ops.len() + 1];
    for (idx, op) in ops.iter().enumerate() {
        old_consumed[idx + 1] = old_consumed[idx]
            + usize::from(matches!(op.kind, DiffOpKind::Equal | DiffOpKind::Remove));
        new_consumed[idx + 1] =
            new_consumed[idx] + usize::from(matches!(op.kind, DiffOpKind::Equal | DiffOpKind::Add));
    }

    // Group changed ops into hunks: runs separated by 7+ unchanged lines split.
    let mut groups: Vec<(usize, usize)> = Vec::new();
    let mut group_start = changed[0];
    let mut group_last = changed[0];
    for &idx in &changed[1..] {
        if idx - group_last - 1 >= 7 {
            groups.push((group_start, group_last));
            group_start = idx;
        }
        group_last = idx;
    }
    groups.push((group_start, group_last));

    let old_header = if before.is_none() {
        "/dev/null".to_string()
    } else {
        format!("a/{path}")
    };
    let new_header = if after.is_none() {
        "/dev/null".to_string()
    } else {
        format!("b/{path}")
    };
    let mut out = String::new();
    out.push_str(&format!("--- {old_header}\n+++ {new_header}\n"));
    let mut additions = 0usize;
    let mut removals = 0usize;
    for (first, last) in groups {
        let hunk_start = first.saturating_sub(3);
        let hunk_end = (last + 4).min(ops.len());
        let old_count = old_consumed[hunk_end] - old_consumed[hunk_start];
        let new_count = new_consumed[hunk_end] - new_consumed[hunk_start];
        let old_start = if old_count == 0 {
            old_consumed[hunk_start]
        } else {
            old_consumed[hunk_start] + 1
        };
        let new_start = if new_count == 0 {
            new_consumed[hunk_start]
        } else {
            new_consumed[hunk_start] + 1
        };
        out.push_str(&format!(
            "@@ -{old_start},{old_count} +{new_start},{new_count} @@\n"
        ));
        for op in &ops[hunk_start..hunk_end] {
            match op.kind {
                DiffOpKind::Equal => out.push_str(&format!(" {}\n", op.text)),
                DiffOpKind::Remove => {
                    removals += 1;
                    out.push_str(&format!("-{}\n", op.text));
                }
                DiffOpKind::Add => {
                    additions += 1;
                    out.push_str(&format!("+{}\n", op.text));
                }
            }
        }
    }
    (out, additions, removals)
}

fn build_diff_ops<'a>(
    old_lines: &'a [String],
    new_lines: &'a [String],
    prefix: usize,
    old_end: usize,
    new_end: usize,
) -> Vec<DiffOp<'a>> {
    let mut ops = Vec::with_capacity(old_lines.len() + new_lines.len());
    for line in &old_lines[..prefix] {
        ops.push(DiffOp {
            kind: DiffOpKind::Equal,
            text: line,
        });
    }
    let middle_old = &old_lines[prefix..old_end];
    let middle_new = &new_lines[prefix..new_end];
    if middle_old.len().saturating_mul(middle_new.len()) > DIFF_LCS_MAX_CELLS {
        // Coarse fallback: whole middle as remove+add in a single block.
        for line in middle_old {
            ops.push(DiffOp {
                kind: DiffOpKind::Remove,
                text: line,
            });
        }
        for line in middle_new {
            ops.push(DiffOp {
                kind: DiffOpKind::Add,
                text: line,
            });
        }
    } else {
        let m = middle_old.len();
        let n = middle_new.len();
        let width = n + 1;
        let mut dp = vec![0u32; (m + 1) * width];
        for i in (0..m).rev() {
            for j in (0..n).rev() {
                dp[i * width + j] = if middle_old[i] == middle_new[j] {
                    dp[(i + 1) * width + j + 1] + 1
                } else {
                    dp[(i + 1) * width + j].max(dp[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0usize, 0usize);
        while i < m && j < n {
            if middle_old[i] == middle_new[j] {
                ops.push(DiffOp {
                    kind: DiffOpKind::Equal,
                    text: &middle_old[i],
                });
                i += 1;
                j += 1;
            } else if dp[(i + 1) * width + j] >= dp[i * width + j + 1] {
                ops.push(DiffOp {
                    kind: DiffOpKind::Remove,
                    text: &middle_old[i],
                });
                i += 1;
            } else {
                ops.push(DiffOp {
                    kind: DiffOpKind::Add,
                    text: &middle_new[j],
                });
                j += 1;
            }
        }
        while i < m {
            ops.push(DiffOp {
                kind: DiffOpKind::Remove,
                text: &middle_old[i],
            });
            i += 1;
        }
        while j < n {
            ops.push(DiffOp {
                kind: DiffOpKind::Add,
                text: &middle_new[j],
            });
            j += 1;
        }
    }
    for line in &old_lines[old_end..] {
        ops.push(DiffOp {
            kind: DiffOpKind::Equal,
            text: line,
        });
    }
    ops
}

fn diff_lines(content: &str) -> Vec<String> {
    if content.is_empty() {
        return Vec::new();
    }
    let normalized = content.replace("\r\n", "\n");
    let mut lines = normalized
        .split('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    if normalized.ends_with('\n') {
        lines.pop();
    }
    lines
}

fn mutation_operation_label(operation: &str) -> &'static str {
    match operation {
        "create" => "Created",
        "overwrite" => "Overwrote",
        "edit" => "Updated",
        "delete" => "Deleted",
        "noop" => "No changes",
        _ => "Changed",
    }
}

pub fn list_dir(workspace: &NativeToolWorkspace, arguments: &Value) -> Result<String, String> {
    let path = arguments
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let include_hidden = arguments
        .get("include_hidden")
        .or_else(|| arguments.get("includeHidden"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let max_entries = arguments
        .get("max_entries")
        .or_else(|| arguments.get("maxEntries"))
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(200)
        .clamp(1, MAX_LIST_ENTRIES);

    let dir = resolve_tool_read_path(workspace, path)?;
    if !dir.is_dir() {
        return Err(format!("不是可列出的文件夹: {path}"));
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|err| format!("Read directory failed: {err}"))? {
        let entry = entry.map_err(|err| format!("Read directory entry failed: {err}"))?;
        let path = entry.path();
        if !include_hidden && is_hidden_path(&path) {
            continue;
        }
        let metadata = entry
            .metadata()
            .map_err(|err| format!("Read entry metadata failed: {err}"))?;
        entries.push(path_info(workspace, &path, &metadata)?);
    }

    entries.sort_by(|a, b| {
        a.get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .cmp(b.get("type").and_then(|v| v.as_str()).unwrap_or(""))
            .then_with(|| {
                a.get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .cmp(b.get("path").and_then(|v| v.as_str()).unwrap_or(""))
            })
    });
    let truncated = entries.len() > max_entries;
    entries.truncate(max_entries);

    format_json(json!({
        "path": workspace_display_path(workspace, &dir),
        "entries": entries,
        "truncated": truncated
    }))
}

pub fn glob_files(workspace: &NativeToolWorkspace, arguments: &Value) -> Result<String, String> {
    let pattern = required_string(arguments, "pattern")?;
    validate_glob_pattern(pattern)?;
    let include_hidden = arguments
        .get("include_hidden")
        .or_else(|| arguments.get("includeHidden"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let max_results = arguments
        .get("max_results")
        .or_else(|| arguments.get("maxResults"))
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(200)
        .clamp(1, MAX_GLOB_RESULTS);
    let root = resolve_tool_read_path(
        workspace,
        arguments
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("."),
    )?;
    if !root.is_dir() {
        return Err("glob_files path must be a directory".to_string());
    }

    let mut matches = Vec::new();
    for path in walk_paths(&root, true, include_hidden, MAX_SEARCH_FILES)? {
        let rel = relative_slash_path(&root, &path);
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if glob_match(pattern, &rel) || (!pattern.contains('/') && glob_match(pattern, file_name)) {
            let metadata =
                fs::metadata(&path).map_err(|err| format!("Read metadata failed: {err}"))?;
            matches.push(path_info(workspace, &path, &metadata)?);
            if matches.len() >= max_results {
                break;
            }
        }
    }

    format_json(json!({
        "pattern": pattern,
        "matches": matches,
        "truncated": matches.len() >= max_results
    }))
}

pub fn search_files(workspace: &NativeToolWorkspace, arguments: &Value) -> Result<String, String> {
    // `query` 为主名；接受 `pattern` 作为别名——模型常受 grep/Claude Code 的 Grep 习惯影响
    // 传 `pattern`，否则要白白浪费一轮重试（和已有的 caseSensitive/maxResults 别名一致）。
    let query = arguments
        .get("query")
        .or_else(|| arguments.get("pattern"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "search_files requires query (or its alias pattern)".to_string())?;
    let search_root = resolve_tool_read_path(
        workspace,
        arguments
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("."),
    )?;
    if !search_root.exists() {
        let display = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".");
        return Err(format!("search_files path not found: {display}"));
    }
    let case_sensitive = arguments
        .get("case_sensitive")
        .or_else(|| arguments.get("caseSensitive"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let include_hidden = arguments
        .get("include_hidden")
        .or_else(|| arguments.get("includeHidden"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let use_regex = arguments
        .get("regex")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let max_results = arguments
        .get("max_results")
        .or_else(|| arguments.get("maxResults"))
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(100)
        .clamp(1, MAX_SEARCH_MATCHES);
    let output_mode = arguments
        .get("output_mode")
        .or_else(|| arguments.get("outputMode"))
        .and_then(|v| v.as_str())
        .unwrap_or("content");
    if !matches!(output_mode, "content" | "files_with_matches" | "count") {
        return Err("output_mode must be one of: content, files_with_matches, count".to_string());
    }
    // Optional context lines emitted before/after each content match (default 0).
    let context = arguments
        .get("context")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(0);
    let glob = arguments
        .get("glob")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|g| !g.is_empty());
    // `*.{py,ts}` 花括号展开：把单个 glob 展开成多个候选 patterns，每个文件满足任一即通过。
    let glob_patterns: Vec<String> = glob.map(expand_glob_braces).unwrap_or_default();

    // 匹配器：regex 为可选（默认 false 走字面量子串，保持向后兼容的旧行为）。
    enum Matcher {
        Regex(regex::Regex),
        Literal(String),
    }
    let matcher = if use_regex {
        let re = regex::RegexBuilder::new(query)
            .case_insensitive(!case_sensitive)
            .build()
            .map_err(|err| format!("Invalid regex: {err}"))?;
        Matcher::Regex(re)
    } else if case_sensitive {
        Matcher::Literal(query.to_string())
    } else {
        Matcher::Literal(query.to_lowercase())
    };
    let is_match = |line: &str| -> bool {
        match &matcher {
            Matcher::Regex(re) => re.is_match(line),
            Matcher::Literal(needle) if case_sensitive => line.contains(needle.as_str()),
            Matcher::Literal(needle) => line.to_lowercase().contains(needle.as_str()),
        }
    };

    let (scan_root, paths, walk_truncated) = if search_root.is_dir() {
        let paths = walk_paths(&search_root, true, include_hidden, MAX_SEARCH_FILES)?;
        let walk_truncated = paths.len() >= MAX_SEARCH_FILES;
        (search_root.clone(), paths, walk_truncated)
    } else if search_root.is_file() {
        let scan_root = search_root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        (scan_root, vec![search_root.clone()], false)
    } else {
        let display = workspace_display_path(workspace, &search_root);
        return Err(format!(
            "search_files path is neither a file nor a directory: {display}"
        ));
    };
    let mut files_scanned = 0usize;
    let mut content_matches = Vec::new();
    let mut files_with_matches = Vec::new();
    let mut counts = Vec::new();
    let mut limit_hit = false;

    'outer: for path in paths {
        if !path.is_file() {
            continue;
        }
        if !glob_patterns.is_empty() {
            let rel = relative_slash_path(&scan_root, &path);
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            let matches_any = glob_patterns
                .iter()
                .any(|p| glob_match(p, &rel) || (!p.contains('/') && glob_match(p, file_name)));
            if !matches_any {
                continue;
            }
        }
        let metadata = fs::metadata(&path).map_err(|err| format!("Read metadata failed: {err}"))?;
        if metadata.len() > MAX_SEARCH_FILE_BYTES {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        files_scanned += 1;
        let display = workspace_display_path(workspace, &path);
        let mut file_count = 0usize;
        let lines: Vec<&str> = content.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if !is_match(line) {
                continue;
            }
            file_count += 1;
            if output_mode == "content" {
                let mut entry = json!({
                    "path": display,
                    "line": idx + 1,
                    "text": cap_grep_line(line),
                });
                if context > 0 {
                    let before_start = idx.saturating_sub(context);
                    let after_end = (idx + 1 + context).min(lines.len());
                    let before: Vec<Value> = (before_start..idx)
                        .map(|i| json!({ "line": i + 1, "text": cap_grep_line(lines[i]) }))
                        .collect();
                    let after: Vec<Value> = ((idx + 1)..after_end)
                        .map(|i| json!({ "line": i + 1, "text": cap_grep_line(lines[i]) }))
                        .collect();
                    entry["before"] = json!(before);
                    entry["after"] = json!(after);
                }
                content_matches.push(entry);
                if content_matches.len() >= max_results {
                    limit_hit = true;
                    break 'outer;
                }
            }
        }
        if file_count > 0 {
            match output_mode {
                "files_with_matches" => {
                    files_with_matches.push(json!(display));
                    if files_with_matches.len() >= max_results {
                        limit_hit = true;
                        break 'outer;
                    }
                }
                "count" => {
                    counts.push(json!({ "path": display, "count": file_count }));
                    if counts.len() >= max_results {
                        limit_hit = true;
                        break 'outer;
                    }
                }
                _ => {}
            }
        }
    }

    let mut out = json!({
        "query": query,
        "regex": use_regex,
        "mode": output_mode,
        "files_scanned": files_scanned,
        "truncated": limit_hit,
        "walk_truncated": walk_truncated,
    });
    match output_mode {
        "files_with_matches" => {
            out["files"] = json!(files_with_matches);
        }
        "count" => {
            let total: u64 = counts.iter().filter_map(|c| c["count"].as_u64()).sum();
            out["counts"] = json!(counts);
            out["total"] = json!(total);
        }
        _ => {
            out["matches"] = json!(content_matches);
        }
    }
    format_json(out)
}

/// Cap a single grep output line at MAX_GREP_LINE_CHARS chars, appending a marker
/// when truncated. Counts by chars (not bytes) and respects char boundaries.
fn cap_grep_line(line: &str) -> String {
    if line.chars().count() <= MAX_GREP_LINE_CHARS {
        return line.to_string();
    }
    let truncated: String = line.chars().take(MAX_GREP_LINE_CHARS).collect();
    format!("{truncated}... [line truncated to {MAX_GREP_LINE_CHARS} chars]")
}

fn required_string<'a>(arguments: &'a Value, key: &str) -> Result<&'a str, String> {
    arguments
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn format_json(value: Value) -> Result<String, String> {
    serde_json::to_string_pretty(&value)
        .map_err(|err| format!("Serialize tool result failed: {err}"))
}

fn validate_glob_pattern(pattern: &str) -> Result<(), String> {
    let pattern_path = Path::new(pattern);
    if pattern_path.is_absolute() {
        return Err(
            "glob_files pattern must be relative to the search path; put the directory in path instead."
                .to_string(),
        );
    }
    if pattern_path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("glob_files pattern cannot contain '..'.".to_string());
    }
    Ok(())
}

fn path_info(
    workspace: &NativeToolWorkspace,
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<Value, String> {
    let kind = if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "file"
    } else {
        "other"
    };
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs());
    Ok(json!({
        "path": workspace_display_path(workspace, path),
        "type": kind,
        "sizeBytes": metadata.len(),
        "modifiedAt": modified
    }))
}

fn is_hidden_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with('.'))
        .unwrap_or(false)
}

/// Walk a directory tree, honoring `.gitignore` (and `.ignore`, global gitignore,
/// parent ignores) via the `ignore` crate — the same engine ripgrep uses — so
/// grep/find skip `node_modules`, `target`, `dist`, etc. automatically. A small
/// hardcoded floor (`DEFAULT_IGNORED_DIRS`) is always skipped too, so repos with
/// no `.gitignore` still avoid the obvious noise. `include_hidden` toggles
/// dotfile visibility (gitignore is still respected either way, like `rg --hidden`).
fn walk_paths(
    root: &Path,
    recursive: bool,
    include_hidden: bool,
    max_paths: usize,
) -> Result<Vec<PathBuf>, String> {
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(!include_hidden)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        // Honor .gitignore even when the directory isn't inside a git repo.
        .require_git(false)
        .max_depth(if recursive { None } else { Some(1) })
        .filter_entry(|entry| {
            // Never prune the search root itself (depth 0) — the `ignore` crate
            // applies this predicate to the root too, so without this guard a
            // search rooted at a dir literally named `build`/`dist`/`target`/…
            // would return nothing. Only prune DIRECTORIES whose name is in the
            // floor list; a regular file that happens to share the name stays.
            if entry.depth() == 0 {
                return true;
            }
            let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            if !is_dir {
                return true;
            }
            entry
                .file_name()
                .to_str()
                .map(|name| !DEFAULT_IGNORED_DIRS.contains(&name))
                .unwrap_or(true)
        });

    let mut out = Vec::new();
    for result in builder.build() {
        let entry = match result {
            Ok(entry) => entry,
            // Skip unreadable entries (permissions, races) rather than failing
            // the whole walk.
            Err(_) => continue,
        };
        // The walk yields the root itself at depth 0; callers want its contents.
        if entry.depth() == 0 {
            continue;
        }
        out.push(entry.into_path());
        if out.len() >= max_paths {
            break;
        }
    }
    Ok(out)
}

fn relative_slash_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// 展开 glob 花括号语法：`*.{py,ts}` → `["*.py", "*.ts"]`，`{a,b}/*.rs` → `["a/*.rs", "b/*.rs"]`。
/// 无花括号时直接返回单元素 Vec。嵌套花括号不支持（返回原始 pattern）。
fn expand_glob_braces(pattern: &str) -> Vec<String> {
    if let Some(open) = pattern.find('{') {
        if let Some(close) = pattern[open..].find('}') {
            let close = open + close;
            let prefix = &pattern[..open];
            let suffix = &pattern[close + 1..];
            let alternatives = &pattern[open + 1..close];
            return alternatives
                .split(',')
                .map(|alt| format!("{}{}{}", prefix, alt.trim(), suffix))
                .collect();
        }
    }
    vec![pattern.to_string()]
}

fn glob_match(pattern: &str, value: &str) -> bool {
    let pattern_parts: Vec<&str> = pattern.split('/').filter(|part| !part.is_empty()).collect();
    let value_parts: Vec<&str> = value.split('/').filter(|part| !part.is_empty()).collect();
    glob_match_parts(&pattern_parts, &value_parts)
}

fn glob_match_parts(pattern: &[&str], value: &[&str]) -> bool {
    if pattern.is_empty() {
        return value.is_empty();
    }
    if pattern[0] == "**" {
        return glob_match_parts(&pattern[1..], value)
            || (!value.is_empty() && glob_match_parts(pattern, &value[1..]));
    }
    if value.is_empty() {
        return false;
    }
    segment_match(pattern[0], value[0]) && glob_match_parts(&pattern[1..], &value[1..])
}

fn segment_match(pattern: &str, value: &str) -> bool {
    let p = pattern.as_bytes();
    let v = value.as_bytes();
    let (mut pi, mut vi) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut star_match = 0usize;
    while vi < v.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == v[vi]) {
            pi += 1;
            vi += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            star_match = vi;
            pi += 1;
        } else if let Some(star_idx) = star {
            pi = star_idx + 1;
            star_match += 1;
            vi = star_match;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    /// End-to-end simulation of an agent session using the Pi-style tool set.
    /// Exercises the new behaviors together: gitignore-aware grep/find, no-boundary
    /// writes outside the project root, read-back, and bash large-output offload.
    /// Run with: cargo test --bin kivio simulated_agent_session -- --nocapture
    #[tokio::test]
    async fn simulated_agent_session_exercises_pi_style_tools() {
        // ---- set up a realistic mini project ----
        let proj = std::env::temp_dir().join(format!("kivio_sim_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(proj.join("src")).expect("mkdir src");
        fs::create_dir_all(proj.join("node_modules/leftpad")).expect("mkdir node_modules");
        fs::write(proj.join(".gitignore"), "node_modules/\ndist/\n").expect("write gitignore");
        fs::write(
            proj.join("src/app.rs"),
            "fn main() {\n    // TODO: wire up the CLI\n}\n",
        )
        .expect("write app.rs");
        fs::write(proj.join("src/util.rs"), "pub fn helper() -> u32 { 42 }\n")
            .expect("write util.rs");
        fs::write(
            proj.join("node_modules/leftpad/index.js"),
            "// TODO: vendored junk that must NOT show up\n",
        )
        .expect("write vendored js");
        let ws = NativeToolWorkspace::project(
            "sim".into(),
            "Sim".into(),
            Some(proj.to_string_lossy().into_owned()),
        );
        println!("\n=== Simulated agent session in {} ===", proj.display());

        // 1) registry exposes exactly the file/shell short names (read now also
        // lists directories; find renamed glob; ls removed).
        let names: Vec<&str> = crate::mcp::native_registry::NATIVE_TOOLS
            .iter()
            .map(|e| e.name)
            .filter(|n| matches!(*n, "read" | "write" | "edit" | "bash" | "grep" | "glob"))
            .collect();
        println!("\n[1] file/shell tools in registry: {names:?}");
        assert_eq!(names.len(), 6, "exactly the 6 file/shell short-named tools");

        // 2) grep "TODO" — finds src/app.rs, skips gitignored node_modules.
        let grep = search_files(&ws, &json!({ "query": "TODO" })).expect("grep");
        println!("\n[2] grep TODO:\n{grep}");
        assert!(grep.contains("app.rs"), "grep finds the source TODO");
        assert!(
            !grep.contains("node_modules") && !grep.contains("leftpad"),
            "gitignored node_modules is skipped"
        );

        // 3) find "*.rs" finds sources; find "*.js" skips gitignored vendor js.
        let find_rs = glob_files(&ws, &json!({ "pattern": "*.rs" })).expect("find rs");
        println!("\n[3a] find *.rs:\n{find_rs}");
        assert!(find_rs.contains("app.rs") && find_rs.contains("util.rs"));
        let find_js = glob_files(&ws, &json!({ "pattern": "*.js" })).expect("find js");
        println!("\n[3b] find *.js:\n{find_js}");
        assert!(!find_js.contains("leftpad"), "gitignored js is skipped");

        // 4) no-boundary: write to an absolute path OUTSIDE the project root.
        let outside =
            std::env::temp_dir().join(format!("kivio_sim_outside_{}.txt", uuid::Uuid::new_v4()));
        let written = write_file(
            &ws,
            &json!({ "path": outside.to_string_lossy(), "content": "escaped the project root\n" }),
        )
        .expect("write outside project (no boundary)");
        println!(
            "\n[4] write outside project -> ok (operation={}, path={})",
            written.operation,
            outside.display()
        );
        assert!(outside.is_file(), "file written outside project root");

        // 5) read it back.
        let read =
            read_file(&ws, &json!({ "path": outside.to_string_lossy() })).expect("read back");
        println!("[5] read back content: {:?}", read.content.trim());
        assert_eq!(read.content, "escaped the project root\n");

        // 6) bash: a large output is offloaded to a temp log with a path note.
        let bash = crate::native_tools::run_command(
            &ws,
            &json!({
                "command": "for i in $(seq 1 4000); do echo \"line $i ----------------------------------------------------------------\"; done"
            }),
            None,
            None,
            )
        .await
        .expect("bash large output");
        let first_line = bash.lines().next().unwrap_or("");
        println!("\n[6] bash large output, first line:\n{first_line}");
        assert!(
            bash.contains("complete log saved to"),
            "large bash output is offloaded to a temp log"
        );

        // cleanup
        let _ = fs::remove_dir_all(&proj);
        let _ = fs::remove_file(&outside);
        if let Some(rest) = first_line.strip_prefix("[full output:") {
            if let Some(idx) = rest.find("saved to ") {
                if let Some(path) = rest[idx + "saved to ".len()..].split('.').next() {
                    let _ = fs::remove_file(path.trim());
                }
            }
        }
        println!("\n=== simulation complete: all assertions passed ===\n");
    }

    #[test]
    fn read_file_allows_temp_paths() {
        let file = std::env::temp_dir().join(format!("kivio_read_{}.txt", uuid::Uuid::new_v4()));
        fs::write(&file, "alpha\nbeta\n").expect("write");

        let workspace = NativeToolWorkspace::global(&[]);
        let result =
            read_file(&workspace, &json!({ "path": file.to_string_lossy() })).expect("read");
        let canonical = fs::canonicalize(&file).expect("canonicalize temp file");
        assert_eq!(result.content, "alpha\nbeta\n");
        assert_eq!(result.path, canonical.to_string_lossy());
        assert_eq!(result.resolved_path, canonical.to_string_lossy());
        assert_eq!(result.total_lines, 2);
        assert_eq!(result.start_line, 1);
        assert_eq!(result.end_line, 2);
        assert!(!result.truncated);
        assert_eq!(result.next_offset, None);

        let _ = fs::remove_file(file);
    }

    #[test]
    fn read_file_returns_range_metadata_for_partial_reads() {
        let file = std::env::temp_dir().join(format!("kivio_read_{}.txt", uuid::Uuid::new_v4()));
        fs::write(&file, "one\ntwo\nthree\nfour\n").expect("write");

        let workspace = NativeToolWorkspace::global(&[]);
        let result = read_file(
            &workspace,
            &json!({
                "path": file.to_string_lossy(),
                "offset": 2,
                "limit": 2
            }),
        )
        .expect("read range");

        assert_eq!(result.content, "two\nthree");
        assert_eq!(result.total_lines, 4);
        assert_eq!(result.start_line, 2);
        assert_eq!(result.end_line, 3);
        assert!(result.truncated);
        assert_eq!(result.next_offset, Some(4));

        let _ = fs::remove_file(file);
    }

    #[test]
    fn read_file_windows_oversized_file_instead_of_failing() {
        let file = std::env::temp_dir().join(format!("kivio_big_{}.txt", uuid::Uuid::new_v4()));
        let line = "x".repeat(1024);
        let mut body = String::new();
        for idx in 0..3000 {
            body.push_str(&format!("{idx} {line}\n"));
        }
        assert!(body.len() as u64 > MAX_READ_FILE_BYTES);
        fs::write(&file, &body).expect("write big file");

        let workspace = NativeToolWorkspace::global(&[]);
        // No offset/limit: used to be a hard error. Now it returns the head window,
        // bounded by the 50KB byte budget (each line is ~1KB → ~50 lines).
        let head = read_file(&workspace, &json!({ "path": file.to_string_lossy() }))
            .expect("oversized read returns a window");
        assert_eq!(head.total_lines, 3000);
        assert_eq!(head.start_line, 1);
        assert!(head.content.len() <= TOOL_OUTPUT_MAX_BYTES);
        assert!(head.end_line < 3000, "capped, not the whole file");
        assert!(head.truncated);
        assert_eq!(head.next_offset, Some(head.end_line + 1));
        assert!(
            head.warnings.iter().any(|w| w.contains("50KB")),
            "byte cap explained to the model: {:?}",
            head.warnings
        );

        let result = read_file(
            &workspace,
            &json!({
                "path": file.to_string_lossy(),
                "offset": 2001,
                "limit": 2
            }),
        )
        .expect("windowed read of oversized file");
        assert_eq!(result.total_lines, 3000);
        assert_eq!(result.start_line, 2001);
        assert_eq!(result.end_line, 2002);
        assert!(result.content.starts_with("2000 "));
        assert!(result.truncated);
        assert_eq!(result.next_offset, Some(2003));
        assert!(result.warnings.is_empty(), "user limit is not a cap hit");

        let _ = fs::remove_file(file);
    }

    #[test]
    fn read_file_caps_output_at_line_and_byte_budgets() {
        let workspace = NativeToolWorkspace::global(&[]);

        // Line budget: many short lines, well under the byte budget.
        let many = std::env::temp_dir().join(format!("kivio_lines_{}.txt", uuid::Uuid::new_v4()));
        let body = (0..TOOL_OUTPUT_MAX_LINES + 500)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&many, &body).expect("write");
        let result = read_file(&workspace, &json!({ "path": many.to_string_lossy() }))
            .expect("read many lines");
        assert_eq!(result.end_line, TOOL_OUTPUT_MAX_LINES);
        assert!(result.truncated);
        assert_eq!(result.next_offset, Some(TOOL_OUTPUT_MAX_LINES + 1));
        assert!(result.warnings[0].contains("2000 行"));
        // An oversized model-supplied `limit` cannot lift the cap.
        let forced = read_file(
            &workspace,
            &json!({ "path": many.to_string_lossy(), "limit": 999_999 }),
        )
        .expect("read with huge limit");
        assert_eq!(forced.end_line, TOOL_OUTPUT_MAX_LINES);
        let _ = fs::remove_file(many);

        // Byte budget: few lines, each huge.
        let fat = std::env::temp_dir().join(format!("kivio_fat_{}.txt", uuid::Uuid::new_v4()));
        let chunk = "y".repeat(20 * 1024);
        fs::write(&fat, format!("{chunk}\n{chunk}\n{chunk}\n{chunk}\n")).expect("write");
        let result =
            read_file(&workspace, &json!({ "path": fat.to_string_lossy() })).expect("read fat");
        assert!(result.content.len() <= TOOL_OUTPUT_MAX_BYTES);
        assert_eq!(result.end_line, 2, "two 20KB lines fit under 50KB");
        assert!(result.warnings[0].contains("50KB"));
        let _ = fs::remove_file(fat);

        // A single line bigger than the whole budget: nothing to return, but the
        // model gets an escape hatch and next_offset never points at that line.
        let monster = std::env::temp_dir().join(format!("kivio_mon_{}.txt", uuid::Uuid::new_v4()));
        fs::write(&monster, format!("{}\nshort\n", "z".repeat(60 * 1024))).expect("write");
        let result = read_file(&workspace, &json!({ "path": monster.to_string_lossy() }))
            .expect("read monster line");
        assert!(result.content.is_empty());
        assert_eq!(result.next_offset, Some(2), "skips past the monster line");
        assert!(result.warnings[0].contains("run_command"));
        let _ = fs::remove_file(monster);
    }

    #[test]
    fn zero_limit_stays_put_and_multibyte_budget_counts_bytes() {
        let workspace = NativeToolWorkspace::global(&[]);

        // `limit: 0` also yields kept == 0, but it is NOT the monster-line case:
        // next_offset must point back at the same line, never skip one. Regression
        // for the `kept == 0` shortcut that silently swallowed line 1.
        let file = std::env::temp_dir().join(format!("kivio_zero_{}.txt", uuid::Uuid::new_v4()));
        fs::write(&file, "one\ntwo\nthree\n").expect("write");
        let result = read_file(
            &workspace,
            &json!({ "path": file.to_string_lossy(), "limit": 0 }),
        )
        .expect("zero limit");
        assert_eq!(result.next_offset, Some(1), "must not skip line 1");
        assert!(result.warnings.is_empty(), "no cap was hit");
        let _ = fs::remove_file(file);

        // The byte budget counts BYTES, not chars: CJK is 3 bytes per char, so two
        // 9k-char lines are 54KB and cannot share one 50KB window.
        let cjk = std::env::temp_dir().join(format!("kivio_cjk_{}.txt", uuid::Uuid::new_v4()));
        let line = "中".repeat(9_000); // 27_000 bytes
        fs::write(&cjk, format!("{line}\n{line}\n{line}\n")).expect("write");
        let result =
            read_file(&workspace, &json!({ "path": cjk.to_string_lossy() })).expect("read cjk");
        assert_eq!(result.end_line, 1, "27KB × 2 would blow the 50KB budget");
        assert_eq!(
            result.content.chars().count(),
            9_000,
            "whole chars, never cut mid-codepoint"
        );
        let _ = fs::remove_file(cjk);
    }

    #[test]
    fn truncate_head_matches_pi_semantics() {
        // Fits → untouched.
        assert_eq!(truncate_head(&["a", "b"]), (2, None));
        // Line budget wins when lines are short.
        let short: Vec<&str> = vec!["a"; TOOL_OUTPUT_MAX_LINES + 10];
        assert_eq!(
            truncate_head(&short),
            (TOOL_OUTPUT_MAX_LINES, Some(TruncatedBy::Lines))
        );
        // Byte budget wins when lines are long, and only whole lines come back.
        let long = "q".repeat(30 * 1024);
        let fat: Vec<&str> = vec![long.as_str(); 5];
        assert_eq!(truncate_head(&fat), (1, Some(TruncatedBy::Bytes)));
        // First line alone over budget → nothing kept.
        let huge = "q".repeat(TOOL_OUTPUT_MAX_BYTES + 1);
        assert_eq!(
            truncate_head(&[huge.as_str(), "tail"]),
            (0, Some(TruncatedBy::Bytes))
        );
        assert_eq!(truncate_head(&[]), (0, None));
    }

    #[test]
    fn edit_file_requires_unique_match_and_supports_multiple_edits() {
        let home = super::super::user_home_dir().expect("home");
        let dir = home.join(format!(".kivio_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        let file = dir.join("sample.txt");
        fs::write(&file, "alpha\nbeta\nalpha\n").expect("write");

        let rel = file.to_string_lossy().to_string();
        let workspace = NativeToolWorkspace::global(&[]);

        // A non-unique old_string is rejected (replace_all no longer exists).
        let err = edit_file(
            &workspace,
            &json!({ "path": rel, "edits": [{ "old_string": "alpha", "new_string": "gamma" }] }),
        )
        .unwrap_err();
        assert!(err.contains("appears"));

        // Multiple edits in one call, applied in order: disambiguate the first with
        // surrounding context, then the remaining occurrence becomes unique.
        edit_file(
            &workspace,
            &json!({
                "path": rel,
                "edits": [
                    { "old_string": "alpha\nbeta", "new_string": "gamma\nbeta" },
                    { "old_string": "alpha", "new_string": "gamma" }
                ]
            }),
        )
        .expect("two edits in one call");

        let content = fs::read_to_string(&file).expect("read");
        assert_eq!(content, "gamma\nbeta\ngamma\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_file_reports_noop_when_old_equals_new() {
        let home = super::super::user_home_dir().expect("home");
        let dir = home.join(format!(".kivio_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        let file = dir.join("sample.txt");
        fs::write(&file, "hello world").expect("write");

        let rel = file.to_string_lossy().to_string();
        let workspace = NativeToolWorkspace::global(&[]);
        let result = edit_file(
            &workspace,
            &json!({
                "path": rel,
                "edits": [{ "old_string": "hello world", "new_string": "hello world" }]
            }),
        )
        .expect("noop edit");
        assert_eq!(result.files[0].operation, "noop");
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.contains("identical")));
        assert_eq!(fs::read_to_string(&file).expect("read"), "hello world");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_file_matches_lf_old_string_against_crlf_file_and_keeps_crlf() {
        let root = std::env::temp_dir().join(format!("kivio_edit_crlf_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("mkdir");
        let workspace = NativeToolWorkspace::project(
            "proj".to_string(),
            "T".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );
        let file = root.join("crlf.txt");
        fs::write(&file, "line one\r\nline two\r\nline three\r\n").expect("write");

        // 模型给的是 LF old_string；文件是 CRLF —— 归一化后仍命中。
        let result = edit_file(
            &workspace,
            &json!({
                "path": "crlf.txt",
                "edits": [{ "old_string": "line two\n", "new_string": "line 2\n" }]
            }),
        )
        .expect("LF old_string must match a CRLF file");
        assert!(result.ok);
        assert_eq!(result.files[0].operation, "edit");
        // diff 只反映真实变更，不把 CRLF→LF 算成整文件变更。
        assert_eq!(result.additions, 1);
        assert_eq!(result.removals, 1);
        // 写回保持 CRLF 风格。
        let on_disk = String::from_utf8(fs::read(&file).expect("read bytes")).expect("utf8");
        assert_eq!(on_disk, "line one\r\nline 2\r\nline three\r\n");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn edit_file_treats_crlf_vs_lf_only_change_as_noop() {
        let root = std::env::temp_dir().join(format!("kivio_edit_noop_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("mkdir");
        let workspace = NativeToolWorkspace::project(
            "proj".to_string(),
            "T".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );
        let file = root.join("file.txt");
        fs::write(&file, "alpha\r\nbeta\r\n").expect("write");

        // old/new 仅行尾写法不同，归一后相等 → 视为 noop，不改盘。
        let result = edit_file(
            &workspace,
            &json!({
                "path": "file.txt",
                "edits": [{ "old_string": "alpha\r\nbeta", "new_string": "alpha\nbeta" }]
            }),
        )
        .expect("line-ending-only change is a noop");
        assert_eq!(result.files[0].operation, "noop");
        assert_eq!(
            String::from_utf8(fs::read(&file).expect("read")).expect("utf8"),
            "alpha\r\nbeta\r\n"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn edit_file_lf_file_still_edits_and_keeps_lf() {
        let root = std::env::temp_dir().join(format!("kivio_edit_lf_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("mkdir");
        let workspace = NativeToolWorkspace::project(
            "proj".to_string(),
            "T".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );
        let file = root.join("lf.txt");
        fs::write(&file, "x\ny\nz\n").expect("write");

        let result = edit_file(
            &workspace,
            &json!({
                "path": "lf.txt",
                "edits": [{ "old_string": "y\n", "new_string": "Y\n" }]
            }),
        )
        .expect("LF file edit");
        assert!(result.ok);
        let on_disk = String::from_utf8(fs::read(&file).expect("read")).expect("utf8");
        assert_eq!(on_disk, "x\nY\nz\n");
        assert!(!on_disk.contains('\r'), "LF file must not gain CR");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn edit_file_fuzzy_matches_smart_quotes() {
        let root = std::env::temp_dir().join(format!("kivio_fuzzy_q_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("mkdir");
        let workspace = NativeToolWorkspace::project(
            "proj".to_string(),
            "T".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );
        let file = root.join("q.txt");
        // File uses curly single + double quotes (e.g. pasted from a doc).
        fs::write(&file, "say \u{201C}hello\u{201D} and don\u{2019}t stop\n").expect("write");

        // Model supplies ASCII quotes — exact match fails, fuzzy must succeed.
        let result = edit_file(
            &workspace,
            &json!({
                "path": "q.txt",
                "edits": [{ "old_string": "say \"hello\" and don't stop", "new_string": "say BYE and quit" }]
            }),
        )
        .expect("fuzzy smart-quote match");
        assert!(result.ok);
        assert_eq!(result.files[0].operation, "edit");
        let on_disk = fs::read_to_string(&file).expect("read");
        assert_eq!(on_disk, "say BYE and quit\n");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn edit_file_fuzzy_matches_dashes_and_nfkc() {
        let root = std::env::temp_dir().join(format!("kivio_fuzzy_d_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("mkdir");
        let workspace = NativeToolWorkspace::project(
            "proj".to_string(),
            "T".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );
        let file = root.join("d.txt");
        // File: em-dash + a NFKC-decomposable ligature (ﬁ U+FB01 -> "fi").
        fs::write(&file, "range A\u{2014}B \u{FB01}le\n").expect("write");

        // Model: ASCII hyphen + plain "file" — only fuzzy (NFKC + dash) matches.
        let result = edit_file(
            &workspace,
            &json!({
                "path": "d.txt",
                "edits": [{ "old_string": "range A-B file", "new_string": "range done" }]
            }),
        )
        .expect("fuzzy dash + nfkc match");
        assert!(result.ok);
        let on_disk = fs::read_to_string(&file).expect("read");
        assert_eq!(on_disk, "range done\n");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn edit_file_fuzzy_matches_whitespace_runs() {
        let root = std::env::temp_dir().join(format!("kivio_fuzzy_ws_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("mkdir");
        let workspace = NativeToolWorkspace::project(
            "proj".to_string(),
            "T".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );
        let file = root.join("ws.txt");
        // File uses multiple spaces + a tab between tokens.
        fs::write(&file, "let   x\t=  1;\n").expect("write");

        // Model uses single spaces — whitespace-run normalization makes it match.
        let result = edit_file(
            &workspace,
            &json!({
                "path": "ws.txt",
                "edits": [{ "old_string": "let x = 1;", "new_string": "let y = 2;" }]
            }),
        )
        .expect("fuzzy whitespace match");
        assert!(result.ok);
        let on_disk = fs::read_to_string(&file).expect("read");
        // The matched run is replaced wholesale; surrounding bytes preserved.
        assert_eq!(on_disk, "let y = 2;\n");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn edit_file_fuzzy_rejects_ambiguous_match() {
        let root = std::env::temp_dir().join(format!("kivio_fuzzy_amb_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("mkdir");
        let workspace = NativeToolWorkspace::project(
            "proj".to_string(),
            "T".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );
        let file = root.join("amb.txt");
        // Two curly-quote occurrences; an ASCII-quote needle fuzzily hits both.
        fs::write(&file, "a \u{201C}x\u{201D} b\na \u{201C}x\u{201D} b\n").expect("write");

        let err = edit_file(
            &workspace,
            &json!({
                "path": "amb.txt",
                "edits": [{ "old_string": "a \"x\" b", "new_string": "z" }]
            }),
        )
        .unwrap_err();
        assert!(
            err.contains("appears"),
            "ambiguous fuzzy match must error: {err}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn edit_file_exact_match_still_preferred_over_fuzzy() {
        let root = std::env::temp_dir().join(format!("kivio_fuzzy_exact_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("mkdir");
        let workspace = NativeToolWorkspace::project(
            "proj".to_string(),
            "T".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );
        let file = root.join("exact.txt");
        fs::write(&file, "plain ascii line\n").expect("write");

        // Exact match path: identical bytes, no normalization needed.
        let result = edit_file(
            &workspace,
            &json!({
                "path": "exact.txt",
                "edits": [{ "old_string": "plain ascii line", "new_string": "changed line" }]
            }),
        )
        .expect("exact match still works");
        assert!(result.ok);
        assert_eq!(fs::read_to_string(&file).expect("read"), "changed line\n");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn edit_file_fuzzy_not_found_errors() {
        let root = std::env::temp_dir().join(format!("kivio_fuzzy_nf_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("mkdir");
        let workspace = NativeToolWorkspace::project(
            "proj".to_string(),
            "T".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );
        let file = root.join("nf.txt");
        fs::write(&file, "alpha beta\n").expect("write");

        let err = edit_file(
            &workspace,
            &json!({
                "path": "nf.txt",
                "edits": [{ "old_string": "gamma delta", "new_string": "z" }]
            }),
        )
        .unwrap_err();
        assert!(
            err.contains("not found"),
            "no fuzzy match must error: {err}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cap_grep_line_truncates_long_lines() {
        let short = "abc";
        assert_eq!(cap_grep_line(short), short);
        let long: String = "x".repeat(MAX_GREP_LINE_CHARS + 50);
        let capped = cap_grep_line(&long);
        assert!(capped.starts_with(&"x".repeat(MAX_GREP_LINE_CHARS)));
        assert!(capped.contains("line truncated"));
        assert_eq!(
            capped.chars().filter(|c| *c == 'x').count(),
            MAX_GREP_LINE_CHARS
        );
    }

    #[test]
    fn search_files_context_and_long_line_cap() {
        let root = std::env::temp_dir().join(format!("kivio_grep_ctx_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("mkdir");
        let workspace = NativeToolWorkspace::project(
            "proj".to_string(),
            "T".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );
        // 5 lines; match on line 3 (index 2). Line 3 itself is very long.
        let long_match = format!("NEEDLE {}", "y".repeat(MAX_GREP_LINE_CHARS + 100));
        let body = format!("l1\nl2\n{long_match}\nl4\nl5\n");
        fs::write(root.join("ctx.txt"), &body).expect("write");

        let parse = |s: String| serde_json::from_str::<Value>(&s).expect("json");
        let out = parse(
            search_files(&workspace, &json!({ "query": "NEEDLE", "context": 1 }))
                .expect("context grep"),
        );
        let matches = out["matches"].as_array().expect("matches");
        assert_eq!(matches.len(), 1);
        let m = &matches[0];
        assert_eq!(m["line"], 3);
        // Long matching line is capped.
        let text = m["text"].as_str().unwrap();
        assert!(
            text.contains("line truncated"),
            "long line must be capped: {text}"
        );
        // context=1 → one before (l2) + one after (l4).
        let before = m["before"].as_array().expect("before");
        let after = m["after"].as_array().expect("after");
        assert_eq!(before.len(), 1);
        assert_eq!(before[0]["text"], "l2");
        assert_eq!(before[0]["line"], 2);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0]["text"], "l4");
        assert_eq!(after[0]["line"], 4);

        // Default context=0 → no before/after keys emitted.
        let out0 = parse(search_files(&workspace, &json!({ "query": "NEEDLE" })).expect("no ctx"));
        let m0 = &out0["matches"].as_array().unwrap()[0];
        assert!(m0.get("before").is_none());
        assert!(m0.get("after").is_none());

        let _ = fs::remove_dir_all(&root);
    }
    #[test]
    fn search_files_regex_output_modes_and_glob() {
        let root = std::env::temp_dir().join(format!("kivio_search_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("mkdir");
        let workspace = NativeToolWorkspace::project(
            "proj".to_string(),
            "T".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );
        fs::write(root.join("a.rs"), "fn alpha() {}\nlet x = 1;\n").expect("write a");
        fs::write(root.join("b.txt"), "alpha beta\nALPHA\n").expect("write b");

        let parse = |s: String| serde_json::from_str::<Value>(&s).expect("json");

        // 默认字面量、大小写不敏感：alpha 命中 a.rs 1 行 + b.txt 2 行 = 3。
        let out = parse(search_files(&workspace, &json!({ "query": "alpha" })).expect("literal"));
        assert_eq!(out["mode"], "content");
        assert_eq!(out["regex"], false);
        assert_eq!(out["matches"].as_array().unwrap().len(), 3);

        // regex：仅以 "let " 开头的行。
        let out = parse(
            search_files(&workspace, &json!({ "query": "^let ", "regex": true })).expect("regex"),
        );
        assert_eq!(out["regex"], true);
        assert_eq!(out["matches"].as_array().unwrap().len(), 1);

        // files_with_matches：两个文件都含 alpha。
        let out = parse(
            search_files(
                &workspace,
                &json!({ "query": "alpha", "output_mode": "files_with_matches" }),
            )
            .expect("fwm"),
        );
        assert_eq!(out["files"].as_array().unwrap().len(), 2);

        // count：总命中 3。
        let out = parse(
            search_files(
                &workspace,
                &json!({ "query": "alpha", "output_mode": "count" }),
            )
            .expect("count"),
        );
        assert_eq!(out["total"], 3);

        // glob：只搜 *.rs → alpha 命中 1 行。
        let out = parse(
            search_files(&workspace, &json!({ "query": "alpha", "glob": "*.rs" })).expect("glob"),
        );
        assert_eq!(out["matches"].as_array().unwrap().len(), 1);

        // 非法 regex 报错。
        assert!(search_files(&workspace, &json!({ "query": "(", "regex": true })).is_err());

        // pattern 作为 query 的别名（模型受 grep 习惯传 pattern 时不再白白失败一轮）。
        let out =
            parse(search_files(&workspace, &json!({ "pattern": "alpha" })).expect("pattern alias"));
        assert_eq!(out["matches"].as_array().unwrap().len(), 3);
        // 两者都缺则报错。
        assert!(search_files(&workspace, &json!({ "path": "." })).is_err());

        // 花括号 glob：`*.{rs,txt}` 只命中两种扩展名，不命中 .py。
        let out = parse(
            search_files(
                &workspace,
                &json!({ "query": "alpha", "glob": "*.{rs,txt}" }),
            )
            .expect("brace glob"),
        );
        // 只有 a.rs 有 alpha，b.txt 没有（内容是 "alpha beta..."）——实际有匹配
        // 关键断言：b.txt 和 a.rs 都在匹配范围内，但 a.rs 的 alpha 一定命中。
        let paths: Vec<_> = out["matches"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["path"].as_str())
            .collect();
        assert!(
            paths
                .iter()
                .all(|p| p.ends_with(".rs") || p.ends_with(".txt")),
            "brace glob must only match .rs and .txt files, got: {paths:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn search_files_accepts_single_file_paths() {
        let root = std::env::temp_dir().join(format!("kivio_search_file_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("mkdir");
        let workspace = NativeToolWorkspace::project(
            "proj".to_string(),
            "T".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );
        let file = root.join("one.ts");
        fs::write(
            &file,
            "first\nconst ClaudeAgentClient = 1;\nmatch NEEDLE here\nlast\n",
        )
        .expect("write file");
        let file_path = file.to_string_lossy().into_owned();
        let parse = |s: String| serde_json::from_str::<Value>(&s).expect("json");

        let literal = parse(
            search_files(
                &workspace,
                &json!({ "query": "ClaudeAgentClient", "path": file_path }),
            )
            .expect("literal file"),
        );
        assert_eq!(literal["files_scanned"], 1);
        assert_eq!(literal["walk_truncated"], false);
        assert_eq!(literal["matches"].as_array().unwrap().len(), 1);
        assert!(literal["matches"][0]["path"]
            .as_str()
            .unwrap()
            .ends_with("one.ts"));

        let regex = parse(
            search_files(
                &workspace,
                &json!({ "query": "^const\\s+ClaudeAgentClient", "regex": true, "path": file_path }),
            )
            .expect("regex file"),
        );
        assert_eq!(regex["matches"].as_array().unwrap().len(), 1);

        let with_context = parse(
            search_files(
                &workspace,
                &json!({ "query": "NEEDLE", "path": file_path, "context": 1 }),
            )
            .expect("context file"),
        );
        let m = &with_context["matches"].as_array().unwrap()[0];
        assert_eq!(m["line"], 3);
        assert_eq!(
            m["before"].as_array().unwrap()[0]["text"],
            "const ClaudeAgentClient = 1;"
        );
        assert_eq!(m["after"].as_array().unwrap()[0]["text"], "last");

        let count = parse(
            search_files(
                &workspace,
                &json!({ "query": "e", "path": file_path, "output_mode": "count" }),
            )
            .expect("count file"),
        );
        assert_eq!(count["counts"].as_array().unwrap().len(), 1);
        assert!(count["total"].as_u64().unwrap() >= 2);

        let files = parse(
            search_files(
                &workspace,
                &json!({ "query": "ClaudeAgentClient", "path": file_path, "output_mode": "files_with_matches" }),
            )
            .expect("files_with_matches file"),
        );
        assert_eq!(files["files"].as_array().unwrap().len(), 1);

        let glob_hit = parse(
            search_files(
                &workspace,
                &json!({ "query": "ClaudeAgentClient", "path": file_path, "glob": "*.ts" }),
            )
            .expect("glob hit"),
        );
        assert_eq!(glob_hit["matches"].as_array().unwrap().len(), 1);

        let glob_miss = parse(
            search_files(
                &workspace,
                &json!({ "query": "ClaudeAgentClient", "path": file_path, "glob": "*.rs" }),
            )
            .expect("glob miss"),
        );
        assert_eq!(glob_miss["matches"].as_array().unwrap().len(), 0);
        assert_eq!(glob_miss["files_scanned"], 0);

        let missing = search_files(
            &workspace,
            &json!({ "query": "ClaudeAgentClient", "path": root.join("missing.ts").to_string_lossy().into_owned() }),
        )
        .unwrap_err();
        assert!(
            missing.contains("path not found"),
            "missing path must mention not found: {missing}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn search_respects_gitignore_and_walks_root_named_like_ignored_dir() {
        // Regression: filter_entry must not prune the search root itself, and must
        // skip gitignored dirs (node_modules) without hiding same-named files.
        let root = std::env::temp_dir().join(format!("kivio_gi_build_{}", uuid::Uuid::new_v4()));
        // Root is literally named like an ignored dir ("build") — must still walk.
        let root = root.join("build");
        fs::create_dir_all(root.join("src")).expect("mkdir src");
        fs::create_dir_all(root.join("node_modules/pkg")).expect("mkdir node_modules");
        fs::write(root.join(".gitignore"), "node_modules/\n").expect("write gitignore");
        fs::write(root.join("src/app.rs"), "let needle = 1;\n").expect("write app");
        fs::write(root.join("node_modules/pkg/index.js"), "needle vendored\n")
            .expect("write vendor");
        // A regular FILE named "build" must not be pruned by the dir floor list.
        fs::write(root.join("build"), "needle in a file named build\n").expect("write build file");
        let workspace = NativeToolWorkspace::project(
            "proj".to_string(),
            "T".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );

        let parse = |s: String| serde_json::from_str::<Value>(&s).expect("json");
        let out = parse(
            search_files(
                &workspace,
                &json!({ "query": "needle", "output_mode": "files_with_matches" }),
            )
            .expect("search"),
        );
        let files: Vec<String> = out["files"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|f| f.as_str().map(str::to_string))
            .collect();
        // Root walked (not pruned by its "build" name): src/app.rs found.
        assert!(
            files.iter().any(|f| f.ends_with("app.rs")),
            "root must be walked: {files:?}"
        );
        // The file literally named "build" is found (only DIRS are floor-pruned).
        assert!(
            files.iter().any(|f| f.ends_with("build")),
            "same-named file kept: {files:?}"
        );
        // node_modules is gitignored → its file is skipped.
        assert!(
            !files.iter().any(|f| f.contains("node_modules")),
            "gitignored dir skipped: {files:?}"
        );

        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn expand_glob_braces_splits_alternatives() {
        assert_eq!(
            expand_glob_braces("*.{py,ts}"),
            vec!["*.py".to_string(), "*.ts".to_string()]
        );
        assert_eq!(
            expand_glob_braces("src/{a,b,c}.rs"),
            vec![
                "src/a.rs".to_string(),
                "src/b.rs".to_string(),
                "src/c.rs".to_string()
            ]
        );
        assert_eq!(expand_glob_braces("*.rs"), vec!["*.rs".to_string()]);
        assert_eq!(expand_glob_braces("*.{rs}"), vec!["*.rs".to_string()]);
    }

    #[test]
    fn write_file_returns_structured_diff_metadata() {
        let root = std::env::temp_dir().join(format!("kivio_write_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("mkdir");
        let workspace = NativeToolWorkspace::project(
            "proj_test".to_string(),
            "Test".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );

        let result = write_file(
            &workspace,
            &json!({
                "path": "hello.txt",
                "content": "alpha\nbeta\n"
            }),
        )
        .expect("write");

        assert_eq!(result.operation, "create");
        assert_eq!(result.resolved_path.as_deref(), Some("hello.txt"));
        assert_eq!(result.files[0].path, "hello.txt");
        assert_eq!(result.additions, 2);
        assert_eq!(result.removals, 0);
        assert!(result.diff.contains("+++ b/hello.txt"));
        assert!(result.diff.contains("+alpha"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn project_workspace_rejects_escape_paths() {
        let root = std::env::temp_dir().join(format!("kivio_project_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("mkdir");
        let workspace = NativeToolWorkspace::project(
            "proj_test".to_string(),
            "Test".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );

        let err = read_file(&workspace, &json!({ "path": "../secret.txt" })).unwrap_err();
        assert!(err.contains(".."));

        // Explicit absolute paths outside the project are allowed for reads,
        // matching non-project conversations.
        let outside = std::env::temp_dir().join(format!("kivio_outside_{}", uuid::Uuid::new_v4()));
        fs::write(&outside, "secret").expect("write outside");
        let result = read_file(&workspace, &json!({ "path": outside.to_string_lossy() }))
            .expect("absolute read outside project");
        assert_eq!(result.content, "secret");

        let _ = fs::remove_file(outside);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn project_workspace_no_boundary_writes_anywhere() {
        let root = std::env::temp_dir().join(format!("kivio_project_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("mkdir");
        let workspace = NativeToolWorkspace::project(
            "proj_test".to_string(),
            "Test".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );

        // No-boundary: an explicit absolute path outside the project writes fine.
        let home = super::super::user_home_dir().expect("home");
        let dir = home.join(format!(".kivio_escape_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir home target");
        let target = dir.join("note.html");
        write_file(
            &workspace,
            &json!({ "path": target.to_string_lossy(), "content": "<html></html>" }),
        )
        .expect("explicit absolute write outside project");
        assert_eq!(
            fs::read_to_string(&target).expect("read back"),
            "<html></html>"
        );

        // No-boundary: a relative `..` path now escapes the project root instead
        // of being rejected (it resolves against the parent of the root).
        let escape_name = format!("kivio_escape_{}.txt", uuid::Uuid::new_v4());
        write_file(
            &workspace,
            &json!({ "path": format!("../{escape_name}"), "content": "x" }),
        )
        .expect("relative `..` write is allowed under no-boundary");
        let escaped = root.parent().expect("root parent").join(&escape_name);
        assert_eq!(fs::read_to_string(&escaped).expect("read escaped"), "x");

        let _ = fs::remove_file(&escaped);
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn glob_files_rejects_path_like_patterns() {
        let root = std::env::temp_dir().join(format!("kivio_glob_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("mkdir");
        fs::write(root.join("package.json"), "{}").expect("write package");
        let workspace = NativeToolWorkspace::project(
            "proj_test".to_string(),
            "Test".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );

        let absolute_err = glob_files(
            &workspace,
            &json!({
                "pattern": format!("{}/*.json", root.display())
            }),
        )
        .unwrap_err();
        assert!(absolute_err.contains("relative"));

        let parent_err = glob_files(
            &workspace,
            &json!({
                "pattern": "../*.json"
            }),
        )
        .unwrap_err();
        assert!(parent_err.contains(".."));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn write_file_overwrites_non_utf8_file_with_warning() {
        let root = std::env::temp_dir().join(format!("kivio_binary_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("mkdir");
        fs::write(root.join("bin.dat"), [0xffu8, 0xfe, 0x01]).expect("write binary");
        let workspace = NativeToolWorkspace::project(
            "proj_test".to_string(),
            "Test".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );

        let result = write_file(
            &workspace,
            &json!({
                "path": "bin.dat",
                "content": "clean\n"
            }),
        )
        .expect("overwrite non-utf8 file");

        assert_eq!(result.operation, "overwrite");
        assert!(result.diff.is_empty());
        assert_eq!(result.additions, 0);
        assert_eq!(result.removals, 0);
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.contains("not valid UTF-8")));
        assert_eq!(
            fs::read_to_string(root.join("bin.dat")).expect("read"),
            "clean\n"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn edit_file_multiple_edits_report_exact_stats_with_multiple_hunks() {
        let root = std::env::temp_dir().join(format!("kivio_hunks_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("mkdir");
        let mut lines = vec!["needle old".to_string()];
        for i in 0..20 {
            lines.push(format!("unchanged line {i}"));
        }
        lines.push("needle old".to_string());
        let content = format!("{}\n", lines.join("\n"));
        fs::write(root.join("scatter.txt"), &content).expect("write scatter");
        let workspace = NativeToolWorkspace::project(
            "proj_test".to_string(),
            "Test".to_string(),
            Some(root.to_string_lossy().into_owned()),
        );

        // Two scattered occurrences, each disambiguated with one line of context.
        let result = edit_file(
            &workspace,
            &json!({
                "path": "scatter.txt",
                "edits": [
                    { "old_string": "needle old\nunchanged line 0", "new_string": "needle new\nunchanged line 0" },
                    { "old_string": "unchanged line 19\nneedle old", "new_string": "unchanged line 19\nneedle new" }
                ]
            }),
        )
        .expect("two scattered edits");

        assert_eq!(result.additions, 2);
        assert_eq!(result.removals, 2);
        assert_eq!(result.diff.matches("@@ -").count(), 2);
        let removed_lines = result
            .diff
            .lines()
            .filter(|line| line.starts_with('-') && !line.starts_with("---"))
            .count();
        let added_lines = result
            .diff
            .lines()
            .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
            .count();
        assert_eq!(removed_lines, 2);
        assert_eq!(added_lines, 2);

        let _ = fs::remove_dir_all(root);
    }
}
