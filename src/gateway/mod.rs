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
use tracing::{info, warn};

use crate::db::Database;
use crate::pool::thinking;
use crate::proxy::failover::UpstreamClient;

/// Build the API Gateway router.
pub fn create_router(db: Arc<Database>, proxy_client: Arc<UpstreamClient>) -> Router {
    let state = GatewayState { db, proxy_client };

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

    info!(
        "Received chat completion request for model={}, stream={}",
        model,
        body.get("stream").and_then(|s| s.as_bool()).unwrap_or(false)
    );

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

    // Get upstreams for this pool
    let upstreams = match state.db.get_pool_upstreams(&pool.id) {
        Ok(upstreams) => upstreams,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("failed to load pool upstreams: {}", e) })),
            )
                .into_response();
        }
    };

    if upstreams.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "pool has no available upstreams" })),
        )
            .into_response();
    }

    // Inject thinking mode if enabled
    let mut request_body = body.clone();
    let upstream_vendor = upstreams
        .first()
        .map(|u| u.provider_name.as_str())
        .unwrap_or("");
    if let Some(thinking_param) =
        thinking::get_thinking_param(upstream_vendor, pool.thinking_enabled)
    {
        thinking::merge_thinking_params(&mut request_body, &Some(thinking_param));
    }

    // Try each upstream in order (failover)
    let mut last_error: Option<String> = None;
    let mut failed_upstreams = Vec::new();

    for upstream in &upstreams {
        match state
            .proxy_client
            .forward_request("", "", "", &request_body)
            .await
        {
            Ok(response) => {
                // Replace model name in response
                let mut resp_body = response.body;
                if let Some(obj) = resp_body.as_object_mut() {
                    obj.insert(
                        "model".to_string(),
                        Value::String(pool.display_name.clone()),
                    );
                }
                return (StatusCode::OK, Json(resp_body)).into_response();
            }
            Err(e) => {
                warn!("Upstream {} failed: {}", upstream.provider_name, e);
                failed_upstreams.push(json!({
                    "provider": upstream.provider_name,
                    "error": e.to_string()
                }));
                last_error = Some(e.to_string());
            }
        }
    }

    // All upstreams exhausted
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({
            "error": "all upstreams failed",
            "details": failed_upstreams,
            "last_error": last_error
        })),
    )
        .into_response()
}
