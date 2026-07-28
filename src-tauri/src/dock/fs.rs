//! Dock 文件树命令：单层列表、搜索、增删改名、系统打开。
//! 路径安全思路参考 LiveAgent `commands/workspace/fs.rs`，精简为
//! `Result<T, String>` 错误模型。

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use ignore::WalkBuilder;
use serde::Serialize;

/// 单层列表上限：超出置 hasMore，前端提示收敛目录。
const FS_LIST_MAX_ENTRIES: usize = 1000;
/// 搜索候选池上限：遍历是深度优先，只收到 max_results 会让浅层结果被先填满，
/// 先收一批更大的候选再按匹配质量排序截断。
const FS_SEARCH_CANDIDATE_LIMIT: usize = 2000;
const FS_SEARCH_DEFAULT_MAX_RESULTS: usize = 100;
/// 搜索时跳过的常见大目录（遍历时整棵剪枝）。
const FS_SEARCH_SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "vendor",
    "Pods",
    "DerivedData",
    "__pycache__",
    "venv",
    ".git",
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockFsEntry {
    pub path: String,
    pub kind: String,
    pub hidden: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsListResponse {
    pub entries: Vec<DockFsEntry>,
    pub has_more: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsSearchResponse {
    pub entries: Vec<DockFsEntry>,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsWriteResponse {
    pub path: String,
    pub kind: String,
}

// ---- 路径安全 ----

/// workdir 必须是已存在的绝对目录，canonicalize 后作为一切校验的基准。
fn canonicalize_workdir(workdir: &str) -> Result<PathBuf, String> {
    let raw = workdir.trim();
    if raw.is_empty() {
        return Err("工作目录不能为空。".to_string());
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(format!("工作目录必须是绝对路径：{raw}"));
    }
    let meta = fs::metadata(&path).map_err(|_| format!("工作目录不存在：{raw}"))?;
    if !meta.is_dir() {
        return Err(format!("工作目录不是目录：{raw}"));
    }
    fs::canonicalize(&path).map_err(|e| format!("解析工作目录失败：{e}"))
}

fn is_windows_reserved_name(input: &str) -> bool {
    let stem = input
        .split('.')
        .next()
        .unwrap_or(input)
        .trim_matches(|ch| ch == ' ' || ch == '.')
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

/// 清洗相对路径：拒绝 `..`、根/前缀组件、`:`（Windows 盘符/ADS）、Windows
/// 保留名。保留名在所有平台都拒绝——workspace 可能跨平台共享，且单测在 macOS 跑。
fn sanitize_rel_path_core(input: &str) -> Result<Option<PathBuf>, String> {
    let normalized = input.trim().replace('\\', "/");
    if normalized.is_empty() {
        return Err("路径不能为空。".to_string());
    }
    let mut out = PathBuf::new();
    for component in Path::new(&normalized).components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                return Err(format!("路径不合法：{input}"));
            }
            Component::CurDir => {}
            Component::Normal(seg) => {
                let segment = seg.to_string_lossy();
                if segment.contains(':') || is_windows_reserved_name(&segment) {
                    return Err(format!("路径不合法：{input}"));
                }
                out.push(seg);
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Ok(None);
    }
    Ok(Some(out))
}

fn sanitize_rel_path(input: &str) -> Result<PathBuf, String> {
    sanitize_rel_path_core(input)?.ok_or_else(|| format!("路径不合法：{input}"))
}

fn sanitize_optional_rel_path(input: Option<String>) -> Result<Option<PathBuf>, String> {
    match input {
        None => Ok(None),
        Some(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            sanitize_rel_path_core(&value)
        }
    }
}

/// 解析已存在的目标：canonicalize 后必须仍在 workdir 内（防 symlink 逃逸）。
fn resolve_target(workdir: &Path, rel: &Path) -> Result<PathBuf, String> {
    let target = workdir.join(rel);
    let canon = fs::canonicalize(&target)
        .map_err(|e| format!("路径不存在：{}（{e}）", rel.display()))?;
    if !canon.starts_with(workdir) {
        return Err(format!("路径越出工作目录：{}", rel.display()));
    }
    Ok(canon)
}

/// 写入类操作校验父目录（目标本身尚不存在，无法 canonicalize）。
fn resolve_parent_for_write(workdir: &Path, rel: &Path) -> Result<(PathBuf, std::ffi::OsString), String> {
    let name = rel
        .file_name()
        .ok_or_else(|| "目标路径不合法。".to_string())?
        .to_os_string();
    let parent_rel = rel.parent().map(Path::to_path_buf).unwrap_or_default();
    let parent = resolve_target(workdir, &parent_rel)?;
    if !parent.is_dir() {
        return Err(format!("父路径不是目录：{}", parent_rel.display()));
    }
    Ok((parent, name))
}

fn rel_to_workdir_str(workdir: &Path, abs: &Path) -> String {
    abs.strip_prefix(workdir)
        .unwrap_or(abs)
        .to_string_lossy()
        .replace('\\', "/")
}

// ---- 遍历 ----

fn build_walker(base: &Path, max_depth: Option<usize>, show_hidden: bool) -> ignore::Walk {
    let mut builder = WalkBuilder::new(base);
    // show_hidden=false：过滤点文件且尊重 .gitignore；true：全量放开，
    // hidden 标记由调用方与可见遍历 diff 得出。require_git(false) 让
    // .gitignore 在非 git 仓库目录同样生效（chat-workspaces 通常不是仓库）。
    builder
        .hidden(!show_hidden)
        .ignore(!show_hidden)
        .git_ignore(!show_hidden)
        .git_global(!show_hidden)
        .git_exclude(!show_hidden)
        .require_git(false)
        .follow_links(false);
    if let Some(depth) = max_depth {
        builder.max_depth(Some(depth));
    }
    builder.build()
}

fn visible_paths(base: &Path, max_depth: Option<usize>) -> HashSet<PathBuf> {
    build_walker(base, max_depth, false)
        .filter_map(Result::ok)
        .map(|entry| entry.into_path())
        .collect()
}

// ---- 命令实现 ----

fn fs_list_impl(
    workdir: &Path,
    path: Option<String>,
    max_results: Option<usize>,
    show_hidden: Option<bool>,
) -> Result<FsListResponse, String> {
    let rel = sanitize_optional_rel_path(path)?;
    let base = match rel.as_ref() {
        None => workdir.to_path_buf(),
        Some(rel) => resolve_target(workdir, rel)?,
    };
    let show_hidden = show_hidden.unwrap_or(false);
    let max_results = max_results.unwrap_or(FS_LIST_MAX_ENTRIES).clamp(1, FS_LIST_MAX_ENTRIES);

    let meta = fs::metadata(&base).map_err(|e| format!("读取路径失败：{e}"))?;
    if meta.is_file() {
        return Ok(FsListResponse {
            entries: vec![DockFsEntry {
                path: rel_to_workdir_str(workdir, &base),
                kind: "file".to_string(),
                hidden: false,
            }],
            has_more: false,
        });
    }
    if !meta.is_dir() {
        return Err("只能列目录或文件。".to_string());
    }

    // show_hidden 时用一次可见遍历做 diff，给放开的条目标 hidden 标记。
    let normally_visible = show_hidden.then(|| visible_paths(&base, Some(1)));
    let mut entries = Vec::new();
    let mut has_more = false;
    for result in build_walker(&base, Some(1), show_hidden) {
        let entry = match result {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if entry.path() == base.as_path() {
            continue;
        }
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if entries.len() >= max_results {
            has_more = true;
            break;
        }
        entries.push(DockFsEntry {
            path: rel_to_workdir_str(workdir, entry.path()),
            kind: if file_type.is_dir() { "dir" } else { "file" }.to_string(),
            hidden: normally_visible
                .as_ref()
                .is_some_and(|paths| !paths.contains(entry.path())),
        });
    }
    // 目录优先排序由前端做，这里只需稳定顺序。
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(FsListResponse { entries, has_more })
}

#[tauri::command]
pub async fn dock_fs_list(
    workdir: String,
    path: Option<String>,
    max_results: Option<usize>,
    show_hidden: Option<bool>,
) -> Result<FsListResponse, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let wd = canonicalize_workdir(&workdir)?;
        fs_list_impl(&wd, path, max_results, show_hidden)
    })
    .await
    .map_err(|e| format!("dock_fs_list join: {e}"))?
}

fn search_sort_key(path: &str, query: &str) -> (usize, usize, String) {
    let normalized_path = path.to_lowercase();
    let name = normalized_path.rsplit('/').next().unwrap_or(&normalized_path);
    let rank = if name.starts_with(query) {
        0
    } else if normalized_path.starts_with(query) {
        1
    } else if name.contains(query) {
        2
    } else {
        3
    };
    (rank, path.bytes().filter(|b| *b == b'/').count(), normalized_path)
}

fn fs_search_impl(
    workdir: &Path,
    query: String,
    max_results: Option<usize>,
    show_hidden: Option<bool>,
) -> Result<FsSearchResponse, String> {
    let query = query.trim().replace('\\', "/").to_lowercase();
    if query.is_empty() {
        return Ok(FsSearchResponse {
            entries: Vec::new(),
            truncated: false,
        });
    }
    let max_results = max_results.unwrap_or(FS_SEARCH_DEFAULT_MAX_RESULTS).max(1);
    let show_hidden = show_hidden.unwrap_or(false);
    let normally_visible = show_hidden.then(|| visible_paths(workdir, None));

    let mut candidates: Vec<DockFsEntry> = Vec::new();
    let mut truncated = false;
    // 大目录整棵剪枝：文件本身也大概率是构建产物，搜索价值低。
    let mut builder = WalkBuilder::new(workdir);
    builder
        .hidden(!show_hidden)
        .ignore(!show_hidden)
        .git_ignore(!show_hidden)
        .git_global(!show_hidden)
        .git_exclude(!show_hidden)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| {
            !entry.file_type().is_some_and(|ft| ft.is_dir())
                || entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| !FS_SEARCH_SKIP_DIRS.contains(&name))
        });
    for result in builder.build() {
        let entry = match result {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if entry.path() == workdir {
            continue;
        }
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        let rel = rel_to_workdir_str(workdir, entry.path());
        if !rel.to_lowercase().contains(&query) {
            continue;
        }
        if candidates.len() >= FS_SEARCH_CANDIDATE_LIMIT {
            truncated = true;
            break;
        }
        candidates.push(DockFsEntry {
            path: rel,
            kind: if file_type.is_dir() { "dir" } else { "file" }.to_string(),
            hidden: normally_visible
                .as_ref()
                .is_some_and(|paths| !paths.contains(entry.path())),
        });
    }

    candidates.sort_by(|a, b| {
        search_sort_key(&a.path, &query).cmp(&search_sort_key(&b.path, &query))
    });
    if candidates.len() > max_results {
        truncated = true;
        candidates.truncate(max_results);
    }
    Ok(FsSearchResponse {
        entries: candidates,
        truncated,
    })
}

#[tauri::command]
pub async fn dock_fs_search(
    workdir: String,
    query: String,
    max_results: Option<usize>,
    show_hidden: Option<bool>,
) -> Result<FsSearchResponse, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let wd = canonicalize_workdir(&workdir)?;
        fs_search_impl(&wd, query, max_results, show_hidden)
    })
    .await
    .map_err(|e| format!("dock_fs_search join: {e}"))?
}

fn fs_create_impl(workdir: &Path, path: String, kind: String) -> Result<FsWriteResponse, String> {
    let rel = sanitize_rel_path(&path)?;
    let logical = rel.to_string_lossy().replace('\\', "/");
    let (parent, name) = resolve_parent_for_write(workdir, &rel)?;
    let target = parent.join(&name);
    match fs::symlink_metadata(&target) {
        Ok(_) => return Err(format!("目标已存在：{logical}")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("检查目标失败：{e}")),
    }
    match kind.as_str() {
        "file" => {
            fs::File::create(&target).map_err(|e| format!("创建文件失败：{e}"))?;
        }
        "dir" => {
            fs::create_dir(&target).map_err(|e| format!("创建目录失败：{e}"))?;
        }
        other => return Err(format!("kind 只支持 file/dir，收到：{other}")),
    }
    Ok(FsWriteResponse {
        path: logical,
        kind,
    })
}

#[tauri::command]
pub async fn dock_fs_create(
    workdir: String,
    path: String,
    kind: String,
) -> Result<FsWriteResponse, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let wd = canonicalize_workdir(&workdir)?;
        fs_create_impl(&wd, path, kind)
    })
    .await
    .map_err(|e| format!("dock_fs_create join: {e}"))?
}

fn fs_rename_impl(
    workdir: &Path,
    from_path: String,
    to_path: String,
) -> Result<FsWriteResponse, String> {
    let from_rel = sanitize_rel_path(&from_path)?;
    let to_rel = sanitize_rel_path(&to_path)?;
    // 只支持同目录改名：跨目录移动容易误操作，文件树 UI 也只暴露改名。
    if from_rel.parent() != to_rel.parent() {
        return Err("重命名只支持同目录改名。".to_string());
    }
    let to_logical = to_rel.to_string_lossy().replace('\\', "/");
    let (parent, from_name) = resolve_parent_for_write(workdir, &from_rel)?;
    let to_name = to_rel
        .file_name()
        .ok_or_else(|| "目标路径不合法。".to_string())?
        .to_os_string();
    let source = parent.join(&from_name);
    let target = parent.join(&to_name);

    let meta = fs::symlink_metadata(&source).map_err(|e| format!("源路径不存在：{e}"))?;
    let kind = if meta.file_type().is_symlink() {
        "symlink"
    } else if meta.is_file() {
        "file"
    } else if meta.is_dir() {
        "dir"
    } else {
        return Err("只支持重命名常规文件、目录或符号链接。".to_string());
    };
    match fs::symlink_metadata(&target) {
        Ok(_) => return Err(format!("目标已存在：{to_logical}")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("检查目标失败：{e}")),
    }
    fs::rename(&source, &target).map_err(|e| format!("重命名失败：{e}"))?;
    Ok(FsWriteResponse {
        path: to_logical,
        kind: kind.to_string(),
    })
}

#[tauri::command]
pub async fn dock_fs_rename(
    workdir: String,
    from_path: String,
    to_path: String,
) -> Result<FsWriteResponse, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let wd = canonicalize_workdir(&workdir)?;
        fs_rename_impl(&wd, from_path, to_path)
    })
    .await
    .map_err(|e| format!("dock_fs_rename join: {e}"))?
}

fn fs_delete_impl(workdir: &Path, path: String) -> Result<FsWriteResponse, String> {
    let rel = sanitize_rel_path(&path)?;
    let logical = rel.to_string_lossy().replace('\\', "/");
    let (parent, name) = resolve_parent_for_write(workdir, &rel)?;
    let target = parent.join(&name);

    let meta = fs::symlink_metadata(&target).map_err(|e| format!("路径不存在：{e}"))?;
    // symlink 一律解链（remove_file 语义），绝不跟随删目标内容。
    let kind = if meta.file_type().is_symlink() {
        remove_symlink(&target)?;
        "symlink"
    } else if meta.is_file() {
        fs::remove_file(&target).map_err(|e| format!("删除文件失败：{e}"))?;
        "file"
    } else if meta.is_dir() {
        fs::remove_dir_all(&target).map_err(|e| format!("删除目录失败：{e}"))?;
        "dir"
    } else {
        return Err("只支持删除常规文件、目录或符号链接。".to_string());
    };
    Ok(FsWriteResponse {
        path: logical,
        kind: kind.to_string(),
    })
}

#[cfg(not(windows))]
fn remove_symlink(target: &Path) -> Result<(), String> {
    fs::remove_file(target).map_err(|e| format!("删除符号链接失败：{e}"))
}

#[cfg(windows)]
fn remove_symlink(target: &Path) -> Result<(), String> {
    // Windows 上目录符号链接必须用 remove_dir 解链；is_dir 跟随链接判断指向类型。
    if target.is_dir() {
        fs::remove_dir(target).map_err(|e| format!("删除符号链接失败：{e}"))
    } else {
        fs::remove_file(target).map_err(|e| format!("删除符号链接失败：{e}"))
    }
}

#[tauri::command]
pub async fn dock_fs_delete(workdir: String, path: String) -> Result<FsWriteResponse, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let wd = canonicalize_workdir(&workdir)?;
        fs_delete_impl(&wd, path)
    })
    .await
    .map_err(|e| format!("dock_fs_delete join: {e}"))?
}

#[cfg(target_os = "macos")]
fn spawn_open_command(target: &Path, kind: &str, mode: &str) -> Result<(), String> {
    let mut command = Command::new("open");
    if mode == "reveal" && kind == "file" {
        command.arg("-R");
    }
    command.arg(target);
    command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("系统打开失败（macOS open）：{e}"))
}

#[cfg(target_os = "windows")]
fn spawn_open_command(target: &Path, kind: &str, mode: &str) -> Result<(), String> {
    let mut command = Command::new("explorer.exe");
    if mode == "reveal" && kind == "file" {
        command.arg(format!("/select,{}", target.display()));
    } else {
        command.arg(target);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("系统打开失败（Windows Explorer）：{e}"))
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn spawn_open_command(target: &Path, kind: &str, mode: &str) -> Result<(), String> {
    let open_target = if mode == "reveal" && kind == "file" {
        target.parent().unwrap_or(target)
    } else {
        target
    };
    Command::new("xdg-open")
        .arg(open_target)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("系统打开失败（xdg-open）：{e}"))
}

fn fs_open_path_impl(
    workdir: &Path,
    path: String,
    mode: Option<String>,
) -> Result<FsWriteResponse, String> {
    let rel = sanitize_rel_path(&path)?;
    let logical = rel.to_string_lossy().replace('\\', "/");
    let target = resolve_target(workdir, &rel)?;
    let meta = fs::metadata(&target).map_err(|e| format!("读取路径失败：{e}"))?;
    let kind = if meta.is_file() {
        "file"
    } else if meta.is_dir() {
        "dir"
    } else {
        return Err("只支持打开常规文件或目录。".to_string());
    };
    let mode = match mode.as_deref().unwrap_or("open").trim().to_ascii_lowercase().as_str() {
        "" | "open" => "open",
        "reveal" => "reveal",
        other => return Err(format!("mode 只支持 open/reveal，收到：{other}")),
    };
    spawn_open_command(&target, kind, mode)?;
    Ok(FsWriteResponse {
        path: logical,
        kind: kind.to_string(),
    })
}

#[tauri::command]
pub async fn dock_fs_open_path(
    workdir: String,
    path: String,
    mode: Option<String>,
) -> Result<FsWriteResponse, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let wd = canonicalize_workdir(&workdir)?;
        fs_open_path_impl(&wd, path, mode)
    })
    .await
    .map_err(|e| format!("dock_fs_open_path join: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static TEMP_COUNTER: AtomicU32 = AtomicU32::new(0);

    /// 无 tempfile 依赖（不新增 crate），用进程 id + 自增序号造唯一临时目录。
    fn temp_workdir(tag: &str) -> PathBuf {
        let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("kivio-dock-fs-{tag}-{}-{id}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp workdir");
        fs::canonicalize(&dir).expect("canonicalize temp workdir")
    }

    #[test]
    fn sanitize_rejects_parent_absolute_and_reserved() {
        for input in ["../evil", "a/../../b", "/etc/passwd", "C:/windows", "a:b", "CON", "com1.txt", "lpt9"] {
            assert!(sanitize_rel_path(input).is_err(), "{input} should be rejected");
        }
        for input in ["src/main.rs", "a/./b.txt", "console.log"] {
            assert!(sanitize_rel_path(input).is_ok(), "{input} should be accepted");
        }
    }

    #[cfg(unix)]
    #[test]
    fn resolve_target_rejects_symlink_escape() {
        let workdir = temp_workdir("symlink");
        let outside = temp_workdir("outside");
        std::os::unix::fs::symlink(&outside, workdir.join("link")).expect("create symlink");
        let err = resolve_target(&workdir, Path::new("link")).unwrap_err();
        assert!(err.contains("越出工作目录"), "unexpected error: {err}");
        fs::remove_dir_all(&workdir).ok();
        fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn list_respects_gitignore_and_marks_hidden() {
        let workdir = temp_workdir("list");
        fs::write(workdir.join(".gitignore"), "ignored.txt\n").expect("write .gitignore");
        fs::write(workdir.join("ignored.txt"), "x").expect("write ignored");
        fs::write(workdir.join("visible.txt"), "x").expect("write visible");
        fs::write(workdir.join(".secret"), "x").expect("write hidden");

        let visible = fs_list_impl(&workdir, None, None, Some(false)).expect("list visible");
        let paths: Vec<&str> = visible.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"visible.txt"));
        assert!(!paths.contains(&".gitignore"), "dotfiles filtered when show_hidden=false");
        assert!(!paths.contains(&"ignored.txt"));
        assert!(visible.entries.iter().all(|e| !e.hidden));

        let all = fs_list_impl(&workdir, None, None, Some(true)).expect("list all");
        let by_path = |p: &str| all.entries.iter().find(|e| e.path == p);
        assert!(by_path("visible.txt").is_some_and(|e| !e.hidden));
        assert!(by_path("ignored.txt").is_some_and(|e| e.hidden), "gitignored entry marked hidden");
        assert!(by_path(".secret").is_some_and(|e| e.hidden), "dotfile marked hidden");
        fs::remove_dir_all(&workdir).ok();
    }

    #[test]
    fn rename_rejects_cross_directory() {
        let workdir = temp_workdir("rename");
        fs::create_dir(workdir.join("sub")).expect("create sub");
        fs::write(workdir.join("a.txt"), "x").expect("write a.txt");
        let err = fs_rename_impl(&workdir, "a.txt".to_string(), "sub/a.txt".to_string()).unwrap_err();
        assert!(err.contains("同目录"), "unexpected error: {err}");
        fs_rename_impl(&workdir, "a.txt".to_string(), "b.txt".to_string()).expect("same-dir rename");
        assert!(workdir.join("b.txt").exists());
        fs::remove_dir_all(&workdir).ok();
    }

    #[test]
    fn search_skips_common_dirs_and_ranks() {
        let workdir = temp_workdir("search");
        fs::create_dir_all(workdir.join("node_modules/pkg")).expect("mkdir node_modules");
        fs::write(workdir.join("node_modules/pkg/app.js"), "x").expect("write dep");
        fs::create_dir_all(workdir.join("src")).expect("mkdir src");
        fs::write(workdir.join("src/app.ts"), "x").expect("write src/app.ts");
        fs::write(workdir.join("my-app-notes.md"), "x").expect("write notes");

        let result = fs_search_impl(&workdir, "app".to_string(), None, Some(false)).expect("search");
        let paths: Vec<&str> = result.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(!paths.iter().any(|p| p.contains("node_modules")), "node_modules skipped: {paths:?}");
        // 名称前缀匹配（app.ts）排在名称包含（my-app-notes.md）之前。
        let pos_app = paths.iter().position(|p| *p == "src/app.ts").expect("app.ts ranked");
        let pos_notes = paths.iter().position(|p| *p == "my-app-notes.md").expect("notes ranked");
        assert!(pos_app < pos_notes, "basename-prefix should rank first: {paths:?}");
        fs::remove_dir_all(&workdir).ok();
    }
}
