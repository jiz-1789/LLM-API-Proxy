use reqwest::Client;
use serde_json::Value;
use tracing::debug;

use crate::proxy::error::{TimeoutPhase, UpstreamError};
use crate::proxy::url_util::build_chat_completions_url;

/// Connect timeout for establishing a TCP connection to the upstream.
///
/// This is the ONLY timeout applied to upstream requests: it guards against
/// unreachable/hanging upstreams during connection setup. No request-level
/// total timeout is imposed, so upstream models that think for a long time
/// before returning (e.g. DeepSeek R1, Claude thinking) are never cut off.
///
/// Set to 60 seconds to tolerate network jitter and slow handshakes without
/// false "unreachable" judgements during fluctuation.
const CONNECT_TIMEOUT_SECS: u64 = 60;

/// Forward a request to an upstream provider with round-robin failover.
pub struct UpstreamClient {
    http_client: Client,
}

impl UpstreamClient {
    pub fn new() -> Self {
        let http_client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|e| {
                tracing::error!("Failed to build HTTP client with custom settings, falling back to default: {}", e);
                Client::new()
            });
        Self { http_client }
    }

    /// Forward a single request to the given upstream (non-streaming).
    ///
    /// No request-level timeout is applied: the request waits indefinitely for
    /// the upstream to respond, so long-thinking models are never interrupted.
    /// Only the TCP connect timeout guards against unreachable upstreams.
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
        extra_headers: Option<&reqwest::header::HeaderMap>,
    ) -> Result<Response, UpstreamError> {
        let url = build_chat_completions_url(base_url);
        debug!("Forwarding request to upstream: {}", url);

        let mut req = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(body);

        if let Some(headers) = extra_headers {
            req = req.headers(headers.clone());
        }

        let response = req.send().await.map_err(|e| {
            if e.is_timeout() {
                UpstreamError::Timeout {
                    phase: TimeoutPhase::Connect,
                    timeout_secs: CONNECT_TIMEOUT_SECS,
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
            let raw_body = serde_json::from_slice::<Value>(&body_bytes).ok();
            return Err(UpstreamError::from_http_status_with_body(
                status.as_u16(),
                &body_str,
                raw_body,
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
    /// No request-level timeout is applied when waiting for response headers,
    /// so upstream models that think for a long time before emitting the first
    /// SSE chunk are never cut off. Only the TCP connect timeout guards
    /// against unreachable upstreams; the caller should apply an idle timeout
    /// when reading chunks to guard against a hung stream.
    ///
    /// `extra_headers` allows the caller to inject headers like `X-Request-Id`
    /// into the upstream request for end-to-end tracing.
    pub async fn forward_stream_request(
        &self,
        base_url: &str,
        api_key: &str,
        _model: &str,
        body: &Value,
        extra_headers: Option<&reqwest::header::HeaderMap>,
    ) -> Result<reqwest::Response, UpstreamError> {
        let url = build_chat_completions_url(base_url);
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

        let response = req.send().await.map_err(|e| {
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
            let raw_body = serde_json::from_slice::<Value>(&body_bytes).ok();
            return Err(UpstreamError::from_http_status_with_body(
                status.as_u16(),
                &body_str,
                raw_body,
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
