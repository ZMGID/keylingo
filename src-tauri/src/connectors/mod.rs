//! 连接器：目录化 + 一键授权的外部数据源接入。
//!
//! Phase A（token 类）只在前端把一条带 Authorization header 的 ChatMcpServer
//! 写入 settings.chat_tools.servers，无需后端逻辑。Phase B 在这里实现远程 MCP 的
//! OAuth（PKCE + 动态客户端注册 DCR + loopback 回调）+ token 刷新，让 Notion 及
//! 任意支持 DCR 的远程 MCP 一键授权连接。

pub mod oauth;
pub mod obsidian;

pub use obsidian::list_obsidian_vaults_cmd;

use tauri::{AppHandle, State};

use crate::settings::ChatMcpServer;
use crate::state::AppState;

/// 内置 OAuth 连接器目录：catalog_id → (展示名, MCP resource URL)。
/// 与前端 `connectorCatalog.ts` 中 authKind:'oauth' 的项保持一致。
fn builtin_oauth_url(catalog_id: &str) -> Option<(&'static str, &'static str)> {
    match catalog_id {
        "notion" => Some(("Notion", "https://mcp.notion.com/mcp")),
        _ => None,
    }
}

/// 跑一次完整 OAuth 流程，返回物化好的 ChatMcpServer。
///
/// - 传 `catalog_id`：命中内置 OAuth 连接器（如 notion），用其 resource URL。
/// - 传 `url`（+可选 `name`）：自定义 OAuth 连接器，直接用该 URL。
///
/// 不直接写 settings——返回给前端，由前端合并进 `chat_tools.servers` 并保存
/// （沿用既有「前端改 settings → save_settings」模式）。
#[tauri::command]
pub async fn connector_oauth_connect(
    app: AppHandle,
    state: State<'_, AppState>,
    catalog_id: Option<String>,
    url: Option<String>,
    name: Option<String>,
) -> Result<ChatMcpServer, String> {
    let http = state.http.clone();

    if let Some(catalog_id) = catalog_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        // 命中内置目录用其 resource URL；未命中但前端传了 url（catalog 项自带 url）时用 url，
        // connector_id 仍取 catalog_id 以保留图标/展示映射。
        if let Some((default_name, resource_url)) = builtin_oauth_url(catalog_id) {
            return oauth::run_oauth_connect(&app, &http, catalog_id, default_name, resource_url)
                .await;
        }
        let resource_url = url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("Unknown OAuth connector: {catalog_id}"))?;
        let display_name = name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(catalog_id);
        return oauth::run_oauth_connect(&app, &http, catalog_id, display_name, resource_url).await;
    }

    let resource_url = url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "OAuth connector requires a catalog id or URL".to_string())?;
    let display_name = name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Custom Connector");
    let connector_id = format!("custom-{}", slugify(display_name));
    oauth::run_oauth_connect(&app, &http, &connector_id, display_name, resource_url).await
}

/// 与前端 `slugify` 对齐：转小写、非字母数字折叠为连字符、去首尾连字符；空则 "custom"。
fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in name.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "custom".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_matches_frontend() {
        assert_eq!(slugify("My Server"), "my-server");
        assert_eq!(slugify("  Acme!! Corp  "), "acme-corp");
        assert_eq!(slugify("***"), "custom");
        assert_eq!(slugify(""), "custom");
    }

    #[test]
    fn builtin_oauth_url_known_and_unknown() {
        assert!(builtin_oauth_url("notion").is_some());
        assert!(builtin_oauth_url("github").is_none());
    }
}
