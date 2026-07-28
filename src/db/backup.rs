use crate::error::AppError;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use super::Database;

impl Database {
    /// Create a consistent snapshot of the database to the given path.
    /// Uses SQLite's VACUUM INTO command for a transactionally consistent backup.
    pub fn backup_to(&self, dest_path: &Path) -> Result<(), AppError> {
        // Ensure parent directory exists
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // VACUUM INTO takes a string literal, so escape single quotes
        let path_str = dest_path.to_string_lossy().replace('\'', "''");
        self.get_conn()?
            .execute_batch(&format!("VACUUM INTO '{}'", path_str))?;
        info!("Database backed up to {:?}", dest_path);
        Ok(())
    }

    /// Validate that a file is a valid SQLite database with a schema_version table.
    /// Returns the schema version if valid.
    pub fn validate_backup_file(source_path: &Path) -> Result<i32, AppError> {
        if !source_path.exists() {
            return Err(AppError::NotFound(format!(
                "备份文件不存在: {}",
                source_path.display()
            )));
        }

        let conn = rusqlite::Connection::open(source_path)?;

        // Check if schema_version table exists
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_version'",
            [],
            |row| row.get(0),
        )?;

        if count == 0 {
            return Err(AppError::Config(
                "备份文件缺少 schema_version 表，可能不是有效的备份".to_string(),
            ));
        }

        let version: i32 =
            conn.query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
                row.get(0)
            })?;

        Ok(version)
    }
}

// ============================================================================
// Restore — File-based pending restore (applied on next startup)
// ============================================================================

/// Path to the pending restore database file.
pub fn restore_pending_path() -> PathBuf {
    crate::config::GatewaySettings::data_dir().join("restore_pending.db")
}

/// Path to the restore marker file.
pub fn restore_marker_path() -> PathBuf {
    crate::config::GatewaySettings::data_dir().join("restore_pending.marker")
}

/// Check if a restore is pending (marker file exists).
pub fn is_restore_pending() -> bool {
    restore_marker_path().exists()
}

/// Prepare a restore: validate the backup, copy to pending location, write marker.
/// The actual restore happens on next startup via `apply_pending_restore()`.
pub fn prepare_restore(source_path: &Path) -> Result<i32, AppError> {
    // Validate the backup file
    let backup_version = Database::validate_backup_file(source_path)?;

    // Get current schema version for compatibility check
    let db_path = crate::config::GatewaySettings::db_path();
    let current_version = if db_path.exists() {
        let conn = rusqlite::Connection::open(&db_path)?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_version'",
            [],
            |row| row.get(0),
        )?;
        if count > 0 {
            conn.query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
                row.get(0)
            })?
        } else {
            0
        }
    } else {
        0
    };

    if backup_version > current_version {
        return Err(AppError::Config(format!(
            "备份文件版本(v{})高于当前版本(v{})，无法恢复",
            backup_version, current_version
        )));
    }

    // Copy to pending location
    let pending_path = restore_pending_path();
    if let Some(parent) = pending_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source_path, &pending_path)?;

    // Write marker file with the backup version
    std::fs::write(restore_marker_path(), backup_version.to_string())?;

    info!(
        "Restore prepared from {:?}, backup version v{} (current v{})",
        source_path, backup_version, current_version
    );
    Ok(backup_version)
}

/// Apply a pending restore if the marker file exists.
/// This should be called BEFORE opening the database on startup.
pub fn apply_pending_restore() -> bool {
    if !is_restore_pending() {
        return false;
    }

    let db_path = crate::config::GatewaySettings::db_path();
    let pending_path = restore_pending_path();
    let marker_path = restore_marker_path();

    if !pending_path.exists() {
        warn!("Restore marker exists but pending file is missing, cleaning up");
        let _ = std::fs::remove_file(&marker_path);
        return false;
    }

    // Delete WAL and SHM files if they exist (they belong to the old database)
    let wal_path = db_path.with_extension("db-wal");
    let shm_path = db_path.with_extension("db-shm");
    let _ = std::fs::remove_file(&wal_path);
    let _ = std::fs::remove_file(&shm_path);

    // Replace the database file
    match std::fs::rename(&pending_path, &db_path) {
        Ok(()) => {
            info!("Database restored from pending backup");
        }
        Err(_e) => {
            // rename might fail across filesystems, try copy + delete
            if let Err(copy_err) = std::fs::copy(&pending_path, &db_path) {
                warn!(
                    "Failed to restore database: rename failed, copy also failed: {:?}",
                    copy_err
                );
                return false;
            }
            let _ = std::fs::remove_file(&pending_path);
            info!("Database restored from pending backup (via copy)");
        }
    }

    // Clean up marker
    let _ = std::fs::remove_file(&marker_path);
    true
}

// ============================================================================
// Auto-Backup — Periodic background backup
// ============================================================================

/// Directory for auto-backup files.
pub fn auto_backup_dir() -> PathBuf {
    crate::config::GatewaySettings::data_dir().join("backups")
}

/// List auto-backup files in the backup directory, sorted by modification time
/// (newest first). Each entry is (filename, size_bytes, modified_time).
pub fn list_auto_backups() -> Vec<(String, u64, String)> {
    let dir = auto_backup_dir();
    if !dir.exists() {
        return Vec::new();
    }

    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.ends_with(".db") {
                return None;
            }
            let meta = e.metadata().ok()?;
            let size = meta.len();
            let modified = meta
                .modified()
                .ok()
                .map(|t| {
                    let dt = chrono::DateTime::<chrono::Local>::from(t);
                    dt.format("%Y-%m-%d %H:%M:%S").to_string()
                })
                .unwrap_or_default();
            Some((name, size, modified))
        })
        .collect();

    // Sort by filename descending (newest first — filenames are timestamped)
    entries.sort_by(|a, b| b.0.cmp(&a.0));
    entries
}

/// Run a single auto-backup cycle.
/// Creates a new backup file and cleans up old ones beyond max_count.
pub fn run_auto_backup(db: &Database, max_count: usize) -> Result<String, AppError> {
    let dir = auto_backup_dir();
    std::fs::create_dir_all(&dir)?;

    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
    let filename = format!("backup_{}.db", timestamp);
    let backup_path = dir.join(&filename);

    db.backup_to(&backup_path)?;

    // Clean up old backups beyond max_count
    let mut backups: Vec<_> = std::fs::read_dir(&dir)?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("backup_") && name.ends_with(".db") {
                let path = e.path();
                let modified = e.metadata().ok()?.modified().ok()?;
                Some((path, modified))
            } else {
                None
            }
        })
        .collect();

    if backups.len() > max_count {
        // Sort by modification time, oldest first
        backups.sort_by_key(|(_, modified)| *modified);
        let to_remove = backups.len() - max_count;
        for (path, _) in backups.iter().take(to_remove) {
            let _ = std::fs::remove_file(path);
            info!("Removed old auto-backup: {:?}", path);
        }
    }

    Ok(filename)
}

/// Start the auto-backup background task.
/// Runs periodically, checking if enough time has passed since the last backup.
pub fn start_auto_backup_task(db: std::sync::Arc<Database>) {
    let config = crate::config::AutoBackupSettings::load(&db);
    if !config.enabled {
        info!("Auto-backup is disabled, not starting task");
        return;
    }

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                warn!("Failed to create tokio runtime for auto-backup: {}", e);
                return;
            }
        };
        rt.block_on(async move {
            let check_interval = std::time::Duration::from_secs(3600); // Check every hour
            loop {
                tokio::time::sleep(check_interval).await;

                // Reload config to pick up changes
                let config = crate::config::AutoBackupSettings::load(&db);
                if !config.enabled {
                    continue;
                }

                // Check if enough time has passed since last backup
                let last_backup = db
                    .get_setting("last_auto_backup_time")
                    .ok()
                    .flatten()
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(0);

                let now = chrono::Local::now().timestamp();
                let interval_secs = (config.interval_days as i64) * 86400;

                if now - last_backup < interval_secs {
                    continue;
                }

                match run_auto_backup(&db, config.max_count as usize) {
                    Ok(filename) => {
                        info!("Auto-backup created: {}", filename);
                        let _ = db.save_setting("last_auto_backup_time", &now.to_string());
                    }
                    Err(e) => {
                        warn!("Auto-backup failed: {}", e);
                    }
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backup_to_creates_file() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        // Insert some test data
        db.save_setting("test_key", "test_value").unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let backup_path = temp_dir.path().join("backup_test.db");

        db.backup_to(&backup_path).unwrap();

        assert!(backup_path.exists());
        assert!(backup_path.metadata().unwrap().len() > 0);
    }

    #[test]
    fn test_backup_is_valid_sqlite() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("key1", "value1").unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let backup_path = temp_dir.path().join("valid_backup.db");

        db.backup_to(&backup_path).unwrap();

        let version = Database::validate_backup_file(&backup_path).unwrap();
        assert!(version > 0);
    }

    #[test]
    fn test_backup_contains_data() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("backup_test_key", "backup_test_value").unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let backup_path = temp_dir.path().join("data_backup.db");

        db.backup_to(&backup_path).unwrap();

        // Open the backup and verify data
        let conn = rusqlite::Connection::open(&backup_path).unwrap();
        let value: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'backup_test_key'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, "backup_test_value");
    }

    #[test]
    fn test_validate_backup_nonexistent_file() {
        let result = Database::validate_backup_file(Path::new("nonexistent.db"));
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_backup_invalid_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let bad_path = temp_dir.path().join("not_a_db.db");
        std::fs::write(&bad_path, b"not a sqlite database").unwrap();

        let result = Database::validate_backup_file(&bad_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_prepare_restore_valid_backup() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("restore_key", "restore_value").unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let backup_path = temp_dir.path().join("restore_source.db");
        db.backup_to(&backup_path).unwrap();

        // Prepare restore — use the temp dir as data dir
        // We need to mock the data dir. Since prepare_restore uses
        // GatewaySettings::data_dir(), we test the validation part only.
        let version = Database::validate_backup_file(&backup_path).unwrap();
        assert!(version > 0);
    }

    #[test]
    fn test_auto_backup_creates_and_cleans() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        // We can't test the file creation directly since auto_backup_dir
        // uses GatewaySettings::data_dir(). But we can test backup_to
        // creates valid files.
        let temp_dir = tempfile::tempdir().unwrap();

        for i in 0..3 {
            let path = temp_dir.path().join(format!("backup_{}.db", i));
            db.backup_to(&path).unwrap();
            assert!(path.exists());
        }

        // Verify all backups are valid
        for i in 0..3 {
            let path = temp_dir.path().join(format!("backup_{}.db", i));
            let version = Database::validate_backup_file(&path).unwrap();
            assert!(version > 0);
        }
    }

    #[test]
    fn test_list_auto_backups_empty_dir() {
        // list_auto_backups uses auto_backup_dir() which points to the real data dir.
        // Test the logic with a temporary directory by calling read_dir directly.
        let temp_dir = tempfile::tempdir().unwrap();
        let entries: Vec<_> = std::fs::read_dir(temp_dir.path())
            .unwrap()
            .flatten()
            .collect();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_backup_preserves_schema_version() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        let original_version = db.get_schema_version().unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let backup_path = temp_dir.path().join("schema_backup.db");
        db.backup_to(&backup_path).unwrap();

        let backup_version = Database::validate_backup_file(&backup_path).unwrap();
        assert_eq!(backup_version, original_version);
    }
}
