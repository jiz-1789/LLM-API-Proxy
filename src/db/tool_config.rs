use crate::error::AppError;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::Database;

/// A tool configuration record (one per tool).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfigRecord {
    pub id: String,
    pub tool_app_id: String,
    pub pool_id: Option<String>,
    pub api_key_id: Option<String>,
    pub provider_name: String,
    pub switch_enabled: bool,
    /// Original config file content(s) as JSON, for backup/restore.
    pub original_config: String,
    /// Snapshot of the proxy config written, as JSON.
    pub config_snapshot: String,
    pub last_written_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Detection result for a tool installation status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDetectionResult {
    pub app_id: String,
    pub display_name: String,
    pub installed: bool,
    pub config_paths: Vec<String>,
    pub download_url: String,
}

/// Combined switch status for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSwitchStatus {
    pub app_id: String,
    pub display_name: String,
    pub installed: bool,
    pub switch_enabled: bool,
    pub pool_id: Option<String>,
    pub pool_name: Option<String>,
    pub api_key_id: Option<String>,
    pub provider_name: String,
    /// Persisted role→pool mapping (from config_snapshot), if any.
    #[serde(default)]
    pub model_roles: Vec<(String, String)>,
    /// Persisted roles with 1M-context enabled (from config_snapshot), if any.
    #[serde(default)]
    pub model_roles_1m: Vec<String>,
    pub last_written_at: Option<String>,
}

impl Database {
    /// Save (upsert) a tool config record keyed by tool_app_id.
    pub fn save_tool_config(&self, record: &ToolConfigRecord) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "INSERT INTO tool_configs (id, tool_app_id, pool_id, api_key_id, provider_name,
                 switch_enabled, original_config, config_snapshot, last_written_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                     datetime('now','localtime'), datetime('now','localtime'))
             ON CONFLICT(tool_app_id) DO UPDATE SET
                 pool_id=?3, api_key_id=?4, provider_name=?5,
                 switch_enabled=?6, original_config=?7, config_snapshot=?8, last_written_at=?9,
                 updated_at=datetime('now','localtime')",
            params![
                record.id,
                record.tool_app_id,
                record.pool_id,
                record.api_key_id,
                record.provider_name,
                record.switch_enabled as i32,
                record.original_config,
                record.config_snapshot,
                record.last_written_at,
            ],
        )?;
        Ok(())
    }

    /// Get a tool config record by app_id.
    pub fn get_tool_config(&self, app_id: &str) -> Result<Option<ToolConfigRecord>, AppError> {
        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, tool_app_id, pool_id, api_key_id, provider_name, switch_enabled,
                    original_config, config_snapshot, last_written_at, created_at, updated_at
             FROM tool_configs WHERE tool_app_id = ?1",
        )?;
        let result = stmt.query_row(params![app_id], Self::map_tool_config_row);
        match result {
            Ok(r) => Ok(Some(r)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e)),
        }
    }

    /// Get all tool config records.
    pub fn get_all_tool_configs(&self) -> Result<Vec<ToolConfigRecord>, AppError> {
        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, tool_app_id, pool_id, api_key_id, provider_name, switch_enabled,
                    original_config, config_snapshot, last_written_at, created_at, updated_at
             FROM tool_configs ORDER BY tool_app_id",
        )?;
        let rows = stmt.query_map([], Self::map_tool_config_row)?;
        Self::collect_rows(rows)
    }

    /// Get all tool configs with switch enabled.
    pub fn get_tool_configs_by_switch(&self, enabled: bool) -> Result<Vec<ToolConfigRecord>, AppError> {
        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, tool_app_id, pool_id, api_key_id, provider_name, switch_enabled,
                    original_config, config_snapshot, last_written_at, created_at, updated_at
             FROM tool_configs WHERE switch_enabled = ?1 ORDER BY tool_app_id",
        )?;
        let rows = stmt.query_map(params![enabled as i32], Self::map_tool_config_row)?;
        Self::collect_rows(rows)
    }

    /// Delete a tool config record.
    pub fn delete_tool_config(&self, app_id: &str) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "DELETE FROM tool_configs WHERE tool_app_id=?1",
            params![app_id],
        )?;
        Ok(())
    }

    pub(crate) fn map_tool_config_row(row: &rusqlite::Row) -> rusqlite::Result<ToolConfigRecord> {
        Ok(ToolConfigRecord {
            id: row.get(0)?,
            tool_app_id: row.get(1)?,
            pool_id: row.get(2)?,
            api_key_id: row.get(3)?,
            provider_name: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            switch_enabled: row.get::<_, i32>(5)? != 0,
            original_config: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
            config_snapshot: row.get::<_, Option<String>>(7)?.unwrap_or_else(|| "{}".to_string()),
            last_written_at: row.get(8)?,
            created_at: row.get(9)?,
            updated_at: row.get(10)?,
        })
    }
}
