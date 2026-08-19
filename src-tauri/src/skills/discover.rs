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
const MAX_PROJECT_WALK: usize = 24;

const SKIP_DIR_NAMES: &[&str] = &[".git", "node_modules", ".svn", ".hg"];

struct SkillScanRoot {
    path: PathBuf,
    source: &'static str,
}

/// 共享 Agent Skills 目录：`~/.agents/skills`（Codex / OpenCode / Pi 等共用）。
/// 只扫描，Kivio 不往这里写、也不从这里删。
pub fn home_agents_skills_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|base| base.home_dir().join(".agents").join("skills"))
}

/// Kivio 自己的个人技能目录：`~/.kivio/skills`（对齐 `~/.claude/skills` / `~/.codex/skills`）。
pub fn kivio_skills_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|base| base.home_dir().join(".kivio").join("skills"))
}

/// 旧版个人目录：`{app_data}/skills`。只扫已有内容，不再写入。
pub fn legacy_app_data_skills_dir() -> Option<PathBuf> {
    crate::app_data::app_data_dir().map(|dir| dir.join("skills"))
}

/// 个人 Skill 写入目录（导入 / 打开文件夹）：`~/.kivio/skills`。
pub fn user_skills_dir(_app: &AppHandle) -> Result<PathBuf, String> {
    let dir = kivio_skills_dir().ok_or_else(|| "home directory unavailable".to_string())?;
    fs::create_dir_all(&dir).map_err(|err| format!("create skills dir failed: {err}"))?;
    Ok(dir)
}

/// 从 `cwd` 向上收集项目技能目录（近的在前）。碰到 `.git` 或家目录停止。
/// 每一层先 `.kivio/skills`（Kivio 自己的，对齐 `.claude/skills`），再 `.agents/skills`（共享）。
/// 跳过全局 `~/.kivio/skills` 与 `~/.agents/skills`，避免把家目录当成项目。
pub fn project_skill_dirs(cwd: &Path) -> Vec<PathBuf> {
    let skip: Vec<PathBuf> = [kivio_skills_dir(), home_agents_skills_dir()]
        .into_iter()
        .flatten()
        .collect();
    project_skill_dirs_inner(cwd, &skip)
}

fn project_skill_dirs_inner(cwd: &Path, skip: &[PathBuf]) -> Vec<PathBuf> {
    let home = directories::BaseDirs::new().map(|base| base.home_dir().to_path_buf());
    let mut out = Vec::new();
    let mut current = cwd.to_path_buf();
    for _ in 0..MAX_PROJECT_WALK {
        for rel in [".kivio", ".agents"] {
            let skills = current.join(rel).join("skills");
            if skills.is_dir() && !is_skipped(&skills, skip) {
                out.push(skills);
            }
        }
        if current.join(".git").exists() {
            break;
        }
        if home.as_ref().is_some_and(|home| paths_eq(&current, Some(home))) {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current.as_path() {
            break;
        }
        current = parent.to_path_buf();
    }
    out
}

fn is_skipped(path: &Path, skip: &[PathBuf]) -> bool {
    skip.iter().any(|other| paths_eq(path, Some(other)))
}

fn paths_eq(path: &Path, other: Option<&Path>) -> bool {
    let Some(other) = other else {
        return false;
    };
    let left = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let right = fs::canonicalize(other).unwrap_or_else(|_| other.to_path_buf());
    left == right
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
    project_cwd: Option<&Path>,
) -> Result<Vec<SkillScanRoot>, String> {
    let mut roots = Vec::new();
    if let Some(path) = bundled_skills_dir(app) {
        push_root(&mut roots, path, "builtin");
    }
    // 项目优先于全局（同 id 近处覆盖远处）：`.kivio/skills` 再 `.agents/skills`，cwd → git 根。
    if let Some(cwd) = project_cwd {
        for path in project_skill_dirs(cwd) {
            push_root(&mut roots, path, "project");
        }
    }
    // 仅「已启用」插件的附属 Skill 进入扫描；关闭插件后立刻从 registry 消失。
    for path in crate::plugins::enabled_skill_roots() {
        push_root(&mut roots, path, "plugin");
    }
    if let Some(path) = kivio_skills_dir() {
        push_root(&mut roots, path, "user");
    }
    if let Some(path) = legacy_app_data_skills_dir() {
        push_root(&mut roots, path, "user");
    }
    if let Some(path) = home_agents_skills_dir() {
        push_root(&mut roots, path, "agents");
    }
    append_external_roots(&mut roots, extra_paths);
    Ok(roots)
}

fn push_root(roots: &mut Vec<SkillScanRoot>, path: PathBuf, source: &'static str) {
    if !path.is_dir() {
        return;
    }
    if roots.iter().any(|root| root.path == path) {
        return;
    }
    roots.push(SkillScanRoot { path, source });
}

fn append_external_roots(roots: &mut Vec<SkillScanRoot>, extra_paths: &[String]) {
    for raw in extra_paths {
        push_root(roots, PathBuf::from(raw), "external");
    }
}

pub fn build_registry(app: &AppHandle, extra_paths: &[String]) -> Result<SkillRegistry, String> {
    build_registry_in(app, extra_paths, None)
}

pub fn build_registry_in(
    app: &AppHandle,
    extra_paths: &[String],
    project_cwd: Option<&Path>,
) -> Result<SkillRegistry, String> {
    build_registry_inner(app, extra_paths, project_cwd, true)
}

pub fn build_registry_metadata(
    app: &AppHandle,
    extra_paths: &[String],
) -> Result<SkillRegistry, String> {
    build_registry_inner(app, extra_paths, None, false)
}

pub fn build_registry_metadata_in(
    app: &AppHandle,
    extra_paths: &[String],
    project_cwd: Option<&Path>,
) -> Result<SkillRegistry, String> {
    build_registry_inner(app, extra_paths, project_cwd, false)
}

fn build_registry_inner(
    app: &AppHandle,
    extra_paths: &[String],
    project_cwd: Option<&Path>,
    include_files: bool,
) -> Result<SkillRegistry, String> {
    let roots = scan_root_entries(app, extra_paths, project_cwd)?;
    Ok(build_registry_from_roots(roots, include_files))
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
        if let Some(&index) = seen.get(&key) {
            // 近处覆盖远处是扫描顺序的设计：内置 / 项目 / 插件 / 个人 / ~/.agents 同 id 只留一份。
            // 跨源 overlay 不必报给用户（Cursor 的 frontend-design 与内置撞名是常态）。
            // 同一源里出现两份才是真冲突。
            if out[index].meta.source == record.meta.source {
                warnings.push(format!(
                    "Skill {} shadowed duplicate id {}",
                    out[index].meta.name, record.meta.name
                ));
            }
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

    fn write_skill(dir: &Path, id: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {id}\ndescription: {id}\n---\n"),
        )
        .unwrap();
    }

    #[test]
    fn project_walk_prefers_nearest_and_stops_at_git() {
        let base = std::env::temp_dir().join(format!("kivio-agents-walk-{}", uuid::Uuid::new_v4()));
        let root = base.join("repo");
        let nested = root.join("packages").join("app");
        write_skill(&root.join(".agents").join("skills").join("root-skill"), "root-skill");
        write_skill(
            &nested.join(".agents").join("skills").join("nested-skill"),
            "nested-skill",
        );
        write_skill(
            &base.join(".agents").join("skills").join("outside"),
            "outside",
        );
        fs::create_dir_all(root.join(".git")).unwrap();

        let dirs = project_skill_dirs_inner(&nested, &[]);
        assert_eq!(dirs.len(), 2);
        assert!(dirs[0].ends_with(Path::new(".agents").join("skills")));
        assert!(dirs[0].starts_with(&nested));
        assert!(dirs[1].starts_with(&root));
        assert!(!dirs.iter().any(|d| d.starts_with(&base.join(".agents"))));

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn project_walk_skips_global_home_agents() {
        let base = std::env::temp_dir().join(format!("kivio-agents-skip-{}", uuid::Uuid::new_v4()));
        let global = base.join(".agents").join("skills");
        write_skill(&global.join("shared"), "shared");
        fs::create_dir_all(base.join(".git")).unwrap();
        let dirs = project_skill_dirs_inner(&base, &[global]);
        assert!(dirs.is_empty());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn same_id_prefers_project_over_agents() {
        let base = std::env::temp_dir().join(format!("kivio-agents-dedup-{}", uuid::Uuid::new_v4()));
        let project_root = base.join("project").join(".agents").join("skills");
        let agents_root = base.join("home").join(".agents").join("skills");
        write_skill(&project_root.join("shared"), "shared");
        write_skill(&agents_root.join("shared"), "shared");

        let registry = build_registry_from_roots(
            vec![
                SkillScanRoot {
                    path: project_root,
                    source: "project",
                },
                SkillScanRoot {
                    path: agents_root,
                    source: "agents",
                },
            ],
            false,
        );
        assert_eq!(registry.records.len(), 1);
        assert_eq!(registry.records[0].meta.source, "project");
        assert!(
            registry.warnings.is_empty(),
            "cross-source overlay is expected, not a user warning: {:?}",
            registry.warnings
        );

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn same_source_duplicate_id_warns() {
        let base = std::env::temp_dir().join(format!("kivio-agents-samedup-{}", uuid::Uuid::new_v4()));
        let one = base.join("a");
        let two = base.join("b");
        write_skill(&one.join("shared"), "shared");
        write_skill(&two.join("shared"), "shared");

        let registry = build_registry_from_roots(
            vec![
                SkillScanRoot {
                    path: one,
                    source: "user",
                },
                SkillScanRoot {
                    path: two,
                    source: "user",
                },
            ],
            false,
        );
        assert_eq!(registry.records.len(), 1);
        assert_eq!(registry.warnings.len(), 1);
        assert!(
            registry.warnings[0].contains("shadowed duplicate id"),
            "{:?}",
            registry.warnings
        );

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn project_walk_kivio_before_agents_at_same_level() {
        let base = std::env::temp_dir().join(format!("kivio-proj-both-{}", uuid::Uuid::new_v4()));
        let root = base.join("repo");
        write_skill(
            &root.join(".kivio").join("skills").join("kivio-one"),
            "kivio-one",
        );
        write_skill(
            &root.join(".agents").join("skills").join("agents-one"),
            "agents-one",
        );
        fs::create_dir_all(root.join(".git")).unwrap();

        let dirs = project_skill_dirs_inner(&root, &[]);
        assert_eq!(dirs.len(), 2);
        assert!(dirs[0].ends_with(Path::new(".kivio").join("skills")));
        assert!(dirs[1].ends_with(Path::new(".agents").join("skills")));

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn project_walk_skips_global_home_kivio() {
        let base = std::env::temp_dir().join(format!("kivio-home-skip-{}", uuid::Uuid::new_v4()));
        let global = base.join(".kivio").join("skills");
        write_skill(&global.join("shared"), "shared");
        fs::create_dir_all(base.join(".git")).unwrap();
        let dirs = project_skill_dirs_inner(&base, &[global]);
        assert!(dirs.is_empty());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn kivio_skills_dir_is_dot_kivio_skills() {
        let dir = kivio_skills_dir().expect("home directory");
        assert!(
            dir.ends_with(Path::new(".kivio").join("skills")),
            "expected ~/.kivio/skills, got {}",
            dir.display()
        );
    }
}
