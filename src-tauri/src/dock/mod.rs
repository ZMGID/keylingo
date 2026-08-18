//! 右侧 Dock（文件树 + Git 面板 + 终端面板）后端：文件系统命令、Git 命令、
//! PTY 终端会话、workspace 变更监听。
//! 架构思路参考 LiveAgent 的 Right Dock，按 kivio 惯例精简重写（命令返回
//! `Result<T, String>`，响应结构体 serde camelCase）。

pub mod fs;
pub mod git;
pub mod terminal;
pub mod watch;

/// 解析 Dock 显示的工作目录。关键约束：必须与 agent 实际写文件的目录一致——
/// - 外部 Agent（claude/codex/kimi…）：`resolve_effective_cwd` → 项目根，否则
///   `chat-workspaces/<conversation_id>`；
/// - 内置 runtime：`resolve_conversation_working_directory` → 项目根，否则
///   `<nativeTools.workingDirectory>/<conversation_id>`（默认 `~/Kivio/workspace/<id>`）。
/// 一律走前者会让内置 runtime 的无项目会话盯着一个 agent 从不写入的目录（树永远为空）。
#[tauri::command]
pub async fn dock_resolve_cwd(
    app: tauri::AppHandle,
    conversation_id: Option<String>,
    project_id: Option<String>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let conversation_id = conversation_id
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty());

        // 会话已落盘：按其 runtime 走对应的解析器，和 agent 写文件的目录严格一致。
        if let Some(conv_id) = conversation_id.as_deref() {
            if let Ok(conversation) = crate::chat::storage::load_conversation(&app, conv_id) {
                if conversation.agent_runtime.is_external() {
                    // 右栏是用户主动打开的，且 `dock_fs_list` 要求 workdir 已存在 ⇒ 这里按需建。
                    // 与下面内置分支同一口径（项目根不建，避免把配错的路径悄悄建成空目录）。
                    let path = crate::external_agents::workspace::ensure_effective_cwd(
                        &app,
                        &conversation.id,
                        conversation.project_id.as_deref(),
                    )?;
                    return Ok(path.to_string_lossy().to_string());
                }
                let settings = crate::settings::load_settings(&app);
                let path = crate::chat::storage::resolve_conversation_working_directory(
                    &app,
                    &conversation,
                    &settings.chat_tools.native_tools.working_directory,
                )?;
                // 非项目会话的工作目录可能还没被任何写入创建；项目根不主动建，
                // 避免把配错的路径悄悄建成空目录。
                if crate::chat::storage::resolve_conversation_project(&app, &conversation)?
                    .is_none()
                {
                    std::fs::create_dir_all(&path)
                        .map_err(|e| format!("create dock workspace: {e}"))?;
                }
                return Ok(path.to_string_lossy().to_string());
            }
        }

        // 草稿态（会话未落盘）：默认内置 runtime。项目根优先（与内置解析顺序一致）。
        if let Some(project_id) = project_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            if let Ok(project) = crate::chat::storage::find_project_by_id(&app, project_id) {
                if let Some(root) = project.root_path.filter(|p| !p.trim().is_empty()) {
                    if let Some(path) =
                        crate::external_agents::workspace::cli_dir_if_exists(std::path::PathBuf::from(
                            root,
                        ))
                    {
                        return Ok(path.to_string_lossy().to_string());
                    }
                }
            }
        }

        // 无项目草稿：内置 runtime 工作根下的会话目录（默认 ~/Kivio/workspace/<id>）。
        let settings = crate::settings::load_settings(&app);
        let configured = settings.chat_tools.native_tools.working_directory;
        let working_root = {
            let raw = configured.trim();
            if raw.is_empty() {
                crate::settings::default_chat_working_directory()
            } else {
                raw.to_string()
            }
        };
        let conv_id = conversation_id.unwrap_or_else(|| "__global__".to_string());
        // conversation_workspace_directory 只接受 conv_ 前缀 id；其余（如 __global__）
        // 手动拼到工作根下。
        let path = crate::native_tools::conversation_workspace_directory(&working_root, &conv_id)
            .unwrap_or_else(|_| std::path::PathBuf::from(&working_root).join(&conv_id));
        std::fs::create_dir_all(&path).map_err(|e| format!("create dock workspace: {e}"))?;
        Ok(path.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| format!("dock_resolve_cwd join: {e}"))?
}
