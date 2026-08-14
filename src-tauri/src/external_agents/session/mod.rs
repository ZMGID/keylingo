use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use tauri::AppHandle;
use uuid::Uuid;

use tauri::Manager;

use crate::external_agents::types::ExternalAgentSession;

pub mod acp;
pub mod claude_init;
/// 常驻 `claude` 会话（B1）：一个会话一个进程。
pub mod claude_stream;
pub mod codex_app_server;
pub mod dsh_jsonrpc;
pub mod live;
pub mod pi_rpc;

fn sessions_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir unavailable: {e}"))?;
    let dir = base.join("external-agent-sessions");
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| format!("create sessions dir: {e}"))?;
    }
    Ok(dir)
}

fn session_path(app: &AppHandle, conversation_id: &str) -> Result<PathBuf, String> {
    Ok(sessions_dir(app)?.join(format!("{conversation_id}.json")))
}

pub fn load_session(app: &AppHandle, conversation_id: &str) -> Option<ExternalAgentSession> {
    let path = session_path(app, conversation_id).ok()?;
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn save_session(app: &AppHandle, session: &ExternalAgentSession) -> Result<(), String> {
    let path = session_path(app, &session.conversation_id)?;
    let raw = serde_json::to_string_pretty(session).map_err(|e| e.to_string())?;
    fs::write(path, raw).map_err(|e| e.to_string())
}

pub fn stable_prompt_hash(instructions: &str) -> String {
    let mut hasher = DefaultHasher::new();
    instructions.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub struct AgentResumeContext {
    pub resume_session_id: Option<String>,
    pub new_session_id: Option<String>,
    pub is_resuming: bool,
    pub stored_stable_prompt_hash: Option<String>,
    pub skip_instructions: bool,
    /// Effective model at delivery time, normalized (empty / "default" → `None`). Persisted
    /// alongside the session so the stored record reflects what the CLI was last asked to use.
    pub delivered_model: Option<String>,
}

/// Normalize a model selection to what actually gets passed to the CLI: blank or the sentinel
/// `"default"` mean "no explicit `--model`", i.e. `None`.
fn normalize_model(model: Option<&str>) -> Option<String> {
    model
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "default")
        .map(str::to_string)
}

pub fn resolve_agent_resume_context(
    app: &AppHandle,
    conversation_id: &str,
    agent_id: &str,
    resumes_via_cli: bool,
    instructions: &str,
    current_model: Option<&str>,
) -> AgentResumeContext {
    let delivered_model = normalize_model(current_model);
    if !resumes_via_cli {
        return AgentResumeContext {
            resume_session_id: None,
            new_session_id: None,
            is_resuming: false,
            stored_stable_prompt_hash: None,
            skip_instructions: false,
            delivered_model,
        };
    }

    let hash = stable_prompt_hash(instructions);
    if let Some(stored) = load_session(app, conversation_id).filter(|s| s.agent_id == agent_id) {
        // 换模型**照常 resume**，不开新会话。
        //
        // 这里曾经有一段「模型变了就丢弃会话、生成新 session id」的逻辑，依据是一句没有实测
        // 支撑的注释（「CLI 在 `--resume` 上会忽略 `--model`，会话钉死在旧模型」）。
        // 2026-07-29 本机实测（claude 2.1.220）推翻了它：`--session-id X --model sonnet` 记住
        // 一个数字后，`--resume X --model opus` 起来的会话**既报自己是 Opus、又记得那个数字**
        // ——切换生效，上下文也没丢。丢弃会话是纯粹的损失（用户换个模型就丢掉整段对话）。
        //
        // 进程侧由 `claude_stream` 的启动参数指纹接管：模型变了 ⇒ 指纹不匹配 ⇒ 换进程重连，
        // 但带 `--resume <同一个 id>`，上下文照样续上。
        let skip = stored
            .stable_prompt_hash
            .as_ref()
            .is_some_and(|h| h == &hash);
        return AgentResumeContext {
            resume_session_id: Some(stored.session_id.clone()),
            new_session_id: None,
            is_resuming: true,
            stored_stable_prompt_hash: stored.stable_prompt_hash.clone(),
            skip_instructions: skip,
            delivered_model,
        };
    }

    AgentResumeContext {
        resume_session_id: None,
        new_session_id: Some(Uuid::new_v4().to_string()),
        is_resuming: false,
        stored_stable_prompt_hash: None,
        skip_instructions: false,
        delivered_model,
    }
}

/// 把落盘的原生会话 id 换成一个新的（其余字段原样保留）。
///
/// **为什么必须做这一步**：claude 的 `--resume <id>` 是从这条落盘记录读出来的
/// （`resolve_agent_resume_context` → `build_claude_args`），**不是**从 live handle 读的。
/// 会话记录在 claude 那边被清掉之后（`No conversation found with session ID`），只改本轮的
/// argv 是不够的 —— 下一轮又会拿那个死 id 去 `--resume`，于是**每一轮**都要降级一次、
/// 每一轮都丢一次上下文并弹一次「上下文已重置」。
///
/// 记录不存在（或属于别的 agent）就什么也不做：那种情况下 argv 里本来也不会有 `--resume`。
pub fn replace_stored_session_id(
    app: &AppHandle,
    conversation_id: &str,
    agent_id: &str,
    session_id: &str,
) {
    let Some(mut stored) = load_session(app, conversation_id).filter(|s| s.agent_id == agent_id)
    else {
        return;
    };
    stored.session_id = session_id.to_string();
    let _ = save_session(app, &stored);
}

pub fn persist_delivered_session(
    app: &AppHandle,
    conversation_id: &str,
    agent_id: &str,
    resume_ctx: &AgentResumeContext,
    instructions: &str,
    is_slash: bool,
) -> Result<(), String> {
    // Slash turns pass the raw slash text as `instructions` (no daemon prompt, no memory), so
    // rewriting the stored hash from them would poison the next non-slash turn's diff check.
    // Same reasoning for `delivered_model`: on a resume-under-slash we intentionally kept it as
    // the stored model, so there's nothing to write.
    if is_slash {
        return Ok(());
    }
    if !resume_ctx.is_resuming {
        if let Some(session_id) = resume_ctx.new_session_id.as_ref() {
            save_session(
                app,
                &ExternalAgentSession {
                    conversation_id: conversation_id.to_string(),
                    agent_id: agent_id.to_string(),
                    session_id: session_id.clone(),
                    stable_prompt_hash: Some(stable_prompt_hash(instructions)),
                    model: resume_ctx.delivered_model.clone(),
                },
            )?;
        }
    } else if let Some(mut stored) = load_session(app, conversation_id) {
        // 记录随「系统提示变了」或「模型换了」任一变化而更新。以前只看提示哈希——换模型走的是
        // 另一条（丢弃会话）分支，所以看不看模型无所谓；现在换模型也 resume，只看哈希会让存下
        // 的模型一直停在第一次那个值上。
        let next_hash = stable_prompt_hash(instructions);
        let hash_changed = stored.stable_prompt_hash.as_deref() != Some(next_hash.as_str());
        let model_changed = stored.model != resume_ctx.delivered_model;
        if hash_changed || model_changed {
            stored.stable_prompt_hash = Some(next_hash);
            stored.model = resume_ctx.delivered_model.clone();
            save_session(app, &stored)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{normalize_model, stable_prompt_hash};

    #[test]
    fn stable_prompt_hash_is_deterministic() {
        assert_eq!(stable_prompt_hash("a"), stable_prompt_hash("a"));
        assert_ne!(stable_prompt_hash("a"), stable_prompt_hash("b"));
    }

    #[test]
    fn normalize_model_treats_blank_and_default_as_none() {
        assert_eq!(normalize_model(None), None);
        assert_eq!(normalize_model(Some("")), None);
        assert_eq!(normalize_model(Some("   ")), None);
        assert_eq!(normalize_model(Some("default")), None);
        assert_eq!(normalize_model(Some("  opus  ")), Some("opus".to_string()));
        assert_eq!(
            normalize_model(Some("provider/model-x")),
            Some("provider/model-x".to_string())
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Phase 2: persisted handle for a live rich-protocol session, so a conversation can RESUME its
// native thread/session after an app restart. Stored separately from ExternalAgentSession (which
// drives claude's CLI `--resume`) to avoid clobbering it.
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LiveSessionHandle {
    pub agent_id: String,
    /// `"codex_app_server"` | `"acp_json_rpc"` | `"claude_stream_json"`.
    pub protocol: String,
    /// Native thread id (codex) / session id (ACP / claude)。
    pub native_id: String,
    pub cwd: String,
}

fn live_handle_path(app: &AppHandle, conversation_id: &str) -> Result<PathBuf, String> {
    Ok(sessions_dir(app)?.join(format!("live-{conversation_id}.json")))
}

pub fn load_live_handle(app: &AppHandle, conversation_id: &str) -> Option<LiveSessionHandle> {
    let raw = fs::read_to_string(live_handle_path(app, conversation_id).ok()?).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn save_live_handle(
    app: &AppHandle,
    conversation_id: &str,
    handle: &LiveSessionHandle,
) -> Result<(), String> {
    let path = live_handle_path(app, conversation_id)?;
    let raw = serde_json::to_string_pretty(handle).map_err(|e| e.to_string())?;
    fs::write(path, raw).map_err(|e| e.to_string())
}

pub fn clear_live_handle(app: &AppHandle, conversation_id: &str) {
    if let Ok(path) = live_handle_path(app, conversation_id) {
        let _ = fs::remove_file(path);
    }
}

/// 删除一条对话时，清掉它的所有会话绑定记录（`conv_*.json` / `live-*.json` / `imported-*.json`）。
///
/// **不清会留下幽灵绑定**：那条原生会话会在导入列表里永远显示"已导入"、永远不能再导，
/// 点进去还跳到一条已经不存在的对话。
///
/// **不碰 CLI 自己的 transcript**——`~/.claude/projects/` 之类是 CLI 的数据，Kivio 从不写它，
/// 删 Kivio 对话不该连带毁掉用户在终端里还能 resume 的会话。
///
/// 尽力而为：单个文件删不掉只记警告，不让删除对话整体失败。
pub fn remove_all_bindings(app: &AppHandle, conversation_id: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut targets = Vec::new();
    if let Ok(path) = session_path(app, conversation_id) {
        targets.push(path);
    }
    if let Ok(path) = live_handle_path(app, conversation_id) {
        targets.push(path);
    }
    if let Ok(base) = sessions_dir(app) {
        targets.push(base.join(format!("imported-{conversation_id}.json")));
    }
    for path in targets {
        if !path.exists() {
            continue;
        }
        if let Err(e) = fs::remove_file(&path) {
            warnings.push(format!("会话绑定未能清理（{}）：{e}", path.display()));
        }
    }
    warnings
}
