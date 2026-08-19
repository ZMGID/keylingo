use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use super::user_home_dir;
use crate::mcp::types::ChatToolArtifact;
use base64::{engine::general_purpose, Engine as _};

/// Legacy per-conversation output root from versions before ordinary chats used
/// a unified workbench. Nothing writes here anymore; existing data is migrated
/// lazily or removed with its conversation.
const OUTPUTS_ROOT: &str = "Kivio/outputs";
/// Legacy ephemeral exports tree from prior versions (`run_python` used to write
/// here under `<conversation>/<message>/`). Still GC'd at startup so old runs go
/// away; nothing writes here anymore.
const LEGACY_RUNS_ROOT: &str = "Kivio/runs";
const LEGACY_RUNS_RETENTION_DAYS: u64 = 7;
const MAX_EXPORT_FILE_BYTES: u64 = 12 * 1024 * 1024;
const MAX_EXPORT_FILES_PER_RUN: usize = 16;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxExportContext {
    pub conversation_id: String,
    pub message_id: String,
    pub tool_call_id: Option<String>,
    /// Actual default output directory for this tool call (conversation
    /// workbench or bound project root). This is not a confinement boundary.
    pub output_directory: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxExportedArtifact {
    pub artifact_index: usize,
    pub path: PathBuf,
}

fn sanitize_path_segment(raw: &str) -> String {
    let sanitized = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let trimmed = sanitized.trim_matches('_');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

fn outputs_root() -> Result<PathBuf, String> {
    Ok(user_home_dir()?.join(OUTPUTS_ROOT))
}

/// True when `path` resolves inside `directory`. Used only to decide whether a
/// successful ordinary-chat write should be surfaced as a downloadable card.
pub fn path_under_directory(directory: &Path, path: &Path) -> bool {
    let dir_canon = canonicalize_lenient(directory);
    let path_canon = canonicalize_lenient(path);
    path_canon.starts_with(&dir_canon)
}

/// Canonicalize a path, falling back to canonicalizing the nearest existing
/// ancestor and re-appending the missing tail (so a not-yet-created file still
/// resolves symlinks/`..` in its parent chain).
fn canonicalize_lenient(path: &Path) -> PathBuf {
    if let Ok(canon) = fs::canonicalize(path) {
        return canon;
    }
    let mut missing = Vec::new();
    let mut current = path;
    loop {
        if let Ok(canon) = fs::canonicalize(current) {
            let mut resolved = canon;
            for name in missing.iter().rev() {
                resolved.push(name);
            }
            return resolved;
        }
        match current.parent() {
            Some(parent) => {
                if let Some(name) = current.file_name() {
                    missing.push(name.to_os_string());
                }
                current = parent;
            }
            None => return path.to_path_buf(),
        }
    }
}

/// Resolve a backend-generated artifact path for open/reveal actions. Artifact
/// cards already carry the path produced by a completed tool call; the workbench
/// itself is not a sandbox, so explicitly requested output paths may be anywhere.
pub fn resolve_sandbox_export_file_path(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Generated file path is empty".to_string());
    }
    let full = Path::new(trimmed);
    if !full.is_absolute() {
        return Err("Generated file path must be absolute".to_string());
    }
    if !full.is_file() {
        return Err("Generated file does not exist".to_string());
    }
    fs::canonicalize(full).map_err(|err| format!("Resolve generated file path failed: {err}"))
}

pub fn merge_directory_without_overwrite(source: &Path, target: &Path) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }
    if !source.is_dir() {
        return Err(format!(
            "Migration source is not a directory: {}",
            source.display()
        ));
    }
    preflight_directory_merge(source, target)?;
    merge_directory_entries(source, target)
}

pub(crate) fn preflight_directory_merge(source: &Path, target: &Path) -> Result<(), String> {
    if target.exists() && !target.is_dir() {
        return Err(format!(
            "Workspace migration conflict: target is not a directory: {}",
            target.display()
        ));
    }
    for entry in
        fs::read_dir(source).map_err(|err| format!("Read migration source failed: {err}"))?
    {
        let entry = entry.map_err(|err| format!("Read migration entry failed: {err}"))?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if !target_path.exists() {
            continue;
        }
        if source_path.is_dir() && target_path.is_dir() {
            preflight_directory_merge(&source_path, &target_path)?;
        } else {
            return Err(format!(
                "Workspace migration conflict; refusing to overwrite: {}",
                target_path.display()
            ));
        }
    }
    Ok(())
}

fn merge_directory_entries(source: &Path, target: &Path) -> Result<(), String> {
    if !target.exists() {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("Create migration target parent failed: {err}"))?;
        }
        if fs::rename(source, target).is_ok() {
            return Ok(());
        }
        copy_directory_tree(source, target)?;
        fs::remove_dir_all(source)
            .map_err(|err| format!("Remove migrated source failed: {err}"))?;
        return Ok(());
    }

    fs::create_dir_all(target).map_err(|err| format!("Create migration target failed: {err}"))?;
    let entries = fs::read_dir(source)
        .map_err(|err| format!("Read migration source failed: {err}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Read migration entry failed: {err}"))?;
    for entry in entries {
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if source_path.is_dir() {
            merge_directory_entries(&source_path, &target_path)?;
        } else if fs::rename(&source_path, &target_path).is_err() {
            fs::copy(&source_path, &target_path)
                .map_err(|err| format!("Copy migrated file failed: {err}"))?;
            fs::remove_file(&source_path)
                .map_err(|err| format!("Remove migrated source file failed: {err}"))?;
        }
    }
    fs::remove_dir(source).map_err(|err| format!("Remove empty migration source failed: {err}"))?;
    Ok(())
}

fn copy_directory_tree(source: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(target).map_err(|err| format!("Create migration target failed: {err}"))?;
    for entry in
        fs::read_dir(source).map_err(|err| format!("Read migration source failed: {err}"))?
    {
        let entry = entry.map_err(|err| format!("Read migration entry failed: {err}"))?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if source_path.is_dir() {
            copy_directory_tree(&source_path, &target_path)?;
        } else {
            fs::copy(&source_path, &target_path)
                .map_err(|err| format!("Copy migrated file failed: {err}"))?;
        }
    }
    Ok(())
}

pub fn legacy_outputs_dir(conversation_id: &str) -> Result<PathBuf, String> {
    Ok(outputs_root()?.join(sanitize_path_segment(conversation_id)))
}

fn decode_data_url(data_url: &str) -> Result<Vec<u8>, String> {
    let payload = data_url
        .split_once(',')
        .map(|(_, data)| data)
        .ok_or_else(|| "Invalid artifact data URL".to_string())?;
    general_purpose::STANDARD
        .decode(payload)
        .map_err(|err| format!("Decode artifact failed: {err}"))
}

fn sanitize_export_filename(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("output");
    // Keep Unicode letters/digits (CJK, etc.). NTFS/APFS already accept them.
    // Only strip path separators, Windows reserved punctuation, and other
    // non-identifier junk — `is_ascii_alphanumeric` was collapsing 销售报表.xlsx
    // into ________.xlsx on the no-workspace Pyodide export path.
    let sanitized = base
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let trimmed = sanitized.trim_matches(['.', '_']);
    if trimmed.is_empty() {
        "output.bin".to_string()
    } else {
        trimmed.to_string()
    }
}

fn unique_export_path(dir: &Path, filename: &str) -> PathBuf {
    let mut candidate = dir.join(filename);
    if !candidate.exists() {
        return candidate;
    }
    let stem = Path::new(filename)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("output");
    let ext = Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();
    for index in 2..=99 {
        candidate = dir.join(format!("{stem}-{index}{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem}-{}", uuid::Uuid::new_v4()))
}

/// Export Pyodide artifacts into the current default workbench. The caller
/// resolves that directory from the ordinary conversation or bound project.
pub fn export_sandbox_artifacts(
    ctx: &SandboxExportContext,
    artifacts: &[ChatToolArtifact],
) -> Result<Vec<SandboxExportedArtifact>, String> {
    if artifacts.is_empty() {
        return Ok(Vec::new());
    }

    let export_dir = &ctx.output_directory;
    fs::create_dir_all(export_dir).map_err(|err| {
        format!(
            "Create artifact output directory failed ({}): {err}",
            export_dir.display()
        )
    })?;

    let mut exported = Vec::new();
    for (artifact_index, artifact) in artifacts.iter().enumerate().take(MAX_EXPORT_FILES_PER_RUN) {
        if artifact.data_url.trim().is_empty() {
            continue;
        }
        let bytes = decode_data_url(&artifact.data_url)?;
        if bytes.is_empty() {
            continue;
        }
        if bytes.len() as u64 > MAX_EXPORT_FILE_BYTES {
            continue;
        }
        let filename = sanitize_export_filename(&artifact.name);
        let path = unique_export_path(&export_dir, &filename);
        fs::write(&path, &bytes).map_err(|err| format!("Write sandbox export failed: {err}"))?;
        exported.push(SandboxExportedArtifact {
            artifact_index,
            path,
        });
    }

    Ok(exported)
}

/// Guess a MIME type from a file extension. Used to label generated file cards
/// when no explicit MIME type is known.
/// Falls back to `application/octet-stream`.
pub fn guess_mime_from_name(name: &str) -> String {
    let ext = Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    let mime = match ext.as_str() {
        "txt" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "xml" => "application/xml",
        "yaml" | "yml" => "application/x-yaml",
        "js" | "mjs" | "cjs" => "text/javascript",
        "ts" | "tsx" => "text/plain",
        "css" => "text/css",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xls" => "application/vnd.ms-excel",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "doc" => "application/msword",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    };
    mime.to_string()
}

/// Build a [`ChatToolArtifact`] from a file already written to the
/// ordinary conversation workbench. The `data_url` (read back) is only populated for files at
/// or under the export size cap so previews/downloads of small files work; for
/// larger files only `path`/`size_bytes` are set (the UI can still open it).
pub fn build_delivery_artifact_for_path(path: &Path) -> Result<ChatToolArtifact, String> {
    let metadata = fs::metadata(path).map_err(|err| format!("Stat delivery file failed: {err}"))?;
    let size_bytes = metadata.len();
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("output")
        .to_string();
    let mime_type = guess_mime_from_name(&name);
    let data_url = if size_bytes <= MAX_EXPORT_FILE_BYTES {
        let bytes = fs::read(path).map_err(|err| format!("Read delivery file failed: {err}"))?;
        Some(format!(
            "data:{mime_type};base64,{}",
            general_purpose::STANDARD.encode(&bytes)
        ))
    } else {
        None
    };
    Ok(ChatToolArtifact {
        id: None,
        name,
        mime_type,
        data_url: data_url.unwrap_or_default(),
        size_bytes: Some(size_bytes),
        path: Some(path.display().to_string()),
    })
}

pub fn format_exported_paths(exports: &[SandboxExportedArtifact]) -> String {
    if exports.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        "generated files (saved to the current workbench and registered as artifacts; use their returned artifact IDs with present_artifacts to show selected files in chat):"
            .to_string(),
    ];
    for export in exports {
        lines.push(format!("- {}", export.path.display()));
    }
    lines.join("\n")
}

pub fn format_export_error(err: &str) -> String {
    format!("export warning: failed to save files to the current workbench: {err}")
}

/// Remove legacy outputs/runs for one conversation when the chat is deleted.
pub fn remove_sandbox_exports_for_conversation(conversation_id: &str) {
    let home = match user_home_dir() {
        Ok(home) => home,
        Err(_) => return,
    };
    let path = home
        .join(OUTPUTS_ROOT)
        .join(sanitize_path_segment(conversation_id));
    if path.is_dir() {
        let _ = fs::remove_dir_all(path);
    }
    // Also sweep any leftover legacy ephemeral exports for this conversation.
    let legacy = home
        .join(LEGACY_RUNS_ROOT)
        .join(sanitize_path_segment(conversation_id));
    if legacy.is_dir() {
        let _ = fs::remove_dir_all(legacy);
    }
}

fn dir_modified_at(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

fn remove_dir_if_stale(path: &Path, cutoff: SystemTime, removed: &mut u32, bytes_freed: &mut u64) {
    let modified = match dir_modified_at(path) {
        Some(value) => value,
        None => return,
    };
    if modified >= cutoff {
        return;
    }
    let size = dir_size(path);
    if fs::remove_dir_all(path).is_ok() {
        *removed += 1;
        *bytes_freed += size;
    }
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return 0,
    };
    for entry in entries.flatten() {
        let entry_path = entry.path();
        if entry_path.is_dir() {
            total += dir_size(&entry_path);
        } else if let Ok(meta) = entry.metadata() {
            total += meta.len();
        }
    }
    total
}

/// Startup GC for the legacy ephemeral exports tree (`~/Kivio/runs/`) from
/// prior versions. The legacy outputs tree is preserved for lazy migration into
/// conversation workbenches and is only removed with its conversation. Old runs
/// are eventually removed; nothing writes to `~/Kivio/runs/` anymore.
pub fn cleanup_stale_sandbox_exports() {
    let retention = Duration::from_secs(LEGACY_RUNS_RETENTION_DAYS * 24 * 60 * 60);
    let home = match user_home_dir() {
        Ok(home) => home,
        Err(err) => {
            eprintln!("[sandbox-export-cleanup] home dir unavailable: {err}");
            return;
        }
    };

    let mut removed = 0u32;
    let mut bytes_freed = 0u64;

    let runs_root = home.join(LEGACY_RUNS_ROOT);
    if runs_root.is_dir() {
        let cutoff = SystemTime::now()
            .checked_sub(retention)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if let Ok(conversations) = fs::read_dir(&runs_root) {
            for conv_entry in conversations.flatten() {
                let conv_path = conv_entry.path();
                if !conv_path.is_dir() {
                    continue;
                }
                let Ok(messages) = fs::read_dir(&conv_path) else {
                    continue;
                };
                for msg_entry in messages.flatten() {
                    let msg_path = msg_entry.path();
                    if msg_path.is_dir() {
                        remove_dir_if_stale(&msg_path, cutoff, &mut removed, &mut bytes_freed);
                    }
                }
                if fs::read_dir(&conv_path)
                    .map(|mut dir| dir.next().is_none())
                    .unwrap_or(false)
                {
                    let _ = fs::remove_dir(&conv_path);
                }
            }
        }
    }

    if removed > 0 {
        eprintln!(
            "[sandbox-export-cleanup] removed {removed} stale legacy run folder(s), freed {} KB",
            bytes_freed / 1024
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("kivio_{label}_{}", uuid::Uuid::new_v4().simple()))
    }

    #[test]
    fn export_uses_supplied_workbench_and_never_prunes_existing_files() {
        let dir = temp_dir("artifact_workspace");
        fs::create_dir_all(&dir).expect("mkdir");
        for index in 0..24 {
            fs::write(dir.join(format!("existing_{index}.txt")), "keep").expect("seed");
        }
        let png = general_purpose::STANDARD.encode([137u8, 80, 78, 71, 13, 10, 26, 10]);
        let ctx = SandboxExportContext {
            conversation_id: "conv_test".to_string(),
            message_id: "msg_test".to_string(),
            tool_call_id: None,
            output_directory: dir.clone(),
        };
        let exported = export_sandbox_artifacts(
            &ctx,
            &[ChatToolArtifact {
                id: None,
                name: "chart.png".to_string(),
                mime_type: "image/png".to_string(),
                data_url: format!("data:image/png;base64,{png}"),
                size_bytes: Some(8),
                path: None,
            }],
        )
        .expect("export");
        assert_eq!(exported.len(), 1);
        assert!(exported[0].path.starts_with(&dir));
        assert!(!dir.join("meta.json").exists());
        for index in 0..24 {
            assert!(dir.join(format!("existing_{index}.txt")).exists());
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn directory_membership_is_lenient_but_does_not_allow_escape() {
        let dir = temp_dir("membership");
        fs::create_dir_all(&dir).expect("mkdir");
        assert!(path_under_directory(&dir, &dir.join("nested/file.txt")));
        assert!(!path_under_directory(&dir, &dir.join("../outside.txt")));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn merge_directory_refuses_to_overwrite_conflicts() {
        let root = temp_dir("merge_conflict");
        let source = root.join("source");
        let target = root.join("target");
        fs::create_dir_all(&source).expect("source");
        fs::create_dir_all(&target).expect("target");
        fs::write(source.join("same.txt"), "source").expect("source file");
        fs::write(target.join("same.txt"), "target").expect("target file");
        let err = merge_directory_without_overwrite(&source, &target).expect_err("conflict");
        assert!(err.contains("refusing to overwrite"));
        assert_eq!(
            fs::read_to_string(source.join("same.txt")).unwrap(),
            "source"
        );
        assert_eq!(
            fs::read_to_string(target.join("same.txt")).unwrap(),
            "target"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn merge_directory_combines_non_conflicting_trees() {
        let root = temp_dir("merge_ok");
        let source = root.join("source");
        let target = root.join("target");
        fs::create_dir_all(source.join("nested")).expect("source");
        fs::create_dir_all(&target).expect("target");
        fs::write(source.join("nested/new.txt"), "new").expect("new");
        fs::write(target.join("keep.txt"), "keep").expect("keep");
        merge_directory_without_overwrite(&source, &target).expect("merge");
        assert!(!source.exists());
        assert_eq!(
            fs::read_to_string(target.join("nested/new.txt")).unwrap(),
            "new"
        );
        assert_eq!(fs::read_to_string(target.join("keep.txt")).unwrap(), "keep");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sanitize_export_filename_keeps_unicode_letters() {
        assert_eq!(sanitize_export_filename("销售报表.xlsx"), "销售报表.xlsx");
        assert_eq!(sanitize_export_filename("Q1销售.xlsx"), "Q1销售.xlsx");
        assert_eq!(
            sanitize_export_filename("C25月度汇总.xlsx"),
            "C25月度汇总.xlsx"
        );
        assert_eq!(sanitize_export_filename("chart.png"), "chart.png");
    }

    #[test]
    fn sanitize_export_filename_strips_path_and_reserved_chars() {
        assert_eq!(sanitize_export_filename("../报表.xlsx"), "报表.xlsx");
        assert_eq!(sanitize_export_filename("a<>b?.xlsx"), "a__b_.xlsx");
        assert_eq!(sanitize_export_filename("___"), "output.bin");
    }

    #[test]
    fn export_preserves_cjk_filename_on_disk() {
        let dir = temp_dir("cjk_export");
        fs::create_dir_all(&dir).expect("mkdir");
        let png = general_purpose::STANDARD.encode([137u8, 80, 78, 71, 13, 10, 26, 10]);
        let ctx = SandboxExportContext {
            conversation_id: "conv_cjk".to_string(),
            message_id: "msg_cjk".to_string(),
            tool_call_id: None,
            output_directory: dir.clone(),
        };
        let exported = export_sandbox_artifacts(
            &ctx,
            &[ChatToolArtifact {
                id: None,
                name: "销售报表.xlsx".to_string(),
                mime_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                    .to_string(),
                data_url: format!("data:image/png;base64,{png}"),
                size_bytes: Some(8),
                path: None,
            }],
        )
        .expect("export");
        assert_eq!(exported.len(), 1);
        assert_eq!(
            exported[0].path.file_name().and_then(|value| value.to_str()),
            Some("销售报表.xlsx")
        );
        assert!(dir.join("销售报表.xlsx").is_file());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn generated_file_resolver_accepts_existing_absolute_paths_outside_legacy_outputs() {
        let dir = temp_dir("generated_open");
        fs::create_dir_all(&dir).expect("mkdir");
        let file = dir.join("report.txt");
        fs::write(&file, "ok").expect("write");
        let resolved = resolve_sandbox_export_file_path(&file.to_string_lossy()).expect("resolve");
        assert_eq!(resolved, fs::canonicalize(&file).unwrap());
        let _ = fs::remove_dir_all(dir);
    }
}
