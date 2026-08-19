use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};

use base64::{engine::general_purpose, Engine as _};
use tauri::AppHandle;
use uuid::Uuid;

use crate::chat::model::MessagePart;
use crate::mcp::types::ChatToolArtifact;

use super::storage::conversation_attachments_dir;
use super::{Attachment, ChatMessage};

const MAX_ATTACHMENT_PREVIEW_BYTES: u64 = 12 * 1024 * 1024;
const MAX_PASTED_IMAGE_BYTES: usize = 12 * 1024 * 1024;
const MAX_PASTED_ATTACHMENT_BYTES: usize = 25 * 1024 * 1024;

/// 内存文本附件（粘贴长文本自动生成的虚拟 txt）：正文只存在于请求内存、不落盘。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TextAttachmentInput {
    pub name: String,
    pub content: String,
}

/// 判断是否为内存虚拟文本附件：path 以 `memory://` 标记（正文不落盘，仅持久化附件记录）。
pub(crate) fn is_memory_text_attachment(attachment: &Attachment) -> bool {
    attachment.path.starts_with("memory://")
}

pub(crate) enum PastedImageSave {
    Saved {
        path: PathBuf,
        name: String,
        mime_type: &'static str,
    },
    Failed {
        error: String,
    },
}

pub(crate) enum PastedAttachmentSave {
    Saved { path: PathBuf, name: String },
    Failed { error: String },
}

pub(crate) fn save_pasted_image(
    name: &str,
    mime_type: &str,
    data_base64: &str,
) -> Result<PastedImageSave, String> {
    let mime = normalize_pasted_image_mime(mime_type)?;
    let ext = extension_for_image_mime(mime);
    let mut safe_name = sanitize_attachment_name(name);
    if attachment_type_for_name(&safe_name) != "image" {
        safe_name = format!("{safe_name}.{ext}");
    }

    let payload = data_base64.trim();
    if payload.is_empty() {
        return Ok(PastedImageSave::Failed {
            error: "剪贴板图片为空".to_string(),
        });
    }

    let bytes = match general_purpose::STANDARD.decode(payload) {
        Ok(bytes) => bytes,
        Err(err) => {
            return Ok(PastedImageSave::Failed {
                error: format!("解析剪贴板图片失败: {err}"),
            });
        }
    };
    if bytes.len() > MAX_PASTED_IMAGE_BYTES {
        return Ok(PastedImageSave::Failed {
            error: "剪贴板图片过大，无法添加".to_string(),
        });
    }

    let (path, saved_name) = write_pasted_attachment_bytes(&safe_name, &bytes)
        .map_err(|e| format!("保存剪贴板图片失败: {e}"))?;

    Ok(PastedImageSave::Saved {
        path,
        name: saved_name,
        mime_type: mime,
    })
}

pub(crate) fn save_pasted_attachment(
    name: &str,
    data_base64: &str,
) -> Result<PastedAttachmentSave, String> {
    let safe_name = sanitize_attachment_name(name);
    if !is_attachable_file_name(&safe_name) {
        return Ok(PastedAttachmentSave::Failed {
            error: "无效的文件名".to_string(),
        });
    }

    let payload = data_base64.trim();
    if payload.is_empty() {
        return Ok(PastedAttachmentSave::Failed {
            error: "剪贴板附件为空".to_string(),
        });
    }

    let bytes = match general_purpose::STANDARD.decode(payload) {
        Ok(bytes) => bytes,
        Err(err) => {
            return Ok(PastedAttachmentSave::Failed {
                error: format!("解析剪贴板附件失败: {err}"),
            });
        }
    };
    if bytes.len() > MAX_PASTED_ATTACHMENT_BYTES {
        return Ok(PastedAttachmentSave::Failed {
            error: "剪贴板附件过大，无法添加".to_string(),
        });
    }

    let (path, saved_name) = write_pasted_attachment_bytes(&safe_name, &bytes)?;
    Ok(PastedAttachmentSave::Saved {
        path,
        name: saved_name,
    })
}

fn write_pasted_attachment_bytes(name: &str, bytes: &[u8]) -> Result<(PathBuf, String), String> {
    let dir = std::env::temp_dir().join("kivio-chat-paste");
    fs::create_dir_all(&dir).map_err(|e| format!("创建临时附件目录失败: {e}"))?;
    let file_name = format!("paste-{}-{}", Uuid::new_v4(), name);
    let path = dir.join(&file_name);
    fs::write(&path, bytes).map_err(|e| format!("保存剪贴板附件失败: {e}"))?;
    Ok((path, name.to_string()))
}

pub(crate) fn is_attachable_file_name(name: &str) -> bool {
    !name.trim().is_empty()
}

pub(crate) fn resolve_attachment_file_path(
    app: &AppHandle,
    conversation_id: Option<&str>,
    path: &str,
) -> Result<PathBuf, String> {
    if path.trim().is_empty() {
        return Err("附件路径为空".to_string());
    }

    if let Some(conversation_id) = conversation_id {
        if path.contains('/') || path.contains('\\') {
            return Err("无效的附件路径".to_string());
        }
        let dir = conversation_attachments_dir(app, conversation_id)?;
        let full = dir.join(path);
        if !full.is_file() {
            return Err(format!("附件不存在: {path}"));
        }
        return Ok(full);
    }

    let full = PathBuf::from(path);
    if !full.is_file() {
        return Err(format!("文件不存在: {path}"));
    }
    Ok(full)
}

fn normalize_pasted_image_mime(mime_type: &str) -> Result<&'static str, String> {
    match mime_type.trim().to_ascii_lowercase().as_str() {
        "image/png" => Ok("image/png"),
        "image/jpeg" | "image/jpg" => Ok("image/jpeg"),
        "image/gif" => Ok("image/gif"),
        "image/webp" => Ok("image/webp"),
        "image/bmp" => Ok("image/bmp"),
        "image/tiff" => Ok("image/tiff"),
        "image/heic" => Ok("image/heic"),
        "image/heif" => Ok("image/heif"),
        _ => Err("仅支持粘贴图片".to_string()),
    }
}

fn extension_for_image_mime(mime_type: &str) -> &'static str {
    match mime_type {
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        "image/heic" => "heic",
        "image/heif" => "heif",
        _ => "png",
    }
}

fn mime_type_for_attachment(name: &str) -> &'static str {
    let ext = Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xlsm" => "application/vnd.ms-excel.sheet.macroenabled.12",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "txt" => "text/plain",
        "md" => "text/markdown",
        _ => "application/octet-stream",
    }
}

pub(crate) fn read_attachment_as_data_url(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|e| format!("读取附件信息失败: {e}"))?;
    if metadata.len() > MAX_ATTACHMENT_PREVIEW_BYTES {
        return Err("附件过大，无法在界面内预览".to_string());
    }
    let bytes = fs::read(path).map_err(|e| format!("读取附件失败: {e}"))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("attachment");
    let mime = mime_type_for_attachment(file_name);
    let encoded = general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{mime};base64,{encoded}"))
}

// 超过该大小的内联图片 artifact 才外置到磁盘（小图保持内联，避免无谓的读盘往返）。
const ARTIFACT_INLINE_THRESHOLD_BYTES: usize = 32 * 1024;
// 缩略图最长边像素：列表里只需要小预览，原图点开时再按 path 懒加载。
const ARTIFACT_THUMBNAIL_MAX_DIM: u32 = 256;

/// 把一条消息里内联的大图 artifact（含 `tool_calls` 内的）外置到对话附件目录：
/// 整图写盘并置 `path`，`data_url` 替换为内联缩略图（生成失败则留空，前端按 path 懒加载原图）。
/// 返回是否发生了修改。已带 `path` 的 artifact 直接跳过，因此可重复安全调用。
pub(crate) fn externalize_message_artifacts(
    app: &AppHandle,
    conversation_id: &str,
    message: &mut ChatMessage,
) -> bool {
    let mut changed = false;
    for artifact in message.artifacts.iter_mut() {
        changed |= externalize_image_artifact(app, conversation_id, artifact);
    }
    for tool_call in message.tool_calls.iter_mut() {
        for artifact in tool_call.artifacts.iter_mut() {
            changed |= externalize_image_artifact(app, conversation_id, artifact);
        }
    }
    changed |= externalize_model_message_images(app, conversation_id, message);
    changed |= externalize_api_message_images(app, conversation_id, message);
    changed
}

/// 图片外置文件名前缀。`msgimg-<sha256 前 16 位>.<ext>`：内容寻址 ⇒ 同一张图被
/// `read` 看 N 次只落一个文件（实测同图读 3 次曾在 JSON 里存 3 份 2.5MB base64）。
const MODEL_IMAGE_FILE_PREFIX: &str = "msgimg-";

/// 把 `model_messages` 里 `MessagePart::Image` 的 base64 外置到会话附件目录：
/// 字节按内容哈希写盘、`path` 置文件名、`data` 清空。返回是否发生修改。
///
/// **这是"会话 JSON 里绝不出现 base64"的落地点**。`model_messages` 是模型看到的完整
/// 转录，模型每看一张图就有一份整图 base64 被追加进去并随消息落盘；`artifacts` 那条
/// 路径早就外置了，唯独这里漏了，于是一张 1.8MB 截图读三次 = 7.57MB JSON（占文件
/// 99.8%），而每轮工具执行完的部分快照都要整本读 + clone + 序列化 + fsync 一遍。
///
/// 已有 `path` 的部件直接跳过 ⇒ 可重复安全调用（每次落盘都会跑）。写盘失败保守留着
/// base64 不动：宁可文件大，也不要让模型丢一张已经看过的图。
fn externalize_model_message_images(
    app: &AppHandle,
    conversation_id: &str,
    message: &mut ChatMessage,
) -> bool {
    if !message_has_model_message_image_to_externalize(message) {
        return false;
    }
    let Ok(dir) = conversation_attachments_dir(app, conversation_id) else {
        return false;
    };
    externalize_model_message_images_in_dir(&dir, &mut message.model_messages)
}

/// [`externalize_model_message_images`] 的纯目录版（可单测，不需要 `AppHandle`）。
fn externalize_model_message_images_in_dir(
    dir: &Path,
    messages: &mut [crate::chat::model::ModelMessage],
) -> bool {
    let mut changed = false;
    for model_message in messages.iter_mut() {
        for part in model_message.content.iter_mut() {
            let MessagePart::Image {
                mime_type,
                data,
                path,
            } = part
            else {
                continue;
            };
            if path.as_deref().is_some_and(|p| !p.is_empty()) || data.is_empty() {
                continue;
            }
            let Ok(bytes) = general_purpose::STANDARD.decode(data.as_bytes()) else {
                continue; // 解不出来就别动，保守留着原样
            };
            let file_name = format!(
                "{MODEL_IMAGE_FILE_PREFIX}{}.{}",
                &sha256_hex(&bytes)[..16],
                extension_for_image_mime(&normalize_stored_image_mime(mime_type))
            );
            let dest = dir.join(&file_name);
            // 内容寻址：同哈希同内容，已存在就直接复用，不重复写盘。
            if !dest.exists() && fs::write(&dest, &bytes).is_err() {
                continue;
            }
            *path = Some(file_name);
            data.clear();
            changed = true;
        }
    }
    changed
}

/// 廉价预扫描：`model_messages` 里有没有"带 base64 且没 path"的图片部件。
pub(crate) fn message_has_model_message_image_to_externalize(message: &ChatMessage) -> bool {
    message.model_messages.iter().any(|model_message| {
        model_message.content.iter().any(|part| {
            matches!(
                part,
                MessagePart::Image { data, path, .. }
                    if !data.is_empty() && path.as_deref().is_none_or(|p| p.is_empty())
            )
        })
    })
}

/// [`externalize_model_message_images`] 的逆操作：按 `path` 读盘把 base64 填回 `data`，
/// 供回放发给模型。文件缺失/读失败就留空，由适配器换成
/// [`crate::chat::model::MISSING_IMAGE_PLACEHOLDER`] 文本（绝不发空 base64，provider 会 400）。
///
/// 在 `build_chat_api_messages` 展开 `model_messages` 之前调用；只动传入的内存副本。
pub(crate) fn rehydrate_model_message_images(
    app: &AppHandle,
    conversation_id: &str,
    messages: &mut [crate::chat::model::ModelMessage],
) {
    if !messages.iter().any(|model_message| {
        model_message.content.iter().any(|part| {
            matches!(part, MessagePart::Image { data, path, .. }
                if data.is_empty() && path.as_deref().is_some_and(|p| !p.is_empty()))
        })
    }) {
        return;
    }
    let Ok(dir) = conversation_attachments_dir(app, conversation_id) else {
        return;
    };
    rehydrate_model_message_images_in_dir(&dir, messages);
}

/// [`rehydrate_model_message_images`] 的纯目录版（可单测，不需要 `AppHandle`）。
fn rehydrate_model_message_images_in_dir(
    dir: &Path,
    messages: &mut [crate::chat::model::ModelMessage],
) {
    for model_message in messages.iter_mut() {
        for part in model_message.content.iter_mut() {
            let MessagePart::Image { data, path, .. } = part else {
                continue;
            };
            if !data.is_empty() {
                continue;
            }
            let Some(file_name) = path.as_deref().filter(|p| !p.is_empty()) else {
                continue;
            };
            // 只信文件名（外置时我们自己生成的），任何分隔符都拒绝——别让历史脏数据
            // 变成路径穿越。
            if Path::new(file_name).components().count() != 1 {
                continue;
            }
            if let Ok(bytes) = fs::read(dir.join(file_name)) {
                *data = general_purpose::STANDARD.encode(bytes);
            }
        }
    }
}

/// 外置后写在 `api_messages` 图片部件 url 位置的哨兵 URI：`kivio-attachment://<mime>/<文件名>`。
///
/// 为什么要自造一个 scheme 而不是直接塞文件名：`api_messages` 是**原始 wire JSON**
/// （`Vec<Value>`），没有 `MessagePart` 那样的结构化 `path` 字段可用。哨兵必须
/// ① 不含 base64、② 能原样还原出 `data:<mime>;base64,<payload>`（mime 不能靠扩展名猜，
/// `image/jpeg` 与 `.jpg` 不是双射）、③ 一眼能认出不是真 URL（免得漏 rehydrate 时被
/// 当成远程图发出去）。
const ATTACHMENT_URI_SCHEME: &str = "kivio-attachment://";

/// `api_messages` 里可能出现的图片部件 `type`（与 `agent::prepare::IMAGE_PART_TYPES` 同口径）。
const API_IMAGE_PART_TYPES: [&str; 3] = ["image_url", "input_image", "image"];

/// 把 `api_messages`（OpenAI wire 格式的隐藏转录）里的图片 base64 外置。
///
/// **为什么它和 `model_messages` 都要处理**：中断草稿（`stream_outcome == "interrupted"`）
/// 是唯一同时持有两份转录的形态（`commands/messages.rs::persist_partial_assistant_snapshot`
/// 两个字段都填，因为「继续」要靠它恢复工具上下文）。只外置 `model_messages` 的话，
/// 「读了图之后点停止」这条路径照样把整张 base64 落进 JSON —— 同一个 bug 的另一半。
///
/// 完成态消息走 `build_assistant_message`，`model_messages` 非空时 `api_messages` 会被清空，
/// 所以那条路径上这个函数是空转（廉价谓词直接返回）。
fn externalize_api_message_images(
    app: &AppHandle,
    conversation_id: &str,
    message: &mut ChatMessage,
) -> bool {
    if !message_has_api_message_image_to_externalize(message) {
        return false;
    }
    let Ok(dir) = conversation_attachments_dir(app, conversation_id) else {
        return false;
    };
    externalize_api_message_images_in_dir(&dir, &mut message.api_messages)
}

/// [`externalize_api_message_images`] 的纯目录版（可单测，不需要 `AppHandle`）。
fn externalize_api_message_images_in_dir(dir: &Path, messages: &mut [serde_json::Value]) -> bool {
    let mut changed = false;
    for message in messages.iter_mut() {
        for url_slot in api_message_image_url_slots(message) {
            let Some(url) = url_slot.as_str() else {
                continue;
            };
            let Some((mime, payload)) = parse_data_url(url.trim()) else {
                continue; // 已经是哨兵 / 远程 URL / 非 base64 → 不动
            };
            if !mime.starts_with("image/") {
                continue;
            }
            let Ok(bytes) = general_purpose::STANDARD.decode(payload) else {
                continue;
            };
            let normalized = normalize_stored_image_mime(&mime);
            let file_name = format!(
                "{MODEL_IMAGE_FILE_PREFIX}{}.{}",
                &sha256_hex(&bytes)[..16],
                extension_for_image_mime(&normalized)
            );
            let dest = dir.join(&file_name);
            // 内容寻址 ⇒ 与 `model_messages` 侧同名同文件，两份转录引用同一张图不重复写盘。
            if !dest.exists() && fs::write(&dest, &bytes).is_err() {
                continue;
            }
            *url_slot = serde_json::Value::String(format!(
                "{ATTACHMENT_URI_SCHEME}{normalized}/{file_name}"
            ));
            changed = true;
        }
    }
    changed
}

/// 廉价预扫描：`api_messages` 里有没有内联 base64 图片。
pub(crate) fn message_has_api_message_image_to_externalize(message: &ChatMessage) -> bool {
    message.api_messages.iter().any(|api_message| {
        let mut probe = api_message.clone();
        api_message_image_url_slots(&mut probe)
            .into_iter()
            .any(|slot| {
                slot.as_str()
                    .and_then(|url| parse_data_url(url.trim()))
                    .is_some_and(|(mime, _)| mime.starts_with("image/"))
            })
    })
}

/// [`externalize_api_message_images`] 的逆操作：把哨兵 URI 还原成 `data:` URL。
///
/// 文件读不到就把整个部件换成文本占位符（与 `MessagePart` 侧的
/// [`crate::chat::model::MISSING_IMAGE_PLACEHOLDER`] 同语义）——绝不把
/// `kivio-attachment://` 原样发给 provider。
pub(crate) fn rehydrate_api_message_images(
    app: &AppHandle,
    conversation_id: &str,
    messages: &mut [serde_json::Value],
) {
    if !messages.iter().any(api_message_has_attachment_uri) {
        return;
    }
    let Ok(dir) = conversation_attachments_dir(app, conversation_id) else {
        return;
    };
    rehydrate_api_message_images_in_dir(&dir, messages);
}

fn api_message_has_attachment_uri(message: &serde_json::Value) -> bool {
    let mut probe = message.clone();
    api_message_image_url_slots(&mut probe)
        .into_iter()
        .any(|slot| {
            slot.as_str()
                .is_some_and(|url| url.starts_with(ATTACHMENT_URI_SCHEME))
        })
}

/// [`rehydrate_api_message_images`] 的纯目录版（可单测，不需要 `AppHandle`）。
fn rehydrate_api_message_images_in_dir(dir: &Path, messages: &mut [serde_json::Value]) {
    for message in messages.iter_mut() {
        for url_slot in api_message_image_url_slots(message) {
            let Some(url) = url_slot.as_str() else {
                continue;
            };
            let Some((mime, file_name)) = parse_attachment_uri(url) else {
                continue;
            };
            match fs::read(dir.join(file_name)) {
                Ok(bytes) => {
                    let encoded = general_purpose::STANDARD.encode(bytes);
                    *url_slot = serde_json::Value::String(format!("data:{mime};base64,{encoded}"));
                }
                // 读不到：留下人类可读的说明。这里只能改 url 字段（拿不到 part 本体），
                // 而空 url / 残留哨兵都会被 provider 当成非法图片，所以退化成 data URL 形态的
                // 1x1 透明 PNG 不可行——直接置空串，由 `sanitize_api_message_for_model`
                // 之后的适配器按空 url 跳过该部件。
                Err(_) => {
                    *url_slot = serde_json::Value::String(String::new());
                }
            }
        }
    }
}

/// 解析哨兵 URI → `(mime, 文件名)`。
///
/// 严格要求 scheme 之后**恰好三段** `image/<subtype>/<文件名>`：`rsplit_once('/')` 那种松散
/// 切法会把 `image/png/../../etc/passwd` 切成 mime=`image/png/../..` + file=`passwd`
/// —— 文件名本身是安全的（单段，join 不会逃出目录），但那个 mime 会被原样拼进
/// `data:<mime>;base64,` 发给 provider。宁可整条拒绝。
fn parse_attachment_uri(url: &str) -> Option<(String, &str)> {
    let rest = url.strip_prefix(ATTACHMENT_URI_SCHEME)?;
    let mut segments = rest.split('/');
    let top = segments.next()?;
    let subtype = segments.next()?;
    let file_name = segments.next()?;
    if segments.next().is_some() {
        return None; // 多于三段 ⇒ 脏数据
    }
    if top != "image" || subtype.is_empty() || file_name.is_empty() {
        return None;
    }
    // 文件名是外置时我们自己生成的，必须是单段、不含任何路径分量。
    if Path::new(file_name).components().count() != 1 {
        return None;
    }
    Some((format!("{top}/{subtype}"), file_name))
}

/// 收集一条 wire 消息里所有「装着图片 URL 的槽位」的可变引用。
///
/// 三种 wire 形状共用一个出口，免得每个调用方各写一遍 match：
/// - OpenAI Chat：`{type:"image_url", image_url:{url:"data:…"}}`
/// - Responses：`{type:"input_image", image_url:"data:…"}`（url 直接是字符串）
/// - 少数中转仿写：`{type:"image", image_url:…}`
///
/// Anthropic 的 `source.data` 形状**不在**此列：`api_messages` 按定义是 OpenAI 兼容转录
/// （`model_messages_from_openai_messages` 只认这几种），Anthropic 的块数组从不落到这里。
fn api_message_image_url_slots(message: &mut serde_json::Value) -> Vec<&mut serde_json::Value> {
    let mut slots = Vec::new();
    let Some(parts) = message.get_mut("content").and_then(|c| c.as_array_mut()) else {
        return slots;
    };
    for part in parts.iter_mut() {
        let is_image = part
            .get("type")
            .and_then(|t| t.as_str())
            .is_some_and(|kind| API_IMAGE_PART_TYPES.contains(&kind));
        if !is_image {
            continue;
        }
        let Some(image_url) = part.get_mut("image_url") else {
            continue;
        };
        // 对象形（`image_url.url`）与字符串形（`image_url` 本身）都要覆盖。
        if image_url.is_object() {
            if let Some(url) = image_url.get_mut("url") {
                slots.push(url);
            }
        } else if image_url.is_string() {
            slots.push(image_url);
        }
    }
    slots
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// 把 `MessagePart::Image` 的 mime 归一到 `extension_for_image_mime` 认识的形态
/// （它只匹配全小写、且 `image/jpg` 不在其列）。
fn normalize_stored_image_mime(mime_type: &str) -> String {
    let lowered = mime_type.trim().to_ascii_lowercase();
    if lowered == "image/jpg" {
        "image/jpeg".to_string()
    } else {
        lowered
    }
}
/// 快速判断:消息里是否存在"需要外置"的内联大图(图片 + 无 path + data_url 超阈值)。
/// 用于会话持久化的廉价预扫描——没有这类 artifact 就完全不必克隆对话。
pub(crate) fn message_has_inline_image_to_externalize(message: &ChatMessage) -> bool {
    let needs = |artifact: &ChatToolArtifact| {
        if artifact
            .path
            .as_deref()
            .map(|p| !p.is_empty())
            .unwrap_or(false)
        {
            return false;
        }
        match parse_data_url(artifact.data_url.trim()) {
            Some((mime, payload)) => {
                mime.starts_with("image/")
                    && decoded_base64_len(payload) > ARTIFACT_INLINE_THRESHOLD_BYTES
            }
            None => false,
        }
    };
    message.artifacts.iter().any(needs)
        || message
            .tool_calls
            .iter()
            .any(|tool_call| tool_call.artifacts.iter().any(needs))
}

fn externalize_image_artifact(
    app: &AppHandle,
    conversation_id: &str,
    artifact: &mut ChatToolArtifact,
) -> bool {
    if artifact
        .path
        .as_deref()
        .map(|p| !p.is_empty())
        .unwrap_or(false)
    {
        return false;
    }
    let Some((mime, payload)) = parse_data_url(artifact.data_url.trim()) else {
        return false;
    };
    if !mime.starts_with("image/") {
        return false;
    }
    let Ok(bytes) = general_purpose::STANDARD.decode(payload) else {
        return false;
    };
    if bytes.len() <= ARTIFACT_INLINE_THRESHOLD_BYTES {
        return false;
    }

    let dir = match conversation_attachments_dir(app, conversation_id) {
        Ok(dir) => dir,
        Err(_) => return false,
    };
    let file_name = format!(
        "artifact-{}.{}",
        Uuid::new_v4(),
        extension_for_image_mime(&mime)
    );
    if fs::write(dir.join(&file_name), &bytes).is_err() {
        return false;
    }

    artifact.size_bytes = Some(bytes.len() as u64);
    artifact.data_url = make_thumbnail_data_url(&bytes).unwrap_or_default();
    artifact.path = Some(file_name);
    true
}

/// 解析 `data:<mime>;base64,<payload>`，返回 (小写 mime, payload)。非 base64 data URL 返回 None。
fn parse_data_url(data_url: &str) -> Option<(String, &str)> {
    let rest = data_url.strip_prefix("data:")?;
    let comma = rest.find(',')?;
    let meta = &rest[..comma];
    if !meta.contains("base64") {
        return None;
    }
    let mime = meta.split(';').next().unwrap_or("").to_ascii_lowercase();
    Some((mime, &rest[comma + 1..]))
}

/// 不真正解码，估算 base64 payload 的解码字节数(用于阈值预扫描)。
fn decoded_base64_len(payload: &str) -> usize {
    let len = payload.trim_end_matches('=').len();
    len / 4 * 3 + (len % 4).saturating_sub(1)
}

/// 用已有的 `image` crate 生成 PNG 缩略图的内联 data URL。解码/编码失败返回 None。
fn make_thumbnail_data_url(bytes: &[u8]) -> Option<String> {
    let img = image::load_from_memory(bytes).ok()?;
    let thumb = img.thumbnail(ARTIFACT_THUMBNAIL_MAX_DIM, ARTIFACT_THUMBNAIL_MAX_DIM);
    let mut buf = Cursor::new(Vec::new());
    thumb.write_to(&mut buf, image::ImageFormat::Png).ok()?;
    Some(format!(
        "data:image/png;base64,{}",
        general_purpose::STANDARD.encode(buf.get_ref())
    ))
}

pub(crate) fn save_message_attachments(
    app: &AppHandle,
    conversation_id: &str,
    attachment_paths: Vec<String>,
) -> Result<Vec<Attachment>, String> {
    let mut attachments = Vec::new();
    if attachment_paths.is_empty() {
        return Ok(attachments);
    }

    let dir = conversation_attachments_dir(app, conversation_id)?;
    for source in attachment_paths {
        let source_path = Path::new(&source);
        if !source_path.is_file() {
            return Err(format!("附件不存在或不是文件: {source}"));
        }

        let id = format!("att_{}", Uuid::new_v4());
        let original_name = source_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("attachment");
        let safe_name = sanitize_attachment_name(original_name);
        let stored_name = format!("{}-{}", id, safe_name);
        let dest = dir.join(&stored_name);
        fs::copy(source_path, &dest).map_err(|e| format!("保存附件失败: {e}"))?;

        attachments.push(Attachment {
            id,
            attachment_type: attachment_type_for_name(original_name).to_string(),
            name: original_name.to_string(),
            path: stored_name,
            content: None,
        });
    }

    Ok(attachments)
}

fn sanitize_attachment_name(name: &str) -> String {
    // Keep Unicode letters/digits (CJK, etc.). Only replace path separators and
    // other non-identifier junk — ASCII-only used to store 销售报表.xlsx as
    // att_<id>-________.xlsx (or even lose the extension after trim).
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches(['.', ' ', '_']).trim();
    if trimmed.is_empty() {
        "attachment".to_string()
    } else {
        trimmed.to_string()
    }
}

fn attachment_type_for_name(name: &str) -> &'static str {
    let ext = Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "heic" | "heif" => {
            "image"
        }
        _ => "file",
    }
}

fn attachment_type_label(attachment_type: &str) -> &'static str {
    match attachment_type {
        "image" => "图片",
        _ => "文件",
    }
}

fn attachment_extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

fn attachment_skill_for_name(name: &str) -> Option<&'static str> {
    match attachment_extension(name).as_str() {
        "pdf" => Some("pdf"),
        "doc" | "docx" => Some("docx"),
        "xls" | "xlsx" | "xlsm" | "csv" | "tsv" => Some("xlsx"),
        _ => None,
    }
}

fn attachment_format_label(attachment: &Attachment) -> &'static str {
    if attachment.attachment_type == "image" {
        return "图片";
    }

    match attachment_extension(&attachment.name).as_str() {
        "pdf" => "PDF",
        "doc" | "docx" => "Word 文档",
        "xls" | "xlsx" | "xlsm" => "Excel 工作簿",
        "csv" => "CSV 表格",
        "tsv" => "TSV 表格",
        "txt" | "md" => "文本文件",
        _ => attachment_type_label(&attachment.attachment_type),
    }
}

fn stored_attachment_path_for_prompt(
    attachment: &Attachment,
    attachment_dir: Option<&Path>,
) -> String {
    attachment_dir
        .map(|dir| dir.join(&attachment.path).display().to_string())
        .unwrap_or_else(|| attachment.path.clone())
}

fn attachment_processing_hint(attachment: &Attachment) -> String {
    if attachment.attachment_type == "image" {
        return "图片附件会随本轮请求发送给视觉模型。".to_string();
    }

    if let Some(skill) = attachment_skill_for_name(&attachment.name) {
        format!(
            "推荐复用现成 `{skill}` Skill：需要读取或分析该文件时，先调用 skill(name=\"{skill}\")，再按该 Skill 的 SKILL.md / reference / scripts 流程处理安全副本路径。"
        )
    } else {
        "此文件已保存为 Kivio 安全副本；仅在有可用读取工具或对应 Skill 时处理正文。".to_string()
    }
}

pub(crate) fn compose_user_content_for_api(
    content: &str,
    attachments: &[Attachment],
    attachment_dir: Option<&Path>,
) -> String {
    let trimmed = content.trim();
    // 虚拟文本附件（memory://）正文已由 text_attachments 通道内联进 API content，
    // 这里过滤掉，避免再输出一段「Kivio 安全副本路径」造成重复。
    let real_attachments: Vec<&Attachment> = attachments
        .iter()
        .filter(|attachment| !is_memory_text_attachment(attachment))
        .collect();
    if real_attachments.is_empty() {
        return trimmed.to_string();
    }

    let has_images = real_attachments
        .iter()
        .any(|attachment| attachment.attachment_type == "image");
    let has_files = real_attachments
        .iter()
        .any(|attachment| attachment.attachment_type != "image");
    let attachment_lines = real_attachments
        .iter()
        .map(|attachment| {
            let stored_path = stored_attachment_path_for_prompt(attachment, attachment_dir);
            format!(
                "- {} ({})\n  - 附件 ID：{}\n  - Kivio 安全副本路径：{}\n  - 处理建议：{}",
                attachment.name,
                attachment_format_label(attachment),
                attachment.id,
                stored_path,
                attachment_processing_hint(attachment)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let capability_note = match (has_images, has_files) {
        (true, true) => {
            "图片附件会随本轮请求发送给视觉模型；文档/表格附件不会直接随模型请求内联正文，必须复用对应 Agent Skill 或可用工具实际读取安全副本后再分析。"
        }
        (true, false) => "图片附件会随本轮请求发送给视觉模型。",
        (false, true) => {
            "文档/表格附件不会直接随模型请求内联正文，必须复用对应 Agent Skill 或可用工具实际读取安全副本后再分析；不要仅凭文件名臆测内容。"
        }
        (false, false) => "",
    };
    let attachment_note = format!(
        "[已添加附件]\n{}\n\n注意：{}",
        attachment_lines, capability_note
    );

    if trimmed.is_empty() {
        attachment_note
    } else {
        format!("{trimmed}\n\n{attachment_note}")
    }
}

/// 把内存文本附件（虚拟 txt）正文直接内联进 API 内容：模型直接可见、无需工具读取，
/// 与磁盘附件的「安全副本 + 工具读取」路径不同。虚拟附件不落盘、不持久化到历史消息。
pub(crate) fn compose_text_attachments_for_api(
    content: &str,
    text_attachments: &[TextAttachmentInput],
) -> String {
    if text_attachments.is_empty() {
        return content.to_string();
    }

    let attachment_blocks = text_attachments
        .iter()
        .map(|attachment| {
            format!(
                "--- 文本附件：{} ---\n{}",
                attachment.name, attachment.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let note = format!(
        "[已添加文本附件]\n{}\n\n注意：文本附件正文已直接内联在本消息中，直接阅读使用即可，无需工具读取。",
        attachment_blocks
    );

    let trimmed = content.trim();
    if trimmed.is_empty() {
        note
    } else {
        format!("{trimmed}\n\n{note}")
    }
}

/// 从已持久化的附件记录还原虚拟文本附件（正文只存在 `attachment.content` 里，
/// 落库消息的原始 content 不含正文）：发送失败重试 / 重新生成时据此重建请求体。
pub(crate) fn text_attachments_from_attachments(
    attachments: &[Attachment],
) -> Vec<TextAttachmentInput> {
    attachments
        .iter()
        .filter_map(|attachment| {
            attachment
                .content
                .as_ref()
                .map(|content| TextAttachmentInput {
                    name: attachment.name.clone(),
                    content: content.clone(),
                })
        })
        .collect()
}

pub(crate) fn title_source_for_user_message(content: &str, attachments: &[Attachment]) -> String {
    let trimmed = content.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }

    let names = attachments
        .iter()
        .map(|attachment| attachment.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if names.is_empty() {
        "新对话".to_string()
    } else {
        format!("附件: {names}")
    }
}

pub(crate) fn stored_image_paths_for_attachments(
    app: &AppHandle,
    conversation_id: &str,
    attachments: &[Attachment],
) -> Result<Vec<PathBuf>, String> {
    let image_attachments = attachments
        .iter()
        .filter(|attachment| attachment.attachment_type == "image")
        .collect::<Vec<_>>();
    if image_attachments.is_empty() {
        return Ok(Vec::new());
    }

    let dir = conversation_attachments_dir(app, conversation_id)?;
    image_attachments
        .into_iter()
        .map(|attachment| {
            let stored = Path::new(&attachment.path);
            if stored.components().count() != 1 {
                return Err(format!("Invalid attachment path: {}", attachment.path));
            }
            let path = dir.join(stored);
            if !path.is_file() {
                return Err(format!("图片附件不存在: {}", attachment.name));
            }
            Ok(path)
        })
        .collect()
}

/// 解析非图片文件附件的绝对路径（外部 CLI 用：拼进 prompt + allowed-dir）。
/// 缺失文件跳过（best-effort），不致命——避免一个失效附件阻断整条消息。
pub(crate) fn stored_file_paths_for_attachments(
    app: &AppHandle,
    conversation_id: &str,
    attachments: &[Attachment],
) -> Result<Vec<PathBuf>, String> {
    let file_attachments = attachments
        .iter()
        .filter(|attachment| attachment.attachment_type != "image")
        .collect::<Vec<_>>();
    if file_attachments.is_empty() {
        return Ok(Vec::new());
    }
    let dir = conversation_attachments_dir(app, conversation_id)?;
    let mut paths = Vec::new();
    for attachment in file_attachments {
        // 虚拟文本附件（memory://）无磁盘文件，跳过。
        if is_memory_text_attachment(attachment) {
            continue;
        }
        let stored = Path::new(&attachment.path);
        if stored.components().count() != 1 {
            return Err(format!("Invalid attachment path: {}", attachment.path));
        }
        let path = dir.join(stored);
        if path.is_file() {
            paths.push(path);
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn attachment_type_detects_images_case_insensitively() {
        assert_eq!(attachment_type_for_name("screenshot.PNG"), "image");
        assert_eq!(attachment_type_for_name("scan.tif"), "image");
        assert_eq!(attachment_type_for_name("photo.heic"), "image");
        assert_eq!(attachment_type_for_name("notes.pdf"), "file");
    }

    #[test]
    fn attachable_file_names_accept_any_non_empty_name() {
        assert!(is_attachable_file_name("notes.pdf"));
        assert!(is_attachable_file_name("sheet.xlsx"));
        assert!(is_attachable_file_name("archive.zip"));
        assert!(is_attachable_file_name("main.rs"));
        assert!(!is_attachable_file_name("   "));
    }

    #[test]
    fn sanitize_attachment_name_removes_path_like_characters() {
        assert_eq!(sanitize_attachment_name("../secret?.png"), "secret_.png");
        assert_eq!(sanitize_attachment_name("   "), "attachment");
        assert_eq!(sanitize_attachment_name("销售报表.xlsx"), "销售报表.xlsx");
        assert_eq!(sanitize_attachment_name("Q1 销售.xlsx"), "Q1 销售.xlsx");
    }

    #[test]
    fn compose_user_content_for_api_mentions_attachment_names() {
        let content = compose_user_content_for_api(
            "看看这个",
            &[Attachment {
                id: "att_1".to_string(),
                attachment_type: "image".to_string(),
                name: "screen.png".to_string(),
                path: "att_1-screen.png".to_string(),
                content: None,
            }],
            None,
        );

        assert!(content.contains("看看这个"));
        assert!(content.contains("screen.png"));
        assert!(content.contains("图片附件会随本轮请求发送给视觉模型"));
    }

    #[test]
    fn compose_user_content_for_api_recommends_document_skill() {
        let content = compose_user_content_for_api(
            "总结一下",
            &[Attachment {
                id: "att_1".to_string(),
                attachment_type: "file".to_string(),
                name: "report.PDF".to_string(),
                path: "att_1-report.PDF".to_string(),
                content: None,
            }],
            Some(Path::new("/Users/test/Library/Application Support/com.zmair.kivio/conversations/conv_1_attachments")),
        );

        assert!(content.contains("report.PDF"));
        assert!(content.contains("PDF"));
        assert!(content.contains("skill(name=\"pdf\")"));
        assert!(content.contains("Kivio 安全副本路径"));
        assert!(content.contains("不要仅凭文件名臆测内容"));
    }

    #[test]
    fn compose_text_attachments_inlines_body_without_tools_note() {
        let content = compose_text_attachments_for_api(
            "分析这段日志",
            &[TextAttachmentInput {
                name: "已粘贴的文本.txt".to_string(),
                content: "line1\nline2".to_string(),
            }],
        );
        assert!(content.contains("分析这段日志"));
        assert!(content.contains("已粘贴的文本.txt"));
        assert!(content.contains("line1\nline2"));
        assert!(content.contains("无需工具读取"));
    }

    #[test]
    fn compose_text_attachments_empty_is_passthrough() {
        let content = compose_text_attachments_for_api("hello", &[]);
        assert_eq!(content, "hello");
    }

    #[test]
    fn compose_text_attachments_works_with_blank_prompt() {
        let content = compose_text_attachments_for_api(
            "   ",
            &[TextAttachmentInput {
                name: "a.txt".to_string(),
                content: "body".to_string(),
            }],
        );
        assert!(content.contains("a.txt"));
        assert!(content.contains("body"));
    }

    #[test]
    fn title_source_uses_attachment_name_when_content_empty() {
        let title = title_source_for_user_message(
            "",
            &[Attachment {
                id: "att_1".to_string(),
                attachment_type: "file".to_string(),
                name: "notes.pdf".to_string(),
                path: "att_1-notes.pdf".to_string(),
                content: None,
            }],
        );

        assert_eq!(title, "附件: notes.pdf");
    }

    #[test]
    fn text_attachments_from_attachments_restores_inline_bodies() {
        let restored = text_attachments_from_attachments(&[
            Attachment {
                id: "att_1".to_string(),
                attachment_type: "file".to_string(),
                name: "日志.txt".to_string(),
                path: "memory://abc".to_string(),
                content: Some("日志正文".to_string()),
            },
            // 磁盘附件没有正文，不得被还原成虚拟附件。
            Attachment {
                id: "att_2".to_string(),
                attachment_type: "file".to_string(),
                name: "notes.pdf".to_string(),
                path: "att_2-notes.pdf".to_string(),
                content: None,
            },
        ]);

        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].name, "日志.txt");
        assert_eq!(restored[0].content, "日志正文");
        assert!(text_attachments_from_attachments(&[]).is_empty());
    }

    #[test]
    fn parse_data_url_extracts_mime_and_payload() {
        let (mime, payload) = parse_data_url("data:image/png;base64,aGVsbG8=").unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(payload, "aGVsbG8=");
        // 大小写 mime 归一化
        let (mime, _) = parse_data_url("data:IMAGE/JPEG;base64,QQ==").unwrap();
        assert_eq!(mime, "image/jpeg");
        // 非 base64 / 非 data URL 返回 None
        assert!(parse_data_url("data:text/plain,hi").is_none());
        assert!(parse_data_url("https://example.com/a.png").is_none());
    }

    #[test]
    fn decoded_base64_len_estimates_within_one_byte() {
        // "aGVsbG8=" 解码为 "hello"(5 字节)
        assert_eq!(decoded_base64_len("aGVsbG8="), 5);
        // 无 padding 的 4 字符块 → 3 字节
        assert_eq!(decoded_base64_len("QUJD"), 3);
    }

    #[test]
    fn make_thumbnail_data_url_shrinks_large_image() {
        // 生成 512x512 PNG,缩略图(<=256)应远小于原图。
        let img = image::RgbImage::from_fn(512, 512, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        let mut original = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut original, image::ImageFormat::Png)
            .unwrap();

        let data_url = make_thumbnail_data_url(original.get_ref()).unwrap();
        assert!(data_url.starts_with("data:image/png;base64,"));
        let thumb_payload = data_url.split_once(',').unwrap().1;
        assert!(decoded_base64_len(thumb_payload) < original.get_ref().len());
    }

    #[test]
    fn message_inline_image_scan_respects_threshold_and_path() {
        let big_payload = "A".repeat(60_000); // 解码约 45KB > 32KB 阈值
        let make_message = |data_url: &str, path: Option<&str>| -> ChatMessage {
            serde_json::from_value(serde_json::json!({
                "id": "m1",
                "role": "assistant",
                "content": "",
                "timestamp": 0,
                "artifacts": [{
                    "name": "chart.png",
                    "mime_type": "image/png",
                    "data_url": data_url,
                    "path": path,
                }],
            }))
            .unwrap()
        };

        // 大内联图、无 path → 需要外置
        let msg = make_message(&format!("data:image/png;base64,{big_payload}"), None);
        assert!(message_has_inline_image_to_externalize(&msg));

        // 已有 path → 跳过
        let msg = make_message(
            &format!("data:image/png;base64,{big_payload}"),
            Some("artifact-x.png"),
        );
        assert!(!message_has_inline_image_to_externalize(&msg));

        // 小图 → 跳过
        let msg = make_message("data:image/png;base64,aGVsbG8=", None);
        assert!(!message_has_inline_image_to_externalize(&msg));
    }

    /// 造一条只含图片部件的 `model_messages`。
    fn image_model_messages(
        parts: Vec<(&str, &str, Option<&str>)>,
    ) -> Vec<crate::chat::model::ModelMessage> {
        vec![crate::chat::model::ModelMessage {
            role: crate::chat::model::ModelRole::User,
            content: parts
                .into_iter()
                .map(|(mime, data, path)| MessagePart::Image {
                    mime_type: mime.to_string(),
                    data: data.to_string(),
                    path: path.map(str::to_string),
                })
                .collect(),
        }]
    }

    fn image_parts(messages: &[crate::chat::model::ModelMessage]) -> Vec<(&str, Option<&str>)> {
        messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(|part| match part {
                MessagePart::Image { data, path, .. } => Some((data.as_str(), path.as_deref())),
                _ => None,
            })
            .collect()
    }

    /// 落盘的会话 JSON 里绝不能出现 base64：图片必须写成附件文件 + `path` 引用。
    #[test]
    fn externalize_model_message_images_moves_base64_to_disk() {
        let dir = std::env::temp_dir().join(format!("kivio-extimg-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let payload = general_purpose::STANDARD.encode(b"fake-png-bytes");

        let mut messages = image_model_messages(vec![("image/png", &payload, None)]);
        assert!(externalize_model_message_images_in_dir(&dir, &mut messages));

        let parts = image_parts(&messages);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].0, "", "base64 必须从内存形态清空");
        let file_name = parts[0].1.expect("path 必须被填上");
        assert!(file_name.starts_with(MODEL_IMAGE_FILE_PREFIX));
        assert!(file_name.ends_with(".png"));
        assert_eq!(fs::read(dir.join(file_name)).unwrap(), b"fake-png-bytes");

        // 序列化后不含 data 字段（skip_serializing_if）⇒ JSON 里没有 base64。
        let json = serde_json::to_string(&messages).unwrap();
        assert!(!json.contains(&payload));
        assert!(!json.contains("\"data\""));

        fs::remove_dir_all(&dir).ok();
    }

    /// 同一张图被读 N 次只落一个文件（内容寻址）。这是本次修复省得最狠的一处：
    /// 实测同图读 3 次曾在 JSON 里存 3 份 2.5MB base64。
    #[test]
    fn externalize_model_message_images_dedupes_identical_images() {
        let dir = std::env::temp_dir().join(format!("kivio-extimg-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let payload = general_purpose::STANDARD.encode(b"same-image");

        let mut messages = image_model_messages(vec![
            ("image/png", &payload, None),
            ("image/png", &payload, None),
            ("image/png", &payload, None),
        ]);
        assert!(externalize_model_message_images_in_dir(&dir, &mut messages));

        let parts = image_parts(&messages);
        assert_eq!(parts.len(), 3);
        let names: std::collections::HashSet<_> = parts.iter().map(|(_, p)| *p).collect();
        assert_eq!(names.len(), 1, "三份相同图片必须指向同一个文件");
        let files: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect();
        assert_eq!(files.len(), 1, "磁盘上只该有一个文件，实际 {files:?}");

        fs::remove_dir_all(&dir).ok();
    }

    /// 已有 path 的部件跳过 ⇒ 每次落盘都跑也是幂等的。
    #[test]
    fn externalize_model_message_images_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("kivio-extimg-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        let mut messages = image_model_messages(vec![(
            "image/png",
            &general_purpose::STANDARD.encode(b"bytes"),
            None,
        )]);
        assert!(externalize_model_message_images_in_dir(&dir, &mut messages));
        assert!(
            !externalize_model_message_images_in_dir(&dir, &mut messages),
            "第二次不该再改动"
        );

        fs::remove_dir_all(&dir).ok();
    }

    /// 回放时按 path 读盘填回 base64，模型看到的内容与当初一字不差。
    #[test]
    fn rehydrate_model_message_images_restores_base64_from_disk() {
        let dir = std::env::temp_dir().join(format!("kivio-extimg-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let bytes = b"round-trip-bytes";
        let payload = general_purpose::STANDARD.encode(bytes);

        let mut messages = image_model_messages(vec![("image/png", &payload, None)]);
        externalize_model_message_images_in_dir(&dir, &mut messages);
        assert_eq!(image_parts(&messages)[0].0, "");

        rehydrate_model_message_images_in_dir(&dir, &mut messages);
        assert_eq!(image_parts(&messages)[0].0, payload, "必须原样还原");

        fs::remove_dir_all(&dir).ok();
    }

    /// 附件文件被删 → data 留空，由适配器换成占位文本，绝不发空 base64 让 provider 400。
    #[test]
    fn rehydrate_tolerates_missing_and_unsafe_paths() {
        let dir = std::env::temp_dir().join(format!("kivio-extimg-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        let mut messages = image_model_messages(vec![
            ("image/png", "", Some("msgimg-deadbeef.png")),
            ("image/png", "", Some("../../../etc/passwd")),
        ]);
        rehydrate_model_message_images_in_dir(&dir, &mut messages);

        for (data, _) in image_parts(&messages) {
            assert_eq!(data, "", "读不到就留空");
        }

        fs::remove_dir_all(&dir).ok();
    }

    /// `image/jpg`（非标准写法）不能落成 `.image/jpg` 之类的坏扩展名。
    #[test]
    fn externalize_normalizes_jpg_mime_to_jpeg_extension() {
        let dir = std::env::temp_dir().join(format!("kivio-extimg-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        let mut messages = image_model_messages(vec![(
            "IMAGE/JPG",
            &general_purpose::STANDARD.encode(b"jpeg-bytes"),
            None,
        )]);
        externalize_model_message_images_in_dir(&dir, &mut messages);
        assert!(image_parts(&messages)[0].1.unwrap().ends_with(".jpg"));

        fs::remove_dir_all(&dir).ok();
    }

    /// 造一条带图的 OpenAI wire 消息（对象形 `image_url.url`）。
    fn api_image_message(data_url: &str) -> serde_json::Value {
        serde_json::json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "看这张图" },
                { "type": "image_url", "image_url": { "url": data_url } },
            ]
        })
    }

    fn api_image_url(message: &serde_json::Value) -> &str {
        message["content"][1]["image_url"]["url"]
            .as_str()
            .expect("url is a string")
    }

    /// 中断草稿的 `api_messages` 同样不许把 base64 落盘（同一个 bug 的另一半）。
    #[test]
    fn externalize_api_message_images_replaces_base64_with_sentinel() {
        let dir = std::env::temp_dir().join(format!("kivio-extimg-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let payload = general_purpose::STANDARD.encode(b"wire-png-bytes");

        let mut messages = vec![api_image_message(&format!(
            "data:image/png;base64,{payload}"
        ))];
        assert!(externalize_api_message_images_in_dir(&dir, &mut messages));

        let url = api_image_url(&messages[0]);
        assert!(url.starts_with("kivio-attachment://image/png/msgimg-"));
        let json = serde_json::to_string(&messages).unwrap();
        assert!(!json.contains(&payload), "JSON 里不许有 base64");

        // 文件真的落盘了，且与 model_messages 侧同名（内容寻址 ⇒ 两份转录共享一个文件）。
        let file_name = url.rsplit('/').next().unwrap();
        assert_eq!(fs::read(dir.join(file_name)).unwrap(), b"wire-png-bytes");

        fs::remove_dir_all(&dir).ok();
    }

    /// 回放前必须还原成 data URL，且 mime 逐字保留（不能靠扩展名猜）。
    #[test]
    fn rehydrate_api_message_images_round_trips() {
        let dir = std::env::temp_dir().join(format!("kivio-extimg-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let payload = general_purpose::STANDARD.encode(b"jpeg-wire-bytes");
        let original = format!("data:image/jpeg;base64,{payload}");

        let mut messages = vec![api_image_message(&original)];
        externalize_api_message_images_in_dir(&dir, &mut messages);
        assert!(api_image_url(&messages[0]).starts_with("kivio-attachment://"));

        rehydrate_api_message_images_in_dir(&dir, &mut messages);
        assert_eq!(api_image_url(&messages[0]), original, "必须逐字还原");

        fs::remove_dir_all(&dir).ok();
    }

    /// Responses 的 `input_image` 是字符串形（`image_url` 本身就是 url），也要覆盖。
    #[test]
    fn externalize_api_message_images_handles_string_image_url() {
        let dir = std::env::temp_dir().join(format!("kivio-extimg-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let payload = general_purpose::STANDARD.encode(b"responses-bytes");

        let mut messages = vec![serde_json::json!({
            "role": "user",
            "content": [{
                "type": "input_image",
                "image_url": format!("data:image/png;base64,{payload}"),
            }]
        })];
        assert!(externalize_api_message_images_in_dir(&dir, &mut messages));
        assert!(messages[0]["content"][0]["image_url"]
            .as_str()
            .unwrap()
            .starts_with("kivio-attachment://"));

        rehydrate_api_message_images_in_dir(&dir, &mut messages);
        assert!(messages[0]["content"][0]["image_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));

        fs::remove_dir_all(&dir).ok();
    }

    /// 文件缺失 ⇒ url 置空（由适配器跳过该部件），**绝不把哨兵原样发给 provider**。
    #[test]
    fn rehydrate_api_message_images_blanks_missing_files() {
        let dir = std::env::temp_dir().join(format!("kivio-extimg-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        let mut messages = vec![api_image_message(
            "kivio-attachment://image/png/msgimg-deadbeef.png",
        )];
        rehydrate_api_message_images_in_dir(&dir, &mut messages);
        assert_eq!(api_image_url(&messages[0]), "");

        fs::remove_dir_all(&dir).ok();
    }

    /// 哨兵里的文件名必须是单段，路径穿越一律拒绝。
    #[test]
    fn parse_attachment_uri_rejects_traversal_and_malformed() {
        assert_eq!(
            parse_attachment_uri("kivio-attachment://image/png/msgimg-a.png"),
            Some(("image/png".to_string(), "msgimg-a.png"))
        );
        assert!(parse_attachment_uri("kivio-attachment://image/png/../../etc/passwd").is_none());
        assert!(parse_attachment_uri("kivio-attachment://noslash").is_none());
        assert!(parse_attachment_uri("data:image/png;base64,AAAA").is_none());
    }

    /// 幂等：第二次不该再改动（已是哨兵，不是 data URL）。
    #[test]
    fn externalize_api_message_images_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("kivio-extimg-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        let mut messages = vec![api_image_message(&format!(
            "data:image/png;base64,{}",
            general_purpose::STANDARD.encode(b"bytes")
        ))];
        assert!(externalize_api_message_images_in_dir(&dir, &mut messages));
        assert!(
            !externalize_api_message_images_in_dir(&dir, &mut messages),
            "第二次不该再改动"
        );

        fs::remove_dir_all(&dir).ok();
    }
}
