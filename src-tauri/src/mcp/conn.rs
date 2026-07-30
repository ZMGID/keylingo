//! MCP wire 层 —— 唯一的协议实现，基于官方 rmcp SDK。
//!
//! 取代 `client.rs`（一次性连接）和 `manager.rs`（连接池自己那套 `StdioConn` /
//! `http_*`）两份手写 JSON-RPC 收发。上层的连接池、指纹重建、状态事件、退避、
//! 空闲回收、工具快照缓存仍然留在 `manager.rs` —— 那些是运维逻辑，rmcp 不管。
//!
//! 这一层只负责三件事：
//! 1. 按 `ChatMcpServer` 建 transport 并握手，产出 `RunningService`；
//! 2. 把 `tools/list_changed` 通知转成 `tools_revision` 自增（stdio 与 HTTP 都生效，
//!    手写版只有 stdio 收得到）；
//! 3. 把 rmcp 的错误类型翻译成我们的 `String`，并保住 `OAUTH_REQUIRED:` 前缀契约。

use std::{
    collections::HashMap,
    error::Error as StdError,
    future::Future,
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use reqwest::header::{HeaderName, HeaderValue};
use rmcp::{
    model::{
        CallToolRequest, CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo,
        ClientRequest, Implementation, ProtocolVersion, ServerResult,
    },
    service::{NotificationContext, PeerRequestOptions, RoleClient, RunningService},
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
        TokioChildProcess,
    },
    ClientHandler, ServiceExt,
};
use serde_json::Value;
use tokio::process::{ChildStderr, Command};

use crate::proc::NoConsoleWindow;
use crate::settings::ChatMcpServer;

/// 我们向服务器宣称的协议版本。
///
/// rmcp 自己的 `ProtocolVersion::LATEST` 是 2025-11-25，但握手不做任何版本校验
/// （见 `service::client::legacy_startup`），未知版本字符串也能反序列化通过，所以
/// 宣称哪个版本纯粹是兼容性选择，不影响我们要不要实现规范。
///
/// ponytail: 先沿用手写版一直在发的 2025-06-18，让这次重构在 wire 上零变化；
/// 想吃 2025-11-25/2026-07-28 的新特性时改这一行即可。
const ADVERTISED_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::V_2025_06_18;

/// `tools/list` 的整体超时上限。工具清单不该慢过这个数，慢了就是服务器有问题。
pub const LIST_TOOLS_TIMEOUT: Duration = Duration::from_secs(30);

/// rmcp 服务句柄的具体类型 —— stdio 与 HTTP 共用同一个，所以上层不再需要
/// 按传输方式分叉。
pub type McpService = RunningService<RoleClient, KivioClientHandler>;

/// 一条活着的 MCP 连接。
pub struct Conn {
    pub service: Arc<McpService>,
    /// 服务器每发一次 `notifications/tools/list_changed` 就 +1，上层据此决定
    /// 要不要重新 `tools/list`。
    pub tools_revision: Arc<AtomicU64>,
    /// stdio 才有；上层用它起 stderr 尾巴任务（连接失败时把最后几行贴进错误信息）。
    pub stderr: Option<ChildStderr>,
}

/// 我们的 `ClientHandler`：除了上报 client info，唯一职责就是把
/// `tools/list_changed` 记成一次 revision 自增。
#[derive(Clone, Default)]
pub struct KivioClientHandler {
    tools_revision: Arc<AtomicU64>,
}

impl KivioClientHandler {
    pub fn revision_handle(&self) -> Arc<AtomicU64> {
        self.tools_revision.clone()
    }
}

impl ClientHandler for KivioClientHandler {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::new(
            ClientCapabilities::default(),
            Implementation::new("Kivio", env!("CARGO_PKG_VERSION")),
        )
        .with_protocol_version(ADVERTISED_PROTOCOL_VERSION)
    }

    fn on_tool_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        self.tools_revision.fetch_add(1, Ordering::Relaxed);
        std::future::ready(())
    }
}

/// 建连并握手。`http` 只有 streamable_http 传输会用到。
pub async fn connect(server: &ChatMcpServer, http: &reqwest::Client) -> Result<Conn, String> {
    let handler = KivioClientHandler::default();
    let tools_revision = handler.revision_handle();

    match server.transport.as_str() {
        "streamable_http" => {
            let transport = build_http(server, http)?;
            let service = handler
                .serve(transport)
                .await
                .map_err(|err| classify_error("connect", &err))?;
            Ok(Conn {
                service: Arc::new(service),
                tools_revision,
                stderr: None,
            })
        }
        _ => {
            let (transport, stderr) = build_stdio(server)?;
            let service = handler
                .serve(transport)
                .await
                .map_err(|err| classify_error("connect", &err))?;
            Ok(Conn {
                service: Arc::new(service),
                tools_revision,
                stderr,
            })
        }
    }
}

/// 一次性连接：建连 → 干活 → 关掉，不进连接池。
///
/// 只有两个正当用途，别拿它当第二条 wire 路径：
/// - 设置页的「测试连接」按钮 —— 测的是**还没保存**的草稿配置，不该污染连接池；
/// - `web_search` 里那个临时合成的 exa MCP server（api key 在 URL 里）。
///
/// 用的是和 `connect` 完全相同的 transport 构造，所以 wire 实现只有一份。
pub async fn connect_once<T, F, Fut>(
    server: &ChatMcpServer,
    http: &reqwest::Client,
    work: F,
) -> Result<T, String>
where
    F: FnOnce(Arc<McpService>) -> Fut,
    Fut: Future<Output = Result<T, String>>,
{
    let established = connect(server, http).await?;
    let service = established.service.clone();
    let outcome = work(service).await;
    // 无论成败都收干净：取消服务循环 → stdio 走 graceful_shutdown 杀子进程，
    // HTTP 发 DELETE 释放 session。
    let Conn { service, .. } = established;
    service.cancellation_token().cancel();
    if let Some(mut owned) = Arc::into_inner(service) {
        let _ = owned.close_with_timeout(Duration::from_secs(5)).await;
    }
    outcome
}

/// 列一次工具就走，`connect_once` 最常见的用法。
pub async fn list_tools_once(
    server: &ChatMcpServer,
    http: &reqwest::Client,
) -> Result<Vec<crate::mcp::types::McpTool>, String> {
    connect_once(server, http, |service| async move {
        list_tools(&service, LIST_TOOLS_TIMEOUT).await
    })
    .await
}

/// 调一次工具就走。
pub async fn call_tool_once(
    server: &ChatMcpServer,
    http: &reqwest::Client,
    name: &str,
    arguments: Value,
    timeout: Duration,
) -> Result<CallToolResult, String> {
    connect_once(server, http, |service| async move {
        call_tool(&service, name, arguments, timeout).await
    })
    .await
}

fn build_stdio(server: &ChatMcpServer) -> Result<(TokioChildProcess, Option<ChildStderr>), String> {
    if server.command.trim().is_empty() {
        return Err("MCP server command is empty".to_string());
    }
    // which 解析绝对路径：Windows 上 `npx` 这类 `.cmd` shim 直接交给
    // tokio::Command 是找不到的。解析失败就退回原样，让 spawn 去报真正的错。
    let mut command = rmcp::transport::which_command(&server.command)
        .unwrap_or_else(|_| Command::new(&server.command));
    command.args(&server.args);
    if let Some(cwd) = server.cwd.as_deref().filter(|cwd| !cwd.trim().is_empty()) {
        command.current_dir(cwd);
    }
    command.envs(clean_env(&server.env));
    command.kill_on_drop(true);
    command.no_console_window();

    TokioChildProcess::builder(command)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("Failed to start MCP server {}: {err}", server.name))
}

fn build_http(
    server: &ChatMcpServer,
    http: &reqwest::Client,
) -> Result<StreamableHttpClientTransport<reqwest::Client>, String> {
    let url = server.url.trim();
    if url.is_empty() {
        return Err("MCP server url is empty".to_string());
    }
    // OAuth 的 bearer token 由 connectors 写进 server.headers["Authorization"]，
    // 所以这里不用 rmcp 的 auth_header —— headers 整体过一遍就够了，只有一条路径。
    let mut config = StreamableHttpClientTransportConfig::with_uri(url);
    config.custom_headers = custom_headers(&server.headers)?;
    Ok(StreamableHttpClientTransport::with_client(
        http.clone(),
        config,
    ))
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

/// 发一次 `tools/call`，带超时。
///
/// 走 `send_cancellable_request` + `PeerRequestOptions::with_timeout` 而**不是**
/// `tokio::time::timeout(service.call_tool(..))`：`RequestHandle` 没有 `Drop` 实现，
/// 从外面把 future 丢掉只会静默放弃请求，服务器那边还在跑。rmcp 自己的超时路径会
/// 发 `notifications/cancelled` 再返回 `ServiceError::Timeout`，这才是我们一直有的
/// 语义 ——「结果未知、通知对端、绝不重放」。
pub async fn call_tool(
    service: &McpService,
    name: &str,
    arguments: Value,
    timeout: Duration,
) -> Result<CallToolResult, String> {
    let mut params = CallToolRequestParams::new(name.to_string());
    // 非 object 的 arguments（模型偶尔发 null / 数组）按「无参数」处理，与手写版一致。
    if let Value::Object(map) = arguments {
        params = params.with_arguments(map);
    }
    let handle = service
        .send_cancellable_request(
            ClientRequest::CallToolRequest(CallToolRequest::new(params)),
            PeerRequestOptions::with_timeout(timeout),
        )
        .await
        .map_err(|err| classify_error("tools/call", &err))?;
    match handle
        .await_response()
        .await
        .map_err(|err| classify_error("tools/call", &err))?
    {
        ServerResult::CallToolResult(result) => Ok(result),
        other => Err(format!(
            "MCP tools/call returned an unexpected result type: {other:?}"
        )),
    }
}

/// 列全部工具，带整体超时。
///
/// rmcp 的 `list_all_tools` 会跟着 `nextCursor` 一直翻页，**没有页数上限**（手写版有
/// `MAX_TOOL_LIST_PAGES = 100` + 重复游标检测）。游标坏掉的服务器会让它永远转下去，
/// 而 `tools/list` 在聊天热路径上。这里套一层超时把这个洞堵住 —— 取消 future 是安全的，
/// `tools/list` 是幂等读。
pub async fn list_tools(
    service: &McpService,
    timeout: Duration,
) -> Result<Vec<crate::mcp::types::McpTool>, String> {
    let tools = tokio::time::timeout(timeout, service.list_all_tools())
        .await
        .map_err(|_| {
            "MCP tools/list timed out (server may be paginating without end)".to_string()
        })?
        .map_err(|err| classify_error("tools/list", &err))?;
    tools.into_iter().map(tool_from_rmcp).collect()
}

/// 连接是不是已经没了。
///
/// `is_closed()` 看的是 rmcp 服务循环有没有结束，子进程刚死时它可能还没反应过来；
/// 手写版是直接 `try_wait()` 子进程 + 匹配「closed stdout」错误串。这里补上错误串这一路，
/// 免得在那个时间窗里把「该重连」误判成「透传错误」。
pub fn connection_is_gone(service: &McpService, err: &str) -> bool {
    service.is_closed() || err.contains("Transport closed")
}

/// rmcp 的 `Tool` → 我们的 `McpTool`。两边 serde 都是 camelCase（`inputSchema` /
/// `outputSchema`），所以过一趟 JSON 就行，不用手抄字段。见 `result.rs` 的契约测试。
pub fn tool_from_rmcp(tool: rmcp::model::Tool) -> Result<crate::mcp::types::McpTool, String> {
    let value = serde_json::to_value(&tool)
        .map_err(|err| format!("MCP tool {} serialize failed: {err}", tool.name))?;
    serde_json::from_value(value)
        .map_err(|err| format!("MCP tool {} parse failed: {err}", tool.name))
}

fn custom_headers(
    headers: &HashMap<String, String>,
) -> Result<HashMap<HeaderName, HeaderValue>, String> {
    let mut out = HashMap::new();
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

/// 把 rmcp 的错误变成我们的错误串，并在需要授权时加上 `OAUTH_REQUIRED:` 前缀
/// —— 设置页据此引导用户走 OAuth，而不是把裸 401 抛出去。
///
/// ponytail: 走 Display + source 链的文本匹配，而不是 downcast。真正的 HTTP 错误
/// 被 `ServiceError::TransportSend(DynamicTransportError)` 装成
/// `Box<dyn Error>`，要 downcast 得先拼出 `WorkerTransport<StreamableHttpClientWorker<
/// reqwest::Client>>` 这串泛型，不值当。下面的单测用真的 rmcp 错误值构造，
/// 所以 rmcp 改文案会让测试红掉，而不是静默回退。
pub fn classify_error<E: StdError>(context: &str, err: &E) -> String {
    let chain = error_chain(err);
    let mut base = format!("MCP {context} failed: {chain}");
    if is_timeout(&chain) {
        // rmcp 只说 "request timeout after PT1S"，但用户需要知道的是：请求已被取消
        // 通知给服务器，可服务器那边到底执行没执行不知道，我们也不会替他重试。
        // 手写版一直是这么说的，别丢。
        base.push_str("; timed out — request outcome is unknown and was not retried");
    }
    if requires_oauth(&chain) {
        format!("OAUTH_REQUIRED: {base}")
    } else {
        base
    }
}

fn is_timeout(chain: &str) -> bool {
    chain.to_ascii_lowercase().contains("request timeout after")
}

fn error_chain<E: StdError>(err: &E) -> String {
    let mut parts = vec![err.to_string()];
    let mut source: Option<&(dyn StdError + 'static)> = err.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        if !parts.iter().any(|part| part == &text) {
            parts.push(text);
        }
        source = cause.source();
    }
    parts.join(": ")
}

fn requires_oauth(chain: &str) -> bool {
    let lower = chain.to_ascii_lowercase();
    // AuthRequiredError / InsufficientScopeError 的 Display 文案。
    if lower.contains("authorization required") || lower.contains("insufficient scope") {
        return true;
    }
    // 401 但没带 WWW-Authenticate 质询头时 rmcp 不走 AuthRequired，而是落到
    // UnexpectedServerResponse("HTTP 401 ...")。手写版对这种也引导 OAuth，保持一致。
    lower.contains("http 401")
}

#[cfg(test)]
mod tests {
    use rmcp::transport::streamable_http_client::{
        AuthRequiredError, InsufficientScopeError, StreamableHttpError,
    };
    use std::borrow::Cow;

    use super::*;

    type TestHttpError = StreamableHttpError<std::io::Error>;

    #[test]
    fn oauth_prefix_on_401_with_bearer_challenge() {
        let err: TestHttpError = StreamableHttpError::AuthRequired(AuthRequiredError::new(
            "Bearer realm=\"mcp\"".to_string(),
        ));
        assert!(classify_error("connect", &err).starts_with("OAUTH_REQUIRED: "));
    }

    #[test]
    fn oauth_prefix_on_bare_401_without_challenge_header() {
        // rmcp 对无质询头的 401 走 UnexpectedServerResponse，串里带状态码。
        let err: TestHttpError = StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
            "HTTP 401 Unauthorized: {\"error\":\"unauthorized\"}".to_string(),
        ));
        assert!(classify_error("connect", &err).starts_with("OAUTH_REQUIRED: "));
    }

    #[test]
    fn oauth_prefix_on_403_insufficient_scope() {
        let err: TestHttpError = StreamableHttpError::InsufficientScope(
            InsufficientScopeError::new(
                "Bearer error=\"insufficient_scope\", scope=\"files:read\"".to_string(),
                Some("files:read".to_string()),
            ),
        );
        assert!(classify_error("connect", &err).starts_with("OAUTH_REQUIRED: "));
    }

    #[test]
    fn no_oauth_prefix_on_session_expired_or_500() {
        let expired: TestHttpError = StreamableHttpError::SessionExpired;
        assert!(!classify_error("call", &expired).starts_with("OAUTH_REQUIRED: "));

        let server_error: TestHttpError = StreamableHttpError::UnexpectedServerResponse(
            Cow::Owned("HTTP 500 Internal Server Error: boom".to_string()),
        );
        let text = classify_error("call", &server_error);
        assert!(!text.starts_with("OAUTH_REQUIRED: "));
        assert!(text.contains("500"));
    }

    #[test]
    fn no_oauth_prefix_on_transport_closed() {
        let err: TestHttpError = StreamableHttpError::TransportChannelClosed;
        assert!(!classify_error("call", &err).starts_with("OAUTH_REQUIRED: "));
    }

    #[test]
    fn timeout_says_outcome_is_unknown_and_not_retried() {
        // 这句话是给用户看的：超时的工具调用可能已经在服务器上执行了，我们没有重试。
        // 光说 "request timeout after PT1S" 传达不到这一点。
        let err = rmcp::ServiceError::Timeout {
            timeout: Duration::from_secs(1),
        };
        let text = classify_error("tools/call", &err);
        assert!(text.contains("outcome is unknown"), "{text}");
        assert!(text.contains("was not retried"), "{text}");
        assert!(!text.starts_with("OAUTH_REQUIRED: "));
    }

    #[test]
    fn error_chain_includes_source_detail() {
        let err: TestHttpError = StreamableHttpError::AuthRequired(AuthRequiredError::new(
            "Bearer realm=\"notion\"".to_string(),
        ));
        // 顶层 Display 只有 "Auth required"，质询串在 source 里 —— 必须带出来，
        // 否则用户看不到是哪个 realm 要授权。
        assert!(classify_error("connect", &err).contains("notion"));
    }

    #[test]
    fn clean_env_drops_blank_keys() {
        let env = HashMap::from([
            ("  ".to_string(), "ignored".to_string()),
            (" TOKEN ".to_string(), "v".to_string()),
        ]);
        let out = clean_env(&env);
        assert_eq!(out, vec![("TOKEN".to_string(), "v".to_string())]);
    }

    /// stderr 必须真的被 piped 拿到 —— 这是唯一会**静默降级**的点：拿不到 stderr，
    /// 连接失败时用户就看不到服务器到底为什么起不来（`connect_session` 会把
    /// stderr 尾巴贴进错误信息）。
    #[tokio::test]
    async fn build_stdio_pipes_stderr_and_reads_it() {
        let mut server = ChatMcpServer::default();
        server.name = "stderr probe".to_string();
        // 一个只往 stderr 写一行然后退出的进程。它不说 MCP 协议，握手会失败 —— 无所谓，
        // 这里测的就是 build_stdio 那一步。
        #[cfg(windows)]
        {
            server.command = "cmd".to_string();
            server.args = vec!["/c".into(), "echo boom 1>&2".into()];
        }
        #[cfg(not(windows))]
        {
            server.command = "sh".to_string();
            server.args = vec!["-c".into(), "echo boom 1>&2".into()];
        }

        let (transport, stderr) = build_stdio(&server).expect("spawn");
        let mut stderr = stderr.expect("rmcp 必须把 piped stderr 交回来");

        let mut buf = String::new();
        tokio::io::AsyncReadExt::read_to_string(&mut stderr, &mut buf)
            .await
            .expect("read stderr");
        assert!(buf.contains("boom"), "读到的 stderr: {buf:?}");

        drop(transport);
    }

    #[test]
    fn empty_command_is_rejected_before_spawn() {
        let server = ChatMcpServer::default();
        assert!(build_stdio(&server).is_err());
    }

    #[test]
    fn custom_headers_maps_authorization_and_rejects_bad_names() {
        let ok = custom_headers(&HashMap::from([(
            "Authorization".to_string(),
            "Bearer abc".to_string(),
        )]))
        .expect("Authorization 不是 rmcp 的保留头，必须能过");
        assert_eq!(ok.get(&HeaderName::from_static("authorization")).unwrap(), "Bearer abc");

        assert!(custom_headers(&HashMap::from([(
            "bad header".to_string(),
            "v".to_string()
        )]))
        .is_err());
    }
}
