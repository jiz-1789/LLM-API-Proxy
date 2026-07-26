use reqwest::Client;
use serde_json::Value;
use tracing::debug;

use crate::error::AppError;

/// Forward a request to an upstream provider with round-robin failover.
pub struct UpstreamClient {
    http_client: Client,
}

impl UpstreamClient {
    pub fn new() -> Self {
        Self {
            http_client: Client::builder()
                .connect_timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    /// Forward a single request to the given upstream (non-streaming).
    pub async fn forward_request(
        &self,
        base_url: &str,
        api_key: &str,
        _model: &str,
        body: &Value,
        timeout_secs: u64,
    ) -> Result<Response, AppError> {
        let url = format!("{}/v1/chat/completions", base_url.trim_end_matches('/'));
        debug!("Forwarding request to upstream: {}", url);

        let response = self.http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .json(body)
            .send()
            .await
            .map_err(|e| AppError::UpstreamFailed(format!("connection error: {}", e)))?;

        let status = response.status();
        let body_bytes = response.bytes().await
            .map_err(|e| AppError::UpstreamFailed(format!("response body error: {}", e)))?;

        if !status.is_success() {
            return Err(AppError::UpstreamFailed(format!(
                "HTTP {}: {}",
                status,
                String::from_utf8_lossy(&body_bytes)
            )));
        }

        let json_body: Value = serde_json::from_slice(&body_bytes).unwrap_or(Value::String(
            String::from_utf8_lossy(&body_bytes).to_string(),
        ));

        Ok(Response {
            status_code: status.as_u16() as i32,
            headers: reqwest::header::HeaderMap::new(),
            body: json_body,
        })
    }

    /// Forward a streaming request and return the raw HTTP response.
    /// The caller is responsible for consuming the response body as a byte stream.
    /// A 60-second timeout is applied to the initial connection + response headers;
    /// the body stream itself is not bounded (streams may be long-lived).
    pub async fn forward_stream_request(
        &self,
        base_url: &str,
        api_key: &str,
        _model: &str,
        body: &Value,
    ) -> Result<reqwest::Response, AppError> {
        let url = format!("{}/v1/chat/completions", base_url.trim_end_matches('/'));
        debug!("Forwarding streaming request to upstream: {}", url);

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            self.http_client
                .post(&url)
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json")
                .json(body)
                .send(),
        )
        .await
        .map_err(|_| AppError::UpstreamFailed("stream request timed out (60s)".to_string()))?
        .map_err(|e| AppError::UpstreamFailed(format!("stream connection error: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body_bytes = response.bytes().await.unwrap_or_default();
            return Err(AppError::UpstreamFailed(format!(
                "HTTP {}: {}",
                status,
                String::from_utf8_lossy(&body_bytes)
            )));
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
