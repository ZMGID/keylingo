//! Optional rerank adapter (cross-encoder reranking). Cohere / Jina expose a
//! compatible `POST {base_url}/rerank` shape, so one adapter covers both. Rerank
//! is global + optional: callers degrade to the pre-rerank order on any error.

use serde_json::Value;

use crate::api::{send_with_failover, with_standard_request_timeout};
use crate::settings::ModelProvider;
use crate::state::AppState;

/// Rerank `documents` against `query`. Returns `(original_index, relevance_score)`
/// pairs sorted best-first (length ≤ documents.len()). Cohere/Jina return
/// `results` already sorted by relevance, each with its original `index` and a
/// `relevance_score` — the score lets callers apply a real relevance threshold
/// (D5), not just reorder.
pub async fn rerank(
    state: &AppState,
    provider: &ModelProvider,
    model: &str,
    query: &str,
    documents: &[String],
    top_n: usize,
    attempts: usize,
) -> Result<Vec<(usize, f32)>, String> {
    if documents.is_empty() {
        return Ok(Vec::new());
    }
    if model.trim().is_empty() {
        return Err("Rerank model is not set".to_string());
    }
    let keys: Vec<String> = provider
        .api_keys
        .iter()
        .filter(|k| !k.trim().is_empty())
        .cloned()
        .collect();
    if keys.is_empty() {
        return Err(format!(
            "Rerank provider '{}' has no API key",
            provider.name
        ));
    }
    let url = format!("{}/rerank", provider.base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model,
        "query": query,
        "documents": documents,
        "top_n": top_n,
    });

    let response = send_with_failover(state, "Rerank API", attempts, &provider.id, &keys, |key| {
        with_standard_request_timeout(
            crate::provider_request::apply(
                state
                    .client_for(provider)
                    .post(url.clone())
                    .bearer_auth(key),
                provider,
                None,
            )
            .json(&body),
        )
        .send()
    })
    .await?;

    let value: Value = response
        .json()
        .await
        .map_err(|e| format!("rerank response not JSON: {e}"))?;

    let results = value
        .get("results")
        .and_then(|r| r.as_array())
        .ok_or_else(|| {
            let msg = value
                .get("error")
                .and_then(|e| e.get("message").or(Some(e)))
                .and_then(|m| m.as_str())
                .unwrap_or("missing `results` array");
            format!("rerank API error: {msg}")
        })?;

    let order: Vec<(usize, f32)> = results
        .iter()
        .filter_map(|r| {
            let idx = r.get("index").and_then(|i| i.as_u64())? as usize;
            if idx >= documents.len() {
                return None;
            }
            // relevance_score is the calibrated 0..1 relevance (Cohere/Jina);
            // default to 0 if a provider omits it so ordering still works.
            let score = r
                .get("relevance_score")
                .and_then(|s| s.as_f64())
                .unwrap_or(0.0) as f32;
            Some((idx, score))
        })
        .collect();
    Ok(order)
}
