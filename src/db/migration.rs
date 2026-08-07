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
                thinking_level TEXT NOT NULL DEFAULT 'off',
                thinking_custom_params TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
            );

            CREATE TABLE IF NOT EXISTS pool_upstreams (
                pool_id TEXT NOT NULL REFERENCES pools(id) ON DELETE CASCADE,
                upstream_id TEXT NOT NULL REFERENCES upstreams(id) ON DELETE CASCADE,
                sort_order INTEGER NOT NULL DEFAULT 0,
                model TEXT NOT NULL DEFAULT '',
                thinking_level_override TEXT NOT NULL DEFAULT '',
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

            -- Indexes for high-frequency query patterns on request_logs
            CREATE INDEX IF NOT EXISTS idx_request_logs_created_at ON request_logs(created_at);
            CREATE INDEX IF NOT EXISTS idx_request_logs_upstream_id ON request_logs(upstream_id);
            CREATE INDEX IF NOT EXISTS idx_request_logs_status_code ON request_logs(status_code);
            CREATE INDEX IF NOT EXISTS idx_request_logs_pool_name ON request_logs(pool_name);

            CREATE TABLE IF NOT EXISTS config_changes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                key TEXT NOT NULL,
                old_value TEXT,
                new_value TEXT NOT NULL,
                changed_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
            );

            CREATE INDEX IF NOT EXISTS idx_config_changes_changed_at ON config_changes(changed_at);

            CREATE TABLE IF NOT EXISTS rate_limit_state (
                client_ip TEXT PRIMARY KEY,
                count INTEGER NOT NULL,
                window_start INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS api_keys (
                id TEXT PRIMARY KEY,
                key TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL DEFAULT '',
                enabled INTEGER NOT NULL DEFAULT 1,
                allowed_pools TEXT NOT NULL DEFAULT '[]',
                expires_at TEXT,
                last_used_at TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
            );

            CREATE INDEX IF NOT EXISTS idx_api_keys_key ON api_keys(key);"
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

        // v7 migration: add high-frequency query indexes on request_logs (idempotent)
        if current < 7 {
            self.get_conn()?.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_request_logs_created_at ON request_logs(created_at);
                 CREATE INDEX IF NOT EXISTS idx_request_logs_upstream_id ON request_logs(upstream_id);
                 CREATE INDEX IF NOT EXISTS idx_request_logs_status_code ON request_logs(status_code);
                 CREATE INDEX IF NOT EXISTS idx_request_logs_pool_name ON request_logs(pool_name);",
            )?;
            self.get_conn()?.execute(
                "UPDATE schema_version SET version = ?1",
                params![7],
            )?;
            info!("Database migrated to version 7");
        }

        // v8 migration: create config_changes table for audit trail (idempotent)
        if current < 8 {
            self.get_conn()?.execute_batch(
                "CREATE TABLE IF NOT EXISTS config_changes (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    key TEXT NOT NULL,
                    old_value TEXT,
                    new_value TEXT NOT NULL,
                    changed_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
                );
                CREATE INDEX IF NOT EXISTS idx_config_changes_changed_at ON config_changes(changed_at);",
            )?;
            self.get_conn()?.execute(
                "UPDATE schema_version SET version = ?1",
                params![8],
            )?;
            info!("Database migrated to version 8");
        }

        // v9 migration: create rate_limit_state table for persistence (idempotent)
        if current < 9 {
            self.get_conn()?.execute_batch(
                "CREATE TABLE IF NOT EXISTS rate_limit_state (
                    client_ip TEXT PRIMARY KEY,
                    count INTEGER NOT NULL,
                    window_start INTEGER NOT NULL
                );",
            )?;
            self.get_conn()?.execute(
                "UPDATE schema_version SET version = ?1",
                params![9],
            )?;
            info!("Database migrated to version 9");
        }

        // v10 migration: create api_keys table for multi-key access control (idempotent)
        if current < 10 {
            self.get_conn()?.execute_batch(
                "CREATE TABLE IF NOT EXISTS api_keys (
                    id TEXT PRIMARY KEY,
                    key TEXT NOT NULL UNIQUE,
                    name TEXT NOT NULL DEFAULT '',
                    enabled INTEGER NOT NULL DEFAULT 1,
                    allowed_pools TEXT NOT NULL DEFAULT '[]',
                    expires_at TEXT,
                    last_used_at TEXT,
                    created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
                    updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
                );
                CREATE INDEX IF NOT EXISTS idx_api_keys_key ON api_keys(key);",
            )?;
            self.get_conn()?.execute(
                "UPDATE schema_version SET version = ?1",
                params![10],
            )?;
            info!("Database migrated to version 10");
        }

        // v11 migration: create tool_configs table for tool config switch system (idempotent)
        if current < 11 {
            self.get_conn()?.execute_batch(
                "CREATE TABLE IF NOT EXISTS tool_configs (
                    id TEXT PRIMARY KEY,
                    tool_app_id TEXT NOT NULL UNIQUE,
                    pool_id TEXT,
                    api_key_id TEXT,
                    provider_name TEXT NOT NULL DEFAULT '',
                    switch_enabled INTEGER NOT NULL DEFAULT 0,
                    original_config TEXT NOT NULL DEFAULT '',
                    config_snapshot TEXT NOT NULL DEFAULT '{}',
                    last_written_at TEXT,
                    created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
                    updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
                );",
            )?;
            self.get_conn()?.execute(
                "UPDATE schema_version SET version = ?1",
                params![11],
            )?;
            info!("Database migrated to version 11");
        }

        // v12 migration: upstream api_format column (idempotent)
        if current < 12 {
            let conn = self.get_conn()?;
            if !Self::column_exists_on_conn(&conn, "upstreams", "api_format")? {
                conn.execute(
                    "ALTER TABLE upstreams ADD COLUMN api_format TEXT NOT NULL DEFAULT 'openai_chat'",
                    [],
                )?;
            }
            conn.execute(
                "UPDATE schema_version SET version = ?1",
                params![12],
            )?;
            info!("Database migrated to version 12");
        }

        // v13 migration: multi-level thinking intensity (idempotent)
        // Adds pools.thinking_level, pools.thinking_custom_params,
        // pool_upstreams.thinking_level_override. Old thinking_enabled
        // column is kept for backward compatibility.
        if current < 13 {
            let conn = self.get_conn()?;
            if !Self::column_exists_on_conn(&conn, "pools", "thinking_level")? {
                conn.execute(
                    "ALTER TABLE pools ADD COLUMN thinking_level TEXT NOT NULL DEFAULT 'off'",
                    [],
                )?;
            }
            if !Self::column_exists_on_conn(&conn, "pools", "thinking_custom_params")? {
                conn.execute(
                    "ALTER TABLE pools ADD COLUMN thinking_custom_params TEXT NOT NULL DEFAULT ''",
                    [],
                )?;
            }
            if !Self::column_exists_on_conn(&conn, "pool_upstreams", "thinking_level_override")? {
                conn.execute(
                    "ALTER TABLE pool_upstreams ADD COLUMN thinking_level_override TEXT NOT NULL DEFAULT ''",
                    [],
                )?;
            }
            // Data backfill: legacy thinking_enabled=true → thinking_level='high'
            conn.execute(
                "UPDATE pools SET thinking_level = 'high' WHERE thinking_enabled = 1 AND thinking_level = 'off'",
                [],
            )?;
            conn.execute(
                "UPDATE schema_version SET version = ?1",
                params![13],
            )?;
            info!("Database migrated to version 13");
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Check if an index exists on a table using PRAGMA index_list.
    fn index_exists(conn: &rusqlite::Connection, table: &str, index_name: &str) -> bool {
        let mut stmt = conn
            .prepare(&format!("PRAGMA index_list({})", table))
            .unwrap();
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap();
        for name in rows {
            if name.unwrap() == index_name {
                return true;
            }
        }
        false
    }

    #[test]
    fn test_indexes_created_on_new_db() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let conn = db.get_conn().unwrap();
        assert!(index_exists(&conn, "request_logs", "idx_request_logs_created_at"));
        assert!(index_exists(&conn, "request_logs", "idx_request_logs_upstream_id"));
        assert!(index_exists(&conn, "request_logs", "idx_request_logs_status_code"));
        assert!(index_exists(&conn, "request_logs", "idx_request_logs_pool_name"));
    }

    #[test]
    fn test_schema_version_is_v10_after_migration() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        let version = db.get_schema_version().unwrap();
        assert_eq!(version, 13);
    }

    #[test]
    fn test_thinking_columns_present_after_migration() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        assert!(db.column_exists("pools", "thinking_level").unwrap());
        assert!(db.column_exists("pools", "thinking_custom_params").unwrap());
        assert!(db.column_exists("pool_upstreams", "thinking_level_override").unwrap());
        assert!(db.column_exists("pools", "thinking_enabled").unwrap());
    }

    #[test]
    fn test_indexes_idempotent() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        // Running initialize again should not fail (CREATE INDEX IF NOT EXISTS)
        db.initialize().unwrap();

        let conn = db.get_conn().unwrap();
        assert!(index_exists(&conn, "request_logs", "idx_request_logs_created_at"));
    }

    #[test]
    fn test_config_changes_table_created() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        // Table should exist and be queryable
        let count = db.count_config_changes().unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_insert_and_query_config_change() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        db.insert_config_change("listen_port", Some("47339"), "8080").unwrap();
        db.insert_config_change("log_level", Some("info"), "debug").unwrap();

        let changes = db.get_config_changes(10, 0).unwrap();
        assert_eq!(changes.len(), 2);
        // Most recent first (ORDER BY changed_at DESC)
        assert_eq!(changes[0].key, "log_level");
        assert_eq!(changes[0].old_value.as_deref(), Some("info"));
        assert_eq!(changes[0].new_value, "debug");
        assert_eq!(changes[1].key, "listen_port");
        assert_eq!(changes[1].old_value.as_deref(), Some("47339"));
        assert_eq!(changes[1].new_value, "8080");
    }

    #[test]
    fn test_save_setting_with_audit_no_change() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("test_key", "value1").unwrap();

        // Same value → no audit entry
        let changed = db.save_setting_with_audit("test_key", "value1").unwrap();
        assert!(!changed); // no change recorded
        assert_eq!(db.count_config_changes().unwrap(), 0);
    }

    #[test]
    fn test_save_setting_with_audit_with_change() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("test_key", "value1").unwrap();

        // Different value → audit entry created
        let changed = db.save_setting_with_audit("test_key", "value2").unwrap();
        assert!(changed); // change recorded

        let changes = db.get_config_changes(10, 0).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].key, "test_key");
        assert_eq!(changes[0].old_value.as_deref(), Some("value1"));
        assert_eq!(changes[0].new_value, "value2");
    }

    #[test]
    fn test_save_setting_with_audit_new_key() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        // Key doesn't exist yet → old_value is None
        let changed = db.save_setting_with_audit("new_key", "new_value").unwrap();
        assert!(changed);

        let changes = db.get_config_changes(10, 0).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].key, "new_key");
        assert!(changes[0].old_value.is_none());
        assert_eq!(changes[0].new_value, "new_value");
    }
}
