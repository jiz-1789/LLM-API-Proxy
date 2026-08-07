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
