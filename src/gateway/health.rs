//! Three-tier health check module.
//!
//! Provides a comprehensive health check endpoint that aggregates:
//! 1. **Application health**: process running status + uptime
//! 2. **Database health**: connectivity verification via `get_stats()`
//! 3. **Upstream health**: aggregate count of healthy/degraded/down upstreams
//!
//! The overall status is "ok" only when both app and database are healthy.
//! If any upstream is down, the overall status becomes "degraded".

use std::sync::OnceLock;
use std::time::Instant;

use axum::Json;
use serde_json::{json, Value};

use crate::db::Database;

/// Process start time, set once on first health check call.
static START_TIME: OnceLock<Instant> = OnceLock::new();

/// Get the process start time (lazy-initialized on first call).
fn start_time() -> Instant {
    *START_TIME.get_or_init(Instant::now)
}

/// Get uptime in seconds.
fn uptime_seconds() -> u64 {
    start_time().elapsed().as_secs()
}

/// Application health layer.
fn app_health() -> Value {
    json!({
        "status": "ok",
        "uptime_seconds": uptime_seconds(),
        "version": env!("CARGO_PKG_VERSION"),
    })
}

/// Database health layer.
/// Returns status "ok" if `get_stats()` succeeds, "error" otherwise.
fn db_health(db: &Database) -> Value {
    match db.get_stats() {
        Ok(stats) => json!({
            "status": "ok",
            "upstream_count": stats.upstream_count,
            "pool_count": stats.pool_count,
        }),
        Err(e) => json!({
            "status": "error",
            "error": e.to_string(),
        }),
    }
}

/// Upstream health layer.
/// Aggregates all upstreams into healthy/degraded/down counts.
fn upstream_health(db: &Database) -> Value {
    match db.get_upstream_status_summary() {
        Ok(summaries) => {
            let healthy = summaries.iter().filter(|s| s.status == "healthy").count();
            let degraded = summaries.iter().filter(|s| s.status == "degraded").count();
            let down = summaries.iter().filter(|s| s.status == "down").count();
            let total = summaries.len();

            let status = if down == total && total > 0 {
                "down"
            } else if down > 0 || degraded > 0 {
                "degraded"
            } else {
                "ok"
            };

            json!({
                "status": status,
                "total": total,
                "healthy": healthy,
                "degraded": degraded,
                "down": down,
            })
        }
        Err(e) => json!({
            "status": "error",
            "error": e.to_string(),
        }),
    }
}

/// Build the combined three-tier health check JSON response.
pub fn health_response(db: &Database) -> Json<Value> {
    let app = app_health();
    let database = db_health(db);
    let upstream = upstream_health(db);

    // Overall status: "ok" only if app and db are both ok.
    // If any upstream is down/degraded, overall is "degraded".
    let app_ok = app.get("status").and_then(|v| v.as_str()) == Some("ok");
    let db_ok = database.get("status").and_then(|v| v.as_str()) == Some("ok");
    let upstream_status = upstream.get("status").and_then(|v| v.as_str()).unwrap_or("ok");

    let overall = if !app_ok || !db_ok {
        "down"
    } else if upstream_status == "down" || upstream_status == "degraded" {
        "degraded"
    } else {
        "ok"
    };

    Json(json!({
        "status": overall,
        "service": "LLM-API-Proxy",
        "version": env!("CARGO_PKG_VERSION"),
        "app": app,
        "database": database,
        "upstreams": upstream,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn test_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db
    }

    #[test]
    fn test_health_response_overall_ok() {
        let db = test_db();
        let resp = health_response(&db);
        let status = resp.get("status").and_then(|v| v.as_str()).unwrap();
        assert_eq!(status, "ok");
    }

    #[test]
    fn test_health_response_has_app_layer() {
        let db = test_db();
        let resp = health_response(&db);
        let app = resp.get("app").unwrap();
        assert_eq!(app.get("status").unwrap(), "ok");
        assert!(app.get("uptime_seconds").is_some());
        assert!(app.get("version").is_some());
    }

    #[test]
    fn test_health_response_has_database_layer() {
        let db = test_db();
        let resp = health_response(&db);
        let database = resp.get("database").unwrap();
        assert_eq!(database.get("status").unwrap(), "ok");
        assert_eq!(database.get("upstream_count").unwrap(), &0);
        assert_eq!(database.get("pool_count").unwrap(), &0);
    }

    #[test]
    fn test_health_response_has_upstream_layer() {
        let db = test_db();
        let resp = health_response(&db);
        let upstreams = resp.get("upstreams").unwrap();
        assert_eq!(upstreams.get("status").unwrap(), "ok");
        assert_eq!(upstreams.get("total").unwrap(), &0);
        assert_eq!(upstreams.get("healthy").unwrap(), &0);
        assert_eq!(upstreams.get("degraded").unwrap(), &0);
        assert_eq!(upstreams.get("down").unwrap(), &0);
    }

    #[test]
    fn test_uptime_increases() {
        let t1 = uptime_seconds();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let t2 = uptime_seconds();
        assert!(t2 >= t1 + 1, "uptime should increase by at least 1 second");
    }
}
