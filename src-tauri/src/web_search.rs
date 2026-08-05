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
        WebSearchProvider::Unknown => {
            Err("Selected web search provider is not supported yet".to_string())
        }
    }
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
        format_web_context, parse_exa_mcp_results, parse_grok_response, ExaSearchResponse,
        OllamaSearchResponse, TavilySearchResponse, WebSearchResult,
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
}
