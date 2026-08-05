use std::path::{Path, PathBuf};

use tauri::AppHandle;

use crate::chat::storage::{conversations_dir, find_project_by_id, load_conversation};
use crate::external_agents::types::RuntimeAgentDef;

pub fn resolve_effective_cwd(
    app: &AppHandle,
    conversation_id: &str,
    project_id: Option<&str>,
) -> Result<PathBuf, String> {
    if let Some(project_id) = project_id.filter(|id| !id.trim().is_empty()) {
        if let Ok(project) = find_project_by_id(app, project_id) {
            if let Some(root) = project.root_path.filter(|p| !p.trim().is_empty()) {
                let path = PathBuf::from(root);
                if path.is_dir() {
                    return Ok(path);
                }
            }
        }
    }

    let base = conversations_dir(app)?
        .parent()
        .ok_or_else(|| "chat data root unavailable".to_string())?
        .join("chat-workspaces")
        .join(conversation_id);
    std::fs::create_dir_all(&base).map_err(|e| format!("create workspace: {e}"))?;
    Ok(base)
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
        if let Ok(conversation) = load_conversation(app, conversation_id) {
            if let Some(project_id) = conversation
                .project_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
            {
                if let Ok(project) = find_project_by_id(app, project_id) {
                    if let Some(root) = project.root_path.filter(|p| !p.trim().is_empty()) {
                        let path = PathBuf::from(root);
                        if path.is_dir() {
                            return Ok(path);
                        }
                    }
                }
            }
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
