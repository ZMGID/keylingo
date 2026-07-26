//! 从 kimi CLI 自己的落盘日志里读真实 token 用量。
//!
//! **为什么需要这一层**：kimi 走 ACP，但其上游什么用量都不给——实测 `session/new` 无 token
//! 字段、`session/prompt` 结果只有 `{"stopReason":"end_turn"}`、也不发 ACP 官方的
//! `usage_update` 通知。没有这一层，kimi 会话的分子只能靠字符估算，实测把 23605 估成 ~24
//! （差三个数量级，主因是 `inputCacheRead` 占了 97.6%）。
//!
//! **数据源**：`<kimi_home>/sessions/<wd_hash>/<session_id>/agents/main/wire.jsonl`，每轮一条
//! ```json
//! {"type":"usage.record","model":"kimi-code/k3-256k",
//!  "usage":{"inputOther":565,"output":228,"inputCacheRead":23040,"inputCacheCreation":0},
//!  "usageScope":"turn","time":1784987956825}
//! ```
//! 真实 input = `inputOther + inputCacheRead + inputCacheCreation`。
//!
//! **关联方式：workDir，不是 session id。** kimi 的 session id 由 kimi 侧生成，Kivio 走 ACP
//! 时根本没存（实测 `external-agent-sessions/` 里 18 个 claude + 3 个 pi + **0 个 kimi**）。
//! 改用 `<kimi_home>/session_index.jsonl` 的 `workDir` 字段——它恰好等于 Kivio 的
//! `workspace::resolve_effective_cwd()`（`chat-workspaces/<conversation_id>`）。
//!
//! **必须跳过空壳会话**：Kivio 的斜杠命令探测每次 `session/new` 都会在 kimi 侧留下一个没有
//! 任何 turn 的会话（见 `.trellis/spec/guides/external-cli-agents.md` 第 11b 条的探测残渣）。
//! 实测某个 workDir 下 53 个 session 里 52 个是空壳。判据：wire.jsonl 里存在
//! `type == "usage.record"` 且 `usageScope == "turn"` 的记录。有效候选按 wire.jsonl mtime 取最新。
//!
//! **全程只读**，绝不写/删/改 kimi 的任何文件；任何失败（目录不存在、JSON 损坏、字段缺）
//! 一律静默返回 `None`，由调用方退回字符估算。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

/// 单个 wire.jsonl 的读取上限。实测最大 116KB，8MB 足够宽松；纯粹是防御一个异常大的日志
/// 把整个用量条卡住/吃满内存。
const MAX_WIRE_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// session_index.jsonl 的读取上限（每行约 300B，8MB ≈ 2.7 万条会话）。
const MAX_INDEX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// 从 kimi wire.jsonl 读到的一轮真实用量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KimiTurnUsage {
    /// `inputOther + inputCacheRead + inputCacheCreation`。
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// 缓存命中读取部分（已计入 `input_tokens`，单独留出供上层填 `cached_input_tokens`）。
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    /// 该轮实际使用的模型 id（形如 `kimi-code/k3-256k`），可用于窗口映射。
    pub model: Option<String>,
}

/// kimi 的数据根目录：`KIMI_CODE_HOME` 优先，回落 `~/.kimi-code`。
pub fn kimi_home() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("KIMI_CODE_HOME") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    directories::BaseDirs::new().map(|base| base.home_dir().join(".kimi-code"))
}

/// 读某个 Kivio 执行 cwd 对应的最近一轮 kimi 用量。找不到任何有效会话时 `None`。
pub fn latest_turn_usage(work_dir: &Path) -> Option<KimiTurnUsage> {
    latest_turn_usage_in(&kimi_home()?, work_dir)
}

/// `latest_turn_usage` 的可注入 home 版本（单测用临时目录造 index + wire）。
pub fn latest_turn_usage_in(home: &Path, work_dir: &Path) -> Option<KimiTurnUsage> {
    let mut candidates = wire_paths_for_work_dir(home, work_dir);
    // 新的在前：同一个 workDir 下会堆很多会话，只有最新那个有 turn 的才是当前对话。
    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    candidates
        .into_iter()
        .find_map(|(path, _)| last_turn_usage_in_wire(&path))
}

/// 扫 `session_index.jsonl`，返回 workDir 匹配的会话的 `(wire.jsonl 路径, mtime)`。
/// 只保留 wire.jsonl 真实存在的候选；空壳判定留给 `last_turn_usage_in_wire`（读到内容才知道）。
fn wire_paths_for_work_dir(home: &Path, work_dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let index = match read_capped(&home.join("session_index.jsonl"), MAX_INDEX_FILE_BYTES) {
        Some(text) => text,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for line in index.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // 单行坏 JSON 只跳过，不放弃整个索引（对齐流式 reader 的既有约定）。
        let entry: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let entry_work_dir = entry.get("workDir").and_then(|v| v.as_str()).unwrap_or("");
        if entry_work_dir.is_empty() || Path::new(entry_work_dir) != work_dir {
            continue;
        }
        let session_dir = match entry.get("sessionDir").and_then(|v| v.as_str()) {
            Some(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
            _ => continue,
        };
        let wire = session_dir.join("agents").join("main").join("wire.jsonl");
        let mtime = match std::fs::metadata(&wire).and_then(|meta| meta.modified()) {
            Ok(mtime) => mtime,
            Err(_) => continue,
        };
        out.push((wire, mtime));
    }
    out
}

/// 取一个 wire.jsonl 里最后一条 `type=="usage.record" && usageScope=="turn"` 的记录。
/// 没有任何这类记录 = 空壳会话（斜杠探测残渣），返回 `None` 让调用方看下一个候选。
fn last_turn_usage_in_wire(path: &Path) -> Option<KimiTurnUsage> {
    let text = read_capped(path, MAX_WIRE_FILE_BYTES)?;
    text.lines().rev().find_map(|line| {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let record: Value = serde_json::from_str(line).ok()?;
        if record.get("type").and_then(|v| v.as_str()) != Some("usage.record") {
            return None;
        }
        if record.get("usageScope").and_then(|v| v.as_str()) != Some("turn") {
            return None;
        }
        let usage = record.get("usage")?;
        let field = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
        let cache_read = field("inputCacheRead");
        let cache_creation = field("inputCacheCreation");
        let input = field("inputOther")
            .saturating_add(cache_read)
            .saturating_add(cache_creation);
        let output = field("output");
        if input == 0 && output == 0 {
            return None;
        }
        Some(KimiTurnUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_creation_tokens: cache_creation,
            model: record
                .get("model")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
        })
    })
}

/// 带大小上限的只读读取。文件不存在 / 超限 / 非 UTF-8 一律 `None`。
fn read_capped(path: &Path, max_bytes: u64) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > max_bytes {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TempHome(PathBuf);

    impl TempHome {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "kivio-kimi-usage-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        /// 造一个会话：写 index 行 + wire.jsonl 内容，返回 wire 路径。
        fn add_session(&self, session_id: &str, work_dir: &str, wire_lines: &str) -> PathBuf {
            let session_dir = self.0.join("sessions").join(session_id);
            let agent_dir = session_dir.join("agents").join("main");
            fs::create_dir_all(&agent_dir).unwrap();
            let wire = agent_dir.join("wire.jsonl");
            fs::write(&wire, wire_lines).unwrap();
            let mut index =
                fs::read_to_string(self.0.join("session_index.jsonl")).unwrap_or_default();
            index.push_str(&format!(
                "{}\n",
                serde_json::json!({
                    "sessionId": session_id,
                    "sessionDir": session_dir.to_string_lossy(),
                    "workDir": work_dir,
                })
            ));
            fs::write(self.0.join("session_index.jsonl"), index).unwrap();
            wire
        }
    }

    /// 显式设定 wire.jsonl 的 mtime，让「按 mtime 取最新」的分支可确定性地测。
    fn set_mtime(path: &Path, at: SystemTime) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 本机实测样本（`conv_2c0108ea.../agents/main/wire.jsonl` 末条，2026-07-26）。
    const REAL_RECORD: &str = r#"{"type":"usage.record","model":"kimi-code/k3-256k","usage":{"inputOther":565,"output":228,"inputCacheRead":23040,"inputCacheCreation":0},"usageScope":"turn","time":1784987956825}"#;

    #[test]
    fn reads_latest_turn_usage_including_cache() {
        let home = TempHome::new("basic");
        let work_dir = "/tmp/kivio-ws/conv-1";
        home.add_session("session-a", work_dir, &format!("{REAL_RECORD}\n"));

        let usage = latest_turn_usage_in(home.path(), Path::new(work_dir)).expect("usage");
        // 565 + 23040 + 0 —— cache 占 97.6%，漏掉就是漏一个数量级。
        assert_eq!(usage.input_tokens, 23_605);
        assert!(usage.input_tokens > 40 * 565, "cache 必须计入 input");
        assert_eq!(usage.output_tokens, 228);
        assert_eq!(usage.cache_read_tokens, 23_040);
        assert_eq!(usage.model.as_deref(), Some("kimi-code/k3-256k"));
    }

    #[test]
    fn takes_last_record_not_first() {
        let home = TempHome::new("last");
        let work_dir = "/tmp/kivio-ws/conv-2";
        let early = r#"{"type":"usage.record","model":"kimi-code/k3","usage":{"inputOther":10,"output":1,"inputCacheRead":0,"inputCacheCreation":0},"usageScope":"turn","time":1}"#;
        home.add_session("session-a", work_dir, &format!("{early}\n{REAL_RECORD}\n"));

        let usage = latest_turn_usage_in(home.path(), Path::new(work_dir)).expect("usage");
        assert_eq!(usage.input_tokens, 23_605);
    }

    #[test]
    fn skips_empty_shell_sessions_from_slash_probing() {
        let home = TempHome::new("shell");
        let work_dir = "/tmp/kivio-ws/conv-3";
        let real_wire = home.add_session("session-real", work_dir, &format!("{REAL_RECORD}\n"));
        // 斜杠探测残渣：有 wire.jsonl，但没有任何 usage.record。必须被跳过。
        let shell_wire = home.add_session(
            "session-shell",
            work_dir,
            "{\"type\":\"session.start\"}\n{\"type\":\"config\"}\n",
        );
        // 让空壳更"新"，确保跳过靠的是内容判据而不是碰巧的排序。
        set_mtime(
            &shell_wire,
            SystemTime::now() + std::time::Duration::from_secs(60),
        );
        set_mtime(
            &real_wire,
            SystemTime::now() - std::time::Duration::from_secs(60),
        );

        let usage = latest_turn_usage_in(home.path(), Path::new(work_dir)).expect("usage");
        assert_eq!(usage.input_tokens, 23_605);
    }

    #[test]
    fn newest_session_with_usage_wins() {
        let home = TempHome::new("newest");
        let work_dir = "/tmp/kivio-ws/conv-7";
        let old_record = r#"{"type":"usage.record","model":"kimi-code/k3","usage":{"inputOther":100,"output":2,"inputCacheRead":0,"inputCacheCreation":0},"usageScope":"turn","time":1}"#;
        let old_wire = home.add_session("session-old", work_dir, &format!("{old_record}\n"));
        let new_wire = home.add_session("session-new", work_dir, &format!("{REAL_RECORD}\n"));
        set_mtime(
            &old_wire,
            SystemTime::now() - std::time::Duration::from_secs(600),
        );
        set_mtime(&new_wire, SystemTime::now());

        let usage = latest_turn_usage_in(home.path(), Path::new(work_dir)).expect("usage");
        assert_eq!(usage.input_tokens, 23_605);
    }

    #[test]
    fn ignores_non_turn_scope_records() {
        let home = TempHome::new("scope");
        let work_dir = "/tmp/kivio-ws/conv-4";
        let session_scope = r#"{"type":"usage.record","model":"kimi-code/k3","usage":{"inputOther":999,"output":9,"inputCacheRead":0,"inputCacheCreation":0},"usageScope":"session","time":9}"#;
        home.add_session("session-a", work_dir, &format!("{session_scope}\n"));
        assert!(latest_turn_usage_in(home.path(), Path::new(work_dir)).is_none());
    }

    #[test]
    fn other_work_dirs_are_not_matched() {
        let home = TempHome::new("workdir");
        home.add_session(
            "session-a",
            "/tmp/kivio-ws/conv-other",
            &format!("{REAL_RECORD}\n"),
        );
        assert!(latest_turn_usage_in(home.path(), Path::new("/tmp/kivio-ws/conv-mine")).is_none());
    }

    #[test]
    fn missing_index_or_corrupt_json_yields_none_without_panic() {
        let home = TempHome::new("corrupt");
        // index 不存在。
        assert!(latest_turn_usage_in(home.path(), Path::new("/tmp/x")).is_none());

        let work_dir = "/tmp/kivio-ws/conv-5";
        home.add_session("session-a", work_dir, "not json at all\n{broken\n");
        // 坏 wire 行被跳过 → 该会话视为无用量。
        assert!(latest_turn_usage_in(home.path(), Path::new(work_dir)).is_none());

        // 坏 index 行不该拖垮后面的好行。
        let index_path = home.path().join("session_index.jsonl");
        let good = std::fs::read_to_string(&index_path).unwrap();
        std::fs::write(&index_path, format!("{{not json\n{good}")).unwrap();
        home.add_session("session-b", work_dir, &format!("{REAL_RECORD}\n"));
        let usage = latest_turn_usage_in(home.path(), Path::new(work_dir)).expect("usage");
        assert_eq!(usage.input_tokens, 23_605);
    }

    #[test]
    fn oversized_wire_file_is_refused() {
        let home = TempHome::new("cap");
        let work_dir = "/tmp/kivio-ws/conv-6";
        let wire = home.add_session("session-a", work_dir, &format!("{REAL_RECORD}\n"));
        assert!(read_capped(&wire, MAX_WIRE_FILE_BYTES).is_some());
        assert!(read_capped(&wire, 4).is_none());
    }

    #[test]
    fn kimi_home_prefers_env_override() {
        let original = std::env::var("KIMI_CODE_HOME").ok();
        std::env::set_var("KIMI_CODE_HOME", "/tmp/custom-kimi-home");
        assert_eq!(kimi_home(), Some(PathBuf::from("/tmp/custom-kimi-home")));
        match original {
            Some(value) => std::env::set_var("KIMI_CODE_HOME", value),
            None => std::env::remove_var("KIMI_CODE_HOME"),
        }
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Live read against the real `~/.kimi-code` on this machine.
    ///
    /// 单测用的是临时目录里手工造的 index + wire；这条跑真实 kimi 数据，证明
    /// workDir 关联、空壳会话跳过、cache 求和在真数据上都成立。
    /// 断言宽松（本机可能没跑过 kimi），但会打印全部命中供人工对照
    /// `research/cli-wire-facts.md` 里记录的样本（23605 / 67728）。
    #[test]
    #[ignore = "reads the real ~/.kimi-code on this machine"]
    fn live_reads_real_kimi_home() {
        let home = kimi_home().expect("kimi home");
        eprintln!("kimi home: {}", home.display());
        let index = home.join("session_index.jsonl");
        let raw = std::fs::read_to_string(&index).expect("session_index.jsonl");

        let mut work_dirs: Vec<String> = raw
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter_map(|v| {
                v.get("workDir")
                    .and_then(|w| w.as_str())
                    .map(str::to_string)
            })
            .collect();
        work_dirs.sort();
        work_dirs.dedup();
        eprintln!("distinct workDirs: {}", work_dirs.len());

        let mut hits = 0usize;
        for wd in &work_dirs {
            let started = std::time::Instant::now();
            let found = latest_turn_usage_in(&home, Path::new(wd));
            let elapsed = started.elapsed();
            match found {
                Some(u) => {
                    hits += 1;
                    eprintln!(
                        "  HIT  {:>7}ms input={} output={} :: {}",
                        elapsed.as_millis(),
                        u.input_tokens,
                        u.output_tokens,
                        wd
                    );
                    // cache 必须计入：真实 kimi 会话的 inputCacheRead 通常占绝大头。
                    assert!(
                        u.input_tokens > 0,
                        "input must be positive when a turn exists"
                    );
                }
                None => eprintln!("  miss {:>7}ms :: {}", elapsed.as_millis(), wd),
            }
        }
        eprintln!("resolved {hits}/{} workDirs", work_dirs.len());
    }
}
