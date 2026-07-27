use crate::error::AppError;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, info, warn};

// ============================================================================
// Public Data Types
// ============================================================================

/// An upstream API provider record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Upstream {
    pub id: String,
    pub provider_name: String,
    pub base_url: String,
    pub api_key_encrypted: Vec<u8>,
    pub selected_model: String,
    pub available_models: String,
    pub enabled: bool,
    pub remark: String,
    pub status: String,
    pub failure_count: i32,
    pub last_failure_time: Option<String>,
    /// Last successful request time (v6, nullable for backward compat).
    pub last_success_time: Option<String>,
    /// Last error reason summary (v6, nullable for backward compat).
    pub last_error_reason: Option<String>,
    /// Timestamp when upstream recovered from down state (v6, nullable).
    pub recovered_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A model pool that groups upstreams with routing configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pool {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub round_robin_strategy: String,
    pub failover_enabled: bool,
    pub timeout_seconds: i32,
    pub max_concurrency: i32,
    pub thinking_enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// An upstream associated with a pool, including sort order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolUpstreamInfo {
    pub upstream_id: String,
    pub provider_name: String,
    pub model: String,
    pub sort_order: i32,
}

/// A single request log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestLogEntry {
pub id: String,
pub request_id: String,
pub pool_name: Option<String>,
pub upstream_id: Option<String>,
pub model: Option<String>,
pub failed_upstreams: String,
pub method: String,
pub endpoint: String,
pub status_code: i32,
pub response_time_ms: i32,
pub is_streaming: bool,
pub prompt_tokens: i64,
pub completion_tokens: i64,
pub total_tokens: i64,
pub created_at: String,
}

/// Aggregate statistics about the system.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stats {
pub upstream_count: i64,
pub active_upstream_count: i64,
pub pool_count: i64,
pub today_request_count: i64,
pub today_success_count: i64,
pub today_error_count: i64,
}

/// Daily token usage for charts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyTokenUsage {
pub date: String,
pub prompt_tokens: i64,
pub completion_tokens: i64,
pub total_tokens: i64,
pub request_count: i64,
}

/// Hourly token usage for today's bar chart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HourlyTokenUsage {
pub hour: String,
pub prompt_tokens: i64,
pub completion_tokens: i64,
pub total_tokens: i64,
pub request_count: i64,
}

/// Token totals summary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenTotals {
pub prompt_tokens: i64,
pub completion_tokens: i64,
pub total_tokens: i64,
}

/// Per-model token usage summary for an upstream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelTokenUsage {
pub model: String,
pub today_tokens: i64,
pub total_tokens: i64,
pub request_count: i64,
}

/// Per-upstream health summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamStatusSummary {
    pub id: String,
    pub provider_name: String,
    pub status: String,
    pub failure_count: i32,
    pub last_failure_time: Option<String>,
    pub last_success_time: Option<String>,
    pub last_error_reason: Option<String>,
    pub recovered_at: Option<String>,
}

/// Log query filter parameters.
#[derive(Debug, Clone, Default)]
pub struct LogFilter {
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub pool_name: Option<String>,
    /// Status code prefix for range filtering (e.g. 2 = 2xx, 4 = 4xx, 5 = 5xx).
    pub status_prefix: Option<i32>,
    pub limit: i64,
    pub offset: i64,
}

// ============================================================================
// Database
// ============================================================================

/// Database wrapper around SQLite.
/// All configuration and request logs are stored here.
pub struct Database {
    conn: Mutex<Connection>,
    /// Counter to trigger periodic log cleanup (every 100 inserts).
    log_insert_counter: AtomicU64,
}

impl Database {
    /// Helper to acquire the database connection lock safely.
    /// Returns an error if the mutex is poisoned (another thread panicked while holding it).
    fn get_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, AppError> {
        self.conn.lock()
            .map_err(|_| AppError::Internal("Database connection lock poisoned".into()))
    }

    /// Open or create the SQLite database at the given path.
    /// Configures WAL mode for better concurrent read/write performance.
    pub fn open(path: &Path) -> Result<Self, AppError> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             PRAGMA synchronous=NORMAL;
             PRAGMA cache_size=-64000;
             PRAGMA temp_store=MEMORY;"
        )?;
        debug!("Opened SQLite database at {:?} (WAL mode)", path);
        Ok(Self { conn: Mutex::new(conn), log_insert_counter: AtomicU64::new(0) })
    }

    /// Create an in-memory database (for testing).
    #[cfg(test)]
    pub(crate) fn open_in_memory() -> Result<Self, AppError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        Ok(Self { conn: Mutex::new(conn), log_insert_counter: AtomicU64::new(0) })
    }

    /// Initialize all tables and run migrations. Safe to call multiple times.
    /// Also performs automatic cleanup of old request logs.
    pub fn initialize(&self) -> Result<(), AppError> {
        self.create_schema()?;
        self.run_migrations()?;
        info!("Database schema initialized (version {})", self.get_schema_version()?);
        // Clean up old logs on startup: keep 5 days, cap at 200 entries
        let deleted = self.cleanup_old_logs(5, 200)?;
        if deleted > 0 {
            info!("Startup log cleanup: removed {} old log entries", deleted);
        }
        Ok(())
    }

    // ========================================================================
    // Schema & Migrations
    // ========================================================================

    fn create_schema(&self) -> Result<(), AppError> {
        self.get_conn()?.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS upstreams (
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
                created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
            );

            CREATE TABLE IF NOT EXISTS pools (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                display_name TEXT NOT NULL,
                round_robin_strategy TEXT NOT NULL DEFAULT 'round_robin',
                failover_enabled INTEGER NOT NULL DEFAULT 1,
                timeout_seconds INTEGER NOT NULL DEFAULT 30,
                max_concurrency INTEGER NOT NULL DEFAULT 5,
                thinking_enabled INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
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
                updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
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
                created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
            );

            CREATE INDEX IF NOT EXISTS idx_request_logs_created_at ON request_logs(created_at);
            CREATE INDEX IF NOT EXISTS idx_request_logs_pool_name ON request_logs(pool_name);"
        )?;

        // Ensure schema_version has a row
        let count: i64 = self.get_conn()?.query_row(
            "SELECT COUNT(*) FROM schema_version", [], |row| row.get(0)
        )?;
        if count == 0 {
            self.get_conn()?.execute("INSERT INTO schema_version (version) VALUES (0)", [])?;
        }
        Ok(())
    }

    fn run_migrations(&self) -> Result<(), AppError> {
        let current = self.get_schema_version()?;
        let migrations: Vec<(i32, &str)> = vec![
            (1, ""), // v1: initial schema (already created above)
            (2, "ALTER TABLE upstreams ADD COLUMN available_models TEXT NOT NULL DEFAULT '[]';
                 ALTER TABLE pool_upstreams ADD COLUMN model TEXT NOT NULL DEFAULT '';"),
            (3, "ALTER TABLE request_logs ADD COLUMN prompt_tokens INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE request_logs ADD COLUMN completion_tokens INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE request_logs ADD COLUMN total_tokens INTEGER NOT NULL DEFAULT 0;"),
            (4, "ALTER TABLE request_logs ADD COLUMN model TEXT;"),
            (5, "UPDATE pools SET round_robin_strategy = 'round_robin' WHERE round_robin_strategy = 'sequential';"),
            // v6 is handled separately below (idempotent ALTER TABLE)
        ];
        for (version, sql) in migrations {
            if current < version {
                if !sql.is_empty() {
                    self.get_conn()?.execute_batch(sql)?;
                }
                self.get_conn()?.execute(
                    "UPDATE schema_version SET version = ?1",
                    params![version],
                )?;
                info!("Database migrated to version {}", version);
            }
        }

        // v6: Add upstream health tracking columns.
        //
        // DEFENSIVE: Some development builds may have had schema_version
        // incorrectly bumped without the columns actually being present.
        // We therefore check column existence via PRAGMA instead of relying
        // solely on the version number.
        let has_last_success_time = self.column_exists("upstreams", "last_success_time")?;
        let has_last_error_reason = self.column_exists("upstreams", "last_error_reason")?;
        let has_recovered_at = self.column_exists("upstreams", "recovered_at")?;

        if current < 6 || !has_last_success_time || !has_last_error_reason || !has_recovered_at {
            let alter_statements = [
                ("ALTER TABLE upstreams ADD COLUMN last_success_time TEXT", has_last_success_time),
                ("ALTER TABLE upstreams ADD COLUMN last_error_reason TEXT", has_last_error_reason),
                ("ALTER TABLE upstreams ADD COLUMN recovered_at TEXT", has_recovered_at),
            ];
            for (sql, already_exists) in &alter_statements {
                if *already_exists {
                    continue;
                }
                if let Err(e) = self.get_conn()?.execute(sql, []) {
                    // Ignore "duplicate column name" — column already exists from a partial run
                    if !e.to_string().contains("duplicate column name") {
                        return Err(AppError::Database(e));
                    }
                }
            }
            self.get_conn()?.execute(
                "UPDATE schema_version SET version = 6",
                [],
            )?;
            info!("Database migrated to version 6 (columns verified)");
        }

        Ok(())
    }

    /// Get the current schema version.
    pub fn get_schema_version(&self) -> Result<i32, AppError> {
        let result = self.get_conn()?.query_row(
            "SELECT version FROM schema_version LIMIT 1",
            [],
            |row| row.get::<_, i32>(0),
        );
        match result {
            Ok(v) => Ok(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(0),
            Err(e) => Err(AppError::Database(e)),
        }
    }

    /// Check whether a column exists in a given table.
    /// Uses SQLite PRAGMA table_info for a reliable, portable check.
    fn column_exists(&self, table: &str, column: &str) -> Result<bool, AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
        let rows = stmt.query_map([], |row| {
            // PRAGMA table_info returns: cid, name, type, notnull, dflt_value, pk
            let name: String = row.get(1)?;
            Ok(name)
        })?;
        for name in rows {
            if name? == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // ========================================================================
    // Upstream CRUD
    // ========================================================================

    /// Create a new upstream record.
    pub fn create_upstream(
        &self,
        id: &str,
        provider_name: &str,
        base_url: &str,
        api_key_encrypted: &[u8],
        selected_model: &str,
        available_models: &str,
        enabled: bool,
        remark: &str,
    ) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "INSERT INTO upstreams (id, provider_name, base_url, api_key_encrypted, selected_model, available_models, enabled, remark)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, provider_name, base_url, api_key_encrypted, selected_model, available_models, enabled as i32, remark],
        )?;
        Ok(())
    }

    /// Get all upstreams ordered by creation time (newest first).
    pub fn get_upstreams(&self) -> Result<Vec<Upstream>, AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, provider_name, base_url, api_key_encrypted, selected_model,
                    available_models, enabled, remark, status, failure_count, last_failure_time,
                    last_success_time, last_error_reason, recovered_at,
                    created_at, updated_at
             FROM upstreams ORDER BY created_at DESC"
        )?;
        let rows = stmt.query_map([], Self::map_upstream_row)?;
        Self::collect_rows(rows)
    }

    /// Get a single upstream by its ID.
    pub fn get_upstream_by_id(&self, id: &str) -> Result<Option<Upstream>, AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, provider_name, base_url, api_key_encrypted, selected_model,
                    available_models, enabled, remark, status, failure_count, last_failure_time,
                    last_success_time, last_error_reason, recovered_at,
                    created_at, updated_at
             FROM upstreams WHERE id = ?1"
        )?;
        let result = stmt.query_row(params![id], Self::map_upstream_row);
        match result {
            Ok(u) => Ok(Some(u)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e)),
        }
    }

    /// Get multiple upstreams by a list of IDs.
    pub fn get_upstreams_by_ids(&self, ids: &[String]) -> Result<Vec<Upstream>, AppError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{}", i)).collect();
        let sql = format!(
            "SELECT id, provider_name, base_url, api_key_encrypted, selected_model,
                    available_models, enabled, remark, status, failure_count, last_failure_time,
                    last_success_time, last_error_reason, recovered_at,
                    created_at, updated_at
             FROM upstreams WHERE id IN ({})",
            placeholders.join(",")
        );
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), Self::map_upstream_row)?;
        Self::collect_rows(rows)
    }

    /// Update an existing upstream record.
    pub fn update_upstream(
        &self,
        id: &str,
        provider_name: &str,
        base_url: &str,
        api_key_encrypted: &[u8],
        selected_model: &str,
        available_models: &str,
        enabled: bool,
        remark: &str,
    ) -> Result<(), AppError> {
        let rows = self.get_conn()?.execute(
            "UPDATE upstreams SET provider_name=?1, base_url=?2, api_key_encrypted=?3,
             selected_model=?4, available_models=?5, enabled=?6, remark=?7, updated_at=datetime('now', 'localtime')
             WHERE id=?8",
            params![provider_name, base_url, api_key_encrypted, selected_model, available_models, enabled as i32, remark, id],
        )?;
        if rows == 0 {
            return Err(AppError::NotFound(format!("upstream {}", id)));
        }
        Ok(())
    }

    /// Delete an upstream by ID.
    pub fn delete_upstream(&self, id: &str) -> Result<(), AppError> {
        self.get_conn()?.execute("DELETE FROM upstreams WHERE id=?", params![id])?;
        Ok(())
    }

    /// Bulk-delete multiple upstreams in a single transaction.
    pub fn bulk_delete_upstreams(&self, ids: &[String]) -> Result<usize, AppError> {
        let mut total = 0usize;
        self.with_transaction(|conn| {
            for id in ids {
                let rows = conn.execute("DELETE FROM upstreams WHERE id=?", params![id])?;
                total += rows;
            }
            Ok(())
        })?;
        Ok(total)
    }

    /// Toggle an upstream's enabled state.
    pub fn toggle_upstream(&self, id: &str, enabled: bool) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "UPDATE upstreams SET enabled=?, updated_at=datetime('now', 'localtime') WHERE id=?",
            params![enabled as i32, id],
        )?;
        Ok(())
    }

    /// Update an upstream's health status and failure tracking.
    pub fn update_upstream_status(
        &self,
        id: &str,
        status: &str,
        failure_count: i32,
        last_failure_time: Option<String>,
    ) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "UPDATE upstreams SET status=?, failure_count=?, last_failure_time=?, updated_at=datetime('now', 'localtime')
             WHERE id=?",
            params![status, failure_count, last_failure_time, id],
        )?;
        Ok(())
    }

    // ========================================================================
    // Pool CRUD
    // ========================================================================

    /// Create a new pool record.
    pub fn create_pool(
        &self,
        id: &str,
        name: &str,
        display_name: &str,
        max_concurrency: i32,
        thinking_enabled: bool,
    ) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "INSERT INTO pools (id, name, display_name, max_concurrency, thinking_enabled)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, name, display_name, max_concurrency, thinking_enabled as i32],
        )?;
        Ok(())
    }

    /// Get all pools ordered by creation time (newest first).
    pub fn get_pools(&self) -> Result<Vec<Pool>, AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, display_name, round_robin_strategy, failover_enabled,
                    timeout_seconds, max_concurrency, thinking_enabled,
                    created_at, updated_at
             FROM pools ORDER BY created_at DESC"
        )?;
        let rows = stmt.query_map([], Self::map_pool_row)?;
        Self::collect_rows(rows)
    }

    /// Get a single pool by its ID.
    pub fn get_pool_by_id(&self, id: &str) -> Result<Option<Pool>, AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, display_name, round_robin_strategy, failover_enabled,
                    timeout_seconds, max_concurrency, thinking_enabled,
                    created_at, updated_at
             FROM pools WHERE id = ?1"
        )?;
        let result = stmt.query_row(params![id], Self::map_pool_row);
        match result {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e)),
        }
    }

    /// Get a pool by its unique model name.
    pub fn get_pool_by_name(&self, name: &str) -> Result<Option<Pool>, AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, display_name, round_robin_strategy, failover_enabled,
                    timeout_seconds, max_concurrency, thinking_enabled,
                    created_at, updated_at
             FROM pools WHERE name = ?1"
        )?;
        let result = stmt.query_row(params![name], Self::map_pool_row);
        match result {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e)),
        }
    }

    /// Update pool configuration.
    pub fn update_pool(
        &self,
        id: &str,
        display_name: &str,
        max_concurrency: i32,
        thinking_enabled: bool,
    ) -> Result<(), AppError> {
        let rows = self.get_conn()?.execute(
            "UPDATE pools SET display_name=?1, max_concurrency=?2, thinking_enabled=?3,
             updated_at=datetime('now', 'localtime') WHERE id=?4",
            params![
                display_name, max_concurrency, thinking_enabled as i32, id
            ],
        )?;
        if rows == 0 {
            return Err(AppError::NotFound(format!("pool {}", id)));
        }
        Ok(())
    }

    /// Delete a pool by ID (cascade removes pool_upstreams associations).
    pub fn delete_pool(&self, id: &str) -> Result<(), AppError> {
        self.get_conn()?.execute("DELETE FROM pools WHERE id=?", params![id])?;
        Ok(())
    }

    // ========================================================================
    // Pool-Upstream Association
    // ========================================================================

    /// Associate an upstream with a pool at the given sort order, specifying which model to use.
    pub fn add_upstream_to_pool(
        &self,
        pool_id: &str,
        upstream_id: &str,
        sort_order: i32,
        model: &str,
    ) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "INSERT OR IGNORE INTO pool_upstreams (pool_id, upstream_id, sort_order, model)
             VALUES (?1, ?2, ?3, ?4)",
            params![pool_id, upstream_id, sort_order, model],
        )?;
        Ok(())
    }

    /// Remove an upstream from a pool.
    pub fn remove_upstream_from_pool(
        &self,
        pool_id: &str,
        upstream_id: &str,
    ) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "DELETE FROM pool_upstreams WHERE pool_id=?1 AND upstream_id=?2",
            params![pool_id, upstream_id],
        )?;
        Ok(())
    }

    /// Get all upstreams for a pool, ordered by sort_order.
    pub fn get_pool_upstreams(&self, pool_id: &str) -> Result<Vec<PoolUpstreamInfo>, AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT u.id, u.provider_name, pu.model, pu.sort_order
             FROM pool_upstreams pu
             JOIN upstreams u ON u.id = pu.upstream_id
             WHERE pu.pool_id=?1
             ORDER BY pu.sort_order ASC"
        )?;
        let rows = stmt.query_map(params![pool_id], |row| {
            Ok(PoolUpstreamInfo {
                upstream_id: row.get(0)?,
                provider_name: row.get(1)?,
                model: row.get(2)?,
                sort_order: row.get(3)?,
            })
        })?;
        Self::collect_rows(rows)
    }

    /// Reorder upstreams in a pool by providing the full desired order.
    pub fn reorder_pool_upstreams(
        &self,
        pool_id: &str,
        ordered_upstream_ids: &[String],
    ) -> Result<(), AppError> {
        self.with_transaction(|conn| {
            for (idx, uid) in ordered_upstream_ids.iter().enumerate() {
                conn.execute(
                    "UPDATE pool_upstreams SET sort_order=?1 WHERE pool_id=?2 AND upstream_id=?3",
                    params![idx as i32, pool_id, uid],
                )?;
            }
            Ok(())
        })
    }

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

    // ========================================================================
    // Request Logs
    // ========================================================================

    /// Insert a request log entry.
    /// Automatically triggers periodic cleanup every 100 inserts to prevent
    /// unbounded database growth.
pub fn insert_request_log(
&self,
id: &str,
request_id: &str,
pool_name: Option<&str>,
upstream_id: Option<&str>,
model: Option<&str>,
failed_upstreams: &str,
method: &str,
endpoint: &str,
status_code: i32,
response_time_ms: i32,
is_streaming: bool,
prompt_tokens: i64,
completion_tokens: i64,
total_tokens: i64,
) -> Result<(), AppError> {
self.get_conn()?.execute(
"INSERT INTO request_logs (id, request_id, pool_name, upstream_id, model, failed_upstreams,
method, endpoint, status_code, response_time_ms, is_streaming, prompt_tokens, completion_tokens, total_tokens)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
params![
id, request_id, pool_name, upstream_id, model, failed_upstreams,
method, endpoint, status_code, response_time_ms, is_streaming as i32,
prompt_tokens, completion_tokens, total_tokens
],
)?;

        // Periodic cleanup: every 100 inserts, remove old logs
        let count = self.log_insert_counter.fetch_add(1, Ordering::Relaxed) + 1;
        if count % 100 == 0 {
            if let Err(e) = self.cleanup_old_logs(5, 200) {
                warn!("Periodic log cleanup failed: {}", e);
            }
        }

        Ok(())
}

    /// Update token usage for an existing request log entry.
    /// Used for streaming requests where usage is only known after the stream completes.
    pub fn update_request_log_tokens(
        &self,
        log_id: &str,
        prompt_tokens: i64,
        completion_tokens: i64,
        total_tokens: i64,
    ) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "UPDATE request_logs SET prompt_tokens=?1, completion_tokens=?2, total_tokens=?3 WHERE id=?4",
            params![prompt_tokens, completion_tokens, total_tokens, log_id],
        )?;
        Ok(())
    }

    /// Update the status code of an existing request log entry.
    /// Used for streaming requests where the error is only detected mid-stream.
    pub fn update_request_log_status(
        &self,
        log_id: &str,
        status_code: i32,
    ) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "UPDATE request_logs SET status_code=?1 WHERE id=?2",
            params![status_code, log_id],
        )?;
        Ok(())
    }

    /// Get recent logs with optional time-range filter, pagination.
    pub fn get_recent_logs(&self, filter: &LogFilter) -> Result<Vec<RequestLogEntry>, AppError> {
        let mut conditions = Vec::new();
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(ref start) = filter.start_date {
            param_values.push(Box::new(start.clone()));
            conditions.push(format!("created_at >= ?{}", param_values.len()));
        }
        if let Some(ref end) = filter.end_date {
            param_values.push(Box::new(end.clone()));
            conditions.push(format!("created_at <= ?{}", param_values.len()));
        }
        if let Some(ref pool) = filter.pool_name {
            param_values.push(Box::new(pool.clone()));
            conditions.push(format!("pool_name = ?{}", param_values.len()));
        }
        if let Some(prefix) = filter.status_prefix {
            param_values.push(Box::new(prefix * 100));
            let lower_idx = param_values.len();
            param_values.push(Box::new((prefix + 1) * 100));
            let upper_idx = param_values.len();
            conditions.push(format!("status_code >= ?{} AND status_code < ?{}", lower_idx, upper_idx));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        param_values.push(Box::new(filter.limit));
        param_values.push(Box::new(filter.offset));

        let sql = format!(
            "SELECT id, request_id, pool_name, upstream_id, model, failed_upstreams,
                    method, endpoint, status_code, response_time_ms, is_streaming,
                    prompt_tokens, completion_tokens, total_tokens, created_at
             FROM request_logs {} ORDER BY created_at DESC LIMIT ?{} OFFSET ?{}",
            where_clause,
            param_values.len() - 1,
            param_values.len()
        );

        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = param_values
            .iter()
            .map(|p| p.as_ref() as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), Self::map_log_row)?;
        Self::collect_rows(rows)
    }

    /// Clear all request logs. Returns the number of deleted rows.
    pub fn clear_logs(&self) -> Result<i64, AppError> {
        let rows = self.get_conn()?.execute("DELETE FROM request_logs", [])?;
        Ok(rows as i64)
    }

    /// Automatically clean up old request logs to prevent unbounded database growth.
    /// Deletes logs older than `max_age_days` and caps total entries to `max_count`.
    /// Returns the total number of deleted rows.
    pub fn cleanup_old_logs(&self, max_age_days: i32, max_count: i64) -> Result<i64, AppError> {
        let conn = self.get_conn()?;
        let mut total_deleted: i64 = 0;

        // 1. Delete logs older than max_age_days
        let deleted_by_age = conn.execute(
            "DELETE FROM request_logs WHERE created_at < datetime('now', ?1)",
            params![format!("-{} days", max_age_days)],
        )? as i64;
        total_deleted += deleted_by_age;

        // 2. If still over max_count, delete the oldest excess entries
        let remaining: i64 = conn.query_row(
            "SELECT COUNT(*) FROM request_logs", [], |row| row.get(0)
        )?;
        if remaining > max_count {
            let excess = remaining - max_count;
            let deleted_by_count = conn.execute(
                "DELETE FROM request_logs WHERE id IN (
                    SELECT id FROM request_logs ORDER BY created_at ASC LIMIT ?1
                )",
                params![excess],
            )? as i64;
            total_deleted += deleted_by_count;
        }

        if total_deleted > 0 {
            debug!("Log cleanup: removed {} entries ({} by age, remaining {})",
                   total_deleted, deleted_by_age, remaining.saturating_sub(total_deleted - deleted_by_age));
        }

        Ok(total_deleted)
    }

    /// Delete all request logs for a specific upstream (reset its token stats).
    pub fn reset_upstream_token_stats(&self, upstream_id: &str) -> Result<i64, AppError> {
        let rows = self.get_conn()?.execute(
            "DELETE FROM request_logs WHERE upstream_id = ?1",
            params![upstream_id],
        )?;
        Ok(rows as i64)
    }

    /// Get daily token usage for an upstream over the last N days.
    pub fn get_upstream_token_stats(&self, upstream_id: &str, days: i32) -> Result<Vec<DailyTokenUsage>, AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT date(created_at) as day,
                    COALESCE(SUM(prompt_tokens), 0) as prompt_tokens,
                    COALESCE(SUM(completion_tokens), 0) as completion_tokens,
                    COALESCE(SUM(total_tokens), 0) as total_tokens,
                    COUNT(*) as request_count
             FROM request_logs
             WHERE upstream_id = ?1
               AND created_at >= datetime('now', ?2)
             GROUP BY date(created_at)
             ORDER BY day ASC",
        )?;
        let offset = format!("-{} days", days);
        let rows = stmt.query_map(params![upstream_id, offset], |row| {
            Ok(DailyTokenUsage {
                date: row.get(0)?,
                prompt_tokens: row.get(1)?,
                completion_tokens: row.get(2)?,
                total_tokens: row.get(3)?,
                request_count: row.get(4)?,
            })
        })?;
        Self::collect_rows(rows)
    }

    /// Get today's token totals for an upstream.
    pub fn get_upstream_today_tokens(&self, upstream_id: &str) -> Result<TokenTotals, AppError> {
        let conn = self.get_conn()?;
        let result = conn.query_row(
            "SELECT COALESCE(SUM(prompt_tokens), 0),
                    COALESCE(SUM(completion_tokens), 0),
                    COALESCE(SUM(total_tokens), 0)
             FROM request_logs
             WHERE upstream_id = ?1
               AND date(created_at) = date('now', 'localtime')",
            params![upstream_id],
            |row| Ok(TokenTotals {
                prompt_tokens: row.get(0)?,
                completion_tokens: row.get(1)?,
                total_tokens: row.get(2)?,
            }),
        )?;
        Ok(result)
    }

    /// Get all-time token totals for an upstream.
    pub fn get_upstream_total_tokens(&self, upstream_id: &str) -> Result<TokenTotals, AppError> {
        let conn = self.get_conn()?;
        let result = conn.query_row(
            "SELECT COALESCE(SUM(prompt_tokens), 0),
                    COALESCE(SUM(completion_tokens), 0),
                    COALESCE(SUM(total_tokens), 0)
             FROM request_logs
             WHERE upstream_id = ?1",
            params![upstream_id],
            |row| Ok(TokenTotals {
                prompt_tokens: row.get(0)?,
                completion_tokens: row.get(1)?,
                total_tokens: row.get(2)?,
            }),
        )?;
        Ok(result)
    }

    /// Get per-model token usage for an upstream (today + total + request count).
    pub fn get_upstream_model_token_stats(&self, upstream_id: &str) -> Result<Vec<ModelTokenUsage>, AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT COALESCE(NULLIF(model, ''), '未记录') as model,
                    COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN total_tokens ELSE 0 END), 0) as today_tokens,
                    COALESCE(SUM(total_tokens), 0) as total_tokens,
                    COUNT(*) as request_count
             FROM request_logs
             WHERE upstream_id = ?1
             GROUP BY COALESCE(NULLIF(model, ''), '未记录')
             ORDER BY total_tokens DESC",
        )?;
        let rows = stmt.query_map(params![upstream_id], |row| {
            Ok(ModelTokenUsage {
                model: row.get(0)?,
                today_tokens: row.get(1)?,
                total_tokens: row.get(2)?,
                request_count: row.get(3)?,
            })
        })?;
        Self::collect_rows(rows)
    }

    /// Get daily token usage for an upstream, optionally filtered by model, over the last N days.
    pub fn get_upstream_token_stats_filtered(
        &self,
        upstream_id: &str,
        model: Option<&str>,
        days: i32,
    ) -> Result<Vec<DailyTokenUsage>, AppError> {
        let conn = self.get_conn()?;
        let offset = format!("-{} days", days);

        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match model {
            Some(m) => (
                "SELECT date(created_at) as day,
                        COALESCE(SUM(prompt_tokens), 0) as prompt_tokens,
                        COALESCE(SUM(completion_tokens), 0) as completion_tokens,
                        COALESCE(SUM(total_tokens), 0) as total_tokens,
                        COUNT(*) as request_count
                 FROM request_logs
                 WHERE upstream_id = ?1
                   AND model = ?2
                   AND created_at >= datetime('now', ?3)
                 GROUP BY date(created_at)
                 ORDER BY day ASC"
                    .to_string(),
                vec![
                    Box::new(upstream_id.to_string()),
                    Box::new(m.to_string()),
                    Box::new(offset),
                ],
            ),
            None => (
                "SELECT date(created_at) as day,
                        COALESCE(SUM(prompt_tokens), 0) as prompt_tokens,
                        COALESCE(SUM(completion_tokens), 0) as completion_tokens,
                        COALESCE(SUM(total_tokens), 0) as total_tokens,
                        COUNT(*) as request_count
                 FROM request_logs
                 WHERE upstream_id = ?1
                   AND created_at >= datetime('now', ?2)
                 GROUP BY date(created_at)
                 ORDER BY day ASC"
                    .to_string(),
                vec![
                    Box::new(upstream_id.to_string()),
                    Box::new(offset),
                ],
            ),
        };

        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok(DailyTokenUsage {
                date: row.get(0)?,
                prompt_tokens: row.get(1)?,
                completion_tokens: row.get(2)?,
                total_tokens: row.get(3)?,
                request_count: row.get(4)?,
            })
        })?;
        Self::collect_rows(rows)
    }

    /// Get hourly token usage for today, optionally filtered by model.
    /// Returns 24 rows (one per hour, 00–23).
    pub fn get_upstream_hourly_stats(
        &self,
        upstream_id: &str,
        model: Option<&str>,
    ) -> Result<Vec<HourlyTokenUsage>, AppError> {
        let conn = self.get_conn()?;

        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match model {
            Some(m) => (
                "SELECT strftime('%H', created_at, 'localtime') as hour,
                        COALESCE(SUM(prompt_tokens), 0) as prompt_tokens,
                        COALESCE(SUM(completion_tokens), 0) as completion_tokens,
                        COALESCE(SUM(total_tokens), 0) as total_tokens,
                        COUNT(*) as request_count
                 FROM request_logs
                 WHERE upstream_id = ?1
                   AND model = ?2
                   AND date(created_at) = date('now', 'localtime')
                 GROUP BY hour
                 ORDER BY hour ASC"
                    .to_string(),
                vec![
                    Box::new(upstream_id.to_string()),
                    Box::new(m.to_string()),
                ],
            ),
            None => (
                "SELECT strftime('%H', created_at, 'localtime') as hour,
                        COALESCE(SUM(prompt_tokens), 0) as prompt_tokens,
                        COALESCE(SUM(completion_tokens), 0) as completion_tokens,
                        COALESCE(SUM(total_tokens), 0) as total_tokens,
                        COUNT(*) as request_count
                 FROM request_logs
                 WHERE upstream_id = ?1
                   AND date(created_at) = date('now', 'localtime')
                 GROUP BY hour
                 ORDER BY hour ASC"
                    .to_string(),
                vec![Box::new(upstream_id.to_string())],
            ),
        };

        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let db_rows: std::collections::HashMap<String, HourlyTokenUsage> = stmt
            .query_map(params.as_slice(), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    HourlyTokenUsage {
                        hour: row.get(0)?,
                        prompt_tokens: row.get(1)?,
                        completion_tokens: row.get(2)?,
                        total_tokens: row.get(3)?,
                        request_count: row.get(4)?,
                    },
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();

        // Fill all 24 hours
        let mut result = Vec::with_capacity(24);
        for h in 0..24 {
            let key = format!("{:02}", h);
            let entry = db_rows.get(&key).cloned().unwrap_or(HourlyTokenUsage {
                hour: key.clone(),
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
                request_count: 0,
            });
            result.push(entry);
        }
        Ok(result)
    }

    /// Get today's token totals for an upstream, optionally filtered by model.
    pub fn get_upstream_today_tokens_filtered(
        &self,
        upstream_id: &str,
        model: Option<&str>,
    ) -> Result<TokenTotals, AppError> {
        let conn = self.get_conn()?;
        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match model {
            Some(m) => (
                "SELECT COALESCE(SUM(prompt_tokens), 0),
                        COALESCE(SUM(completion_tokens), 0),
                        COALESCE(SUM(total_tokens), 0)
                 FROM request_logs
                 WHERE upstream_id = ?1 AND model = ?2
                   AND date(created_at) = date('now', 'localtime')"
                    .to_string(),
                vec![
                    Box::new(upstream_id.to_string()),
                    Box::new(m.to_string()),
                ],
            ),
            None => (
                "SELECT COALESCE(SUM(prompt_tokens), 0),
                        COALESCE(SUM(completion_tokens), 0),
                        COALESCE(SUM(total_tokens), 0)
                 FROM request_logs
                 WHERE upstream_id = ?1
                   AND date(created_at) = date('now', 'localtime')"
                    .to_string(),
                vec![Box::new(upstream_id.to_string())],
            ),
        };
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let result = stmt.query_row(params.as_slice(), |row| {
            Ok(TokenTotals {
                prompt_tokens: row.get(0)?,
                completion_tokens: row.get(1)?,
                total_tokens: row.get(2)?,
            })
        })?;
        Ok(result)
    }

    /// Get all-time token totals for an upstream, optionally filtered by model.
    pub fn get_upstream_total_tokens_filtered(
        &self,
        upstream_id: &str,
        model: Option<&str>,
    ) -> Result<TokenTotals, AppError> {
        let conn = self.get_conn()?;
        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match model {
            Some(m) => (
                "SELECT COALESCE(SUM(prompt_tokens), 0),
                        COALESCE(SUM(completion_tokens), 0),
                        COALESCE(SUM(total_tokens), 0)
                 FROM request_logs
                 WHERE upstream_id = ?1 AND model = ?2"
                    .to_string(),
                vec![
                    Box::new(upstream_id.to_string()),
                    Box::new(m.to_string()),
                ],
            ),
            None => (
                "SELECT COALESCE(SUM(prompt_tokens), 0),
                        COALESCE(SUM(completion_tokens), 0),
                        COALESCE(SUM(total_tokens), 0)
                 FROM request_logs
                 WHERE upstream_id = ?1"
                    .to_string(),
                vec![Box::new(upstream_id.to_string())],
            ),
        };
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let result = stmt.query_row(params.as_slice(), |row| {
            Ok(TokenTotals {
                prompt_tokens: row.get(0)?,
                completion_tokens: row.get(1)?,
                total_tokens: row.get(2)?,
            })
        })?;
        Ok(result)
    }

    // ========================================================================
    // Query Helpers
    // ========================================================================

    /// Check if an upstream exists by ID.
    pub fn upstream_exists(&self, id: &str) -> Result<bool, AppError> {
        let count: i64 = self.get_conn()?.query_row(
            "SELECT COUNT(*) FROM upstreams WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Check if a pool exists by ID.
    pub fn pool_exists(&self, id: &str) -> Result<bool, AppError> {
        let count: i64 = self.get_conn()?.query_row(
            "SELECT COUNT(*) FROM pools WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Count total upstreams.
    pub fn count_upstreams(&self) -> Result<i64, AppError> {
        let count: i64 = self.get_conn()?.query_row(
            "SELECT COUNT(*) FROM upstreams", [], |row| row.get(0)
        )?;
        Ok(count)
    }

    /// Count total pools.
    pub fn count_pools(&self) -> Result<i64, AppError> {
        let count: i64 = self.get_conn()?.query_row(
            "SELECT COUNT(*) FROM pools", [], |row| row.get(0)
        )?;
        Ok(count)
    }

    /// Count active (enabled) upstreams.
    pub fn count_active_upstreams(&self) -> Result<i64, AppError> {
        let count: i64 = self.get_conn()?.query_row(
            "SELECT COUNT(*) FROM upstreams WHERE enabled = 1", [], |row| row.get(0)
        )?;
        Ok(count)
    }

    /// Get status summary for all upstreams.
    pub fn get_upstream_status_summary(&self) -> Result<Vec<UpstreamStatusSummary>, AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, provider_name, status, failure_count, last_failure_time,
                    last_success_time, last_error_reason, recovered_at
             FROM upstreams ORDER BY provider_name"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(UpstreamStatusSummary {
                id: row.get(0)?,
                provider_name: row.get(1)?,
                status: row.get(2)?,
                failure_count: row.get(3)?,
                last_failure_time: row.get(4)?,
                last_success_time: row.get(5)?,
                last_error_reason: row.get(6)?,
                recovered_at: row.get(7)?,
            })
        })?;
        Self::collect_rows(rows)
    }

    /// Update upstream health status after a successful or failed request.
    ///
    /// On success: resets failure_count to 0, sets status to 'healthy',
    /// updates last_success_time, and sets recovered_at if transitioning from down/degraded.
    ///
    /// On failure: increments failure_count, sets last_failure_time and last_error_reason,
    /// and updates status to 'degraded' or 'down' based on the failure threshold.
    pub fn update_upstream_health(
        &self,
        upstream_id: &str,
        success: bool,
        error_reason: Option<&str>,
        failure_threshold: i32,
    ) -> Result<(), AppError> {
        if success {
            // Check if transitioning from down/degraded to healthy
            let prev_status: Option<String> = self.get_conn()?.query_row(
                "SELECT status FROM upstreams WHERE id = ?1",
                params![upstream_id],
                |row| row.get(0),
            ).ok();

            let should_set_recovered = matches!(prev_status.as_deref(), Some("down") | Some("degraded"));

            if should_set_recovered {
                self.get_conn()?.execute(
                    "UPDATE upstreams SET
                        status = 'healthy',
                        failure_count = 0,
                        last_success_time = datetime('now', 'localtime'),
                        last_error_reason = NULL,
                        recovered_at = datetime('now', 'localtime'),
                        updated_at = datetime('now', 'localtime')
                     WHERE id = ?1",
                    params![upstream_id],
                )?;
            } else {
                self.get_conn()?.execute(
                    "UPDATE upstreams SET
                        status = 'healthy',
                        failure_count = 0,
                        last_success_time = datetime('now', 'localtime'),
                        last_error_reason = NULL,
                        updated_at = datetime('now', 'localtime')
                     WHERE id = ?1",
                    params![upstream_id],
                )?;
            }
        } else {
            // Increment failure count and determine new status based on threshold
            if failure_threshold > 0 {
                self.get_conn()?.execute(
                    "UPDATE upstreams SET
                        failure_count = failure_count + 1,
                        last_failure_time = datetime('now', 'localtime'),
                        last_error_reason = ?2,
                        status = CASE
                            WHEN failure_count + 1 >= ?3 THEN 'down'
                            ELSE 'degraded'
                        END,
                        updated_at = datetime('now', 'localtime')
                     WHERE id = ?1",
                    params![upstream_id, error_reason, failure_threshold],
                )?;
            } else {
                self.get_conn()?.execute(
                    "UPDATE upstreams SET
                        failure_count = failure_count + 1,
                        last_failure_time = datetime('now', 'localtime'),
                        last_error_reason = ?2,
                        status = 'degraded',
                        updated_at = datetime('now', 'localtime')
                     WHERE id = ?1",
                    params![upstream_id, error_reason],
                )?;
            }
        }
        Ok(())
    }

    /// Get aggregate statistics.
    pub fn get_stats(&self) -> Result<Stats, AppError> {
        let upstream_count = self.count_upstreams()?;
        let active_upstream_count = self.count_active_upstreams()?;
        let pool_count = self.count_pools()?;

        let today_request_count: i64 = self.get_conn()?.query_row(
            "SELECT COUNT(*) FROM request_logs WHERE date(created_at) = date('now', 'localtime')",
            [], |row| row.get(0),
        )?;
        let today_success_count: i64 = self.get_conn()?.query_row(
            "SELECT COUNT(*) FROM request_logs WHERE date(created_at) = date('now', 'localtime') AND status_code >= 200 AND status_code < 300",
            [], |row| row.get(0),
        )?;
        let today_error_count: i64 = self.get_conn()?.query_row(
            "SELECT COUNT(*) FROM request_logs WHERE date(created_at) = date('now', 'localtime') AND status_code >= 400",
            [], |row| row.get(0),
        )?;

        Ok(Stats {
            upstream_count,
            active_upstream_count,
            pool_count,
            today_request_count,
            today_success_count,
            today_error_count,
        })
    }

    // ========================================================================
    // Transaction Support
    // ========================================================================

    /// Execute a closure within a database transaction.
    /// The connection lock is held for the entire transaction duration to
    /// prevent other threads from interleaving statements within the transaction.
    /// If the closure returns an error, the transaction is rolled back.
    pub fn with_transaction<F>(&self, f: F) -> Result<(), AppError>
    where
        F: FnOnce(&Connection) -> Result<(), AppError>,
    {
        let conn = self.get_conn()?;
        conn.execute_batch("BEGIN TRANSACTION")?;
        match f(&conn) {
            Ok(()) => {
                conn.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(e) => {
                conn.execute_batch("ROLLBACK")?;
                Err(e)
            }
        }
    }

    // ========================================================================
    // Private Row Mappers
    // ========================================================================

    fn map_upstream_row(row: &rusqlite::Row) -> rusqlite::Result<Upstream> {
        Ok(Upstream {
            id: row.get(0)?,
            provider_name: row.get(1)?,
            base_url: row.get(2)?,
            api_key_encrypted: row.get(3)?,
            selected_model: row.get(4)?,
            available_models: row.get(5)?,
            enabled: row.get::<_, i32>(6)? != 0,
            remark: row.get(7)?,
            status: row.get(8)?,
            failure_count: row.get(9)?,
            last_failure_time: row.get(10)?,
            last_success_time: row.get(11)?,
            last_error_reason: row.get(12)?,
            recovered_at: row.get(13)?,
            created_at: row.get(14)?,
            updated_at: row.get(15)?,
        })
    }

    fn map_pool_row(row: &rusqlite::Row) -> rusqlite::Result<Pool> {
        Ok(Pool {
            id: row.get(0)?,
            name: row.get(1)?,
            display_name: row.get(2)?,
            round_robin_strategy: row.get(3)?,
            failover_enabled: row.get::<_, i32>(4)? != 0,
            timeout_seconds: row.get(5)?,
            max_concurrency: row.get(6)?,
            thinking_enabled: row.get::<_, i32>(7)? != 0,
            created_at: row.get(8)?,
            updated_at: row.get(9)?,
        })
    }

fn map_log_row(row: &rusqlite::Row) -> rusqlite::Result<RequestLogEntry> {
Ok(RequestLogEntry {
id: row.get(0)?,
request_id: row.get(1)?,
pool_name: row.get(2)?,
upstream_id: row.get(3)?,
model: row.get(4)?,
failed_upstreams: row.get(5)?,
method: row.get(6)?,
endpoint: row.get(7)?,
status_code: row.get(8)?,
response_time_ms: row.get(9)?,
is_streaming: row.get::<_, i32>(10)? != 0,
prompt_tokens: row.get(11)?,
completion_tokens: row.get(12)?,
total_tokens: row.get(13)?,
created_at: row.get(14)?,
})
}

    fn collect_rows<T>(rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>) -> Result<Vec<T>, AppError> {
        let mut result = Vec::new();
        for r in rows {
            result.push(r?);
        }
        Ok(result)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    /// Helper: create an in-memory test database with schema initialized.
    fn test_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db
    }

    fn sample_upstream_id() -> String {
        Uuid::new_v4().to_string()
    }

    fn sample_pool_id() -> String {
        Uuid::new_v4().to_string()
    }

    // ---- Schema & Migration Tests ----

    #[test]
    fn test_initialize_creates_all_tables() {
        let db = test_db();
        // Verify tables exist by querying them
        let upstreams = db.get_upstreams().unwrap();
        assert!(upstreams.is_empty());
        let pools = db.get_pools().unwrap();
        assert!(pools.is_empty());
    }

    #[test]
    fn test_schema_version_starts_at_zero() {
        let db = test_db();
        // After initialize with empty migration SQL, version should be 1
        // (migration v1 has empty SQL, so it just bumps the version)
        let version = db.get_schema_version().unwrap();
        assert!(version >= 1);
    }

    #[test]
    fn test_initialize_is_idempotent() {
        let db = test_db();
        // Calling initialize again should not fail
        db.initialize().unwrap();
        db.initialize().unwrap();
    }

    // ---- Upstream CRUD Tests ----

    #[test]
    fn test_create_and_get_upstream() {
        let db = test_db();
        let id = sample_upstream_id();
        let encrypted_key = b"encrypted_key_bytes";

        db.create_upstream(&id, "OpenAI", "http://api.openai.com", encrypted_key, "gpt-4", "[]", true, "test remark").unwrap();

        let upstreams = db.get_upstreams().unwrap();
        assert_eq!(upstreams.len(), 1);
        assert_eq!(upstreams[0].id, id);
        assert_eq!(upstreams[0].provider_name, "OpenAI");
        assert_eq!(upstreams[0].base_url, "http://api.openai.com");
        assert_eq!(upstreams[0].api_key_encrypted, encrypted_key.to_vec());
        assert_eq!(upstreams[0].selected_model, "gpt-4");
        assert!(upstreams[0].enabled);
        assert_eq!(upstreams[0].remark, "test remark");
        assert_eq!(upstreams[0].status, "healthy");
        assert_eq!(upstreams[0].failure_count, 0);
    }

    #[test]
    fn test_get_upstream_by_id() {
        let db = test_db();
        let id = sample_upstream_id();
        db.create_upstream(&id, "DeepSeek", "http://api.deepseek.com", b"key", "deepseek-v3", "[]", true, "").unwrap();

        let found = db.get_upstream_by_id(&id).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().provider_name, "DeepSeek");

        let not_found = db.get_upstream_by_id("nonexistent").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_update_upstream() {
        let db = test_db();
        let id = sample_upstream_id();
        db.create_upstream(&id, "OpenAI", "http://old.url", b"old_key", "gpt-3.5", "[]", true, "").unwrap();

        db.update_upstream(&id, "OpenAI-v2", "http://new.url", b"new_key", "gpt-4", "[]", false, "updated").unwrap();

        let u = db.get_upstream_by_id(&id).unwrap().unwrap();
        assert_eq!(u.provider_name, "OpenAI-v2");
        assert_eq!(u.base_url, "http://new.url");
        assert_eq!(u.api_key_encrypted, b"new_key".to_vec());
        assert_eq!(u.selected_model, "gpt-4");
        assert!(!u.enabled);
        assert_eq!(u.remark, "updated");
    }

    #[test]
    fn test_update_nonexistent_upstream_returns_not_found() {
        let db = test_db();
        let result = db.update_upstream("no-such-id", "X", "http://x", b"k", "m", "[]", true, "");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AppError::NotFound(_)));
    }

    #[test]
    fn test_delete_upstream() {
        let db = test_db();
        let id = sample_upstream_id();
        db.create_upstream(&id, "Test", "http://test", b"k", "m", "[]", true, "").unwrap();
        assert_eq!(db.count_upstreams().unwrap(), 1);

        db.delete_upstream(&id).unwrap();
        assert_eq!(db.count_upstreams().unwrap(), 0);
    }

    #[test]
    fn test_toggle_upstream() {
        let db = test_db();
        let id = sample_upstream_id();
        db.create_upstream(&id, "Test", "http://test", b"k", "m", "[]", true, "").unwrap();

        db.toggle_upstream(&id, false).unwrap();
        let u = db.get_upstream_by_id(&id).unwrap().unwrap();
        assert!(!u.enabled);

        db.toggle_upstream(&id, true).unwrap();
        let u = db.get_upstream_by_id(&id).unwrap().unwrap();
        assert!(u.enabled);
    }

    #[test]
    fn test_update_upstream_status() {
        let db = test_db();
        let id = sample_upstream_id();
        db.create_upstream(&id, "Test", "http://test", b"k", "m", "[]", true, "").unwrap();

        db.update_upstream_status(&id, "degraded", 5, Some("2026-07-25T10:00:00".to_string())).unwrap();
        let u = db.get_upstream_by_id(&id).unwrap().unwrap();
        assert_eq!(u.status, "degraded");
        assert_eq!(u.failure_count, 5);
        assert_eq!(u.last_failure_time, Some("2026-07-25T10:00:00".to_string()));
    }

    #[test]
    fn test_get_upstreams_by_ids() {
        let db = test_db();
        let id1 = sample_upstream_id();
        let id2 = sample_upstream_id();
        let id3 = sample_upstream_id();
        db.create_upstream(&id1, "A", "http://a", b"k", "m", "[]", true, "").unwrap();
        db.create_upstream(&id2, "B", "http://b", b"k", "m", "[]", true, "").unwrap();
        db.create_upstream(&id3, "C", "http://c", b"k", "m", "[]", true, "").unwrap();

        let result = db.get_upstreams_by_ids(&[id1.clone(), id3.clone()]).unwrap();
        assert_eq!(result.len(), 2);
        let names: Vec<&str> = result.iter().map(|u| u.provider_name.as_str()).collect();
        assert!(names.contains(&"A"));
        assert!(names.contains(&"C"));

        // Empty input returns empty output
        let empty = db.get_upstreams_by_ids(&[]).unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn test_bulk_delete_upstreams() {
        let db = test_db();
        let ids: Vec<String> = (0..5).map(|_| sample_upstream_id()).collect();
        for (i, id) in ids.iter().enumerate() {
            db.create_upstream(id, &format!("Up{}", i), "http://test", b"k", "m", "[]", true, "").unwrap();
        }
        assert_eq!(db.count_upstreams().unwrap(), 5);

        let deleted = db.bulk_delete_upstreams(&ids[0..3]).unwrap();
        assert_eq!(deleted, 3);
        assert_eq!(db.count_upstreams().unwrap(), 2);
    }

    // ---- Pool CRUD Tests ----

    #[test]
    fn test_create_and_get_pool() {
        let db = test_db();
        let id = sample_pool_id();
        db.create_pool(&id, "grok-4.5", "Grok 4.5", 10, false).unwrap();

        let pools = db.get_pools().unwrap();
        assert_eq!(pools.len(), 1);
        assert_eq!(pools[0].id, id);
        assert_eq!(pools[0].name, "grok-4.5");
        assert_eq!(pools[0].display_name, "Grok 4.5");
        assert_eq!(pools[0].max_concurrency, 10);
        assert!(!pools[0].thinking_enabled);
    }

    #[test]
    fn test_get_pool_by_id() {
        let db = test_db();
        let id = sample_pool_id();
        db.create_pool(&id, "test-pool", "Test Pool", 5, true).unwrap();

        let found = db.get_pool_by_id(&id).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "test-pool");

        let not_found = db.get_pool_by_id("nonexistent").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_get_pool_by_name() {
        let db = test_db();
        let id = sample_pool_id();
        db.create_pool(&id, "unique-model", "Unique Model", 5, false).unwrap();

        let found = db.get_pool_by_name("unique-model").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, id);

        let not_found = db.get_pool_by_name("no-such-model").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_update_pool() {
        let db = test_db();
        let id = sample_pool_id();
        db.create_pool(&id, "my-pool", "My Pool", 5, false).unwrap();

        db.update_pool(&id, "Updated Pool", 20, true).unwrap();
        let p = db.get_pool_by_id(&id).unwrap().unwrap();
        assert_eq!(p.display_name, "Updated Pool");
        assert_eq!(p.max_concurrency, 20);
        assert!(p.thinking_enabled);
    }

    #[test]
    fn test_delete_pool() {
        let db = test_db();
        let id = sample_pool_id();
        db.create_pool(&id, "temp-pool", "Temp", 5, false).unwrap();
        assert_eq!(db.count_pools().unwrap(), 1);

        db.delete_pool(&id).unwrap();
        assert_eq!(db.count_pools().unwrap(), 0);
    }

    // ---- Pool-Upstream Association Tests ----

    #[test]
    fn test_add_and_get_pool_upstreams() {
        let db = test_db();
        let pool_id = sample_pool_id();
        let u1 = sample_upstream_id();
        let u2 = sample_upstream_id();

        db.create_pool(&pool_id, "pool1", "Pool 1", 5, false).unwrap();
        db.create_upstream(&u1, "ProviderA", "http://a", b"k", "m", "[]", true, "").unwrap();
        db.create_upstream(&u2, "ProviderB", "http://b", b"k", "m", "[]", true, "").unwrap();

        db.add_upstream_to_pool(&pool_id, &u1, 0, "").unwrap();
        db.add_upstream_to_pool(&pool_id, &u2, 1, "").unwrap();

        let pus = db.get_pool_upstreams(&pool_id).unwrap();
        assert_eq!(pus.len(), 2);
        assert_eq!(pus[0].upstream_id, u1);
        assert_eq!(pus[0].provider_name, "ProviderA");
        assert_eq!(pus[0].sort_order, 0);
        assert_eq!(pus[1].upstream_id, u2);
        assert_eq!(pus[1].sort_order, 1);
    }

    #[test]
    fn test_remove_upstream_from_pool() {
        let db = test_db();
        let pool_id = sample_pool_id();
        let u1 = sample_upstream_id();

        db.create_pool(&pool_id, "pool1", "Pool 1", 5, false).unwrap();
        db.create_upstream(&u1, "Provider", "http://x", b"k", "m", "[]", true, "").unwrap();
        db.add_upstream_to_pool(&pool_id, &u1, 0, "").unwrap();
        assert_eq!(db.get_pool_upstreams(&pool_id).unwrap().len(), 1);

        db.remove_upstream_from_pool(&pool_id, &u1).unwrap();
        assert_eq!(db.get_pool_upstreams(&pool_id).unwrap().len(), 0);
    }

    #[test]
    fn test_reorder_pool_upstreams() {
        let db = test_db();
        let pool_id = sample_pool_id();
        let u1 = sample_upstream_id();
        let u2 = sample_upstream_id();
        let u3 = sample_upstream_id();

        db.create_pool(&pool_id, "pool1", "Pool 1", 5, false).unwrap();
        db.create_upstream(&u1, "A", "http://a", b"k", "m", "[]", true, "").unwrap();
        db.create_upstream(&u2, "B", "http://b", b"k", "m", "[]", true, "").unwrap();
        db.create_upstream(&u3, "C", "http://c", b"k", "m", "[]", true, "").unwrap();

        db.add_upstream_to_pool(&pool_id, &u1, 0, "").unwrap();
        db.add_upstream_to_pool(&pool_id, &u2, 1, "").unwrap();
        db.add_upstream_to_pool(&pool_id, &u3, 2, "").unwrap();

        // Reorder: C first, A second, B third
        db.reorder_pool_upstreams(&pool_id, &[u3.clone(), u1.clone(), u2.clone()]).unwrap();

        let pus = db.get_pool_upstreams(&pool_id).unwrap();
        assert_eq!(pus[0].upstream_id, u3);
        assert_eq!(pus[0].sort_order, 0);
        assert_eq!(pus[1].upstream_id, u1);
        assert_eq!(pus[1].sort_order, 1);
        assert_eq!(pus[2].upstream_id, u2);
        assert_eq!(pus[2].sort_order, 2);
    }

    #[test]
    fn test_cascade_delete_pool_removes_associations() {
        let db = test_db();
        let pool_id = sample_pool_id();
        let u1 = sample_upstream_id();

        db.create_pool(&pool_id, "pool1", "Pool 1", 5, false).unwrap();
        db.create_upstream(&u1, "Provider", "http://x", b"k", "m", "[]", true, "").unwrap();
        db.add_upstream_to_pool(&pool_id, &u1, 0, "").unwrap();

        db.delete_pool(&pool_id).unwrap();
        // Upstream should still exist, but association should be gone
        assert!(db.upstream_exists(&u1).unwrap());
        assert_eq!(db.get_pool_upstreams(&pool_id).unwrap().len(), 0);
    }

    #[test]
    fn test_cascade_delete_upstream_removes_associations() {
        let db = test_db();
        let pool_id = sample_pool_id();
        let u1 = sample_upstream_id();

        db.create_pool(&pool_id, "pool1", "Pool 1", 5, false).unwrap();
        db.create_upstream(&u1, "Provider", "http://x", b"k", "m", "[]", true, "").unwrap();
        db.add_upstream_to_pool(&pool_id, &u1, 0, "").unwrap();

        db.delete_upstream(&u1).unwrap();
        // Pool should still exist, but association should be gone
        assert!(db.pool_exists(&pool_id).unwrap());
        assert_eq!(db.get_pool_upstreams(&pool_id).unwrap().len(), 0);
    }

    // ---- Settings Tests ----

    #[test]
    fn test_save_and_get_setting() {
        let db = test_db();
        db.save_setting("listen_port", "8080").unwrap();
        let val = db.get_setting("listen_port").unwrap();
        assert_eq!(val, Some("8080".to_string()));
    }

    #[test]
    fn test_get_missing_setting_returns_none() {
        let db = test_db();
        let val = db.get_setting("nonexistent").unwrap();
        assert!(val.is_none());
    }

    #[test]
    fn test_setting_upsert() {
        let db = test_db();
        db.save_setting("key", "value1").unwrap();
        assert_eq!(db.get_setting("key").unwrap(), Some("value1".to_string()));

        db.save_setting("key", "value2").unwrap();
        assert_eq!(db.get_setting("key").unwrap(), Some("value2".to_string()));

        // Should still be one row
        let all = db.get_all_settings().unwrap();
        let matching: Vec<_> = all.iter().filter(|(k, _)| k == "key").collect();
        assert_eq!(matching.len(), 1);
    }

    #[test]
    fn test_delete_setting() {
        let db = test_db();
        db.save_setting("temp", "value").unwrap();
        db.delete_setting("temp").unwrap();
        assert!(db.get_setting("temp").unwrap().is_none());
    }

    #[test]
    fn test_get_all_settings() {
        let db = test_db();
        db.save_setting("alpha", "1").unwrap();
        db.save_setting("beta", "2").unwrap();
        db.save_setting("gamma", "3").unwrap();

        let all = db.get_all_settings().unwrap();
        assert_eq!(all.len(), 3);
        // Should be ordered by key
        assert_eq!(all[0].0, "alpha");
        assert_eq!(all[1].0, "beta");
        assert_eq!(all[2].0, "gamma");
    }

    // ---- Request Log Tests ----

    #[test]
    fn test_insert_and_get_request_log() {
        let db = test_db();
db.insert_request_log(
"log-1", "req-1", Some("pool-a"), Some("up-1"), Some("gpt-4o"),
"[]", "POST", "/v1/chat/completions", 200, 150, false, 100, 50, 150,
).unwrap();

        let filter = LogFilter { limit: 10, ..Default::default() };
        let logs = db.get_recent_logs(&filter).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].id, "log-1");
        assert_eq!(logs[0].request_id, "req-1");
        assert_eq!(logs[0].pool_name, Some("pool-a".to_string()));
        assert_eq!(logs[0].status_code, 200);
        assert_eq!(logs[0].response_time_ms, 150);
        assert!(!logs[0].is_streaming);
    }

    #[test]
    fn test_log_pagination() {
        let db = test_db();
        for i in 0..10 {
db.insert_request_log(
&format!("log-{}", i), &format!("req-{}", i), None, None, None,
"[]", "POST", "/v1/chat/completions", 200, 100, false, 0, 0, 0,
).unwrap();
        }

        // First page
        let filter = LogFilter { limit: 3, offset: 0, ..Default::default() };
        let page1 = db.get_recent_logs(&filter).unwrap();
        assert_eq!(page1.len(), 3);

        // Second page
        let filter = LogFilter { limit: 3, offset: 3, ..Default::default() };
        let page2 = db.get_recent_logs(&filter).unwrap();
        assert_eq!(page2.len(), 3);

        // Pages should have different IDs
        assert_ne!(page1[0].id, page2[0].id);
    }

    #[test]
    fn test_log_filter_by_pool() {
        let db = test_db();
db.insert_request_log("l1", "r1", Some("pool-a"), None, None, "[]", "POST", "/", 200, 50, false, 0, 0, 0).unwrap();
db.insert_request_log("l2", "r2", Some("pool-b"), None, None, "[]", "POST", "/", 200, 50, false, 0, 0, 0).unwrap();
db.insert_request_log("l3", "r3", Some("pool-a"), None, None, "[]", "POST", "/", 200, 50, false, 0, 0, 0).unwrap();

        let filter = LogFilter { pool_name: Some("pool-a".to_string()), limit: 100, ..Default::default() };
        let logs = db.get_recent_logs(&filter).unwrap();
        assert_eq!(logs.len(), 2);
        assert!(logs.iter().all(|l| l.pool_name == Some("pool-a".to_string())));
    }

    #[test]
    fn test_log_filter_by_status_code() {
        let db = test_db();
db.insert_request_log("l1", "r1", None, None, None, "[]", "POST", "/", 200, 50, false, 0, 0, 0).unwrap();
db.insert_request_log("l2", "r2", None, None, None, "[]", "POST", "/", 500, 50, false, 0, 0, 0).unwrap();
db.insert_request_log("l3", "r3", None, None, None, "[]", "POST", "/", 200, 50, false, 0, 0, 0).unwrap();

        let filter = LogFilter { status_prefix: Some(5), limit: 100, ..Default::default() };
        let logs = db.get_recent_logs(&filter).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].status_code, 500);
    }

    #[test]
    fn test_clear_logs() {
        let db = test_db();
        for i in 0..5 {
db.insert_request_log(&format!("l{}", i), &format!("r{}", i), None, None, None,
"[]", "POST", "/", 200, 50, false, 0, 0, 0).unwrap();
        }

        let deleted = db.clear_logs().unwrap();
        assert_eq!(deleted, 5);

        let filter = LogFilter { limit: 100, ..Default::default() };
        assert!(db.get_recent_logs(&filter).unwrap().is_empty());
    }

    // ---- Query Helper Tests ----

    #[test]
    fn test_upstream_exists() {
        let db = test_db();
        let id = sample_upstream_id();
        assert!(!db.upstream_exists(&id).unwrap());

        db.create_upstream(&id, "Test", "http://t", b"k", "m", "[]", true, "").unwrap();
        assert!(db.upstream_exists(&id).unwrap());
    }

    #[test]
    fn test_pool_exists() {
        let db = test_db();
        let id = sample_pool_id();
        assert!(!db.pool_exists(&id).unwrap());

        db.create_pool(&id, "test", "Test", 5, false).unwrap();
        assert!(db.pool_exists(&id).unwrap());
    }

    #[test]
    fn test_count_upstreams() {
        let db = test_db();
        assert_eq!(db.count_upstreams().unwrap(), 0);
        db.create_upstream(&sample_upstream_id(), "A", "http://a", b"k", "m", "[]", true, "").unwrap();
        db.create_upstream(&sample_upstream_id(), "B", "http://b", b"k", "m", "[]", false, "").unwrap();
        assert_eq!(db.count_upstreams().unwrap(), 2);
    }

    #[test]
    fn test_count_active_upstreams() {
        let db = test_db();
        db.create_upstream(&sample_upstream_id(), "A", "http://a", b"k", "m", "[]", true, "").unwrap();
        db.create_upstream(&sample_upstream_id(), "B", "http://b", b"k", "m", "[]", false, "").unwrap();
        db.create_upstream(&sample_upstream_id(), "C", "http://c", b"k", "m", "[]", true, "").unwrap();

        assert_eq!(db.count_active_upstreams().unwrap(), 2);
    }

    #[test]
    fn test_count_pools() {
        let db = test_db();
        assert_eq!(db.count_pools().unwrap(), 0);
        db.create_pool(&sample_pool_id(), "p1", "P1", 5, false).unwrap();
        assert_eq!(db.count_pools().unwrap(), 1);
    }

    #[test]
    fn test_get_stats() {
        let db = test_db();
        db.create_upstream(&sample_upstream_id(), "A", "http://a", b"k", "m", "[]", true, "").unwrap();
        db.create_upstream(&sample_upstream_id(), "B", "http://b", b"k", "m", "[]", false, "").unwrap();
        db.create_pool(&sample_pool_id(), "pool1", "Pool 1", 5, false).unwrap();

        let stats = db.get_stats().unwrap();
        assert_eq!(stats.upstream_count, 2);
        assert_eq!(stats.active_upstream_count, 1);
        assert_eq!(stats.pool_count, 1);
    }

    #[test]
    fn test_get_upstream_status_summary() {
        let db = test_db();
        let id = sample_upstream_id();
        db.create_upstream(&id, "TestProvider", "http://t", b"k", "m", "[]", true, "").unwrap();
        db.update_upstream_status(&id, "degraded", 3, Some("2026-07-25T10:00:00".to_string())).unwrap();

        let summary = db.get_upstream_status_summary().unwrap();
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].status, "degraded");
        assert_eq!(summary[0].failure_count, 3);
    }

    // ---- Transaction Tests ----

    #[test]
    fn test_transaction_commit() {
        let db = test_db();
        let id1 = sample_upstream_id();
        let id2 = sample_upstream_id();

        db.with_transaction(|conn| {
            conn.execute(
                "INSERT INTO upstreams (id, provider_name, base_url, api_key_encrypted, selected_model, available_models, enabled, remark) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![id1, "Tx1", "http://1", b"k", "m", "[]", 1, ""],
            )?;
            conn.execute(
                "INSERT INTO upstreams (id, provider_name, base_url, api_key_encrypted, selected_model, available_models, enabled, remark) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![id2, "Tx2", "http://2", b"k", "m", "[]", 1, ""],
            )?;
            Ok(())
        }).unwrap();

        assert_eq!(db.count_upstreams().unwrap(), 2);
    }

    #[test]
    fn test_transaction_rollback_on_error() {
        let db = test_db();
        let id = sample_upstream_id();

        let result = db.with_transaction(|conn| {
            conn.execute(
                "INSERT INTO upstreams (id, provider_name, base_url, api_key_encrypted, selected_model, available_models, enabled, remark) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![id, "Tx1", "http://1", b"k", "m", "[]", 1, ""],
            )?;
            // Simulate an error
            Err(AppError::Internal("simulated failure".to_string()))
        });

        assert!(result.is_err());
        // The upstream should NOT have been created (rolled back)
        assert_eq!(db.count_upstreams().unwrap(), 0);
    }

    // ---- Encryption Integration Test ----

    #[test]
    fn test_upstream_encrypted_key_roundtrip() {
        use crate::crypto::KeyManager;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let km = KeyManager::initialize(dir.path()).unwrap();
        let db = test_db();
        let id = sample_upstream_id();

        // Encrypt the API key
        let plaintext_key = "sk-secret-api-key-12345";
        let encrypted = km.encrypt_api_key(plaintext_key).unwrap();

        // Store encrypted key in database
        db.create_upstream(&id, "Encrypted", "http://test", &encrypted, "model", "[]", true, "").unwrap();

        // Retrieve and decrypt
        let upstream = db.get_upstream_by_id(&id).unwrap().unwrap();
        let decrypted = km.decrypt_api_key(&upstream.api_key_encrypted).unwrap();
        assert_eq!(decrypted, plaintext_key);
    }

    // ---- Multiple Pools / Upstreams Stress Test ----

    #[test]
    fn test_many_upstreams_and_pools() {
        let db = test_db();
        let mut upstream_ids = Vec::new();

        // Create 20 upstreams
        for i in 0..20 {
            let id = sample_upstream_id();
            db.create_upstream(&id, &format!("Provider-{}", i), &format!("http://p{}.test", i), b"k", "m", "", i % 2 == 0, "").unwrap();
            upstream_ids.push(id);
        }
        assert_eq!(db.count_upstreams().unwrap(), 20);
        assert_eq!(db.count_active_upstreams().unwrap(), 10);

        // Create 5 pools, each with 4 upstreams
        for i in 0..5 {
            let pool_id = sample_pool_id();
            db.create_pool(&pool_id, &format!("pool-{}", i), &format!("Pool {}", i), 5, false).unwrap();
            for j in 0..4 {
                db.add_upstream_to_pool(&pool_id, &upstream_ids[i * 4 + j], j as i32, "").unwrap();
            }
        }
        assert_eq!(db.count_pools().unwrap(), 5);

        // Verify stats
        let stats = db.get_stats().unwrap();
        assert_eq!(stats.upstream_count, 20);
        assert_eq!(stats.pool_count, 5);
    }
}
