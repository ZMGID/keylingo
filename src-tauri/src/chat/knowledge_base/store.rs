//! SQLite-backed knowledge base store (sqlite-vec `vec0` + FTS5).
//!
//! Replaces the V1 JSON files. One SQLite file per library at
//! `{app_data}/knowledge_base/<kb_id>/store.db` (a `vec0` virtual table has a
//! fixed dimension, and each library binds its own embedding dim, so the vector
//! table is per-library and created lazily once the dim is known).
//!
//! Engineering red lines (see PRD V2 §D1): we use `rusqlite` directly (NOT
//! `tauri-plugin-sql`, whose sqlx can't load the vec extension); the extension
//! is registered via `sqlite3_auto_extension` before any connection opens;
//! `vec0` virtual tables can't carry extra columns, so chunk text/metadata live
//! in a normal `chunks` table joined by rowid.

use std::path::Path;
use std::sync::Once;

use rusqlite::{Connection, OpenFlags};

use super::{DocStatus, KnowledgeChunk, KnowledgeDocument};

/// Register sqlite-vec once per process so every opened connection sees `vec0`.
fn ensure_vec_extension() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // SAFETY: the documented sqlite-vec + rusqlite registration. Must run
        // before any connection is opened; `Once` guarantees single execution.
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
    });
}

/// Serialize an embedding to the little-endian f32 blob sqlite-vec expects.
pub fn embedding_to_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

/// Open (creating if needed) a library's store.db with base tables. The `vec0`
/// table is created separately once the embedding dim is known.
pub fn open_db(path: &Path) -> Result<Connection, String> {
    ensure_vec_extension();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create kb dir: {e}"))?;
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )
    .map_err(|e| format!("open store.db: {e}"))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA foreign_keys=ON;
         CREATE TABLE IF NOT EXISTS documents (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            size_bytes INTEGER NOT NULL DEFAULT 0,
            hash TEXT NOT NULL DEFAULT '',
            chunk_count INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL DEFAULT 'indexing',
            error TEXT,
            created_at INTEGER NOT NULL DEFAULT 0
         );
         CREATE TABLE IF NOT EXISTS chunks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            chunk_id TEXT NOT NULL,
            doc_id TEXT NOT NULL,
            doc_name TEXT NOT NULL DEFAULT '',
            text TEXT NOT NULL,
            search_text TEXT NOT NULL DEFAULT '',
            heading_path TEXT,
            page INTEGER,
            char_start INTEGER NOT NULL DEFAULT 0,
            char_end INTEGER NOT NULL DEFAULT 0,
            order_index INTEGER NOT NULL DEFAULT 0
         );
         CREATE INDEX IF NOT EXISTS idx_chunks_doc ON chunks(doc_id);",
    )
    .map_err(|e| format!("init store.db schema: {e}"))?;
    // FTS schema (D2): CJK-bigram + latin-word text indexed with unicode61, so
    // natural-language CJK queries recall on shared bigrams instead of failing
    // the old whole-query trigram phrase match. Detects + migrates legacy DBs.
    ensure_fts_schema(&conn)?;
    Ok(conn)
}

/// Whether a column exists on a table (for idempotent migration).
fn column_exists(conn: &Connection, table: &str, col: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| e.to_string())?;
    let names: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(names.iter().any(|n| n == col))
}

/// Create (fresh) or migrate (legacy) the `chunks_fts` index to the D2 shape:
/// external-content FTS5 over `chunks.search_text` with the `unicode61`
/// tokenizer. Legacy DBs indexed raw `text` with `trigram`, which cannot match
/// CJK overlaps shorter than 3 chars. Idempotent: a no-op once already migrated.
fn ensure_fts_schema(conn: &Connection) -> Result<(), String> {
    // Old DBs lack the search_text column entirely.
    if !column_exists(conn, "chunks", "search_text")? {
        conn.execute(
            "ALTER TABLE chunks ADD COLUMN search_text TEXT NOT NULL DEFAULT ''",
            [],
        )
        .map_err(|e| format!("add search_text column: {e}"))?;
    }
    // Is the FTS table already the new (search_text) one?
    let fts_sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='chunks_fts'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok();
    if fts_sql
        .as_deref()
        .map(|s| s.contains("search_text"))
        .unwrap_or(false)
    {
        return Ok(());
    }
    // (Re)build FTS on search_text with unicode61, then backfill + rebuild the
    // index from existing chunk text (no re-embed / re-parse needed).
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS chunks_ai;
         DROP TRIGGER IF EXISTS chunks_ad;
         DROP TABLE IF EXISTS chunks_fts;
         CREATE VIRTUAL TABLE chunks_fts USING fts5(
            search_text, content='chunks', content_rowid='id', tokenize='unicode61'
         );
         CREATE TRIGGER chunks_ai AFTER INSERT ON chunks BEGIN
            INSERT INTO chunks_fts(rowid, search_text) VALUES (new.id, new.search_text);
         END;
         CREATE TRIGGER chunks_ad AFTER DELETE ON chunks BEGIN
            INSERT INTO chunks_fts(chunks_fts, rowid, search_text) VALUES('delete', old.id, old.search_text);
         END;",
    )
    .map_err(|e| format!("create chunks_fts: {e}"))?;
    backfill_search_text(conn)?;
    // Rebuild the external-content index from the populated search_text column.
    conn.execute("INSERT INTO chunks_fts(chunks_fts) VALUES('rebuild')", [])
        .map_err(|e| format!("rebuild chunks_fts: {e}"))?;
    Ok(())
}

/// Recompute `search_text` for every chunk from its stored text/doc_name/
/// heading (legacy rows had an empty column). Cheap: text is already on disk.
fn backfill_search_text(conn: &Connection) -> Result<(), String> {
    let rows: Vec<(i64, String, String, Option<String>)> = {
        let mut stmt = conn
            .prepare("SELECT id, text, doc_name, heading_path FROM chunks")
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(|e| e.to_string())?;
        let mut v = Vec::new();
        for row in mapped {
            v.push(row.map_err(|e| e.to_string())?);
        }
        v
    };
    for (id, text, doc_name, heading) in rows {
        let st = build_search_text(&text, &doc_name, heading.as_deref());
        conn.execute(
            "UPDATE chunks SET search_text=?1 WHERE id=?2",
            rusqlite::params![st, id],
        )
        .map_err(|e| format!("backfill search_text: {e}"))?;
    }
    Ok(())
}

/// Create the per-library `vec0` table for the given dimension if absent.
/// Cosine distance to match V1 retrieval semantics.
pub fn ensure_vec_table(conn: &Connection, dim: usize) -> Result<(), String> {
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS vec_chunks USING vec0(embedding float[{dim}] distance_metric=cosine);"
    ))
    .map_err(|e| format!("create vec_chunks(dim={dim}): {e}"))
}

// ===== documents =====

fn status_str(s: DocStatus) -> &'static str {
    match s {
        DocStatus::Indexing => "indexing",
        DocStatus::Ready => "ready",
        DocStatus::Error => "error",
    }
}

fn status_from(s: &str) -> DocStatus {
    match s {
        "ready" => DocStatus::Ready,
        "error" => DocStatus::Error,
        _ => DocStatus::Indexing,
    }
}

fn row_to_doc(row: &rusqlite::Row<'_>) -> rusqlite::Result<KnowledgeDocument> {
    let status: String = row.get(5)?;
    Ok(KnowledgeDocument {
        id: row.get(0)?,
        name: row.get(1)?,
        size_bytes: row.get::<_, i64>(2)? as u64,
        hash: row.get(3)?,
        chunk_count: row.get::<_, i64>(4)? as usize,
        status: status_from(&status),
        error: row.get(6)?,
        created_at: row.get(7)?,
    })
}

const DOC_COLS: &str = "id, name, size_bytes, hash, chunk_count, status, error, created_at";

pub fn load_docs(conn: &Connection) -> Result<Vec<KnowledgeDocument>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {DOC_COLS} FROM documents ORDER BY created_at"
        ))
        .map_err(|e| e.to_string())?;
    let docs = stmt
        .query_map([], row_to_doc)
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    Ok(docs)
}

pub fn doc_by_hash(conn: &Connection, hash: &str) -> Result<Option<KnowledgeDocument>, String> {
    let mut stmt = conn
        .prepare(&format!("SELECT {DOC_COLS} FROM documents WHERE hash = ?1"))
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map([hash], row_to_doc)
        .map_err(|e| e.to_string())?;
    match rows.next() {
        Some(r) => Ok(Some(r.map_err(|e| e.to_string())?)),
        None => Ok(None),
    }
}

pub fn insert_doc(conn: &Connection, doc: &KnowledgeDocument) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO documents (id, name, size_bytes, hash, chunk_count, status, error, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            doc.id,
            doc.name,
            doc.size_bytes as i64,
            doc.hash,
            doc.chunk_count as i64,
            status_str(doc.status),
            doc.error,
            doc.created_at,
        ],
    )
    .map_err(|e| format!("insert doc: {e}"))?;
    Ok(())
}

pub fn set_doc_status(
    conn: &Connection,
    doc_id: &str,
    status: DocStatus,
    chunk_count: usize,
    error: Option<&str>,
) -> Result<(), String> {
    conn.execute(
        "UPDATE documents SET status=?2, chunk_count=?3, error=?4 WHERE id=?1",
        rusqlite::params![doc_id, status_str(status), chunk_count as i64, error],
    )
    .map_err(|e| format!("set doc status: {e}"))?;
    Ok(())
}

/// Delete a document, its chunks (FTS auto-synced by trigger) and vec rows.
pub fn delete_doc(conn: &Connection, doc_id: &str) -> Result<bool, String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    delete_doc_vec_rows(&tx, doc_id)?;
    tx.execute("DELETE FROM chunks WHERE doc_id=?1", [doc_id])
        .map_err(|e| format!("delete chunks: {e}"))?;
    let n = tx
        .execute("DELETE FROM documents WHERE id=?1", [doc_id])
        .map_err(|e| format!("delete doc: {e}"))?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n > 0)
}

/// Remove vec rows whose rowid belongs to this doc's chunks (vtable has no FK).
fn delete_doc_vec_rows(conn: &Connection, doc_id: &str) -> Result<(), String> {
    if !vec_table_exists(conn)? {
        return Ok(());
    }
    conn.execute(
        "DELETE FROM vec_chunks WHERE rowid IN (SELECT id FROM chunks WHERE doc_id=?1)",
        [doc_id],
    )
    .map_err(|e| format!("delete vec rows: {e}"))?;
    Ok(())
}

fn vec_table_exists(conn: &Connection) -> Result<bool, String> {
    let n: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='vec_chunks'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

// ===== chunks =====

/// Replace all of a document's chunks (delete old + insert new) in one tx.
/// `dim` is the embedding dimension (used to lazily create the vec table).
pub fn replace_doc_chunks(
    conn: &Connection,
    doc_id: &str,
    dim: usize,
    chunks: &[KnowledgeChunk],
) -> Result<(), String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    if dim > 0 {
        ensure_vec_table(&tx, dim)?;
    }
    delete_doc_vec_rows(&tx, doc_id)?;
    tx.execute("DELETE FROM chunks WHERE doc_id=?1", [doc_id])
        .map_err(|e| format!("delete old chunks: {e}"))?;
    for c in chunks {
        let search_text = build_search_text(&c.text, &c.doc_name, c.heading_path.as_deref());
        tx.execute(
            "INSERT INTO chunks (chunk_id, doc_id, doc_name, text, search_text, heading_path, page, char_start, char_end, order_index)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            rusqlite::params![
                c.id,
                c.doc_id,
                c.doc_name,
                c.text,
                search_text,
                c.heading_path,
                c.page.map(|p| p as i64),
                c.char_start as i64,
                c.char_end as i64,
                c.order_index as i64,
            ],
        )
        .map_err(|e| format!("insert chunk: {e}"))?;
        let rowid = tx.last_insert_rowid();
        if dim > 0 {
            tx.execute(
                "INSERT INTO vec_chunks(rowid, embedding) VALUES (?1, ?2)",
                rusqlite::params![rowid, embedding_to_blob(&c.embedding)],
            )
            .map_err(|e| format!("insert vec: {e}"))?;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

/// Drop every chunk + vec row (used by full reindex before refilling).
pub fn clear_chunks(conn: &Connection) -> Result<(), String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    if vec_table_exists(&tx)? {
        tx.execute("DROP TABLE vec_chunks", [])
            .map_err(|e| format!("drop vec table: {e}"))?;
    }
    tx.execute("DELETE FROM chunks", [])
        .map_err(|e| format!("clear chunks: {e}"))?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn counts(conn: &Connection) -> Result<(usize, usize), String> {
    let docs: i64 = conn
        .query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let chunks: i64 = conn
        .query_row("SELECT count(*) FROM chunks", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    Ok((docs as usize, chunks as usize))
}

// ===== search text preprocessing + FTS query building (D2) =====

/// Whether a char is a CJK/ideographic **word** character (not punctuation):
/// callers gate on `is_alphanumeric()` first, so CJK punctuation like `。，！`
/// (which lies inside these ranges but is not alphanumeric) is treated as a
/// separator, not a token.
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x9FFF      // CJK radicals … unified ideographs (incl. kana, bopomofo)
        | 0xAC00..=0xD7AF    // hangul syllables
        | 0xF900..=0xFAFF    // CJK compat ideographs
        | 0x20000..=0x2FA1F  // CJK ext B+ / compat supplement
    )
}

/// Turn text into a whitespace-tokenizable search string: CJK runs become
/// space-joined char **bigrams** (so a natural-language CJK query recalls on any
/// shared 2-char span), while latin/digit runs are kept **whole** and lowercased
/// (so error codes / versions / IDs stay exact-matchable). Everything else is a
/// separator. Pure + unit-tested; both indexing and querying go through it so
/// the two sides always agree.
pub fn bigrammize(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut cjk: Vec<char> = Vec::new();
    let mut latin = String::new();
    let flush_cjk = |cjk: &mut Vec<char>, out: &mut Vec<String>| {
        match cjk.len() {
            0 => {}
            1 => out.push(cjk[0].to_string()),
            _ => {
                for w in cjk.windows(2) {
                    out.push(format!("{}{}", w[0], w[1]));
                }
            }
        }
        cjk.clear();
    };
    let flush_latin = |latin: &mut String, out: &mut Vec<String>| {
        if !latin.is_empty() {
            out.push(std::mem::take(latin));
        }
    };
    for c in text.chars() {
        if !c.is_alphanumeric() {
            // separator (whitespace, ASCII/CJK punctuation, symbols)
            flush_cjk(&mut cjk, &mut out);
            flush_latin(&mut latin, &mut out);
        } else if is_cjk(c) {
            flush_latin(&mut latin, &mut out);
            cjk.push(c);
        } else {
            flush_cjk(&mut cjk, &mut out);
            latin.extend(c.to_lowercase());
        }
    }
    flush_cjk(&mut cjk, &mut out);
    flush_latin(&mut latin, &mut out);
    out.join(" ")
}

/// The indexed search string for a chunk: its text plus doc name and heading
/// path (so a query can hit on the document title / section), bigrammized.
pub fn build_search_text(text: &str, doc_name: &str, heading_path: Option<&str>) -> String {
    let mut combined = String::from(text);
    combined.push(' ');
    combined.push_str(doc_name);
    if let Some(h) = heading_path {
        combined.push(' ');
        combined.push_str(h);
    }
    bigrammize(&combined)
}

/// Max query tokens sent to FTS5 (bounds a pathologically long query).
const MAX_FTS_TERMS: usize = 128;

/// Build a safe FTS5 MATCH string from a raw user query: bigrammize it the same
/// way documents are indexed, then OR the tokens (recall-first; BM25 ranks
/// docs matching more tokens higher). Each token is quoted so FTS5 operators /
/// punctuation in user text are treated as literals, never query syntax.
/// Returns None when the query has no indexable content (empty/punctuation).
pub fn build_fts_query(raw: &str) -> Option<String> {
    let processed = bigrammize(raw);
    let terms: Vec<String> = processed
        .split(' ')
        .filter(|t| !t.is_empty())
        .take(MAX_FTS_TERMS)
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

fn row_to_chunk(row: &rusqlite::Row<'_>) -> rusqlite::Result<KnowledgeChunk> {
    Ok(KnowledgeChunk {
        id: row.get("chunk_id")?,
        doc_id: row.get("doc_id")?,
        doc_name: row.get("doc_name")?,
        text: row.get("text")?,
        heading_path: row.get("heading_path")?,
        page: row.get::<_, Option<i64>>("page")?.map(|p| p as usize),
        char_start: row.get::<_, i64>("char_start")? as usize,
        char_end: row.get::<_, i64>("char_end")? as usize,
        order_index: row.get::<_, i64>("order_index")? as usize,
        embedding: Vec::new(), // not needed in search results
    })
}

/// Vector KNN via sqlite-vec. Returns (rowid, chunk, cosine_distance) best-first.
fn vector_rows(
    conn: &Connection,
    query: &[f32],
    limit: usize,
) -> Result<Vec<(i64, KnowledgeChunk, f32)>, String> {
    if limit == 0 || query.is_empty() || !vec_table_exists(conn)? {
        return Ok(Vec::new());
    }
    let sql = "SELECT c.id AS rowid, c.chunk_id, c.doc_id, c.doc_name, c.text, c.heading_path,
                      c.page, c.char_start, c.char_end, c.order_index, v.distance AS distance
               FROM vec_chunks v
               JOIN chunks c ON c.id = v.rowid
               WHERE v.embedding MATCH ?1 AND k = ?2
               ORDER BY v.distance";
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let blob = embedding_to_blob(query);
    let rows = stmt
        .query_map(rusqlite::params![blob, limit as i64], |row| {
            let rowid: i64 = row.get("rowid")?;
            let distance: f64 = row.get("distance")?;
            Ok((rowid, row_to_chunk(row)?, distance as f32))
        })
        .map_err(|e| format!("vector search: {e}"))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// FTS5 BM25 keyword search. Returns (rowid, chunk) best-first.
fn fts_rows(
    conn: &Connection,
    query_text: &str,
    limit: usize,
) -> Result<Vec<(i64, KnowledgeChunk)>, String> {
    let q = query_text.trim();
    if limit == 0 || q.is_empty() {
        return Ok(Vec::new());
    }
    // Build a safe bigrammized OR query; None ⇒ nothing indexable to match.
    let Some(match_query) = build_fts_query(q) else {
        return Ok(Vec::new());
    };
    let sql = "SELECT c.id AS rowid, c.chunk_id, c.doc_id, c.doc_name, c.text, c.heading_path,
                      c.page, c.char_start, c.char_end, c.order_index
               FROM chunks_fts f
               JOIN chunks c ON c.id = f.rowid
               WHERE chunks_fts MATCH ?1
               ORDER BY f.rank
               LIMIT ?2";
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![match_query, limit as i64], |row| {
            let rowid: i64 = row.get("rowid")?;
            Ok((rowid, row_to_chunk(row)?))
        })
        .map_err(|e| format!("fts search: {e}"))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Hybrid search: fuse vector (cosine) + FTS5 (BM25) rankings with Reciprocal
/// Rank Fusion (k=60). Weights gate each lane (0 disables it); with only the
/// vector lane on this is equivalent to pure vector search. Returns (chunk, score)
/// best-first where score is the (unnormalized) fused RRF score.
pub fn hybrid_search(
    conn: &Connection,
    query_vec: &[f32],
    query_text: &str,
    top_k: usize,
    weight_vector: f32,
    weight_keyword: f32,
) -> Result<Vec<(KnowledgeChunk, f32)>, String> {
    Ok(hybrid_search_detailed(
        conn,
        query_vec,
        query_text,
        top_k,
        weight_vector,
        weight_keyword,
    )?
    .into_iter()
    .map(|c| (c.chunk, c.fused_score))
    .collect())
}

/// A fused candidate carrying per-lane diagnostics. `hybrid_search` is a thin
/// projection of this; the retrieval orchestrator uses the full detail to show
/// which lane surfaced each hit (Retrieval Test) and to feed later stages
/// (dedup / rerank / threshold) without re-querying.
#[derive(Debug, Clone)]
pub struct FusedCandidate {
    pub rowid: i64,
    pub chunk: KnowledgeChunk,
    pub vector_rank: Option<usize>,
    pub vector_distance: Option<f32>,
    pub keyword_rank: Option<usize>,
    pub fused_score: f32,
}

/// Fusion core (see `hybrid_search`) that retains per-lane ranks/distance.
/// `top_k` bounds the returned list; each lane over-fetches internally so
/// fusion sees beyond it. Behavior of the fused score is identical to the
/// legacy `hybrid_search` — this is a superset, not a change.
pub fn hybrid_search_detailed(
    conn: &Connection,
    query_vec: &[f32],
    query_text: &str,
    top_k: usize,
    weight_vector: f32,
    weight_keyword: f32,
) -> Result<Vec<FusedCandidate>, String> {
    use std::collections::HashMap;
    const RRF_K: f32 = 60.0;
    // Over-fetch each lane so fusion sees beyond the final top_k.
    let fetch = (top_k * 5).max(20);

    let mut cand: HashMap<i64, FusedCandidate> = HashMap::new();

    if weight_vector > 0.0 {
        for (rank, (rowid, chunk, dist)) in
            vector_rows(conn, query_vec, fetch)?.into_iter().enumerate()
        {
            let entry = cand.entry(rowid).or_insert_with(|| FusedCandidate {
                rowid,
                chunk,
                vector_rank: None,
                vector_distance: None,
                keyword_rank: None,
                fused_score: 0.0,
            });
            entry.vector_rank = Some(rank);
            entry.vector_distance = Some(dist);
            entry.fused_score += weight_vector / (RRF_K + (rank as f32 + 1.0));
        }
    }
    if weight_keyword > 0.0 {
        for (rank, (rowid, chunk)) in fts_rows(conn, query_text, fetch)?.into_iter().enumerate() {
            let entry = cand.entry(rowid).or_insert_with(|| FusedCandidate {
                rowid,
                chunk,
                vector_rank: None,
                vector_distance: None,
                keyword_rank: None,
                fused_score: 0.0,
            });
            entry.keyword_rank = Some(rank);
            entry.fused_score += weight_keyword / (RRF_K + (rank as f32 + 1.0));
        }
    }

    let mut out: Vec<FusedCandidate> = cand.into_values().collect();
    out.sort_by(|a, b| {
        b.fused_score
            .partial_cmp(&a.fused_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(top_k);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    // De-risk: confirm bundled SQLite has FTS5 and sqlite-vec's vec0 registers
    // and answers a KNN query end-to-end.
    #[test]
    fn sqlite_vec_and_fts5_smoke() {
        ensure_vec_extension();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE chunks(id INTEGER PRIMARY KEY, text TEXT);
             CREATE VIRTUAL TABLE chunks_fts USING fts5(text, content='chunks', content_rowid='id');
             CREATE TRIGGER chunks_ai AFTER INSERT ON chunks BEGIN
                INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
             END;
             CREATE VIRTUAL TABLE vec_chunks USING vec0(embedding float[3]);",
        )
        .expect("FTS5 + vec0 must be available");

        // vector rows
        for (rowid, v) in [
            (1i64, [1.0f32, 0.0, 0.0]),
            (2, [0.0, 1.0, 0.0]),
            (3, [0.9, 0.1, 0.0]),
        ] {
            conn.execute(
                "INSERT INTO vec_chunks(rowid, embedding) VALUES (?, ?)",
                params![rowid, embedding_to_blob(&v)],
            )
            .unwrap();
        }
        // KNN: query near [1,0,0] → rowid 1 then 3
        let q = embedding_to_blob(&[1.0, 0.0, 0.0]);
        let rows: Vec<i64> = conn
            .prepare(
                "SELECT rowid FROM vec_chunks WHERE embedding MATCH ? ORDER BY distance LIMIT 2",
            )
            .unwrap()
            .query_map(params![q], |r| r.get::<_, i64>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows, vec![1, 3]);

        // FTS5 BM25
        conn.execute(
            "INSERT INTO chunks(id, text) VALUES (1, 'the quick brown fox')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chunks(id, text) VALUES (2, 'lazy dog sleeps')",
            [],
        )
        .unwrap();
        let hits: Vec<i64> = conn
            .prepare("SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ? ORDER BY rank")
            .unwrap()
            .query_map(params!["fox"], |r| r.get::<_, i64>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(hits, vec![1]);
    }

    fn tmp_db() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("kivio-store-test-{}.db", uuid::Uuid::new_v4()))
    }

    fn mk_chunk(id: &str, text: &str, emb: Vec<f32>) -> KnowledgeChunk {
        KnowledgeChunk {
            id: id.to_string(),
            doc_id: "d".to_string(),
            doc_name: "d.md".to_string(),
            text: text.to_string(),
            heading_path: None,
            page: None,
            char_start: 0,
            char_end: 0,
            order_index: 0,
            embedding: emb,
        }
    }

    // ---- D2: bigram search-text + FTS query builder ----

    #[test]
    fn bigrammize_cjk_and_latin() {
        // CJK run → space-joined char bigrams.
        assert_eq!(bigrammize("退款条件"), "退款 款条 条件");
        // Single CJK char kept as-is.
        assert_eq!(bigrammize("退"), "退");
        // Latin/digit run kept whole and lowercased; codes survive intact.
        assert_eq!(bigrammize("Error E1021"), "error e1021");
        // Mixed CJK + latin + punctuation as separators.
        assert_eq!(bigrammize("错误码 E1021！"), "错误 误码 e1021");
        // Punctuation-only / empty → nothing.
        assert_eq!(bigrammize("，。！"), "");
        assert_eq!(bigrammize(""), "");
    }

    #[test]
    fn build_fts_query_is_safe_and_or_joined() {
        // Empty / punctuation-only → None (no indexable content).
        assert!(build_fts_query("").is_none());
        assert!(build_fts_query("   ").is_none());
        assert!(build_fts_query("，。！").is_none());
        // CJK question → OR of bigrams, each quoted.
        assert_eq!(
            build_fts_query("退款条件").unwrap(),
            "\"退款\" OR \"款条\" OR \"条件\""
        );
        // Exact code preserved as a single quoted token.
        assert_eq!(build_fts_query("E1021").unwrap(), "\"e1021\"");
        // FTS5 operators / quotes in user text are neutralized (quoted literals),
        // never parsed as query syntax.
        let q = build_fts_query("a OR b* AND (c) \"x\"").unwrap();
        assert!(q.contains("\"a\""));
        assert!(q.contains("\"or\"") || q.contains("\"b\"")); // 'or' is a literal token here
        assert!(!q.contains("b*"), "wildcard must not leak: {q}");
    }

    #[test]
    fn fts_special_chars_do_not_error() {
        let path = tmp_db();
        let conn = open_db(&path).unwrap();
        replace_doc_chunks(
            &conn,
            "d",
            0,
            &[mk_chunk("c1", "退款需要在七天内申请并提供订单编号", vec![])],
        )
        .unwrap();
        // These would be FTS5 syntax errors if not quoted/handled.
        for q in ["\"", "OR", "a AND b", "*", "(x", "NEAR/2", "，、。"] {
            let r = fts_rows(&conn, q, 10);
            assert!(r.is_ok(), "query {q:?} errored: {r:?}");
        }
        drop(conn);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn cjk_paraphrase_recalls_via_keyword_lane() {
        // The D2 headline: a natural-language question worded differently from
        // the source sentence must hit the keyword lane (old phrase query → 0).
        let path = tmp_db();
        let conn = open_db(&path).unwrap();
        replace_doc_chunks(
            &conn,
            "refund",
            0,
            &[mk_chunk(
                "c1",
                "退款需要在购买后七天内提出申请，并提供订单编号",
                vec![],
            )],
        )
        .unwrap();
        // Keyword-only lane (weight_vector 0). Shares the bigram "退款".
        let hits = hybrid_search(&conn, &[], "退款要满足什么条件", 5, 0.0, 1.0).unwrap();
        assert!(
            !hits.is_empty(),
            "paraphrase should recall via keyword lane, got nothing"
        );
        drop(conn);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn migrates_legacy_trigram_fts_to_search_text() {
        // Simulate a legacy DB: chunks table without search_text + a trigram FTS
        // on `text`. open_db's ensure_fts_schema must migrate it in place.
        let path = tmp_db();
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE chunks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, chunk_id TEXT, doc_id TEXT,
                    doc_name TEXT DEFAULT '', text TEXT NOT NULL, heading_path TEXT,
                    page INTEGER, char_start INTEGER DEFAULT 0, char_end INTEGER DEFAULT 0,
                    order_index INTEGER DEFAULT 0
                 );
                 CREATE VIRTUAL TABLE chunks_fts USING fts5(text, content='chunks', content_rowid='id', tokenize='trigram');
                 CREATE TRIGGER chunks_ai AFTER INSERT ON chunks BEGIN
                    INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
                 END;",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chunks(chunk_id,doc_id,doc_name,text) VALUES ('c','refund','r.md','退款需要在七天内申请')",
                [],
            )
            .unwrap();
        }
        // Re-open through the real path → triggers migration + backfill + rebuild.
        let conn = open_db(&path).unwrap();
        assert!(column_exists(&conn, "chunks", "search_text").unwrap());
        let hits = fts_rows(&conn, "退款要什么条件", 5).unwrap();
        assert!(
            !hits.is_empty(),
            "migrated legacy DB must recall CJK paraphrase"
        );
        drop(conn);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn hybrid_fuses_vector_and_keyword_lanes() {
        let path = tmp_db();
        let conn = open_db(&path).unwrap();
        replace_doc_chunks(
            &conn,
            "d",
            2,
            &[
                mk_chunk("c1", "rust memory safety and ownership", vec![1.0, 0.0]),
                mk_chunk("c2", "cats are independent pets", vec![0.0, 1.0]),
                mk_chunk("c3", "the weather is nice today", vec![0.9, 0.1]),
            ],
        )
        .unwrap();

        // Query vector points at c2's direction, but the keyword "memory" only
        // matches c1 — hybrid must surface BOTH (vector lane c2, keyword lane c1).
        let hits = hybrid_search(&conn, &[0.0, 1.0], "memory", 3, 1.0, 1.0).unwrap();
        let ids: Vec<&str> = hits.iter().map(|(c, _)| c.id.as_str()).collect();
        assert!(
            ids.contains(&"c1"),
            "keyword lane should surface c1: {ids:?}"
        );
        assert!(
            ids.contains(&"c2"),
            "vector lane should surface c2: {ids:?}"
        );

        // Pure vector (keyword weight 0): query [1,0] → c1 ranks first, keyword ignored.
        let v = hybrid_search(&conn, &[1.0, 0.0], "nonexistentword", 3, 1.0, 0.0).unwrap();
        assert_eq!(v[0].0.id, "c1");

        drop(conn);
        std::fs::remove_file(&path).ok();
    }
}
