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
    })
}

/// Update gateway settings (persisted to database).
#[tauri::command]
pub fn update_settings(req: SettingsVO, state: State<'_, AppState>) -> Result<(), String> {
    state.set_minimize_to_tray(req.minimize_to_tray);
    tracing::info!("AtomicBool cache updated: minimize_to_tray={}", req.minimize_to_tray);

    let db = &state.db;
    db.save_setting("listen_address", &req.listen_address).map_err(|e| e.to_string())?;
    db.save_setting("listen_port", &req.listen_port.to_string()).map_err(|e| e.to_string())?;
    db.save_setting("gateway_api_key", &req.api_key).map_err(|e| e.to_string())?;
    db.save_setting("log_level", &req.log_level).map_err(|e| e.to_string())?;
    db.save_setting("theme", &req.theme).map_err(|e| e.to_string())?;
    db.save_setting("minimize_to_tray", &req.minimize_to_tray.to_string()).map_err(|e| e.to_string())?;
    db.save_setting("language", &req.language).map_err(|e| e.to_string())?;

    let rl = llm_api_proxy_lib::config::RateLimitSettings {
        enabled: req.rate_limit_enabled,
        max_requests: req.rate_limit_max_requests,
        window_seconds: req.rate_limit_window_seconds,
        trust_forwarded_for: req.rate_limit_trust_xff,
    };
    rl.save(db).map_err(|e| e.to_string())?;

    let probe = llm_api_proxy_lib::config::ProbeSettings {
        enabled: req.probe_enabled,
        interval_seconds: req.probe_interval_seconds,
        failure_threshold: req.probe_failure_threshold,
    };
    probe.save(db).map_err(|e| e.to_string())?;

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
        .add_filter("所有文件", &["*"])
        .save_file()
        .await
        .ok_or_else(|| "用户取消了保存".to_string())?;

    let path = file_handle.path().to_path_buf();
    std::fs::write(&path, content.as_bytes())
        .map_err(|e| format!("写入文件失败: {}", e))?;

    Ok(path.to_string_lossy().to_string())
}
