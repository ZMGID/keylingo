//! 远程 MCP 的 OAuth 2.1 授权流程（PKCE + 动态客户端注册 DCR + loopback 回调）。
//!
//! 符合 MCP Authorization / OAuth 2.1，对 Notion 及任意支持 DCR 的远程 MCP 通用。
//! 流程：发现授权服务器 → DCR 注册公有客户端 → 生成 PKCE → 起 loopback 监听并开浏览器
//! 授权 → 拿 code 换 token → 物化成带 Authorization header 的 ChatMcpServer。
//!
//! 设计原则：把"判断/构造"做成纯函数（可单测，不碰网络/时间/IO），IO 与时间只在
//! 薄薄一层 async 函数里。`StreamableHttpMcpClient` 不变（仍只发 header）；token 刷新
//! 的纯逻辑（是否需要刷新、刷新请求体）也放在这里供 manager 复用。

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::Rng;
use sha2::{Digest, Sha256};
use tauri::AppHandle;
use tauri_plugin_shell::ShellExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::timeout;

use crate::settings::{ChatMcpServer, ConnectorAuth};

/// 发现 / DCR / token 单次请求的网络超时。
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
/// 等待用户在浏览器完成授权的整体超时。
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);
/// access_token 还剩多少秒内将过期就提前刷新（避免临界值连接失败）。
pub const REFRESH_LEEWAY_SECS: i64 = 60;

/// 授权服务器元数据（从 well-known 发现得到）。
#[derive(Debug, Clone)]
pub struct AuthServerMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub scopes_supported: Vec<String>,
    /// OIDC userinfo 端点（若 metadata 提供）。token 响应缺账户信息时可用它兜底取 email/name。
    pub userinfo_endpoint: Option<String>,
}

/// PKCE 一对：verifier 发往 token 端点，challenge 发往 authorize 端点。
#[derive(Debug, Clone)]
pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
}

// ============================ 纯函数（可单测，无 IO） ============================

/// 生成 PKCE：code_verifier 为 43–128 个 unreserved 字符；
/// code_challenge = base64url-nopad(SHA256(verifier))，method S256。
pub fn generate_pkce() -> PkcePair {
    const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::thread_rng();
    // 取 64 字符（落在 43–128 区间）。
    let verifier: String = (0..64)
        .map(|_| {
            let idx = rng.gen_range(0..UNRESERVED.len());
            UNRESERVED[idx] as char
        })
        .collect();
    let challenge = pkce_challenge(&verifier);
    PkcePair {
        verifier,
        challenge,
    }
}

/// 由 verifier 算 S256 challenge（独立纯函数，便于断言已知向量）。
pub fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

/// 解析 protected-resource metadata，取第一个 authorization server URL。
pub fn parse_authorization_server(value: &serde_json::Value) -> Option<String> {
    value
        .get("authorization_servers")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .map(|s| s.trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
}

/// 解析 authorization-server / openid-configuration metadata。
pub fn parse_auth_server_metadata(value: &serde_json::Value) -> Option<AuthServerMetadata> {
    let authorization_endpoint = value
        .get("authorization_endpoint")
        .and_then(|v| v.as_str())?
        .to_string();
    let token_endpoint = value
        .get("token_endpoint")
        .and_then(|v| v.as_str())?
        .to_string();
    let registration_endpoint = value
        .get("registration_endpoint")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let scopes_supported = value
        .get("scopes_supported")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(AuthServerMetadata {
        authorization_endpoint,
        token_endpoint,
        registration_endpoint,
        scopes_supported,
        userinfo_endpoint: value
            .get("userinfo_endpoint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    })
}

/// 由 resource URL 推导发现起点的 origin（scheme://host[:port]）与 path。
pub fn split_origin_and_path(resource_url: &str) -> Result<(String, String), String> {
    let parsed =
        url::Url::parse(resource_url).map_err(|err| format!("Invalid connector URL: {err}"))?;
    let scheme = parsed.scheme();
    let host = parsed
        .host_str()
        .ok_or_else(|| "Connector URL has no host".to_string())?;
    let origin = match parsed.port() {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    };
    let path = parsed.path().trim_end_matches('/').to_string();
    Ok((origin, path))
}

/// 候选的 protected-resource well-known URL 列表（带 path 变体优先，再回退根）。
pub fn protected_resource_well_known_urls(origin: &str, path: &str) -> Vec<String> {
    let mut urls = Vec::new();
    if !path.is_empty() {
        urls.push(format!(
            "{origin}/.well-known/oauth-protected-resource{path}"
        ));
    }
    urls.push(format!("{origin}/.well-known/oauth-protected-resource"));
    urls
}

/// 候选的 authorization-server metadata well-known URL（标准在前，OIDC 回退在后）。
pub fn auth_server_well_known_urls(auth_server: &str) -> Vec<String> {
    let base = auth_server.trim_end_matches('/');
    vec![
        format!("{base}/.well-known/oauth-authorization-server"),
        format!("{base}/.well-known/openid-configuration"),
    ]
}

/// RFC 8707 的 resource indicator：这次授权/这枚 token 是**给哪个 MCP 服务器**用的。
///
/// MCP 规范（2025-06-18 起）原文是 MUST，而且是「不管授权服务器支持不支持都必须发」：
/// > MCP clients MUST implement Resource Indicators for OAuth 2.0 as defined in RFC 8707 …
/// > MUST be included in both authorization requests and token requests … MCP clients MUST send
/// > this parameter regardless of whether authorization servers support it.
///
/// 不发的代价不只是「不合规」：拿到的 token 没有 audience 绑定，规范专门有一节
/// *Access Token Privilege Restriction* 讲这正是跨服务重用 token 的攻击面。
///
/// 规范化：**显式**去掉 fragment（RFC 9728 明确禁止）并把 host 转小写，其余原样保留。
/// host 小写其实 `url` crate 解析时就顺手做了，这里再做一次是为了**不依赖那个顺带行为**
/// —— 哪天有人把这里换成纯字符串处理，规范化就会静默丢掉。
pub fn canonical_resource_indicator(resource_url: &str) -> Result<String, String> {
    let mut url = url::Url::parse(resource_url.trim())
        .map_err(|err| format!("Invalid MCP resource URL: {err}"))?;
    url.set_fragment(None);
    if let Some(host) = url.host_str().map(str::to_ascii_lowercase) {
        url.set_host(Some(&host))
            .map_err(|err| format!("Invalid MCP resource host: {err}"))?;
    }
    Ok(url.to_string())
}

/// 构造 authorize URL（含 PKCE / state / scope / RFC 8707 resource）。
pub fn build_authorize_url(
    authorization_endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
    scopes: &[String],
    resource: &str,
) -> Result<String, String> {
    let mut url = url::Url::parse(authorization_endpoint)
        .map_err(|err| format!("Invalid authorization endpoint: {err}"))?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("response_type", "code");
        query.append_pair("client_id", client_id);
        query.append_pair("redirect_uri", redirect_uri);
        query.append_pair("state", state);
        query.append_pair("code_challenge", challenge);
        query.append_pair("code_challenge_method", "S256");
        if !scopes.is_empty() {
            query.append_pair("scope", &scopes.join(" "));
        }
        // 见 `canonical_resource_indicator`：规范要求无条件带上。
        query.append_pair("resource", resource);
    }
    Ok(url.to_string())
}

/// 从 loopback 回调请求行（`GET /callback?... HTTP/1.1`）解析 query 参数。
pub fn parse_callback_query(request_line: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    // 取第二个 token（请求目标），如 `/callback?code=x&state=y`。
    let target = request_line.split_whitespace().nth(1).unwrap_or_default();
    let query = match target.split_once('?') {
        Some((_, q)) => q,
        None => return out,
    };
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        let key = url_decode(k);
        let value = url_decode(v);
        out.insert(key, value);
    }
    out
}

/// 极简 application/x-www-form-urlencoded 解码（`+`→空格，`%XX`→字节）。
fn url_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = hex_val(bytes[i + 1]);
                let lo = hex_val(bytes[i + 2]);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push(hi * 16 + lo);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// 计算 access_token 的绝对过期时间戳（unix 秒）。`expires_in` 为相对秒数。
pub fn compute_expires_at(now_unix: i64, expires_in: Option<i64>) -> Option<i64> {
    expires_in.map(|secs| now_unix + secs)
}

/// 是否需要刷新：oauth 类型、有 refresh_token，且 expires_at 已过期或将在 leeway 内过期。
/// 无 expires_at（服务端没给）则保守判断为不需要刷新（用现有 token 试连）。
pub fn needs_refresh(auth: &ConnectorAuth, now_unix: i64, leeway_secs: i64) -> bool {
    if auth.kind != "oauth" {
        return false;
    }
    if auth
        .refresh_token
        .as_deref()
        .map(|t| t.trim().is_empty())
        .unwrap_or(true)
    {
        return false;
    }
    if auth
        .token_endpoint
        .as_deref()
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        return false;
    }
    match auth.expires_at {
        Some(expires_at) => now_unix + leeway_secs >= expires_at,
        None => false,
    }
}

/// 构造 refresh_token 授权的表单字段（application/x-www-form-urlencoded 的键值）。
pub fn build_refresh_form(
    refresh_token: &str,
    client_id: Option<&str>,
    resource: Option<&str>,
) -> Vec<(String, String)> {
    let mut form = vec![
        ("grant_type".to_string(), "refresh_token".to_string()),
        ("refresh_token".to_string(), refresh_token.to_string()),
    ];
    if let Some(client_id) = client_id.filter(|c| !c.trim().is_empty()) {
        form.push(("client_id".to_string(), client_id.to_string()));
    }
    // 刷新同样要带 resource —— 否则换回来的新 token 又丢了 audience 绑定
    // （见 `canonical_resource_indicator`）。
    if let Some(resource) = resource.filter(|r| !r.trim().is_empty()) {
        form.push(("resource".to_string(), resource.to_string()));
    }
    form
}

/// token 端点返回解析结果。
#[derive(Debug, Clone)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<i64>,
    pub scope: Option<String>,
    /// 从 token 响应里尽力提取的真实账户标识（邮箱 > 工作区名 > 用户名），拿不到为 None。
    pub account: Option<String>,
}

/// 解析 token 端点 JSON 响应。
pub fn parse_token_response(value: &serde_json::Value) -> Result<TokenResponse, String> {
    let access_token = value
        .get("access_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            let err = value
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("token response missing access_token");
            format!("OAuth token error: {err}")
        })?;
    let refresh_token = value
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let expires_in = value.get("expires_in").and_then(|v| v.as_i64());
    let scope = value
        .get("scope")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Ok(TokenResponse {
        access_token,
        refresh_token,
        expires_in,
        scope,
        account: extract_account(value),
    })
}

/// 从 token 响应 JSON 里尽力提取真实账户标识。
///
/// 优先级：邮箱 > 工作区名 > 用户名。覆盖 Notion OAuth 形态（顶层 `workspace_name` /
/// `owner.user.person.email` / `owner.user.name`）与扁平 `email` / `name` / `account`。
/// 一个都没有返回 None（绝不回退成端点 URL）。
pub fn extract_account(value: &serde_json::Value) -> Option<String> {
    // 1) 邮箱优先：顶层 email，或 Notion owner.user.person.email。
    let email = str_field(value, "email").or_else(|| {
        value
            .pointer("/owner/user/person/email")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    });
    if let Some(email) = non_empty(email) {
        return Some(email);
    }
    // 2) 工作区名（Notion 常带）。
    if let Some(ws) = non_empty(str_field(value, "workspace_name")) {
        return Some(ws);
    }
    // 3) 用户名：Notion owner.user.name，或顶层 name / account。
    let name = value
        .pointer("/owner/user/name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| str_field(value, "name"))
        .or_else(|| str_field(value, "account"));
    non_empty(name)
}

/// 取 JSON 顶层字符串字段。
fn str_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// 去除首尾空白后非空才保留。
fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 由换 token 结果与连接器元信息物化成 ChatMcpServer。
pub fn materialize_server(
    connector_id: &str,
    name: &str,
    resource_url: &str,
    token: &TokenResponse,
    token_endpoint: &str,
    client_id: &str,
    requested_scopes: &[String],
    now_unix: i64,
) -> ChatMcpServer {
    let mut headers = HashMap::new();
    headers.insert(
        "Authorization".to_string(),
        format!("Bearer {}", token.access_token),
    );
    let scopes = match &token.scope {
        Some(scope) if !scope.trim().is_empty() => {
            scope.split_whitespace().map(|s| s.to_string()).collect()
        }
        _ => requested_scopes.to_vec(),
    };
    let auth = ConnectorAuth {
        kind: "oauth".to_string(),
        access_token: token.access_token.clone(),
        refresh_token: token.refresh_token.clone(),
        expires_at: compute_expires_at(now_unix, token.expires_in),
        token_endpoint: Some(token_endpoint.to_string()),
        client_id: Some(client_id.to_string()),
        scopes,
        account: token.account.clone(),
    };
    ChatMcpServer {
        id: format!("connector-{connector_id}"),
        name: name.to_string(),
        enabled: true,
        transport: "streamable_http".to_string(),
        url: resource_url.to_string(),
        command: String::new(),
        args: Vec::new(),
        env: HashMap::new(),
        headers,
        cwd: None,
        enabled_tools: Vec::new(),
        connector_id: Some(connector_id.to_string()),
        auth: Some(auth),
    }
}

/// 当前 unix 时间戳（秒）。
fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ============================ IO 层（网络 / 浏览器 / loopback） ============================

/// 跑完整 OAuth 流程，返回物化好的 ChatMcpServer（不写 settings，由前端保存）。
pub async fn run_oauth_connect(
    app: &AppHandle,
    http: &reqwest::Client,
    connector_id: &str,
    name: &str,
    resource_url: &str,
) -> Result<ChatMcpServer, String> {
    // 1. 起 loopback 监听，拿真实端口 → redirect_uri。
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| format!("Failed to bind loopback listener: {err}"))?;
    let port = listener
        .local_addr()
        .map_err(|err| format!("Failed to read loopback port: {err}"))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    // 2. 发现授权服务器与端点。
    let metadata = discover_auth_server(http, resource_url).await?;
    let registration_endpoint = metadata.registration_endpoint.clone().ok_or_else(|| {
        "This server does not advertise dynamic client registration (DCR); \
         Phase B only supports the DCR path."
            .to_string()
    })?;

    // 3. DCR 注册公有客户端。
    let client_id = register_client(
        http,
        &registration_endpoint,
        &redirect_uri,
        &metadata.scopes_supported,
    )
    .await?;

    // 4. PKCE + state + RFC 8707 resource。
    let pkce = generate_pkce();
    let state = uuid::Uuid::new_v4().to_string();
    let resource = canonical_resource_indicator(resource_url)?;
    let authorize_url = build_authorize_url(
        &metadata.authorization_endpoint,
        &client_id,
        &redirect_uri,
        &state,
        &pkce.challenge,
        &metadata.scopes_supported,
        &resource,
    )?;

    // 5. 开浏览器，等 loopback 回调拿 code（校验 state，带整体超时）。
    #[allow(deprecated)]
    app.shell()
        .open(authorize_url, None)
        .map_err(|err| format!("Failed to open browser for authorization: {err}"))?;
    let code = wait_for_callback(listener, &state).await?;

    // 6. 换 token。
    let mut token = exchange_code(
        http,
        &metadata.token_endpoint,
        &code,
        &redirect_uri,
        &client_id,
        &pkce.verifier,
        &resource,
    )
    .await?;

    // 6b. token 响应没带账户信息且服务器声明了 OIDC userinfo 端点时，尽力 GET 一次兜底
    //     （加超时、失败忽略，不影响连接成功）。
    if token.account.is_none() {
        if let Some(userinfo_endpoint) = metadata.userinfo_endpoint.as_deref() {
            if let Some(account) =
                fetch_userinfo_account(http, userinfo_endpoint, &token.access_token).await
            {
                token.account = Some(account);
            }
        }
    }

    Ok(materialize_server(
        connector_id,
        name,
        resource_url,
        &token,
        &metadata.token_endpoint,
        &client_id,
        &metadata.scopes_supported,
        now_unix(),
    ))
}

/// 发现授权服务器元数据。失败给出清晰错误（不硬编码任何厂商端点）。
async fn discover_auth_server(
    http: &reqwest::Client,
    resource_url: &str,
) -> Result<AuthServerMetadata, String> {
    let (origin, path) = split_origin_and_path(resource_url)?;

    // 1) protected-resource → authorization server。
    let mut auth_server: Option<String> = None;
    for url in protected_resource_well_known_urls(&origin, &path) {
        if let Some(value) = fetch_json(http, &url).await {
            if let Some(server) = parse_authorization_server(&value) {
                auth_server = Some(server);
                break;
            }
        }
    }
    // 回退：没有 protected-resource metadata 时，把 origin 直接当授权服务器试。
    let auth_server = auth_server.unwrap_or_else(|| origin.clone());

    // 2) authorization-server / openid-configuration metadata。
    for url in auth_server_well_known_urls(&auth_server) {
        if let Some(value) = fetch_json(http, &url).await {
            if let Some(metadata) = parse_auth_server_metadata(&value) {
                return Ok(metadata);
            }
        }
    }

    Err(format!(
        "OAuth discovery failed: could not resolve authorization-server metadata for {auth_server}. \
         The server may not support OAuth 2.1 discovery."
    ))
}

/// GET 一个 well-known URL 并尝试解析 JSON；失败返回 None（让上层试下一个候选）。
async fn fetch_json(http: &reqwest::Client, url: &str) -> Option<serde_json::Value> {
    let response = timeout(DISCOVERY_TIMEOUT, http.get(url).send())
        .await
        .ok()?
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let value = timeout(DISCOVERY_TIMEOUT, response.json::<serde_json::Value>())
        .await
        .ok()?
        .ok()?;
    Some(value)
}

/// RFC 7591 动态客户端注册（公有客户端，无 secret）。
async fn register_client(
    http: &reqwest::Client,
    registration_endpoint: &str,
    redirect_uri: &str,
    scopes: &[String],
) -> Result<String, String> {
    let mut body = serde_json::json!({
        "client_name": "Kivio",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    if !scopes.is_empty() {
        body["scope"] = serde_json::Value::String(scopes.join(" "));
    }
    let response = timeout(
        DISCOVERY_TIMEOUT,
        http.post(registration_endpoint).json(&body).send(),
    )
    .await
    .map_err(|_| "Dynamic client registration timed out".to_string())?
    .map_err(|err| format!("Dynamic client registration failed: {err}"))?;
    let status = response.status();
    let value = response
        .json::<serde_json::Value>()
        .await
        .map_err(|err| format!("Failed to parse DCR response: {err}"))?;
    if !status.is_success() {
        return Err(format!(
            "Dynamic client registration rejected ({}): {}",
            status.as_u16(),
            value
        ));
    }
    value
        .get("client_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "DCR response missing client_id".to_string())
}

/// 等 loopback 回调请求，解析 code 并校验 state；给浏览器回简短 HTML。带整体超时。
async fn wait_for_callback(listener: TcpListener, expected_state: &str) -> Result<String, String> {
    let result = timeout(CALLBACK_TIMEOUT, async {
        loop {
            let (mut stream, _) = listener
                .accept()
                .await
                .map_err(|err| format!("Loopback accept failed: {err}"))?;

            // 读到请求头结束（GET 回调没有 body）。
            let mut buffer = vec![0_u8; 8192];
            let mut read = 0_usize;
            let request_line = loop {
                let n = stream
                    .read(&mut buffer[read..])
                    .await
                    .map_err(|err| format!("Loopback read failed: {err}"))?;
                if n == 0 {
                    break String::new();
                }
                read += n;
                let text = String::from_utf8_lossy(&buffer[..read]);
                if let Some(end) = text.find("\r\n") {
                    break text[..end].to_string();
                }
                if read >= buffer.len() {
                    break String::new();
                }
            };

            // 非回调路径（如浏览器探测 /favicon.ico）：回 404 继续等。
            let target = request_line.split_whitespace().nth(1).unwrap_or_default();
            if !target.starts_with("/callback") {
                let _ = stream
                    .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
                let _ = stream.shutdown().await;
                continue;
            }

            let params = parse_callback_query(&request_line);
            let (status, body) = if params.get("state").map(String::as_str) != Some(expected_state) {
                (
                    "400 Bad Request",
                    "<html><body><p>授权失败：state 校验未通过，可关闭此窗口。</p></body></html>",
                )
            } else if let Some(error) = params.get("error") {
                eprintln!("OAuth callback error: {error}");
                (
                    "400 Bad Request",
                    "<html><body><p>授权被拒绝，可关闭此窗口。</p></body></html>",
                )
            } else if params.get("code").is_some() {
                (
                    "200 OK",
                    "<html><body><p>授权完成，可关闭此窗口返回 Kivio。</p></body></html>",
                )
            } else {
                (
                    "400 Bad Request",
                    "<html><body><p>授权失败：未收到授权码，可关闭此窗口。</p></body></html>",
                )
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;

            if params.get("state").map(String::as_str) != Some(expected_state) {
                return Err("OAuth callback state mismatch".to_string());
            }
            if let Some(error) = params.get("error") {
                return Err(format!("OAuth authorization denied: {error}"));
            }
            match params.get("code") {
                Some(code) if !code.is_empty() => return Ok(code.clone()),
                _ => return Err("OAuth callback missing authorization code".to_string()),
            }
        }
    })
    .await;
    match result {
        Ok(inner) => inner,
        Err(_) => Err("Timed out waiting for OAuth authorization in the browser".to_string()),
    }
}

/// authorization_code 换 token。
#[allow(clippy::too_many_arguments)]
async fn exchange_code(
    http: &reqwest::Client,
    token_endpoint: &str,
    code: &str,
    redirect_uri: &str,
    client_id: &str,
    code_verifier: &str,
    resource: &str,
) -> Result<TokenResponse, String> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", code_verifier),
        // 见 `canonical_resource_indicator`：授权请求与 token 请求**两处都**要带。
        ("resource", resource),
    ];
    let response = timeout(
        DISCOVERY_TIMEOUT,
        http.post(token_endpoint).form(&form).send(),
    )
    .await
    .map_err(|_| "Token exchange timed out".to_string())?
    .map_err(|err| format!("Token exchange failed: {err}"))?;
    let status = response.status();
    let value = response
        .json::<serde_json::Value>()
        .await
        .map_err(|err| format!("Failed to parse token response: {err}"))?;
    if !status.is_success() {
        return Err(format!(
            "Token exchange rejected ({}): {}",
            status.as_u16(),
            value
        ));
    }
    parse_token_response(&value)
}

/// 用 bearer token GET OIDC userinfo，尽力取 email/name 当账户标识。
/// 带超时；任何失败（网络/非 2xx/无字段）都返回 None，绝不影响连接流程。
async fn fetch_userinfo_account(
    http: &reqwest::Client,
    userinfo_endpoint: &str,
    access_token: &str,
) -> Option<String> {
    let response = timeout(
        DISCOVERY_TIMEOUT,
        http.get(userinfo_endpoint).bearer_auth(access_token).send(),
    )
    .await
    .ok()?
    .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let value = timeout(DISCOVERY_TIMEOUT, response.json::<serde_json::Value>())
        .await
        .ok()?
        .ok()?;
    extract_account(&value)
}
pub async fn refresh_access_token(
    http: &reqwest::Client,
    token_endpoint: &str,
    refresh_token: &str,
    client_id: Option<&str>,
    // 该 MCP 服务器的 URL（RFC 8707 resource indicator）。拿不到时传 `None`，
    // 但正常路径上应该总是有 —— 见 `canonical_resource_indicator`。
    resource_url: Option<&str>,
) -> Result<TokenResponse, String> {
    let resource = resource_url.and_then(|url| canonical_resource_indicator(url).ok());
    let form = build_refresh_form(refresh_token, client_id, resource.as_deref());
    let response = timeout(
        DISCOVERY_TIMEOUT,
        http.post(token_endpoint).form(&form).send(),
    )
    .await
    .map_err(|_| "Token refresh timed out".to_string())?
    .map_err(|err| format!("Token refresh failed: {err}"))?;
    let status = response.status();
    let value = response
        .json::<serde_json::Value>()
        .await
        .map_err(|err| format!("Failed to parse refresh response: {err}"))?;
    if !status.is_success() {
        return Err(format!(
            "Token refresh rejected ({}): {}",
            status.as_u16(),
            value
        ));
    }
    parse_token_response(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc7636_test_vector() {
        // RFC 7636 Appendix B 的官方测试向量。
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let expected = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert_eq!(pkce_challenge(verifier), expected);
    }

    #[test]
    fn generated_verifier_is_within_length_and_charset() {
        let pair = generate_pkce();
        assert!(pair.verifier.len() >= 43 && pair.verifier.len() <= 128);
        assert!(pair
            .verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~')));
        assert_eq!(pkce_challenge(&pair.verifier), pair.challenge);
    }

    #[test]
    fn parses_authorization_server_from_metadata() {
        let value = serde_json::json!({
            "authorization_servers": ["https://auth.example.com/"]
        });
        assert_eq!(
            parse_authorization_server(&value),
            Some("https://auth.example.com".to_string())
        );
        assert_eq!(parse_authorization_server(&serde_json::json!({})), None);
    }

    #[test]
    fn parses_auth_server_metadata() {
        let value = serde_json::json!({
            "authorization_endpoint": "https://auth.example.com/authorize",
            "token_endpoint": "https://auth.example.com/token",
            "registration_endpoint": "https://auth.example.com/register",
            "scopes_supported": ["read", "write"],
        });
        let meta = parse_auth_server_metadata(&value).expect("metadata");
        assert_eq!(
            meta.authorization_endpoint,
            "https://auth.example.com/authorize"
        );
        assert_eq!(meta.token_endpoint, "https://auth.example.com/token");
        assert_eq!(
            meta.registration_endpoint.as_deref(),
            Some("https://auth.example.com/register")
        );
        assert_eq!(meta.scopes_supported, vec!["read", "write"]);
        // 缺 token_endpoint → None。
        assert!(parse_auth_server_metadata(&serde_json::json!({
            "authorization_endpoint": "https://x/authorize"
        }))
        .is_none());
    }

    #[test]
    fn splits_origin_and_path() {
        assert_eq!(
            split_origin_and_path("https://mcp.notion.com/mcp").unwrap(),
            ("https://mcp.notion.com".to_string(), "/mcp".to_string())
        );
        assert_eq!(
            split_origin_and_path("https://example.com:8443/a/b/").unwrap(),
            ("https://example.com:8443".to_string(), "/a/b".to_string())
        );
        assert!(split_origin_and_path("not a url").is_err());
    }

    #[test]
    fn protected_resource_urls_prefer_path_variant() {
        let urls = protected_resource_well_known_urls("https://mcp.notion.com", "/mcp");
        assert_eq!(
            urls,
            vec![
                "https://mcp.notion.com/.well-known/oauth-protected-resource/mcp".to_string(),
                "https://mcp.notion.com/.well-known/oauth-protected-resource".to_string(),
            ]
        );
        // 无 path 时只返回根候选。
        let urls = protected_resource_well_known_urls("https://x.com", "");
        assert_eq!(urls.len(), 1);
    }

    #[test]
    fn builds_authorize_url_with_pkce_and_scope() {
        let url = build_authorize_url(
            "https://auth.example.com/authorize",
            "client-123",
            "http://127.0.0.1:5555/callback",
            "state-xyz",
            "challenge-abc",
            &["read".to_string(), "write".to_string()],
            "https://mcp.example.com/mcp",
        )
        .unwrap();
        assert!(url.contains("response_type=code"));
        // RFC 8707：规范要求无条件带 resource，缺了它 token 就没有 audience 绑定。
        assert!(
            url.contains("resource=https%3A%2F%2Fmcp.example.com%2Fmcp"),
            "{url}"
        );
        assert!(url.contains("client_id=client-123"));
        assert!(url.contains("code_challenge=challenge-abc"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=state-xyz"));
        // scope 空格用 %20 或 + 编码。
        assert!(url.contains("scope=read"));
    }

    #[test]
    fn parses_callback_query() {
        let params = parse_callback_query("GET /callback?code=abc123&state=xyz HTTP/1.1");
        assert_eq!(params.get("code").map(String::as_str), Some("abc123"));
        assert_eq!(params.get("state").map(String::as_str), Some("xyz"));
        // url 编码解码。
        let params = parse_callback_query("GET /callback?code=a%2Bb%20c&state=s HTTP/1.1");
        assert_eq!(params.get("code").map(String::as_str), Some("a+b c"));
        // 无 query。
        assert!(parse_callback_query("GET /callback HTTP/1.1").is_empty());
    }

    #[test]
    fn computes_expires_at() {
        assert_eq!(compute_expires_at(1000, Some(3600)), Some(4600));
        assert_eq!(compute_expires_at(1000, None), None);
    }

    fn oauth_auth(refresh: Option<&str>, expires_at: Option<i64>) -> ConnectorAuth {
        ConnectorAuth {
            kind: "oauth".to_string(),
            access_token: "at".to_string(),
            refresh_token: refresh.map(|s| s.to_string()),
            expires_at,
            token_endpoint: Some("https://auth.example.com/token".to_string()),
            client_id: Some("client-1".to_string()),
            scopes: Vec::new(),
            account: None,
        }
    }

    #[test]
    fn needs_refresh_only_when_expiring_with_refresh_token() {
        let now = 1000;
        // 已过期 → 需要刷新。
        assert!(needs_refresh(
            &oauth_auth(Some("rt"), Some(900)),
            now,
            REFRESH_LEEWAY_SECS
        ));
        // leeway 内将过期 → 需要刷新。
        assert!(needs_refresh(
            &oauth_auth(Some("rt"), Some(1030)),
            now,
            REFRESH_LEEWAY_SECS
        ));
        // 远未过期 → 不刷新。
        assert!(!needs_refresh(
            &oauth_auth(Some("rt"), Some(99999)),
            now,
            REFRESH_LEEWAY_SECS
        ));
        // 无 refresh_token → 不刷新。
        assert!(!needs_refresh(
            &oauth_auth(None, Some(900)),
            now,
            REFRESH_LEEWAY_SECS
        ));
        // 无 expires_at → 不刷新（保守用旧 token）。
        assert!(!needs_refresh(
            &oauth_auth(Some("rt"), None),
            now,
            REFRESH_LEEWAY_SECS
        ));
        // token 类（非 oauth）→ 不刷新。
        let mut token = oauth_auth(Some("rt"), Some(900));
        token.kind = "token".to_string();
        assert!(!needs_refresh(&token, now, REFRESH_LEEWAY_SECS));
    }

    #[test]
    fn builds_refresh_form() {
        let form = build_refresh_form(
            "rt-1",
            Some("client-1"),
            Some("https://mcp.example.com/mcp"),
        );
        assert!(form.contains(&("grant_type".to_string(), "refresh_token".to_string())));
        assert!(form.contains(&("refresh_token".to_string(), "rt-1".to_string())));
        assert!(form.contains(&("client_id".to_string(), "client-1".to_string())));
        // 刷新同样要带 resource，否则新 token 又丢了 audience 绑定。
        assert!(form.contains(&(
            "resource".to_string(),
            "https://mcp.example.com/mcp".to_string()
        )));
        // 无 client_id / 无 resource 时省略对应字段。
        let form = build_refresh_form("rt-1", None, None);
        assert!(!form.iter().any(|(k, _)| k == "client_id"));
        assert!(!form.iter().any(|(k, _)| k == "resource"));
    }

    #[test]
    fn resource_indicator_drops_fragment() {
        // RFC 9728 明确禁止 fragment。
        assert_eq!(
            canonical_resource_indicator("https://mcp.example.com/mcp#frag").unwrap(),
            "https://mcp.example.com/mcp"
        );
        // host 小写是**显式**做的，不靠 url crate 顺带（注释与实现必须对得上）。
        assert_eq!(
            canonical_resource_indicator("https://MCP.Example.COM/MCP").unwrap(),
            "https://mcp.example.com/MCP",
            "host 要小写，path 不能动"
        );
        assert!(canonical_resource_indicator("not a url").is_err());
    }

    #[test]
    fn parses_token_response() {
        let value = serde_json::json!({
            "access_token": "at-1",
            "refresh_token": "rt-1",
            "expires_in": 3600,
            "scope": "read write",
        });
        let token = parse_token_response(&value).expect("token");
        assert_eq!(token.access_token, "at-1");
        assert_eq!(token.refresh_token.as_deref(), Some("rt-1"));
        assert_eq!(token.expires_in, Some(3600));
        assert_eq!(token.scope.as_deref(), Some("read write"));
        // 缺 access_token → Err（带 error 字段）。
        let err = parse_token_response(&serde_json::json!({ "error": "invalid_grant" }))
            .expect_err("should error");
        assert!(err.contains("invalid_grant"));
    }

    #[test]
    fn materializes_server_with_oauth_auth() {
        let token = TokenResponse {
            access_token: "at-1".to_string(),
            refresh_token: Some("rt-1".to_string()),
            expires_in: Some(3600),
            scope: None,
            account: Some("acme-workspace".to_string()),
        };
        let server = materialize_server(
            "notion",
            "Notion",
            "https://mcp.notion.com/mcp",
            &token,
            "https://auth.example.com/token",
            "client-1",
            &["read".to_string()],
            1000,
        );
        assert_eq!(server.id, "connector-notion");
        assert_eq!(server.connector_id.as_deref(), Some("notion"));
        assert_eq!(server.transport, "streamable_http");
        assert_eq!(
            server.headers.get("Authorization").map(String::as_str),
            Some("Bearer at-1")
        );
        let auth = server.auth.expect("auth");
        assert_eq!(auth.kind, "oauth");
        assert_eq!(auth.refresh_token.as_deref(), Some("rt-1"));
        assert_eq!(auth.expires_at, Some(4600));
        assert_eq!(
            auth.token_endpoint.as_deref(),
            Some("https://auth.example.com/token")
        );
        // scope 缺省时回退到请求的 scopes。
        assert_eq!(auth.scopes, vec!["read"]);
        // account 从 token 响应透传。
        assert_eq!(auth.account.as_deref(), Some("acme-workspace"));
    }

    #[test]
    fn extracts_account_from_notion_owner_shape() {
        // Notion OAuth：owner.user.person.email 优先于工作区名。
        let value = serde_json::json!({
            "access_token": "at",
            "workspace_name": "Acme Workspace",
            "owner": {
                "user": {
                    "name": "Jane Doe",
                    "person": { "email": "jane@acme.com" }
                }
            }
        });
        assert_eq!(extract_account(&value).as_deref(), Some("jane@acme.com"));

        // 无 email 时回退到工作区名。
        let value = serde_json::json!({
            "workspace_name": "Acme Workspace",
            "owner": { "user": { "name": "Jane Doe" } }
        });
        assert_eq!(extract_account(&value).as_deref(), Some("Acme Workspace"));

        // 无 email、无工作区名时回退到用户名。
        let value = serde_json::json!({
            "owner": { "user": { "name": "Jane Doe" } }
        });
        assert_eq!(extract_account(&value).as_deref(), Some("Jane Doe"));
    }

    #[test]
    fn extracts_account_from_flat_email() {
        // 扁平 OIDC userinfo 形态：顶层 email。
        let value = serde_json::json!({ "email": "bob@example.com", "name": "Bob" });
        assert_eq!(extract_account(&value).as_deref(), Some("bob@example.com"));
        // 仅 name。
        let value = serde_json::json!({ "name": "Bob" });
        assert_eq!(extract_account(&value).as_deref(), Some("Bob"));
        // 仅 account 字段。
        let value = serde_json::json!({ "account": "team-x" });
        assert_eq!(extract_account(&value).as_deref(), Some("team-x"));
    }

    #[test]
    fn extracts_account_missing_returns_none() {
        // 无任何账户线索 → None（绝不回退成端点 URL）。
        let value = serde_json::json!({ "access_token": "at", "expires_in": 3600 });
        assert_eq!(extract_account(&value), None);
        // 空字符串视为缺失。
        let value = serde_json::json!({ "email": "  ", "workspace_name": "" });
        assert_eq!(extract_account(&value), None);
    }

    #[test]
    fn parse_token_response_includes_account() {
        let value = serde_json::json!({
            "access_token": "at-1",
            "workspace_name": "Acme",
        });
        let token = parse_token_response(&value).expect("token");
        assert_eq!(token.account.as_deref(), Some("Acme"));
    }
}
