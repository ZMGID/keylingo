//! 外部 CLI 附件处理：把用户消息的图片/文件送进本地 CLI。
//!
//! 设计（调研 Paseo `getpaseo/paseo` 得出，见任务 07-19 research/）：
//! - **图片**：支持原生图片块的协议（Claude base64 / ACP image / Codex localImage）直接注入；
//!   不支持的协议（pi/kimi）或超出 mime 白名单的图片，降级为在 prompt 文本里写出绝对路径。
//! - **文件**：所有协议一律渲染成「文件名/路径/MIME/大小」文本块（对齐 Paseo `uploaded_file`），
//!   不 inline 内容——CLI 用自己的 read 工具读该路径（附件目录会加进 allowed-dir）。

use std::path::{Path, PathBuf};

use base64::{engine::general_purpose, Engine as _};

/// 一张图片编码后的原生块载荷（各协议 adapter 再包成自己的形状）。
#[derive(Clone)]
pub struct ImageBlock {
    pub data_base64: String,
    pub mime: String,
    /// 磁盘原图路径（Codex 需要把它 copy 进临时目录传 localImage path）。
    pub path: PathBuf,
}

/// 按扩展名推断图片 MIME（与 GUI 的 `image_mime_for_path` 一致）。
pub fn image_mime_for_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "image/png",
    }
}

/// 通用文件 MIME（只覆盖常见几类，其余 octet-stream）。仅用于文件说明文本，无需精确。
fn file_mime_for_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain",
        "json" => "application/json",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
}

/// mime 是否在白名单内。空白名单 = 不限（放行任意 mime）。
fn mime_allowed(mime: &str, whitelist: &[&str]) -> bool {
    whitelist.is_empty() || whitelist.contains(&mime)
}

/// 读图片 → base64 原生块。仅对 mime 在 `whitelist` 内、且读取成功的图片生成块。
///
/// 返回 `(native, degraded)`：
/// - `native`：可原生注入的图片块。
/// - `degraded`：mime 超出白名单 **或** 读取失败的图片路径 —— 交由调用方走 [`image_paths_note`]
///   降级（写进 prompt 文本），保证图片不被静默丢弃（对齐 Paseo 的 pi/omp 降级，优于 Claude 静默 drop）。
pub fn load_image_blocks(paths: &[PathBuf], whitelist: &[&str]) -> (Vec<ImageBlock>, Vec<PathBuf>) {
    let mut native = Vec::new();
    let mut degraded = Vec::new();
    for path in paths {
        let mime = image_mime_for_path(path);
        if !mime_allowed(mime, whitelist) {
            degraded.push(path.clone());
            continue;
        }
        match std::fs::read(path) {
            Ok(bytes) => native.push(ImageBlock {
                data_base64: general_purpose::STANDARD.encode(bytes),
                mime: mime.to_string(),
                path: path.clone(),
            }),
            Err(_) => degraded.push(path.clone()),
        }
    }
    (native, degraded)
}

/// Codex 用：把原图 copy 成临时文件（前缀 `kivio-ext-img-`），返回临时文件路径列表。
/// Codex sandbox 锁 cwd，读不到会话附件目录，故与 Paseo 一样落一份临时文件传 `localImage` path。
/// 扁平文件（与 kivio 现有 `kivio-mcpimg-` 约定一致），靠 app 的 `cleanup_orphan_temp_files`
/// 按前缀回收；单张失败跳过。
pub fn materialize_images_to_tempdir(images: &[ImageBlock]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for img in images {
        let ext = img
            .path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png");
        let dest =
            std::env::temp_dir().join(format!("kivio-ext-img-{}.{ext}", uuid::Uuid::new_v4()));
        if std::fs::copy(&img.path, &dest).is_ok() {
            out.push(dest);
        }
    }
    out
}

/// 降级：把图片绝对路径拼成一段可追加到 prompt 的文本（协议不支持原生图片时用）。
/// 空输入返回空串。格式对齐 Paseo 的 `[Image available at: {path}]`。
pub fn image_paths_note(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\n# 附带图片（用你的读取工具查看）\n");
    for path in paths {
        out.push_str(&format!("[Image available at: {}]\n", path.display()));
    }
    out
}

/// 非图片文件说明块（所有协议通用，对齐 Paseo `uploaded_file`）：文件名/路径/MIME/大小。
/// 不 inline 内容，CLI 自读路径。空输入返回空串。
pub fn file_attachments_note(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\n# 附带文件（用你的读取工具查看）\n");
    for path in paths {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("attachment");
        let mime = file_mime_for_path(path);
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        out.push_str(&format!(
            "Attached file: {name}\nPath: {}\nMIME: {mime}\nSize: {size} bytes\n\n",
            path.display()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_for_path_covers_image_extensions() {
        assert_eq!(image_mime_for_path(Path::new("a.png")), "image/png");
        assert_eq!(image_mime_for_path(Path::new("a.JPG")), "image/jpeg");
        assert_eq!(image_mime_for_path(Path::new("a.webp")), "image/webp");
        assert_eq!(image_mime_for_path(Path::new("a.bin")), "image/png");
    }

    #[test]
    fn mime_allowed_empty_whitelist_allows_all() {
        assert!(mime_allowed("image/png", &[]));
        assert!(mime_allowed("image/tiff", &[]));
    }

    #[test]
    fn mime_allowed_respects_whitelist() {
        let wl = &["image/jpeg", "image/png"];
        assert!(mime_allowed("image/png", wl));
        assert!(!mime_allowed("image/bmp", wl));
    }

    #[test]
    fn load_blocks_empty_is_empty() {
        let (native, degraded) = load_image_blocks(&[], &[]);
        assert!(native.is_empty() && degraded.is_empty());
    }

    #[test]
    fn load_blocks_missing_file_degrades() {
        let (native, degraded) = load_image_blocks(&[PathBuf::from("/no/such/img.png")], &[]);
        assert!(native.is_empty());
        assert_eq!(degraded.len(), 1);
    }

    #[test]
    fn load_blocks_mime_outside_whitelist_degrades_without_reading() {
        // bmp not in whitelist → degraded even though the path doesn't exist (no read attempted).
        let (native, degraded) = load_image_blocks(
            &[PathBuf::from("/no/such/img.bmp")],
            &["image/jpeg", "image/png"],
        );
        assert!(native.is_empty());
        assert_eq!(degraded.len(), 1);
    }

    #[test]
    fn notes_empty_inputs_are_blank() {
        assert!(image_paths_note(&[]).is_empty());
        assert!(file_attachments_note(&[]).is_empty());
    }

    #[test]
    fn image_note_lists_paths() {
        let note = image_paths_note(&[PathBuf::from("/tmp/a.png")]);
        assert!(note.contains("/tmp/a.png"));
        assert!(note.contains("Image available at"));
    }

    #[test]
    fn file_note_has_name_and_path() {
        let note = file_attachments_note(&[PathBuf::from("/tmp/report.pdf")]);
        assert!(note.contains("Attached file: report.pdf"));
        assert!(note.contains("Path: /tmp/report.pdf"));
        assert!(note.contains("application/pdf"));
    }
}
