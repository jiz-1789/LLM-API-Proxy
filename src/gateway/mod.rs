use crate::config::GatewaySettings;
use axum::{
    extract::{State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response, Sse},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::{error, info, warn};

use crate::db::Database;
use crate::gateway::auth;
use crate::pool::thinking;
use crate::proxy::client::UpstreamClient;
use crate::proxy::model_filter;

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
                .map(|(_, name, display_name, _, _)| {
                    json!({
                        "id": name,
                        "object": "model",
                        "owned_by": "llm-api-proxy"
                    })
                })
                .collect();

            Json(json!({ "data": data }))
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
    let api_key = match auth::validate_api_key(&headers, &state.gateway_settings().api_key) {
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

    // Find matching pool
    let pool = match state.db.get_pools() {
        Ok(pools) => pools.iter().find(|(_, name, _, _, _)| *name == model).cloned(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("database error: {}", e) })),
            )
                .into_response();
        }
    };

    let (_, pool_id, display_name, _, thinking_enabled) = match pool {
        Some(p) => p,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("unknown model: {}", model) })),
            )
                .into_response();
        }
    };

    // Get upstreams for this pool
    let upstreams = match state.db.get_pool_upstreams(&pool_id) {
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
    if let Some(thinking_param) = thinking::get_thinking_param("UnknownVendor", thinking_enabled) {
        thinking::merge_thinking_params(&mut request_body, &Some(thinking_param));
    }

    // Try each upstream in order
    let mut last_error: Option<String> = None;
    let mut failed_upstreams = Vec::new();

    for (upstream_id, provider_name, _) in &upstreams {
        let upstream_config = match state.proxy_client.upstream_config(*upstream_id) {
            Some(config) => config,
            None => continue,
        };

        match state.proxy_client.forward_request(
            &upstream_config.base_url,
            &upstream_config.api_key,
            &upstream_config.selected_model,
            &request_body,
        ) {
            Ok(response) => {
                // Replace model name in response
                if let Some(ref mut json_body) = response.body.as_json_object_mut() {
                    model_filter::replace_model_name(json_body, &display_name);
                }

                return (StatusCode::OK, Json(response.body)).into_response();
            }
            Err(e) => {
                warn!("Upstream {} failed: {}", provider_name, e);
                state.proxy_client.record_failure(*upstream_id);
                failed_upstreams.push(json!({
                    "provider": provider_name,
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
