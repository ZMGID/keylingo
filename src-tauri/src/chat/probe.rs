//! 无头测试通道（probe）——仅 debug 构建。
//!
//! 自动化写 `<app_data>/chat_probe/request.json`（字段见 [`ProbeRequest`]）；运行中的 app
//! 轮询到后，走**与聊天窗口完全相同的生成路径**
//! （`commands::run_chat_probe` → `complete_assistant_reply_inner(probe=true)` →
//! `run_agent_loop` + 全量工具集，ProbeAgentHost 自动放行；会话若绑外部 CLI 运行时则走
//! `run_external_cli_reply`），把结果写到
//! `<app_data>/chat_probe/result.json`（带 `id` 时另写 `result-<id>.json`，字段见 [`ProbeResult`]）。
//!
//! **多轮**：把 result 里的 `conversationId` 填回下一次请求，即续聊同一会话——跨轮记忆、
//! 外部 CLI 常驻进程复用、压缩边界这些只有多轮才能验的场景靠它。
//!
//! 用途：自动化 / CI 真实验证 GUI 客户端的行为（工具调用、用量口径、常驻会话复用、取消后
//! 会话是否还能用），免手测。端到端套件是 `scripts/probe-e2e.mjs`（`npm run probe:e2e`）。
//! 整模块 `#[cfg(debug_assertions)]`，release 不含。

#![cfg(debug_assertions)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Listener, Manager};

use crate::chat::types::{ConversationContextState, ToolCallStatus};
use crate::state::AppState;

/// 单次生成超时（无 GUI 应答，靠这个兜底避免 watcher 永久卡住）。
/// orchestrate 编排流程（多子代理并行多轮）常超 2 分钟，放宽到 6 分钟。
const PROBE_TIMEOUT: Duration = Duration::from_secs(360);
/// 轮询间隔——调试用途，延迟无所谓。
const POLL_INTERVAL: Duration = Duration::from_millis(700);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProbeRequest {
    #[serde(default)]
    pub(crate) id: Option<String>,
    pub(crate) prompt: String,
    #[serde(default)]
    pub(crate) provider: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<String>,
    #[serde(default)]
    pub(crate) skill_id: Option<String>,
    /// agent 运行模式（act/plan/orchestrate），省略 = act。用于验证模式提示词。
    #[serde(default)]
    pub(crate) mode: Option<String>,
    /// 文件工具的根目录（read/glob/grep 相对路径从此解析）。省略则用进程 cwd
    /// （dev 通常是仓库根），使文件工具开箱即用。
    ///
    /// **外部 CLI 会话必须每轮传同一个值**：它同时是常驻会话注册表的复用判据之一
    /// （`LiveSession::is_reusable` 比 cwd），换了就等于换会话。
    #[serde(default)]
    pub(crate) cwd: Option<String>,
    /// 会话级联网搜索模式（off/builtin/third_party），省略 = 跟随全局。
    /// 用于无头验证内置搜索链路（任务 07-23）。
    #[serde(default)]
    pub(crate) web_search_mode: Option<String>,
    /// 续聊已有会话（用上一次 result 回传的 `conversationId`）。省略 = 新建会话。
    /// **多轮场景只能靠它**：跨轮记忆、外部 CLI 常驻会话复用、压缩边界，都要同一会话连发多轮。
    #[serde(default)]
    pub(crate) conversation_id: Option<String>,
    /// 把**新建**的会话钉到外部 CLI 运行时（`claude` / `codex` / …）。省略 = 跟随
    /// `settings.chat.defaultAgentRuntime`。续聊时忽略（有消息的外部会话禁切运行时）。
    #[serde(default)]
    pub(crate) external_agent_id: Option<String>,
    /// 外部 CLI 的模型 / 推理档位 / sandbox 档位。**新建与续聊都生效**——spec 第 3b 条只禁切
    /// kind/agent，这三项明确放行，而它们是 claude 的**启动参数**，中途改动会触发
    /// `LaunchConfig` 指纹不匹配 ⇒ 换进程 + 原生 resume（spec 第 26 条）。这条路此前
    /// 完全无法从 app 层验证。
    #[serde(default)]
    pub(crate) external_model: Option<String>,
    #[serde(default)]
    pub(crate) external_reasoning: Option<String>,
    #[serde(default)]
    pub(crate) external_sandbox: Option<String>,
    /// 到点触发一次取消，走**与用户点「停止」完全相同**的路径
    /// （`chat_cancel_stream` 背后的 `AppState::cancel_chat_generation`）。
    ///
    /// 计时从「本轮 generation 真正登记」起算——早于登记调用取消是空操作（那个函数
    /// 只是清空活跃集合），会静默失效。
    #[serde(default)]
    pub(crate) cancel_after_ms: Option<u64>,
    /// 轮末主动算一次上下文状态再回传（复用 `chat_get_context_stats` 那条生产路径）。
    ///
    /// 默认 **false**：`context_state` 平时只在用户点开用量条时才算，probe 跑完不会自动算，
    /// 不开这个开关 `contextState` 里的分子/分母全是空的。
    #[serde(default)]
    pub(crate) compute_context_stats: bool,
}

#[derive(Debug, Serialize)]
struct ProbeToolCall {
    name: String,
    /// 原样的入参（ToolCallRecord.arguments 本身就是 JSON 串）。
    arguments: String,
    status: ToolCallStatus,
}

/// assistant 消息的 provider/CLI 实报用量，**全部字段**。
///
/// 之前只能去翻磁盘上的会话 JSON 才能看到，于是「cache 有没有计进分子」「零用量的轮次有没有
/// 写成 `Some(0)` 污染用量」这类 spec 第 14 系列的口径修复在 app 层根本断言不了。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeUsage {
    input: Option<u64>,
    output: Option<u64>,
    total: Option<u64>,
    cached_input: Option<u64>,
    cache_creation: Option<u64>,
    reasoning: Option<u64>,
    /// CLI 实报的上下文窗口（分母）。外部 CLI 才有；内置路径恒空。
    context_window: Option<u64>,
}

/// 生成过程中推给前端的「上下文占用活数」快照（一次一条，按到达顺序）。
///
/// 轮末的 `contextState` 只能证明**结束后**的数对不对；用量条要不要在**过程中**跟着走，
/// 在 app 层此前完全断言不了。这里靠 Rust 侧订阅 context protocol update（与聊天窗口收的是同一条
/// 事件、同一份载荷）把过程中的每一次推送记下来，让端到端套件能断言
/// 「一轮里至少推过一次」「数字单调不减」。
///
/// 纯 probe 侧代码（整模块 `#[cfg(debug_assertions)]`），生产路径零改动。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeLiveUsageTick {
    used_tokens: u64,
    context_window_tokens: Option<u64>,
}

/// 会话上下文状态投影（分子 / 分母 / 来源 / 压缩计数）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeContextState {
    estimated_input_tokens: usize,
    context_window_tokens: Option<usize>,
    usage_ratio: Option<f32>,
    status: String,
    /// `cli_reported` / `estimated` / `provider_reported`。
    token_count_source: Option<String>,
    /// `external_cli` / `kivio_builtin`。
    context_source: Option<String>,
    compression_count: usize,
    compaction_boundary_count: usize,
}

/// 常驻会话注册表自省（**只读**）。
///
/// 之前判定「常驻真的生效了吗」只能 `Get-CimInstance Win32_Process` 数进程。有了 `childPid`
/// 就能直接断言「两轮是同一个进程」「换启动参数之后 pid 变了」，以及每会话一个进程的隔离。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeLiveSession {
    /// 这个会话在注册表里有条目吗。
    registered: bool,
    /// 条目的 actor 还在听命令吗（`false` = 死条目，下一轮才会被发现）。
    alive: bool,
    child_pid: Option<u32>,
    /// 这个进程已服过几轮（注册即 1，每次被复用 +1）。
    turns_served: Option<u32>,
    /// 注册表里一共几个会话（每会话一个进程 + LRU 上限的观察点）。
    registry_size: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conversation_id: Option<String>,
    answer: String,
    tool_calls: Vec<ProbeToolCall>,
    /// 分段的 kind 序列（按顺序），如 `["reasoning","text","tool","text"]`。
    ///
    /// 只给**相对顺序**，因为消费方只看这个：`phase` / `order` / `round` / `toolCallId`
    /// 四个字段一个都没人读过（order 本来就是数组下标），正文与长度也刻意不带 ——
    /// 全文在 `answer` 里，「正文有没有重复」由 `chat::commands::tests` 的单测守着。
    segments: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<ProbeUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_state: Option<ProbeContextState>,
    /// 生成过程中收到的实时占用推送。**只有内置 agent 会推**（每个 planning 轮一次）；
    /// 外部 CLI 一轮只在轮末更新一次占用，所以那条路上这里恒为空 —— 不是坏了。
    live_usage_ticks: Vec<ProbeLiveUsageTick>,
    live_session: ProbeLiveSession,
    /// 本轮墙钟耗时。常驻的收益（首轮约 3.2s 冷启动 vs 后续约 0.1s）此前只能在 bash 里掐表。
    duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    finished_at: i64,
}

fn probe_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {e}"))?;
    Ok(base.join("chat_probe"))
}

/// 后台轮询 watcher：仅在 debug 构建由 lib.rs 的 `.setup` spawn。
pub async fn run_probe_watcher(app: AppHandle) {
    let dir = match probe_dir(&app) {
        Ok(d) => d,
        Err(err) => {
            eprintln!("[chat-probe] cannot resolve probe dir: {err}");
            return;
        }
    };
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!("[chat-probe] cannot create {}: {err}", dir.display());
        return;
    }
    let request_path = dir.join("request.json");
    eprintln!(
        "[chat-probe] watching {} (debug-only test channel)",
        request_path.display()
    );

    let mut ticker = tokio::time::interval(POLL_INTERVAL);
    let mut last_mtime: Option<std::time::SystemTime> = None;
    loop {
        ticker.tick().await;

        // 去抖：仅在 request.json 存在且 mtime 变化时处理一次。
        let Ok(meta) = std::fs::metadata(&request_path) else {
            continue;
        };
        let mtime = meta.modified().ok();
        if mtime.is_some() && mtime == last_mtime {
            continue;
        }
        last_mtime = mtime;

        let raw = match std::fs::read_to_string(&request_path) {
            Ok(s) => s,
            Err(err) => {
                eprintln!("[chat-probe] read request failed: {err}");
                continue;
            }
        };
        // 先重命名消费，避免重复执行（下次 mtime 也不再命中）。
        let _ = std::fs::rename(&request_path, dir.join("request.consumed"));

        let req: ProbeRequest = match serde_json::from_str(&raw) {
            Ok(r) => r,
            Err(err) => {
                write_result(
                    &dir,
                    &failed_result(None, None, format!("invalid request.json: {err}"), 0),
                );
                continue;
            }
        };

        eprintln!("[chat-probe] running: {:?}", req.prompt);
        let result = handle_probe_request(&app, req).await;
        write_result(&dir, &result);
        eprintln!(
            "[chat-probe] done in {}ms: outcome={:?}, {} tool call(s), pid={:?}{}",
            result.duration_ms,
            result.stream_outcome,
            result.tool_calls.len(),
            result.live_session.child_pid,
            result
                .error
                .as_ref()
                .map(|e| format!(", error={e}"))
                .unwrap_or_default()
        );
    }
}

async fn handle_probe_request(app: &AppHandle, req: ProbeRequest) -> ProbeResult {
    let id = req.id.clone();
    let state = app.state::<AppState>();
    // 缺省 cwd 用进程当前目录（dev 通常是仓库根），让文件工具相对路径开箱即用。
    let mut req = req;
    if req.cwd.is_none() {
        req.cwd = std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string());
    }

    let started = Instant::now();
    // 订阅生成过程中的实时占用推送。用 Rust 侧的事件订阅（`app.listen`）而不是给生产代码
    // 加旁路：`emit_chat_context_usage_live` 发的就是聊天窗口收的同一条 protocol update，
    // 这里收到什么前端就收到什么。整段是 probe 侧代码，生产路径零改动。
    let live_ticks: std::sync::Arc<std::sync::Mutex<Vec<ProbeLiveUsageTick>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let tick_sink = live_ticks.clone();
    let tick_listener = app.listen(crate::chat::protocol::CHAT_PROTOCOL_EVENT, move |event| {
        let Ok(payload) = serde_json::from_str::<serde_json::Value>(event.payload()) else {
            return;
        };
        // 只收实时载荷；轮末的权威快照（`contextState`）走 result 里的 `contextState`。
        if payload.get("type").and_then(|value| value.as_str()) != Some("context_usage_updated") {
            return;
        }
        let Some(usage) = payload.get("usage") else {
            return;
        };
        let used = usage
            .get("usedTokens")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        tick_sink
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .push(ProbeLiveUsageTick {
                used_tokens: used,
                context_window_tokens: usage
                    .get("contextWindowTokens")
                    .and_then(|value| value.as_u64()),
            });
    });
    let run = crate::chat::commands::run_chat_probe(app, &state, &req);
    let outcome = tokio::time::timeout(PROBE_TIMEOUT, run).await;
    app.unlisten(tick_listener);
    let live_usage_ticks =
        std::mem::take(&mut *live_ticks.lock().unwrap_or_else(|err| err.into_inner()));
    let duration_ms = started.elapsed().as_millis() as u64;
    let finished_at = chrono::Local::now().timestamp();

    match outcome {
        Ok(Ok(run)) => {
            let message = run.message;
            ProbeResult {
                id,
                // 回传会话 id：下一次请求带上它就续聊同一会话（多轮验证的入口）。
                conversation_id: Some(run.conversation_id.clone()),
                answer: message.content.clone(),
                tool_calls: message
                    .tool_calls
                    .iter()
                    .map(|r| ProbeToolCall {
                        name: r.name.clone(),
                        arguments: r.arguments.clone(),
                        status: r.status.clone(),
                    })
                    .collect(),
                segments: message
                    .segments
                    .iter()
                    .map(|segment| serde_variant_name(&segment.kind))
                    .collect(),
                stream_outcome: message.stream_outcome.clone(),
                usage: message.usage.as_ref().map(probe_usage),
                context_state: run.context_state.as_ref().map(probe_context_state),
                live_usage_ticks,
                live_session: live_session_snapshot(&state, &run.conversation_id),
                duration_ms,
                error: None,
                finished_at,
            }
        }
        // 生成失败：会话 id 仍要回传（失败往往正是要在同一会话里追查的对象）。
        Ok(Err(run)) => failed_result(
            id,
            Some((&state, run.conversation_id.as_deref())),
            run.error,
            duration_ms,
        ),
        Err(_) => {
            let mut result = failed_result(
                id,
                None,
                format!("probe generation timed out after {PROBE_TIMEOUT:?}"),
                duration_ms,
            );
            result.stream_outcome = Some("timeout".to_string());
            result
        }
    }
}

/// 失败 / 非法请求的 result 骨架。`live` 给了会话 id 时照样带上注册表快照——
/// 「失败之后常驻会话有没有被丢掉」本身就是要断言的东西。
fn failed_result(
    id: Option<String>,
    live: Option<(&AppState, Option<&str>)>,
    error: String,
    duration_ms: u64,
) -> ProbeResult {
    let (conversation_id, live_session) = match live {
        Some((state, Some(conversation_id))) => (
            Some(conversation_id.to_string()),
            live_session_snapshot(state, conversation_id),
        ),
        Some((state, None)) => (None, live_session_snapshot(state, "")),
        None => (None, ProbeLiveSession::default_empty()),
    };
    ProbeResult {
        id,
        conversation_id,
        answer: String::new(),
        tool_calls: Vec::new(),
        segments: Vec::new(),
        stream_outcome: None,
        usage: None,
        context_state: None,
        live_usage_ticks: Vec::new(),
        live_session,
        duration_ms,
        error: Some(error),
        finished_at: chrono::Local::now().timestamp(),
    }
}

impl ProbeLiveSession {
    fn default_empty() -> Self {
        Self {
            registered: false,
            alive: false,
            child_pid: None,
            turns_served: None,
            registry_size: 0,
        }
    }
}

/// 读一次常驻会话注册表。**只读**：不改任何条目（`last_activity` 不动，否则自省本身就会
/// 把空闲回收的时钟拨回去），也**不跨 await 持锁**（本函数是同步的，state.rs 的既有约定）。
fn live_session_snapshot(state: &AppState, conversation_id: &str) -> ProbeLiveSession {
    let map = state
        .external_live_sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let registry_size = map.len();
    match map.get(conversation_id) {
        Some(session) => ProbeLiveSession {
            registered: true,
            alive: !session.control.is_closed(),
            child_pid: session.child_pid,
            turns_served: Some(session.turns_served),
            registry_size,
        },
        None => ProbeLiveSession {
            registry_size,
            ..ProbeLiveSession::default_empty()
        },
    }
}

/// 枚举的线上名字（`snake_case`，与前端事件同一套口径）。走 serde 而不是手写 match：
/// 手写的第二份映射迟早跟 `#[serde(rename_all)]` 分叉（spec 第 2 条）。
fn serde_variant_name<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn probe_usage(usage: &crate::chat::model::ModelUsage) -> ProbeUsage {
    ProbeUsage {
        input: usage.input_tokens,
        output: usage.output_tokens,
        total: usage.total_tokens,
        cached_input: usage.cached_input_tokens,
        cache_creation: usage.cache_creation_input_tokens,
        reasoning: usage.reasoning_tokens,
        context_window: usage.context_window_tokens,
    }
}

fn probe_context_state(state: &ConversationContextState) -> ProbeContextState {
    ProbeContextState {
        estimated_input_tokens: state.estimated_input_tokens,
        context_window_tokens: state.context_window_tokens,
        usage_ratio: state.usage_ratio,
        status: state.status.clone(),
        token_count_source: state.token_count_source.clone(),
        context_source: state.context_source.clone(),
        compression_count: state.compression_count,
        compaction_boundary_count: state.compaction_boundaries.len(),
    }
}

fn write_result(dir: &std::path::Path, result: &ProbeResult) {
    let json = match serde_json::to_string_pretty(result) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("[chat-probe] serialize result failed: {err}");
            return;
        }
    };
    let _ = std::fs::write(dir.join("result.json"), &json);
    if let Some(id) = &result.id {
        let _ = std::fs::write(dir.join(format!("result-{id}.json")), &json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::types::ChatMessageSegmentKind;

    #[test]
    fn probe_request_parses_camelcase_and_defaults() {
        let req: ProbeRequest =
            serde_json::from_str(r#"{"prompt":"hi","skillId":"pdf"}"#).expect("parse");
        assert_eq!(req.prompt, "hi");
        assert_eq!(req.skill_id.as_deref(), Some("pdf"));
        assert!(req.id.is_none() && req.provider.is_none() && req.model.is_none());
        // 新探点的缺省必须是「什么都不做」：取消不触发，上下文状态不额外计算。
        assert!(req.cancel_after_ms.is_none());
        assert!(!req.compute_context_stats);
        assert!(req.external_model.is_none() && req.external_sandbox.is_none());
    }

    #[test]
    fn probe_request_parses_the_new_probes() {
        let req: ProbeRequest = serde_json::from_str(
            r#"{"prompt":"hi","cancelAfterMs":1500,"computeContextStats":true,
                "externalAgentId":"claude","externalModel":"sonnet",
                "externalReasoning":"high","externalSandbox":"bypassPermissions"}"#,
        )
        .expect("parse");
        assert_eq!(req.cancel_after_ms, Some(1500));
        assert!(req.compute_context_stats);
        assert_eq!(req.external_agent_id.as_deref(), Some("claude"));
        assert_eq!(req.external_model.as_deref(), Some("sonnet"));
        assert_eq!(req.external_reasoning.as_deref(), Some("high"));
        assert_eq!(req.external_sandbox.as_deref(), Some("bypassPermissions"));
    }

    #[test]
    fn probe_result_serializes_expected_shape() {
        let result = ProbeResult {
            id: Some("t1".to_string()),
            conversation_id: None,
            answer: "ok".to_string(),
            tool_calls: vec![ProbeToolCall {
                name: "glob".to_string(),
                arguments: r#"{"pattern":"*.rs"}"#.to_string(),
                status: ToolCallStatus::Success,
            }],
            segments: Vec::new(),
            stream_outcome: Some("completed".to_string()),
            usage: None,
            context_state: None,
            live_usage_ticks: vec![ProbeLiveUsageTick {
                used_tokens: 47_300,
                context_window_tokens: Some(1_000_000),
            }],
            live_session: ProbeLiveSession::default_empty(),
            duration_ms: 1234,
            error: None,
            finished_at: 123,
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&result).unwrap()).unwrap();
        assert_eq!(v["id"], "t1");
        assert_eq!(v["answer"], "ok");
        assert_eq!(v["toolCalls"][0]["name"], "glob");
        assert_eq!(v["streamOutcome"], "completed");
        assert_eq!(v["durationMs"], 1234);
        // liveSession 恒存在（缺失与「没有常驻会话」是两种意思，脚本要能分开）。
        assert_eq!(v["liveSession"]["registered"], false);
        assert_eq!(v["liveSession"]["registrySize"], 0);
        // conversation_id/error/usage/contextState 为 None → 不序列化
        assert!(v.get("conversationId").is_none());
        assert!(v.get("error").is_none());
        assert!(v.get("usage").is_none());
        assert!(v.get("contextState").is_none());
        // 实时占用推送：脚本按 camelCase 读，且分母允许为 null（本次上报没带窗口）。
        assert_eq!(v["liveUsageTicks"][0]["usedTokens"], 47_300);
        assert_eq!(v["liveUsageTicks"][0]["contextWindowTokens"], 1_000_000);
    }

    #[test]
    fn usage_projection_keeps_every_field() {
        let usage = crate::chat::model::ModelUsage {
            input_tokens: Some(1),
            output_tokens: Some(2),
            total_tokens: Some(3),
            cached_input_tokens: Some(4),
            cache_creation_input_tokens: Some(5),
            reasoning_tokens: Some(6),
            context_window_tokens: Some(1_000_000),
        };
        let v = serde_json::to_value(probe_usage(&usage)).unwrap();
        assert_eq!(v["input"], 1);
        assert_eq!(v["output"], 2);
        assert_eq!(v["total"], 3);
        assert_eq!(v["cachedInput"], 4);
        assert_eq!(v["cacheCreation"], 5);
        assert_eq!(v["reasoning"], 6);
        assert_eq!(v["contextWindow"], 1_000_000);
    }

    /// `Some(0)` 与 `None` 必须能在 result 里区分开：spec 第 14h 条整条修复的判据就是
    /// 「零用量的轮次不许写成 `Some(0)`」，投影把两者压成同一个 JSON 就断言不了。
    #[test]
    fn usage_projection_distinguishes_zero_from_missing() {
        let zeroed = crate::chat::model::ModelUsage {
            input_tokens: Some(0),
            ..Default::default()
        };
        let v = serde_json::to_value(probe_usage(&zeroed)).unwrap();
        assert_eq!(v["input"], 0);
        assert!(v["output"].is_null());
    }

    /// 分段投影只出 kind 的线上名字（`snake_case`，与前端事件同一套口径）。
    /// 手写第二份枚举映射迟早和 `#[serde(rename_all)]` 分叉，所以走 serde。
    #[test]
    fn segment_projection_uses_the_wire_kind_name() {
        assert_eq!(
            serde_variant_name(&ChatMessageSegmentKind::Reasoning),
            "reasoning"
        );
        assert_eq!(serde_variant_name(&ChatMessageSegmentKind::Text), "text");
    }

    #[test]
    fn context_state_projection_counts_boundaries() {
        let mut state = ConversationContextState {
            estimated_input_tokens: 47_300,
            context_window_tokens: Some(1_000_000),
            usage_ratio: Some(0.0473),
            status: "normal".to_string(),
            token_count_source: Some("cli_reported".to_string()),
            context_source: Some("external_cli".to_string()),
            compression_count: 2,
            ..Default::default()
        };
        state
            .compaction_boundaries
            .push(crate::chat::types::CompactionBoundaryRecord {
                id: "ctxbd_1".to_string(),
                source_until_message_id: String::new(),
                display_after_message_id: Some("msg_1".to_string()),
                token_estimate_before: 100,
                token_estimate_after: 10,
                summary_content: String::new(),
                trigger: "auto".to_string(),
                created_at: 1,
            });
        let v = serde_json::to_value(probe_context_state(&state)).unwrap();
        assert_eq!(v["estimatedInputTokens"], 47_300);
        assert_eq!(v["contextWindowTokens"], 1_000_000);
        assert_eq!(v["tokenCountSource"], "cli_reported");
        assert_eq!(v["contextSource"], "external_cli");
        assert_eq!(v["compressionCount"], 2);
        assert_eq!(v["compactionBoundaryCount"], 1);
    }
}
