pub mod auth;
pub mod stream;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

use crate::crypto::KeyManager;
use crate::db::Database;
use crate::pool::thinking;
use crate::proxy::failover::UpstreamClient;

/// Build the API Gateway router.
pub fn create_router(
    db: Arc<Database>,
    proxy_client: Arc<UpstreamClient>,
    crypto: Arc<KeyManager>,
) -> Router {
    let state = GatewayState {
        db,
        proxy_client,
        crypto,
    };

    Router::new()
        // OpenAI-compatible endpoints
        .route("/v1/models", get(handle_models))
        .route("/v1/chat/completions", post(handle_chat_completions))
        // Health check for monitoring
        .route("/api/health", get(handle_health))
        .with_state(state)
}

#[derive(Clone)]
struct GatewayState {
    db: Arc<Database>,
    proxy_client: Arc<UpstreamClient>,
    crypto: Arc<KeyManager>,
}

/// GET /api/health — Returns gateway status.
async fn handle_health() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "service": "LLM-API-Proxy",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

/// GET /v1/models — Return all pools as available models.
async fn handle_models(State(state): State<GatewayState>) -> impl IntoResponse {
    match state.db.get_pools() {
        Ok(pools) => {
            let data: Vec<Value> = pools
                .iter()
                .map(|pool| {
                    json!({
                        "id": pool.name,
                        "object": "model",
                        "owned_by": "llm-api-proxy"
                    })
                })
                .collect();

            Json(json!({ "data": data })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// POST /v1/chat/completions — Forward to upstream pool with round-robin + failover.
async fn handle_chat_completions(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let start_time = Instant::now();

    // Authenticate Gateway Key
    let _api_key = match auth::validate_api_key(&headers, &state.db) {
        Ok(key) => key,
        Err(e) => return (StatusCode::UNAUTHORIZED, Json(e)).into_response(),
    };

    let model = match body.get("model").and_then(|m| m.as_str()) {
        Some(m) => m.to_string(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "missing model field" })),
            )
                .into_response();
        }
    };

    let is_stream = body.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    info!("Received chat completion request for model={}, stream={}", model, is_stream);

    // Find matching pool by model name
    let pool = match state.db.get_pool_by_name(&model) {
        Ok(Some(p)) => p,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("unknown model: {}", model) })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("database error: {}", e) })),
            )
                .into_response();
        }
    };

    // Get upstreams for this pool (ordered by sort_order)
    let pool_upstreams = match state.db.get_pool_upstreams(&pool.id) {
        Ok(u) => u,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("failed to load pool upstreams: {}", e) })),
            )
                .into_response();
        }
    };

    if pool_upstreams.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "pool has no associated upstreams" })),
        )
            .into_response();
    }

    // Build a mutable request body; replace model with upstream-specific model later
    let mut request_body = body.clone();

    // Inject thinking mode if enabled for this pool
    let upstream_vendor = pool_upstreams
        .first()
        .map(|u| u.provider_name.as_str())
        .unwrap_or("");
    if let Some(thinking_param) = thinking::get_thinking_param(upstream_vendor, pool.thinking_enabled) {
        thinking::merge_thinking_params(&mut request_body, &Some(thinking_param));
    }

    // Generate a request ID for logging
    let request_id = format!(
        "req_{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
    );

    // Try each upstream in order (failover chain)
    let mut last_error: Option<String> = None;
    let mut failed_upstreams_json = Vec::new();

    for pu in &pool_upstreams {
        // Look up full upstream record to get base_url and encrypted api_key
        let upstream = match state.db.get_upstream_by_id(&pu.upstream_id) {
            Ok(Some(u)) => u,
            Ok(None) => {
                warn!("Pool upstream {} references missing upstream {}", pool.name, pu.upstream_id);
                continue;
            }
            Err(e) => {
                warn!("Failed to load upstream {}: {}", pu.upstream_id, e);
                continue;
            }
        };

        if !upstream.enabled {
            continue;
        }

        // Decrypt the API key
        let api_key = match state.crypto.decrypt_api_key(&upstream.api_key_encrypted) {
            Ok(k) => k,
            Err(e) => {
                warn!("Failed to decrypt API key for {}: {}", upstream.provider_name, e);
                failed_upstreams_json.push(json!({
                    "provider": upstream.provider_name,
                    "error": "key decryption failed"
                }));
                continue;
            }
        };

        // Override model in request body with the pool-specific model for this upstream
        let target_model = if !pu.model.is_empty() {
            &pu.model
        } else {
            &upstream.selected_model
        };
        if let Some(obj) = request_body.as_object_mut() {
            obj.insert("model".to_string(), Value::String(target_model.to_string()));
        }

        // Forward the request
        match state
            .proxy_client
            .forward_request(&upstream.base_url, &api_key, target_model, &request_body)
            .await
        {
            Ok(response) => {
                let elapsed = start_time.elapsed().as_millis() as i32;

                // Replace model name in response with the pool's display name
                let mut resp_body = response.body;
                if let Some(obj) = resp_body.as_object_mut() {
                    obj.insert("model".to_string(), Value::String(pool.display_name.clone()));
                }

                // Log successful request
                let log_id = format!("log_{:x}", elapsed as u32 ^ request_id.len() as u32);
                let _ = state.db.insert_request_log(
                    &log_id,
                    &request_id,
                    Some(&pool.name),
                    Some(&upstream.id),
                    &serde_json::to_string(&failed_upstreams_json).unwrap_or_default(),
                    "POST",
                    "/v1/chat/completions",
                    response.status_code,
                    elapsed,
                    is_stream,
                );

                return (StatusCode::OK, Json(resp_body)).into_response();
            }
            Err(e) => {
                warn!("Upstream {} failed: {}", upstream.provider_name, e);
                failed_upstreams_json.push(json!({
                    "provider": upstream.provider_name,
                    "error": e.to_string()
                }));
                last_error = Some(e.to_string());
            }
        }
    }

    // All upstreams exhausted — log the failure
    let elapsed = start_time.elapsed().as_millis() as i32;
    let log_id = format!("log_fail_{:x}", elapsed as u32);
    let _ = state.db.insert_request_log(
        &log_id,
        &request_id,
        Some(&pool.name),
        None,
        &serde_json::to_string(&failed_upstreams_json).unwrap_or_default(),
        "POST",
        "/v1/chat/completions",
        502,
        elapsed,
        is_stream,
    );

    (
        StatusCode::BAD_GATEWAY,
        Json(json!({
            "error": "all upstreams failed",
            "details": failed_upstreams_json,
            "last_error": last_error
        })),
    )
        .into_response()
}
