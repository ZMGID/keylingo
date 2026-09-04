//! 中断草稿的 append-only 日志(对齐 Claude Code / Codex 的会话 JSONL 模式)。
//!
//! 工具轮之间的 crash-safety 草稿原来走「整会话 load → 重写 → fsync → 全局索引重写」,
//! 多对话工具密集并发时是最大的磁盘热点(长对话数 MB × 每轮)。现在每轮只把草稿消息
//! **追加**成一行 JSONL(`conversations/{id}.draft.jsonl`),主会话文件与索引在 run 期间
//! 完全不动:
//!
//! - **追加**(`append_draft`):每行是一份自包含的草稿 `ChatMessage`,后写覆盖先写。
//!   不 fsync——追加进页缓存后进程崩溃也不丢,只有掉电才丢,和成熟 CLI 的取舍一致。
//! - **清除**(`clear_draft`):终态写入主文件(`push_assistant_message` 的 mutate 成功)后
//!   日志立即失效,尽力删;删不掉也无害,恢复端对「主文件已有同 id 消息」会跳过。
//! - **恢复**(`recover_orphan_drafts`):启动时扫残留日志,按**每个 message_id 的最后一行**
//!   合并回会话文件(一次日志可能叠着多条 run 的草稿——panic 后同会话又跑了新 run)。
//!   跑在 setup 的 spawn 里,此时不可能有活跃 run,无并发写冲突。
//!
//! 扩展名刻意是 `.jsonl` 而非 `.json`:`storage::conversation_file_ids_in_dir` 的目录
//! 扫描按扩展名过滤,不会把日志当成会话文件。

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use tauri::AppHandle;

use super::storage;
use super::types::ChatMessage;

/// 超长 run 的日志按轮数²增长(每行都是全量累积草稿)。超过上限就地重置为单行——
/// 恢复语义只看每个 id 的最后一行,截断当前 run 不丢有效信息(极罕见的代价:同一日志里
/// 更早 run 的 panic 残留草稿一并丢弃)。
const DRAFT_JOURNAL_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// 追加一行草稿。同步阻塞 IO,调用方负责放进 `spawn_blocking`。
pub(crate) fn append_draft(
    app: &AppHandle,
    conversation_id: &str,
    mut draft: ChatMessage,
) -> Result<(), String> {
    let path = storage::draft_journal_path(app, conversation_id)?;
    // 图片外置与主文件写入(storage::write_conversation_file)同一套谓词/实现:
    // 草稿的两份转录里可能有模型看过的整图 base64,不外置的话每轮追加都整份复制。
    if super::attachments::message_has_inline_image_to_externalize(&draft)
        || super::attachments::message_has_model_message_image_to_externalize(&draft)
        || super::attachments::message_has_api_message_image_to_externalize(&draft)
    {
        super::attachments::externalize_message_artifacts(app, conversation_id, &mut draft);
    }
    let mut line =
        serde_json::to_string(&draft).map_err(|e| format!("serialize draft message: {e}"))?;
    line.push('\n');
    let oversized = fs::metadata(&path)
        .map(|meta| meta.len() > DRAFT_JOURNAL_MAX_BYTES)
        .unwrap_or(false);
    if oversized {
        fs::write(&path, line.as_bytes()).map_err(|e| format!("reset draft journal: {e}"))
    } else {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("open draft journal: {e}"))?;
        file.write_all(line.as_bytes())
            .map_err(|e| format!("append draft journal: {e}"))
    }
}

/// 终态已落主文件,草稿日志随即失效。删除失败不致命(下次启动恢复会识别出
/// 「主文件已有同 id 消息」并清理)。
pub(crate) fn clear_draft(app: &AppHandle, conversation_id: &str) {
    if let Ok(path) = storage::draft_journal_path(app, conversation_id) {
        let _ = fs::remove_file(path);
    }
}

/// 启动恢复:把崩溃残留的草稿日志合并回会话文件,然后删除日志。
pub(crate) async fn recover_orphan_drafts(app: &AppHandle) {
    let Ok(dir) = storage::conversations_dir(app) else {
        return;
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(conversation_id) = name.strip_suffix(".draft.jsonl") else {
            continue;
        };
        recover_one(app, conversation_id, &path).await;
    }
}

async fn recover_one(app: &AppHandle, conversation_id: &str, path: &Path) {
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    let drafts = latest_drafts_per_message(&content);
    let Ok(conversation) = storage::load_conversation(app, conversation_id) else {
        // 会话已删,日志是孤儿。
        let _ = fs::remove_file(path);
        return;
    };
    for draft in drafts {
        // 主文件已有同 id 消息 = 终态写入已发生(部分落盘不再碰主文件),草稿是过期残留。
        if conversation.messages.iter().any(|m| m.id == draft.id) {
            continue;
        }
        let draft_id = draft.id.clone();
        if let Err(error) = crate::chat::repository::repository(app)
            .upsert_message(app, conversation_id, draft)
            .await
        {
            // 保留日志,下次启动再试。
            eprintln!("recover draft {draft_id} for {conversation_id} failed: {error}");
            return;
        }
        eprintln!("Recovered interrupted draft {draft_id} for {conversation_id}");
    }
    let _ = fs::remove_file(path);
}

/// 读当前会话的草稿日志（每个 message_id 最后一行）。日志不存在或读失败时返回空。
/// 给同轮工具（例如 mixer 改图）查找主 JSON 里还没有的 artifact。
pub(crate) fn latest_drafts_for(app: &AppHandle, conversation_id: &str) -> Vec<ChatMessage> {
    let Ok(path) = storage::draft_journal_path(app, conversation_id) else {
        return Vec::new();
    };
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    latest_drafts_per_message(&content)
}

/// 解析日志:每个 message_id 取最后一行(自包含快照,后写覆盖先写),
/// 保持首次出现的顺序;坏行(崩溃时写了半行)跳过。
fn latest_drafts_per_message(content: &str) -> Vec<ChatMessage> {
    let mut order: Vec<String> = Vec::new();
    let mut latest: HashMap<String, ChatMessage> = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<ChatMessage>(line) else {
            continue;
        };
        if !latest.contains_key(&message.id) {
            order.push(message.id.clone());
        }
        latest.insert(message.id.clone(), message);
    }
    order
        .into_iter()
        .filter_map(|id| latest.remove(&id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(id: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: id.to_string(),
            role: "assistant".to_string(),
            content: content.to_string(),
            attachments: Vec::new(),
            reasoning: None,
            artifacts: Vec::new(),
            tool_calls: Vec::new(),
            segments: Vec::new(),
            agent_plan: None,
            api_messages: Vec::new(),
            model_messages: Vec::new(),
            active_skill_id: None,
            run_entry: None,
            stream_outcome: Some("interrupted".to_string()),
            usage: None,
            anchor_usage: None,
            group_id: None,
            provider_id: None,
            model: None,
            timestamp: 0,
            degraded: None,
        }
    }

    #[test]
    fn latest_drafts_takes_last_line_per_id_and_skips_bad_lines() {
        let lines = [
            serde_json::to_string(&draft("msg_a", "round 1")).unwrap(),
            "{ broken json".to_string(),
            serde_json::to_string(&draft("msg_b", "other run")).unwrap(),
            serde_json::to_string(&draft("msg_a", "round 2")).unwrap(),
            String::new(),
        ];
        let content = lines.join("\n");
        let drafts = latest_drafts_per_message(&content);
        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0].id, "msg_a");
        assert_eq!(drafts[0].content, "round 2");
        assert_eq!(drafts[1].id, "msg_b");
    }

    #[test]
    fn latest_drafts_empty_content_yields_nothing() {
        assert!(latest_drafts_per_message("").is_empty());
        assert!(latest_drafts_per_message("\n\n").is_empty());
    }
}
