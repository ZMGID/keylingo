//! Knowledge base (RAG) — storage layer + vector search.
//!
//! MVP design:
//! - Multiple libraries, each bound to one `(embedding_provider, model, dim)`.
//! - Vectors stored as plain `f32` in a per-library JSON file; search is an
//!   exact brute-force cosine scan in Rust.
//!   ponytail: brute-force cosine over a loaded JSON file. Fine for a desktop
//!   KB (thousands–tens-of-thousands of chunks). Swap to sqlite-vec / LanceDB
//!   behind a trait if a library ever grows past ~1e5 chunks.
//! - Parsing / chunking / embedding live in `ingest.rs` (PR2); this file owns
//!   the on-disk layout, CRUD, and retrieval math only.
//!
//! Layout: `{app_data}/knowledge_base/`
//! ```text
//! libraries.json            # Vec<KnowledgeLibrary>
//! <kb_id>/docs.json         # Vec<KnowledgeDocument>
//! <kb_id>/chunks.json       # Vec<KnowledgeChunk> (text + metadata + embedding)
//! <kb_id>/sources/<file>    # original file snapshots
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::storage::atomic_write;

pub mod chunking;
pub mod commands;
pub mod embeddings;
pub mod ingest;
pub mod parse;
pub mod process;
pub mod rerank;
pub mod retrieval;
pub mod store;

#[cfg(test)]
mod eval_tests;
#[cfg(test)]
mod live_e2e_tests;

/// A knowledge library. `embedding_dim` is 0 until the first chunk is indexed
/// (the dimension is learned from the first embedding response).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeLibrary {
    pub id: String,
    pub name: String,
    pub embedding_provider_id: String,
    pub embedding_model: String,
    #[serde(default)]
    pub embedding_dim: usize,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub doc_count: usize,
    #[serde(default)]
    pub chunk_count: usize,
    /// 每次 embedding 请求打包的片段数。0 = 用内置默认（`ingest::DEFAULT_EMBED_BATCH`）。
    /// 只影响后续索引的请求分批，不改变已存向量，改它无需重建。
    #[serde(default)]
    pub embed_batch_size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocStatus {
    Indexing,
    Ready,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDocument {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub chunk_count: usize,
    pub status: DocStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: i64,
}

/// One indexed chunk. `embedding` is the dense vector; everything else is
/// citation metadata so a retrieval hit can point back to the source.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeChunk {
    pub id: String,
    pub doc_id: String,
    pub doc_name: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
    #[serde(default)]
    pub char_start: usize,
    #[serde(default)]
    pub char_end: usize,
    #[serde(default)]
    pub order_index: usize,
    pub embedding: Vec<f32>,
}

/// A retrieval hit: the chunk plus its cosine score and which library it came
/// from (chunks from multiple libraries can be merged in one search).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoredChunk {
    pub kb_id: String,
    pub score: f32,
    #[serde(flatten)]
    pub chunk: KnowledgeChunk,
}

// ===== id generation (dependency-free) =====

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

fn gen_id(prefix: &str) -> String {
    let millis = chrono::Local::now().timestamp_millis();
    let n = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    // Separator between timestamp and counter so distinct (millis, n) pairs
    // can't collapse to the same hex string.
    format!("{prefix}_{millis:x}_{n:x}")
}

fn validate_kb_id(id: &str) -> Result<(), String> {
    let valid = id.starts_with("kb_")
        && id.len() > 3
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if valid {
        Ok(())
    } else {
        Err(format!("Invalid knowledge base id: {id}"))
    }
}

// ===== paths =====

pub fn kb_root(app: &AppHandle) -> Result<PathBuf, String> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir unavailable: {e}"))?;
    let dir = base.join("knowledge_base");
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| format!("create knowledge_base dir: {e}"))?;
    }
    Ok(dir)
}

pub fn sources_dir(app: &AppHandle, kb_id: &str) -> Result<PathBuf, String> {
    sources_dir_at(&kb_root(app)?, kb_id)
}

// ===== root-injectable cores (testable without an AppHandle) =====
//
// Every disk op funnels through a `*_at(root, …)` core so the integration
// tests can run the full create → index → search → delete cycle against a temp
// directory. The public `app`-taking functions just resolve `kb_root(app)` and
// delegate.

fn kb_dir_at(root: &Path, kb_id: &str) -> Result<PathBuf, String> {
    validate_kb_id(kb_id)?;
    Ok(root.join(kb_id))
}

fn sources_dir_at(root: &Path, kb_id: &str) -> Result<PathBuf, String> {
    let dir = kb_dir_at(root, kb_id)?.join("sources");
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| format!("create sources dir: {e}"))?;
    }
    Ok(dir)
}

fn load_libraries_at(root: &Path) -> Result<Vec<KnowledgeLibrary>, String> {
    let path = root.join("libraries.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path).map_err(|e| format!("read libraries.json: {e}"))?;
    match serde_json::from_str(&content) {
        Ok(libs) => Ok(libs),
        Err(e) => {
            // libraries.json is the root of every KB operation. If it's
            // corrupt (manual edit / FS damage), don't brick the whole panel:
            // back the bad file up and start empty so the user can rebuild.
            eprintln!(
                "libraries.json corrupt ({e}); backing up to libraries.json.bak and starting empty"
            );
            let _ = fs::rename(&path, root.join("libraries.json.bak"));
            Ok(Vec::new())
        }
    }
}

fn save_libraries_at(root: &Path, libs: &[KnowledgeLibrary]) -> Result<(), String> {
    let content =
        serde_json::to_string_pretty(libs).map_err(|e| format!("serialize libraries: {e}"))?;
    atomic_write(&root.join("libraries.json"), &content, "libraries")
}

fn get_library_at(root: &Path, kb_id: &str) -> Result<KnowledgeLibrary, String> {
    load_libraries_at(root)?
        .into_iter()
        .find(|l| l.id == kb_id)
        .ok_or_else(|| format!("Knowledge base not found: {kb_id}"))
}

fn create_library_at(
    root: &Path,
    name: &str,
    provider_id: &str,
    model: &str,
) -> Result<KnowledgeLibrary, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Knowledge base name is empty".to_string());
    }
    if provider_id.trim().is_empty() || model.trim().is_empty() {
        return Err("Embedding provider and model are required".to_string());
    }
    let now = chrono::Local::now().timestamp();
    let lib = KnowledgeLibrary {
        id: gen_id("kb"),
        name: name.to_string(),
        embedding_provider_id: provider_id.to_string(),
        embedding_model: model.to_string(),
        embedding_dim: 0,
        created_at: now,
        updated_at: now,
        doc_count: 0,
        chunk_count: 0,
        embed_batch_size: 0,
    };
    let mut libs = load_libraries_at(root)?;
    libs.push(lib.clone());
    save_libraries_at(root, &libs)?;
    Ok(lib)
}

fn rename_library_at(root: &Path, kb_id: &str, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Knowledge base name is empty".to_string());
    }
    let mut libs = load_libraries_at(root)?;
    let lib = libs
        .iter_mut()
        .find(|l| l.id == kb_id)
        .ok_or_else(|| format!("Knowledge base not found: {kb_id}"))?;
    lib.name = name.to_string();
    lib.updated_at = chrono::Local::now().timestamp();
    save_libraries_at(root, &libs)
}

fn delete_library_at(root: &Path, kb_id: &str) -> Result<(), String> {
    let mut libs = load_libraries_at(root)?;
    let before = libs.len();
    libs.retain(|l| l.id != kb_id);
    if libs.len() == before {
        return Err(format!("Knowledge base not found: {kb_id}"));
    }
    save_libraries_at(root, &libs)?;
    if let Ok(dir) = kb_dir_at(root, kb_id) {
        let _ = fs::remove_dir_all(dir);
    }
    Ok(())
}

fn refresh_library_counts_at(root: &Path, kb_id: &str) -> Result<(), String> {
    let (ndocs, nchunks) = store::counts(&open_kb_at(root, kb_id)?)?;
    let mut libs = load_libraries_at(root)?;
    if let Some(lib) = libs.iter_mut().find(|l| l.id == kb_id) {
        lib.doc_count = ndocs;
        lib.chunk_count = nchunks;
        lib.updated_at = chrono::Local::now().timestamp();
        save_libraries_at(root, &libs)?;
    }
    Ok(())
}

/// Persist the learned embedding dimension on the library record (set on first
/// successful index; dimension is fixed per library).
fn set_library_dim_at(root: &Path, kb_id: &str, dim: usize) -> Result<(), String> {
    if dim == 0 {
        return Ok(());
    }
    let mut libs = load_libraries_at(root)?;
    if let Some(lib) = libs.iter_mut().find(|l| l.id == kb_id) {
        if lib.embedding_dim != dim {
            lib.embedding_dim = dim;
            lib.updated_at = chrono::Local::now().timestamp();
            save_libraries_at(root, &libs)?;
        }
    }
    Ok(())
}

// ===== per-kb SQLite store access =====

fn kb_db_path(root: &Path, kb_id: &str) -> Result<PathBuf, String> {
    Ok(kb_dir_at(root, kb_id)?.join("store.db"))
}

fn open_kb_at(root: &Path, kb_id: &str) -> Result<rusqlite::Connection, String> {
    let conn = store::open_db(&kb_db_path(root, kb_id)?)?;
    migrate_json_if_needed(root, kb_id, &conn)?;
    Ok(conn)
}

/// One-time migration of legacy V1 `docs.json` / `chunks.json` into store.db,
/// then rename them so we don't re-import. No-op once the store has data.
fn migrate_json_if_needed(
    root: &Path,
    kb_id: &str,
    conn: &rusqlite::Connection,
) -> Result<(), String> {
    let dir = kb_dir_at(root, kb_id)?;
    let docs_json = dir.join("docs.json");
    if !docs_json.exists() || store::counts(conn)?.0 > 0 {
        return Ok(());
    }
    let docs: Vec<KnowledgeDocument> = fs::read_to_string(&docs_json)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let chunks_json = dir.join("chunks.json");
    let chunks: Vec<KnowledgeChunk> = fs::read_to_string(&chunks_json)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let dim = chunks.first().map(|c| c.embedding.len()).unwrap_or(0);
    for doc in &docs {
        let _ = store::insert_doc(conn, doc);
    }
    let mut by_doc: std::collections::HashMap<String, Vec<KnowledgeChunk>> =
        std::collections::HashMap::new();
    for c in chunks {
        by_doc.entry(c.doc_id.clone()).or_default().push(c);
    }
    for (doc_id, cs) in by_doc {
        let _ = store::replace_doc_chunks(conn, &doc_id, dim, &cs);
    }
    let _ = fs::rename(&docs_json, dir.join("docs.json.migrated"));
    let _ = fs::rename(&chunks_json, dir.join("chunks.json.migrated"));
    eprintln!(
        "knowledge_base: migrated {kb_id} JSON → store.db ({} docs)",
        docs.len()
    );
    Ok(())
}

fn load_docs_at(root: &Path, kb_id: &str) -> Result<Vec<KnowledgeDocument>, String> {
    store::load_docs(&open_kb_at(root, kb_id)?)
}

fn insert_doc_at(root: &Path, kb_id: &str, doc: &KnowledgeDocument) -> Result<(), String> {
    store::insert_doc(&open_kb_at(root, kb_id)?, doc)
}

fn doc_by_hash_at(
    root: &Path,
    kb_id: &str,
    hash: &str,
) -> Result<Option<KnowledgeDocument>, String> {
    store::doc_by_hash(&open_kb_at(root, kb_id)?, hash)
}

fn set_doc_status_at(
    root: &Path,
    kb_id: &str,
    doc_id: &str,
    status: DocStatus,
    count: usize,
    error: Option<&str>,
) -> Result<(), String> {
    store::set_doc_status(&open_kb_at(root, kb_id)?, doc_id, status, count, error)
}

fn replace_doc_chunks_at(
    root: &Path,
    kb_id: &str,
    doc_id: &str,
    dim: usize,
    chunks: &[KnowledgeChunk],
) -> Result<(), String> {
    store::replace_doc_chunks(&open_kb_at(root, kb_id)?, doc_id, dim, chunks)
}

fn clear_chunks_at(root: &Path, kb_id: &str) -> Result<(), String> {
    store::clear_chunks(&open_kb_at(root, kb_id)?)
}

fn delete_document_at(root: &Path, kb_id: &str, doc_id: &str) -> Result<(), String> {
    let removed = store::delete_doc(&open_kb_at(root, kb_id)?, doc_id)?;
    if !removed {
        return Err(format!("Document not found: {doc_id}"));
    }
    // remove source snapshot(s) for this doc
    if let Ok(dir) = sources_dir_at(root, kb_id) {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&format!("{doc_id}__"))
                {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
    refresh_library_counts_at(root, kb_id)
}

/// Cross-library hybrid (vector + keyword RRF) search, top-k best-first.
/// Test-only: production retrieval goes through `retrieval::retrieve`; this
/// remains as the unit under test for the multi-library fusion/sort/truncate path.
#[cfg(test)]
fn search_at(
    root: &Path,
    kb_ids: &[String],
    query: &[f32],
    query_text: &str,
    top_k: usize,
    weight_vector: f32,
    weight_keyword: f32,
) -> Result<Vec<ScoredChunk>, String> {
    let mut all: Vec<ScoredChunk> = Vec::new();
    for kb_id in kb_ids {
        // Tolerate a single broken/corrupt library: skip (logged) so it can't
        // starve the rest of a cross-library search.
        match open_kb_at(root, kb_id).and_then(|c| {
            store::hybrid_search(&c, query, query_text, top_k, weight_vector, weight_keyword)
        }) {
            Ok(hits) => {
                for (chunk, score) in hits {
                    all.push(ScoredChunk {
                        kb_id: kb_id.clone(),
                        score,
                        chunk,
                    });
                }
            }
            Err(e) => eprintln!("kb search: skipping library {kb_id}: {e}"),
        }
    }
    all.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    all.truncate(top_k);
    Ok(all)
}

// ===== public app wrappers =====

pub fn load_libraries(app: &AppHandle) -> Result<Vec<KnowledgeLibrary>, String> {
    load_libraries_at(&kb_root(app)?)
}

fn save_libraries(app: &AppHandle, libs: &[KnowledgeLibrary]) -> Result<(), String> {
    save_libraries_at(&kb_root(app)?, libs)
}

pub fn get_library(app: &AppHandle, kb_id: &str) -> Result<KnowledgeLibrary, String> {
    validate_kb_id(kb_id)?;
    get_library_at(&kb_root(app)?, kb_id)
}

pub fn create_library(
    app: &AppHandle,
    name: &str,
    provider_id: &str,
    model: &str,
) -> Result<KnowledgeLibrary, String> {
    create_library_at(&kb_root(app)?, name, provider_id, model)
}

pub fn rename_library(app: &AppHandle, kb_id: &str, name: &str) -> Result<(), String> {
    validate_kb_id(kb_id)?;
    rename_library_at(&kb_root(app)?, kb_id, name)
}

pub fn delete_library(app: &AppHandle, kb_id: &str) -> Result<(), String> {
    validate_kb_id(kb_id)?;
    delete_library_at(&kb_root(app)?, kb_id)
}

/// Recompute and persist `doc_count` / `chunk_count` / `embedding_dim` on the
/// library record from its docs + chunks. Called after any ingest/delete.
pub fn refresh_library_counts(app: &AppHandle, kb_id: &str) -> Result<(), String> {
    refresh_library_counts_at(&kb_root(app)?, kb_id)
}

/// Persist the learned embedding dimension (set on first successful index).
pub fn set_library_dim(app: &AppHandle, kb_id: &str, dim: usize) -> Result<(), String> {
    set_library_dim_at(&kb_root(app)?, kb_id, dim)
}

pub fn load_docs(app: &AppHandle, kb_id: &str) -> Result<Vec<KnowledgeDocument>, String> {
    load_docs_at(&kb_root(app)?, kb_id)
}

pub fn insert_doc(app: &AppHandle, kb_id: &str, doc: &KnowledgeDocument) -> Result<(), String> {
    insert_doc_at(&kb_root(app)?, kb_id, doc)
}

pub fn doc_by_hash(
    app: &AppHandle,
    kb_id: &str,
    hash: &str,
) -> Result<Option<KnowledgeDocument>, String> {
    doc_by_hash_at(&kb_root(app)?, kb_id, hash)
}

pub fn set_doc_status(
    app: &AppHandle,
    kb_id: &str,
    doc_id: &str,
    status: DocStatus,
    count: usize,
    error: Option<&str>,
) -> Result<(), String> {
    set_doc_status_at(&kb_root(app)?, kb_id, doc_id, status, count, error)
}

/// Replace all of a document's chunks (used by indexing). `dim` lazily creates
/// the per-library vector table.
pub fn replace_doc_chunks(
    app: &AppHandle,
    kb_id: &str,
    doc_id: &str,
    dim: usize,
    chunks: &[KnowledgeChunk],
) -> Result<(), String> {
    replace_doc_chunks_at(&kb_root(app)?, kb_id, doc_id, dim, chunks)
}

/// Drop every chunk + vector (used by full reindex before refilling).
pub fn clear_chunks(app: &AppHandle, kb_id: &str) -> Result<(), String> {
    clear_chunks_at(&kb_root(app)?, kb_id)
}

/// Remove a document and all its chunks + source snapshot, then refresh counts.
pub fn delete_document(app: &AppHandle, kb_id: &str, doc_id: &str) -> Result<(), String> {
    delete_document_at(&kb_root(app)?, kb_id, doc_id)
}

/// Startup self-heal: any document left in `indexing` is stale (its indexing
/// task died with the app / crashed / was killed mid-embed). Flip it to `error`
/// so the UI stops spinning forever and the user can retry. Returns how many
/// were healed.
fn heal_stale_indexing_at(root: &Path) -> usize {
    let mut healed = 0usize;
    for lib in load_libraries_at(root).unwrap_or_default() {
        let Ok(conn) = open_kb_at(root, &lib.id) else {
            continue;
        };
        let Ok(docs) = store::load_docs(&conn) else {
            continue;
        };
        let mut changed = false;
        for doc in &docs {
            if doc.status == DocStatus::Indexing {
                let _ = store::set_doc_status(
                    &conn,
                    &doc.id,
                    DocStatus::Error,
                    doc.chunk_count,
                    Some("索引被中断，请重试 / Indexing was interrupted; please retry"),
                );
                changed = true;
                healed += 1;
            }
        }
        drop(conn);
        if changed {
            let _ = refresh_library_counts_at(root, &lib.id);
        }
    }
    healed
}

/// Call once at app startup (no indexing in flight) to clear stale `indexing`
/// statuses left by a previous run that didn't finish.
pub fn heal_stale_indexing(app: &AppHandle) {
    if let Ok(root) = kb_root(app) {
        let n = heal_stale_indexing_at(&root);
        if n > 0 {
            eprintln!("knowledge_base: healed {n} stale 'indexing' document(s) → error");
        }
    }
}

/// 懒触发版：不在 app 启动时跑（开 SQLite 会拖慢首窗），改为用户第一次真正访问知识库
/// （打开知识库面板 → `kb_list_libraries`）时跑，且一个进程内最多跑一次。不用知识库则永不跑。
///
/// 前提（调用点必须保证）：触发时本会话不能有正在进行的 ingest。`heal_stale_indexing_at`
/// 不持 `kb_lock_for`，会把任意 `Indexing` 文档直接刷成 `Error`；当前唯一调用点是「上传前
/// 首次列库」+ `Once`，此刻不可能有 in-flight ingest，故安全。若以后要把它挪到上传后的刷新
/// 路径，必须改为持 kb 锁、或排除当前正在索引的文档，否则会误判运行中的任务为中断。
pub fn heal_stale_indexing_once(app: &AppHandle) {
    static HEAL_ONCE: std::sync::Once = std::sync::Once::new();
    HEAL_ONCE.call_once(|| heal_stale_indexing(app));
}

/// System-prompt segment injected when the conversation has knowledge bases
/// attached. Tells the model the user's docs are already indexed (don't ask for
/// re-upload), to prefer `knowledge_search`, and to cite passages inline as
/// `[n]`. When `force` is set, the "prefer" nudge becomes a hard "must search
/// first" directive. Returns None when nothing resolvable is attached.
pub fn mount_system_prompt(app: &AppHandle, kb_ids: &[String], force: bool) -> Option<String> {
    if kb_ids.is_empty() {
        return None;
    }
    let libs = load_libraries(app).unwrap_or_default();
    let names: Vec<String> = kb_ids
        .iter()
        .filter_map(|id| libs.iter().find(|l| &l.id == id).map(|l| l.name.clone()))
        .collect();
    if names.is_empty() {
        return None;
    }
    let names_str = names.join(", ");
    Some(if force {
        format!(
            "This conversation has knowledge bases attached: {names_str}. **Forced retrieval is ON: you MUST call knowledge_search before every answer — do not skip it**, even if you think you already know the answer; search first to confirm. The documents are already indexed, so do not ask the user to re-upload files. When you use a retrieved passage, cite its source number inline as [n]; only if nothing relevant is found, say the knowledge base doesn't cover it."
        )
    } else {
        format!(
            "This conversation has knowledge bases attached: {names_str}. When the user's question may relate to these documents, prefer calling knowledge_search first — the documents are already indexed, so do not ask the user to re-upload files. When you use a retrieved passage, cite its source number inline as [n] (the number shown before each returned passage) so the user can trace it; only if nothing relevant is found, say the knowledge base doesn't cover it."
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kb_id_validation() {
        assert!(validate_kb_id("kb_abc123").is_ok());
        assert!(validate_kb_id("kb_a-b_c").is_ok());
        assert!(validate_kb_id("conv_x").is_err());
        assert!(validate_kb_id("kb_../etc").is_err());
        assert!(validate_kb_id("kb_").is_err());
    }

    // ===== full storage + retrieval e2e (temp dir, no AppHandle / network) =====

    fn temp_root() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("kivio-kb-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn doc(id: &str, name: &str, chunks: usize) -> KnowledgeDocument {
        KnowledgeDocument {
            id: id.to_string(),
            name: name.to_string(),
            size_bytes: 10,
            hash: format!("h-{id}"),
            chunk_count: chunks,
            status: DocStatus::Ready,
            error: None,
            created_at: 0,
        }
    }

    fn chunk_emb(
        id: &str,
        doc_id: &str,
        name: &str,
        heading: Option<&str>,
        emb: Vec<f32>,
    ) -> KnowledgeChunk {
        KnowledgeChunk {
            id: id.to_string(),
            doc_id: doc_id.to_string(),
            doc_name: name.to_string(),
            text: format!("text of {id}"),
            heading_path: heading.map(|s| s.to_string()),
            page: None,
            char_start: 0,
            char_end: 0,
            order_index: 0,
            embedding: emb,
        }
    }

    #[test]
    fn e2e_create_index_search_delete_cycle() {
        let root = temp_root();

        // 1) create a library
        let lib = create_library_at(&root, "Docs", "openai", "text-embedding-3-small").unwrap();
        let kb = lib.id.clone();
        assert_eq!(load_libraries_at(&root).unwrap().len(), 1);

        // 2) ingest output: two docs, three chunks with known vectors
        insert_doc_at(&root, &kb, &doc("doc_a", "a.md", 2)).unwrap();
        insert_doc_at(&root, &kb, &doc("doc_b", "b.md", 1)).unwrap();
        replace_doc_chunks_at(
            &root,
            &kb,
            "doc_a",
            3,
            &[
                chunk_emb(
                    "c1",
                    "doc_a",
                    "a.md",
                    Some("Intro > Setup"),
                    vec![1.0, 0.0, 0.0],
                ),
                chunk_emb("c2", "doc_a", "a.md", None, vec![0.0, 1.0, 0.0]),
            ],
        )
        .unwrap();
        replace_doc_chunks_at(
            &root,
            &kb,
            "doc_b",
            3,
            &[chunk_emb("c3", "doc_b", "b.md", None, vec![0.9, 0.1, 0.0])],
        )
        .unwrap();
        set_library_dim_at(&root, &kb, 3).unwrap();

        // 3) counts + dimension
        refresh_library_counts_at(&root, &kb).unwrap();
        let lib = get_library_at(&root, &kb).unwrap();
        assert_eq!(lib.doc_count, 2);
        assert_eq!(lib.chunk_count, 3);
        assert_eq!(lib.embedding_dim, 3);

        // 4) search (vec0 cosine): query near [1,0,0] → c1 (exact) then c3 (close)
        let hits = search_at(&root, &[kb.clone()], &[1.0, 0.0, 0.0], "", 2, 1.0, 0.0).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].chunk.id, "c1");
        assert_eq!(hits[0].chunk.doc_name, "a.md");
        assert_eq!(hits[0].chunk.heading_path.as_deref(), Some("Intro > Setup"));
        assert_eq!(hits[1].chunk.id, "c3");
        assert!(hits[0].score >= hits[1].score);

        // 5) delete doc_a → its chunks gone, doc_b intact, counts refreshed
        delete_document_at(&root, &kb, "doc_a").unwrap();
        let remaining = load_docs_at(&root, &kb).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "doc_b");
        let lib = get_library_at(&root, &kb).unwrap();
        assert_eq!(lib.doc_count, 1);
        assert_eq!(lib.chunk_count, 1);
        let hits = search_at(&root, &[kb.clone()], &[1.0, 0.0, 0.0], "", 5, 1.0, 0.0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].chunk.doc_id, "doc_b");

        // 6) delete library removes its directory
        delete_library_at(&root, &kb).unwrap();
        assert!(load_libraries_at(&root).unwrap().is_empty());
        assert!(!root.join(&kb).exists());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn e2e_multi_library_search_merges_and_ranks() {
        let root = temp_root();
        let a = create_library_at(&root, "A", "openai", "m").unwrap().id;
        let b = create_library_at(&root, "B", "openai", "m").unwrap().id;
        replace_doc_chunks_at(
            &root,
            &a,
            "d",
            2,
            &[chunk_emb("a1", "d", "a.md", None, vec![1.0, 0.0])],
        )
        .unwrap();
        replace_doc_chunks_at(
            &root,
            &b,
            "d",
            2,
            &[chunk_emb("b1", "d", "b.md", None, vec![0.2, 1.0])],
        )
        .unwrap();

        // Query closest to a1; both libraries searched, hit tagged with its kb id.
        let hits = search_at(&root, &[a.clone(), b.clone()], &[1.0, 0.0], "", 5, 1.0, 0.0).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].chunk.id, "a1");
        assert_eq!(hits[0].kb_id, a);
        assert_eq!(hits[1].kb_id, b);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn e2e_reindex_replaces_doc_chunks_without_dup() {
        // Models the index_one replace step: drop a doc's old chunks, add new.
        let root = temp_root();
        let kb = create_library_at(&root, "L", "openai", "m").unwrap().id;
        replace_doc_chunks_at(
            &root,
            &kb,
            "doc_x",
            1,
            &[chunk_emb("old1", "doc_x", "x.md", None, vec![1.0])],
        )
        .unwrap();
        replace_doc_chunks_at(
            &root,
            &kb,
            "doc_x",
            1,
            &[
                chunk_emb("new1", "doc_x", "x.md", None, vec![1.0]),
                chunk_emb("new2", "doc_x", "x.md", None, vec![1.0]),
            ],
        )
        .unwrap();

        let hits = search_at(&root, &[kb.clone()], &[1.0], "", 5, 1.0, 0.0).unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|c| c.chunk.id.starts_with("new")));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rename_and_missing_library_errors() {
        let root = temp_root();
        let kb = create_library_at(&root, "Old", "openai", "m").unwrap().id;
        rename_library_at(&root, &kb, "New").unwrap();
        assert_eq!(get_library_at(&root, &kb).unwrap().name, "New");
        assert!(rename_library_at(&root, "kb_missing", "X").is_err());
        assert!(delete_document_at(&root, &kb, "doc_missing").is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn gen_id_has_separator_between_time_and_counter() {
        // prefix + millis + counter → at least two '_' so segments can't merge.
        let id = gen_id("kb");
        assert!(id.starts_with("kb_"));
        assert_eq!(id.matches('_').count(), 2, "id was {id}");
    }

    #[test]
    fn heal_flips_stale_indexing_to_error() {
        let root = temp_root();
        let kb = create_library_at(&root, "L", "openai", "m").unwrap().id;
        insert_doc_at(&root, &kb, &{
            let mut d = doc("doc_stuck", "a.md", 0);
            d.status = DocStatus::Indexing;
            d
        })
        .unwrap();
        insert_doc_at(&root, &kb, &doc("doc_ok", "b.md", 3)).unwrap();

        let healed = heal_stale_indexing_at(&root);
        assert_eq!(healed, 1);
        let docs = load_docs_at(&root, &kb).unwrap();
        let stuck = docs.iter().find(|d| d.id == "doc_stuck").unwrap();
        assert_eq!(stuck.status, DocStatus::Error);
        assert!(stuck.error.is_some());
        // healthy doc untouched
        assert_eq!(
            docs.iter().find(|d| d.id == "doc_ok").unwrap().status,
            DocStatus::Ready
        );
        // idempotent: nothing left to heal
        assert_eq!(heal_stale_indexing_at(&root), 0);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn search_skips_corrupt_library_instead_of_failing() {
        let root = temp_root();
        let good = create_library_at(&root, "good", "openai", "m").unwrap().id;
        let bad = create_library_at(&root, "bad", "openai", "m").unwrap().id;
        replace_doc_chunks_at(
            &root,
            &good,
            "d",
            2,
            &[chunk_emb("g1", "d", "g.md", None, vec![1.0, 0.0])],
        )
        .unwrap();
        // Corrupt the bad library's store.db with non-SQLite garbage.
        let bad_db = kb_db_path(&root, &bad).unwrap();
        std::fs::create_dir_all(bad_db.parent().unwrap()).unwrap();
        std::fs::write(&bad_db, "not a sqlite database").unwrap();

        let hits = search_at(
            &root,
            &[good.clone(), bad.clone()],
            &[1.0, 0.0],
            "",
            5,
            1.0,
            0.0,
        )
        .unwrap();
        assert_eq!(
            hits.len(),
            1,
            "healthy library's hit must survive a corrupt sibling"
        );
        assert_eq!(hits[0].chunk.id, "g1");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn corrupt_libraries_json_degrades_to_empty_with_backup() {
        let root = temp_root();
        std::fs::write(root.join("libraries.json"), "{ broken").unwrap();
        // Must not error — returns empty so the panel still opens.
        assert!(load_libraries_at(&root).unwrap().is_empty());
        // Bad file is backed up, not silently destroyed.
        assert!(root.join("libraries.json.bak").exists());
        std::fs::remove_dir_all(&root).ok();
    }
}
