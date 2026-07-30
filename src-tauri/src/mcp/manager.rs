//! 持久 MCP 连接管理器。
//!
//! 取代旧的"每次调用都 spawn + 握手"一次性连接：每个 server 维护一个
//! 长连接 `McpSession`，stdio 子进程常驻、握手只做一次，按 server_id 挂在
//! `AppState.mcp_sessions` 连接池里。生命周期相关的 reaper / warmup / 退出杀进程
//! 由 main.rs 调度。
//!
//! wire 协议本身不在这里 —— 那是 `super::conn`（官方 rmcp SDK）的活。本文件只管
//! 连接池、配置指纹、状态事件、退避、空闲回收、工具快照这些运维逻辑，stdio 与
//! HTTP 共用同一个 `RunningService`，不再分叉。
//!
//! 关键约束（见 prd 风险段）：
//! - 绝不跨握手 / RPC await 持 `mcp_sessions` 外层池锁；命中即克隆 per-session
//!   `Arc<Mutex<McpSession>>` 后立即释放外层锁。
//! - 会话锁只保护生命周期状态迁移；等 RPC 响应前必须先克隆 `Arc<McpService>`
//!   并释放会话锁，否则一次丢响应就会 head-of-line 阻塞后续每个请求。
//! - stdio 子进程 `kill_on_drop(true)`，`RunningService` 的 DropGuard 会取消服务
//!   循环并走 `transport.close()`（stdio = graceful_shutdown 杀子进程），退出时再走
//!   一遍 `disconnect_all` 兜底，避免孤儿进程。

use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::ChildStderr,
    sync::Mutex,
};

use crate::settings::ChatMcpServer;
use crate::state::AppState;

use super::conn::{self, McpService};
use super::result;
use super::types::{McpTool, McpToolCallResult};

const MCP_DISCOVERY_RETRY_BASE: Duration = Duration::from_secs(2);
const MCP_DISCOVERY_RETRY_MAX: Duration = Duration::from_secs(60);

/// 关连接时最多等多久（rmcp 内部 stdio graceful_shutdown 自己有 3s 上限）。
const TRANSPORT_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// stderr 尾巴最多保留多少行（用于状态面板诊断）。
pub const STDERR_TAIL_LINES: usize = 20;

/// 给前端的 MCP 服务器连接状态。`#[serde(tag = "kind")]` ⇒
/// `{ "kind": "connected" }` / `{ "kind": "error", "message": "..." }`。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum McpServerState {
    Connecting,
    Connected,
    Error { message: String },
    Disconnected,
}

/// 状态事件发射器。生产代码用 `AppHandle` 发 `mcp-server-state`；测试用 `()` 空实现，
/// 这样核心连接逻辑无需真实 Tauri AppHandle 即可单测。
pub trait McpEventSink {
    fn emit_server_state(&self, server: &ChatMcpServer, state: &McpServerState);
    /// 仅靠 server_id 发 Disconnected（reload / reap 用，可能拿不到完整 server）。
    fn emit_disconnected(&self, server_id: &str);
    /// 供 token 刷新钩子持久化新 token 用的 AppHandle。测试用 `()` 返回 None。
    fn app_handle(&self) -> Option<&AppHandle> {
        None
    }
}

impl McpEventSink for AppHandle {
    fn emit_server_state(&self, server: &ChatMcpServer, state: &McpServerState) {
        let _ = self.emit(
            "mcp-server-state",
            serde_json::json!({
                "serverId": server.id,
                "serverName": server.name,
                "state": state,
            }),
        );
    }

    fn emit_disconnected(&self, server_id: &str) {
        let _ = self.emit(
            "mcp-server-state",
            serde_json::json!({
                "serverId": server_id,
                "state": McpServerState::Disconnected,
            }),
        );
    }

    fn app_handle(&self) -> Option<&AppHandle> {
        Some(self)
    }
}

impl McpEventSink for () {
    fn emit_server_state(&self, _server: &ChatMcpServer, _state: &McpServerState) {}
    fn emit_disconnected(&self, _server_id: &str) {}
}

/// 单个 MCP 服务器的持久会话。
pub struct McpSession {
    /// ChatMcpServer 序列化指纹：配置变更即重建会话。
    pub config_fingerprint: String,
    pub state: McpServerState,
    pub tools: Vec<McpTool>,
    /// 上一次物化进 `tools` 时的 `tools/list_changed` 计数。
    pub tools_revision: u64,
    /// stderr 尾巴（最近 STDERR_TAIL_LINES 行），用于状态面板。
    pub stderr_tail: Arc<Mutex<VecDeque<String>>>,
    pub last_used: Instant,
    pub handshake_count: u64,
    /// Discovery reconnect failures are throttled so a dead server cannot delay every chat turn.
    pub consecutive_connect_failures: u32,
    pub discovery_retry_after: Option<Instant>,
    /// 活着的 rmcp 服务句柄；`None` 表示占位/已断开。stdio 与 HTTP 同一个类型。
    pub transport: Option<Arc<McpService>>,
    /// 服务器侧 `tools/list_changed` 计数器（由 `conn::KivioClientHandler` 递增）。
    /// 和 `transport` 一起换，`None` 时不参与比较。
    revision_source: Option<Arc<std::sync::atomic::AtomicU64>>,
    /// stderr 尾巴任务，换连接时 abort 掉旧的。
    stderr_task: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for McpSession {
    fn drop(&mut self) {
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
    }
}

impl McpSession {
    /// 新建占位会话（Connecting，无 transport），插入连接池占位以阻止并发重复握手。
    fn placeholder(fingerprint: String) -> Self {
        Self {
            config_fingerprint: fingerprint,
            state: McpServerState::Connecting,
            tools: Vec::new(),
            tools_revision: 0,
            stderr_tail: Arc::new(Mutex::new(VecDeque::new())),
            last_used: Instant::now(),
            handshake_count: 0,
            consecutive_connect_failures: 0,
            discovery_retry_after: None,
            transport: None,
            revision_source: None,
            stderr_task: None,
        }
    }

    /// 读取 stderr 尾巴快照（拼成多行字符串）。
    pub async fn stderr_tail_text(&self) -> String {
        let tail = self.stderr_tail.lock().await;
        tail.iter().cloned().collect::<Vec<_>>().join("\n")
    }

    /// 服务器当前上报的 tools 版本；没有连接时退回已物化的版本（⇒ 不触发重列）。
    fn live_tools_revision(&self) -> u64 {
        match &self.revision_source {
            Some(source) => source.load(std::sync::atomic::Ordering::Acquire),
            None => self.tools_revision,
        }
    }

    /// 摘下当前 transport（换连接 / 断开时用），顺手 abort stderr 任务。
    fn take_transport(&mut self) -> Option<Arc<McpService>> {
        self.revision_source = None;
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
        self.transport.take()
    }
}

/// 把子进程 stderr 折进会话的环形尾巴（最多 `STDERR_TAIL_LINES` 行），
/// 连接失败时贴进错误信息给用户看。
fn spawn_stderr_tail(
    stderr: ChildStderr,
    tail: Arc<Mutex<VecDeque<String>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let mut tail = tail.lock().await;
            if tail.len() >= STDERR_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
        }
    })
}

impl AppState {
    /// 当前生效的空闲超时（来自设置 `mcp_idle_timeout_ms`）。
    pub fn mcp_idle_timeout(&self) -> Duration {
        let ms = self.settings_read().chat_tools.mcp_idle_timeout_ms;
        Duration::from_millis(ms)
    }

    /// 取该 server 的工具超时（ms）。
    fn mcp_tool_timeout(&self) -> Duration {
        let ms = self.settings_read().chat_tools.tool_timeout_ms.max(1_000);
        Duration::from_millis(ms)
    }

    /// 命中已连接会话则克隆 Arc 返回；否则建立连接（握手一次）。
    ///
    /// 关键：算 fingerprint / 命中判断在持外层池锁时完成，确定要新建后插入
    /// Connecting 占位、**立即释放外层锁**，再 spawn + initialize（不跨外层锁 await）。
    pub async fn mcp_get_or_connect(
        &self,
        sink: &impl McpEventSink,
        server: &ChatMcpServer,
    ) -> Result<Arc<Mutex<McpSession>>, String> {
        self.mcp_get_or_connect_inner(sink, server, false).await
    }

    async fn mcp_get_or_connect_inner(
        &self,
        sink: &impl McpEventSink,
        server: &ChatMcpServer,
        respect_discovery_backoff: bool,
    ) -> Result<Arc<Mutex<McpSession>>, String> {
        // 连接前钩子：远程 MCP 的 OAuth token 若临近/已过期，先用 refresh_token 刷新，
        // 更新 server 的 Authorization header 与 auth，并持久化新 token。失败则用旧
        // token 继续连接（记录错误，不 panic）。StreamableHttpMcpClient 不变（仍只发 header）。
        let refreshed = self.refresh_oauth_if_needed(sink, server).await;
        let server = refreshed.as_ref().unwrap_or(server);
        let fingerprint = config_fingerprint(server);

        // 单飞门闩：在持外层池锁时完成「命中已有会话」或「插入 Connecting 占位」二选一，
        // 让并发的第二个调用者一定观察到第一个的占位 Arc（而非各插各的）。
        // 仅当 pool.get 返回 None（或配置已变需重建）时才新建占位并立即插入。
        enum Resolved {
            // 命中已有会话（Connected 或正在 Connecting）：共享其 Arc，锁会话后再判定。
            Existing(Arc<Mutex<McpSession>>),
            // 新建了占位：本调用者负责握手。
            Fresh(Arc<Mutex<McpSession>>),
        }

        let resolved = {
            let mut pool = self.mcp_sessions.lock().await;
            match pool.get(&server.id) {
                Some(existing) => Resolved::Existing(existing.clone()),
                None => {
                    let session =
                        Arc::new(Mutex::new(McpSession::placeholder(fingerprint.clone())));
                    pool.insert(server.id.clone(), session.clone());
                    Resolved::Fresh(session)
                }
            }
        };

        match resolved {
            Resolved::Existing(session) => {
                // 锁会话后重判：可能已被先到的调用者握手成功；或配置已变需重建。
                let mut guard = session.lock().await;
                if guard.config_fingerprint == fingerprint
                    && matches!(guard.state, McpServerState::Connected)
                {
                    drop(guard);
                    return Ok(session);
                }
                if guard.config_fingerprint != fingerprint {
                    // A schema snapshot belongs to the exact server config that produced it.
                    guard.config_fingerprint = fingerprint.clone();
                    guard.tools.clear();
                    guard.tools_revision = 0;
                    guard.consecutive_connect_failures = 0;
                    guard.discovery_retry_after = None;
                    let old_transport = guard.take_transport();
                    guard.state = McpServerState::Connecting;
                    close_transport(old_transport).await;
                } else if respect_discovery_backoff {
                    if let (McpServerState::Error { message }, Some(retry_after)) =
                        (&guard.state, guard.discovery_retry_after)
                    {
                        let now = Instant::now();
                        if retry_after > now {
                            return Err(format!(
                                "{message} (MCP reconnect is cooling down for {} ms)",
                                retry_after.duration_since(now).as_millis()
                            ));
                        }
                    }
                }
                // Connecting/Error sessions retry here; discovery callers honor the cooldown above.
                self.connect_session(sink, server, &mut guard).await?;
                drop(guard);
                Ok(session)
            }
            Resolved::Fresh(session) => {
                sink.emit_server_state(server, &McpServerState::Connecting);
                let mut guard = session.lock().await;
                self.connect_session(sink, server, &mut guard).await?;
                drop(guard);
                Ok(session)
            }
        }
    }

    /// OAuth token 刷新钩子：若该 server 是 OAuth 连接器且 token 临近/已过期，用
    /// refresh_token 换新 token，返回更新后的 server（headers + auth）。无需刷新或刷新
    /// 失败则返回 None（调用方用原 server 继续）。成功刷新时把新 token 持久化回 settings。
    async fn refresh_oauth_if_needed(
        &self,
        sink: &impl McpEventSink,
        server: &ChatMcpServer,
    ) -> Option<ChatMcpServer> {
        let auth = server.auth.as_ref()?;
        let now = now_unix();
        if !crate::connectors::oauth::needs_refresh(
            auth,
            now,
            crate::connectors::oauth::REFRESH_LEEWAY_SECS,
        ) {
            return None;
        }
        let token_endpoint = auth.token_endpoint.clone()?;
        let refresh_token = auth.refresh_token.clone()?;
        let client_id = auth.client_id.clone();

        match crate::connectors::oauth::refresh_access_token(
            &self.http,
            &token_endpoint,
            &refresh_token,
            client_id.as_deref(),
        )
        .await
        {
            Ok(token) => {
                let mut updated = server.clone();
                let mut new_auth = updated.auth.take().unwrap_or_default();
                new_auth.access_token = token.access_token.clone();
                // 多数实现 refresh 不回新的 refresh_token；只在确实下发时替换（轮换场景）。
                if let Some(rt) = token.refresh_token {
                    new_auth.refresh_token = Some(rt);
                }
                new_auth.expires_at =
                    crate::connectors::oauth::compute_expires_at(now, token.expires_in);
                updated.auth = Some(new_auth);
                updated.headers.insert(
                    "Authorization".to_string(),
                    format!("Bearer {}", token.access_token),
                );
                // 持久化新 token：仅在拿得到 AppHandle 时（生产路径）。
                if let Some(app) = sink.app_handle() {
                    self.persist_refreshed_server(app, &updated);
                }
                Some(updated)
            }
            Err(err) => {
                eprintln!(
                    "OAuth token refresh failed for connector {}: {err}; using existing token",
                    server.name
                );
                None
            }
        }
    }

    /// 把刷新后的 server 写回 settings（内存 + 落盘）。在 settings.chat_tools.servers 里
    /// 按 id 定位并替换。失败仅记录，不影响本次连接。
    fn persist_refreshed_server(&self, app: &AppHandle, server: &ChatMcpServer) {
        let updated_settings = {
            let mut guard = self.settings_write();
            let found = guard
                .chat_tools
                .servers
                .iter_mut()
                .find(|existing| existing.id == server.id);
            match found {
                Some(slot) => {
                    slot.headers = server.headers.clone();
                    slot.auth = server.auth.clone();
                    guard.clone()
                }
                None => return,
            }
        };
        if let Err(err) = crate::settings::persist_settings(app, &updated_settings) {
            eprintln!("Failed to persist refreshed OAuth token: {err}");
        }
    }

    /// 在持有会话锁的前提下完成一次握手（不持外层池锁）。失败时写 Error 状态并返回错误。
    async fn connect_session(
        &self,
        sink: &impl McpEventSink,
        server: &ChatMcpServer,
        guard: &mut McpSession,
    ) -> Result<(), String> {
        match self.mcp_connect_into(server, guard).await {
            Ok(()) => {
                guard.state = McpServerState::Connected;
                guard.handshake_count = guard.handshake_count.saturating_add(1);
                guard.consecutive_connect_failures = 0;
                guard.discovery_retry_after = None;
                guard.last_used = Instant::now();
                self.remember_mcp_tools(server, &guard.tools);
                sink.emit_server_state(server, &McpServerState::Connected);
                Ok(())
            }
            Err(err) => {
                let stderr_tail = guard.stderr_tail_text().await;
                let message = if stderr_tail.trim().is_empty() {
                    err
                } else {
                    format!("{err}\n{stderr_tail}")
                };
                guard.state = McpServerState::Error {
                    message: message.clone(),
                };
                guard.consecutive_connect_failures =
                    guard.consecutive_connect_failures.saturating_add(1);
                guard.discovery_retry_after = Some(
                    Instant::now() + discovery_retry_backoff(guard.consecutive_connect_failures),
                );
                guard.take_transport();
                sink.emit_server_state(
                    server,
                    &McpServerState::Error {
                        message: message.clone(),
                    },
                );
                Err(message)
            }
        }
    }

    /// 建立 transport 并完成握手，把元数据写入会话（不改 state）。
    /// stdio 与 HTTP 走同一条路：`conn::connect` 出 `RunningService`，然后列一次工具。
    async fn mcp_connect_into(
        &self,
        server: &ChatMcpServer,
        session: &mut McpSession,
    ) -> Result<(), String> {
        // 换连接前先摘掉旧的（含 abort 旧 stderr 任务），旧 Arc 落地即取消服务循环。
        let old = session.take_transport();
        drop(old);

        let established = conn::connect(server, &self.http).await?;
        if let Some(stderr) = established.stderr {
            session.stderr_task = Some(spawn_stderr_tail(stderr, session.stderr_tail.clone()));
        }

        // 先读版本号再列工具：列表期间来的 list_changed 会让 revision 前进，
        // 下一次 mcp_list_tools 就会重列，不会把变更吃掉。
        let revision_before = established
            .tools_revision
            .load(std::sync::atomic::Ordering::Acquire);
        session.tools = conn::list_tools(&established.service, conn::LIST_TOOLS_TIMEOUT).await?;
        session.tools_revision = revision_before;
        session.revision_source = Some(established.tools_revision);
        session.transport = Some(established.service);
        Ok(())
    }

    /// 调用某个 MCP server 的工具：get-or-connect → 锁会话判活 → 释放锁再发请求；
    /// 连接确实死了才重连一次重试。
    ///
    /// HTTP 的 404 session 过期由 rmcp 内部单次 best-effort 重握手处理
    /// （`reinit_on_expired_session`，默认开，且只对 `SessionExpired` 生效 ——
    /// 500 不会触发重连），所以这里不再有 stdio / HTTP 两条分支。
    pub async fn mcp_call_tool(
        &self,
        sink: &impl McpEventSink,
        server: &ChatMcpServer,
        name: &str,
        arguments: Value,
    ) -> Result<McpToolCallResult, String> {
        let session = self.mcp_get_or_connect(sink, server).await?;
        let timeout_dur = self.mcp_tool_timeout();

        // 会话锁只保护生命周期状态迁移：拿到 Arc 后必须先释放锁再 await 响应。
        // rmcp 按 JSON-RPC id 路由响应，所以一次丢响应不会阻塞后面对同一个健康
        // 服务器的请求。
        let service = {
            let mut guard = session.lock().await;
            let dead = guard
                .transport
                .as_ref()
                .map(|service| service.is_closed())
                .unwrap_or(true);
            if dead {
                guard.take_transport();
                guard.state = McpServerState::Connecting;
                sink.emit_server_state(server, &McpServerState::Connecting);
                self.connect_session(sink, server, &mut guard).await?;
            }
            // 请求开始时也刷一次 last_used，否则空闲回收器会把一个正在跑的长请求
            // 误判成闲置会话。
            guard.last_used = Instant::now();
            guard.transport.clone()
        };
        let Some(service) = service else {
            return Err("MCP transport unavailable".to_string());
        };

        let first = conn::call_tool(&service, name, arguments.clone(), timeout_dur).await;
        let result = match first {
            Ok(value) => Ok(result::parse_tool_result(
                serde_json::to_value(&value).map_err(|err| err.to_string())?,
            )),
            Err(err) => {
                // 超时的执行结果是未知的。绝不因此杀掉健康的服务器，也绝不静默重放
                // 一个可能非幂等的工具调用。只有传输真的死了才走重连重试。
                if !conn::connection_is_gone(&service, &err) {
                    Err(err)
                } else {
                    let retry_service = {
                        let mut guard = session.lock().await;
                        let must_reconnect = guard
                            .transport
                            .as_ref()
                            .map(|current| Arc::ptr_eq(current, &service))
                            .unwrap_or(true);
                        if must_reconnect {
                            guard.take_transport();
                            guard.state = McpServerState::Connecting;
                            sink.emit_server_state(server, &McpServerState::Connecting);
                            self.connect_session(sink, server, &mut guard).await?;
                        }
                        guard.transport.clone()
                    };
                    match retry_service {
                        Some(retry_service) => {
                            let value = conn::call_tool(
                                &retry_service,
                                name,
                                arguments,
                                self.mcp_tool_timeout(),
                            )
                            .await?;
                            Ok(result::parse_tool_result(
                                serde_json::to_value(&value).map_err(|err| err.to_string())?,
                            ))
                        }
                        None => Err(err),
                    }
                }
            }
        };
        if result.is_ok() {
            session.lock().await.last_used = Instant::now();
        }
        result
    }

    /// 返回某个 server 的工具列表（持久会话）。供 list_enabled_tool_catalog 复用。
    ///
    /// 只在服务器发过 `tools/list_changed` 时重列。手写版这条只对 stdio 生效，
    /// 现在 HTTP 也一样 —— rmcp 的 `ClientHandler` 两种传输都收得到通知。
    pub async fn mcp_list_tools(
        &self,
        sink: &impl McpEventSink,
        server: &ChatMcpServer,
    ) -> Result<Vec<McpTool>, String> {
        let session = self.mcp_get_or_connect_inner(sink, server, true).await?;
        let mut guard = session.lock().await;
        let revision = guard.live_tools_revision();
        if revision != guard.tools_revision {
            if let Some(service) = guard.transport.clone() {
                let tools = conn::list_tools(&service, conn::LIST_TOOLS_TIMEOUT).await?;
                guard.tools = tools;
                guard.tools_revision = revision;
                self.remember_mcp_tools(server, &guard.tools);
            }
        }
        guard.last_used = Instant::now();
        Ok(guard.tools.clone())
    }

    /// Last successful tool schema, independent from the transport/session lifetime.
    /// A snapshot is reusable only for the exact config fingerprint that produced it.
    pub async fn mcp_cached_tools(&self, server: &ChatMcpServer) -> Option<Vec<McpTool>> {
        self.get_mcp_tool_snapshot(&server.id, &config_fingerprint(server))
    }

    fn remember_mcp_tools(&self, server: &ChatMcpServer, tools: &[McpTool]) {
        self.set_mcp_tool_snapshot(
            server.id.clone(),
            config_fingerprint(server),
            tools.to_vec(),
        );
    }

    /// 无法降级进列表，只能在系统提示词里声明「已配置但连接失败」。
    pub async fn mcp_unreachable_server_ids(&self) -> Vec<String> {
        let candidates: Vec<(String, Arc<Mutex<McpSession>>)> = {
            let pool = self.mcp_sessions.lock().await;
            pool.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        let mut out = Vec::new();
        for (id, session) in candidates {
            let guard = session.lock().await;
            if matches!(guard.state, McpServerState::Error { .. })
                && self
                    .get_mcp_tool_snapshot(&id, &guard.config_fingerprint)
                    .is_none()
            {
                out.push(id);
            }
        }
        out
    }

    /// 读取某个 server 的状态快照（给状态命令）。无会话 ⇒ Disconnected。
    pub async fn mcp_server_state(&self, server_id: &str) -> McpServerStatusSnapshot {
        let session = {
            let pool = self.mcp_sessions.lock().await;
            pool.get(server_id).cloned()
        };
        match session {
            Some(session) => {
                let guard = session.lock().await;
                McpServerStatusSnapshot {
                    server_id: server_id.to_string(),
                    state: guard.state.clone(),
                    handshake_count: guard.handshake_count,
                    stderr_tail: guard.stderr_tail_text().await,
                }
            }
            None => McpServerStatusSnapshot {
                server_id: server_id.to_string(),
                state: McpServerState::Disconnected,
                handshake_count: 0,
                stderr_tail: String::new(),
            },
        }
    }

    /// 主动丢弃某个 server 的会话（重连按钮用），下次调用透明重连。
    pub async fn mcp_reload_server(&self, sink: &impl McpEventSink, server_id: &str) {
        let removed = {
            let mut pool = self.mcp_sessions.lock().await;
            pool.remove(server_id)
        };
        if let Some(session) = removed {
            let transport = {
                let mut guard = session.lock().await;
                guard.state = McpServerState::Disconnected;
                guard.take_transport()
            };
            close_transport(transport).await;
        }
        sink.emit_disconnected(server_id);
    }

    /// 空闲回收：移除 last_used 超过 idle_timeout 的会话。锁内只收集+移除，无 await。
    /// 返回被回收的 server_id（供发 Disconnected 事件）。
    pub async fn mcp_reap_idle(
        &self,
        idle_timeout: Duration,
    ) -> Vec<(String, Arc<Mutex<McpSession>>)> {
        let now = Instant::now();
        let mut evicted = Vec::new();
        // 先并发拿每个会话的 last_used 需要锁会话；为避免锁内 await，这里改为：
        // 收集所有 (id, Arc)，释放池锁后逐个判断。
        let candidates: Vec<(String, Arc<Mutex<McpSession>>)> = {
            let pool = self.mcp_sessions.lock().await;
            pool.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        let mut expired_ids = Vec::new();
        for (id, session) in &candidates {
            let guard = session.lock().await;
            if now.duration_since(guard.last_used) > idle_timeout {
                expired_ids.push(id.clone());
            }
        }
        if expired_ids.is_empty() {
            return evicted;
        }
        {
            let mut pool = self.mcp_sessions.lock().await;
            for id in &expired_ids {
                if let Some(session) = pool.remove(id) {
                    evicted.push((id.clone(), session));
                }
            }
        }
        for (_, session) in &evicted {
            let transport = {
                let mut guard = session.lock().await;
                guard.state = McpServerState::Disconnected;
                guard.take_transport()
            };
            close_transport(transport).await;
        }
        evicted
    }

    /// 排干连接池：每个会话 Drop transport 触发 abort task + kill 子进程。退出钩子用。
    pub async fn mcp_disconnect_all(&self) {
        let drained: Vec<(String, Arc<Mutex<McpSession>>)> = {
            let mut pool = self.mcp_sessions.lock().await;
            pool.drain().collect()
        };
        for (_, session) in drained {
            let transport = {
                let mut guard = session.lock().await;
                guard.state = McpServerState::Disconnected;
                guard.take_transport()
            };
            close_transport(transport).await;
        }
    }

    /// 断开并移除单个 server 的持久会话（插件关闭 / 卸载时用）。
    pub async fn mcp_disconnect_server(&self, server_id: &str) {
        let session = {
            let mut pool = self.mcp_sessions.lock().await;
            pool.remove(server_id)
        };
        if let Some(session) = session {
            let transport = {
                let mut guard = session.lock().await;
                guard.state = McpServerState::Disconnected;
                guard.take_transport()
            };
            close_transport(transport).await;
        }
    }
}

/// 关掉一条 MCP 连接。
///
/// 取消 rmcp 的服务循环即触发 `transport.close()`：stdio 走 `graceful_shutdown`
/// （关管道 → 等 3s → kill），HTTP 发 `DELETE` 释放 session。能独占 Arc 时带超时
/// 等它跑完，这样退出钩子能真正收掉子进程而不是留孤儿；还有别人持着 Arc（在途
/// 工具调用）就只发取消信号，等对方 drop 时收尾。
async fn close_transport(transport: Option<Arc<McpService>>) {
    let Some(transport) = transport else {
        return;
    };
    transport.cancellation_token().cancel();
    if let Some(mut owned) = Arc::into_inner(transport) {
        let _ = owned.close_with_timeout(TRANSPORT_CLOSE_TIMEOUT).await;
    }
}


#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatusSnapshot {
    pub server_id: String,
    pub state: McpServerState,
    pub handshake_count: u64,
    pub stderr_tail: String,
}

fn discovery_retry_backoff(consecutive_failures: u32) -> Duration {
    let shift = consecutive_failures.saturating_sub(1).min(5);
    let multiplier = 1u32 << shift;
    MCP_DISCOVERY_RETRY_BASE
        .saturating_mul(multiplier)
        .min(MCP_DISCOVERY_RETRY_MAX)
}

/// ChatMcpServer configuration fingerprint; a changed config rebuilds the session.
pub fn config_fingerprint(server: &ChatMcpServer) -> String {
    let serialized = serde_json::to_vec(server)
        .unwrap_or_else(|_| format!("{}:{}", server.id, server.command).into_bytes());
    format!("{:x}", Sha256::digest(serialized))
}

/// 当前 unix 时间戳（秒），用于 OAuth token 过期判断。
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_app_state;

    #[test]
    fn config_fingerprint_changes_with_config() {
        let mut server = stdio_server("echo", &[]);
        let a = config_fingerprint(&server);
        server.args = vec!["--flag".to_string()];
        let b = config_fingerprint(&server);
        assert_ne!(a, b);
        // Stable for the same config, and safe to persist without raw credentials.
        assert_eq!(b, config_fingerprint(&server));
        server
            .headers
            .insert("Authorization".to_string(), "Bearer top-secret".to_string());
        let credential_fingerprint = config_fingerprint(&server);
        assert_eq!(credential_fingerprint.len(), 64);
        assert!(!credential_fingerprint.contains("top-secret"));
    }

    #[test]
    fn rmcp_reinitializes_only_on_expired_session() {
        // 手写版靠 `is_session_expired()` 匹配错误串来决定 404 重连；现在这件事归
        // rmcp 的 `SessionExpired` + `reinit_on_expired_session` 管。它默认必须是开的，
        // 否则远程 MCP 的 session 过期后就再也连不回来了 —— 这是我们依赖的默认值，
        // 钉在这里，rmcp 哪天改默认会让测试红。
        // 「404 重连、500 不重连」的端到端行为由下面 http_reconnect_only_on_404 /
        // http_500_does_not_reconnect 两个假服务器测试覆盖。
        let config = rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri("http://localhost/mcp");
        assert!(config.reinit_on_expired_session);
    }

    fn stdio_server(command: &str, args: &[&str]) -> ChatMcpServer {
        ChatMcpServer {
            id: "test-stdio".to_string(),
            name: "Test Stdio".to_string(),
            enabled: true,
            transport: "stdio".to_string(),
            url: String::new(),
            command: command.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            env: std::collections::HashMap::new(),
            headers: std::collections::HashMap::new(),
            cwd: None,
            enabled_tools: Vec::new(),
            connector_id: None,
            auth: None,
        }
    }

    fn http_server(url: String) -> ChatMcpServer {
        ChatMcpServer {
            id: "test-http".to_string(),
            name: "Test HTTP".to_string(),
            enabled: true,
            transport: "streamable_http".to_string(),
            url,
            command: String::new(),
            args: Vec::new(),
            env: std::collections::HashMap::new(),
            headers: std::collections::HashMap::new(),
            cwd: None,
            enabled_tools: Vec::new(),
            connector_id: None,
            auth: None,
        }
    }

    /// fake HTTP MCP server：第一次 tools/call 返回 `first_call_status`（>=400 视为失败），
    /// 之后 tools/call 正常回 echo。initialize/tools/list 始终正常。
    async fn spawn_test_http_mcp_server(first_call_status: u16) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let call_count = Arc::new(AtomicU64::new(0));
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let call_count = call_count.clone();
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
                        let message: Value = serde_json::from_str(body).expect("json");
                        let method = message
                            .get("method")
                            .and_then(|m| m.as_str())
                            .unwrap_or_default();
                        let id = message.get("id").cloned().unwrap_or(Value::Null);

                        // tools/call 第一次按配置返回错误状态。
                        if method == "tools/call" {
                            let nth = call_count.fetch_add(1, Ordering::SeqCst) + 1;
                            if nth == 1 && first_call_status >= 400 {
                                let raw = format!(
                                    "HTTP/1.1 {} ERR\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                                    first_call_status
                                );
                                let _ = stream.write_all(raw.as_bytes()).await;
                                let _ = stream.shutdown().await;
                                return;
                            }
                        }

                        let response = match method {
                            "initialize" => serde_json::json!({
                                "jsonrpc":"2.0","id":id,
                                "result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"1.0.0"}}
                            }),
                            "tools/list" => serde_json::json!({
                                "jsonrpc":"2.0","id":id,
                                "result":{"tools":[{"name":"echo","description":"Echo","inputSchema":{"type":"object"}}]}
                            }),
                            "tools/call" => serde_json::json!({
                                "jsonrpc":"2.0","id":id,
                                "result":{"content":[{"type":"text","text":"echo: ok"}]}
                            }),
                            _ => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{}}),
                        };
                        let payload = if message.get("id").is_some() {
                            response.to_string()
                        } else {
                            String::new()
                        };
                        let raw = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: sess-1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            payload.len(),
                            payload,
                        );
                        let _ = stream.write_all(raw.as_bytes()).await;
                        let _ = stream.shutdown().await;
                        return;
                    }
                });
            }
        });
        format!("http://{addr}/mcp")
    }

    #[tokio::test]
    async fn http_reconnect_only_on_404() {
        // 404 → 清 session 重 initialize 重试 → 成功。
        let url = spawn_test_http_mcp_server(404).await;
        let state = test_app_state();
        let server = http_server(url);
        let result = state
            .mcp_call_tool(&(), &server, "echo", serde_json::json!({}))
            .await
            .expect("404 should transparently reconnect and retry");
        assert_eq!(result.content, "echo: ok");
        state.mcp_disconnect_all().await;
    }

    #[tokio::test]
    async fn http_500_does_not_reconnect() {
        // 500 不是 session 过期 → 透传错误，不重试。
        let url = spawn_test_http_mcp_server(500).await;
        let state = test_app_state();
        let server = http_server(url);
        let err = state
            .mcp_call_tool(&(), &server, "echo", serde_json::json!({}))
            .await
            .expect_err("500 should surface as error");
        assert!(err.contains("500"), "got: {err}");
        state.mcp_disconnect_all().await;
    }

    #[tokio::test]
    async fn connect_once_lists_tools_without_touching_the_pool() {
        // 设置页的「测试连接」测的是还没保存的草稿配置，绝不能进连接池 ——
        // 否则一个用户还在编辑的半成品配置会被当成常驻服务器缓存下来。
        let url = spawn_test_http_mcp_server(200).await;
        let state = test_app_state();
        let server = http_server(url);

        let tools = conn::list_tools_once(&server, &state.http)
            .await
            .expect("one-shot list should work");
        assert_eq!(
            tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["echo"]
        );
        assert!(
            state.mcp_sessions.lock().await.is_empty(),
            "一次性连接不得进连接池"
        );
    }

    #[tokio::test]
    async fn call_tool_once_works_and_stays_out_of_the_pool() {
        // web_search 的 exa server 是临时合成的（api key 在 URL 里），同样不该进池。
        let url = spawn_test_http_mcp_server(200).await;
        let state = test_app_state();
        let server = http_server(url);

        let raw = conn::call_tool_once(
            &server,
            &state.http,
            "echo",
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await
        .expect("one-shot call should work");
        let parsed =
            result::parse_tool_result(serde_json::to_value(&raw).expect("serialize"));
        assert_eq!(parsed.content, "echo: ok");
        assert!(state.mcp_sessions.lock().await.is_empty());
    }

    /// fake HTTP MCP server：始终 200，计数收到的 initialize 次数（用于断言会话复用）。
    /// 返回 (url, initialize_count)。
    async fn spawn_counting_http_mcp_server() -> (
        String,
        std::sync::Arc<std::sync::atomic::AtomicU64>,
        std::sync::Arc<std::sync::atomic::AtomicU64>,
        std::sync::Arc<std::sync::atomic::AtomicU64>,
    ) {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let init_count = Arc::new(AtomicU64::new(0));
        let negotiated_header_count = Arc::new(AtomicU64::new(0));
        let delete_count = Arc::new(AtomicU64::new(0));
        let init_count_server = init_count.clone();
        let negotiated_header_count_server = negotiated_header_count.clone();
        let delete_count_server = delete_count.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let init_count = init_count_server.clone();
                let negotiated_header_count = negotiated_header_count_server.clone();
                let delete_count = delete_count_server.clone();
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
                                line.split_once(':').and_then(|(name, value)| {
                                    name.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().ok())
                                        .flatten()
                                })
                            })
                            .unwrap_or(0);
                        if read < header_end + 4 + content_length {
                            continue;
                        }
                        let http_method = request
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().next())
                            .unwrap_or_default();
                        let header_value = |name: &str| {
                            request.lines().find_map(|line| {
                                line.split_once(':').and_then(|(header_name, value)| {
                                    header_name
                                        .eq_ignore_ascii_case(name)
                                        .then(|| value.trim().to_string())
                                })
                            })
                        };
                        let negotiated_headers_ok = header_value("mcp-protocol-version").as_deref()
                            == Some("2025-03-26")
                            && header_value("mcp-session-id").as_deref() == Some("sess-1");

                        if http_method == "DELETE" {
                            if negotiated_headers_ok {
                                delete_count.fetch_add(1, Ordering::SeqCst);
                            }
                            let raw =
                                "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                            let _ = stream.write_all(raw.as_bytes()).await;
                            let _ = stream.shutdown().await;
                            return;
                        }

                        let body = &request[header_end + 4..header_end + 4 + content_length];
                        let message: Value = serde_json::from_str(body).expect("json");
                        let method = message
                            .get("method")
                            .and_then(|m| m.as_str())
                            .unwrap_or_default();
                        let id = message.get("id").cloned().unwrap_or(Value::Null);

                        if method == "initialize" {
                            init_count.fetch_add(1, Ordering::SeqCst);
                        } else if negotiated_headers_ok {
                            negotiated_header_count.fetch_add(1, Ordering::SeqCst);
                        }

                        let response = match method {
                            "initialize" => serde_json::json!({
                                "jsonrpc":"2.0","id":id,
                                "result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"1.0.0"}}
                            }),
                            "tools/list" => serde_json::json!({
                                "jsonrpc":"2.0","id":id,
                                "result":{"tools":[{"name":"echo","description":"Echo","inputSchema":{"type":"object"}}]}
                            }),
                            "tools/call" => serde_json::json!({
                                "jsonrpc":"2.0","id":id,
                                "result":{"content":[{"type":"text","text":"echo: ok"}]}
                            }),
                            _ => serde_json::json!({"jsonrpc":"2.0","id":id,"result":{}}),
                        };
                        let payload = if message.get("id").is_some() {
                            response.to_string()
                        } else {
                            String::new()
                        };
                        let raw = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: sess-1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            payload.len(),
                            payload,
                        );
                        let _ = stream.write_all(raw.as_bytes()).await;
                        let _ = stream.shutdown().await;
                        return;
                    }
                });
            }
        });
        (
            format!("http://{addr}/mcp"),
            init_count,
            negotiated_header_count,
            delete_count,
        )
    }

    #[tokio::test]
    async fn http_reuses_negotiated_version_and_deletes_session() {
        use std::sync::atomic::Ordering;
        let (url, init_count, negotiated_header_count, delete_count) =
            spawn_counting_http_mcp_server().await;
        let state = test_app_state();
        let server = http_server(url);

        let first = state
            .mcp_call_tool(&(), &server, "echo", serde_json::json!({}))
            .await
            .expect("first call ok");
        assert_eq!(first.content, "echo: ok");
        let second = state
            .mcp_call_tool(&(), &server, "echo", serde_json::json!({}))
            .await
            .expect("second call ok");
        assert_eq!(second.content, "echo: ok");

        assert_eq!(
            init_count.load(Ordering::SeqCst),
            1,
            "two successful HTTP calls must share a single initialize"
        );
        assert!(
            negotiated_header_count.load(Ordering::SeqCst) >= 3,
            "initialized and later requests must use the negotiated protocol version and session id"
        );

        state.mcp_disconnect_all().await;
        assert_eq!(
            delete_count.load(Ordering::SeqCst),
            1,
            "disconnect must DELETE the negotiated HTTP session"
        );
    }

    #[cfg(unix)]
    mod stdio {
        use super::*;
        use std::io::Write;

        /// 写一个 fake stdio MCP server python 脚本到临时文件，返回脚本路径。
        /// 协议：逐行读 JSON-RPC；initialize/tools/list/tools/call 各自回包；
        /// 无 id 的通知忽略。若设置 `KIVIO_DIE_AFTER_CALL=N`，在第 N 次 tools/call 回包后退出，
        /// 用于模拟子进程死亡 → 透明重连。
        /// `KIVIO_DELAY_CALL_MS=N`：响应 tools/call 前先 sleep N 毫秒（模拟慢但健康的工具）。
        /// `KIVIO_CALL_MARKER=path`：每次执行 tools/call 时往该文件追加一行（统计实际执行次数）。
        fn write_fake_server() -> std::path::PathBuf {
            let script = r#"#!/usr/bin/env python3
import sys, json, os, time
die_after = int(os.environ.get("KIVIO_DIE_AFTER_CALL", "0"))
delay_ms = int(os.environ.get("KIVIO_DELAY_CALL_MS", "0"))
marker = os.environ.get("KIVIO_CALL_MARKER", "")
calls = 0
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
        # notification, ignore
        continue
    if method == "initialize":
        resp = {"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"1.0.0"}}}
    elif method == "tools/list":
        resp = {"jsonrpc":"2.0","id":mid,"result":{"tools":[{"name":"echo","description":"Echo","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}}
    elif method == "tools/call":
        calls += 1
        if marker:
            with open(marker, "a") as f:
                f.write("call\n")
        text = ""
        try:
            text = msg["params"]["arguments"].get("text","")
        except Exception:
            text = ""
        if text == "hang":
            # Simulate a buggy MCP server that loses one response but remains healthy.
            continue
        if delay_ms:
            time.sleep(delay_ms / 1000.0)
        resp = {"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":"echo: "+str(text)}]}}
        sys.stdout.write(json.dumps(resp)+"\n")
        sys.stdout.flush()
        if die_after and calls >= die_after:
            sys.exit(0)
        continue
    else:
        resp = {"jsonrpc":"2.0","id":mid,"result":{}}
    sys.stdout.write(json.dumps(resp)+"\n")
    sys.stdout.flush()
"#;
            let mut path = std::env::temp_dir();
            path.push(format!("kivio-fake-mcp-{}.py", uuid::Uuid::new_v4()));
            let mut file = std::fs::File::create(&path).expect("create fake server");
            file.write_all(script.as_bytes())
                .expect("write fake server");
            path
        }

        fn python_server(script: &std::path::Path) -> ChatMcpServer {
            super::stdio_server("python3", &["-u", script.to_str().unwrap()])
        }

        #[tokio::test]
        async fn ten_calls_one_handshake() {
            let script = write_fake_server();
            let state = test_app_state();
            let server = python_server(&script);

            for i in 0..10 {
                let result = state
                    .mcp_call_tool(&(), &server, "echo", serde_json::json!({ "text": i }))
                    .await
                    .expect("call should succeed");
                assert_eq!(result.content, format!("echo: {i}"));
            }

            let handshake_count = {
                let pool = state.mcp_sessions.lock().await;
                let session = pool.get(&server.id).expect("session present").clone();
                drop(pool);
                let guard = session.lock().await;
                guard.handshake_count
            };
            assert_eq!(handshake_count, 1, "10 calls must share 1 handshake");
            state.mcp_disconnect_all().await;
            let _ = std::fs::remove_file(&script);
        }

        #[tokio::test]
        async fn timeout_on_healthy_child_does_not_kill_or_reexecute() {
            // FIX 1: 一个慢但健康的工具超过 tool_timeout_ms ⇒ request 返回 "read timed out"。
            // 必须把错误透传，绝不杀健康子进程、不重连、不重发同一个 tools/call
            // （否则非幂等工具会被静默重复执行）。
            let script = write_fake_server();
            let state = test_app_state();
            // 注入最小工具超时（1s，受 .max(1000) 约束）；server 延迟 2.5s 远超之。
            state.settings_write().chat_tools.tool_timeout_ms = 1_000;

            let mut marker = std::env::temp_dir();
            marker.push(format!(
                "kivio-fake-mcp-marker-{}.txt",
                uuid::Uuid::new_v4()
            ));

            let mut server = python_server(&script);
            server
                .env
                .insert("KIVIO_DELAY_CALL_MS".to_string(), "2500".to_string());
            server.env.insert(
                "KIVIO_CALL_MARKER".to_string(),
                marker.to_string_lossy().into_owned(),
            );

            let err = state
                .mcp_call_tool(&(), &server, "echo", serde_json::json!({ "text": "slow" }))
                .await
                .expect_err("slow healthy tool should surface a timeout error");
            assert!(
                err.contains("timed out"),
                "expected a timeout error, got: {err}"
            );

            // 握手仍为 1（未重连），子进程仍存活。
            let (handshake_count, pid) = {
                let pool = state.mcp_sessions.lock().await;
                let session = pool.get(&server.id).expect("session present").clone();
                drop(pool);
                let mut guard = session.lock().await;
                let alive = match &mut guard.transport {
                    McpTransport::Stdio(conn) => !conn.is_dead() && conn.child_id().is_some(),
                    _ => false,
                };
                assert!(alive, "healthy child must not be killed by a timeout");
                let pid = match &guard.transport {
                    McpTransport::Stdio(conn) => conn.child_id(),
                    _ => None,
                };
                (guard.handshake_count, pid)
            };
            assert_eq!(
                handshake_count, 1,
                "timeout must not trigger a reconnect/re-handshake"
            );
            assert!(pid.is_some(), "child pid should still be present");

            // 给延迟的 tools/call 充足时间真正执行完一次（验证只执行一次，没被重发）。
            tokio::time::sleep(Duration::from_millis(3_000)).await;
            let marker_lines = std::fs::read_to_string(&marker).unwrap_or_default();
            let executed = marker_lines.lines().filter(|l| *l == "call").count();
            assert_eq!(
                executed, 1,
                "the tool body must run exactly once (no silent re-execution)"
            );

            state.mcp_disconnect_all().await;
            let _ = std::fs::remove_file(&script);
            let _ = std::fs::remove_file(&marker);
        }

        #[tokio::test]
        async fn concurrent_get_or_connect_share_one_handshake() {
            // FIX 2: 同一 server_id 的两个并发 get_or_connect（无已连接会话）必须收敛到
            // 单飞门闩，只做一次握手、只有一个池条目。
            let script = write_fake_server();
            let state = std::sync::Arc::new(test_app_state());
            let server = python_server(&script);

            let s1 = state.clone();
            let srv1 = server.clone();
            let s2 = state.clone();
            let srv2 = server.clone();
            let h1 =
                tokio::spawn(async move { s1.mcp_get_or_connect(&(), &srv1).await.map(|_| ()) });
            let h2 =
                tokio::spawn(async move { s2.mcp_get_or_connect(&(), &srv2).await.map(|_| ()) });
            let (r1, r2) = tokio::join!(h1, h2);
            r1.unwrap().expect("connect one ok");
            r2.unwrap().expect("connect two ok");

            let (entries, handshake_count) = {
                let pool = state.mcp_sessions.lock().await;
                let entries = pool.len();
                let session = pool.get(&server.id).expect("session present").clone();
                drop(pool);
                let guard = session.lock().await;
                (entries, guard.handshake_count)
            };
            assert_eq!(entries, 1, "exactly one pool entry for the server");
            assert_eq!(
                handshake_count, 1,
                "two concurrent connects must share a single handshake"
            );

            state.mcp_disconnect_all().await;
            let _ = std::fs::remove_file(&script);
        }

        #[tokio::test]
        async fn liveness_reconnect_on_dead_child() {
            let script = write_fake_server();
            let state = test_app_state();
            let mut server = python_server(&script);
            // server 在第 1 次 tools/call 后退出 → 第 2 次调用探活发现死连接 → 透明重连。
            server
                .env
                .insert("KIVIO_DIE_AFTER_CALL".to_string(), "1".to_string());

            let first = state
                .mcp_call_tool(&(), &server, "echo", serde_json::json!({ "text": "a" }))
                .await
                .expect("first call ok");
            assert_eq!(first.content, "echo: a");

            // 给子进程一点时间真正退出。
            tokio::time::sleep(Duration::from_millis(200)).await;

            let second = state
                .mcp_call_tool(&(), &server, "echo", serde_json::json!({ "text": "b" }))
                .await
                .expect("second call should transparently reconnect");
            assert_eq!(second.content, "echo: b");

            let handshake_count = {
                let pool = state.mcp_sessions.lock().await;
                let session = pool.get(&server.id).expect("session present").clone();
                drop(pool);
                let guard = session.lock().await;
                guard.handshake_count
            };
            assert_eq!(
                handshake_count, 2,
                "dead child should trigger exactly one reconnect"
            );
            state.mcp_disconnect_all().await;
            let _ = std::fs::remove_file(&script);
        }

        #[tokio::test]
        async fn lost_response_does_not_head_of_line_block_later_request() {
            // The fake server deliberately drops the response for "hang" but keeps reading stdin.
            // A later request on the same stdio session must still reach the server and complete.
            let script = write_fake_server();
            let state = std::sync::Arc::new(test_app_state());
            state.settings_write().chat_tools.tool_timeout_ms = 1_000;
            let server = python_server(&script);

            state
                .mcp_call_tool(&(), &server, "echo", serde_json::json!({ "text": "warm" }))
                .await
                .expect("warmup ok");

            let first_state = state.clone();
            let first_server = server.clone();
            let first = tokio::spawn(async move {
                first_state
                    .mcp_call_tool(
                        &(),
                        &first_server,
                        "echo",
                        serde_json::json!({ "text": "hang" }),
                    )
                    .await
            });

            tokio::time::sleep(Duration::from_millis(100)).await;

            let second_state = state.clone();
            let second_server = server.clone();
            let second = tokio::time::timeout(Duration::from_millis(500), async move {
                second_state
                    .mcp_call_tool(
                        &(),
                        &second_server,
                        "echo",
                        serde_json::json!({ "text": "two" }),
                    )
                    .await
            })
            .await
            .expect("second request must not wait for the first request timeout")
            .expect("second request should succeed");
            assert_eq!(second.content, "echo: two");

            let first_err = first
                .await
                .expect("first task join")
                .expect_err("the deliberately lost response should time out");
            assert!(first_err.contains("outcome is unknown"), "{first_err}");
            assert!(first_err.contains("was not retried"), "{first_err}");

            state.mcp_disconnect_all().await;
            let _ = std::fs::remove_file(&script);
        }

        #[tokio::test]
        async fn idle_reap_evicts_and_reconnects() {
            let script = write_fake_server();
            let state = test_app_state();
            let server = python_server(&script);

            state
                .mcp_call_tool(&(), &server, "echo", serde_json::json!({ "text": "x" }))
                .await
                .expect("call ok");
            {
                let pool = state.mcp_sessions.lock().await;
                assert!(pool.contains_key(&server.id));
            }

            // 注入极小空闲超时 → 立即过期回收。
            tokio::time::sleep(Duration::from_millis(20)).await;
            let evicted = state.mcp_reap_idle(Duration::from_millis(1)).await;
            assert_eq!(evicted.len(), 1);
            {
                let pool = state.mcp_sessions.lock().await;
                assert!(!pool.contains_key(&server.id), "session should be reaped");
            }

            // 回收后下次调用透明重连。
            let again = state
                .mcp_call_tool(&(), &server, "echo", serde_json::json!({ "text": "y" }))
                .await
                .expect("reconnect after reap ok");
            assert_eq!(again.content, "echo: y");

            // 回收后的重连必须是一次全新握手。回收会把旧会话整个移出连接池
            // （上面已断言 !contains_key），下次调用新建一个全新会话并握手一次 ⇒
            // 新会话 handshake_count == 1。这证明重连走了全新握手而非复用旧连接。
            let handshake_count = {
                let pool = state.mcp_sessions.lock().await;
                let session = pool.get(&server.id).expect("session present").clone();
                drop(pool);
                let guard = session.lock().await;
                guard.handshake_count
            };
            assert_eq!(
                handshake_count, 1,
                "reconnect after reap must build a fresh session with exactly one handshake"
            );

            state.mcp_disconnect_all().await;
            let _ = std::fs::remove_file(&script);
        }

        #[tokio::test]
        async fn disconnect_all_kills_children() {
            let script = write_fake_server();
            let state = test_app_state();
            let server = python_server(&script);

            state
                .mcp_call_tool(&(), &server, "echo", serde_json::json!({ "text": "x" }))
                .await
                .expect("call ok");
            // 记录子进程 pid。
            let pid = {
                let pool = state.mcp_sessions.lock().await;
                let session = pool.get(&server.id).unwrap().clone();
                drop(pool);
                let guard = session.lock().await;
                match &guard.transport {
                    McpTransport::Stdio(conn) => conn.child_id(),
                    _ => panic!("expected stdio transport"),
                }
            };
            assert!(pid.is_some());

            state.mcp_disconnect_all().await;
            {
                let pool = state.mcp_sessions.lock().await;
                assert!(pool.is_empty(), "pool drained on disconnect_all");
            }
            // 给 kill 一点时间生效后确认进程不再存活。
            tokio::time::sleep(Duration::from_millis(200)).await;
            if let Some(pid) = pid {
                // kill -0：进程不存在则失败。
                let alive = std::process::Command::new("kill")
                    .args(["-0", &pid.to_string()])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                assert!(!alive, "child process should be killed");
            }
            let _ = std::fs::remove_file(&script);
        }

        #[tokio::test]
        async fn config_fingerprint_rebuilds_session() {
            let script = write_fake_server();
            let state = test_app_state();
            let mut server = python_server(&script);

            state
                .mcp_call_tool(&(), &server, "echo", serde_json::json!({ "text": "x" }))
                .await
                .expect("call ok");
            let first_fp = {
                let pool = state.mcp_sessions.lock().await;
                let session = pool.get(&server.id).unwrap().clone();
                drop(pool);
                let guard = session.lock().await;
                guard.config_fingerprint.clone()
            };

            // 改配置（新增 arg）→ fingerprint 变化 → get_or_connect 重建会话。
            server.env.insert("EXTRA".to_string(), "1".to_string());
            state
                .mcp_call_tool(&(), &server, "echo", serde_json::json!({ "text": "y" }))
                .await
                .expect("call ok after config change");
            let second_fp = {
                let pool = state.mcp_sessions.lock().await;
                let session = pool.get(&server.id).unwrap().clone();
                drop(pool);
                let guard = session.lock().await;
                guard.config_fingerprint.clone()
            };
            assert_ne!(first_fp, second_fp, "config change must rebuild session");

            state.mcp_disconnect_all().await;
            let _ = std::fs::remove_file(&script);
        }
    }

    /// 跨平台 fake MCP stdio 测试：验证复用会话时也会读取最新的 tool_timeout_ms。
    mod stdio_cross_platform {
        use super::*;
        use std::io::Write;

        fn python_command() -> &'static str {
            if cfg!(windows) {
                "python"
            } else {
                "python3"
            }
        }

        fn write_fake_server() -> std::path::PathBuf {
            let script = r#"#!/usr/bin/env python3
import sys, json, os, time
delay_ms = int(os.environ.get("KIVIO_DELAY_CALL_MS", "0"))
marker = os.environ.get("KIVIO_CALL_MARKER", "")
changed = False
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
        if method == "notifications/cancelled" and marker:
            with open(marker, "a") as f:
                f.write("cancel:"+str(msg.get("params", {}).get("requestId"))+"\n")
        continue
    if method == "initialize":
        resp = {"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"1.0.0"}}}
    elif method == "tools/list":
        cursor = msg.get("params", {}).get("cursor")
        if cursor is None:
            resp = {"jsonrpc":"2.0","id":mid,"result":{"tools":[{"name":"echo","description":"Echo","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}],"nextCursor":"page-2"}}
        else:
            page = [{"name":"second","description":"Second","inputSchema":{"type":"object"}}]
            if changed:
                page.append({"name":"dynamic","description":"Dynamic","inputSchema":{"type":"object"}})
            resp = {"jsonrpc":"2.0","id":mid,"result":{"tools":page}}
    elif method == "tools/call":
        text = ""
        try:
            text = msg["params"]["arguments"].get("text","")
        except Exception:
            text = ""
        if marker:
            with open(marker, "a") as f:
                f.write(str(text)+"\n")
        if text == "hang":
            # Simulate a buggy MCP server that loses one response but remains healthy.
            continue
        if text == "ping":
            sys.stdout.write(json.dumps({"jsonrpc":"2.0","id":mid,"method":"ping"})+"\n")
            sys.stdout.flush()
            ping_response = json.loads(sys.stdin.readline())
            if marker:
                with open(marker, "a") as f:
                    f.write("ping-result:"+str(ping_response.get("result"))+"\n")
            sys.stdout.write(json.dumps({"jsonrpc":"2.0","id":"unknown-"+str(mid),"method":"server/unknown"})+"\n")
            sys.stdout.flush()
            unknown_response = json.loads(sys.stdin.readline())
            if marker:
                with open(marker, "a") as f:
                    f.write("unknown-code:"+str(unknown_response.get("error", {}).get("code"))+"\n")
            resp = {"jsonrpc":"2.0","id":str(mid),"result":{"content":[{"type":"text","text":"echo: ping"}]}}
            sys.stdout.write(json.dumps(resp)+"\n")
            sys.stdout.flush()
            continue
        if text == "change":
            changed = True
            sys.stdout.write(json.dumps({"jsonrpc":"2.0","method":"notifications/tools/list_changed"})+"\n")
            sys.stdout.flush()
        if delay_ms:
            time.sleep(delay_ms / 1000.0)
        resp = {"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":"echo: "+str(text)}]}}
        sys.stdout.write(json.dumps(resp)+"\n")
        sys.stdout.flush()
        continue
    else:
        resp = {"jsonrpc":"2.0","id":mid,"result":{}}
    sys.stdout.write(json.dumps(resp)+"\n")
    sys.stdout.flush()
"#;
            let mut path = std::env::temp_dir();
            path.push(format!("kivio-fake-mcp-xplat-{}.py", uuid::Uuid::new_v4()));
            let mut file = std::fs::File::create(&path).expect("create fake server");
            file.write_all(script.as_bytes())
                .expect("write fake server");
            path
        }

        fn python_server(script: &std::path::Path) -> ChatMcpServer {
            stdio_server(python_command(), &["-u", script.to_str().unwrap()])
        }

        #[tokio::test]
        async fn lost_response_does_not_block_later_request() {
            let script = write_fake_server();
            let state = std::sync::Arc::new(test_app_state());
            state.settings_write().chat_tools.tool_timeout_ms = 1_000;
            let mut marker = std::env::temp_dir();
            marker.push(format!("kivio-fake-mcp-hol-{}.txt", uuid::Uuid::new_v4()));
            let mut server = python_server(&script);
            server.env.insert(
                "KIVIO_CALL_MARKER".to_string(),
                marker.to_string_lossy().into_owned(),
            );

            state
                .mcp_call_tool(&(), &server, "echo", serde_json::json!({ "text": "warm" }))
                .await
                .expect("warmup ok");

            let first_state = state.clone();
            let first_server = server.clone();
            let first = tokio::spawn(async move {
                first_state
                    .mcp_call_tool(
                        &(),
                        &first_server,
                        "echo",
                        serde_json::json!({ "text": "hang" }),
                    )
                    .await
            });
            tokio::time::sleep(Duration::from_millis(100)).await;

            let second = tokio::time::timeout(
                Duration::from_millis(500),
                state.mcp_call_tool(&(), &server, "echo", serde_json::json!({ "text": "two" })),
            )
            .await
            .expect("later request must bypass the lost response")
            .expect("later request should succeed");
            assert_eq!(second.content, "echo: two");

            let first_err = first
                .await
                .expect("first task join")
                .expect_err("lost response should time out");
            assert!(first_err.contains("outcome is unknown"), "{first_err}");
            assert!(first_err.contains("was not retried"), "{first_err}");

            // Writing the notification only guarantees that it reached the child stdin;
            // allow the fake server a short scheduling window to consume it and persist
            // the marker before asserting the observable side effect.
            let mut calls = String::new();
            for _ in 0..50 {
                calls = std::fs::read_to_string(&marker).unwrap_or_default();
                if calls.lines().any(|line| line.starts_with("cancel:")) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(calls.lines().filter(|line| *line == "hang").count(), 1);
            assert_eq!(calls.lines().filter(|line| *line == "two").count(), 1);
            assert!(
                calls.lines().any(|line| line.starts_with("cancel:")),
                "timed-out request should emit notifications/cancelled: {calls}"
            );

            state.mcp_disconnect_all().await;
            let _ = std::fs::remove_file(&script);
            let _ = std::fs::remove_file(&marker);
        }

        #[tokio::test]
        async fn server_ping_is_answered_without_consuming_the_real_response() {
            let script = write_fake_server();
            let state = test_app_state();
            let mut marker = std::env::temp_dir();
            marker.push(format!("kivio-fake-mcp-ping-{}.txt", uuid::Uuid::new_v4()));
            let mut server = python_server(&script);
            server.env.insert(
                "KIVIO_CALL_MARKER".to_string(),
                marker.to_string_lossy().into_owned(),
            );

            let result = state
                .mcp_call_tool(&(), &server, "echo", serde_json::json!({ "text": "ping" }))
                .await
                .expect("server ping should be answered and string response id accepted");
            assert_eq!(result.content, "echo: ping");
            let calls = std::fs::read_to_string(&marker).unwrap_or_default();
            assert!(calls.contains("ping-result:{}"), "{calls}");
            assert!(calls.contains("unknown-code:-32601"), "{calls}");

            state.mcp_disconnect_all().await;
            let _ = std::fs::remove_file(&script);
            let _ = std::fs::remove_file(&marker);
        }

        #[tokio::test]
        async fn tools_list_paginates_and_refreshes_after_list_changed() {
            let script = write_fake_server();
            let state = test_app_state();
            let server = python_server(&script);

            let first = state
                .mcp_list_tools(&(), &server)
                .await
                .expect("initial paginated tools/list");
            assert_eq!(
                first
                    .iter()
                    .map(|tool| tool.name.as_str())
                    .collect::<Vec<_>>(),
                vec!["echo", "second"]
            );

            state
                .mcp_call_tool(
                    &(),
                    &server,
                    "echo",
                    serde_json::json!({ "text": "change" }),
                )
                .await
                .expect("change notification call");
            let refreshed = state
                .mcp_list_tools(&(), &server)
                .await
                .expect("tools/list should refresh after list_changed");
            assert!(refreshed.iter().any(|tool| tool.name == "dynamic"));

            state.mcp_disconnect_all().await;
            let _ = std::fs::remove_file(&script);
        }

        #[tokio::test]
        async fn same_config_snapshot_survives_session_removal_and_restart() {
            let script = write_fake_server();
            let state = test_app_state();
            let usage_dir = state.usage_dir.clone();
            let server = python_server(&script);

            let tools = state
                .mcp_list_tools(&(), &server)
                .await
                .expect("initial list ok");
            assert!(tools.iter().any(|tool| tool.name == "echo"));

            let evicted = state.mcp_reap_idle(Duration::ZERO).await;
            assert_eq!(evicted.len(), 1, "idle reap should remove the live session");
            assert!(state.mcp_sessions.lock().await.is_empty());
            assert!(state
                .mcp_cached_tools(&server)
                .await
                .expect("snapshot survives session removal")
                .iter()
                .any(|tool| tool.name == "echo"));

            let restarted =
                AppState::new_headless(crate::settings::Settings::default(), usage_dir.clone());
            assert!(restarted
                .mcp_cached_tools(&server)
                .await
                .expect("snapshot reloads across AppState restart")
                .iter()
                .any(|tool| tool.name == "echo"));

            let _ = std::fs::remove_file(&script);
            let _ = std::fs::remove_dir_all(&usage_dir);
        }

        #[tokio::test]
        async fn same_config_failure_reuses_snapshot_but_changed_config_does_not() {
            let script = write_fake_server();
            let state = test_app_state();
            let server = python_server(&script);

            state
                .mcp_list_tools(&(), &server)
                .await
                .expect("initial list ok");
            state.mcp_disconnect_all().await;
            std::fs::remove_file(&script).expect("remove fake server to force same-config failure");

            state
                .mcp_list_tools(&(), &server)
                .await
                .expect_err("same config should now fail to reconnect");
            assert!(state
                .mcp_cached_tools(&server)
                .await
                .expect("same config may use last-known schema")
                .iter()
                .any(|tool| tool.name == "echo"));
            assert!(
                state.mcp_unreachable_server_ids().await.is_empty(),
                "a failed server with a matching snapshot is degraded, not unreachable"
            );

            let mut changed = server.clone();
            changed.command = "kivio-definitely-missing-cmd".to_string();
            state
                .mcp_list_tools(&(), &changed)
                .await
                .expect_err("changed config must fail independently");
            assert!(
                state.mcp_cached_tools(&changed).await.is_none(),
                "a schema from an old fingerprint must never leak into changed config"
            );
            assert_eq!(
                state.mcp_unreachable_server_ids().await,
                vec![server.id.clone()]
            );

            state.mcp_disconnect_all().await;
            let _ = std::fs::remove_dir_all(&state.usage_dir);
        }

        #[tokio::test]
        async fn discovery_failures_are_throttled_but_explicit_connect_can_retry() {
            let state = test_app_state();
            let server = stdio_server("kivio-definitely-missing-cmd", &[]);

            state
                .mcp_list_tools(&(), &server)
                .await
                .expect_err("first discovery connect must fail");
            let session = {
                let pool = state.mcp_sessions.lock().await;
                pool.get(&server.id)
                    .cloned()
                    .expect("error session retained")
            };
            assert_eq!(session.lock().await.consecutive_connect_failures, 1);

            let second = state
                .mcp_list_tools(&(), &server)
                .await
                .expect_err("second discovery should honor cooldown");
            assert!(
                second.contains("cooling down"),
                "unexpected error: {second}"
            );
            assert_eq!(
                session.lock().await.consecutive_connect_failures,
                1,
                "cooldown must avoid another spawn/handshake attempt"
            );

            assert!(
                state.mcp_get_or_connect(&(), &server).await.is_err(),
                "explicit path retries immediately and still fails"
            );
            assert_eq!(
                session.lock().await.consecutive_connect_failures,
                2,
                "explicit tool path must bypass discovery cooldown"
            );

            state.mcp_disconnect_all().await;
            let _ = std::fs::remove_dir_all(&state.usage_dir);
        }

        #[tokio::test]
        async fn never_connected_error_server_is_unreachable() {
            let state = test_app_state();
            let server = stdio_server("kivio-definitely-missing-cmd", &[]);
            assert!(
                state.mcp_get_or_connect(&(), &server).await.is_err(),
                "missing command must fail"
            );
            assert!(state.mcp_cached_tools(&server).await.is_none());
            assert_eq!(
                state.mcp_unreachable_server_ids().await,
                vec![server.id.clone()]
            );
            state.mcp_disconnect_all().await;
            let _ = std::fs::remove_dir_all(&state.usage_dir);
        }

        #[test]
        fn discovery_retry_backoff_is_exponential_and_capped() {
            assert_eq!(discovery_retry_backoff(1), Duration::from_secs(2));
            assert_eq!(discovery_retry_backoff(2), Duration::from_secs(4));
            assert_eq!(discovery_retry_backoff(5), Duration::from_secs(32));
            assert_eq!(discovery_retry_backoff(6), Duration::from_secs(60));
            assert_eq!(discovery_retry_backoff(99), Duration::from_secs(60));
        }

        #[tokio::test]
        async fn reused_stdio_session_honors_increased_tool_timeout() {
            let script = write_fake_server();
            let state = test_app_state();
            state.settings_write().chat_tools.tool_timeout_ms = 1_000;

            let mut server = python_server(&script);
            server
                .env
                .insert("KIVIO_DELAY_CALL_MS".to_string(), "2500".to_string());

            let err = state
                .mcp_call_tool(&(), &server, "echo", serde_json::json!({ "text": "slow" }))
                .await
                .expect_err("1s timeout should fail on a 2.5s tool");
            assert!(
                err.contains("timed out"),
                "expected timeout error, got: {err}"
            );

            state.settings_write().chat_tools.tool_timeout_ms = 5_000;
            let result = state
                .mcp_call_tool(&(), &server, "echo", serde_json::json!({ "text": "slow" }))
                .await
                .expect("5s timeout should succeed on the same reused session");
            assert_eq!(result.content, "echo: slow");

            state.mcp_disconnect_all().await;
            let _ = std::fs::remove_file(&script);
        }
    }
}
