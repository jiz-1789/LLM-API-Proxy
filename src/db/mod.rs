use crate::error::AppError;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, info, warn};

mod migration;
mod upstream;
mod pool;
mod log;
mod settings;

pub use migration::*;
pub use upstream::*;
pub use pool::*;
pub use log::*;
pub use settings::*;

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
    pub upstream_id: Option<String>,
    pub model: Option<String>,
    /// Status code prefix for range filtering (e.g. 2 = 2xx, 4 = 4xx, 5 = 5xx).
    pub status_prefix: Option<i32>,
    pub limit: i64,
    pub offset: i64,
}

/// Statistics filter parameters for aggregated request stats.
#[derive(Debug, Clone, Default)]
pub struct StatsFilter {
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub pool_name: Option<String>,
    pub upstream_id: Option<String>,
    pub model: Option<String>,
}

/// Aggregated request statistics for a single upstream+model group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestStatsEntry {
    pub upstream_id: Option<String>,
    pub upstream_name: Option<String>,
    pub model: Option<String>,
    pub total_count: i64,
    pub success_count: i64,
    pub error_count: i64,
    pub success_rate: f64,
    pub avg_response_time_ms: f64,
    pub p95_response_time_ms: i32,
    pub p99_response_time_ms: i32,
}

/// A configuration change audit entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigChangeEntry {
    pub id: i64,
    pub key: String,
    pub old_value: Option<String>,
    pub new_value: String,
    pub changed_at: String,
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
    pub(crate) fn get_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, AppError> {
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
        // Clean up old logs on startup using configured retention policy
        let retention = crate::config::LogRetentionSettings::load(self);
        let deleted = self.cleanup_old_logs(retention.retention_days, retention.max_entries)?;
        if deleted > 0 {
            info!("Startup log cleanup: removed {} old log entries", deleted);
        }
        Ok(())
    }

    // ========================================================================
    // Private Row Mappers
    // ========================================================================

    pub(crate) fn map_upstream_row(row: &rusqlite::Row) -> rusqlite::Result<Upstream> {
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

    pub(crate) fn map_pool_row(row: &rusqlite::Row) -> rusqlite::Result<Pool> {
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

    pub(crate) fn map_log_row(row: &rusqlite::Row) -> rusqlite::Result<RequestLogEntry> {
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

    pub(crate) fn collect_rows<T>(rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>) -> Result<Vec<T>, AppError> {
        let mut result = Vec::new();
        for r in rows {
            result.push(r?);
        }
        Ok(result)
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
}
