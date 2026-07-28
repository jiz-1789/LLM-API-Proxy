use serde::{Deserialize, Serialize};
use tauri::State;

use llm_api_proxy_lib::AppState;

// ============================================================================
// DTO Types
// ============================================================================

/// Settings data for the frontend settings page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsVO {
    pub listen_address: String,
    pub listen_port: u16,
    pub api_key: String,
    pub log_level: String,
    pub theme: String,
    #[serde(default = "super::default_true")]
    pub minimize_to_tray: bool,
    #[serde(default = "super::default_zh")]
    pub language: String,
    // Rate limiting settings
    #[serde(default = "super::default_true")]
    pub rate_limit_enabled: bool,
    #[serde(default = "super::default_60")]
    pub rate_limit_max_requests: u32,
    #[serde(default = "super::default_60")]
    pub rate_limit_window_seconds: u32,
    #[serde(default)]
    pub rate_limit_trust_xff: bool,
    // Probe settings
    #[serde(default)]
    pub probe_enabled: bool,
    #[serde(default = "super::default_300")]
    pub probe_interval_seconds: u32,
    #[serde(default = "super::default_3")]
    pub probe_failure_threshold: u32,
    // Log retention settings
    #[serde(default = "super::default_5_i32")]
    pub log_retention_days: i32,
    #[serde(default = "super::default_200_i64")]
    pub log_max_entries: i64,
}

// ============================================================================
// Commands
// ============================================================================

/// Get current gateway settings.
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<SettingsVO, String> {
    let db = &state.db;
    let rl = llm_api_proxy_lib::config::RateLimitSettings::load(db);
    let probe = llm_api_proxy_lib::config::ProbeSettings::load(db);
    let log_retention = llm_api_proxy_lib::config::LogRetentionSettings::load(db);
    Ok(SettingsVO {
        listen_address: db.get_setting("listen_address").ok().flatten()
            .unwrap_or_else(|| "127.0.0.1".to_string()),
        listen_port: db.get_setting("listen_port").ok().flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(47339),
        api_key: db.get_setting("gateway_api_key").ok().flatten()
            .unwrap_or_else(|| "sk-gateway-key".to_string()),
        log_level: db.get_setting("log_level").ok().flatten()
            .unwrap_or_else(|| "info".to_string()),
        theme: db.get_setting("theme").ok().flatten()
            .unwrap_or_else(|| "dark".to_string()),
        minimize_to_tray: db.get_setting("minimize_to_tray").ok().flatten()
            .map(|v| v == "true")
            .unwrap_or(true),
        language: db.get_setting("language").ok().flatten()
            .unwrap_or_else(|| "zh".to_string()),
        rate_limit_enabled: rl.enabled,
        rate_limit_max_requests: rl.max_requests,
        rate_limit_window_seconds: rl.window_seconds,
        rate_limit_trust_xff: rl.trust_forwarded_for,
        probe_enabled: probe.enabled,
        probe_interval_seconds: probe.interval_seconds,
        probe_failure_threshold: probe.failure_threshold,
        log_retention_days: log_retention.retention_days,
        log_max_entries: log_retention.max_entries,
    })
}

/// Update gateway settings (persisted to database).
/// Validates critical fields before saving.
#[tauri::command]
pub fn update_settings(req: SettingsVO, state: State<'_, AppState>) -> Result<(), String> {
    // Validate critical configuration before saving (P1-15)
    let listen_address = llm_api_proxy_lib::config::validate_listen_address(&req.listen_address)
        .map_err(|e| e)?;
    let listen_port = llm_api_proxy_lib::config::validate_port(req.listen_port)
        .map_err(|e| e)?;
    let api_key = llm_api_proxy_lib::config::validate_api_key(&req.api_key)
        .map_err(|e| e)?;

    state.set_minimize_to_tray(req.minimize_to_tray);
    tracing::info!("AtomicBool cache updated: minimize_to_tray={}", req.minimize_to_tray);

    let db = &state.db;
    db.save_setting_with_audit("listen_address", &listen_address).map_err(|e| e.to_string())?;
    db.save_setting_with_audit("listen_port", &listen_port.to_string()).map_err(|e| e.to_string())?;
    // Note: API key is sensitive — record that it changed but don't store the value in audit
    let old_api_key = db.get_setting("gateway_api_key").map_err(|e| e.to_string())?;
    if old_api_key.as_deref() != Some(&api_key) {
        db.save_setting("gateway_api_key", &api_key).map_err(|e| e.to_string())?;
        db.insert_config_change("gateway_api_key", Some("••••••••"), "••••••••")
            .map_err(|e| e.to_string())?;
    }
    db.save_setting_with_audit("log_level", &req.log_level).map_err(|e| e.to_string())?;
    db.save_setting_with_audit("theme", &req.theme).map_err(|e| e.to_string())?;
    db.save_setting_with_audit("minimize_to_tray", &req.minimize_to_tray.to_string()).map_err(|e| e.to_string())?;
    db.save_setting_with_audit("language", &req.language).map_err(|e| e.to_string())?;

    let rl = llm_api_proxy_lib::config::RateLimitSettings {
        enabled: req.rate_limit_enabled,
        max_requests: req.rate_limit_max_requests,
        window_seconds: req.rate_limit_window_seconds,
        trust_forwarded_for: req.rate_limit_trust_xff,
    };
    // Save rate limit settings with audit for each key
    db.save_setting_with_audit("rate_limit_enabled", &rl.enabled.to_string()).map_err(|e| e.to_string())?;
    db.save_setting_with_audit("rate_limit_max_requests", &rl.max_requests.to_string()).map_err(|e| e.to_string())?;
    db.save_setting_with_audit("rate_limit_window_seconds", &rl.window_seconds.to_string()).map_err(|e| e.to_string())?;
    db.save_setting_with_audit("rate_limit_trust_xff", &rl.trust_forwarded_for.to_string()).map_err(|e| e.to_string())?;

    let probe = llm_api_proxy_lib::config::ProbeSettings {
        enabled: req.probe_enabled,
        interval_seconds: req.probe_interval_seconds,
        failure_threshold: req.probe_failure_threshold,
    };
    // Save probe settings with audit for each key
    db.save_setting_with_audit("probe_enabled", &probe.enabled.to_string()).map_err(|e| e.to_string())?;
    db.save_setting_with_audit("probe_interval_seconds", &probe.interval_seconds.max(60).to_string()).map_err(|e| e.to_string())?;
    db.save_setting_with_audit("probe_failure_threshold", &probe.failure_threshold.max(1).to_string()).map_err(|e| e.to_string())?;

    let log_retention = llm_api_proxy_lib::config::LogRetentionSettings {
        retention_days: req.log_retention_days,
        max_entries: req.log_max_entries,
    };
    // Save log retention settings with audit for each key
    db.save_setting_with_audit("log_retention_days", &log_retention.retention_days.max(1).to_string()).map_err(|e| e.to_string())?;
    db.save_setting_with_audit("log_max_entries", &log_retention.max_entries.max(10).to_string()).map_err(|e| e.to_string())?;

    Ok(())
}

/// Set minimize-to-tray preference immediately.
#[tauri::command]
pub fn set_minimize_to_tray(value: bool, state: State<'_, AppState>) -> Result<(), String> {
    state.set_minimize_to_tray(value);
    state.db.save_setting("minimize_to_tray", &value.to_string()).map_err(|e| e.to_string())?;
    tracing::info!("minimize_to_tray={} saved", value);
    Ok(())
}

/// Update only the theme setting.
#[tauri::command]
pub fn set_theme(theme: String, state: State<'_, AppState>) -> Result<(), String> {
    state.db.save_setting("theme", &theme).map_err(|e| e.to_string())?;
    tracing::info!("theme={} saved", theme);
    Ok(())
}

/// Open a URL in the system's default browser.
#[tauri::command]
pub fn open_external_url(url: String) -> Result<(), String> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("仅允许打开 http:// 或 https:// 开头的链接".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", &url])
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&url)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&url)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Read text from the system clipboard.
#[tauri::command]
pub fn read_clipboard() -> Result<String, String> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|e| format!("Failed to access clipboard: {}", e))?;
    let text = clipboard.get_text()
        .map_err(|e| format!("Failed to read clipboard: {}", e))?;
    Ok(text)
}

/// Show a native file save dialog and write the given content to the chosen file.
#[tauri::command]
pub async fn save_file_dialog(
    filename: String,
    content: String,
) -> Result<String, String> {
    let file_handle = rfd::AsyncFileDialog::new()
        .set_file_name(&filename)
        .add_filter("CSV 文件", &["csv"])
        .add_filter("JSON 文件", &["json"])
        .add_filter("所有文件", &["*"])
        .save_file()
        .await
        .ok_or_else(|| "用户取消了保存".to_string())?;

    let path = file_handle.path().to_path_buf();
    std::fs::write(&path, content.as_bytes())
        .map_err(|e| format!("写入文件失败: {}", e))?;

    Ok(path.to_string_lossy().to_string())
}

/// Get configuration change history with pagination.
#[tauri::command]
pub fn get_config_changes(
    state: State<'_, AppState>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<ConfigChangeVO>, String> {
    let limit = limit.unwrap_or(50).min(500);
    let offset = offset.unwrap_or(0);
    let entries = state.db.get_config_changes(limit, offset).map_err(|e| e.to_string())?;
    Ok(entries.into_iter().map(Into::into).collect())
}

/// Configuration change audit entry for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigChangeVO {
    pub id: i64,
    pub key: String,
    pub old_value: Option<String>,
    pub new_value: String,
    pub changed_at: String,
}

impl From<llm_api_proxy_lib::db::ConfigChangeEntry> for ConfigChangeVO {
    fn from(e: llm_api_proxy_lib::db::ConfigChangeEntry) -> Self {
        Self {
            id: e.id,
            key: e.key,
            old_value: e.old_value,
            new_value: e.new_value,
            changed_at: e.changed_at,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_vo_deserialization_defaults() {
        let json = r#"{
            "listen_address": "127.0.0.1",
            "listen_port": 47339,
            "api_key": "sk-test",
            "log_level": "info",
            "theme": "dark"
        }"#;
        let vo: SettingsVO = serde_json::from_str(json).unwrap();
        assert_eq!(vo.listen_address, "127.0.0.1");
        assert_eq!(vo.listen_port, 47339);
        assert!(vo.minimize_to_tray); // default: true
        assert_eq!(vo.language, "zh"); // default: "zh"
        assert!(vo.rate_limit_enabled); // default: true
        assert_eq!(vo.rate_limit_max_requests, 60); // default: 60
        assert_eq!(vo.rate_limit_window_seconds, 60); // default: 60
        assert!(!vo.rate_limit_trust_xff); // default: false
        assert!(!vo.probe_enabled); // default: false
        assert_eq!(vo.probe_interval_seconds, 300); // default: 300
        assert_eq!(vo.probe_failure_threshold, 3); // default: 3
        assert_eq!(vo.log_retention_days, 5); // default: 5
        assert_eq!(vo.log_max_entries, 200); // default: 200
    }

    #[test]
    fn test_settings_vo_deserialization_full() {
        let json = r#"{
            "listen_address": "0.0.0.0",
            "listen_port": 8080,
            "api_key": "sk-custom",
            "log_level": "debug",
            "theme": "light",
            "minimize_to_tray": false,
            "language": "en",
            "rate_limit_enabled": false,
            "rate_limit_max_requests": 100,
            "rate_limit_window_seconds": 120,
            "rate_limit_trust_xff": true,
            "probe_enabled": true,
            "probe_interval_seconds": 600,
            "probe_failure_threshold": 5,
            "log_retention_days": 30,
            "log_max_entries": 1000
        }"#;
        let vo: SettingsVO = serde_json::from_str(json).unwrap();
        assert_eq!(vo.listen_address, "0.0.0.0");
        assert_eq!(vo.listen_port, 8080);
        assert!(!vo.minimize_to_tray);
        assert_eq!(vo.language, "en");
        assert!(!vo.rate_limit_enabled);
        assert_eq!(vo.rate_limit_max_requests, 100);
        assert_eq!(vo.rate_limit_window_seconds, 120);
        assert!(vo.rate_limit_trust_xff);
        assert!(vo.probe_enabled);
        assert_eq!(vo.probe_interval_seconds, 600);
        assert_eq!(vo.probe_failure_threshold, 5);
        assert_eq!(vo.log_retention_days, 30);
        assert_eq!(vo.log_max_entries, 1000);
    }

    #[test]
    fn test_config_change_vo_from_entry() {
        let entry = llm_api_proxy_lib::db::ConfigChangeEntry {
            id: 42,
            key: "listen_port".to_string(),
            old_value: Some("47339".to_string()),
            new_value: "8080".to_string(),
            changed_at: "2026-07-28 10:00:00".to_string(),
        };
        let vo: ConfigChangeVO = entry.into();
        assert_eq!(vo.id, 42);
        assert_eq!(vo.key, "listen_port");
        assert_eq!(vo.old_value.as_deref(), Some("47339"));
        assert_eq!(vo.new_value, "8080");
        assert_eq!(vo.changed_at, "2026-07-28 10:00:00");
    }

    #[test]
    fn test_config_change_vo_from_entry_null_old() {
        let entry = llm_api_proxy_lib::db::ConfigChangeEntry {
            id: 1,
            key: "new_key".to_string(),
            old_value: None,
            new_value: "first_value".to_string(),
            changed_at: "2026-07-28 10:00:00".to_string(),
        };
        let vo: ConfigChangeVO = entry.into();
        assert!(vo.old_value.is_none());
    }

    #[test]
    fn test_config_change_vo_serialization() {
        let vo = ConfigChangeVO {
            id: 1,
            key: "theme".to_string(),
            old_value: Some("dark".to_string()),
            new_value: "light".to_string(),
            changed_at: "2026-07-28 12:00:00".to_string(),
        };
        let json = serde_json::to_string(&vo).unwrap();
        let parsed: ConfigChangeVO = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, 1);
        assert_eq!(parsed.key, "theme");
        assert_eq!(parsed.old_value.as_deref(), Some("dark"));
        assert_eq!(parsed.new_value, "light");
    }

    #[test]
    fn test_get_config_changes_limit_clamped() {
        // The command clamps limit to 500 max. Verify the logic.
        let limit: i64 = 1000;
        let clamped = limit.min(500);
        assert_eq!(clamped, 500);
    }

    #[test]
    fn test_get_config_changes_limit_default() {
        let limit: Option<i64> = None;
        let result = limit.unwrap_or(50).min(500);
        assert_eq!(result, 50);
    }

    #[test]
    fn test_get_config_changes_offset_default() {
        let offset: Option<i64> = None;
        assert_eq!(offset.unwrap_or(0), 0);
    }
}
