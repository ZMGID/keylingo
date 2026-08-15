#![cfg_attr(target_os = "macos", allow(unexpected_cfgs))]

pub mod agents;
pub mod api;
pub mod app_data;
pub mod capture_geometry;
pub mod chat;
pub mod commands;
pub mod connectors;
pub mod dock;
pub mod external_agents;
pub mod fonts;
pub mod inpainting;
pub mod lens;
pub mod lens_commands;
#[cfg(target_os = "macos")]
pub mod macos_ocr;
pub mod mcp;
pub mod native_tools;
pub mod notes;
pub mod offline_models;
pub mod path_env;
pub mod plugins;
pub mod proc;
pub mod prompts;
pub mod provider_request;
pub mod rapidocr;
pub mod replace_translation;
#[cfg(target_os = "macos")]
pub mod sck;
pub mod screenshot;
pub mod settings;
pub mod shortcuts;
pub mod skills;
pub mod state;
pub mod updates;
pub mod usage;
pub mod utils;
pub mod web_search;
pub mod windows;
#[cfg(target_os = "windows")]
pub mod windows_ocr;

use std::time::Duration;

use futures::StreamExt;
use tauri::{Emitter, Manager, State};
#[cfg(target_os = "macos")]
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_single_instance::init as init_single_instance;

use api::build_http_client;
use commands::apply_launch_at_startup;
use native_tools::cleanup_stale_sandbox_exports;
use screenshot::cleanup_orphan_temp_files;
use settings::load_settings;
#[cfg(target_os = "macos")]
use shortcuts::set_macos_regular_activation_policy;
use shortcuts::{
    display_hotkey_errors, open_chat_window, open_settings_window_for_activation, register_hotkeys,
    setup_tray,
};
use state::AppState;
use updates::check_github_latest_release;
#[cfg(target_os = "macos")]
use windows::{ensure_overlay_panel, restore_previous_frontmost_app};

/// 自启动参数，用于区分用户手动启动和系统自动启动
const AUTOSTART_ARG: &str = "--from-autostart";
/// Bound startup process/memory spikes when many MCP servers are enabled.
const MCP_STARTUP_WARMUP_CONCURRENCY: usize = 2;

#[cfg(target_os = "macos")]
const USER_WINDOW_LABELS: &[&str] = &["chat", "main"];

#[cfg(target_os = "macos")]
fn first_visible_user_window(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    USER_WINDOW_LABELS.iter().find_map(|label| {
        app.get_webview_window(label)
            .filter(|window| window.is_visible().ok().unwrap_or(false))
    })
}

/// Windows：让本进程退出 EcoQoS 执行速度节流，使其在无窗口/后台空闲时仍保持正常调度，
/// 避免全局热键的 WM_HOTKEY 消息泵被饿死。ControlMask=EXECUTION_SPEED + StateMask=0 表示
/// "由本进程接管该项节流并关闭它"。best-effort：失败静默忽略。
#[cfg(target_os = "windows")]
fn disable_process_power_throttling() {
    use ::windows::Win32::System::Threading::{
        GetCurrentProcess, ProcessPowerThrottling, SetProcessInformation,
        PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        PROCESS_POWER_THROTTLING_STATE,
    };
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: 0,
    };
    unsafe {
        let _ = SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &state as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
    }
}

/// 应用入口函数
/// 初始化 Tauri Builder，加载插件，配置窗口事件处理，设置全局状态、热键和托盘
pub fn run() {
    // GUI launches don't always inherit the *current* user PATH, so packaged
    // builds can't find user-installed CLIs. Enrich the process PATH once,
    // before any window creation or CLI probing. macOS: login-shell PATH is not
    // inherited from Finder/Dock. Windows: explorer's PATH is a stale login-time
    // snapshot, so read the current value from the registry, then also run the
    // user's PowerShell profile to pick up version-manager dirs (fnm/nvm) that
    // never touch the registry. No-op on Linux. See `path_env` module docs.
    #[cfg(target_os = "macos")]
    path_env::enrich_path_macos();
    #[cfg(target_os = "windows")]
    path_env::enrich_path_windows();

    let autostart_plugin = {
        #[cfg(target_os = "macos")]
        {
            tauri_plugin_autostart::Builder::new()
                .arg(AUTOSTART_ARG)
                .macos_launcher(MacosLauncher::LaunchAgent)
                .build()
        }
        #[cfg(not(target_os = "macos"))]
        {
            tauri_plugin_autostart::Builder::new()
                .arg(AUTOSTART_ARG)
                .build()
        }
    };

    tauri::Builder::default()
        .plugin(init_single_instance(|app, _args, _cwd| {
            if let Err(err) = open_settings_window_for_activation(app) {
                eprintln!("Single-instance activation failed: {err}");
            }
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(autostart_plugin)
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                if window.label() == "lens" || window.label() == "translate" {
                    api.prevent_close();
                    // Windows：原生关闭（Alt+F4 等 WM_CLOSE）也要走完整清理 + destroy，回收内存，
                    // 不留隐藏僵尸 overlay（active_overlay_window 只认可见窗，hide 掉的再也销毁不到）。
                    #[cfg(target_os = "windows")]
                    {
                        let _ = lens_commands::lens_close(window.app_handle().clone());
                    }
                    // macOS：换回原类后可安全 destroy（见 windows::destroy_overlay_window），
                    // 原生关闭也走完整清理 + destroy 回收内存，不留隐藏僵尸 overlay。
                    #[cfg(not(target_os = "windows"))]
                    {
                        let _ = lens_commands::lens_close(window.app_handle().clone());
                    }
                    return;
                }
                // 翻译窗（main）若仍收到默认 CloseRequested，必须拦截并走安全销毁：先恢复
                // TaoWindow 原类，再 destroy WebView/NSPanel，同时把前台交还给打开它之前的 App。
                #[cfg(target_os = "macos")]
                if window.label() == "main" {
                    api.prevent_close();
                    let handle = window.app_handle();
                    let st = handle.state::<AppState>();
                    restore_previous_frontmost_app(handle, &st.prev_frontmost_pid_main);
                    if let Some(webview_window) = handle.get_webview_window("main") {
                        windows::destroy_overlay_window(&webview_window);
                    }
                    return;
                }
            }
            tauri::WindowEvent::Focused(true) =>
            {
                #[cfg(target_os = "macos")]
                if window.label() == "lens" || window.label() == "translate" {
                    if let Some(webview_window) =
                        window.app_handle().get_webview_window(window.label())
                    {
                        ensure_overlay_panel(&webview_window);
                    }
                }
            }
            tauri::WindowEvent::Destroyed => {
                // macOS：Dock 图标身份由 Chat 窗口撑起（open/reveal 时切 Regular）。Chat
                // 销毁后切回 Accessory 隐藏 Dock 图标，回到后台常驻形态；其余窗口
                // （translator/lens/translate）本就是 Accessory 友好的浮层，不占 Dock。
                // 下次打开 Chat 时 open_chat_window/reveal_chat_window 会再切回 Regular。
                #[cfg(target_os = "macos")]
                if window.label() == "chat" {
                    let _ = window
                        .app_handle()
                        .set_activation_policy(tauri::ActivationPolicy::Accessory);
                }
            }
            _ => {}
        })
        .setup(|app| {
            let launched_from_autostart = std::env::args().any(|arg| arg == AUTOSTART_ARG);

            // Windows：退出后台执行速度节流（EcoQoS）。本应用所有窗口都可关闭（关闭即销毁,
            // 空闲降到 ~50MB），无窗口时进程会被 Win11 当后台空闲进程节流,饿死全局热键的
            // WM_HOTKEY 消息泵 → 热键失灵（托盘点击是 shell 唤醒故仍可用）。退出 EXECUTION_SPEED
            // 节流后,即便无窗口空闲也保持正常调度,热键消息泵持续工作。
            #[cfg(target_os = "windows")]
            disable_process_power_throttling();

            #[cfg(target_os = "macos")]
            {
                if launched_from_autostart {
                    let _ = app
                        .handle()
                        .set_activation_policy(tauri::ActivationPolicy::Accessory);
                } else {
                    set_macos_regular_activation_policy(app.handle());
                }
            }

            // 清理上次崩溃 / 强杀 / 旧版本遗留的截图 PNG（24h 之前的，避免误删并发实例的活文件）
            cleanup_orphan_temp_files();
            cleanup_stale_sandbox_exports();
            // 会话副产物：空/孤儿工作区目录、已删会话残留的附件目录。只碰 Kivio 自己造的
            // `conv_*` 目录，非空的孤儿工作区只报数不删（里面是用户产物）。
            chat::gc::sweep_conversation_side_artifacts(app.handle());

            // 周期性回收闲置的持久外部 CLI 会话（10 分钟无活动即丢弃 → actor 关闭其子进程），
            // 避免长时间挂着空转进程占内存。注册时也会做一次清扫 + LRU 限流，这里覆盖纯闲置场景。
            {
                let sweeper = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(120)).await;
                        // 启动竞态加固：AppState 可能尚未 manage（极早期 tick），此时跳过本轮，
                        // 120s 后再试，避免 state() panic。
                        let Some(state) = sweeper.try_state::<AppState>() else {
                            continue;
                        };
                        state.sweep_idle_external_live_sessions(std::time::Duration::from_secs(600));
                    }
                });
            }

            let mut settings = load_settings(&app.handle());
            // 一次性内置专家迁移（v1）：清空旧专家索引（含用户自建——用户明确选择），
            // 装入 4 个内置专家（写作/编程/研究/数据）。靠 settings flag 幂等，成功后立即写盘，
            // 否则下次启动会再次覆盖。持久化失败则回滚内存 flag，下次重试（仍是覆盖为这 4 个，可接受）。
            if !settings.builtin_assistants_seeded_v1 {
                let now = chrono::Local::now().timestamp();
                match chat::storage::seed_builtin_assistants_v1(&app.handle(), now) {
                    Ok(()) => {
                        settings.builtin_assistants_seeded_v1 = true;
                        if let Err(err) = settings::persist_settings(&app.handle(), &settings) {
                            eprintln!(
                                "Failed to persist settings after seeding built-in assistants: {err}"
                            );
                            settings.builtin_assistants_seeded_v1 = false;
                        }
                    }
                    Err(err) => eprintln!("Failed to seed built-in assistants: {err}"),
                }
            }
            // 非破坏性内置专家迁移（v2）：按 id upsert 新一批内置专家（升级 4 个 + 新增前端/翻译/文档），
            // 保留用户自建。已 seed v1 的老用户靠它拿到新专家；新装用户 v1 已装全套，此处为幂等 no-op。
            // 靠 settings flag 幂等，成功后立即写盘；持久化失败则回滚 flag 下次重试（merge 保留用户项，重试安全）。
            if !settings.builtin_assistants_seeded_v2 {
                let now = chrono::Local::now().timestamp();
                match chat::storage::merge_builtin_assistants_v2(&app.handle(), now) {
                    Ok(()) => {
                        settings.builtin_assistants_seeded_v2 = true;
                        if let Err(err) = settings::persist_settings(&app.handle(), &settings) {
                            eprintln!(
                                "Failed to persist settings after merging built-in assistants v2: {err}"
                            );
                            settings.builtin_assistants_seeded_v2 = false;
                        }
                    }
                    Err(err) => eprintln!("Failed to merge built-in assistants v2: {err}"),
                }
            }
            if let Err(err) = apply_launch_at_startup(&app.handle(), settings.launch_at_startup) {
                eprintln!("Failed to apply launch-at-startup setting: {err}");
            }
            // 开机自启带 `--from-autostart` 时不弹窗；用户显式打开「启动后最小化到托盘」时
            // 任何启动路径都不弹（含参数丢失的自启、开始菜单快捷方式）。
            let skip_chat_on_launch =
                launched_from_autostart || settings.launch_minimized_to_tray;
            #[cfg(target_os = "macos")]
            if skip_chat_on_launch {
                let _ = app
                    .handle()
                    .set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
            let usage_dir = usage::usage_dir(&app.handle()).unwrap_or_else(|err| {
                eprintln!("Failed to initialize usage ledger dir: {err}");
                std::env::temp_dir().join("kivio-usage")
            });

            let offline_models =
                offline_models::OfflineModelManager::new(&app.handle(), build_http_client());
            let inpainting = inpainting::InpaintingClient::new(offline_models.clone());
            app.manage(AppState::base(
                settings,
                usage_dir,
                build_http_client(),
                #[cfg(target_os = "macos")]
                macos_ocr::MacOcrClient::new(&app.handle()),
                offline_models.clone(),
                rapidocr::RapidOcrClient::new(offline_models),
                inpainting,
            ));
            app.manage(chat::repository::ConversationRepository::default());
            // Dock 的 workspace 文件监听服务（文件树 / Git 面板的秒级刷新源）。
            app.manage(std::sync::Arc::new(dock::watch::WorkspaceWatchService::new(
                app.handle().clone(),
            )));
            // Dock 终端面板的 PTY 会话登记表（会话 drop 时兜底 kill 子进程）。
            app.manage(std::sync::Arc::new(dock::terminal::TerminalService::new(
                app.handle().clone(),
            )));

            // Apply the stored sub-agent concurrency cap (default sizes the gate
            // to DEFAULT_SUB_AGENT_CONCURRENCY; reconcile to the user's setting).
            {
                let state: State<AppState> = app.state();
                let n = state.settings_read().chat_tools.sub_agent_concurrency;
                state.sub_agents.set_concurrency(n);
            }

            if let Err(err) = register_hotkeys(&app.handle()) {
                eprintln!(
                    "Failed to register hotkeys: {}",
                    display_hotkey_errors(&err)
                );
            }
            if let Err(err) = setup_tray(&app.handle()) {
                eprintln!("Failed to setup tray: {err}");
            }

            // 启动后 5s 静默检查更新（settings.auto_check_update 控制）
            // 发现新版 → emit "update-available" 事件，前端 Settings 打开时会展示提示
            // 失败 / 限流 / 网络问题全部静默，不打扰用户
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                // 启动竞态加固：若 AppState 尚未 manage（异常早期），直接跳过本次更新检查。
                let Some(state) = app_handle.try_state::<AppState>() else {
                    return;
                };
                if !state.settings_read().auto_check_update {
                    return;
                }
                if let Ok(value) = check_github_latest_release(state).await {
                    if value
                        .get("available")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                    {
                        let _ = app_handle.emit("update-available", value);
                    }
                }
            });

            // MCP 持久连接空闲回收 reaper：每 60s 扫描连接池，回收 last_used 超过
            // 设置 mcp_idle_timeout_ms 的会话（Drop 杀子进程），发 Disconnected 事件。
            {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let mut ticker = tokio::time::interval(Duration::from_secs(60));
                    loop {
                        ticker.tick().await;
                        // 启动竞态加固：AppState 可能尚未 manage（极早期 tick），跳过本轮，60s 后再试。
                        let Some(state) = app_handle.try_state::<AppState>() else {
                            continue;
                        };
                        let idle_timeout = state.mcp_idle_timeout();
                        let evicted = state.mcp_reap_idle(idle_timeout).await;
                        for (server_id, _) in evicted {
                            let _ = app_handle.emit(
                                "mcp-server-state",
                                serde_json::json!({
                                    "serverId": server_id,
                                    "state": { "kind": "disconnected" },
                                }),
                            );
                        }
                    }
                });
            }

            // 无头测试通道（probe）——仅 debug 构建。轮询 <app_data>/chat_probe/request.json，
            // 走与聊天窗口相同的生成路径并把结果写 result.json，供自动化真实验证工具调用。
            #[cfg(debug_assertions)]
            {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    crate::chat::probe::run_probe_watcher(app_handle).await;
                });
            }

            // 启动期并行预热：对每个已启用的 MCP server 建立持久连接（非阻塞）。
            // 失败仅置 Error 态（mcp_get_or_connect 内部已发事件），不影响启动。
            {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    // 启动竞态加固：若 AppState 尚未 manage，跳过预热（首次使用时会 lazy 连接）。
                    let Some(state) = app_handle.try_state::<AppState>() else {
                        return;
                    };
                    let settings = state.settings_read().clone();
                    if !settings.chat_tools.enabled {
                        return;
                    }
                    let servers: Vec<_> = settings
                        .chat_tools
                        .servers
                        .iter()
                        .filter(|server| crate::mcp::registry::mcp_server_is_runtime_eligible(server))
                        .cloned()
                        .collect();
                    if !servers.is_empty() {
                        eprintln!(
                            "[mcp] warming {} server(s) with concurrency {}",
                            servers.len(),
                            MCP_STARTUP_WARMUP_CONCURRENCY
                        );
                    }
                    futures::stream::iter(servers)
                        .for_each_concurrent(MCP_STARTUP_WARMUP_CONCURRENCY, |server| {
                            let app_handle = app_handle.clone();
                            async move {
                                // 启动竞态加固：若 AppState 尚未 manage，跳过该 server
                                // 预热（lazy 连接兜底）。
                                let Some(state) = app_handle.try_state::<AppState>() else {
                                    return;
                                };
                                let _ = state.mcp_get_or_connect(Some(&app_handle), &server).await;
                            }
                        })
                        .await;
                });
            }

            // 手动启动默认打开聊天窗口；自启 / 「启动后最小化到托盘」则只留托盘常驻。
            if !skip_chat_on_launch {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    if let Err(err) = open_chat_window(&app_handle) {
                        eprintln!("Failed to open chat on launch: {err}");
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            windows::chat_window_apply_mica,
            windows::chat_window_set_opaque,
            windows::chat_traffic_light_center_y,
            fonts::list_system_fonts,
            commands::get_default_prompt_templates,
            commands::save_settings,
            commands::set_favorite_models,
            commands::set_translate_card_size,
            commands::export_settings,
            commands::import_settings,
            commands::open_settings_window,
            commands::close_translator_window,
            commands::translate_text,
            commands::commit_translation,
            commands::open_external,
            commands::open_local_file,
            commands::open_data_url_file,
            commands::open_html_preview,
            lens_commands::explain_read_image,
            commands::fetch_models,
            commands::test_provider_connection,
            commands::test_web_search,
            commands::get_permission_status,
            commands::open_permission_settings,
            lens_commands::lens_list_windows,
            lens_commands::lens_capture_window,
            lens_commands::lens_capture_region,
            lens_commands::lens_register_annotated_image,
            lens_commands::lens_copy_image_to_clipboard,
            lens_commands::lens_save_annotated_png,
            lens_commands::lens_ask,
            lens_commands::lens_send_to_chat,
            lens_commands::lens_send_history_to_chat,
            lens_commands::lens_translate,
            lens_commands::lens_translate_text,
            lens_commands::lens_replace_translate,
            lens_commands::lens_cancel_stream,
            lens_commands::lens_focus_webview,
            lens_commands::lens_close,
            lens_commands::lens_set_escape_guard,
            lens_commands::lens_take_reset_payload,
            lens_commands::lens_set_floating,
            lens_commands::lens_animate_floating,
            commands::take_lens_selection,
            lens_commands::lens_commit_image_to_history,
            lens_commands::lens_delete_history_image,
            updates::check_github_latest_release,
            updates::download_update_asset,
            updates::install_update_and_quit,
            commands::rapidocr_status,
            commands::rapidocr_install,
            commands::replace_translation_pack_status,
            commands::replace_translation_pack_install,
            usage::usage_get_stats,
            usage::usage_clear,
            chat::commands::interaction::get_request_debug_records,
            chat::commands::interaction::clear_request_debug_records,
            chat::protocol::chat_sync_state,
            // Chat 模块命令
            chat::commands::catalog::chat_get_conversations,
            chat::commands::interaction::chat_list_background_tasks,
            chat::commands::interaction::chat_clear_finished_background_tasks,
            chat::commands::interaction::chat_stop_external_background_task,
            chat::commands::interaction::chat_kill_background_command,
            chat::commands::catalog::chat_search_conversations,
            chat::commands::catalog::chat_query_conversations,
            chat::commands::catalog::chat_get_conversation,
            chat::export::chat_export_conversation_markdown,
            chat::commands::catalog::chat_create_conversation,
            chat::commands::catalog::chat_import_external_conversation,
            chat::commands::catalog::chat_create_builder_conversation,
            chat::commands::catalog::chat_get_assistants,
            chat::commands::catalog::chat_create_assistant,
            chat::commands::catalog::chat_update_assistant,
            chat::commands::catalog::chat_duplicate_assistant,
            chat::commands::catalog::chat_delete_assistant,
            chat::commands::catalog::chat_get_projects,
            chat::commands::catalog::chat_reorder_projects,
            chat::commands::catalog::chat_get_conversation_pins,
            chat::commands::catalog::chat_set_conversation_pins,
            chat::commands::catalog::chat_create_project,
            chat::commands::catalog::chat_update_project,
            chat::commands::catalog::chat_delete_project,
            chat::commands::catalog::chat_project_open_folder,
            chat::commands::catalog::chat_get_sets,
            chat::commands::catalog::chat_reorder_sets,
            chat::commands::catalog::chat_create_set,
            chat::commands::catalog::chat_update_set,
            chat::commands::catalog::chat_delete_set,
            chat::commands::context::chat_get_context_stats,
            chat::commands::context::chat_compress_context,
            chat::commands::interaction::chat_take_external_sends,
            chat::commands::interaction::chat_set_agent_plan_mode,
            chat::commands::interaction::chat_execute_agent_plan,
            chat::commands::send::chat_send_message,
            chat::commands::interaction::chat_cancel_stream,
            chat::commands::interaction::chat_confirm_tool_call,
            chat::commands::interaction::chat_respond_session_consent,
            chat::commands::interaction::chat_submit_user_choice,
            chat::commands::interaction::chat_steer_message,
            chat::commands::interaction::chat_python_complete,
            chat::commands::attachments::chat_read_attachment,
            chat::commands::attachments::chat_open_attachment,
            chat::commands::attachments::chat_open_generated_artifact,
            chat::commands::attachments::chat_reveal_generated_artifact,
            chat::commands::attachments::chat_save_pasted_image,
            chat::commands::attachments::chat_save_pasted_attachment,
            chat::commands::attachments::chat_read_clipboard_files,
            chat::commands::mutations::chat_delete_conversation,
            chat::commands::mutations::chat_update_conversation,
            chat::commands::mutations::chat_bulk_update_conversations,
            chat::commands::mutations::chat_bulk_delete_conversations,
            chat::commands::reasoning::chat_reasoning_efforts_for_model,
            chat::commands::mutations::chat_update_message,
            chat::commands::mutations::chat_delete_message,
            chat::commands::mutations::chat_set_group_selection,
            chat::commands::mutations::chat_regenerate_message,
            chat::commands::mutations::chat_rewind_to_message,
            chat::commands::mutations::chat_fork_conversation,
            external_agents::commands::chat_detect_external_agents,
            external_agents::commands::chat_detect_external_agent_models,
            external_agents::commands::chat_list_external_cli_slash_commands,
            external_agents::commands::chat_external_cli_provider_cleanup,
            external_agents::commands::chat_external_cli_pi_agent_dir,
            external_agents::commands::chat_external_cli_scan_cc_switch,
            external_agents::commands::chat_external_cli_fetch_relay_models,
            external_agents::installer::chat_external_cli_install_info,
            external_agents::installer::chat_external_cli_install,
            external_agents::installer::chat_external_cli_open_config_dir,
            external_agents::dsh_plugins::chat_dsh_plugin_settings_get,
            external_agents::dsh_plugins::chat_dsh_plugin_settings_save,
            external_agents::dsh_plugins::chat_dsh_plugin_inventory,
            external_agents::dsh_plugins::chat_dsh_open_settings_file,
            external_agents::dsh_plugins::chat_dsh_official_credential_status,
            external_agents::dsh_plugins::chat_dsh_official_credential_save,
            external_agents::dsh_plugins::chat_dsh_native_provider_get,
            external_agents::dsh_plugins::chat_dsh_native_provider_delete,
            external_agents::commands::chat_set_agent_runtime,
            external_agents::commands::chat_list_importable_cli_sessions,
            external_agents::commands::chat_import_cli_sessions,
            external_agents::commands::chat_imported_history_stale,
            chat::memory::chat_memory_get,
            chat::memory::chat_memory_save,
            chat::memory::chat_memory_open_folder,
            mcp::registry::chat_mcp_list_tools,
            mcp::registry::chat_mcp_test_server,
            mcp::registry::chat_mcp_import_json,
            mcp::registry::chat_cli_import_scan,
            mcp::registry::chat_mcp_server_status,
            mcp::registry::chat_mcp_list_tool_defs,
            mcp::registry::chat_mcp_reload_server,
            mcp::registry::chat_mcp_warmup,
            connectors::connector_oauth_connect,
            connectors::obsidian::list_obsidian_vaults_cmd,
            connectors::himalaya::list_email_provider_presets,
            connectors::himalaya::himalaya_status_cmd,
            connectors::himalaya::himalaya_install_cmd,
            connectors::himalaya::test_himalaya_email_cmd,
            plugins::plugins_list,
            plugins::plugins_list_cached,
            plugins::plugins_install_brief,
            plugins::plugins_set_enabled,
            plugins::plugins_uninstall,
            notes::notes_list,
            notes::notes_read,
            notes::notes_create,
            notes::notes_update,
            notes::notes_delete,
            notes::notes_folders_list,
            notes::notes_folder_create,
            notes::notes_folder_rename,
            notes::notes_folder_delete,
            notes::notes_open_folder,
            notes::notes_dir_path,
            skills::chat_skills_list,
            skills::chat_skills_read,
            skills::chat_skills_import,
            skills::chat_skills_uninstall,
            skills::chat_skills_install_from_url,
            skills::chat_skills_open_folder,
            chat::knowledge_base::commands::kb_list_libraries,
            chat::knowledge_base::commands::kb_create_library,
            chat::knowledge_base::commands::kb_rename_library,
            chat::knowledge_base::commands::kb_delete_library,
            chat::knowledge_base::commands::kb_list_documents,
            chat::knowledge_base::commands::kb_delete_document,
            chat::knowledge_base::commands::kb_retrieval_test,
            chat::knowledge_base::ingest::kb_upload_document,
            chat::knowledge_base::ingest::kb_import_url,
            chat::knowledge_base::ingest::kb_reindex_library,
            chat::knowledge_base::ingest::kb_update_embedding,
            chat::knowledge_base::ingest::kb_set_embed_batch_size,
            // Dock 模块命令（右侧文件树 + Git 面板 + 终端面板 + workspace 监听）
            dock::dock_resolve_cwd,
            dock::fs::dock_fs_list,
            dock::fs::dock_fs_search,
            dock::fs::dock_fs_read,
            dock::fs::dock_fs_write,
            dock::fs::dock_fs_create,
            dock::fs::dock_fs_rename,
            dock::fs::dock_fs_move,
            dock::fs::dock_fs_delete,
            dock::fs::dock_fs_open_path,
            dock::git::dock_git_status,
            dock::git::dock_git_diff,
            dock::git::dock_git_log,
            dock::git::dock_git_commit_diff,
            dock::git::dock_git_branches,
            dock::git::dock_git_stage,
            dock::git::dock_git_stage_all,
            dock::git::dock_git_unstage,
            dock::git::dock_git_unstage_all,
            dock::git::dock_git_discard,
            dock::git::dock_git_discard_all,
            dock::git::dock_git_commit,
            dock::git::dock_git_switch_branch,
            dock::git::dock_git_create_branch,
            dock::git::dock_git_init,
            dock::git::dock_git_diff_stat,
            dock::git::dock_git_add_to_gitignore,
            dock::watch::dock_workspace_watch_set,
            dock::terminal::dock_terminal_create,
            dock::terminal::dock_terminal_write,
            dock::terminal::dock_terminal_resize,
            dock::terminal::dock_terminal_close,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| match event {
            tauri::RunEvent::ExitRequested { api, code, .. } => {
                if code.is_none() {
                    api.prevent_exit();
                } else {
                    // 真正退出：同步排干 MCP 连接池，杀掉所有持久子进程，避免孤儿进程。
                    let state: State<AppState> = app_handle.state();
                    // 带超时：一个卡在握手里的 server 会占着会话锁不放，没有这层
                    // 上限的话退出钩子会永久阻塞在主线程上 —— 表现是「点关闭没反应、
                    // 进程不退」，连带其余所有 MCP 子进程全留在系统里。
                    //
                    // ⚠️ `timeout(..)` **必须在 async 块里构造**，不能当参数传给 block_on。
                    // 参数在进入运行时之前就求值了，而 `tokio::time::Sleep` 构造时就要求时间
                    // 驱动在场，否则 panic「there is no reactor running」。这条 panic 会让
                    // 下面整段退出清理（外部 CLI 会话 / 后台命令进程组 / 插件预览）全部不执行
                    // —— 表现是每次退出漏一批孤儿子进程，外加退出码 101。
                    let drained = tauri::async_runtime::block_on(async {
                        tokio::time::timeout(
                            std::time::Duration::from_secs(3),
                            state.mcp_disconnect_all(),
                        )
                        .await
                    });
                    if drained.is_err() {
                        // 超时说明某个会话锁拿不到（多半卡在握手里）。优雅关停这条路已经走不通，
                        // 按 pid 杀进程树兜底 —— 否则那句「falling back to process kill」就是空话，
                        // stdio 子进程（及它自己拉起的孙子）会留在系统里。
                        let killed = state.kill_mcp_children_now();
                        eprintln!(
                            "MCP disconnect timed out on exit; killed {killed} stdio child process tree(s)."
                        );
                    }
                    // 外部 CLI 会话：必须**同步**等它们关完。只 clear 掉 sender 是不够的
                    // —— actor 要等下一次被 poll 才会走 close()，而运行时马上就随进程走了，
                    // `kill_on_drop` 也因此不会触发（Child 在那个永不 drop 的帧里）。
                    // 结果是每次退出留下一批 CLI 进程，各自还挂着自己拉起的 MCP 子进程。
                    // 同上：timeout 必须在 async 块里构造。
                    let closed = tauri::async_runtime::block_on(async {
                        tokio::time::timeout(
                            std::time::Duration::from_secs(3),
                            state.close_all_external_live_sessions(),
                        )
                        .await
                    });
                    if closed.is_err() {
                        eprintln!("External CLI sessions did not close in time on exit.");
                    }
                    // 杀掉所有跟踪中的后台 run_command 进程组（跨 turn 存活，只在这里或
                    // 显式 kill_background 才清理），删除其 per-job 日志，避免孤儿进程/文件。
                    let killed = state.kill_all_background_commands();
                    if killed > 0 {
                        eprintln!("Killed {killed} background command process group(s) on exit.");
                    }
                    #[cfg(target_os = "macos")]
                    state.macos_ocr.shutdown();
                    // OfficeCLI live preview (`officecli watch`) 等插件附属进程
                    crate::plugins::stop_all_previews();
                }
            }
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen {
                has_visible_windows,
                ..
            } => {
                if !has_visible_windows {
                    if let Err(err) = open_chat_window(app_handle) {
                        eprintln!("Failed to open chat on dock reopen: {err}");
                    }
                } else if let Some(window) = first_visible_user_window(app_handle) {
                    // 已有可见窗口：只把它带到前台，绝不调 open_chat_window——那会把停在
                    // #chat/settings 的用户重置回 #chat，丢失填到一半的配置。
                    let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);
                    if window.is_minimized().ok().unwrap_or(false) {
                        let _ = window.unminimize();
                    }
                    let _ = window.show();
                    let _ = window.set_focus();
                } else if let Err(err) = open_chat_window(app_handle) {
                    eprintln!("Failed to open chat on dock reopen: {err}");
                }
            }
            _ => {}
        });
}
