use crate::error::AppError;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::{
    Database, DailyTokenUsage, HourlyTokenUsage, ModelTokenUsage, TokenOverviewEntry, TokenTotals,
};

/// A standalone token usage record, decoupled from request_logs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageRecord {
    pub id: String,
    pub request_id: String,
    pub pool_name: Option<String>,
    pub upstream_id: Option<String>,
    pub model: Option<String>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    pub status_code: i32,
    pub created_at: String,
}

impl Database {
    /// Record a token usage entry. Idempotent by `id` (upsert semantics:
    /// streaming requests update usage after the stream completes).
    pub fn record_token_usage(&self, record: &TokenUsageRecord) -> Result<(), AppError> {
        let created_at = if record.created_at.is_empty() {
            None
        } else {
            Some(record.created_at.clone())
        };
        self.get_conn()?.execute(
            "INSERT INTO token_usage (id, request_id, pool_name, upstream_id, model,
                 prompt_tokens, completion_tokens, total_tokens, status_code, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                     COALESCE(?10, datetime('now', 'localtime')))
             ON CONFLICT(id) DO UPDATE SET
                 prompt_tokens=?6, completion_tokens=?7, total_tokens=?8, status_code=?9",
            params![
                record.id,
                record.request_id,
                record.pool_name,
                record.upstream_id,
                record.model,
                record.prompt_tokens,
                record.completion_tokens,
                record.total_tokens,
                record.status_code,
                created_at,
            ],
        )?;
        Ok(())
    }

    /// Update the token usage of an existing record (streaming completion).
    pub fn update_token_usage_tokens(
        &self,
        id: &str,
        prompt_tokens: i64,
        completion_tokens: i64,
        total_tokens: i64,
    ) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "UPDATE token_usage SET prompt_tokens=?1, completion_tokens=?2, total_tokens=?3 WHERE id=?4",
            params![prompt_tokens, completion_tokens, total_tokens, id],
        )?;
        Ok(())
    }

    /// Today's token totals for an upstream (optionally filtered by model).
    pub fn token_today(&self, upstream_id: &str, model: Option<&str>) -> Result<TokenTotals, AppError> {
        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match model {
            Some(m) => (
                "SELECT COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                        COALESCE(SUM(total_tokens), 0)
                 FROM token_usage
                 WHERE upstream_id = ?1 AND model = ?2
                   AND date(created_at) = date('now', 'localtime')".to_string(),
                vec![Box::new(upstream_id.to_string()), Box::new(m.to_string())],
            ),
            None => (
                "SELECT COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                        COALESCE(SUM(total_tokens), 0)
                 FROM token_usage
                 WHERE upstream_id = ?1
                   AND date(created_at) = date('now', 'localtime')".to_string(),
                vec![Box::new(upstream_id.to_string())],
            ),
        };
        self.query_token_totals(&sql, params_vec)
    }

    /// All-time token totals for an upstream (optionally filtered by model).
    pub fn token_total(&self, upstream_id: &str, model: Option<&str>) -> Result<TokenTotals, AppError> {
        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match model {
            Some(m) => (
                "SELECT COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                        COALESCE(SUM(total_tokens), 0)
                 FROM token_usage WHERE upstream_id = ?1 AND model = ?2".to_string(),
                vec![Box::new(upstream_id.to_string()), Box::new(m.to_string())],
            ),
            None => (
                "SELECT COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                        COALESCE(SUM(total_tokens), 0)
                 FROM token_usage WHERE upstream_id = ?1".to_string(),
                vec![Box::new(upstream_id.to_string())],
            ),
        };
        self.query_token_totals(&sql, params_vec)
    }

    /// Daily token usage over the last N days for an upstream (optionally filtered by model).
    pub fn token_daily(
        &self,
        upstream_id: &str,
        model: Option<&str>,
        days: i32,
    ) -> Result<Vec<DailyTokenUsage>, AppError> {
        let offset = format!("-{} days", days);
        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match model {
            Some(m) => (
                "SELECT date(created_at) as day, COALESCE(SUM(prompt_tokens), 0),
                        COALESCE(SUM(completion_tokens), 0), COALESCE(SUM(total_tokens), 0),
                        COUNT(*)
                 FROM token_usage
                 WHERE upstream_id = ?1 AND model = ?2 AND created_at >= datetime('now', 'localtime', ?3)
                 GROUP BY date(created_at) ORDER BY day ASC".to_string(),
                vec![
                    Box::new(upstream_id.to_string()),
                    Box::new(m.to_string()),
                    Box::new(offset),
                ],
            ),
            None => (
                "SELECT date(created_at) as day, COALESCE(SUM(prompt_tokens), 0),
                        COALESCE(SUM(completion_tokens), 0), COALESCE(SUM(total_tokens), 0),
                        COUNT(*)
                 FROM token_usage
                 WHERE upstream_id = ?1 AND created_at >= datetime('now', 'localtime', ?2)
                 GROUP BY date(created_at) ORDER BY day ASC".to_string(),
                vec![Box::new(upstream_id.to_string()), Box::new(offset)],
            ),
        };
        self.query_daily(&sql, params_vec)
    }

    /// Hourly token usage for today for an upstream (optionally filtered by model).
    pub fn token_hourly(
        &self,
        upstream_id: &str,
        model: Option<&str>,
    ) -> Result<Vec<HourlyTokenUsage>, AppError> {
        let conn = self.get_read_conn()?;
        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match model {
            Some(m) => (
                "SELECT strftime('%H', created_at) as hour, COALESCE(SUM(prompt_tokens), 0),
                        COALESCE(SUM(completion_tokens), 0), COALESCE(SUM(total_tokens), 0),
                        COUNT(*)
                 FROM token_usage
                 WHERE upstream_id = ?1 AND model = ?2
                   AND date(created_at) = date('now', 'localtime')
                 GROUP BY strftime('%H', created_at) ORDER BY hour ASC".to_string(),
                vec![Box::new(upstream_id.to_string()), Box::new(m.to_string())],
            ),
            None => (
                "SELECT strftime('%H', created_at) as hour, COALESCE(SUM(prompt_tokens), 0),
                        COALESCE(SUM(completion_tokens), 0), COALESCE(SUM(total_tokens), 0),
                        COUNT(*)
                 FROM token_usage
                 WHERE upstream_id = ?1
                   AND date(created_at) = date('now', 'localtime')
                 GROUP BY strftime('%H', created_at) ORDER BY hour ASC".to_string(),
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

    /// Per-model token usage for an upstream (today + all-time + request count).
    pub fn token_per_model(&self, upstream_id: &str) -> Result<Vec<ModelTokenUsage>, AppError> {
        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(
            "SELECT COALESCE(NULLIF(model, ''), '未记录') as model,
                    COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN total_tokens ELSE 0 END), 0) as today_tokens,
                    COALESCE(SUM(total_tokens), 0) as total_tokens,
                    COUNT(*) as request_count
             FROM token_usage
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

    /// Token overview grouped by pool (today + all-time).
    pub fn token_overview_by_pool(&self) -> Result<Vec<TokenOverviewEntry>, AppError> {
        self.query_token_overview("COALESCE(pool_name, '(unknown)')", "pool_name")
    }

    /// Token overview grouped by upstream (today + all-time).
    pub fn token_overview_by_upstream(&self) -> Result<Vec<TokenOverviewEntry>, AppError> {
        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(
            "SELECT COALESCE(upstream_id, '(unknown)') as name,
                    COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN prompt_tokens ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN completion_tokens ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN total_tokens ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                    COALESCE(SUM(total_tokens), 0), COUNT(*)
             FROM token_usage
             WHERE upstream_id IS NOT NULL
             GROUP BY upstream_id
             ORDER BY 8 DESC",
        )?;
        let rows = stmt.query_map([], |row| self.map_token_overview_row(row))?;
        Self::collect_rows(rows)
    }

    /// Clear all token usage statistics.
    pub fn clear_token_usage(&self) -> Result<i64, AppError> {
        let rows = self.get_conn()?.execute("DELETE FROM token_usage", [])?;
        Ok(rows as i64)
    }

    /// Clear token usage statistics for a specific upstream.
    pub fn clear_upstream_token_usage(&self, upstream_id: &str) -> Result<i64, AppError> {
        let rows = self
            .get_conn()?
            .execute("DELETE FROM token_usage WHERE upstream_id = ?1", params![upstream_id])?;
        Ok(rows as i64)
    }

    // ========================================================================
    // Internal helpers
    // ========================================================================

    fn query_token_totals(
        &self,
        sql: &str,
        params_vec: Vec<Box<dyn rusqlite::types::ToSql>>,
    ) -> Result<TokenTotals, AppError> {
        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(sql)?;
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

    fn query_daily(
        &self,
        sql: &str,
        params_vec: Vec<Box<dyn rusqlite::types::ToSql>>,
    ) -> Result<Vec<DailyTokenUsage>, AppError> {
        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(sql)?;
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

    fn query_token_overview(
        &self,
        name_expr: &str,
        group_col: &str,
    ) -> Result<Vec<TokenOverviewEntry>, AppError> {
        let conn = self.get_read_conn()?;
        let sql = format!(
            "SELECT {name_expr} as name,
                    COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN prompt_tokens ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN completion_tokens ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN total_tokens ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN date(created_at) = date('now', 'localtime') THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                    COALESCE(SUM(total_tokens), 0), COUNT(*)
             FROM token_usage
             GROUP BY {group_col}
             ORDER BY 8 DESC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| self.map_token_overview_row(row))?;
        Self::collect_rows(rows)
    }

    fn map_token_overview_row(&self, row: &rusqlite::Row) -> rusqlite::Result<TokenOverviewEntry> {
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
    }
}

/// Create a token usage record for a request.
#[allow(clippy::too_many_arguments)]
pub fn make_token_usage_record(
    id: &str,
    request_id: &str,
    pool_name: Option<&str>,
    upstream_id: Option<&str>,
    model: Option<&str>,
    prompt_tokens: i64,
    completion_tokens: i64,
    status_code: i32,
    created_at: Option<&str>,
) -> TokenUsageRecord {
    TokenUsageRecord {
        id: id.to_string(),
        request_id: request_id.to_string(),
        pool_name: pool_name.map(str::to_string),
        upstream_id: upstream_id.map(str::to_string),
        model: model.map(str::to_string),
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens + completion_tokens,
        status_code,
        created_at: created_at.unwrap_or("").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db
    }

    fn record(db: &Database, id: &str, upstream: &str, model: &str, p: i64, c: i64) {
        db.record_token_usage(&make_token_usage_record(
            id,
            &format!("req_{}", id),
            Some("pool-1"),
            Some(upstream),
            Some(model),
            p,
            c,
            200,
            None,
        ))
        .unwrap();
    }

    #[test]
    fn test_token_usage_upsert_updates_existing() {
        let db = test_db();
        db.record_token_usage(&make_token_usage_record(
            "tu_1",
            "req_1",
            Some("p"),
            Some("up"),
            Some("m"),
            10,
            5,
            200,
            None,
        ))
        .unwrap();
        // Same id → upsert updates totals.
        db.record_token_usage(&make_token_usage_record(
            "tu_1",
            "req_1",
            Some("p"),
            Some("up"),
            Some("m"),
            30,
            20,
            200,
            None,
        ))
        .unwrap();
        let conn = db.get_read_conn().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM token_usage WHERE id='tu_1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let (p, c): (i64, i64) = conn
            .query_row("SELECT prompt_tokens, completion_tokens FROM token_usage WHERE id='tu_1'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(p, 30);
        assert_eq!(c, 20);
    }

    #[test]
    fn test_token_today_total_and_per_model() {
        let db = test_db();
        record(&db, "a", "up1", "gpt-4", 100, 50);
        record(&db, "b", "up1", "gpt-4", 10, 5);
        record(&db, "c", "up1", "claude", 200, 100);
        record(&db, "d", "up2", "gpt-4", 7, 3);

        let today = db.token_today("up1", None).unwrap();
        assert_eq!(today.total_tokens, 100 + 50 + 10 + 5 + 200 + 100);

        let gpt = db.token_today("up1", Some("gpt-4")).unwrap();
        assert_eq!(gpt.total_tokens, 165);

        let total = db.token_total("up1", None).unwrap();
        assert_eq!(total.total_tokens, 465);

        let per_model = db.token_per_model("up1").unwrap();
        assert_eq!(per_model.len(), 2);
        assert_eq!(per_model[0].model, "claude");
        assert_eq!(per_model[0].total_tokens, 300);
    }

    #[test]
    fn test_token_daily_and_hourly() {
        let db = test_db();
        record(&db, "a", "up1", "gpt-4", 10, 5);
        let daily = db.token_daily("up1", None, 30).unwrap();
        assert_eq!(daily.len(), 1);
        assert_eq!(daily[0].total_tokens, 15);
        assert_eq!(daily[0].request_count, 1);

        let hourly = db.token_hourly("up1", None).unwrap();
        assert_eq!(hourly.len(), 1);
        assert_eq!(hourly[0].total_tokens, 15);
    }

    #[test]
    fn test_token_overview_by_pool_and_upstream() {
        let db = test_db();
        record(&db, "a", "up1", "gpt-4", 10, 5);
        record(&db, "b", "up2", "claude", 20, 10);

        let by_pool = db.token_overview_by_pool().unwrap();
        assert_eq!(by_pool.len(), 1);
        assert_eq!(by_pool[0].total_total_tokens, 45);

        let by_upstream = db.token_overview_by_upstream().unwrap();
        assert_eq!(by_upstream.len(), 2);
    }

    #[test]
    fn test_clear_token_usage() {
        let db = test_db();
        record(&db, "a", "up1", "gpt-4", 10, 5);
        record(&db, "b", "up1", "claude", 1, 1);

        let deleted = db.clear_upstream_token_usage("up1").unwrap();
        assert_eq!(deleted, 2);
        assert_eq!(db.token_total("up1", None).unwrap().total_tokens, 0);

        record(&db, "c", "up1", "gpt-4", 5, 5);
        let deleted_all = db.clear_token_usage().unwrap();
        assert_eq!(deleted_all, 1);
    }
}
