use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
            api_key: String::new(),
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

// ============================================================================
// Rate Limit Settings
// ============================================================================

/// Rate limiting configuration persisted in the settings table.
///
/// Settings keys (all stored as strings):
/// - `rate_limit_enabled` (default: true)
/// - `rate_limit_max_requests` (default: 60)
/// - `rate_limit_window_seconds` (default: 60)
/// - `rate_limit_trust_xff` (default: false)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitSettings {
    pub enabled: bool,
    pub max_requests: u32,
    pub window_seconds: u32,
    pub trust_forwarded_for: bool,
}

impl Default for RateLimitSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_requests: 60,
            window_seconds: 60,
            trust_forwarded_for: false,
        }
    }
}

impl RateLimitSettings {
    /// Load rate limit settings from the database settings table.
    pub fn load(db: &crate::db::Database) -> Self {
        Self {
            enabled: db
                .get_setting("rate_limit_enabled")
                .ok()
                .flatten()
                .map(|v| v == "true")
                .unwrap_or(true),
            max_requests: db
                .get_setting("rate_limit_max_requests")
                .ok()
                .flatten()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
            window_seconds: db
                .get_setting("rate_limit_window_seconds")
                .ok()
                .flatten()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
            trust_forwarded_for: db
                .get_setting("rate_limit_trust_xff")
                .ok()
                .flatten()
                .map(|v| v == "true")
                .unwrap_or(false),
        }
    }

    /// Save rate limit settings to the database settings table.
    pub fn save(&self, db: &crate::db::Database) -> Result<(), crate::error::AppError> {
        db.save_setting("rate_limit_enabled", &self.enabled.to_string())?;
        db.save_setting("rate_limit_max_requests", &self.max_requests.to_string())?;
        db.save_setting("rate_limit_window_seconds", &self.window_seconds.to_string())?;
        db.save_setting("rate_limit_trust_xff", &self.trust_forwarded_for.to_string())?;
        Ok(())
    }
}

// ============================================================================
// Probe Settings
// ============================================================================

/// Upstream probe configuration persisted in the settings table.
///
/// Settings keys (all stored as strings):
/// - `probe_enabled` (default: false)
/// - `probe_interval_seconds` (default: 300, minimum: 60)
/// - `probe_failure_threshold` (default: 3, minimum: 1)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeSettings {
    pub enabled: bool,
    pub interval_seconds: u32,
    pub failure_threshold: u32,
}

impl Default for ProbeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_seconds: 300,
            failure_threshold: 3,
        }
    }
}

impl ProbeSettings {
    /// Load probe settings from the database settings table.
    /// Values are clamped to minimum bounds to prevent misconfiguration.
    pub fn load(db: &crate::db::Database) -> Self {
        Self {
            enabled: db
                .get_setting("probe_enabled")
                .ok()
                .flatten()
                .map(|v| v == "true")
                .unwrap_or(false),
            interval_seconds: db
                .get_setting("probe_interval_seconds")
                .ok()
                .flatten()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300)
                .max(60),
            failure_threshold: db
                .get_setting("probe_failure_threshold")
                .ok()
                .flatten()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3)
                .max(1),
        }
    }

    /// Save probe settings to the database settings table.
    /// Values are clamped before saving.
    pub fn save(&self, db: &crate::db::Database) -> Result<(), crate::error::AppError> {
        db.save_setting("probe_enabled", &self.enabled.to_string())?;
        db.save_setting(
            "probe_interval_seconds",
            &self.interval_seconds.max(60).to_string(),
        )?;
        db.save_setting(
            "probe_failure_threshold",
            &self.failure_threshold.max(1).to_string(),
        )?;
        Ok(())
    }
}
