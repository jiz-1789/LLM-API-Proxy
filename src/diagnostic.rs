//! One-click diagnostic package module (P2-11).
//!
//! Collects system diagnostic information and exports it as a ZIP archive.
//! All sensitive data (API keys, tokens, secrets) is masked before export.
//!
//! ## Diagnostic package contents
//!
//! The generated ZIP archive contains:
//! - `diagnostic.json` — main summary (version, schema, stats, health)
//! - `config_summary.json` — all settings with sensitive values masked
//! - `upstream_status.json` — per-upstream health and error info
//! - `recent_logs.json` — last 50 request log entries
//! - `pools.json` — all pool configurations (no API keys)

use crate::db::Database;
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use tracing::{info, warn};

/// Number of recent log entries to include in the diagnostic package.
const RECENT_LOG_LIMIT: i64 = 50;

/// Sensitive setting key substrings — values for these keys are masked.
const SENSITIVE_KEY_PATTERNS: &[&str] = &["api_key", "token", "secret", "password"];

// ============================================================================
// Sensitive Data Masking
// ============================================================================

/// Check if a settings key name is sensitive and its value should be masked.
pub fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    SENSITIVE_KEY_PATTERNS.iter().any(|p| lower.contains(p))
}

/// Mask a sensitive value, preserving only length metadata.
pub fn mask_value(value: &str) -> String {
    if value.is_empty() {
        "(empty)".to_string()
    } else {
        format!("•••••••• (len={})", value.len())
    }
}

/// Mask a settings key-value pair if the key is sensitive.
fn mask_setting(key: &str, value: &str) -> (String, String) {
    if is_sensitive_key(key) {
        (key.to_string(), mask_value(value))
    } else {
        (key.to_string(), value.to_string())
    }
}

// ============================================================================
// Diagnostic Data Structures
// ============================================================================

/// Top-level diagnostic information included in the package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticInfo {
    /// Application version (from Cargo.toml).
    pub app_version: String,
    /// Database schema version.
    pub schema_version: i32,
    /// Timestamp when the diagnostic package was generated.
    pub generated_at: String,
    /// Aggregate system statistics.
    pub stats: crate::db::Stats,
    /// Three-tier health check result (app / database / upstreams).
    pub health_check: Value,
}

/// A sanitized upstream entry without the encrypted API key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SanitizedUpstream {
    pub id: String,
    pub provider_name: String,
    pub base_url: String,
    /// `true` if an API key is configured, `false` if empty.
    pub has_api_key: bool,
    pub selected_model: String,
    pub enabled: bool,
    pub status: String,
    pub failure_count: i32,
    pub last_failure_time: Option<String>,
    pub last_success_time: Option<String>,
    pub last_error_reason: Option<String>,
    pub recovered_at: Option<String>,
    /// JSON-serialized ModelCapabilities; empty means unknown, v14.
    pub capabilities: String,
}

/// A sanitized pool entry for diagnostic output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SanitizedPool {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub timeout_seconds: i32,
    pub max_concurrency: i32,
    pub thinking_enabled: bool,
    pub thinking_level: String,
    pub failover_enabled: bool,
    /// Pool-level aggregated capabilities (JSON), v14.
    pub capabilities: String,
}

// ============================================================================
// Collection
// ============================================================================

/// Collect all diagnostic information from the database.
pub fn collect_diagnostic_info(db: &Database) -> Result<DiagnosticInfo, AppError> {
    let schema_version = db.get_schema_version()?;
    let stats = db.get_stats().unwrap_or_else(|e| {
        warn!("Failed to get stats for diagnostic: {}", e);
        crate::db::Stats::default()
    });

    let health_check = build_health_check(db);

    Ok(DiagnosticInfo {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        schema_version,
        generated_at: chrono::Local::now()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        stats,
        health_check,
    })
}

/// Collect all settings with sensitive values masked.
pub fn collect_config_summary(db: &Database) -> Vec<(String, String)> {
    match db.get_all_settings() {
        Ok(settings) => settings
            .into_iter()
            .map(|(k, v)| mask_setting(&k, &v))
            .collect(),
        Err(e) => {
            warn!("Failed to get settings for diagnostic: {}", e);
            vec![]
        }
    }
}

/// Collect upstream status summaries (no API keys).
pub fn collect_upstream_status(db: &Database) -> Vec<crate::db::UpstreamStatusSummary> {
    db.get_upstream_status_summary().unwrap_or_else(|e| {
        warn!("Failed to get upstream status for diagnostic: {}", e);
        vec![]
    })
}

/// Collect sanitized upstream records (excludes encrypted API key).
pub fn collect_sanitized_upstreams(db: &Database) -> Vec<SanitizedUpstream> {
    db.get_upstreams()
        .unwrap_or_default()
        .into_iter()
        .map(|u| SanitizedUpstream {
            id: u.id,
            provider_name: u.provider_name,
            base_url: u.base_url,
            has_api_key: !u.api_key_encrypted.is_empty(),
            selected_model: u.selected_model,
            enabled: u.enabled,
            status: u.status,
            failure_count: u.failure_count,
            last_failure_time: u.last_failure_time,
            last_success_time: u.last_success_time,
            last_error_reason: u.last_error_reason,
            recovered_at: u.recovered_at,
            capabilities: u.capabilities,
        })
        .collect()
}

/// Collect sanitized pool records.
pub fn collect_sanitized_pools(db: &Database) -> Vec<SanitizedPool> {
    db.get_pools()
        .unwrap_or_default()
        .into_iter()
        .map(|p| SanitizedPool {
            id: p.id,
            name: p.name,
            display_name: p.display_name,
            timeout_seconds: p.timeout_seconds,
            max_concurrency: p.max_concurrency,
            thinking_enabled: p.thinking_enabled,
            thinking_level: p.thinking_level,
            failover_enabled: p.failover_enabled,
            capabilities: p.capabilities,
        })
        .collect()
}

// ============================================================================
// Health Check (mirrors gateway::health logic without axum dependency)
// ============================================================================

/// Build a three-tier health check JSON value.
fn build_health_check(db: &Database) -> Value {
    let app = json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    });

    let database = match db.get_stats() {
        Ok(stats) => json!({
            "status": "ok",
            "upstream_count": stats.upstream_count,
            "pool_count": stats.pool_count,
        }),
        Err(e) => json!({
            "status": "error",
            "error": e.to_string(),
        }),
    };

    let upstream = match db.get_upstream_status_summary() {
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
    };

    let app_ok = app.get("status").and_then(|v| v.as_str()) == Some("ok");
    let db_ok = database.get("status").and_then(|v| v.as_str()) == Some("ok");
    let upstream_status = upstream
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("ok");

    let overall = if !app_ok || !db_ok {
        "down"
    } else if upstream_status == "down" || upstream_status == "degraded" {
        "degraded"
    } else {
        "ok"
    };

    json!({
        "overall": overall,
        "app": app,
        "database": database,
        "upstreams": upstream,
    })
}

// ============================================================================
// ZIP Export
// ============================================================================

/// Export a diagnostic ZIP package to the given output path.
///
/// The archive contains:
/// - `diagnostic.json` — main summary
/// - `config_summary.json` — settings (masked)
/// - `upstream_status.json` — upstream health
/// - `upstreams.json` — sanitized upstream records
/// - `pools.json` — pool configurations
/// - `recent_logs.json` — last 50 request logs
pub fn export_diagnostic_zip(db: &Database, output_path: &Path) -> Result<(), AppError> {
    // Collect all diagnostic data
    let diagnostic = collect_diagnostic_info(db)?;
    let config_summary = collect_config_summary(db);
    let upstream_status = collect_upstream_status(db);
    let sanitized_upstreams = collect_sanitized_upstreams(db);
    let sanitized_pools = collect_sanitized_pools(db);

    // Collect recent logs
    let filter = crate::db::LogFilter {
        limit: RECENT_LOG_LIMIT,
        offset: 0,
        ..Default::default()
    };
    let recent_logs = db.get_recent_logs(&filter).unwrap_or_else(|e| {
        warn!("Failed to get recent logs for diagnostic: {}", e);
        vec![]
    });

    // Serialize all sections to JSON
    let diagnostic_json = serde_json::to_string_pretty(&diagnostic)
        .map_err(|e| AppError::Internal(format!("序列化诊断信息失败: {}", e)))?;
    let config_json = serde_json::to_string_pretty(&config_summary)
        .map_err(|e| AppError::Internal(format!("序列化配置摘要失败: {}", e)))?;
    let upstream_status_json = serde_json::to_string_pretty(&upstream_status)
        .map_err(|e| AppError::Internal(format!("序列化上游状态失败: {}", e)))?;
    let upstreams_json = serde_json::to_string_pretty(&sanitized_upstreams)
        .map_err(|e| AppError::Internal(format!("序列化上游信息失败: {}", e)))?;
    let pools_json = serde_json::to_string_pretty(&sanitized_pools)
        .map_err(|e| AppError::Internal(format!("序列化号池信息失败: {}", e)))?;
    let logs_json = serde_json::to_string_pretty(&recent_logs)
        .map_err(|e| AppError::Internal(format!("序列化日志失败: {}", e)))?;

    // Create the ZIP archive
    let file = std::fs::File::create(output_path)?;
    let mut zip = zip::ZipWriter::new(file);

    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // Write each section
    zip.start_file("diagnostic.json", options)
        .map_err(|e| AppError::Internal(format!("ZIP 写入失败: {}", e)))?;
    zip.write_all(diagnostic_json.as_bytes())?;

    zip.start_file("config_summary.json", options)
        .map_err(|e| AppError::Internal(format!("ZIP 写入失败: {}", e)))?;
    zip.write_all(config_json.as_bytes())?;

    zip.start_file("upstream_status.json", options)
        .map_err(|e| AppError::Internal(format!("ZIP 写入失败: {}", e)))?;
    zip.write_all(upstream_status_json.as_bytes())?;

    zip.start_file("upstreams.json", options)
        .map_err(|e| AppError::Internal(format!("ZIP 写入失败: {}", e)))?;
    zip.write_all(upstreams_json.as_bytes())?;

    zip.start_file("pools.json", options)
        .map_err(|e| AppError::Internal(format!("ZIP 写入失败: {}", e)))?;
    zip.write_all(pools_json.as_bytes())?;

    zip.start_file("recent_logs.json", options)
        .map_err(|e| AppError::Internal(format!("ZIP 写入失败: {}", e)))?;
    zip.write_all(logs_json.as_bytes())?;

    zip.finish()
        .map_err(|e| AppError::Internal(format!("ZIP 完成失败: {}", e)))?;

    info!("Diagnostic package exported to {:?}", output_path);
    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db
    }

    // ── Masking tests ──────────────────────────────────────────────

    #[test]
    fn test_is_sensitive_key_api_key() {
        assert!(is_sensitive_key("gateway_api_key"));
        assert!(is_sensitive_key("API_KEY"));
        assert!(is_sensitive_key("my_api_key_123"));
    }

    #[test]
    fn test_is_sensitive_key_token() {
        assert!(is_sensitive_key("access_token"));
        assert!(is_sensitive_key("refresh_Token"));
    }

    #[test]
    fn test_is_sensitive_key_secret() {
        assert!(is_sensitive_key("client_secret"));
        assert!(is_sensitive_key("SECRET"));
    }

    #[test]
    fn test_is_sensitive_key_password() {
        assert!(is_sensitive_key("user_password"));
        assert!(is_sensitive_key("PASSWORD"));
    }

    #[test]
    fn test_is_sensitive_key_not_sensitive() {
        assert!(!is_sensitive_key("listen_port"));
        assert!(!is_sensitive_key("log_level"));
        assert!(!is_sensitive_key("rate_limit_enabled"));
        assert!(!is_sensitive_key("probe_interval_seconds"));
    }

    #[test]
    fn test_mask_value_non_empty() {
        let masked = mask_value("sk-abc123xyz");
        assert!(masked.contains("••••••••"));
        assert!(masked.contains("len=12"));
    }

    #[test]
    fn test_mask_value_empty() {
        let masked = mask_value("");
        assert_eq!(masked, "(empty)");
    }

    #[test]
    fn test_mask_setting_sensitive() {
        let (k, v) = mask_setting("gateway_api_key", "sk-gw-abc123");
        assert_eq!(k, "gateway_api_key");
        assert!(v.contains("••••••••"));
        assert!(!v.contains("sk-gw-abc123"));
    }

    #[test]
    fn test_mask_setting_not_sensitive() {
        let (k, v) = mask_setting("listen_port", "47339");
        assert_eq!(k, "listen_port");
        assert_eq!(v, "47339");
    }

    // ── Diagnostic info collection tests ────────────────────────────

    #[test]
    fn test_collect_diagnostic_info_basic() {
        let db = test_db();
        let info = collect_diagnostic_info(&db).unwrap();

        assert_eq!(info.app_version, env!("CARGO_PKG_VERSION"));
        assert!(info.schema_version > 0);
        assert!(!info.generated_at.is_empty());
    }

    #[test]
    fn test_collect_diagnostic_info_has_stats() {
        let db = test_db();
        let info = collect_diagnostic_info(&db).unwrap();

        assert_eq!(info.stats.upstream_count, 0);
        assert_eq!(info.stats.pool_count, 0);
    }

    #[test]
    fn test_collect_diagnostic_info_health_check() {
        let db = test_db();
        let info = collect_diagnostic_info(&db).unwrap();

        let overall = info.health_check.get("overall").unwrap();
        assert_eq!(overall, "ok");

        let app = info.health_check.get("app").unwrap();
        assert_eq!(app.get("status").unwrap(), "ok");

        let database = info.health_check.get("database").unwrap();
        assert_eq!(database.get("status").unwrap(), "ok");
    }

    #[test]
    fn test_collect_config_summary_masks_sensitive() {
        let db = test_db();
        db.save_setting("gateway_api_key", "sk-gw-secret123").unwrap();
        db.save_setting("listen_port", "47339").unwrap();

        let config = collect_config_summary(&db);

        // Find the API key entry
        let api_key_entry = config.iter().find(|(k, _)| k == "gateway_api_key");
        assert!(api_key_entry.is_some());
        let (_, masked_value) = api_key_entry.unwrap();
        assert!(masked_value.contains("••••••••"));
        assert!(!masked_value.contains("sk-gw-secret123"));

        // Non-sensitive value should be unchanged
        let port_entry = config.iter().find(|(k, _)| k == "listen_port");
        assert_eq!(port_entry.unwrap().1, "47339");
    }

    #[test]
    fn test_collect_upstream_status_empty() {
        let db = test_db();
        let status = collect_upstream_status(&db);
        assert!(status.is_empty());
    }

    #[test]
    fn test_collect_upstream_status_with_data() {
        let db = test_db();
        let crypto = crate::crypto::KeyManager::initialize(&std::env::temp_dir()).unwrap();
        let enc = crypto.encrypt_api_key("sk-test").unwrap();
        db.create_upstream(
            "up_test",
            "TestProvider",
            "https://test.com",
            &enc,
            "gpt-4",
            "[]",
            true,
            "",
            "",
        )
        .unwrap();

        let status = collect_upstream_status(&db);
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].provider_name, "TestProvider");
    }

    #[test]
    fn test_collect_sanitized_upstreams_no_api_key() {
        let db = test_db();
        let crypto = crate::crypto::KeyManager::initialize(&std::env::temp_dir()).unwrap();
        let enc = crypto.encrypt_api_key("sk-secret-key").unwrap();
        db.create_upstream(
            "up_test",
            "TestProvider",
            "https://test.com",
            &enc,
            "gpt-4",
            "[]",
            true,
            "",
            "",
        )
        .unwrap();

        let upstreams = collect_sanitized_upstreams(&db);
        assert_eq!(upstreams.len(), 1);

        // Verify no encrypted key data is present
        let json = serde_json::to_string(&upstreams[0]).unwrap();
        assert!(!json.contains("api_key_encrypted"));
        assert!(!json.contains("sk-secret-key"));
        assert!(upstreams[0].has_api_key);
    }

    #[test]
    fn test_collect_sanitized_upstreams_no_key() {
        let db = test_db();
        db.create_upstream(
            "up_empty",
            "EmptyProvider",
            "https://empty.com",
            &[],
            "model",
            "[]",
            true,
            "",
            "",
        )
        .unwrap();

        let upstreams = collect_sanitized_upstreams(&db);
        assert_eq!(upstreams.len(), 1);
        assert!(!upstreams[0].has_api_key);
    }

    #[test]
    fn test_collect_sanitized_pools() {
        let db = test_db();
        db.create_pool("pool_1", "test-pool", "Test Pool", 5, false, "off", "", "")
            .unwrap();

        let pools = collect_sanitized_pools(&db);
        assert_eq!(pools.len(), 1);
        assert_eq!(pools[0].name, "test-pool");
        assert_eq!(pools[0].display_name, "Test Pool");
    }

    // ── ZIP export tests ────────────────────────────────────────────

    #[test]
    fn test_export_diagnostic_zip_creates_file() {
        let db = test_db();
        let temp_dir = tempfile::tempdir().unwrap();
        let zip_path = temp_dir.path().join("diagnostic.zip");

        export_diagnostic_zip(&db, &zip_path).unwrap();

        assert!(zip_path.exists());
        assert!(zip_path.metadata().unwrap().len() > 0);
    }

    #[test]
    fn test_export_diagnostic_zip_contains_files() {
        let db = test_db();
        db.save_setting("gateway_api_key", "sk-gw-secret").unwrap();
        db.save_setting("listen_port", "47339").unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let zip_path = temp_dir.path().join("diagnostic.zip");

        export_diagnostic_zip(&db, &zip_path).unwrap();

        // Open and verify contents
        let file = std::fs::File::open(&zip_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();

        // Verify all expected files are present
        let expected_files = [
            "diagnostic.json",
            "config_summary.json",
            "upstream_status.json",
            "upstreams.json",
            "pools.json",
            "recent_logs.json",
        ];
        for name in &expected_files {
            assert!(archive.file_names().any(|n| n == *name), "Missing {} in zip", name);
        }
    }

    #[test]
    fn test_export_diagnostic_zip_no_plaintext_api_key() {
        let db = test_db();
        let crypto = crate::crypto::KeyManager::initialize(&std::env::temp_dir()).unwrap();
        let enc = crypto.encrypt_api_key("sk-super-secret-key-12345").unwrap();
        db.create_upstream(
            "up_test",
            "TestProvider",
            "https://test.com",
            &enc,
            "gpt-4",
            "[]",
            true,
            "",
            "",
        )
        .unwrap();
        db.save_setting("gateway_api_key", "sk-gw-super-secret").unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let zip_path = temp_dir.path().join("diagnostic.zip");

        export_diagnostic_zip(&db, &zip_path).unwrap();

        // Read the entire zip content and verify no plaintext API key
        let file = std::fs::File::open(&zip_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();

        for i in 0..archive.len() {
            let mut file = archive.by_index(i).unwrap();
            let mut content = String::new();
            use std::io::Read;
            file.read_to_string(&mut content).unwrap();

            assert!(
                !content.contains("sk-super-secret-key-12345"),
                "Plaintext upstream API key found in {}",
                file.name()
            );
            assert!(
                !content.contains("sk-gw-super-secret"),
                "Plaintext gateway API key found in {}",
                file.name()
            );
        }
    }

    #[test]
    fn test_export_diagnostic_zip_has_version_info() {
        let db = test_db();
        let temp_dir = tempfile::tempdir().unwrap();
        let zip_path = temp_dir.path().join("diagnostic.zip");

        export_diagnostic_zip(&db, &zip_path).unwrap();

        let file = std::fs::File::open(&zip_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();

        let mut diag_file = archive.by_name("diagnostic.json").unwrap();
        let mut content = String::new();
        use std::io::Read;
        diag_file.read_to_string(&mut content).unwrap();

        let diag: DiagnosticInfo = serde_json::from_str(&content).unwrap();
        assert_eq!(diag.app_version, env!("CARGO_PKG_VERSION"));
        assert!(diag.schema_version > 0);
    }
}
