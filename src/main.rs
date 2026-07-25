// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Standalone mode: run the gateway server without the Tauri desktop window.
    // For the full desktop experience, use `cargo tauri dev` instead.
    match llm_api_proxy_lib::initialize_backend() {
        Ok(state) => {
            println!(
                "LLM-API-Proxy running at {}",
                state.settings.gateway_url()
            );
            println!("Press Ctrl+C to stop");
            // Keep the main thread alive
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
        Err(e) => {
            eprintln!("Failed to start LLM-API-Proxy: {}", e);
            std::process::exit(1);
        }
    }
}
