pub mod auth;
pub mod stream;

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
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
use crate::pool::circuit_breaker::CircuitBreaker;
use crate::pool::thinking;
use crate::proxy::failover::UpstreamClient;

/// Per-pool round-robin counters (pool_id → next index).
type RoundRobinCounters = Arc<Mutex<HashMap<String, usize>>>;

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
        circuit_breaker: Arc::new(CircuitBreaker::new()),
        rr_counters: Arc::new(Mutex::new(HashMap::new())),
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
    circuit_breaker: Arc<CircuitBreaker>,
    rr_counters: RoundRobinCounters,
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
/// 2. Circuit breaker: Each upstream is checked against the circuit breaker
///    (threshold & duration from pool config). Open circuits are skipped.
/// 3. Failover: If `failover_enabled` is true, subsequent upstreams are tried
///    on failure. Otherwise only the selected upstream is attempted.
/// 4. Thinking mode: Injected **per-upstream** based on each upstream's
///    `provider_name`, not the first upstream's. A client may override with
///    `"reasoning": false` to disable thinking entirely.
/// 5. Timeout: Uses `pool.timeout_seconds` for non-streaming requests.
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
    info!(
        "Received chat completion request for model={}, stream={}",
        model, is_stream
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

    // Generate a request ID for logging
    let request_id = format!(
        "req_{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
    );

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

        // Ensure circuit breaker is registered with pool-specific config
        state.circuit_breaker.ensure_registered(
            &upstream.id,
            pool.circuit_breaker_threshold as u32,
            pool.circuit_breaker_duration_seconds as u64,
        );

        // Check circuit breaker — skip if open
        if !state.circuit_breaker.allow_request(&upstream.id) {
            warn!(
                "Upstream {} circuit open, skipping (failures={})",
                upstream.provider_name,
                state.circuit_breaker.get_failure_count(&upstream.id)
            );
            failed_upstreams_json.push(json!({
                "provider": upstream.provider_name,
                "error": "circuit breaker open"
            }));
            continue;
        }

        attempted_any = true;

        // Decrypt the API key
        let api_key = match state.crypto.decrypt_api_key(&upstream.api_key_encrypted) {
            Ok(k) => k,
            Err(e) => {
                warn!(
                    "Failed to decrypt API key for {}: {}",
                    upstream.provider_name, e
                );
                failed_upstreams_json.push(json!({
                    "provider": upstream.provider_name,
                    "error": "key decryption failed"
                }));
                continue;
            }
        };

        // Build a fresh request body for this upstream attempt.
        // We clone from the original `body` each time to avoid stale
        // thinking params or model overrides from a previous iteration.
        let mut request_body = body.clone();

        // Override model with the pool-specific model for this upstream
        let target_model = if !pu.model.is_empty() {
            &pu.model
        } else {
            &upstream.selected_model
        };
        if let Some(obj) = request_body.as_object_mut() {
            obj.insert("model".to_string(), Value::String(target_model.to_string()));
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
                    target_model,
                    &request_body,
                )
                .await
            {
                Ok(upstream_response) => {
                    // Record success for circuit breaker
                    state.circuit_breaker.record_success(&upstream.id);

                    let elapsed = start_time.elapsed().as_millis() as i32;

                    // Log successful stream start
                    let log_id =
                        format!("log_{:x}", elapsed as u32 ^ request_id.len() as u32);
                    let _ = state.db.insert_request_log(
                        &log_id,
                        &request_id,
                        Some(&pool.name),
                        Some(&upstream.id),
                        Some(target_model),
                        &serde_json::to_string(&failed_upstreams_json).unwrap_or_default(),
                        "POST",
                        "/v1/chat/completions",
                        upstream_response.status().as_u16() as i32,
                        elapsed,
                        true,
                        0,
                        0,
                        0,
                    );

                    info!(
                        "Streaming from {} (model={}) for pool={}",
                        upstream.provider_name, target_model, pool.display_name
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

                        // After stream completes, update the log with token usage if found
                        if let Some((prompt, completion, total)) = last_usage {
                            let _ = db_clone.update_request_log_tokens(
                                &log_id_clone,
                                prompt,
                                completion,
                                total,
                            );
                        }
                    });

                    // Build streaming SSE response
                    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
                    let body = Body::from_stream(stream);

                    return Response::builder()
                        .status(StatusCode::OK)
                        .header("Content-Type", "text/event-stream")
                        .header("Cache-Control", "no-cache")
                        .header("Connection", "keep-alive")
                        .body(body)
                        .unwrap()
                        .into_response();
                }
                Err(e) => {
                    warn!("Stream upstream {} failed: {}", upstream.provider_name, e);
                    state.circuit_breaker.record_failure(&upstream.id);
                    failed_upstreams_json.push(json!({
                        "provider": upstream.provider_name,
                        "error": e.to_string()
                    }));
                    last_error = Some(e.to_string());
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
                    target_model,
                    &request_body,
                    timeout_secs,
                )
                .await
            {
                Ok(response) => {
                    // Record success for circuit breaker
                    state.circuit_breaker.record_success(&upstream.id);

                    let elapsed = start_time.elapsed().as_millis() as i32;

                    // Replace model name in response with the pool's display name
                    let mut resp_body = response.body;
                    if let Some(obj) = resp_body.as_object_mut() {
                        obj.insert(
                            "model".to_string(),
                            Value::String(pool.display_name.clone()),
                        );
                    }

                    // Extract token usage from response
                    let (prompt_tokens, completion_tokens, total_tokens) = extract_usage(&resp_body);

                    // Log successful request
                    let log_id =
                        format!("log_{:x}", elapsed as u32 ^ request_id.len() as u32);
                    let _ = state.db.insert_request_log(
                        &log_id,
                        &request_id,
                        Some(&pool.name),
                        Some(&upstream.id),
                        Some(target_model),
                        &serde_json::to_string(&failed_upstreams_json).unwrap_or_default(),
                        "POST",
                        "/v1/chat/completions",
                        response.status_code,
                        elapsed,
                        false,
                        prompt_tokens,
                        completion_tokens,
                        total_tokens,
                    );

                    return (StatusCode::OK, Json(resp_body)).into_response();
                }
                Err(e) => {
                    warn!("Upstream {} failed: {}", upstream.provider_name, e);
                    state.circuit_breaker.record_failure(&upstream.id);
                    failed_upstreams_json.push(json!({
                        "provider": upstream.provider_name,
                        "error": e.to_string()
                    }));
                    last_error = Some(e.to_string());
                    continue;
                }
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
    );

    // If we never actually attempted any upstream (all were disabled or
    // circuit-broken), return 503 instead of 502.
    let status = if !attempted_any {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::BAD_GATEWAY
    };

    let error_msg = if !attempted_any {
        "all upstreams are disabled or circuit-broken"
    } else {
        "all upstreams failed"
    };

    (
        status,
        Json(json!({
            "error": error_msg,
            "details": failed_upstreams_json,
            "last_error": last_error
        })),
    )
        .into_response()
}
