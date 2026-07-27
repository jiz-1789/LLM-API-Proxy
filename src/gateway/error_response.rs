//! OpenAI 兼容的统一错误响应构建器。
//!
//! 所有网关错误响应都通过此模块构建，确保格式一致：
//! ```json
//! {
//!   "error": {
//!     "message": "...",
//!     "type": "...",
//!     "code": "...",
//!     "param": null
//!   }
//! }
//! ```

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

/// 构建 OpenAI 兼容的错误响应。
///
/// 参数：
/// - `status`: HTTP 状态码
/// - `message`: 错误描述（用户可见）
/// - `error_type`: 错误类型（如 `rate_limit_error`、`invalid_request_error`）
/// - `code`: 错误代码（如 `rate_limit_exceeded`、`model_not_found`）
/// - `extra_headers`: 额外的响应头（如 `Retry-After`、`X-Request-Id`）
pub fn error_response(
    status: StatusCode,
    message: &str,
    error_type: &str,
    code: &str,
    extra_headers: Option<HeaderMap>,
) -> Response {
    let body = json!({
        "error": {
            "message": message,
            "type": error_type,
            "code": code,
            "param": null
        }
    });

    let mut response = (status, Json(body)).into_response();

    if let Some(headers) = extra_headers {
        for (key, value) in headers.iter() {
            if let Ok(v) = value.to_str()
                && let Ok(hv) = HeaderValue::from_str(v)
            {
                response.headers_mut().insert(key, hv);
            }
        }
    }

    response
}

/// 429 限流错误响应，附带 `Retry-After` 头。
pub fn rate_limit_exceeded(retry_after_secs: u64) -> Response {
    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&retry_after_secs.to_string()) {
        headers.insert("retry-after", v);
    }

    error_response(
        StatusCode::TOO_MANY_REQUESTS,
        "Rate limit exceeded. Please try again later.",
        "rate_limit_error",
        "rate_limit_exceeded",
        Some(headers),
    )
}

/// 401 认证失败错误响应。
pub fn authentication_error(message: &str) -> Response {
    error_response(
        StatusCode::UNAUTHORIZED,
        message,
        "authentication_error",
        "invalid_api_key",
        None,
    )
}

/// 400 请求格式错误响应。
pub fn invalid_request(message: &str, code: &str) -> Response {
    error_response(
        StatusCode::BAD_REQUEST,
        message,
        "invalid_request_error",
        code,
        None,
    )
}

/// 404 模型未找到错误响应。
pub fn model_not_found(model: &str) -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        &format!("The model '{}' does not exist", model),
        "invalid_request_error",
        "model_not_found",
        None,
    )
}

/// 502 所有上游失败错误响应。
pub fn all_upstreams_failed(details: &[Value], last_error: Option<&str>) -> Response {
    let message = "All upstream providers failed to process the request";
    let mut body = json!({
        "error": {
            "message": message,
            "type": "api_error",
            "code": "all_upstreams_failed",
            "param": null
        }
    });

    if let Some(obj) = body.get_mut("error").and_then(|e| e.as_object_mut()) {
        obj.insert(
            "details".to_string(),
            Value::Array(details.to_vec()),
        );
        if let Some(err) = last_error {
            obj.insert("last_error".to_string(), Value::String(err.to_string()));
        }
    }

    (StatusCode::BAD_GATEWAY, Json(body)).into_response()
}

/// 503 无可用上游错误响应。
pub fn no_available_upstream(reason: &str) -> Response {
    let code = if reason.contains("disabled") {
        "all_upstreams_disabled"
    } else {
        "no_available_upstream"
    };

    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        reason,
        "api_error",
        code,
        None,
    )
}

/// 500 内部服务器错误响应。
pub fn internal_error(message: &str) -> Response {
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        message,
        "api_error",
        "internal_error",
        None,
    )
}
