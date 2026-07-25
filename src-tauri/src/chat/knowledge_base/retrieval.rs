//! Unified retrieval orchestration.
//!
//! One pipeline shared by the `knowledge_search` tool and the Retrieval Test
//! command, so what the model sees and what the user tests are byte-identical.
//! Stages: embed → per-lane recall + fusion → dedup → threshold → optional
//! rerank → context selection, with per-stage diagnostics captured along the
//! way (Retrieval Test surfaces them; the tool path just formats `hits`).
//!
//! The stage seams are deliberate: dedup (D6), threshold (D5) and the candidate
//! knobs (D4) each own a stage here. Today they reproduce the legacy behavior;
//! their dedicated deliverables refine one stage without replumbing the rest.

use std::collections::BTreeMap;
use std::time::Instant;

use serde::Serialize;
use tauri::AppHandle;

use crate::settings::Settings;
use crate::state::AppState;

use super::store::FusedCandidate;
use super::ScoredChunk;

/// What to retrieve and how. Built from tool args (production) or the Retrieval
/// Test form (diagnostics). `candidate_k`/`rerank_top_k`/`context_top_k` are the
/// three independent pool sizes (D4).
#[derive(Debug, Clone)]
pub struct RetrievalRequest {
    pub query: String,
    pub kb_ids: Vec<String>,
    /// Per-library fused candidate pool before rerank/threshold/context select.
    pub candidate_k: usize,
    /// How many top candidates to send to the reranker (ignored when rerank off).
    pub rerank_top_k: usize,
    /// Final passages returned to the caller.
    pub context_top_k: usize,
    pub weight_vector: f32,
    pub weight_keyword: f32,
    pub rerank: Option<RerankConfig>,
    /// Relevance threshold (D5). `<= 0` disables it (keep everything). With
    /// rerank on it gates the calibrated relevance score; with rerank off it is
    /// a cosine-similarity floor for vector-only hits (lexical hits always pass).
    pub min_score: f32,
}

#[derive(Debug, Clone)]
pub struct RerankConfig {
    pub provider_id: String,
    pub model: String,
}

/// Why a candidate did or didn't make the final context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Kept,
    Duplicate,
    BelowThreshold,
    Truncated,
}

/// One candidate with per-lane diagnostics and its final disposition.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalCandidate {
    pub kb_id: String,
    pub doc_id: String,
    pub chunk_id: String,
    pub doc_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading_path: Option<String>,
    pub text: String,
    pub order_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_rank: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_distance: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyword_rank: Option<usize>,
    pub fused_score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank_score: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_rank: Option<usize>,
    pub decision: Decision,
}

/// Rerank outcome, surfaced so the UI/log never hides a silent degrade (R4).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RerankStatus {
    Off,
    Ok,
    Failed { error: String },
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StageTimings {
    pub embed_ms: u64,
    pub search_ms: u64,
    pub rerank_ms: u64,
    pub total_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveConfig {
    pub candidate_k: usize,
    pub rerank_top_k: usize,
    pub context_top_k: usize,
    pub weight_vector: f32,
    pub weight_keyword: f32,
    pub rerank_on: bool,
    pub min_score: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalResponse {
    /// Final passages, best-first — what the tool returns to the model.
    #[serde(skip)]
    pub hits: Vec<ScoredChunk>,
    /// All candidates with per-stage diagnostics (for Retrieval Test).
    pub candidates: Vec<RetrievalCandidate>,
    pub timings: StageTimings,
    pub rerank_status: RerankStatus,
    pub effective_config: EffectiveConfig,
}

/// A fused candidate tagged with the library it came from (fusion is per-kb;
/// this carries the kb id through the global merge).
struct KbCandidate {
    kb_id: String,
    fused: FusedCandidate,
    rerank_score: Option<f32>,
}

/// Run the full retrieval pipeline. `retrieve` is the single entry point for
/// both the `knowledge_search` tool and the Retrieval Test command.
pub async fn retrieve(
    app: &AppHandle,
    state: &AppState,
    settings: &Settings,
    req: &RetrievalRequest,
) -> Result<RetrievalResponse, String> {
    let clock = Instant::now();
    let rerank_on = req
        .rerank
        .as_ref()
        .map(|r| !r.provider_id.trim().is_empty() && !r.model.trim().is_empty())
        .unwrap_or(false);

    let effective_config = EffectiveConfig {
        candidate_k: req.candidate_k,
        rerank_top_k: req.rerank_top_k,
        context_top_k: req.context_top_k,
        weight_vector: req.weight_vector,
        weight_keyword: req.weight_keyword,
        rerank_on,
        min_score: req.min_score,
    };

    let libs = super::load_libraries(app)?;
    let root = super::kb_root(app)?;

    // Group libraries by (embedding provider, model) so each group embeds the
    // query with its own model; fused (cosine) scores merge across groups.
    let mut groups: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for id in &req.kb_ids {
        if let Some(l) = libs.iter().find(|l| &l.id == id) {
            groups
                .entry((l.embedding_provider_id.clone(), l.embedding_model.clone()))
                .or_default()
                .push(id.clone());
        }
    }

    let attempts = if settings.retry_enabled {
        settings.retry_attempts as usize
    } else {
        1
    };

    // ---- stage: embed + per-lane recall + fusion ----
    let mut embed_ms = 0u64;
    let mut search_ms = 0u64;
    let mut merged: Vec<KbCandidate> = Vec::new();
    for ((provider_id, model), ids) in groups {
        let Some(provider) = settings.get_provider(&provider_id).cloned() else {
            continue;
        };
        let t_embed = Instant::now();
        let qvec =
            super::embeddings::embed_query(state, &provider, &model, &req.query, attempts).await?;
        embed_ms += t_embed.elapsed().as_millis() as u64;

        let t_search = Instant::now();
        for kb_id in &ids {
            match super::open_kb_at(&root, kb_id).and_then(|c| {
                super::store::hybrid_search_detailed(
                    &c,
                    &qvec,
                    &req.query,
                    req.candidate_k,
                    req.weight_vector,
                    req.weight_keyword,
                )
            }) {
                Ok(cands) => {
                    for fused in cands {
                        merged.push(KbCandidate {
                            kb_id: kb_id.clone(),
                            fused,
                            rerank_score: None,
                        });
                    }
                }
                // Tolerate a single broken library: skip (logged) so it can't
                // starve the rest of a cross-library search.
                Err(e) => eprintln!("kb retrieve: skipping library {kb_id}: {e}"),
            }
        }
        search_ms += t_search.elapsed().as_millis() as u64;
    }

    // Global merge: best fused first, then bound the candidate pool. The hard
    // cap keeps many mounted libraries from linearly blowing up rerank cost /
    // context size regardless of per-library candidate_k.
    const MAX_TOTAL_CANDIDATES: usize = 500;
    merged.sort_by(|a, b| {
        b.fused
            .fused_score
            .partial_cmp(&a.fused.fused_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    merged.truncate(
        req.candidate_k
            .max(req.context_top_k)
            .max(req.rerank_top_k)
            .min(MAX_TOTAL_CANDIDATES),
    );

    // ---- stage: dedup (D6) ----
    // Drop same-document near-duplicates (adjacent chunks overlap by construction,
    // plus high char-trigram overlap) BEFORE rerank, so a single passage can't
    // occupy multiple context slots and the reranker isn't spent on copies. The
    // list is fused-score-sorted, so the first occurrence kept is the strongest
    // representative; cross-document hits are never merged (preserve diversity).
    let mut deduped: Vec<KbCandidate> = Vec::new();
    let mut dupes: Vec<KbCandidate> = Vec::new();
    for c in merged {
        if deduped
            .iter()
            .any(|k| is_near_duplicate(&k.fused.chunk, &c.fused.chunk))
        {
            dupes.push(c);
        } else {
            deduped.push(c);
        }
    }
    let mut merged = deduped;

    // ---- stage: optional rerank (real relevance scores) ----
    let mut rerank_ms = 0u64;
    let mut rerank_status = if rerank_on {
        RerankStatus::Ok
    } else {
        RerankStatus::Off
    };
    if rerank_on && !merged.is_empty() {
        if let Some(rc) = &req.rerank {
            if let Some(rp) = settings.get_provider(&rc.provider_id).cloned() {
                let send_n = rerank_send_n(req.rerank_top_k, req.context_top_k, merged.len());
                let docs: Vec<String> = merged
                    .iter()
                    .take(send_n)
                    .map(|c| c.fused.chunk.text.clone())
                    .collect();
                let t = Instant::now();
                let result =
                    super::rerank::rerank(state, &rp, &rc.model, &req.query, &docs, send_n, attempts)
                        .await;
                rerank_ms = t.elapsed().as_millis() as u64;
                match result {
                    Ok(scored) if !scored.is_empty() => {
                        // Reorder the reranked head by relevance; keep the tail
                        // (beyond send_n) after it in fused order.
                        let tail: Vec<KbCandidate> = merged.split_off(send_n);
                        let head = std::mem::take(&mut merged);
                        let mut head_opt: Vec<Option<KbCandidate>> =
                            head.into_iter().map(Some).collect();
                        let mut reordered: Vec<KbCandidate> = Vec::with_capacity(head_opt.len());
                        for (i, score) in scored {
                            if let Some(mut c) = head_opt.get_mut(i).and_then(Option::take) {
                                c.rerank_score = Some(score);
                                reordered.push(c);
                            }
                        }
                        // Any head candidate the reranker dropped, appended in order.
                        for c in head_opt.into_iter().flatten() {
                            reordered.push(c);
                        }
                        reordered.extend(tail);
                        merged = reordered;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("kb rerank failed, using fused order: {e}");
                        rerank_status = RerankStatus::Failed { error: e };
                    }
                }
            }
        }
    }

    // ---- stage: relevance threshold (D5) ----
    // Score-kind-aware: with rerank on, gate on the calibrated relevance score;
    // with rerank off, lexical hits (exact term/code match) always pass while a
    // vector-only hit must clear a cosine-similarity floor. min_score <= 0 ⇒ off
    // (the conservative default — no false rejection until the user opts in).
    let mut kept: Vec<KbCandidate> = Vec::new();
    let mut below: Vec<KbCandidate> = Vec::new();
    for c in merged {
        if passes_threshold(&c, rerank_on, req.min_score) {
            kept.push(c);
        } else {
            below.push(c);
        }
    }

    // ---- stage: context selection ----
    let context_n = req.context_top_k.min(kept.len());
    let mut hits: Vec<ScoredChunk> = Vec::with_capacity(context_n);
    let mut candidates: Vec<RetrievalCandidate> = Vec::new();
    for (rank, c) in kept.iter().enumerate() {
        let decision = if rank < context_n {
            Decision::Kept
        } else {
            Decision::Truncated
        };
        if rank < context_n {
            hits.push(ScoredChunk {
                kb_id: c.kb_id.clone(),
                // rerank 命中时用校准后的 rerank 分（回退 fused）：否则返回给模型/来源卡的
                // 分数与实际（rerank 后的）排序不单调，排第一的片段可能显示比第二名更低的分。
                score: c.rerank_score.unwrap_or(c.fused.fused_score),
                chunk: c.fused.chunk.clone(),
            });
        }
        candidates.push(candidate_diag(c, decision, (rank < context_n).then_some(rank)));
    }
    for c in &below {
        candidates.push(candidate_diag(c, Decision::BelowThreshold, None));
    }
    for c in &dupes {
        candidates.push(candidate_diag(c, Decision::Duplicate, None));
    }

    let timings = StageTimings {
        embed_ms,
        search_ms,
        rerank_ms,
        total_ms: clock.elapsed().as_millis() as u64,
    };

    Ok(RetrievalResponse {
        hits,
        candidates,
        timings,
        rerank_status,
        effective_config,
    })
}

/// Whether two chunks are near-duplicates for dedup (D6). Only same-document
/// chunks are ever merged (cross-document diversity is preserved). Overlap is
/// gated on actual char-trigram Jaccard so a unique passage next to a kept one
/// is never dropped; adjacent chunks use a lower bar because chunk overlap makes
/// them share their boundary window by construction.
fn is_near_duplicate(a: &super::KnowledgeChunk, b: &super::KnowledgeChunk) -> bool {
    if a.doc_id != b.doc_id {
        return false;
    }
    let adjacent = (a.order_index as isize - b.order_index as isize).abs() <= 1;
    let threshold = if adjacent { 0.3 } else { 0.7 };
    jaccard_trigram(&a.text, &b.text) >= threshold
}

/// Char-trigram Jaccard similarity in [0,1]. Deterministic, no model.
fn jaccard_trigram(a: &str, b: &str) -> f32 {
    use std::collections::HashSet;
    let grams = |s: &str| -> HashSet<String> {
        let chars: Vec<char> = s.chars().collect();
        if chars.len() < 3 {
            return chars.iter().map(|c| c.to_string()).collect();
        }
        chars.windows(3).map(|w| w.iter().collect()).collect()
    };
    let ga = grams(a);
    let gb = grams(b);
    if ga.is_empty() || gb.is_empty() {
        return 0.0;
    }
    let inter = ga.intersection(&gb).count() as f32;
    let union = ga.union(&gb).count() as f32;
    inter / union
}

/// How many candidates to send to the reranker. Must cover at least
/// `context_top_k`: with a threshold active, `passes_threshold` drops every
/// candidate the reranker never scored, so sending fewer than the number of
/// slots we intend to fill silently truncates the result to `rerank_top_k`
/// (e.g. rerank_top_k=5 + context_top_k=20 ⇒ never more than 5 passages).
fn rerank_send_n(rerank_top_k: usize, context_top_k: usize, available: usize) -> usize {
    rerank_top_k.max(context_top_k).min(available).max(1)
}

/// Score-kind-aware relevance gate (D5). See the threshold stage for semantics.
fn passes_threshold(c: &KbCandidate, rerank_on: bool, min_score: f32) -> bool {
    if min_score <= 0.0 {
        return true; // threshold off
    }
    if rerank_on {
        // Gate on the calibrated rerank relevance score. Candidates the reranker
        // never scored (beyond rerank_top_k) are unverified ⇒ drop under an
        // active threshold.
        return c.rerank_score.map(|s| s >= min_score).unwrap_or(false);
    }
    // Rerank off: a lexical (keyword) hit is an exact-ish match ⇒ always pass.
    // A vector-only hit must clear a cosine-similarity floor (cos = 1 - dist).
    if c.fused.keyword_rank.is_some() {
        return true;
    }
    c.fused
        .vector_distance
        .map(|d| (1.0 - d) >= min_score)
        .unwrap_or(false)
}

fn candidate_diag(c: &KbCandidate, decision: Decision, final_rank: Option<usize>) -> RetrievalCandidate {
    RetrievalCandidate {
        kb_id: c.kb_id.clone(),
        doc_id: c.fused.chunk.doc_id.clone(),
        chunk_id: c.fused.chunk.id.clone(),
        doc_name: c.fused.chunk.doc_name.clone(),
        heading_path: c.fused.chunk.heading_path.clone(),
        text: c.fused.chunk.text.clone(),
        order_index: c.fused.chunk.order_index,
        vector_rank: c.fused.vector_rank,
        vector_distance: c.fused.vector_distance,
        keyword_rank: c.fused.keyword_rank,
        fused_score: c.fused.fused_score,
        rerank_score: c.rerank_score,
        final_rank,
        decision,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::knowledge_base::KnowledgeChunk;
    use crate::chat::knowledge_base::store::FusedCandidate;

    fn cand(rerank_score: Option<f32>, keyword_rank: Option<usize>, vector_distance: Option<f32>) -> KbCandidate {
        KbCandidate {
            kb_id: "k".into(),
            fused: FusedCandidate {
                rowid: 1,
                chunk: KnowledgeChunk {
                    id: "c".into(),
                    doc_id: "d".into(),
                    doc_name: "d".into(),
                    text: "t".into(),
                    heading_path: None,
                    page: None,
                    char_start: 0,
                    char_end: 0,
                    order_index: 0,
                    embedding: vec![],
                },
                vector_rank: vector_distance.map(|_| 0),
                vector_distance,
                keyword_rank,
                fused_score: 0.01,
            },
            rerank_score,
        }
    }

    #[test]
    fn threshold_off_keeps_everything() {
        // min_score 0 ⇒ off, even a nothing candidate passes.
        assert!(passes_threshold(&cand(None, None, None), false, 0.0));
        assert!(passes_threshold(&cand(None, None, None), true, 0.0));
    }

    #[test]
    fn rerank_on_gates_on_relevance_score() {
        // Calibrated relevance score vs threshold 0.5.
        assert!(passes_threshold(&cand(Some(0.8), None, None), true, 0.5));
        assert!(!passes_threshold(&cand(Some(0.2), None, None), true, 0.5));
        // Unscored (beyond rerank_top_k) ⇒ unverified ⇒ dropped under a threshold.
        assert!(!passes_threshold(&cand(None, Some(0), None), true, 0.5));
    }

    #[test]
    fn rerank_off_lexical_passes_vector_needs_similarity() {
        // Lexical (keyword) hit always passes regardless of vector signal.
        assert!(passes_threshold(&cand(None, Some(0), None), false, 0.5));
        // Vector-only: cos_sim = 1 - distance. 1-0.2=0.8 ≥ 0.5 ✓ ; 1-0.7=0.3 < 0.5 ✗.
        assert!(passes_threshold(&cand(None, None, Some(0.2)), false, 0.5));
        assert!(!passes_threshold(&cand(None, None, Some(0.7)), false, 0.5));
        // No signal at all under an active threshold ⇒ rejected (negative sample).
        assert!(!passes_threshold(&cand(None, None, None), false, 0.5));
    }

    #[test]
    fn rerank_send_n_covers_context_slots() {
        // B4b: rerank_top_k < context_top_k 时必须按 context_top_k 送，否则阈值阶段
        // 会把未打分的尾部全判死，结果被静默截到 rerank_top_k。
        assert_eq!(rerank_send_n(5, 20, 100), 20);
        // rerank_top_k 更大时以它为准（用户显式想让更多候选参与重排）。
        assert_eq!(rerank_send_n(30, 10, 100), 30);
        // 不得超过实际可用候选数。
        assert_eq!(rerank_send_n(5, 20, 8), 8);
        // 至少 1（空池由调用方的 !merged.is_empty() 挡掉，这里守住下界）。
        assert_eq!(rerank_send_n(0, 0, 0), 1);
    }

    #[test]
    fn hit_score_prefers_rerank_over_fused() {
        // B4a: rerank 命中时 hits.score 用校准分，否则回退 fused —— 保证返回的分数
        // 与实际排序单调一致。这里直接断言 context selection 用的取分表达式。
        let with_rerank = cand(Some(0.93), None, None);
        assert_eq!(
            with_rerank
                .rerank_score
                .unwrap_or(with_rerank.fused.fused_score),
            0.93
        );
        let without = cand(None, Some(0), None);
        assert_eq!(
            without.rerank_score.unwrap_or(without.fused.fused_score),
            without.fused.fused_score
        );
    }

    fn chunk(doc: &str, order: usize, text: &str) -> super::super::KnowledgeChunk {
        super::super::KnowledgeChunk {
            id: format!("{doc}-{order}"),
            doc_id: doc.into(),
            doc_name: doc.into(),
            text: text.into(),
            heading_path: None,
            page: None,
            char_start: 0,
            char_end: 0,
            order_index: order,
            embedding: vec![],
        }
    }

    #[test]
    fn dedup_merges_adjacent_same_doc_not_across_docs() {
        let a = chunk("d1", 0, "退款需要在七天内申请并提供订单编号");
        let b = chunk("d1", 1, "退款需要在七天内申请并提供订单编号（续）"); // adjacent
        let c = chunk("d2", 5, "完全不同文档的另一段内容"); // other doc
        assert!(is_near_duplicate(&a, &b), "adjacent same-doc chunks are dupes");
        assert!(!is_near_duplicate(&a, &c), "different docs must never be merged");
    }

    #[test]
    fn dedup_keeps_unique_answer_between_duplicates() {
        // A doc where chunk 0 and 2 are near-identical boilerplate but chunk 1
        // (non-adjacent to neither being kept first) is the unique answer. Order
        // 0 and 2 are 2 apart (not adjacent) but high text overlap → dupes; the
        // unique answer (different text) must survive.
        let boiler = "本页脚注：版权所有，未经许可不得转载，保留一切权利。";
        let c0 = chunk("d", 0, boiler);
        let c1 = chunk("d", 1, "退款必须在购买后七天内提出，逾期不予受理。"); // unique answer
        let c2 = chunk("d", 2, boiler); // duplicate of c0 by text
        assert!(is_near_duplicate(&c0, &c2), "high-overlap boilerplate = dupe");
        assert!(!is_near_duplicate(&c0, &c1), "unique answer is not a dupe of boilerplate");
        assert!(!is_near_duplicate(&c1, &c2), "unique answer is not a dupe of boilerplate");
    }
}
