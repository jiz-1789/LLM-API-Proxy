//! Active upstream probing module.
//!
//! Periodically tests upstream connectivity in the background and updates
//! upstream health status based on probe results.
//!
//! ## Recovery Window Strategy
//! - **Healthy → Degraded**: First failure sets status to "degraded"
//! - **Degraded → Down**: When `failure_count >= failure_threshold`, status becomes "down"
//! - **Down → Healthy**: Only a successful probe can restore status to "healthy"
//!   (sets `recovered_at` timestamp)
//!
//! ## Configuration
//! - `probe_enabled` (default: false): Whether to enable background probing
//! - `probe_interval_seconds` (default: 300, min: 60): Interval between probe cycles
//! - `probe_failure_threshold` (default: 3): Consecutive failures before marking "down"

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use crate::config::ProbeSettings;
use crate::crypto::KeyManager;
use crate::db::Database;

/// Re-export probe configuration from the config module.
pub use crate::config::ProbeSettings as ProbeConfig;

/// Load probe configuration from the settings table.
///
/// Delegates to [`ProbeSettings::load`] for a single source of truth.
pub fn load_probe_config(db: &Database) -> ProbeConfig {
    ProbeSettings::load(db)
}

/// Test connectivity to an upstream by sending a GET /v1/models request.
///
/// Returns `Ok(latency_ms)` on success, `Err(error_message)` on failure.
pub async fn probe_upstream(base_url: &str, api_key: &str) -> Result<u64, String> {
    let url = format!("{}/v1/models", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {}", e))?;

    let start = std::time::Instant::now();
    let result = client.get(&url).bearer_auth(api_key).send().await;
    let elapsed = start.elapsed().as_millis() as u64;

    match result {
        Ok(resp) if resp.status().is_success() => Ok(elapsed),
        Ok(resp) => Err(format!("HTTP {} ({}ms)", resp.status(), elapsed)),
        Err(e) if e.is_timeout() => Err(format!("timeout ({}ms)", elapsed)),
        Err(e) => Err(e.to_string()),
    }
}

/// Probe all enabled upstreams and update their health status in the database.
///
/// This function is called periodically by the background probe task.
pub async fn probe_all_upstreams(
    db: &Arc<Database>,
    crypto: &Arc<KeyManager>,
    failure_threshold: i32,
) {
    let upstreams = match db.get_upstreams() {
        Ok(u) => u,
        Err(e) => {
            warn!(error = %e, "Failed to load upstreams for probing");
            return;
        }
    };

    let mut handles = Vec::new();

    for upstream in upstreams {
        if !upstream.enabled {
            continue;
        }

        let api_key = match crypto.decrypt_api_key(&upstream.api_key_encrypted) {
            Ok(k) => k,
            Err(e) => {
                warn!(
                    upstream_id = %upstream.id,
                    provider = %upstream.provider_name,
                    error = %e,
                    "Failed to decrypt API key for probing"
                );
                // Mark as failure
                if let Err(e) = db.update_upstream_health(
                    &upstream.id,
                    false,
                    Some("key decryption failed"),
                    failure_threshold,
                ) {
                    warn!(upstream_id = %upstream.id, error = %e, "Failed to update upstream health");
                }
                continue;
            }
        };

        let upstream_id = upstream.id.clone();
        let provider_name = upstream.provider_name.clone();
        let base_url = upstream.base_url.clone();

        handles.push(tokio::spawn(async move {
            let result = probe_upstream(&base_url, &api_key).await;
            (upstream_id, provider_name, result)
        }));
    }

    for handle in handles {
        if let Ok((upstream_id, provider_name, result)) = handle.await {
            match result {
                Ok(latency) => {
                    info!(
                        upstream_id = %upstream_id,
                        provider = %provider_name,
                        latency_ms = latency,
                        "Probe succeeded"
                    );
                    if let Err(e) = db.update_upstream_health(&upstream_id, true, None, failure_threshold) {
                        warn!(upstream_id = %upstream_id, error = %e, "Failed to update upstream health after successful probe");
                    }
                }
                Err(err) => {
                    warn!(
                        upstream_id = %upstream_id,
                        provider = %provider_name,
                        error = %err,
                        "Probe failed"
                    );
                    if let Err(e) = db.update_upstream_health(
                        &upstream_id,
                        false,
                        Some(&err),
                        failure_threshold,
                    ) {
                        warn!(upstream_id = %upstream_id, error = %e, "Failed to update upstream health after failed probe");
                    }
                }
            }
        }
    }
}

/// Start the background probe task.
///
/// This spawns a tokio task that periodically probes all enabled upstreams.
/// The task runs indefinitely until the runtime is shut down.
pub fn start_probe_task(
    db: Arc<Database>,
    crypto: Arc<KeyManager>,
    config: ProbeConfig,
) {
    if !config.enabled {
        info!("Upstream probing is disabled");
        return;
    }

    let interval = Duration::from_secs(config.interval_seconds as u64);
    let failure_threshold = config.failure_threshold as i32;

    info!(
        interval_secs = config.interval_seconds,
        failure_threshold = failure_threshold,
        "Starting upstream probe task"
    );

    tokio::spawn(async move {
        // Initial probe shortly after startup (wait 10s for services to initialize)
        tokio::time::sleep(Duration::from_secs(10)).await;

        loop {
            info!("Running periodic upstream probe");
            probe_all_upstreams(&db, &crypto, failure_threshold).await;
            tokio::time::sleep(interval).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_probe_config_default() {
        let config = ProbeConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.interval_seconds, 300);
        assert_eq!(config.failure_threshold, 3);
    }

    #[test]
    fn test_load_probe_config_defaults() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        let config = load_probe_config(&db);
        assert!(!config.enabled);
        assert_eq!(config.interval_seconds, 300);
        assert_eq!(config.failure_threshold, 3);
    }

    #[test]
    fn test_load_probe_config_custom() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("probe_enabled", "true").unwrap();
        db.save_setting("probe_interval_seconds", "120").unwrap();
        db.save_setting("probe_failure_threshold", "5").unwrap();

        let config = load_probe_config(&db);
        assert!(config.enabled);
        assert_eq!(config.interval_seconds, 120);
        assert_eq!(config.failure_threshold, 5);
    }

    #[test]
    fn test_load_probe_config_min_interval() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("probe_enabled", "true").unwrap();
        db.save_setting("probe_interval_seconds", "10").unwrap(); // below minimum

        let config = load_probe_config(&db);
        assert_eq!(config.interval_seconds, 60); // clamped to minimum
    }

    #[test]
    fn test_load_probe_config_min_threshold() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("probe_failure_threshold", "0").unwrap(); // below minimum

        let config = load_probe_config(&db);
        assert_eq!(config.failure_threshold, 1); // clamped to minimum
    }

    #[test]
    fn test_update_upstream_health_success_resets_failures() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let id = uuid::Uuid::new_v4().to_string();
        db.create_upstream(&id, "Test", "http://test", b"key", "model", "[]", true, "")
            .unwrap();

        // Simulate failures first
        db.update_upstream_health(&id, false, Some("timeout"), 3).unwrap();
        db.update_upstream_health(&id, false, Some("timeout"), 3).unwrap();
        assert_eq!(db.get_upstream_by_id(&id).unwrap().unwrap().failure_count, 2);

        // Success should reset
        db.update_upstream_health(&id, true, None, 3).unwrap();
        let upstream = db.get_upstream_by_id(&id).unwrap().unwrap();
        assert_eq!(upstream.failure_count, 0);
        assert_eq!(upstream.status, "healthy");
        assert!(upstream.last_success_time.is_some());
    }

    #[test]
    fn test_update_upstream_health_failure_increments() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let id = uuid::Uuid::new_v4().to_string();
        db.create_upstream(&id, "Test", "http://test", b"key", "model", "[]", true, "")
            .unwrap();

        db.update_upstream_health(&id, false, Some("error"), 3).unwrap();
        let upstream = db.get_upstream_by_id(&id).unwrap().unwrap();
        assert_eq!(upstream.failure_count, 1);
        assert_eq!(upstream.status, "degraded");
        assert!(upstream.last_failure_time.is_some());
        assert_eq!(upstream.last_error_reason, Some("error".to_string()));
    }

    #[test]
    fn test_update_upstream_health_threshold_marks_down() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let id = uuid::Uuid::new_v4().to_string();
        db.create_upstream(&id, "Test", "http://test", b"key", "model", "[]", true, "")
            .unwrap();

        // 3 failures with threshold 3 → down on 3rd
        db.update_upstream_health(&id, false, Some("err"), 3).unwrap();
        assert_eq!(db.get_upstream_by_id(&id).unwrap().unwrap().status, "degraded");

        db.update_upstream_health(&id, false, Some("err"), 3).unwrap();
        assert_eq!(db.get_upstream_by_id(&id).unwrap().unwrap().status, "degraded");

        db.update_upstream_health(&id, false, Some("err"), 3).unwrap();
        let upstream = db.get_upstream_by_id(&id).unwrap().unwrap();
        assert_eq!(upstream.status, "down");
        assert_eq!(upstream.failure_count, 3);
    }

    #[test]
    fn test_update_upstream_health_recovery_sets_recovered_at() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let id = uuid::Uuid::new_v4().to_string();
        db.create_upstream(&id, "Test", "http://test", b"key", "model", "[]", true, "")
            .unwrap();

        // Mark as down
        for _ in 0..3 {
            db.update_upstream_health(&id, false, Some("err"), 3).unwrap();
        }
        assert_eq!(db.get_upstream_by_id(&id).unwrap().unwrap().status, "down");

        // Recovery
        db.update_upstream_health(&id, true, None, 3).unwrap();
        let upstream = db.get_upstream_by_id(&id).unwrap().unwrap();
        assert_eq!(upstream.status, "healthy");
        assert!(upstream.recovered_at.is_some());
    }

    #[test]
    fn test_update_upstream_health_no_threshold_always_degraded() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let id = uuid::Uuid::new_v4().to_string();
        db.create_upstream(&id, "Test", "http://test", b"key", "model", "[]", true, "")
            .unwrap();

        // threshold = 0 means always degraded, never down
        for _ in 0..10 {
            db.update_upstream_health(&id, false, Some("err"), 0).unwrap();
        }
        let upstream = db.get_upstream_by_id(&id).unwrap().unwrap();
        assert_eq!(upstream.status, "degraded");
        assert_eq!(upstream.failure_count, 10);
    }
}
