//! Third-party search providers that also expose a URL extract/fetch API.
//!
//! `web_fetch` prefers this path when the configured Lens search provider
//! supports it, then falls back to the generic direct + reader fetch.

use std::time::Duration;

use reqwest::RequestBuilder;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    api::send_with_retry,
    settings::{LensWebSearchConfig, WebSearchProvider},
    state::AppState,
};

use super::{
    exa_mcp_server, kimi_search_url, map_tinyfish_mcp_error, normalized_base_url, provider_label,
    read_search_json, tinyfish_mcp_server,
};

/// Extract/contents/fetch can be slower than a SERP call (JS render, livecrawl).
const FETCH_HTTP_TIMEOUT: Duration = Duration::from_secs(90);
const FETCH_MCP_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_OUTPUT_CHARS: usize = 120_000;
const DEFAULT_TINYFISH_FETCH_URL: &str = "https://api.fetch.tinyfish.ai";
const DEFAULT_SERPER_SCRAPE_URL: &str = "https://scrape.serper.dev";

#[derive(Debug, Clone)]
pub struct WebFetchPage {
    pub url: String,
    pub final_url: Option<String>,
    pub title: Option<String>,
    pub text: String,
    pub method: &'static str,
}

pub fn provider_supports_fetch(provider: WebSearchProvider) -> bool {
    matches!(
        provider,
        WebSearchProvider::Tavily
            | WebSearchProvider::Exa
            | WebSearchProvider::ExaMcp
            | WebSearchProvider::Ollama
            | WebSearchProvider::Serper
            | WebSearchProvider::Tinyfish
            | WebSearchProvider::TinyfishMcp
            | WebSearchProvider::Kimi
    )
}

/// Effective `web_fetch` provider: explicit `fetch_provider`, else the search
/// provider when it has an extract API. `None` → caller should use direct fetch.
pub fn resolved_fetch_provider(config: &LensWebSearchConfig) -> Option<WebSearchProvider> {
    let candidate = config.fetch_provider.unwrap_or(config.provider);
    provider_supports_fetch(candidate).then_some(candidate)
}

pub async fn fetch_web(
    state: &AppState,
    config: &LensWebSearchConfig,
    url: &str,
    retry_attempts: usize,
    app: Option<&tauri::AppHandle>,
) -> Result<WebFetchPage, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("web_fetch requires url".to_string());
    }
    let provider = resolved_fetch_provider(config).ok_or_else(|| {
        format!(
            "{} does not provide a URL fetch API",
            provider_label(config.fetch_provider.unwrap_or(config.provider))
        )
    })?;

    match provider {
        WebSearchProvider::Tavily => fetch_tavily(state, config, url, retry_attempts).await,
        WebSearchProvider::Exa => fetch_exa(state, config, url, retry_attempts).await,
        WebSearchProvider::ExaMcp => fetch_exa_mcp(state, config, url).await,
        WebSearchProvider::Ollama => fetch_ollama(state, config, url, retry_attempts).await,
        WebSearchProvider::Serper => fetch_serper(state, config, url, retry_attempts).await,
        WebSearchProvider::Tinyfish => fetch_tinyfish(state, config, url, retry_attempts).await,
        WebSearchProvider::TinyfishMcp => fetch_tinyfish_mcp(state, config, url, app).await,
        WebSearchProvider::Kimi => fetch_kimi(state, config, url, retry_attempts).await,
        _ => Err(format!(
            "{} does not provide a URL fetch API",
            provider_label(provider)
        )),
    }
}

pub fn format_web_fetch(page: &WebFetchPage) -> String {
    let mut out = String::new();
    out.push_str(&format!("URL: {}\n", page.url));
    if let Some(final_url) = page
        .final_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != page.url)
    {
        out.push_str(&format!("Final URL: {final_url}\n"));
    }
    out.push_str(&format!("Fetch method: {}\n", page.method));
    if let Some(title) = page
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        out.push_str(&format!("Title: {title}\n"));
    }
    out.push('\n');
    out.push_str(&truncate_chars(page.text.trim(), MAX_OUTPUT_CHARS));
    out
}

fn with_fetch_timeout(request: RequestBuilder) -> RequestBuilder {
    request.timeout(FETCH_HTTP_TIMEOUT)
}

fn page(
    url: &str,
    final_url: Option<String>,
    title: Option<String>,
    text: String,
    method: &'static str,
) -> Result<WebFetchPage, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(format!("{method} fetch returned empty content"));
    }
    Ok(WebFetchPage {
        url: url.to_string(),
        final_url,
        title: title.filter(|value| !value.trim().is_empty()),
        text,
        method,
    })
}

fn json_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
        Value::Null | Value::Bool(_) | Value::Number(_) => None,
        other => {
            let serialized = other.to_string();
            if serialized.trim().is_empty() || serialized == "null" {
                None
            } else {
                Some(serialized)
            }
        }
    }
}

fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(json_text) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated = text.chars().take(max_chars).collect::<String>();
    truncated.push_str("\n\n[Truncated]");
    truncated
}

#[derive(Debug, Deserialize)]
struct TavilyExtractResponse {
    #[serde(default)]
    results: Vec<TavilyExtractResult>,
    #[serde(default)]
    failed_results: Vec<TavilyFailedExtract>,
}

#[derive(Debug, Deserialize)]
struct TavilyExtractResult {
    #[serde(default)]
    url: String,
    #[serde(default)]
    raw_content: String,
}

#[derive(Debug, Deserialize)]
struct TavilyFailedExtract {
    #[serde(default)]
    url: String,
    #[serde(default)]
    error: String,
}

fn tavily_extract_depth(search_depth: &str) -> &'static str {
    match search_depth {
        "advanced" => "advanced",
        _ => "basic",
    }
}

async fn fetch_tavily(
    state: &AppState,
    config: &LensWebSearchConfig,
    url: &str,
    retry_attempts: usize,
) -> Result<WebFetchPage, String> {
    let api_key = config.tavily_api_key.trim();
    if api_key.is_empty() {
        return Err("Tavily API key is not configured".to_string());
    }
    let base = normalized_base_url(&config.tavily_base_url, "https://api.tavily.com");
    let endpoint = format!("{base}/extract");
    let body = serde_json::json!({
        "urls": url,
        "extract_depth": tavily_extract_depth(&config.search_depth),
        "include_images": false,
        "format": "markdown",
    });
    let response = send_with_retry("Tavily extract", retry_attempts, || {
        with_fetch_timeout(state.http.post(&endpoint).bearer_auth(api_key).json(&body)).send()
    })
    .await?;
    let parsed: TavilyExtractResponse = read_search_json("Tavily extract", response).await?;
    if let Some(result) = parsed
        .results
        .into_iter()
        .find(|result| !result.raw_content.trim().is_empty())
    {
        let final_url = (!result.url.trim().is_empty()).then(|| result.url.trim().to_string());
        return page(url, final_url, None, result.raw_content, "Tavily");
    }
    let failed = parsed
        .failed_results
        .iter()
        .map(|item| {
            let failed_url = item.url.trim();
            if item.error.trim().is_empty() {
                failed_url.to_string()
            } else if failed_url.is_empty() {
                item.error.trim().to_string()
            } else {
                format!("{failed_url}: {}", item.error.trim())
            }
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if failed.is_empty() {
        Err("Tavily extract returned empty content".to_string())
    } else {
        Err(format!("Tavily extract failed: {}", failed.join("; ")))
    }
}

#[derive(Debug, Deserialize)]
struct ExaContentsResponse {
    #[serde(default)]
    results: Vec<ExaContentsResult>,
}

#[derive(Debug, Deserialize)]
struct ExaContentsResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    text: String,
}

async fn fetch_exa(
    state: &AppState,
    config: &LensWebSearchConfig,
    url: &str,
    retry_attempts: usize,
) -> Result<WebFetchPage, String> {
    let api_key = config.exa_api_key.trim();
    if api_key.is_empty() {
        return Err("Exa API key is not configured".to_string());
    }
    let base = normalized_base_url(&config.exa_base_url, "https://api.exa.ai");
    let endpoint = format!("{base}/contents");
    let body = serde_json::json!({
        "urls": [url],
        "text": { "maxCharacters": MAX_OUTPUT_CHARS },
    });
    let response = send_with_retry("Exa contents", retry_attempts, || {
        with_fetch_timeout(
            state
                .http
                .post(&endpoint)
                .header("x-api-key", api_key)
                .json(&body),
        )
        .send()
    })
    .await?;
    let parsed: ExaContentsResponse = read_search_json("Exa contents", response).await?;
    let result = parsed
        .results
        .into_iter()
        .find(|result| !result.text.trim().is_empty())
        .ok_or_else(|| "Exa contents returned empty content".to_string())?;
    let final_url = (!result.url.trim().is_empty()).then(|| result.url.trim().to_string());
    let title = (!result.title.trim().is_empty()).then(|| result.title.trim().to_string());
    page(url, final_url, title, result.text, "Exa")
}

async fn fetch_exa_mcp(
    state: &AppState,
    config: &LensWebSearchConfig,
    url: &str,
) -> Result<WebFetchPage, String> {
    let server = exa_mcp_server(config)?;
    let raw = crate::mcp::conn::call_tool_once(
        &server,
        &state.http,
        "web_fetch_exa",
        serde_json::json!({
            "urls": [url],
            "maxCharacters": MAX_OUTPUT_CHARS,
        }),
        FETCH_MCP_TIMEOUT,
    )
    .await?;
    let result = crate::mcp::result::parse_tool_result(
        serde_json::to_value(&raw).map_err(|err| err.to_string())?,
    );
    if result.is_error {
        return Err(format!("Exa MCP fetch failed: {}", result.content));
    }
    page_from_mcp_text(
        url,
        &result.content,
        result.structured_content.as_ref(),
        "Exa MCP",
    )
}

#[derive(Debug, Deserialize)]
struct OllamaFetchResponse {
    #[serde(default)]
    title: String,
    #[serde(default)]
    content: String,
}

async fn fetch_ollama(
    state: &AppState,
    config: &LensWebSearchConfig,
    url: &str,
    retry_attempts: usize,
) -> Result<WebFetchPage, String> {
    let api_key = config.ollama_api_key.trim();
    if api_key.is_empty() {
        return Err("Ollama API key is not configured".to_string());
    }
    let base = normalized_base_url(&config.ollama_base_url, "https://ollama.com");
    let endpoint = format!("{base}/api/web_fetch");
    let body = serde_json::json!({ "url": url });
    let response = send_with_retry("Ollama fetch", retry_attempts, || {
        with_fetch_timeout(state.http.post(&endpoint).bearer_auth(api_key).json(&body)).send()
    })
    .await?;
    let parsed: OllamaFetchResponse = read_search_json("Ollama fetch", response).await?;
    let title = (!parsed.title.trim().is_empty()).then(|| parsed.title.trim().to_string());
    page(url, None, title, parsed.content, "Ollama")
}

#[derive(Debug, Deserialize)]
struct SerperScrapeResponse {
    #[serde(default)]
    text: String,
    #[serde(default)]
    markdown: String,
    #[serde(default)]
    metadata: Option<SerperScrapeMetadata>,
}

#[derive(Debug, Deserialize)]
struct SerperScrapeMetadata {
    #[serde(default)]
    title: Option<String>,
}

async fn fetch_serper(
    state: &AppState,
    config: &LensWebSearchConfig,
    url: &str,
    retry_attempts: usize,
) -> Result<WebFetchPage, String> {
    let api_key = config.serper_api_key.trim();
    if api_key.is_empty() {
        return Err("Serper API key is not configured".to_string());
    }
    let body = serde_json::json!({
        "url": url,
        "includeMarkdown": true,
    });
    let response = send_with_retry("Serper scrape", retry_attempts, || {
        with_fetch_timeout(
            state
                .http
                .post(DEFAULT_SERPER_SCRAPE_URL)
                .header("X-API-KEY", api_key)
                .json(&body),
        )
        .send()
    })
    .await?;
    let parsed: SerperScrapeResponse = read_search_json("Serper scrape", response).await?;
    let text = if !parsed.markdown.trim().is_empty() {
        parsed.markdown
    } else {
        parsed.text
    };
    let title = parsed
        .metadata
        .and_then(|meta| meta.title)
        .map(|title| title.trim().to_string())
        .filter(|title| !title.is_empty());
    page(url, None, title, text, "Serper")
}

#[derive(Debug, Deserialize)]
struct TinyfishFetchResponse {
    #[serde(default)]
    results: Vec<TinyfishFetchResult>,
    #[serde(default)]
    errors: Vec<TinyfishFetchError>,
}

#[derive(Debug, Deserialize)]
struct TinyfishFetchResult {
    #[serde(default)]
    url: String,
    #[serde(default)]
    final_url: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    text: Value,
}

#[derive(Debug, Deserialize)]
struct TinyfishFetchError {
    #[serde(default)]
    url: String,
    #[serde(default)]
    error: String,
}

/// TinyFish Search and Fetch live on different hosts. Custom search relays
/// almost never expose Fetch, so extract always goes to the official Fetch API.
fn tinyfish_fetch_endpoint() -> &'static str {
    DEFAULT_TINYFISH_FETCH_URL
}

async fn fetch_tinyfish(
    state: &AppState,
    config: &LensWebSearchConfig,
    url: &str,
    retry_attempts: usize,
) -> Result<WebFetchPage, String> {
    let api_key = config.tinyfish_api_key.trim();
    if api_key.is_empty() {
        return Err("TinyFish API key is not configured".to_string());
    }
    let endpoint = tinyfish_fetch_endpoint();
    let body = serde_json::json!({
        "urls": [url],
        "format": "markdown",
    });
    let response = send_with_retry("TinyFish fetch", retry_attempts, || {
        with_fetch_timeout(
            state
                .http
                .post(endpoint)
                .header("X-API-Key", api_key)
                .json(&body),
        )
        .send()
    })
    .await?;
    let parsed: TinyfishFetchResponse = read_search_json("TinyFish fetch", response).await?;
    if let Some(result) = parsed
        .results
        .into_iter()
        .find(|result| json_text(&result.text).is_some())
    {
        return tinyfish_result_to_page(url, result);
    }
    let failed = parsed
        .errors
        .iter()
        .map(|item| {
            let failed_url = item.url.trim();
            if item.error.trim().is_empty() {
                failed_url.to_string()
            } else if failed_url.is_empty() {
                item.error.trim().to_string()
            } else {
                format!("{failed_url}: {}", item.error.trim())
            }
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if failed.is_empty() {
        Err("TinyFish fetch returned empty content".to_string())
    } else {
        Err(format!("TinyFish fetch failed: {}", failed.join("; ")))
    }
}

fn tinyfish_result_to_page(url: &str, result: TinyfishFetchResult) -> Result<WebFetchPage, String> {
    let text = json_text(&result.text)
        .ok_or_else(|| "TinyFish fetch returned empty content".to_string())?;
    let final_url = result
        .final_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| (!result.url.trim().is_empty()).then(|| result.url.trim().to_string()));
    let title = result
        .title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    page(url, final_url, title, text, "TinyFish")
}

async fn fetch_tinyfish_mcp(
    state: &AppState,
    config: &LensWebSearchConfig,
    url: &str,
    app: Option<&tauri::AppHandle>,
) -> Result<WebFetchPage, String> {
    let base = normalized_base_url(&config.tinyfish_mcp_url, "https://agent.tinyfish.ai/mcp");
    if base.is_empty() {
        return Err("TinyFish MCP endpoint is not configured".to_string());
    }
    let server = tinyfish_mcp_server(state, config, base);
    let result = state
        .mcp_call_tool(
            app,
            &server,
            "fetch_content",
            serde_json::json!({
                "urls": [url],
                "format": "markdown",
            }),
        )
        .await
        .map_err(|err| map_tinyfish_mcp_error(&err))?;
    if result.is_error {
        return Err(map_tinyfish_mcp_error(&format!(
            "TinyFish MCP fetch failed: {}",
            result.content
        )));
    }
    page_from_mcp_text(
        url,
        &result.content,
        result.structured_content.as_ref(),
        "TinyFish MCP",
    )
}

fn kimi_fetch_url(configured: &str) -> String {
    let search = kimi_search_url(configured);
    if let Some(prefix) = search.strip_suffix("/search") {
        format!("{prefix}/fetch")
    } else {
        format!("{search}/fetch")
    }
}

async fn fetch_kimi(
    state: &AppState,
    config: &LensWebSearchConfig,
    url: &str,
    retry_attempts: usize,
) -> Result<WebFetchPage, String> {
    let api_key = config.kimi_api_key.trim();
    if api_key.is_empty() {
        return Err("Kimi API key is not configured".to_string());
    }
    let endpoint = kimi_fetch_url(&config.kimi_base_url);
    let body = serde_json::json!({ "url": url });
    let response = send_with_retry("Kimi fetch", retry_attempts, || {
        with_fetch_timeout(
            state
                .http
                .post(&endpoint)
                .bearer_auth(api_key)
                .header("Accept", "text/markdown")
                .json(&body),
        )
        .send()
    })
    .await?;
    let raw = response
        .text()
        .await
        .map_err(|err| format!("Kimi fetch read body: {err}"))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Kimi fetch returned empty content".to_string());
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        if let Some(text) = first_string(&value, &["content", "markdown", "text", "raw_content"]) {
            let title = first_string(&value, &["title"]);
            return page(url, None, title, text, "Kimi");
        }
    }
    page(
        url,
        None,
        markdown_title(trimmed),
        trimmed.to_string(),
        "Kimi",
    )
}

fn markdown_title(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix("# ")
            .or_else(|| trimmed.strip_prefix("Title:"))
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(str::to_string)
    })
}

fn page_from_mcp_text(
    url: &str,
    content: &str,
    structured: Option<&Value>,
    method: &'static str,
) -> Result<WebFetchPage, String> {
    if let Some(value) = structured {
        if let Some(page) = page_from_value(url, value, method) {
            return Ok(page);
        }
    }
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err(format!("{method} fetch returned empty content"));
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        if let Some(page) = page_from_value(url, &value, method) {
            return Ok(page);
        }
    }
    page(
        url,
        None,
        markdown_title(trimmed),
        trimmed.to_string(),
        method,
    )
}

fn page_from_value(url: &str, value: &Value, method: &'static str) -> Option<WebFetchPage> {
    if let Some(results) = value.get("results").and_then(Value::as_array) {
        for item in results {
            if let Some(page) = page_from_object(url, item, method) {
                return Some(page);
            }
        }
    }
    page_from_object(url, value, method)
}

fn page_from_object(url: &str, value: &Value, method: &'static str) -> Option<WebFetchPage> {
    let text = first_string(value, &["text", "content", "markdown", "raw_content"])?;
    let title = first_string(value, &["title"]);
    let final_url = first_string(value, &["final_url", "url"]);
    page(url, final_url, title, text, method).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::WebSearchProvider;

    #[test]
    fn resolved_fetch_provider_follows_search_until_overridden() {
        let mut config = crate::settings::LensWebSearchConfig::default();
        config.provider = WebSearchProvider::Exa;
        assert_eq!(
            resolved_fetch_provider(&config),
            Some(WebSearchProvider::Exa)
        );

        config.fetch_provider = Some(WebSearchProvider::Tavily);
        assert_eq!(
            resolved_fetch_provider(&config),
            Some(WebSearchProvider::Tavily)
        );

        config.provider = WebSearchProvider::Brave;
        config.fetch_provider = None;
        assert_eq!(resolved_fetch_provider(&config), None);

        config.fetch_provider = Some(WebSearchProvider::Tavily);
        assert_eq!(
            resolved_fetch_provider(&config),
            Some(WebSearchProvider::Tavily)
        );
    }

    #[test]
    fn provider_supports_fetch_covers_extract_apis_only() {
        for provider in [
            WebSearchProvider::Tavily,
            WebSearchProvider::Exa,
            WebSearchProvider::ExaMcp,
            WebSearchProvider::Ollama,
            WebSearchProvider::Serper,
            WebSearchProvider::Tinyfish,
            WebSearchProvider::TinyfishMcp,
            WebSearchProvider::Kimi,
        ] {
            assert!(
                provider_supports_fetch(provider),
                "{provider:?} should expose fetch"
            );
        }
        for provider in [
            WebSearchProvider::Brave,
            WebSearchProvider::Bocha,
            WebSearchProvider::Zhipu,
            WebSearchProvider::Searxng,
            WebSearchProvider::Grok,
            WebSearchProvider::Deepseek,
            WebSearchProvider::Unknown,
        ] {
            assert!(
                !provider_supports_fetch(provider),
                "{provider:?} should fall back to direct fetch"
            );
        }
    }

    #[test]
    fn tavily_extract_depth_maps_advanced_only() {
        assert_eq!(tavily_extract_depth("advanced"), "advanced");
        assert_eq!(tavily_extract_depth("basic"), "basic");
        assert_eq!(tavily_extract_depth("fast"), "basic");
        assert_eq!(tavily_extract_depth("ultra-fast"), "basic");
    }

    #[test]
    fn kimi_fetch_url_swaps_search_segment() {
        assert_eq!(kimi_fetch_url(""), "https://api.kimi.com/coding/v1/fetch");
        assert_eq!(
            kimi_fetch_url("https://api.kimi.com/coding/v1/search"),
            "https://api.kimi.com/coding/v1/fetch"
        );
        assert_eq!(
            kimi_fetch_url("https://api.moonshot.cn/v1"),
            "https://api.moonshot.cn/v1/fetch"
        );
        assert_eq!(
            kimi_fetch_url("https://relay.example/v1/search"),
            "https://relay.example/v1/fetch"
        );
    }

    #[test]
    fn tinyfish_fetch_uses_official_extract_host() {
        assert_eq!(tinyfish_fetch_endpoint(), "https://api.fetch.tinyfish.ai");
    }

    #[test]
    fn tavily_extract_response_reads_raw_content() {
        let parsed: TavilyExtractResponse = serde_json::from_str(
            r##"{
                "results": [{ "url": "https://example.com", "raw_content": "# Hello" }],
                "failed_results": []
            }"##,
        )
        .expect("tavily extract");
        assert_eq!(parsed.results[0].raw_content, "# Hello");
    }

    #[test]
    fn ollama_fetch_response_reads_title_and_content() {
        let parsed: OllamaFetchResponse = serde_json::from_str(
            r#"{ "title": "Ollama", "content": "Cloud models", "links": ["https://ollama.com"] }"#,
        )
        .expect("ollama fetch");
        assert_eq!(parsed.title, "Ollama");
        assert_eq!(parsed.content, "Cloud models");
    }

    #[test]
    fn serper_scrape_prefers_markdown() {
        let parsed: SerperScrapeResponse = serde_json::from_str(
            r##"{
                "text": "plain",
                "markdown": "# Title",
                "metadata": { "title": "Example" }
            }"##,
        )
        .expect("serper scrape");
        assert_eq!(parsed.markdown, "# Title");
        assert_eq!(parsed.metadata.unwrap().title.as_deref(), Some("Example"));
    }

    #[test]
    fn tinyfish_fetch_response_reads_markdown_text() {
        let parsed: TinyfishFetchResponse = serde_json::from_str(
            r##"{
                "results": [{
                    "url": "https://example.com",
                    "final_url": "https://example.com/",
                    "title": "Example",
                    "text": "# Hello"
                }],
                "errors": []
            }"##,
        )
        .expect("tinyfish fetch");
        assert_eq!(
            json_text(&parsed.results[0].text).as_deref(),
            Some("# Hello")
        );
    }

    #[test]
    fn format_web_fetch_includes_method_and_title() {
        let output = format_web_fetch(&WebFetchPage {
            url: "https://example.com".to_string(),
            final_url: Some("https://example.com/final".to_string()),
            title: Some("Example".to_string()),
            text: "hello".to_string(),
            method: "Tavily",
        });
        assert!(output.contains("URL: https://example.com"));
        assert!(output.contains("Final URL: https://example.com/final"));
        assert!(output.contains("Fetch method: Tavily"));
        assert!(output.contains("Title: Example"));
        assert!(output.contains("hello"));
    }

    #[test]
    fn page_from_mcp_text_reads_structured_results() {
        let value = serde_json::json!({
            "results": [{
                "title": "Doc",
                "url": "https://exa.ai/doc",
                "text": "full page"
            }]
        });
        let page = page_from_mcp_text("https://exa.ai/doc", "ignored", Some(&value), "Exa MCP")
            .expect("page");
        assert_eq!(page.title.as_deref(), Some("Doc"));
        assert_eq!(page.text, "full page");
        assert_eq!(page.method, "Exa MCP");
    }
}
