//! 统一上游错误模型。
//!
//! 将所有上游故障场景归类为结构化错误类型，替代原来模糊的
//! `AppError::UpstreamFailed(String)`，使故障转移判定和错误响应
//! 映射有据可依。

use crate::error::AppError;

/// 超时发生的阶段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeoutPhase {
    /// 连接上游服务器超时
    Connect,
    /// 等待响应头超时（请求已发出但上游未响应）
    ResponseHeaders,
    /// 流式响应中 chunk 间空闲超时
    ResponseBody,
}

impl std::fmt::Display for TimeoutPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TimeoutPhase::Connect => write!(f, "connect"),
            TimeoutPhase::ResponseHeaders => write!(f, "response_headers"),
            TimeoutPhase::ResponseBody => write!(f, "response_body"),
        }
    }
}

/// 上游错误分类。
///
/// 判定 `should_failover()` 的核心规则：
/// - **4xx 客户端错误**（AuthFailed / ClientError / ResponseFormatError）：
///   不触发故障转移，因为是请求本身的问题，换一个上游也会同样失败。
/// - **5xx 服务端错误 / 网络错误 / 超时**：触发故障转移。
/// - **EmbeddedError**（HTTP 200 但 body 含 error）：触发故障转移。
#[derive(Debug, Clone)]
pub enum UpstreamError {
    /// 网络连接失败（DNS 解析失败、TCP 连接拒绝等）
    ConnectionFailed { detail: String },

    /// 请求超时
    Timeout {
        phase: TimeoutPhase,
        timeout_secs: u64,
    },

    /// 上游认证失败（401 / 403）
    AuthFailed { status: u16, detail: String },

    /// 上游客户端错误（400 / 404 / 422 等，不含 401/403/429）
    ClientError {
        status: u16,
        detail: String,
    },

    /// 上游服务端错误（500 / 502 / 503 等）
    ServerError {
        status: u16,
        detail: String,
    },

    /// 响应体格式错误（JSON 解析失败等）
    ResponseFormatError { detail: String },

    /// HTTP 200 但响应体包含 `error` 字段（"假成功"）
    EmbeddedError { message: String },

    /// API Key 解密失败
    KeyDecryptionFailed { detail: String },
}

impl UpstreamError {
    /// 判断此错误是否应触发故障转移（尝试下一个上游）。
    ///
    /// **不触发故障转移**的情况：
    /// - `AuthFailed`：API Key 无效，换一个上游也可能无效（但实际中
    ///   不同上游的 Key 不同，所以这里**仍然触发**故障转移）
    /// - `ClientError`：请求格式有问题，换上游也一样
    /// - `ResponseFormatError`：响应格式异常，可能是上游实现差异
    ///
    /// **触发故障转移**的情况：
    /// - `ConnectionFailed` / `Timeout`：网络问题，换上游可能恢复
    /// - `ServerError`：上游服务端故障
    /// - `EmbeddedError`：上游返回了错误内容
    /// - `KeyDecryptionFailed`：解密失败，可能该上游 Key 配置有误
    pub fn should_failover(&self) -> bool {
        match self {
            // 4xx 客户端错误：请求本身的问题，不故障转移
            UpstreamError::ClientError { .. } => false,
            UpstreamError::ResponseFormatError { .. } => false,
            // 其余全部故障转移
            UpstreamError::ConnectionFailed { .. } => true,
            UpstreamError::Timeout { .. } => true,
            UpstreamError::AuthFailed { .. } => true,
            UpstreamError::ServerError { .. } => true,
            UpstreamError::EmbeddedError { .. } => true,
            UpstreamError::KeyDecryptionFailed { .. } => true,
        }
    }

    /// 返回人类可读的错误摘要（用于日志和 `failed_upstreams` JSON）。
    pub fn error_summary(&self) -> String {
        match self {
            UpstreamError::ConnectionFailed { detail } => {
                format!("connection failed: {}", detail)
            }
            UpstreamError::Timeout { phase, timeout_secs } => {
                format!("timeout in {} phase ({}s)", phase, timeout_secs)
            }
            UpstreamError::AuthFailed { status, detail } => {
                format!("auth failed ({}): {}", status, detail)
            }
            UpstreamError::ClientError { status, detail } => {
                format!("client error ({}): {}", status, detail)
            }
            UpstreamError::ServerError { status, detail } => {
                format!("server error ({}): {}", status, detail)
            }
            UpstreamError::ResponseFormatError { detail } => {
                format!("response format error: {}", detail)
            }
            UpstreamError::EmbeddedError { message } => {
                format!("embedded error: {}", message)
            }
            UpstreamError::KeyDecryptionFailed { detail } => {
                format!("key decryption failed: {}", detail)
            }
        }
    }

    /// 从 HTTP 状态码和响应体推断上游错误类型。
    ///
    /// 用于将 `failover.rs` 中原始的 HTTP 错误转换为结构化 `UpstreamError`。
    pub fn from_http_status(status: u16, body: &str) -> UpstreamError {
        let detail = if body.is_empty() {
            format!("HTTP {}", status)
        } else {
            // 截断过长的错误体，避免日志和 JSON 爆炸
            let truncated = if body.len() > 500 {
                format!("{}...", &body[..500])
            } else {
                body.to_string()
            };
            format!("HTTP {}: {}", status, truncated)
        };

        match status {
            401 | 403 => UpstreamError::AuthFailed { status, detail },
            400..=499 => UpstreamError::ClientError { status, detail },
            500..=599 => UpstreamError::ServerError { status, detail },
            _ => UpstreamError::ServerError {
                status,
                detail: format!("unexpected status: {}", detail),
            },
        }
    }
}

impl std::fmt::Display for UpstreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error_summary())
    }
}

impl std::error::Error for UpstreamError {}

impl From<UpstreamError> for AppError {
    fn from(err: UpstreamError) -> Self {
        AppError::UpstreamFailed(err.error_summary())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── should_failover 判定测试 ──────────────────────────────

    #[test]
    fn test_connection_failed_should_failover() {
        let err = UpstreamError::ConnectionFailed {
            detail: "dns resolution failed".to_string(),
        };
        assert!(err.should_failover());
    }

    #[test]
    fn test_timeout_should_failover() {
        let err = UpstreamError::Timeout {
            phase: TimeoutPhase::Connect,
            timeout_secs: 15,
        };
        assert!(err.should_failover());
    }

    #[test]
    fn test_auth_failed_should_failover() {
        let err = UpstreamError::AuthFailed {
            status: 401,
            detail: "invalid api key".to_string(),
        };
        assert!(err.should_failover());
    }

    #[test]
    fn test_client_error_should_not_failover() {
        let err = UpstreamError::ClientError {
            status: 400,
            detail: "bad request".to_string(),
        };
        assert!(!err.should_failover());
    }

    #[test]
    fn test_server_error_should_failover() {
        let err = UpstreamError::ServerError {
            status: 500,
            detail: "internal server error".to_string(),
        };
        assert!(err.should_failover());
    }

    #[test]
    fn test_embedded_error_should_failover() {
        let err = UpstreamError::EmbeddedError {
            message: "rate limited by provider".to_string(),
        };
        assert!(err.should_failover());
    }

    #[test]
    fn test_response_format_error_should_not_failover() {
        let err = UpstreamError::ResponseFormatError {
            detail: "invalid json".to_string(),
        };
        assert!(!err.should_failover());
    }

    #[test]
    fn test_key_decryption_failed_should_failover() {
        let err = UpstreamError::KeyDecryptionFailed {
            detail: "aes decryption failed".to_string(),
        };
        assert!(err.should_failover());
    }

    // ── from_http_status 测试 ─────────────────────────────

    #[test]
    fn test_from_http_status_401() {
        let err = UpstreamError::from_http_status(401, "unauthorized");
        assert!(matches!(err, UpstreamError::AuthFailed { status: 401, .. }));
    }

    #[test]
    fn test_from_http_status_400() {
        let err = UpstreamError::from_http_status(400, "bad request");
        assert!(matches!(err, UpstreamError::ClientError { status: 400, .. }));
    }

    #[test]
    fn test_from_http_status_500() {
        let err = UpstreamError::from_http_status(500, "internal error");
        assert!(matches!(err, UpstreamError::ServerError { status: 500, .. }));
    }

    #[test]
    fn test_from_http_status_503() {
        let err = UpstreamError::from_http_status(503, "service unavailable");
        assert!(matches!(err, UpstreamError::ServerError { status: 503, .. }));
    }

    #[test]
    fn test_from_http_status_empty_body() {
        let err = UpstreamError::from_http_status(500, "");
        let summary = err.error_summary();
        assert!(summary.contains("HTTP 500"));
    }

    #[test]
    fn test_from_http_status_long_body_truncated() {
        let long_body = "x".repeat(1000);
        let err = UpstreamError::from_http_status(500, &long_body);
        let summary = err.error_summary();
        assert!(summary.contains("..."));
        assert!(summary.len() < 600);
    }

    // ── error_summary 测试 ─────────────────────────────────────

    #[test]
    fn test_timeout_summary_includes_phase() {
        let err = UpstreamError::Timeout {
            phase: TimeoutPhase::ResponseHeaders,
            timeout_secs: 60,
        };
        let summary = err.error_summary();
        assert!(summary.contains("response_headers"));
        assert!(summary.contains("60s"));
    }

    // ── Display trait 测试 ─────────────────────────────────────

    #[test]
    fn test_display_uses_error_summary() {
        let err = UpstreamError::ConnectionFailed {
            detail: "test".to_string(),
        };
        assert_eq!(format!("{}", err), "connection failed: test");
    }

    // ── TimeoutPhase Display 测试 ──────────────────────────────

    #[test]
    fn test_timeout_phase_display() {
        assert_eq!(format!("{}", TimeoutPhase::Connect), "connect");
        assert_eq!(
            format!("{}", TimeoutPhase::ResponseHeaders),
            "response_headers"
        );
        assert_eq!(
            format!("{}", TimeoutPhase::ResponseBody),
            "response_body"
        );
    }
}
