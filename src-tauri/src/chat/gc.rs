//! 会话副产物垃圾回收：空工作区 / 孤儿工作区 / 孤儿附件。
//!
//! 三类垃圾的来源都一样——**目录/文件的创建时机比它的主人更早、生命周期比主人更长**：
//!
//! - **空工作区**：`chat-workspaces/<conv_id>/` 以前在解析路径时就无条件 `create_dir_all`，
//!   哪怕模型这一轮压根没写文件。实测 110 个目录里 105 个是空的。创建时机已改为懒创建
//!   （`workspace::ensure_effective_cwd`），这里负责清掉历史残留。
//! - **孤儿工作区**：会话删了但目录还在。`delete_conversation` 会清，但它「尽力清、失败只
//!   记警告」，且历史上有过删除中止的 bug（Windows 上跑着的 `npm run dev` 把 cwd 钉住）。
//! - **孤儿附件**：消息被删/重新生成后，它引用的 `msgimg-*` / `artifact-*` 文件仍留在
//!   `<conv_id>_attachments/` 里。
//!
//! **共同的红线：只碰 Kivio 自己造的目录。** 绑定项目的会话工作目录指向**用户自己的项目根**，
//! 那里可能是一整个 git 仓库；本模块所有扫描都锚定在 Kivio 数据目录下的 `chat-workspaces/`
//! 与 `conversations/`，且只认 `conv_` 前缀的条目，绝不接受外部传入的路径。
//!
//! 全部「尽力而为」：任何一步失败只记日志、继续下一项，绝不向上抛错。GC 是优化，
//! 不该让启动或打开会话失败。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use super::storage::conversations_dir;

/// 一次 GC 的统计（用于日志与单测断言）。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    /// 删掉的空工作区目录数。
    pub empty_workspaces: usize,
    /// 删掉的孤儿工作区目录数（会话已不存在，且目录为空）。
    pub orphan_workspaces: usize,
    /// 保留下来的非空孤儿工作区（有用户产物，只报数不删）。
    pub kept_orphan_workspaces: usize,
    /// 删掉的孤儿附件文件数。
    pub orphan_attachments: usize,
    /// 回收的字节数（仅统计孤儿附件，目录本身可忽略）。
    pub freed_bytes: u64,
}

impl GcReport {
    fn is_empty(&self) -> bool {
        self.empty_workspaces == 0
            && self.orphan_workspaces == 0
            && self.orphan_attachments == 0
            && self.kept_orphan_workspaces == 0
    }
}

/// `conv_` 前缀校验（与 `storage::validate_conversation_id` 同口径，但这里只要布尔）。
fn is_conversation_dir_name(name: &str) -> bool {
    name.starts_with("conv_")
        && name.len() > "conv_".len()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn dir_is_empty(path: &Path) -> bool {
    match fs::read_dir(path) {
        Ok(mut entries) => entries.next().is_none(),
        Err(_) => false, // 读不出来就当它非空，保守不删
    }
}

/// 启动时的一次性清扫。**只在 App 启动调用**（`lib.rs` setup），失败全部吞掉。
pub fn sweep_conversation_side_artifacts(app: &AppHandle) {
    let report = sweep_with_roots(app);
    if !report.is_empty() {
        eprintln!(
            "[chat-gc] 空工作区 {} / 孤儿工作区 {}（保留非空 {}）/ 孤儿附件 {}（回收 {:.1} MB）",
            report.empty_workspaces,
            report.orphan_workspaces,
            report.kept_orphan_workspaces,
            report.orphan_attachments,
            report.freed_bytes as f64 / (1024.0 * 1024.0)
        );
    }
}

fn sweep_with_roots(app: &AppHandle) -> GcReport {
    let mut report = GcReport::default();
    let Ok(conversations) = conversations_dir(app) else {
        return report;
    };
    let alive = alive_conversation_ids(&conversations);

    // ① + ② 工作区：Kivio 数据目录下的 chat-workspaces/，以及用户配置的内置 runtime 工作根。
    let mut workspace_roots: Vec<PathBuf> = Vec::new();
    if let Some(parent) = conversations.parent() {
        workspace_roots.push(parent.join("chat-workspaces"));
    }
    // 内置 runtime 的工作根（默认 ~/Kivio/workspace）。只扫它下面的 `conv_*` 子目录，
    // 根目录本身是用户的、绝不动。
    if let Some(state) = app.try_state::<crate::state::AppState>() {
        let configured = state
            .settings_read()
            .chat_tools
            .native_tools
            .working_directory
            .trim()
            .to_string();
        let root = if configured.is_empty() {
            crate::settings::default_chat_working_directory()
        } else {
            configured
        };
        if !root.is_empty() {
            let root = PathBuf::from(root);
            if !workspace_roots.contains(&root) {
                workspace_roots.push(root);
            }
        }
    }
    for root in &workspace_roots {
        sweep_workspace_root(root, &alive, &mut report);
    }

    // ③ 孤儿附件。
    sweep_attachment_dirs(&conversations, &alive, &mut report);

    report
}

/// 现存会话 id（只读文件名，不反序列化任何正文）。
fn alive_conversation_ids(conversations_dir: &Path) -> HashSet<String> {
    let mut alive = HashSet::new();
    let Ok(entries) = fs::read_dir(conversations_dir) else {
        return alive;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            if is_conversation_dir_name(stem) {
                alive.insert(stem.to_string());
            }
        }
    }
    alive
}

/// 扫一个工作区根：删空目录；会话已删且目录为空 ⇒ 孤儿。
///
/// **非空的孤儿一律保留**：里面是用户/模型产出的真实文件（实测残留里有 `hello.py`、
/// `jilin-weather-tomorrow.html` 这种），删掉就是数据丢失。只记数，让用户自己处置。
fn sweep_workspace_root(root: &Path, alive: &HashSet<String>, report: &mut GcReport) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // `__global__`（外部 CLI 探测用的稳定 scope）不是会话目录，必须留着。
        if !is_conversation_dir_name(name) {
            continue;
        }
        let orphan = !alive.contains(name);
        if !dir_is_empty(&path) {
            if orphan {
                report.kept_orphan_workspaces += 1;
            }
            continue;
        }
        // 空目录：无论会话还在不在都可以删——真要用时 `ensure_effective_cwd` 会重建。
        if fs::remove_dir(&path).is_ok() {
            if orphan {
                report.orphan_workspaces += 1;
            } else {
                report.empty_workspaces += 1;
            }
        }
    }
}

/// 扫附件目录：会话已删 ⇒ 整个目录删掉；会话还在 ⇒ 交给按引用对账。
fn sweep_attachment_dirs(conversations_dir: &Path, alive: &HashSet<String>, report: &mut GcReport) {
    let Ok(entries) = fs::read_dir(conversations_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(conv_id) = name.strip_suffix("_attachments") else {
            continue;
        };
        if !is_conversation_dir_name(conv_id) {
            continue;
        }
        if alive.contains(conv_id) {
            continue; // 活会话的附件走 `collect_unreferenced_attachments`，不在启动时做
        }
        // 会话已删：整个目录是孤儿。
        let (files, bytes) = dir_file_stats(&path);
        if fs::remove_dir_all(&path).is_ok() {
            report.orphan_attachments += files;
            report.freed_bytes += bytes;
        }
    }
}

fn dir_file_stats(dir: &Path) -> (usize, u64) {
    let mut files = 0usize;
    let mut bytes = 0u64;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    files += 1;
                    bytes += meta.len();
                }
            }
        }
    }
    (files, bytes)
}

/// GC 自动生成的附件文件前缀。**只清这两类**：
/// - `msgimg-` 模型看过的图（`attachments::externalize_model_message_images`）
/// - `artifact-` 工具产出的图（`attachments::externalize_image_artifact`）
///
/// 用户上传的附件是 `att_<uuid>-<原名>`，**永不自动删**：即使当前没有任何消息引用它
/// （用户删了那条消息），那也是用户自己拖进来的文件，删掉是数据丢失。
const GC_MANAGED_ATTACHMENT_PREFIXES: &[&str] = &["msgimg-", "artifact-"];

fn is_gc_managed_attachment(name: &str) -> bool {
    GC_MANAGED_ATTACHMENT_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

/// 收集一个会话附件目录里**没有任何消息引用**的 GC 托管附件。
///
/// 纯函数（吃「目录里的文件名」+「消息引用到的文件名」两个集合），便于单测覆盖那条
/// 关键不变式：**引用集合为空时必须返回空**——若上层因为任何原因（消息还没落盘、
/// 转录被剥离、解析失败）交来一个空引用集，绝不能把整目录判成孤儿全删。
pub fn unreferenced_attachment_names(
    files_on_disk: &[String],
    referenced: &HashSet<String>,
) -> Vec<String> {
    if referenced.is_empty() {
        // 保守：一个都不删。宁可留垃圾，也不能误删还在用的图。
        return Vec::new();
    }
    files_on_disk
        .iter()
        .filter(|name| is_gc_managed_attachment(name) && !referenced.contains(*name))
        .cloned()
        .collect()
}

/// 一条会话里被引用到的附件文件名：消息附件、artifact 的 `path`、`model_messages` 里
/// 图片部件的 `path`。三处都要算，漏一处就会误删还在用的文件。
pub fn referenced_attachment_names(conversation: &super::Conversation) -> HashSet<String> {
    let mut referenced = HashSet::new();
    for message in &conversation.messages {
        for attachment in &message.attachments {
            if !attachment.path.is_empty() {
                referenced.insert(attachment.path.clone());
            }
        }
        let mut push_artifact_path = |artifact: &crate::mcp::types::ChatToolArtifact| {
            if let Some(path) = artifact.path.as_deref().filter(|p| !p.is_empty()) {
                referenced.insert(path.to_string());
            }
        };
        for artifact in &message.artifacts {
            push_artifact_path(artifact);
        }
        for tool_call in &message.tool_calls {
            for artifact in &tool_call.artifacts {
                push_artifact_path(artifact);
            }
        }
        for model_message in &message.model_messages {
            for part in &model_message.content {
                match part {
                    crate::chat::model::MessagePart::Image { path, .. } => {
                        if let Some(path) = path.as_deref().filter(|p| !p.is_empty()) {
                            referenced.insert(path.to_string());
                        }
                    }
                    // 工具结果里的 artifact 也带 path（与 `tool_calls[].artifacts` 可能不同源）。
                    crate::chat::model::MessagePart::ToolResult { artifacts, .. } => {
                        for artifact in artifacts {
                            if let Some(path) = artifact.path.as_deref().filter(|p| !p.is_empty()) {
                                referenced.insert(path.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // 中断草稿的 `api_messages` 里图片被外置成 `kivio-attachment://<mime>/<文件名>`
        // 哨兵（`attachments::externalize_api_message_images`）。**这也是一处引用来源**，
        // 漏掉就会把中断草稿还要用的图当成孤儿删掉。
        for api_message in &message.api_messages {
            collect_attachment_uri_names(api_message, &mut referenced);
        }
    }
    referenced
}

/// 从一条 wire 消息里挖出所有 `kivio-attachment://` 哨兵引用的文件名。
///
/// 不复用 `attachments::api_message_image_url_slots`（那个要 `&mut`），这里是纯只读遍历，
/// 且刻意**不限定部件 type**：宁可多认几个槽位，也不要漏掉一个引用而误删文件。
fn collect_attachment_uri_names(value: &serde_json::Value, out: &mut HashSet<String>) {
    match value {
        serde_json::Value::String(text) => {
            if let Some(name) = attachment_uri_file_name(text) {
                out.insert(name.to_string());
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_attachment_uri_names(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values() {
                collect_attachment_uri_names(item, out);
            }
        }
        _ => {}
    }
}

fn attachment_uri_file_name(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("kivio-attachment://")?;
    let (_mime, file_name) = rest.rsplit_once('/')?;
    if file_name.is_empty() || Path::new(file_name).components().count() != 1 {
        return None;
    }
    Some(file_name)
}

/// 清掉一条会话附件目录里不再被引用的 GC 托管附件。返回删除数与回收字节。
///
/// 调用点是**删除消息 / rewind 之后**（那是引用真正消失的唯一时机），且必须在那次改动
/// **已经落盘**之后调用。
///
/// **只吃 `conversation_id`，自己从磁盘重读会话** —— 这是刻意的，不接受调用方传进来的
/// `Conversation` 对象：内存里的副本随时可能已经被 `strip_transcripts_for_frontend`
/// 洗过（它把完成态消息的 `model_messages` / `api_messages` 整个清空），拿那种残缺副本
/// 算引用集合，会把**还在被引用的图**判成孤儿删掉。磁盘上的副本永远是完整转录，
/// 是唯一可信的引用来源。读盘失败就什么都不删。
pub fn sweep_conversation_attachments(app: &AppHandle, conversation_id: &str) -> (usize, u64) {
    let Ok(conversations) = conversations_dir(app) else {
        return (0, 0);
    };
    let dir = conversations.join(format!("{conversation_id}_attachments"));
    if !dir.is_dir() {
        return (0, 0);
    }
    // 读盘失败 ⇒ 引用集合不可信 ⇒ 一个都不删。
    let Ok(conversation) = super::storage::load_conversation(app, conversation_id) else {
        return (0, 0);
    };

    let referenced = referenced_attachment_names(&conversation);
    let files_on_disk: Vec<String> = match fs::read_dir(&dir) {
        Ok(entries) => entries
            .flatten()
            .filter(|entry| entry.path().is_file())
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .collect(),
        Err(_) => return (0, 0),
    };
    let mut removed = 0usize;
    let mut freed = 0u64;
    for name in unreferenced_attachment_names(&files_on_disk, &referenced) {
        let path = dir.join(&name);
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if fs::remove_file(&path).is_ok() {
            removed += 1;
            freed += size;
        }
    }
    (removed, freed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kivio-gc-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn conversation_dir_names_reject_foreign_entries() {
        assert!(is_conversation_dir_name("conv_abc-123"));
        assert!(!is_conversation_dir_name("__global__"));
        assert!(!is_conversation_dir_name("conv_"));
        assert!(!is_conversation_dir_name("../etc"));
        assert!(!is_conversation_dir_name("my-project"));
    }

    /// 空目录删掉；非空的孤儿保留（里面是用户产物）；`__global__` 绝不碰。
    #[test]
    fn sweep_workspace_root_removes_empty_keeps_nonempty_orphans() {
        let root = temp_root("ws");
        let alive: HashSet<String> = ["conv_alive".to_string()].into_iter().collect();

        fs::create_dir_all(root.join("conv_alive")).unwrap(); // 活会话，空 → 删
        fs::create_dir_all(root.join("conv_dead_empty")).unwrap(); // 孤儿，空 → 删
        fs::create_dir_all(root.join("conv_dead_full")).unwrap();
        fs::write(root.join("conv_dead_full/hello.py"), "print(1)").unwrap(); // 孤儿，非空 → 留
        fs::create_dir_all(root.join("__global__")).unwrap(); // 探测 scope → 留

        let mut report = GcReport::default();
        sweep_workspace_root(&root, &alive, &mut report);

        assert_eq!(report.empty_workspaces, 1);
        assert_eq!(report.orphan_workspaces, 1);
        assert_eq!(report.kept_orphan_workspaces, 1);
        assert!(!root.join("conv_alive").exists());
        assert!(!root.join("conv_dead_empty").exists());
        assert!(
            root.join("conv_dead_full/hello.py").exists(),
            "用户产物绝不能删"
        );
        assert!(root.join("__global__").exists(), "__global__ 必须保留");

        fs::remove_dir_all(&root).ok();
    }

    /// 会话已删 ⇒ 整个附件目录是孤儿。
    #[test]
    fn sweep_attachment_dirs_removes_only_dead_conversations() {
        let root = temp_root("att");
        let alive: HashSet<String> = ["conv_alive".to_string()].into_iter().collect();

        let alive_dir = root.join("conv_alive_attachments");
        fs::create_dir_all(&alive_dir).unwrap();
        fs::write(alive_dir.join("msgimg-aaa.png"), b"x").unwrap();

        let dead_dir = root.join("conv_dead_attachments");
        fs::create_dir_all(&dead_dir).unwrap();
        fs::write(dead_dir.join("msgimg-bbb.png"), b"yy").unwrap();
        fs::write(dead_dir.join("att_1-user.pdf"), b"zzz").unwrap();

        let mut report = GcReport::default();
        sweep_attachment_dirs(&root, &alive, &mut report);

        assert_eq!(report.orphan_attachments, 2);
        assert_eq!(report.freed_bytes, 5);
        assert!(
            alive_dir.join("msgimg-aaa.png").exists(),
            "活会话附件不能碰"
        );
        assert!(!dead_dir.exists());

        fs::remove_dir_all(&root).ok();
    }

    /// 只清 GC 托管的前缀；用户上传的 `att_*` 永不自动删。
    #[test]
    fn unreferenced_only_covers_gc_managed_prefixes() {
        let files = vec![
            "msgimg-referenced.png".to_string(),
            "msgimg-orphan.png".to_string(),
            "artifact-orphan.png".to_string(),
            "att_1-user-upload.pdf".to_string(), // 无引用也必须留
        ];
        let referenced: HashSet<String> =
            ["msgimg-referenced.png".to_string()].into_iter().collect();

        let mut orphans = unreferenced_attachment_names(&files, &referenced);
        orphans.sort();
        assert_eq!(orphans, vec!["artifact-orphan.png", "msgimg-orphan.png"]);
    }

    /// **关键不变式**：引用集合为空时一个都不删。空引用集只可能来自上层的意外
    /// （消息未落盘 / 转录被剥离 / 解析失败），此时删除等于清空用户的图。
    #[test]
    fn unreferenced_returns_nothing_when_reference_set_is_empty() {
        let files = vec!["msgimg-a.png".to_string(), "artifact-b.png".to_string()];
        assert!(unreferenced_attachment_names(&files, &HashSet::new()).is_empty());
    }

    /// 中断草稿的 `api_messages` 哨兵也是一处引用来源，漏掉就会误删它还要用的图。
    #[test]
    fn attachment_uri_names_are_collected_from_wire_messages() {
        let api_message = serde_json::json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "看图" },
                {
                    "type": "image_url",
                    "image_url": { "url": "kivio-attachment://image/png/msgimg-wire.png" }
                },
                {
                    "type": "input_image",
                    "image_url": "kivio-attachment://image/jpeg/msgimg-responses.jpg"
                }
            ]
        });
        let mut names = HashSet::new();
        collect_attachment_uri_names(&api_message, &mut names);

        let mut sorted: Vec<_> = names.into_iter().collect();
        sorted.sort();
        assert_eq!(sorted, vec!["msgimg-responses.jpg", "msgimg-wire.png"]);
    }

    /// 路径穿越的哨兵不进引用集合（它本来也不该存在），且不 panic。
    #[test]
    fn attachment_uri_names_reject_traversal() {
        assert_eq!(
            attachment_uri_file_name("kivio-attachment://image/png/msgimg-ok.png"),
            Some("msgimg-ok.png")
        );
        assert!(attachment_uri_file_name("kivio-attachment://image/png/").is_none());
        assert!(attachment_uri_file_name("data:image/png;base64,AAAA").is_none());
    }
}
