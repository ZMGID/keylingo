use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use arboard::Clipboard;
use base64::Engine as _;
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_shell::ShellExt;

use crate::api::{
    call_openai_text, effective_retry_attempts, resolve_provider_credentials, send_with_failover,
    send_with_retry, with_standard_request_timeout, ProviderConnectionInput,
};
use crate::prompts::{
    build_translation_prompt, DEFAULT_REPLACE_TRANSLATION_TEMPLATE,
    DEFAULT_SCREENSHOT_TRANSLATION_TEMPLATE, DEFAULT_SELECTED_TEXT_TRANSLATION_TEMPLATE,
    DEFAULT_TRANSLATION_TEMPLATE,
};
use crate::rapidocr;
use crate::settings::{
    default_chat_system_prompt, default_lens_system_prompt, default_question_prompt,
    persist_settings, sanitize_settings, ProviderApiFormat, Settings,
};
#[cfg(target_os = "macos")]
use crate::shortcuts::{check_accessibility, check_screen_recording_permission};
use crate::shortcuts::{
    open_chat_settings_window as open_settings_window_impl, register_hotkeys,
    restore_runtime_settings, send_paste_shortcut, setup_tray,
};
use crate::state::AppState;
use crate::utils::{language_name, resolve_target_lang};
use crate::windows::get_main_window;

pub(crate) fn apply_launch_at_startup(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let auto_launch = app.autolaunch();
    let current = auto_launch.is_enabled().map_err(|e| e.to_string())?;

    if enabled && !current {
        auto_launch.enable().map_err(|e| e.to_string())?;
    } else if !enabled && current {
        auto_launch.disable().map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// 获取当前应用设置
#[tauri::command]
pub(crate) fn get_settings(state: State<AppState>) -> Settings {
    state.settings_read().clone()
}

/// 获取默认提示词模板
/// 返回翻译模板、截图翻译模板，以及 lens 视觉对话用的系统/提问提示词
#[tauri::command]
pub(crate) fn get_default_prompt_templates() -> serde_json::Value {
    serde_json::json!({
      "translationTemplate": DEFAULT_TRANSLATION_TEMPLATE,
      "screenshotTranslationTemplate": DEFAULT_SCREENSHOT_TRANSLATION_TEMPLATE,
      "selectedTextTranslationTemplate": DEFAULT_SELECTED_TEXT_TRANSLATION_TEMPLATE,
      "replaceTranslationTemplate": DEFAULT_REPLACE_TRANSLATION_TEMPLATE,
      "lensPrompts": {
        "zh": {
          "system": default_lens_system_prompt("zh", true),
          "question": default_question_prompt("zh", true)
        },
        "en": {
          "system": default_lens_system_prompt("en", true),
          "question": default_question_prompt("en", true)
        }
      },
      "chatPrompts": {
        "zh": default_chat_system_prompt(false),
        "en": default_chat_system_prompt(false)
      },
      // Single English source — same string the Chat runtime injects when system_prompt is empty.
      "chatRuntimePrompt": crate::chat::plan::chat_runtime_prompt()
    })
}

/// 保存设置
/// 先对传入的设置进行清理（sanitize），然后应用开机自启动、重新注册热键、持久化设置、更新托盘菜单
/// 如果热键注册失败，则回滚运行时设置到之前的状态
#[tauri::command]
pub(crate) async fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<Settings, String> {
    apply_settings(&app, &state, settings).await
}

/// trim + 去空 + 去重（保序）。
fn dedup_preserve_order(items: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let trimmed = item.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.clone()) {
            out.push(trimmed);
        }
    }
    out
}

/// 轻量持久化收藏模型："providerId:model" 列表。
/// 只更新内存 settings + 写盘，**不**走 apply_settings 的运行时重应用（热键/托盘/自启），
/// 因为收藏切换与这些无关，没必要承担其开销与副作用。
#[tauri::command]
pub(crate) fn set_favorite_models(
    app: AppHandle,
    state: State<AppState>,
    models: Vec<String>,
) -> Result<(), String> {
    let cleaned = dedup_preserve_order(models);
    let snapshot = {
        let mut guard = state.settings_write();
        guard.favorite_models = cleaned;
        guard.clone()
    };
    persist_settings(&app, &snapshot)
}

/// 轻量持久化快速翻译卡宽度（拖拽缩放的记忆；高度始终自动不持久化）。
/// 与 set_favorite_models 同理：不走 apply_settings 的热键/托盘重应用。
/// clamp 到 360–720 与设置页一致。
#[tauri::command]
pub(crate) fn set_translate_card_size(
    app: AppHandle,
    state: State<AppState>,
    width: u32,
) -> Result<(), String> {
    let clamped = width.clamp(360, 720);
    let snapshot = {
        let mut guard = state.settings_write();
        guard.screenshot_translation.card_width = clamped;
        guard.clone()
    };
    persist_settings(&app, &snapshot)?;
    // 通知可能开着的设置窗口同步草稿里的宽度，避免其随后 save_settings 用陈旧草稿覆盖掉这次拖拽。
    let _ = tauri::Emitter::emit_to(&app, "settings", "translate-card-width", clamped);
    Ok(())
}

/// sanitize → 应用运行时（自启/热键/托盘）→ 持久化，失败回滚。save_settings 与 import_settings 共用。
async fn apply_settings(
    app: &AppHandle,
    state: &State<'_, AppState>,
    settings: Settings,
) -> Result<Settings, String> {
    let previous_settings = state.settings_read().clone();
    let sanitized = sanitize_settings(settings);
    apply_launch_at_startup(app, sanitized.launch_at_startup)?;
    {
        let mut guard = state.settings_write();
        *guard = sanitized.clone();
    }
    state
        .sub_agents
        .set_concurrency(sanitized.chat_tools.sub_agent_concurrency);

    if let Err(err) = register_hotkeys(app) {
        // 热键被系统/其他应用占用不该阻断保存——能注册的已注册,失败的作为警告推给前端,
        // 设置照常落盘(否则用户连"删掉这个冲突热键"的改动都存不下)。
        let _ = tauri::Emitter::emit(app, "hotkey-warning", err);
    }

    let old_working_directory = &previous_settings.chat_tools.native_tools.working_directory;
    let new_working_directory = &sanitized.chat_tools.native_tools.working_directory;
    let workspace_root_changed = old_working_directory.trim() != new_working_directory.trim();
    if workspace_root_changed {
        if let Err(err) = crate::chat::storage::migrate_ordinary_conversation_workspaces(
            app,
            old_working_directory,
            new_working_directory,
        )
        .await
        {
            restore_runtime_settings(app, state, &previous_settings);
            return Err(format!("Failed to migrate conversation workspaces: {err}"));
        }
    }

    if let Err(err) = persist_settings(app, &sanitized) {
        eprintln!("Failed to save settings: {err}");
        if workspace_root_changed {
            if let Err(rollback_err) =
                crate::chat::storage::migrate_ordinary_conversation_workspaces(
                    app,
                    new_working_directory,
                    old_working_directory,
                )
                .await
            {
                eprintln!("Failed to roll back conversation workspace migration: {rollback_err}");
            }
        }
        restore_runtime_settings(app, state, &previous_settings);
        return Err(err);
    }

    let had_email = !previous_settings.email_accounts.is_empty();
    let has_email = !sanitized.email_accounts.is_empty();
    if has_email || had_email {
        if let Err(err) =
            crate::connectors::himalaya::sync_himalaya_config(&sanitized.email_accounts)
        {
            eprintln!("himalaya config sync: {err}");
        }
    }

    if let Err(err) = setup_tray(app) {
        eprintln!("Failed to update tray: {err}");
    }

    Ok(sanitized)
}

/// 设置备份文件格式版本。结构变化不兼容时递增。
const SETTINGS_BACKUP_VERSION: u32 = 1;

/// 导出全部设置（含供应商/模型配置与 API Key）到指定路径的 JSON 备份文件。
#[tauri::command]
pub(crate) fn export_settings(state: State<AppState>, path: String) -> Result<(), String> {
    let settings = state.settings_read().clone();
    let backup = serde_json::json!({
        "app": "kivio",
        "type": "settings-backup",
        "version": SETTINGS_BACKUP_VERSION,
        "settings": serde_json::to_value(&settings).map_err(|e| e.to_string())?,
    });
    let json = serde_json::to_string_pretty(&backup).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("写入失败: {e}"))?;
    Ok(())
}

/// 从备份文件导入设置，覆盖当前全部设置并立即生效（与保存同样走 sanitize/回滚）。
#[tauri::command]
pub(crate) async fn import_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<Settings, String> {
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("读取失败: {e}"))?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|_| "文件不是有效的 JSON".to_string())?;
    if value.get("type").and_then(|v| v.as_str()) != Some("settings-backup") {
        return Err("这不是 Kivio 设置备份文件".to_string());
    }
    let settings_value = value
        .get("settings")
        .ok_or_else(|| "备份文件缺少 settings 字段".to_string())?;
    let settings: Settings = serde_json::from_value(settings_value.clone())
        .map_err(|e| format!("备份内容无法解析: {e}"))?;
    apply_settings(&app, &state, settings).await
}

#[tauri::command]
pub(crate) fn open_settings_window(app: AppHandle) -> Result<(), String> {
    open_settings_window_impl(&app)
}

#[tauri::command]
pub(crate) fn close_translator_window(app: AppHandle, state: State<'_, AppState>) {
    if let Some(window) = get_main_window(&app) {
        #[cfg(target_os = "macos")]
        {
            crate::windows::destroy_overlay_window(&window);
            crate::windows::restore_previous_frontmost_app(&app, &state.prev_frontmost_pid_main);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = window.close();
    }
}

/// 翻译文本命令
/// 根据设置中的翻译供应商和模型进行翻译；如果 API Key 为空则返回提示信息
#[tauri::command]
pub(crate) async fn translate_text(
    state: State<'_, AppState>,
    text: String,
) -> Result<String, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok("".to_string());
    }

    let settings = state.settings_read().clone();
    let provider = settings
        .get_provider(&settings.translator_provider_id)
        .ok_or_else(|| "Translator provider not found".to_string())?;

    if provider.api_keys.is_empty() {
        return Ok("Missing API Key".to_string());
    }
    if settings.translator_model.trim().is_empty() {
        return Ok("Please select a model first".to_string());
    }

    let target_lang = resolve_target_lang(&settings.target_lang, trimmed);
    let lang_name = language_name(&target_lang).to_string();
    let prompt =
        build_translation_prompt(trimmed, &lang_name, settings.translator_prompt.as_deref());

    let retry_attempts = effective_retry_attempts(&settings);
    // 主翻译路径默认关思考：reasoning 模型对单句翻译几乎无质量收益但显著拖慢；非 reasoning 模型该字段被忽略
    call_openai_text(
        &state,
        provider,
        &settings.translator_model,
        prompt,
        retry_attempts,
        false,
        "translator",
        "translate_text",
    )
    .await
}

/// 提交翻译结果
/// 将翻译后的文本写入剪贴板，隐藏主窗口，如果启用了自动粘贴则发送粘贴快捷键到之前的应用
#[tauri::command]
pub(crate) async fn commit_translation(
    app: AppHandle,
    state: State<'_, AppState>,
    text: String,
) -> Result<(), String> {
    if text.trim().is_empty() {
        return Ok(());
    }

    let auto_paste = state.settings_read().auto_paste;
    let mut clipboard = Clipboard::new().map_err(|e| e.to_string())?;
    clipboard.set_text(text).map_err(|e| e.to_string())?;

    // commit 用下面的 [NSApp hide:] 把前台让回原 App（成熟路径）。先清掉翻译窗的前台快照，
    // 避免后续窗口事件再次驱动焦点交还。
    #[cfg(target_os = "macos")]
    crate::windows::forget_frontmost_app(&state.prev_frontmost_pid_main);

    // macOS 输入翻译窗口被重分类为 KivioOverlayPanel；必须先换回 TaoWindow 再 destroy，
    // 否则 WebKit 清理 contentLayoutRect KVO observer 时会抛 ObjC 异常并让 Rust abort。
    #[cfg(target_os = "macos")]
    if let Some(window) = get_main_window(&app) {
        crate::windows::destroy_overlay_window(&window);
    }

    // 其他平台没有 macOS TSM/IMK 的销毁问题，保持原有的关闭释放行为。
    #[cfg(not(target_os = "macos"))]
    if let Some(window) = get_main_window(&app) {
        let _ = window.close();
    }

    // 让前台还给原 App。[NSApp hide:] 是 AppKit，只能主线程调用。
    #[cfg(target_os = "macos")]
    crate::windows::hide_app_guarded(&app);

    if auto_paste {
        // 增加延迟以确保焦点切换完成
        tokio::time::sleep(Duration::from_millis(600)).await;
        send_paste_shortcut();
    }

    Ok(())
}

/// 读取 Rust 端在 lens_request_internal 中暂存的 selection 文本（peek，不清除）。
/// 不能"读一次清一次"：前端 enterSelect 在 React StrictMode（dev 双调）/ 冷挂载 / 复用事件等
/// 情况下可能被调用多次，take 会让第一次取走、随后被 reqId 作废丢弃，第二次拿到空 → 选区丢失。
/// 每次打开 lens 时 lens_request_internal 都会覆盖 pending_selection（有选区写文本、无选区写
/// None），所以读不清除不会产生跨次 stale：当前这次打开读到的始终是这次的值。
#[tauri::command]
pub(crate) fn take_lens_selection(state: State<'_, AppState>) -> Result<String, String> {
    match state.pending_selection.lock() {
        Ok(guard) => Ok(guard.clone().unwrap_or_default()),
        Err(_) => Ok(String::new()),
    }
}

/// 使用系统默认浏览器打开外部链接（仅限 http/https）
#[tauri::command]
#[allow(deprecated)]
pub(crate) fn open_external(app: AppHandle, url: String) -> Result<(), String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("Invalid URL".to_string());
    }

    app.shell().open(url, None).map_err(|e| e.to_string())
}

/// 模型输出里的本地文件链接**禁止**打开的扩展名。
///
/// 为什么是黑名单而不是白名单：白名单必漏——docx / xlsx / zip / mp4 / py 都是用户会点的正常
/// 文件，漏一个就是「点了没反应」，跟「在 Kivio 里打开」一样是坏的。真正需要挡的只有
/// 「默认处理器会**执行代码**」那一小类，那是可枚举的：`shell().open` 对 `.command` / `.app`
/// / Windows `.bat` 是执行语义，而 href 来自模型输出（不可信输入）。安装器（pkg/msi）一并挡,
/// 它们点一下就进安装流程。
///
/// 没有扩展名的文件也一律拒：类 Unix 下带执行位的裸文件名恰好是最危险的一种。
const EXECUTABLE_LOCAL_EXTENSIONS: &[&str] = &[
    // macOS / 类 Unix：脚本与可执行包
    "command",
    "sh",
    "bash",
    "zsh",
    "csh",
    "ksh",
    "fish",
    "app",
    "scpt",
    "applescript",
    "workflow",
    "terminal",
    "action",
    "jar",
    "py",
    "pyw",
    "rb",
    "pl",
    "php",
    "lua",
    // Windows：默认处理器就是执行
    "exe",
    "com",
    "bat",
    "cmd",
    "ps1",
    "psm1",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "hta",
    "scr",
    "cpl",
    "dll",
    "reg",
    "lnk",
    "inf",
    // 安装器：点一下就开始装
    "pkg",
    "mpkg",
    "msi",
    "msp",
    "dmg",
    "appx",
    "msix",
];

/// 扩展名闸门：`shell().open` 对这些是执行/安装语义，一律不开。空扩展名同样拒。
fn ensure_openable_extension(path: &std::path::Path) -> Result<(), String> {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    if ext.is_empty() || EXECUTABLE_LOCAL_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!(
            "为安全起见不直接打开这种文件（默认程序可能是执行它）：{}",
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        ));
    }
    Ok(())
}

/// `file://` URL / 绝对路径 / 相对路径 → 校验过的本地文件路径。
///
/// `base` 是相对路径的解析基准（会话工作目录）。给不出 base 时相对路径一律拒绝——猜一个
/// base 只会开错文件。给了 base 也要挡住 `../../` 逃逸：href 来自模型输出。
fn local_file_path_from_href(
    href: &str,
    base: Option<&std::path::Path>,
) -> Result<PathBuf, String> {
    let href = href.trim();
    let raw = if href.len() >= 7 && href[..7].eq_ignore_ascii_case("file://") {
        // 走 url crate 而不是手撕前缀：`file:///a%20b.html` 这类百分号编码得正确还原。
        url::Url::parse(href)
            .map_err(|e| format!("无法解析 file URL：{e}"))?
            .to_file_path()
            .map_err(|_| "file URL 不是本机路径".to_string())?
    } else {
        PathBuf::from(href)
    };
    let path = if raw.is_absolute() {
        raw
    } else {
        let base = base.ok_or_else(|| "无法定位这个相对路径对应的文件".to_string())?;
        let joined = base.join(&raw);
        // 逃逸检查放在存在性之前：canonicalize 要求文件真在，失败就当不存在处理。
        let real = joined
            .canonicalize()
            .map_err(|_| "文件不存在".to_string())?;
        let base_real = base
            .canonicalize()
            .map_err(|e| format!("无法解析工作目录：{e}"))?;
        if !real.starts_with(&base_real) {
            return Err("链接指向了工作目录之外".to_string());
        }
        real
    };
    ensure_openable_extension(&path)?;
    if !path.is_file() {
        return Err("文件不存在".to_string());
    }
    Ok(path)
}

/// 用系统默认程序打开**本地文件**（模型输出里的 `file://` / 绝对路径 / 相对路径链接）。
///
/// 与 `open_external`（只认 http(s)）分开而不是放宽它：把关方式完全不同——见
/// `local_file_path_from_href` 与 `EXECUTABLE_LOCAL_EXTENSIONS`。
///
/// 相对路径的基准取 `dock_resolve_cwd`——**与 agent 实际写文件的目录同一个解析器**（右侧
/// Dock 也用它）。CLI 写完文件常给一条 `assets/index.html` 这样的相对链接。
#[tauri::command]
#[allow(deprecated)]
pub(crate) async fn open_local_file(
    app: AppHandle,
    href: String,
    conversation_id: Option<String>,
) -> Result<(), String> {
    let needs_base = {
        let trimmed = href.trim();
        !(trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("file://"))
            && !std::path::Path::new(trimmed).is_absolute()
    };
    let base = if needs_base {
        let id = conversation_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        match crate::dock::dock_resolve_cwd(app.clone(), id, None).await {
            Ok(dir) => Some(PathBuf::from(dir)),
            Err(_) => None,
        }
    } else {
        None
    };
    let path = local_file_path_from_href(&href, base.as_deref())?;
    let path_str = path.to_str().ok_or_else(|| "路径不是 UTF-8".to_string())?;
    app.shell().open(path_str, None).map_err(|e| e.to_string())
}

/// data URL → 临时文件名（只保留 basename，挡住 `../` 与目录分隔符）。
fn temp_file_name_from_artifact_name(name: &str) -> String {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('.');
    if base.is_empty() {
        // 无名产物：故意不给扩展名 ⇒ 被 `ensure_openable_extension` 拒掉。不知道是什么就别开。
        "file".to_string()
    } else {
        base.to_string()
    }
}

/// 把内存里的 data URL 写成临时文件，再用系统默认程序打开。
///
/// 用于**没有 path 的旧 artifact**（新产物都带 path，走 `chat_open_generated_artifact`）。
/// 替代前端原来的 `window.open(dataUrl)`——那条在 Tauri 里由 webview 自己处理，不会交给
/// 默认程序（表现就是「点了在 Kivio 里打开 / 什么都没发生」）。
///
/// 扩展名同样过 `ensure_openable_extension`：这里是**先落盘再交给默认程序**，一个 `.command`
/// 产物落地就成了可执行文件。
#[tauri::command]
#[allow(deprecated)]
pub(crate) fn open_data_url_file(
    app: AppHandle,
    name: String,
    data_url: String,
) -> Result<(), String> {
    let payload = data_url
        .split_once(";base64,")
        .map(|(_, rest)| rest)
        .ok_or_else(|| "只支持 base64 的 data URL".to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.trim().as_bytes())
        .map_err(|e| format!("data URL 解码失败：{e}"))?;

    let file_name = temp_file_name_from_artifact_name(&name);
    // 每次一个独立子目录：同名产物不会互相覆盖，也不用担心撞到别的临时文件。
    let dir = std::env::temp_dir().join(format!("kivio-artifact-{}", uuid::Uuid::new_v4()));
    let path = dir.join(&file_name);
    ensure_openable_extension(&path)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建临时目录失败：{e}"))?;
    std::fs::write(&path, bytes).map_err(|e| format!("写临时文件失败：{e}"))?;

    let path_str = path.to_str().ok_or_else(|| "路径不是 UTF-8".to_string())?;
    app.shell().open(path_str, None).map_err(|e| e.to_string())
}

/// 将 Chat 里的 HTML 预览写成临时文件，并用系统默认浏览器打开。
#[tauri::command]
#[allow(deprecated)]
pub(crate) fn open_html_preview(app: AppHandle, html: String) -> Result<(), String> {
    let path =
        std::env::temp_dir().join(format!("kivio-html-preview-{}.html", uuid::Uuid::new_v4()));
    std::fs::write(&path, html).map_err(|e| format!("Write HTML preview failed: {e}"))?;
    let path_str = path
        .to_str()
        .ok_or_else(|| "Invalid HTML preview path".to_string())?;
    app.shell().open(path_str, None).map_err(|e| e.to_string())
}

// ===== RapidOCR 离线 OCR 命令 =====
//
// status: 检查 app data 目录里 RapidOCR 文件齐不齐(dylib + det/rec/keys),
// 前端据此决定是否渲染下载按钮。
// install: 顺序下载文件到 app data 目录,~150MB,前端转圈圈等返回。

/// 查询 RapidOCR 模型 + dylib 是否就绪。
/// async + spawn_blocking:validation_state 对未缓存文件做同步 SHA-256(~150MB),
/// 同步 command 会在主线程上执行并冻结 UI(每次启动后首个调用都会全量校验)。
#[tauri::command]
pub(crate) async fn rapidocr_status(
    state: State<'_, AppState>,
) -> Result<rapidocr::RapidOcrStatus, String> {
    let client = state.rapidocr.clone();
    tauri::async_runtime::spawn_blocking(move || client.status())
        .await
        .map_err(|e| format!("rapidocr status task failed: {e}"))
}

/// 下载 RapidOCR 包(onnxruntime dylib + PP-OCRv6 medium 模型 + 字典)到 app data 目录。
/// 阻塞到全部完成(成功或失败),前端转圈圈等返回。
#[tauri::command]
pub(crate) async fn rapidocr_install(
    state: State<'_, AppState>,
    tier: String,
) -> Result<rapidocr::RapidOcrInstallResult, String> {
    let client = state.rapidocr.clone();
    let tier = crate::offline_models::OcrModelTier::parse(&tier);
    Ok(client.install(tier).await)
}

/// 查询替换翻译完整离线包（ONNX Runtime + RapidOCR + MI-GAN）的校验状态与实际字节数。
/// async + spawn_blocking:同 rapidocr_status,SHA-256 校验不能占用主线程。
#[tauri::command]
pub(crate) async fn replace_translation_pack_status(
    state: State<'_, AppState>,
    tier: String,
) -> Result<crate::offline_models::ReplaceTranslationPackStatus, String> {
    let manager = state.offline_models.clone();
    let tier = crate::offline_models::OcrModelTier::parse(&tier);
    tauri::async_runtime::spawn_blocking(move || manager.replace_translation_status(tier))
        .await
        .map_err(|e| format!("replace translation pack status task failed: {e}"))
}

/// 显式安装替换翻译离线包。替换翻译执行路径本身不会触发任何静默下载。
#[tauri::command]
pub(crate) async fn replace_translation_pack_install(
    state: State<'_, AppState>,
    tier: String,
) -> Result<crate::offline_models::OfflineModelInstallResult, String> {
    let manager = state.offline_models.clone();
    let tier = crate::offline_models::OcrModelTier::parse(&tier);
    Ok(manager.install_replace_translation(tier).await)
}

/// 拼一个只用来读「请求配置」的临时 provider：优先前端传来的编辑中配置，缺省回落已保存的；
/// 供应商都还没保存过时给一份默认值（跟随系统代理、无自定义头）。
///
/// `provider_request::apply` / `AppState::client_for` 只读 `request` 字段，其余字段是什么
/// 不影响结果，所以这里用 `Default` 填充而不是去构造一份真实配置。
fn effective_request_provider(
    settings: &crate::settings::Settings,
    provider_id: &str,
    request_override: Option<crate::settings::ProviderRequestConfig>,
) -> crate::settings::ModelProvider {
    let mut provider = settings
        .get_provider(provider_id)
        .cloned()
        .unwrap_or_else(|| crate::settings::ModelProvider {
            id: provider_id.to_string(),
            name: String::new(),
            api_keys: Vec::new(),
            api_key_legacy: None,
            base_url: String::new(),
            available_models: Vec::new(),
            enabled_models: Vec::new(),
            enabled: true,
            api_format: "openai_chat".to_string(),
            model_overrides: Default::default(),
            compress_request_body: false,
            request: Default::default(),
        });
    if let Some(request) = request_override {
        provider.request = request;
    }
    // 设置文件可被手改，前端也可能传来没过校验的草稿，这里按发送前的规则再滤一遍。
    provider
        .request
        .custom_headers
        .retain(crate::provider_request::is_usable_header);
    provider
}

/// 拉模型列表 / 测连接共用的鉴权：必须跟真实对话请求一致。
/// Gemini 官方 key 不是 OAuth token，Bearer 会 400/401；Anthropic 要 `x-api-key`。
fn apply_provider_auth(
    request: reqwest::RequestBuilder,
    api_format: ProviderApiFormat,
    api_key: &str,
) -> reqwest::RequestBuilder {
    match api_format {
        ProviderApiFormat::AnthropicMessages => request
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01"),
        ProviderApiFormat::Gemini => request.header("x-goog-api-key", api_key),
        _ => request.bearer_auth(api_key),
    }
}

fn resolve_api_format(
    settings: &Settings,
    provider_id: &str,
    provider: Option<&ProviderConnectionInput>,
) -> ProviderApiFormat {
    provider
        .and_then(|p| p.api_format.as_deref())
        .map(ProviderApiFormat::from_raw)
        .or_else(|| {
            settings
                .get_provider(provider_id)
                .map(|p| p.api_format_kind())
        })
        .unwrap_or(ProviderApiFormat::OpenAiChat)
}

/// 从 `/models` JSON 抽出模型 id。
/// OpenAI / Anthropic / Responses：`data[].id`（或 `data[]` 字符串）。
/// Gemini 原生 ListModels：`models[].name`，形如 `models/gemini-2.5-flash`。
fn parse_model_list_ids(value: &serde_json::Value) -> Result<Vec<String>, String> {
    if let Some(msg) = models_api_error_message(value) {
        return Err(format!("Models API error: {msg}"));
    }

    let items = value
        .get("data")
        .and_then(|v| v.as_array())
        .or_else(|| value.get("models").and_then(|v| v.as_array()))
        .or_else(|| value.as_array())
        .ok_or_else(|| {
            "Invalid response format: expected a 'data' or 'models' array".to_string()
        })?;

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for item in items {
        if !model_item_is_listable(item) {
            continue;
        }
        let Some(id) = model_item_id(item) else {
            continue;
        };
        if seen.insert(id.clone()) {
            out.push(id);
        }
    }
    Ok(out)
}

fn models_api_error_message(value: &serde_json::Value) -> Option<String> {
    let err = value.get("error")?;
    err.as_str()
        .map(str::to_string)
        .or_else(|| {
            err.get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .filter(|msg| !msg.trim().is_empty())
}

fn model_item_id(item: &serde_json::Value) -> Option<String> {
    if let Some(s) = item.as_str() {
        return normalize_listed_model_id(s);
    }
    item.get("id")
        .and_then(|v| v.as_str())
        .or_else(|| item.get("name").and_then(|v| v.as_str()))
        .and_then(normalize_listed_model_id)
}

fn normalize_listed_model_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Gemini ListModels 的 `name` 是 `models/gemini-2.5-flash`；对话 URL 会再拼一层
    // `/models/`，这里去掉前缀，和 generateContent 路径去重保持同一口径。
    let stripped = trimmed.strip_prefix("models/").unwrap_or(trimmed).trim();
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}

/// Gemini 原生条目带 `supportedGenerationMethods` 时，只收能 generate 的模型
///（embedding-only 进聊天选择器没有意义）。中转站 / OpenAI 形没有该字段，原样保留。
fn model_item_is_listable(item: &serde_json::Value) -> bool {
    let Some(methods) = item
        .get("supportedGenerationMethods")
        .and_then(|v| v.as_array())
    else {
        return true;
    };
    methods.iter().any(|method| {
        matches!(
            method.as_str(),
            Some("generateContent")
                | Some("streamGenerateContent")
                | Some("bidiGenerateContent")
        )
    })
}

#[tauri::command]
pub(crate) async fn fetch_models(
    state: State<'_, AppState>,
    provider_id: String,
    provider: Option<ProviderConnectionInput>,
) -> Result<Vec<String>, String> {
    let settings = state.settings_read().clone();
    let api_format = resolve_api_format(&settings, &provider_id, provider.as_ref());
    let request_override = provider.as_ref().and_then(|p| p.request.clone());
    let (base_url, api_keys) = resolve_provider_credentials(&settings, &provider_id, provider)?;
    let retry_attempts = effective_retry_attempts(&settings);
    let effective = effective_request_provider(&settings, &provider_id, request_override);

    if api_keys.is_empty() {
        return Err("Missing API Key".to_string());
    }

    let base = base_url.trim_end_matches('/');
    // Gemini 原生 ListModels 默认每页约 50；官方上限 1000。中转站一般忽略未知 query。
    let url = match api_format {
        ProviderApiFormat::Gemini => format!("{base}/models?pageSize=1000"),
        _ => format!("{base}/models"),
    };

    let response = send_with_failover(
        &state,
        "Models API",
        retry_attempts,
        &provider_id,
        &api_keys,
        |key| {
            // 拉模型列表也走该供应商的请求配置：中转站常按自定义头/UA 决定放行与可见模型。
            let request = crate::provider_request::apply(
                apply_provider_auth(
                    state.client_for(&effective).get(url.clone()),
                    api_format,
                    key,
                ),
                &effective,
                None,
            );
            with_standard_request_timeout(request).send()
        },
    )
    .await?;

    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse models response JSON: {e}"))?;

    parse_model_list_ids(&value)
}

/// 测试供应商连接是否可用
/// 多 key：测试时只用第一个 key（避免一次连接测试遍历多 key 让用户困惑）
/// 有 model 时发一条极小对话请求，比 /models 更能反映“能不能调模型”，
/// 也不依赖供应商支持 /models；无 model 时回退到 /models 探测。
/// 请求按供应商的 api_format 走对应协议（URL + 鉴权 + body），
/// 否则 Anthropic/Gemini/Responses 供应商会对本可用的模型误报失败。
#[tauri::command]
pub(crate) async fn test_provider_connection(
    state: State<'_, AppState>,
    provider_id: String,
    provider: Option<ProviderConnectionInput>,
) -> Result<serde_json::Value, String> {
    let settings = state.settings_read().clone();
    let model = provider.as_ref().and_then(|p| p.model.clone());
    // 协议优先取前端传入（未保存的编辑中配置），缺省回退 settings 里已保存的。
    let api_format = resolve_api_format(&settings, &provider_id, provider.as_ref());
    let request_override = provider.as_ref().and_then(|p| p.request.clone());
    let (base_url, api_keys) = resolve_provider_credentials(&settings, &provider_id, provider)?;

    let api_key = match api_keys.first() {
        Some(k) if !k.trim().is_empty() => k.clone(),
        _ => {
            return Ok(serde_json::json!({
              "success": false,
              "error": "Missing API Key"
            }));
        }
    };

    let retry_attempts = effective_retry_attempts(&settings);
    let base = base_url.trim_end_matches('/');
    // 测试连接必须和真实请求带一样的头（网关常按 UA / 自定义头放行），
    // 否则这里通过、聊天时 403，用户完全查不出原因。
    // 用 `effective` 这个临时 provider：优先取前端传来的编辑中配置，缺省回落已保存的。
    let effective = effective_request_provider(&settings, &provider_id, request_override);
    let with_request_config = |request: reqwest::RequestBuilder| {
        crate::provider_request::apply(request, &effective, None)
    };
    let client = state.client_for(&effective);

    let result = match model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        Some(model) => {
            let (url, body) = match api_format {
                ProviderApiFormat::AnthropicMessages => (
                    format!("{base}/messages"),
                    serde_json::json!({
                        "model": model,
                        "messages": [{ "role": "user", "content": "hi" }],
                        "max_tokens": 1,
                    }),
                ),
                ProviderApiFormat::Gemini => (
                    format!(
                        "{base}/models/{}:generateContent",
                        model.trim_start_matches("models/")
                    ),
                    serde_json::json!({
                        "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
                        "generationConfig": { "maxOutputTokens": 1 },
                    }),
                ),
                // xAI 与 OpenAI 的 Responses 端点同形，测试请求体本来就只有这三个字段，
                // 都不在 xAI 的拒收名单上，无需分叉。
                ProviderApiFormat::OpenAiResponses | ProviderApiFormat::XaiResponses => (
                    format!("{base}/responses"),
                    serde_json::json!({
                        "model": model,
                        "input": "hi",
                        "max_output_tokens": 16,
                    }),
                ),
                ProviderApiFormat::OpenAiChat => (
                    format!("{base}/chat/completions"),
                    serde_json::json!({
                        "model": model,
                        "messages": [{ "role": "user", "content": "hi" }],
                        "max_tokens": 1,
                    }),
                ),
            };
            send_with_retry("Provider API", retry_attempts, || {
                let request = apply_provider_auth(
                    with_request_config(client.post(url.clone())).json(&body),
                    api_format,
                    &api_key,
                );
                with_standard_request_timeout(request).send()
            })
            .await
        }
        None => {
            // /models 探测（Gemini 原生同样有 GET /models；Anthropic 也提供 /models）。
            let url = format!("{base}/models");
            send_with_retry("Provider API", retry_attempts, || {
                let request = apply_provider_auth(
                    with_request_config(client.get(url.clone())),
                    api_format,
                    &api_key,
                );
                with_standard_request_timeout(request).send()
            })
            .await
        }
    };

    match result {
        Ok(_) => Ok(serde_json::json!({ "success": true })),
        Err(err) => Ok(serde_json::json!({ "success": false, "error": err })),
    }
}

/// 测试网络搜索：用传入的（可能未保存的）配置真实跑一次搜索，返回结果或错误。
/// 供设置页「测试搜索」用，验证 key/endpoint 是否可用。
#[tauri::command]
pub(crate) async fn test_web_search(
    state: State<'_, AppState>,
    config: crate::settings::LensWebSearchConfig,
    query: String,
) -> Result<serde_json::Value, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(serde_json::json!({ "success": false, "error": "Empty query" }));
    }
    let settings = state.settings_read().clone();
    let retry_attempts = effective_retry_attempts(&settings);
    match crate::web_search::search_web(&state, &config, query, retry_attempts).await {
        Ok(results) => Ok(serde_json::json!({
            "success": true,
            "provider": crate::web_search::provider_label(config.provider),
            "results": results,
        })),
        Err(err) => Ok(serde_json::json!({ "success": false, "error": err })),
    }
}

/// 获取平台权限状态（仅限 macOS：辅助功能和屏幕录制权限）
#[tauri::command]
pub(crate) fn get_permission_status() -> serde_json::Value {
    #[cfg(target_os = "macos")]
    {
        let accessibility = check_accessibility(false);
        let screen_recording = check_screen_recording_permission();
        return serde_json::json!({
          "platform": "macos",
          "accessibility": accessibility,
          "screenRecording": screen_recording,
        });
    }

    #[cfg(not(target_os = "macos"))]
    {
        serde_json::json!({
          "platform": "other",
          "accessibility": true,
          "screenRecording": true,
        })
    }
}

/// 打开系统权限设置面板（仅限 macOS）
#[tauri::command]
pub(crate) fn open_permission_settings(kind: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;

        let target = match kind.as_str() {
            "accessibility" => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            "screen-recording" => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
            }
            _ => return Err("Unsupported permission kind".to_string()),
        };

        Command::new("open")
            .arg(target)
            .output()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = kind;
        Err("Permission settings are only available on macOS".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::dedup_preserve_order;
    use super::local_file_path_from_href;
    use super::parse_model_list_ids;
    use serde_json::json;

    /// 旧 artifact（无 path）落临时文件时的文件名清洗：只取 basename，挡目录穿越。
    /// 扩展名闸门与本地文件链接共用 `ensure_openable_extension`——落盘后同样是「交给默认程序」。
    #[test]
    fn artifact_temp_name_keeps_basename_and_refuses_executables() {
        use super::{ensure_openable_extension, temp_file_name_from_artifact_name};
        use std::path::Path;

        assert_eq!(
            temp_file_name_from_artifact_name("report.docx"),
            "report.docx"
        );
        // 目录穿越：只保留最后一段。
        assert_eq!(
            temp_file_name_from_artifact_name("../../etc/passwd.txt"),
            "passwd.txt"
        );
        assert_eq!(temp_file_name_from_artifact_name("a\\b\\c.csv"), "c.csv");
        // 空 / 纯点：给个不带扩展名的名字，随后被扩展名闸门拒掉（不知道是什么就别开）。
        assert_eq!(temp_file_name_from_artifact_name("  "), "file");

        assert!(ensure_openable_extension(Path::new("/t/report.docx")).is_ok());
        assert!(ensure_openable_extension(Path::new("/t/x.command")).is_err());
        assert!(ensure_openable_extension(Path::new("/t/file")).is_err());
    }

    ///
    /// 钉住两件事：① 正常文件（不止 html —— docx/zip/csv 这些都得能开）都放行；
    /// ② `shell().open` 对 `.command` 是**执行**语义，而 href 来自模型输出 ⇒ 必须拒。
    #[test]
    fn local_file_href_opens_ordinary_files_and_refuses_executables() {
        let dir = std::env::temp_dir().join(format!("kivio-open-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let page = dir.join("a b.html");
        std::fs::write(&page, "<html></html>").unwrap();

        // file:// + 百分号编码的空格必须还原成真实路径。
        let url = url::Url::from_file_path(&page).unwrap();
        assert!(url.as_str().contains("%20"), "样本应带百分号编码：{url}");
        assert_eq!(local_file_path_from_href(url.as_str(), None).unwrap(), page);
        // 裸绝对路径同样放行。
        assert_eq!(
            local_file_path_from_href(page.to_str().unwrap(), None).unwrap(),
            page
        );

        // 不止 html：常见的「交给默认程序」文件都要放行。
        for name in [
            "report.docx",
            "data.xlsx",
            "bundle.zip",
            "clip.mp4",
            "notes.md",
        ] {
            let file = dir.join(name);
            std::fs::write(&file, "x").unwrap();
            assert!(
                local_file_path_from_href(file.to_str().unwrap(), None).is_ok(),
                "{name} 应该能用默认程序打开"
            );
        }

        // 默认处理器会执行的一类：即便文件真存在也不许开。
        for name in ["run.command", "install.pkg", "go.bat", "tool.app", "s.py"] {
            let file = dir.join(name);
            std::fs::write(&file, "x").unwrap();
            assert!(
                local_file_path_from_href(file.to_str().unwrap(), None).is_err(),
                "{name} 的默认程序可能是执行它，必须拒"
            );
        }
        // 没有扩展名：类 Unix 下带执行位的裸文件名最危险。
        let bare = dir.join("runme");
        std::fs::write(&bare, "x").unwrap();
        assert!(local_file_path_from_href(bare.to_str().unwrap(), None).is_err());

        // 相对路径：给了会话工作目录才解析，且不许越出它（href 是模型输出）。
        assert_eq!(
            local_file_path_from_href("a b.html", Some(&dir)).unwrap(),
            page.canonicalize().unwrap()
        );
        assert!(
            local_file_path_from_href("a b.html", None).is_err(),
            "没有基准目录时不许猜"
        );
        assert!(
            local_file_path_from_href("../../etc/hosts", Some(&dir)).is_err(),
            "../ 逃逸必须挡住"
        );
        // 存在性：白名单内但文件不存在。
        assert!(
            local_file_path_from_href(dir.join("missing.html").to_str().unwrap(), None).is_err()
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dedup_preserve_order_trims_dedups_keeps_order() {
        let out = dedup_preserve_order(vec![
            " p1:m ".into(),
            "p2:m".into(),
            "p1:m".into(), // 重复 → 去掉
            "".into(),     // 空 → 去掉
            "   ".into(),  // 纯空白 → 去掉
            "p3:m".into(),
        ]);
        assert_eq!(out, vec!["p1:m", "p2:m", "p3:m"]);
    }

    #[test]
    fn dedup_preserve_order_empty() {
        assert!(dedup_preserve_order(vec![]).is_empty());
    }

    #[test]
    fn parse_model_list_openai_data_ids() {
        let value = json!({
            "object": "list",
            "data": [
                { "id": "gpt-4o", "object": "model" },
                { "id": "gpt-4o-mini" },
                "o4-mini"
            ]
        });
        assert_eq!(
            parse_model_list_ids(&value).unwrap(),
            vec!["gpt-4o", "gpt-4o-mini", "o4-mini"]
        );
    }

    #[test]
    fn parse_model_list_gemini_native_names_and_filters_embeddings() {
        let value = json!({
            "models": [
                {
                    "name": "models/gemini-2.5-flash",
                    "supportedGenerationMethods": ["generateContent", "countTokens"]
                },
                {
                    "name": "models/gemini-embedding-001",
                    "supportedGenerationMethods": ["embedContent", "countTokens"]
                },
                {
                    "name": "models/gemini-2.5-pro",
                    "supportedGenerationMethods": ["generateContent"]
                },
                { "name": "models/" },
                "models/gemini-2.0-flash"
            ]
        });
        assert_eq!(
            parse_model_list_ids(&value).unwrap(),
            vec!["gemini-2.5-flash", "gemini-2.5-pro", "gemini-2.0-flash"]
        );
    }

    #[test]
    fn parse_model_list_prefers_data_over_models_and_dedups() {
        let value = json!({
            "data": [
                { "id": "gemini-2.5-flash" },
                { "name": "models/gemini-2.5-flash" }
            ]
        });
        assert_eq!(
            parse_model_list_ids(&value).unwrap(),
            vec!["gemini-2.5-flash"]
        );
    }

    #[test]
    fn parse_model_list_surfaces_gemini_error_object() {
        let value = json!({
            "error": { "code": 400, "message": "API key not valid. Please pass a valid API key.", "status": "INVALID_ARGUMENT" }
        });
        let err = parse_model_list_ids(&value).unwrap_err();
        assert!(err.contains("API key not valid"), "{err}");
    }

    #[test]
    fn parse_model_list_rejects_unknown_shape() {
        let err = parse_model_list_ids(&json!({ "hello": "world" })).unwrap_err();
        assert!(err.contains("data") && err.contains("models"), "{err}");
    }
}
