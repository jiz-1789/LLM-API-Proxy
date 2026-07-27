use crate::error::AppError;
use rusqlite::params;
use tracing::info;

use super::Database;

impl Database {
    // ========================================================================
    // Schema & Migrations
    // ========================================================================

    pub(crate) fn create_schema(&self) -> Result<(), AppError> {
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
                model TEXT NOT NULL DEFAULT '',
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
            );"
        )?;
        Ok(())
    }

    pub(crate) fn run_migrations(&self) -> Result<(), AppError> {
        let current = self.get_schema_version()?;

        let migrations: Vec<(i32, &str)> = vec![
            (1, ""), // v1: initial schema (created in create_schema)
            (2, "ALTER TABLE request_logs ADD COLUMN model TEXT;"),
            (3, "ALTER TABLE request_logs ADD COLUMN prompt_tokens INTEGER NOT NULL DEFAULT 0;\nALTER TABLE request_logs ADD COLUMN completion_tokens INTEGER NOT NULL DEFAULT 0;\nALTER TABLE request_logs ADD COLUMN total_tokens INTEGER NOT NULL DEFAULT 0;"),
            (4, "ALTER TABLE upstreams ADD COLUMN available_models TEXT NOT NULL DEFAULT '[]';"),
            (5, "ALTER TABLE pools ADD COLUMN display_name TEXT NOT NULL DEFAULT '';"), // idempotent: create_schema already has display_name, but old dbs need this
        ];

        for (version, sql) in migrations {
            if current < version {
                if !sql.is_empty() {
                    // Wrap each migration in a transaction and ignore duplicate column errors
                    // for idempotency when schema was partially created by newer create_schema
                    match self.get_conn()?.execute_batch(sql) {
                        Ok(()) => {}
                        Err(rusqlite::Error::SqliteFailure(err, Some(msg)))
                            if err.extended_code == 1 && msg.contains("duplicate column name") => {}
                        Err(e) => return Err(AppError::Database(e)),
                    }
                }
                self.get_conn()?.execute(
                    "UPDATE schema_version SET version = ?1",
                    params![version],
                )?;
                info!("Database migrated to version {}", version);
            }
        }

        // v6 migration: add health tracking columns (idempotent via column_exists)
        if current < 6 {
            let conn = self.get_conn()?;
            if !Self::column_exists_on_conn(&conn, "upstreams", "last_success_time")? {
                conn.execute(
                    "ALTER TABLE upstreams ADD COLUMN last_success_time TEXT",
                    [],
                )?;
            }
            if !Self::column_exists_on_conn(&conn, "upstreams", "last_error_reason")? {
                conn.execute(
                    "ALTER TABLE upstreams ADD COLUMN last_error_reason TEXT",
                    [],
                )?;
            }
            if !Self::column_exists_on_conn(&conn, "upstreams", "recovered_at")? {
                conn.execute(
                    "ALTER TABLE upstreams ADD COLUMN recovered_at TEXT",
                    [],
                )?;
            }
            conn.execute(
                "UPDATE schema_version SET version = ?1",
                params![6],
            )?;
            info!("Database migrated to version 6");
        }

        Ok(())
    }

    pub fn get_schema_version(&self) -> Result<i32, AppError> {
        let conn = self.get_conn()?;
        // Ensure schema_version table exists (for brand new databases)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL DEFAULT 0)",
            [],
        )?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM schema_version",
            [],
            |row| row.get(0),
        )?;
        if count == 0 {
            conn.execute("INSERT INTO schema_version (version) VALUES (0)", [])?;
            Ok(0)
        } else {
            let version: i32 = conn.query_row(
                "SELECT version FROM schema_version",
                [],
                |row| row.get(0),
            )?;
            Ok(version)
        }
    }

    /// Check if a column exists in a table using PRAGMA table_info.
    pub fn column_exists(&self, table: &str, column: &str) -> Result<bool, AppError> {
        let conn = self.get_conn()?;
        Self::column_exists_on_conn(&conn, table, column)
    }

    /// Internal variant that operates on an already-held connection reference.
    /// Used inside migration code to avoid re-acquiring the Mutex lock.
    pub(crate) fn column_exists_on_conn(
        conn: &rusqlite::Connection,
        table: &str,
        column: &str,
    ) -> Result<bool, AppError> {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
        let rows = stmt.query_map([], |row| Ok(row.get::<_, String>(1)?))?;
        for name in rows {
            if name? == column {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
