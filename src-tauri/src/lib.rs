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
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::list_upstreams,
            commands::get_upstream,
            commands::create_upstream,
            commands::update_upstream,
            commands::delete_upstream,
            commands::toggle_upstream,
            commands::fetch_upstream_models,
commands::fetch_upstream_models_by_id,
            commands::list_pools,
            commands::get_pool,
            commands::create_pool,
            commands::update_pool,
            commands::delete_pool,
            commands::add_upstream_to_pool,
            commands::remove_upstream_from_pool,
            commands::get_pool_upstreams,
            commands::reorder_pool_upstreams,
            commands::get_stats,
            commands::get_gateway_info,
            commands::get_request_logs,
            commands::get_upstream_token_usage,
            commands::reset_upstream_token_stats,
            commands::get_upstream_model_detail,
            commands::check_upstream_health,
            commands::check_all_upstreams_health,
            commands::get_settings,
            commands::update_settings,
            commands::set_minimize_to_tray,
            commands::set_theme,
        ])
        .setup(|app| {
            // Build system tray menu
            let open_item = MenuItemBuilder::with_id("open", "打开主窗口").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "退出").build(app)?;
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
            }
        });
}
