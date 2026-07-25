// Tauri v2 app entry point (compiled by tauri-build)

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 1. Initialize all backend services (DB, crypto, gateway server)
    let _app_state = match llm_api_proxy_lib::initialize_backend() {
        Ok(state) => state,
        Err(e) => {
            eprintln!("Failed to initialize LLM-API-Proxy backend: {}", e);
            std::process::exit(1);
        }
    };

    // 2. Build and run the Tauri desktop application
    tauri::Builder::default()
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
