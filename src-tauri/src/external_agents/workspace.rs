use std::path::{Path, PathBuf};

use tauri::AppHandle;

use crate::chat::storage::{
    conversations_dir, find_project_by_id, load_conversation, resolve_conversation_project,
};
use crate::chat::types::ChatProject;
use crate::external_agents::types::RuntimeAgentDef;
use crate::utils::strip_windows_verbatim_prefix;

pub fn resolve_effective_cwd(
    app: &AppHandle,
    conversation_id: &str,
    project_id: Option<&str>,
) -> Result<PathBuf, String> {
    if let Some(path) = project_cli_root_for_conversation(app, conversation_id, project_id) {
        return Ok(path);
    }

    conversation_workspace_path(app, conversation_id)
}

fn project_cli_root_for_conversation(
    app: &AppHandle,
    conversation_id: &str,
    project_id: Option<&str>,
) -> Option<PathBuf> {
    if let Ok(conversation) = load_conversation(app, conversation_id) {
        if let Ok(Some(project)) = resolve_conversation_project(app, &conversation) {
            return cli_dir_if_exists_from_project(project);
        }
        return None;
    }
    let project_id = project_id.filter(|id| !id.trim().is_empty())?;
    cli_dir_if_exists_from_project(find_project_by_id(app, project_id).ok()?)
}

fn cli_dir_if_exists_from_project(project: ChatProject) -> Option<PathBuf> {
    let root = project.root_path.filter(|path| !path.trim().is_empty())?;
    cli_dir_if_exists(PathBuf::from(root))
}

/// Project roots are stored via `fs::canonicalize` (`\\?\E:\...` on Windows).
/// External CLIs (Node `dsh` especially) need the non-verbatim spelling.
pub fn cli_dir_if_exists(path: PathBuf) -> Option<PathBuf> {
    let stripped = strip_windows_verbatim_prefix(path);
    stripped.is_dir().then_some(stripped)
}

/// 会话工作区路径，**不创建目录**。
///
/// 目录要不要建取决于调用方：真的要往里写（spawn CLI）才建，只是想知道路径
/// （dock 展示 / 上下文统计）就别建。原来这里无条件 `create_dir_all`，于是每开一个
/// 新会话都留一个空壳——实测 110 个 conv 目录里 105 个是空的。
fn conversation_workspace_path(app: &AppHandle, conversation_id: &str) -> Result<PathBuf, String> {
    Ok(conversations_dir(app)?
        .parent()
        .ok_or_else(|| "chat data root unavailable".to_string())?
        .join("chat-workspaces")
        .join(conversation_id))
}

/// `resolve_effective_cwd` + 确保目录存在。**只给真的要在里面跑进程/写文件的路径用**
/// （spawn 外部 CLI、压缩）。项目根不会被创建（配错路径不该被悄悄建成空目录）。
pub fn ensure_effective_cwd(
    app: &AppHandle,
    conversation_id: &str,
    project_id: Option<&str>,
) -> Result<PathBuf, String> {
    let path = resolve_effective_cwd(app, conversation_id, project_id)?;
    // 项目根由用户配置，存在性已在 resolve 里校验过（is_dir 才返回）；只有会话工作区
    // 这条分支需要按需创建。
    if !path.is_dir() {
        std::fs::create_dir_all(&path).map_err(|e| format!("create workspace: {e}"))?;
    }
    Ok(path)
}

/// 检测（模型/斜杠命令探测）用的 cwd。与会话执行 cwd（`resolve_effective_cwd`，每会话独立
/// workspace）不同：探测结果按 `(agent, cwd)` 缓存，若沿用每会话目录，**每个新会话都是新缓存键**
/// → 必然冷探测 15-25s，下拉框长时间只有 Default。因此非项目会话统一用一个全局稳定 scope
/// （参照 Paseo 的 GLOBAL_PROVIDER_SNAPSHOT_KEY）：首次探测后全 App 命中热缓存。
/// 只有绑定项目的会话才用项目根（opencode 等模型目录随项目配置变化的场景）。
/// 会话未落盘/不存在时也落到全局 scope，绝不硬失败。
pub fn resolve_detection_cwd(
    app: &AppHandle,
    conversation_id: Option<&str>,
) -> Result<PathBuf, String> {
    if let Some(conversation_id) = conversation_id.filter(|id| !id.trim().is_empty()) {
        if let Some(path) = project_cli_root_for_conversation(app, conversation_id, None) {
            return Ok(path);
        }
    }
    let base = conversations_dir(app)?
        .parent()
        .ok_or_else(|| "chat data root unavailable".to_string())?
        .join("chat-workspaces")
        .join("__global__");
    std::fs::create_dir_all(&base).map_err(|e| format!("create detection workspace: {e}"))?;
    Ok(base)
}

pub fn extra_allowed_dirs_for_agent(
    def: &RuntimeAgentDef,
    skill_scan_paths: &[String],
) -> Vec<String> {
    if def.id == "codex" {
        return Vec::new();
    }
    skill_scan_paths
        .iter()
        .filter(|p| !p.trim().is_empty() && Path::new(p).is_dir())
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::cli_dir_if_exists;
    use std::path::PathBuf;

    #[test]
    fn cli_dir_if_exists_rejects_missing_paths() {
        assert_eq!(
            cli_dir_if_exists(PathBuf::from(
                r"\\?\E:\this-folder-does-not-exist-kivio-cwd-test"
            )),
            None
        );
    }

    #[test]
    fn cli_dir_if_exists_accepts_a_real_directory() {
        let cwd = std::env::current_dir().expect("cwd");
        #[cfg(windows)]
        let verbatim = PathBuf::from(format!(r"\\?\{}", cwd.display()));
        #[cfg(not(windows))]
        let verbatim = cwd.clone();
        let resolved = cli_dir_if_exists(verbatim).expect("cwd is a directory");
        assert!(resolved.is_dir());
        #[cfg(windows)]
        assert!(
            !resolved.to_string_lossy().starts_with(r"\\?\"),
            "cli cwd must not keep a verbatim prefix: {}",
            resolved.display()
        );
    }
}
