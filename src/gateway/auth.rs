use axum::http::HeaderMap;
use serde_json::json;

/// Validate the Gateway API Key from request headers.
pub fn validate_api_key(expected_key: &str, headers: &HeaderMap) -> Result<String, serde_json::Value> {
    let auth_header = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !auth_header.starts_with("Bearer ") {
        return Err(json!({ "error": "missing or invalid authorization header" }));
    }

    let provided_key = &auth_header[7..];
    if provided_key == expected_key {
        Ok(provided_key.to_string())
    } else {
        Err(json!({ "error": "invalid API key" }))
    }
}
