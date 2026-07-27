pub mod auth;
pub mod error_response;
pub mod rate_limit;
pub mod stream;

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::io::StreamReader;
use tracing::{info, warn};

use crate::crypto::KeyManager;
use crate::db::Database;
use crate::pool::thinking;
use crate::proxy::error::UpstreamError;
use crate::proxy::failover::UpstreamClient;

use rate_limit::{RateLimitConfig, RateLimiter};

/// Per-pool round-robin counters (pool_id → next index).
type RoundRobinCounters = Arc<Mutex<HashMap<String, usize>>>;

/// Build the API Gateway router.
///
/// `rate_limit_config` is loaded from the settings table at startup;
/// changes take effect on next server restart.
pub fn create_router(
    db: Arc<Database>,
    proxy_client: Arc<UpstreamClient>,
    crypto: Arc<KeyManager>,
    rate_limit_config: RateLimitConfig,
) -> Router {
    let rate_limiter = RateLimiter::new(rate_limit_config);
    let state = GatewayState {
        db,
        proxy_client,
        crypto,
        rr_counters: Arc::new(Mutex::new(HashMap::new())),
        rate_limiter,
    };

    Router::new()
        // OpenAI-compatible endpoints
        .route("/v1/models", get(handle_models))
        .route("/v1/chat/completions", post(handle_chat_completions))
        // Health check for monitoring
        .route("/api/health", get(handle_health))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit_middleware,
        ))
        .with_state(state)
}

/// Rate limiting middleware: checks if client IP is within rate limits.
///
/// IP identification strategy is controlled by `trust_forwarded_for`:
/// - `false` (default): uses TCP connection's `remote_addr` (direct mode)
/// - `true`: takes the rightmost IP from `X-Forwarded-For` (reverse proxy mode)
///
/// Returns 429 with `Retry-After` header when rate limited.
async fn rate_limit_middleware(
    State(state): State<GatewayState>,
    req: axum::http::Request<Body>,
    next: Next,
) -> Response {
    let trust_xff = state.rate_limiter.config().trust_forwarded_for;
    let client_ip = rate_limit::extract_client_ip_from_request(&req, trust_xff);

    match state.rate_limiter.check(&client_ip) {
        Ok(()) => next.run(req).await,
        Err(retry_after_secs) => {
            warn!(
                "Rate limit exceeded for client {} (retry after {}s)",
                client_ip, retry_after_secs
            );
            error_response::rate_limit_exceeded(retry_after_secs)
        }
    }
}

#[derive(Clone)]
struct GatewayState {
    db: Arc<Database>,
    proxy_client: Arc<UpstreamClient>,
    crypto: Arc<KeyManager>,
    rr_counters: RoundRobinCounters,
    rate_limiter: RateLimiter,
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
        Err(e) => error_response::internal_error(&format!("database error: {}", e)),
    }
}

/// Extract token usage from an OpenAI-compatible response body.
/// Returns (prompt_tokens, completion_tokens, total_tokens).
/// Returns (0, 0, 0) if usage is not present (e.g. streaming or error responses).
fn extract_usage(resp: &serde_json::Value) -> (i64, i64, i64) {
    let usage = resp.get("usage");
    if let Some(usage) = usage {
        let prompt = usage.get("prompt_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
        let completion = usage.get("completion_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
        let total = usage.get("total_tokens").and_then(|v| v.as_i64()).unwrap_or(prompt + completion);
        (prompt, completion, total)
    } else {
        (0, 0, 0)
    }
}

/// POST /v1/chat/completions — Forward to upstream pool with round-robin + failover.
///
/// Routing logic:
/// 1. Round-robin: If `round_robin_strategy == "round_robin"`, rotate the
///    starting upstream per request. Otherwise always start from index 0.
/// 2. Failover: If `failover_enabled` is true, subsequent upstreams are tried
///    on failure. Otherwise only the selected upstream is attempted.
/// 3. Thinking mode: Injected **per-upstream** based on each upstream's
///    `provider_name`, not the first upstream's. A client may override with
///    `"reasoning": false` to disable thinking entirely.
/// 4. Timeout: Uses `pool.timeout_seconds` for non-streaming requests.
async fn handle_chat_completions(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let start_time = Instant::now();

    // Authenticate Gateway Key
    let _api_key = match auth::validate_api_key(&headers, &state.db) {
        Ok(key) => key,
        Err(e) => {
            let msg = e.get("error").and_then(|v| v.as_str()).unwrap_or("authentication failed");
            return error_response::authentication_error(msg);
        }
    };

    let model = match body.get("model").and_then(|m| m.as_str()) {
        Some(m) => m.to_string(),
        None => {
            return error_response::invalid_request("Missing required field: model", "missing_model");
        }
    };

    let is_stream = body.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    info!(
        "Received chat completion request for model={}, stream={}",
        model, is_stream
    );

    // Find matching pool by model name
    let pool = match state.db.get_pool_by_name(&model) {
        Ok(Some(p)) => p,
        Ok(None) => {
            return error_response::model_not_found(&model);
        }
        Err(e) => {
            return error_response::internal_error(&format!("database error: {}", e));
        }
    };

    // Get upstreams for this pool (ordered by sort_order)
    let pool_upstreams = match state.db.get_pool_upstreams(&pool.id) {
        Ok(u) => u,
        Err(e) => {
            return error_response::internal_error(&format!("failed to load pool upstreams: {}", e));
        }
    };

    if pool_upstreams.is_empty() {
        return error_response::no_available_upstream("pool has no associated upstreams");
    }

    // ── Pre-request configuration ──────────────────────────────────────

    // Check if client explicitly disabled reasoning/thinking
    let client_reasoning_disabled =
        body.get("reasoning").and_then(|v| v.as_bool()) == Some(false);

    // Pool-level timeout for non-streaming requests
    let timeout_secs = if pool.timeout_seconds > 0 {
        pool.timeout_seconds as u64
    } else {
        60
    };

    // Determine round-robin starting index
    let n = pool_upstreams.len();
    let start_idx = if pool.round_robin_strategy == "round_robin" {
        let mut counters = state.rr_counters.lock().unwrap();
        let counter = counters.entry(pool.id.clone()).or_insert(0);
        let idx = *counter % n;
        *counter = (*counter + 1) % n;
        idx
    } else {
        0 // sequential: always start from first upstream
    };

    // Number of upstreams to try
    let max_attempts = if pool.failover_enabled { n } else { 1 };

    // Generate a request ID for logging (UUID to avoid collisions)
    let request_id = format!("req_{}", uuid::Uuid::new_v4().simple());

    // ── Failover loop ──────────────────────────────────────────────────

    let mut last_error: Option<String> = None;
    let mut failed_upstreams_json: Vec<Value> = Vec::new();
    let mut attempted_any = false;

    for attempt in 0..max_attempts {
        let idx = (start_idx + attempt) % n;
        let pu = &pool_upstreams[idx];

        // Look up full upstream record to get base_url and encrypted api_key
        let upstream = match state.db.get_upstream_by_id(&pu.upstream_id) {
            Ok(Some(u)) => u,
            Ok(None) => {
                warn!(
                    "Pool upstream {} references missing upstream {}",
                    pool.name, pu.upstream_id
                );
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

        attempted_any = true;

        // Decrypt the API key
        let api_key = match state.crypto.decrypt_api_key(&upstream.api_key_encrypted) {
            Ok(k) => k,
            Err(e) => {
                let upstream_err = UpstreamError::KeyDecryptionFailed {
                    detail: e.to_string(),
                };
                warn!(
                    "Failed to decrypt API key for {}: {}",
                    upstream.provider_name, e
                );
                failed_upstreams_json.push(json!({
                    "provider": upstream.provider_name,
                    "model": upstream.selected_model,
                    "error": upstream_err.error_summary()
                }));
                // Key decryption failure should failover to next upstream
                continue;
            }
        };

        // Build a fresh request body for this upstream attempt.
        // We clone from the original `body` each time to avoid stale
        // thinking params or model overrides from a previous iteration.
        let mut request_body = body.clone();

        // Override model with the pool-specific model for this upstream.
        // The model field may contain comma-separated values (multi-select);
        // round-robin across the available models for load balancing.
        let models: Vec<&str> = if !pu.model.is_empty() {
            pu.model.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect()
        } else {
            Vec::new()
        };
        let model_str = if models.is_empty() {
            upstream.selected_model.as_str()
        } else if models.len() == 1 {
            models[0]
        } else {
            // Round-robin across models: use a composite key of pool_id + upstream_id
            let key = format!("{}:{}", pool.id, pu.upstream_id);
            let mut counters = state.rr_counters.lock().unwrap();
            let counter = counters.entry(key).or_insert(0);
            let idx = *counter % models.len();
            *counter = (*counter + 1) % models.len();
            models[idx]
        };
        if let Some(obj) = request_body.as_object_mut() {
            obj.insert("model".to_string(), Value::String(model_str.to_string()));
        }

        // Inject thinking params **per-upstream** based on this upstream's
        // provider_name. Skip if the client explicitly set reasoning=false.
        if !client_reasoning_disabled && pool.thinking_enabled {
            if let Some(thinking_param) =
                thinking::get_thinking_param(&upstream.provider_name, true)
            {
                thinking::merge_thinking_params(&mut request_body, &Some(thinking_param));
            }
        }

        // For streaming requests, ensure we get usage info in the final chunk
        if is_stream {
            if let Some(obj) = request_body.as_object_mut() {
                obj.insert(
                    "stream_options".to_string(),
                    json!({ "include_usage": true }),
                );
            }
        }

        // ── SSE streaming path ──────────────────────────────────────
        if is_stream {
            match state
                .proxy_client
                .forward_stream_request(
                    &upstream.base_url,
                    &api_key,
                    model_str,
                    &request_body,
                )
                .await
            {
                Ok(upstream_response) => {
                    let elapsed = start_time.elapsed().as_millis() as i32;

                    // Log successful stream start
                    let log_id = format!("log_{}", uuid::Uuid::new_v4().simple());
                    if let Err(e) = state.db.insert_request_log(
                        &log_id,
                        &request_id,
                        Some(&pool.name),
                        Some(&upstream.id),
                        Some(model_str),
                        &serde_json::to_string(&failed_upstreams_json).unwrap_or_default(),
                        "POST",
                        "/v1/chat/completions",
                        upstream_response.status().as_u16() as i32,
                        elapsed,
                        true,
                        0,
                        0,
                        0,
                    ) {
                        warn!("Failed to insert request log: {}", e);
                    }

                    info!(
                        "Streaming from {} (model={}) for pool={}",
                        upstream.provider_name, model_str, pool.display_name
                    );

                    // Build byte stream from upstream response
                    let byte_stream = upstream_response.bytes_stream();
                    let display_name = pool.display_name.clone();
                    let db_clone = state.db.clone();
                    let log_id_clone = log_id.clone();
                    let (tx, rx) =
                        tokio::sync::mpsc::channel::<Result<Bytes, std::convert::Infallible>>(64);

                    // Spawn a task to read lines, replace model, extract usage, and forward chunks
                    tokio::spawn(async move {
                        use tokio_stream::StreamExt;
                        let mapped_stream = byte_stream
                            .map(|result| result.map_err(|e| std::io::Error::other(e)));
                        let stream_reader = StreamReader::new(mapped_stream);
                        let reader = BufReader::new(stream_reader);
                        let mut lines = reader.lines();
                        let mut last_usage: Option<(i64, i64, i64)> = None;
                        let mut stream_error: Option<String> = None;
                        while let Ok(Some(line)) = lines.next_line().await {
                            let output = if let Some(json_str) = line.strip_prefix("data: ") {
                                let trimmed = json_str.trim();
                                if trimmed == "[DONE]" {
                                    "data: [DONE]\n\n".to_string()
                                } else {
                                    // Single-pass: parse JSON once, replace model + extract usage
                                    let (chunk, usage) = stream::process_sse_chunk(trimmed, &display_name);
                                    if let Some(u) = usage {
                                        last_usage = Some(u);
                                    }
                                    // Detect error in stream chunk
                                    if stream_error.is_none() {
                                        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                                            if let Some(err) = v.get("error") {
                                                let msg = if let Some(m) = err.get("message").and_then(|m| m.as_str()) {
                                                    m.to_string()
                                                } else {
                                                    err.to_string()
                                                };
                                                stream_error = Some(msg);
                                            }
                                        }
                                    }
                                    chunk
                                }
                            } else if line.is_empty() || line.starts_with(':') {
                                // Skip blank separators and SSE comments
                                continue;
                            } else {
                                format!("{}\n\n", line)
                            };
                            if tx.send(Ok(Bytes::from(output))).await.is_err() {
                                break; // client disconnected
                            }
                        }

                        // After stream completes, update the log:
                        // - If an error was detected mid-stream, update status to 500
                        // - Update token usage if found
                        if stream_error.is_some() {
                            if let Err(e) = db_clone.update_request_log_status(&log_id_clone, 500) {
                                warn!("Failed to update request log status: {}", e);
                            }
                        }
                        if let Some((prompt, completion, total)) = last_usage {
                            if let Err(e) = db_clone.update_request_log_tokens(
                                &log_id_clone,
                                prompt,
                                completion,
                                total,
                            ) {
                                warn!("Failed to update request log tokens: {}", e);
                            }
                        }
                    });

                    // Build streaming SSE response
                    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
                    let body = Body::from_stream(stream);

                    match Response::builder()
                        .status(StatusCode::OK)
                        .header("Content-Type", "text/event-stream")
                        .header("Cache-Control", "no-cache")
                        .header("Connection", "keep-alive")
                        .body(body)
                    {
                        Ok(resp) => return resp.into_response(),
                        Err(e) => {
                            warn!("Failed to build stream response: {}", e);
                            return error_response::internal_error("failed to build response");
                        }
                    }
                }
                Err(e) => {
                    warn!("Stream upstream {} failed: {}", upstream.provider_name, e);
                    failed_upstreams_json.push(json!({
                        "provider": upstream.provider_name,
                        "model": model_str,
                        "error": e.error_summary()
                    }));
                    last_error = Some(e.error_summary());
                    if !e.should_failover() {
                        break;
                    }
                    continue;
                }
            }
        } else {
            // ── Non-streaming path ──────────────────────────────────
            match state
                .proxy_client
                .forward_request(
                    &upstream.base_url,
                    &api_key,
                    model_str,
                    &request_body,
                    timeout_secs,
                )
                .await
            {
                Ok(response) => {
                    let elapsed = start_time.elapsed().as_millis() as i32;

                    // Replace model name in response with the pool's display name
                    let mut resp_body = response.body;
                    if let Some(obj) = resp_body.as_object_mut() {
                        obj.insert(
                            "model".to_string(),
                            Value::String(pool.display_name.clone()),
                        );
                    }

                    // Check if the response body contains an error field.
                    // Some providers return HTTP 200 but embed an error in the body.
                    if let Some(err_obj) = resp_body.get("error") {
                        let err_msg = if let Some(msg) = err_obj.get("message").and_then(|m| m.as_str()) {
                            msg.to_string()
                        } else {
                            err_obj.to_string()
                        };
                        warn!("Upstream {} returned error in body: {}", upstream.provider_name, err_msg);
                        let embedded_err = UpstreamError::EmbeddedError { message: err_msg };
                        failed_upstreams_json.push(json!({
                            "provider": upstream.provider_name,
                            "model": model_str,
                            "error": embedded_err.error_summary()
                        }));
                        last_error = Some(embedded_err.error_summary());
                        // Embedded errors should failover
                        continue;
                    }

                    // Extract token usage from response
                    let (prompt_tokens, completion_tokens, total_tokens) = extract_usage(&resp_body);

                    // Log successful request
                    let log_id = format!("log_{}", uuid::Uuid::new_v4().simple());
                    if let Err(e) = state.db.insert_request_log(
                        &log_id,
                        &request_id,
                        Some(&pool.name),
                        Some(&upstream.id),
                        Some(model_str),
                        &serde_json::to_string(&failed_upstreams_json).unwrap_or_default(),
                        "POST",
                        "/v1/chat/completions",
                        response.status_code,
                        elapsed,
                        false,
                        prompt_tokens,
                        completion_tokens,
                        total_tokens,
                    ) {
                        warn!("Failed to insert request log: {}", e);
                    }

                    return (StatusCode::OK, Json(resp_body)).into_response();
                }
                Err(e) => {
                    warn!("Upstream {} failed: {}", upstream.provider_name, e);
                    failed_upstreams_json.push(json!({
                        "provider": upstream.provider_name,
                        "model": model_str,
                        "error": e.error_summary()
                    }));
                    last_error = Some(e.error_summary());
                    if !e.should_failover() {
                        break;
                    }
                    continue;
                }
            }
        }
    }

    // All upstreams exhausted — log the failure
    let elapsed = start_time.elapsed().as_millis() as i32;
    let log_id = format!("log_fail_{}", uuid::Uuid::new_v4().simple());
    if let Err(e) = state.db.insert_request_log(
        &log_id,
        &request_id,
        Some(&pool.name),
        None,
        None,
        &serde_json::to_string(&failed_upstreams_json).unwrap_or_default(),
        "POST",
        "/v1/chat/completions",
        502,
        elapsed,
        is_stream,
        0,
        0,
        0,
    ) {
        warn!("Failed to insert failure request log: {}", e);
    }

    // If we never actually attempted any upstream (all were disabled),
    // return 503 instead of 502.
    if !attempted_any {
        return error_response::no_available_upstream("all upstreams are disabled");
    }

    error_response::all_upstreams_failed(&failed_upstreams_json, last_error.as_deref())
}
