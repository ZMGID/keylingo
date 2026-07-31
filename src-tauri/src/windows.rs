// macOS cocoa interop below uses the legacy `cocoa` crate (objc, not objc2),
// which is deprecated. Migrating to objc2 is out of scope; suppress the lint here.
#![allow(deprecated)]

use tauri::{
    window::Color, AppHandle, LogicalSize, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
#[cfg(target_os = "macos")]
use tauri::{LogicalPosition, TitleBarStyle};

/// 侧栏收起时主内容区最小宽度（与前端 `CHAT_MIN_SIZE_COLLAPSED` 一致）。
pub const CHAT_MIN_INNER_WIDTH_COLLAPSED: f64 = 400.0;
/// 侧栏展开时整窗最小宽度（240px 侧栏 + 主内容区最小宽度）。
pub const CHAT_MIN_INNER_WIDTH_EXPANDED: f64 = 640.0;
pub const CHAT_MIN_INNER_HEIGHT: f64 = 400.0;
const CHAT_DEFAULT_INNER_WIDTH: f64 = 1280.0;
const CHAT_DEFAULT_INNER_HEIGHT: f64 = 800.0;

/// Windows / Linux 的自绘全宽标题栏带高度（与 index.css `.chat-titlebar-strip { height: 44px }` 同步）。
/// macOS 用系统 Overlay 标题栏、不占额外内容高度，故为 0。
#[cfg(target_os = "macos")]
const CHAT_TITLEBAR_STRIP_HEIGHT: f64 = 0.0;
#[cfg(not(target_os = "macos"))]
const CHAT_TITLEBAR_STRIP_HEIGHT: f64 = 44.0;

/// 把「可见内容所需尺寸」换算成窗口 inner 尺寸：非 macOS 要额外容纳那条标题栏带，
/// 否则小窗下带会吃掉 44px 内容高度，把输入框挤出可视区。
fn chat_window_size_for_visible_content(width: f64, height: f64) -> (f64, f64) {
    (width, height + CHAT_TITLEBAR_STRIP_HEIGHT)
}

pub fn apply_chat_window_min_size(window: &WebviewWindow, sidebar_expanded: bool) {
    let width = if sidebar_expanded {
        CHAT_MIN_INNER_WIDTH_EXPANDED
    } else {
        CHAT_MIN_INNER_WIDTH_COLLAPSED
    };
    let (width, height) = chat_window_size_for_visible_content(width, CHAT_MIN_INNER_HEIGHT);
    let _ = window.set_min_size(Some(LogicalSize::new(width, height)));
}

/// 悬浮卡片式侧栏的外边距（与 index.css `.chat-sidebar-shell { margin: 8px }` 同步）。
/// 灯 x 随卡片左缘 +8；y 跟随侧栏顶栏图标（图标在卡片内上提后，窗口坐标中心仍约 26px，与主区顶栏齐）。
#[cfg(target_os = "macos")]
const CHAT_SIDEBAR_CARD_INSET: f64 = 8.0;

/// 灯到卡片左缘的距离（不是到窗口左缘）。
#[cfg(target_os = "macos")]
const CHAT_TRAFFIC_LIGHT_X: f64 = 14.0 + CHAT_SIDEBAR_CARD_INSET;

/// 交给 tao `traffic_light_inset` 的 y。tao 会在每次内容视图 `drawRect` 重新应用这个 inset
/// （见 tao 源 view.rs::draw_rect → inset_traffic_lights），故窗口拖动/缩放/移动全程都保持对齐。
///
/// 这个 y **不等于**灯中心。tao 只写按钮的 `origin.x`，`origin.y` 始终是 AppKit 布局出来的值：
///   容器高 = 按钮高 + y，灯中心距顶 = y − button.origin.y + button.height / 2
/// 而 `button.origin.y` / `button.height` 随 macOS 版本变（标题栏容器自然高度不同），
/// 所以「y=32 → 中心 30」只在某些系统上成立 —— 早先几次「统一到 30」的修复就是栽在这里。
/// 现在不再假设：前端用 `chat_traffic_light_center_y` 量出真实中心，顶栏那条线跟着灯走
/// （见 index.css `--chat-traffic-center-y`）。这个常数只决定灯大致落在哪，不需要精确。
#[cfg(target_os = "macos")]
const CHAT_TRAFFIC_LIGHT_INSET_Y: f64 = 32.0;

/// 交通灯中心距 WebView 内容顶缘的距离（CSS px / AppKit point，同一单位）。
///
/// 前端据此把侧栏顶栏图标、主区顶栏控件摆到同一条线上。非 macOS / 取不到 / 数值离谱都返回
/// `None`，前端退回 CSS 里的默认 30px。
#[tauri::command]
pub async fn chat_traffic_light_center_y(window: WebviewWindow) -> Option<f64> {
    #[cfg(target_os = "macos")]
    {
        // NSView 几何只能在主线程读，命令本身在 worker 线程，用 channel 取回。
        let (tx, rx) = std::sync::mpsc::channel();
        let window_for_main = window.clone();
        window
            .run_on_main_thread(move || {
                let measured = window_for_main
                    .ns_window()
                    .ok()
                    .filter(|ptr| !ptr.is_null())
                    .and_then(|ptr| unsafe {
                        measure_traffic_light_center_y(ptr as cocoa::base::id)
                    });
                let _ = tx.send(measured);
            })
            .ok()?;
        rx.recv_timeout(std::time::Duration::from_millis(500))
            .ok()
            .flatten()
            // 灯只可能在标题栏那一带；离谱值当没量到，别把 padding 算成负数。
            .filter(|y| (8.0..=80.0).contains(y))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        None
    }
}

/// 把 close 按钮的 bounds 转到 contentView 坐标系，换算成「距内容顶缘」。
/// contentView 未翻转（y 自下而上），故距顶 = 内容高 − (y + 高/2)。
/// Overlay 标题栏下 contentView 铺满整个窗口 frame，所以这就是 CSS 的 y。
#[cfg(target_os = "macos")]
unsafe fn measure_traffic_light_center_y(window: cocoa::base::id) -> Option<f64> {
    use cocoa::base::{id, nil};
    use cocoa::foundation::NSRect;
    use objc::{msg_send, sel, sel_impl};

    const NS_WINDOW_CLOSE_BUTTON: u64 = 0;
    let close: id = msg_send![window, standardWindowButton: NS_WINDOW_CLOSE_BUTTON];
    let content: id = msg_send![window, contentView];
    if close == nil || content == nil {
        return None;
    }
    let bounds: NSRect = msg_send![close, bounds];
    let in_content: NSRect = msg_send![close, convertRect: bounds toView: content];
    let content_bounds: NSRect = msg_send![content, bounds];
    if content_bounds.size.height <= 0.0 || in_content.size.height <= 0.0 {
        return None;
    }
    Some(content_bounds.size.height - (in_content.origin.y + in_content.size.height / 2.0))
}

/// 隐藏 Overlay 标题栏的窗口标题文字。交通灯位置本身由 builder 的 `traffic_light_position`
/// （= tao `traffic_light_inset`，tao 每次 drawRect 自动重新应用）负责并持久保持，这里不再手动重排。
#[cfg(target_os = "macos")]
pub(crate) fn apply_macos_traffic_light_position(window: &WebviewWindow) {
    use cocoa::base::id;

    let window_for_main = window.clone();
    let _ = window.run_on_main_thread(move || {
        let Ok(ptr) = window_for_main.ns_window() else {
            return;
        };
        if ptr.is_null() {
            return;
        }
        unsafe {
            hide_macos_window_title(ptr as id);
        }
    });
}

/// NSWindowTitleHidden — 隐藏 Overlay 标题栏中的窗口标题文字。
#[cfg(target_os = "macos")]
unsafe fn hide_macos_window_title(window: cocoa::base::id) {
    use objc::{msg_send, sel, sel_impl};

    const NS_WINDOW_TITLE_HIDDEN: u64 = 1;
    let _: () = msg_send![window, setTitleVisibility: NS_WINDOW_TITLE_HIDDEN];
}

/// Chat 作为普通桌面窗口：不置顶、不跨全 Space（与 Lens overlay 区分）。
pub fn normalize_chat_window_behavior(window: &WebviewWindow) {
    let _ = window.set_always_on_top(false);
    let _ = window.set_skip_taskbar(false);
    #[cfg(target_os = "macos")]
    let _ = window.set_visible_on_all_workspaces(false);
}

/// macOS Chat：系统 Overlay 标题栏 + 原生交通灯；其他平台保持无边框自绘控件。
pub fn apply_chat_window_chrome(window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        let _ = window.set_decorations(true);
        let _ = window.set_title_bar_style(TitleBarStyle::Overlay);
        let _ = window.set_shadow(true);
        apply_macos_traffic_light_position(window);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window.set_decorations(false);
        #[cfg(target_os = "windows")]
        {
            // Mica 需要透明 WebView 背景（见 ensure_chat_window_with_hash）；不支持 Mica 时
            // 由前端不透明 shell 完整覆盖回退。所以这里不再设主题清屏色。
            let _ = window.set_background_color(Some(Color(0, 0, 0, 0)));
            let _ = window.set_shadow(true);
            apply_windows_chat_window_frame(window);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = window.set_background_color(Some(Color(0, 0, 0, 0)));
            let _ = window.set_shadow(false);
        }
    }
}

// Windows Chat: let DWM own the outer shadow/corners. The WebView content
// fills the window without a second CSS-drawn rounded frame.
#[cfg(target_os = "windows")]
fn apply_windows_chat_window_frame(window: &WebviewWindow) {
    use std::ffi::c_void;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_COLOR_DEFAULT,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };

    let Ok(hwnd) = window.hwnd() else {
        return;
    };

    unsafe {
        let corner = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const c_void,
            std::mem::size_of_val(&corner) as u32,
        );

        // 使用系统默认描边颜色（随浅色/深色主题自适应），呈现原生 Windows 窗口边框。
        let border_color = DWMWA_COLOR_DEFAULT;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &border_color as *const _ as *const c_void,
            std::mem::size_of_val(&border_color) as u32,
        );
    }
}

/// 给 chat 窗口上 Mica，返回「材质是否真的生效」。
///
/// 不能用 `window.set_effects()`：tauri 的 `vibrancy::apply_effects` 直接丢弃
/// `apply_mica` 的 Result，`set_effects` 又是 `let _ =`，于是 Win10（build < 22000，
/// 根本没有 Mica）上前端也会收到 resolve，误判「材质已生效」把外壳设成 transparent
/// —— 透明窗口 + 没有材质 = 直接透出后面的桌面和别的窗口。
#[tauri::command]
pub fn chat_window_apply_mica(window: WebviewWindow, dark: bool) -> bool {
    #[cfg(target_os = "windows")]
    {
        window_vibrancy::apply_mica(&window, Some(dark)).is_ok()
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window, dark);
        false
    }
}

/// macOS chat 窗口：材质没上时把 NSWindow 设回 opaque。
///
/// 建窗时写死 `transparent(true)`（Menu 材质要透过 WebView 才看得见），但材质关掉后内容
/// 本来就是铺满整窗的不透明外壳 —— 非 opaque 的 NSWindow 白拿不到合成器的不透明快路径，
/// 阴影还要每帧从 alpha 反推。台前调度 / Mission Control 缩放整窗时这笔开销肉眼可见掉帧。
/// 所以按「材质是否激活」来回切：材质上了必须 NO，没上就 YES。
#[tauri::command]
pub fn chat_window_set_opaque(window: WebviewWindow, opaque: bool) {
    #[cfg(target_os = "macos")]
    {
        use cocoa::base::id;
        let window_for_main = window.clone();
        let _ = window.run_on_main_thread(move || {
            let Ok(ptr) = window_for_main.ns_window() else {
                return;
            };
            if ptr.is_null() {
                return;
            }
            unsafe {
                use objc::{class, msg_send, sel, sel_impl};
                let ns_window = ptr as id;
                // opaque 窗口的 backgroundColor 必须是实色：clearColor + setOpaque:YES
                // 会让页面还没画到的地方读到未定义内容。windowBackgroundColor 跟 NSAppearance，
                // 首帧之后整窗都被外壳盖住，用户看不到它。
                let color: id = if opaque {
                    msg_send![class!(NSColor), windowBackgroundColor]
                } else {
                    msg_send![class!(NSColor), clearColor]
                };
                let _: () = msg_send![ns_window, setBackgroundColor: color];
                let _: () = msg_send![ns_window, setOpaque: opaque];
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, opaque);
    }
}

/// 翻译器 / 设置等悬浮小窗：无边框透明壳。
pub fn apply_frameless_window_chrome(window: &WebviewWindow) {
    let _ = window.set_decorations(false);
    let _ = window.set_shadow(false);
    #[cfg(target_os = "macos")]
    {
        let _ = window.set_title_bar_style(TitleBarStyle::Visible);
    }
}

/**
 * 获取主窗口
 */
pub fn get_main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
}

pub fn get_chat_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("chat")
}

/**
 * 确保主窗口存在（不存在则创建）
 * 从 tauri.conf.json 中读取主窗口配置进行创建
 */
pub fn ensure_main_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    if let Some(window) = get_main_window(app) {
        return Ok(window);
    }

    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .ok_or_else(|| "Main window config not found".to_string())?;

    WebviewWindowBuilder::from_config(app, config)
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|e| e.to_string())
}

/**
 * 确保独立 Chat 窗口存在。
 */
pub fn ensure_chat_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    ensure_chat_window_with_hash(app, "chat")
}

/**
 * 确保独立 Chat 窗口存在，并在首次创建时进入指定 hash 路由。
 */
pub fn ensure_chat_window_with_hash(app: &AppHandle, hash: &str) -> Result<WebviewWindow, String> {
    if let Some(window) = get_chat_window(app) {
        return Ok(window);
    }

    let route = hash.trim_start_matches('#');
    let route = if route.is_empty() { "chat" } else { route };
    let url = format!("index.html#{route}");
    let (min_width, min_height) =
        chat_window_size_for_visible_content(CHAT_MIN_INNER_WIDTH_COLLAPSED, CHAT_MIN_INNER_HEIGHT);
    let (default_width, default_height) =
        chat_window_size_for_visible_content(CHAT_DEFAULT_INNER_WIDTH, CHAT_DEFAULT_INNER_HEIGHT);
    let mut builder = WebviewWindowBuilder::new(app, "chat", WebviewUrl::App(url.into()))
        .title("Kivio Desktop")
        .inner_size(default_width, default_height)
        .min_inner_size(min_width, min_height)
        .resizable(true)
        .visible_on_all_workspaces(false)
        .skip_taskbar(false)
        .visible(false);

    #[cfg(target_os = "macos")]
    {
        builder = builder
            .decorations(true)
            .title_bar_style(TitleBarStyle::Overlay)
            .hidden_title(true)
            .traffic_light_position(LogicalPosition::new(
                CHAT_TRAFFIC_LIGHT_X,
                CHAT_TRAFFIC_LIGHT_INSET_Y,
            ))
            .transparent(true)
            .background_color(Color(0, 0, 0, 0))
            .shadow(true);
    }

    #[cfg(target_os = "windows")]
    {
        // Windows：透明无边框窗口让 Mica 穿过 WebView，圆角 / 描边 / 阴影交给 DWM。
        builder = builder
            .decorations(false)
            .transparent(true)
            .background_color(Color(0, 0, 0, 0))
            .shadow(true);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Linux：不支持系统窗口材质，始终使用不透明回退。
        builder = builder
            .decorations(false)
            .transparent(false)
            .background_color(Color(255, 255, 255, 255))
            .shadow(false);
    }

    builder.build().map_err(|e| e.to_string())
}

/**
 * 确保 Lens 窗口存在（不存在则创建）
 * 单 webview 三态：select 全屏 / ready 悬浮 600x72 / answering 悬浮 600x420。
 * 创建时尺寸为悬浮态默认值；后端按需要 set_size 切换。
 */
pub fn ensure_lens_window(app: &AppHandle, mode: &str) -> Result<WebviewWindow, String> {
    ensure_overlay_window(app, "lens", "Lens", mode)
}

/// 确保独立"快速翻译"窗口存在（不存在则创建）。
/// 与 lens 浮窗共用同一套无边框透明 NSPanel 形态与 Lens.tsx bundle，按 hash query
/// 的 mode（translate / translateText）渲染翻译 UI。与 lens 问答窗口互斥（同一时刻
/// 只有一个浮窗可见，由 `lens_is_active` 泛化 + 热键 toggle 保证）。
pub fn ensure_translate_window(app: &AppHandle, mode: &str) -> Result<WebviewWindow, String> {
    ensure_overlay_window(app, "translate", "Translate", mode)
}

/// lens / translate 浮窗共用的创建逻辑：无边框、透明、无原生阴影、初始隐藏，建窗后在
/// macOS 上转成非激活 NSPanel（`ensure_overlay_panel`）。两窗口除 label / title 外完全一致。
/// mode 烤进创建 URL 的 hash query，使冷挂载的前端首帧即读到正确 mode（不依赖事后 eval 设 hash 的时机）。
fn ensure_overlay_window(
    app: &AppHandle,
    label: &str,
    title: &str,
    mode: &str,
) -> Result<WebviewWindow, String> {
    if let Some(window) = app.get_webview_window(label) {
        return Ok(window);
    }

    // chat 模式 hash 为 '#lens'（readModeFromHash 默认即 chat）；其余模式带 query。
    let hash = if mode == "translate"
        || mode == "translateText"
        || mode == "replace"
        || mode == "screenshot"
    {
        format!("lens?mode={mode}")
    } else {
        "lens".to_string()
    };
    let url = format!("index.html#{hash}");
    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App(url.into()))
        .title(title)
        .inner_size(600.0, 72.0)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .resizable(true)
        .decorations(false)
        .shadow(false)
        .transparent(true)
        // 把 WebView2 / WKWebView 的默认背景设成全透明。Windows 上 WebView2 控件
        // 在 HTML/CSS 把 html、body、#root 设为 transparent 之前会用系统主题色（白）
        // 渲染首帧，导致全屏白闪 —— 设了 (0,0,0,0) 后默认背景本身就是透明的。
        // 文档：Windows 8+ 上 webview 层的 alpha=0 被尊重；macOS 上此调用是 no-op。
        .background_color(Color(0, 0, 0, 0))
        .skip_taskbar(true)
        // Tauri 默认让新窗口初始聚焦；即使 visible=false，macOS 冷创建普通 NSWindow 时也会
        // 短暂激活 Kivio。截图浮窗随后再恢复原 App 就会触发整组窗口重排，表现为桌面闪一下。
        // 先以非聚焦隐藏窗口创建，后续 NSPanel 显示时再由 show_overlay_panel 精确取键盘焦点。
        .focused(false)
        .visible(false)
        .build()
        .map_err(|e| e.to_string())?;

    // macOS：把浮窗转成非激活 NSPanel，使其能浮现在别的 App 原生全屏 Space 上方。
    #[cfg(target_os = "macos")]
    ensure_overlay_panel(&window);

    Ok(window)
}

/// macOS：把短命浮窗（lens / 翻译）转成**非激活 NSPanel**，使其能浮现在别的 App
/// 原生全屏 Space 上方且不切换 Space。幂等：已是 panel 则跳过重分类，只重申行为。
///
/// 背景：macOS（Big Sur 起）只允许 NSPanel、或 Accessory(LSUIElement) 策略 app 的窗口
/// 画进别的 App 的全屏 Space。本 app 为 `ActivationPolicy::Regular`（Chat 需要 Dock 身份），
/// 所以普通 NSWindow 无论 collectionBehavior / level 怎么设都进不去别人的全屏 Space。改成
/// 非激活 NSPanel 后：①panel 被系统允许覆盖全屏；②非激活 → 点击/聚焦不激活宿主 app →
/// 不会把用户从全屏 Space 拽走。Chat 窗口**绝不**走这里，保持普通 NSWindow。
#[cfg(target_os = "macos")]
pub fn ensure_overlay_panel(window: &WebviewWindow) {
    run_overlay_on_main(window, |ptr| unsafe {
        configure_overlay_panel(ptr);
    });
}

/// macOS：显示浮窗。`orderFrontRegardless` 不激活 app、不切 Space；`need_key=true` 时
/// `makeKeyWindow` 后把内部 WKWebView 设为 first responder（见 `find_wk_webview`），让 WebView
/// 能接收键盘（翻译输入框 / lens 问题框 / Escape），并修掉"复用窗口第二次打开不聚焦、要手动点
/// 一下"的问题。配合 `_setPreventsActivation:` + `_isNonactivatingPanel` 才能真正拿到键盘焦点。
/// 非激活 panel 成为 key 也不会激活 app。**绝不**用 `set_focus` / `makeKeyAndOrderFront` /
/// `activateIgnoringOtherApps`——那会激活 Regular app 并把用户从全屏 Space 拽走。
#[cfg(target_os = "macos")]
pub fn show_overlay_panel(window: &WebviewWindow, need_key: bool) {
    use objc::{msg_send, sel, sel_impl};
    run_overlay_on_main(window, move |ptr| unsafe {
        // 显示前再重申一次 panel 行为，抵消 tao set_resizable / set_always_on_top 可能造成的
        // styleMask / level 漂移。
        configure_overlay_panel(ptr);
        let _: () = msg_send![ptr, orderFrontRegardless];
        if need_key {
            let _: () = msg_send![ptr, makeKeyWindow];
            // 把 first responder 精确落到内部 WKWebView（等价于用户手动点一下输入框）。
            // 复用 lens 窗口时，contentView 是 wry 容器视图，makeFirstResponder(contentView) 不一定
            // 下沉到 WKWebView → 第二次打开网页收不到键盘、必须手动点一下才聚焦。直接在视图树里找到
            // WKWebView 设为 first responder 可消除这个"第二次不聚焦"的复用问题；找不到时回退 contentView。
            let cv: *mut objc::runtime::Object = msg_send![ptr, contentView];
            if !cv.is_null() {
                let wk = find_wk_webview(cv);
                let target = if wk.is_null() { cv } else { wk };
                let _: () = msg_send![ptr, makeFirstResponder: target];
            }
        }
    });
}

/// 在视图树里深度优先找到第一个 WKWebView（wry 把 WKWebView 作为窗口 contentView 的子视图）。
/// 找不到 / WebKit 未加载时返回 null。
#[cfg(target_os = "macos")]
unsafe fn find_wk_webview(view: *mut objc::runtime::Object) -> *mut objc::runtime::Object {
    use objc::{msg_send, sel, sel_impl};

    let nil: *mut objc::runtime::Object = std::ptr::null_mut();
    if view.is_null() {
        return nil;
    }
    // 运行时查类，避免 WebKit 未加载时 class! 直接 panic。
    let Some(wk_class) = objc::runtime::Class::get("WKWebView") else {
        return nil;
    };
    let is_wk: bool = msg_send![view, isKindOfClass: wk_class];
    if is_wk {
        return view;
    }
    let subviews: *mut objc::runtime::Object = msg_send![view, subviews];
    if subviews.is_null() {
        return nil;
    }
    let count: usize = msg_send![subviews, count];
    let mut i = 0usize;
    while i < count {
        let sub: *mut objc::runtime::Object = msg_send![subviews, objectAtIndex: i];
        let found = find_wk_webview(sub);
        if !found.is_null() {
            return found;
        }
        i += 1;
    }
    nil
}

/// macOS：把浮窗内部 WKWebView 设为 first responder。前端在聚焦输入框时调用（复用其
/// [0,40,120,240,420] 多次重试时序），用来磨平"复用 lens 窗口第二次打开偶尔要手点一下才聚焦"
/// 的时序问题。只 makeKeyWindow + makeFirstResponder(WKWebView)，不销毁窗口（销毁重分类窗口会
/// 抛 ObjC 异常崩溃），零崩溃风险。
#[cfg(target_os = "macos")]
pub fn focus_overlay_webview(window: &WebviewWindow) {
    use objc::{msg_send, sel, sel_impl};
    run_overlay_on_main(window, |ptr| unsafe {
        let _: () = msg_send![ptr, makeKeyWindow];
        let cv: *mut objc::runtime::Object = msg_send![ptr, contentView];
        if !cv.is_null() {
            let wk = find_wk_webview(cv);
            let target = if wk.is_null() { cv } else { wk };
            let _: () = msg_send![ptr, makeFirstResponder: target];
        }
    });
}

/// 冷创建 Panel 后重新激活原 App 是异步完成的；若立刻 makeKeyWindow，随后到达的 App 激活
/// 会再次夺走 key focus。稍等一个很短的窗口排序周期，再只对非激活 Panel/WKWebView 取焦点，
/// 从而同时满足“原 App/Chat 排序不变”和“翻译输入框能收键盘”。
#[cfg(target_os = "macos")]
pub fn refocus_overlay_after_frontmost_reassert(window: &WebviewWindow) {
    let window = window.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        focus_overlay_webview(&window);
    });
}

/// 在主线程上拿到 ns_window 指针并执行 `f`（AppKit 调用必须落在主线程）。
///
/// **FFI 边界拦 ObjC 异常（真 bug 修复，非兜底）**：overlay 的所有 objc 闭包
/// （`ensure_overlay_panel` / `show_overlay_panel` / `focus_overlay_webview`）都经此单一漏斗。
/// 这些闭包里的 `msg_send!` 一旦让某个 AppKit/ObjC 调用抛出 `NSException`，异常会沿主线程
/// runloop 往上穿过 tao 的 `stop_app_on_panic`(`catch_unwind`)——而 `catch_unwind` **接不住
/// 外来（ObjC）异常**，于是触发 `__rust_foreign_exception` → `abort`，整个 app 崩溃。
/// 因此必须在这里用 `objc_exception::try`（底层 C 层 `@try/@catch`）把闭包执行包起来：
/// ① 不再 abort（优雅吞掉这一次 overlay 配置/显示）；
/// ② 捕获到异常时打印 `NSException` 的 `name` + `reason`——这是诊断职责，下次复现即可凭日志
///   定位到底是哪个 ObjC 调用抛的异常，再去修真正的根因。
#[cfg(target_os = "macos")]
fn run_overlay_on_main<F>(window: &WebviewWindow, f: F) -> bool
where
    F: FnOnce(*mut objc::runtime::Object) + Send + 'static,
{
    let run = move |window: &WebviewWindow| -> bool {
        let Ok(ptr) = window.ns_window() else {
            return false;
        };
        if ptr.is_null() {
            return false;
        }
        let ptr = ptr as *mut objc::runtime::Object;
        // 在 FFI 边界用 @try/@catch 包住闭包执行。两个执行分支（直接执行 / run_on_main_thread）
        // 都通过 `run` 走这里，因此一处 guard 覆盖全部。
        let result = unsafe { objc_exception::r#try(move || f(ptr)) };
        if let Err(exc) = result {
            log_overlay_objc_exception(exc);
            return false;
        }
        true
    };

    if macos_is_main_thread() {
        return run(window);
    }

    let window_for_task = window.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    if let Err(err) = window.run_on_main_thread(move || {
        let _ = tx.send(run(&window_for_task));
    }) {
        eprintln!("[overlay] failed to schedule main-thread operation: {err}");
        return false;
    }
    rx.recv_timeout(std::time::Duration::from_millis(250))
        .unwrap_or(false)
}

/// 在主线程调用 `[NSApp hide:]` 把前台让回原 App，@try/@catch 兜底任何 ObjC 异常。
#[cfg(target_os = "macos")]
pub fn hide_app_guarded(app: &AppHandle) {
    let run = || unsafe {
        let result = objc_exception::r#try(|| {
            use cocoa::base::{id, nil};
            use objc::{class, msg_send, sel, sel_impl};
            let ns_app: id = msg_send![class!(NSApplication), sharedApplication];
            let _: () = msg_send![ns_app, hide: nil];
        });
        if let Err(exc) = result {
            log_overlay_objc_exception(exc);
        }
    };
    if macos_is_main_thread() {
        run();
    } else {
        let _ = app.run_on_main_thread(run);
    }
}

/// 把捕获到的 `NSException` 指针里的 `name` + `reason` 提取成 Rust 字符串并打印，用于定位到底是
/// 哪个 ObjC 调用抛的异常。已在 `objc_exception::try` 的 `Err` 安全区内，这里只读裸指针、判空后再
/// 取 UTF8String，绝不再触发新异常。
#[cfg(target_os = "macos")]
fn log_overlay_objc_exception(exc: *mut objc_exception::Exception) {
    use objc::{msg_send, sel, sel_impl};

    let exc = exc as *mut objc::runtime::Object;
    if exc.is_null() {
        eprintln!("[overlay-objc] caught nil NSException");
        return;
    }

    // 从 NSString 安全取出 Rust 字符串：判空 → UTF8String(*const c_char) → CStr → 拷贝。
    unsafe fn ns_string_to_rust(s: *mut objc::runtime::Object) -> String {
        use objc::{msg_send, sel, sel_impl};
        use std::ffi::CStr;
        use std::os::raw::c_char;
        if s.is_null() {
            return "<nil>".to_string();
        }
        let utf8: *const c_char = msg_send![s, UTF8String];
        if utf8.is_null() {
            return "<nil>".to_string();
        }
        CStr::from_ptr(utf8).to_string_lossy().into_owned()
    }

    unsafe {
        let name_obj: *mut objc::runtime::Object = msg_send![exc, name];
        let reason_obj: *mut objc::runtime::Object = msg_send![exc, reason];
        let name = ns_string_to_rust(name_obj);
        let reason = ns_string_to_rust(reason_obj);
        eprintln!("[overlay-objc] caught NSException name={name} reason={reason}");
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_is_main_thread() -> bool {
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let is_main: bool = msg_send![class!(NSThread), isMainThread];
        is_main
    }
}

#[cfg(target_os = "macos")]
extern "C" {
    fn object_setClass(
        obj: *mut objc::runtime::Object,
        cls: *const objc::runtime::Class,
    ) -> *const objc::runtime::Class;
}

/// 浮窗被 `object_setClass` 换成 NSPanel 子类前的原类（tao `TaoWindow`）。全 app 的 tao 窗口
/// 共用同一个类，故一个 static 足够。destroy 前换回它，避免 tao 按错类析构触发 ObjC abort。
/// 存 usize（裸类指针进程常驻、只读）以满足 Send/Sync。
#[cfg(target_os = "macos")]
static ORIGINAL_OVERLAY_CLASS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

/// macOS：销毁重分类过的浮窗。在同一个主线程保护闭包内先换回原类（`TaoWindow`），再
/// `destroy()`，保证 tao/WebKit 析构时不会看到自定义 Panel 类。主线程调度失败或未记到原类时
/// 回退 hide，不从当前线程冒险销毁。
#[cfg(target_os = "macos")]
pub fn destroy_overlay_window(window: &WebviewWindow) {
    let Some(orig) = ORIGINAL_OVERLAY_CLASS.get().copied() else {
        // 理论不会发生（destroy 前必经 configure_overlay_panel 设过原类）；真走到这里说明
        // 假设被打破，退回旧的 hide 复用（不回收内存但不崩），留日志便于诊断。
        eprintln!("[overlay] destroy_overlay_window: 原类未记录，退回 hide");
        let _ = window.hide();
        return;
    };
    // 恢复原类和 destroy 必须在同一个主线程闭包里连续完成。若先等待恢复、再从后台线程
    // destroy，主线程排队超过等待时限时就可能在窗口仍是 KivioOverlayPanel 的情况下销毁，
    // 重新触发 tao/WebKit 的 ObjC abort；两步合并后即使调用方等待超时，排队闭包最终执行时
    // 仍会严格按“换类 → 销毁”的顺序运行。
    let window_for_destroy = window.clone();
    if !run_overlay_on_main(window, move |ptr| unsafe {
        object_setClass(ptr, orig as *const objc::runtime::Class);
        let _ = window_for_destroy.destroy();
    }) {
        // 调度失败或窗口句柄已失效时不冒险从当前线程 destroy；隐藏是安全的降级路径。
        eprintln!("[overlay] destroy_overlay_window: 主线程安全销毁未确认，退回 hide");
        let _ = window.hide();
    }
}

/// 运行时注册一个 NSPanel 子类：borderless 窗口默认 `canBecomeKeyWindow=NO`，强制 YES 才能
/// 接收键盘；`canBecomeMainWindow=NO` 保持其辅助身份。进程内只注册一次。
#[cfg(target_os = "macos")]
fn kivio_overlay_panel_class() -> *const objc::runtime::Class {
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel, BOOL, NO, YES};
    use objc::{class, sel, sel_impl};
    use std::sync::OnceLock;

    // ClassDecl::register 返回的类指针进程生命周期常驻、只读，可安全跨线程共享。
    struct PanelClass(*const Class);
    unsafe impl Send for PanelClass {}
    unsafe impl Sync for PanelClass {}

    static PANEL_CLASS: OnceLock<PanelClass> = OnceLock::new();

    extern "C" fn yes(_: &Object, _: Sel) -> BOOL {
        YES
    }
    extern "C" fn no_(_: &Object, _: Sel) -> BOOL {
        NO
    }

    PANEL_CLASS
        .get_or_init(|| {
            let superclass = class!(NSPanel);
            let mut decl =
                ClassDecl::new("KivioOverlayPanel", superclass).expect("declare KivioOverlayPanel");
            unsafe {
                decl.add_method(
                    sel!(canBecomeKeyWindow),
                    yes as extern "C" fn(&Object, Sel) -> BOOL,
                );
                decl.add_method(
                    sel!(canBecomeMainWindow),
                    no_ as extern "C" fn(&Object, Sel) -> BOOL,
                );
                // 让 AppKit 一致地把本 panel 当作非激活 panel（与 _setPreventsActivation: 的
                // WindowServer tag 配合，确保 key-focus theft 生效、键盘进得去）。私有 selector。
                decl.add_method(
                    sel!(_isNonactivatingPanel),
                    yes as extern "C" fn(&Object, Sel) -> BOOL,
                );
            }
            PanelClass(decl.register() as *const Class)
        })
        .0
}

/// 重分类窗口为非激活 NSPanel 并设置全屏浮现所需的 styleMask / collectionBehavior / level（幂等）。
#[cfg(target_os = "macos")]
unsafe fn configure_overlay_panel(window: *mut objc::runtime::Object) {
    use objc::{msg_send, sel, sel_impl};

    // 1) 重分类到 NSPanel 子类（已是则跳过 object_setClass）。
    //    注意：重分类后实例的类不再是 tao 的 `TaoWindow`，丢失了它的 `focusable` ivar——
    //    因此**绝不能**对 lens/翻译窗调用 `WebviewWindow::set_focusable()`，tao 会用
    //    `get_mut_ivar::<Bool>("focusable")` 在新类上找不到该 ivar 而 abort。当前代码无人调用，
    //    且 show/hide/set_size/set_resizable/set_focus 都不触发它，安全。
    //    实例尺寸安全：NSPanel 不新增 ivar，尺寸与 NSWindow 一致（≤ TaoWindow = NSWindow + 1 Bool），
    //    重分类不会越界读写。
    let panel_class = kivio_overlay_panel_class();
    let already: bool = msg_send![window, isKindOfClass: panel_class];
    if !already {
        // object_setClass 返回原类（tao 的 `TaoWindow`，全 app 共用一个）。存下来，
        // destroy 前换回它，让 tao 按认识的类析构，避免类不匹配的 ObjC abort。
        let prev = object_setClass(window, panel_class);
        let _ = ORIGINAL_OVERLAY_CLASS.set(prev as usize);
    }

    // 2) 非激活面板样式：点击/聚焦不激活宿主 app（Spotlight 式）。保留既有 borderless/resizable 位。
    const NONACTIVATING_PANEL: usize = 1 << 7;
    let mask: usize = msg_send![window, styleMask];
    let _: () = msg_send![window, setStyleMask: mask | NONACTIVATING_PANEL];

    // 2b) 关键修复（AppKit FB16484811）：object_setClass 重分类的窗口不会像 NSPanel 真正 init
    //     那样设置 WindowServer 的 kCGSPreventsActivationTagBit；缺这个 tag，非激活 panel 成 key
    //     也拿不到键盘（AppKit 不为它做 key-focus theft）→ 输入框聚焦却收不到打字/Esc、未处理
    //     按键还会 beep。setStyleMask 之后显式补调私有 _setPreventsActivation:(YES) 补上该 tag。
    let prevents_sel = sel!(_setPreventsActivation:);
    let responds: bool = msg_send![window, respondsToSelector: prevents_sel];
    if responds {
        let _: () = msg_send![window, _setPreventsActivation: true];
    }

    // 3) collectionBehavior：用 **CanJoinAllSpaces**（Spotlight/Alfred 同款）让浮窗出现在**所有
    //    Space**——它永远在当前这个 Space，orderFront 不需要切 Space → 不跳屏。配 FullScreenAuxiliary
    //    覆盖别的 App 全屏、Transient 随 Space 浮动、IgnoresCycle 不进 Cmd+`。
    //    不要用 MoveToActiveSpace：非激活 panel 只"成为 key"不"激活 app"，它的"移到活动 Space"
    //    触发不发生 → 窗口停在归属 Space、orderFront 时系统把你切过去（每次都跳）。
    //    不要用 Stationary：会把窗口钉在某个 Space（复用时跑回旧 Space）。
    const CAN_JOIN_ALL_SPACES: usize = 1 << 0;
    const MOVE_TO_ACTIVE_SPACE: usize = 1 << 1;
    const TRANSIENT: usize = 1 << 3;
    const STATIONARY: usize = 1 << 4;
    const IGNORES_CYCLE: usize = 1 << 6;
    const FULL_SCREEN_AUXILIARY: usize = 1 << 8;
    let behavior: usize = msg_send![window, collectionBehavior];
    let behavior = (behavior & !MOVE_TO_ACTIVE_SPACE & !STATIONARY)
        | CAN_JOIN_ALL_SPACES
        | TRANSIENT
        | IGNORES_CYCLE
        | FULL_SCREEN_AUXILIARY;
    let _: () = msg_send![window, setCollectionBehavior: behavior];

    // 4) 置于菜单栏之上以盖住全屏内容；用 status 档(25)，避开 screenSaver(1000) 那种会在
    //    错误 Space 闪一下的过高层级。
    const NS_STATUS_WINDOW_LEVEL: isize = 25;
    let _: () = msg_send![window, setLevel: NS_STATUS_WINDOW_LEVEL];

    // 5) 关键：NSPanel 默认在宿主 app 失活时自动隐藏；浮窗显示时前台是别的 App（如全屏 Chrome），
    //    不设 NO 会立刻消失。
    let _: () = msg_send![window, setHidesOnDeactivate: false];

    // 6) 关掉原生窗口阴影：lens 窗口是透明无边框，卡片/对话栏的阴影由 CSS（.window-frosted /
    //    .lens-floating-surface）按圆角画。建窗时虽 .shadow(false)，但上面的 setStyleMask 加
    //    NonactivatingPanel 会被 AppKit 把 hasShadow 重置成 YES → 透明矩形窗的原生矩形阴影露在
    //    圆角卡外/下方（截图翻译结果卡下方那块怪阴影）。显式设 NO，只保留 CSS 阴影。
    let _: () = msg_send![window, setHasShadow: false];
}

// ===== 浮窗关闭时把前台交还给"打开浮窗前的那个 App" =====
//
// 非激活 NSPanel 关闭（orderOut）时，AppKit 有时会把 Regular 策略的 Kivio 进程重新激活成
// 前台；此刻屏上只有浮窗（panel 不计入 hasVisibleWindows、也不在 USER_WINDOW_LABELS），
// 于是 main.rs 的 RunEvent::Reopen 分支会误判"无可见窗口"而 open_chat_window，凭空弹出 Chat。
// 解法：显示浮窗前快照当时的前台 App，关闭后把前台还给它 → Kivio 不会变成前台无窗口 →
// 那个误触的 Reopen 不再发生。这也顺带让 Esc 后正确回到用户原来的位置（Spotlight 式）。

/// 读取当前前台 App 的 PID（NSWorkspace 线程安全，可后台线程读）。取不到返回 0。
#[cfg(target_os = "macos")]
fn macos_frontmost_app_pid() -> i32 {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let workspace: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
        if workspace.is_null() {
            return 0;
        }
        let app: *mut Object = msg_send![workspace, frontmostApplication];
        if app.is_null() {
            return 0;
        }
        let pid: i32 = msg_send![app, processIdentifier];
        pid
    }
}

/// 把 `pid` 对应的 App 带回前台（主线程；激活属于 UI 操作）。
#[cfg(target_os = "macos")]
unsafe fn macos_activate_app(pid: i32) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    let running: *mut Object =
        msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid];
    if running.is_null() {
        return;
    }
    // NSApplicationActivateAllWindows = 1<<0：把上一个 App 带回前台。
    // activateWithOptions: 在 macOS 14+ 标记 deprecated，但经 objc msg_send 动态调用不受弃用属性
    // 影响、在 14/15 仍可用；用户发起的激活无需 IgnoringOtherApps 位。
    const NS_ACTIVATE_ALL_WINDOWS: u64 = 1 << 0;
    let _: bool = msg_send![running, activateWithOptions: NS_ACTIVATE_ALL_WINDOWS];
}

/// 显示浮窗前调用：记下当前前台 App 到给定槽。前台是 Kivio 自己（或取不到）时记 0 —— 不需要
/// 交还，而"Chat 在前"的情况由 RunEvent::Reopen 的 has_visible_windows=true 分支正确处理。
/// `slot`：lens 与输入翻译各用一个独立槽，避免两个浮窗同时存在时相互覆盖。
#[cfg(target_os = "macos")]
pub fn remember_frontmost_app(slot: &std::sync::atomic::AtomicI32) {
    use std::sync::atomic::Ordering;
    let pid = macos_frontmost_app_pid();
    let self_pid = std::process::id() as i32;
    let to_store = if pid > 0 && pid != self_pid { pid } else { 0 };
    slot.store(to_store, Ordering::SeqCst);
}

/// 冷创建隐藏 WebView 可能在它被重分类成非激活 NSPanel 之前短暂激活 Kivio，连带把普通 Chat
/// 窗口排到其他 App 前面。Panel 配置完成、真正显示之前调用本函数：若原前台 App 已被抢走，
/// 立刻把它重新激活，但不清空快照（关闭浮窗时仍可再次交还）。前台原本就是 Kivio 时槽为 0，
/// 因而不会隐藏、显示、聚焦或改变 Chat 窗口本身。
#[cfg(target_os = "macos")]
pub fn reassert_previous_frontmost_app(app: &AppHandle, slot: &std::sync::atomic::AtomicI32) {
    use std::sync::atomic::Ordering;
    let pid = slot.load(Ordering::SeqCst);
    if pid <= 0 {
        return;
    }

    // 大多数情况下非激活 Panel 从未抢走前台。此时再次 activate 原 App 会触发 macOS
    // 重排它的全部窗口，表现为按下 Lens 快捷键时桌面窗口整体闪一下。只有前台 PID
    // 确实发生变化时才执行恢复；显示后的第二次校正也因此保持无副作用。
    if macos_frontmost_app_pid() == pid {
        return;
    }

    let activate = move || unsafe { macos_activate_app(pid) };
    if macos_is_main_thread() {
        activate();
        return;
    }

    let (tx, rx) = std::sync::mpsc::channel();
    if app
        .run_on_main_thread(move || {
            activate();
            let _ = tx.send(());
        })
        .is_ok()
    {
        let _ = rx.recv_timeout(std::time::Duration::from_millis(250));
    }
}

/// 故意打开 Chat 的路径（open_chat_window / open_chat_settings_window）调用：清掉快照槽，避免
/// 随后的浮窗关闭把前台从刚打开的 Chat 又交还回旧 App。
#[cfg(target_os = "macos")]
pub fn forget_frontmost_app(slot: &std::sync::atomic::AtomicI32) {
    use std::sync::atomic::Ordering;
    slot.store(0, Ordering::SeqCst);
}

/// 关闭浮窗后调用：把前台交还给该槽里记的 App（取出即清零，幂等）。0 = 无需交还。
#[cfg(target_os = "macos")]
pub fn restore_previous_frontmost_app(app: &AppHandle, slot: &std::sync::atomic::AtomicI32) {
    use std::sync::atomic::Ordering;
    let pid = slot.swap(0, Ordering::SeqCst);
    if pid <= 0 {
        return;
    }
    // 非激活 Panel 正常工作时，原 App 从未失去前台。重复 activateWithOptions(AllWindows)
    // 会重排它的全部窗口并产生闪烁；只有前台确实变化时才需要交还。
    if macos_frontmost_app_pid() == pid {
        return;
    }
    let _ = app.run_on_main_thread(move || unsafe {
        macos_activate_app(pid);
    });
}
