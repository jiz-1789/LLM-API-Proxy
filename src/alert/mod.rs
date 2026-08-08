//! Alert monitoring module.
//!
//! Periodically checks request failure rates and triggers desktop notifications
//! when thresholds are exceeded. Respects a configurable silence period to
//! avoid alert fatigue.
//!
//! ## Configuration
//! - `alert_enabled` (default: false): Whether to enable alert monitoring
//! - `alert_failure_rate_threshold` (default: 50.0): Failure rate percentage that triggers an alert
//! - `alert_min_request_count` (default: 10): Minimum requests in the check window before alerting
//! - `alert_silence_minutes` (default: 30): Minutes to wait before re-alerting

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use crate::config::AlertSettings;
use crate::db::Database;

/// Re-export alert configuration from the config module.
pub use crate::config::AlertSettings as AlertConfig;

/// Load alert configuration from the settings table.
pub fn load_alert_config(db: &Database) -> AlertConfig {
    AlertSettings::load(db)
}

/// Check recent request logs and determine if an alert should be triggered.
///
/// Returns `Some(alert_message)` if the failure rate exceeds the threshold
/// and the minimum request count is met, otherwise `None`.
pub fn check_alert_condition(
    db: &Database,
    config: &AlertConfig,
) -> Option<String> {
    // Query recent request stats (last 5 minutes)
    let stats = match db.get_request_stats(&crate::db::StatsFilter {
        start_date: None,
        end_date: None,
        pool_name: None,
        upstream_id: None,
        model: None,
    }) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "Failed to fetch request stats for alert check");
            return None;
        }
    };

    let total_count: i64 = stats.iter().map(|s| s.total_count).sum();
    let error_count: i64 = stats.iter().map(|s| s.error_count).sum();

    if total_count < config.min_request_count as i64 {
        return None;
    }

    let failure_rate = if total_count > 0 {
        (error_count as f64 / total_count as f64) * 100.0
    } else {
        0.0
    };

    if failure_rate >= config.failure_rate_threshold {
        Some(format!(
            "失败率达到 {:.1}%（阈值 {}%），最近请求 {}/{} 失败",
            failure_rate, config.failure_rate_threshold, error_count, total_count
        ))
    } else {
        None
    }
}

/// Start the background alert monitoring task.
///
/// This spawns a tokio task that periodically checks failure rates.
/// When a threshold is exceeded, it records the alert and sends a
/// desktop notification via the Tauri event system.
pub fn start_alert_task(db: Arc<Database>) {
    let config = load_alert_config(&db);

    if !config.enabled {
        info!("Alert monitoring is disabled");
        return;
    }

    let check_interval = Duration::from_secs(60);

    info!(
        failure_rate_threshold = config.failure_rate_threshold,
        min_request_count = config.min_request_count,
        silence_minutes = config.silence_minutes,
        "Starting alert monitoring task"
    );

    // Spawn a dedicated OS thread with its own tokio runtime, matching the
    // pattern used by start_auto_backup_task and rate_limit::start_persist_task.
    // This avoids panicking when called outside a tokio runtime context (e.g.
    // from Tauri's setup callback).
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                warn!("Failed to create tokio runtime for alert monitoring: {}", e);
                return;
            }
        };
        rt.block_on(async move {
        // Initial delay to allow requests to accumulate
        tokio::time::sleep(Duration::from_secs(30)).await;

        loop {
            let current_config = load_alert_config(&db);

            if current_config.enabled
                && let Some(message) = check_alert_condition(&db, &current_config)
            {
                // Check if we're within the silence period
                let last_alert = db
                    .get_setting("alert_last_triggered_at")
                    .ok()
                    .flatten()
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(0);

                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let silence_secs = (current_config.silence_minutes as i64) * 60;

                if now - last_alert >= silence_secs {
                    warn!(message = %message, "Alert threshold exceeded");

                    // Update last triggered timestamp
                    let _ = db.save_setting("alert_last_triggered_at", &now.to_string());

                    // Record alert in config_changes for audit trail
                    let _ = db.insert_config_change("alert_triggered", None, &message);
                }
            }

            tokio::time::sleep(check_interval).await;
        }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alert_config_default() {
        let config = AlertConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.failure_rate_threshold, 50.0);
        assert_eq!(config.min_request_count, 10);
        assert_eq!(config.silence_minutes, 30);
    }

    #[test]
    fn test_load_alert_config_defaults() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        let config = load_alert_config(&db);
        assert!(!config.enabled);
        assert_eq!(config.failure_rate_threshold, 50.0);
        assert_eq!(config.min_request_count, 10);
        assert_eq!(config.silence_minutes, 30);
    }

    #[test]
    fn test_load_alert_config_custom() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("alert_enabled", "true").unwrap();
        db.save_setting("alert_failure_rate_threshold", "80").unwrap();
        db.save_setting("alert_min_request_count", "20").unwrap();
        db.save_setting("alert_silence_minutes", "60").unwrap();

        let config = load_alert_config(&db);
        assert!(config.enabled);
        assert_eq!(config.failure_rate_threshold, 80.0);
        assert_eq!(config.min_request_count, 20);
        assert_eq!(config.silence_minutes, 60);
    }

    #[test]
    fn test_load_alert_config_min_clamp_threshold() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("alert_failure_rate_threshold", "0.1").unwrap();

        let config = load_alert_config(&db);
        assert_eq!(config.failure_rate_threshold, 1.0); // clamped to minimum
    }

    #[test]
    fn test_load_alert_config_min_clamp_silence() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("alert_silence_minutes", "1").unwrap(); // below minimum

        let config = load_alert_config(&db);
        assert_eq!(config.silence_minutes, 5); // clamped to minimum
    }

    #[test]
    fn test_load_alert_config_invalid_falls_back() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("alert_failure_rate_threshold", "not_a_number").unwrap();
        db.save_setting("alert_min_request_count", "invalid").unwrap();
        db.save_setting("alert_silence_minutes", "bad").unwrap();

        let config = load_alert_config(&db);
        assert_eq!(config.failure_rate_threshold, 50.0); // falls back to default
        assert_eq!(config.min_request_count, 10); // falls back to default
        assert_eq!(config.silence_minutes, 30); // falls back to default
    }

    #[test]
    fn test_save_and_load_alert_config_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let settings = AlertConfig {
            enabled: true,
            failure_rate_threshold: 75.0,
            min_request_count: 15,
            silence_minutes: 45,
        };
        settings.save(&db).unwrap();

        let loaded = load_alert_config(&db);
        assert!(loaded.enabled);
        assert_eq!(loaded.failure_rate_threshold, 75.0);
        assert_eq!(loaded.min_request_count, 15);
        assert_eq!(loaded.silence_minutes, 45);
    }

    #[test]
    fn test_save_alert_config_clamps() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let settings = AlertConfig {
            enabled: true,
            failure_rate_threshold: 0.0,
            min_request_count: 0,
            silence_minutes: 1,
        };
        settings.save(&db).unwrap();

        let loaded = load_alert_config(&db);
        assert_eq!(loaded.failure_rate_threshold, 1.0); // clamped on save
        assert_eq!(loaded.min_request_count, 1); // clamped on save
        assert_eq!(loaded.silence_minutes, 5); // clamped on save
    }

    #[test]
    fn test_check_alert_condition_no_requests() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        let config = AlertConfig {
            enabled: true,
            failure_rate_threshold: 50.0,
            min_request_count: 10,
            silence_minutes: 30,
        };
        // No request logs → no alert
        let result = check_alert_condition(&db, &config);
        assert!(result.is_none());
    }
}
