use crate::error::AppError;
use rusqlite::params;

use super::{Database, Upstream};

impl Database {
    // ========================================================================
    // Upstream CRUD
    // ========================================================================

    /// Create a new upstream record.
    #[allow(clippy::too_many_arguments)]
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
        capabilities: &str,
        api_format: &str,
    ) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "INSERT INTO upstreams (id, provider_name, base_url, api_key_encrypted, selected_model, available_models, enabled, remark, capabilities, api_format)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![id, provider_name, base_url, api_key_encrypted, selected_model, available_models, enabled as i32, remark, capabilities, api_format],
        )?;
        Ok(())
    }

    /// Get all upstreams ordered by creation time (newest first).
    pub fn get_upstreams(&self) -> Result<Vec<Upstream>, AppError> {
        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, provider_name, base_url, api_key_encrypted, selected_model,
                    available_models, enabled, remark, status, failure_count, last_failure_time,
                    last_success_time, last_error_reason, recovered_at, capabilities, api_format,
                    created_at, updated_at
             FROM upstreams ORDER BY created_at DESC"
        )?;
        let rows = stmt.query_map([], Self::map_upstream_row)?;
        Self::collect_rows(rows)
    }

    /// Get a single upstream by its ID.
    pub fn get_upstream_by_id(&self, id: &str) -> Result<Option<Upstream>, AppError> {
        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, provider_name, base_url, api_key_encrypted, selected_model,
                    available_models, enabled, remark, status, failure_count, last_failure_time,
                    last_success_time, last_error_reason, recovered_at, capabilities, api_format,
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
                    last_success_time, last_error_reason, recovered_at, capabilities, api_format,
                    created_at, updated_at
             FROM upstreams WHERE id IN ({})",
            placeholders.join(", ")
        );

        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), Self::map_upstream_row)?;
        Self::collect_rows(rows)
    }

    /// Update an existing upstream record.
    #[allow(clippy::too_many_arguments)]
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
        capabilities: &str,
        api_format: &str,
    ) -> Result<(), AppError> {
        let rows = self.get_conn()?.execute(
            "UPDATE upstreams SET provider_name=?1, base_url=?2, api_key_encrypted=?3,
             selected_model=?4, available_models=?5, enabled=?6, remark=?7, capabilities=?8, api_format=?9, updated_at=datetime('now', 'localtime')
             WHERE id=?10",
            params![provider_name, base_url, api_key_encrypted, selected_model, available_models, enabled as i32, remark, capabilities, api_format, id],
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
            // Atomic single-statement UPDATE using CASE WHEN to conditionally
            // set recovered_at only when transitioning from down/degraded.
            // This avoids the TOCTOU race of SELECT-then-UPDATE.
            self.get_conn()?.execute(
                "UPDATE upstreams SET
                    status = 'healthy',
                    failure_count = 0,
                    last_success_time = datetime('now', 'localtime'),
                    last_error_reason = NULL,
                    recovered_at = CASE
                        WHEN status IN ('down', 'degraded') THEN datetime('now', 'localtime')
                        ELSE recovered_at
                    END,
                    updated_at = datetime('now', 'localtime')
                 WHERE id = ?1",
                params![upstream_id],
            )?;
        } else {
            // Use the write connection (not read_conn) and perform an atomic
            // UPDATE to avoid the TOCTOU race where two threads read the same
            // failure_count, each increment by 1, and one increment is lost.
            // The new status is computed inside SQL using CASE WHEN.
            let conn = self.get_conn()?;

            // Determine the new status expression based on threshold.
            // When threshold <= 0, always "degraded"; otherwise "down" when
            // failure_count + 1 >= threshold, else "degraded".
            let new_status_expr = if failure_threshold <= 0 {
                "'degraded'"
            } else {
                "CASE WHEN failure_count + 1 >= ?2 THEN 'down' ELSE 'degraded' END"
            };

            conn.execute(
                &format!(
                    "UPDATE upstreams SET
                        status = {status_expr},
                        failure_count = failure_count + 1,
                        last_failure_time = datetime('now', 'localtime'),
                        last_error_reason = ?3,
                        updated_at = datetime('now', 'localtime')
                     WHERE id = ?4",
                    status_expr = new_status_expr
                ),
                params![failure_threshold, failure_threshold, error_reason, upstream_id],
            )?;
        }
        Ok(())
    }

    /// Check if an upstream exists by ID.
    pub fn upstream_exists(&self, id: &str) -> Result<bool, AppError> {
        let count: i64 = self.get_read_conn()?.query_row(
            "SELECT COUNT(*) FROM upstreams WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Count total upstreams.
    pub fn count_upstreams(&self) -> Result<i64, AppError> {
        let count: i64 = self.get_read_conn()?.query_row(
            "SELECT COUNT(*) FROM upstreams", [], |row| row.get(0)
        )?;
        Ok(count)
    }

    /// Count active (enabled) upstreams.
    pub fn count_active_upstreams(&self) -> Result<i64, AppError> {
        let count: i64 = self.get_read_conn()?.query_row(
            "SELECT COUNT(*) FROM upstreams WHERE enabled = 1", [], |row| row.get(0)
        )?;
        Ok(count)
    }

    /// Get status summary for all upstreams.
    pub fn get_upstream_status_summary(&self) -> Result<Vec<super::UpstreamStatusSummary>, AppError> {
        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, provider_name, status, failure_count, last_failure_time,
                    last_success_time, last_error_reason, recovered_at
             FROM upstreams ORDER BY provider_name"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(super::UpstreamStatusSummary {
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
}
