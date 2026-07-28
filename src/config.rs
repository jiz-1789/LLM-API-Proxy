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
// Configuration Validation
// ============================================================================
//
// Configuration lifecycle (P1-14):
//
// 1. **Default values** — hard-coded in `Default` implementations (e.g., port 47339).
// 2. **User saved values** — persisted in the `settings` table via `update_settings` command.
// 3. **Runtime effective values** — loaded from DB at startup, falling back to defaults.
//
// Loading priority: DB (user saved) > hardcoded default.
// On first run, defaults are persisted to DB only if the key doesn't already exist.
// This prevents overwriting user-saved values on restart.

/// Validate a port number. Must be between 1 and 65535.
pub fn validate_port(port: u16) -> Result<u16, String> {
    if port == 0 {
        Err("端口号不能为 0".to_string())
    } else {
        Ok(port)
    }
}

/// Validate a listen address. Must be a valid IP address (v4 or v6) or hostname.
pub fn validate_listen_address(addr: &str) -> Result<String, String> {
    let trimmed = addr.trim();
    if trimmed.is_empty() {
        return Err("监听地址不能为空".to_string());
    }
    // Accept common addresses: 127.0.0.1, 0.0.0.0, ::1, localhost, or any IP
    if trimmed.parse::<std::net::IpAddr>().is_ok()
        || trimmed == "localhost"
    {
        Ok(trimmed.to_string())
    } else {
        Err(format!("无效的监听地址: {}", trimmed))
    }
}

/// Validate a gateway API key. Must be non-empty.
pub fn validate_api_key(key: &str) -> Result<String, String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        Err("API Key 不能为空".to_string())
    } else {
        Ok(trimmed.to_string())
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

// ============================================================================
// Log Retention Settings
// ============================================================================

/// Log retention configuration persisted in the settings table.
///
/// Settings keys (all stored as strings):
/// - `log_retention_days` (default: 5, minimum: 1)
/// - `log_max_entries` (default: 200, minimum: 10)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogRetentionSettings {
    pub retention_days: i32,
    pub max_entries: i64,
}

impl Default for LogRetentionSettings {
    fn default() -> Self {
        Self {
            retention_days: 5,
            max_entries: 200,
        }
    }
}

impl LogRetentionSettings {
    /// Load log retention settings from the database settings table.
    /// Values are clamped to minimum bounds to prevent misconfiguration.
    pub fn load(db: &crate::db::Database) -> Self {
        Self {
            retention_days: db
                .get_setting("log_retention_days")
                .ok()
                .flatten()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5)
                .max(1),
            max_entries: db
                .get_setting("log_max_entries")
                .ok()
                .flatten()
                .and_then(|v| v.parse().ok())
                .unwrap_or(200)
                .max(10),
        }
    }

    /// Save log retention settings to the database settings table.
    /// Values are clamped before saving.
    pub fn save(&self, db: &crate::db::Database) -> Result<(), crate::error::AppError> {
        db.save_setting("log_retention_days", &self.retention_days.max(1).to_string())?;
        db.save_setting("log_max_entries", &self.max_entries.max(10).to_string())?;
        Ok(())
    }
}

// ============================================================================
// Alert Settings
// ============================================================================

/// Alert configuration persisted in the settings table.
///
/// Settings keys (all stored as strings):
/// - `alert_enabled` (default: false)
/// - `alert_failure_rate_threshold` (default: 50.0, minimum: 1.0)
/// - `alert_min_request_count` (default: 10, minimum: 1)
/// - `alert_silence_minutes` (default: 30, minimum: 5)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertSettings {
    pub enabled: bool,
    pub failure_rate_threshold: f64,
    pub min_request_count: u32,
    pub silence_minutes: u32,
}

impl Default for AlertSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            failure_rate_threshold: 50.0,
            min_request_count: 10,
            silence_minutes: 30,
        }
    }
}

impl AlertSettings {
    /// Load alert settings from the database settings table.
    /// Values are clamped to minimum bounds to prevent misconfiguration.
    pub fn load(db: &crate::db::Database) -> Self {
        Self {
            enabled: db
                .get_setting("alert_enabled")
                .ok()
                .flatten()
                .map(|v| v == "true")
                .unwrap_or(false),
            failure_rate_threshold: db
                .get_setting("alert_failure_rate_threshold")
                .ok()
                .flatten()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(50.0)
                .max(1.0),
            min_request_count: db
                .get_setting("alert_min_request_count")
                .ok()
                .flatten()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10)
                .max(1),
            silence_minutes: db
                .get_setting("alert_silence_minutes")
                .ok()
                .flatten()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30)
                .max(5),
        }
    }

    /// Save alert settings to the database settings table.
    /// Values are clamped before saving.
    pub fn save(&self, db: &crate::db::Database) -> Result<(), crate::error::AppError> {
        db.save_setting("alert_enabled", &self.enabled.to_string())?;
        db.save_setting(
            "alert_failure_rate_threshold",
            &self.failure_rate_threshold.max(1.0).to_string(),
        )?;
        db.save_setting(
            "alert_min_request_count",
            &self.min_request_count.max(1).to_string(),
        )?;
        db.save_setting(
            "alert_silence_minutes",
            &self.silence_minutes.max(5).to_string(),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_retention_default() {
        let config = LogRetentionSettings::default();
        assert_eq!(config.retention_days, 5);
        assert_eq!(config.max_entries, 200);
    }

    #[test]
    fn test_load_log_retention_defaults() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        let config = LogRetentionSettings::load(&db);
        assert_eq!(config.retention_days, 5);
        assert_eq!(config.max_entries, 200);
    }

    #[test]
    fn test_load_log_retention_custom() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("log_retention_days", "30").unwrap();
        db.save_setting("log_max_entries", "5000").unwrap();

        let config = LogRetentionSettings::load(&db);
        assert_eq!(config.retention_days, 30);
        assert_eq!(config.max_entries, 5000);
    }

    #[test]
    fn test_load_log_retention_min_clamp_days() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("log_retention_days", "0").unwrap(); // below minimum

        let config = LogRetentionSettings::load(&db);
        assert_eq!(config.retention_days, 1); // clamped to minimum
    }

    #[test]
    fn test_load_log_retention_min_clamp_entries() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("log_max_entries", "5").unwrap(); // below minimum

        let config = LogRetentionSettings::load(&db);
        assert_eq!(config.max_entries, 10); // clamped to minimum
    }

    #[test]
    fn test_load_log_retention_invalid_falls_back() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("log_retention_days", "not_a_number").unwrap();
        db.save_setting("log_max_entries", "invalid").unwrap();

        let config = LogRetentionSettings::load(&db);
        assert_eq!(config.retention_days, 5); // falls back to default
        assert_eq!(config.max_entries, 200); // falls back to default
    }

    #[test]
    fn test_save_and_load_log_retention_roundtrip() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let settings = LogRetentionSettings {
            retention_days: 14,
            max_entries: 1000,
        };
        settings.save(&db).unwrap();

        let loaded = LogRetentionSettings::load(&db);
        assert_eq!(loaded.retention_days, 14);
        assert_eq!(loaded.max_entries, 1000);
    }

    #[test]
    fn test_save_log_retention_clamps() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        // Save with below-minimum values
        let settings = LogRetentionSettings {
            retention_days: 0,
            max_entries: 1,
        };
        settings.save(&db).unwrap();

        let loaded = LogRetentionSettings::load(&db);
        assert_eq!(loaded.retention_days, 1); // clamped on save
        assert_eq!(loaded.max_entries, 10); // clamped on save
    }

    // ── Configuration validation tests (P1-15) ──────────────────────

    #[test]
    fn test_validate_port_valid() {
        assert_eq!(validate_port(1).unwrap(), 1);
        assert_eq!(validate_port(8080).unwrap(), 8080);
        assert_eq!(validate_port(65535).unwrap(), 65535);
        assert_eq!(validate_port(47339).unwrap(), 47339);
    }

    #[test]
    fn test_validate_port_zero_rejected() {
        assert!(validate_port(0).is_err());
    }

    #[test]
    fn test_validate_listen_address_ipv4() {
        assert_eq!(validate_listen_address("127.0.0.1").unwrap(), "127.0.0.1");
        assert_eq!(validate_listen_address("0.0.0.0").unwrap(), "0.0.0.0");
        assert_eq!(validate_listen_address("192.168.1.1").unwrap(), "192.168.1.1");
    }

    #[test]
    fn test_validate_listen_address_ipv6() {
        assert_eq!(validate_listen_address("::1").unwrap(), "::1");
        assert_eq!(validate_listen_address("::").unwrap(), "::");
    }

    #[test]
    fn test_validate_listen_address_localhost() {
        assert_eq!(validate_listen_address("localhost").unwrap(), "localhost");
    }

    #[test]
    fn test_validate_listen_address_empty_rejected() {
        assert!(validate_listen_address("").is_err());
        assert!(validate_listen_address("   ").is_err());
    }

    #[test]
    fn test_validate_listen_address_invalid_rejected() {
        assert!(validate_listen_address("not_an_address").is_err());
        assert!(validate_listen_address("999.999.999.999").is_err());
    }

    #[test]
    fn test_validate_listen_address_trims_whitespace() {
        assert_eq!(validate_listen_address("  127.0.0.1  ").unwrap(), "127.0.0.1");
    }

    #[test]
    fn test_validate_api_key_valid() {
        assert_eq!(validate_api_key("sk-test-key").unwrap(), "sk-test-key");
        assert_eq!(validate_api_key("any-non-empty-string").unwrap(), "any-non-empty-string");
    }

    #[test]
    fn test_validate_api_key_empty_rejected() {
        assert!(validate_api_key("").is_err());
        assert!(validate_api_key("   ").is_err());
    }

    #[test]
    fn test_validate_api_key_trims_whitespace() {
        assert_eq!(validate_api_key("  sk-key  ").unwrap(), "sk-key");
    }

    // ── Configuration loading regression tests (P1-17) ──────────────

    #[test]
    fn test_gateway_settings_default() {
        let settings = GatewaySettings::default();
        assert_eq!(settings.listen_address, "127.0.0.1");
        assert_eq!(settings.listen_port, 47339);
        assert!(settings.api_key.is_empty());
        assert_eq!(settings.log_level, "info");
        assert!(settings.gateway_enabled);
    }

    #[test]
    fn test_gateway_settings_gateway_url() {
        let settings = GatewaySettings::default();
        assert_eq!(settings.gateway_url(), "http://127.0.0.1:47339");
        assert_eq!(settings.gateway_base_path(), "http://127.0.0.1:47339/v1");
    }

    #[test]
    fn test_rate_limit_settings_default() {
        let settings = RateLimitSettings::default();
        assert!(settings.enabled);
        assert_eq!(settings.max_requests, 60);
        assert_eq!(settings.window_seconds, 60);
        assert!(!settings.trust_forwarded_for);
    }

    #[test]
    fn test_rate_limit_settings_load_defaults() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        let settings = RateLimitSettings::load(&db);
        assert!(settings.enabled);
        assert_eq!(settings.max_requests, 60);
        assert_eq!(settings.window_seconds, 60);
        assert!(!settings.trust_forwarded_for);
    }

    #[test]
    fn test_rate_limit_settings_load_custom() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("rate_limit_enabled", "false").unwrap();
        db.save_setting("rate_limit_max_requests", "100").unwrap();
        db.save_setting("rate_limit_window_seconds", "120").unwrap();
        db.save_setting("rate_limit_trust_xff", "true").unwrap();

        let settings = RateLimitSettings::load(&db);
        assert!(!settings.enabled);
        assert_eq!(settings.max_requests, 100);
        assert_eq!(settings.window_seconds, 120);
        assert!(settings.trust_forwarded_for);
    }

    #[test]
    fn test_rate_limit_settings_load_invalid_falls_back() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("rate_limit_enabled", "not_a_bool").unwrap();
        db.save_setting("rate_limit_max_requests", "invalid").unwrap();
        db.save_setting("rate_limit_window_seconds", "not_a_number").unwrap();

        let settings = RateLimitSettings::load(&db);
        // Invalid bool string → false (since "not_a_bool" != "true")
        assert!(!settings.enabled);
        // Invalid numbers fall back to defaults
        assert_eq!(settings.max_requests, 60);
        assert_eq!(settings.window_seconds, 60);
    }

    #[test]
    fn test_probe_settings_default() {
        let settings = ProbeSettings::default();
        assert!(!settings.enabled);
        assert_eq!(settings.interval_seconds, 300);
        assert_eq!(settings.failure_threshold, 3);
    }

    #[test]
    fn test_probe_settings_load_defaults() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        let settings = ProbeSettings::load(&db);
        assert!(!settings.enabled);
        assert_eq!(settings.interval_seconds, 300);
        assert_eq!(settings.failure_threshold, 3);
    }

    #[test]
    fn test_probe_settings_load_invalid_falls_back() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("probe_interval_seconds", "not_a_number").unwrap();
        db.save_setting("probe_failure_threshold", "invalid").unwrap();

        let settings = ProbeSettings::load(&db);
        assert_eq!(settings.interval_seconds, 300); // falls back to default
        assert_eq!(settings.failure_threshold, 3); // falls back to default
    }
}
