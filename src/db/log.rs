use crate::error::AppError;
use rusqlite::params;
use std::sync::atomic::Ordering;
use tracing::warn;

use super::{Database, DailyTokenUsage, HourlyTokenUsage, LogFilter, ModelTokenUsage, RequestLogEntry, TokenTotals};

impl Database {
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
            let retention = crate::config::LogRetentionSettings::load(self);
            if let Err(e) = self.cleanup_old_logs(retention.retention_days, retention.max_entries) {
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
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(start) = &filter.start_date {
            conditions.push("created_at >= ?".to_string());
            params_vec.push(Box::new(start.clone()));
        }
        if let Some(end) = &filter.end_date {
            conditions.push("created_at <= ?".to_string());
            params_vec.push(Box::new(end.clone()));
        }
        if let Some(pool) = &filter.pool_name {
            conditions.push("pool_name = ?".to_string());
            params_vec.push(Box::new(pool.clone()));
        }
        if let Some(prefix) = filter.status_prefix {
            // Range filtering: prefix 2 -> 200-299, 4 -> 400-499, 5 -> 500-599
            let lower = prefix * 100;
            let upper = lower + 99;
            conditions.push("status_code >= ?".to_string());
            conditions.push("status_code <= ?".to_string());
            params_vec.push(Box::new(lower));
            params_vec.push(Box::new(upper));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            "SELECT id, request_id, pool_name, upstream_id, model, failed_upstreams,
                    method, endpoint, status_code, response_time_ms, is_streaming,
                    prompt_tokens, completion_tokens, total_tokens, created_at
             FROM request_logs
             {}
             ORDER BY created_at DESC
             LIMIT ? OFFSET ?",
            where_clause
        );

        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(&sql)?;
        params_vec.push(Box::new(filter.limit));
        params_vec.push(Box::new(filter.offset));
        let params: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
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
            tracing::debug!("Log cleanup: removed {} entries ({} by age, remaining {})",
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

    /// Get today's token totals for an upstream, optionally filtered by model.
    pub fn get_upstream_today_tokens(&self, upstream_id: &str, model: Option<&str>) -> Result<TokenTotals, AppError> {
        let conn = self.get_conn()?;
        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match model {
            Some(m) => (
                "SELECT COALESCE(SUM(prompt_tokens), 0),
                        COALESCE(SUM(completion_tokens), 0),
                        COALESCE(SUM(total_tokens), 0)
                 FROM request_logs
                 WHERE upstream_id = ?1
                   AND model = ?2
                   AND date(created_at) = date('now', 'localtime')".to_string(),
                vec![Box::new(upstream_id.to_string()), Box::new(m.to_string())],
            ),
            None => (
                "SELECT COALESCE(SUM(prompt_tokens), 0),
                        COALESCE(SUM(completion_tokens), 0),
                        COALESCE(SUM(total_tokens), 0)
                 FROM request_logs
                 WHERE upstream_id = ?1
                   AND date(created_at) = date('now', 'localtime')".to_string(),
                vec![Box::new(upstream_id.to_string())],
            ),
        };
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let result = stmt.query_row(params.as_slice(), |row| Ok(TokenTotals {
            prompt_tokens: row.get(0)?,
            completion_tokens: row.get(1)?,
            total_tokens: row.get(2)?,
        }))?;
        Ok(result)
    }

    /// Get all-time token totals for an upstream, optionally filtered by model.
    pub fn get_upstream_total_tokens(&self, upstream_id: &str, model: Option<&str>) -> Result<TokenTotals, AppError> {
        let conn = self.get_conn()?;
        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match model {
            Some(m) => (
                "SELECT COALESCE(SUM(prompt_tokens), 0),
                        COALESCE(SUM(completion_tokens), 0),
                        COALESCE(SUM(total_tokens), 0)
                 FROM request_logs
                 WHERE upstream_id = ?1 AND model = ?2".to_string(),
                vec![Box::new(upstream_id.to_string()), Box::new(m.to_string())],
            ),
            None => (
                "SELECT COALESCE(SUM(prompt_tokens), 0),
                        COALESCE(SUM(completion_tokens), 0),
                        COALESCE(SUM(total_tokens), 0)
                 FROM request_logs
                 WHERE upstream_id = ?1".to_string(),
                vec![Box::new(upstream_id.to_string())],
            ),
        };
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let result = stmt.query_row(params.as_slice(), |row| Ok(TokenTotals {
            prompt_tokens: row.get(0)?,
            completion_tokens: row.get(1)?,
            total_tokens: row.get(2)?,
        }))?;
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
    /// Returns 24 rows (one per hour, 00-23).
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
                 GROUP BY strftime('%H', created_at, 'localtime')
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
                 GROUP BY strftime('%H', created_at, 'localtime')
                 ORDER BY hour ASC"
                    .to_string(),
                vec![Box::new(upstream_id.to_string())],
            ),
        };

        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok(HourlyTokenUsage {
                hour: row.get(0)?,
                prompt_tokens: row.get(1)?,
                completion_tokens: row.get(2)?,
                total_tokens: row.get(3)?,
                request_count: row.get(4)?,
            })
        })?;
        Self::collect_rows(rows)
    }

    /// Get aggregate statistics.
    pub fn get_stats(&self) -> Result<super::Stats, AppError> {
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

        Ok(super::Stats {
            upstream_count,
            active_upstream_count,
            pool_count,
            today_request_count,
            today_success_count,
            today_error_count,
        })
    }
}
