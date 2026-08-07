// Tauri v2 app entry point (compiled by tauri-build)

mod commands;

use tauri::Manager;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 1. Initialize all backend services (DB, crypto, gateway server)
    let app_state = match llm_api_proxy_lib::initialize_backend() {
        Ok(state) => state,
        Err(e) => {
            eprintln!("Failed to initialize LLM-API-Proxy backend: {}", e);
            std::process::exit(1);
        }
    };

    // 2. Build and run the Tauri desktop application
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // When a second instance is launched, show the existing window
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::upstream::list_upstreams,
            commands::upstream::get_upstream,
            commands::upstream::create_upstream,
            commands::upstream::update_upstream,
            commands::upstream::delete_upstream,
            commands::upstream::reveal_api_key,
            commands::upstream::toggle_upstream,
            commands::upstream::fetch_upstream_models,
            commands::upstream::fetch_upstream_models_by_id,
            commands::upstream::test_upstream_chat,
            commands::upstream::test_upstream_chat_by_id,
            commands::pool::list_pools,
            commands::pool::get_pool,
            commands::pool::create_pool,
            commands::pool::update_pool,
            commands::pool::delete_pool,
            commands::pool::add_upstream_to_pool,
            commands::pool::remove_upstream_from_pool,
            commands::pool::get_pool_upstreams,
            commands::pool::reorder_pool_upstreams,
            commands::log::get_stats,
            commands::log::get_gateway_info,
            commands::log::get_request_logs,
            commands::log::get_upstream_token_usage,
            commands::log::reset_upstream_token_stats,
            commands::log::get_upstream_model_detail,
            commands::log::export_request_logs,
            commands::log::get_request_stats,
            commands::log::get_failover_events,
            commands::log::get_token_overview,
            commands::log::clear_all_token_usage,
            commands::health::check_upstream_health,
            commands::health::check_all_upstreams_health,
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::settings::set_minimize_to_tray,
            commands::settings::set_theme,
            commands::settings::open_external_url,
            commands::settings::read_clipboard,
            commands::settings::write_clipboard,
            commands::settings::save_file_dialog,
            commands::settings::get_config_changes,
            commands::backup::backup_database,
            commands::backup::restore_database,
            commands::backup::check_restore_pending,
            commands::backup::get_auto_backup_settings,
            commands::backup::update_auto_backup_settings,
            commands::backup::list_auto_backups,
            commands::backup::create_backup_now,
            commands::backup::export_config,
            commands::backup::import_config,
            commands::diagnostic::export_diagnostic,
            commands::api_key::list_api_keys,
            commands::api_key::create_api_key,
            commands::api_key::update_api_key,
            commands::api_key::delete_api_key,
            commands::api_key::toggle_api_key,
            commands::api_key::regenerate_api_key,
            commands::update::check_for_updates,
            commands::update::download_update,
            commands::update::apply_update,
            commands::update::check_pending_update,
            commands::shortcut::check_first_run,
            commands::shortcut::check_desktop_shortcut,
            commands::shortcut::create_desktop_shortcut,
            commands::tool_config::detect_all_tools,
            commands::tool_config::get_tool_switches,
            commands::tool_config::enable_tool_switch,
            commands::tool_config::disable_tool_switch,
            commands::tool_config::update_tool_config,
            commands::tool_config::suggest_pool,
            commands::tool_config::detect_env_conflicts,
            commands::tool_config::cleanup_env_conflicts,
            commands::tool_config::restore_env_backup,
        ])
        .setup(|app| {
            // Read language setting for bilingual tray menu
            let state = app.state::<llm_api_proxy_lib::AppState>();
            let lang = state.db.get_setting("language").ok().flatten().unwrap_or_else(|| "zh".to_string());
            let (open_text, quit_text) = if lang == "en" {
                ("Open Main Window", "Quit")
            } else {
                ("打开主窗口", "退出")
            };

            // Build system tray menu
            let open_item = MenuItemBuilder::with_id("open", open_text).build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", quit_text).build(app)?;
            let tray_menu = MenuBuilder::new(app)
                .item(&open_item)
                .separator()
                .item(&quit_item)
                .build()?;

            // Create tray icon with menu
            let icon = app.default_window_icon().cloned();
            let mut tray_builder = TrayIconBuilder::new()
                .menu(&tray_menu)
                .tooltip("LLM-API-Proxy");
            if let Some(icon) = icon {
                tray_builder = tray_builder.icon(icon);
            }

            // Keep the TrayIcon handle alive — Tauri v2 removes the icon on Drop
            let _tray = tray_builder
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "open" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        // Gracefully shut down the gateway server before exiting
                        let state = app.state::<llm_api_proxy_lib::AppState>();
                        state.shutdown();
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // Double-click tray icon to show window
                    if let tauri::tray::TrayIconEvent::DoubleClick { .. } = event {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            // Start background alert monitoring task
            llm_api_proxy_lib::alert::start_alert_task(state.db.clone());

            // Restore tool configs on startup: first recover from any abnormal
            // exit (secondary backups), then re-inject switch=ON tools.
            if let Err(e) = state.tool_switch_manager.recover_from_crash() {
                tracing::error!(error = %e, "启动时恢复工具配置失败");
            }

            Ok(())
        })
        // Respect user's minimize-to-tray preference on window close
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let state = app.state::<llm_api_proxy_lib::AppState>();
                // Read directly from the database to ensure we always have the
                // latest persisted value. The AtomicBool cache can be stale if
                // another command (e.g. update_settings from toggleTheme)
                // overwrote it with a value read before the user's change.
                let minimize = llm_api_proxy_lib::load_minimize_to_tray(&state.db);
                tracing::info!("CloseRequested: minimize_to_tray={} (from DB)", minimize);
                if minimize {
                    let _ = window.hide();
                    api.prevent_close();
                } else {
                    // User chose to exit on close.
                    // Must exit on a background thread — calling app.exit()
                    // on the main thread inside on_window_event can deadlock
                    // or show a black screen on Windows.
                    api.prevent_close();
                    state.shutdown();
                    let app_handle = app.clone();
                    std::thread::spawn(move || {
                        app_handle.exit(0);
                    });
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // Safety net: ensure gateway shuts down on any exit path
            if let tauri::RunEvent::Exit = event {
                let state = app_handle.state::<llm_api_proxy_lib::AppState>();
                state.shutdown();
                // Restore tool configs on exit (方案 B: 退出时必须恢复).
                if let Err(e) = state.tool_switch_manager.restore_on_exit() {
                    tracing::error!(error = %e, "退出时恢复工具配置失败");
                }
            }
        });
}
