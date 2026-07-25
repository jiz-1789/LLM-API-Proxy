use axum::http::HeaderMap;
use serde_json::json;
use std::sync::Arc;

use crate::db::Database;

/// Validate the Gateway API Key from request headers.
/// Loads the expected key from the settings table (key = "gateway_api_key").
/// Falls back to "sk-gateway-key" if no key is configured.
pub fn validate_api_key(
    headers: &HeaderMap,
    db: &Arc<Database>,
) -> Result<String, serde_json::Value> {
    let auth_header = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !auth_header.starts_with("Bearer ") {
        return Err(json!({ "error": "missing or invalid authorization header" }));
    }

    let provided_key = &auth_header[7..];

    let expected_key = db
        .get_setting("gateway_api_key")
        .unwrap_or(None)
        .unwrap_or_else(|| "sk-gateway-key".to_string());

    if provided_key == expected_key {
        Ok(provided_key.to_string())
    } else {
        Err(json!({ "error": "invalid API key" }))
    }
}
