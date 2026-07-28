use crate::error::AppError;
use rusqlite::params;

use super::{ConfigChangeEntry, Database};

impl Database {
    // ========================================================================
    // Settings
    // ========================================================================

    /// Save a setting (insert or update).
    pub fn save_setting(&self, key: &str, value: &str) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=?2, updated_at=datetime('now', 'localtime')",
            params![key, value],
        )?;
        Ok(())
    }

    /// Get a setting value by key.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>, AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare("SELECT value FROM settings WHERE key=?1")?;
        let result = stmt.query_row(params![key], |row| row.get::<_, String>(0));
        match result {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e)),
        }
    }

    /// Get all settings as key-value pairs.
    pub fn get_all_settings(&self) -> Result<Vec<(String, String)>, AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare("SELECT key, value FROM settings ORDER BY key")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        Self::collect_rows(rows)
    }

    /// Delete a setting by key.
    pub fn delete_setting(&self, key: &str) -> Result<(), AppError> {
        self.get_conn()?.execute("DELETE FROM settings WHERE key=?", params![key])?;
        Ok(())
    }

    /// Save a setting and record the change in the audit table.
    /// If the value hasn't changed, no audit entry is written.
    pub fn save_setting_with_audit(&self, key: &str, value: &str) -> Result<bool, AppError> {
        let old_value = self.get_setting(key)?;
        // Only record if the value actually changed
        if old_value.as_deref() == Some(value) {
            return Ok(false); // no change
        }
        self.save_setting(key, value)?;
        self.insert_config_change(key, old_value.as_deref(), value)?;
        Ok(true)
    }

    /// Insert a configuration change audit entry.
    pub fn insert_config_change(
        &self,
        key: &str,
        old_value: Option<&str>,
        new_value: &str,
    ) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "INSERT INTO config_changes (key, old_value, new_value) VALUES (?1, ?2, ?3)",
            params![key, old_value, new_value],
        )?;
        Ok(())
    }

    /// Get recent configuration changes with pagination.
    pub fn get_config_changes(&self, limit: i64, offset: i64) -> Result<Vec<ConfigChangeEntry>, AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, key, old_value, new_value, changed_at
             FROM config_changes
             ORDER BY changed_at DESC
             LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![limit, offset], |row| {
            Ok(ConfigChangeEntry {
                id: row.get(0)?,
                key: row.get(1)?,
                old_value: row.get(2)?,
                new_value: row.get(3)?,
                changed_at: row.get(4)?,
            })
        })?;
        Self::collect_rows(rows)
    }

    /// Count total configuration change entries.
    pub fn count_config_changes(&self) -> Result<i64, AppError> {
        let count: i64 = self.get_conn()?.query_row(
            "SELECT COUNT(*) FROM config_changes",
            [], |row| row.get(0),
        )?;
        Ok(count)
    }
}
