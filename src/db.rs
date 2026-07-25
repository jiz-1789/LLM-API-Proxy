use crate::error::AppError;
use rusqlite::{params, Connection};
use std::path::Path;
use tracing::{debug, info};

/// Database wrapper around SQLite.
/// All configuration and request logs are stored here.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open or create the SQLite database at the given path.
    pub fn open(path: &Path) -> Result<Self, AppError> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        debug!("Opened SQLite database at {:?}", path);
        Ok(Self { conn })
    }

    /// Initialize all tables. Safe to call multiple times.
    pub fn initialize(&self) -> Result<(), AppError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS upstreams (
                id TEXT PRIMARY KEY,
                provider_name TEXT NOT NULL,
                base_url TEXT NOT NULL,
                api_key_encrypted BLOB NOT NULL,
                selected_model TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                remark TEXT DEFAULT '',
                status TEXT NOT NULL DEFAULT 'healthy',
                failure_count INTEGER NOT NULL DEFAULT 0,
                last_failure_time TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS pools (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                display_name TEXT NOT NULL,
                round_robin_strategy TEXT NOT NULL DEFAULT 'sequential',
                failover_enabled INTEGER NOT NULL DEFAULT 1,
                timeout_seconds INTEGER NOT NULL DEFAULT 30,
                max_concurrency INTEGER NOT NULL DEFAULT 5,
                thinking_enabled INTEGER NOT NULL DEFAULT 0,
                circuit_breaker_threshold INTEGER NOT NULL DEFAULT 3,
                circuit_breaker_duration_seconds INTEGER NOT NULL DEFAULT 60,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS pool_upstreams (
                pool_id TEXT NOT NULL REFERENCES pools(id) ON DELETE CASCADE,
                upstream_id TEXT NOT NULL REFERENCES upstreams(id) ON DELETE CASCADE,
                sort_order INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (pool_id, upstream_id)
            );

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS request_logs (
                id TEXT PRIMARY KEY,
                request_id TEXT NOT NULL,
                pool_name TEXT,
                upstream_id TEXT,
                failed_upstreams TEXT DEFAULT '[]',
                method TEXT NOT NULL,
                endpoint TEXT NOT NULL,
                status_code INTEGER NOT NULL,
                response_time_ms INTEGER NOT NULL,
                is_streaming INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_request_logs_created_at ON request_logs(created_at);
            CREATE INDEX IF NOT EXISTS idx_request_logs_pool_name ON request_logs(pool_name);"
        )?;
        info!("Database schema initialized");
        Ok(())
    }

    // ---- Upstream CRUD ----

    pub fn create_upstream(
        &self,
        id: &str,
        provider_name: &str,
        base_url: &str,
        api_key_encrypted: &[u8],
        selected_model: &str,
        enabled: bool,
        remark: &str,
    ) -> Result<(), AppError> {
        self.conn.execute(
            "INSERT INTO upstreams (id, provider_name, base_url, api_key_encrypted, selected_model, enabled, remark)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, provider_name, base_url, api_key_encrypted, selected_model, enabled as i32, remark],
        )?;
        Ok(())
    }

    pub fn get_upstreams(&self) -> Result<Vec<(String, String, String, Vec<u8>, String, bool, String, String, i32, Option<String>)>, AppError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, provider_name, base_url, api_key_encrypted, selected_model, enabled, remark, status, failure_count, last_failure_time
             FROM upstreams ORDER BY created_at DESC"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i32>(5)? != 0,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, i32>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })?;
        let mut result = Vec::new();
        for r in rows {
            result.push(r?);
        }
        Ok(result)
    }

    pub fn update_upstream(
        &self,
        id: &str,
        provider_name: &str,
        base_url: &str,
        api_key_encrypted: &[u8],
        selected_model: &str,
        enabled: bool,
        remark: &str,
    ) -> Result<(), AppError> {
        self.conn.execute(
            "UPDATE upstreams SET provider_name=?1, base_url=?2, api_key_encrypted=?3,
             selected_model=?4, enabled=?5, remark=?6, updated_at=datetime('now')
             WHERE id=?7",
            params![provider_name, base_url, api_key_encrypted, selected_model, enabled as i32, remark, id],
        )?;
        Ok(())
    }

    pub fn delete_upstream(&self, id: &str) -> Result<(), AppError> {
        self.conn.execute("DELETE FROM upstreams WHERE id=?", params![id])?;
        Ok(())
    }

    pub fn toggle_upstream(&self, id: &str, enabled: bool) -> Result<(), AppError> {
        self.conn.execute(
            "UPDATE upstreams SET enabled=?, updated_at=datetime('now') WHERE id=?",
            params![enabled as i32, id],
        )?;
        Ok(())
    }

    pub fn update_upstream_status(
        &self,
        id: &str,
        status: &str,
        failure_count: i32,
        last_failure_time: Option<String>,
    ) -> Result<(), AppError> {
        self.conn.execute(
            "UPDATE upstreams SET status=?, failure_count=?, last_failure_time=?, updated_at=datetime('now')
             WHERE id=?",
            params![status, failure_count, last_failure_time, id],
        )?;
        Ok(())
    }

    // ---- Pool CRUD ----

    pub fn create_pool(
        &self,
        id: &str,
        name: &str,
        display_name: &str,
        max_concurrency: i32,
        thinking_enabled: bool,
    ) -> Result<(), AppError> {
        self.conn.execute(
            "INSERT INTO pools (id, name, display_name, max_concurrency, thinking_enabled)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, name, display_name, max_concurrency, thinking_enabled as i32],
        )?;
        Ok(())
    }

    pub fn get_pools(&self) -> Result<Vec<(String, String, String, i32, bool)>, AppError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, display_name, max_concurrency, thinking_enabled
             FROM pools ORDER BY created_at DESC"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i32>(3)?,
                row.get::<_, i32>(4)? != 0,
            ))
        })?;
        let mut result = Vec::new();
        for r in rows {
            result.push(r?);
        }
        Ok(result)
    }

    pub fn delete_pool(&self, id: &str) -> Result<(), AppError> {
        self.conn.execute("DELETE FROM pools WHERE id=?", params![id])?;
        Ok(())
    }

    pub fn add_upstream_to_pool(&self, pool_id: &str, upstream_id: &str, sort_order: i32) -> Result<(), AppError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO pool_upstreams (pool_id, upstream_id, sort_order)
             VALUES (?1, ?2, ?3)",
            params![pool_id, upstream_id, sort_order],
        )?;
        Ok(())
    }

    pub fn remove_upstream_from_pool(&self, pool_id: &str, upstream_id: &str) -> Result<(), AppError> {
        self.conn.execute(
            "DELETE FROM pool_upstreams WHERE pool_id=?1 AND upstream_id=?2",
            params![pool_id, upstream_id],
        )?;
        Ok(())
    }

    pub fn get_pool_upstreams(&self, pool_id: &str) -> Result<Vec<(String, String, i32)>, AppError> {
        let mut stmt = self.conn.prepare(
            "SELECT u.id, u.provider_name, pu.sort_order
             FROM pool_upstreams pu
             JOIN upstreams u ON u.id = pu.upstream_id
             WHERE pu.pool_id=?1
             ORDER BY pu.sort_order ASC"
        )?;
        let rows = stmt.query_map(params![pool_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i32>(2)?,
            ))
        })?;
        let mut result = Vec::new();
        for r in rows {
            result.push(r?);
        }
        Ok(result)
    }

    // ---- Settings ----

    pub fn save_setting(&self, key: &str, value: &str) -> Result<(), AppError> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=?2, updated_at=datetime('now')",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, AppError> {
        let mut stmt = self.conn.prepare("SELECT value FROM settings WHERE key=?1")?;
        let result = stmt.query_row(params![key], |row| row.get::<_, String>(0));
        match result {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e)),
        }
    }

    // ---- Request Logs ----

    pub fn insert_request_log(
        &self,
        id: &str,
        request_id: &str,
        pool_name: Option<&str>,
        upstream_id: Option<&str>,
        failed_upstreams: &str,
        method: &str,
        endpoint: &str,
        status_code: i32,
        response_time_ms: i32,
        is_streaming: bool,
    ) -> Result<(), AppError> {
        self.conn.execute(
            "INSERT INTO request_logs (id, request_id, pool_name, upstream_id, failed_upstreams, method, endpoint, status_code, response_time_ms, is_streaming)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id, request_id, pool_name, upstream_id, failed_upstreams,
                method, endpoint, status_code, response_time_ms, is_streaming as i32
            ],
        )?;
        Ok(())
    }

    pub fn get_recent_logs(&self, limit: i32) -> Result<Vec<(String, String, Option<String>, String, String, i32, i32, bool, String)>, AppError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, request_id, pool_name, method, endpoint, status_code, response_time_ms, is_streaming, created_at
             FROM request_logs ORDER BY created_at DESC LIMIT ?1"
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i32>(5)?,
                row.get::<_, i32>(6)?,
                row.get::<_, i32>(7)? != 0,
                row.get::<_, String>(8)?,
            ))
        })?;
        let mut result = Vec::new();
        for r in rows {
            result.push(r?);
        }
        Ok(result)
    }

    pub fn clear_logs(&self) -> Result<i64, AppError> {
        let rows = self.conn.execute("DELETE FROM request_logs", [])?;
        Ok(rows)
    }
}
