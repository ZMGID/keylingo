use serde::{Deserialize, Serialize};

use crate::{
    api::{send_with_retry, with_standard_request_timeout},
    settings::{ChatMcpServer, LensWebSearchConfig, WebSearchProvider},
    state::AppState,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    pub content: String,
    pub published_date: Option<String>,
    pub score: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct TavilySearchResponse {
    #[serde(default)]
    answer: Option<String>,
    #[serde(default)]
    results: Vec<TavilySearchResult>,
}

#[derive(Debug, Deserialize)]
struct TavilySearchResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    score: Option<f64>,
    #[serde(default)]
    published_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExaSearchResponse {
    #[serde(default)]
    results: Vec<ExaSearchResult>,
}

#[derive(Debug, Deserialize)]
struct OllamaSearchResponse {
    #[serde(default)]
    results: Vec<OllamaSearchResult>,
}

#[derive(Debug, Deserialize)]
struct OllamaSearchResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
}

#[derive(Debug, Deserialize)]
struct BraveSearchResponse {
    #[serde(default)]
    web: Option<BraveWeb>,
}

#[derive(Debug, Deserialize)]
struct BraveWeb {
    #[serde(default)]
    results: Vec<BraveSearchResult>,
}

#[derive(Debug, Deserialize)]
struct BraveSearchResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    extra_snippets: Vec<String>,
    #[serde(default)]
    page_age: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SerperSearchResponse {
    #[serde(default)]
    answer_box: Option<SerperAnswerBox>,
    #[serde(default)]
    organic: Vec<SerperOrganic>,
}

#[derive(Debug, Deserialize)]
struct SerperAnswerBox {
    #[serde(default)]
    answer: Option<String>,
    #[serde(default)]
    snippet: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    link: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SerperOrganic {
    #[serde(default)]
    title: String,
    #[serde(default)]
    link: String,
    #[serde(default)]
    snippet: String,
    #[serde(default)]
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BochaSearchResponse {
    #[serde(default)]
    data: Option<BochaSearchData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BochaSearchData {
    #[serde(default)]
    web_pages: Option<BochaWebPages>,
}

#[derive(Debug, Deserialize)]
struct BochaWebPages {
    #[serde(default)]
    value: Vec<BochaPage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BochaPage {
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    snippet: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    date_published: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ZhipuSearchResponse {
    #[serde(default)]
    search_result: Vec<ZhipuSearchResult>,
}

#[derive(Debug, Deserialize)]
struct ZhipuSearchResult {
    #[serde(default)]
    title: String,
    #[serde(default, alias = "url")]
    link: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    publish_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearxngSearchResponse {
    #[serde(default)]
    results: Vec<SearxngResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearxngResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    published_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TinyfishSearchResponse {
    #[serde(default)]
    results: Vec<TinyfishSearchResult>,
}

#[derive(Debug, Deserialize)]
struct TinyfishSearchResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    snippet: String,
    #[serde(default)]
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExaSearchResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    highlights: Vec<String>,
    #[serde(default)]
    score: Option<f64>,
    #[serde(default)]
    published_date: Option<String>,
}

/// 搜索服务的显示名，供前端工具卡片标注「用了哪个搜索服务」。
pub fn provider_label(provider: WebSearchProvider) -> &'static str {
    match provider {
        WebSearchProvider::Tavily => "Tavily",
        WebSearchProvider::Exa => "Exa",
        WebSearchProvider::ExaMcp => "Exa MCP",
        WebSearchProvider::Ollama => "Ollama",
        WebSearchProvider::Grok => "Grok",
        WebSearchProvider::Brave => "Brave",
        WebSearchProvider::Serper => "Serper",
        WebSearchProvider::Bocha => "Bocha",
        WebSearchProvider::Zhipu => "Zhipu",
        WebSearchProvider::Tinyfish => "TinyFish",
        WebSearchProvider::TinyfishMcp => "TinyFish MCP",
        WebSearchProvider::Searxng => "SearXNG",
        WebSearchProvider::Unknown => "Web",
    }
}

/// 用户可配置的搜索 API base：去尾斜杠；留空回退到官方默认（settings 的 serde default
/// 已给默认值，这里兜「用户手动清空输入框」的情况）。
fn normalized_base_url<'a>(configured: &'a str, fallback: &'a str) -> &'a str {
    let trimmed = configured.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        fallback
    } else {
        trimmed
    }
}

fn with_query(base: &str, pairs: &[(&str, &str)]) -> String {
    let qs = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs.iter().copied())
        .finish();
    if qs.is_empty() {
        return base.to_string();
    }
    if base.contains('?') {
        format!("{base}&{qs}")
    } else {
        format!("{base}?{qs}")
    }
}

pub async fn search_web(
    state: &AppState,
    config: &LensWebSearchConfig,
    query: &str,
    retry_attempts: usize,
) -> Result<Vec<WebSearchResult>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }

    match config.provider {
        WebSearchProvider::Tavily => search_tavily(state, config, query, retry_attempts).await,
        WebSearchProvider::Exa => search_exa(state, config, query, retry_attempts).await,
        WebSearchProvider::ExaMcp => search_exa_mcp(state, config, query).await,
        WebSearchProvider::Ollama => search_ollama(state, config, query, retry_attempts).await,
        WebSearchProvider::Grok => search_grok(state, config, query, retry_attempts).await,
        WebSearchProvider::Brave => search_brave(state, config, query, retry_attempts).await,
        WebSearchProvider::Serper => search_serper(state, config, query, retry_attempts).await,
        WebSearchProvider::Bocha => search_bocha(state, config, query, retry_attempts).await,
        WebSearchProvider::Zhipu => search_zhipu(state, config, query, retry_attempts).await,
        WebSearchProvider::Tinyfish => search_tinyfish(state, config, query, retry_attempts).await,
        WebSearchProvider::TinyfishMcp => search_tinyfish_mcp(state, config, query).await,
        WebSearchProvider::Searxng => search_searxng(state, config, query, retry_attempts).await,
        WebSearchProvider::Unknown => {
            Err("Selected web search provider is not supported yet".to_string())
        }
    }
}

async fn read_search_json<T: serde::de::DeserializeOwned>(
    label: &str,
    response: reqwest::Response,
) -> Result<T, String> {
    let raw = response
        .text()
        .await
        .map_err(|err| format!("{label} read body: {err}"))?;
    serde_json::from_str(&raw).map_err(|err| {
        format!(
            "{label} parse JSON: {} (body: {})",
            err,
            raw.chars().take(500).collect::<String>()
        )
    })
}

/// Ollama Web Search（Ollama Cloud）：`POST https://ollama.com/api/web_search`，
/// Bearer key，body `{query, max_results}`，返回 `{results:[{title,url,content}]}`。
async fn search_ollama(
    state: &AppState,
    config: &LensWebSearchConfig,
    query: &str,
    retry_attempts: usize,
) -> Result<Vec<WebSearchResult>, String> {
    let api_key = config.ollama_api_key.trim();
    if api_key.is_empty() {
        return Err("Ollama API key is not configured".to_string());
    }

    let max_results = config.max_results.clamp(1, 10);
    let body = serde_json::json!({
        "query": query,
        "max_results": max_results,
    });

    // 自定义 base（默认 https://ollama.com）：代理/自建网关场景可改。
    let base = normalized_base_url(&config.ollama_base_url, "https://ollama.com");
    let url = format!("{base}/api/web_search");
    let response = send_with_retry("Ollama search", retry_attempts, || {
        with_standard_request_timeout(state.http.post(&url).bearer_auth(api_key).json(&body)).send()
    })
    .await?;

    let raw = response
        .text()
        .await
        .map_err(|err| format!("Ollama search read body: {err}"))?;
    let parsed: OllamaSearchResponse = serde_json::from_str(&raw).map_err(|err| {
        format!(
            "Ollama search parse JSON: {} (body: {})",
            err,
            raw.chars().take(500).collect::<String>()
        )
    })?;

    Ok(parsed
        .results
        .into_iter()
        .filter(|result| !result.url.trim().is_empty())
        .map(|result| WebSearchResult {
            title: result.title.trim().to_string(),
            url: result.url.trim().to_string(),
            content: result.content.trim().to_string(),
            published_date: None,
            score: None,
        })
        .collect())
}

/// Exa MCP 搜索：调用 Exa 官方 MCP 服务器（默认 https://mcp.exa.ai/mcp）的
/// `web_search_exa` 工具，复用通用的 Streamable HTTP MCP 客户端。API Key 走
/// `?exaApiKey=` 查询参数（Exa MCP 的约定），无 key 也可低配额试用。
async fn search_exa_mcp(
    state: &AppState,
    config: &LensWebSearchConfig,
    query: &str,
) -> Result<Vec<WebSearchResult>, String> {
    let base = config.exa_mcp_url.trim();
    if base.is_empty() {
        return Err("Exa MCP endpoint is not configured".to_string());
    }
    let api_key = config.exa_api_key.trim();
    let url = if api_key.is_empty() {
        base.to_string()
    } else if base.contains('?') {
        format!("{base}&exaApiKey={api_key}")
    } else {
        format!("{base}?exaApiKey={api_key}")
    };

    let server = ChatMcpServer {
        id: "exa-mcp".to_string(),
        name: "Exa MCP".to_string(),
        enabled: true,
        transport: "streamable_http".to_string(),
        url,
        ..ChatMcpServer::default()
    };
    // 一次性连接，不进 MCP 连接池：这个 server 是临时合成的（api key 就在 URL 里），
    // 不该被当成用户配置的常驻服务器缓存起来。
    let max_results = config.max_results.clamp(1, 10);
    let raw = crate::mcp::conn::call_tool_once(
        &server,
        &state.http,
        "web_search_exa",
        serde_json::json!({ "query": query, "numResults": max_results }),
        std::time::Duration::from_secs(30),
    )
    .await?;
    let result = crate::mcp::result::parse_tool_result(
        serde_json::to_value(&raw).map_err(|err| err.to_string())?,
    );
    if result.is_error {
        return Err(format!("Exa MCP search failed: {}", result.content));
    }

    Ok(parse_exa_mcp_results(&result.content, max_results as usize))
}

/// Exa MCP 的工具返回体是一段文本，通常内嵌 JSON（`{ "results": [...] }`）。
/// 尽量结构化解析；解析失败时把整段文本作为单条结果返回，保证至少有可用内容。
fn parse_exa_mcp_results(content: &str, max_results: usize) -> Vec<WebSearchResult> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Ok(parsed) = serde_json::from_str::<ExaSearchResponse>(trimmed) {
        let results: Vec<WebSearchResult> = parsed
            .results
            .into_iter()
            .filter(|result| !result.url.trim().is_empty())
            .map(|result| {
                let content = if !result.highlights.is_empty() {
                    result.highlights.join("\n")
                } else if !result.summary.trim().is_empty() {
                    result.summary
                } else {
                    result.text
                };
                WebSearchResult {
                    title: result.title.trim().to_string(),
                    url: result.url.trim().to_string(),
                    content: content.trim().to_string(),
                    published_date: result.published_date,
                    score: result.score,
                }
            })
            .take(max_results)
            .collect();
        if !results.is_empty() {
            return results;
        }
    }
    // Exa MCP 的 web_search_exa 实际返回一段格式化文本：多条结果以单独一行 `---` 分隔，
    // 每条含 `Title:` / `URL:` / `Published:` / `Author:` / `Highlights:` 头 + 正文。
    let text_results = parse_exa_mcp_text_blocks(trimmed, max_results);
    if !text_results.is_empty() {
        return text_results;
    }
    vec![WebSearchResult {
        title: "Exa MCP result".to_string(),
        url: "https://mcp.exa.ai/mcp".to_string(),
        content: trimmed.chars().take(4000).collect(),
        published_date: None,
        score: None,
    }]
}

/// 解析 Exa MCP 的文本结果块（见 parse_exa_mcp_results 说明）。
fn parse_exa_mcp_text_blocks(text: &str, max_results: usize) -> Vec<WebSearchResult> {
    let mut results: Vec<WebSearchResult> = Vec::new();
    let lines: Vec<&str> = text.split('\n').collect();
    // 以单独一行的 `---`（或更长的连字符）分块。
    for block in lines.split(|line| {
        let t = line.trim();
        t.len() >= 3 && t.chars().all(|c| c == '-')
    }) {
        if results.len() >= max_results {
            break;
        }
        let mut title = String::new();
        let mut url = String::new();
        let mut published: Option<String> = None;
        let mut body: Vec<String> = Vec::new();
        let mut in_highlights = false;
        for &line in block {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("Title:") {
                title = rest.trim().to_string();
            } else if let Some(rest) = trimmed.strip_prefix("URL:") {
                url = rest.trim().to_string();
            } else if let Some(rest) = trimmed.strip_prefix("Published:") {
                let v = rest.trim();
                if !v.is_empty() && v != "N/A" {
                    published = Some(v.to_string());
                }
            } else if trimmed.starts_with("Author:") {
                // 忽略作者行
            } else if trimmed.starts_with("Highlights:") {
                in_highlights = true;
            } else if in_highlights && trimmed != "..." && !trimmed.is_empty() {
                body.push(trimmed.to_string());
            }
        }
        if url.is_empty() {
            continue;
        }
        let content: String = body.join("\n").chars().take(1500).collect();
        results.push(WebSearchResult {
            title,
            url,
            content: content.trim().to_string(),
            published_date: published,
            score: None,
        });
    }
    results
}

/// Brave Search API：`GET {base}/res/v1/web/search?q=&count=`，
/// 鉴权头 `X-Subscription-Token`。
async fn search_brave(
    state: &AppState,
    config: &LensWebSearchConfig,
    query: &str,
    retry_attempts: usize,
) -> Result<Vec<WebSearchResult>, String> {
    let api_key = config.brave_api_key.trim();
    if api_key.is_empty() {
        return Err("Brave API key is not configured".to_string());
    }
    let max_results = config.max_results.clamp(1, 10);
    let count = max_results.to_string();
    let base = normalized_base_url(&config.brave_base_url, "https://api.search.brave.com");
    let url = with_query(
        &format!("{base}/res/v1/web/search"),
        &[("q", query), ("count", &count)],
    );
    let response = send_with_retry("Brave search", retry_attempts, || {
        with_standard_request_timeout(
            state
                .http
                .get(&url)
                .header("X-Subscription-Token", api_key)
                .header("Accept", "application/json"),
        )
        .send()
    })
    .await?;
    let parsed: BraveSearchResponse = read_search_json("Brave search", response).await?;
    Ok(parsed
        .web
        .unwrap_or(BraveWeb {
            results: Vec::new(),
        })
        .results
        .into_iter()
        .filter(|result| !result.url.trim().is_empty())
        .map(|result| {
            let mut content = result.description.trim().to_string();
            if !result.extra_snippets.is_empty() {
                let extra = result.extra_snippets.join("\n");
                if content.is_empty() {
                    content = extra;
                } else {
                    content = format!("{content}\n{extra}");
                }
            }
            WebSearchResult {
                title: result.title.trim().to_string(),
                url: result.url.trim().to_string(),
                content,
                published_date: result.page_age.filter(|age| !age.trim().is_empty()),
                score: None,
            }
        })
        .collect())
}

/// TinyFish Search：`GET {base}?query=`，头 `X-API-Key`，结果在 `results[]`。
/// 官方 endpoint 就是根路径（https://api.search.tinyfish.ai），没有 /search 后缀。
/// Search API 目前不按条数计费；官方没有 count 参数，结果条数在客户端截断。
async fn search_tinyfish(
    state: &AppState,
    config: &LensWebSearchConfig,
    query: &str,
    retry_attempts: usize,
) -> Result<Vec<WebSearchResult>, String> {
    let api_key = config.tinyfish_api_key.trim();
    if api_key.is_empty() {
        return Err("TinyFish API key is not configured".to_string());
    }
    let max_results = config.max_results.clamp(1, 10) as usize;
    let base = normalized_base_url(&config.tinyfish_base_url, "https://api.search.tinyfish.ai");
    let url = with_query(base, &[("query", query)]);
    let response = send_with_retry("TinyFish search", retry_attempts, || {
        with_standard_request_timeout(
            state
                .http
                .get(&url)
                .header("X-API-Key", api_key)
                .header("Accept", "application/json"),
        )
        .send()
    })
    .await?;
    let parsed: TinyfishSearchResponse = read_search_json("TinyFish search", response).await?;
    Ok(tinyfish_web_results(parsed.results, max_results))
}

/// TinyFish MCP：`https://agent.tinyfish.ai/mcp` 的 `search` 工具。
/// 不贴 API Key，走 OAuth Bearer（设置页授权，或复用已连接的同 URL MCP server）。
async fn search_tinyfish_mcp(
    state: &AppState,
    config: &LensWebSearchConfig,
    query: &str,
) -> Result<Vec<WebSearchResult>, String> {
    let base = normalized_base_url(&config.tinyfish_mcp_url, "https://agent.tinyfish.ai/mcp");
    if base.is_empty() {
        return Err("TinyFish MCP endpoint is not configured".to_string());
    }
    let authorization = tinyfish_mcp_authorization(state, config, base);
    let mut server = ChatMcpServer {
        id: "tinyfish-mcp".to_string(),
        name: "TinyFish MCP".to_string(),
        enabled: true,
        transport: "streamable_http".to_string(),
        url: base.to_string(),
        ..ChatMcpServer::default()
    };
    if let Some(value) = authorization {
        server.headers.insert("Authorization".to_string(), value);
    }

    let max_results = config.max_results.clamp(1, 10) as usize;
    let raw = crate::mcp::conn::call_tool_once(
        &server,
        &state.http,
        "search",
        serde_json::json!({ "query": query }),
        std::time::Duration::from_secs(30),
    )
    .await
    .map_err(|err| map_tinyfish_mcp_error(&err))?;
    let result = crate::mcp::result::parse_tool_result(
        serde_json::to_value(&raw).map_err(|err| err.to_string())?,
    );
    if result.is_error {
        return Err(map_tinyfish_mcp_error(&format!(
            "TinyFish MCP search failed: {}",
            result.content
        )));
    }

    Ok(parse_tinyfish_mcp_payload(
        &result.content,
        result.structured_content.as_ref(),
        max_results,
    ))
}

fn map_tinyfish_mcp_error(err: &str) -> String {
    let lower = err.to_ascii_lowercase();
    if err.contains("OAUTH_REQUIRED")
        || lower.contains("unauthorized")
        || lower.contains("valid oauth bearer")
    {
        "TinyFish MCP requires TinyFish account authorization in the browser (no API key). Open Settings → Web Search → TinyFish MCP and click Authorize.".to_string()
    } else {
        err.to_string()
    }
}

fn bearer_header(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    if token.len() >= 7 && token[..7].eq_ignore_ascii_case("bearer ") {
        Some(token.to_string())
    } else {
        Some(format!("Bearer {token}"))
    }
}

fn mcp_url_matches(left: &str, right: &str) -> bool {
    left.trim().trim_end_matches('/') == right.trim().trim_end_matches('/')
}

fn tinyfish_mcp_authorization(
    state: &AppState,
    config: &LensWebSearchConfig,
    endpoint: &str,
) -> Option<String> {
    if let Some(auth) = &config.tinyfish_mcp_auth {
        if let Some(header) = bearer_header(&auth.access_token) {
            return Some(header);
        }
    }
    let settings = state.settings_read();
    for server in &settings.chat_tools.servers {
        if !mcp_url_matches(&server.url, endpoint) {
            continue;
        }
        if let Some(value) = server
            .headers
            .get("Authorization")
            .and_then(|value| bearer_header(value))
        {
            return Some(value);
        }
        if let Some(auth) = &server.auth {
            if let Some(value) = bearer_header(&auth.access_token) {
                return Some(value);
            }
        }
    }
    None
}

fn tinyfish_web_results(
    results: Vec<TinyfishSearchResult>,
    max_results: usize,
) -> Vec<WebSearchResult> {
    results
        .into_iter()
        .filter(|result| !result.url.trim().is_empty())
        .take(max_results)
        .map(|result| WebSearchResult {
            title: result.title.trim().to_string(),
            url: result.url.trim().to_string(),
            content: result.snippet.trim().to_string(),
            published_date: result.date.filter(|date| !date.trim().is_empty()),
            score: None,
        })
        .collect()
}

fn parse_tinyfish_mcp_payload(
    content: &str,
    structured: Option<&serde_json::Value>,
    max_results: usize,
) -> Vec<WebSearchResult> {
    if let Some(value) = structured {
        let parsed = tinyfish_results_from_value(value, max_results);
        if !parsed.is_empty() {
            return parsed;
        }
    }
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        let parsed = tinyfish_results_from_value(&value, max_results);
        if !parsed.is_empty() {
            return parsed;
        }
    }
    vec![WebSearchResult {
        title: "TinyFish MCP result".to_string(),
        url: "https://agent.tinyfish.ai/mcp".to_string(),
        content: trimmed.chars().take(4000).collect(),
        published_date: None,
        score: None,
    }]
}

fn tinyfish_results_from_value(
    value: &serde_json::Value,
    max_results: usize,
) -> Vec<WebSearchResult> {
    if let Ok(parsed) = serde_json::from_value::<TinyfishSearchResponse>(value.clone()) {
        let results = tinyfish_web_results(parsed.results, max_results);
        if !results.is_empty() {
            return results;
        }
    }
    if let Ok(results) = serde_json::from_value::<Vec<TinyfishSearchResult>>(value.clone()) {
        return tinyfish_web_results(results, max_results);
    }
    Vec::new()
}

/// Serper：`POST {base}/search`，头 `X-API-KEY`，body `{q, num}`，结果在 `organic[]`。
async fn search_serper(
    state: &AppState,
    config: &LensWebSearchConfig,
    query: &str,
    retry_attempts: usize,
) -> Result<Vec<WebSearchResult>, String> {
    let api_key = config.serper_api_key.trim();
    if api_key.is_empty() {
        return Err("Serper API key is not configured".to_string());
    }
    let max_results = config.max_results.clamp(1, 10);
    let body = serde_json::json!({
        "q": query,
        "num": max_results,
    });
    let base = normalized_base_url(&config.serper_base_url, "https://google.serper.dev");
    let url = format!("{base}/search");
    let response = send_with_retry("Serper search", retry_attempts, || {
        with_standard_request_timeout(
            state
                .http
                .post(&url)
                .header("X-API-KEY", api_key)
                .json(&body),
        )
        .send()
    })
    .await?;
    let parsed: SerperSearchResponse = read_search_json("Serper search", response).await?;

    let mut results: Vec<WebSearchResult> = Vec::new();
    if let Some(box_) = parsed.answer_box {
        let content = box_
            .answer
            .as_deref()
            .or(box_.snippet.as_deref())
            .unwrap_or("")
            .trim()
            .to_string();
        let url = box_
            .link
            .as_deref()
            .unwrap_or("https://serper.dev")
            .trim()
            .to_string();
        if !content.is_empty() {
            results.push(WebSearchResult {
                title: box_
                    .title
                    .as_deref()
                    .unwrap_or("Serper answer")
                    .trim()
                    .to_string(),
                url,
                content,
                published_date: None,
                score: None,
            });
        }
    }
    results.extend(
        parsed
            .organic
            .into_iter()
            .filter(|result| !result.link.trim().is_empty())
            .map(|result| WebSearchResult {
                title: result.title.trim().to_string(),
                url: result.link.trim().to_string(),
                content: result.snippet.trim().to_string(),
                published_date: result.date.filter(|date| !date.trim().is_empty()),
                score: None,
            }),
    );
    Ok(results)
}

/// 博查 Web Search：`POST {base}/v1/web-search`，Bearer key，结果在 `data.webPages.value[]`。
async fn search_bocha(
    state: &AppState,
    config: &LensWebSearchConfig,
    query: &str,
    retry_attempts: usize,
) -> Result<Vec<WebSearchResult>, String> {
    let api_key = config.bocha_api_key.trim();
    if api_key.is_empty() {
        return Err("Bocha API key is not configured".to_string());
    }
    let max_results = config.max_results.clamp(1, 10);
    let body = serde_json::json!({
        "query": query,
        "count": max_results,
        "summary": true,
        "freshness": "noLimit",
    });
    let base = normalized_base_url(&config.bocha_base_url, "https://api.bochaai.com");
    let url = format!("{base}/v1/web-search");
    let response = send_with_retry("Bocha search", retry_attempts, || {
        with_standard_request_timeout(state.http.post(&url).bearer_auth(api_key).json(&body)).send()
    })
    .await?;
    let parsed: BochaSearchResponse = read_search_json("Bocha search", response).await?;
    let pages = parsed
        .data
        .and_then(|data| data.web_pages)
        .map(|pages| pages.value)
        .unwrap_or_default();
    Ok(pages
        .into_iter()
        .filter(|page| !page.url.trim().is_empty())
        .map(|page| {
            let content = if !page.summary.trim().is_empty() {
                page.summary
            } else {
                page.snippet
            };
            WebSearchResult {
                title: page.name.trim().to_string(),
                url: page.url.trim().to_string(),
                content: content.trim().to_string(),
                published_date: page.date_published.filter(|date| !date.trim().is_empty()),
                score: None,
            }
        })
        .collect())
}

/// 智谱 Web Search：`POST {base}/web_search`，Bearer key，结果在 `search_result[]`。
async fn search_zhipu(
    state: &AppState,
    config: &LensWebSearchConfig,
    query: &str,
    retry_attempts: usize,
) -> Result<Vec<WebSearchResult>, String> {
    let api_key = config.zhipu_api_key.trim();
    if api_key.is_empty() {
        return Err("Zhipu API key is not configured".to_string());
    }
    let max_results = config.max_results.clamp(1, 10);
    let body = serde_json::json!({
        "search_query": query,
        "search_engine": "search_std",
        "search_intent": false,
        "count": max_results,
        "content_size": "medium",
    });
    let base = normalized_base_url(
        &config.zhipu_base_url,
        "https://open.bigmodel.cn/api/paas/v4",
    );
    let url = format!("{base}/web_search");
    let response = send_with_retry("Zhipu search", retry_attempts, || {
        with_standard_request_timeout(state.http.post(&url).bearer_auth(api_key).json(&body)).send()
    })
    .await?;
    let parsed: ZhipuSearchResponse = read_search_json("Zhipu search", response).await?;
    Ok(parsed
        .search_result
        .into_iter()
        .filter(|result| !result.link.trim().is_empty())
        .map(|result| WebSearchResult {
            title: result.title.trim().to_string(),
            url: result.link.trim().to_string(),
            content: result.content.trim().to_string(),
            published_date: result.publish_date.filter(|date| !date.trim().is_empty()),
            score: None,
        })
        .collect())
}

fn searxng_search_url(configured: &str) -> Result<String, String> {
    let trimmed = configured.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("SearXNG instance URL is not configured".to_string());
    }
    if trimmed.ends_with("/search") {
        Ok(trimmed.to_string())
    } else {
        Ok(format!("{trimmed}/search"))
    }
}

/// SearXNG 自建实例：`GET {instance}/search?q=&format=json`，无需 API key。
/// 实例须在 settings.yml 开启 json 输出，公网实例多数默认关闭。
async fn search_searxng(
    state: &AppState,
    config: &LensWebSearchConfig,
    query: &str,
    retry_attempts: usize,
) -> Result<Vec<WebSearchResult>, String> {
    let url = searxng_search_url(&config.searxng_base_url)?;
    let max_results = config.max_results.clamp(1, 10) as usize;
    let form = [("q", query), ("format", "json")];
    let response = send_with_retry("SearXNG search", retry_attempts, || {
        with_standard_request_timeout(
            state
                .http
                .post(&url)
                .header("Accept", "application/json")
                .form(&form),
        )
        .send()
    })
    .await?;
    let parsed: SearxngSearchResponse = read_search_json("SearXNG search", response).await?;
    Ok(parsed
        .results
        .into_iter()
        .filter(|result| !result.url.trim().is_empty())
        .take(max_results)
        .map(|result| WebSearchResult {
            title: result.title.trim().to_string(),
            url: result.url.trim().to_string(),
            content: result.content.trim().to_string(),
            published_date: result.published_date.filter(|date| !date.trim().is_empty()),
            score: None,
        })
        .collect())
}

async fn search_tavily(
    state: &AppState,
    config: &LensWebSearchConfig,
    query: &str,
    retry_attempts: usize,
) -> Result<Vec<WebSearchResult>, String> {
    let api_key = config.tavily_api_key.trim();
    if api_key.is_empty() {
        return Err("Tavily API key is not configured".to_string());
    }

    let max_results = config.max_results.clamp(1, 10);
    let search_depth = match config.search_depth.as_str() {
        "ultra-fast" | "fast" | "basic" | "advanced" => config.search_depth.as_str(),
        _ => "basic",
    };
    let body = serde_json::json!({
        "query": query,
        "search_depth": search_depth,
        "max_results": max_results,
        "include_answer": true,
        "include_raw_content": false,
        "include_images": false,
        "include_favicon": false,
    });

    // 自定义 base（默认 https://api.tavily.com）：代理/自建网关场景可改。
    let base = normalized_base_url(&config.tavily_base_url, "https://api.tavily.com");
    let url = format!("{base}/search");
    let response = send_with_retry("Tavily search", retry_attempts, || {
        with_standard_request_timeout(state.http.post(&url).bearer_auth(api_key).json(&body)).send()
    })
    .await?;

    let raw = response
        .text()
        .await
        .map_err(|err| format!("Tavily search read body: {err}"))?;
    let parsed: TavilySearchResponse = serde_json::from_str(&raw).map_err(|err| {
        format!(
            "Tavily search parse JSON: {} (body: {})",
            err,
            raw.chars().take(500).collect::<String>()
        )
    })?;

    let mut results: Vec<WebSearchResult> = parsed
        .results
        .into_iter()
        .filter(|result| !result.url.trim().is_empty())
        .map(|result| WebSearchResult {
            title: result.title.trim().to_string(),
            url: result.url.trim().to_string(),
            content: result.content.trim().to_string(),
            published_date: result.published_date,
            score: result.score,
        })
        .collect();

    if let Some(answer) = parsed
        .answer
        .as_deref()
        .filter(|answer| !answer.trim().is_empty())
    {
        results.insert(
            0,
            WebSearchResult {
                title: "Tavily answer".to_string(),
                url: "https://api.tavily.com/search".to_string(),
                content: answer.trim().to_string(),
                published_date: None,
                score: None,
            },
        );
    }

    Ok(results)
}

async fn search_exa(
    state: &AppState,
    config: &LensWebSearchConfig,
    query: &str,
    retry_attempts: usize,
) -> Result<Vec<WebSearchResult>, String> {
    let api_key = config.exa_api_key.trim();
    if api_key.is_empty() {
        return Err("Exa API key is not configured".to_string());
    }

    let max_results = config.max_results.clamp(1, 10);
    let body = serde_json::json!({
        "query": query,
        "numResults": max_results,
        "contents": {
            "highlights": true
        }
    });

    // 自定义 base（默认 https://api.exa.ai）：代理/自建网关场景可改。
    let base = normalized_base_url(&config.exa_base_url, "https://api.exa.ai");
    let url = format!("{base}/search");
    let response = send_with_retry("Exa search", retry_attempts, || {
        with_standard_request_timeout(
            state
                .http
                .post(&url)
                .header("x-api-key", api_key)
                .json(&body),
        )
        .send()
    })
    .await?;

    let raw = response
        .text()
        .await
        .map_err(|err| format!("Exa search read body: {err}"))?;
    let parsed: ExaSearchResponse = serde_json::from_str(&raw).map_err(|err| {
        format!(
            "Exa search parse JSON: {} (body: {})",
            err,
            raw.chars().take(500).collect::<String>()
        )
    })?;

    Ok(parsed
        .results
        .into_iter()
        .filter(|result| !result.url.trim().is_empty())
        .map(|result| {
            let content = if !result.highlights.is_empty() {
                result.highlights.join("\n")
            } else if !result.summary.trim().is_empty() {
                result.summary
            } else {
                result.text
            };
            WebSearchResult {
                title: result.title.trim().to_string(),
                url: result.url.trim().to_string(),
                content: content.trim().to_string(),
                published_date: result.published_date,
                score: result.score,
            }
        })
        .collect())
}

/// Grok（xAI）模型驱动搜索：走 xAI 的 Responses API（`{base}/responses`）+ `web_search`
/// 工具，让模型自己联网并返回带引用的答案。旧版 chat completions 的 Live Search
/// (`search_parameters`) 已于 2026-01 停用，故用 Responses API。答案作为首条结果，
/// 引用 URL 追加为后续结果。
async fn search_grok(
    state: &AppState,
    config: &LensWebSearchConfig,
    query: &str,
    retry_attempts: usize,
) -> Result<Vec<WebSearchResult>, String> {
    let api_key = config.grok_api_key.trim();
    if api_key.is_empty() {
        return Err("Grok API key is not configured".to_string());
    }
    let base = config.grok_base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("Grok base URL is not configured".to_string());
    }
    let model = config.grok_model.trim();
    let system = config.grok_system_prompt.trim();
    let url = format!("{base}/responses");
    let body = serde_json::json!({
        "model": model,
        "input": [
            { "role": "system", "content": system },
            { "role": "user", "content": query },
        ],
        "tools": [ { "type": "web_search" } ],
    });

    let response = send_with_retry("Grok search", retry_attempts, || {
        with_standard_request_timeout(
            state
                .http
                .post(url.clone())
                .bearer_auth(api_key)
                .json(&body),
        )
        .send()
    })
    .await?;

    let raw = response
        .text()
        .await
        .map_err(|err| format!("Grok search read body: {err}"))?;
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|err| {
        format!(
            "Grok search parse JSON: {} (body: {})",
            err,
            raw.chars().take(500).collect::<String>()
        )
    })?;

    let (answer, citations) = parse_grok_response(&value);
    if answer.is_empty() && citations.is_empty() {
        return Err(format!(
            "Grok search returned no answer (body: {})",
            raw.chars().take(300).collect::<String>()
        ));
    }

    let mut results: Vec<WebSearchResult> = Vec::new();
    if !answer.is_empty() {
        results.push(WebSearchResult {
            title: "Grok answer".to_string(),
            url: citations
                .first()
                .cloned()
                .unwrap_or_else(|| "https://x.ai".to_string()),
            content: answer,
            published_date: None,
            score: None,
        });
    }
    let max_results = config.max_results.clamp(1, 10) as usize;
    for citation in citations.into_iter().take(max_results) {
        results.push(WebSearchResult {
            title: citation.clone(),
            url: citation,
            content: String::new(),
            published_date: None,
            score: None,
        });
    }
    Ok(results)
}

/// 从 xAI Responses API（或退化的 chat completions）返回体里尽力提取「答案文本」和
/// 「引用 URL 列表」。字段形态随版本变化，故做多路径兜底：
/// - 答案：`output_text` → `output[].content[].text`（type=output_text）→ `choices[0].message.content`
/// - 引用：顶层 `citations` 数组 → `output[].content[].annotations[].url`
fn parse_grok_response(value: &serde_json::Value) -> (String, Vec<String>) {
    let mut answer_parts: Vec<String> = Vec::new();
    let mut citations: Vec<String> = Vec::new();

    let mut push_citation = |url: &str| {
        let url = url.trim();
        if !url.is_empty() && !citations.iter().any(|c| c == url) {
            citations.push(url.to_string());
        }
    };

    // 顶层便捷字段
    if let Some(text) = value.get("output_text").and_then(|v| v.as_str()) {
        if !text.trim().is_empty() {
            answer_parts.push(text.trim().to_string());
        }
    }
    if let Some(list) = value.get("citations").and_then(|v| v.as_array()) {
        for item in list {
            if let Some(url) = item.as_str() {
                push_citation(url);
            } else if let Some(url) = item.get("url").and_then(|v| v.as_str()) {
                push_citation(url);
            }
        }
    }

    // Responses API：output[].content[]
    if let Some(output) = value.get("output").and_then(|v| v.as_array()) {
        for item in output {
            let Some(content) = item.get("content").and_then(|v| v.as_array()) else {
                continue;
            };
            for chunk in content {
                if let Some(text) = chunk.get("text").and_then(|v| v.as_str()) {
                    if !text.trim().is_empty() {
                        answer_parts.push(text.trim().to_string());
                    }
                }
                if let Some(annotations) = chunk.get("annotations").and_then(|v| v.as_array()) {
                    for annotation in annotations {
                        if let Some(url) = annotation.get("url").and_then(|v| v.as_str()) {
                            push_citation(url);
                        }
                    }
                }
            }
        }
    }

    // 退化：chat completions 形态
    if answer_parts.is_empty() {
        if let Some(text) = value
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|v| v.as_str())
        {
            if !text.trim().is_empty() {
                answer_parts.push(text.trim().to_string());
            }
        }
    }

    (answer_parts.join("\n\n"), citations)
}

/// Render web search results into the textual context block injected into the
/// model conversation: a two-line header, then per result a `[N] Title` line, a
/// `URL: …` line, and optional `Published:` / `Score:` / `Snippet:` lines.
pub fn format_web_context(results: &[WebSearchResult]) -> String {
    if results.is_empty() {
        return String::new();
    }

    let mut lines = Vec::with_capacity(results.len() * 5 + 4);
    lines.push("Web search context:".to_string());
    lines.push(
        "Use only these sources for current web facts. Cite sources with [1], [2], etc. If the sources are insufficient, say so."
            .to_string(),
    );

    for (idx, result) in results.iter().enumerate() {
        let title = if result.title.is_empty() {
            "Untitled"
        } else {
            result.title.as_str()
        };
        lines.push(format!("[{}] {}", idx + 1, title));
        lines.push(format!("URL: {}", result.url));
        if let Some(date) = result
            .published_date
            .as_deref()
            .filter(|d| !d.trim().is_empty())
        {
            lines.push(format!("Published: {}", date.trim()));
        }
        if let Some(score) = result.score {
            lines.push(format!("Score: {:.3}", score));
        }
        if !result.content.is_empty() {
            let snippet: String = result.content.chars().take(1200).collect();
            lines.push(format!("Snippet: {}", snippet));
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        format_web_context, parse_exa_mcp_results, parse_grok_response, parse_tinyfish_mcp_payload,
        searxng_search_url, BochaSearchResponse, BraveSearchResponse, ExaSearchResponse,
        OllamaSearchResponse, SearxngSearchResponse, SerperSearchResponse, TavilySearchResponse,
        TinyfishSearchResponse, WebSearchResult, ZhipuSearchResponse,
    };

    #[test]
    fn tavily_response_deserializes_results_and_answer() {
        let raw = r#"{
            "answer": "Sample answer",
            "results": [
                {
                    "title": "Example",
                    "url": "https://example.com",
                    "content": "Snippet",
                    "score": 0.91,
                    "published_date": "2026-01-01"
                }
            ]
        }"#;
        let parsed: TavilySearchResponse = serde_json::from_str(raw).expect("tavily json");
        assert_eq!(parsed.answer.as_deref(), Some("Sample answer"));
        assert_eq!(parsed.results.len(), 1);
        assert_eq!(parsed.results[0].title, "Example");
        assert_eq!(
            parsed.results[0].published_date.as_deref(),
            Some("2026-01-01")
        );
    }

    #[test]
    fn exa_response_deserializes_camel_case_fields() {
        let raw = r#"{
            "results": [
                {
                    "title": "Exa Result",
                    "url": "https://exa.ai/article",
                    "text": "Body text",
                    "summary": "Summary text",
                    "highlights": ["highlight one"],
                    "score": 0.75,
                    "publishedDate": "2026-02-02"
                }
            ]
        }"#;
        let parsed: ExaSearchResponse = serde_json::from_str(raw).expect("exa json");
        assert_eq!(parsed.results.len(), 1);
        let result = &parsed.results[0];
        assert_eq!(result.title, "Exa Result");
        assert_eq!(result.highlights, vec!["highlight one".to_string()]);
        assert_eq!(result.published_date.as_deref(), Some("2026-02-02"));
    }

    #[test]
    fn format_web_context_includes_numbered_sources_and_snippets() {
        let context = format_web_context(&[WebSearchResult {
            title: "Docs".to_string(),
            url: "https://docs.example.com".to_string(),
            content: "Helpful snippet".to_string(),
            published_date: Some("2026-03-03".to_string()),
            score: Some(0.5),
        }]);
        assert!(context.contains("Web search context:"));
        assert!(context.contains("[1] Docs"));
        assert!(context.contains("URL: https://docs.example.com"));
        assert!(context.contains("Published: 2026-03-03"));
        assert!(context.contains("Snippet: Helpful snippet"));
    }

    #[test]
    fn exa_mcp_parses_embedded_json_results() {
        let raw = r#"{
            "results": [
                { "title": "MCP Doc", "url": "https://exa.ai/mcp", "text": "body", "highlights": ["hl"], "publishedDate": "2026-04-04" }
            ]
        }"#;
        let results = parse_exa_mcp_results(raw, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://exa.ai/mcp");
        assert_eq!(results[0].content, "hl");
    }

    #[test]
    fn exa_mcp_falls_back_to_raw_text_when_not_json() {
        let results = parse_exa_mcp_results("plain text answer", 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "plain text answer");
    }

    #[test]
    fn exa_mcp_parses_formatted_text_blocks() {
        // Exa MCP 的真实返回形态：Title/URL/Published/Highlights + `---` 分隔。
        let raw = "Title: First Result\nURL: https://example.com/a\nPublished: 2026-07-03T00:57:48.000Z\nAuthor: Someone\nHighlights:\nFirst Result\n...\nBody snippet one.\n\n---\n\nTitle: Second Result\nURL: https://example.com/b\nPublished: N/A\nAuthor: N/A\nHighlights:\nBody snippet two.";
        let results = parse_exa_mcp_results(raw, 5);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://example.com/a");
        assert_eq!(results[0].title, "First Result");
        assert_eq!(
            results[0].published_date.as_deref(),
            Some("2026-07-03T00:57:48.000Z")
        );
        assert!(results[0].content.contains("Body snippet one."));
        assert_eq!(results[1].url, "https://example.com/b");
        // Published: N/A 不应写入
        assert_eq!(results[1].published_date, None);
    }

    #[test]
    fn exa_mcp_empty_content_yields_no_results() {
        assert!(parse_exa_mcp_results("   ", 5).is_empty());
    }

    #[test]
    fn ollama_response_deserializes_results() {
        let raw = r#"{
            "results": [
                { "title": "Ollama", "url": "https://ollama.com/", "content": "Cloud models..." },
                { "title": "No URL", "url": "", "content": "skip me" }
            ]
        }"#;
        let parsed: OllamaSearchResponse = serde_json::from_str(raw).expect("ollama json");
        assert_eq!(parsed.results.len(), 2);
        assert_eq!(parsed.results[0].title, "Ollama");
        assert_eq!(parsed.results[0].url, "https://ollama.com/");
    }

    #[test]
    fn grok_parses_responses_api_output_and_annotations() {
        let raw = r#"{
            "output": [
                {
                    "content": [
                        {
                            "type": "output_text",
                            "text": "Grok found the answer.",
                            "annotations": [
                                { "type": "url_citation", "url": "https://example.com/a" },
                                { "type": "url_citation", "url": "https://example.com/b" }
                            ]
                        }
                    ]
                }
            ]
        }"#;
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        let (answer, citations) = parse_grok_response(&value);
        assert_eq!(answer, "Grok found the answer.");
        assert_eq!(
            citations,
            vec!["https://example.com/a", "https://example.com/b"]
        );
    }

    #[test]
    fn grok_falls_back_to_chat_completions_and_top_level_citations() {
        let raw = r#"{
            "choices": [ { "message": { "content": "Fallback answer." } } ],
            "citations": ["https://x.ai/post", "https://x.ai/post"]
        }"#;
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        let (answer, citations) = parse_grok_response(&value);
        assert_eq!(answer, "Fallback answer.");
        // 去重：重复 URL 只保留一条
        assert_eq!(citations, vec!["https://x.ai/post"]);
    }

    #[test]
    fn brave_response_deserializes_web_results() {
        let raw = r#"{
            "web": {
                "results": [
                    {
                        "title": "Brave",
                        "url": "https://brave.com/",
                        "description": "Independent search",
                        "extra_snippets": ["snippet two"],
                        "page_age": "2026-08-01"
                    }
                ]
            }
        }"#;
        let parsed: BraveSearchResponse = serde_json::from_str(raw).expect("brave json");
        let result = &parsed.web.as_ref().unwrap().results[0];
        assert_eq!(result.title, "Brave");
        assert_eq!(result.extra_snippets, vec!["snippet two".to_string()]);
        assert_eq!(result.page_age.as_deref(), Some("2026-08-01"));
    }

    #[test]
    fn serper_response_deserializes_answer_box_and_organic() {
        let raw = r#"{
            "answerBox": { "answer": "42", "title": "Answer", "link": "https://example.com/a" },
            "organic": [
                { "title": "Hit", "link": "https://example.com/b", "snippet": "Body", "date": "Aug 18, 2026" }
            ]
        }"#;
        let parsed: SerperSearchResponse = serde_json::from_str(raw).expect("serper json");
        assert_eq!(
            parsed.answer_box.as_ref().unwrap().answer.as_deref(),
            Some("42")
        );
        assert_eq!(parsed.organic[0].link, "https://example.com/b");
    }

    #[test]
    fn bocha_response_deserializes_web_pages() {
        let raw = r#"{
            "data": {
                "webPages": {
                    "value": [
                        {
                            "name": "博查",
                            "url": "https://bochaai.com",
                            "snippet": "short",
                            "summary": "long summary",
                            "datePublished": "2026-08-18"
                        }
                    ]
                }
            }
        }"#;
        let parsed: BochaSearchResponse = serde_json::from_str(raw).expect("bocha json");
        let page = &parsed.data.unwrap().web_pages.unwrap().value[0];
        assert_eq!(page.name, "博查");
        assert_eq!(page.summary, "long summary");
        assert_eq!(page.date_published.as_deref(), Some("2026-08-18"));
    }

    #[test]
    fn zhipu_response_deserializes_search_result() {
        let raw = r#"{
            "search_result": [
                {
                    "title": "财经",
                    "link": "https://example.com/news",
                    "content": "摘要",
                    "publish_date": "2026-05-23"
                }
            ]
        }"#;
        let parsed: ZhipuSearchResponse = serde_json::from_str(raw).expect("zhipu json");
        assert_eq!(parsed.search_result[0].link, "https://example.com/news");
        assert_eq!(
            parsed.search_result[0].publish_date.as_deref(),
            Some("2026-05-23")
        );
    }

    #[test]
    fn tinyfish_response_deserializes_results() {
        let raw = r#"{
            "query": "web automation tools",
            "results": [
                {
                    "position": 1,
                    "site_name": "tinyfish.ai",
                    "title": "TinyFish",
                    "url": "https://www.tinyfish.ai/",
                    "snippet": "Web infrastructure for AI agents",
                    "date": "2026-08-01"
                }
            ]
        }"#;
        let parsed: TinyfishSearchResponse = serde_json::from_str(raw).expect("tinyfish json");
        assert_eq!(parsed.results[0].url, "https://www.tinyfish.ai/");
        assert_eq!(
            parsed.results[0].snippet,
            "Web infrastructure for AI agents"
        );
        assert_eq!(parsed.results[0].date.as_deref(), Some("2026-08-01"));
    }

    #[test]
    fn tinyfish_mcp_parses_structured_results() {
        let value = serde_json::json!({
            "results": [
                {
                    "title": "TinyFish",
                    "url": "https://www.tinyfish.ai/",
                    "snippet": "Web infrastructure for AI agents"
                }
            ]
        });
        let results = parse_tinyfish_mcp_payload("", Some(&value), 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://www.tinyfish.ai/");
        assert_eq!(results[0].content, "Web infrastructure for AI agents");
    }

    #[test]
    fn tinyfish_mcp_parses_content_json() {
        let raw = r#"{"results":[{"title":"Docs","url":"https://docs.tinyfish.ai/","snippet":"MCP"}]}"#;
        let results = parse_tinyfish_mcp_payload(raw, None, 5);
        assert_eq!(results[0].url, "https://docs.tinyfish.ai/");
    }

    #[test]
    fn searxng_response_deserializes_results() {
        let raw = r#"{
            "results": [
                { "title": "SearXNG", "url": "https://docs.searxng.org/", "content": "metasearch", "publishedDate": "2026-01-01" }
            ]
        }"#;
        let parsed: SearxngSearchResponse = serde_json::from_str(raw).expect("searxng json");
        assert_eq!(parsed.results[0].url, "https://docs.searxng.org/");
        assert_eq!(
            parsed.results[0].published_date.as_deref(),
            Some("2026-01-01")
        );
    }

    #[test]
    fn searxng_search_url_appends_search_and_rejects_empty() {
        assert_eq!(
            searxng_search_url("https://searx.example/").unwrap(),
            "https://searx.example/search"
        );
        assert_eq!(
            searxng_search_url("https://searx.example/search").unwrap(),
            "https://searx.example/search"
        );
        assert!(searxng_search_url("  ").is_err());
    }

    #[test]
    fn with_query_encodes_spaces_and_joins_params() {
        let url = super::with_query(
            "https://api.search.brave.com/res/v1/web/search",
            &[("q", "hello world"), ("count", "5")],
        );
        assert_eq!(
            url,
            "https://api.search.brave.com/res/v1/web/search?q=hello+world&count=5"
        );
    }
}
