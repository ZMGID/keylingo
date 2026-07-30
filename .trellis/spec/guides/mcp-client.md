# MCP 客户端执行契约

> **适用**:改动 `src-tauri/src/mcp/conn.rs`、`manager.rs`、`result.rs`,或升级 `rmcp` 版本时**必读**。

## 背景:wire 协议只准有一份,而且不是我们写的

历史上 MCP 的 JSON-RPC 收发在这个仓库里**实现了两遍**:`client.rs` 的一次性连接和 `manager.rs` 里连接池自己那套 `StdioConn` / `http_*`。后果是协议永远追不上规范(我们停在 `2025-06-18`),而且 404 session 重连、超时取消这些只有 `manager.rs` 有 —— 同一个服务器走两个入口表现不同。

现在 wire 层是**官方 rmcp SDK 独占**,入口只有 `mcp/conn.rs`。

**任何时候都不要再手写 MCP 的 JSON-RPC 收发、SSE 解析、协议版本协商、tools/list 游标分页。** 需要新能力就去 rmcp 里找;rmcp 没有就提 issue 或本地 patch,不要在 `manager.rs` 里长出第二套。

## 分层红线

| 层 | 归谁 | 不准干什么 |
|---|---|---|
| `conn.rs` | 建 transport、握手、`tools/call`、`tools/list`、错误翻译 | 不碰连接池、状态事件、退避、快照 |
| `manager.rs` | 连接池 + 单飞、配置指纹、`McpServerState` 事件、发现退避、空闲回收、工具快照 | 不出现任何 JSON-RPC 字面量、HTTP header、子进程管道 |
| `result.rs` | MCP 结果 JSON → `McpToolCallResult` | 不认识任何 rmcp 类型(输入是 `serde_json::Value`) |

## 契约一:`OAUTH_REQUIRED:` 前缀

设置页靠这个前缀决定「弹 OAuth 授权引导」还是「显示裸错误」。`conn::classify_error` 是唯一产地,三种情况必须命中:

- 401 **带** `WWW-Authenticate` → rmcp 给 `StreamableHttpError::AuthRequired`(Display 含 `authorization required`)
- 403 `insufficient_scope` → `InsufficientScope`
- 401 **不带**质询头 → rmcp **不走** `AuthRequired`,落到 `UnexpectedServerResponse("HTTP 401 ...")`,靠串里的 `http 401` 捞回来

`SessionExpired` / 500 / 传输关闭**不得**加前缀。有单测锁定,改 `classify_error` 前先看那几条。

## 契约二:超时说人话

rmcp 的超时错误只说 `request timeout after PT1S`。用户真正需要知道的是**服务器可能已经执行了,而我们没有重试**,所以 `classify_error` 检到超时会追加 `timed out — request outcome is unknown and was not retried`。

这不是装饰,是 `lost_response_does_not_block_later_request` 断言的内容,也是「绝不重放非幂等工具调用」这条规则对用户的交代。

## 契约三:超时必须走 rmcp 自己的超时路径

发 `tools/call` 用 `send_cancellable_request` + `PeerRequestOptions::with_timeout`,**不准**写成 `tokio::time::timeout(service.call_tool(..))`。

原因:`RequestHandle` **没有 `Drop` 实现**。从外面把 future 丢掉只是静默放弃,服务器那边还在跑、也收不到取消通知。rmcp 自己的超时路径会先发 `notifications/cancelled` 再返回 `ServiceError::Timeout`。

例外:`registry.rs::list_server_tools`(设置页测试连接)和 `conn::list_tools`(见下)整体套了一层 `tokio::time::timeout`。那里可以,因为握手 + `tools/list` 都是幂等读。**`tools/call` 上没有这个例外。**

## 契约四:rmcp 缺的两个保护得我们自己补

手写版有、rmcp 没有,已经补回来了 —— 别在重构中再把它们弄丢:

1. **`tools/list` 的页数上限。** rmcp 的 `list_all_tools` 跟着 `nextCursor` 一直翻页,**没有页数上限**(手写版有 `MAX_TOOL_LIST_PAGES = 100` + 重复游标检测)。游标坏掉的服务器会让它永远转下去,而 `tools/list` 在聊天热路径上。所有列工具都必须走 `conn::list_tools`(带 `LIST_TOOLS_TIMEOUT`),**不要直接调 `service.list_all_tools()`**。
2. **判活只能靠错误串,`is_closed()` 是恒假的。** 已读源码核实:`is_closed() = handle.is_none() || cancellation_token.is_cancelled()`,而服务循环在传输关闭时是 `break QuitReason::Closed`(`service.rs:1313`)—— **不 cancel token**,`handle` 也只被 `waiting()` / `close()` / `cancel()` 拿走。所以对连接池里握着的 `RunningService`,子进程死了、循环也退了,`is_closed()` **永远不会变 true**。判「该不该重连」一律用 `conn::connection_is_gone(service, err)`,里面那条 `err.contains("Transport closed")` **不是冗余、是唯一有用的那一路**,别当成时间窗兜底删掉。
3. **握手超时。** rmcp 的 `legacy_startup` 等 `initialize` 响应是**裸 await**,没有内建超时;手写版一直是带超时的。少了它,一个「进程起来了但不回 initialize」的 server 会永久占住会话锁,连带让退出钩子里的 `mcp_disconnect_all` 拿不到锁 ⇒ 应用关不掉。见 `conn::HANDSHAKE_TIMEOUT`。
4. **握手失败时的 stderr。** `spawn_stderr_tail` 只在握手**成功后**才挂上,所以 `conn::connect` 的失败路径必须自己 `peek_stderr` 一段 —— 「服务器起不来 / 缺依赖 / 认证失败」这类故障只在 stderr 上说话。

## 契约五:`McpToolCallResult` 的形状靠 serde 命名撑着

`result::parse_tool_result` 读的是 MCP wire 形状:`isError` / `structuredContent` / `content[].type|text|data|mimeType`。rmcp 的 `CallToolResult` 和 `Tool` 都是 `rename_all = "camelCase"`,所以调用方一律 `serde_json::to_value(..)` 再喂进去,不手抄字段。

`result.rs` 有 5 个契约测试用真的 rmcp 类型构造。**rmcp 改了 serde 命名,那几个测试会红** —— 别把它们改绿了了事,那意味着工具结果在运行时会静默退化成一坨 JSON,图片 artifact 会整个消失。

`McpTool` / `ChatToolArtifact` / `follow_up_user_messages` 是前端和 agent loop 的契约,换 SDK 不得改。

## 契约六:锁的顺序

两条都是踩过坑的(见 prd 风险段):

1. 绝不跨 await 持 `AppState.mcp_sessions` 外层池锁。命中就克隆 `Arc<Mutex<McpSession>>`,立刻放锁。新建走「插 `Connecting` 占位 → 放外层锁 → 才握手」的单飞门闩,并发的第二个调用者一定观察到第一个的占位。
2. 会话锁只保护生命周期状态迁移。等 RPC 响应前**必须**先克隆 `Arc<McpService>` 再放会话锁 —— 否则一次丢响应会 head-of-line 阻塞后面每个请求。`lost_response_does_not_block_later_request` 就是钉这个的。

## 契约七:一次性连接只有两个正当用途

`conn::connect_once`(及 `list_tools_once` / `call_tool_once`)用的是和 `connect` **完全相同**的 transport 构造,不是第二条 wire 路径。只准用在:

- `registry.rs::list_server_tools` —— 设置页「测试连接」测的是**还没保存**的草稿配置,进池就等于把半成品当常驻服务器缓存了;
- `web_search.rs::search_exa_mcp` —— 临时合成的 exa server,api key 就在 URL 里。

两个测试断言「一次性连接后 `mcp_sessions` 仍为空」。新增用途前先问:这个 server 是用户配置的常驻服务器吗?是就走连接池。

## 我们依赖的 rmcp 默认值

这几个默认值是行为的一部分,升版本时逐条确认:

- `.serve()` 默认 `ClientLifecycleMode::Initialize`(legacy 握手),**不做协议版本校验**,`ProtocolVersion` 的 Deserialize 对未知字符串也放行。所以老服务器(2024-11-05 之类)照样连得上 —— 比我们原来的白名单更宽松。
- `reinit_on_expired_session = true`,且**只对** `SessionExpired`(404 + 已附 session)生效。「404 重连、500 不重连」靠它。有 `http_reconnect_only_on_404` / `http_500_does_not_reconnect` 两个假服务器测试。
- `ClientCacheConfig::default()`:`enabled = true` 但 `default_ttl = ZERO`(服务器不给 `ttlMs` 就立刻过期),`serve_stale_on_error = true`。**后者不是纯利好**:`tools/list` 失败时 rmcp 会返回上一次的陈旧工具列表并当成 `Ok`,而不是报错(`service/client.rs:1556`)。叠上上面那条「`is_closed()` 恒假」,一个刚死的 stdio server 会「看起来还健康、工具列表还在」,直到真的调用工具才炸。这是刻意的 SEP-2549 行为,不是 bug,但别把它记成「默认值对我们有利」。
- `max_sse_event_size = 16MB`:单个 SSE 事件超过就被拒。MCP 工具返回大 base64 图片时可能踩到。
- `retry_config = ExponentialBackoff::default()` 即 `max_times: None` ⇒ **SSE 重连次数无上限**(间隔 1s×2^n)。它和 `manager.rs` 的发现退避 / 空闲回收是两套独立退避,叠加行为没人验证过。
- `CommandWrap::from(Command)` 是**空 wrapper 集合**,`spawn()` 直接调 `command.spawn()`。我们的 `no_console_window()`(`CREATE_NO_WINDOW`)因此原样生效。Windows 上一旦闪控制台窗口,先查这里。
- `RunningService` 的 DropGuard:Arc 落地即取消服务循环,循环退出时走 `transport.close()`(stdio = `graceful_shutdown`,3s 后 kill;HTTP = 发 `DELETE` 释放 session)。`close_transport` 在能独占 Arc 时带超时等它跑完,这样退出钩子能真收掉子进程。

## 版本策略

`Cargo.toml` 里 rmcp **写死版本号,不用 caret**:

```toml
rmcp = { version = "3.0.0", default-features = false, features = [...] }
```

3.x 仍在破坏性演进(文档覆盖率约 47%,不少 API 只能读源码),升版本当独立任务做:先按上面「我们依赖的 rmcp 默认值」逐条复核,再跑 `pwsh scripts/win-cargo-test.ps1 --lib mcp::`。

读源码的位置:`~/.cargo/registry/src/*/rmcp-<ver>/src/`。**不要按 docs.rs 猜。**

## 顺带的既有事实

- 我们向服务器宣称的协议版本是 `conn.rs::ADVERTISED_PROTOCOL_VERSION`,当前仍是 `2025-06-18`(重构时刻意在 wire 上零变化)。**但这已经不只是版本号问题了**:规范最新 revision 是 **2026-07-28**,它把 `initialize` / `notifications/initialized` 握手**整个删掉**、改成 `server/discover`(服务器 MUST 实现)。只实现新规范的 server 不认 `initialize` ⇒ **连不上**,不是降级。rmcp 3.0.0 已经给了 `ClientLifecycleMode::Auto { preferred_versions, legacy_version }`(先探 `server/discover`,证明对端是 legacy 才回落),改动量约等于把 `.serve(t)` 换成 `.serve_with_lifecycle(t, Auto{..})`,并顺手升到 3.0.1(#1080 正是这块)。**这是本文件里唯一一条「以后会真连不上」的欠账。**
- OAuth token 由 `connectors/oauth.rs` 写进 `ChatMcpServer.headers["Authorization"]`,`conn.rs` 整体映射成 rmcp 的 `custom_headers`,**不用** `auth_header`,只有一条路径。rmcp 的保留头只有 `accept` / `Mcp-Session-Id` / `Last-Event-Id`,`Authorization` 不在里面。
- `connectors/oauth.rs` 那套 PKCE + DCR 是刻意保留的,**没有**换成 rmcp 的 `AuthorizationManager`。**注意此处原先的理由(「rmcp 没有文档化的注入已有 token 入口」)是错的**:rmcp 有 `CredentialStore` trait + `set_credential_store` + `initialize_from_store()`(`transport/auth.rs`,带 doc comment)。保留手写版现在只剩「不想在这次重构里动授权流程」这一个理由,重新评估时别再引用那个错前提。
- RFC 8707 `resource` 参数已补齐(authorize / code 换 token / refresh 三处,见 `oauth::canonical_resource_indicator`)。这是 2025-06-18 规范的 **MUST**,而且要求「不管授权服务器支持不支持都必须发」。rmcp 自己的 auth 模块也是这么做的。仍**缺**的两个小口子:DCR 请求体没带 `application_type`(2026-07-28 minor #8),授权响应没校验 `iss`(RFC 9207,只校验了 `state`)。
- `which-command` feature 顺手修了 Windows 上 `npx.cmd` 这类 shim 找不到的老 bug(`conn::build_stdio` 先 `which_command`,失败退回 `Command::new`)。
