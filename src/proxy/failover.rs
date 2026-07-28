use reqwest::Client;
use serde_json::Value;
use tracing::debug;

use crate::proxy::error::{TimeoutPhase, UpstreamError};

/// Forward a request to an upstream provider with round-robin failover.
pub struct UpstreamClient {
    http_client: Client,
}

impl UpstreamClient {
    pub fn new() -> Self {
        let http_client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_else(|e| {
                tracing::error!("Failed to build HTTP client with custom settings, falling back to default: {}", e);
                Client::new()
            });
        Self { http_client }
    }

    /// Forward a single request to the given upstream (non-streaming).
    ///
    /// `extra_headers` allows the caller to inject headers like `X-Request-Id`
    /// into the upstream request for end-to-end tracing.
    ///
    /// Returns `UpstreamError` on failure, which carries structured
    /// error classification for failover decisions.
    pub async fn forward_request(
        &self,
        base_url: &str,
        api_key: &str,
        _model: &str,
        body: &Value,
        timeout_secs: u64,
        extra_headers: Option<&reqwest::header::HeaderMap>,
    ) -> Result<Response, UpstreamError> {
        let url = format!("{}/v1/chat/completions", base_url.trim_end_matches('/'));
        debug!("Forwarding request to upstream: {}", url);

        let mut req = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .json(body);

        if let Some(headers) = extra_headers {
            req = req.headers(headers.clone());
        }

        let response = req.send().await.map_err(|e| {
            if e.is_timeout() {
                UpstreamError::Timeout {
                    phase: TimeoutPhase::ResponseHeaders,
                    timeout_secs,
                }
            } else if e.is_connect() {
                UpstreamError::ConnectionFailed {
                    detail: e.to_string(),
                }
            } else {
                UpstreamError::ConnectionFailed {
                    detail: format!("request error: {}", e),
                }
            }
        })?;

        let status = response.status();
        // Capture headers before consuming the body
        let headers = response.headers().clone();
        let body_bytes = response
            .bytes()
            .await
            .map_err(|e| UpstreamError::ConnectionFailed {
                detail: format!("response body error: {}", e),
            })?;

        if !status.is_success() {
            let body_str = String::from_utf8_lossy(&body_bytes);
            return Err(UpstreamError::from_http_status(
                status.as_u16(),
                &body_str,
            ));
        }

        let json_body: Value = serde_json::from_slice(&body_bytes).unwrap_or(Value::String(
            String::from_utf8_lossy(&body_bytes).to_string(),
        ));

        Ok(Response {
            status_code: status.as_u16() as i32,
            headers,
            body: json_body,
        })
    }

    /// Forward a streaming request and return the raw HTTP response.
    /// The caller is responsible for consuming the response body as a byte stream.
    ///
    /// `timeout_secs` is applied to the initial connection + response headers;
    /// the body stream itself is not bounded here (the caller should apply
    /// idle timeout when reading chunks).
    ///
    /// `extra_headers` allows the caller to inject headers like `X-Request-Id`
    /// into the upstream request for end-to-end tracing.
    pub async fn forward_stream_request(
        &self,
        base_url: &str,
        api_key: &str,
        _model: &str,
        body: &Value,
        timeout_secs: u64,
        extra_headers: Option<&reqwest::header::HeaderMap>,
    ) -> Result<reqwest::Response, UpstreamError> {
        let url = format!("{}/v1/chat/completions", base_url.trim_end_matches('/'));
        debug!("Forwarding streaming request to upstream: {}", url);

        let mut req = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(body);

        if let Some(headers) = extra_headers {
            req = req.headers(headers.clone());
        }

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            req.send(),
        )
        .await
        .map_err(|_| UpstreamError::Timeout {
            phase: TimeoutPhase::ResponseHeaders,
            timeout_secs,
        })?
        .map_err(|e| {
            if e.is_connect() {
                UpstreamError::ConnectionFailed {
                    detail: e.to_string(),
                }
            } else {
                UpstreamError::ConnectionFailed {
                    detail: format!("stream connection error: {}", e),
                }
            }
        })?;

        let status = response.status();
        if !status.is_success() {
            let body_bytes = response.bytes().await.unwrap_or_default();
            let body_str = String::from_utf8_lossy(&body_bytes);
            return Err(UpstreamError::from_http_status(
                status.as_u16(),
                &body_str,
            ));
        }

        Ok(response)
    }
}

impl Default for UpstreamClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Response wrapper from upstream.
#[derive(Debug)]
pub struct Response {
    pub status_code: i32,
    pub headers: reqwest::header::HeaderMap,
    pub body: Value,
}
