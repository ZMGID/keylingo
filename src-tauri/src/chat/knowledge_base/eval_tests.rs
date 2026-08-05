//! Retrieval evaluation harness (D7).
//!
//! A deterministic, offline retrieval quality gate. It runs the store-level
//! hybrid core (`hybrid_search_detailed`) against a small, versioned, in-repo
//! bilingual corpus with relevance labels, and reports Recall@K / MRR / nDCG@10
//! **per lane** (vector-only, keyword-only, hybrid) so a lexical change (D2) is
//! visible as a keyword-lane recall jump.
//!
//! Why store-level, not `retrieve()`: `retrieve()` needs an `AppHandle` and a
//! live embedding provider (network). To stay deterministic and offline, the
//! harness embeds text with a fixed hashing stand-in (`mock_embed`) — a crude
//! bag of char uni/bi-grams, NOT a real model. It is meaningful only for
//! measuring that lanes fuse and that lexical recall moves; absolute vector
//! numbers are not comparable to production embeddings.
//!
//! The command is `cargo test`:
//!   cargo test --manifest-path src-tauri/Cargo.toml eval_retrieval -- --nocapture
//! `--nocapture` prints the report table (compare before/after a change).

#![cfg(test)]

use super::store::{self, hybrid_search_detailed};
use super::KnowledgeChunk;

/// Deterministic offline embedding stand-in: hash char unigrams + bigrams of
/// the normalized text into a fixed-dim vector, L2-normalized. Shared terms →
/// positive cosine. NOT a real model — see module docs.
const MOCK_DIM: usize = 96;
fn mock_embed(text: &str) -> Vec<f32> {
    let norm: Vec<char> = text
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let mut v = vec![0.0f32; MOCK_DIM];
    let bump = |v: &mut Vec<f32>, s: &str| {
        // FNV-1a → bucket.
        let mut h: u64 = 0xcbf29ce484222325;
        for b in s.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        v[(h as usize) % MOCK_DIM] += 1.0;
    };
    for c in &norm {
        bump(&mut v, &c.to_string());
    }
    for w in norm.windows(2) {
        bump(&mut v, &format!("{}{}", w[0], w[1]));
    }
    let mag = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag > 0.0 {
        for x in &mut v {
            *x /= mag;
        }
    }
    v
}

/// One labeled document (single chunk for eval simplicity).
struct EvalDoc {
    id: &'static str,
    text: &'static str,
}

/// A labeled query. `relevant` lists doc ids that answer it; empty = negative
/// sample (nothing in the KB should answer it).
struct EvalQuery {
    query: &'static str,
    relevant: &'static [&'static str],
    lang: &'static str,
}

/// Versioned corpus. Deliberately synthetic and non-private (R7/R8): natural
/// language questions worded DIFFERENTLY from the source sentence, plus exact
/// codes/numbers, plus negatives with no answer in the KB.
const DOCS: &[EvalDoc] = &[
    EvalDoc { id: "refund", text: "退款需要在购买后七天内提出申请，并提供订单编号，逾期不予受理。" },
    EvalDoc { id: "shipping", text: "标准配送通常在下单后三到五个工作日内送达，偏远地区可能延迟。" },
    EvalDoc { id: "password", text: "如需重置密码，请在登录页点击忘记密码，系统会向绑定邮箱发送验证码。" },
    EvalDoc { id: "errcode", text: "错误码 E1021 表示鉴权令牌已过期，请重新登录后重试该操作。" },
    EvalDoc { id: "warranty", text: "The product warranty covers manufacturing defects for twelve months from the date of purchase." },
    EvalDoc { id: "return", text: "Returns are accepted within 30 days if the item is unused and in its original packaging." },
    EvalDoc { id: "invoice", text: "发票可在订单完成后于「我的订单」页面自助下载，支持增值税专用发票。" },
    EvalDoc { id: "vip", text: "会员等级根据累计消费金额计算，达到白金等级可享受专属客服与折扣。" },
];

const QUERIES: &[EvalQuery] = &[
    // 中文自然语言改写（措辞与原句不同）——关键词 phrase 查询会漏，这是 D2 的核心。
    EvalQuery {
        query: "退款要满足什么条件",
        relevant: &["refund"],
        lang: "zh",
    },
    EvalQuery {
        query: "多久能收到货",
        relevant: &["shipping"],
        lang: "zh",
    },
    EvalQuery {
        query: "忘记密码怎么办",
        relevant: &["password"],
        lang: "zh",
    },
    EvalQuery {
        query: "怎么开发票",
        relevant: &["invoice"],
        lang: "zh",
    },
    EvalQuery {
        query: "白金会员有什么好处",
        relevant: &["vip"],
        lang: "zh",
    },
    // 精确编号/错误码——应命中。
    EvalQuery {
        query: "E1021",
        relevant: &["errcode"],
        lang: "code",
    },
    // 英文改写。
    EvalQuery {
        query: "how long is the warranty period",
        relevant: &["warranty"],
        lang: "en",
    },
    EvalQuery {
        query: "can I return an item I bought",
        relevant: &["return"],
        lang: "en",
    },
    // 负样本——知识库里没有答案。
    EvalQuery {
        query: "你们支持哪些加密货币支付",
        relevant: &[],
        lang: "neg",
    },
    EvalQuery {
        query: "what is the CEO's phone number",
        relevant: &[],
        lang: "neg",
    },
];

struct Metrics {
    recall5: f32,
    recall10: f32,
    recall20: f32,
    mrr: f32,
    ndcg10: f32,
    n: usize,
}

fn recall_at_k(ranked: &[String], relevant: &[&str], k: usize) -> f32 {
    if relevant.is_empty() {
        return 0.0;
    }
    let hit = relevant
        .iter()
        .filter(|r| ranked.iter().take(k).any(|x| x == *r))
        .count();
    hit as f32 / relevant.len() as f32
}

fn rr(ranked: &[String], relevant: &[&str]) -> f32 {
    for (i, id) in ranked.iter().enumerate() {
        if relevant.contains(&id.as_str()) {
            return 1.0 / (i as f32 + 1.0);
        }
    }
    0.0
}

fn ndcg_at_k(ranked: &[String], relevant: &[&str], k: usize) -> f32 {
    if relevant.is_empty() {
        return 0.0;
    }
    let dcg: f32 = ranked
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, id)| {
            if relevant.contains(&id.as_str()) {
                1.0 / ((i as f32 + 2.0).log2())
            } else {
                0.0
            }
        })
        .sum();
    let ideal: f32 = (0..relevant.len().min(k))
        .map(|i| 1.0 / ((i as f32 + 2.0).log2()))
        .sum();
    if ideal > 0.0 {
        dcg / ideal
    } else {
        0.0
    }
}

/// Run all positive queries through one lane weighting; aggregate metrics.
fn eval_lane(conn: &rusqlite::Connection, w_vec: f32, w_kw: f32) -> Metrics {
    let mut r5 = 0.0;
    let mut r10 = 0.0;
    let mut r20 = 0.0;
    let mut mrr = 0.0;
    let mut ndcg = 0.0;
    let mut n = 0;
    for q in QUERIES.iter().filter(|q| !q.relevant.is_empty()) {
        let qvec = mock_embed(q.query);
        let cands = hybrid_search_detailed(conn, &qvec, q.query, 20, w_vec, w_kw).unwrap();
        let ranked: Vec<String> = cands.iter().map(|c| c.chunk.doc_id.clone()).collect();
        r5 += recall_at_k(&ranked, q.relevant, 5);
        r10 += recall_at_k(&ranked, q.relevant, 10);
        r20 += recall_at_k(&ranked, q.relevant, 20);
        mrr += rr(&ranked, q.relevant);
        ndcg += ndcg_at_k(&ranked, q.relevant, 10);
        n += 1;
    }
    let d = n.max(1) as f32;
    Metrics {
        recall5: r5 / d,
        recall10: r10 / d,
        recall20: r20 / d,
        mrr: mrr / d,
        ndcg10: ndcg / d,
        n,
    }
}

fn build_store() -> (rusqlite::Connection, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("kivio-eval-{}.db", uuid::Uuid::new_v4()));
    let conn = store::open_db(&path).unwrap();
    let chunks: Vec<KnowledgeChunk> = DOCS
        .iter()
        .enumerate()
        .map(|(i, d)| KnowledgeChunk {
            id: format!("{}-c0", d.id),
            doc_id: d.id.to_string(),
            doc_name: format!("{}.md", d.id),
            text: d.text.to_string(),
            heading_path: None,
            page: None,
            char_start: 0,
            char_end: 0,
            order_index: i,
            embedding: mock_embed(d.text),
        })
        .collect();
    // Each doc is its own single-chunk "document".
    for c in &chunks {
        store::replace_doc_chunks(&conn, &c.doc_id, MOCK_DIM, std::slice::from_ref(c)).unwrap();
    }
    (conn, path)
}

/// Baseline / regression report. Prints a per-lane metric table (run with
/// `-- --nocapture`) and asserts weak sanity floors so a hard regression trips
/// CI without pinning exact numbers (which the mock embedding makes arbitrary).
#[test]
fn eval_retrieval_report() {
    let (conn, path) = build_store();

    let vec_only = eval_lane(&conn, 1.0, 0.0);
    let kw_only = eval_lane(&conn, 0.0, 1.0);
    let hybrid = eval_lane(&conn, 1.0, 1.0);

    // Negative-sample separation: top-1 fused score for negatives vs positives.
    // A working threshold (D5) must sit between these; the false-hit RATE is
    // finalized once the threshold stage lands.
    let mut neg_top = Vec::new();
    for q in QUERIES.iter().filter(|q| q.relevant.is_empty()) {
        let qvec = mock_embed(q.query);
        let cands = hybrid_search_detailed(&conn, &qvec, q.query, 20, 1.0, 1.0).unwrap();
        neg_top.push(cands.first().map(|c| c.fused_score).unwrap_or(0.0));
    }
    let neg_mean = neg_top.iter().sum::<f32>() / neg_top.len().max(1) as f32;

    let row = |name: &str, m: &Metrics| {
        println!(
            "{name:>12} | R@5 {:.3} | R@10 {:.3} | R@20 {:.3} | MRR {:.3} | nDCG@10 {:.3} | n={}",
            m.recall5, m.recall10, m.recall20, m.mrr, m.ndcg10, m.n
        );
    };
    println!("\n===== retrieval eval (mock embeddings; per-lane) =====");
    let mut by_lang: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for q in QUERIES {
        *by_lang.entry(q.lang).or_default() += 1;
    }
    println!(
        "corpus: {} docs, {} queries {by_lang:?}",
        DOCS.len(),
        QUERIES.len()
    );
    row("vector", &vec_only);
    row("keyword", &kw_only);
    row("hybrid", &hybrid);
    println!(
        "negatives    | top-1 fused mean {neg_mean:.5} (n={})",
        neg_top.len()
    );
    println!("======================================================\n");

    // Weak sanity: the pipeline must retrieve *something* relevant on the vector
    // lane, and hybrid must be no worse than vector-only at R@20.
    assert!(
        vec_only.recall20 > 0.0,
        "vector lane retrieved nothing relevant"
    );
    assert!(
        hybrid.recall20 + 1e-6 >= vec_only.recall20,
        "hybrid R@20 regressed below vector-only"
    );
    // D2 regression guard: CJK-bigram keyword recall must stay well above the
    // pre-D2 phrase-query baseline (0.125). A drop here means CJK keyword recall
    // broke (e.g. tokenizer/search-text regression).
    assert!(
        kw_only.recall10 >= 0.5,
        "keyword-lane R@10 {:.3} regressed toward the pre-D2 phrase-query baseline",
        kw_only.recall10
    );

    drop(conn);
    std::fs::remove_file(&path).ok();
}
