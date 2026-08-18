//! 能力插件（领域 CLI 等）：**优先用官方安装器**；启用开关由插件页统一管理。
//!
//! - 点「安装」→ 运行 catalog 里 GitHub README 的官方命令（按平台）
//! - 「让 AI 代装」可选：前端开对话，把 `install_doc` 交给 Agent
//! - 检测 PATH / 托管目录判断是否已安装
//! - **启用** 后才注入 PATH 提示、Skill 门闸、MCP；关闭则全部卸下
//! - 与独立 Skill 页、连接器 MCP 分离

mod catalog;
mod install;
mod lifecycle;
mod preview;
mod state;

pub use catalog::{catalog_plugin, CatalogPlugin, PLUGIN_CATALOG};
pub use install::{
    get_install_brief, list_plugin_statuses, list_plugin_statuses_cached_with_state,
    list_plugin_statuses_with_state, run_official_install, set_plugin_enabled, uninstall_plugin,
    PluginActionResult, PluginInstallBrief, PluginStatus,
};
pub use lifecycle::{
    ensure_officecli_mcp_flush_env, plugin_skill_available, skill_owned_by_plugin,
};
pub use preview::{note_after_officecli_tool, stop_all_previews};
pub use state::{
    enabled_bin_dirs, enabled_path_env, enabled_skill_roots, enabled_system_prompt, is_enabled,
    is_installed, plugins_root, resolve_binary, resolve_binary_for_status,
};

use tauri::{AppHandle, State};

use crate::state::AppState;

#[tauri::command]
pub fn plugins_list(state: State<'_, AppState>) -> Result<Vec<PluginStatus>, String> {
    list_plugin_statuses_with_state(&state)
}

/// 无 spawn 的缓存态列表：前端首屏立即渲染，再拉 `plugins_list` 覆盖为精确探测。
#[tauri::command]
pub fn plugins_list_cached(state: State<'_, AppState>) -> Result<Vec<PluginStatus>, String> {
    list_plugin_statuses_cached_with_state(&state)
}

/// 返回交给 Agent 的安装任务（规范文档 + 用户消息）。前端据此开新对话并自动发送。
#[tauri::command]
pub fn plugins_install_brief(id: String) -> Result<PluginInstallBrief, String> {
    get_install_brief(&id)
}

/// 运行当前系统对应的 GitHub README 安装命令。
#[tauri::command]
pub async fn plugins_run_official_install(id: String) -> Result<PluginActionResult, String> {
    run_official_install(&id).await
}

#[tauri::command]
pub async fn plugins_set_enabled(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<PluginActionResult, String> {
    set_plugin_enabled(&app, &state, &id, enabled).await
}

#[tauri::command]
pub async fn plugins_uninstall(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<PluginActionResult, String> {
    uninstall_plugin(&app, &state, &id).await
}
