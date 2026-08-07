use crate::error::AppError;
use rusqlite::params;

use super::{Database, Pool, PoolUpstreamInfo};

impl Database {
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
        thinking_level: &str,
        thinking_custom_params: &str,
    ) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "INSERT INTO pools (id, name, display_name, max_concurrency, thinking_enabled,
                                thinking_level, thinking_custom_params)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                name,
                display_name,
                max_concurrency,
                thinking_enabled as i32,
                thinking_level,
                thinking_custom_params
            ],
        )?;
        Ok(())
    }

    /// Get all pools ordered by creation time (newest first).
    pub fn get_pools(&self) -> Result<Vec<Pool>, AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, display_name, round_robin_strategy, failover_enabled,
                    timeout_seconds, max_concurrency, thinking_enabled,
                    thinking_level, thinking_custom_params,
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
                    thinking_level, thinking_custom_params,
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
                    thinking_level, thinking_custom_params,
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
        thinking_level: &str,
        thinking_custom_params: &str,
    ) -> Result<(), AppError> {
        let rows = self.get_conn()?.execute(
            "UPDATE pools SET display_name=?1, max_concurrency=?2, thinking_enabled=?3,
             thinking_level=?4, thinking_custom_params=?5,
             updated_at=datetime('now', 'localtime') WHERE id=?6",
            params![
                display_name,
                max_concurrency,
                thinking_enabled as i32,
                thinking_level,
                thinking_custom_params,
                id
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
            "INSERT INTO pool_upstreams (pool_id, upstream_id, sort_order, model)
             VALUES (?1, ?2, ?3, ?4)",
            params![pool_id, upstream_id, sort_order, model],
        )?;
        Ok(())
    }

    /// Remove an upstream from a pool.
    pub fn remove_upstream_from_pool(&self, pool_id: &str, upstream_id: &str) -> Result<(), AppError> {
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
            "SELECT u.id, u.provider_name, pu.model, pu.sort_order, pu.thinking_level_override
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
                thinking_level_override: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
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

    /// Check if a pool exists by ID.
    pub fn pool_exists(&self, id: &str) -> Result<bool, AppError> {
        let count: i64 = self.get_conn()?.query_row(
            "SELECT COUNT(*) FROM pools WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Count total pools.
    pub fn count_pools(&self) -> Result<i64, AppError> {
        let count: i64 = self.get_conn()?.query_row(
            "SELECT COUNT(*) FROM pools", [], |row| row.get(0)
        )?;
        Ok(count)
    }
}
