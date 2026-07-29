use std::{
    fs,
    path::Path,
    time::{Duration, SystemTime},
};

/**
 * 清理临时截图文件
 */
pub fn cleanup_temp_file(path: &Path) {
    let _ = fs::remove_file(path);
}

/// 启动时清理 temp_dir 下遗留的截图文件。
///
/// 我们在几个地方写图片文件到 temp_dir：
///   - `lens-<uuid>.png`（macOS SCK 整窗截图）
///   - `lens-region-<uuid>.png`（macOS SCK 区域截图）
///   - `screenshot-<uuid>.png`（Windows xcap）
///   - `kivio-mcpimg-<uuid>.<ext>`（R1：MCP 工具结果图片走辅助视觉模型分析前的
///     临时落盘，扩展名跟随实际 mime，正常路径分析完就地删除；这里兜底崩溃/
///     强杀残留）
///
/// 正常路径里 lens_close 会删除 active image_id 对应的文件，但以下情况会留 orphan：
///   - 应用被强杀 / 崩溃前来不及清
///   - 历史记录引用的旧 image_id（应用重启后历史里指针还在但文件不再被 active 引用）
///   - 之前版本（v2.2 及更早）的旧文件
///
/// 这里只删 24 小时之前的文件，避免误删可能正在被另一个 Kivio 实例使用的新文件。
pub fn cleanup_orphan_temp_files() {
    const PNG_PREFIXES: &[&str] = &["lens-", "lens-region-", "screenshot-"];
    const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

    let temp_dir = std::env::temp_dir();
    let entries = match fs::read_dir(&temp_dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("[orphan-cleanup] read_dir({:?}) err: {}", temp_dir, e);
            return;
        }
    };

    let now = SystemTime::now();
    let mut removed = 0u32;
    let mut bytes_freed = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        // Capture PNGs (lens/screenshot), MCP-image temp files (R1, any image
        // extension), and large-command output logs (kivio-bash-*.log) all live
        // in temp; GC either when stale. Background command logs
        // (kivio-bgcmd-*.log) are normally removed by the app-exit sweep, but a
        // crash can leave them behind — GC stale ones here too.
        let is_orphan = (PNG_PREFIXES.iter().any(|p| name.starts_with(p))
            && name.ends_with(".png"))
            || name.starts_with("kivio-mcpimg-")
            || name.starts_with("kivio-ext-img-")
            // 外部 CLI 的系统提示文件（`--append-system-prompt-file` 的载体）。正常路径下
            // 按 conversation_id 覆写、每会话最多一个，删掉的会话会留下残渣 —— 一并 GC。
            || (name.starts_with("kivio-extsys-") && name.ends_with(".md"))
            || (name.starts_with("kivio-bash-") && name.ends_with(".log"))
            || (name.starts_with(crate::native_tools::BG_CMD_LOG_PREFIX) && name.ends_with(".log"));
        if !is_orphan {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let modified = match metadata.modified() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let age = match now.duration_since(modified) {
            Ok(a) => a,
            Err(_) => continue, // 文件 mtime 在未来 → 跳过
        };
        if age < MAX_AGE {
            continue;
        }
        let size = metadata.len();
        if fs::remove_file(&path).is_ok() {
            removed += 1;
            bytes_freed += size;
        }
    }
    if removed > 0 {
        eprintln!(
            "[orphan-cleanup] removed {} stale screenshot file(s), freed {} KB",
            removed,
            bytes_freed / 1024
        );
    }
}
