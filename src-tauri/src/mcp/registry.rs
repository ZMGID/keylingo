use std::{collections::HashMap, fs, path::Path, time::Duration};

use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::oneshot;

use crate::{
    native_tools::{
        resolve_tool_read_path, resolve_tool_write_path, FileMutationResult, NativeToolWorkspace,
        ReadFileResult,
    },
    settings::{ChatMcpServer, WebSearchProvider},
    state::AppState,
};

use super::types::{
    list_native_builtin_tool_defs, mixer_generate_image_tool, native_skill_tools,
    tool_definition_from_mcp, ChatToolDefinition, McpToolCallResult,
};

#[derive(Debug, Clone)]
pub struct NativeToolContext {
    pub conversation_id: String,
    pub message_id: String,
    pub tool_call_id: Option<String>,
    /// Parent run id of the agent loop issuing this call. Used by sub-agent
    /// management tools to address the parent tool card and cascade
    /// cancellation. Empty when not running under an agent loop.
    pub run_id: String,
    /// Generation of the issuing agent loop (for cancellation cascade).
    pub generation: u64,
    /// Sub-agent nesting depth of the issuing agent loop (0 = top-level).
    pub depth: u8,
}
const MAX_PYTHON_INPUT_FILE_BYTES: u64 = 100 * 1024 * 1024;
const MAX_PYTHON_INPUT_FILES: usize = 8;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PythonInputFilePayload {
    name: String,
    data_base64: String,
    size_bytes: u64,
}

fn sanitize_python_input_name(path: &Path) -> String {
    let raw = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("input");
    // Keep Unicode letters/digits so a host file like 销售报表.xlsx is still
    // that name inside Pyodide (`KIVIO_INPUT_FILES`), not ________.xlsx.
    let sanitized = raw
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '.' | '-' | '_' | ' ') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let trimmed = sanitized.trim_matches(['.', ' ', '_']).trim();
    if trimmed.is_empty() {
        "input".to_string()
    } else {
        trimmed.to_string()
    }
}

fn collect_python_input_files(
    _app: &AppHandle,
    workspace: &NativeToolWorkspace,
    arguments: &Value,
) -> Result<Vec<PythonInputFilePayload>, String> {
    let Some(files) = arguments.get("files") else {
        return Ok(Vec::new());
    };
    let files = files
        .as_array()
        .ok_or_else(|| "run_python files must be an array of file paths".to_string())?;
    if files.len() > MAX_PYTHON_INPUT_FILES {
        return Err(format!(
            "run_python supports at most {MAX_PYTHON_INPUT_FILES} input files"
        ));
    }

    let mut payloads = Vec::new();
    for file in files {
        let raw_path = file
            .as_str()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| "run_python files entries must be non-empty strings".to_string())?;
        let path = resolve_tool_read_path(workspace, raw_path)?;
        if !path.is_file() {
            return Err(format!("run_python input is not a file: {raw_path}"));
        }
        let metadata =
            fs::metadata(&path).map_err(|err| format!("Read input metadata failed: {err}"))?;
        if metadata.len() > MAX_PYTHON_INPUT_FILE_BYTES {
            return Err(format!(
                "run_python input file too large: {} bytes (max {MAX_PYTHON_INPUT_FILE_BYTES})",
                metadata.len()
            ));
        }
        let bytes = fs::read(&path).map_err(|err| format!("Read input file failed: {err}"))?;
        payloads.push(PythonInputFilePayload {
            name: sanitize_python_input_name(&path),
            data_base64: general_purpose::STANDARD.encode(bytes),
            size_bytes: metadata.len(),
        });
    }
    Ok(payloads)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpListToolsResult {
    pub success: bool,
    pub tools: Vec<ChatToolDefinition>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTestResult {
    pub success: bool,
    pub tools: Vec<ChatToolDefinition>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpImportResult {
    pub success: bool,
    pub servers: Vec<ChatMcpServer>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CursorMcpJson {
    #[serde(default, rename = "mcpServers")]
    mcp_servers: HashMap<String, CursorMcpServer>,
}

#[derive(Debug, Deserialize)]
struct CursorMcpServer {
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    transport: Option<String>,
    #[serde(default, rename = "type")]
    server_type: Option<String>,
}

#[derive(Debug, Default)]
pub struct EnabledToolCatalog {
    pub tools: Vec<ChatToolDefinition>,
    pub unavailable_mcp_servers: Vec<String>,
}

#[tauri::command]
pub async fn chat_mcp_list_tools(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<McpListToolsResult, String> {
    let catalog = list_enabled_tool_catalog(&app, &state).await;
    Ok(McpListToolsResult {
        success: true,
        tools: catalog.tools,
        error: None,
    })
}

pub async fn list_enabled_tool_catalog(app: &AppHandle, state: &AppState) -> EnabledToolCatalog {
    let settings = state.settings_read().clone();
    let mut tools = list_native_builtin_tool_defs(
        &settings.chat_tools.native_tools,
        web_search_configured(&settings),
        crate::settings::chat_memory_tools_enabled(&settings),
    );
    if let Some((provider_id, model)) = settings.image_generation_model() {
        let mut tool = mixer_generate_image_tool();
        let provider_name = settings
            .get_provider(&provider_id)
            .map(|provider| {
                if provider.name.trim().is_empty() {
                    provider.id.clone()
                } else {
                    provider.name.clone()
                }
            })
            .unwrap_or(provider_id);
        tool.server_id = Some(format!("{provider_name} / {model}"));
        tools.push(tool);
    }

    if let Some((provider_id, model)) = settings.advisor_model() {
        let mut tool = super::types::native_advisor_tool();
        let provider_name = settings
            .get_provider(&provider_id)
            .map(|provider| {
                if provider.name.trim().is_empty() {
                    provider.id.clone()
                } else {
                    provider.name.clone()
                }
            })
            .unwrap_or(provider_id);
        tool.server_id = Some(format!("{provider_name} / {model}"));
        tools.push(tool);
    }

    let (mcp_tools, unavailable_mcp_servers) =
        collect_enabled_mcp_tool_defs(state, Some(app), &settings).await;
    tools.extend(mcp_tools);
    tools.extend(list_skill_tool_defs(&settings));

    EnabledToolCatalog {
        tools,
        unavailable_mcp_servers,
    }
}

/// Discover the enabled MCP servers once. A live schema is preferred; when a
/// connection fails, one matching last-known schema is the only fallback.
/// There is deliberately no aggregate tool-list cache: connected sessions
/// already retain their schema, and failed sessions already have reconnect
/// backoff, so another cache only creates stale-state invalidation paths.
///
/// 每个 server 的现场 listing 有 [`WARM_TOOL_LIST_TIMEOUT`] 的等待上限：慢/坏 server
/// 不再拖住整轮工具收集，超时与失败同路降级到落盘快照 / unavailable。
pub(crate) async fn collect_enabled_mcp_tool_defs(
    state: &AppState,
    sink: super::manager::McpEventSink<'_>,
    settings: &crate::settings::Settings,
) -> (Vec<ChatToolDefinition>, Vec<String>) {
    let servers = eligible_mcp_servers(settings);
    let listings = servers.iter().map(|server| async move {
        let result = list_tools_bounded(state, sink, server).await;
        (*server, result)
    });

    let mut tools = Vec::new();
    let mut unavailable = Vec::new();
    for (server, result) in futures::future::join_all(listings).await {
        match result {
            Ok(server_tools) => tools.extend(tools_from_mcp(server, server_tools)),
            Err(err) => match state.mcp_cached_tools(server).await {
                Some(last_known) => {
                    eprintln!(
                        "MCP server {} discovery failed; using its last-known tool schema: {err}",
                        server.name
                    );
                    tools.extend(tools_from_mcp(server, last_known));
                }
                None => {
                    eprintln!("MCP server {} discovery failed: {err}", server.name);
                    unavailable.push(server.name.clone());
                }
            },
        }
    }
    (tools, unavailable)
}

/// 单个 server 现场 tools/list 的等待上限。已连接的 server 走池命中微秒级返回，不受影响；
/// 未连上的 server 最多等这么久，之后本轮转入快照兜底 / unavailable。
pub(crate) const WARM_TOOL_LIST_TIMEOUT: Duration = Duration::from_secs(3);

/// 有界等待的现场 listing。生产路径（sink 带 AppHandle）把 listing spawn 成独立后台任务，
/// 超时只**放弃等待、不取消任务**——慢 server 后台继续连完，成功后 `remember_mcp_tools`
/// 写快照 + emit Connected，下一轮直接可用。无 AppHandle（单测 / headless sink）拿不到
/// `'static` 的状态引用，退化为对 future 本体 timeout（超时即取消，对测试语义足够）。
async fn list_tools_bounded(
    state: &AppState,
    sink: super::manager::McpEventSink<'_>,
    server: &ChatMcpServer,
) -> Result<Vec<crate::mcp::types::McpTool>, String> {
    match sink {
        Some(app) => {
            let app = app.clone();
            let owned = server.clone();
            let task = tauri::async_runtime::spawn(async move {
                let state = app.state::<AppState>();
                state.mcp_list_tools(Some(&app), &owned).await
            });
            match tokio::time::timeout(WARM_TOOL_LIST_TIMEOUT, task).await {
                Ok(Ok(result)) => result,
                Ok(Err(join_err)) => Err(format!("MCP listing task failed: {join_err}")),
                Err(_) => Err(format!(
                    "tools/list did not answer within {}s; connection keeps warming in the background",
                    WARM_TOOL_LIST_TIMEOUT.as_secs()
                )),
            }
        }
        None => tokio::time::timeout(WARM_TOOL_LIST_TIMEOUT, state.mcp_list_tools(sink, server))
            .await
            .unwrap_or_else(|_| {
                Err(format!(
                    "tools/list did not answer within {}s",
                    WARM_TOOL_LIST_TIMEOUT.as_secs()
                ))
            }),
    }
}

/// 预热目标筛选：None = 全部 runtime-eligible 的 server；Some = 其中 id 命中的子集。
/// 停用 / 全局工具开关关闭的 server 一律不预热。
fn select_warmup_servers(
    settings: &crate::settings::Settings,
    server_ids: Option<&[String]>,
) -> Vec<ChatMcpServer> {
    eligible_mcp_servers(settings)
        .into_iter()
        .filter(|server| server_ids.is_none_or(|ids| ids.iter().any(|id| id == &server.id)))
        .cloned()
        .collect()
}

/// 后台预热 MCP 连接：对指定（None = 全部启用）的 server 各自发起一次「连接 + tools/list」。
/// fire-and-forget：立即返回 Ok，结果经 `mcp-server-state` 事件推送呈现（连接中 → 已连接/错误）；
/// 成功的 tools/list 会顺手写内存 + 落盘快照。连接池单飞门闩保证与手动「测试连接」/
/// 对话触发的连接并发无害；单个 server 失败只打日志 + emit Error，不影响其他 server。
#[tauri::command]
pub async fn chat_mcp_warmup(
    app: AppHandle,
    state: State<'_, AppState>,
    server_ids: Option<Vec<String>>,
) -> Result<(), String> {
    let settings = state.settings_read().clone();
    for server in select_warmup_servers(&settings, server_ids.as_deref()) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let state = app.state::<AppState>();
            if let Err(err) = state.mcp_list_tools(Some(&app), &server).await {
                eprintln!("MCP warmup for {} failed: {err}", server.name);
            }
        });
    }
    Ok(())
}

/// Apply the server's explicit per-tool setting. An empty list means all tools.
pub(crate) fn tools_from_mcp(
    server: &ChatMcpServer,
    tools: Vec<crate::mcp::types::McpTool>,
) -> Vec<ChatToolDefinition> {
    let allowed = server
        .enabled_tools
        .iter()
        .map(|tool| tool.as_str())
        .collect::<Vec<_>>();
    tools
        .into_iter()
        .filter(|tool| allowed.is_empty() || allowed.contains(&tool.name.as_str()))
        .map(|tool| tool_definition_from_mcp(server, tool))
        .collect()
}

#[tauri::command]
pub async fn chat_mcp_test_server(
    state: State<'_, AppState>,
    server: ChatMcpServer,
    timeout_ms: Option<u64>,
) -> Result<McpTestResult, String> {
    // tauri command 必须返回 Result 才能用 State<'_, _>；前端仍读 McpTestResult（永不 Err）。
    Ok(
        match list_server_tools(&state.http, &server, timeout_ms.unwrap_or(60_000)).await {
            Ok(tools) => McpTestResult {
                success: true,
                tools,
                error: None,
            },
            Err(err) => McpTestResult {
                success: false,
                tools: Vec::new(),
                error: Some(err),
            },
        },
    )
}

#[tauri::command]
pub fn chat_mcp_import_json(path: String) -> McpImportResult {
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) => {
            return McpImportResult {
                success: false,
                servers: Vec::new(),
                error: Some(format!("Read mcp.json failed: {err}")),
            }
        }
    };
    let parsed: CursorMcpJson = match serde_json::from_str(&raw) {
        Ok(parsed) => parsed,
        Err(err) => {
            return McpImportResult {
                success: false,
                servers: Vec::new(),
                error: Some(format!("Parse mcp.json failed: {err}")),
            }
        }
    };

    let servers = parsed
        .mcp_servers
        .into_iter()
        .map(|(name, server)| ChatMcpServer {
            id: format!("mcp-{}", uuid::Uuid::new_v4()),
            name,
            enabled: false,
            transport: normalize_imported_transport(&server),
            url: server.url,
            command: server.command,
            args: server.args,
            env: server.env,
            headers: server.headers,
            cwd: server.cwd,
            enabled_tools: Vec::new(),
            connector_id: None,
            auth: None,
        })
        .collect();

    McpImportResult {
        success: true,
        servers,
        error: None,
    }
}

/// 一个 CLI 的 MCP 扫描结果。`available` = 该 CLI 的配置文件存在（与是否解析出
/// server 无关）；解析失败降级为 `available:true, servers:[]`，不炸整个扫描。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CliMcpGroup {
    available: bool,
    servers: Vec<ChatMcpServer>,
}

/// 四个本地 CLI（Claude Code / Codex / OpenCode / Pi）的 MCP 扫描结果。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CliImportScan {
    claude: CliMcpGroup,
    codex: CliMcpGroup,
    opencode: CliMcpGroup,
    pi: CliMcpGroup,
}

/// Codex `~/.codex/config.toml` 的 `[mcp_servers.<name>]` 子表（全 stdio）。
/// 忽略 `enabled` / `startup_timeout_sec` 等字段。
#[derive(Debug, Deserialize)]
struct CodexMcpServer {
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodexConfig {
    #[serde(default)]
    mcp_servers: HashMap<String, CodexMcpServer>,
}

/// OpenCode `~/.config/opencode/opencode.json` 的 `mcp.<name>`。
/// `type:"local"` → stdio（`command` 是 `[cmd, ...args]` 数组）；
/// `type:"remote"` → streamable_http（`url` + `headers`）。
#[derive(Debug, Deserialize)]
struct OpencodeMcpServer {
    #[serde(default, rename = "type")]
    server_type: Option<String>,
    #[serde(default)]
    command: Vec<String>,
    #[serde(default)]
    environment: HashMap<String, String>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct OpencodeConfig {
    #[serde(default)]
    mcp: HashMap<String, OpencodeMcpServer>,
}

/// 组装一条导入用的 `ChatMcpServer`：新 uuid、默认停用、无连接器/认证。
fn imported_mcp_server(
    name: String,
    transport: String,
    url: String,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    headers: HashMap<String, String>,
    cwd: Option<String>,
) -> ChatMcpServer {
    ChatMcpServer {
        id: format!("mcp-{}", uuid::Uuid::new_v4()),
        name,
        enabled: false,
        transport,
        url,
        command,
        args,
        env,
        headers,
        cwd,
        enabled_tools: Vec::new(),
        connector_id: None,
        auth: None,
    }
}

/// 读 `~/.claude.json` 顶层 `mcpServers`（schema 同 Cursor mcp.json）。跳过
/// `projects[*].mcpServers`。文件缺失 → `available:false`；解析失败 → 空组。
fn parse_claude_mcp(home: &Path) -> CliMcpGroup {
    let path = home.join(".claude.json");
    if !path.exists() {
        return CliMcpGroup::default();
    }
    let servers = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<CursorMcpJson>(&raw).ok())
        .map(|parsed| {
            parsed
                .mcp_servers
                .into_iter()
                .map(|(name, server)| {
                    imported_mcp_server(
                        name,
                        normalize_imported_transport(&server),
                        server.url,
                        server.command,
                        server.args,
                        server.env,
                        server.headers,
                        server.cwd,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    CliMcpGroup {
        available: true,
        servers,
    }
}

/// 读 `~/.codex/config.toml` 的 `[mcp_servers.*]`（全 stdio）。
fn parse_codex_mcp(home: &Path) -> CliMcpGroup {
    let path = home.join(".codex").join("config.toml");
    if !path.exists() {
        return CliMcpGroup::default();
    }
    let servers = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| toml::from_str::<CodexConfig>(&raw).ok())
        .map(|parsed| {
            parsed
                .mcp_servers
                .into_iter()
                .map(|(name, server)| {
                    imported_mcp_server(
                        name,
                        "stdio".to_string(),
                        String::new(),
                        server.command,
                        server.args,
                        server.env,
                        HashMap::new(),
                        server.cwd,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    CliMcpGroup {
        available: true,
        servers,
    }
}

/// 读 `~/.config/opencode/opencode.json` 的 `mcp.*`（local→stdio / remote→http）。
fn parse_opencode_mcp(home: &Path) -> CliMcpGroup {
    let path = home.join(".config").join("opencode").join("opencode.json");
    if !path.exists() {
        return CliMcpGroup::default();
    }
    let servers = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<OpencodeConfig>(&raw).ok())
        .map(|parsed| {
            parsed
                .mcp
                .into_iter()
                .map(|(name, server)| {
                    let is_remote = server
                        .server_type
                        .as_deref()
                        .map(|kind| kind.eq_ignore_ascii_case("remote"))
                        .unwrap_or(false);
                    if is_remote {
                        imported_mcp_server(
                            name,
                            "streamable_http".to_string(),
                            server.url,
                            String::new(),
                            Vec::new(),
                            HashMap::new(),
                            server.headers,
                            None,
                        )
                    } else {
                        let mut parts = server.command.into_iter();
                        let command = parts.next().unwrap_or_default();
                        let args = parts.collect::<Vec<_>>();
                        imported_mcp_server(
                            name,
                            "stdio".to_string(),
                            String::new(),
                            command,
                            args,
                            server.environment,
                            HashMap::new(),
                            None,
                        )
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    CliMcpGroup {
        available: true,
        servers,
    }
}

/// 读 Pi 用户级 MCP 配置。自有层认 `PI_CODING_AGENT_DIR`（否则 `~/.pi/agent/mcp.json`），
/// 再合并 `~/.config/mcp/mcp.json` 与 `~/.agents` 共享层；按优先级去重，单层解析失败跳过。
fn parse_pi_mcp(home: &Path) -> CliMcpGroup {
    let agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".pi").join("agent"));
    parse_pi_mcp_layers([
        agent_dir.join("mcp.json"),
        home.join(".config").join("mcp").join("mcp.json"),
        home.join(".agents").join("mcp.json"),
        home.join(".agents").join("mcp").join("mcp.json"),
    ])
}

fn parse_pi_mcp_layers(candidates: impl IntoIterator<Item = std::path::PathBuf>) -> CliMcpGroup {
    let mut available = false;
    let mut seen = std::collections::HashSet::new();
    let mut servers = Vec::new();
    for path in candidates {
        if !path.exists() {
            continue;
        }
        available = true;
        let Some(parsed) = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<CursorMcpJson>(&raw).ok())
        else {
            continue;
        };
        // HashMap 迭代序随机，排序保证扫描结果稳定。
        let mut entries: Vec<_> = parsed.mcp_servers.into_iter().collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, server) in entries {
            if !seen.insert(name.clone()) {
                continue;
            }
            servers.push(imported_mcp_server(
                name,
                normalize_imported_transport(&server),
                server.url,
                server.command,
                server.args,
                server.env,
                server.headers,
                server.cwd,
            ));
        }
    }
    CliMcpGroup { available, servers }
}

/// 扫描本机已安装的 Claude Code / Codex / OpenCode / Pi 配置，解析出可导入的
/// MCP 服务器（按 CLI 分组）。缺配置文件 = 该组 `available:false`；单个 CLI
/// 解析失败降级为空组，绝不 panic、绝不返回 Err。
#[tauri::command]
pub fn chat_cli_import_scan() -> CliImportScan {
    let Some(base) = directories::BaseDirs::new() else {
        return CliImportScan::default();
    };
    let home = base.home_dir();
    CliImportScan {
        claude: parse_claude_mcp(home),
        codex: parse_codex_mcp(home),
        opencode: parse_opencode_mcp(home),
        pi: parse_pi_mcp(home),
    }
}

/// 单个连接器工具的元信息（名称 + 描述），给连接器详情面板的工具列表用。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorToolInfo {
    pub name: String,
    pub description: String,
}

/// 列出某个 MCP server 的工具（含描述）。连接器详情面板用来渲染「工具」列表
/// 与逐工具允许/停用开关。不按 enabled_tools 过滤——前端需要拿到全部工具名
/// 才能正确展开/收拢白名单。
#[tauri::command]
pub async fn chat_mcp_list_tool_defs(
    app: AppHandle,
    state: State<'_, AppState>,
    server_id: String,
) -> Result<Vec<ConnectorToolInfo>, String> {
    let settings = state.settings_read().clone();
    let server = settings
        .chat_tools
        .servers
        .iter()
        .find(|server| server.id == server_id)
        .cloned()
        .ok_or_else(|| "MCP server is missing".to_string())?;
    let tools = state.mcp_list_tools(Some(&app), &server).await?;
    Ok(tools
        .into_iter()
        .map(|tool| ConnectorToolInfo {
            name: tool.name,
            description: tool.description,
        })
        .collect())
}

/// 读取某个 MCP server 的持久连接状态快照（状态点 / handshake 次数 / stderr 尾巴）。
#[tauri::command]
pub async fn chat_mcp_server_status(
    state: State<'_, AppState>,
    server_id: String,
) -> Result<crate::mcp::manager::McpServerStatusSnapshot, String> {
    Ok(state.mcp_server_state(&server_id).await)
}

/// 主动断开某个 MCP server 的持久会话（重连按钮）：下次调用透明重连。
#[tauri::command]
pub async fn chat_mcp_reload_server(
    app: AppHandle,
    state: State<'_, AppState>,
    server_id: String,
) -> Result<(), String> {
    state.mcp_reload_server(Some(&app), &server_id).await;
    Ok(())
}

pub async fn call_tool(
    app: &AppHandle,
    state: &AppState,
    tool: &ChatToolDefinition,
    arguments: Value,
    skill_cache: Option<&mut crate::skills::SkillRunCache>,
    native_ctx: Option<NativeToolContext>,
) -> Result<McpToolCallResult, String> {
    if tool.source == "native" {
        return call_native_tool(app, state, tool, arguments, native_ctx).await;
    }

    if tool.source == "skill" {
        return call_skill_tool(app, state, tool, arguments, skill_cache).await;
    }

    if tool.source == "mixer" {
        return call_mixer_tool(app, state, tool, arguments, native_ctx).await;
    }

    let server_id = tool
        .server_id
        .as_deref()
        .ok_or_else(|| "MCP tool has no server id".to_string())?;
    let settings = state.settings_read().clone();
    let server = settings
        .chat_tools
        .servers
        .iter()
        .find(|server| server.id == server_id && mcp_server_is_runtime_eligible(server))
        .cloned()
        .ok_or_else(|| "MCP server is disabled or missing".to_string())?;
    // 走持久连接池：复用长连接、liveness 探活 + 透明重连、按 server_id 隔离。
    let mut result = state
        .mcp_call_tool(Some(app), &server, &tool.name, arguments.clone())
        .await?;
    // R1：MCP 工具结果里的图片 artifact 直达模型——vision 主模型直喂原图，
    // 纯文本主模型走辅助视觉模型审查向分析（R2）。通用于所有 MCP server，非
    // officecli 专属；无 native_ctx（如独立调用场景）时无法解析会话 provider/
    // model，直接跳过，保留 client.rs 生成的 `[image: <mime>]` 占位符。
    if let Some(ctx) = native_ctx.as_ref() {
        crate::chat::commands::attach_image_artifacts_for_model(
            app,
            &settings,
            &ctx.conversation_id,
            &ctx.message_id,
            &mut result,
        )
        .await;
    }
    // OfficeCLI 插件：文档写/改成功后默认拉起 live preview（浏览器实时看）
    if let Some(note) =
        crate::plugins::note_after_officecli_tool(app, state, tool, &arguments, &result)
    {
        if result.content.trim().is_empty() {
            result.content = note;
        } else {
            result.content = format!("{}\n\n{note}", result.content.trim_end());
        }
    }
    Ok(result)
}

/// 一次性列出某个 server 的工具。给设置页的「测试连接」按钮用 —— 测的是**还没保存**
/// 的草稿配置，所以刻意不进连接池。
///
/// 整体套一层超时：rmcp 的握手本身不带超时，死服务器会把设置页挂死。这里取消 future
/// 是安全的（连接建完就拆，握手和 tools/list 都是幂等读），和 `tools/call` 不同。
async fn list_server_tools(
    http: &reqwest::Client,
    server: &ChatMcpServer,
    timeout_ms: u64,
) -> Result<Vec<ChatToolDefinition>, String> {
    let tools = tokio::time::timeout(
        Duration::from_millis(timeout_ms.max(1_000)),
        super::conn::list_tools_once(server, http),
    )
    .await
    .map_err(|_| format!("MCP connection to {} timed out", server.name))??;
    Ok(tools_from_mcp(server, tools))
}

fn normalize_imported_transport(server: &CursorMcpServer) -> String {
    let raw = server
        .transport
        .as_deref()
        .or(server.server_type.as_deref())
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if raw == "http" || raw == "sse" || raw == "streamable_http" || !server.url.trim().is_empty() {
        "streamable_http".to_string()
    } else {
        "stdio".to_string()
    }
}

pub(crate) fn mcp_server_is_runtime_eligible(server: &ChatMcpServer) -> bool {
    if !server.enabled {
        return false;
    }
    if let Some(plugin_id) = server
        .connector_id
        .as_deref()
        .and_then(|connector_id| connector_id.strip_prefix("plugin:"))
    {
        return crate::plugins::is_enabled(plugin_id) && crate::plugins::is_installed(plugin_id);
    }
    true
}

fn eligible_mcp_servers(settings: &crate::settings::Settings) -> Vec<&ChatMcpServer> {
    if !settings.chat_tools.enabled {
        return Vec::new();
    }
    settings
        .chat_tools
        .servers
        .iter()
        .filter(|server| mcp_server_is_runtime_eligible(server))
        .collect()
}

/// 从未成功连接过（Error 态且无缓存工具）的已启用 server 说明，注入系统提示词：
/// 让模型知道这些 server 已配置但当前不可用，而不是"不存在"。无此类 server ⇒ None。
pub fn unavailable_mcp_servers_note(names: &[String]) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    Some(format!(
        "MCP servers configured but currently unreachable (connection failed, tools unavailable): {}. \
         If the user asks about these tools, tell them the server is configured but failing to \
         connect (they can check it in Settings) ? do not claim the tool does not exist.",
        names.join(", ")
    ))
}

fn tinyfish_mcp_configured(settings: &crate::settings::Settings) -> bool {
    let url = settings.lens.web_search.tinyfish_mcp_url.trim();
    if url.is_empty() {
        return false;
    }
    if settings
        .lens
        .web_search
        .tinyfish_mcp_auth
        .as_ref()
        .is_some_and(|auth| !auth.access_token.trim().is_empty())
    {
        return true;
    }
    let normalized = url.trim_end_matches('/');
    settings.chat_tools.servers.iter().any(|server| {
        if server.url.trim().trim_end_matches('/') != normalized {
            return false;
        }
        server
            .headers
            .get("Authorization")
            .is_some_and(|value| !value.trim().is_empty())
            || server
                .auth
                .as_ref()
                .is_some_and(|auth| !auth.access_token.trim().is_empty())
    })
}

pub(crate) fn web_search_configured(settings: &crate::settings::Settings) -> bool {
    match settings.lens.web_search.provider {
        WebSearchProvider::Tavily => !settings.lens.web_search.tavily_api_key.trim().is_empty(),
        WebSearchProvider::Exa => !settings.lens.web_search.exa_api_key.trim().is_empty(),
        WebSearchProvider::ExaMcp => !settings.lens.web_search.exa_mcp_url.trim().is_empty(),
        WebSearchProvider::Ollama => !settings.lens.web_search.ollama_api_key.trim().is_empty(),
        WebSearchProvider::Grok => !settings.lens.web_search.grok_api_key.trim().is_empty(),
        WebSearchProvider::Brave => !settings.lens.web_search.brave_api_key.trim().is_empty(),
        WebSearchProvider::Serper => !settings.lens.web_search.serper_api_key.trim().is_empty(),
        WebSearchProvider::Bocha => !settings.lens.web_search.bocha_api_key.trim().is_empty(),
        WebSearchProvider::Zhipu => !settings.lens.web_search.zhipu_api_key.trim().is_empty(),
        WebSearchProvider::Tinyfish => !settings.lens.web_search.tinyfish_api_key.trim().is_empty(),
        WebSearchProvider::TinyfishMcp => tinyfish_mcp_configured(settings),
        WebSearchProvider::Searxng => !settings.lens.web_search.searxng_base_url.trim().is_empty(),
        WebSearchProvider::Unknown => false,
    }
}

async fn call_mixer_tool(
    app: &AppHandle,
    state: &AppState,
    tool: &ChatToolDefinition,
    arguments: Value,
    native_ctx: Option<NativeToolContext>,
) -> Result<McpToolCallResult, String> {
    match tool.name.as_str() {
        "mixer_generate_image" => {
            let conversation_id = native_ctx.as_ref().map(|ctx| ctx.conversation_id.as_str());
            crate::chat::image_generation::tool_generate_image(
                app,
                state,
                conversation_id,
                &arguments,
            )
            .await
        }
        other => Err(format!("Unknown mixer tool: {other}")),
    }
}

async fn call_skill_tool(
    app: &AppHandle,
    state: &AppState,
    tool: &ChatToolDefinition,
    arguments: Value,
    skill_cache: Option<&mut crate::skills::SkillRunCache>,
) -> Result<McpToolCallResult, String> {
    let settings = state.settings_read().clone();
    let skill_name = crate::skills::extract_skill_name(&arguments)?;

    // Resolve the SkillRecord, preferring the run-scoped cached registry (T1).
    // Clone it out so we drop the immutable borrow on the cache before we need a
    // mutable borrow for activate/read dispatch and run-scoped activation state.
    let mut skill_cache = skill_cache;
    let record = if let Some(cache) = skill_cache.as_deref_mut() {
        let registry = cache.registry_for(app, &settings.chat_tools.skill_scan_paths)?;
        crate::skills::lookup_skill(registry, &skill_name)
            .cloned()
            .ok_or_else(|| format!("Skill not found: {skill_name}"))?
    } else {
        let registry = crate::skills::build_registry(app, &settings.chat_tools.skill_scan_paths)?;
        crate::skills::lookup_skill(&registry, &skill_name)
            .cloned()
            .ok_or_else(|| format!("Skill not found: {skill_name}"))?
    };
    if let Some(err) = crate::settings::skill_global_unavailable_error(
        &settings.chat_tools,
        &record.meta.id,
        crate::settings::obsidian_connector_configured(&settings.obsidian_vault_path),
        &skill_name,
    ) {
        return Err(err);
    }
    // 助手技能白名单硬 gate(防绕过):模型报个不在目录里的技能名也会被这里拦下。
    if let Some(cache) = skill_cache.as_deref() {
        if !cache.skill_id_allowed(&record.meta.id) {
            return Err(format!(
                "Skill is not allowed for the active assistant: {skill_name}"
            ));
        }
    }

    let content = match tool.name.as_str() {
        "skill" => {
            if let Some(cache) = skill_cache.as_deref_mut() {
                cache.activate_with_cache(&record)
            } else {
                crate::skills::activate_skill(&record)
            }
        }
        other => return Err(format!("Unknown skill tool: {other}")),
    };

    Ok(McpToolCallResult {
        content,
        is_error: false,
        raw: Value::Null,
        artifacts: Vec::new(),
        structured_content: None,
        follow_up_user_messages: Vec::new(),
    })
}

pub fn skill_runtime_tools_enabled(settings: &crate::settings::Settings) -> bool {
    settings.chat_tools.native_tools.skill_runtime
}

pub fn list_skill_tool_defs(settings: &crate::settings::Settings) -> Vec<ChatToolDefinition> {
    if skill_runtime_tools_enabled(settings) {
        native_skill_tools()
    } else {
        Vec::new()
    }
}

async fn call_native_tool(
    app: &AppHandle,
    state: &AppState,
    tool: &ChatToolDefinition,
    arguments: Value,
    native_ctx: Option<NativeToolContext>,
) -> Result<McpToolCallResult, String> {
    use super::native_registry::{find_entry, text_tool_result, NativeCallCtx, NativeToolCall};

    let Some(entry) = find_entry(&tool.name) else {
        return Err(format!("Unknown native tool: {}", tool.name));
    };

    // Conversation-scoped tools (todo) run before workspace resolution: they
    // only need the conversation id and must not fail when the conversation
    // project cannot be resolved.
    if let NativeToolCall::Conversation(handler) = &entry.call {
        let ctx = native_ctx
            .as_ref()
            .ok_or_else(|| format!("{} requires a conversation context", entry.name))?;
        return handler(app, &ctx.conversation_id, &tool.name, arguments).await;
    }
    if let NativeToolCall::SubAgent(handler) = &entry.call {
        // Sub-agent management tools manage agents, not files: dispatch before
        // workspace resolution with the parent run context.
        let ctx = native_ctx
            .as_ref()
            .ok_or_else(|| format!("{} requires an agent context", entry.name))?;
        return handler(crate::chat::sub_agent::SubAgentCallCtx {
            app,
            state,
            native_ctx: ctx,
            arguments: &arguments,
        })
        .await;
    }
    if matches!(entry.call, NativeToolCall::HostMediated) {
        // ask_user is host-mediated in chat/agent/execute.rs and must never
        // reach the registry dispatcher; keep the legacy fallback wording.
        return Err(format!("Unknown native tool: {}", tool.name));
    }

    let settings = state.settings_read().clone();
    let workspace = resolve_native_workspace(
        app,
        &settings.chat_tools.native_tools.working_directory,
        native_ctx.as_ref(),
    )
    .await?;

    match &entry.call {
        NativeToolCall::SyncText(call) => Ok(text_tool_result(call(&workspace, &arguments)?)),
        NativeToolCall::SyncResult(call) => call(&workspace, &arguments),
        NativeToolCall::BlockingText(call) => {
            let content = run_blocking_file_mutation(&workspace, &arguments, *call).await?;
            Ok(text_tool_result(content))
        }
        NativeToolCall::BlockingMutation(call) => {
            let result = run_blocking_file_mutation(&workspace, &arguments, *call).await?;
            let mut tool_result = file_mutation_tool_result(result)?;
            // If an ordinary-chat `write` lands inside that conversation's
            // workbench, attach a downloadable file-card artifact. Explicit writes
            // elsewhere and project edits do not become delivery cards.
            if tool.name == "write" {
                if let Some(artifact) =
                    delivery_artifact_for_write(&workspace, &arguments, native_ctx.as_ref())
                {
                    tool_result.artifacts.push(artifact);
                }
            }
            Ok(tool_result)
        }
        NativeToolCall::Async(call) => {
            call(NativeCallCtx {
                app,
                state,
                settings: &settings,
                workspace: &workspace,
                arguments: &arguments,
                native_ctx: native_ctx.as_ref(),
            })
            .await
        }
        NativeToolCall::Conversation(_)
        | NativeToolCall::HostMediated
        | NativeToolCall::SubAgent(_) => unreachable!(),
    }
}

/// Runs a file mutation tool on the blocking thread pool so in-process path
/// lock waits (`Condvar::wait`) and large synchronous IO do not stall tokio
/// runtime workers.
async fn run_blocking_file_mutation<T, F>(
    workspace: &NativeToolWorkspace,
    arguments: &Value,
    mutate: F,
) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&NativeToolWorkspace, &Value) -> Result<T, String> + Send + 'static,
{
    let workspace = workspace.clone();
    let arguments = arguments.clone();
    tokio::task::spawn_blocking(move || mutate(&workspace, &arguments))
        .await
        .map_err(|err| format!("File mutation task failed: {err}"))?
}

/// 文件变更结果给模型的 diff 最多带多少行（对齐 clawspring 的 80 行裁剪）。
const FILE_MUTATION_DIFF_MAX_LINES: usize = 80;

pub fn file_mutation_tool_result(result: FileMutationResult) -> Result<McpToolCallResult, String> {
    let summary = result.summary();
    let mut content = summary;
    if !result.warnings.is_empty() {
        content = format!("{}\n{}", content, result.warnings.join("\n"));
    }
    // 把裁剪后的 unified diff 直接回显给模型：模型在结果里"看到"自己实际改了什么，
    // 能立即发现写歪。完整 diff 始终在 structured_content 里给前端渲染。
    if !result.diff.trim().is_empty() {
        let lines: Vec<&str> = result.diff.lines().collect();
        if lines.len() > FILE_MUTATION_DIFF_MAX_LINES {
            let clipped = lines[..FILE_MUTATION_DIFF_MAX_LINES].join("\n");
            content = format!(
                "{}\n\n{}\n[... diff clipped: showing first {FILE_MUTATION_DIFF_MAX_LINES} of {} lines ...]",
                content,
                clipped,
                lines.len()
            );
        } else {
            content = format!("{}\n\n{}", content, result.diff);
        }
    }
    let is_error = !result.ok;
    let structured = serde_json::to_value(&result)
        .map_err(|err| format!("Serialize file mutation result failed: {err}"))?;
    Ok(McpToolCallResult {
        content,
        is_error,
        raw: structured.clone(),
        artifacts: Vec::new(),
        structured_content: Some(structured),
        follow_up_user_messages: Vec::new(),
    })
}

/// Build a downloadable file-card artifact when an ordinary-chat `write` resolves
/// inside that conversation's workbench. Returns `None` for project/external/temp
/// writes and for standalone calls
/// without a conversation context. Re-resolves the write path the same way
/// `write_file` did (pure path resolution, no IO) so the absolute on-disk path
/// is reliable across project vs. global workspaces.
fn delivery_artifact_for_write(
    workspace: &NativeToolWorkspace,
    arguments: &Value,
    native_ctx: Option<&NativeToolContext>,
) -> Option<crate::mcp::types::ChatToolArtifact> {
    native_ctx?;
    let raw_path = arguments.get("path").and_then(|v| v.as_str())?;
    let resolved = resolve_tool_write_path(workspace, raw_path).ok()?;
    let directory = workspace.conversation_directory()?;
    if !crate::native_tools::path_under_directory(directory, &resolved) {
        return None;
    }
    crate::native_tools::build_delivery_artifact_for_path(&resolved).ok()
}

pub fn read_file_tool_result(result: ReadFileResult) -> Result<McpToolCallResult, String> {
    // structured_content 保留完整 ReadFileResult 给前端 ToolCallBlock 渲染（不变）。
    let structured = serde_json::to_value(&result)
        .map_err(|err| format!("Serialize read_file result failed: {err}"))?;
    // 模型看到的 content 改成 cat -n 风格（`行号\t内容` + 精简头），行号便于模型引用并构造
    // 后续 edit_file。行号仅供参考、不属于文件内容；read_file/edit_file 描述里已说明
    // old_string 不要带行号前缀。
    let content = format_read_file_for_model(&result);
    Ok(McpToolCallResult {
        content,
        is_error: false,
        raw: structured.clone(),
        artifacts: Vec::new(),
        structured_content: Some(structured),
        follow_up_user_messages: Vec::new(),
    })
}

/// 把 ReadFileResult 渲染成模型友好的 `cat -n` 文本：一行精简元数据头 + `右对齐行号\t原文`。
fn format_read_file_for_model(result: &ReadFileResult) -> String {
    let mut out = format!(
        "{} — lines {}-{} of {}",
        result.path, result.start_line, result.end_line, result.total_lines
    );
    if result.truncated {
        match result.next_offset {
            Some(next) => out.push_str(&format!(" (truncated; continue with offset={next})")),
            None => out.push_str(" (truncated)"),
        }
    }
    for warning in &result.warnings {
        out.push_str("\n! ");
        out.push_str(warning);
    }
    if !result.content.is_empty() {
        let start = result.start_line.max(1);
        out.push('\n');
        let numbered: Vec<String> = result
            .content
            .lines()
            .enumerate()
            .map(|(i, line)| format!("{:>6}\t{}", start + i, line))
            .collect();
        out.push_str(&numbered.join("\n"));
    }
    out
}

async fn resolve_native_workspace(
    app: &AppHandle,
    working_directory: &str,
    native_ctx: Option<&NativeToolContext>,
) -> Result<NativeToolWorkspace, String> {
    let Some(native_ctx) = native_ctx else {
        return Ok(NativeToolWorkspace::standalone());
    };
    let conversation = crate::chat::storage::load_conversation(app, &native_ctx.conversation_id)
        .map_err(|err| {
            format!(
                "Resolve native tool workspace failed for conversation {}: {err}",
                native_ctx.conversation_id
            )
        })?;
    let Some(project) = crate::chat::storage::resolve_conversation_project(app, &conversation)?
    else {
        let directory = crate::chat::repository::repository(app)
            .prepare_ordinary_workspace(app, &conversation.id, working_directory)
            .await
            .map_err(crate::chat::repository::repository_error)?;
        return Ok(NativeToolWorkspace::conversation(directory));
    };
    Ok(NativeToolWorkspace::project(
        project.id,
        project.name,
        project.root_path,
    ))
}

pub(super) async fn run_python_via_pyodide(
    app: &AppHandle,
    state: &AppState,
    settings: &crate::settings::Settings,
    workspace: &NativeToolWorkspace,
    arguments: &Value,
    native_ctx: Option<NativeToolContext>,
) -> Result<McpToolCallResult, String> {
    let code = arguments
        .get("code")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "run_python requires code".to_string())?;

    let timeout_ms = arguments
        .get("timeout_ms")
        .and_then(|value| value.as_u64())
        .unwrap_or(settings.chat_tools.tool_timeout_ms)
        .clamp(1_000, 300_000);
    let input_files = collect_python_input_files(app, workspace, arguments)?;
    let run_id = uuid::Uuid::new_v4().to_string();
    let parent = native_ctx.clone();
    let output_directory = workspace.default_output_directory()?;
    let export_ctx = native_ctx
        .map(|ctx| crate::native_tools::SandboxExportContext {
            conversation_id: ctx.conversation_id,
            message_id: ctx.message_id,
            tool_call_id: ctx.tool_call_id,
            output_directory: output_directory.clone(),
        })
        .unwrap_or_else(|| crate::native_tools::SandboxExportContext {
            conversation_id: "standalone".to_string(),
            message_id: run_id.clone(),
            tool_call_id: None,
            output_directory,
        });
    let (tx, rx) = oneshot::channel();
    let payload = crate::chat::protocol::ChatRunPythonPayload {
        protocol_version: crate::chat::protocol::CHAT_PROTOCOL_VERSION,
        run_id: run_id.clone(),
        parent_conversation_id: parent.as_ref().map(|ctx| ctx.conversation_id.clone()),
        parent_run_id: parent.as_ref().map(|ctx| ctx.run_id.clone()),
        parent_message_id: parent.as_ref().map(|ctx| ctx.message_id.clone()),
        code: code.to_string(),
        timeout_ms,
        files: input_files
            .into_iter()
            .map(|file| crate::chat::protocol::ChatPythonInputFile {
                name: file.name,
                data_base64: file.data_base64,
                size_bytes: file.size_bytes,
            })
            .collect(),
    };
    {
        let mut pending = state
            .pending_python_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        pending.insert(
            run_id.clone(),
            crate::state::PendingPythonRun {
                sender: tx,
                export_ctx: export_ctx.clone(),
            },
        );
    }
    if let Err(error) = crate::chat::protocol::attach_python_request(app, payload.clone()) {
        eprintln!("Failed to attach Python request to chat snapshot: {error}");
    }
    let emit_result = app.emit("chat-run-python", payload);
    if let Err(err) = emit_result {
        let mut pending = state
            .pending_python_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        pending.remove(&run_id);
        crate::chat::protocol::detach_python_request(app, &run_id);
        return Err(format!("Failed to start Python runner: {err}"));
    }

    // The Worker gives cold Pyodide initialization a 10s grace and resets itself
    // at timeout_ms + 10s. Keep the Rust receiver slightly later so the frontend
    // always gets the first chance to terminate the memory-heavy Worker and send
    // a structured completion instead of continuing after this command returns.
    const PYODIDE_FRONTEND_GRACE_MS: u64 = 10_000;
    const PYODIDE_COMPLETION_TRANSPORT_GRACE_MS: u64 = 2_000;
    let wait_ms = timeout_ms
        .saturating_add(PYODIDE_FRONTEND_GRACE_MS)
        .saturating_add(PYODIDE_COMPLETION_TRANSPORT_GRACE_MS);
    let wait = tokio::time::timeout(Duration::from_millis(wait_ms), rx).await;

    match wait {
        Ok(Ok(result)) => {
            if result.is_error {
                Err(result.content)
            } else {
                let mut content = result.content;
                let mut artifacts = result.artifacts;
                match crate::native_tools::export_sandbox_artifacts(&export_ctx, &artifacts) {
                    Ok(exported_artifacts) => {
                        for exported in &exported_artifacts {
                            if let Some(artifact) = artifacts.get_mut(exported.artifact_index) {
                                artifact.path = Some(exported.path.display().to_string());
                                if let Some(name) =
                                    exported.path.file_name().and_then(|value| value.to_str())
                                {
                                    artifact.name = name.to_string();
                                }
                            }
                        }
                        let export_note =
                            crate::native_tools::format_exported_paths(&exported_artifacts);
                        if !export_note.is_empty() {
                            if !content.trim().is_empty() {
                                content.push_str("\n\n");
                            }
                            content.push_str(&export_note);
                        }
                    }
                    Err(err) => {
                        if !content.trim().is_empty() {
                            content.push_str("\n\n");
                        }
                        content.push_str(&crate::native_tools::format_export_error(&err));
                    }
                }
                Ok(McpToolCallResult {
                    content,
                    is_error: false,
                    raw: Value::Null,
                    artifacts,
                    structured_content: None,
                    follow_up_user_messages: Vec::new(),
                })
            }
        }
        Ok(Err(_)) => Err("Python runner channel closed".to_string()),
        Err(_) => {
            let mut pending = state
                .pending_python_runs
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            pending.remove(&run_id);
            crate::chat::protocol::detach_python_request(app, &run_id);
            Err(format!("Python execution timed out after {timeout_ms}ms"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_tools::ReadFileResult;
    use std::path::Path;

    #[test]
    fn python_input_name_keeps_unicode_letters() {
        assert_eq!(
            sanitize_python_input_name(Path::new("销售报表.xlsx")),
            "销售报表.xlsx"
        );
        assert_eq!(
            sanitize_python_input_name(Path::new("/tmp/Q1 销售.csv")),
            "Q1 销售.csv"
        );
        assert_eq!(
            sanitize_python_input_name(Path::new("../secret?.png")),
            "secret_.png"
        );
        assert_eq!(
            sanitize_python_input_name(Path::new("chart.png")),
            "chart.png"
        );
    }

    #[test]
    fn read_file_tool_result_preserves_structured_content() {
        let result = ReadFileResult {
            path: "src/App.tsx".to_string(),
            resolved_path: "/tmp/project/src/App.tsx".to_string(),
            content: "alpha\nbeta".to_string(),
            total_lines: 2,
            start_line: 1,
            end_line: 2,
            truncated: false,
            file_size: 10,
            next_offset: None,
            warnings: Vec::new(),
        };

        let output = read_file_tool_result(result).expect("tool result");
        let structured = output
            .structured_content
            .as_ref()
            .expect("structured content");

        assert!(!output.is_error);
        assert_eq!(output.raw, *structured);
        assert_eq!(structured["path"], "src/App.tsx");
        assert_eq!(structured["resolved_path"], "/tmp/project/src/App.tsx");
        assert_eq!(structured["content"], "alpha\nbeta");
        assert_eq!(structured["total_lines"], 2);
        assert_eq!(structured["start_line"], 1);
        assert_eq!(structured["end_line"], 2);
        assert_eq!(structured["truncated"], false);
        assert_eq!(structured["file_size"], 10);
        // 模型看到的 content 是 cat -n 文本（不再是 JSON），结构化内容仍完整保留给前端。
        assert_eq!(
            output.content,
            "src/App.tsx — lines 1-2 of 2\n     1\talpha\n     2\tbeta"
        );
    }

    #[test]
    fn runtime_eligibility_rejects_disabled_server_and_accepts_plain_enabled_server() {
        let mut server = ChatMcpServer::default();
        assert!(!mcp_server_is_runtime_eligible(&server));

        server.enabled = true;
        assert!(mcp_server_is_runtime_eligible(&server));
    }

    fn enabled_server(id: &str) -> ChatMcpServer {
        ChatMcpServer {
            id: id.to_string(),
            name: format!("{id} server"),
            enabled: true,
            transport: "stdio".to_string(),
            command: "true".to_string(),
            ..ChatMcpServer::default()
        }
    }

    fn settings_with_servers(servers: Vec<ChatMcpServer>) -> crate::settings::Settings {
        let mut settings = crate::settings::Settings::default();
        settings.chat_tools.enabled = true;
        settings.chat_tools.servers = servers;
        settings
    }

    #[test]
    fn warmup_selection_covers_all_eligible_or_the_requested_subset() {
        let mut disabled = enabled_server("off");
        disabled.enabled = false;
        let settings =
            settings_with_servers(vec![enabled_server("a"), enabled_server("b"), disabled]);

        // None = 全部 eligible（停用的不预热）
        let all = select_warmup_servers(&settings, None);
        assert_eq!(
            all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );

        // Some = 指定 id 子集；未知 id / 停用 server 被忽略
        let subset = select_warmup_servers(
            &settings,
            Some(&["b".to_string(), "off".to_string(), "ghost".to_string()]),
        );
        assert_eq!(
            subset.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["b"]
        );
    }

    #[test]
    fn warmup_selection_is_empty_when_chat_tools_disabled() {
        let mut settings = settings_with_servers(vec![enabled_server("a")]);
        settings.chat_tools.enabled = false;
        assert!(select_warmup_servers(&settings, None).is_empty());
    }

    /// 一快一慢（慢 = 永不应答握手）的工具收集：慢 server 不把整轮拖到全局 60s 超时，
    /// 快 server 工具照常返回。unix-gated：依赖 python3 + sleep 起 stdio 假 server。
    #[cfg(unix)]
    mod warm_timeout {
        use super::*;
        use crate::state::test_app_state;
        use std::io::Write;

        fn write_fast_fake_server() -> std::path::PathBuf {
            let script = r#"#!/usr/bin/env python3
import sys, json
while True:
    line = sys.stdin.readline()
    if not line:
        break
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    mid = msg.get("id")
    method = msg.get("method")
    if mid is None:
        continue
    if method == "initialize":
        resp = {"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"1.0.0"}}}
    elif method == "tools/list":
        resp = {"jsonrpc":"2.0","id":mid,"result":{"tools":[{"name":"echo","description":"Echo","inputSchema":{"type":"object"}}]}}
    else:
        resp = {"jsonrpc":"2.0","id":mid,"result":{}}
    sys.stdout.write(json.dumps(resp)+"\n")
    sys.stdout.flush()
"#;
            let mut path = std::env::temp_dir();
            path.push(format!("kivio-fake-mcp-warm-{}.py", uuid::Uuid::new_v4()));
            let mut file = std::fs::File::create(&path).expect("create fake server");
            file.write_all(script.as_bytes())
                .expect("write fake server");
            path
        }

        fn fast_server(script: &std::path::Path) -> ChatMcpServer {
            ChatMcpServer {
                id: "fast".to_string(),
                name: "fast server".to_string(),
                enabled: true,
                transport: "stdio".to_string(),
                command: "python3".to_string(),
                args: vec!["-u".to_string(), script.to_str().unwrap().to_string()],
                ..ChatMcpServer::default()
            }
        }

        /// 进程能起但永不应答 initialize：没有本地超时时会等满全局 tool timeout（默认 60s）。
        fn hanging_server() -> ChatMcpServer {
            ChatMcpServer {
                id: "hang".to_string(),
                name: "hang server".to_string(),
                enabled: true,
                transport: "stdio".to_string(),
                command: "sleep".to_string(),
                args: vec!["120".to_string()],
                ..ChatMcpServer::default()
            }
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn slow_server_is_bounded_and_fast_server_tools_survive() {
            let script = write_fast_fake_server();
            let state = test_app_state();
            let settings = settings_with_servers(vec![fast_server(&script), hanging_server()]);

            let started = std::time::Instant::now();
            let (tools, unavailable) = collect_enabled_mcp_tool_defs(&state, None, &settings).await;
            let elapsed = started.elapsed();

            assert!(
                elapsed < Duration::from_secs(WARM_TOOL_LIST_TIMEOUT.as_secs() + 7),
                "collection must be bounded by the per-server timeout, took {elapsed:?}"
            );
            assert!(
                tools.iter().any(|tool| tool.id == "mcp__fast__echo"),
                "fast server tools must survive the slow sibling: {tools:?}"
            );
            assert_eq!(
                unavailable,
                vec!["hang server".to_string()],
                "the hanging server has no snapshot, so it is unavailable this round"
            );

            state.mcp_disconnect_all().await;
            let _ = std::fs::remove_file(&script);
            let _ = std::fs::remove_dir_all(&state.usage_dir);
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn slow_server_with_snapshot_degrades_to_last_known_schema() {
            let state = test_app_state();
            let server = hanging_server();
            state.set_mcp_tool_snapshot(
                server.id.clone(),
                crate::mcp::manager::config_fingerprint(&server),
                vec![crate::mcp::types::McpTool {
                    name: "cached_tool".to_string(),
                    description: "from snapshot".to_string(),
                    input_schema: serde_json::json!({ "type": "object" }),
                    output_schema: None,
                    annotations: None,
                }],
            );
            let settings = settings_with_servers(vec![server]);

            let (tools, unavailable) = collect_enabled_mcp_tool_defs(&state, None, &settings).await;

            assert!(
                tools.iter().any(|tool| tool.id == "mcp__hang__cached_tool"),
                "timeout with a matching snapshot must degrade to last-known schema: {tools:?}"
            );
            assert!(unavailable.is_empty());

            state.mcp_disconnect_all().await;
            let _ = std::fs::remove_dir_all(&state.usage_dir);
        }
    }

    #[test]
    fn read_file_tool_result_numbers_from_offset_and_flags_truncation() {
        let result = ReadFileResult {
            path: "src/big.txt".to_string(),
            resolved_path: "/tmp/project/src/big.txt".to_string(),
            content: "line ten\nline eleven".to_string(),
            total_lines: 100,
            start_line: 10,
            end_line: 11,
            truncated: true,
            file_size: 4096,
            next_offset: Some(12),
            warnings: Vec::new(),
        };
        let output = read_file_tool_result(result).expect("tool result");
        assert_eq!(
            output.content,
            "src/big.txt — lines 10-11 of 100 (truncated; continue with offset=12)\n    10\tline ten\n    11\tline eleven"
        );
    }

    fn temp_home(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kivio-cli-import-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("create temp home");
        dir
    }

    #[test]
    fn parse_claude_mcp_reads_top_level_servers() {
        let home = temp_home("claude");
        fs::write(
            home.join(".claude.json"),
            r#"{
              "mcpServers": {
                "codegraph": {
                  "command": "codegraph",
                  "args": ["mcp"],
                  "env": { "CODEGRAPH_DB": "/tmp/cg.db" }
                }
              },
              "projects": {
                "/some/path": { "mcpServers": { "should-skip": { "command": "nope" } } }
              }
            }"#,
        )
        .unwrap();

        let group = parse_claude_mcp(&home);
        assert!(group.available);
        assert_eq!(group.servers.len(), 1);
        let server = &group.servers[0];
        assert_eq!(server.name, "codegraph");
        assert_eq!(server.transport, "stdio");
        assert_eq!(server.command, "codegraph");
        assert_eq!(server.args, vec!["mcp".to_string()]);
        assert_eq!(
            server.env.get("CODEGRAPH_DB").map(String::as_str),
            Some("/tmp/cg.db")
        );
        assert!(!server.enabled);
        assert!(server.connector_id.is_none());

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn parse_codex_mcp_reads_toml_env_subtable() {
        let home = temp_home("codex");
        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::write(
            home.join(".codex").join("config.toml"),
            r#"
[mcp_servers.node_repl]
command = "node"
args = ["--experimental-repl-await"]
startup_timeout_sec = 20

[mcp_servers.node_repl.env]
NODE_ENV = "development"
FOO = "bar"
"#,
        )
        .unwrap();

        let group = parse_codex_mcp(&home);
        assert!(group.available);
        assert_eq!(group.servers.len(), 1);
        let server = &group.servers[0];
        assert_eq!(server.name, "node_repl");
        assert_eq!(server.transport, "stdio");
        assert_eq!(server.command, "node");
        assert_eq!(server.args, vec!["--experimental-repl-await".to_string()]);
        assert_eq!(
            server.env.get("NODE_ENV").map(String::as_str),
            Some("development")
        );
        assert_eq!(server.env.get("FOO").map(String::as_str), Some("bar"));

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn parse_opencode_mcp_splits_local_command_array() {
        let home = temp_home("opencode");
        fs::create_dir_all(home.join(".config").join("opencode")).unwrap();
        fs::write(
            home.join(".config").join("opencode").join("opencode.json"),
            r#"{
              "mcp": {
                "local-fs": {
                  "type": "local",
                  "command": ["npx", "-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
                  "environment": { "DEBUG": "1" }
                },
                "remote-svc": {
                  "type": "remote",
                  "url": "https://mcp.example.com/mcp",
                  "headers": { "Authorization": "Bearer x" }
                }
              }
            }"#,
        )
        .unwrap();

        let group = parse_opencode_mcp(&home);
        assert!(group.available);
        assert_eq!(group.servers.len(), 2);

        let local = group.servers.iter().find(|s| s.name == "local-fs").unwrap();
        assert_eq!(local.transport, "stdio");
        assert_eq!(local.command, "npx");
        assert_eq!(
            local.args,
            vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-filesystem".to_string(),
                "/tmp".to_string()
            ]
        );
        assert_eq!(local.env.get("DEBUG").map(String::as_str), Some("1"));

        let remote = group
            .servers
            .iter()
            .find(|s| s.name == "remote-svc")
            .unwrap();
        assert_eq!(remote.transport, "streamable_http");
        assert_eq!(remote.url, "https://mcp.example.com/mcp");
        assert_eq!(
            remote.headers.get("Authorization").map(String::as_str),
            Some("Bearer x")
        );

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn parsers_return_unavailable_group_when_config_missing() {
        let home = temp_home("missing");
        assert!(!parse_claude_mcp(&home).available);
        assert!(!parse_codex_mcp(&home).available);
        assert!(!parse_opencode_mcp(&home).available);
        assert!(!parse_pi_mcp(&home).available);
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn parse_pi_mcp_merges_layers_and_dedupes_by_name() {
        let home = temp_home("pi");
        // Pi 自有层：同名 server 应压过共享层。
        fs::create_dir_all(home.join(".pi").join("agent")).unwrap();
        fs::write(
            home.join(".pi").join("agent").join("mcp.json"),
            r#"{
              "mcpServers": {
                "shared": { "command": "pi-owned", "args": ["stdio"] }
              }
            }"#,
        )
        .unwrap();
        // 共享层：一个同名（应被去重）+ 一个独有的 http server。
        fs::create_dir_all(home.join(".config").join("mcp")).unwrap();
        fs::write(
            home.join(".config").join("mcp").join("mcp.json"),
            r#"{
              "mcpServers": {
                "shared": { "command": "should-lose" },
                "remote": { "url": "https://mcp.example.com/mcp", "headers": { "X-Token": "t" } }
              }
            }"#,
        )
        .unwrap();

        let group = parse_pi_mcp(&home);
        assert!(group.available);
        assert_eq!(group.servers.len(), 2);
        let shared = group
            .servers
            .iter()
            .find(|server| server.name == "shared")
            .expect("shared server");
        assert_eq!(shared.command, "pi-owned");
        assert_eq!(shared.transport, "stdio");
        let remote = group
            .servers
            .iter()
            .find(|server| server.name == "remote")
            .expect("remote server");
        assert_eq!(remote.url, "https://mcp.example.com/mcp");
        assert_eq!(remote.transport, "streamable_http");

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn parse_pi_mcp_skips_corrupt_layer_but_reads_others() {
        let home = temp_home("pi-corrupt");
        fs::create_dir_all(home.join(".pi").join("agent")).unwrap();
        fs::write(home.join(".pi").join("agent").join("mcp.json"), "{ nope").unwrap();
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::write(
            home.join(".agents").join("mcp.json"),
            r#"{ "mcpServers": { "ok": { "command": "run" } } }"#,
        )
        .unwrap();

        let group = parse_pi_mcp(&home);
        assert!(group.available);
        assert_eq!(group.servers.len(), 1);
        assert_eq!(group.servers[0].name, "ok");

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn parse_pi_mcp_layers_reads_relocated_agent_dir() {
        let home = temp_home("pi-relocated");
        let agent = home.join("custom-pi");
        fs::create_dir_all(&agent).unwrap();
        fs::write(
            agent.join("mcp.json"),
            r#"{ "mcpServers": { "moved": { "command": "run" } } }"#,
        )
        .unwrap();
        fs::create_dir_all(home.join(".config").join("mcp")).unwrap();
        fs::write(
            home.join(".config").join("mcp").join("mcp.json"),
            r#"{ "mcpServers": { "shared": { "command": "shared" } } }"#,
        )
        .unwrap();

        let group = parse_pi_mcp_layers([
            agent.join("mcp.json"),
            home.join(".config").join("mcp").join("mcp.json"),
            home.join(".agents").join("mcp.json"),
            home.join(".agents").join("mcp").join("mcp.json"),
        ]);
        assert!(group.available);
        assert_eq!(group.servers.len(), 2);
        assert!(group.servers.iter().any(|server| server.name == "moved"));
        assert!(group.servers.iter().any(|server| server.name == "shared"));

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn parsers_degrade_to_empty_group_on_corrupt_config() {
        let home = temp_home("corrupt");
        fs::write(home.join(".claude.json"), "{ not valid json").unwrap();
        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::write(home.join(".codex").join("config.toml"), "= broken toml =").unwrap();
        fs::create_dir_all(home.join(".config").join("opencode")).unwrap();
        fs::write(
            home.join(".config").join("opencode").join("opencode.json"),
            "not json at all",
        )
        .unwrap();

        let claude = parse_claude_mcp(&home);
        assert!(claude.available);
        assert!(claude.servers.is_empty());
        let codex = parse_codex_mcp(&home);
        assert!(codex.available);
        assert!(codex.servers.is_empty());
        let opencode = parse_opencode_mcp(&home);
        assert!(opencode.available);
        assert!(opencode.servers.is_empty());

        fs::remove_dir_all(&home).ok();
    }
}
