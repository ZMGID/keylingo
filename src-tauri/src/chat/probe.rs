//! 无头测试通道（probe）——仅 debug 构建。
//!
//! 自动化写 `<app_data>/chat_probe/request.json`
//! `{prompt, provider?, model?, skillId?, mode?, cwd?, webSearchMode?, conversationId?, externalAgentId?}`；
//! 运行中的 app 轮询到后，走**与聊天窗口完全相同的生成路径**
//! （`commands::run_chat_probe` → `complete_assistant_reply_inner(probe=true)` →
//! `run_agent_loop` + 全量工具集，ProbeAgentHost 自动放行；会话若绑外部 CLI 运行时则走
//! `run_external_cli_reply`），把结果写到
//! `<app_data>/chat_probe/result.json`（带 `id` 时另写 `result-<id>.json`）：
//! `{id?, conversationId, answer, toolCalls:[{name,arguments,status}], streamOutcome, error?, finishedAt}`。
//!
//! **多轮**：把 result 里的 `conversationId` 填回下一次请求，即续聊同一会话——跨轮记忆、
//! 外部 CLI 常驻进程复用、压缩边界这些只有多轮才能验的场景靠它。
//!
//! 用途：自动化 / CI 真实验证 GUI 客户端的工具调用（如工具改名后模型是否还能调对），
//! 免手测。整模块 `#[cfg(debug_assertions)]`，release 不含。

#![cfg(debug_assertions)]

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::chat::types::ToolCallStatus;
use crate::state::AppState;

/// 单次生成超时（无 GUI 应答，靠这个兜底避免 watcher 永久卡住）。
/// orchestrate 编排流程（多子代理并行多轮）常超 2 分钟，放宽到 6 分钟。
const PROBE_TIMEOUT: Duration = Duration::from_secs(360);
/// 轮询间隔——调试用途，延迟无所谓。
const POLL_INTERVAL: Duration = Duration::from_millis(700);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProbeRequest {
    #[serde(default)]
    id: Option<String>,
    prompt: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    skill_id: Option<String>,
    /// agent 运行模式（act/plan/orchestrate），省略 = act。用于验证模式提示词。
    #[serde(default)]
    mode: Option<String>,
    /// 文件工具的根目录（read/glob/grep 相对路径从此解析）。省略则用进程 cwd
    /// （dev 通常是仓库根），使文件工具开箱即用。
    #[serde(default)]
    cwd: Option<String>,
    /// 会话级联网搜索模式（off/builtin/third_party），省略 = 跟随全局。
    /// 用于无头验证内置搜索链路（任务 07-23）。
    #[serde(default)]
    web_search_mode: Option<String>,
    /// 续聊已有会话（用上一次 result 回传的 `conversationId`）。省略 = 新建会话。
    /// **多轮场景只能靠它**：跨轮记忆、外部 CLI 常驻会话复用、压缩边界，都要同一会话连发多轮。
    #[serde(default)]
    conversation_id: Option<String>,
    /// 把**新建**的会话钉到外部 CLI 运行时（`claude` / `codex` / …）。省略 = 跟随
    /// `settings.chat.defaultAgentRuntime`。续聊时忽略（有消息的外部会话禁切运行时）。
    #[serde(default)]
    external_agent_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProbeToolCall {
    name: String,
    /// 原样的入参（ToolCallRecord.arguments 本身就是 JSON 串）。
    arguments: String,
    status: ToolCallStatus,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_outcome: Option<String>,
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
                    &ProbeResult {
                        id: None,
                        conversation_id: None,
                        answer: String::new(),
                        tool_calls: Vec::new(),
                        stream_outcome: None,
                        error: Some(format!("invalid request.json: {err}")),
                        finished_at: 0,
                    },
                );
                continue;
            }
        };

        eprintln!("[chat-probe] running: {:?}", req.prompt);
        let result = handle_probe_request(&app, req).await;
        write_result(&dir, &result);
        eprintln!(
            "[chat-probe] done: {} tool call(s){}",
            result.tool_calls.len(),
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
    let cwd = req.cwd.or_else(|| {
        std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string())
    });
    let run = crate::chat::commands::run_chat_probe(
        app,
        &state,
        req.prompt,
        req.provider,
        req.model,
        req.skill_id,
        req.mode,
        cwd,
        req.web_search_mode,
        req.conversation_id,
        req.external_agent_id,
    );

    let finished_at = chrono::Local::now().timestamp();
    match tokio::time::timeout(PROBE_TIMEOUT, run).await {
        Ok(Ok((conversation_id, message))) => ProbeResult {
            id,
            // 回传会话 id：下一次请求带上它就续聊同一会话（多轮验证的入口）。
            conversation_id: Some(conversation_id),
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
            stream_outcome: message.stream_outcome.clone(),
            error: None,
            finished_at,
        },
        Ok(Err(err)) => ProbeResult {
            id,
            conversation_id: None,
            answer: String::new(),
            tool_calls: Vec::new(),
            stream_outcome: None,
            error: Some(err),
            finished_at,
        },
        Err(_) => ProbeResult {
            id,
            conversation_id: None,
            answer: String::new(),
            tool_calls: Vec::new(),
            stream_outcome: Some("timeout".to_string()),
            error: Some(format!(
                "probe generation timed out after {PROBE_TIMEOUT:?}"
            )),
            finished_at,
        },
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

    #[test]
    fn probe_request_parses_camelcase_and_defaults() {
        let req: ProbeRequest =
            serde_json::from_str(r#"{"prompt":"hi","skillId":"pdf"}"#).expect("parse");
        assert_eq!(req.prompt, "hi");
        assert_eq!(req.skill_id.as_deref(), Some("pdf"));
        assert!(req.id.is_none() && req.provider.is_none() && req.model.is_none());
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
            stream_outcome: Some("completed".to_string()),
            error: None,
            finished_at: 123,
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&result).unwrap()).unwrap();
        assert_eq!(v["id"], "t1");
        assert_eq!(v["answer"], "ok");
        assert_eq!(v["toolCalls"][0]["name"], "glob");
        assert_eq!(v["streamOutcome"], "completed");
        // conversation_id/error 为 None → 不序列化
        assert!(v.get("conversationId").is_none());
        assert!(v.get("error").is_none());
    }
}
