use axum::http::HeaderMap;
use dashmap::DashMap;
use serde::Serialize;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::db::Database;

/// Minimum interval between `last_used_at` database updates for the same key.
/// This prevents a write-lock acquisition on every authenticated request,
/// which would serialize concurrent API calls under high load.
const LAST_USED_THROTTLE: Duration = Duration::from_secs(60);

/// Thread-safe throttle map: api_key_id → last DB write time.
/// Prevents redundant UPDATE statements on every auth request.
static LAST_USED_CACHE: std::sync::LazyLock<DashMap<String, Instant>> =
    std::sync::LazyLock::new(DashMap::new);

/// Best-effort update of `last_used_at`, throttled to at most once per
/// `LAST_USED_THROTTLE` per key ID. The first call always writes; subsequent
/// calls within the throttle window are skipped.
fn throttled_update_last_used(db: &Database, key_id: &str) {
    let now = Instant::now();
    let should_write = match LAST_USED_CACHE.get(key_id) {
        Some(last) => now.duration_since(*last) >= LAST_USED_THROTTLE,
        None => true,
    };
    if should_write {
        let _ = db.update_api_key_last_used(key_id);
        LAST_USED_CACHE.insert(key_id.to_string(), now);
    }
}

/// Result of a successful API key authentication.
///
/// Contains information needed by the gateway to enforce pool-level access control.
#[derive(Debug, Clone, Serialize)]
pub struct AuthResult {
    /// The API key string that was validated (for logging).
    pub key: String,
    /// Whether this key has access to all pools (legacy key or key with empty allowed_pools).
    pub all_pools_allowed: bool,
    /// List of pool IDs this key is allowed to access.
    /// Only meaningful when `all_pools_allowed` is false.
    pub allowed_pools: Vec<String>,
}

impl AuthResult {
    /// Check if this auth result grants access to the given pool ID.
    pub fn can_access_pool(&self, pool_id: &str) -> bool {
        if self.all_pools_allowed {
            return true;
        }
        self.allowed_pools.iter().any(|p| p == pool_id)
    }
}

/// Validate the Gateway API Key from request headers.
///
/// Authentication flow (P2-8 multi-key support):
/// 1. Extract the provided key from one of:
///    - `Authorization: Bearer <key>` (OpenAI style)
///    - `x-api-key: <key>` (Anthropic style)
///    - `x-goog-api-key: <key>` (Gemini style)
/// 2. Look up the key in the `api_keys` table
/// 3. If found: check enabled status, check expiration, return AuthResult with pool access
/// 4. If not found in `api_keys`: fall back to legacy `gateway_api_key` setting (all pools)
///
/// # Security
/// Uses constant-time comparison to prevent timing side-channel attacks on the legacy key.
/// The `api_keys` table lookup uses SQLite's indexed UNIQUE constraint, which is not
/// constant-time, but the key space is large enough (32+ hex chars) to make brute-force
/// infeasible.
///
/// # Errors
/// Returns a JSON error object with an `error` field describing the failure.
pub fn validate_api_key(
    headers: &HeaderMap,
    db: &Arc<Database>,
) -> Result<AuthResult, serde_json::Value> {
    let provided_key = extract_api_key(headers);
    let Some(provided_key) = provided_key else {
        return Err(json!({ "error": "missing or invalid authorization header" }));
    };

    // ── Multi-key lookup (P2-8) ──────────────────────────────────────
    // First, check the api_keys table for fine-grained access control.
    if let Some(api_key) = db
        .get_api_key_by_key(provided_key)
        .map_err(|e| json!({ "error": format!("failed to query API key: {}", e) }))?
    {
        // Check if key is enabled
        if !api_key.enabled {
            return Err(json!({ "error": "API key is disabled" }));
        }

        // Check if key has expired
        if let Some(ref expires_at) = api_key.expires_at
            && is_expired(expires_at)
        {
            return Err(json!({ "error": "API key has expired" }));
        }

        // Parse allowed pools from JSON
        let allowed_pools: Vec<String> = serde_json::from_str(&api_key.allowed_pools)
            .unwrap_or_default();

        let all_pools_allowed = allowed_pools.is_empty();

        // Best-effort: update last_used_at timestamp (throttled to avoid
        // write-lock contention on every authenticated request)
        throttled_update_last_used(db, &api_key.id);

        return Ok(AuthResult {
            key: provided_key.to_string(),
            all_pools_allowed,
            allowed_pools,
        });
    }

    // ── Legacy fallback: gateway_api_key setting ─────────────────────
    // If the key is not found in the api_keys table, fall back to the
    // legacy single-key stored in the settings table. This ensures
    // backward compatibility for existing users.
    let expected_key = db
        .get_setting("gateway_api_key")
        .map_err(|e| json!({ "error": format!("failed to load API key: {}", e) }))?
        .unwrap_or_default();

    if !expected_key.is_empty() && constant_time_eq(provided_key.as_bytes(), expected_key.as_bytes()) {
        Ok(AuthResult {
            key: provided_key.to_string(),
            all_pools_allowed: true, // Legacy key always has full access
            allowed_pools: Vec::new(),
        })
    } else {
        Err(json!({ "error": "invalid API key" }))
    }
}

/// Extract the provided API key from request headers.
///
/// Supports three authentication conventions so native clients can talk to
/// the proxy directly:
/// - `Authorization: Bearer <key>` (OpenAI Chat / Responses)
/// - `x-api-key: <key>` (Anthropic Messages)
/// - `x-goog-api-key: <key>` (Gemini Native)
fn extract_api_key(headers: &HeaderMap) -> Option<&str> {
    // 1. Authorization: Bearer
    if let Some(auth) = headers.get("Authorization").and_then(|v| v.to_str().ok())
        && let Some(key) = auth.strip_prefix("Bearer ")
    {
        return Some(key.trim());
    }
    // 2. x-api-key (Anthropic)
    if let Some(key) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        return Some(key.trim());
    }
    // 3. x-goog-api-key (Gemini)
    if let Some(key) = headers.get("x-goog-api-key").and_then(|v| v.to_str().ok()) {
        return Some(key.trim());
    }
    None
}

/// Check if an expiration timestamp has passed.
///
/// Compares against the current local time using SQLite's datetime format
/// (`YYYY-MM-DD HH:MM:SS`). If parsing or format validation fails, the key is
/// treated as NOT expired (fail-open to avoid locking users out due to format issues).
fn is_expired(expires_at: &str) -> bool {
    let trimmed = expires_at.trim();
    // Validate format: must be "YYYY-MM-DD HH:MM:SS" (19 chars)
    if trimmed.len() != 19
        || !trimmed.chars().nth(4).is_some_and(|c| c == '-')
        || !trimmed.chars().nth(7).is_some_and(|c| c == '-')
        || !trimmed.chars().nth(10).is_some_and(|c| c == ' ')
        || !trimmed.chars().nth(13).is_some_and(|c| c == ':')
        || !trimmed.chars().nth(16).is_some_and(|c| c == ':')
    {
        tracing::warn!("Invalid expiration format: '{}', treating as not expired", trimmed);
        return false;
    }
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    // Safe string comparison for ISO 8601 / SQLite datetime format
    now.as_str() > trimmed
}

/// Constant-time comparison to prevent timing side-channel attacks.
///
/// Iterates over the maximum of both slice lengths (padding the shorter
/// with zeros) so that the timing does not reveal whether lengths match.
/// The length difference is tracked via an initial non-zero `result` bit.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let max_len = a.len().max(b.len());
    let mut result: u8 = (a.len() != b.len()) as u8;
    for i in 0..max_len {
        let av = a.get(i).copied().unwrap_or(0);
        let bv = b.get(i).copied().unwrap_or(0);
        result |= av ^ bv;
    }
    result == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_time_eq_equal() {
        assert!(constant_time_eq(b"hello", b"hello"));
    }

    #[test]
    fn test_constant_time_eq_not_equal() {
        assert!(!constant_time_eq(b"hello", b"world"));
    }

    #[test]
    fn test_constant_time_eq_different_length() {
        assert!(!constant_time_eq(b"short", b"longer"));
    }

    #[test]
    fn test_constant_time_eq_empty() {
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn test_auth_result_all_pools_allowed() {
        let result = AuthResult {
            key: "sk-test".to_string(),
            all_pools_allowed: true,
            allowed_pools: vec![],
        };
        assert!(result.can_access_pool("pool_1"));
        assert!(result.can_access_pool("pool_2"));
        assert!(result.can_access_pool("any_pool"));
    }

    #[test]
    fn test_auth_result_specific_pools_allowed() {
        let result = AuthResult {
            key: "sk-test".to_string(),
            all_pools_allowed: false,
            allowed_pools: vec!["pool_a".to_string(), "pool_b".to_string()],
        };
        assert!(result.can_access_pool("pool_a"));
        assert!(result.can_access_pool("pool_b"));
        assert!(!result.can_access_pool("pool_c"));
    }

    #[test]
    fn test_auth_result_empty_allowed_pools_but_not_all() {
        // Edge case: all_pools_allowed is false but allowed_pools is empty.
        // This means no pools are accessible (deny all).
        let result = AuthResult {
            key: "sk-test".to_string(),
            all_pools_allowed: false,
            allowed_pools: vec![],
        };
        assert!(!result.can_access_pool("pool_a"));
    }

    #[test]
    fn test_is_expired_past_date() {
        assert!(is_expired("2020-01-01 00:00:00"));
    }

    #[test]
    fn test_is_expired_future_date() {
        assert!(!is_expired("2099-12-31 23:59:59"));
    }

    #[test]
    fn test_validate_api_key_missing_header() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        let db = Arc::new(db);

        let headers = HeaderMap::new();
        let result = validate_api_key(&headers, &db);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_api_key_no_bearer_prefix() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        let db = Arc::new(db);

        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Basic abc123".parse().unwrap());
        let result = validate_api_key(&headers, &db);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_api_key_legacy_key_success() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("gateway_api_key", "sk-legacy-test").unwrap();
        let db = Arc::new(db);

        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer sk-legacy-test".parse().unwrap());
        let result = validate_api_key(&headers, &db);
        assert!(result.is_ok());
        let auth = result.unwrap();
        assert!(auth.all_pools_allowed);
        assert!(auth.allowed_pools.is_empty());
    }

    #[test]
    fn test_validate_api_key_legacy_key_invalid() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("gateway_api_key", "sk-correct").unwrap();
        let db = Arc::new(db);

        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer sk-wrong".parse().unwrap());
        let result = validate_api_key(&headers, &db);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_api_key_x_api_key_header() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("gateway_api_key", "sk-native-test").unwrap();
        let db = Arc::new(db);

        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", "sk-native-test".parse().unwrap());
        let result = validate_api_key(&headers, &db);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_api_key_x_goog_api_key_header() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("gateway_api_key", "sk-gemini-test").unwrap();
        let db = Arc::new(db);

        let mut headers = HeaderMap::new();
        headers.insert("x-goog-api-key", "sk-gemini-test".parse().unwrap());
        let result = validate_api_key(&headers, &db);
        assert!(result.is_ok());
    }

    #[test]
    fn test_extract_api_key_prefers_bearer() {
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer sk-bearer".parse().unwrap());
        headers.insert("x-api-key", "sk-xapi".parse().unwrap());
        assert_eq!(extract_api_key(&headers), Some("sk-bearer"));
    }

    #[test]
    fn test_validate_api_key_multi_key_success() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.create_api_key("ak_1", "sk-multi-test", "测试", "[]", None)
            .unwrap();
        let db = Arc::new(db);

        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer sk-multi-test".parse().unwrap());
        let result = validate_api_key(&headers, &db);
        assert!(result.is_ok());
        let auth = result.unwrap();
        assert!(auth.all_pools_allowed); // empty allowed_pools = all pools
    }

    #[test]
    fn test_validate_api_key_multi_key_with_pool_restriction() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.create_api_key(
            "ak_1",
            "sk-restricted",
            "受限",
            "[\"pool_a\",\"pool_b\"]",
            None,
        )
        .unwrap();
        let db = Arc::new(db);

        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer sk-restricted".parse().unwrap());
        let result = validate_api_key(&headers, &db).unwrap();
        assert!(!result.all_pools_allowed);
        assert!(result.can_access_pool("pool_a"));
        assert!(result.can_access_pool("pool_b"));
        assert!(!result.can_access_pool("pool_c"));
    }

    #[test]
    fn test_validate_api_key_disabled_key() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.create_api_key("ak_1", "sk-disabled", "已禁用", "[]", None)
            .unwrap();
        db.toggle_api_key("ak_1", false).unwrap();
        let db = Arc::new(db);

        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer sk-disabled".parse().unwrap());
        let result = validate_api_key(&headers, &db);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(
            err.get("error").and_then(|v| v.as_str()),
            Some("API key is disabled")
        );
    }

    #[test]
    fn test_validate_api_key_expired_key() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.create_api_key(
            "ak_1",
            "sk-expired",
            "已过期",
            "[]",
            Some("2020-01-01 00:00:00"),
        )
        .unwrap();
        let db = Arc::new(db);

        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer sk-expired".parse().unwrap());
        let result = validate_api_key(&headers, &db);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(
            err.get("error").and_then(|v| v.as_str()),
            Some("API key has expired")
        );
    }

    #[test]
    fn test_validate_api_key_multi_key_takes_precedence_over_legacy() {
        // If a key exists in both api_keys table and settings,
        // the api_keys table entry should take precedence.
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("gateway_api_key", "sk-shared").unwrap();
        db.create_api_key("ak_1", "sk-shared", "多密钥", "[\"pool_x\"]", None)
            .unwrap();
        let db = Arc::new(db);

        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer sk-shared".parse().unwrap());
        let result = validate_api_key(&headers, &db).unwrap();
        // Should use the api_keys table entry (restricted to pool_x),
        // not the legacy setting (all pools)
        assert!(!result.all_pools_allowed);
        assert!(result.can_access_pool("pool_x"));
        assert!(!result.can_access_pool("pool_y"));
    }

    #[test]
    fn test_validate_api_key_updates_last_used() {
        // Use a unique key ID to avoid interference with the global
        // LAST_USED_CACHE that persists across tests in the same process.
        let unique_id = "ak_last_used_test_unique";
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.create_api_key(unique_id, "sk-usage", "使用追踪", "[]", None)
            .unwrap();

        let key_before = db.get_api_key_by_id(unique_id).unwrap().unwrap();
        assert!(key_before.last_used_at.is_none());

        let db = Arc::new(db);
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer sk-usage".parse().unwrap());
        let _ = validate_api_key(&headers, &db).unwrap();

        let key_after = db.get_api_key_by_id(unique_id).unwrap().unwrap();
        assert!(key_after.last_used_at.is_some());
    }

    #[test]
    fn test_throttled_update_skips_repeated_calls() {
        // Use a unique key ID to avoid interference with the global
        // LAST_USED_CACHE that persists across tests in the same process.
        let unique_id = "ak_throttle_test_unique";
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.create_api_key(unique_id, "sk-throttle", "节流测试", "[]", None)
            .unwrap();

        // First call should write to DB
        throttled_update_last_used(&db, unique_id);
        let ts1 = db.get_api_key_by_id(unique_id).unwrap().unwrap();
        assert!(ts1.last_used_at.is_some());
        let first_ts = ts1.last_used_at.unwrap();

        // Second call within throttle window should be skipped
        // (timestamp should not change)
        throttled_update_last_used(&db, unique_id);
        let ts2 = db.get_api_key_by_id(unique_id).unwrap().unwrap();
        assert_eq!(ts2.last_used_at.as_deref(), Some(first_ts.as_str()));
    }
}
