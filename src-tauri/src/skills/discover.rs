use std::{
    fs,
    path::{Path, PathBuf},
};

use tauri::{AppHandle, Manager};

use super::{
    parse::parse_skill_record,
    types::{SkillFileEntry, SkillFileKind, SkillRegistry},
};

const MAX_SCAN_DEPTH: usize = 6;

const SKIP_DIR_NAMES: &[&str] = &[".git", "node_modules", ".svn", ".hg"];

struct SkillScanRoot {
    path: PathBuf,
    source: &'static str,
}

pub fn user_skills_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("app_data_dir unavailable: {err}"))?
        .join("skills");
    fs::create_dir_all(&dir).map_err(|err| format!("create skills dir failed: {err}"))?;
    Ok(dir)
}

fn bundled_skills_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .resource_dir()
        .ok()
        .map(|dir| dir.join("skills"))
        .filter(|dir| dir.is_dir())
}

fn scan_root_entries(
    app: &AppHandle,
    extra_paths: &[String],
) -> Result<Vec<SkillScanRoot>, String> {
    let mut roots = Vec::new();
    if let Some(path) = bundled_skills_dir(app) {
        roots.push(SkillScanRoot {
            path,
            source: "builtin",
        });
    }
    roots.push(SkillScanRoot {
        path: user_skills_dir(app)?,
        source: "user",
    });
    // 仅「已启用」插件的附属 Skill 进入扫描；关闭插件后立刻从 registry 消失。
    for path in crate::plugins::enabled_skill_roots() {
        roots.push(SkillScanRoot {
            path,
            source: "plugin",
        });
    }
    append_external_roots(&mut roots, extra_paths);
    Ok(roots)
}

/// Resolve the same per-user skills directory the GUI uses
/// (`<app_data_dir>/skills`), but WITHOUT a Tauri `AppHandle`. `app_data_dir`
/// is derived from the `directories` crate against the bundle identifier
/// `com.zmair.kivio`, via `crate::app_data::app_data_dir`.
///
/// Returns `None` only when no home/data directory can be determined. The
/// directory is created if missing so the user has a place to drop skills.
pub fn user_skills_dir_headless() -> Option<PathBuf> {
    let dir = crate::app_data::app_data_dir()?.join("skills");
    // Best-effort create; ignore failures (read-only / permission) — discovery
    // simply finds nothing rather than erroring.
    let _ = fs::create_dir_all(&dir);
    Some(dir)
}

/// Headless variant of [`scan_root_entries`]: resolves skill roots without an
/// `AppHandle`. Built-in (bundled) skills are resolved relative to the running
/// executable's resource layout when present; the user skills dir comes from
/// [`user_skills_dir_headless`]; `extra_paths` are appended as external roots.
fn scan_root_entries_headless(extra_paths: &[String]) -> Vec<SkillScanRoot> {
    let mut roots = Vec::new();
    if let Some(path) = bundled_skills_dir_headless() {
        roots.push(SkillScanRoot {
            path,
            source: "builtin",
        });
    }
    if let Some(path) = user_skills_dir_headless() {
        roots.push(SkillScanRoot {
            path,
            source: "user",
        });
    }
    for path in crate::plugins::enabled_skill_roots() {
        roots.push(SkillScanRoot {
            path,
            source: "plugin",
        });
    }
    append_external_roots(&mut roots, extra_paths);
    roots
}

/// Best-effort location of bundled skills next to the executable when running
/// headless (no Tauri `resource_dir`). Checks `<exe_dir>/skills` and, for the
/// macOS app-bundle layout, `<exe_dir>/../Resources/skills`. Returns `None`
/// when neither exists (e.g. plain `cargo run`), which is fine — the CLI then
/// surfaces only user skills.
fn bundled_skills_dir_headless() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    bundled_skills_dir_from_exe(&exe)
}

/// Pure path logic for [`bundled_skills_dir_headless`]: given an executable
/// path, canonicalize it (so a PATH symlink resolves to the real binary's
/// directory) and probe the bundled-skills candidates next to it.
///
/// `current_exe()` does not resolve symlinks on macOS, so a PATH symlink
/// (e.g. `~/.cargo/bin/kivio-code -> target/debug/kivio-code`) would otherwise
/// point `exe_dir` at the symlink's directory — where `skills/` doesn't exist —
/// and built-in skills would silently go missing. Canonicalizing reaches the
/// real binary's directory.
fn bundled_skills_dir_from_exe(exe: &Path) -> Option<PathBuf> {
    let real = fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    let exe_dir = real.parent()?;
    let candidates = [
        exe_dir.join("skills"),
        exe_dir.join("..").join("Resources").join("skills"),
    ];
    candidates.into_iter().find(|dir| dir.is_dir())
}

fn append_external_roots(roots: &mut Vec<SkillScanRoot>, extra_paths: &[String]) {
    roots.extend(extra_paths.iter().map(PathBuf::from).filter_map(|path| {
        path.is_dir().then_some(SkillScanRoot {
            path,
            source: "external",
        })
    }));
}

pub fn build_registry(app: &AppHandle, extra_paths: &[String]) -> Result<SkillRegistry, String> {
    build_registry_inner(app, extra_paths, true)
}

pub fn build_registry_metadata(
    app: &AppHandle,
    extra_paths: &[String],
) -> Result<SkillRegistry, String> {
    build_registry_inner(app, extra_paths, false)
}

fn build_registry_inner(
    app: &AppHandle,
    extra_paths: &[String],
    include_files: bool,
) -> Result<SkillRegistry, String> {
    let roots = scan_root_entries(app, extra_paths)?;
    Ok(build_registry_from_roots(roots, include_files))
}

/// Headless registry builder (no `AppHandle`): discovers user-dir + bundled +
/// `extra_paths` skills exactly like [`build_registry`], indexing skill files so
/// the activated skill's `<skill_resources>` list is populated. Used by the
/// `kivio-code` CLI.
pub fn build_registry_headless(extra_paths: &[String]) -> SkillRegistry {
    build_registry_from_roots(scan_root_entries_headless(extra_paths), true)
}

fn build_registry_from_roots(roots: Vec<SkillScanRoot>, include_files: bool) -> SkillRegistry {
    let mut registry = SkillRegistry::default();
    for root in roots {
        if let Err(err) =
            collect_skill_files(&root.path, 0, &mut registry, root.source, include_files)
        {
            registry
                .warnings
                .push(format!("Scan {} failed: {err}", root.path.display()));
        }
    }
    dedup_records(&mut registry.records, &mut registry.warnings);
    registry
        .records
        .sort_by(|a, b| a.meta.name.to_lowercase().cmp(&b.meta.name.to_lowercase()));
    registry
}

fn dedup_records(records: &mut Vec<super::types::SkillRecord>, warnings: &mut Vec<String>) {
    let mut seen = std::collections::HashMap::<String, usize>::new();
    let mut out: Vec<super::types::SkillRecord> = Vec::new();
    for record in records.drain(..) {
        let key = record.meta.id.clone();
        if let Some(index) = seen.get(&key) {
            warnings.push(format!(
                "Skill {} shadowed duplicate id {}",
                out[*index].meta.name, record.meta.name
            ));
            continue;
        }
        seen.insert(key, out.len());
        out.push(record);
    }
    *records = out;
}

fn collect_skill_files(
    root: &Path,
    depth: usize,
    registry: &mut SkillRegistry,
    source: &str,
    include_files: bool,
) -> Result<(), String> {
    if depth > MAX_SCAN_DEPTH || !root.is_dir() {
        return Ok(());
    }

    let skill_md = root.join("SKILL.md");
    if skill_md.is_file() {
        match load_skill_at(&skill_md, source, include_files) {
            Ok(record) => registry.records.push(record),
            Err(err) => registry
                .warnings
                .push(format!("Parse skill {} failed: {err}", skill_md.display())),
        }
        return Ok(());
    }

    if depth == MAX_SCAN_DEPTH {
        return Ok(());
    }

    let entries = fs::read_dir(root).map_err(|err| format!("read skills dir failed: {err}"))?;
    for entry in entries {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if SKIP_DIR_NAMES.contains(&name) {
            continue;
        }
        collect_skill_files(&path, depth + 1, registry, source, include_files)?;
    }
    Ok(())
}

fn load_skill_at(
    skill_md_path: &Path,
    source: &str,
    include_files: bool,
) -> Result<super::types::SkillRecord, String> {
    let raw = fs::read_to_string(skill_md_path)
        .map_err(|err| format!("Read skill {} failed: {err}", skill_md_path.display()))?;
    let base_dir = skill_md_path
        .parent()
        .ok_or_else(|| "Skill path has no parent directory".to_string())?;
    let files = if include_files {
        index_skill_files(base_dir)?
    } else {
        Vec::new()
    };
    let mut warnings = Vec::new();
    parse_skill_record(skill_md_path, &raw, source, files, &mut warnings)
}

pub fn index_skill_files(base_dir: &Path) -> Result<Vec<SkillFileEntry>, String> {
    let mut files = Vec::new();
    walk_files(base_dir, base_dir, 0, &mut files)?;
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(files)
}

fn walk_files(
    base_dir: &Path,
    current: &Path,
    depth: usize,
    out: &mut Vec<SkillFileEntry>,
) -> Result<(), String> {
    if depth > MAX_SCAN_DEPTH {
        return Ok(());
    }
    let entries = fs::read_dir(current).map_err(|err| err.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if path.is_dir() {
            if SKIP_DIR_NAMES.iter().any(|skip| *skip == name.as_ref()) {
                continue;
            }
            walk_files(base_dir, &path, depth + 1, out)?;
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let relative = path
            .strip_prefix(base_dir)
            .map_err(|err| err.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if relative == "SKILL.md" {
            continue;
        }
        let size_bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        out.push(SkillFileEntry {
            relative_path: relative.clone(),
            kind: classify_file(&relative),
            size_bytes,
        });
    }
    Ok(())
}

fn classify_file(relative_path: &str) -> SkillFileKind {
    let normalized = relative_path.replace('\\', "/");
    if normalized.starts_with("scripts/") {
        return SkillFileKind::Script;
    }
    if normalized.starts_with("references/") || normalized.ends_with(".md") {
        return SkillFileKind::Reference;
    }
    if normalized.starts_with("assets/") {
        return SkillFileKind::Asset;
    }
    SkillFileKind::Other
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_skill_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kivio-skill-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            r#"---
name: skill-test
description: Test skill.
---

# Skill body
"#,
        )
        .unwrap();
        fs::write(dir.join("scripts").join("run.sh"), "echo ok").unwrap();
        dir
    }

    #[test]
    fn metadata_registry_skips_bundled_file_indexing() {
        let dir = temp_skill_dir();
        let skill_md = dir.join("SKILL.md");

        let metadata_record = load_skill_at(&skill_md, "user", false).unwrap();
        assert!(metadata_record.meta.files.is_empty());

        let full_record = load_skill_at(&skill_md, "user", true).unwrap();
        assert_eq!(full_record.meta.files.len(), 1);
        assert_eq!(full_record.meta.files[0].relative_path, "scripts/run.sh");

        fs::remove_dir_all(dir).unwrap();
    }

    /// Regression test for non-deterministic bundled-skill discovery: a PATH
    /// symlink to the real binary must still resolve `skills/` next to the real
    /// binary (macOS `current_exe()` does not resolve symlinks).
    #[cfg(unix)]
    #[test]
    fn bundled_skills_dir_resolves_through_path_symlink() {
        let base =
            std::env::temp_dir().join(format!("kivio-skill-symlink-{}", uuid::Uuid::new_v4()));
        // Real binary lives in `bin_dir`, with `bin_dir/skills/<name>/SKILL.md`.
        let bin_dir = base.join("real");
        let skills_dir = bin_dir.join("skills").join("demo");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(skills_dir.join("SKILL.md"), "---\nname: demo\n---\n").unwrap();
        let real_bin = bin_dir.join("kivio-code");
        fs::write(&real_bin, "#!/bin/sh\n").unwrap();

        // Symlink in a separate dir (e.g. ~/.cargo/bin) with NO skills/ alongside.
        let link_dir = base.join("path");
        fs::create_dir_all(&link_dir).unwrap();
        let link = link_dir.join("kivio-code");
        std::os::unix::fs::symlink(&real_bin, &link).unwrap();

        // Resolving via the symlink must reach the real binary's skills dir.
        let found = bundled_skills_dir_from_exe(&link).unwrap();
        assert_eq!(
            fs::canonicalize(found).unwrap(),
            fs::canonicalize(bin_dir.join("skills")).unwrap()
        );

        // And of course resolving via the real path also works.
        assert!(bundled_skills_dir_from_exe(&real_bin).is_some());

        fs::remove_dir_all(base).unwrap();
    }
}
