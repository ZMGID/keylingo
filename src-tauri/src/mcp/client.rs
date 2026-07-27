use std::{
    collections::{HashMap, HashSet},
    process::Stdio,
    time::Duration,
};

use base64::{engine::general_purpose, Engine as _};
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE},
    Client,
};
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    time::timeout,
};

use crate::proc::NoConsoleWindow;
use crate::settings::ChatMcpServer;

use super::types::{ChatToolArtifact, McpTool, McpToolCallResult};

pub(crate) const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] =
    &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
pub(crate) const MAX_TOOL_LIST_PAGES: usize = 100;

/// JSON-RPC `initialize` params shared by every transport and the persistent
/// session manager so handshake fields stay in one place.
pub(crate) fn initialize_params() -> Value {
    serde_json::json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {
            "name": "Kivio",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

pub(crate) fn negotiated_protocol_version(
    initialize_result: &Value,
    streamable_http: bool,
) -> Result<String, String> {
    let version = initialize_result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .ok_or_else(|| "MCP initialize response is missing protocolVersion".to_string())?;
    if !SUPPORTED_PROTOCOL_VERSIONS.contains(&version) {
        return Err(format!(
            "MCP server selected unsupported protocolVersion {version}"
        ));
    }
    if streamable_http && version == "2024-11-05" {
        return Err(
            "MCP server selected protocolVersion 2024-11-05, which requires the legacy HTTP+SSE transport that Kivio does not support"
                .to_string(),
        );
    }
    Ok(version.to_string())
}

pub(crate) fn parse_tools_page(
    value: &Value,
    context: &str,
) -> Result<(Vec<McpTool>, Option<String>), String> {
    let tools = value
        .get("tools")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let tools = serde_json::from_value(tools)
        .map_err(|err| format!("{context} tools/list parse failed: {err}"))?;
    let next_cursor = match value.get("nextCursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(cursor)) => Some(cursor.clone()),
        Some(_) => return Err(format!("{context} tools/list nextCursor must be a string")),
    };
    Ok((tools, next_cursor))
}

/// 为非成功的 MCP HTTP 响应构造错误串。当服务器以 401 + Bearer 的 `WWW-Authenticate`
/// 质询（或 401 且无质询头）应答时，加 `OAUTH_REQUIRED:` 前缀，让设置页据此引导 OAuth
/// 授权，而不是把裸 401 抛给用户。非 Bearer 质询（如 Basic）不触发。
pub(crate) fn classify_http_error(
    context: &str,
    status: u16,
    www_authenticate: Option<&str>,
    body: &str,
) -> String {
    let base = format!(
        "MCP HTTP {context} failed {}: {}",
        status,
        body.chars().take(500).collect::<String>()
    );
    let is_oauth = status == 401
        && www_authenticate.map_or(true, |v| v.to_ascii_lowercase().contains("bearer"));
    if is_oauth {
        format!("OAUTH_REQUIRED: {base}")
    } else {
        base
    }
}

pub struct StdioMcpClient {
    server: ChatMcpServer,
    timeout: Duration,
}

pub struct StreamableHttpMcpClient {
    server: ChatMcpServer,
    timeout: Duration,
    http: Client,
}

struct StdioSession {
    child: Child,
    stdin: ChildStdin,
    lines: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    next_id: u64,
    timeout: Duration,
}

impl Drop for StdioSession {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl StdioMcpClient {
    pub fn new(server: ChatMcpServer, timeout_ms: u64) -> Self {
        Self {
            server,
            timeout: Duration::from_millis(timeout_ms.max(1_000)),
        }
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>, String> {
        let mut session = self.connect().await?;
        let mut tools = Vec::new();
        let mut cursor = None;
        let mut seen = HashSet::new();
        for _ in 0..MAX_TOOL_LIST_PAGES {
            let params = cursor
                .as_ref()
                .map(|cursor| serde_json::json!({ "cursor": cursor }))
                .unwrap_or(Value::Null);
            let value = session.request("tools/list", params).await?;
            let (page, next_cursor) = parse_tools_page(&value, "MCP")?;
            tools.extend(page);
            let Some(next_cursor) = next_cursor else {
                return Ok(tools);
            };
            if !seen.insert(next_cursor.clone()) {
                return Err("MCP tools/list returned a repeated nextCursor".to_string());
            }
            cursor = Some(next_cursor);
        }
        Err(format!(
            "MCP tools/list exceeded {MAX_TOOL_LIST_PAGES} pages"
        ))
    }

    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<McpToolCallResult, String> {
        let mut session = self.connect().await?;
        let value = session
            .request(
                "tools/call",
                serde_json::json!({
                    "name": name,
                    "arguments": arguments,
                }),
            )
            .await?;
        Ok(parse_tool_result(value))
    }

    async fn connect(&self) -> Result<StdioSession, String> {
        if self.server.transport != "stdio" {
            return Err("Only stdio MCP transport is supported".to_string());
        }
        if self.server.command.trim().is_empty() {
            return Err("MCP server command is empty".to_string());
        }

        let mut command = Command::new(&self.server.command);
        command.args(&self.server.args);
        if let Some(cwd) = self
            .server
            .cwd
            .as_deref()
            .filter(|cwd| !cwd.trim().is_empty())
        {
            command.current_dir(cwd);
        }
        command.envs(clean_env(&self.server.env));
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::null());
        command.no_console_window();

        let mut child = command
            .spawn()
            .map_err(|err| format!("Failed to start MCP server {}: {err}", self.server.name))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "MCP server stdin unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "MCP server stdout unavailable".to_string())?;

        let mut session = StdioSession {
            child,
            stdin,
            lines: BufReader::new(stdout).lines(),
            next_id: 1,
            timeout: self.timeout,
        };
        session.initialize().await?;
        Ok(session)
    }
}

impl StreamableHttpMcpClient {
    pub fn new(server: ChatMcpServer, timeout_ms: u64, http: Client) -> Self {
        Self {
            server,
            timeout: Duration::from_millis(timeout_ms.max(1_000)),
            http,
        }
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>, String> {
        let init = self.initialize().await?;
        let result = async {
            let mut tools = Vec::new();
            let mut cursor = None;
            let mut seen = HashSet::new();
            for _ in 0..MAX_TOOL_LIST_PAGES {
                let params = cursor
                    .as_ref()
                    .map(|cursor| serde_json::json!({ "cursor": cursor }))
                    .unwrap_or(Value::Null);
                let response = self
                    .request(
                        "tools/list",
                        params,
                        init.session_id.as_deref(),
                        &init.protocol_version,
                    )
                    .await?;
                let (page, next_cursor) = parse_tools_page(&response.result, "MCP HTTP")?;
                tools.extend(page);
                let Some(next_cursor) = next_cursor else {
                    return Ok(tools);
                };
                if !seen.insert(next_cursor.clone()) {
                    return Err("MCP HTTP tools/list returned a repeated nextCursor".to_string());
                }
                cursor = Some(next_cursor);
            }
            Err(format!(
                "MCP HTTP tools/list exceeded {MAX_TOOL_LIST_PAGES} pages"
            ))
        }
        .await;
        let _ = self
            .delete_session(init.session_id.as_deref(), &init.protocol_version)
            .await;
        result
    }

    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<McpToolCallResult, String> {
        let init = self.initialize().await?;
        let result = self
            .request(
                "tools/call",
                serde_json::json!({
                    "name": name,
                    "arguments": arguments,
                }),
                init.session_id.as_deref(),
                &init.protocol_version,
            )
            .await
            .map(|response| parse_tool_result(response.result));
        let _ = self
            .delete_session(init.session_id.as_deref(), &init.protocol_version)
            .await;
        result
    }

    async fn initialize(&self) -> Result<HttpMcpInit, String> {
        let response = self
            .request(
                "initialize",
                initialize_params(),
                None,
                MCP_PROTOCOL_VERSION,
            )
            .await?;
        let protocol_version = negotiated_protocol_version(&response.result, true)?;
        self.notify(
            "notifications/initialized",
            Value::Null,
            response.session_id.as_deref(),
            &protocol_version,
        )
        .await?;
        Ok(HttpMcpInit {
            session_id: response.session_id,
            protocol_version,
        })
    }

    async fn notify(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
        protocol_version: &str,
    ) -> Result<(), String> {
        let mut message = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
        });
        if !params.is_null() {
            message["params"] = params;
        }
        let response = self
            .post_json(message, session_id, protocol_version)
            .await?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let www_auth = response
                .headers()
                .get("www-authenticate")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let text = response.text().await.unwrap_or_default();
            return Err(classify_http_error(
                "notify",
                status,
                www_auth.as_deref(),
                &text,
            ));
        }
        Ok(())
    }

    async fn request(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
        protocol_version: &str,
    ) -> Result<HttpMcpResponse, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let outcome = async {
            let mut message = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id.clone(),
                "method": method,
            });
            if !params.is_null() {
                message["params"] = params;
            }
            let response = self
                .post_json(message, session_id, protocol_version)
                .await?;
            let next_session_id = response
                .headers()
                .get("mcp-session-id")
                .and_then(|value| value.to_str().ok())
                .map(|value| value.to_string())
                .or_else(|| session_id.map(|value| value.to_string()));
            if !response.status().is_success() {
                let status = response.status().as_u16();
                let www_auth = response
                    .headers()
                    .get("www-authenticate")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                let text = response.text().await.unwrap_or_default();
                return Err(classify_http_error(
                    "request",
                    status,
                    www_auth.as_deref(),
                    &text,
                ));
            }
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let value = if content_type.contains("text/event-stream") {
                timeout(self.timeout, read_sse_json_rpc_response(response, &id))
                    .await
                    .map_err(|_| "MCP HTTP SSE read timed out".to_string())??
            } else {
                let text = timeout(self.timeout, response.text())
                    .await
                    .map_err(|_| "MCP HTTP read body timed out".to_string())?
                    .map_err(|err| format!("MCP HTTP read body failed: {err}"))?;
                if text.trim_start().starts_with("event:") || text.trim_start().starts_with("data:")
                {
                    parse_sse_json_rpc(&text, &id)?
                } else if text.trim().is_empty() {
                    Value::Null
                } else {
                    serde_json::from_str(&text).map_err(|err| {
                        format!(
                            "MCP HTTP parse JSON failed: {} (body: {})",
                            err,
                            text.chars().take(500).collect::<String>()
                        )
                    })?
                }
            };

            if let Some(error) = value.get("error") {
                return Err(format!("MCP HTTP error: {}", compact_json(error, 500)));
            }
            Ok(HttpMcpResponse {
                result: value.get("result").cloned().unwrap_or(Value::Null),
                session_id: next_session_id,
            })
        }
        .await;

        if method != "initialize"
            && outcome
                .as_ref()
                .err()
                .is_some_and(|err| err.contains("timed out"))
        {
            let _ = self
                .notify(
                    "notifications/cancelled",
                    serde_json::json!({
                        "requestId": id,
                        "reason": "Client request timed out",
                    }),
                    session_id,
                    protocol_version,
                )
                .await;
        }
        outcome
    }

    async fn delete_session(
        &self,
        session_id: Option<&str>,
        protocol_version: &str,
    ) -> Result<(), String> {
        let Some(session_id) = session_id.filter(|value| !value.trim().is_empty()) else {
            return Ok(());
        };
        let mut headers = http_headers(&self.server.headers)?;
        headers.insert(
            HeaderName::from_static("mcp-session-id"),
            HeaderValue::from_str(session_id)
                .map_err(|err| format!("Invalid MCP session id header: {err}"))?,
        );
        headers.insert(
            HeaderName::from_static("mcp-protocol-version"),
            HeaderValue::from_str(protocol_version)
                .map_err(|err| format!("Invalid MCP protocol version header: {err}"))?,
        );
        let response = timeout(
            self.timeout.min(Duration::from_secs(5)),
            self.http
                .delete(self.server.url.clone())
                .headers(headers)
                .send(),
        )
        .await
        .map_err(|_| "MCP HTTP session DELETE timed out".to_string())?
        .map_err(|err| format!("MCP HTTP session DELETE failed: {err}"))?;
        if response.status().is_success() || response.status().as_u16() == 404 {
            Ok(())
        } else {
            Err(format!(
                "MCP HTTP session DELETE failed {}",
                response.status().as_u16()
            ))
        }
    }

    async fn post_json(
        &self,
        message: Value,
        session_id: Option<&str>,
        protocol_version: &str,
    ) -> Result<reqwest::Response, String> {
        if self.server.url.trim().is_empty() {
            return Err("MCP HTTP server URL is empty".to_string());
        }
        let mut headers = http_headers(&self.server.headers)?;
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        headers.insert(
            HeaderName::from_static("mcp-protocol-version"),
            HeaderValue::from_str(protocol_version)
                .map_err(|err| format!("Invalid MCP protocol version header: {err}"))?,
        );
        if let Some(session_id) = session_id.filter(|value| !value.trim().is_empty()) {
            let value = HeaderValue::from_str(session_id)
                .map_err(|err| format!("Invalid MCP session id header: {err}"))?;
            headers.insert(HeaderName::from_static("mcp-session-id"), value);
        }
        timeout(
            self.timeout,
            self.http
                .post(self.server.url.clone())
                .headers(headers)
                .json(&message)
                .send(),
        )
        .await
        .map_err(|_| "MCP HTTP request timed out".to_string())?
        .map_err(|err| format!("MCP HTTP request failed: {err}"))
    }
}

struct HttpMcpInit {
    session_id: Option<String>,
    protocol_version: String,
}

struct HttpMcpResponse {
    result: Value,
    session_id: Option<String>,
}

pub(crate) async fn read_sse_json_rpc_response(
    mut response: reqwest::Response,
    expected_id: &str,
) -> Result<Value, String> {
    let mut buffer = String::new();
    let mut data_lines = Vec::new();

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| format!("MCP HTTP read SSE chunk failed: {err}"))?
    {
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buffer.find('\n') {
            let line: String = buffer.drain(..=pos).collect();
            if let Some(value) = handle_sse_line(&line, &mut data_lines, expected_id)? {
                return Ok(value);
            }
        }
    }

    if !buffer.is_empty() {
        if let Some(value) = handle_sse_line(&buffer, &mut data_lines, expected_id)? {
            return Ok(value);
        }
    }
    parse_sse_event(&data_lines, expected_id)?.ok_or_else(|| {
        format!("MCP HTTP SSE response had no JSON-RPC response for id {expected_id}")
    })
}

impl StdioSession {
    async fn initialize(&mut self) -> Result<(), String> {
        let result = self.request("initialize", initialize_params()).await?;
        negotiated_protocol_version(&result, false)?;
        self.notify("notifications/initialized", Value::Null).await
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let mut message = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
        });
        if !params.is_null() {
            message["params"] = params;
        }
        self.write_message(&message).await
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut message = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
        });
        if !params.is_null() {
            message["params"] = params;
        }
        self.write_message(&message).await?;
        match self.read_response(id).await {
            Ok(value) => Ok(value),
            Err(err) => {
                if method != "initialize" && err.contains("timed out") {
                    let _ = self
                        .notify(
                            "notifications/cancelled",
                            serde_json::json!({
                                "requestId": id,
                                "reason": "Client request timed out",
                            }),
                        )
                        .await;
                }
                Err(err)
            }
        }
    }

    async fn write_message(&mut self, message: &Value) -> Result<(), String> {
        let line = serde_json::to_string(message).map_err(|err| err.to_string())?;
        timeout(self.timeout, async {
            self.stdin.write_all(line.as_bytes()).await?;
            self.stdin.write_all(b"\n").await?;
            self.stdin.flush().await
        })
        .await
        .map_err(|_| "MCP stdio write timed out".to_string())?
        .map_err(|err| format!("MCP stdio write failed: {err}"))
    }

    async fn read_response(&mut self, id: u64) -> Result<Value, String> {
        loop {
            let line = timeout(self.timeout, self.lines.next_line())
                .await
                .map_err(|_| "MCP stdio read timed out".to_string())?
                .map_err(|err| format!("MCP stdio read failed: {err}"))?
                .ok_or_else(|| "MCP server closed stdout".to_string())?;
            let value: Value = match serde_json::from_str(&line) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if let Some(method) = value.get("method").and_then(Value::as_str) {
                if let Some(request_id) = value.get("id").cloned() {
                    let response = if method == "ping" {
                        serde_json::json!({"jsonrpc":"2.0","id":request_id,"result":{}})
                    } else {
                        serde_json::json!({
                            "jsonrpc":"2.0",
                            "id":request_id,
                            "error":{"code":-32601,"message":"Method not found"}
                        })
                    };
                    self.write_message(&response).await?;
                }
                continue;
            }
            if !json_rpc_id_matches(&value, &id.to_string()) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(format!("MCP error: {}", compact_json(error, 500)));
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

pub(crate) fn clean_env(env: &HashMap<String, String>) -> Vec<(String, String)> {
    env.iter()
        .filter_map(|(key, value)| {
            let key = key.trim();
            if key.is_empty() {
                None
            } else {
                Some((key.to_string(), value.clone()))
            }
        })
        .collect()
}

pub(crate) fn http_headers(headers: &HashMap<String, String>) -> Result<HeaderMap, String> {
    let mut out = HeaderMap::new();
    for (key, value) in headers {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let name = HeaderName::from_bytes(key.as_bytes())
            .map_err(|err| format!("Invalid MCP HTTP header {key}: {err}"))?;
        let value = HeaderValue::from_str(value.trim())
            .map_err(|err| format!("Invalid MCP HTTP header value for {key}: {err}"))?;
        out.insert(name, value);
    }
    Ok(out)
}

fn handle_sse_line(
    line: &str,
    data_lines: &mut Vec<String>,
    expected_id: &str,
) -> Result<Option<Value>, String> {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
        let value = parse_sse_event(data_lines, expected_id)?;
        data_lines.clear();
        return Ok(value);
    }
    if trimmed.starts_with(':') || trimmed.starts_with("event:") || trimmed.starts_with("id:") {
        return Ok(None);
    }
    if let Some(data) = trimmed.strip_prefix("data:") {
        data_lines.push(data.trim_start().to_string());
    }
    Ok(None)
}

pub(crate) fn parse_sse_json_rpc(text: &str, expected_id: &str) -> Result<Value, String> {
    let mut data_lines = Vec::new();
    for line in text.lines() {
        if let Some(value) = handle_sse_line(line, &mut data_lines, expected_id)? {
            return Ok(value);
        }
    }
    parse_sse_event(&data_lines, expected_id)?.ok_or_else(|| {
        format!("MCP HTTP SSE response had no JSON-RPC response for id {expected_id}")
    })
}

fn parse_sse_event(data_lines: &[String], expected_id: &str) -> Result<Option<Value>, String> {
    let Some(value) = parse_sse_data_lines(data_lines)? else {
        return Ok(None);
    };
    if json_rpc_id_matches(&value, expected_id) {
        Ok(Some(value))
    } else {
        Ok(None)
    }
}

fn parse_sse_data_lines(data_lines: &[String]) -> Result<Option<Value>, String> {
    if data_lines.is_empty() {
        return Ok(None);
    }
    let data = data_lines.join("\n");
    if data.trim() == "[DONE]" {
        return Ok(None);
    }
    serde_json::from_str(&data).map(Some).map_err(|err| {
        format!(
            "MCP HTTP parse SSE JSON failed: {} (data: {})",
            err,
            data.chars().take(500).collect::<String>()
        )
    })
}

fn json_rpc_id_matches(value: &Value, expected_id: &str) -> bool {
    let Some(id) = value.get("id") else {
        return false;
    };
    id.as_str() == Some(expected_id)
        || id
            .as_u64()
            .map(|value| value.to_string() == expected_id)
            .unwrap_or(false)
        || id
            .as_i64()
            .map(|value| value.to_string() == expected_id)
            .unwrap_or(false)
}

pub(crate) fn parse_tool_result(value: Value) -> McpToolCallResult {
    let is_error = value
        .get("isError")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let structured_content = value.get("structuredContent").cloned();

    let mut artifacts: Vec<ChatToolArtifact> = Vec::new();
    let content = value
        .get("content")
        .and_then(|content| content.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| content_block_text(item, &mut artifacts))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| {
            if artifacts.is_empty() {
                compact_json(&value, 4000)
            } else {
                String::new()
            }
        });

    McpToolCallResult {
        content,
        is_error,
        raw: value,
        artifacts,
        structured_content,
        follow_up_user_messages: Vec::new(),
    }
}

/// Maps a single MCP content block to its model-facing text. Image blocks are
/// pushed onto `artifacts` and represented in text by a `[image: <mime>]`
/// placeholder so the model knows an image was produced without inlining bytes.
fn content_block_text(item: &Value, artifacts: &mut Vec<ChatToolArtifact>) -> Option<String> {
    let block_type = item.get("type").and_then(|value| value.as_str());
    if block_type == Some("image") {
        if let Some(artifact) = image_block_to_artifact(item, artifacts.len()) {
            let placeholder = format!("[image: {}]", artifact.mime_type);
            artifacts.push(artifact);
            return Some(placeholder);
        }
        return None;
    }
    item.get("text")
        .and_then(|text| text.as_str())
        .map(|text| text.to_string())
        .or_else(|| {
            item.get("resource")
                .map(|resource| compact_json(resource, 4000))
        })
}

/// Builds a `ChatToolArtifact` from an MCP `image` content block
/// (`{ "type": "image", "data": "<base64>", "mimeType": "image/png" }`).
fn image_block_to_artifact(item: &Value, index: usize) -> Option<ChatToolArtifact> {
    let data = item.get("data").and_then(|value| value.as_str())?;
    if data.trim().is_empty() {
        return None;
    }
    let mime_type = item
        .get("mimeType")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("image/png")
        .to_string();
    let size_bytes = general_purpose::STANDARD
        .decode(data.trim())
        .ok()
        .map(|bytes| bytes.len() as u64);
    let extension = mime_type
        .rsplit('/')
        .next()
        .filter(|ext| !ext.is_empty())
        .unwrap_or("png");
    // 文件名必须全局唯一：同一消息里多次 MCP 截图若都叫 mcp-image-1.png，
    // 重载后按 basename 解析会互相覆盖，且外置缩略图只显示 256px 小图。
    let unique = uuid::Uuid::new_v4().to_string();
    let short = unique.get(..8).unwrap_or(unique.as_str());
    Some(ChatToolArtifact {
        id: None,
        name: format!("mcp-image-{}-{}.{}", index + 1, short, extension),
        mime_type: mime_type.clone(),
        data_url: format!("data:{};base64,{}", mime_type, data.trim()),
        size_bytes,
        path: None,
    })
}

pub(crate) fn compact_json(value: &Value, max_chars: usize) -> String {
    let raw = serde_json::to_string(value).unwrap_or_else(|_| String::new());
    raw.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_http_error_flags_oauth_on_401_bearer() {
        // 401 + Bearer 质询 → OAUTH_REQUIRED
        assert!(classify_http_error(
            "request",
            401,
            Some("Bearer resource_metadata=\"https://x/.well-known/oauth-protected-resource/mcp\""),
            "body",
        )
        .starts_with("OAUTH_REQUIRED:"));
        // 401 且无质询头 → 仍视为需要授权（部分服务器省略该头）
        assert!(classify_http_error("request", 401, None, "body").starts_with("OAUTH_REQUIRED:"));
        // 401 但非 Bearer 质询（Basic）→ 不触发
        assert!(
            !classify_http_error("notify", 401, Some("Basic realm=x"), "body")
                .starts_with("OAUTH_REQUIRED:")
        );
        // 非 401 即使带 Bearer 也不触发
        assert!(!classify_http_error("request", 500, Some("Bearer"), "oops")
            .starts_with("OAUTH_REQUIRED:"));
        // 错误串仍保留状态码与截断的 body
        assert!(classify_http_error("request", 403, None, "denied").contains("403"));
    }

    #[test]
    fn initialize_protocol_version_is_validated_per_transport() {
        let current = serde_json::json!({ "protocolVersion": "2025-06-18" });
        assert_eq!(
            negotiated_protocol_version(&current, true).unwrap(),
            "2025-06-18"
        );

        // 服务器可选更新的修订版（TinyFish 等只讲 2025-11-25）。
        let newest = serde_json::json!({ "protocolVersion": "2025-11-25" });
        assert_eq!(
            negotiated_protocol_version(&newest, true).unwrap(),
            "2025-11-25"
        );

        let legacy = serde_json::json!({ "protocolVersion": "2024-11-05" });
        assert_eq!(
            negotiated_protocol_version(&legacy, false).unwrap(),
            "2024-11-05"
        );
        assert!(negotiated_protocol_version(&legacy, true)
            .unwrap_err()
            .contains("legacy HTTP+SSE"));

        let unsupported = serde_json::json!({ "protocolVersion": "2099-01-01" });
        assert!(negotiated_protocol_version(&unsupported, false)
            .unwrap_err()
            .contains("unsupported protocolVersion"));
    }

    #[test]
    fn tools_page_rejects_non_string_cursor() {
        let err = parse_tools_page(&serde_json::json!({ "tools": [], "nextCursor": 42 }), "MCP")
            .unwrap_err();
        assert!(err.contains("nextCursor must be a string"));
    }
    use std::collections::HashMap;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn parse_sse_json_rpc_reads_matching_json_data_event() {
        let value = parse_sse_json_rpc(
            r#"event: message
data: {"jsonrpc":"2.0","method":"notifications/progress","params":{"progress":1}}

event: message
data: {"jsonrpc":"2.0","id":"other","result":{"tools":[{"name":"wrong"}]}}

event: message
data: {"jsonrpc":"2.0","id":"target","result":{"tools":[{"name":"fetch"}]}}

"#,
            "target",
        )
        .expect("sse json should parse");
        assert_eq!(
            value
                .get("result")
                .and_then(|result| result.get("tools"))
                .and_then(|tools| tools.get(0))
                .and_then(|tool| tool.get("name"))
                .and_then(|name| name.as_str()),
            Some("fetch"),
        );
    }

    #[test]
    fn parse_sse_json_rpc_rejects_empty_streams() {
        let err =
            parse_sse_json_rpc("event: ping\n\n", "target").expect_err("empty data should fail");
        assert!(err.contains("no JSON-RPC response"));
    }

    #[test]
    fn parse_sse_json_rpc_accepts_numeric_matching_id() {
        let value = parse_sse_json_rpc(
            r#"event: message
data: {"jsonrpc":"2.0","id":7,"result":{"ok":true}}

"#,
            "7",
        )
        .expect("numeric id should match string request id");
        assert_eq!(
            value
                .get("result")
                .and_then(|result| result.get("ok"))
                .and_then(|ok| ok.as_bool()),
            Some(true),
        );
    }

    #[test]
    fn parse_tool_result_preserves_structured_content() {
        let result = parse_tool_result(serde_json::json!({
            "content": [{ "type": "text", "text": "summary" }],
            "structuredContent": {
                "items": [{ "title": "A" }]
            },
            "isError": false
        }));

        assert_eq!(result.content, "summary");
        assert_eq!(
            result.structured_content.as_ref(),
            Some(&serde_json::json!({ "items": [{ "title": "A" }] }))
        );
        assert!(!result.is_error);
    }

    #[test]
    fn parse_tool_result_maps_image_to_artifact() {
        // "hello" base64 → aGVsbG8=
        let result = parse_tool_result(serde_json::json!({
            "content": [
                { "type": "text", "text": "here is a chart" },
                { "type": "image", "data": "aGVsbG8=", "mimeType": "image/png" }
            ],
            "isError": false
        }));

        assert_eq!(result.artifacts.len(), 1);
        let artifact = &result.artifacts[0];
        assert_eq!(artifact.mime_type, "image/png");
        assert_eq!(artifact.data_url, "data:image/png;base64,aGVsbG8=");
        assert_eq!(artifact.size_bytes, Some(5));
        assert!(artifact.name.ends_with(".png"));
        // Text content keeps the prose and inserts a placeholder for the image.
        assert_eq!(result.content, "here is a chart\n[image: image/png]");
        assert!(!result.is_error);
    }

    #[test]
    fn parse_tool_result_image_only_has_empty_content() {
        let result = parse_tool_result(serde_json::json!({
            "content": [
                { "type": "image", "data": "aGVsbG8=", "mimeType": "image/jpeg" }
            ]
        }));

        assert_eq!(result.artifacts.len(), 1);
        assert_eq!(result.artifacts[0].mime_type, "image/jpeg");
        assert!(result.artifacts[0].name.ends_with(".jpeg"));
        assert_eq!(result.content, "[image: image/jpeg]");
    }

    #[tokio::test]
    async fn streamable_http_client_lists_and_calls_tools_from_json_responses() {
        let url = spawn_test_http_mcp_server(false).await;
        let client =
            StreamableHttpMcpClient::new(test_http_server(url), 5_000, reqwest::Client::new());

        let tools = client.list_tools().await.expect("tools/list should work");
        assert_eq!(tools.first().map(|tool| tool.name.as_str()), Some("echo"));

        let result = client
            .call_tool("echo", serde_json::json!({ "text": "hello" }))
            .await
            .expect("tools/call should work");
        assert_eq!(result.content, "echo: hello");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn streamable_http_client_lists_and_calls_tools_from_sse_responses() {
        let url = spawn_test_http_mcp_server(true).await;
        let client =
            StreamableHttpMcpClient::new(test_http_server(url), 5_000, reqwest::Client::new());

        let tools = client
            .list_tools()
            .await
            .expect("sse tools/list should work");
        assert_eq!(tools.first().map(|tool| tool.name.as_str()), Some("echo"));

        let result = client
            .call_tool("echo", serde_json::json!({ "text": "sse" }))
            .await
            .expect("sse tools/call should work");
        assert_eq!(result.content, "echo: sse");
    }

    fn test_http_server(url: String) -> ChatMcpServer {
        ChatMcpServer {
            id: "test-http".to_string(),
            name: "Test HTTP".to_string(),
            enabled: true,
            transport: "streamable_http".to_string(),
            url,
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            headers: HashMap::new(),
            cwd: None,
            enabled_tools: Vec::new(),
            connector_id: None,
            auth: None,
        }
    }

    async fn spawn_test_http_mcp_server(use_sse: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("test server addr");
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buffer = vec![0_u8; 8192];
                    let mut read = 0_usize;
                    loop {
                        let Ok(n) = stream.read(&mut buffer[read..]).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        read += n;
                        let request = String::from_utf8_lossy(&buffer[..read]);
                        let Some(header_end) = request.find("\r\n\r\n") else {
                            continue;
                        };
                        let content_length = request
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("Content-Length:")
                                    .or_else(|| line.strip_prefix("content-length:"))
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if read < header_end + 4 + content_length {
                            continue;
                        }
                        let body = &request[header_end + 4..header_end + 4 + content_length];
                        let message: Value = serde_json::from_str(body).expect("json request");
                        let response = test_http_mcp_response(&message);
                        let raw = if use_sse && message.get("id").is_some() {
                            let other = serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": "other",
                                "result": { "ignored": true },
                            });
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nMcp-Session-Id: test-session\r\nConnection: close\r\n\r\nevent: message\ndata: {}\n\nevent: message\ndata: {}\n\n",
                                other,
                                response,
                            )
                        } else {
                            let body = if message.get("id").is_some() {
                                response.to_string()
                            } else {
                                String::new()
                            };
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: test-session\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                body.len(),
                                body,
                            )
                        };
                        let _ = stream.write_all(raw.as_bytes()).await;
                        let _ = stream.shutdown().await;
                        return;
                    }
                });
            }
        });
        format!("http://{addr}/mcp")
    }

    fn test_http_mcp_response(message: &Value) -> Value {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        match message
            .get("method")
            .and_then(|method| method.as_str())
            .unwrap_or_default()
        {
            "initialize" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "test", "version": "1.0.0" },
                },
            }),
            "tools/list" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [{
                        "name": "echo",
                        "description": "Echo text",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "text": { "type": "string" } },
                        },
                    }],
                },
            }),
            "tools/call" => {
                let text = message
                    .get("params")
                    .and_then(|params| params.get("arguments"))
                    .and_then(|arguments| arguments.get("text"))
                    .and_then(|text| text.as_str())
                    .unwrap_or_default();
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [{ "type": "text", "text": format!("echo: {text}") }],
                    },
                })
            }
            _ => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {},
            }),
        }
    }
}
