use std::time::Duration;

use arboard::Clipboard;
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
      }
    })
}

/// 保存设置
/// 先对传入的设置进行清理（sanitize），然后应用开机自启动、重新注册热键、持久化设置、更新托盘菜单
/// 如果热键注册失败，则回滚运行时设置到之前的状态
#[tauri::command]
pub(crate) fn save_settings(
    app: AppHandle,
    state: State<AppState>,
    settings: Settings,
) -> Result<Settings, String> {
    apply_settings(&app, &state, settings)
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
fn apply_settings(
    app: &AppHandle,
    state: &State<AppState>,
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
        ) {
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
pub(crate) fn import_settings(
    app: AppHandle,
    state: State<AppState>,
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
    apply_settings(&app, &state, settings)
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

#[tauri::command]
pub(crate) async fn fetch_models(
    state: State<'_, AppState>,
    provider_id: String,
    provider: Option<ProviderConnectionInput>,
) -> Result<Vec<String>, String> {
    let settings = state.settings_read().clone();
    let request_override = provider.as_ref().and_then(|p| p.request.clone());
    let (base_url, api_keys) = resolve_provider_credentials(&settings, &provider_id, provider)?;
    let retry_attempts = effective_retry_attempts(&settings);
    let effective = effective_request_provider(&settings, &provider_id, request_override);

    if api_keys.is_empty() {
        return Err("Missing API Key".to_string());
    }

    let url = format!("{}/models", base_url.trim_end_matches('/'));

    let response = send_with_failover(
        &state,
        "Models API",
        retry_attempts,
        &provider_id,
        &api_keys,
        |key| {
            // 拉模型列表也走该供应商的请求配置：中转站常按自定义头/UA 决定放行与可见模型。
            let request = crate::provider_request::apply(
                state
                    .client_for(&effective)
                    .get(url.clone())
                    .bearer_auth(key),
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

    let models = value
        .get("data")
        .and_then(|data| data.as_array())
        .ok_or_else(|| "Invalid response format: expected 'data' array".to_string())?
        .iter()
        .filter_map(|m| {
            if let Some(s) = m.as_str() {
                Some(s.to_string())
            } else {
                m.get("id")
                    .and_then(|id| id.as_str())
                    .map(|s| s.to_string())
            }
        })
        .collect::<Vec<String>>();

    Ok(models)
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
    let api_format = provider
        .as_ref()
        .and_then(|p| p.api_format.as_deref())
        .map(ProviderApiFormat::from_raw)
        .or_else(|| {
            settings
                .get_provider(&provider_id)
                .map(|p| p.api_format_kind())
        })
        .unwrap_or(ProviderApiFormat::OpenAiChat);
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
                let request = with_request_config(client.post(url.clone())).json(&body);
                let request = match api_format {
                    ProviderApiFormat::AnthropicMessages => request
                        .header("x-api-key", &api_key)
                        .header("anthropic-version", "2023-06-01"),
                    ProviderApiFormat::Gemini => request.header("x-goog-api-key", &api_key),
                    _ => request.bearer_auth(&api_key),
                };
                with_standard_request_timeout(request).send()
            })
            .await
        }
        None => {
            // /models 探测（Gemini 原生同样有 GET /models；Anthropic 也提供 /models）。
            let url = format!("{base}/models");
            send_with_retry("Provider API", retry_attempts, || {
                let request = with_request_config(client.get(url.clone()));
                let request = match api_format {
                    ProviderApiFormat::AnthropicMessages => request
                        .header("x-api-key", &api_key)
                        .header("anthropic-version", "2023-06-01"),
                    ProviderApiFormat::Gemini => request.header("x-goog-api-key", &api_key),
                    _ => request.bearer_auth(&api_key),
                };
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
}
