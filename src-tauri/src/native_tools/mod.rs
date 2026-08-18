mod fetch;
mod files;
mod sandbox_exports;
mod shell;

pub(crate) use fetch::html_to_text;
pub use fetch::web_fetch;
pub use files::{
    edit_file, glob_files, list_dir, read_file, search_files, write_file, FileMutationResult,
    ReadFileResult,
};
pub(crate) use sandbox_exports::preflight_directory_merge;
pub use sandbox_exports::{
    build_delivery_artifact_for_path, cleanup_stale_sandbox_exports, export_sandbox_artifacts,
    format_export_error, format_exported_paths, legacy_outputs_dir,
    merge_directory_without_overwrite, path_under_directory,
    remove_sandbox_exports_for_conversation, resolve_sandbox_export_file_path,
    SandboxExportContext,
};
pub(crate) use shell::build_shell_command;
pub use shell::run_command;
pub use shell::{
    bash_output, kill_background, kill_process_group, list_background, run_command_shell_hint,
    BackgroundCommand, BackgroundCommandStatus, BG_CMD_LOG_PREFIX,
};

use std::{
    fs,
    path::{Path, PathBuf},
};

/// Above this size a file is read through the streaming line-window reader
/// instead of being slurped into memory. NOT an output budget — that is
/// [`TOOL_OUTPUT_MAX_BYTES`], which applies to every read regardless of size.
pub const MAX_READ_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// 工具输出的统一上限，照抄 pi `core/tools/truncate.ts` 的 `DEFAULT_MAX_LINES` /
/// `DEFAULT_MAX_BYTES`：**行数和字节数谁先到算谁**，且永远只返回完整行。
///
/// pi 的规范是「每个工具的输出出口都过一遍 truncate」。Kivio 的 `run_command`
/// 早就在用这两个数（tail 方向），但 `read` 一直没有出口上限：一次 `read` 能把
/// 2MB 的长行文件整个灌进上下文（≈50 万 token），连读几个这样的文件就把请求撑到
/// 供应商静默返回 200 + 空正文——表现为「模型返回了空响应」，实为框架没有地板。
pub const TOOL_OUTPUT_MAX_LINES: usize = 2_000;
pub const TOOL_OUTPUT_MAX_BYTES: usize = 50 * 1024;

#[derive(Debug, Clone)]
pub struct ProjectWorkspaceContext {
    pub project_id: String,
    pub project_name: String,
    pub root_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct NativeToolWorkspace {
    pub project: Option<ProjectWorkspaceContext>,
    /// Default workbench for a non-project conversation. This is an ergonomic
    /// base path, never a sandbox or permission boundary.
    pub default_directory: Option<PathBuf>,
}

impl NativeToolWorkspace {
    /// Compatibility constructor for tests and standalone callers. The first
    /// legacy root, when present, acts only as a default base directory.
    pub fn global(workspace_roots: &[String]) -> Self {
        workspace_roots
            .iter()
            .map(|root| root.trim())
            .find(|root| !root.is_empty())
            .map(|root| Self::conversation(PathBuf::from(root)))
            .unwrap_or_else(Self::standalone)
    }

    pub fn standalone() -> Self {
        Self {
            project: None,
            default_directory: None,
        }
    }

    pub fn conversation(default_directory: PathBuf) -> Self {
        Self {
            project: None,
            default_directory: Some(default_directory),
        }
    }

    pub fn project(project_id: String, project_name: String, root_path: Option<String>) -> Self {
        Self {
            project: Some(ProjectWorkspaceContext {
                project_id,
                project_name,
                root_path: root_path.map(PathBuf::from),
            }),
            default_directory: None,
        }
    }

    pub fn has_project(&self) -> bool {
        self.project.is_some()
    }

    pub fn conversation_directory(&self) -> Option<&Path> {
        if self.project.is_none() {
            self.default_directory.as_deref()
        } else {
            None
        }
    }

    pub fn default_output_directory(&self) -> Result<PathBuf, String> {
        if self.has_project() {
            project_root_required(self)
        } else if let Some(path) = self.default_directory.as_ref() {
            ensure_directory(path)
        } else {
            user_home_dir()
        }
    }
}

pub fn conversation_workspace_directory(
    working_directory: &str,
    conversation_id: &str,
) -> Result<PathBuf, String> {
    let valid = conversation_id.starts_with("conv_")
        && conversation_id.len() > "conv_".len()
        && conversation_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !valid {
        return Err(format!("Invalid conversation id: {conversation_id}"));
    }
    let root = candidate_path(working_directory)?;
    let root = fs_canonicalize_existing_or_self(&root)?;
    Ok(root.join(conversation_id))
}

pub fn user_home_dir() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("USERPROFILE")
            .map(PathBuf::from)
            .map_err(|_| "USERPROFILE is not set".to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("HOME")
            .map(PathBuf::from)
            .map_err(|_| "HOME is not set".to_string())
    }
}

pub fn resolve_tool_read_path(
    workspace: &NativeToolWorkspace,
    raw_path: &str,
) -> Result<PathBuf, String> {
    resolve_tool_path(workspace, raw_path, false)
}

pub fn resolve_tool_write_path(
    workspace: &NativeToolWorkspace,
    raw_path: &str,
) -> Result<PathBuf, String> {
    resolve_tool_path(workspace, raw_path, true)
}

fn resolve_tool_path(
    workspace: &NativeToolWorkspace,
    raw_path: &str,
    allow_missing: bool,
) -> Result<PathBuf, String> {
    let expanded = expand_home_prefix(raw_path)?;
    if expanded.trim().is_empty() {
        return Err("File path cannot be empty".to_string());
    }
    let raw = Path::new(&expanded);
    if raw.is_absolute() {
        return canonicalize_existing_or_missing(raw, allow_missing);
    }
    if workspace.has_project() {
        return resolve_project_path(workspace, raw_path, allow_missing);
    }
    let base = match workspace.default_directory.as_ref() {
        Some(path) => ensure_directory(path)?,
        None => user_home_dir()?,
    };
    canonicalize_existing_or_missing(&base.join(raw), allow_missing)
}

pub fn resolve_tool_existing_dir(
    workspace: &NativeToolWorkspace,
    raw_path: Option<&str>,
) -> Result<PathBuf, String> {
    if workspace.has_project() {
        if let Some(cwd_arg) = raw_path.map(str::trim).filter(|path| !path.is_empty()) {
            let expanded = expand_home_prefix(cwd_arg)?;
            if Path::new(&expanded).is_absolute() {
                let path = resolve_read_path(cwd_arg)?;
                if !path.is_dir() {
                    return Err(format!(
                        "Working directory is not a directory: {}",
                        path.display()
                    ));
                }
                return Ok(path);
            }
        }
        let path = resolve_project_path(workspace, raw_path.unwrap_or("."), false)?;
        if !path.is_dir() {
            return Err(format!(
                "Working directory is not a directory: {}",
                path.display()
            ));
        }
        return Ok(path);
    }

    let path = match raw_path.map(str::trim).filter(|path| !path.is_empty()) {
        Some(raw) => resolve_tool_path(workspace, raw, false)?,
        None => match workspace.default_directory.as_ref() {
            Some(path) => ensure_directory(path)?,
            None => user_home_dir()?,
        },
    };
    if !path.is_dir() {
        return Err(format!(
            "Working directory is not a directory: {}",
            path.display()
        ));
    }
    Ok(path)
}

fn ensure_directory(path: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(path).map_err(|err| {
        format!(
            "Create conversation workspace failed ({}): {err}",
            path.display()
        )
    })?;
    fs::canonicalize(path).map_err(|err| {
        format!(
            "Resolve conversation workspace failed ({}): {err}",
            path.display()
        )
    })
}

pub fn workspace_display_path(workspace: &NativeToolWorkspace, path: &Path) -> String {
    if let Some(project) = &workspace.project {
        if let Some(root) = project.root_path.as_ref() {
            if let Ok(root_canon) = fs_canonicalize_existing_or_self(root) {
                if let Ok(relative) = path.strip_prefix(&root_canon) {
                    let rel = relative.to_string_lossy();
                    return if rel.is_empty() {
                        ".".to_string()
                    } else {
                        rel.to_string()
                    };
                }
            }
        }
    }
    path.display().to_string()
}

/// Legacy standalone resolver. Runtime tool calls should use the workspace-aware
/// helpers above. There is no path confinement: absolute/`~` paths are honored.
pub fn resolve_read_path(raw_path: &str) -> Result<PathBuf, String> {
    let candidate = candidate_path(raw_path)?;
    fs_canonicalize_existing_or_self(&candidate)
}

fn project_root_required(workspace: &NativeToolWorkspace) -> Result<PathBuf, String> {
    let Some(project) = &workspace.project else {
        return Err("No active project workspace.".to_string());
    };
    let Some(root_path) = project.root_path.as_ref() else {
        return Err(format!(
            "项目「{}」尚未绑定本地文件夹。请先在项目菜单中绑定文件夹，再使用文件或命令工具。",
            project.project_name
        ));
    };
    if !root_path.is_dir() {
        return Err(format!(
            "项目「{}」绑定的文件夹不存在或不可访问：{}",
            project.project_name,
            root_path.display()
        ));
    }
    fs::canonicalize(root_path).map_err(|err| format!("Resolve project root failed: {err}"))
}

fn resolve_project_path(
    workspace: &NativeToolWorkspace,
    raw_path: &str,
    allow_missing: bool,
) -> Result<PathBuf, String> {
    let root = project_root_required(workspace)?;
    let expanded = expand_home_prefix(raw_path)?;
    if expanded.trim().is_empty() {
        return Err("路径为空，请提供项目内的文件或目录路径。".to_string());
    }

    // Relative paths resolve against the project root (ergonomic default).
    // `..` and absolute paths are allowed — no project-root confinement.
    let raw = Path::new(&expanded);
    let candidate = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        root.join(raw)
    };
    canonicalize_existing_or_missing(&candidate, allow_missing)
}

fn canonicalize_existing_or_missing(path: &Path, _allow_missing: bool) -> Result<PathBuf, String> {
    if path.exists() {
        return fs::canonicalize(path).map_err(|err| format!("Resolve path failed: {err}"));
    }

    let mut missing = Vec::new();
    let mut current = path;
    while !current.exists() {
        let name = current
            .file_name()
            .ok_or_else(|| "Invalid path".to_string())?;
        missing.push(name.to_os_string());
        current = current.parent().ok_or_else(|| "Invalid path".to_string())?;
    }

    let mut resolved =
        fs::canonicalize(current).map_err(|err| format!("Resolve path failed: {err}"))?;
    for name in missing.iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

/// Expands a leading `~` / `~/` / `~\` to the user home directory (shell-style).
fn expand_home_prefix(raw_path: &str) -> Result<String, String> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed == "~" {
        return Ok(user_home_dir()?.to_string_lossy().to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        let home = user_home_dir()?;
        return Ok(home.join(rest).to_string_lossy().to_string());
    }
    #[cfg(target_os = "windows")]
    if let Some(rest) = trimmed.strip_prefix("~\\") {
        let home = user_home_dir()?;
        return Ok(home.join(rest).to_string_lossy().to_string());
    }
    Ok(trimmed.to_string())
}

fn candidate_path(raw_path: &str) -> Result<PathBuf, String> {
    let expanded = expand_home_prefix(raw_path)?;
    if expanded.is_empty() {
        return Err("路径为空，请提供要访问的文件或目录路径。".to_string());
    }

    let path = Path::new(&expanded);
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        user_home_dir()?.join(path)
    })
}

fn fs_canonicalize_existing_or_self(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        fs::canonicalize(path).map_err(|err| format!("Resolve path failed: {err}"))
    } else {
        Ok(path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_dir_traversal_is_allowed_no_boundary() {
        // No-boundary model: `..` is no longer rejected. The path resolves to an
        // absolute, canonical form (access is gated by session consent instead).
        let resolved = resolve_read_path("../").expect("`..` resolves under no-boundary");
        assert!(resolved.is_absolute());
    }

    #[test]
    fn resolve_read_path_allows_temp_paths() {
        let file = std::env::temp_dir().join(format!("kivio_read_{}.txt", uuid::Uuid::new_v4()));
        fs::write(&file, "hello").expect("write temp file");

        let resolved = resolve_read_path(&file.to_string_lossy()).expect("resolve read path");
        assert_eq!(
            resolved,
            fs::canonicalize(&file).expect("canonical temp file")
        );

        let _ = fs::remove_file(file);
    }

    #[test]
    fn resolve_read_path_expands_tilde_home_prefix() {
        let home = user_home_dir().expect("home");
        let dir = home.join(format!(".kivio_tilde_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        let file = dir.join("sample.csv");
        fs::write(&file, "alpha,beta").expect("write");

        let rel = dir
            .strip_prefix(&home)
            .expect("dir under home")
            .to_string_lossy();
        let tilde_path = format!("~/{}/sample.csv", rel);
        let resolved = resolve_read_path(&tilde_path).expect("resolve tilde path");
        assert_eq!(resolved, fs::canonicalize(&file).expect("canonical file"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn conversation_relative_paths_use_and_create_default_workbench() {
        let root =
            std::env::temp_dir().join(format!("kivio_workspace_{}", uuid::Uuid::new_v4().simple()));
        let workspace = NativeToolWorkspace::conversation(root.clone());
        assert!(!root.exists());
        let resolved = resolve_tool_write_path(&workspace, "reports/result.txt").expect("resolve");
        assert!(root.is_dir());
        assert_eq!(
            resolved,
            fs::canonicalize(&root).unwrap().join("reports/result.txt")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_absolute_path_does_not_create_conversation_workbench() {
        let root =
            std::env::temp_dir().join(format!("kivio_workspace_{}", uuid::Uuid::new_v4().simple()));
        let explicit = std::env::temp_dir().join(format!(
            "kivio_explicit_{}.txt",
            uuid::Uuid::new_v4().simple()
        ));
        let workspace = NativeToolWorkspace::conversation(root.clone());
        let resolved =
            resolve_tool_write_path(&workspace, &explicit.to_string_lossy()).expect("resolve");
        let expected = fs::canonicalize(explicit.parent().expect("temp file parent"))
            .expect("canonical temp dir")
            .join(explicit.file_name().expect("temp file name"));
        assert_eq!(resolved, expected);
        assert!(!root.exists());
    }

    #[test]
    fn omitted_command_cwd_creates_and_uses_conversation_workbench() {
        let root =
            std::env::temp_dir().join(format!("kivio_workspace_{}", uuid::Uuid::new_v4().simple()));
        let workspace = NativeToolWorkspace::conversation(root.clone());
        let cwd = resolve_tool_existing_dir(&workspace, None).expect("cwd");
        assert_eq!(cwd, fs::canonicalize(&root).unwrap());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn conversation_workspace_directory_rejects_path_like_ids() {
        assert!(conversation_workspace_directory("C:/tmp", "../conv_bad").is_err());
        assert!(conversation_workspace_directory("C:/tmp", "conv_good-123").is_ok());
    }
}
