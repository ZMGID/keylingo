/**
 * 全局快捷键默认值 — 与 `src-tauri/src/settings.rs` 的 default_*_hotkey 保持一致。
 * 「恢复默认」与后端冷启动默认共用此表，改一处需同步另一处。
 */
export const DEFAULT_HOTKEYS = {
  hotkey: 'CommandOrControl+Alt+T',
  chatHotkey: 'CommandOrControl+Shift+K',
  closeChatHotkey: 'CommandOrControl+Shift+W',
  screenshotHotkey: 'CommandOrControl+Shift+A',
  screenshotTextHotkey: 'CommandOrControl+Shift+T',
  screenshotReplaceHotkey: 'CommandOrControl+Shift+R',
  screenshotAnnotateHotkey: 'CommandOrControl+Shift+S',
  lensHotkey: 'CommandOrControl+Shift+G',
} as const
