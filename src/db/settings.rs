use crate::error::AppError;
use rusqlite::params;

use super::Database;

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
}
