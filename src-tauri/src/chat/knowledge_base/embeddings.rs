//! Embedding provider adapter. Anthropic has no embeddings endpoint, so this
//! is deliberately separate from `chat/model/` and only speaks the
//! OpenAI-compatible `POST {base_url}/embeddings` shape (OpenAI / Jina / Voyage
//! / DashScope / SiliconFlow / local LM Studio all match it). Multi-key
//! failover + retry is reused from `crate::api`.

use serde_json::Value;

use crate::api::{send_with_failover, with_standard_request_timeout};
use crate::settings::ModelProvider;
use crate::state::AppState;
use crate::usage::{self, UsageRecordInput};

/// 用量统计里嵌入调用的来源标签（索引与检索共用同一条通道）。
const EMBED_USAGE_SOURCE: &str = "knowledge_base";

/// Retrieval role of an embedding request (D3). Query and document sides need
/// asymmetric encoding on many modern models (Voyage `input_type`, Jina `task`,
/// E5 `query:`/`passage:` prefixes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingRole {
    Query,
    Document,
}

/// Apply the provider/model retrieval role: prefix inputs and/or contribute
/// extra top-level request-body fields. Unknown models return the inputs
/// unchanged with no extra body — the symmetric legacy behavior, which is also
/// what Gemini/OpenAI need (they 400 on unknown body fields). Pure + tested.
///
/// The profile is derived purely from (base_url, model); since changing a
/// library's embedding model already forces a full reindex, the document side
/// can never drift out of sync with what was indexed — no extra bookkeeping.
pub fn apply_retrieval_role(
    base_url: &str,
    model: &str,
    role: EmbeddingRole,
    inputs: &[String],
) -> (Vec<String>, serde_json::Map<String, Value>) {
    let m = model.to_lowercase();
    let u = base_url.to_lowercase();
    let is_query = role == EmbeddingRole::Query;
    let mut extra = serde_json::Map::new();
    let mut prefix = "";
    if m.contains("voyage") || u.contains("voyage") {
        // Voyage: input_type=query|document.
        extra.insert(
            "input_type".into(),
            Value::String(if is_query { "query" } else { "document" }.into()),
        );
    } else if m.contains("jina") {
        // Jina v3+: task=retrieval.query|retrieval.passage.
        extra.insert(
            "task".into(),
            Value::String(
                if is_query {
                    "retrieval.query"
                } else {
                    "retrieval.passage"
                }
                .into(),
            ),
        );
    } else if m.contains("e5") {
        // E5 family: instruct via query:/passage: prefixes.
        prefix = if is_query { "query: " } else { "passage: " };
    }
    // Unknown (OpenAI, BGE-M3, Gemini, local, …) → symmetric, no change.
    let mapped = if prefix.is_empty() {
        inputs.to_vec()
    } else {
        inputs.iter().map(|s| format!("{prefix}{s}")).collect()
    };
    (mapped, extra)
}

/// Embed a batch of inputs in one request, applying the retrieval `role`.
/// Returns one vector per input, in input order.
pub async fn embed_batch(
    state: &AppState,
    provider: &ModelProvider,
    model: &str,
    inputs: &[String],
    role: EmbeddingRole,
    attempts: usize,
) -> Result<Vec<Vec<f32>>, String> {
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    if model.trim().is_empty() {
        return Err("Embedding model is not set".to_string());
    }
    let keys: Vec<String> = provider
        .api_keys
        .iter()
        .filter(|k| !k.trim().is_empty())
        .cloned()
        .collect();
    if keys.is_empty() {
        return Err(format!("Provider '{}' has no API key", provider.name));
    }
    let url = format!("{}/embeddings", provider.base_url.trim_end_matches('/'));
    let (inputs, extra) = apply_retrieval_role(&provider.base_url, model, role, inputs);
    let mut body = serde_json::json!({ "model": model, "input": inputs });
    if let Some(obj) = body.as_object_mut() {
        obj.extend(extra);
    }

    // 记一次用量：/embeddings 是真实计费调用，成功/失败都进「用量统计」，来源=知识库。
    let started_at = chrono::Local::now().timestamp();
    let clock = std::time::Instant::now();
    let record = |status: &str,
                  status_code: Option<u16>,
                  usage: Option<crate::chat::model::ModelUsage>,
                  error_kind: Option<String>| {
        usage::record_model_call(
            state,
            UsageRecordInput {
                provider,
                model,
                source: EMBED_USAGE_SOURCE,
                operation: "embed",
                status,
                status_code,
                usage,
                usage_source: "provider_reported",
                started_at,
                duration_ms: clock.elapsed().as_millis() as u64,
                conversation_id: None,
                message_id: None,
                error_kind,
            },
        );
    };

    let response = match send_with_failover(
        state,
        "Embeddings API",
        attempts,
        &provider.id,
        &keys,
        |key| {
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
        },
    )
    .await
    {
        Ok(resp) => resp,
        Err(e) => {
            record(
                "error",
                crate::api::extract_status_code(&e),
                None,
                Some(usage::error_kind_from_message(&e)),
            );
            return Err(e);
        }
    };

    let value: Value = response
        .json()
        .await
        .map_err(|e| format!("embeddings response not JSON: {e}"))?;

    // 调用已计费成功——先记用量（含 provider 返回的 token 数），再做数量校验。
    record(
        "success",
        Some(200),
        usage::model_usage_from_openai_value(&value),
        None,
    );

    let vectors = parse_embeddings_response(&value)?;
    if vectors.len() != inputs.len() {
        return Err(format!(
            "embeddings count mismatch: got {}, expected {}",
            vectors.len(),
            inputs.len()
        ));
    }
    Ok(vectors)
}

/// Parse the OpenAI-compatible `/embeddings` response body into row-ordered
/// vectors. Pure (no I/O) so the wire-format contract is unit-testable.
pub fn parse_embeddings_response(value: &Value) -> Result<Vec<Vec<f32>>, String> {
    let data = value
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| {
            // Surface the provider's error message if present.
            let msg = value
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("missing `data` array");
            format!("embeddings API error: {msg}")
        })?;

    let mut indexed: Vec<(usize, Vec<f32>)> = Vec::with_capacity(data.len());
    for item in data {
        let idx = item.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
        let emb = item
            .get("embedding")
            .and_then(|e| e.as_array())
            .ok_or("embeddings: item missing `embedding`")?
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect::<Vec<f32>>();
        indexed.push((idx, emb));
    }
    indexed.sort_by_key(|(i, _)| *i);
    Ok(indexed.into_iter().map(|(_, e)| e).collect())
}

/// Embed a single query string (Query role).
pub async fn embed_query(
    state: &AppState,
    provider: &ModelProvider,
    model: &str,
    query: &str,
    attempts: usize,
) -> Result<Vec<f32>, String> {
    let mut v = embed_batch(
        state,
        provider,
        model,
        &[query.to_string()],
        EmbeddingRole::Query,
        attempts,
    )
    .await?;
    v.pop()
        .ok_or_else(|| "embeddings: empty result for query".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_reorders_by_index() {
        // Provider returns rows out of order; we must sort by `index`.
        let body = serde_json::json!({
            "data": [
                { "index": 1, "embedding": [0.0, 1.0] },
                { "index": 0, "embedding": [1.0, 0.0] },
            ],
            "usage": { "prompt_tokens": 4 }
        });
        let vectors = parse_embeddings_response(&body).unwrap();
        assert_eq!(vectors, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
    }

    #[test]
    fn surfaces_provider_error_message() {
        let body = serde_json::json!({ "error": { "message": "invalid api key" } });
        let err = parse_embeddings_response(&body).unwrap_err();
        assert!(err.contains("invalid api key"), "got: {err}");
    }

    #[test]
    fn rejects_item_without_embedding() {
        let body = serde_json::json!({ "data": [{ "index": 0 }] });
        assert!(parse_embeddings_response(&body).is_err());
    }

    #[test]
    fn voyage_gets_asymmetric_input_type() {
        let inp = vec!["hello".to_string()];
        let (q_in, q_extra) = apply_retrieval_role(
            "https://api.voyageai.com/v1",
            "voyage-3",
            EmbeddingRole::Query,
            &inp,
        );
        assert_eq!(q_in, inp); // no prefix
        assert_eq!(q_extra.get("input_type").unwrap(), "query");
        let (_, d_extra) = apply_retrieval_role(
            "https://api.voyageai.com/v1",
            "voyage-3",
            EmbeddingRole::Document,
            &inp,
        );
        assert_eq!(d_extra.get("input_type").unwrap(), "document");
    }

    #[test]
    fn jina_uses_task_and_e5_uses_prefix() {
        let inp = vec!["x".to_string()];
        let (_, jina) = apply_retrieval_role(
            "https://api.jina.ai/v1",
            "jina-embeddings-v3",
            EmbeddingRole::Query,
            &inp,
        );
        assert_eq!(jina.get("task").unwrap(), "retrieval.query");
        let (e5_in, e5_extra) = apply_retrieval_role(
            "http://localhost:1234/v1",
            "multilingual-e5-large",
            EmbeddingRole::Document,
            &inp,
        );
        assert_eq!(e5_in, vec!["passage: x".to_string()]);
        assert!(e5_extra.is_empty()); // prefix only, no body field
    }

    #[test]
    fn unknown_model_stays_symmetric_and_bodyless() {
        // OpenAI / BGE-M3 / Gemini / local: no prefix, no extra body — a request
        // byte-identical to the pre-D3 behavior (Gemini 400s on unknown fields).
        let inp = vec!["hello".to_string()];
        for model in [
            "text-embedding-3-small",
            "bge-m3",
            "gemini-embedding-001",
            "nomic-embed",
        ] {
            let (q_in, q_extra) = apply_retrieval_role(
                "https://api.openai.com/v1",
                model,
                EmbeddingRole::Query,
                &inp,
            );
            let (d_in, d_extra) = apply_retrieval_role(
                "https://api.openai.com/v1",
                model,
                EmbeddingRole::Document,
                &inp,
            );
            assert_eq!(q_in, inp, "{model} query prefixed");
            assert_eq!(d_in, inp, "{model} doc prefixed");
            assert!(
                q_extra.is_empty() && d_extra.is_empty(),
                "{model} added body fields"
            );
        }
    }
}
