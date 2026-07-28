use crate::error::AppError;
use rusqlite::params;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tracing::warn;

use super::{Database, DailyTokenUsage, FailoverEvent, FailedUpstreamEntry, HourlyTokenUsage, LogFilter, ModelTokenUsage, RequestLogEntry, RequestStatsEntry, StatsFilter, TokenOverviewEntry, TokenTotals};

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
        if let Some(upstream_id) = &filter.upstream_id {
            conditions.push("upstream_id = ?".to_string());
            params_vec.push(Box::new(upstream_id.clone()));
        }
        if let Some(model) = &filter.model {
            conditions.push("model = ?".to_string());
            params_vec.push(Box::new(model.clone()));
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

        let conn = self.get_read_conn()?;
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
        let conn = self.get_read_conn()?;
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
        let conn = self.get_read_conn()?;
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
        let conn = self.get_read_conn()?;
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
        let conn = self.get_read_conn()?;
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
        let conn = self.get_read_conn()?;
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
        let conn = self.get_read_conn()?;

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

        let today_request_count: i64 = self.get_read_conn()?.query_row(
            "SELECT COUNT(*) FROM request_logs WHERE date(created_at) = date('now', 'localtime')",
            [], |row| row.get(0),
        )?;
        let today_success_count: i64 = self.get_read_conn()?.query_row(
            "SELECT COUNT(*) FROM request_logs WHERE date(created_at) = date('now', 'localtime') AND status_code >= 200 AND status_code < 300",
            [], |row| row.get(0),
        )?;
        let today_error_count: i64 = self.get_read_conn()?.query_row(
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

    /// Get aggregated request statistics grouped by upstream + model.
    ///
    /// Computes total/success/error counts, success rate, average latency,
    /// and P95/P99 latency percentiles for each group.
    /// Results are sorted by total_count descending.
    pub fn get_request_stats(&self, filter: &StatsFilter) -> Result<Vec<RequestStatsEntry>, AppError> {
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
        if let Some(upstream_id) = &filter.upstream_id {
            conditions.push("upstream_id = ?".to_string());
            params_vec.push(Box::new(upstream_id.clone()));
        }
        if let Some(model) = &filter.model {
            conditions.push("model = ?".to_string());
            params_vec.push(Box::new(model.clone()));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        // Fetch individual rows for percentile computation (capped at 10000 for safety)
        let sql = format!(
            "SELECT upstream_id, model, status_code, response_time_ms
             FROM request_logs
             {}
             ORDER BY created_at DESC
             LIMIT 10000",
            where_clause
        );

        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        // Group rows by (upstream_id, model) and collect latency values
        struct GroupData {
            total: i64,
            success: i64,
            error: i64,
            latencies: Vec<i32>,
            upstream_id: Option<String>,
            model: Option<String>,
        }

        let mut groups: HashMap<(Option<String>, Option<String>), GroupData> = HashMap::new();

        let rows = stmt.query_map(params.as_slice(), |row| {
            let upstream_id: Option<String> = row.get(0)?;
            let model: Option<String> = row.get(1)?;
            let status_code: i32 = row.get(2)?;
            let response_time_ms: i32 = row.get(3)?;
            Ok((upstream_id, model, status_code, response_time_ms))
        })?;

        for row in rows {
            let (upstream_id, model, status_code, response_time_ms) = row?;
            let key = (upstream_id.clone(), model.clone());
            let group = groups.entry(key).or_insert_with(|| GroupData {
                total: 0,
                success: 0,
                error: 0,
                latencies: Vec::new(),
                upstream_id,
                model,
            });
            group.total += 1;
            if (200..300).contains(&status_code) {
                group.success += 1;
            } else if status_code >= 400 {
                group.error += 1;
            }
            group.latencies.push(response_time_ms);
        }

        // Compute stats for each group
        let mut entries: Vec<RequestStatsEntry> = groups
            .into_values()
            .map(|g| {
                let success_rate = if g.total > 0 {
                    g.success as f64 / g.total as f64 * 100.0
                } else {
                    0.0
                };
                let avg_response_time_ms = if g.latencies.is_empty() {
                    0.0
                } else {
                    g.latencies.iter().map(|&v| v as f64).sum::<f64>() / g.latencies.len() as f64
                };
                let p95 = percentile(&g.latencies, 95);
                let p99 = percentile(&g.latencies, 99);

                RequestStatsEntry {
                    upstream_id: g.upstream_id,
                    upstream_name: None, // Will be resolved by the caller
                    model: g.model,
                    total_count: g.total,
                    success_count: g.success,
                    error_count: g.error,
                    success_rate,
                    avg_response_time_ms,
                    p95_response_time_ms: p95,
                    p99_response_time_ms: p99,
                }
            })
            .collect();

        // Sort by total_count descending
        entries.sort_by_key(|e| std::cmp::Reverse(e.total_count));

        Ok(entries)
    }

    /// Get request logs that had failover events (where `failed_upstreams`
    /// is not an empty JSON array).
    ///
    /// Returns parsed failover events with the full failure chain.
    /// Results are sorted by `created_at` descending (most recent first).
    pub fn get_failover_events(
        &self,
        start_date: Option<&str>,
        end_date: Option<&str>,
        pool_name: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<FailoverEvent>, AppError> {
        let mut conditions = vec![
            "failed_upstreams IS NOT NULL".to_string(),
            "failed_upstreams != '[]'".to_string(),
            "failed_upstreams != ''".to_string(),
        ];
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(start) = start_date {
            conditions.push("created_at >= ?".to_string());
            params_vec.push(Box::new(start.to_string()));
        }
        if let Some(end) = end_date {
            conditions.push("created_at <= ?".to_string());
            params_vec.push(Box::new(end.to_string()));
        }
        if let Some(pool) = pool_name {
            conditions.push("pool_name = ?".to_string());
            params_vec.push(Box::new(pool.to_string()));
        }

        let sql = format!(
            "SELECT id, request_id, pool_name, upstream_id, model,
                    failed_upstreams, status_code, response_time_ms,
                    is_streaming, created_at
             FROM request_logs
             WHERE {}
             ORDER BY created_at DESC
             LIMIT ? OFFSET ?",
            conditions.join(" AND ")
        );

        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(&sql)?;
        params_vec.push(Box::new(limit));
        params_vec.push(Box::new(offset));
        let params: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(params.as_slice(), |row| {
            let failed_json: String = row.get(5)?;
            // Parse the failed_upstreams JSON into structured entries
            let failed: Vec<FailedUpstreamEntry> =
                serde_json::from_str(&failed_json).unwrap_or_default();

            let upstream_id: Option<String> = row.get(3)?;
            // total_attempts = failed count + 1 if there was a successful upstream
            let total_attempts = failed.len() as i32 + if upstream_id.is_some() { 1 } else { 0 };

            Ok(FailoverEvent {
                id: row.get(0)?,
                request_id: row.get(1)?,
                pool_name: row.get(2)?,
                upstream_id,
                upstream_name: None, // Resolved by the caller
                model: row.get(4)?,
                failed_upstreams: failed,
                status_code: row.get(6)?,
                response_time_ms: row.get(7)?,
                is_streaming: row.get::<_, i32>(8)? != 0,
                created_at: row.get(9)?,
                total_attempts,
            })
        })?;

        Self::collect_rows(rows)
    }

    /// Count total failover events matching the filter (for pagination).
    pub fn count_failover_events(
        &self,
        start_date: Option<&str>,
        end_date: Option<&str>,
        pool_name: Option<&str>,
    ) -> Result<i64, AppError> {
        let mut conditions = vec![
            "failed_upstreams IS NOT NULL".to_string(),
            "failed_upstreams != '[]'".to_string(),
            "failed_upstreams != ''".to_string(),
        ];
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(start) = start_date {
            conditions.push("created_at >= ?".to_string());
            params_vec.push(Box::new(start.to_string()));
        }
        if let Some(end) = end_date {
            conditions.push("created_at <= ?".to_string());
            params_vec.push(Box::new(end.to_string()));
        }
        if let Some(pool) = pool_name {
            conditions.push("pool_name = ?".to_string());
            params_vec.push(Box::new(pool.to_string()));
        }

        let sql = format!(
            "SELECT COUNT(*) FROM request_logs WHERE {}",
            conditions.join(" AND ")
        );

        let conn = self.get_read_conn()?;
        let params: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let count: i64 = conn.query_row(&sql, params.as_slice(), |row| row.get(0))?;
        Ok(count)
    }

    /// Get aggregated token usage grouped by pool name.
    ///
    /// Returns today's and all-time token totals for each pool.
    /// Pools with no logs are excluded.
    pub fn get_pool_token_overview(&self) -> Result<Vec<TokenOverviewEntry>, AppError> {
        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(
            "SELECT
                COALESCE(pool_name, '(unknown)') as name,
                COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN prompt_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN completion_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN total_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(prompt_tokens), 0),
                COALESCE(SUM(completion_tokens), 0),
                COALESCE(SUM(total_tokens), 0),
                COUNT(*)
             FROM request_logs
             GROUP BY COALESCE(pool_name, '(unknown)')
             ORDER BY total_total_tokens DESC",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(TokenOverviewEntry {
                name: row.get(0)?,
                today_prompt_tokens: row.get(1)?,
                today_completion_tokens: row.get(2)?,
                today_total_tokens: row.get(3)?,
                today_request_count: row.get(4)?,
                total_prompt_tokens: row.get(5)?,
                total_completion_tokens: row.get(6)?,
                total_total_tokens: row.get(7)?,
                total_request_count: row.get(8)?,
            })
        })?;

        Self::collect_rows(rows)
    }

    /// Get aggregated token usage grouped by upstream.
    ///
    /// Returns today's and all-time token totals for each upstream.
    /// Upstream names are resolved by the caller.
    pub fn get_upstream_token_overview(&self) -> Result<Vec<TokenOverviewEntry>, AppError> {
        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(
            "SELECT
                COALESCE(upstream_id, '(unknown)') as name,
                COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN prompt_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN completion_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN total_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(prompt_tokens), 0),
                COALESCE(SUM(completion_tokens), 0),
                COALESCE(SUM(total_tokens), 0),
                COUNT(*)
             FROM request_logs
             WHERE upstream_id IS NOT NULL
             GROUP BY upstream_id
             ORDER BY total_total_tokens DESC",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(TokenOverviewEntry {
                name: row.get(0)?,
                today_prompt_tokens: row.get(1)?,
                today_completion_tokens: row.get(2)?,
                today_total_tokens: row.get(3)?,
                today_request_count: row.get(4)?,
                total_prompt_tokens: row.get(5)?,
                total_completion_tokens: row.get(6)?,
                total_total_tokens: row.get(7)?,
                total_request_count: row.get(8)?,
            })
        })?;

        Self::collect_rows(rows)
    }
}

/// Compute the P-th percentile of a list of values.
/// Uses the nearest-rank method.
fn percentile(values: &[i32], p: u32) -> i32 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = ((p as f64 / 100.0) * (sorted.len() as f64)).ceil() as usize;
    let idx = rank.saturating_sub(1).min(sorted.len() - 1);
    sorted[idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_percentile_empty() {
        assert_eq!(percentile(&[], 95), 0);
    }

    #[test]
    fn test_percentile_single() {
        assert_eq!(percentile(&[42], 95), 42);
        assert_eq!(percentile(&[42], 99), 42);
    }

    #[test]
    fn test_percentile_multiple() {
        let values: Vec<i32> = (1..=100).collect();
        assert_eq!(percentile(&values, 95), 95);
        assert_eq!(percentile(&values, 99), 99);
        assert_eq!(percentile(&values, 50), 50);
    }

    #[test]
    fn test_percentile_small_set() {
        let values = vec![10, 20, 30, 40, 50];
        // p=95: ceil(0.95 * 5) = ceil(4.75) = 5 → idx 4 → 50
        assert_eq!(percentile(&values, 95), 50);
        // p=99: ceil(0.99 * 5) = ceil(4.95) = 5 → idx 4 → 50
        assert_eq!(percentile(&values, 99), 50);
    }
}
