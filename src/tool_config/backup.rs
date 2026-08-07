//! Configuration backup & restore helpers.
//!
//! Backup data (file paths + original contents) is persisted in the DB
//! `tool_configs.original_config` JSON column by the switch manager. This
//! module provides the read/write helpers used by writers.

use crate::error::AppError;
use std::path::{Path, PathBuf};

/// A backed-up config file: path + optional original content (None = file didn't exist).
pub type BackupEntry = (PathBuf, Option<String>);

/// Serialize backup entries to a JSON string for DB storage.
pub fn serialize_backup(entries: &[BackupEntry]) -> String {
    serde_json::to_string(entries).unwrap_or_else(|_| "[]".to_string())
}

/// Deserialize backup entries from a JSON string.
pub fn deserialize_backup(json: &str) -> Vec<BackupEntry> {
    serde_json::from_str(json).unwrap_or_default()
}

/// Write the given content to the target path (creating parent dirs).
pub fn write_content(path: &Path, content: &str) -> Result<(), AppError> {
    super::writer::atomic_write(path, content.as_bytes())
}

// ============================================================================
// Secondary on-disk backup (crash recovery, see PLAN 1.4.9)
//
// When a tool is enabled we also drop a copy of the ORIGINAL config next to the
// live file, named `<config>.llm-proxy-backup`. If the app is killed (no exit
// hook ran) the backup file survives; on next startup `recover_from_crash`
// detects it, restores the original, then re-injects (switch stays ON).
// A marker payload records the case where the config file did not exist before.
// ============================================================================

/// Suffix appended to a config path to form the secondary backup path.
pub const BACKUP_SUFFIX: &str = ".llm-proxy-backup";

/// Marker stored in a secondary backup file when the original config file
/// did not exist (restoring it means deleting the injected file).
pub const ABSENT_MARKER: &str = "__LLM_PROXY_ABSENT__";

/// Secondary backup path for a config file (`<config>.llm-proxy-backup`).
pub fn secondary_backup_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(BACKUP_SUFFIX);
    PathBuf::from(os)
}

/// Write the secondary backup for one config entry (content is the original).
pub fn write_secondary_backup(path: &Path, content: Option<&str>) -> Result<(), AppError> {
    let bp = secondary_backup_path(path);
    let data = match content {
        Some(c) => c.as_bytes().to_vec(),
        None => ABSENT_MARKER.as_bytes().to_vec(),
    };
    std::fs::write(&bp, data)
        .map_err(|e| AppError::Config(format!("写入二级备份失败 {}: {e}", bp.display())))
}

/// Remove the secondary backup file for a config path (after clean restore).
pub fn remove_secondary_backup(path: &Path) {
    let _ = std::fs::remove_file(secondary_backup_path(path));
}

/// Restore a config file from its secondary backup, if present.
///
/// Returns `true` when a backup existed and was applied (i.e. a previous run
/// terminated abnormally). The backup file itself is deleted afterwards.
pub fn restore_from_secondary_backup(path: &Path) -> Result<bool, AppError> {
    let bp = secondary_backup_path(path);
    if !bp.exists() {
        return Ok(false);
    }
    let data = std::fs::read(&bp)
        .map_err(|e| AppError::Config(format!("读取二级备份失败 {}: {e}", bp.display())))?;
    if data == ABSENT_MARKER.as_bytes() {
        let _ = std::fs::remove_file(path);
    } else {
        super::writer::atomic_write(path, &data)?;
    }
    let _ = std::fs::remove_file(&bp);
    Ok(true)
}
