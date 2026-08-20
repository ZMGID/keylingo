//! Static registry for Kivio's builtin (native-source) tools.
//!
//! This table is the single source of truth that replaces the previously
//! drifting hardcoded lists:
//! - exposure if-chain in `types.rs::list_native_builtin_tool_defs`
//! - dispatch match in `registry.rs::call_native_tool`
//! - `BUILTIN_NAMES` in `chat/agent/prepare.rs::disabled_builtin_tool_feedback`
//! - approval bypass list in `chat/agent/prepare.rs::builtin_tool_bypasses_approval`
//! - parallel whitelist in `chat/agent/rounds.rs::tool_call_parallel_eligible`
//! - native read-only arm in `types.rs::ChatToolDefinition::is_read_only_tool`
//!
//! Contract notes:
//! - The `parallel_safe` set is intentionally narrow: web_search/web_fetch/
//!   read plus the read-side project tools (ls/grep/find), and only when
//!   approval-free. Do not widen or narrow it here without a spec change.
//!   memory_read is read-only and approval-free but deliberately NOT
//!   parallel-safe. `agent` is also parallel-safe (multi-agent fan-out): each
//!   spawn runs isolated and is capped by the SubAgentManager semaphore
//!   (default `DEFAULT_SUB_AGENT_CONCURRENCY` = 12, user-configurable). Each
//!   `agent` call blocks until its sub-agent finishes and returns the result
//!   inline; parallelism comes from the model emitting several `agent` calls in
//!   one round.
//! - Table order is the model-facing tool list order; keep it stable.

use std::future::Future;
use std::pin::Pin;

use serde_json::Value;
use tauri::AppHandle;

use crate::native_tools::{
    build_delivery_artifact_for_path, resolve_tool_read_path, FileMutationResult,
    NativeToolWorkspace,
};
use crate::settings::{ChatNativeToolsConfig, Settings};
use crate::state::AppState;

use super::registry::NativeToolContext;
use super::types::{
    native_advisor_tool, native_bash_output_tool, native_edit_file_tool, native_glob_files_tool,
    native_kill_background_tool, native_knowledge_search_tool, native_list_dir_tool,
    native_memory_modify_tool, native_memory_read_tool, native_memory_search_tool,
    native_present_artifacts_tool, native_read_file_tool, native_run_command_tool,
    native_save_assistant_tool, native_search_files_tool,
    native_web_fetch_tool, native_web_search_tool, native_write_file_tool, ChatToolArtifact,
    ChatToolDefinition, McpToolCallResult,
};

/// Gate signature mirrors `list_native_builtin_tool_defs(native,
/// web_search_configured, memory_enabled)` so exposure stays bit-identical.
pub type NativeToolEnabledFn = fn(&ChatNativeToolsConfig, bool, bool) -> bool;

pub type NativeToolFuture<'a> =
    Pin<Box<dyn Future<Output = Result<McpToolCallResult, String>> + Send + 'a>>;

/// Full-context async call. `workspace` is already resolved by
/// `call_native_tool` before dispatch, matching the legacy behavior where
/// every native tool resolved the workspace at entry.
pub struct NativeCallCtx<'a> {
    pub app: &'a AppHandle,
    pub state: &'a AppState,
    pub settings: &'a Settings,
    pub workspace: &'a NativeToolWorkspace,
    pub arguments: &'a Value,
    pub native_ctx: Option<&'a NativeToolContext>,
}

/// How `call_native_tool` dispatches a registry entry. The blocking-vs-sync
/// split is kept explicit here so "which tools run on the blocking thread
/// pool" stays auditable in one place (see `run_blocking_file_mutation`).
pub enum NativeToolCall {
    /// Synchronous, workspace-scoped, plain-text result.
    SyncText(fn(&NativeToolWorkspace, &Value) -> Result<String, String>),
    /// Synchronous, workspace-scoped, custom tool result.
    SyncResult(fn(&NativeToolWorkspace, &Value) -> Result<McpToolCallResult, String>),
    /// spawn_blocking, plain-text result (path mutations with lock waits).
    BlockingText(fn(&NativeToolWorkspace, &Value) -> Result<String, String>),
    /// spawn_blocking, structured `FileMutationResult` (write_file/edit_file).
    BlockingMutation(fn(&NativeToolWorkspace, &Value) -> Result<FileMutationResult, String>),
    /// Full-context async call (web/memory/shell).
    Async(for<'a> fn(NativeCallCtx<'a>) -> NativeToolFuture<'a>),
    /// Conversation-scoped call (todo tools): runs before workspace
    /// resolution because it only needs the conversation id, matching the
    /// legacy `RegistryToolExecutor` special case which never resolved a
    /// workspace for todo tools.
    Conversation(for<'a> fn(&'a AppHandle, &'a str, &'a str, Value) -> NativeToolFuture<'a>),
    /// Host-mediated tool (ask_user): intercepted in
    /// `chat/agent/execute.rs::execute_ask_user_call` and must never reach
    /// the registry dispatcher.
    HostMediated,
    /// Sub-agent spawn tool (`agent`): dispatched before workspace resolution
    /// (it manages agents, not files) with the parent run context from
    /// `NativeToolContext` (depth, run_id, generation, parent conversation/
    /// tool-call id).
    SubAgent(for<'a> fn(crate::chat::sub_agent::SubAgentCallCtx<'a>) -> NativeToolFuture<'a>),
}

pub struct NativeToolEntry {
    pub name: &'static str,
    pub def: fn() -> ChatToolDefinition,
    /// Whether the tool is exposed in the model tool list for the given
    /// settings. todo/ask_user return false here because they are appended
    /// separately in `chat/commands.rs` (`append_agent_todo_tools` /
    /// `append_agent_ask_user_tools`), not via
    /// `list_native_builtin_tool_defs`.
    pub enabled: NativeToolEnabledFn,
    pub parallel_safe: bool,
    pub bypasses_approval: bool,
    pub read_only: bool,
    /// File/shell tools (read/write/edit/bash/grep/find/ls) gated by one-time
    /// per-conversation session consent. The flag lives on the entry so a rename
    /// or a newly-added tool can't silently bypass the consent gate.
    pub requires_session_consent: bool,
    pub call: NativeToolCall,
}

/// Declaration order is the model-facing exposure order (the legacy push
/// order of `list_native_builtin_tool_defs`), followed by the
/// conversation-level tools that are appended elsewhere.
pub static NATIVE_TOOLS: &[NativeToolEntry] = &[
    NativeToolEntry {
        name: "web_search",
        def: native_web_search_tool,
        enabled: |native, web_search_configured, _| native.web_search && web_search_configured,
        parallel_safe: true,
        bypasses_approval: false,
        read_only: true,
        requires_session_consent: false,
        call: NativeToolCall::Async(call_web_search),
    },
    NativeToolEntry {
        name: "web_fetch",
        def: native_web_fetch_tool,
        enabled: |native, _, _| native.web_fetch,
        parallel_safe: true,
        bypasses_approval: false,
        read_only: true,
        requires_session_consent: false,
        call: NativeToolCall::Async(call_web_fetch),
    },
    NativeToolEntry {
        name: "knowledge_search",
        def: native_knowledge_search_tool,
        enabled: |native, _, _| native.knowledge_search,
        parallel_safe: true,
        bypasses_approval: false,
        read_only: true,
        requires_session_consent: false,
        call: NativeToolCall::Async(call_knowledge_search),
    },
    NativeToolEntry {
        name: "advisor",
        def: native_advisor_tool,
        // Never auto-listed: exposure depends on `default_models.advisor` being
        // configured, which this gate can't see. Appended separately in
        // `list_enabled_tool_catalog` when configured (mirrors mixer_generate_image).
        // find_entry still resolves it here for dispatch.
        enabled: |_, _, _| false,
        parallel_safe: true,
        bypasses_approval: true,
        read_only: true,
        requires_session_consent: false,
        call: NativeToolCall::Async(call_advisor),
    },
    NativeToolEntry {
        name: "read",
        def: native_read_file_tool,
        enabled: |native, _, _| native.read_file,
        parallel_safe: true,
        bypasses_approval: false,
        read_only: true,
        requires_session_consent: true,
        call: NativeToolCall::Async(call_read_file),
    },
    // `ls` (list_dir) is folded into `read` in the GUI, so it is NOT advertised
    // there (enabled=false, kept out of the GUI's built-in tool set). kivio-code,
    // however, exposes a standalone `ls` tool via its own `core_tool_definitions`.
    // This entry exists purely so metadata lookups by the name "ls"
    // (is_read_only_tool / session-consent / bypass) resolve — without it, plan
    // mode drops `ls` and the session-consent gate skips it. `call` is unused
    // (the GUI never dispatches it; kivio-code dispatches `ls` in its own executor).
    NativeToolEntry {
        name: "ls",
        def: native_list_dir_tool,
        enabled: |_, _, _| false,
        parallel_safe: true,
        bypasses_approval: false,
        read_only: true,
        requires_session_consent: true,
        call: NativeToolCall::SyncText(crate::native_tools::list_dir),
    },
    NativeToolEntry {
        name: "grep",
        def: native_search_files_tool,
        enabled: |native, _, _| native.read_file,
        parallel_safe: true,
        bypasses_approval: false,
        read_only: true,
        requires_session_consent: true,
        call: NativeToolCall::SyncText(crate::native_tools::search_files),
    },
    NativeToolEntry {
        name: "glob",
        def: native_glob_files_tool,
        enabled: |native, _, _| native.read_file,
        parallel_safe: true,
        bypasses_approval: false,
        read_only: true,
        requires_session_consent: true,
        call: NativeToolCall::SyncText(crate::native_tools::glob_files),
    },
    NativeToolEntry {
        name: "write",
        def: native_write_file_tool,
        enabled: |native, _, _| native.write_file,
        parallel_safe: false,
        bypasses_approval: false,
        read_only: false,
        requires_session_consent: true,
        call: NativeToolCall::BlockingMutation(crate::native_tools::write_file),
    },
    NativeToolEntry {
        name: "edit",
        def: native_edit_file_tool,
        enabled: |native, _, _| native.edit_file,
        parallel_safe: false,
        bypasses_approval: false,
        read_only: false,
        requires_session_consent: true,
        call: NativeToolCall::BlockingMutation(crate::native_tools::edit_file),
    },
    NativeToolEntry {
        name: "bash",
        def: native_run_command_tool,
        enabled: |native, _, _| native.run_command,
        parallel_safe: false,
        bypasses_approval: false,
        read_only: false,
        requires_session_consent: true,
        call: NativeToolCall::Async(call_run_command),
    },
    // Background-command observability tools. Gated by the same `run_command`
    // toggle + session consent as `bash` (they expose host-shell job
    // state/control). bash_output is read-only and parallel-safe (pure
    // registry/log reads) and, with no job_id, lists all tracked jobs (folds in
    // the former list_background tool); kill_background is a control action and
    // is neither. Neither bypasses approval — they ride bash's consent.
    NativeToolEntry {
        name: "bash_output",
        def: native_bash_output_tool,
        enabled: |native, _, _| native.run_command,
        parallel_safe: true,
        bypasses_approval: false,
        read_only: true,
        requires_session_consent: true,
        call: NativeToolCall::Async(call_bash_output),
    },
    NativeToolEntry {
        name: "kill_background",
        def: native_kill_background_tool,
        enabled: |native, _, _| native.run_command,
        parallel_safe: false,
        bypasses_approval: false,
        read_only: false,
        requires_session_consent: true,
        call: NativeToolCall::Async(call_kill_background),
    },
    NativeToolEntry {
        // 仅在「对话搭建专家」会话里手动 append(见 commands.rs);全局列表永不暴露。
        // call_native_tool 按名分发、调用期不复查 enabled,故手动 append 仍可执行。
        name: "save_assistant",
        def: native_save_assistant_tool,
        enabled: |_, _, _| false,
        parallel_safe: false,
        bypasses_approval: false,
        read_only: false,
        requires_session_consent: false,
        call: NativeToolCall::Async(call_save_assistant),
    },
    NativeToolEntry {
        name: "present_artifacts",
        def: native_present_artifacts_tool,
        enabled: |_, _, _| true,
        parallel_safe: false,
        bypasses_approval: true,
        read_only: true,
        requires_session_consent: false,
        call: NativeToolCall::SyncResult(call_present_artifacts),
    },
    NativeToolEntry {
        name: "memory_read",
        def: native_memory_read_tool,
        enabled: |_, _, memory_enabled| memory_enabled,
        parallel_safe: false,
        bypasses_approval: true,
        read_only: true,
        requires_session_consent: false,
        call: NativeToolCall::Async(call_memory_read),
    },
    NativeToolEntry {
        name: "memory_modify",
        def: native_memory_modify_tool,
        enabled: |_, _, memory_enabled| memory_enabled,
        parallel_safe: false,
        bypasses_approval: true,
        read_only: false,
        requires_session_consent: false,
        call: NativeToolCall::Async(call_memory_modify),
    },
    NativeToolEntry {
        name: "memory_search",
        def: native_memory_search_tool,
        enabled: |_, _, memory_enabled| memory_enabled,
        parallel_safe: false,
        bypasses_approval: true,
        read_only: true,
        requires_session_consent: false,
        call: NativeToolCall::Async(call_memory_search),
    },
    // Conversation-level tools below are appended in chat/commands.rs and
    // never exposed via list_native_builtin_tool_defs (enabled = false).
    NativeToolEntry {
        name: crate::chat::todo::TODO_WRITE_TOOL_NAME,
        def: crate::chat::todo::todo_write_tool,
        enabled: |_, _, _| false,
        parallel_safe: false,
        bypasses_approval: true,
        read_only: false,
        requires_session_consent: false,
        call: NativeToolCall::Conversation(crate::chat::todo::handle_conversation_tool_call),
    },
    NativeToolEntry {
        name: crate::chat::ask_user::ASK_USER_TOOL_NAME,
        def: crate::chat::ask_user::ask_user_tool,
        enabled: |_, _, _| false,
        parallel_safe: false, // spec: ask_user is forced serial with batch flush
        bypasses_approval: true,
        read_only: false,
        requires_session_consent: false,
        call: NativeToolCall::HostMediated,
    },
    // Sub-agent spawn tool (P3). Appended in chat/commands.rs (via
    // `sub_agent::append_tool_definitions`) when the multi-agent toggle is on, so
    // enabled = false here. bypasses_approval = true: spawning sub-agents is
    // governed by depth + concurrency caps, not per-call approval prompts.
    NativeToolEntry {
        name: crate::chat::sub_agent::AGENT_TOOL_NAME,
        // The real schema is built per-request with the loaded role registry
        // (`sub_agent::append_tool_definitions`); the registry only needs the
        // name/shape here, so an empty role list is correct.
        def: || crate::chat::sub_agent::agent_tool(&[]),
        enabled: |_, _, _| false,
        // parallel_safe = true: each `agent` spawn runs in isolation (its own
        // synthetic conversation/generation/message history), bypasses approval,
        // and is capped by the SubAgentManager semaphore (default 12, user-
        // configurable). Concurrent fan-out is the core value of multi-agent: a
        // single round may dispatch several `agent` calls in parallel (scheduler
        // caps at MAX_PARALLEL_TOOL_CALLS_PER_ROUND = 12, semaphore at the
        // setting). Each call blocks until its sub-agent finishes and returns the
        // full result inline (Claude Code Task model).
        parallel_safe: true,
        bypasses_approval: true,
        read_only: false,
        requires_session_consent: false,
        call: NativeToolCall::SubAgent(crate::chat::sub_agent::dispatch_agent_spawn),
    },
];

pub fn find_entry(name: &str) -> Option<&'static NativeToolEntry> {
    NATIVE_TOOLS.iter().find(|entry| entry.name == name)
}

/// The native file/shell tools (Pi's 7) are gated by a single one-time
/// per-conversation **session consent** prompt — granting one authorizes
/// full-disk read/write and arbitrary command execution for the rest of that
/// conversation. Everything else (web/python/memory/todo/sub-agent/...) keeps
/// its own gating and is NOT behind this consent. Driven by the per-entry
/// `requires_session_consent` flag so a rename or new tool can't drift.
pub fn native_tool_requires_session_consent(name: &str) -> bool {
    find_entry(name).is_some_and(|entry| entry.requires_session_consent)
}

pub fn text_tool_result(content: String) -> McpToolCallResult {
    McpToolCallResult {
        content,
        is_error: false,
        raw: Value::Null,
        artifacts: Vec::new(),
        structured_content: None,
        follow_up_user_messages: Vec::new(),
    }
}

fn call_read_file(ctx: NativeCallCtx<'_>) -> NativeToolFuture<'_> {
    Box::pin(async move {
        let raw_path = ctx
            .arguments
            .get("path")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        if let Ok(path) = crate::native_tools::resolve_tool_read_path(ctx.workspace, raw_path) {
            // 目录 → 列目录（并入原 ls 工具）。offset/limit 对目录忽略，走 list_dir 默认。
            if path.is_dir() {
                let listing = crate::native_tools::list_dir(ctx.workspace, ctx.arguments)?;
                return Ok(text_tool_result(listing));
            }
            // 图片 → 三级视觉/OCR 策略（需要会话上下文取主模型能力）。
            if crate::chat::knowledge_base::process::is_image_ext(&path) {
                if let Some(nc) = ctx.native_ctx {
                    return crate::chat::commands::read_image_as_tool_result(
                        ctx.app,
                        ctx.settings,
                        &nc.conversation_id,
                        &nc.message_id,
                        &path,
                    )
                    .await;
                }
            } else if let Some(hint) = skill_backed_document_hint(&path) {
                // PDF / Word / Excel 等二进制文档：read 不解析，引导走对应 skill。
                return Ok(text_tool_result(hint));
            }
        }
        // 文本文件（及无法预解析为图片/文档的路径）→ 原同步文本读取。
        let result = crate::native_tools::read_file(ctx.workspace, ctx.arguments)?;
        super::registry::read_file_tool_result(result)
    })
}

/// PDF/Word/Excel 由内置 skill + 主机命令解析；read 工具不读二进制文档；命中时返回引导提示而非 UTF-8 报错。
fn skill_backed_document_hint(path: &std::path::Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let (skill, kind) = match ext.as_str() {
        "pdf" => ("pdf", "PDF"),
        "doc" | "docx" => ("docx", "Word 文档"),
        "xls" | "xlsx" | "xlsm" => ("xlsx", "Excel 表格"),
        _ => return None,
    };
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    Some(format!(
        "{name} 是{kind}，read 工具不解析此类文件。请改用「{skill}」skill：用 bash 在主机上提取内容（若本机有对应 CLI 或 Python）；没有可用工具时如实告诉用户。"
    ))
}

fn call_web_search(ctx: NativeCallCtx<'_>) -> NativeToolFuture<'_> {
    Box::pin(async move {
        let query = ctx
            .arguments
            .get("query")
            .and_then(|query| query.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if query.is_empty() {
            return Err("web_search query is empty".to_string());
        }
        let retry_attempts = if ctx.settings.retry_enabled {
            ctx.settings.retry_attempts as usize
        } else {
            1
        };
        let results = crate::web_search::search_web(
            ctx.state,
            &ctx.settings.lens.web_search,
            &query,
            retry_attempts,
        )
        .await?;
        let raw = serde_json::to_value(&results).unwrap_or(Value::Null);
        // 带上实际使用的搜索服务名，供前端工具卡片显示「Web search · <provider>」。
        let provider = crate::web_search::provider_label(ctx.settings.lens.web_search.provider);
        // 与内置搜索卡同构的富引用列表（title/url 必有；snippet/published_date 尽力而为），
        // 前端据此渲染可点来源目录，并把答案里的 [n] 锚到对应来源。
        let citations: Vec<serde_json::Value> = results
            .iter()
            .map(|result| {
                let snippet = (!result.content.is_empty())
                    .then(|| result.content.chars().take(400).collect::<String>());
                serde_json::json!({
                    "title": result.title,
                    "url": result.url,
                    "snippet": snippet,
                    "published_date": result.published_date,
                })
            })
            .collect();
        Ok(McpToolCallResult {
            content: crate::web_search::format_web_context(&results),
            is_error: false,
            raw,
            artifacts: Vec::new(),
            structured_content: Some(serde_json::json!({
                "type": "third_party_web_search",
                "provider": provider,
                "queries": [query],
                "citations": citations,
            })),
            follow_up_user_messages: Vec::new(),
        })
    })
}

fn call_web_fetch(ctx: NativeCallCtx<'_>) -> NativeToolFuture<'_> {
    Box::pin(async move {
        let content = crate::native_tools::web_fetch(&ctx.state.http, ctx.arguments).await?;
        Ok(text_tool_result(content))
    })
}

/// Knowledge-base retrieval (RAG). Embeds the query with each target library's
/// own embedding model, runs a cosine search, merges the hits, and returns
/// passages tagged with `[n]` citation markers plus structured hits for the UI.
fn call_knowledge_search(ctx: NativeCallCtx<'_>) -> NativeToolFuture<'_> {
    Box::pin(async move {
        use crate::chat::knowledge_base as kb;

        let query = ctx
            .arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if query.is_empty() {
            return Err("knowledge_search query is empty".to_string());
        }
        let default_top_k = (ctx.settings.knowledge_base.top_k as usize).clamp(1, 20);
        let top_k = ctx
            .arguments
            .get("top_k")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .filter(|n| *n > 0)
            .unwrap_or(default_top_k)
            .min(20);

        // Target libraries: explicit arg > conversation mount > all libraries.
        let mut kb_ids: Vec<String> = ctx
            .arguments
            .get("kb_ids")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if kb_ids.is_empty() {
            if let Some(nc) = ctx.native_ctx {
                if let Ok(conv) =
                    crate::chat::storage::load_conversation(ctx.app, &nc.conversation_id)
                {
                    kb_ids = conv.knowledge_base_ids.clone();
                }
            }
        }
        // No explicit kb_ids and nothing attached to this conversation →
        // retrieve NOTHING. Attaching libraries is an explicit per-conversation
        // choice; an empty selection must not silently fan out to every library.
        if kb_ids.is_empty() {
            return Ok(text_tool_result(
                "No knowledge base is attached to this conversation. Ask the user to attach one via the 知识库 selector before searching, then retry."
                    .to_string(),
            ));
        }

        // Build the retrieval request from settings; the orchestrator owns
        // embed → recall → fuse → rerank → threshold → context select so the
        // Retrieval Test command runs the exact same core.
        let kbcfg = &ctx.settings.knowledge_base;
        let (w_vec, w_kw) = if kbcfg.hybrid_enabled {
            (kbcfg.weight_vector, kbcfg.weight_keyword)
        } else {
            (1.0, 0.0)
        };
        let rerank = (!kbcfg.rerank_provider_id.trim().is_empty()
            && !kbcfg.rerank_model.trim().is_empty())
        .then(|| kb::retrieval::RerankConfig {
            provider_id: kbcfg.rerank_provider_id.clone(),
            model: kbcfg.rerank_model.clone(),
        });
        // Over-fetch the candidate pool (independently configurable, D4) so
        // fusion/rerank see beyond the final context size.
        let candidate_k = kbcfg.candidate_k_clamped();
        let req = kb::retrieval::RetrievalRequest {
            query: query.clone(),
            kb_ids,
            candidate_k,
            rerank_top_k: kbcfg.rerank_top_k_clamped(),
            context_top_k: top_k,
            weight_vector: w_vec,
            weight_keyword: w_kw,
            rerank,
            min_score: kbcfg.min_score.clamp(0.0, 1.0),
        };

        let resp = kb::retrieval::retrieve(ctx.app, ctx.state, ctx.settings, &req).await?;
        let all_hits = resp.hits;

        if all_hits.is_empty() {
            return Ok(text_tool_result(
                "No relevant passages found in the knowledge base.".to_string(),
            ));
        }

        let mut content = format!(
            "Found {} relevant passage(s). Cite each with its [n] marker.\n\n",
            all_hits.len()
        );
        let mut struct_hits = Vec::with_capacity(all_hits.len());
        for (i, h) in all_hits.iter().enumerate() {
            let n = i + 1;
            let src = match &h.chunk.heading_path {
                Some(hp) => format!("{} — {}", h.chunk.doc_name, hp),
                None => h.chunk.doc_name.clone(),
            };
            content.push_str(&format!("[{n}] {src}\n{}\n\n", h.chunk.text.trim()));
            struct_hits.push(serde_json::json!({
                "n": n,
                "kbId": h.kb_id,
                "docId": h.chunk.doc_id,
                "docName": h.chunk.doc_name,
                "headingPath": h.chunk.heading_path,
                "score": h.score,
                "text": h.chunk.text,
            }));
        }
        let payload = serde_json::json!({ "hits": struct_hits });
        Ok(McpToolCallResult {
            content,
            is_error: false,
            raw: payload.clone(),
            artifacts: Vec::new(),
            structured_content: Some(payload),
            follow_up_user_messages: Vec::new(),
        })
    })
}

/// Advisor consultation (executor-advisor pattern). Runs a single-shot chat
/// completion against the model configured in `default_models.advisor` and
/// returns its guidance text. No tools, no recursion. If the advisor model is
/// missing/unusable, returns a tool error (the main loop continues).
fn call_advisor(ctx: NativeCallCtx<'_>) -> NativeToolFuture<'_> {
    Box::pin(async move {
        let question = ctx
            .arguments
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if question.is_empty() {
            return Err("advisor requires a non-empty question".to_string());
        }
        let extra_context = ctx
            .arguments
            .get("context")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();

        // Resolve the advisor model (runtime second gate: the tool list is
        // cached, so the config may have been cleared since listing).
        let Some((provider_id, model)) = ctx.settings.advisor_model() else {
            return Err("No advisor model is configured under Mixer settings.".to_string());
        };
        let Some(provider) = ctx.settings.get_provider(&provider_id).cloned() else {
            return Err("Advisor provider is missing or disabled.".to_string());
        };
        if provider.api_keys.is_empty() {
            return Err("Advisor provider has no API key configured.".to_string());
        }

        let language = crate::settings::resolve_chat_language(ctx.settings);
        let system = if language.starts_with("zh") {
            "你是一位资深技术顾问，被一个正在执行任务的 AI 助手请来咨询。请针对它的问题给出清晰的诊断、根因分析和可执行的方向性建议；聚焦决策与思路，不要代写完整实现。若信息不足，指出还需要哪些关键信息。"
        } else {
            "You are a senior technical advisor consulted by an AI assistant that is executing a task. Give a clear diagnosis, root-cause analysis, and actionable direction for its question; focus on decisions and approach, do not write the full implementation for it. If information is insufficient, say what key details are missing."
        };
        let user_content = if extra_context.is_empty() {
            question.clone()
        } else {
            format!("{question}\n\n---\nContext:\n{extra_context}")
        };
        let messages = vec![
            serde_json::json!({ "role": "system", "content": system }),
            serde_json::json!({ "role": "user", "content": user_content }),
        ];
        let retry_attempts = if ctx.settings.retry_enabled {
            ctx.settings.retry_attempts as usize
        } else {
            1
        };
        // Model-aware output cap (matches sub-agents / top-level chat): a small
        // advisor model may have a real ceiling far below the global chat cap;
        // sending the raw cap makes strict providers 400. Prefer the model
        // library / provider override, fall back to the global setting.
        let max_output_tokens = crate::chat::model_metadata::chat_max_output_tokens_for_model(
            Some(&provider),
            &model,
            ctx.settings.chat.max_output_tokens,
        );
        let conversation_id = ctx
            .native_ctx
            .map(|nc| nc.conversation_id.clone())
            .unwrap_or_default();
        let message_id = ctx
            .native_ctx
            .and_then(|nc| nc.tool_call_id.clone())
            .unwrap_or_default();

        let (message, _usage) =
            crate::chat::agent::planning::call_chat_completion_message_with_usage(
                ctx.state,
                &provider,
                &model,
                messages,
                None,
                retry_attempts,
                false,
                None,
                false,
                max_output_tokens,
                &conversation_id,
                &message_id,
                "Advisor consultation",
            )
            .await?;
        let advice = crate::chat::agent::stop::assistant_content_from_api_message(&message);
        let advice = advice.trim();
        if advice.is_empty() {
            return Err("Advisor returned an empty response.".to_string());
        }
        // Structured content drives the dedicated AdvisorCard in the chat UI
        // (model + question + advice); the text form is the model-facing result.
        let structured = serde_json::json!({
            "type": "advisor",
            "model": model,
            "question": question,
            "advice": advice,
        });
        Ok(McpToolCallResult {
            content: format!("[Advisor · {model}]\n\n{advice}"),
            is_error: false,
            raw: structured.clone(),
            artifacts: Vec::new(),
            structured_content: Some(structured),
            follow_up_user_messages: Vec::new(),
        })
    })
}

fn call_memory_read(ctx: NativeCallCtx<'_>) -> NativeToolFuture<'_> {
    Box::pin(async move {
        // Runtime second gate kept on purpose: the tool list is cached, so a
        // user can disable memory between listing and calling.
        if !ctx.settings.chat_memory.enabled {
            return Err("Chat memory is disabled in Settings".to_string());
        }
        crate::chat::memory::tool_read(ctx.app, ctx.arguments)
    })
}

fn call_memory_modify(ctx: NativeCallCtx<'_>) -> NativeToolFuture<'_> {
    Box::pin(async move {
        if !ctx.settings.chat_memory.enabled {
            return Err("Chat memory is disabled in Settings".to_string());
        }
        crate::chat::memory::tool_modify(ctx.app, ctx.arguments)
    })
}

fn call_memory_search(ctx: NativeCallCtx<'_>) -> NativeToolFuture<'_> {
    Box::pin(async move {
        if !ctx.settings.chat_memory.enabled {
            return Err("Chat memory is disabled in Settings".to_string());
        }
        crate::chat::memory::tool_search(ctx.app, ctx.arguments)
    })
}

fn call_run_command(ctx: NativeCallCtx<'_>) -> NativeToolFuture<'_> {
    Box::pin(async move {
        let content = crate::native_tools::run_command(
            ctx.workspace,
            ctx.settings.chat_tools.tool_timeout_ms,
            ctx.arguments,
            Some(ctx.state),
            ctx.native_ctx.map(|c| c.conversation_id.as_str()),
        )
        .await?;
        Ok(text_tool_result(content))
    })
}

fn call_bash_output(ctx: NativeCallCtx<'_>) -> NativeToolFuture<'_> {
    Box::pin(async move {
        // 无 job_id → 列出本会话所有后台作业（并入原 list_background 工具）。
        let has_job = ctx
            .arguments
            .get("job_id")
            .and_then(|value| value.as_str())
            .map(|id| !id.trim().is_empty())
            .unwrap_or(false);
        let conversation_id = ctx.native_ctx.map(|c| c.conversation_id.as_str());
        let content = if has_job {
            crate::native_tools::bash_output(ctx.state, ctx.arguments, conversation_id)?
        } else {
            crate::native_tools::list_background(ctx.state, ctx.arguments, conversation_id)?
        };
        Ok(text_tool_result(content))
    })
}

fn call_kill_background(ctx: NativeCallCtx<'_>) -> NativeToolFuture<'_> {
    Box::pin(async move {
        let content = crate::native_tools::kill_background(
            ctx.state,
            ctx.arguments,
            ctx.native_ctx.map(|c| c.conversation_id.as_str()),
        )?;
        Ok(text_tool_result(content))
    })
}

fn call_save_assistant(ctx: NativeCallCtx<'_>) -> NativeToolFuture<'_> {
    Box::pin(async move {
        let content = crate::chat::commands::create_assistant_via_builder(ctx.app, ctx.arguments)?;
        Ok(text_tool_result(content))
    })
}

fn string_list_argument(arguments: &Value, name: &str) -> Result<Vec<String>, String> {
    let Some(values) = arguments.get(name) else {
        return Ok(Vec::new());
    };
    let values = values
        .as_array()
        .ok_or_else(|| format!("present_artifacts {name} must be an array"))?;
    let mut items = Vec::new();
    for value in values {
        let Some(item) = value
            .as_str()
            .map(str::trim)
            .filter(|item| !item.is_empty())
        else {
            continue;
        };
        if !items.iter().any(|existing| existing == item) {
            items.push(item.to_string());
        }
    }
    Ok(items)
}

fn local_file_artifact(
    workspace: &NativeToolWorkspace,
    path: &str,
) -> Result<ChatToolArtifact, String> {
    let resolved = resolve_tool_read_path(workspace, path)?;
    if !resolved.is_file() {
        return Err(format!("Not a readable file: {path}"));
    }
    build_delivery_artifact_for_path(&resolved)
}

fn call_present_artifacts(
    workspace: &NativeToolWorkspace,
    arguments: &Value,
) -> Result<McpToolCallResult, String> {
    let artifact_ids = string_list_argument(arguments, "artifact_ids")?;
    let paths = string_list_argument(arguments, "paths")?;
    if artifact_ids.is_empty() && paths.is_empty() {
        return Err("present_artifacts requires artifact_ids or paths".to_string());
    }
    if artifact_ids.len() + paths.len() > 16 {
        return Err("present_artifacts accepts at most 16 files".to_string());
    }

    // 模型常把生成图的 artifact 名（generated-image-1.png）当成 path 一起传进来。
    // 单个 path 读不到不能废掉整次调用，否则同一调用里的 artifact_ids 也一起丢，图就不显示了。
    let mut artifacts = Vec::new();
    let mut skipped = Vec::new();
    for path in &paths {
        match local_file_artifact(workspace, path) {
            Ok(artifact) => artifacts.push(artifact),
            Err(err) => skipped.push(err),
        }
    }
    if artifacts.is_empty() && artifact_ids.is_empty() {
        return Err(skipped.join("; "));
    }
    let caption = arguments
        .get("caption")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let mut structured = serde_json::json!({
        "type": "artifact_presentation",
        "artifactIds": artifact_ids,
    });
    if let Some(caption) = caption {
        structured["caption"] = Value::String(caption);
    }
    // 结果必须如实说清「展示了几个」。此前无条件写 "Selected files will be displayed"
    // 再追加一句 "Skipped ..."，模型收到的是自相矛盾的两句话（说要展示、又说跳过了），
    // 无法判断成没成，于是回空响应把整轮卡死（实测 out=4 tokens，稳定复现）。
    let shown = artifact_ids.len() + artifacts.len();
    let mut content = if shown == 1 {
        "Displayed 1 file in the response.".to_string()
    } else {
        format!("Displayed {shown} files in the response.")
    };
    if !skipped.is_empty() {
        // 措辞对准模型的真实错误：它把生成文件当本地路径传了。明说下次别再传 paths，
        // 且明确「本次展示未受影响」，避免它以为整体失败而不敢往下说话。
        content.push_str(&format!(
            "\n\nIgnored {} unreadable path(s): {}. \
             Generated files are addressed by artifact_ids only — do not pass them in paths. \
             This did not affect the files listed above.",
            skipped.len(),
            skipped.join("; "),
        ));
    }
    Ok(McpToolCallResult {
        content,
        is_error: false,
        raw: structured.clone(),
        artifacts,
        structured_content: Some(structured),
        follow_up_user_messages: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_backed_document_hint_routes_by_extension() {
        use std::path::Path;
        assert!(skill_backed_document_hint(Path::new("/a/report.pdf"))
            .unwrap()
            .contains("pdf"));
        assert!(skill_backed_document_hint(Path::new("/a/notes.docx"))
            .unwrap()
            .contains("docx"));
        // Case-insensitive on extension.
        assert!(skill_backed_document_hint(Path::new("/a/sheet.XLSX"))
            .unwrap()
            .contains("xlsx"));
        // Text and image files are NOT routed to the document-skill hint.
        assert!(skill_backed_document_hint(Path::new("/a/readme.txt")).is_none());
        assert!(skill_backed_document_hint(Path::new("/a/shot.png")).is_none());
    }

    const EXPECTED_ORDER: &[&str] = &[
        "web_search",
        "web_fetch",
        "knowledge_search",
        "advisor",
        "read",
        "ls",
        "grep",
        "glob",
        "write",
        "edit",
        "bash",
        "bash_output",
        "kill_background",
        "save_assistant",
        "present_artifacts",
        "memory_read",
        "memory_modify",
        "memory_search",
        "todo_write",
        "ask_user",
        "agent",
    ];

    #[test]
    fn session_consent_set_is_exactly_the_file_shell_tools() {
        let consent: Vec<&str> = NATIVE_TOOLS
            .iter()
            .filter(|entry| entry.requires_session_consent)
            .map(|entry| entry.name)
            .collect();
        assert_eq!(
            consent,
            [
                "read",
                "ls",
                "grep",
                "glob",
                "write",
                "edit",
                "bash",
                "bash_output",
                "kill_background"
            ],
            "session-consent set must be exactly the file/shell tools (read now also \
             lists directories; `ls` is the standalone kivio-code list_dir tool; find \
             is renamed glob) plus the background-command observability tools (gated \
             identically to bash); a new file/shell tool MUST set \
             requires_session_consent or it silently bypasses the consent gate"
        );
        // The predicate agrees with the flag, and non-file tools are excluded.
        assert!(native_tool_requires_session_consent("bash"));
        assert!(!native_tool_requires_session_consent("web_search"));
        assert!(!native_tool_requires_session_consent("present_artifacts"));
        assert!(!native_tool_requires_session_consent("memory_read"));
    }

    #[test]
    fn registry_order_and_names_match_legacy_exposure_order() {
        let names: Vec<&str> = NATIVE_TOOLS.iter().map(|entry| entry.name).collect();
        assert_eq!(names, EXPECTED_ORDER);
    }

    #[test]
    fn registry_defs_match_entry_names() {
        for entry in NATIVE_TOOLS {
            let def = (entry.def)();
            assert_eq!(def.name, entry.name, "def() name must equal entry name");
            assert_eq!(def.source, "native");
        }
    }

    #[test]
    fn parallel_safe_set_is_exactly_the_narrow_read_whitelist() {
        let parallel: Vec<&str> = NATIVE_TOOLS
            .iter()
            .filter(|entry| entry.parallel_safe)
            .map(|entry| entry.name)
            .collect();
        assert_eq!(
            parallel,
            [
                "web_search",
                "web_fetch",
                "knowledge_search",
                "advisor",
                "read",
                "ls",
                "grep",
                "glob",
                "bash_output",
                "agent",
            ],
            "parallel-safe set is intentionally narrow per agent-runtime spec; \
             bash_output joins it because it is a pure read-only registry/log read \
             (and lists jobs when given no job_id); `agent` joins it because each \
             spawn runs in isolation (own conversation/generation/message history), \
             bypasses approval, and is capped by the SubAgentManager semaphore \
             (default 12), making concurrent fan-out the core multi-agent value"
        );
    }

    #[test]
    fn approval_bypass_set_matches_legacy_list() {
        let bypass: Vec<&str> = NATIVE_TOOLS
            .iter()
            .filter(|entry| entry.bypasses_approval)
            .map(|entry| entry.name)
            .collect();
        assert_eq!(
            bypass,
            [
                "advisor",
                "present_artifacts",
                "memory_read",
                "memory_modify",
                "memory_search",
                "todo_write",
                "ask_user",
                "agent",
            ]
        );
    }

    #[test]
    fn read_only_set_matches_legacy_is_read_only_tool_arm() {
        let read_only: Vec<&str> = NATIVE_TOOLS
            .iter()
            .filter(|entry| entry.read_only)
            .map(|entry| entry.name)
            .collect();
        assert_eq!(
            read_only,
            [
                "web_search",
                "web_fetch",
                "knowledge_search",
                "advisor",
                "read",
                "ls",
                "grep",
                "glob",
                "bash_output",
                "present_artifacts",
                "memory_read",
                "memory_search",
            ],
            "memory_read/memory_search are read-only but deliberately not parallel-safe"
        );
    }

    #[test]
    fn conversation_tools_are_never_listed_via_builtin_exposure() {
        let native = crate::settings::ChatNativeToolsConfig {
            web_search: true,
            web_fetch: true,
            skill_runtime: true,
            read_file: true,
            write_file: true,
            edit_file: true,
            run_command: true,
            knowledge_search: true,
            working_directory: String::new(),
            workspace_roots: Vec::new(),
        };
        for entry in NATIVE_TOOLS {
            if matches!(
                entry.call,
                NativeToolCall::Conversation(_)
                    | NativeToolCall::HostMediated
                    | NativeToolCall::SubAgent(_)
            ) {
                assert!(
                    !(entry.enabled)(&native, true, true),
                    "{} must be appended via chat/commands.rs, not listed here",
                    entry.name
                );
            }
        }
    }

    /// Exposure-surface snapshot: for fixed settings combinations, the exact
    /// ordered tool-name list returned by `list_native_builtin_tool_defs`
    /// must stay frozen. This is the primary regression guard for the
    /// registry refactor.
    #[test]
    fn builtin_exposure_snapshot_per_settings_combination() {
        use crate::mcp::types::list_native_builtin_tool_defs;
        use crate::settings::ChatNativeToolsConfig;

        fn names(
            native: &ChatNativeToolsConfig,
            web_search_configured: bool,
            memory_enabled: bool,
        ) -> Vec<String> {
            list_native_builtin_tool_defs(native, web_search_configured, memory_enabled)
                .into_iter()
                .map(|tool| tool.name)
                .collect()
        }

        let off = ChatNativeToolsConfig {
            web_search: false,
            web_fetch: false,
            skill_runtime: false,
            read_file: false,
            write_file: false,
            edit_file: false,
            run_command: false,
            knowledge_search: false,
            working_directory: String::new(),
            workspace_roots: Vec::new(),
        };
        assert_eq!(names(&off, true, false), ["present_artifacts"]);

        // web_search requires both the toggle and a configured provider key.
        let mut search_only = off.clone();
        search_only.web_search = true;
        assert_eq!(names(&search_only, false, false), ["present_artifacts"]);
        assert_eq!(
            names(&search_only, true, false),
            ["web_search", "present_artifacts"]
        );

        // read_file gate exposes the whole read-side group, in order.
        let mut read_only = off.clone();
        read_only.read_file = true;
        assert_eq!(
            names(&read_only, false, false),
            ["read", "grep", "glob", "present_artifacts"]
        );

        // The write gate exposes the whole-file write tool only. File cards are
        // a path-driven workbench behavior, not a separate delivery tool.
        let mut write_only = off.clone();
        write_only.write_file = true;
        assert_eq!(
            names(&write_only, false, false),
            ["write", "present_artifacts"]
        );

        // memory gate is independent of native toggles.
        assert_eq!(
            names(&off, false, true),
            [
                "present_artifacts",
                "memory_read",
                "memory_modify",
                "memory_search",
            ]
        );

        // Everything on: full ordered surface.
        let all = ChatNativeToolsConfig {
            web_search: true,
            web_fetch: true,
            skill_runtime: true,
            read_file: true,
            write_file: true,
            edit_file: true,
            run_command: true,
            knowledge_search: true,
            working_directory: String::new(),
            workspace_roots: Vec::new(),
        };
        assert_eq!(
            names(&all, true, true),
            [
                "web_search",
                "web_fetch",
                "knowledge_search",
                "read",
                "grep",
                "glob",
                "write",
                "edit",
                "bash",
                "bash_output",
                "kill_background",
                "present_artifacts",
                "memory_read",
                "memory_modify",
                "memory_search",
            ]
        );
    }
    #[test]
    fn present_artifacts_returns_deduplicated_structured_ids() {
        let workspace = NativeToolWorkspace::standalone();
        let result = call_present_artifacts(
            &workspace,
            &serde_json::json!({
                "artifact_ids": ["art_a", "art_a", "art_b"],
                "caption": "Preview"
            }),
        )
        .expect("presentation result");
        assert_eq!(
            result.structured_content,
            Some(serde_json::json!({
                "type": "artifact_presentation",
                "artifactIds": ["art_a", "art_b"],
                "caption": "Preview"
            }))
        );
        assert!(result.artifacts.is_empty());
    }

    #[test]
    fn present_artifacts_keeps_ids_when_a_path_is_unreadable() {
        // 模型把生成图的 artifact 名当 path 传进来时，artifact_ids 仍须生效。
        let workspace = NativeToolWorkspace::standalone();
        let result = call_present_artifacts(
            &workspace,
            &serde_json::json!({
                "artifact_ids": ["art_a"],
                "paths": ["generated-image-1.png"]
            }),
        )
        .expect("presentation survives an unreadable path");
        assert!(result.artifacts.is_empty());
        assert_eq!(
            result.structured_content.as_ref().unwrap()["artifactIds"],
            serde_json::json!(["art_a"])
        );
        assert!(result.content.contains("generated-image-1.png"));

        // 回给模型的文本必须自洽：明说展示了 1 个，且声明本次展示未受影响。
        // 此前是无条件的 "Selected files will be displayed" + "Skipped ..."，两句矛盾，
        // 模型判断不出成没成而回空响应，整轮卡死（实测 out=4 tokens，稳定复现）。
        assert!(
            result.content.contains("Displayed 1 file"),
            "must state how many were shown, got: {}",
            result.content
        );
        assert!(
            result.content.contains("did not affect"),
            "must tell the model the presentation still succeeded, got: {}",
            result.content
        );
        assert!(
            !result.content.contains("will be displayed"),
            "must not promise a future display alongside a skip notice"
        );
        // 顺带引导模型别再拿生成文件当 path 传。
        assert!(result.content.contains("artifact_ids"));

        // 全部无效时仍报错。
        assert!(call_present_artifacts(
            &workspace,
            &serde_json::json!({ "paths": ["generated-image-1.png"] })
        )
        .is_err());
    }

    #[test]
    fn present_artifacts_reports_plural_and_stays_clean_without_skips() {
        let workspace = NativeToolWorkspace::standalone();
        let result = call_present_artifacts(
            &workspace,
            &serde_json::json!({ "artifact_ids": ["art_a", "art_b"] }),
        )
        .expect("presentation result");
        assert!(
            result.content.contains("Displayed 2 files"),
            "got: {}",
            result.content
        );
        // 没有跳过项时不该出现任何「忽略/未受影响」的噪音。
        assert!(!result.content.contains("Ignored"));
        assert!(!result.content.contains("did not affect"));
    }

    #[test]
    fn present_artifacts_loads_existing_local_file() {
        let path = std::env::temp_dir().join(format!(
            "kivio-present-artifact-{}.txt",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&path, b"hello").expect("write test file");
        let workspace = NativeToolWorkspace::standalone();
        let result = call_present_artifacts(
            &workspace,
            &serde_json::json!({ "paths": [path.to_string_lossy()] }),
        )
        .expect("presentation result");
        std::fs::remove_file(&path).ok();

        assert_eq!(result.artifacts.len(), 1);
        assert_eq!(
            result.artifacts[0].name,
            path.file_name().unwrap().to_string_lossy()
        );
        assert_eq!(result.artifacts[0].mime_type, "text/plain");
        assert!(result.artifacts[0].path.as_deref().is_some_and(
            |value| value.ends_with(path.file_name().unwrap().to_string_lossy().as_ref())
        ));
        assert_eq!(
            result.structured_content,
            Some(serde_json::json!({
                "type": "artifact_presentation",
                "artifactIds": []
            }))
        );
    }
}
