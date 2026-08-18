//! 从本地 CLI 导入对话——可导入会话的枚举。
//!
//! 决策依据见 `docs/adr/0001..0003`。三条硬约束：
//!
//! 1. 只列出**工作目录等于给定项目根**的会话（ADR-0001）。路径比对必须 realpath-aware。
//! 2. 走 ACP 的代理（opencode / kimi）由协议 `session/list` 枚举，不在这里读它们的私有存储。
//! 3. 已导入的会话要标出来但仍然返回——前端负责置灰，不在后端悄悄过滤掉。

use std::path::{Path, PathBuf};

use serde::Serialize;

/// 一条可导入的原生会话。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImportableSession {
    /// `RuntimeAgentDef.id`：`claude` / `codex` / `grok` / `kimi` / `opencode`。
    pub agent_id: String,
    /// 该 CLI 自己的会话 id——`session/load` / `--resume` 认的就是这个。
    pub session_id: String,
    pub title: Option<String>,
    /// 会话创建时所在的工作目录（原样，未规范化，供 UI 展示）。
    pub cwd: String,
    /// 最后活动时间，epoch 毫秒。
    pub updated_at: Option<i64>,
    /// 消息条数。`None` = 该来源给不出（ACP `session/list` 不返回条数）——
    /// 界面要显示"未知"而不是"0 条"。
    pub message_count: Option<usize>,
    /// 由本模块导入进来的。
    pub already_imported: bool,
    /// 已经有 Kivio 对话绑着这条原生会话时，那条对话的 id。
    ///
    /// **不等于"已导入"**：Kivio 自己创建的外部 CLI 对话运行时也会写绑定。两种都不能再导入
    /// （绑定是 1:1 的），但界面上要说不同的话，并且都能点进去。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bound_conversation_id: Option<String>,
}

/// 把路径归一成可比对的键：解符号链接 / junction，统一分隔符，Windows 上忽略大小写。
///
/// 直接比字符串会在 junction、`8.3` 短名、大小写差异上失配——同一个目录被判成两个，
/// 于是"项目内导入"什么都列不出来。`canonicalize` 失败（目录已不存在）时退回原路径，
/// 让调用方仍能做纯字符串比对，而不是直接把这条会话丢掉。
pub fn canonical_key(path: &str) -> String {
    let raw = Path::new(path);
    let resolved = std::fs::canonicalize(raw).unwrap_or_else(|_| raw.to_path_buf());
    let text = resolved.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    let text = text.replace('\\', "/");
    let trimmed = text.trim_end_matches('/');
    // 盘符根（`C:`）trim 后为空是正常的，保留原串避免退化成空键。
    let key = if trimmed.is_empty() {
        text.as_str()
    } else {
        trimmed
    };
    if cfg!(windows) {
        key.to_lowercase()
    } else {
        key.to_string()
    }
}

/// 两个路径是否指向同一个目录。
pub fn paths_match(a: &str, b: &str) -> bool {
    !a.trim().is_empty() && !b.trim().is_empty() && canonical_key(a) == canonical_key(b)
}

fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|base| base.home_dir().to_path_buf())
}

fn file_mtime_ms(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let since = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    i64::try_from(since).ok()
}

// ---------------------------------------------------------------------------------------------
// claude
// ---------------------------------------------------------------------------------------------

/// claude 的会话目录：`~/.claude/projects/<编码后的 cwd>/<session-uuid>.jsonl`。
///
/// **不复刻那个目录编码规则。** 它把每个非字母数字字符换成 `-`、超过 200 字符还要截断加哈希，
/// 是有损且未文档化的；而 jsonl 的每一条记录都明文带着 `cwd`。所以扫目录、读首行拿 cwd，
/// 比按项目根正向算目录名稳得多，也免疫 claude 改编码。
///
/// ponytail: 全量扫描所有会话文件的首行（本机 66 目录 / 328 文件，读首行 4KB × 328 可忽略）。
/// 会话数上千再优化成"按编码目录名直接定位、失败才回退扫描"。
fn claude_projects_root() -> Option<PathBuf> {
    Some(home_dir()?.join(".claude").join("projects"))
}

/// 读 jsonl 首部若干行，取出 `(session_id, cwd)`。
///
/// 首行未必是带 `cwd` 的记录（实测开头可能是 `queue-operation`），所以要往下多看几行；
/// 但也不能读整个文件——单条会话实测可达 10MB。
fn claude_session_identity(path: &Path) -> Option<(String, String)> {
    use std::io::{BufRead, BufReader};

    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    for line in reader.lines().take(50) {
        let line = line.ok()?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let cwd = value
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let session_id = value
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if !cwd.is_empty() && !session_id.is_empty() {
            return Some((session_id.to_string(), cwd.to_string()));
        }
    }
    None
}

/// 统计消息条数并取标题。只对 cwd 已匹配的文件调用——这里会读完整个文件。
fn claude_session_detail(path: &Path) -> (usize, Option<String>) {
    use std::io::{BufRead, BufReader};

    let Ok(file) = std::fs::File::open(path) else {
        return (0, None);
    };
    let mut count = 0usize;
    let mut ai_title: Option<String> = None;
    let mut first_user: Option<String> = None;

    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        // 子 agent 分支不计入条数，也不当标题来源（ADR-0002：sidechain 一律丢弃）。
        if value.get("isSidechain").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        match value.get("type").and_then(|v| v.as_str()) {
            Some("user") | Some("assistant") => {
                count += 1;
                if first_user.is_none()
                    && value.get("type").and_then(|v| v.as_str()) == Some("user")
                {
                    first_user = claude_message_text(&value);
                }
            }
            Some("ai-title") => {
                // 字段名是 `aiTitle`（不是 `title`/`content`——猜错会静默退回"第一句用户消息"）。
                // 一条会话里有上百条 ai-title，标题随对话演进被重写，**取最后一条**才是当前标题。
                if let Some(text) = value
                    .get("aiTitle")
                    .or_else(|| value.get("title"))
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    ai_title = Some(text.to_string());
                }
            }
            _ => {}
        }
    }

    let title = ai_title.or(first_user).map(|text| truncate_title(&text));
    (count, title)
}

/// 从一条 claude 记录里取纯文本：`message.content` 可能是字符串，也可能是块数组。
fn claude_message_text(value: &serde_json::Value) -> Option<String> {
    let content = value.get("message")?.get("content")?;
    if let Some(text) = content.as_str() {
        let trimmed = text.trim();
        return (!trimmed.is_empty()).then(|| trimmed.to_string());
    }
    let blocks = content.as_array()?;
    for block in blocks {
        if block.get("type").and_then(|v| v.as_str()) == Some("text") {
            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

fn truncate_title(text: &str) -> String {
    const MAX_CHARS: usize = 60;
    let mut out: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
    let trimmed = out.trim().to_string();
    out = trimmed;
    if out.chars().count() > MAX_CHARS {
        out = out.chars().take(MAX_CHARS).collect::<String>() + "…";
    }
    out
}

/// 列出工作目录等于 `project_root` 的 claude 会话。
///
/// 目录不存在、单个文件损坏都不算错误——返回能读出来的部分，绝不因为一个坏文件让整个导入列表空掉。
pub fn list_claude_sessions(project_root: &str) -> Vec<ImportableSession> {
    let Some(root) = claude_projects_root() else {
        return Vec::new();
    };
    let Ok(project_dirs) = std::fs::read_dir(&root) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for project_dir in project_dirs.flatten() {
        if !project_dir.path().is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(project_dir.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some((session_id, cwd)) = claude_session_identity(&path) else {
                continue;
            };
            if !paths_match(&cwd, project_root) {
                continue;
            }
            let (message_count, title) = claude_session_detail(&path);
            if message_count == 0 {
                continue; // 空会话没有导入价值。
            }
            out.push(ImportableSession {
                agent_id: "claude".to_string(),
                session_id,
                title,
                cwd,
                updated_at: file_mtime_ms(&path),
                message_count: Some(message_count),
                already_imported: false,
                bound_conversation_id: None,
            });
        }
    }

    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
}

// ---------------------------------------------------------------------------------------------
// grok
// ---------------------------------------------------------------------------------------------

/// grok 的会话目录：`~/.grok/sessions/<百分号编码的 cwd>/<uuid>/`，其中 `summary.json` 是现成的
/// 元数据索引（`info.id` / `info.cwd` / `updated_at` / `num_chat_messages` / `session_summary`）。
///
/// 枚举**只读 summary.json**，完全不碰 `chat_history.jsonl`——条数和标题它都给了。
/// 目录名虽然是无损的百分号编码、可以反解，但 `info.cwd` 是明文，读它更直接也更稳。
fn grok_sessions_root() -> Option<PathBuf> {
    Some(home_dir()?.join(".grok").join("sessions"))
}

pub(crate) fn parse_rfc3339_ms(text: &str) -> Option<i64> {
    // 只认 `YYYY-MM-DDTHH:MM:SS` 前缀，秒以下和时区后缀忽略——列表排序不需要那个精度。
    let bytes = text.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let num = |a: usize, b: usize| text.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, s) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    // 民用历 → Unix 天数（Howard Hinnant days_from_civil）。
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = if yy >= 0 { yy } else { yy - 399 } / 400;
    let yoe = yy - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400) + h * 3_600 + mi * 60 + s) * 1_000)
}

/// 列出工作目录等于 `project_root` 的 grok 会话。
pub fn list_grok_sessions(project_root: &str) -> Vec<ImportableSession> {
    let Some(root) = grok_sessions_root() else {
        return Vec::new();
    };
    let Ok(cwd_dirs) = std::fs::read_dir(&root) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for cwd_dir in cwd_dirs.flatten() {
        if !cwd_dir.path().is_dir() {
            continue;
        }
        let Ok(session_dirs) = std::fs::read_dir(cwd_dir.path()) else {
            continue;
        };
        for session_dir in session_dirs.flatten() {
            let summary_path = session_dir.path().join("summary.json");
            let Ok(raw) = std::fs::read_to_string(&summary_path) else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
                continue;
            };
            let Some(session) = grok_session_from_summary(&value, project_root) else {
                continue;
            };
            let updated_at = value
                .get("updated_at")
                .and_then(|v| v.as_str())
                .and_then(parse_rfc3339_ms)
                .or_else(|| file_mtime_ms(&summary_path));
            out.push(ImportableSession {
                updated_at,
                ..session
            });
        }
    }

    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
}

/// 纯函数部分，方便单测：从一份 `summary.json` 得到会话（cwd 不匹配或空会话返回 `None`）。
fn grok_session_from_summary(
    value: &serde_json::Value,
    project_root: &str,
) -> Option<ImportableSession> {
    let info = value.get("info")?;
    let session_id = info.get("id")?.as_str()?.to_string();
    let cwd = info.get("cwd")?.as_str()?.to_string();
    if !paths_match(&cwd, project_root) {
        return None;
    }
    // `num_messages` 实测常年为 0，真正有意义的是 `num_chat_messages`。
    let message_count = value
        .get("num_chat_messages")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    if message_count == 0 {
        return None;
    }
    let title = value
        .get("session_summary")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(truncate_title);
    Some(ImportableSession {
        agent_id: "grok".to_string(),
        session_id,
        title,
        cwd,
        updated_at: None,
        message_count: Some(message_count),
        already_imported: false,
        bound_conversation_id: None,
    })
}

// ---------------------------------------------------------------------------------------------
// codex
// ---------------------------------------------------------------------------------------------

/// codex 的会话：`~/.codex/sessions/YYYY/MM/DD/rollout-<时间>-<uuid>.jsonl`。
///
/// 按**日期**分目录，不按 cwd——所以必须逐个文件读首行的 `session_meta.payload.cwd` 来过滤。
///
/// 注意 `turn_context` 里也有 `cwd`，且**可能与 session_meta 不同**（同一条会话可以在别的目录
/// 里被 resume；本机实测就有这种记录）。归属判定一律用 `session_meta` 的 cwd——那才是
/// 「会话被创建时所在的目录」。
fn codex_sessions_root() -> Option<PathBuf> {
    Some(home_dir()?.join(".codex").join("sessions"))
}

fn codex_session_meta(path: &Path) -> Option<(String, String)> {
    use std::io::{BufRead, BufReader};

    let file = std::fs::File::open(path).ok()?;
    for line in BufReader::new(file).lines().take(5) {
        let line = line.ok()?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("session_meta") {
            continue;
        }
        let payload = value.get("payload")?;
        let id = payload.get("id")?.as_str()?.to_string();
        let cwd = payload.get("cwd")?.as_str()?.to_string();
        return Some((id, cwd));
    }
    None
}

/// 统计条数并取标题。只对 cwd 已匹配的文件调用。
fn codex_session_detail(path: &Path) -> (usize, Option<String>) {
    use std::io::{BufRead, BufReader};

    let Ok(file) = std::fs::File::open(path) else {
        return (0, None);
    };
    let mut count = 0usize;
    let mut first_user: Option<String> = None;

    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let payload = value.get("payload");
        let outer = value.get("type").and_then(|v| v.as_str());
        let inner = payload.and_then(|p| p.get("type")).and_then(|v| v.as_str());

        match (outer, inner) {
            (Some("event_msg"), Some("user_message")) => {
                count += 1;
                if first_user.is_none() {
                    first_user = payload
                        .and_then(|p| p.get("message"))
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string);
                }
            }
            // `developer` 角色是注入的权限说明等系统文本，不是对话内容。
            (Some("response_item"), Some("message")) => {
                let role = payload
                    .and_then(|p| p.get("role"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if role == "assistant" {
                    count += 1;
                }
            }
            _ => {}
        }
    }

    (count, first_user.map(|t| truncate_title(&t)))
}

/// 列出工作目录等于 `project_root` 的 codex 会话。
pub fn list_codex_sessions(project_root: &str) -> Vec<ImportableSession> {
    let Some(root) = codex_sessions_root() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // 目录层级固定是 年/月/日，直接三层遍历，不引第三方递归遍历。
    for level1 in read_subdirs(&root) {
        for level2 in read_subdirs(&level1) {
            for level3 in read_subdirs(&level2) {
                collect_codex_in_dir(&level3, project_root, &mut out);
            }
        }
    }
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
}

fn read_subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

fn collect_codex_in_dir(dir: &Path, project_root: &str, out: &mut Vec<ImportableSession>) {
    let Ok(files) = std::fs::read_dir(dir) else {
        return;
    };
    for file in files.flatten() {
        let path = file.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some((session_id, cwd)) = codex_session_meta(&path) else {
            continue;
        };
        if !paths_match(&cwd, project_root) {
            continue;
        }
        let (message_count, title) = codex_session_detail(&path);
        if message_count == 0 {
            continue;
        }
        out.push(ImportableSession {
            agent_id: "codex".to_string(),
            session_id,
            title,
            cwd,
            updated_at: file_mtime_ms(&path),
            message_count: Some(message_count),
            already_imported: false,
            bound_conversation_id: None,
        });
    }
}

/// 三个读文件的 CLI 合起来枚举。走 ACP 的 opencode / kimi 不在这里。
pub fn list_file_based_sessions(project_root: &str) -> Vec<ImportableSession> {
    let mut out = list_claude_sessions(project_root);
    out.extend(list_grok_sessions(project_root));
    out.extend(list_codex_sessions(project_root));
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
}

// ---------------------------------------------------------------------------------------------
// ACP（opencode / kimi）
// ---------------------------------------------------------------------------------------------

/// 走 ACP 协议枚举的代理（ADR-0003）。
///
/// gemini 和 hermes **不在**这里：gemini 的 ACP 没有 `session/list` 且 `loadSession=false`
/// （导进来续不了聊），hermes 不记工作目录。cursor 本机未安装、能力未验证，
/// 但它和 opencode 走同一套 ACP，探针返回 `None` 时会自然表现为"不支持"，不必特判。
pub const ACP_IMPORT_AGENTS: &[&str] = &["opencode", "kimi", "cursor"];

/// 用 ACP `session/list` 枚举某个代理在 `project_root` 下的会话。
///
/// `None` = 该代理不支持导入（未声明 `loadSession`，或 `session/list` 不存在）；
/// `Some(vec![])` = 支持但这个目录下没有会话。两者在界面上要显示成不同的话。
pub async fn list_acp_sessions(
    agent_id: &str,
    project_root: &str,
) -> Option<Vec<ImportableSession>> {
    use crate::external_agents::registry::get_agent_def;
    use crate::external_agents::session::acp::probe_acp_sessions;
    use crate::external_agents::spawn::resolve_binary;
    use crate::external_agents::types::{RuntimeBuildOptions, RuntimeContext};

    let def = get_agent_def(agent_id)?;
    let resolved = resolve_binary(def).await?;

    // ACP 的启动参数是常量（`acp` / `--experimental-acp`），与会话上下文无关，给个空壳即可。
    let ctx = RuntimeContext {
        extra_allowed_dirs: vec![],
        resume_session_id: None,
        new_session_id: None,
        include_partial_messages: false,
    };
    let opts = RuntimeBuildOptions {
        model: None,
        reasoning: None,
        sandbox: None,
    };
    let args = (def.build_args)(&ctx, &opts, None);
    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();

    let summaries = probe_acp_sessions(&resolved, &args_ref, Path::new(project_root), 25).await?;

    Some(
        summaries
            .into_iter()
            // agent 已按 cwd 过滤过，这里再兜一道：万一它忽略了 cwd 参数返回全局会话，
            // 不能让别的项目的会话漏进这个项目的导入列表。
            .filter(|s| paths_match(&s.cwd, project_root))
            .map(|s| ImportableSession {
                agent_id: agent_id.to_string(),
                session_id: s.session_id,
                title: s.title.as_deref().map(truncate_title),
                cwd: s.cwd,
                updated_at: None,
                message_count: None,
                already_imported: false,
                bound_conversation_id: None,
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------------------------
// 导入
// ---------------------------------------------------------------------------------------------

/// 导入留下的指纹，用于**过期检测**（ADR-0002）。
///
/// 存在 `external-agent-sessions/imported-<conv_id>.json`，与会话绑定文件放在一起。
/// 打开对话时拿 `source_path` 现在的 mtime / 条数与这里记的比，对不上就提示"CLI 那边有新内容"。
/// **只提示，不同步。**
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRecord {
    pub conversation_id: String,
    pub agent_id: String,
    pub session_id: String,
    /// 历史来源文件；走 ACP 重放的（opencode）没有文件，为 `None`。
    #[serde(default)]
    pub source_path: Option<String>,
    #[serde(default)]
    pub source_mtime_ms: Option<i64>,
    #[serde(default)]
    pub message_count: usize,
    pub imported_at: i64,
}

fn import_record_path(app: &tauri::AppHandle, conversation_id: &str) -> Result<PathBuf, String> {
    use tauri::Manager;
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir unavailable: {e}"))?
        .join("external-agent-sessions");
    std::fs::create_dir_all(&base).map_err(|e| format!("create sessions dir: {e}"))?;
    Ok(base.join(format!("imported-{conversation_id}.json")))
}

pub fn save_import_record(app: &tauri::AppHandle, record: &ImportRecord) -> Result<(), String> {
    let path = import_record_path(app, &record.conversation_id)?;
    let raw = serde_json::to_string_pretty(record).map_err(|e| e.to_string())?;
    std::fs::write(path, raw).map_err(|e| format!("write import record: {e}"))
}

pub fn load_import_record(app: &tauri::AppHandle, conversation_id: &str) -> Option<ImportRecord> {
    let raw = std::fs::read_to_string(import_record_path(app, conversation_id).ok()?).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 已经有 Kivio 对话绑着的 `(agent_id, session_id)` → 绑定信息。
///
/// **注意这里有两种来源，含义不同**：
/// - Kivio 自己创建的外部 CLI 对话，运行时就写了 `conv_*.json` / `live-*.json`；
/// - 本模块导入进来的，额外写了一份 `imported-*.json`。
///
/// 两种都不能再导入（会话绑定是 1:1 的，两条 Kivio 对话绑同一条原生会话会让双方快照都残缺），
/// 但**界面上要说不同的话**——把 Kivio 自己跑出来的会话标成"已导入"是在撒谎，用户根本没导过。
///
/// 直接扫绑定文件而不是另维护索引：绑定文件才是真相源，多一份索引就多一个会不一致的地方。
#[derive(Debug, Clone, PartialEq)]
pub struct BoundConversation {
    pub conversation_id: String,
    /// `true` = 由本模块导入；`false` = Kivio 自己创建该对话时产生的绑定。
    pub imported: bool,
}

pub fn bound_sessions(
    app: &tauri::AppHandle,
) -> std::collections::HashMap<(String, String), BoundConversation> {
    use tauri::Manager;
    let Ok(base) = app.path().app_data_dir() else {
        return std::collections::HashMap::new();
    };
    let Ok(entries) = std::fs::read_dir(base.join("external-agent-sessions")) else {
        return std::collections::HashMap::new();
    };

    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        files.push((name, value));
    }
    let mut bound = classify_binding_files(&files);

    // 自愈：对话已经不存在的绑定一律丢掉。
    //
    // 删除对话现在会清绑定（`session::remove_all_bindings`），但**存量**里还有历史遗留的
    // 幽灵绑定。留着的后果是那条原生会话永远显示"已导入"、永远导不进来，点进去还跳到一条
    // 不存在的对话——挡住一个本来可用的功能，比多扫一遍文件糟糕得多。
    bound.retain(|_, hit| {
        crate::chat::storage::conversation_file_path(app, &hit.conversation_id)
            .map(|path| path.exists())
            .unwrap_or(false)
    });
    bound
}

/// `bound_sessions` 的纯函数内核：把 `(文件名, JSON)` 分类成绑定表。
///
/// 抽出来是因为**这里出过一次真 bug**：早期版本把所有绑定文件一律当成"已导入"，于是
/// Kivio 自己跑出来的会话在导入列表里被标成"已导入"，而用户从没导过它。
fn classify_binding_files(
    files: &[(String, serde_json::Value)],
) -> std::collections::HashMap<(String, String), BoundConversation> {
    let mut imported_conversations = std::collections::HashSet::new();
    let mut records = Vec::new();

    for (name, value) in files {
        if name.starts_with("imported-") {
            if let Some(id) = value.get("conversationId").and_then(|v| v.as_str()) {
                imported_conversations.insert(id.to_string());
            }
            continue;
        }
        // 三种文件形态字段名不同：`conv_*.json` 是 camelCase，`live-*.json` 是 snake_case。
        let agent = value
            .get("agentId")
            .or_else(|| value.get("agent_id"))
            .and_then(|v| v.as_str());
        let session = value
            .get("sessionId")
            .or_else(|| value.get("session_id"))
            .or_else(|| value.get("native_id"))
            .and_then(|v| v.as_str());
        let conversation = value
            .get("conversationId")
            .or_else(|| value.get("conversation_id"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                // `live-<conv_id>.json` 不带 conversationId，从文件名取。
                name.strip_prefix("live-")
                    .and_then(|rest| rest.strip_suffix(".json"))
                    .map(str::to_string)
            });
        if let (Some(agent), Some(session), Some(conversation)) = (agent, session, conversation) {
            records.push((agent.to_string(), session.to_string(), conversation));
        }
    }

    records
        .into_iter()
        .map(|(agent, session, conversation)| {
            let imported = imported_conversations.contains(&conversation);
            (
                (agent, session),
                BoundConversation {
                    conversation_id: conversation,
                    imported,
                },
            )
        })
        .collect()
}

/// 定位读文件那几个 CLI 的历史来源。
fn history_source_path(agent_id: &str, session_id: &str, project_root: &str) -> Option<PathBuf> {
    match agent_id {
        "claude" => {
            let root = claude_projects_root()?;
            for project_dir in std::fs::read_dir(&root).ok()?.flatten() {
                let candidate = project_dir.path().join(format!("{session_id}.jsonl"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
            None
        }
        "grok" => {
            let root = grok_sessions_root()?;
            for cwd_dir in std::fs::read_dir(&root).ok()?.flatten() {
                let candidate = cwd_dir.path().join(session_id).join("chat_history.jsonl");
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
            None
        }
        "codex" => {
            // 文件名形如 `rollout-<时间>-<uuid>.jsonl`，按 uuid 后缀匹配。
            let root = codex_sessions_root()?;
            for l1 in read_subdirs(&root) {
                for l2 in read_subdirs(&l1) {
                    for l3 in read_subdirs(&l2) {
                        for file in std::fs::read_dir(&l3).ok()?.flatten() {
                            let path = file.path();
                            let name = path.file_name()?.to_string_lossy().to_string();
                            if name.ends_with(&format!("{session_id}.jsonl")) {
                                return Some(path);
                            }
                        }
                    }
                }
            }
            None
        }
        _ => {
            let _ = project_root;
            None
        }
    }
}

/// 用 ACP `session/load` 重放并转成消息（目前只有 opencode 真的重放）。
async fn load_acp_history(
    agent_id: &str,
    project_root: &str,
    session_id: &str,
) -> Option<Vec<crate::external_agents::import_history::ImportedMessage>> {
    use crate::external_agents::registry::get_agent_def;
    use crate::external_agents::session::acp::probe_acp_session_history;
    use crate::external_agents::spawn::resolve_binary;
    use crate::external_agents::types::{RuntimeBuildOptions, RuntimeContext};

    let def = get_agent_def(agent_id)?;
    let resolved = resolve_binary(def).await?;
    let ctx = RuntimeContext {
        extra_allowed_dirs: vec![],
        resume_session_id: None,
        new_session_id: None,
        include_partial_messages: false,
    };
    let opts = RuntimeBuildOptions {
        model: None,
        reasoning: None,
        sandbox: None,
    };
    let args = (def.build_args)(&ctx, &opts, None);
    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
    let updates = probe_acp_session_history(
        &resolved,
        &args_ref,
        Path::new(project_root),
        session_id,
        60,
    )
    .await?;
    Some(crate::external_agents::import_history::parse_acp_updates(
        &updates,
    ))
}

/// `LiveSessionHandle.protocol` 用的协议串。
fn protocol_label(format: crate::external_agents::types::StreamFormat) -> &'static str {
    use crate::external_agents::types::StreamFormat;
    match format {
        StreamFormat::ClaudeStreamJson => "claude_stream_json",
        StreamFormat::CodexAppServer => "codex_app_server",
        StreamFormat::AcpJsonRpc => "acp_json_rpc",
        // pi 走自己的 RPC，且没有可导入的本地历史（design.md），这里不该被走到。
        _ => "unknown",
    }
}

/// 列出某个项目下所有可导入的会话（读文件的 + 走 ACP 的），并标出已导入的。
pub async fn list_importable_for_project(
    app: &tauri::AppHandle,
    project_id: &str,
) -> Result<Vec<ImportableSession>, String> {
    let project = crate::chat::storage::find_project_by_id(app, project_id)?;
    let root = project
        .root_path
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| "该项目没有绑定本地目录，无法按工作目录匹配会话".to_string())?;
    if !Path::new(&root).is_dir() {
        return Err(format!("项目目录不存在：{root}"));
    }

    let mut sessions = list_file_based_sessions(&root);
    for agent in ACP_IMPORT_AGENTS {
        if let Some(list) = list_acp_sessions(agent, &root).await {
            sessions.extend(list);
        }
    }

    let bound = bound_sessions(app);
    for session in sessions.iter_mut() {
        if let Some(hit) = bound.get(&(session.agent_id.clone(), session.session_id.clone())) {
            session.already_imported = hit.imported;
            session.bound_conversation_id = Some(hit.conversation_id.clone());
        }
    }
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(sessions)
}

/// 导入一条会话，返回新建的对话 id。
pub async fn import_one_session(
    app: &tauri::AppHandle,
    project_id: &str,
    agent_id: &str,
    session_id: &str,
) -> Result<String, String> {
    use crate::chat::types::{
        AgentPlanState, AgentRuntimeConfig, AgentRuntimeKind, Attachment, Conversation,
        ConversationContextState,
    };
    use crate::external_agents::import_history::{
        parse_claude_history, parse_codex_history, parse_grok_history,
    };
    use crate::external_agents::registry::get_agent_def;

    let def = get_agent_def(agent_id).ok_or_else(|| format!("未知的 CLI：{agent_id}"))?;
    let project = crate::chat::storage::find_project_by_id(app, project_id)?;
    let root = project
        .root_path
        .clone()
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| "该项目没有绑定本地目录".to_string())?;

    // 历史来源：读文件的三个各自解析；opencode 走 ACP 重放；kimi / 其它没有可读历史。
    let source = history_source_path(agent_id, session_id, &root);
    let (messages, source_mtime) = match (agent_id, source.as_ref()) {
        ("claude", Some(path)) => {
            let raw = std::fs::read_to_string(path).map_err(|e| format!("读取会话失败：{e}"))?;
            (parse_claude_history(&raw), file_mtime_ms(path))
        }
        ("grok", Some(path)) => {
            let raw = std::fs::read_to_string(path).map_err(|e| format!("读取会话失败：{e}"))?;
            (parse_grok_history(&raw), file_mtime_ms(path))
        }
        ("codex", Some(path)) => {
            let raw = std::fs::read_to_string(path).map_err(|e| format!("读取会话失败：{e}"))?;
            (parse_codex_history(&raw), file_mtime_ms(path))
        }
        _ => (
            load_acp_history(agent_id, &root, session_id)
                .await
                .unwrap_or_default(),
            None,
        ),
    };

    // 与对话的 created_at/updated_at 一致：**秒**。
    let now = chrono::Local::now().timestamp();
    let conversation_id = format!("conv_{}", uuid::Uuid::new_v4());

    // 图片落盘：解析器刻意不写盘，到这里才知道对话 id。写失败只丢这张图，不让整条导入失败。
    let mut chat_messages = Vec::with_capacity(messages.len());
    let mut image_seq = 0usize;
    for item in messages {
        let mut message = item.message;
        for image in item.images {
            image_seq += 1;
            let ext = match image.media_type.as_str() {
                "image/jpeg" | "image/jpg" => "jpg",
                "image/webp" => "webp",
                "image/gif" => "gif",
                _ => "png",
            };
            let name = format!("imported-{image_seq}.{ext}");
            match write_imported_image(app, &conversation_id, &name, &image.data_base64) {
                Ok(()) => message.attachments.push(Attachment {
                    id: uuid::Uuid::new_v4().to_string(),
                    attachment_type: "image".to_string(),
                    name: name.clone(),
                    path: name,
                    content: None,
                }),
                Err(err) => eprintln!("导入图片失败（已跳过）：{err}"),
            }
        }
        chat_messages.push(message);
    }

    let title = source_title(agent_id, source.as_deref())
        .or_else(|| {
            chat_messages
                .iter()
                .find(|m| m.role == "user" && !m.content.trim().is_empty())
                .map(|m| truncate_title(&m.content))
        })
        .unwrap_or_else(|| format!("{} 导入的会话", def.name));

    let conversation = Conversation {
        id: conversation_id.clone(),
        revision: 0,
        title,
        // 外部 CLI 对话不走 Kivio provider（ADR-0001），这两个字段留空。
        provider_id: String::new(),
        model: String::new(),
        messages: chat_messages,
        active_skill_id: None,
        assistant_id: None,
        assistant_snapshot: None,
        created_at: now,
        updated_at: now,
        pinned: false,
        archived: false,
        folder: None,
        project_id: Some(project_id.to_string()),
        set_id: None,
        context_state: ConversationContextState::default(),
        agent_todo_state: Default::default(),
        agent_plan_state: AgentPlanState::default(),
        knowledge_base_ids: Vec::new(),
        force_knowledge_search: false,
        thinking_level: None,
        web_search_mode: None,
        reply_models: Vec::new(),
        group_selections: std::collections::HashMap::new(),
        forked_from: None,
        agent_runtime: AgentRuntimeConfig {
            kind: AgentRuntimeKind::External,
            external_agent_id: Some(agent_id.to_string()),
            external_model: None,
            external_reasoning: None,
            external_sandbox: None,
            external_agent_preset: None,
        },
    };
    let message_count = conversation.messages.len();
    crate::chat::repository::repository(app)
        .create(app, conversation)
        .await
        .map_err(crate::chat::repository::repository_error)?;

    // 会话绑定：写哪种文件由 `resumes_session_via_cli` 决定（claude 走 CLI 的 `--resume`，
    // 其余走 live handle）。写错了续聊就会开一条全新会话，历史静默丢失。
    if def.resumes_session_via_cli {
        crate::external_agents::session::save_session(
            app,
            &crate::external_agents::types::ExternalAgentSession {
                conversation_id: conversation_id.clone(),
                agent_id: agent_id.to_string(),
                session_id: session_id.to_string(),
                // 留空：首轮会把 instructions 重发一遍（`skip_instructions=false`），
                // 但**不会**丢弃会话。填一个假的反而会让首轮误以为可以跳过。
                stable_prompt_hash: None,
                model: None,
            },
        )?;
    } else {
        crate::external_agents::session::save_live_handle(
            app,
            &conversation_id,
            &crate::external_agents::session::LiveSessionHandle {
                agent_id: agent_id.to_string(),
                protocol: protocol_label(def.stream_format).to_string(),
                native_id: session_id.to_string(),
                native_path: None,
                cwd: root.clone(),
            },
        )?;
    }

    save_import_record(
        app,
        &ImportRecord {
            conversation_id: conversation_id.clone(),
            agent_id: agent_id.to_string(),
            session_id: session_id.to_string(),
            source_path: source.map(|p| p.to_string_lossy().to_string()),
            source_mtime_ms: source_mtime,
            message_count,
            imported_at: now,
        },
    )?;

    Ok(conversation_id)
}

/// CLI 自己生成的会话标题。拿不到就返回 `None`，由调用方退回"第一句用户消息"。
///
/// 这才是用户在 CLI 里看到的那个标题；退回第一句用户消息只是兜底，不是期望结果。
fn source_title(agent_id: &str, source: Option<&Path>) -> Option<String> {
    let path = source?;
    match agent_id {
        "claude" => claude_session_detail(path).1,
        "grok" => {
            // transcript 是 `chat_history.jsonl`，标题在同目录的 `summary.json` 里。
            let summary = path.parent()?.join("summary.json");
            let raw = std::fs::read_to_string(summary).ok()?;
            let value = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
            value
                .get("session_summary")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(truncate_title)
        }
        // codex 的 rollout 里没有标题字段，只能退回第一句用户消息。
        _ => None,
    }
}

/// 一条**外部 CLI 对话**（不论是导入的还是 Kivio 自己创建的）在 CLI 那边的标题。
///
/// **为什么需要**：`resolve_conversation_title` 是拿 `conversation.provider_id`/`model` 去调模型
/// 生成标题的，而外部 CLI 对话这两个字段是空的（ADR-0001：不归属任何 Kivio provider），
/// 于是必然落到 `generate_title(第一句用户消息)` 兜底——用户看到的标题永远是自己问的第一句话。
/// CLI 自己已经生成了标题，读它既准确又不花一次模型调用。
///
/// 拿不到返回 `None`（codex 的 rollout 没有标题字段；opencode / kimi 的标题在各自的库里，
/// 取它要么起进程要么读 SQLite，不值得为一个标题付这个代价）。
pub fn cli_session_title(app: &tauri::AppHandle, conversation_id: &str) -> Option<String> {
    let (agent_id, session_id) = binding_for_conversation(app, conversation_id)?;
    let root = crate::chat::storage::load_conversation(app, conversation_id)
        .ok()
        .and_then(|c| c.project_id)
        .and_then(|pid| crate::chat::storage::find_project_by_id(app, &pid).ok())
        .and_then(|p| p.root_path)
        .unwrap_or_default();
    let source = history_source_path(&agent_id, &session_id, &root)?;
    source_title(&agent_id, Some(source.as_path()))
}

/// 从绑定文件里读出这条对话绑着哪个 `(agent_id, session_id)`。
fn binding_for_conversation(
    app: &tauri::AppHandle,
    conversation_id: &str,
) -> Option<(String, String)> {
    bound_sessions(app)
        .into_iter()
        .find(|(_, hit)| hit.conversation_id == conversation_id)
        .map(|((agent, session), _)| (agent, session))
}

fn write_imported_image(
    app: &tauri::AppHandle,
    conversation_id: &str,
    name: &str,
    data_base64: &str,
) -> Result<(), String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_base64.trim())
        .map_err(|e| format!("base64 解码失败：{e}"))?;
    if bytes.is_empty() {
        return Err("图片数据为空".to_string());
    }
    let dir = crate::chat::storage::conversation_attachments_dir(app, conversation_id)?;
    std::fs::write(dir.join(name), bytes).map_err(|e| format!("写入图片失败：{e}"))
}

/// 打开一条已导入的对话时检查历史是否过期（ADR-0002）。
///
/// 只比较**来源文件**的 mtime 与条数。走 ACP 重放的（opencode）没有文件可比，一律返回
/// `false`——宁可不提示，也不要靠一次昂贵的 ACP 握手去猜，更不要误报。
pub fn imported_history_is_stale(app: &tauri::AppHandle, conversation_id: &str) -> bool {
    let Some(record) = load_import_record(app, conversation_id) else {
        return false;
    };
    let Some(source) = record.source_path.as_deref() else {
        return false;
    };
    let path = Path::new(source);
    if !path.is_file() {
        return false; // 源文件被删了不算"有新内容"。
    }
    match (file_mtime_ms(path), record.source_mtime_ms) {
        (Some(now), Some(then)) => now > then,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_key_normalizes_separators_and_trailing_slash() {
        assert_eq!(
            canonical_key(r"C:\Users\demo\proj"),
            canonical_key("C:/Users/demo/proj/")
        );
    }

    #[test]
    fn canonical_key_is_case_insensitive_on_windows_only() {
        let lower = canonical_key("C:/Users/demo/Proj");
        let upper = canonical_key("C:/USERS/DEMO/PROJ");
        if cfg!(windows) {
            assert_eq!(lower, upper);
        } else {
            assert_ne!(lower, upper);
        }
    }

    #[test]
    fn canonical_key_never_returns_empty_for_drive_root() {
        assert!(!canonical_key("C:/").is_empty());
    }

    #[test]
    fn paths_match_rejects_blank_input() {
        // 空项目根不该匹配上任何会话——否则会把全机器的会话都列出来。
        assert!(!paths_match("", "C:/Users/demo"));
        assert!(!paths_match("C:/Users/demo", "   "));
    }

    #[test]
    fn paths_match_rejects_different_dirs() {
        assert!(!paths_match("C:/Users/demo/a", "C:/Users/demo/b"));
    }

    #[test]
    fn claude_identity_skips_leading_non_cwd_entries() {
        let dir = std::env::temp_dir().join(format!("kivio-import-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        // 实测开头确实可能是 queue-operation 这类不带 cwd 的记录。
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"queue-operation","operation":"enqueue"}"#,
                "\n",
                r#"{"type":"user","sessionId":"abc-123","cwd":"C:\\Users\\demo\\proj","message":{"content":"hello"}}"#,
                "\n"
            ),
        )
        .unwrap();
        let identity = claude_session_identity(&path).unwrap();
        assert_eq!(identity.0, "abc-123");
        assert_eq!(identity.1, r"C:\Users\demo\proj");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claude_detail_counts_messages_and_skips_sidechain() {
        let dir = std::env::temp_dir().join(format!("kivio-import-test2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"user","message":{"content":"第一句话"}}"#,
                "\n",
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"回答"}]}}"#,
                "\n",
                r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"子 agent"}]}}"#,
                "\n",
                r#"{"type":"file-history-snapshot"}"#,
                "\n"
            ),
        )
        .unwrap();
        let (count, title) = claude_session_detail(&path);
        assert_eq!(count, 2, "sidechain 与内部账务条目不计入");
        assert_eq!(title.as_deref(), Some("第一句话"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncate_title_caps_by_chars_not_bytes() {
        let long = "中".repeat(80);
        let title = truncate_title(&long);
        assert_eq!(title.chars().count(), 61, "60 字 + 省略号");
    }

    /// 拿本机真实的 `~/.claude/projects` 跑一遍。默认不跑（依赖本机数据）。
    /// `cargo test --lib external_agents::import -- --ignored --nocapture KIVIO_IMPORT_ROOT=<项目根>`
    #[test]
    #[ignore]
    fn smoke_list_claude_sessions_against_real_home() {
        let root = std::env::var("KIVIO_IMPORT_ROOT")
            .unwrap_or_else(|_| std::env::current_dir().unwrap().display().to_string());
        let sessions = list_claude_sessions(&root);
        println!("项目根 {root} 命中 {} 条 claude 会话", sessions.len());
        for s in sessions.iter().take(5) {
            println!(
                "  {}  {} 条  {:?}  cwd={}",
                s.session_id,
                s.message_count.unwrap_or(0),
                s.title,
                s.cwd
            );
        }
        assert!(
            sessions.iter().all(|s| paths_match(&s.cwd, &root)),
            "枚举结果里出现了工作目录不匹配的会话"
        );
    }

    #[test]
    fn kivio_own_session_is_bound_but_not_imported() {
        // 这是上线后第一个真 bug：Kivio 自己驱动 claude 跑出来的会话，在导入列表里被标成
        // "已导入"——用户从没导过它。绑定文件存在 ≠ 导入过。
        let files = vec![(
            "conv_ad015aa2.json".to_string(),
            serde_json::json!({
                "conversationId": "conv_ad015aa2",
                "agentId": "claude",
                "sessionId": "1433c5e7",
                "stablePromptHash": "39d96b29",
            }),
        )];
        let bound = classify_binding_files(&files);
        let hit = bound
            .get(&("claude".to_string(), "1433c5e7".to_string()))
            .expect("应该识别为已绑定");
        assert_eq!(hit.conversation_id, "conv_ad015aa2");
        assert!(!hit.imported, "Kivio 自己创建的会话不是导入来的");
    }

    #[test]
    fn imported_marker_flips_the_flag() {
        let files = vec![
            (
                "conv_x.json".to_string(),
                serde_json::json!({
                    "conversationId": "conv_x",
                    "agentId": "claude",
                    "sessionId": "sess-1",
                }),
            ),
            (
                "imported-conv_x.json".to_string(),
                serde_json::json!({
                    "conversationId": "conv_x",
                    "agentId": "claude",
                    "sessionId": "sess-1",
                }),
            ),
        ];
        let bound = classify_binding_files(&files);
        assert!(
            bound
                .get(&("claude".to_string(), "sess-1".to_string()))
                .expect("已绑定")
                .imported
        );
    }

    #[test]
    fn live_handle_takes_conversation_id_from_filename() {
        // `live-*.json` 是 snake_case 且**不带** conversationId，只能从文件名取；
        // 取不到的话这条绑定会被整条丢掉，于是重复导入的防护失效。
        let files = vec![(
            "live-conv_9f.json".to_string(),
            serde_json::json!({
                "agent_id": "opencode",
                "protocol": "acp_json_rpc",
                "native_id": "ses_abc",
                "cwd": "C:/p",
            }),
        )];
        let bound = classify_binding_files(&files);
        let hit = bound
            .get(&("opencode".to_string(), "ses_abc".to_string()))
            .expect("live handle 也算绑定");
        assert_eq!(hit.conversation_id, "conv_9f");
        assert!(!hit.imported);
    }

    #[test]
    fn parse_rfc3339_ms_matches_known_epochs() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_ms("2026-07-30T15:31:07.007792600Z"),
            Some(1785425467000)
        );
        // 闰年 2 月末，跨月边界。
        assert_eq!(
            parse_rfc3339_ms("2024-02-29T00:00:00Z"),
            Some(1709164800000)
        );
        assert_eq!(parse_rfc3339_ms("短"), None);
    }

    #[test]
    fn grok_summary_uses_chat_count_not_num_messages() {
        // 实测 `num_messages` 常年为 0，只有 `num_chat_messages` 有意义；用错字段会把所有会话判成空。
        let value: serde_json::Value = serde_json::from_str(
            r#"{"info":{"id":"019f","cwd":"C:/Users/demo/proj"},
                "num_messages":0,"num_chat_messages":2,"session_summary":"聊了点什么"}"#,
        )
        .unwrap();
        let session = grok_session_from_summary(&value, "C:/Users/demo/proj").unwrap();
        assert_eq!(session.message_count, Some(2));
        assert_eq!(session.title.as_deref(), Some("聊了点什么"));
        assert_eq!(session.agent_id, "grok");
    }

    #[test]
    fn grok_summary_rejects_other_project_and_empty_session() {
        let other: serde_json::Value = serde_json::from_str(
            r#"{"info":{"id":"a","cwd":"C:/Users/demo/other"},"num_chat_messages":5}"#,
        )
        .unwrap();
        assert!(grok_session_from_summary(&other, "C:/Users/demo/proj").is_none());

        let empty: serde_json::Value = serde_json::from_str(
            r#"{"info":{"id":"a","cwd":"C:/Users/demo/proj"},"num_chat_messages":0}"#,
        )
        .unwrap();
        assert!(grok_session_from_summary(&empty, "C:/Users/demo/proj").is_none());
    }

    #[test]
    fn codex_detail_counts_user_and_assistant_but_not_developer() {
        let dir = std::env::temp_dir().join(format!("kivio-import-codex-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rollout.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"session_meta","payload":{"id":"019c","cwd":"C:\\Users\\demo\\proj"}}"#, "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<permissions ...>"}]}}"#, "\n",
                r#"{"type":"event_msg","payload":{"type":"user_message","message":"帮我看下这个"}}"#, "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"好"}]}}"#, "\n",
                r#"{"type":"event_msg","payload":{"type":"token_count"}}"#, "\n"
            ),
        )
        .unwrap();

        let (id, cwd) = codex_session_meta(&path).unwrap();
        assert_eq!(id, "019c");
        assert_eq!(cwd, r"C:\Users\demo\proj");

        let (count, title) = codex_session_detail(&path);
        assert_eq!(count, 2, "developer 注入文本与 token_count 不算对话");
        assert_eq!(title.as_deref(), Some("帮我看下这个"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 三个读文件的 CLI 一起跑真实数据。
    #[test]
    #[ignore]
    fn smoke_list_file_based_sessions_against_real_home() {
        let root = std::env::var("KIVIO_IMPORT_ROOT")
            .unwrap_or_else(|_| std::env::current_dir().unwrap().display().to_string());
        let sessions = list_file_based_sessions(&root);
        let mut by_agent = std::collections::BTreeMap::new();
        for s in &sessions {
            *by_agent.entry(s.agent_id.clone()).or_insert(0usize) += 1;
        }
        println!(
            "项目根 {root} 共 {} 条，按 CLI：{by_agent:?}",
            sessions.len()
        );
        for s in sessions.iter().take(8) {
            println!(
                "  [{}] {} {} 条 {:?}",
                s.agent_id,
                s.session_id,
                s.message_count.unwrap_or(0),
                s.title
            );
        }
        assert!(sessions.iter().all(|s| paths_match(&s.cwd, &root)));
    }

    /// ACP 枚举跑真实数据。默认不跑（要起 CLI 进程）。
    /// `KIVIO_IMPORT_ROOT=<项目根> cargo test --lib external_agents::import::tests::smoke_acp -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn smoke_list_acp_sessions_against_real_clis() {
        let root = std::env::var("KIVIO_IMPORT_ROOT")
            .unwrap_or_else(|_| std::env::current_dir().unwrap().display().to_string());
        for agent in ACP_IMPORT_AGENTS {
            match list_acp_sessions(agent, &root).await {
                None => println!("[{agent}] 不支持导入（未声明 loadSession 或无 session/list）"),
                Some(list) => {
                    println!("[{agent}] {} 条", list.len());
                    for s in list.iter().take(3) {
                        println!("    {}  {:?}  cwd={}", s.session_id, s.title, s.cwd);
                    }
                    assert!(list.iter().all(|s| paths_match(&s.cwd, &root)));
                    assert!(
                        list.iter().all(|s| s.message_count.is_none()),
                        "ACP 不返回条数，必须留 None 而不是 0"
                    );
                }
            }
        }
    }
}
