mod catalog;
mod discover;
pub mod parse;
mod runtime;
mod types;

pub use catalog::format_catalog;
pub use discover::{
    build_registry, build_registry_in, build_registry_metadata, build_registry_metadata_in,
    home_agents_skills_dir, kivio_skills_dir, legacy_app_data_skills_dir, user_skills_dir,
};
pub use parse::parse_skill_markdown;
pub use runtime::{
    activate_skill, extract_skill_name, lookup_skill, substitute_arguments, SkillRunCache,
};
pub use types::{
    slugify, SkillDetail, SkillImportResult, SkillListResult, SkillMeta, SkillOpenFolderResult,
    SkillReadResult, SkillRecord, SkillRegistry,
};

use std::{
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

use tauri::{AppHandle, State};
use tauri_plugin_shell::ShellExt;

use crate::state::AppState;

#[tauri::command]
pub fn chat_skills_list(
    app: AppHandle,
    state: State<'_, AppState>,
    skill_scan_paths: Option<Vec<String>>,
    project_cwd: Option<String>,
) -> SkillListResult {
    let paths = skill_scan_paths
        .unwrap_or_else(|| state.settings_read().chat_tools.skill_scan_paths.clone());
    let cwd = project_cwd
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    match build_registry_metadata_in(&app, &paths, cwd.as_deref()) {
        Ok(registry) => {
            let settings = state.settings_read();
            // 插件附属 skill 一并返回，技能页单独分区展示；
            // 开关仍由「扩展 → 插件」统一管理（前端禁止在技能页改插件 skill）。
            let skills = registry
                .metas()
                .into_iter()
                .filter_map(|mut meta| {
                    if crate::plugins::skill_owned_by_plugin(&meta.id).is_some() {
                        meta.source = "plugin".to_string();
                        return Some(meta);
                    }
                    if meta.source == "plugin" {
                        return Some(meta);
                    }
                    crate::settings::skill_connector_satisfied(
                        &meta.id,
                        crate::settings::obsidian_connector_configured(
                            &settings.obsidian_vault_path,
                        ),
                    )
                    .then_some(meta)
                })
                .collect();
            SkillListResult {
                success: true,
                skills,
                error: None,
                warnings: registry.warnings,
            }
        }
        Err(err) => SkillListResult {
            success: false,
            skills: Vec::new(),
            warnings: Vec::new(),
            error: Some(err),
        },
    }
}

#[tauri::command]
pub fn chat_skills_read(
    app: AppHandle,
    state: State<'_, AppState>,
    skill_id: String,
    project_cwd: Option<String>,
) -> SkillReadResult {
    let settings = state.settings_read();
    if let Some(err) = crate::settings::skill_global_unavailable_error(
        &settings.chat_tools,
        &skill_id,
        crate::settings::obsidian_connector_configured(&settings.obsidian_vault_path),
        &skill_id,
    ) {
        return SkillReadResult {
            success: false,
            skill: None,
            error: Some(err),
        };
    }
    let cwd = project_cwd
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    match read_skill_detail_in(
        &app,
        &settings.chat_tools.skill_scan_paths,
        &skill_id,
        cwd.as_deref(),
    ) {
        Ok(skill) => SkillReadResult {
            success: true,
            skill: Some(skill),
            error: None,
        },
        Err(err) => SkillReadResult {
            success: false,
            skill: None,
            error: Some(err),
        },
    }
}

#[tauri::command]
#[allow(deprecated)]
pub fn chat_skills_open_folder(app: AppHandle) -> SkillOpenFolderResult {
    match user_skills_dir(&app) {
        Ok(dir) => {
            let path = dir.display().to_string();
            if let Err(err) = app.shell().open(&path, None) {
                SkillOpenFolderResult {
                    success: false,
                    path: Some(path),
                    error: Some(err.to_string()),
                }
            } else {
                SkillOpenFolderResult {
                    success: true,
                    path: Some(path),
                    error: None,
                }
            }
        }
        Err(err) => SkillOpenFolderResult {
            success: false,
            path: None,
            error: Some(err),
        },
    }
}

#[tauri::command]
pub fn chat_skills_import(app: AppHandle, path: String) -> SkillImportResult {
    let source = PathBuf::from(path);
    let skills_dir = match user_skills_dir(&app) {
        Ok(path) => path,
        Err(err) => {
            return SkillImportResult {
                success: false,
                skill: None,
                error: Some(err),
            }
        }
    };
    let result = if source.is_dir() {
        import_skill_dir(&source, &skills_dir)
    } else if source
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("zip"))
        .unwrap_or(false)
    {
        import_skill_zip(&source, &skills_dir)
    } else {
        Err("Skill import expects a folder or zip containing SKILL.md".to_string())
    };

    match result {
        Ok(meta) => SkillImportResult {
            success: true,
            skill: Some(meta),
            error: None,
        },
        Err(err) => SkillImportResult {
            success: false,
            skill: None,
            error: Some(err),
        },
    }
}

/// 卸载用户技能：先删 `~/.kivio/skills/<id>`，再回退旧 `{app_data}/skills/<id>`。
/// `~/.agents/skills` 是共享目录，不从这里删。内置与插件技能也无法经此删除。
#[tauri::command]
pub fn chat_skills_uninstall(app: AppHandle, id: String) -> Result<(), String> {
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err("invalid skill id".to_string());
    }
    let mut candidates = Vec::new();
    if let Ok(dir) = user_skills_dir(&app) {
        candidates.push(dir.join(&id));
    }
    if let Some(dir) = legacy_app_data_skills_dir() {
        candidates.push(dir.join(&id));
    }
    let dir = candidates
        .into_iter()
        .find(|path| path.is_dir())
        .ok_or_else(|| "技能不存在或不可删除（仅 ~/.kivio/skills 与旧个人目录可删除）".to_string())?;
    fs::remove_dir_all(&dir).map_err(|err| format!("删除技能失败: {err}"))?;
    Ok(())
}

/// 技能包下载大小上限（与本地 zip 导入的隐含约束一致，防止误装超大包）。
const MAX_SKILL_DOWNLOAD_BYTES: u64 = 50 * 1024 * 1024;

/// 把 GitHub 仓库页面 URL 归一为 codeload zip 直链；直链 zip / clawhub 下载链 / 其它原样返回。
/// 支持 `github.com/{owner}/{repo}`、`.../tree/{ref}`（子目录忽略，安装首个 SKILL.md）。
fn normalize_skill_download_url(url: &str) -> String {
    let trimmed = url.trim();
    if let Ok(parsed) = reqwest::Url::parse(trimmed) {
        if parsed.host_str() == Some("github.com") {
            let segs: Vec<&str> = parsed.path().split('/').filter(|s| !s.is_empty()).collect();
            if segs.len() >= 2 {
                let owner = segs[0];
                let repo = segs[1].trim_end_matches(".git");
                let git_ref = if segs.len() >= 4 && segs[2] == "tree" {
                    segs[3]
                } else {
                    "HEAD"
                };
                return format!("https://codeload.github.com/{owner}/{repo}/zip/{git_ref}");
            }
        }
    }
    trimmed.to_string()
}

/// 技能市场安装：从 ClawHub 下载链 / GitHub 仓库 / 直链 zip 下载 zip 并落盘。
/// 浏览/搜索/owner 消歧在前端完成，这里只负责下载 + 复用 `install_skill_zip_bytes`。
#[tauri::command]
pub async fn chat_skills_install_from_url(app: AppHandle, url: String) -> SkillImportResult {
    match install_skill_from_url(&app, &url).await {
        Ok(meta) => SkillImportResult {
            success: true,
            skill: Some(meta),
            error: None,
        },
        Err(err) => SkillImportResult {
            success: false,
            skill: None,
            error: Some(err),
        },
    }
}

async fn install_skill_from_url(app: &AppHandle, url: &str) -> Result<SkillMeta, String> {
    let skills_dir = user_skills_dir(app)?;
    download_skill_zip_into(url, &skills_dir).await
}

/// 从 GitHub 仓库 / 直链 zip 下载一个 Skill 到指定 skills 目录（解压 zip 内首个 SKILL.md 所在
/// 文件夹到 `{skills_dir}/{id}`）。技能市场安装、URL 导入、插件自带 Skill 下载共用。
pub async fn download_skill_zip_into(url: &str, skills_dir: &Path) -> Result<SkillMeta, String> {
    let download_url = normalize_skill_download_url(url);
    let client = crate::api::build_http_client();
    let response = client
        .get(&download_url)
        .header(reqwest::header::USER_AGENT, "kivio-skill-market")
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|err| format!("Download failed: {err}"))?;
    if !response.status().is_success() {
        return Err(format!("Download failed: HTTP {}", response.status()));
    }
    if let Some(len) = response.content_length() {
        if len > MAX_SKILL_DOWNLOAD_BYTES {
            return Err("Skill package too large (over 50MB)".to_string());
        }
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|err| format!("Read download failed: {err}"))?;
    if bytes.len() as u64 > MAX_SKILL_DOWNLOAD_BYTES {
        return Err("Skill package too large (over 50MB)".to_string());
    }
    install_skill_zip_bytes(bytes.to_vec(), skills_dir)
}

pub fn read_skill_detail(
    app: &AppHandle,
    extra_paths: &[String],
    skill_id: &str,
) -> Result<SkillDetail, String> {
    read_skill_detail_in(app, extra_paths, skill_id, None)
}

pub fn read_skill_detail_in(
    app: &AppHandle,
    extra_paths: &[String],
    skill_id: &str,
    project_cwd: Option<&Path>,
) -> Result<SkillDetail, String> {
    let registry = build_registry_in(app, extra_paths, project_cwd)?;
    let record = registry
        .find(skill_id)
        .ok_or_else(|| format!("Skill not found: {skill_id}"))?;
    Ok(SkillDetail {
        meta: record.meta.clone(),
        body: record.body.clone(),
    })
}

fn import_skill_dir(source: &Path, skills_dir: &Path) -> Result<SkillMeta, String> {
    let skill_file = source.join("SKILL.md");
    if !skill_file.is_file() {
        return Err("Selected folder does not contain SKILL.md".to_string());
    }
    let raw =
        fs::read_to_string(&skill_file).map_err(|err| format!("Read SKILL.md failed: {err}"))?;
    let files = discover::index_skill_files(source)?;
    let mut warnings = Vec::new();
    let parsed = parse::parse_skill_record(&skill_file, &raw, "user", files, &mut warnings)?;
    let dest = skills_dir.join(&parsed.meta.id);
    copy_dir_recursive(source, &dest)?;
    Ok(SkillMeta {
        path: Some(dest.join("SKILL.md").display().to_string()),
        ..parsed.meta
    })
}

fn import_skill_zip(source: &Path, skills_dir: &Path) -> Result<SkillMeta, String> {
    let bytes = fs::read(source).map_err(|err| format!("Read zip failed: {err}"))?;
    install_skill_zip_bytes(bytes, skills_dir)
}

/// 从内存中的 zip 字节解压一个 Skill 到 `{skills_dir}/{id}`。本地导入与技能市场安装共用。
/// ponytail: 失败时删掉刚建的目标目录以免留半个技能；未做 temp+rename 原子安装（技能包小、
/// 单用户本地操作，冲突概率低）——若将来并发安装需要，改成解压到临时目录再 rename。
pub fn install_skill_zip_bytes(bytes: Vec<u8>, skills_dir: &Path) -> Result<SkillMeta, String> {
    let reader = Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|err| format!("Open zip failed: {err}"))?;
    let mut skill_raw = String::new();
    let mut skill_path = String::new();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|err| err.to_string())?;
        if file.name().ends_with("SKILL.md") {
            file.read_to_string(&mut skill_raw)
                .map_err(|err| format!("Read SKILL.md in zip failed: {err}"))?;
            skill_path = file.name().to_string();
            break;
        }
    }
    if skill_raw.trim().is_empty() {
        return Err("Zip does not contain SKILL.md".to_string());
    }
    let parsed = parse_skill_markdown(&skill_raw, "user", None, Vec::new())?;
    let dest = skills_dir.join(&parsed.meta.id);
    // 更新/重装：先清掉旧目录，保证覆盖而不是残留混合。
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(|err| format!("clear old skill dir failed: {err}"))?;
    }
    match extract_zip_into(&mut archive, &skill_path, &dest) {
        Ok(()) => Ok(SkillMeta {
            path: Some(dest.join("SKILL.md").display().to_string()),
            ..parsed.meta
        }),
        Err(err) => {
            let _ = fs::remove_dir_all(&dest);
            Err(err)
        }
    }
}

fn extract_zip_into<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    skill_path: &str,
    dest: &Path,
) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|err| format!("create skill dir failed: {err}"))?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|err| err.to_string())?;
        if file.is_dir() {
            continue;
        }
        let name = file.name();
        let relative = if skill_path.contains('/') {
            let prefix = skill_path
                .rsplit_once('/')
                .map(|(prefix, _)| format!("{prefix}/"))
                .unwrap_or_default();
            name.strip_prefix(&prefix).unwrap_or(name)
        } else {
            name
        };
        if relative.contains("..") {
            continue;
        }
        let out = dest.join(relative);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let mut output = fs::File::create(&out).map_err(|err| err.to_string())?;
        std::io::copy(&mut file, &mut output).map_err(|err| err.to_string())?;
    }
    Ok(())
}

fn copy_dir_recursive(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|err| err.to_string())?;
    for entry in fs::read_dir(from).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_dir_recursive(&src, &dst)?;
        } else if src.is_file() {
            fs::copy(&src, &dst).map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_normalize_github_repo_and_passthrough() {
        assert_eq!(
            normalize_skill_download_url("https://github.com/owner/repo"),
            "https://codeload.github.com/owner/repo/zip/HEAD"
        );
        assert_eq!(
            normalize_skill_download_url("https://github.com/owner/repo.git"),
            "https://codeload.github.com/owner/repo/zip/HEAD"
        );
        assert_eq!(
            normalize_skill_download_url("https://github.com/owner/repo/tree/dev"),
            "https://codeload.github.com/owner/repo/zip/dev"
        );
        // 直链 zip / clawhub 下载链原样返回
        let claw = "https://clawhub.ai/api/v1/download?slug=x&tag=latest&ownerHandle=y";
        assert_eq!(normalize_skill_download_url(claw), claw);
        assert_eq!(
            normalize_skill_download_url("https://example.com/a.zip"),
            "https://example.com/a.zip"
        );
    }

    fn zip_with_skill(id: &str) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            z.start_file("SKILL.md", opts).unwrap();
            let md = format!("---\nname: {id}\ndescription: A test skill.\n---\n# Body\n");
            std::io::Write::write_all(&mut z, md.as_bytes()).unwrap();
            z.start_file("script.py", opts).unwrap();
            std::io::Write::write_all(&mut z, b"print('hi')\n").unwrap();
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn install_skill_zip_bytes_lands_skill_and_files() {
        let dir = std::env::temp_dir().join(format!("kivio-skilltest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let meta = install_skill_zip_bytes(zip_with_skill("zip-skill"), &dir).unwrap();
        assert_eq!(meta.id, "zip-skill");
        assert!(dir.join("zip-skill/SKILL.md").is_file());
        assert!(dir.join("zip-skill/script.py").is_file());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_skill_zip_bytes_bad_zip_errors_without_dir() {
        let dir = std::env::temp_dir().join(format!("kivio-skilltest-bad-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let err = install_skill_zip_bytes(b"not a zip".to_vec(), &dir);
        assert!(err.is_err());
        // 目录里不该留下任何技能子目录
        let leftovers: Vec<_> = fs::read_dir(&dir).unwrap().flatten().collect();
        assert!(leftovers.is_empty(), "bad zip left files behind");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_skill_supports_recommended_tools_and_mcp_tools_alias() {
        let raw = r#"---
name: test-skill
description: Uses selected tools.
recommended-tools:
  - web_search
mcp-tools:
  - fetch
  - web_search
allowed-tools: Bash(git:*)
---
# Body
"#;

        let parsed = parse_skill_markdown(raw, "user", None, Vec::new()).unwrap();

        assert!(parsed.meta.recommended_tools.contains(&"fetch".to_string()));
        assert!(parsed
            .meta
            .recommended_tools
            .contains(&"web_search".to_string()));
        assert!(parsed
            .meta
            .recommended_tools
            .iter()
            .any(|tool| tool.contains("Bash")));
    }

    #[test]
    fn parse_skill_requires_name_and_description() {
        let err = parse_skill_markdown(
            r#"---
description: Missing name.
---
# Body
"#,
            "user",
            None,
            Vec::new(),
        )
        .expect_err("missing name should be rejected");

        assert!(err.contains("name"));
    }

    #[test]
    fn disable_model_invocation_parses_from_frontmatter() {
        let parsed = parse_skill_markdown(
            r#"---
name: manual-only
description: Only when invoked explicitly.
disable-model-invocation: true
---
"#,
            "user",
            None,
            Vec::new(),
        )
        .unwrap();
        assert!(parsed.meta.disable_model_invocation);
    }
}
