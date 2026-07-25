// Tauri v2 app entry point (compiled by tauri-build)

mod commands;

use tauri::Manager;

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
        ])
        .setup(|app| {
            #[cfg(debug_assertions)]
            {
                // Open DevTools in debug mode for frontend development
                if let Some(window) = app.get_webview_window("main") {
                    window.open_devtools();
                }
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
