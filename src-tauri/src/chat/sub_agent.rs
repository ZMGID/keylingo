//! Multi-agent / sub-agent runtime (P3).
//!
//! A sub-agent is "a fresh isolated message history run through the same
//! `run_agent_loop`" — there is no second execution engine. `run_sub_agent`
//! builds an isolated `AgentRunConfig` (system + user only, a synthetic
//! `conversation_id` for generation/streaming isolation, but the PARENT
//! conversation as `tool_conversation_id` so the child's native file tools
//! resolve the parent's project workspace), wraps it in a `SubAgentHost`, and
//! reuses the existing loop. The `agent` native tool spawns one and reports
//! live nested progress onto the parent tool card. The spawn is BLOCKING +
//! single-result: it awaits `run_sub_agent` to completion and returns the full
//! result inline (the Claude Code Task model). Parallelism comes from the model
//! emitting MULTIPLE `agent` tool calls in a single message — `agent` is
//! `parallel_safe`, so a single round runs them concurrently via
//! `execute_parallel_chunk` (join_all, capped by `MAX_PARALLEL_TOOL_CALLS_PER_ROUND`
//! and the `SubAgentManager` semaphore). There is no `background`/`await`/`poll`
//! machinery: the wait stays in the runtime, never in the model token loop.
//!
//! Orchestrator-worker model: a sub-agent is a PURE WORKER. It receives one
//! self-contained prompt, runs in isolation, and returns a result. It is given
//! NO todo tools and NO todo prompt, so it cannot read or mutate any todo list.
//! Task delegation is top-down: the parent conversation owns its todos and uses
//! its own todo tools to set `owner` (= sub-agent name) and status itself.
//!
//! Safety rails (research doc 05 + architecture P3):
//! - the `agent` tool is stripped from every sub-agent's tool table
//!   (`filter::filter_tools_for_agent`), so in practice nesting is a single
//!   level: only the top-level chat (depth 0) can spawn.
//! - depth guard (`MAX_SUB_AGENT_DEPTH`): defense-in-depth backstop should the
//!   strip ever regress; with the strip in place depth ≥ 2 is unreachable.
//! - a `Semaphore` caps concurrent sub-agents (desktop API-quota sensitive);
//!   the cap defaults to `DEFAULT_SUB_AGENT_CONCURRENCY` and is user-overridable
//!   live via `settings.chat_tools.sub_agent_concurrency`.
//! - `SubAgentHost` auto-denies approval-gated (sensitive) tools at depth > 0
//!   and cascades parent cancellation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager};
use tokio::sync::Semaphore;

use crate::agents::AgentDefinition;
use crate::chat::agent::prepare::{available_builtin_tool_names, build_chat_system_prompt};
use crate::chat::agent::types::AgentRunResult;
use crate::chat::agent::{
    run_agent_loop, AgentHost, AgentHostFuture, AgentRunConfig, ToolExecutionContext, ToolExecutor,
    ToolExecutorFuture,
};
use crate::chat::ask_user::{AskUserPromptPayload, AskUserResponseResult};
use crate::chat::types::{ChatMessageSegment, ToolCallRecord, ToolCallStatus};
use crate::mcp::native_registry::NativeToolFuture;
use crate::mcp::types::McpToolCallResult;
use crate::mcp::ChatToolDefinition;
use crate::settings::{ModelProvider, Settings};
use crate::skills::SkillRegistry;
use crate::state::AppState;

/// An agent at depth ≥ this cannot spawn another sub-agent. Top-level chat is
/// depth 0; a spawned sub-agent runs at depth 1. NOTE: this is a
/// defense-in-depth backstop, not the real limiter — `filter_tools_for_agent`
/// always strips the `agent` tool from sub-agents, so actual nesting is one
/// level and depth ≥ 2 is unreachable unless that strip regresses.
pub const MAX_SUB_AGENT_DEPTH: u8 = 3;
/// Default concurrent sub-agent cap. User-overridable (live) via
/// `settings.chat_tools.sub_agent_concurrency`, clamped to
/// `[SUB_AGENT_CONCURRENCY_MIN, SUB_AGENT_CONCURRENCY_MAX]`.
pub const DEFAULT_SUB_AGENT_CONCURRENCY: usize = 12;
pub const SUB_AGENT_CONCURRENCY_MIN: usize = 1;
pub const SUB_AGENT_CONCURRENCY_MAX: usize = 64;
/// 保留的任务记录上限（运行中 + 已完成）。任务表通过 `get`/`list` 按需读取——
/// 包括完成很久之后——所以不能按短 TTL 删；改为封顶保留，超限时优先丢弃「最老的
/// 已完成」记录（绝不丢运行中的）。并发受 sub_agent_concurrency 限，故总有已完成
/// 记录可回收。把「每条记录持有子 agent 完整 result 文本」的进程级泄漏封到约
/// MAX_SUB_AGENT_TASKS × 数 KB。
const MAX_SUB_AGENT_TASKS: usize = 128;
/// Max attempts for a single sub-agent run. Reasoning models (e.g. DeepSeek-V4)
/// intermittently return an empty assistant message in the planning step, which
/// `run_agent_loop` surfaces as `Err`. Top-level chat recovers via user resend;
/// a sub-agent has no resend loop, so we retry the run once before giving up.
const SUB_AGENT_MAX_ATTEMPTS: usize = 2;
/// Substring identifying the "model returned an empty assistant message" error
/// produced by `run_agent_loop`. Single source of truth for both the retry
/// predicate (`should_retry_sub_agent`) and the user-facing error mapping.
const EMPTY_RESPONSE_MARKER: &str = "empty assistant response";
/// Outer per-tool-call timeout for the `agent` spawn tool. The sub-agent run
/// itself has no inner wall-clock cap (it finishes naturally or via cascade
/// cancel); the default generic tool timeout (120s) is far shorter and would
/// mis-kill a long-running sub-agent doing real multi-round work. This generous
/// backstop only guards against a wedged spawn never returning at all — normal
/// lifecycle is governed by completion + cascade cancel, not by this value.
pub const SUB_AGENT_TOOL_TIMEOUT_MS: u64 = 660_000;
const PROGRESS_EMIT_INTERVAL_MS: u128 = 350;
const RESULT_PREVIEW_MAX: usize = 4000;

pub const AGENT_TOOL_NAME: &str = "agent";

pub fn is_sub_agent_tool_name(name: &str) -> bool {
    name == AGENT_TOOL_NAME
}

/// Whether an agent at `depth` may spawn a sub-agent. The child runs at
/// `depth + 1`; an agent at depth ≥ `MAX_SUB_AGENT_DEPTH` is denied
/// (research doc 05 §1.3 / acceptance #2).
pub fn depth_allows_spawn(depth: u8) -> bool {
    depth < MAX_SUB_AGENT_DEPTH
}

/// Whether a failed sub-agent run should be retried. Retry ONLY the empty
/// assistant response case (reasoning models intermittently return an empty
/// planning message and a sub-agent has no user resend loop), with attempts
/// remaining and the parent generation still active.
///
/// Deliberately narrow: a retry re-runs the whole agent loop from scratch, so
/// any other error class would re-execute side effects already performed in the
/// failed attempt (e.g. a run that wrote files over several rounds and only then
/// hit a fatal provider error would write them twice). A cancellation (own stop
/// or parent cascade) is never retried.
fn should_retry_sub_agent(
    outcome: &Result<AgentRunResult, String>,
    attempt: usize,
    parent_active: bool,
) -> bool {
    matches!(outcome, Err(err) if err.contains(EMPTY_RESPONSE_MARKER))
        && attempt + 1 < SUB_AGENT_MAX_ATTEMPTS
        && parent_active
}

// ---------------------------------------------------------------------------
// Task registry (lives on AppState)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentTaskRecord {
    pub id: String,
    pub name: String,
    pub agent_type: String,
    pub status: SubAgentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub depth: u8,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    /// Provider token usage for this sub-agent's own run (input/output/total),
    /// surfaced on the parent tool card. None until the run completes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<SubAgentUsage>,
}

/// Compact token usage for a finished sub-agent run, derived from the run's
/// `AgentRunResult.usage` (the sub-agent's own provider usage, not overlapping
/// the parent conversation's). All fields optional: providers may omit any.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
}

impl SubAgentUsage {
    fn from_model_usage(usage: &crate::chat::model::ModelUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            total_tokens: usage.total_tokens,
        }
    }
}

/// Process-level sub-agent task table + concurrency gate. Held on `AppState`.
pub struct SubAgentManager {
    tasks: Mutex<HashMap<String, SubAgentTaskRecord>>,
    by_name: Mutex<HashMap<String, String>>,
    semaphore: Arc<Semaphore>,
    /// Current configured permit count; lets `set_concurrency` compute the delta
    /// to add/remove (tokio Semaphore exposes no total capacity).
    configured: AtomicUsize,
}

impl Default for SubAgentManager {
    fn default() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
            by_name: Mutex::new(HashMap::new()),
            semaphore: Arc::new(Semaphore::new(DEFAULT_SUB_AGENT_CONCURRENCY)),
            configured: AtomicUsize::new(DEFAULT_SUB_AGENT_CONCURRENCY),
        }
    }
}

impl SubAgentManager {
    fn lock_tasks(&self) -> std::sync::MutexGuard<'_, HashMap<String, SubAgentTaskRecord>> {
        self.tasks.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn register(&self, record: SubAgentTaskRecord) {
        {
            let mut by_name = self.by_name.lock().unwrap_or_else(|e| e.into_inner());
            by_name.insert(record.name.clone(), record.id.clone());
        }
        let evicted = {
            let mut tasks = self.lock_tasks();
            tasks.insert(record.id.clone(), record);
            Self::evict_completed_over_cap(&mut tasks)
        };
        // 清掉被驱逐 id 的 name→id 反查，但只删仍指向被驱逐 id 的项——同名任务可能
        // 已被更新的 id 覆盖，那条映射要留给新任务。
        if !evicted.is_empty() {
            let mut by_name = self.by_name.lock().unwrap_or_else(|e| e.into_inner());
            by_name.retain(|_, id| !evicted.contains(id));
        }
    }

    /// 超过 MAX_SUB_AGENT_TASKS 时，按完成时间从老到新丢弃「已完成」记录，绝不丢
    /// 运行中的。返回被驱逐的 id 供调用方清理 `by_name`。
    fn evict_completed_over_cap(tasks: &mut HashMap<String, SubAgentTaskRecord>) -> Vec<String> {
        let mut evicted = Vec::new();
        if tasks.len() <= MAX_SUB_AGENT_TASKS {
            return evicted;
        }
        let mut completed: Vec<(String, i64)> = tasks
            .values()
            .filter(|r| r.completed_at.is_some())
            .map(|r| (r.id.clone(), r.completed_at.unwrap_or(r.created_at)))
            .collect();
        completed.sort_by_key(|(_, ts)| *ts);
        for (id, _) in &completed {
            if tasks.len() <= MAX_SUB_AGENT_TASKS {
                break;
            }
            tasks.remove(id);
            evicted.push(id.clone());
        }
        evicted
    }

    pub fn set_status(&self, id: &str, status: SubAgentStatus) {
        if let Some(record) = self.lock_tasks().get_mut(id) {
            record.status = status;
        }
    }

    pub fn finish(
        &self,
        id: &str,
        status: SubAgentStatus,
        result: Option<String>,
        error: Option<String>,
        usage: Option<SubAgentUsage>,
    ) {
        if let Some(record) = self.lock_tasks().get_mut(id) {
            record.status = status;
            record.result = result;
            record.error = error;
            record.usage = usage;
            record.completed_at = Some(chrono::Local::now().timestamp());
        }
    }

    pub fn get(&self, id_or_name: &str) -> Option<SubAgentTaskRecord> {
        if let Some(record) = self.lock_tasks().get(id_or_name) {
            return Some(record.clone());
        }
        let resolved = self
            .by_name
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id_or_name)
            .cloned();
        resolved.and_then(|id| self.lock_tasks().get(&id).cloned())
    }

    pub fn list(&self) -> Vec<SubAgentTaskRecord> {
        let mut all: Vec<SubAgentTaskRecord> = self.lock_tasks().values().cloned().collect();
        all.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        all
    }

    fn semaphore(&self) -> Arc<Semaphore> {
        self.semaphore.clone()
    }

    /// Resize the concurrency gate live. Growing adds permits synchronously;
    /// shrinking spawns a task that acquires the surplus and forgets it (so it
    /// waits for in-flight runs to release). Deltas are always computed against
    /// `configured`, so a rapid grow/shrink pair still converges to the latest
    /// target.
    /// ponytail: shrink relies on a tokio runtime (always present in-app) and is
    /// only ever called serially from settings-save / startup — no concurrent
    /// callers to race the swap.
    pub fn set_concurrency(&self, target: usize) {
        let target = target.clamp(SUB_AGENT_CONCURRENCY_MIN, SUB_AGENT_CONCURRENCY_MAX);
        let prev = self.configured.swap(target, Ordering::SeqCst);
        if target > prev {
            self.semaphore.add_permits(target - prev);
        } else if target < prev {
            let sem = self.semaphore.clone();
            let remove = (prev - target) as u32;
            tauri::async_runtime::spawn(async move {
                if let Ok(permits) = sem.acquire_many(remove).await {
                    permits.forget();
                }
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Sub-agent host: forwards live progress, denies sensitive tools, cascades cancel
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ProgressState {
    text: String,
    last_emit: Option<Instant>,
    /// Per-tool-call tracking, ordered by first sight, keyed by `record.id`.
    /// Status is updated in place (Pending→Running→Success/Error) instead of
    /// appending an event line per transition, so the same call never produces
    /// multiple step rows.
    tools: Vec<ToolProgress>,
}

/// One tracked tool call inside a sub-agent run.
struct ToolProgress {
    id: String,
    name: String,
    status: ToolCallStatus,
}

impl ProgressState {
    /// Insert or update a tool call by `id`. Existing call ⇒ refresh its status;
    /// new call ⇒ append (preserving first-seen order).
    fn upsert_tool(&mut self, id: &str, name: &str, status: ToolCallStatus) {
        if let Some(existing) = self.tools.iter_mut().find(|t| t.id == id) {
            existing.status = status;
        } else {
            self.tools.push(ToolProgress {
                id: id.to_string(),
                name: name.to_string(),
                status,
            });
        }
    }

    /// Aggregate tracked tool calls into a compact per-tool-name summary, one
    /// line per distinct tool name with status counts, e.g.
    /// `web_search · 6 done · 2 running`. Zero-count states are omitted.
    fn aggregate_steps(&self) -> Vec<String> {
        // Preserve first-seen order of tool names.
        let mut order: Vec<&str> = Vec::new();
        let mut counts: HashMap<&str, [usize; 3]> = HashMap::new(); // [done, running, failed]
        for tool in &self.tools {
            let entry = counts.entry(tool.name.as_str()).or_insert_with(|| {
                order.push(tool.name.as_str());
                [0, 0, 0]
            });
            match tool.status {
                ToolCallStatus::Success | ToolCallStatus::Skipped => entry[0] += 1,
                ToolCallStatus::Pending | ToolCallStatus::Running => entry[1] += 1,
                ToolCallStatus::Error | ToolCallStatus::Cancelled => entry[2] += 1,
            }
        }
        order
            .into_iter()
            .map(|name| {
                let [done, running, failed] = counts[name];
                let mut line = name.to_string();
                if done > 0 {
                    line.push_str(&format!(" · {done} done"));
                }
                if running > 0 {
                    line.push_str(&format!(" · {running} running"));
                }
                if failed > 0 {
                    line.push_str(&format!(" · {failed} failed"));
                }
                line
            })
            .collect()
    }
}

struct SubAgentHost {
    app: AppHandle,
    parent_conversation_id: String,
    parent_run_id: String,
    parent_tool_call_id: String,
    parent_generation: u64,
    task_id: String,
    name: String,
    model: String,
    depth: u8,
    progress: Mutex<ProgressState>,
}

/// Whether a sub-agent run is still active: BOTH its own generation and the
/// parent generation must be live. Parent cancel ⇒ cascade (acceptance #3).
fn generation_cascade_active(
    state: &AppState,
    conversation_id: &str,
    generation: u64,
    parent_conversation_id: &str,
    parent_generation: u64,
) -> bool {
    state.is_chat_generation_active(conversation_id, generation)
        && state.is_chat_generation_active(parent_conversation_id, parent_generation)
}

impl SubAgentHost {
    fn emit_progress(&self, status: &str, force: bool) {
        let (text, steps) = {
            let mut guard = self.progress.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            if !force {
                if let Some(last) = guard.last_emit {
                    if now.duration_since(last).as_millis() < PROGRESS_EMIT_INTERVAL_MS {
                        return;
                    }
                }
            }
            guard.last_emit = Some(now);
            (clip(&guard.text, 1200), guard.aggregate_steps())
        };
        crate::chat::protocol::emit_run_event(
            &self.app,
            &self.parent_run_id,
            crate::chat::protocol::ChatRunEvent::SubagentUpdated {
                parent_tool_call_id: self.parent_tool_call_id.clone(),
                task_id: self.task_id.clone(),
                name: self.name.clone(),
                model: Some(self.model.clone()),
                depth: self.depth,
                status: status.to_string(),
                preview: Some(text),
                steps,
            },
        );
    }
}

impl AgentHost for SubAgentHost {
    fn emit_stream_delta(
        &self,
        _conversation_id: &str,
        _run_id: &str,
        _message_id: &str,
        delta: &str,
        _reasoning_delta: Option<&str>,
        _segment: Option<&ChatMessageSegment>,
    ) {
        if !delta.is_empty() {
            let mut guard = self.progress.lock().unwrap_or_else(|e| e.into_inner());
            guard.text.push_str(delta);
        }
        self.emit_progress("running", false);
    }

    fn emit_tool_record(
        &self,
        _conversation_id: &str,
        _run_id: &str,
        _message_id: &str,
        record: &ToolCallRecord,
    ) {
        // Surface which tools the sub-agent is using as compact nested step
        // lines. Track each call by id and update its status in place, so a
        // single call (Pending→Running→Success) never spams multiple rows;
        // `emit_progress` aggregates them into per-tool-name count lines.
        {
            let mut guard = self.progress.lock().unwrap_or_else(|e| e.into_inner());
            guard.upsert_tool(&record.id, &record.name, record.status.clone());
        }
        self.emit_progress("running", true);
    }

    fn request_tool_approval<'a>(
        &'a self,
        _ctx: &'a ToolExecutionContext<'a>,
        _record: &'a ToolCallRecord,
    ) -> AgentHostFuture<'a, bool> {
        // depth > 0: a sub-agent can never escalate to the user for approval,
        // so any approval-gated (sensitive) tool is auto-denied. Read-only /
        // bypass-approval tools never reach this method.
        Box::pin(async move { false })
    }

    fn request_session_consent<'a>(
        &'a self,
        _ctx: &'a ToolExecutionContext<'a>,
    ) -> AgentHostFuture<'a, bool> {
        // A sub-agent cannot prompt the user, but it inherits the parent
        // conversation's session consent: if the user already authorized
        // file/shell tools for this conversation, the sub-agent reuses that
        // grant. Otherwise it denies (the parent must consent first).
        Box::pin(async move {
            self.app
                .state::<AppState>()
                .has_chat_consent(&self.parent_conversation_id)
        })
    }

    fn request_user_response<'a>(
        &'a self,
        _ctx: &'a ToolExecutionContext<'a>,
        _record: &'a ToolCallRecord,
        _prompt: AskUserPromptPayload,
    ) -> AgentHostFuture<'a, AskUserResponseResult> {
        // Sub-agents run autonomously; ask_user is filtered out of their tool
        // table, but if reached, resolve as a cancelled prompt.
        Box::pin(async move {
            AskUserResponseResult {
                phase: "cancelled".to_string(),
                answers: HashMap::new(),
            }
        })
    }

    fn is_generation_active(&self, conversation_id: &str, generation: u64) -> bool {
        // Cascade: the sub-agent run is active only while BOTH its own
        // generation and the parent generation are live. Parent cancel ⇒
        // sub-agent stops on its next loop check.
        generation_cascade_active(
            &self.app.state::<AppState>(),
            conversation_id,
            generation,
            &self.parent_conversation_id,
            self.parent_generation,
        )
    }

    fn wait_for_generation_inactive<'a>(
        &'a self,
        conversation_id: &'a str,
        generation: u64,
    ) -> AgentHostFuture<'a, ()> {
        Box::pin(async move {
            loop {
                if !self.is_generation_active(conversation_id, generation) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Tool executor for sub-agents (delegates to the MCP/native registry)
// ---------------------------------------------------------------------------

struct SubAgentToolExecutor {
    app: AppHandle,
}

impl ToolExecutor for SubAgentToolExecutor {
    fn call<'a>(
        &'a self,
        ctx: &'a ToolExecutionContext<'a>,
        tool: &'a ChatToolDefinition,
        arguments: Value,
        skill_cache: Option<&'a mut crate::skills::SkillRunCache>,
    ) -> ToolExecutorFuture<'a> {
        Box::pin(async move {
            let native_ctx = crate::mcp::registry::NativeToolContext {
                conversation_id: ctx.tool_conversation_id.to_string(),
                message_id: ctx.message_id.to_string(),
                tool_call_id: Some(ctx.tool_call_id.to_string()),
                run_id: ctx.run_id.to_string(),
                generation: ctx.generation,
                depth: ctx.depth,
            };
            crate::mcp::registry::call_tool(
                &self.app,
                &self.app.state::<AppState>(),
                tool,
                arguments,
                skill_cache,
                Some(native_ctx),
            )
            .await
        })
    }
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

/// Resolve the provider+model a sub-agent should run on. Three tiers:
/// agent-definition `model` (on the PARENT provider) → global override
/// (`chat_tools.sub_agent_provider_id`/`sub_agent_model`, may switch provider) →
/// parent conversation provider+model. An override provider that no longer
/// resolves falls back to the parent instead of failing the spawn.
fn resolve_sub_agent_provider_model(
    settings: &Settings,
    parent_provider_id: &str,
    parent_model: &str,
    def_model: Option<&str>,
) -> Result<(ModelProvider, String), String> {
    let parent_provider = || {
        settings
            .get_provider(parent_provider_id)
            .cloned()
            .ok_or_else(|| "Parent chat provider not found".to_string())
    };

    // Tier 1: definition override — parent provider, definition's model.
    if let Some(m) = def_model.map(str::trim).filter(|m| !m.is_empty()) {
        return Ok((parent_provider()?, m.to_string()));
    }

    // Tier 2: global sub-agent override (both fields must be usable).
    let ov_provider_id = settings.chat_tools.sub_agent_provider_id.trim();
    let ov_model = settings.chat_tools.sub_agent_model.trim();
    if !ov_provider_id.is_empty() && !ov_model.is_empty() {
        if let Some(p) = settings
            .get_provider(ov_provider_id)
            .filter(|p| p.enabled && !p.api_keys.is_empty())
        {
            return Ok((p.clone(), ov_model.to_string()));
        }
        // Unusable override → fall through to the parent (tier 3).
    }

    // Tier 3: follow the parent conversation.
    Ok((parent_provider()?, parent_model.to_string()))
}

pub struct SubAgentRequest {
    pub task_id: String,
    pub name: String,
    pub agent_type: String,
    pub prompt: String,
    pub system_prompt: String,
    pub provider: ModelProvider,
    pub model: String,
    pub tools: Vec<ChatToolDefinition>,
    pub settings: Settings,
    pub max_output_tokens: u32,
    pub language: String,
    pub depth: u8,
    pub parent_conversation_id: String,
    pub parent_run_id: String,
    pub parent_tool_call_id: String,
    pub parent_generation: u64,
}

/// Run a sub-agent to completion. Builds an isolated config and reuses
/// `run_agent_loop`. Takes an owned `AppHandle` and re-resolves `AppState` from
/// it (`app.state::<AppState>()`). Awaited inline by the synchronous `agent`
/// spawn handler. Cancellation cascades from the parent via `SubAgentHost`. Up
/// to `SUB_AGENT_MAX_ATTEMPTS` tries: reasoning models occasionally return an
/// empty planning response (surfaced as `Err`); since a sub-agent has no user
/// resend loop, we retry once on non-cancel errors.
async fn run_sub_agent(app: AppHandle, req: SubAgentRequest) -> Result<AgentRunResult, String> {
    let state = app.state::<AppState>();
    let state: &AppState = &state;
    let sub_conversation_id = format!("subagent-{}", req.task_id);

    let mut last_outcome: Result<AgentRunResult, String> = Err("Sub-agent did not run".to_string());

    for attempt in 0..SUB_AGENT_MAX_ATTEMPTS {
        // A cascade cancel between attempts must short-circuit: never retry once
        // the parent generation is gone.
        if attempt > 0
            && !state.is_chat_generation_active(&req.parent_conversation_id, req.parent_generation)
        {
            return Err("cancelled".to_string());
        }

        // Fresh generation + runtime per attempt. The config is moved into
        // `run_agent_loop`, so host/executor/config are rebuilt each iteration.
        let sub_generation = state.next_chat_generation(&sub_conversation_id);
        let sub_run_id = format!("subrun-{}", req.task_id);
        let sub_message_id = format!("submsg-{}", req.task_id);

        let runtime_messages = vec![
            serde_json::json!({ "role": "system", "content": req.system_prompt }),
            serde_json::json!({ "role": "user", "content": req.prompt }),
        ];

        let host = SubAgentHost {
            app: app.clone(),
            parent_conversation_id: req.parent_conversation_id.clone(),
            parent_run_id: req.parent_run_id.clone(),
            parent_tool_call_id: req.parent_tool_call_id.clone(),
            parent_generation: req.parent_generation,
            task_id: req.task_id.clone(),
            name: req.name.clone(),
            model: req.model.clone(),
            depth: req.depth,
            progress: Mutex::new(ProgressState::default()),
        };
        let executor = SubAgentToolExecutor { app: app.clone() };

        let thinking_enabled = req.settings.chat.thinking_enabled;
        let stream_enabled = req.settings.chat.stream_enabled;
        let max_output_tokens = req.max_output_tokens;
        let retry_attempts = if req.settings.retry_enabled {
            req.settings.retry_attempts as usize
        } else {
            1
        };
        let effective_chat_tools = req.settings.chat_tools.clone();

        let config = AgentRunConfig {
            state,
            conversation_id: sub_conversation_id.clone(),
            tool_conversation_id: req.parent_conversation_id.clone(),
            depth: req.depth,
            run_id: sub_run_id,
            message_id: sub_message_id,
            generation: sub_generation,
            provider: req.provider.clone(),
            model: req.model.clone(),
            runtime_messages,
            tools: req.tools.clone(),
            blocked_tool_calls: Vec::new(),
            settings: req.settings.clone(),
            effective_chat_tools,
            language: req.language.clone(),
            thinking_enabled,
            thinking_level: None,
            // 子代理不做联网搜索（父代理的搜索结果已在上下文里）。
            web_search_mode: crate::chat::types::WebSearchMode::Off,
            stream_enabled,
            max_output_tokens,
            retry_attempts,
            assistant_snapshot: None,
            provider_tools_fallback_system_prompt: req.system_prompt.clone(),
            initial_anchor_total_tokens: None,
            initial_anchor_trailing_estimate: 0,
        };

        // No wall-clock cap: a sub-agent now runs to natural completion or until
        // cancelled via generation cascade (parent stop ⇒ host.is_generation_active
        // flips false ⇒ the loop ends gracefully). A 300s hard timeout was removed
        // because real multi-round file work legitimately exceeds it.
        let outcome = run_agent_loop(config, &host, &executor).await;
        // Retire this attempt's generation on every exit path (success or failure).
        // Otherwise the synthetic generation reads "active" forever and entries
        // accumulate in chat_active_generations.
        state.cancel_chat_generation(&sub_conversation_id);

        // Success or cancellation → return immediately.
        if matches!(outcome, Ok(_)) || matches!(&outcome, Err(err) if err == "cancelled") {
            return outcome;
        }

        let parent_active =
            state.is_chat_generation_active(&req.parent_conversation_id, req.parent_generation);
        if !should_retry_sub_agent(&outcome, attempt, parent_active) {
            return outcome;
        }
        last_outcome = outcome;
    }

    last_outcome
}

// ---------------------------------------------------------------------------
// Native tool: agent
// ---------------------------------------------------------------------------

/// Context handed to sub-agent management tool handlers, dispatched before
/// workspace resolution (these tools manage agents, not files).
pub struct SubAgentCallCtx<'a> {
    pub app: &'a AppHandle,
    pub state: &'a AppState,
    pub native_ctx: &'a crate::mcp::registry::NativeToolContext,
    pub arguments: &'a Value,
}

/// Compose the `subagent_type` parameter description from the ACTUALLY loaded
/// definitions (built-in + user + project), so the model discovers user-authored
/// roles without being told they exist. Deliberately no JSON-Schema `enum`: a
/// role file created mid-run would be rejected provider-side, and inline ad-hoc
/// roles omit the field entirely — a soft failure listing the available roles is
/// more useful.
fn subagent_type_description(defs: &[AgentDefinition]) -> String {
    let available = defs
        .iter()
        .map(|d| {
            let description = d.description.trim();
            if description.is_empty() {
                d.name.clone()
            } else {
                format!("{} — {}", d.name, description)
            }
        })
        .collect::<Vec<_>>()
        .join("; ");
    let mut out = String::from("Named agent role.");
    if !available.is_empty() {
        out.push_str(" Available: ");
        out.push_str(&available);
        out.push('.');
    }
    out.push_str(
        " Omit to use general-purpose, or omit it and pass system_prompt/tools to define an ad-hoc role inline.",
    );
    out
}

pub fn agent_tool(defs: &[AgentDefinition]) -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__agent".to_string(),
        name: AGENT_TOOL_NAME.to_string(),
        description: "Spawn a sub-agent to handle a focused sub-task and return its result. The sub-agent runs with its own fresh context and a restricted toolset, and this call BLOCKS until it finishes, returning the full result inline. Use for delegating self-contained research/implementation/review work. To run sub-agents in PARALLEL, emit MULTIPLE agent tool calls in a SINGLE message — they execute concurrently and each returns its own result. Provide a complete, self-contained prompt — the sub-agent cannot see this conversation.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Complete, self-contained task for the sub-agent (it has no access to this conversation)."
                },
                "subagent_type": {
                    "type": "string",
                    "description": subagent_type_description(defs)
                },
                "name": {
                    "type": "string",
                    "maxLength": 80,
                    "description": "Optional short label for this sub-agent run."
                },
                "system_prompt": {
                    "type": "string",
                    "description": "Ad-hoc persona for this run. Replaces the named role's prompt."
                },
                "tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Ad-hoc tool allow-list, replacing the named role's. Supports `mcp__<server>__*`, `mcp__*`, `*`. Empty means inherit every tool."
                },
                "disallowed_tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Ad-hoc tool deny-list, replacing the named role's. Applied BEFORE `tools`."
                }
            },
            "required": ["prompt"],
            "additionalProperties": false
        }),
        sensitive: false,
        annotations: Some(serde_json::json!({
            "readOnlyHint": false,
            "destructiveHint": false,
            "openWorldHint": false
        })),
        output_schema: None,
    }
}

pub fn tool_definitions(defs: &[AgentDefinition]) -> Vec<ChatToolDefinition> {
    vec![agent_tool(defs)]
}

/// Append sub-agent management tools (model-facing), skipping the `agent`
/// spawn tool when `allow_spawn` is false (i.e. inside a sub-agent — second
/// guard against recursion alongside the depth check).
pub fn append_tool_definitions(
    tools: &mut Vec<ChatToolDefinition>,
    allow_spawn: bool,
    defs: &[AgentDefinition],
) {
    for tool in tool_definitions(defs) {
        if tool.name == AGENT_TOOL_NAME && !allow_spawn {
            continue;
        }
        if !tools
            .iter()
            .any(|existing| existing.openai_tool_name() == tool.openai_tool_name())
        {
            tools.push(tool);
        }
    }
}

/// Apply the `agent` call's inline role fields onto `def`. Each field is a WHOLE
/// REPLACEMENT, not a merge: merging could not express "researcher, but without
/// web_fetch", while replacement expresses any combination under one rule.
/// Empty strings / empty arrays count as "not provided".
fn apply_inline_overrides(def: &mut AgentDefinition, arguments: &Value) {
    if let Some(prompt) = arguments
        .get("system_prompt")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        def.system_prompt = prompt.to_string();
    }
    if let Some(tools) = inline_string_list(arguments, "tools") {
        def.tools = tools;
    }
    if let Some(denied) = inline_string_list(arguments, "disallowed_tools") {
        def.disallowed_tools = denied;
    }
}

/// Read an inline `Vec<String>` argument, returning `None` when absent or empty
/// (so an empty array never turns into a narrowing that blocks everything).
fn inline_string_list(arguments: &Value, key: &str) -> Option<Vec<String>> {
    let items: Vec<String> = arguments
        .get(key)?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    (!items.is_empty()).then_some(items)
}

/// Message for a spawn refused because its narrowing left no tools. The two
/// causes need different wording: a denylist that emptied the pool is NOT a
/// spelling problem, and reporting correctly-spelled allow entries as
/// "unresolved" sends the model off fixing names that were never wrong.
///
/// `deny_emptied_pool` must be computed on the pre-narrowing catalog, because
/// `unresolved_allow_entries` skips denied tools and therefore reports every
/// allow entry as unresolved under a deny-all — exactly the misleading case.
fn zero_tool_refusal(
    def: &AgentDefinition,
    unresolved: &[String],
    deny_emptied_pool: bool,
) -> String {
    let prefix = format!("Sub-agent '{}' would launch with zero tools", def.name);
    if deny_emptied_pool {
        return format!(
            "{prefix}: disallowed_tools ({}) removed every tool. Narrow the deny-list, or use `tools` to allow-list what the sub-agent needs.",
            def.disallowed_tools.join(", ")
        );
    }
    if !unresolved.is_empty() {
        return format!(
            "{prefix}. These entries matched no available tool: {}.",
            unresolved.join(", ")
        );
    }
    format!("{prefix}. Check its tools / disallowedTools configuration.")
}

fn err_result(message: impl Into<String>) -> McpToolCallResult {
    McpToolCallResult {
        content: message.into(),
        is_error: true,
        raw: Value::Null,
        artifacts: Vec::new(),
        structured_content: None,
        follow_up_user_messages: Vec::new(),
    }
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max).collect();
    format!("{truncated}…")
}

/// Spawn handler (the `agent` tool). Async (Box::pin). Drives `run_sub_agent`
/// to completion and returns the result inline (BLOCKING, single-result — the
/// Claude Code Task model). Parallelism comes from the model emitting multiple
/// `agent` tool calls in one round; the loop runs them concurrently because
/// `agent` is `parallel_safe`. The wait stays in the runtime, never in the
/// model token loop.
pub fn handle_agent_spawn<'a>(
    ctx: SubAgentCallCtx<'a>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<McpToolCallResult, String>> + Send + 'a>,
> {
    Box::pin(async move {
        // Depth guard (research doc 05 §1.3 / acceptance #2): an agent at depth
        // >= MAX cannot spawn. Soft failure (Ok with is_error) so the parent
        // loop continues.
        if !depth_allows_spawn(ctx.native_ctx.depth) {
            return Ok(err_result(format!(
                "Cannot spawn a sub-agent: max nesting depth {MAX_SUB_AGENT_DEPTH} reached."
            )));
        }

        let prompt = ctx
            .arguments
            .get("prompt")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "agent requires a non-empty prompt".to_string())?
            .to_string();
        let agent_type = ctx
            .arguments
            .get("subagent_type")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("general-purpose")
            .to_string();

        let parent_conversation_id = ctx.native_ctx.conversation_id.clone();
        let parent_conversation =
            crate::chat::storage::load_conversation(ctx.app, &parent_conversation_id)?;
        let settings = ctx.state.settings_read().clone();
        let language = crate::settings::resolve_chat_language(&settings);

        // Resolve agent definition (built-in + user + project layers).
        let project_root =
            crate::chat::storage::resolve_conversation_project(ctx.app, &parent_conversation)
                .ok()
                .flatten()
                .and_then(|p| p.root_path)
                .map(std::path::PathBuf::from);
        let defs = crate::agents::load_agent_definitions(ctx.app, project_root.as_deref());
        let Some(def) = crate::agents::find_definition(&defs, &agent_type) else {
            return Ok(err_result(format!(
                "Unknown sub-agent type '{agent_type}'. Available: {}",
                defs.iter()
                    .map(|d| d.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        };
        // The named role is the BASE; inline `system_prompt`/`tools`/
        // `disallowed_tools` replace their counterparts for this run only (R2).
        let mut def = def.clone();
        apply_inline_overrides(&mut def, ctx.arguments);

        // Provider/model resolution, three tiers:
        //   1. agent definition `model` field — highest, resolved against the
        //      PARENT provider (pre-existing semantics, ignores the global override)
        //   2. global sub-agent override (settings.chat_tools.sub_agent_*) —
        //      switches provider AND model, allowing a cheap cross-provider model
        //   3. parent conversation provider+model — the default (follow)
        // Runtime defense: an unusable override provider falls back to the parent
        // rather than failing the spawn (sanitize_settings normally prevents this).
        let (provider, model) = resolve_sub_agent_provider_model(
            &settings,
            &parent_conversation.provider_id,
            &parent_conversation.model,
            def.model.as_deref(),
        )?;
        if provider.api_keys.is_empty() {
            return Ok(err_result(
                "Sub-agent provider has no API key configured.".to_string(),
            ));
        }
        if model.trim().is_empty() {
            return Ok(err_result(
                "No model available for the sub-agent.".to_string(),
            ));
        }

        // Build the sub-agent's toolset: full enabled set, narrowed by the
        // agent definition, with the `agent` tool ALWAYS stripped (acceptance
        // #4). A sub-agent is a pure worker (orchestrator-worker model): it gets
        // NO todo tools, so it can never read or mutate any todo list. Task
        // delegation is top-down — the parent orchestrator owns the todos and
        // marks them itself (owner = sub-agent name) before/after the spawn.
        let mut tools = crate::mcp::registry::list_enabled_tool_catalog(ctx.app, ctx.state)
            .await
            .tools;
        // Computed on the UNFILTERED catalog (before narrowing) so the refusal
        // below can name the entries that matched nothing.
        let unresolved = crate::chat::agent::filter::unresolved_allow_entries(&tools, &def);
        // Whether the DENYLIST alone leaves nothing behind, measured before the
        // allow-list narrows anything. Drives the refusal wording below.
        let deny_emptied_pool = !def.disallowed_tools.is_empty()
            && !tools.is_empty()
            && tools.iter().all(|tool| {
                def.disallowed_tools
                    .iter()
                    .any(|entry| crate::chat::agent::filter::entry_matches(tool, entry))
                    || is_sub_agent_tool_name(&tool.name)
            });
        crate::chat::agent::filter::filter_tools_for_agent(&mut tools, &def);
        // Spec: a narrowing that resolves to the empty set must REFUSE to launch
        // rather than silently start a zero-tool worker. Skill tools are kept
        // unconditionally by the filter, so they don't count as resolved.
        // Both lists are checked: a deny-all (`disallowed_tools: ["*"]`) empties
        // the pool even when `tools` is empty, which the allow-only guard missed.
        let narrowing_requested = !def.tools.is_empty() || !def.disallowed_tools.is_empty();
        if narrowing_requested
            && tools.iter().all(|tool| {
                tool.source == "skill"
                    || crate::chat::agent::prepare::is_native_skill_tool_name(&tool.name)
            })
        {
            return Ok(err_result(zero_tool_refusal(
                &def,
                &unresolved,
                deny_emptied_pool,
            )));
        }
        let available_builtin_tools = available_builtin_tool_names(&tools);

        // Compose the sub-agent system prompt: persona prefix + base chat
        // system prompt. No todo context is injected — the worker is not aware
        // of and cannot touch the parent's todo list.
        let himalaya_binary = crate::connectors::himalaya::resolve_himalaya_binary_when_active(
            &settings.email_accounts,
        )
        .map(|path| path.display().to_string());
        let email_accounts_prompt = crate::settings::email_accounts_system_prompt(
            &settings.email_accounts,
            himalaya_binary.as_deref(),
        );
        // Real skill registry (was `SkillRegistry::default()` — an empty catalog,
        // so the skill tools the filter deliberately keeps had nothing to find).
        // Always the FULL registry: `def.skills` is a preload list, not a
        // visibility narrowing, so the sub-agent can still activate other skills.
        let skill_registry =
            crate::skills::build_registry(ctx.app, &settings.chat_tools.skill_scan_paths)
                .unwrap_or_default();
        let persona =
            compose_persona_with_preloaded_skills(&def.system_prompt, &skill_registry, &def.skills);
        let system_prompt = build_chat_system_prompt(
            &language,
            false,
            settings.chat.thinking_enabled,
            &skill_registry,
            &settings.chat_tools,
            true,
            &available_builtin_tools,
            None,
            None,
            None,
            None,
            &persona,
            None,
            None,
            None,
            None,
            None,
            // Sub-agent tools use the parent conversation's same default workbench.
            crate::chat::storage::resolve_conversation_working_directory(
                ctx.app,
                &parent_conversation,
                &settings.chat_tools.native_tools.working_directory,
            )
            .ok()
            .map(|path| path.display().to_string())
            .as_deref(),
            (!settings.obsidian_vault_path.trim().is_empty())
                .then_some(settings.obsidian_vault_path.as_str()),
            &settings.email_accounts,
            email_accounts_prompt.as_deref(),
        );

        let task_id = format!("agent-{}", uuid::Uuid::new_v4().simple());
        let name = ctx
            .arguments
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| def.name.clone());

        let manager = &ctx.state.sub_agents;
        manager.register(SubAgentTaskRecord {
            id: task_id.clone(),
            name: name.clone(),
            agent_type: def.name.clone(),
            // Pending until a concurrency permit is acquired; flipped to Running
            // once the run actually starts (below). The window is tiny.
            status: SubAgentStatus::Pending,
            result: None,
            error: None,
            depth: ctx.native_ctx.depth + 1,
            created_at: chrono::Local::now().timestamp(),
            completed_at: None,
            usage: None,
        });

        // Arm the drop backstop as soon as the record exists: a drop while
        // queued on the semaphore below would otherwise leave it stuck in
        // Pending (uncollectable — eviction only drops completed records).
        let mut drop_guard = SpawnDropGuard {
            app: ctx.app.clone(),
            task_id: task_id.clone(),
            armed: true,
        };

        // Concurrency gate: an OWNED permit, held for the lifetime of the run.
        // acquire_owned only errors if the semaphore is closed (never — it lives
        // on AppState). Held across the await below so concurrent fan-out is
        // capped by the SubAgentManager semaphore.
        let _permit = manager.semaphore().acquire_owned().await.ok();
        manager.set_status(&task_id, SubAgentStatus::Running);

        // Model-aware output cap: prefer the model library / provider override
        // (matching top-level chat); the raw setting is only a fallback.
        let max_output_tokens = crate::chat::model_metadata::chat_max_output_tokens_for_model(
            Some(&provider),
            &model,
            settings.chat.max_output_tokens,
        );

        let parent_run_id = ctx.native_ctx.run_id.clone();
        let parent_tool_call_id = ctx.native_ctx.tool_call_id.clone().unwrap_or_default();
        let request = SubAgentRequest {
            task_id: task_id.clone(),
            name: name.clone(),
            agent_type: def.name.clone(),
            prompt,
            system_prompt,
            provider,
            model,
            tools,
            settings,
            max_output_tokens,
            language,
            depth: ctx.native_ctx.depth + 1,
            parent_conversation_id: parent_conversation_id.clone(),
            parent_run_id: parent_run_id.clone(),
            parent_tool_call_id: parent_tool_call_id.clone(),
            parent_generation: ctx.native_ctx.generation,
        };

        // Owned context the finalizer needs.
        let finalize_ctx = FinalizeCtx {
            app: ctx.app.clone(),
            task_id: task_id.clone(),
            name: name.clone(),
            agent_type: def.name.clone(),
            model: request.model.clone(),
        };

        // Blocking + single-result: await completion and return the result
        // inline. The permit is held for the duration of this await. The drop
        // guard covers the paths where this future is dropped instead of
        // resuming (user stop / outer timeout) — see finalize_dropped_spawn.
        let app = ctx.app.clone();
        let outcome = run_sub_agent(app, request).await;
        drop_guard.armed = false;
        Ok(finalize_sub_agent_outcome(finalize_ctx, outcome))
    })
}

/// Owned context the outcome finalizer needs.
struct FinalizeCtx {
    app: AppHandle,
    task_id: String,
    name: String,
    agent_type: String,
    model: String,
}

/// Backstop cleanup when the `agent` spawn future is dropped mid-run instead of
/// returning: the tool scheduler's `tokio::select!` drops it when the user stops
/// the parent generation (that branch always wins the race against the loop's
/// own cascade-cancel checkpoint) or when the outer 660s backstop timeout
/// fires. A dropped future never reaches `run_sub_agent`'s post-await cleanup
/// nor `finalize_sub_agent_outcome`, which would leak (a) a task record stuck
/// in `Running` forever — uncollectable, since eviction only drops completed
/// records — and (b) the synthetic sub-conversation's active generation.
fn finalize_dropped_spawn(state: &AppState, task_id: &str) {
    state.sub_agents.finish(
        task_id,
        SubAgentStatus::Cancelled,
        None,
        Some("cancelled".to_string()),
        None,
    );
    state.cancel_chat_generation(&format!("subagent-{task_id}"));
}

/// RAII wrapper arming [`finalize_dropped_spawn`]. Disarmed on the normal
/// return path right after `run_sub_agent` resolves (no await between the two,
/// so the window is drop-free).
struct SpawnDropGuard {
    app: AppHandle,
    task_id: String,
    armed: bool,
}

impl Drop for SpawnDropGuard {
    fn drop(&mut self) {
        if self.armed {
            finalize_dropped_spawn(&self.app.state::<AppState>(), &self.task_id);
        }
    }
}

/// Turn a sub-agent run outcome into the task record update + tool result.
/// Marks the record finished and returns the `McpToolCallResult` (returned
/// inline to the parent loop). The synchronous path emits NO terminal
/// Protocol subagent update: the full result (status + content + usage) propagates
/// inline via the tool update flow, and the card keeps the last running progress
/// event's accumulated steps/preview — a terminal event (whose payload omits
/// steps/preview) would overwrite `subagentProgress` with empty arrays and wipe
/// that step history. Resolves `AppState`/manager from the owned `AppHandle`.
fn finalize_sub_agent_outcome(
    ctx: FinalizeCtx,
    outcome: Result<AgentRunResult, String>,
) -> McpToolCallResult {
    let state = ctx.app.state::<AppState>();
    let manager = &state.sub_agents;
    compute_sub_agent_finalization(
        manager,
        &SubAgentFinalizeParams {
            task_id: &ctx.task_id,
            name: &ctx.name,
            agent_type: &ctx.agent_type,
            model: &ctx.model,
        },
        outcome,
    )
}

/// Borrowed inputs for [`compute_sub_agent_finalization`]. Mirrors the
/// `AppHandle`-bound fields of [`FinalizeCtx`] but free of any Tauri runtime
/// dependency so the finalization logic can be unit-tested directly.
#[derive(Clone, Copy)]
struct SubAgentFinalizeParams<'a> {
    task_id: &'a str,
    name: &'a str,
    agent_type: &'a str,
    model: &'a str,
}

/// Pure finalization: update the manager record for the outcome and build the
/// `McpToolCallResult` returned inline to the parent loop. Free of `AppHandle`
/// so it is directly testable.
fn compute_sub_agent_finalization(
    manager: &SubAgentManager,
    params: &SubAgentFinalizeParams<'_>,
    outcome: Result<AgentRunResult, String>,
) -> McpToolCallResult {
    let SubAgentFinalizeParams {
        task_id,
        name,
        agent_type,
        model,
    } = *params;

    match outcome {
        // A cancelled run (own stop or parent cascade) now returns
        // Ok(cancelled_result) from every loop phase, not just an
        // Err("cancelled") from the planning/stream path. Detect it via
        // `stream_outcome` so a cancelled sub-agent is reported as
        // `cancelled` to the parent — never as a `completed` result whose
        // "content" is just the stopped-generation placeholder.
        Ok(run) if run.stream_outcome == "cancelled" => {
            manager.finish(task_id, SubAgentStatus::Cancelled, None, None, None);
            let structured = serde_json::json!({
                "type": "subagent",
                "taskId": task_id,
                "name": name,
                "agentType": agent_type,
                "model": model,
                "status": "cancelled",
                "error": "cancelled",
            });
            McpToolCallResult {
                content: format!("[Sub-agent: {} ({})] cancelled", name, agent_type),
                is_error: false,
                raw: structured.clone(),
                artifacts: Vec::new(),
                structured_content: Some(structured),
                follow_up_user_messages: Vec::new(),
            }
        }
        Ok(run) => {
            let content = if run.content.trim().is_empty() {
                "(sub-agent produced no text output)".to_string()
            } else {
                run.content.clone()
            };
            let usage = run.usage.as_ref().map(SubAgentUsage::from_model_usage);
            manager.finish(
                task_id,
                SubAgentStatus::Completed,
                Some(clip(&content, RESULT_PREVIEW_MAX)),
                None,
                usage.clone(),
            );
            let mut structured = serde_json::json!({
                "type": "subagent",
                "taskId": task_id,
                "name": name,
                "agentType": agent_type,
                "model": model,
                "status": "completed",
                "result": clip(&content, RESULT_PREVIEW_MAX),
            });
            if let Some(usage) = usage {
                if let Some(obj) = structured.as_object_mut() {
                    obj.insert(
                        "usage".to_string(),
                        serde_json::to_value(&usage).unwrap_or(Value::Null),
                    );
                }
            }
            McpToolCallResult {
                content: format!("[Sub-agent: {} ({})]\n\n{}", name, agent_type, content),
                is_error: false,
                raw: structured.clone(),
                artifacts: Vec::new(),
                structured_content: Some(structured),
                follow_up_user_messages: Vec::new(),
            }
        }
        Err(err) => {
            let cancelled = err == "cancelled";
            let status = if cancelled {
                SubAgentStatus::Cancelled
            } else {
                SubAgentStatus::Failed
            };
            // Surface a clean, user/model-facing message instead of the raw
            // internal error string. The empty-assistant-response case (a
            // reasoning model returning nothing in planning) is the common
            // failure; other errors keep their original text.
            let display_err = if err.contains(EMPTY_RESPONSE_MARKER) {
                "Subagent 运行失败：模型返回了空响应（可重试）。".to_string()
            } else {
                err.clone()
            };
            manager.finish(task_id, status, None, Some(display_err.clone()), None);
            let structured = serde_json::json!({
                "type": "subagent",
                "taskId": task_id,
                "name": name,
                "agentType": agent_type,
                "model": model,
                "status": if cancelled { "cancelled" } else { "failed" },
                "error": display_err,
            });
            McpToolCallResult {
                content: format!(
                    "[Sub-agent: {} ({})] failed: {}",
                    name, agent_type, display_err
                ),
                is_error: !cancelled,
                raw: structured.clone(),
                artifacts: Vec::new(),
                structured_content: Some(structured),
                follow_up_user_messages: Vec::new(),
            }
        }
    }
}

// Registry dispatch entry point (returns NativeToolFuture so the static
// `NativeToolCall::SubAgent` variant can hold a single fn-pointer shape).

/// `agent` spawn — already async-shaped.
pub fn dispatch_agent_spawn(ctx: SubAgentCallCtx<'_>) -> NativeToolFuture<'_> {
    handle_agent_spawn(ctx)
}

fn compose_persona(persona: &str) -> String {
    let persona = persona.trim();
    if persona.is_empty() {
        "You are a sub-agent spawned to complete a focused task autonomously. Use the available tools, then return a clear, complete final answer. You cannot ask the user questions.".to_string()
    } else {
        format!(
            "{persona}\n\nYou are running as a sub-agent: work autonomously with the available tools and return a clear, complete final answer. You cannot ask the user questions."
        )
    }
}

/// Persona + `skills:` PRELOAD (industry semantics): the listed skills' full
/// bodies are injected into the launch context so the sub-agent starts with that
/// knowledge instead of having to activate them first. It does NOT restrict which
/// skills it may activate later — the registry stays full.
///
/// Rides on `custom_system_prompt` (the persona channel) rather than
/// `active_skill_detail`: that channel only injects under the
/// `skill_md_only`/`legacy_full_body` fallback modes, so on the default
/// `progressive` mode a preload through it would silently do nothing.
fn compose_persona_with_preloaded_skills(
    persona: &str,
    registry: &SkillRegistry,
    skills: &[String],
) -> String {
    let base = compose_persona(persona);
    let bodies: Vec<String> = skills
        .iter()
        .filter_map(|id| registry.find(id))
        .filter(|record| !record.body.trim().is_empty())
        .map(|record| format!("Preloaded Skill ({}):\n{}", record.meta.name, record.body))
        .collect();
    if bodies.is_empty() {
        return base;
    }
    format!("{base}\n\n{}", bodies.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_agent_tool_name_detection() {
        assert!(is_sub_agent_tool_name("agent"));
        assert!(!is_sub_agent_tool_name("check_agent_result"));
        assert!(!is_sub_agent_tool_name("read_file"));
    }

    #[test]
    fn progress_upsert_dedups_by_id_and_updates_status() {
        let mut p = ProgressState::default();
        // Same call id transitions Pending→Running→Success: one slot, not three.
        p.upsert_tool("call-1", "web_search", ToolCallStatus::Pending);
        p.upsert_tool("call-1", "web_search", ToolCallStatus::Running);
        p.upsert_tool("call-1", "web_search", ToolCallStatus::Success);
        assert_eq!(p.tools.len(), 1);
        assert!(matches!(p.tools[0].status, ToolCallStatus::Success));
    }

    #[test]
    fn progress_aggregate_counts_per_tool_name() {
        let mut p = ProgressState::default();
        // 6 distinct web_search calls done, 2 still running.
        for i in 0..6 {
            p.upsert_tool(
                &format!("ws-done-{i}"),
                "web_search",
                ToolCallStatus::Success,
            );
        }
        for i in 0..2 {
            p.upsert_tool(
                &format!("ws-run-{i}"),
                "web_search",
                ToolCallStatus::Running,
            );
        }
        // 3 read_file done, 1 failed.
        for i in 0..3 {
            p.upsert_tool(
                &format!("rf-done-{i}"),
                "read_file",
                ToolCallStatus::Success,
            );
        }
        p.upsert_tool("rf-fail", "read_file", ToolCallStatus::Error);

        let steps = p.aggregate_steps();
        // One line per distinct tool name, first-seen order preserved.
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0], "web_search · 6 done · 2 running");
        assert_eq!(steps[1], "read_file · 3 done · 1 failed");
    }

    #[test]
    fn progress_aggregate_omits_zero_count_states() {
        let mut p = ProgressState::default();
        p.upsert_tool("g-1", "grep", ToolCallStatus::Running);
        let steps = p.aggregate_steps();
        assert_eq!(steps, vec!["grep · 1 running".to_string()]);
    }

    #[test]
    fn append_tools_strips_spawn_when_not_allowed() {
        let mut tools = Vec::new();
        append_tool_definitions(&mut tools, false, &[]);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            !names.contains(&"agent"),
            "spawn tool must be hidden in sub-agents"
        );
    }

    #[test]
    fn append_tools_includes_spawn_when_allowed() {
        let mut tools = Vec::new();
        append_tool_definitions(&mut tools, true, &[]);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"agent"));
    }

    // -----------------------------------------------------------------------
    // Dynamic role schema (R1) + inline ad-hoc roles (R2)
    // -----------------------------------------------------------------------

    fn test_def(id: &str, description: &str) -> AgentDefinition {
        AgentDefinition {
            id: id.to_string(),
            name: id.to_string(),
            description: description.to_string(),
            system_prompt: "base persona".to_string(),
            model: None,
            tools: vec!["read".to_string()],
            disallowed_tools: Vec::new(),
            skills: Vec::new(),
            source: "user".to_string(),
        }
    }

    /// The `subagent_type` description must name EVERY loaded role (including
    /// user/project `.md` roles) — that is how the model discovers them.
    #[test]
    fn schema_description_lists_every_loaded_role() {
        let defs = [
            crate::agents::builtin_agent_definitions(),
            vec![test_def("my-analyst", "财报结构化分析")],
        ]
        .concat();
        let tool = agent_tool(&defs);
        let description = tool.input_schema["properties"]["subagent_type"]["description"]
            .as_str()
            .expect("subagent_type description");
        for def in &defs {
            assert!(
                description.contains(&def.name),
                "role {} must be listed in the schema",
                def.name
            );
        }
        assert!(description.contains("财报结构化分析"));
        // No `enum`: an inline ad-hoc role omits the field, and a role file
        // created mid-run must not be rejected provider-side.
        assert!(tool.input_schema["properties"]["subagent_type"]
            .get("enum")
            .is_none());
    }

    /// The inline ad-hoc fields are declared even though `additionalProperties`
    /// stays false — otherwise a strict provider rejects the call.
    #[test]
    fn schema_declares_inline_role_fields() {
        let tool = agent_tool(&[]);
        let properties = &tool.input_schema["properties"];
        for key in ["system_prompt", "tools", "disallowed_tools"] {
            assert!(properties.get(key).is_some(), "{key} must be declared");
        }
        assert_eq!(tool.input_schema["additionalProperties"], false);
    }

    #[test]
    fn inline_overrides_replace_the_named_role_fields() {
        let mut def = test_def("researcher", "Read-only research");
        def.disallowed_tools = vec!["bash".to_string()];
        apply_inline_overrides(
            &mut def,
            &serde_json::json!({
                "system_prompt": "  You only read Notion.  ",
                "tools": ["mcp__notion__*", " "],
                "disallowed_tools": ["write"]
            }),
        );
        assert_eq!(def.system_prompt, "You only read Notion.");
        // Whole replacement, not a merge (blank entries dropped).
        assert_eq!(def.tools, vec!["mcp__notion__*".to_string()]);
        assert_eq!(def.disallowed_tools, vec!["write".to_string()]);
    }

    #[test]
    fn inline_overrides_absent_or_empty_leave_the_role_untouched() {
        let base = test_def("researcher", "Read-only research");
        // Nothing inline.
        let mut def = base.clone();
        apply_inline_overrides(&mut def, &serde_json::json!({ "prompt": "go" }));
        assert_eq!(def.system_prompt, base.system_prompt);
        assert_eq!(def.tools, base.tools);
        assert_eq!(def.disallowed_tools, base.disallowed_tools);
        // Blank string / empty arrays count as "not provided" — an empty allow
        // list must not become a narrowing that blocks everything.
        let mut def = base.clone();
        apply_inline_overrides(
            &mut def,
            &serde_json::json!({ "system_prompt": "   ", "tools": [], "disallowed_tools": [] }),
        );
        assert_eq!(def.system_prompt, base.system_prompt);
        assert_eq!(def.tools, base.tools);
        assert_eq!(def.disallowed_tools, base.disallowed_tools);
    }

    /// Pure-inline role: no `subagent_type`, so `general-purpose` is the base and
    /// the inline fields fill it in completely (no special-case branch needed).
    #[test]
    fn inline_only_role_builds_on_general_purpose() {
        let defs = crate::agents::builtin_agent_definitions();
        let mut def = crate::agents::find_definition(&defs, "general-purpose")
            .expect("general-purpose")
            .clone();
        apply_inline_overrides(
            &mut def,
            &serde_json::json!({
                "prompt": "audit the config",
                "system_prompt": "You audit configuration files.",
                "tools": ["read", "grep"]
            }),
        );
        assert_eq!(def.system_prompt, "You audit configuration files.");
        assert_eq!(
            def.tools,
            vec!["read".to_string(), "grep".to_string()],
            "inline allow-list must narrow a general-purpose base"
        );
    }

    /// The spawn refusal message must name the entries that resolved to nothing,
    /// which is exactly what `unresolved_allow_entries` reports.
    #[test]
    fn zero_tool_refusal_names_the_unresolved_entries() {
        let pool = vec![agent_tool(&[]), crate::mcp::types::native_read_file_tool()];
        let mut def = test_def("typo-agent", "");
        def.tools = vec!["reed_file".to_string(), "mcp__notionn__*".to_string()];
        let unresolved = crate::chat::agent::filter::unresolved_allow_entries(&pool, &def);
        assert_eq!(
            unresolved,
            vec!["reed_file".to_string(), "mcp__notionn__*".to_string()]
        );
        let message = zero_tool_refusal(&def, &unresolved, false);
        assert!(message.contains("reed_file"));
        assert!(message.contains("mcp__notionn__*"));
        assert!(message.contains("matched no available tool"));
        // And the tool table really is empty after filtering.
        let mut filtered = pool.clone();
        crate::chat::agent::filter::filter_tools_for_agent(&mut filtered, &def);
        assert!(filtered.is_empty());
    }

    /// Mirror of the spawn path's `deny_emptied_pool` computation, so the tests
    /// exercise the same predicate the runtime uses.
    fn deny_emptied_pool(pool: &[ChatToolDefinition], def: &AgentDefinition) -> bool {
        !def.disallowed_tools.is_empty()
            && !pool.is_empty()
            && pool.iter().all(|tool| {
                def.disallowed_tools
                    .iter()
                    .any(|entry| crate::chat::agent::filter::entry_matches(tool, entry))
                    || is_sub_agent_tool_name(&tool.name)
            })
    }

    #[test]
    fn deny_all_refusal_blames_the_denylist_not_the_allow_entries() {
        // Observed in the wild: the model sent `disallowed_tools: ["*"]` with the
        // built-in `coder` role. Every allow entry was spelled correctly, yet
        // `unresolved_allow_entries` reports them all (it skips denied tools), so
        // the message must key on the denylist instead — otherwise the model goes
        // off "fixing" names that were never wrong.
        let pool = vec![agent_tool(&[]), crate::mcp::types::native_read_file_tool()];
        let mut def = test_def("coder", "");
        def.tools = vec!["read".to_string(), "grep".to_string()];
        def.disallowed_tools = vec!["*".to_string()];

        assert!(deny_emptied_pool(&pool, &def));
        let unresolved = crate::chat::agent::filter::unresolved_allow_entries(&pool, &def);
        let message = zero_tool_refusal(&def, &unresolved, true);
        assert!(message.contains("disallowed_tools"), "{message}");
        assert!(
            !message.contains("matched no available tool"),
            "correctly-spelled entries must not be blamed: {message}"
        );

        // A deny-all really does empty the table even though `tools` is non-empty.
        let mut filtered = pool.clone();
        crate::chat::agent::filter::filter_tools_for_agent(&mut filtered, &def);
        assert!(filtered.is_empty());
    }

    #[test]
    fn deny_all_is_caught_even_when_no_allow_list_is_set() {
        // The refusal guard must key on EITHER list: with `tools` empty, an
        // allow-only guard would let a zero-tool sub-agent launch silently.
        let pool = vec![crate::mcp::types::native_read_file_tool()];
        let mut def = test_def("general-purpose", "");
        def.tools = Vec::new();
        def.disallowed_tools = vec!["*".to_string()];

        let mut filtered = pool.clone();
        crate::chat::agent::filter::filter_tools_for_agent(&mut filtered, &def);
        assert!(filtered.is_empty(), "deny-all empties the pool");

        let narrowing_requested = !def.tools.is_empty() || !def.disallowed_tools.is_empty();
        assert!(
            narrowing_requested,
            "the denylist alone must trip the guard"
        );
        assert!(deny_emptied_pool(&pool, &def));
    }

    // -----------------------------------------------------------------------
    // Skills preload (R4)
    // -----------------------------------------------------------------------

    fn skill_record(id: &str, body: &str) -> crate::skills::SkillRecord {
        crate::skills::SkillRecord {
            meta: crate::skills::SkillMeta {
                id: id.to_string(),
                name: id.to_string(),
                description: String::new(),
                source: "builtin".to_string(),
                path: None,
                recommended_tools: Vec::new(),
                disable_model_invocation: false,
                files: Vec::new(),
                triggers: Vec::new(),
                argument_hint: None,
                arguments: Vec::new(),
            },
            location: std::path::PathBuf::new(),
            base_dir: std::path::PathBuf::new(),
            body: body.to_string(),
        }
    }

    #[test]
    fn preloaded_skills_inject_full_bodies_into_the_persona() {
        let registry = SkillRegistry {
            records: vec![
                skill_record("pdf", "PDF STEPS BODY"),
                skill_record("docx", "DOCX STEPS BODY"),
            ],
            warnings: Vec::new(),
        };
        let persona = compose_persona_with_preloaded_skills(
            "You analyse documents.",
            &registry,
            &["pdf".to_string(), "docx".to_string()],
        );
        assert!(persona.contains("You analyse documents."));
        assert!(persona.contains("PDF STEPS BODY"));
        assert!(persona.contains("DOCX STEPS BODY"));
    }

    #[test]
    fn no_preload_leaves_the_persona_identical_and_unknown_ids_are_skipped() {
        let registry = SkillRegistry {
            records: vec![skill_record("pdf", "PDF STEPS BODY")],
            warnings: Vec::new(),
        };
        let plain = compose_persona("You analyse documents.");
        assert_eq!(
            compose_persona_with_preloaded_skills("You analyse documents.", &registry, &[]),
            plain
        );
        assert_eq!(
            compose_persona_with_preloaded_skills(
                "You analyse documents.",
                &registry,
                &["nope".to_string()]
            ),
            plain
        );
    }

    #[test]
    fn manager_register_and_lookup_by_id_and_name() {
        let manager = SubAgentManager::default();
        manager.register(SubAgentTaskRecord {
            id: "agent-1".to_string(),
            name: "researcher".to_string(),
            agent_type: "researcher".to_string(),
            status: SubAgentStatus::Running,
            result: None,
            error: None,
            depth: 1,
            created_at: 100,
            completed_at: None,
            usage: None,
        });
        assert_eq!(manager.get("agent-1").unwrap().name, "researcher");
        assert_eq!(manager.get("researcher").unwrap().id, "agent-1");
        manager.finish(
            "agent-1",
            SubAgentStatus::Completed,
            Some("done".into()),
            None,
            None,
        );
        let rec = manager.get("agent-1").unwrap();
        assert_eq!(rec.status, SubAgentStatus::Completed);
        assert_eq!(rec.result.as_deref(), Some("done"));
        assert_eq!(manager.list().len(), 1);
    }

    fn running_record(id: &str) -> SubAgentTaskRecord {
        SubAgentTaskRecord {
            id: id.to_string(),
            name: id.to_string(),
            agent_type: "general-purpose".to_string(),
            status: SubAgentStatus::Running,
            result: None,
            error: None,
            depth: 1,
            created_at: 0,
            completed_at: None,
            usage: None,
        }
    }

    /// A record starts Pending (before a permit is acquired) and flips to
    /// Running once the run starts.
    #[test]
    fn pending_then_running_status_flow() {
        let manager = SubAgentManager::default();
        let mut rec = running_record("agent-p");
        rec.status = SubAgentStatus::Pending;
        manager.register(rec);
        assert_eq!(
            manager.get("agent-p").unwrap().status,
            SubAgentStatus::Pending
        );
        manager.set_status("agent-p", SubAgentStatus::Running);
        assert_eq!(
            manager.get("agent-p").unwrap().status,
            SubAgentStatus::Running
        );
    }

    #[test]
    fn manager_caps_table_and_evicts_oldest_completed() {
        let manager = SubAgentManager::default();
        // One running task that must never be evicted.
        manager.register(SubAgentTaskRecord {
            id: "running".to_string(),
            name: "running".to_string(),
            agent_type: "t".to_string(),
            status: SubAgentStatus::Running,
            result: None,
            error: None,
            depth: 1,
            created_at: 0,
            completed_at: None,
            usage: None,
        });
        // MAX + 50 completed tasks, ascending completed_at so we know the order.
        for i in 0..(MAX_SUB_AGENT_TASKS + 50) {
            manager.register(SubAgentTaskRecord {
                id: format!("done-{i}"),
                name: format!("done-{i}"),
                agent_type: "t".to_string(),
                status: SubAgentStatus::Completed,
                result: Some("r".to_string()),
                error: None,
                depth: 1,
                created_at: i as i64 + 1,
                completed_at: Some(i as i64 + 1),
                usage: None,
            });
        }
        // Capped.
        assert!(manager.list().len() <= MAX_SUB_AGENT_TASKS);
        // Running task survived despite being the oldest by created_at.
        assert!(manager.get("running").is_some());
        // Oldest completed evicted; newest completed retained.
        assert!(manager.get("done-0").is_none());
        assert!(manager
            .get(&format!("done-{}", MAX_SUB_AGENT_TASKS + 49))
            .is_some());
        // by_name reverse lookup for an evicted id is gone too (no leak there).
        assert!(manager.get("done-0").is_none());
    }

    #[test]
    fn depth_guard_rejects_at_max_depth() {
        // Acceptance #2: depth >= 3 cannot spawn.
        assert!(depth_allows_spawn(0));
        assert!(depth_allows_spawn(1));
        assert!(depth_allows_spawn(2));
        assert!(!depth_allows_spawn(3));
        assert!(!depth_allows_spawn(4));
        assert_eq!(MAX_SUB_AGENT_DEPTH, 3);
    }

    #[test]
    fn dropped_spawn_backstop_cancels_record_and_retires_generation() {
        // The tool scheduler's select! drops the spawn future on user stop /
        // outer timeout. The backstop must (a) move the record out of
        // Pending/Running so it stays evictable, and (b) retire the synthetic
        // sub-conversation's generation so chat_active_generations doesn't leak.
        let state = crate::state::test_app_state();
        state.sub_agents.register(running_record("agent-drop"));
        let sub_gen = state.next_chat_generation("subagent-agent-drop");
        assert!(state.is_chat_generation_active("subagent-agent-drop", sub_gen));

        finalize_dropped_spawn(&state, "agent-drop");

        assert_eq!(
            state.sub_agents.get("agent-drop").unwrap().status,
            SubAgentStatus::Cancelled
        );
        assert!(!state.is_chat_generation_active("subagent-agent-drop", sub_gen));
        // The one-shot synthetic conversation key itself is gone (no map growth).
        assert!(!state
            .chat_active_generations
            .lock()
            .unwrap()
            .contains_key("subagent-agent-drop"));
    }

    #[test]
    fn host_cancels_when_parent_generation_cancelled() {
        // Acceptance #3: parent cancel cascades to the sub-agent.
        let state = crate::state::test_app_state();
        let parent_gen = state.next_chat_generation("conv-parent");
        let sub_gen = state.next_chat_generation("subagent-x");
        // Both live → active.
        assert!(generation_cascade_active(
            &state,
            "subagent-x",
            sub_gen,
            "conv-parent",
            parent_gen
        ));
        // Cancel the PARENT → sub-agent must report inactive (cascade).
        state.cancel_chat_generation("conv-parent");
        assert!(!generation_cascade_active(
            &state,
            "subagent-x",
            sub_gen,
            "conv-parent",
            parent_gen
        ));
    }

    fn ok_run_result() -> Result<AgentRunResult, String> {
        Ok(AgentRunResult {
            content: "done".to_string(),
            reasoning: None,
            tool_records: Vec::new(),
            segments: Vec::new(),
            api_messages: Vec::new(),
            stream_outcome: String::new(),
            usage: None,
            last_step_usage: None,
            compacted_history: None,
            compaction_boundary: None,
            compaction_summary: None,
            images: Vec::new(),
            degraded: None,
        })
    }

    /// A cancelled run now surfaces as `Ok(result)` with `stream_outcome ==
    /// "cancelled"` from the planning/loop-top paths (not just `Err("cancelled")`
    /// from the tool round). `run_sub_agent` short-circuits any `Ok(_)`, so it is
    /// never retried; `should_retry_sub_agent` returns false for it because it is
    /// not an `Err`.
    fn ok_cancelled_run_result() -> Result<AgentRunResult, String> {
        Ok(AgentRunResult {
            content: "已停止生成。".to_string(),
            reasoning: None,
            tool_records: Vec::new(),
            segments: Vec::new(),
            api_messages: Vec::new(),
            stream_outcome: "cancelled".to_string(),
            usage: None,
            last_step_usage: None,
            compacted_history: None,
            compaction_boundary: None,
            compaction_summary: None,
            images: Vec::new(),
            degraded: None,
        })
    }

    fn finalize_params<'a>(task_id: &'a str) -> SubAgentFinalizeParams<'a> {
        SubAgentFinalizeParams {
            task_id,
            name: "Researcher",
            agent_type: "general-purpose",
            model: "test-model",
        }
    }

    /// A completed run flips the record to Completed and returns a non-error
    /// result whose structured content carries the completed status.
    #[test]
    fn compute_finalization_completed() {
        let manager = SubAgentManager::default();
        manager.register(running_record("ok"));
        let result =
            compute_sub_agent_finalization(&manager, &finalize_params("ok"), ok_run_result());
        assert!(!result.is_error);
        let structured = result.structured_content.expect("structured");
        assert_eq!(structured["status"], "completed");
        assert_eq!(manager.get("ok").unwrap().status, SubAgentStatus::Completed);
    }

    /// A cancelled run (Ok with stream_outcome == "cancelled") maps to Cancelled,
    /// never Completed, and is not surfaced as a tool error.
    #[test]
    fn compute_finalization_cancelled_outcome_maps_to_cancelled() {
        let manager = SubAgentManager::default();
        manager.register(running_record("cancel"));
        let result = compute_sub_agent_finalization(
            &manager,
            &finalize_params("cancel"),
            ok_cancelled_run_result(),
        );
        assert!(!result.is_error);
        let structured = result.structured_content.expect("structured");
        assert_eq!(structured["status"], "cancelled");
        assert_eq!(
            manager.get("cancel").unwrap().status,
            SubAgentStatus::Cancelled
        );
    }

    /// An Err outcome maps the record to Failed with the error preserved and the
    /// result surfaced as a tool error.
    #[test]
    fn compute_finalization_error_outcome_maps_to_failed() {
        let manager = SubAgentManager::default();
        manager.register(running_record("fail"));
        let result = compute_sub_agent_finalization(
            &manager,
            &finalize_params("fail"),
            Err("boom".to_string()),
        );
        assert!(
            result.is_error,
            "a real failure must surface as a tool error"
        );
        let structured = result.structured_content.expect("structured");
        assert_eq!(structured["status"], "failed");
        assert_eq!(structured["error"], "boom");
        let rec = manager.get("fail").unwrap();
        assert_eq!(rec.status, SubAgentStatus::Failed);
        assert_eq!(rec.error.as_deref(), Some("boom"));
    }

    /// An Err("cancelled") maps to Cancelled (not Failed) and is not a tool error.
    #[test]
    fn compute_finalization_err_cancelled_maps_to_cancelled() {
        let manager = SubAgentManager::default();
        manager.register(running_record("errcancel"));
        let result = compute_sub_agent_finalization(
            &manager,
            &finalize_params("errcancel"),
            Err("cancelled".to_string()),
        );
        assert!(!result.is_error);
        let structured = result.structured_content.expect("structured");
        assert_eq!(structured["status"], "cancelled");
        assert_eq!(
            manager.get("errcancel").unwrap().status,
            SubAgentStatus::Cancelled
        );
    }

    #[test]
    fn retry_only_on_recoverable_error_with_attempts_and_active_parent() {
        // SUB_AGENT_MAX_ATTEMPTS == 2: attempt 0 may retry, attempt 1 may not.
        assert_eq!(SUB_AGENT_MAX_ATTEMPTS, 2);

        let recoverable: Result<AgentRunResult, String> =
            Err("Chat tools planning returned an empty assistant response".to_string());

        // First attempt, parent alive, recoverable error → retry.
        assert!(should_retry_sub_agent(&recoverable, 0, true));
        // Last attempt → no retry even though recoverable.
        assert!(!should_retry_sub_agent(&recoverable, 1, true));
        // Parent cancelled → never retry (cascade).
        assert!(!should_retry_sub_agent(&recoverable, 0, false));

        // Cancellation is not a recoverable error → never retry.
        let cancelled: Result<AgentRunResult, String> = Err("cancelled".to_string());
        assert!(!should_retry_sub_agent(&cancelled, 0, true));

        // Any OTHER fatal error must NOT retry: a retry re-runs the whole loop, so
        // a run that already wrote files over several rounds before failing would
        // repeat those side effects. Only the empty-response class is retryable.
        for other in [
            "provider request failed: 500 Internal Server Error",
            "context length exceeded",
            "tool execution failed: write_file denied",
        ] {
            let fatal: Result<AgentRunResult, String> = Err(other.to_string());
            assert!(
                !should_retry_sub_agent(&fatal, 0, true),
                "must not retry (side-effect replay): {other}"
            );
        }

        // Success → never retry.
        assert!(!should_retry_sub_agent(&ok_run_result(), 0, true));

        // An Ok(cancelled) result (planning/loop-top cancellation now returns
        // Ok, not Err) → never retried: it is not an Err, so should_retry is
        // false, and run_sub_agent short-circuits any Ok(_) before retrying.
        assert!(!should_retry_sub_agent(&ok_cancelled_run_result(), 0, true));
    }

    // -----------------------------------------------------------------------
    // Parallel fan-out: multiple `agent` tool calls in ONE round run
    // concurrently (Claude Code model — blocking + single-result per call, with
    // parallelism coming from the model emitting several `agent` calls at once).
    //
    // The orchestrator's `run_agent_loop` is the REAL production engine, driven
    // against a scripted mock model wire. The `agent` tool is test-shimmed (the
    // production `handle_agent_spawn` is bound to a concrete `AppHandle<Wry>`,
    // which a `tao` event loop can't build off the main thread), but the shim
    // replicates the blocking contract: each call awaits to completion and
    // returns the full result inline. Concurrency is PROVEN by a barrier — every
    // dispatched `agent` call must reach "in-flight" before ANY is allowed to
    // finish, which is only possible if they run in parallel.

    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    use crate::chat::agent::{
        run_agent_loop, AgentHost, AgentHostFuture, AgentRunConfig, ToolExecutionContext,
        ToolExecutor, ToolExecutorFuture,
    };
    use crate::chat::ask_user::{AskUserPromptPayload, AskUserResponseResult};
    use crate::settings::{ChatToolsConfig, ModelProvider, Settings};
    use crate::state::AppState;

    // -----------------------------------------------------------------------
    // resolve_sub_agent_provider_model: the three-tier priority.
    // -----------------------------------------------------------------------

    fn named_provider(id: &str) -> ModelProvider {
        ModelProvider {
            id: id.to_string(),
            name: id.to_string(),
            api_keys: vec!["key".to_string()],
            api_key_legacy: None,
            base_url: "http://localhost".to_string(),
            available_models: Vec::new(),
            enabled_models: Vec::new(),
            enabled: true,
            api_format: "openai_chat".to_string(),
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        }
    }

    fn settings_with_providers(providers: Vec<ModelProvider>) -> Settings {
        Settings {
            providers,
            ..Settings::default()
        }
    }

    #[test]
    fn sub_agent_model_follows_parent_by_default() {
        let settings = settings_with_providers(vec![named_provider("parent-p")]);
        let (provider, model) =
            resolve_sub_agent_provider_model(&settings, "parent-p", "parent-model", None)
                .expect("resolves");
        assert_eq!(provider.id, "parent-p");
        assert_eq!(model, "parent-model");
    }

    #[test]
    fn sub_agent_model_uses_global_override_cross_provider() {
        let mut settings =
            settings_with_providers(vec![named_provider("parent-p"), named_provider("cheap-p")]);
        settings.chat_tools.sub_agent_provider_id = "cheap-p".to_string();
        settings.chat_tools.sub_agent_model = "cheap-model".to_string();
        let (provider, model) =
            resolve_sub_agent_provider_model(&settings, "parent-p", "parent-model", None)
                .expect("resolves");
        assert_eq!(provider.id, "cheap-p");
        assert_eq!(model, "cheap-model");
    }

    #[test]
    fn sub_agent_definition_model_beats_global_override() {
        let mut settings =
            settings_with_providers(vec![named_provider("parent-p"), named_provider("cheap-p")]);
        settings.chat_tools.sub_agent_provider_id = "cheap-p".to_string();
        settings.chat_tools.sub_agent_model = "cheap-model".to_string();
        // Definition model resolves on the PARENT provider (pre-existing semantics).
        let (provider, model) = resolve_sub_agent_provider_model(
            &settings,
            "parent-p",
            "parent-model",
            Some("def-model"),
        )
        .expect("resolves");
        assert_eq!(provider.id, "parent-p");
        assert_eq!(model, "def-model");
    }

    #[test]
    fn sub_agent_unusable_override_falls_back_to_parent() {
        // Override points at a provider that is disabled / keyless / missing →
        // fall back to the parent rather than failing the spawn.
        let mut keyless = named_provider("cheap-p");
        keyless.api_keys.clear();
        let mut settings = settings_with_providers(vec![named_provider("parent-p"), keyless]);
        settings.chat_tools.sub_agent_provider_id = "cheap-p".to_string();
        settings.chat_tools.sub_agent_model = "cheap-model".to_string();
        let (provider, model) =
            resolve_sub_agent_provider_model(&settings, "parent-p", "parent-model", None)
                .expect("resolves");
        assert_eq!(provider.id, "parent-p");
        assert_eq!(model, "parent-model");

        settings.chat_tools.sub_agent_provider_id = "missing-p".to_string();
        let (provider, model) =
            resolve_sub_agent_provider_model(&settings, "parent-p", "parent-model", None)
                .expect("resolves");
        assert_eq!(provider.id, "parent-p");
        assert_eq!(model, "parent-model");
    }

    fn test_provider(base_url: &str) -> ModelProvider {
        ModelProvider {
            id: "test-provider".to_string(),
            name: "Test Provider".to_string(),
            api_keys: vec!["test-key".to_string()],
            api_key_legacy: None,
            base_url: base_url.to_string(),
            available_models: Vec::new(),
            enabled_models: Vec::new(),
            enabled: true,
            api_format: "openai_chat".to_string(),
            model_overrides: std::collections::HashMap::new(),
            compress_request_body: false,
            request: Default::default(),
        }
    }

    /// Body-capturing multi-response mock for the orchestrator's
    /// OpenAI-compatible chat endpoint: serves one canned JSON completion per
    /// accepted connection (in accept order) and records each request body.
    struct MockOrchestratorServer {
        base_url: String,
        captured_bodies: Arc<Mutex<Vec<String>>>,
    }

    impl MockOrchestratorServer {
        fn start(responses: Vec<String>) -> Self {
            use std::io::{Read as _, Write as _};
            use std::net::TcpListener;
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind orchestrator server");
            let addr = listener.local_addr().expect("orchestrator server addr");
            let captured = Arc::new(Mutex::new(Vec::new()));
            let captured_for_thread = Arc::clone(&captured);
            std::thread::spawn(move || {
                for body in responses {
                    let Ok((mut stream, _)) = listener.accept() else {
                        return;
                    };
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                        .ok();
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 1024];
                    let header_end = loop {
                        let Ok(n) = stream.read(&mut chunk) else {
                            break 0;
                        };
                        if n == 0 {
                            break 0;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break pos + 4;
                        }
                    };
                    if header_end == 0 {
                        continue;
                    }
                    let headers = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
                    let content_length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    while buf.len() < header_end + content_length {
                        let Ok(n) = stream.read(&mut chunk) else {
                            break;
                        };
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    captured_for_thread
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(String::from_utf8_lossy(&buf[header_end..]).into_owned());
                    let body = sse_body_from_completion_json(&body);
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.flush();
                }
            });
            Self {
                base_url: format!("http://{addr}/v1"),
                captured_bodies: captured,
            }
        }

        fn captured_bodies(&self) -> Vec<String> {
            self.captured_bodies
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
    }

    /// 把一份非流式 chat-completion JSON 固件转成 SSE 响应体。
    ///
    /// 「要完整结果」的模型调用现在**一律走流式线**（见 planning.rs 上
    /// `call_chat_completion_output_with_usage` 的注释），`stream_enabled=false` 只表示
    /// 「不往 host 发增量」。固件用非流式 JSON 写着更好读，所以在出口做一次机械转换
    /// （与 `agent/loop_tests.rs::sse_from_completion_json` 同形；两个测试模块互不可见，
    /// 为二十行搭一个共享 test-util 模块不划算）。
    fn sse_body_from_completion_json(body: &str) -> String {
        let parsed: Value = serde_json::from_str(body).expect("mock body must be valid JSON");
        let message = &parsed["choices"][0]["message"];
        let mut events: Vec<String> = Vec::new();
        if let Some(content) = message["content"].as_str().filter(|text| !text.is_empty()) {
            events
                .push(serde_json::json!({"choices":[{"delta":{"content": content}}]}).to_string());
        }
        if let Some(tool_calls) = message["tool_calls"].as_array() {
            // 流式约定要 index；参数不切片（测试只关心最终 draft）。
            let deltas = tool_calls
                .iter()
                .enumerate()
                .map(|(index, call)| {
                    let mut call = call.clone();
                    call["index"] = serde_json::json!(index);
                    call
                })
                .collect::<Vec<_>>();
            events.push(
                serde_json::json!({"choices":[{"delta":{"tool_calls": deltas}}]}).to_string(),
            );
        }
        if let Some(finish) = parsed["choices"][0]["finish_reason"].as_str() {
            events.push(
                serde_json::json!({"choices":[{"delta":{},"finish_reason": finish}]}).to_string(),
            );
        }
        if let Some(usage) = parsed.get("usage").filter(|usage| !usage.is_null()) {
            events.push(serde_json::json!({"choices":[],"usage": usage}).to_string());
        }
        events.push("[DONE]".to_string());
        events
            .iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect()
    }

    /// A no-tool-call final assistant answer (ends the loop).
    fn final_answer_json(text: &str) -> String {
        serde_json::json!({
            "choices": [{
                "finish_reason": "stop",
                "message": { "role": "assistant", "content": text }
            }],
            "usage": { "prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18 }
        })
        .to_string()
    }

    /// One assistant turn with N parallel `agent` tool calls (no background param
    /// — the production blocking contract). `calls` is `(tool_call_id, name)`.
    fn fanout_agent_calls_json(calls: &[(&str, &str)]) -> String {
        let tool_calls: Vec<Value> = calls
            .iter()
            .map(|(call_id, name)| {
                serde_json::json!({
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": "agent",
                        "arguments": serde_json::json!({
                            "name": name,
                            "prompt": "do the task"
                        }).to_string()
                    }
                })
            })
            .collect();
        serde_json::json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": { "role": "assistant", "content": Value::Null, "tool_calls": tool_calls }
            }],
            "usage": { "prompt_tokens": 5, "completion_tokens": 5, "total_tokens": 10 }
        })
        .to_string()
    }

    /// Orchestrator host: inert stream/record surface; generation governed solely
    /// by the orchestrator conversation's generation in the real `AppState`.
    struct OrchestratorHost {
        state: Arc<AppState>,
        conversation_id: String,
    }

    impl AgentHost for OrchestratorHost {
        fn emit_stream_delta(
            &self,
            _conversation_id: &str,
            _run_id: &str,
            _message_id: &str,
            _delta: &str,
            _reasoning_delta: Option<&str>,
            _segment: Option<&ChatMessageSegment>,
        ) {
        }
        fn emit_tool_record(
            &self,
            _conversation_id: &str,
            _run_id: &str,
            _message_id: &str,
            _record: &ToolCallRecord,
        ) {
        }
        fn request_tool_approval<'a>(
            &'a self,
            _ctx: &'a ToolExecutionContext<'a>,
            _record: &'a ToolCallRecord,
        ) -> AgentHostFuture<'a, bool> {
            Box::pin(async move { true })
        }
        fn request_user_response<'a>(
            &'a self,
            _ctx: &'a ToolExecutionContext<'a>,
            _record: &'a ToolCallRecord,
            _prompt: AskUserPromptPayload,
        ) -> AgentHostFuture<'a, AskUserResponseResult> {
            Box::pin(async move {
                AskUserResponseResult {
                    phase: "cancelled".to_string(),
                    answers: HashMap::new(),
                }
            })
        }
        fn is_generation_active(&self, conversation_id: &str, generation: u64) -> bool {
            let _ = conversation_id;
            self.state
                .is_chat_generation_active(&self.conversation_id, generation)
        }
        fn wait_for_generation_inactive<'a>(
            &'a self,
            conversation_id: &'a str,
            generation: u64,
        ) -> AgentHostFuture<'a, ()> {
            Box::pin(async move {
                loop {
                    if !self.is_generation_active(conversation_id, generation) {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
        }
    }

    /// Orchestrator executor whose `agent` tool BLOCKS until completion (the
    /// production contract). Concurrency is proven via a shared barrier: each call
    /// increments `in_flight`, then waits until `in_flight == expected` before
    /// returning. If the loop ran the calls serially this would deadlock (the
    /// first call would block forever waiting for the others to start), so the
    /// test only completes if they run concurrently.
    struct ParallelFanoutExecutor {
        expected: usize,
        in_flight: Arc<AtomicUsize>,
        max_observed: Arc<AtomicUsize>,
    }

    impl ToolExecutor for ParallelFanoutExecutor {
        fn call<'a>(
            &'a self,
            _ctx: &'a ToolExecutionContext<'a>,
            tool: &'a ChatToolDefinition,
            arguments: Value,
            _skill_cache: Option<&'a mut crate::skills::SkillRunCache>,
        ) -> ToolExecutorFuture<'a> {
            let expected = self.expected;
            let in_flight = Arc::clone(&self.in_flight);
            let max_observed = Arc::clone(&self.max_observed);
            let tool_name = tool.name.clone();
            Box::pin(async move {
                assert_eq!(tool_name, AGENT_TOOL_NAME, "only agent calls expected");
                let name = arguments
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("worker")
                    .to_string();
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                max_observed.fetch_max(now, Ordering::SeqCst);
                // Block until every dispatched call is in-flight at once.
                for _ in 0..500 {
                    if in_flight.load(Ordering::SeqCst) >= expected {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                assert!(
                    in_flight.load(Ordering::SeqCst) >= expected,
                    "all dispatched agent calls must be in-flight concurrently"
                );
                Ok(McpToolCallResult {
                    content: format!("[Sub-agent: {name}] RESULT_{name}"),
                    is_error: false,
                    raw: Value::Null,
                    artifacts: Vec::new(),
                    structured_content: None,
                    follow_up_user_messages: Vec::new(),
                })
            })
        }
    }

    fn orchestrator_run_config<'a>(
        state: &'a AppState,
        base_url: &str,
        conversation_id: &str,
    ) -> AgentRunConfig<'a> {
        AgentRunConfig {
            state,
            conversation_id: conversation_id.to_string(),
            tool_conversation_id: conversation_id.to_string(),
            depth: 0,
            run_id: "orch-run".to_string(),
            message_id: "orch-msg".to_string(),
            generation: state.next_chat_generation(conversation_id),
            provider: test_provider(base_url),
            model: "test-model".to_string(),
            runtime_messages: vec![
                serde_json::json!({ "role": "system", "content": "you orchestrate workers" }),
                serde_json::json!({ "role": "user", "content": "research three topics in parallel" }),
            ],
            tools: vec![agent_tool(&[])],
            blocked_tool_calls: Vec::new(),
            settings: Settings::default(),
            effective_chat_tools: ChatToolsConfig {
                max_tool_rounds: Some(8),
                ..ChatToolsConfig::default()
            },
            language: "zh-CN".to_string(),
            thinking_enabled: false,
            thinking_level: None,
            // 子代理不做联网搜索（父代理的搜索结果已在上下文里）。
            web_search_mode: crate::chat::types::WebSearchMode::Off,
            stream_enabled: false,
            max_output_tokens: 1024,
            retry_attempts: 1,
            assistant_snapshot: None,
            provider_tools_fallback_system_prompt: String::new(),
            initial_anchor_total_tokens: None,
            initial_anchor_trailing_estimate: 0,
        }
    }

    /// THE parallel-fan-out test. The orchestrator model emits THREE `agent`
    /// tool calls in ONE assistant turn; the loop runs them concurrently via
    /// `execute_parallel_chunk` (join_all). The blocking executor (each call
    /// waits until all three are in-flight before returning) only completes if
    /// they truly run in parallel — serial execution would deadlock and hit the
    /// timeout. All three results return together; a final answer ends the run.
    #[tokio::test]
    async fn three_agent_calls_in_one_round_run_concurrently_and_all_return() {
        let orchestrator = MockOrchestratorServer::start(vec![
            fanout_agent_calls_json(&[("call_a", "A"), ("call_b", "B"), ("call_c", "C")]),
            final_answer_json("All three workers finished."),
        ]);
        let state = Arc::new(crate::state::test_app_state());

        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_observed = Arc::new(AtomicUsize::new(0));
        let executor = ParallelFanoutExecutor {
            expected: 3,
            in_flight: Arc::clone(&in_flight),
            max_observed: Arc::clone(&max_observed),
        };
        let host = OrchestratorHost {
            state: Arc::clone(&state),
            conversation_id: "orch-conv".to_string(),
        };

        let config = orchestrator_run_config(&state, &orchestrator.base_url, "orch-conv");
        let result = tokio::time::timeout(
            Duration::from_secs(20),
            run_agent_loop(config, &host, &executor),
        )
        .await
        .expect("parallel fan-out must not deadlock (it would if calls ran serially)")
        .expect("orchestrator run completes Ok");

        // The run reached its final answer.
        assert_eq!(result.stream_outcome, "completed");
        assert_eq!(result.content, "All three workers finished.");

        // All three agent calls were in-flight at once — true concurrency.
        assert_eq!(
            max_observed.load(Ordering::SeqCst),
            3,
            "all three agent calls must execute concurrently in one round"
        );

        // The second model request's history must carry ALL three agent results.
        let bodies = orchestrator.captured_bodies();
        assert_eq!(bodies.len(), 2, "fan-out round + final = 2 model requests");
        for tag in ["RESULT_A", "RESULT_B", "RESULT_C"] {
            assert!(
                bodies[1].contains(tag),
                "round-2 request history must contain {tag} from the concurrent fan-out"
            );
        }
    }
}
