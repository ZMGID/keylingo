//! 无 Tauri handle 的 per-app data 目录解析。
//!
//! 部分模块(himalaya 连接器、plugins、skills 的 headless 发现)需要在没有
//! `AppHandle` 的情况下定位 Kivio 的 per-app data 目录。这里提供与 Tauri
//! `app_data_dir` 完全一致的纯函数实现。

use std::path::PathBuf;

/// Tauri bundle identifier(须与 `tauri.conf.json` 一致)。
pub const APP_IDENTIFIER: &str = "com.zmair.kivio";

/// 与 Tauri `app_data_dir` = `dirs::data_dir()/<identifier>` 完全一致:
///   Windows: `%APPDATA%\com.zmair.kivio`(Roaming,无 `\data` 子目录)
///   macOS:   `~/Library/Application Support/com.zmair.kivio`
///   Linux:   `$XDG_DATA_HOME/com.zmair.kivio`(或 `~/.local/share/...`)
///
/// 注意:必须用 `BaseDirs::data_dir()`,不能用 `ProjectDirs::data_dir()`——
/// 后者在 Windows 会多追加一层 `\data`,导致读到错误路径。
/// 无法确定 home/data 目录时返回 `None`。
pub fn app_data_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|base| base.data_dir().join(APP_IDENTIFIER))
}
