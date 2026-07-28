use serde::{Deserialize, Serialize};
use tauri::State;

use llm_api_proxy_lib::AppState;

// ============================================================================
// DTOs
// ============================================================================

/// Auto-backup settings DTO for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoBackupSettingsVO {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "super::default_7_u32")]
    pub interval_days: u32,
    #[serde(default = "super::default_5_u32")]
    pub max_count: u32,
}

/// Auto-backup file listing entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoBackupEntry {
    pub filename: String,
    pub size_bytes: u64,
    pub modified_time: String,
}

// ============================================================================
// Database Backup & Restore Commands
// ============================================================================

/// Show a native save dialog and backup the database to the chosen path.
#[tauri::command]
pub async fn backup_database(state: State<'_, AppState>) -> Result<String, String> {
    let file_handle = rfd::AsyncFileDialog::new()
        .set_file_name("proxy_backup.db")
        .add_filter("SQLite 数据库", &["db"])
        .add_filter("所有文件", &["*"])
        .save_file()
        .await
        .ok_or_else(|| "用户取消了备份".to_string())?;

    let path = file_handle.path().to_path_buf();
    state
        .db
        .backup_to(&path)
        .map_err(|e| format!("数据库备份失败: {}", e))?;

    Ok(path.to_string_lossy().to_string())
}

/// Show a native open dialog and prepare a database restore from the chosen file.
/// The restore is applied on next restart.
#[tauri::command]
pub async fn restore_database(state: State<'_, AppState>) -> Result<String, String> {
    let file_handle = rfd::AsyncFileDialog::new()
        .add_filter("SQLite 数据库", &["db"])
        .add_filter("所有文件", &["*"])
        .pick_file()
        .await
        .ok_or_else(|| "用户取消了恢复".to_string())?;

    let path = file_handle.path().to_path_buf();

    // Validate and prepare the restore
    let backup_version =
        llm_api_proxy_lib::db::backup::prepare_restore(&path).map_err(|e| e.to_string())?;

    // Record the restore in config changes audit
    let _ = state.db.insert_config_change(
        "database_restore",
        None,
        &format!("v{}", backup_version),
    );

    Ok(format!(
        "数据库恢复已准备完成（备份版本 v{}），请重启应用以完成恢复",
        backup_version
    ))
}

/// Check if a database restore is pending (marker file exists).
#[tauri::command]
pub fn check_restore_pending() -> bool {
    llm_api_proxy_lib::db::backup::is_restore_pending()
}

// ============================================================================
// Auto-Backup Commands
// ============================================================================

/// Get auto-backup settings.
#[tauri::command]
pub fn get_auto_backup_settings(state: State<'_, AppState>) -> AutoBackupSettingsVO {
    let settings = llm_api_proxy_lib::config::AutoBackupSettings::load(&state.db);
    AutoBackupSettingsVO {
        enabled: settings.enabled,
        interval_days: settings.interval_days,
        max_count: settings.max_count,
    }
}

/// Update auto-backup settings.
#[tauri::command]
pub fn update_auto_backup_settings(
    req: AutoBackupSettingsVO,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let settings = llm_api_proxy_lib::config::AutoBackupSettings {
        enabled: req.enabled,
        interval_days: req.interval_days.max(1),
        max_count: req.max_count.max(1),
    };

    // Save each key with audit — save_setting_with_audit detects value changes
    // and records config_change entries. Do NOT call settings.save() first,
    // as it would overwrite values before save_setting_with_audit can detect
    // the change, causing audit entries to be silently skipped.
    state
        .db
        .save_setting_with_audit("auto_backup_enabled", &settings.enabled.to_string())
        .map_err(|e| e.to_string())?;
    state
        .db
        .save_setting_with_audit(
            "auto_backup_interval_days",
            &settings.interval_days.to_string(),
        )
        .map_err(|e| e.to_string())?;
    state
        .db
        .save_setting_with_audit("auto_backup_max_count", &settings.max_count.to_string())
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// List all auto-backup files in the backup directory.
#[tauri::command]
pub fn list_auto_backups() -> Vec<AutoBackupEntry> {
    llm_api_proxy_lib::db::backup::list_auto_backups()
        .into_iter()
        .map(|(name, size, modified)| AutoBackupEntry {
            filename: name,
            size_bytes: size,
            modified_time: modified,
        })
        .collect()
}

/// Manually trigger an auto-backup now.
#[tauri::command]
pub fn create_backup_now(state: State<'_, AppState>) -> Result<String, String> {
    let settings = llm_api_proxy_lib::config::AutoBackupSettings::load(&state.db);
    let filename =
        llm_api_proxy_lib::db::backup::run_auto_backup(&state.db, settings.max_count as usize)
            .map_err(|e| e.to_string())?;

    // Update last backup time
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let _ = state.db.save_setting("last_auto_backup_time", &now.to_string());

    Ok(filename)
}

// ============================================================================
// Config Import / Export Commands
// ============================================================================

/// Export all configuration (upstreams, pools, settings) to a JSON file.
/// API keys are included in plaintext for portability — warn the user.
#[tauri::command]
pub async fn export_config(state: State<'_, AppState>) -> Result<String, String> {
    let config = llm_api_proxy_lib::config_io::export_config(&state.db, &state.crypto)
        .map_err(|e| e.to_string())?;

    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;

    let file_handle = rfd::AsyncFileDialog::new()
        .set_file_name("llm-api-proxy-config.json")
        .add_filter("JSON 文件", &["json"])
        .add_filter("所有文件", &["*"])
        .save_file()
        .await
        .ok_or_else(|| "用户取消了导出".to_string())?;

    let path = file_handle.path().to_path_buf();
    std::fs::write(&path, json.as_bytes()).map_err(|e| format!("写入文件失败: {}", e))?;

    Ok(path.to_string_lossy().to_string())
}

/// Import configuration from a JSON file.
/// Supports incremental (add/update) and full (replace all) modes.
#[tauri::command]
pub async fn import_config(
    mode: String,
    state: State<'_, AppState>,
) -> Result<ImportResultVO, String> {
    let file_handle = rfd::AsyncFileDialog::new()
        .add_filter("JSON 文件", &["json"])
        .add_filter("所有文件", &["*"])
        .pick_file()
        .await
        .ok_or_else(|| "用户取消了导入".to_string())?;

    let path = file_handle.path().to_path_buf();
    let json = std::fs::read_to_string(&path).map_err(|e| format!("读取文件失败: {}", e))?;

    // Parse the import request
    let import_mode = match mode.as_str() {
        "full" => llm_api_proxy_lib::config_io::ImportMode::Full,
        _ => llm_api_proxy_lib::config_io::ImportMode::Incremental,
    };

    // Try to parse as ConfigExport directly (the ImportRequest wrapper is optional)
    let config: llm_api_proxy_lib::config_io::ConfigExport =
        serde_json::from_str(&json).map_err(|e| format!("JSON 解析失败: {}", e))?;

    let result = llm_api_proxy_lib::config_io::import_config(&state.db, &state.crypto, &config, &import_mode)
        .map_err(|e| e.to_string())?;

    Ok(ImportResultVO {
        upstreams_added: result.upstreams_added,
        upstreams_updated: result.upstreams_updated,
        pools_added: result.pools_added,
        pools_updated: result.pools_updated,
        settings_imported: result.settings_imported,
        warnings: result.warnings,
    })
}

/// Import result DTO for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResultVO {
    pub upstreams_added: usize,
    pub upstreams_updated: usize,
    pub pools_added: usize,
    pub pools_updated: usize,
    pub settings_imported: usize,
    pub warnings: Vec<String>,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_backup_settings_vo_defaults() {
        let json = r#"{}"#;
        let vo: AutoBackupSettingsVO = serde_json::from_str(json).unwrap();
        assert!(!vo.enabled);
        assert_eq!(vo.interval_days, 7);
        assert_eq!(vo.max_count, 5);
    }

    #[test]
    fn test_auto_backup_settings_vo_full() {
        let json = r#"{
            "enabled": true,
            "interval_days": 14,
            "max_count": 10
        }"#;
        let vo: AutoBackupSettingsVO = serde_json::from_str(json).unwrap();
        assert!(vo.enabled);
        assert_eq!(vo.interval_days, 14);
        assert_eq!(vo.max_count, 10);
    }

    #[test]
    fn test_auto_backup_entry_serialization() {
        let entry = AutoBackupEntry {
            filename: "backup_20260728_100000.db".to_string(),
            size_bytes: 1024,
            modified_time: "2026-07-28 10:00:00".to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: AutoBackupEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.filename, "backup_20260728_100000.db");
        assert_eq!(parsed.size_bytes, 1024);
    }

    #[test]
    fn test_import_result_vo_serialization() {
        let result = ImportResultVO {
            upstreams_added: 3,
            upstreams_updated: 1,
            pools_added: 2,
            pools_updated: 0,
            settings_imported: 5,
            warnings: vec![],
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ImportResultVO = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.upstreams_added, 3);
        assert_eq!(parsed.pools_added, 2);
        assert_eq!(parsed.settings_imported, 5);
    }
}
