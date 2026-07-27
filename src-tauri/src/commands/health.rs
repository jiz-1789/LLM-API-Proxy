use serde::{Deserialize, Serialize};
use tauri::State;

use llm_api_proxy_lib::AppState;

// ============================================================================
// DTO Types
// ============================================================================

/// Result of a health check for a single upstream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResult {
    pub upstream_id: String,
    pub provider_name: String,
    pub status: String,
    pub latency_ms: Option<u64>,
    pub message: Option<String>,
}

// ============================================================================
// Helpers
// ============================================================================

async fn do_health_check(base_url: &str, api_key: &str) -> HealthCheckResult {
    let url = format!("{}/v1/models", base_url.trim_end_matches('/'));
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return HealthCheckResult {
                upstream_id: String::new(),
                provider_name: String::new(),
                status: "error".to_string(),
                latency_ms: None,
                message: Some(format!("创建 HTTP 客户端失败: {}", e)),
            };
        }
    };

    let start = std::time::Instant::now();
    let result = client.get(&url).bearer_auth(api_key).send().await;
    let elapsed = start.elapsed().as_millis() as u64;

    match result {
        Ok(resp) if resp.status().is_success() => HealthCheckResult {
            upstream_id: String::new(),
            provider_name: String::new(),
            status: "ok".to_string(),
            latency_ms: Some(elapsed),
            message: None,
        },
        Ok(resp) => HealthCheckResult {
            upstream_id: String::new(),
            provider_name: String::new(),
            status: "error".to_string(),
            latency_ms: Some(elapsed),
            message: Some(format!("HTTP {}", resp.status())),
        },
        Err(e) if e.is_timeout() => HealthCheckResult {
            upstream_id: String::new(),
            provider_name: String::new(),
            status: "timeout".to_string(),
            latency_ms: Some(elapsed),
            message: Some("连接超时".to_string()),
        },
        Err(e) => HealthCheckResult {
            upstream_id: String::new(),
            provider_name: String::new(),
            status: "error".to_string(),
            latency_ms: None,
            message: Some(e.to_string()),
        },
    }
}

// ============================================================================
// Commands
// ============================================================================

/// Test connectivity to a single upstream by sending a minimal request.
#[tauri::command]
pub async fn check_upstream_health(
    base_url: String,
    api_key: String,
) -> Result<HealthCheckResult, String> {
    Ok(do_health_check(&base_url, &api_key).await)
}

/// Test connectivity to all upstreams in parallel.
#[tauri::command]
pub async fn check_all_upstreams_health(
    state: State<'_, AppState>,
) -> Result<Vec<HealthCheckResult>, String> {
    let upstreams = state.db.get_upstreams().map_err(|e| e.to_string())?;

    let mut handles = Vec::new();
    for u in upstreams {
        let api_key = match state.crypto.decrypt_api_key(&u.api_key_encrypted) {
            Ok(k) => k,
            Err(e) => {
                tracing::warn!("Skipping upstream {}: key decryption failed: {}", u.provider_name, e);
                continue;
            }
        };
        let base_url = u.base_url.clone();
        let upstream_id = u.id.clone();
        let provider_name = u.provider_name.clone();

        handles.push(tokio::spawn(async move {
            let mut result = do_health_check(&base_url, &api_key).await;
            result.upstream_id = upstream_id;
            result.provider_name = provider_name;
            result
        }));
    }

    let mut results = Vec::new();
    for handle in handles {
        if let Ok(result) = handle.await {
            results.push(result);
        }
    }

    Ok(results)
}
