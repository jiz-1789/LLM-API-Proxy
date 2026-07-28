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

            // Create a runtime for signal handling and graceful shutdown.
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("Failed to create runtime: {}", e);
                    std::process::exit(1);
                }
            };
            rt.block_on(async move {
                // Wait for Ctrl+C / SIGTERM signal
                tokio::signal::ctrl_c()
                    .await
                    .expect("Failed to listen for Ctrl+C");
                println!("\nShutting down...");
                // Trigger graceful shutdown of the gateway server
                state.shutdown();
                // Give the server a moment to release the port
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            });
        }
        Err(e) => {
            eprintln!("Failed to start LLM-API-Proxy: {}", e);
            std::process::exit(1);
        }
    }
}
