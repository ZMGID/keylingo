//! Live end-to-end tests for the knowledge-base (RAG) V2 retrieval stack.
//!
//! These exercise the REAL pipeline functions (no reimplementations):
//! `embeddings::embed_batch`, `store::{open_db, replace_doc_chunks, hybrid_search}`,
//! `rerank::rerank`, `chunking::chunk_document`, `parse::parse_file`.
//!
//! Test 1 hits live embedding + rerank endpoints and is **gated** behind the
//! `KB_E2E=1` env var (it reads the API base/key/models from env and returns
//! early — passing — when unset, so CI and offline runs stay green; the main
//! session runs it with real keys). NO secrets are ever hardcoded.
//!
//! Tests 2 & 3 cover the built-in (offline, network-free) docx/html parsers
//! against crafted fixtures.

use std::io::Write;

// ===== Test 1: gated live retrieval stack (embedding + hybrid search + rerank) =====

#[tokio::test]
async fn live_retrieval_stack_e2e() {
    if std::env::var("KB_E2E").as_deref() != Ok("1") {
        eprintln!("[kb-e2e] KB_E2E!=1, skipping live test");
        return;
    }

    let base = std::env::var("KB_E2E_BASE").expect("KB_E2E_BASE must be set when KB_E2E=1");
    let key = std::env::var("KB_E2E_KEY").expect("KB_E2E_KEY must be set when KB_E2E=1");
    let embed_model =
        std::env::var("KB_E2E_EMBED_MODEL").expect("KB_E2E_EMBED_MODEL must be set when KB_E2E=1");
    let rerank_model = std::env::var("KB_E2E_RERANK_MODEL")
        .expect("KB_E2E_RERANK_MODEL must be set when KB_E2E=1");

    // Build a provider purely from env — never from settings.json.
    let provider: crate::settings::ModelProvider = serde_json::from_value(serde_json::json!({
        "id": "e2e",
        "name": "e2e",
        "baseUrl": base,
        "apiKeys": [key],
        "enabledModels": [embed_model.clone()],
    }))
    .unwrap();

    let state = crate::state::AppState::new_headless(
        crate::settings::Settings::default(),
        std::env::temp_dir(),
    );

    // A small Chinese corpus: exactly one passage is the right answer to the
    // query; the rest are distractors (weather / recipe / sports / history)
    // so ranking is meaningful.
    let query = "知识库如何做向量检索".to_string();
    const TARGET_KEYWORD: &str = "向量";
    let passages = [
        // index 0 — the TARGET passage (vector / embedding / RRF retrieval).
        "知识库的向量检索流程是：先把文档切分成小块，再用嵌入模型把每个块编码成向量，查询时把问题也编码成向量，用余弦相似度找最近的块，并用 RRF 把关键词检索和向量检索的结果融合排序。",
        // distractors
        "今天的天气晴朗，最高气温二十八度，傍晚可能有阵雨，出门记得带伞并注意防晒。",
        "番茄炒蛋的做法：鸡蛋打散，热油下锅炒成块盛出，再炒番茄出汁，倒回鸡蛋翻炒加盐即可。",
        "昨晚的篮球比赛非常激烈，主队在最后三秒命中绝杀三分，以一分优势战胜了客队。",
        "唐朝是中国历史上一个繁荣的朝代，长安是当时的国际化大都市，丝绸之路贸易往来频繁。",
        "钢琴是一种键盘乐器，通过琴键带动小槌敲击琴弦发声，音域宽广，常用于独奏与伴奏。",
    ];

    // 1) Chunk + embed each passage. Treat one chunk per short passage (still
    //    runs the real chunker so the pipeline path is exercised).
    let mut texts: Vec<String> = Vec::new();
    for p in &passages {
        let pieces = super::chunking::chunk_document(p, false);
        if pieces.is_empty() {
            texts.push((*p).to_string());
        } else {
            for piece in pieces {
                texts.push(piece.text);
            }
        }
    }

    let vectors = super::embeddings::embed_batch(
        &state,
        &provider,
        &embed_model,
        &texts,
        super::embeddings::EmbeddingRole::Document,
        1,
    )
    .await
    .unwrap();
    assert_eq!(vectors.len(), texts.len(), "one vector per chunk text");

    let dim = vectors[0].len();
    // dim must be discovered from the response, NOT hardcoded.
    assert!(dim > 0, "embedding dim must be positive, got {dim}");
    eprintln!("[kb-e2e] embedded {} chunks, dim={dim}", texts.len());

    // 2) Build KnowledgeChunk rows and write them to a real per-library store.
    let mut chunks: Vec<super::KnowledgeChunk> = Vec::new();
    for (i, (text, embedding)) in texts.iter().zip(vectors.iter()).enumerate() {
        chunks.push(super::KnowledgeChunk {
            id: super::gen_id("chunk"),
            doc_id: "doc1".to_string(),
            doc_name: "e2e.txt".to_string(),
            text: text.clone(),
            heading_path: None,
            page: None,
            char_start: 0,
            char_end: text.chars().count(),
            order_index: i,
            embedding: embedding.clone(),
        });
    }

    let db = std::env::temp_dir().join(format!("kb_e2e_{}.db", uuid::Uuid::new_v4().simple()));
    let _ = std::fs::remove_file(&db);
    let conn = super::store::open_db(&db).unwrap();
    super::store::replace_doc_chunks(&conn, "doc1", dim, &chunks).unwrap();

    // 3) Embed the query and run hybrid search.
    let qvec = super::embeddings::embed_batch(
        &state,
        &provider,
        &embed_model,
        &[query.clone()],
        super::embeddings::EmbeddingRole::Query,
        1,
    )
    .await
    .unwrap()
    .remove(0);

    let hits = super::store::hybrid_search(&conn, &qvec, &query, 5, 1.0, 1.0).unwrap();
    assert!(!hits.is_empty(), "hybrid search returned no hits");
    eprintln!("[kb-e2e] hybrid_search ranking:");
    for (rank, (chunk, score)) in hits.iter().enumerate() {
        eprintln!("  #{rank} score={score:.5} :: {}", chunk.text);
    }
    assert!(
        hits[0].0.text.contains(TARGET_KEYWORD),
        "top hit should be the vector-retrieval passage, got: {}",
        hits[0].0.text
    );

    // 4) Rerank the hit texts and confirm the target still ranks first.
    let hit_texts: Vec<String> = hits.iter().map(|(c, _)| c.text.clone()).collect();
    let order = super::rerank::rerank(
        &state,
        &provider,
        &rerank_model,
        &query,
        &hit_texts,
        hit_texts.len(),
        1,
    )
    .await
    .unwrap();
    assert!(!order.is_empty(), "rerank returned an empty order");
    eprintln!("[kb-e2e] rerank reorder (index, score): {order:?}");
    let first_reranked = &hit_texts[order[0].0];
    eprintln!("[kb-e2e] top reranked passage: {first_reranked}");
    assert!(
        first_reranked.contains(TARGET_KEYWORD),
        "first reranked passage should be the target, got: {first_reranked}"
    );

    // 5) Cleanup.
    drop(conn);
    let _ = std::fs::remove_file(&db);
}

// ===== Test 2: built-in docx parsing via crafted fixture (offline) =====

#[test]
fn builtin_docx_parse_extracts_paragraph_text() {
    let path = std::env::temp_dir().join(format!(
        "kb_e2e_fixture_{}.docx",
        uuid::Uuid::new_v4().simple()
    ));
    {
        let f = std::fs::File::create(&path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        z.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        z.write_all(
            r#"<?xml version="1.0"?><w:document><w:body><w:p><w:r><w:t>知识库测试文档</w:t></w:r></w:p></w:body></w:document>"#.as_bytes(),
        )
        .unwrap();
        z.finish().unwrap();
    }

    let parsed = super::parse::parse_file(&path).unwrap();
    assert!(
        parsed.text.contains("知识库测试文档"),
        "docx text missing first paragraph: {:?}",
        parsed.text
    );

    let _ = std::fs::remove_file(&path);
}

// ===== Test 3: built-in html parsing (offline) =====

#[test]
fn builtin_html_parse_extracts_body_text() {
    let path = std::env::temp_dir().join(format!(
        "kb_e2e_fixture_{}.html",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::write(
        &path,
        r#"<html><head><title>页面标题</title></head><body><article><p>正文内容一二三</p></article></body></html>"#,
    )
    .unwrap();

    let parsed = super::parse::parse_file(&path).unwrap();
    assert!(
        parsed.text.contains("正文内容一二三"),
        "html text missing article body: {:?}",
        parsed.text
    );

    let _ = std::fs::remove_file(&path);
}
