use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::path::PathBuf;
use tracing::{info, warn};

/// Gateway configuration loaded from settings table or defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewaySettings {
    /// API gateway listen address (default: 127.0.0.1)
    pub listen_address: String,
    /// API gateway listen port (default: 47339)
    pub listen_port: u16,
    /// Gateway API Key for client authentication
    pub api_key: String,
    /// Management UI listen port (Tauri handles GUI natively, this is fallback)
    pub gui_port: u16,
    /// Log level: trace/debug/info/warn/error
    pub log_level: String,
    /// Whether the gateway service is running
    pub gateway_enabled: bool,
}

impl Default for GatewaySettings {
    fn default() -> Self {
        Self {
            listen_address: "127.0.0.1".to_string(),
            listen_port: 47339,
            api_key: "sk-gateway-key".to_string(),
            gui_port: 1420,
            log_level: "info".to_string(),
            gateway_enabled: true,
        }
    }
}

impl GatewaySettings {
    /// Returns the gateway base URL (e.g. http://127.0.0.1:47339)
    pub fn gateway_url(&self) -> String {
        format!("http://{}:{}", self.listen_address, self.listen_port)
    }

    /// Full OpenAI-compatible base path
    pub fn gateway_base_path(&self) -> String {
        format!("{}/v1", self.gateway_url())
    }

    /// Resolve data directory relative to exe location or current working dir
    pub fn data_dir() -> PathBuf {
        // On Windows, data/ sits next to the executable
        let exe_dir = std::env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("."))
            .parent()
            .unwrap_or(PathBuf::from(".").as_path())
            .to_path_buf();

        if exe_dir.exists() {
            exe_dir.join("data")
        } else {
            PathBuf::from("data")
        }
    }

    /// Path to SQLite database file
    pub fn db_path() -> PathBuf {
        Self::data_dir().join("proxy.db")
    }

    /// Path to Master Key binary file
    pub fn master_key_path() -> PathBuf {
        Self::data_dir().join("master_key.bin")
    }
}
