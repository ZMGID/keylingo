use std::{collections::HashSet, sync::atomic::Ordering, time::Duration};

use arboard::Clipboard;
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use crate::commands::apply_launch_at_startup;
use crate::lens_commands::{
    lens_close, lens_request, lens_request_replace, lens_request_screenshot,
    lens_request_translate, lens_request_translate_text, request_lens_close,
};
use crate::settings::Settings;
use crate::state::AppState;
use crate::windows::{
    apply_chat_window_chrome, apply_frameless_window_chrome, ensure_chat_window,
    ensure_chat_window_with_hash, ensure_main_window, normalize_chat_window_behavior,
};
#[cfg(target_os = "macos")]
use crate::windows::{
    apply_macos_traffic_light_position, ensure_overlay_panel, forget_frontmost_app,
    reassert_previous_frontmost_app, refocus_overlay_after_frontmost_reassert,
    remember_frontmost_app, restore_previous_frontmost_app, show_overlay_panel,
};

/// 模拟一次 Cmd+C(macOS)/Ctrl+C(Windows)。
/// 用于 Lens 启动时把前台 App 的选中文本拷进剪贴板。
/// macOS：直接走 CGEvent（不走 AppleScript），用 Private state source 避免与用户当前
/// 仍按住的热键修饰键(Cmd/Shift/Option)合并出 Cmd+Shift+C 之类的组合。
fn send_copy_shortcut() {
    #[cfg(target_os = "macos")]
    {
        if !check_accessibility(true) {
            eprintln!("[lens-capture] Accessibility permission missing for copy shortcut");
            return;
        }
        use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
        use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

        let source = match CGEventSource::new(CGEventSourceStateID::Private) {
            Ok(s) => s,
            Err(_) => {
                eprintln!("[lens-capture] CGEventSource::new(Private) failed");
                return;
            }
        };

        // ANSI 'c' = keycode 8
        const KEY_C: core_graphics::event::CGKeyCode = 8;
        let down = match CGEvent::new_keyboard_event(source.clone(), KEY_C, true) {
            Ok(ev) => ev,
            Err(_) => {
                eprintln!("[lens-capture] CGEvent::new_keyboard_event(down) failed");
                return;
            }
        };
        down.set_flags(CGEventFlags::CGEventFlagCommand);
        down.post(CGEventTapLocation::HID);

        let up = match CGEvent::new_keyboard_event(source, KEY_C, false) {
            Ok(ev) => ev,
            Err(_) => {
                eprintln!("[lens-capture] CGEvent::new_keyboard_event(up) failed");
                return;
            }
        };
        up.set_flags(CGEventFlags::CGEventFlagCommand);
        up.post(CGEventTapLocation::HID);
    }
    #[cfg(target_os = "windows")]
    {
        use enigo::{Enigo, Key, KeyboardControllable};
        let mut enigo = Enigo::new();
        enigo.key_down(Key::Control);
        enigo.key_click(Key::Layout('c'));
        enigo.key_up(Key::Control);
    }
}

/// Accessibility 选区读取的三态结果。
/// - `Text(s)`：AX 取到非空选区。
/// - `Empty`：AX 可用且确认当前没有选区（元素支持选区属性但内容为空），
///   据此可以直接判定无选区，**跳过 Cmd+C 兜底**（消除空选区时原生 App 的系统提示音）。
/// - `Unavailable`：AX 无权限 / 无 focused element / 元素不支持该属性 / 其它错误——
///   无法判定，保留 Cmd+C 兜底（浏览器/Electron/终端等不暴露 AX 的 App，行为不变）。
enum AxSelection {
    Text(String),
    Empty,
    Unavailable,
}

/// macOS: 直接从当前前台控件读取 Accessibility selected text。
/// 这条路径不碰剪贴板，也不受 Lens 热键仍按住的 Cmd/Shift/G 干扰。
#[cfg(target_os = "macos")]
fn read_accessibility_selected_text() -> AxSelection {
    if !check_accessibility(false) {
        eprintln!("[lens-capture] AX unavailable: accessibility permission missing");
        return AxSelection::Unavailable;
    }

    use core_foundation::{
        base::{CFRelease, CFType, CFTypeRef, TCFType},
        string::{CFString, CFStringRef},
    };

    type AXUIElementRef = *const libc::c_void;
    type AXError = i32;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateSystemWide() -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> AXError;
    }

    const AX_ERROR_SUCCESS: AXError = 0;
    // kAXErrorNoValue：元素支持该属性，但当前没有值（即确认无选区）。
    const AX_ERROR_NO_VALUE: AXError = -25212;

    unsafe {
        let system = AXUIElementCreateSystemWide();
        if system.is_null() {
            eprintln!("[lens-capture] AX unavailable: system-wide element null");
            return AxSelection::Unavailable;
        }

        let focused_attr = CFString::new("AXFocusedUIElement");
        let mut focused_ref: CFTypeRef = std::ptr::null();
        let focused_err = AXUIElementCopyAttributeValue(
            system,
            focused_attr.as_concrete_TypeRef(),
            &mut focused_ref,
        );
        CFRelease(system as CFTypeRef);
        if focused_err != AX_ERROR_SUCCESS || focused_ref.is_null() {
            eprintln!("[lens-capture] AX unavailable: no focused element (err={focused_err})");
            return AxSelection::Unavailable;
        }
        let focused = CFType::wrap_under_create_rule(focused_ref);

        let selected_attr = CFString::new("AXSelectedText");
        let mut selected_ref: CFTypeRef = std::ptr::null();
        let selected_err = AXUIElementCopyAttributeValue(
            focused.as_CFTypeRef() as AXUIElementRef,
            selected_attr.as_concrete_TypeRef(),
            &mut selected_ref,
        );

        // 元素支持该属性但当前无值（无选区）→ 确认无选区。
        if selected_err == AX_ERROR_NO_VALUE {
            eprintln!("[lens-capture] AX confirmed empty selection (kAXErrorNoValue)");
            return AxSelection::Empty;
        }

        if selected_err != AX_ERROR_SUCCESS || selected_ref.is_null() {
            // 元素不支持该属性 / 其它错误 / 空指针 → 无法判定，落 Cmd+C 兜底。
            eprintln!("[lens-capture] AX unavailable: AXSelectedText err={selected_err}");
            return AxSelection::Unavailable;
        }

        let selected = CFType::wrap_under_create_rule(selected_ref);
        match selected.downcast_into::<CFString>() {
            Some(cf) => {
                let text = cf.to_string();
                if text.trim().is_empty() {
                    eprintln!("[lens-capture] AX confirmed empty selection (empty AXSelectedText)");
                    AxSelection::Empty
                } else {
                    AxSelection::Text(text)
                }
            }
            None => {
                eprintln!("[lens-capture] AX unavailable: AXSelectedText not a CFString");
                AxSelection::Unavailable
            }
        }
    }
}

/// Windows: 通过 UI Automation TextPattern 直接读取当前前台控件的选区。
/// 这条路径不碰剪贴板；不支持 TextPattern 的控件会自动降级到 Ctrl+C fallback。
/// 三态语义（与 macOS 分支对齐）：
///   - TextPattern 明确可用（GetCurrentPattern + cast + GetSelection 均成功）且收集到的非空文本为空
///     → `Empty`（确认无选区，上层跳过 Ctrl+C → 原生输入框不再响）。
///   - 任一环节失败（CoCreateInstance / GetFocusedElement / GetCurrentPattern / cast / GetSelection 等）
///     → `Unavailable`（保持 Ctrl+C 兜底，不碰浏览器/Electron 等不支持 TextPattern 的路径）。
///   - 有非空选区 → `Text`。
/// 保守原则：任何不确定/失败一律 `Unavailable`，只有 TextPattern 明确可用且选区确为空才 `Empty`，
/// 避免 UIA underreport 时漏抓真实选区。
#[cfg(target_os = "windows")]
fn read_accessibility_selected_text() -> AxSelection {
    use ::windows::{
        core::Interface,
        Win32::{
            Foundation::RPC_E_CHANGED_MODE,
            System::Com::{
                CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
                COINIT_APARTMENTTHREADED,
            },
            UI::Accessibility::{
                CUIAutomation, IUIAutomation, IUIAutomationTextPattern, UIA_TextPatternId,
            },
        },
    };

    unsafe {
        let init_result = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if init_result.is_err() && init_result != RPC_E_CHANGED_MODE {
            eprintln!("[lens-capture] CoInitializeEx failed: {init_result:?}");
            return AxSelection::Unavailable;
        }
        let should_uninitialize = init_result.is_ok();

        // closure 直接产出三态 AxSelection：
        // 拿不到 TextPattern / GetSelection 失败 → Unavailable；
        // TextPattern 明确可用且选区确为空 → Empty；否则 → Text。
        let result = (|| -> AxSelection {
            let automation: IUIAutomation =
                match CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
                    Ok(a) => a,
                    Err(e) => {
                        eprintln!("[lens-capture] UIA unavailable: CoCreateInstance failed: {e:?}");
                        return AxSelection::Unavailable;
                    }
                };
            let focused = match automation.GetFocusedElement() {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("[lens-capture] UIA unavailable: GetFocusedElement failed: {e:?}");
                    return AxSelection::Unavailable;
                }
            };
            // GetCurrentPattern + cast 任一失败 → 控件不支持 TextPattern（如浏览器/Electron）→ 兜底。
            let pattern_unknown = match focused.GetCurrentPattern(UIA_TextPatternId) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!(
                        "[lens-capture] UIA unavailable: GetCurrentPattern(Text) failed: {e:?}"
                    );
                    return AxSelection::Unavailable;
                }
            };
            let pattern: IUIAutomationTextPattern = match pattern_unknown.cast() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("[lens-capture] UIA unavailable: TextPattern cast failed: {e:?}");
                    return AxSelection::Unavailable;
                }
            };
            // 到这里 TextPattern 明确可用；GetSelection 失败仍视为无法判定 → 兜底。
            let ranges = match pattern.GetSelection() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("[lens-capture] UIA unavailable: GetSelection failed: {e:?}");
                    return AxSelection::Unavailable;
                }
            };
            let count = ranges.Length().unwrap_or(0).max(0);
            let mut parts = Vec::new();

            for index in 0..count {
                let range = match ranges.GetElement(index) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                let text = match range.GetText(-1) {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                let text = text.to_string();
                if !text.trim().is_empty() {
                    parts.push(text);
                }
            }

            if parts.is_empty() {
                // TextPattern 明确可用 + 选区确为空 → Empty（上层跳过 Ctrl+C → 不响）。
                eprintln!("[lens-capture] UIA confirmed empty selection (TextPattern available)");
                AxSelection::Empty
            } else {
                AxSelection::Text(parts.join("\n"))
            }
        })();

        if should_uninitialize {
            CoUninitialize();
        }

        if let AxSelection::Text(ref text) = result {
            eprintln!(
                "[lens-capture] UIA selected text captured len={}",
                text.len()
            );
        }

        result
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn read_accessibility_selected_text() -> AxSelection {
    AxSelection::Unavailable
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn clipboard_change_count() -> Option<i64> {
    use cocoa::{
        appkit::NSPasteboard,
        base::{id, nil},
    };
    unsafe {
        let pasteboard = <id as NSPasteboard>::generalPasteboard(nil);
        if pasteboard == nil {
            None
        } else {
            Some(pasteboard.changeCount() as i64)
        }
    }
}

#[cfg(target_os = "windows")]
fn clipboard_change_count() -> Option<i64> {
    use ::windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
    let count = unsafe { GetClipboardSequenceNumber() };
    if count == 0 {
        None
    } else {
        Some(i64::from(count))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn clipboard_change_count() -> Option<i64> {
    None
}

#[cfg(target_os = "macos")]
fn wait_for_copy_shortcut_modifiers_to_clear(timeout: Duration) {
    use core_graphics::{event::CGEventFlags, event_source::CGEventSourceStateID};
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceFlagsState(state_id: CGEventSourceStateID) -> u64;
    }

    let mask = CGEventFlags::CGEventFlagShift.bits()
        | CGEventFlags::CGEventFlagControl.bits()
        | CGEventFlags::CGEventFlagAlternate.bits()
        | CGEventFlags::CGEventFlagCommand.bits();
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        let flags = unsafe { CGEventSourceFlagsState(CGEventSourceStateID::CombinedSessionState) };
        if flags & mask == 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(target_os = "windows")]
fn wait_for_copy_shortcut_modifiers_to_clear(timeout: Duration) {
    use ::windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };

    let keys = [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN];
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        let pressed = keys
            .iter()
            .any(|key| unsafe { (GetAsyncKeyState(key.0 as i32) as u16 & 0x8000) != 0 });
        if !pressed {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn wait_for_copy_shortcut_modifiers_to_clear(timeout: Duration) {
    std::thread::sleep(timeout.min(Duration::from_millis(120)));
}

/// 在前一个 App 仍持焦点时把选中文本读出来，失败时才模拟 Cmd+C/Ctrl+C 兜底。
/// 失败/Accessibility 权限缺失/剪贴板为非文本格式 → 一律静默降级返回 None。
/// 调用方负责确保此函数在 Lens 窗口 show() 之前执行。
pub(crate) fn capture_active_selection() -> Option<String> {
    match read_accessibility_selected_text() {
        AxSelection::Text(text) => {
            eprintln!(
                "[lens-capture] selected text captured via Accessibility len={}",
                text.len()
            );
            return Some(text);
        }
        AxSelection::Empty => {
            // AX 确认当前无选区：直接判定无选区，跳过 Cmd+C 兜底，避免空选区时原生 App 的系统提示音。
            eprintln!("[lens-capture] AX confirmed no selection, skipping Cmd+C fallback");
            return None;
        }
        AxSelection::Unavailable => {
            // AX 无法判定：继续走下面的剪贴板 snapshot + Cmd+C 兜底逻辑。
        }
    }

    // snapshot 原剪贴板文本(仅 text)。若是图片/文件/空，snapshot=None，事后不还原。
    let snapshot: Option<String> = Clipboard::new().ok().and_then(|mut cb| cb.get_text().ok());
    let before_change_count = clipboard_change_count();
    eprintln!(
        "[lens-capture] snapshot present={} len={} change_count={:?}",
        snapshot.is_some(),
        snapshot.as_ref().map(|s| s.len()).unwrap_or(0),
        before_change_count,
    );

    // 等用户松开 Lens 热键修饰键，避免 Cmd+C 与残留 Shift 等组合成 Cmd+Shift+C。
    wait_for_copy_shortcut_modifiers_to_clear(Duration::from_millis(450));
    send_copy_shortcut();
    std::thread::sleep(Duration::from_millis(150));

    let captured: Option<String> = Clipboard::new().ok().and_then(|mut cb| cb.get_text().ok());
    let text_changed = match (&snapshot, &captured) {
        (Some(a), Some(b)) => a != b,
        (None, Some(_)) => true,
        _ => false,
    };
    let after_change_count = clipboard_change_count();
    let pasteboard_changed = match (before_change_count, after_change_count) {
        (Some(before), Some(after)) => before != after,
        _ => false,
    };
    eprintln!(
    "[lens-capture] captured present={} len={} text_changed={} pasteboard_changed={} change_count={:?}",
    captured.is_some(),
    captured.as_ref().map(|s| s.len()).unwrap_or(0),
    text_changed,
    pasteboard_changed,
    after_change_count,
  );

    if let Some(orig) = &snapshot {
        if let Ok(mut cb) = Clipboard::new() {
            let _ = cb.set_text(orig.clone());
        }
    }

    // pasteboard changeCount 覆盖"选中文本与原剪贴板文本完全相同"的情况。
    if !text_changed && !pasteboard_changed {
        return None;
    }
    match captured {
        Some(t) if !t.trim().is_empty() => Some(t),
        _ => None,
    }
}

/// 热键注册错误的种类。序列化为 snake_case 字符串,前端按它查 i18n 模板。
#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum HotkeyErrorKind {
    /// 系统层冲突：被其他应用或系统占用,OS 拒绝注册
    Conflict,
    /// 应用内重复：用户把同一个组合分配给了多个功能
    Duplicate,
    /// 其他注册失败(网络/权限/未知错误)
    Other,
}

/// 热键所属的功能范围。前端按它查"翻译器"/"截图翻译"等本地化名称。
#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum HotkeyScope {
    Translator,
    Chat,
    CloseChat,
    Screenshot,
    ScreenshotText,
    ScreenshotReplace,
    ScreenshotAnnotate,
    Lens,
    Automation,
}

/// 单条热键注册错误。会被收集成 `Vec<HotkeyError>` 并 JSON 序列化作为 `register_hotkeys`
/// 的错误返回 — 前端 `Settings.tsx` `JSON.parse` 后按用户语言渲染。
#[derive(serde::Serialize)]
struct HotkeyError {
    kind: HotkeyErrorKind,
    scope: HotkeyScope,
    hotkey: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw: Option<String>,
}

/// 把 `register_hotkeys` 返回的 JSON 错误字符串还原成人类可读的英文,
/// 供 main.rs 启动 / shortcuts.rs rollback 这种只能 eprintln 的场景使用。
/// JSON 解析失败时直接原样返回。
pub(crate) fn display_hotkey_errors(json_str: &str) -> String {
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(json_str) else {
        return json_str.to_string();
    };
    items
        .iter()
        .map(|e| {
            let kind = e.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let scope = e.get("scope").and_then(|v| v.as_str()).unwrap_or("");
            let hotkey = e.get("hotkey").and_then(|v| v.as_str()).unwrap_or("");
            let raw = e.get("raw").and_then(|v| v.as_str()).unwrap_or("");
            match kind {
                "conflict" => {
                    format!("Hotkey conflict for {scope}: \"{hotkey}\" is already in use")
                }
                "duplicate" => format!("Duplicate hotkey \"{hotkey}\" for {scope}"),
                "empty" => format!("{scope} hotkey is empty"),
                _ => format!("Failed to register {scope} hotkey \"{hotkey}\": {raw}"),
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn classify_hotkey_error(scope: HotkeyScope, hotkey: String, raw: String) -> HotkeyError {
    let normalized = raw.to_lowercase();
    let is_conflict = normalized.contains("already registered")
        || normalized.contains("already in use")
        || (normalized.contains("hotkey") && normalized.contains("registered"));
    HotkeyError {
        kind: if is_conflict {
            HotkeyErrorKind::Conflict
        } else {
            HotkeyErrorKind::Other
        },
        scope,
        hotkey,
        raw: if is_conflict { None } else { Some(raw) },
    }
}

/// 注册全局热键
/// 包括翻译热键、截图翻译热键、lens 热键；检测重复热键并把错误以结构化形式收集,
/// JSON 序列化后由前端按界面语言渲染。
pub(crate) fn register_hotkeys(app: &AppHandle) -> Result<(), String> {
    let settings = app.state::<AppState>().settings_read().clone();
    let shortcut_manager = app.global_shortcut();
    shortcut_manager
        .unregister_all()
        .map_err(|e| e.to_string())?;
    let mut errors: Vec<HotkeyError> = Vec::new();
    let mut registered = HashSet::new();

    if !settings.hotkey.trim().is_empty() {
        let hotkey = settings.hotkey.trim().to_string();
        let hotkey_key = hotkey.to_lowercase();
        if !registered.insert(hotkey_key) {
            errors.push(HotkeyError {
                kind: HotkeyErrorKind::Duplicate,
                scope: HotkeyScope::Translator,
                hotkey: hotkey.clone(),
                raw: None,
            });
        } else if let Err(err) =
            shortcut_manager.on_shortcut(hotkey.as_str(), move |app, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    toggle_main_window(app);
                }
            })
        {
            errors.push(classify_hotkey_error(
                HotkeyScope::Translator,
                hotkey,
                err.to_string(),
            ));
        }
    }

    if !settings.chat_hotkey.trim().is_empty() {
        let hotkey = settings.chat_hotkey.trim().to_string();
        let hotkey_key = hotkey.to_lowercase();
        if !registered.insert(hotkey_key) {
            errors.push(HotkeyError {
                kind: HotkeyErrorKind::Duplicate,
                scope: HotkeyScope::Chat,
                hotkey: hotkey.clone(),
                raw: None,
            });
        } else if let Err(err) =
            shortcut_manager.on_shortcut(hotkey.as_str(), move |app, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    if let Err(err) = open_chat_window(app) {
                        eprintln!("Chat hotkey trigger error: {err}");
                    }
                }
            })
        {
            errors.push(classify_hotkey_error(
                HotkeyScope::Chat,
                hotkey,
                err.to_string(),
            ));
        }
    }

    if !settings.close_chat_hotkey.trim().is_empty() {
        let hotkey = settings.close_chat_hotkey.trim().to_string();
        let hotkey_key = hotkey.to_lowercase();
        if !registered.insert(hotkey_key) {
            errors.push(HotkeyError {
                kind: HotkeyErrorKind::Duplicate,
                scope: HotkeyScope::CloseChat,
                hotkey: hotkey.clone(),
                raw: None,
            });
        } else if let Err(err) =
            shortcut_manager.on_shortcut(hotkey.as_str(), move |app, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    close_chat_window(app);
                }
            })
        {
            errors.push(classify_hotkey_error(
                HotkeyScope::CloseChat,
                hotkey,
                err.to_string(),
            ));
        }
    }

    if settings.screenshot_translation.enabled {
        let hotkey = settings.screenshot_translation.hotkey.trim().to_string();
        if !hotkey.is_empty() {
            let hotkey_key = hotkey.to_lowercase();
            if !registered.insert(hotkey_key) {
                errors.push(HotkeyError {
                    kind: HotkeyErrorKind::Duplicate,
                    scope: HotkeyScope::Screenshot,
                    hotkey: hotkey.clone(),
                    raw: None,
                });
            } else if let Err(err) =
                shortcut_manager.on_shortcut(hotkey.as_str(), move |app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        // 切换行为：Lens 可见时关闭，不可见时打开截图翻译
                        if lens_is_active(app) {
                            let _ = request_lens_close(app);
                        } else {
                            let handle = app.clone();
                            tauri::async_runtime::spawn(async move {
                                if let Err(err) = lens_request_translate(handle) {
                                    eprintln!("Screenshot translation trigger error: {err}");
                                }
                            });
                        }
                    }
                })
            {
                errors.push(classify_hotkey_error(
                    HotkeyScope::Screenshot,
                    hotkey,
                    err.to_string(),
                ));
            }
        }

        let text_hotkey = settings
            .screenshot_translation
            .text_hotkey
            .trim()
            .to_string();
        if !text_hotkey.is_empty() {
            let hotkey_key = text_hotkey.to_lowercase();
            if !registered.insert(hotkey_key) {
                errors.push(HotkeyError {
                    kind: HotkeyErrorKind::Duplicate,
                    scope: HotkeyScope::ScreenshotText,
                    hotkey: text_hotkey.clone(),
                    raw: None,
                });
            } else if let Err(err) =
                shortcut_manager.on_shortcut(text_hotkey.as_str(), move |app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        if lens_is_active(app) {
                            let _ = request_lens_close(app);
                        } else {
                            let handle = app.clone();
                            tauri::async_runtime::spawn(async move {
                                if let Err(err) = lens_request_translate_text(handle) {
                                    eprintln!(
                                        "Selected text screenshot translation trigger error: {err}"
                                    );
                                }
                            });
                        }
                    }
                })
            {
                errors.push(classify_hotkey_error(
                    HotkeyScope::ScreenshotText,
                    text_hotkey,
                    err.to_string(),
                ));
            }
        }

        if settings.screenshot_translation.replace_enabled {
            let replace_hotkey = settings
                .screenshot_translation
                .replace_hotkey
                .trim()
                .to_string();
            if !replace_hotkey.is_empty() {
                let hotkey_key = replace_hotkey.to_lowercase();
                if !registered.insert(hotkey_key) {
                    errors.push(HotkeyError {
                        kind: HotkeyErrorKind::Duplicate,
                        scope: HotkeyScope::ScreenshotReplace,
                        hotkey: replace_hotkey.clone(),
                        raw: None,
                    });
                } else if let Err(err) = shortcut_manager.on_shortcut(
                    replace_hotkey.as_str(),
                    move |app, _shortcut, event| {
                        if event.state == ShortcutState::Pressed {
                            if lens_is_active(app) {
                                let _ = request_lens_close(app);
                            } else {
                                let handle = app.clone();
                                tauri::async_runtime::spawn(async move {
                                    if let Err(err) = lens_request_replace(handle) {
                                        eprintln!("Replace translation trigger error: {err}");
                                    }
                                });
                            }
                        }
                    },
                ) {
                    errors.push(classify_hotkey_error(
                        HotkeyScope::ScreenshotReplace,
                        replace_hotkey,
                        err.to_string(),
                    ));
                }
            }
        }
    }

    if settings.screenshot_annotate.enabled {
        let hotkey = settings.screenshot_annotate.hotkey.trim().to_string();
        if !hotkey.is_empty() {
            let hotkey_key = hotkey.to_lowercase();
            if !registered.insert(hotkey_key) {
                errors.push(HotkeyError {
                    kind: HotkeyErrorKind::Duplicate,
                    scope: HotkeyScope::ScreenshotAnnotate,
                    hotkey: hotkey.clone(),
                    raw: None,
                });
            } else if let Err(err) =
                shortcut_manager.on_shortcut(hotkey.as_str(), move |app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        if lens_is_active(app) {
                            let _ = request_lens_close(app);
                        } else {
                            let handle = app.clone();
                            tauri::async_runtime::spawn(async move {
                                if let Err(err) = lens_request_screenshot(handle) {
                                    eprintln!("Screenshot annotate trigger error: {err}");
                                }
                            });
                        }
                    }
                })
            {
                errors.push(classify_hotkey_error(
                    HotkeyScope::ScreenshotAnnotate,
                    hotkey,
                    err.to_string(),
                ));
            }
        }
    }

    if settings.lens.enabled {
        let hotkey = settings.lens.hotkey.trim().to_string();
        if !hotkey.is_empty() {
            let hotkey_key = hotkey.to_lowercase();
            if !registered.insert(hotkey_key) {
                errors.push(HotkeyError {
                    kind: HotkeyErrorKind::Duplicate,
                    scope: HotkeyScope::Lens,
                    hotkey: hotkey.clone(),
                    raw: None,
                });
            } else if let Err(err) =
                shortcut_manager.on_shortcut(hotkey.as_str(), move |app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        // 切换行为：Lens 可见时关闭，不可见时打开
                        if lens_is_active(app) {
                            let _ = request_lens_close(app);
                        } else {
                            let handle = app.clone();
                            tauri::async_runtime::spawn(async move {
                                if let Err(err) = lens_request(handle) {
                                    eprintln!("Lens trigger error: {err}");
                                }
                            });
                        }
                    }
                })
            {
                errors.push(classify_hotkey_error(
                    HotkeyScope::Lens,
                    hotkey,
                    err.to_string(),
                ));
            }
        }
    }

    for (hotkey, automation_id) in crate::automation::enabled_bindings(app) {
        let hotkey_key = hotkey.to_lowercase();
        if !registered.insert(hotkey_key) {
            errors.push(HotkeyError {
                kind: HotkeyErrorKind::Duplicate,
                scope: HotkeyScope::Automation,
                hotkey: hotkey.clone(),
                raw: None,
            });
            continue;
        }
        if let Err(err) =
            shortcut_manager.on_shortcut(hotkey.as_str(), move |app, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    let app = app.clone();
                    let id = automation_id.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(err) =
                            crate::automation::enqueue(app, id, crate::automation::RunOrigin::Hotkey, None)
                        {
                            eprintln!("automation hotkey: {err}");
                        }
                    });
                }
            })
        {
            errors.push(classify_hotkey_error(
                HotkeyScope::Automation,
                hotkey,
                err.to_string(),
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        // 序列化失败几乎不可能(字段都是 String/枚举),fallback 给一个最小可读消息
        Err(serde_json::to_string(&errors)
            .unwrap_or_else(|_| "Hotkey registration failed".to_string()))
    }
}

/// 获取当前鼠标位置
pub(crate) fn get_mouse_position(app: &AppHandle) -> Option<tauri::PhysicalPosition<f64>> {
    app.cursor_position().ok()
}

/// 切换输入翻译窗口。
/// 可见时关闭销毁 main WebView；显示时跟随鼠标位置偏移 (10,10) 弹出，翻译器保持置顶。
pub(crate) fn toggle_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) {
            #[cfg(target_os = "macos")]
            {
                crate::windows::destroy_overlay_window(&window);
                let st = app.state::<AppState>();
                restore_previous_frontmost_app(app, &st.prev_frontmost_pid_main);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = window.close();
            return;
        }
    }

    #[cfg(target_os = "macos")]
    {
        let st = app.state::<AppState>();
        remember_frontmost_app(&st.prev_frontmost_pid_main);
    }

    let window = match ensure_main_window(app) {
        Ok(window) => window,
        Err(err) => {
            #[cfg(target_os = "macos")]
            {
                let st = app.state::<AppState>();
                restore_previous_frontmost_app(app, &st.prev_frontmost_pid_main);
            }
            eprintln!("Failed to ensure main window: {}", err);
            return;
        }
    };

    #[cfg(not(target_os = "macos"))]
    let _ = window.set_always_on_top(true);
    #[cfg(target_os = "macos")]
    {
        ensure_overlay_panel(&window);
        // ensure_main_window 的冷创建若短暂激活了 Kivio，在显示非激活 Panel 前立刻纠正；
        // 不触碰 Chat 窗口本身。
        let st = app.state::<AppState>();
        reassert_previous_frontmost_app(app, &st.prev_frontmost_pid_main);
    }

    // 重置 hash 为翻译模式；main 现在只承载输入翻译。
    let _ = window.eval(
        "window.location.hash = ''; window.dispatchEvent(new HashChangeEvent('hashchange'));",
    );

    let pos = get_mouse_position(app).map(|cursor| {
        tauri::PhysicalPosition::new((cursor.x + 10.0) as i32, (cursor.y + 10.0) as i32)
    });

    #[cfg(target_os = "macos")]
    {
        let window_for_task = window.clone();
        let app_for_task = app.clone();
        let _ = window.run_on_main_thread(move || {
            if let Some(pos) = pos {
                if let Err(e) = window_for_task.set_position(pos) {
                    eprintln!("Failed to set window position: {}", e);
                }
            } else {
                eprintln!("Failed to get mouse position");
            }
            // 非激活 panel：need_key=true 让翻译输入框接收键盘，但不激活 app、不切 Space。
            show_overlay_panel(&window_for_task, true);
            // 某些 macOS/tao 组合即便已带 NonactivatingPanel tag，冷创建后的首次
            // makeKeyWindow 仍会激活宿主 App；显示后再校正一次，确保普通 Chat 不被带到前面。
            let st = app_for_task.state::<AppState>();
            reassert_previous_frontmost_app(&app_for_task, &st.prev_frontmost_pid_main);
            refocus_overlay_after_frontmost_reassert(&window_for_task);
        });
        return;
    }

    #[cfg(not(target_os = "macos"))]
    {
        if let Some(pos) = pos {
            if let Err(e) = window.set_position(pos) {
                eprintln!("Failed to set window position: {}", e);
            }
        } else {
            eprintln!("Failed to get mouse position");
        }
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// 恢复运行时设置
/// 当保存设置失败时，将设置、热键、托盘等回滚到之前的状态
pub(crate) fn restore_runtime_settings(
    app: &AppHandle,
    state: &State<AppState>,
    previous: &Settings,
) {
    if let Err(err) = apply_launch_at_startup(app, previous.launch_at_startup) {
        eprintln!("Failed to rollback launch-at-startup setting: {err}");
    }

    {
        let mut guard = state.settings_write();
        *guard = previous.clone();
    }
    state
        .sub_agents
        .set_concurrency(previous.chat_tools.sub_agent_concurrency);

    if let Err(err) = register_hotkeys(app) {
        eprintln!(
            "Failed to rollback hotkeys: {}",
            display_hotkey_errors(&err)
        );
    }

    if let Err(err) = setup_tray(app) {
        eprintln!("Failed to rollback tray: {err}");
    }
}

/// 接收前端合成的带箭头标注 PNG（base64 编码），落盘到 temp_dir、注册新 image_id。
/// 不再次归档:归档目录里只保留 capture 时的原图,合成版只活在 temp_dir。
/// 原 image_id 对应的临时文件在切到新 image_id 后立即清理，避免同一会话里堆积 orphan。

/// macOS 平台：检查辅助功能权限
/// 如果 open_if_needed 为 true 且未授权，则自动打开系统设置面板
#[cfg(target_os = "macos")]
pub(crate) fn check_accessibility(open_if_needed: bool) -> bool {
    use std::process::Command;
    unsafe {
        #[link(name = "ApplicationServices", kind = "framework")]
        extern "C" {
            fn AXIsProcessTrustedWithOptions(options: *mut libc::c_void) -> bool;
        }

        // 先进行简单检查（不传入选项）
        if AXIsProcessTrustedWithOptions(std::ptr::null_mut()) {
            return true;
        }

        if open_if_needed {
            // 直接打开系统设置，而不是尝试通过 FFI 触发授权弹窗
            eprintln!("Accessibility not trusted, opening preferences...");
            let _ = Command::new("open")
                .arg(
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                )
                .output();
        }
        false
    }
}

/// macOS 平台：检查屏幕录制权限
#[cfg(target_os = "macos")]
pub(crate) fn check_screen_recording_permission() -> bool {
    unsafe {
        #[link(name = "ApplicationServices", kind = "framework")]
        extern "C" {
            fn CGPreflightScreenCaptureAccess() -> bool;
        }
        CGPreflightScreenCaptureAccess()
    }
}

/// 发送粘贴快捷键到当前活动应用
/// macOS 通过 AppleScript 发送 Command+V；Windows 通过 enigo 模拟 Ctrl+V
pub(crate) fn send_paste_shortcut() {
    #[cfg(target_os = "macos")]
    {
        if !check_accessibility(true) {
            eprintln!("Accessibility permission missing!");
            return;
        }

        use std::process::Command;
        eprintln!("Sending Paste Shortcut via AppleScript...");
        match Command::new("osascript")
            .arg("-e")
            .arg("tell application \"System Events\" to keystroke \"v\" using command down")
            .output()
        {
            Ok(output) => {
                if !output.status.success() {
                    eprintln!(
                        "AppleScript failed: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                } else {
                    eprintln!("AppleScript success");
                }
            }
            Err(e) => eprintln!("Failed to execute AppleScript: {}", e),
        }
    }
    #[cfg(target_os = "windows")]
    {
        use enigo::{Enigo, Key, KeyboardControllable};
        let mut enigo = Enigo::new();
        enigo.key_down(Key::Control);
        enigo.key_click(Key::Layout('v'));
        enigo.key_up(Key::Control);
    }
}

/// 恢复并聚焦已有 Chat 窗口。
fn reveal_chat_window(app: &AppHandle, window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    set_macos_regular_activation_policy(app);

    if window.is_minimized().ok().unwrap_or(false) {
        let _ = window.unminimize();
    }

    let _ = window.show();
    let _ = window.set_focus();

    #[cfg(target_os = "macos")]
    apply_macos_traffic_light_position(window);
}

/// Show the app in the macOS Dock. In a debug build Tauri runs the bare Cargo
/// executable instead of an app bundle, so recreating the Dock tile after an
/// Accessory -> Regular transition loses the configured bundle icon. Restore
/// it explicitly from the icon embedded in the binary.
#[cfg(target_os = "macos")]
pub(crate) fn set_macos_regular_activation_policy(app: &AppHandle) {
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);

    #[cfg(debug_assertions)]
    restore_macos_development_app_icon();
}

#[cfg(all(target_os = "macos", debug_assertions))]
#[allow(deprecated, unexpected_cfgs)]
fn restore_macos_development_app_icon() {
    use cocoa::base::{id, nil};
    use objc::{class, msg_send, sel, sel_impl};

    static ICON_BYTES: &[u8] = include_bytes!("../icons/icon.png");

    unsafe {
        let data: id = msg_send![
            class!(NSData),
            dataWithBytes: ICON_BYTES.as_ptr()
            length: ICON_BYTES.len()
        ];
        let image: id = msg_send![class!(NSImage), alloc];
        let image: id = msg_send![image, initWithData: data];
        if image != nil {
            let ns_app: id = msg_send![class!(NSApplication), sharedApplication];
            let _: () = msg_send![ns_app, setApplicationIconImage: image];
            let _: () = msg_send![image, release];
        }
    }
}

/// 关闭独立 AI 客户端窗口（与点右上角 × 一致：默认销毁，开启保持后台时隐藏）。
pub(crate) fn close_chat_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("chat") {
        let _ = window.close();
    }
}

/// 隐藏 chat 窗口并回收 Dock 身份，WebView 进程保留以便下次立刻复用。
pub(crate) fn hide_chat_window(app: &AppHandle, window: &tauri::Window) {
    let _ = window.hide();
    #[cfg(target_os = "macos")]
    crate::chat::popout::sync_macos_activation_policy(app);
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

/// 关闭策略从「保持后台」切回「销毁」时，拆掉已隐藏的 chat WebView 以回收内存。
/// 可见窗口不动——设置页就在 chat 里，不能把用户正在看的窗拆掉。
pub(crate) fn destroy_hidden_chat_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("chat") else {
        return;
    };
    if window.is_visible().ok().unwrap_or(true) {
        return;
    }
    let _ = window.destroy();
}

/// 打开独立 AI 客户端窗口。
pub(crate) fn open_chat_window(app: &AppHandle) -> Result<(), String> {
    // 故意打开 Chat：清掉浮窗的"前台交还"快照（两个槽都清），避免随后浮窗关闭把前台从 Chat
    // 又交还回旧 App（例如 lens「在客户端继续」会先 open_chat_window 再关 lens）。
    #[cfg(target_os = "macos")]
    {
        let st = app.state::<AppState>();
        forget_frontmost_app(&st.prev_frontmost_pid_lens);
        forget_frontmost_app(&st.prev_frontmost_pid_main);
    }
    let existing_window = app.get_webview_window("chat");
    let window = ensure_chat_window(app)?;
    apply_chat_window_chrome(&window);
    crate::windows::apply_chat_window_min_size(&window, false);
    normalize_chat_window_behavior(&window);

    if existing_window.is_some() {
        let _ = window.eval(
            "const path = window.location.hash.replace('#', '').split('?')[0]; \
             const isChatSettings = path === 'chat/settings' || path.startsWith('chat/settings/'); \
             if (isChatSettings || (path !== 'chat' && !path.startsWith('chat/'))) { \
               window.location.hash = '#chat'; \
               window.dispatchEvent(new HashChangeEvent('hashchange')); \
             }",
        );
        let _ = app.emit_to("chat", "chat-open-request", ());
        reveal_chat_window(app, &window);
    } else {
        // 首次创建保持 hidden：前端在 useLayoutEffect 里恢复几何后再 show，避免默认尺寸闪一下。
        #[cfg(target_os = "macos")]
        {
            set_macos_regular_activation_policy(app);
            apply_macos_traffic_light_position(&window);
        }
    }

    Ok(())
}

/// 打开 AI 客户端内嵌设置页，替代旧版独立设置窗口。
pub(crate) fn open_chat_settings_window(app: &AppHandle) -> Result<(), String> {
    // 同 open_chat_window：打开内嵌设置也是"故意把 Chat 推到前台"，必须清掉浮窗快照，
    // 否则从翻译窗点设置后，翻译窗关闭会把前台又交还回旧 App，把刚打开的设置页压到后面。
    #[cfg(target_os = "macos")]
    {
        let st = app.state::<AppState>();
        forget_frontmost_app(&st.prev_frontmost_pid_lens);
        forget_frontmost_app(&st.prev_frontmost_pid_main);
    }
    let existing_window = app.get_webview_window("chat");
    let window = ensure_chat_window_with_hash(app, "chat/settings")?;
    apply_chat_window_chrome(&window);
    crate::windows::apply_chat_window_min_size(&window, false);
    normalize_chat_window_behavior(&window);

    if existing_window.is_some() {
        let _ = window.eval(
            "window.location.hash = '#chat/settings'; \
             window.dispatchEvent(new HashChangeEvent('hashchange'));",
        );
        let _ = app.emit_to("chat", "open-settings", ());
        reveal_chat_window(app, &window);
    } else {
        // 首次创建保持 hidden：前端在 useLayoutEffect 里恢复几何后再 show，避免默认尺寸闪一下。
        #[cfg(target_os = "macos")]
        {
            set_macos_regular_activation_policy(app);
            apply_macos_traffic_light_position(&window);
        }
    }

    Ok(())
}

/// 浮窗（lens 问答 或 translate 快速翻译）是否有任意一个正在显示。两窗口互斥，热键 toggle
/// 据此判断"已开 → 关闭，否则打开"，从而保证同一时刻只有一个浮窗可见。
fn lens_is_active(app: &AppHandle) -> bool {
    let any_overlay_visible = || {
        ["lens", "translate"].iter().any(|label| {
            app.get_webview_window(label)
                .and_then(|window| window.is_visible().ok())
                .unwrap_or(false)
        })
    };

    if let Some(state) = app.try_state::<AppState>() {
        if state.lens_busy.load(Ordering::SeqCst) {
            if any_overlay_visible() {
                return true;
            }
            // "刚开启"宽限期内窗口可能还没来得及可见（开启要先截冻结帧，200-500ms）。
            // 此时既不能清 busy（否则快速连按热键会并发双开 lens_request_internal，
            // take-once 复位载荷被吞），也不当作 active（避免把正在开启的会话误关）。
            if state.lens_open_in_grace() {
                return false;
            }
            state.lens_busy.store(false, Ordering::SeqCst);
        }
    }

    any_overlay_visible()
}

fn focus_lens_window(app: &AppHandle) -> bool {
    let Some(window) = ["translate", "lens"]
        .iter()
        .filter_map(|label| app.get_webview_window(label))
        .find(|window| window.is_visible().ok().unwrap_or(false))
    else {
        return false;
    };
    let _ = window.show();
    let _ = window.set_focus();
    true
}

/// 自动激活 app（单实例二次启动 / Windows 普通启动默认页）时使用。
/// 如果用户正在拉起 Lens，就不要再抢前台窗口。
pub(crate) fn open_settings_window_for_activation(app: &AppHandle) -> Result<(), String> {
    if lens_is_active(app) {
        let _ = focus_lens_window(app);
        return Ok(());
    }
    // 激活（单实例二次启动 / Windows 普通启动）时，优先把用户当前已开的主窗口带到前台：
    // 绝不强跳 Chat，更不能把正在 #chat/settings 配置的用户重置回 #chat（会丢失填到一半的
    // API key）。只有一个主窗口都没开时，才新开 Chat 作为默认入口。
    for label in ["settings", "chat", "main"] {
        let Some(window) = app.get_webview_window(label) else {
            continue;
        };
        if !window.is_visible().unwrap_or(false) {
            continue;
        }
        #[cfg(target_os = "macos")]
        set_macos_regular_activation_policy(app);
        if window.is_minimized().unwrap_or(false) {
            let _ = window.unminimize();
        }
        let _ = window.show();
        let _ = window.set_focus();
        #[cfg(target_os = "macos")]
        if label == "chat" {
            apply_macos_traffic_light_position(&window);
        }
        return Ok(());
    }
    if let Some(window) = crate::chat::popout::first_visible_popout(app) {
        #[cfg(target_os = "macos")]
        set_macos_regular_activation_policy(app);
        if window.is_minimized().unwrap_or(false) {
            let _ = window.unminimize();
        }
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    open_chat_window(app)
}

/// 根据语言返回托盘菜单的标签文本
fn tray_labels(lang: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    match lang {
        "en" => ("Open AI Client", "Show Translator", "Settings", "Quit"),
        _ => ("打开 AI 客户端", "显示翻译器", "设置", "退出"),
    }
}

/// 构建托盘菜单
fn build_tray_menu(app: &AppHandle, lang: &str) -> Result<tauri::menu::Menu<tauri::Wry>, String> {
    use tauri::menu::{Menu, MenuItem};
    let (chat_label, show_label, settings_label, quit_label) = tray_labels(lang);
    let chat = MenuItem::with_id(app, "chat", chat_label, true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let show = MenuItem::with_id(app, "show", show_label, true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let settings = MenuItem::with_id(app, "settings", settings_label, true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let quit = MenuItem::with_id(app, "quit", quit_label, true, None::<&str>)
        .map_err(|e| e.to_string())?;
    Menu::with_items(app, &[&chat, &show, &settings, &quit]).map_err(|e| e.to_string())
}

/// 设置系统托盘图标和菜单
/// 如果托盘已存在则只更新菜单；否则创建新的托盘图标并绑定菜单事件
pub(crate) fn setup_tray(app: &AppHandle) -> Result<(), String> {
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let lang = app
        .state::<AppState>()
        .settings_read()
        .settings_language
        .clone()
        .unwrap_or_else(|| "zh".to_string());

    let menu = build_tray_menu(app, &lang)?;

    if let Some(tray) = app.tray_by_id("main") {
        tray.set_menu(Some(menu)).map_err(|e| e.to_string())?;
        return Ok(());
    }

    let icon_bytes = include_bytes!("../icons/tray-icon.png");
    let icon_image = image::load_from_memory(icon_bytes)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    let (width, height) = icon_image.dimensions();
    let tray = TrayIconBuilder::<tauri::Wry>::with_id("main")
        .icon(tauri::image::Image::new_owned(
            icon_image.into_raw(),
            width,
            height,
        ))
        // macOS template image：纯黑透明 PNG,系统按 light/dark 主题自动反色为白
        // (Windows/Linux 上 ignore 此 flag,直接显示原图)
        .icon_as_template(true)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "chat" => {
                if let Err(err) = open_chat_window(app) {
                    eprintln!("Failed to open chat window: {}", err);
                }
            }
            "show" => match ensure_main_window(app) {
                Ok(window) => {
                    apply_frameless_window_chrome(&window);
                    #[cfg(not(target_os = "macos"))]
                    let _ = window.set_always_on_top(true);
                    #[cfg(target_os = "macos")]
                    {
                        let st = app.state::<AppState>();
                        remember_frontmost_app(&st.prev_frontmost_pid_main);
                        ensure_overlay_panel(&window);
                    }
                    let _ = window.eval(
                        "window.location.hash = '#translator'; window.dispatchEvent(new HashChangeEvent('hashchange'));",
                    );
                    #[cfg(target_os = "macos")]
                    show_overlay_panel(&window, true);
                    #[cfg(not(target_os = "macos"))]
                    let _ = window.show();
                    #[cfg(not(target_os = "macos"))]
                    let _ = window.set_focus();
                }
                Err(err) => eprintln!("Failed to ensure main window: {}", err),
            },
            "settings" => {
                if let Err(err) = open_chat_settings_window(app) {
                    eprintln!("Failed to open chat settings window: {}", err);
                }
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if lens_is_active(app) {
                    if let Err(err) = lens_close(app.clone()) {
                        eprintln!("Tray click overlay close error: {}", err);
                        return;
                    }
                }
                // 托盘图标点击 = 把 Kivio 带到前台：优先聚焦当前已开的窗口，不强跳 Chat，
                // 也不把停在 #chat/settings 的用户重置回 #chat（丢失填到一半的配置）。
                // 一个窗口都没开时才新开 Chat。
                if let Err(err) = open_settings_window_for_activation(app) {
                    eprintln!("Tray click activation error: {}", err);
                }
            }
        })
        .build(app)
        .map_err(|e| e.to_string())?;

    tray.set_tooltip(Some("Kivio Desktop".to_string()))
        .map_err(|e| e.to_string())?;
    Ok(())
}
