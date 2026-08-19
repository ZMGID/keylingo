//! 插件附属能力生命周期：Skill 归属门闸 + MCP server 挂/卸。
//!
//! 规则：
//! - **安装**：落盘 Skill 文件；不启用；不挂 MCP
//! - **启用**：Skill 进入扫描 + 短提示 + PATH；若 catalog 声明 MCP 则 upsert 并 enabled=true
//! - **关闭**：Skill 退出扫描 + 去提示 + 去 PATH；MCP server enabled=false 并断连
//! - **卸载**：先走关闭逻辑，再删目录；移除 plugin 名下 MCP 条目

use tauri::{AppHandle, Manager};

use super::catalog::{catalog_plugin, CatalogPlugin, PLUGIN_CATALOG};
use super::state::{is_enabled, is_installed, resolve_binary, resolve_binary_for_status};
use crate::settings::{persist_settings, ChatMcpServer, Settings};
use crate::state::AppState;

/// settings 里 MCP server id：`plugin-<plugin_id>`
pub fn plugin_mcp_server_id(plugin_id: &str) -> String {
    format!("plugin-{plugin_id}")
}

/// connector_id 标记插件归属：连接器页排除；MCP 已安装列表展示但只读（开关在插件页）。
pub fn plugin_mcp_connector_id(plugin_id: &str) -> String {
    format!("plugin:{plugin_id}")
}

/// 某 skill_id 是否由某个插件「拥有」（不论当前是否启用）。
pub fn skill_owned_by_plugin(skill_id: &str) -> Option<&'static str> {
    for plugin in PLUGIN_CATALOG {
        if plugin.skill_ids.iter().any(|id| *id == skill_id) {
            return Some(plugin.id);
        }
    }
    None
}

/// 插件附属 skill 是否应对 agent / 列表可见：仅「已安装且启用」时 true。
pub fn plugin_skill_available(skill_id: &str) -> bool {
    match skill_owned_by_plugin(skill_id) {
        None => true, // 非插件 skill，不由本门闸拦截
        Some(plugin_id) => is_enabled(plugin_id) && is_installed(plugin_id),
    }
}

/// 构建插件 MCP 配置（stdio：`binary mcp`）。binary 用绝对路径，避免依赖 PATH。
pub fn materialize_mcp_server(plugin: &CatalogPlugin) -> Option<ChatMcpServer> {
    let mcp = plugin.mcp.as_ref()?;
    // 启用时再探测一次，避免 PATH 未刷新 / 仅在默认安装目录时失败
    let binary = resolve_binary_for_status(plugin.id).or_else(|| resolve_binary(plugin.id))?;
    // OfficeCLI：MCP 改文档默认只在内存 resident 里，磁盘延迟 2–10s 才刷。
    // 设 each 保证每次工具返回前已落盘，Kivio 才能用 `view html` 刷实时预览。
    let mut env = std::collections::HashMap::new();
    if plugin.id == "officecli" {
        env.insert("OFFICECLI_RESIDENT_FLUSH".to_string(), "each".to_string());
    }
    Some(ChatMcpServer {
        id: plugin_mcp_server_id(plugin.id),
        name: format!("{} (插件)", plugin.name),
        enabled: true,
        transport: "stdio".to_string(),
        url: String::new(),
        command: binary.display().to_string(),
        args: mcp.args.iter().map(|s| (*s).to_string()).collect(),
        env,
        headers: std::collections::HashMap::new(),
        cwd: None,
        enabled_tools: Vec::new(),
        connector_id: Some(plugin_mcp_connector_id(plugin.id)),
        auth: None,
    })
}

/// 启用：挂 MCP（若有）+ 确保 chat 工具总开关 + 落盘 settings。
///
/// 注意：README 里的 `officecli mcp claude/cursor` 是给那些客户端写配置用的；
/// Kivio **不要**跑那些命令，而是在这里注册 stdio：`{binary绝对路径} mcp`。
pub fn apply_enable_side_effects(
    app: &AppHandle,
    state: &AppState,
    plugin_id: &str,
) -> Result<(), String> {
    let plugin = catalog_plugin(plugin_id).ok_or_else(|| format!("unknown plugin: {plugin_id}"))?;

    let updated = {
        let mut guard = state.settings_write();
        // 插件启用后 Agent 需要工具循环；总开关关掉则 MCP 不会被收集
        if !guard.chat_tools.enabled {
            guard.chat_tools.enabled = true;
        }
        // Skill 激活 + 终端调用依赖这些 native 开关
        guard.chat_tools.native_tools.skill_runtime = true;
        guard.chat_tools.native_tools.run_command = true;

        if plugin.mcp.is_some() {
            let server = materialize_mcp_server(plugin)
                .ok_or_else(|| "插件二进制不可用，无法挂载 MCP".to_string())?;
            upsert_plugin_mcp_server(&mut guard, server);
        }
        guard.clone()
    };
    persist_settings(app, &updated)?;
    Ok(())
}

/// 关闭 / 卸载前：禁用并可选删除 MCP 条目，断连会话。
pub async fn apply_disable_side_effects(
    app: &AppHandle,
    state: &AppState,
    plugin_id: &str,
    remove_server: bool,
) -> Result<(), String> {
    let server_id = plugin_mcp_server_id(plugin_id);

    let updated = {
        let mut guard = state.settings_write();
        let servers = &mut guard.chat_tools.servers;
        if remove_server {
            servers.retain(|s| s.id != server_id);
        } else if let Some(slot) = servers.iter_mut().find(|s| s.id == server_id) {
            slot.enabled = false;
        }
        guard.clone()
    };
    persist_settings(app, &updated)?;

    state.mcp_disconnect_server(&server_id).await;
    // OfficeCLI 等：关掉插件时一并停掉 live preview 进程
    if plugin_id == "officecli" {
        super::preview::stop_all_previews();
    }
    Ok(())
}

fn upsert_plugin_mcp_server(settings: &mut Settings, server: ChatMcpServer) {
    let id = server.id.clone();
    if let Some(slot) = settings.chat_tools.servers.iter_mut().find(|s| s.id == id) {
        // 保留用户曾改过的 enabled_tools；其余用最新 binary 路径 / env 覆盖
        let kept_tools = slot.enabled_tools.clone();
        *slot = server;
        slot.enabled_tools = kept_tools;
        slot.enabled = true;
    } else {
        settings.chat_tools.servers.push(server);
    }
}

/// 已启用 OfficeCLI 时，确保 MCP 带上 FLUSH=each（升级后无需用户手动重开插件）。
/// 若改了 env，断开旧 MCP 会话，下次工具调用会按新 env 拉起。
pub fn ensure_officecli_mcp_flush_env(app: &AppHandle, state: &AppState) {
    if !is_enabled("officecli") || !is_installed("officecli") {
        return;
    }
    let Some(catalog) = catalog_plugin("officecli") else {
        return;
    };
    let Some(server) = materialize_mcp_server(catalog) else {
        return;
    };
    let server_id = server.id.clone();
    let need = {
        let guard = state.settings_read();
        match guard.chat_tools.servers.iter().find(|s| s.id == server_id) {
            Some(existing) => {
                existing
                    .env
                    .get("OFFICECLI_RESIDENT_FLUSH")
                    .map(String::as_str)
                    != Some("each")
                    || existing.command != server.command
            }
            None => true,
        }
    };
    if !need {
        return;
    }
    let updated = {
        let mut guard = state.settings_write();
        upsert_plugin_mcp_server(&mut guard, server);
        guard.clone()
    };
    let _ = persist_settings(app, &updated);
    // 旧 stdio 子进程没有 FLUSH=each；异步断开，下一轮工具调用会按新 env 重建
    let app2 = app.clone();
    let sid = server_id;
    tauri::async_runtime::spawn(async move {
        let state = app2.state::<AppState>();
        state.mcp_disconnect_server(&sid).await;
    });
}
