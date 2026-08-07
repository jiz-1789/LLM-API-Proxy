pub mod auth;
pub mod convert;
pub mod error_response;
pub mod health;
pub mod rate_limit;
pub mod stream;

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
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
use crate::pool::thinking::ThinkingLevel;
use crate::proxy::error::UpstreamError;
use crate::proxy::failover::UpstreamClient;

use rate_limit::{RateLimitConfig, RateLimiter};

/// Per-pool round-robin counters (pool_id 鈫?next index).
type RoundRobinCounters = Arc<Mutex<HashMap<String, usize>>>;

/// Default stream idle timeout: if no SSE chunk is received within this
/// duration, the stream is considered stalled and will be terminated.
///
/// Set to 300s (5 minutes) to tolerate long-thinking models that may stay
/// silent for extended periods, while still guarding against a truly hung
/// stream.
const DEFAULT_STREAM_IDLE_TIMEOUT_SECS: u64 = 300;

/// Consecutive failure threshold for marking an upstream as "down".
/// When `failure_count` reaches this value, status changes from "degraded" to "down".
const UPSTREAM_FAILURE_THRESHOLD: i32 = 3;

/// Header prefixes from upstream responses that should be passed through
/// to the client. All other headers are filtered out for security and
/// consistency.
const PASSTHROUGH_HEADER_PREFIXES: &[&str] = &[
    "x-ratelimit-",
    "openai-",
    "anthropic-",
];

/// Filter upstream response headers through the passthrough whitelist.
///
/// Only headers matching `PASSTHROUGH_HEADER_PREFIXES` are retained.
/// This prevents leaking upstream-internal headers (like `Server`,
/// `Set-Cookie`, etc.) to the client.
fn filter_passthrough_headers(headers: &HeaderMap) -> HeaderMap {
    let mut filtered = HeaderMap::new();
    for (key, value) in headers.iter() {
        let key_str = key.as_str();
        if PASSTHROUGH_HEADER_PREFIXES
            .iter()
            .any(|prefix| key_str.starts_with(prefix))
        {
            filtered.insert(key.clone(), value.clone());
        }
    }
    filtered
}

/// Attach `X-Request-Id` header to a response for end-to-end tracing.
fn with_request_id(mut resp: Response, trace_id: &str) -> Response {
    if let Ok(v) = HeaderValue::from_str(trace_id) {
        resp.headers_mut().insert("x-request-id", v);
    }
    resp
}

/// Build a `HeaderMap` containing only the `X-Request-Id` header for
/// injecting into upstream requests.
fn build_trace_headers(trace_id: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(trace_id) {
        headers.insert("x-request-id", v);
    }
    headers
}

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
    // Load persisted rate limit state from database (P2-15)
    rate_limiter.load_from_db(&db);
    // Start background persistence task (P2-15)
    rate_limiter.clone().start_persist_task(db.clone());
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
                client_ip = %client_ip,
                retry_after = retry_after_secs,
                "Rate limit exceeded"
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

/// GET /api/health 鈥?Returns three-tier health check (app + database + upstreams).
async fn handle_health(State(state): State<GatewayState>) -> impl IntoResponse {
    health::health_response(&state.db)
}

/// GET /v1/models 鈥?Return all pools as available models.
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

/// POST /v1/chat/completions 鈥?Forward to upstream pool with round-robin + failover.
///
/// Routing logic:
/// 1. Round-robin: If `round_robin_strategy == "round_robin"`, rotate the
///    starting upstream per request. Otherwise always start from index 0.
/// 2. Failover: If `failover_enabled` is true, subsequent upstreams are tried
///    on failure. Otherwise only the selected upstream is attempted.
/// 3. Thinking mode: Injected **per-upstream** based on each upstream's
///    `provider_name`, not the first upstream's. A client may override with
///    `"reasoning": false` to disable thinking entirely.
/// 4. Timeout: No request-level timeout is applied to upstream requests, so
///    long-thinking models are never cut off. Only a TCP connect timeout guards
///    against unreachable upstreams; streaming additionally applies an idle
///    timeout between chunks to detect a hung stream.
/// 5. Trace ID: A UUID-based `trace_id` is generated per request, sent to
///    upstream as `X-Request-Id` header, and returned to the client in the
///    response header for end-to-end tracing.
async fn handle_chat_completions(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let start_time = Instant::now();

    // Generate trace ID for end-to-end request tracing.
    // This ID is: (a) sent to upstream as X-Request-Id header,
    // (b) included in structured log fields, (c) returned to client
    // in the response header, and (d) used as request_id in the DB log.
    let trace_id = uuid::Uuid::new_v4().simple().to_string();
    let request_id = format!("req_{}", trace_id);
    let trace_headers = build_trace_headers(&trace_id);

    // Authenticate Gateway Key (P2-8: multi-key with pool access control)
    let auth_result = match auth::validate_api_key(&headers, &state.db) {
        Ok(auth) => auth,
        Err(e) => {
            let msg = e.get("error").and_then(|v| v.as_str()).unwrap_or("authentication failed");
            return with_request_id(error_response::authentication_error(msg), &trace_id);
        }
    };

    let model = match body.get("model").and_then(|m| m.as_str()) {
        Some(m) => m.to_string(),
        None => {
            return with_request_id(
                error_response::invalid_request("Missing required field: model", "missing_model"),
                &trace_id,
            );
        }
    };

    let is_stream = body.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    info!(
        trace_id = %trace_id,
        model = %model,
        stream = is_stream,
        "Received chat completion request"
    );

    // Find matching pool by model name
    let pool = match state.db.get_pool_by_name(&model) {
        Ok(Some(p)) => p,
        Ok(None) => {
            return with_request_id(error_response::model_not_found(&model), &trace_id);
        }
        Err(e) => {
            return with_request_id(
                error_response::internal_error(&format!("database error: {}", e)),
                &trace_id,
            );
        }
    };

    // Get upstreams for this pool (ordered by sort_order)
    let pool_upstreams = match state.db.get_pool_upstreams(&pool.id) {
        Ok(u) => u,
        Err(e) => {
            return with_request_id(
                error_response::internal_error(&format!("failed to load pool upstreams: {}", e)),
                &trace_id,
            );
        }
    };

    // P2-8: Check if the authenticated API key has access to this pool.
    // Legacy keys (from settings) and keys with empty allowed_pools have full access.
    if !auth_result.can_access_pool(&pool.id) {
        return with_request_id(
            error_response::forbidden("This API key does not have access to the requested model"),
            &trace_id,
        );
    }

    if pool_upstreams.is_empty() {
        return with_request_id(
            error_response::no_available_upstream("pool has no associated upstreams"),
            &trace_id,
        );
    }

    // 鈹€鈹€ Pre-request configuration 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€

    // Check if client explicitly disabled reasoning/thinking
    let client_reasoning_disabled =
        body.get("reasoning").and_then(|v| v.as_bool()) == Some(false);

    // Determine round-robin starting index
    let n = pool_upstreams.len();
    let start_idx = if pool.round_robin_strategy == "round_robin" {
        let mut counters = state.rr_counters.lock().unwrap_or_else(|e| {
            warn!("rr_counters mutex poisoned, recovering");
            e.into_inner()
        });
        let counter = counters.entry(pool.id.clone()).or_insert(0);
        let idx = *counter % n;
        *counter = (*counter + 1) % n;
        info!(
            trace_id = %trace_id,
            pool = %pool.name,
            pool_id = %pool.id,
            strategy = %pool.round_robin_strategy,
            failover_enabled = pool.failover_enabled,
            upstream_count = n,
            counter_before = idx,
            counter_after = *counter,
            start_idx = idx,
            "Round-robin: selected starting upstream index"
        );
        idx
    } else {
        info!(
            trace_id = %trace_id,
            pool = %pool.name,
            strategy = %pool.round_robin_strategy,
            upstream_count = n,
            "Non-round-robin strategy: always starting from index 0"
        );
        0 // sequential: always start from first upstream
    };

    // Number of upstreams to try
    let max_attempts = if pool.failover_enabled { n } else { 1 };

    // 鈹€鈹€ Failover loop 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€

    let mut last_error: Option<String> = None;
    let mut failed_upstreams_json: Vec<Value> = Vec::new();
    let mut attempted_any = false;

    for attempt in 0..max_attempts {
        let idx = (start_idx + attempt) % n;
        let pu = &pool_upstreams[idx];

        info!(
            trace_id = %trace_id,
            pool = %pool.name,
            attempt = attempt,
            max_attempts = max_attempts,
            idx = idx,
            start_idx = start_idx,
            upstream_id = %pu.upstream_id,
            "Failover loop: attempting upstream"
        );

        // Look up full upstream record to get base_url and encrypted api_key
        let upstream = match state.db.get_upstream_by_id(&pu.upstream_id) {
            Ok(Some(u)) => u,
            Ok(None) => {
                warn!(
                    trace_id = %trace_id,
                    pool = %pool.name,
                    upstream_id = %pu.upstream_id,
                    "Pool upstream references missing upstream"
                );
                continue;
            }
            Err(e) => {
                warn!(
                    trace_id = %trace_id,
                    upstream_id = %pu.upstream_id,
                    error = %e,
                    "Failed to load upstream"
                );
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
                    trace_id = %trace_id,
                    provider = %upstream.provider_name,
                    error = %e,
                    "Failed to decrypt API key"
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
            let mut counters = state.rr_counters.lock().unwrap_or_else(|e| {
                warn!("rr_counters mutex poisoned, recovering");
                e.into_inner()
            });
            let counter = counters.entry(key).or_insert(0);
            let idx = *counter % models.len();
            *counter = (*counter + 1) % models.len();
            models[idx]
        };
        if let Some(obj) = request_body.as_object_mut() {
            obj.insert("model".to_string(), Value::String(model_str.to_string()));
        }

        // Inject thinking params **per-upstream** based on this upstream's
        // provider_name and the configured thinking level (pool level, or the
        // per-upstream override if set). Skip if the client explicitly set
        // reasoning=false.
        if !client_reasoning_disabled {
            let pool_level = ThinkingLevel::parse(&pool.thinking_level);
            let upstream_level = if pu.thinking_level_override.is_empty() {
                pool_level
            } else {
                ThinkingLevel::parse(&pu.thinking_level_override)
            };
            let thinking_params = thinking::get_thinking_params(
                &upstream.provider_name,
                &upstream_level,
                &pool.thinking_custom_params,
            );
            thinking::merge_thinking_params(&mut request_body, &thinking_params);
        }

        // For streaming requests, ensure we get usage info in the final chunk
        if is_stream
            && let Some(obj) = request_body.as_object_mut()
        {
            obj.insert(
                "stream_options".to_string(),
                json!({ "include_usage": true }),
            );
        }

        // Convert request body to the upstream's native API format if needed.
        let upstream_format = upstream.api_format.as_str();
        if convert::needs_request_conversion(upstream_format) {
            match convert::convert_request_to_upstream(&request_body, upstream_format) {
                Ok(converted) => request_body = converted,
                Err(e) => {
                    warn!(
                        trace_id = %trace_id,
                        provider = %upstream.provider_name,
                        api_format = upstream_format,
                        error = %e,
                        "Failed to convert request to upstream format"
                    );
                    failed_upstreams_json.push(json!({
                        "provider": upstream.provider_name,
                        "model": model_str,
                        "error": format!("request conversion failed: {}", e)
                    }));
                    last_error = Some(format!("request conversion failed: {}", e));
                    continue;
                }
            }
        }

        // 鈹€鈹€ SSE streaming path 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
        if is_stream {
            match state
                .proxy_client
                .forward_stream_request(
                    &upstream.base_url,
                    &api_key,
                    model_str,
                    &upstream.api_format,
                    &request_body,
                    Some(&trace_headers),
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
                        warn!(trace_id = %trace_id, error = %e, "Failed to insert request log");
                    }

                    info!(
                        trace_id = %trace_id,
                        provider = %upstream.provider_name,
                        model = %model_str,
                        pool = %pool.display_name,
                        "Streaming from upstream"
                    );

                    // Update upstream health: successful stream start
                    if let Err(e) = state.db.update_upstream_health(&upstream.id, true, None, UPSTREAM_FAILURE_THRESHOLD) {
                        warn!(trace_id = %trace_id, error = %e, "Failed to update upstream health after success");
                    }

                    // Build byte stream from upstream response
                    let byte_stream = upstream_response.bytes_stream();
                    let display_name = pool.display_name.clone();
                    let upstream_format = upstream.api_format.clone();
                    let db_clone = state.db.clone();
                    let log_id_clone = log_id.clone();
                    let trace_id_clone = trace_id.clone();
                    let (tx, rx) =
                        tokio::sync::mpsc::channel::<Result<Bytes, std::convert::Infallible>>(64);

                    // Spawn a task to read lines, replace model, extract usage, and forward chunks.
                    // Applies an idle timeout: if no chunk is received within
                    // DEFAULT_STREAM_IDLE_TIMEOUT_SECS, the stream is terminated.
                    tokio::spawn(async move {
                        use tokio_stream::StreamExt;
                        let mapped_stream = byte_stream
                            .map(|result| result.map_err(std::io::Error::other));
                        let stream_reader = StreamReader::new(mapped_stream);
                        let reader = BufReader::new(stream_reader);
                        let mut lines = reader.lines();
                        let mut last_usage: Option<(i64, i64, i64)> = None;
                        let mut stream_error: Option<String> = None;
                        let use_native_converter = convert::needs_request_conversion(&upstream_format);
                        let mut native_converter = convert::NativeStreamConverter::new(&upstream_format);
                        let idle_timeout = std::time::Duration::from_secs(DEFAULT_STREAM_IDLE_TIMEOUT_SECS);

                        loop {
                            // Wrap next_line() with an idle timeout.
                            // If no data arrives within the timeout, terminate the stream.
                            match tokio::time::timeout(idle_timeout, lines.next_line()).await {
                                Ok(Ok(Some(line))) => {
                                    let mut outputs: Vec<String> = Vec::new();
                                    if use_native_converter {
                                        let converted = native_converter.process(&line, &display_name);
                                        if let Some(u) = converted.usage {
                                            last_usage = Some(u);
                                        }
                                        if stream_error.is_none() && converted.error.is_some() {
                                            stream_error = converted.error.clone();
                                        }
                                        outputs = converted.lines;
                                        if converted.done {
                                            for out in outputs {
                                                if tx.send(Ok(Bytes::from(out))).await.is_err() {
                                                    break;
                                                }
                                            }
                                            break;
                                        }
                                    } else if let Some(json_str) = line.strip_prefix("data: ") {
                                        let trimmed = json_str.trim();
                                        if trimmed == "[DONE]" {
                                            outputs.push("data: [DONE]\n\n".to_string());
                                        } else {
                                            // Single-pass: parse JSON once, replace model,
                                            // extract usage, and detect errors
                                            let (chunk, usage, error_msg) =
                                                stream::process_sse_chunk(trimmed, &display_name);
                                            if let Some(u) = usage {
                                                last_usage = Some(u);
                                            }
                                            if stream_error.is_none()
                                                && let Some(msg) = error_msg
                                            {
                                                stream_error = Some(msg);
                                            }
                                            outputs.push(chunk);
                                        }
                                    } else if line.is_empty() || line.starts_with(':') {
                                        // Skip blank separators and SSE comments
                                        continue;
                                    } else {
                                        outputs.push(format!("{}\n\n", line));
                                    }
                                    for out in outputs {
                                        if tx.send(Ok(Bytes::from(out))).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                                Ok(Ok(None)) => break, // stream ended
                                Ok(Err(e)) => {
                                    warn!(
                                        trace_id = %trace_id_clone,
                                        error = %e,
                                        "Error reading stream line"
                                    );
                                    break;
                                }
                                Err(_) => {
                                    warn!(
                                        trace_id = %trace_id_clone,
                                        idle_timeout_secs = DEFAULT_STREAM_IDLE_TIMEOUT_SECS,
                                        "Stream idle timeout — no data received, terminating stream"
                                    );
                                    break;
                                }
                            }
                        }

                        // Fall back to converter-accumulated usage if the chunk-based
                        // extraction missed it (native formats report usage in control events).
                        if last_usage.is_none() && use_native_converter {
                            let (p, c, t) = native_converter.final_usage();
                            if p > 0 || c > 0 {
                                last_usage = Some((p, c, t));
                            }
                        }

                        // After stream completes, update the log:
                        // - If an error was detected mid-stream, update status to 500
                        // - Update token usage if found
                        if stream_error.is_some()
                            && let Err(e) = db_clone.update_request_log_status(&log_id_clone, 500)
                        {
                            warn!(trace_id = %trace_id_clone, error = %e, "Failed to update request log status");
                        }
                        if let Some((prompt, completion, total)) = last_usage
                            && let Err(e) = db_clone.update_request_log_tokens(
                                &log_id_clone,
                                prompt,
                                completion,
                                total,
                            )
                        {
                            warn!(trace_id = %trace_id_clone, error = %e, "Failed to update request log tokens");
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
                        .header("X-Request-Id", &trace_id)
                        .body(body)
                    {
                        Ok(resp) => return resp.into_response(),
                        Err(e) => {
                            warn!(trace_id = %trace_id, error = %e, "Failed to build stream response");
                            return with_request_id(
                                error_response::internal_error("failed to build response"),
                                &trace_id,
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        trace_id = %trace_id,
                        provider = %upstream.provider_name,
                        error = %e,
                        "Stream upstream failed"
                    );
                    // 4xx锛氬鎴风璇锋眰鏈韩鐨勯棶棰橈紝鍘熸牱閫忎紶涓婃父閿欒缁欏鎴风
                    if let Some((passthrough_status, passthrough_body)) = e.passthrough_response() {
                        let elapsed = start_time.elapsed().as_millis() as i32;
                        let log_id = format!("log_{}", uuid::Uuid::new_v4().simple());
                        if let Err(le) = state.db.insert_request_log(
                            &log_id,
                            &request_id,
                            Some(&pool.name),
                            Some(&upstream.id),
                            Some(model_str),
                            &serde_json::to_string(&failed_upstreams_json).unwrap_or_default(),
                            "POST",
                            "/v1/chat/completions",
                            passthrough_status as i32,
                            elapsed,
                            is_stream,
                            0,
                            0,
                            0,
                        ) {
                            warn!(trace_id = %trace_id, error = %le, "Failed to insert request log");
                        }
                        if matches!(e, UpstreamError::AuthFailed { .. })
                            && let Err(he) = state.db.update_upstream_health(&upstream.id, false, Some(&e.error_summary()), UPSTREAM_FAILURE_THRESHOLD)
                        {
                            warn!(trace_id = %trace_id, error = %he, "Failed to update upstream health after auth failure");
                        }
                        return with_request_id(
                            error_response::passthrough_upstream_error(passthrough_status, passthrough_body, None),
                            &trace_id,
                        );
                    }
                    // Update upstream health: failed stream attempt
                    if let Err(he) = state.db.update_upstream_health(&upstream.id, false, Some(&e.error_summary()), UPSTREAM_FAILURE_THRESHOLD) {
                        warn!(trace_id = %trace_id, error = %he, "Failed to update upstream health after stream failure");
                    }
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
            // 鈹€鈹€ Non-streaming path 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
            match state
                .proxy_client
                .forward_request(
                    &upstream.base_url,
                    &api_key,
                    model_str,
                    &upstream.api_format,
                    &request_body,
                    Some(&trace_headers),
                )
                .await
            {
                Ok(response) => {
                    let elapsed = start_time.elapsed().as_millis() as i32;

                    // Convert native upstream response back to OpenAI Chat format
                    let mut resp_body = if convert::needs_response_conversion(&upstream.api_format) {
                        convert::convert_response_to_client(
                            &response.body,
                            &upstream.api_format,
                            &pool.display_name,
                        )
                    } else {
                        response.body
                    };
                    // Replace model name in response with the pool's display name
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
                        warn!(
                            trace_id = %trace_id,
                            provider = %upstream.provider_name,
                            error = %err_msg,
                            "Upstream returned error in body"
                        );
                        let embedded_err = UpstreamError::EmbeddedError { message: err_msg };
                        failed_upstreams_json.push(json!({
                            "provider": upstream.provider_name,
                            "model": model_str,
                            "error": embedded_err.error_summary()
                        }));
                        last_error = Some(embedded_err.error_summary());
                        // Update upstream health: embedded error (HTTP 200 but error in body)
                        if let Err(he) = state.db.update_upstream_health(&upstream.id, false, Some(&embedded_err.error_summary()), UPSTREAM_FAILURE_THRESHOLD) {
                            warn!(trace_id = %trace_id, error = %he, "Failed to update upstream health after embedded error");
                        }
                        // Embedded errors should failover
                        continue;
                    }

                    // Extract token usage from response
                    let (prompt_tokens, completion_tokens, total_tokens) = extract_usage(&resp_body);

                    // Update upstream health: successful non-streaming response
                    if let Err(e) = state.db.update_upstream_health(&upstream.id, true, None, UPSTREAM_FAILURE_THRESHOLD) {
                        warn!(trace_id = %trace_id, error = %e, "Failed to update upstream health after success");
                    }

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
                        warn!(trace_id = %trace_id, error = %e, "Failed to insert request log");
                    }

                    // Build response with passthrough headers from upstream + X-Request-Id
                    let passthrough = filter_passthrough_headers(&response.headers);
                    let mut builder = Response::builder()
                        .status(StatusCode::OK)
                        .header("X-Request-Id", &trace_id);

                    for (key, value) in passthrough.iter() {
                        builder = builder.header(key, value);
                    }

                    return builder
                        .header("Content-Type", "application/json")
                        .body(Body::from(serde_json::to_vec(&resp_body).unwrap_or_default()))
                        .unwrap_or_else(|_| {
                            (StatusCode::OK, Json(resp_body)).into_response()
                        })
                        .into_response();
                }
                Err(e) => {
                    warn!(
                        trace_id = %trace_id,
                        provider = %upstream.provider_name,
                        error = %e,
                        "Upstream failed"
                    );
                    // 4xx锛氬鎴风璇锋眰鏈韩鐨勯棶棰橈紝鍘熸牱閫忎紶涓婃父閿欒缁欏鎴风锛?
                    // 涓嶅仛鏁呴殰杞Щ锛堟崲涓€涓笂娓镐篃浼氬緱鍒板悓鏍风粨鏋滐級銆?
                    if let Some((passthrough_status, passthrough_body)) = e.passthrough_response() {
                        // 璁板綍璇锋眰鏃ュ織锛岀姸鎬佺爜涓轰笂娓哥湡瀹?4xx 鐘舵€佺爜
                        let elapsed = start_time.elapsed().as_millis() as i32;
                        let log_id = format!("log_{}", uuid::Uuid::new_v4().simple());
                        if let Err(le) = state.db.insert_request_log(
                            &log_id,
                            &request_id,
                            Some(&pool.name),
                            Some(&upstream.id),
                            Some(model_str),
                            &serde_json::to_string(&failed_upstreams_json).unwrap_or_default(),
                            "POST",
                            "/v1/chat/completions",
                            passthrough_status as i32,
                            elapsed,
                            is_stream,
                            0,
                            0,
                            0,
                        ) {
                            warn!(trace_id = %trace_id, error = %le, "Failed to insert request log");
                        }
                        // 401/403 璇存槑涓婃父 Key 鏈夐棶棰橈紝鏍囪鍋ュ悍澶辫触锛?
                        // 鍏朵綑 4xx 鏄鎴风璇锋眰闂锛屼笉鎯╃綒涓婃父
                        if matches!(e, UpstreamError::AuthFailed { .. })
                            && let Err(he) = state.db.update_upstream_health(&upstream.id, false, Some(&e.error_summary()), UPSTREAM_FAILURE_THRESHOLD)
                        {
                            warn!(trace_id = %trace_id, error = %he, "Failed to update upstream health after auth failure");
                        }
                        return with_request_id(
                            error_response::passthrough_upstream_error(passthrough_status, passthrough_body, None),
                            &trace_id,
                        );
                    }
                    // Update upstream health: failed non-stream attempt
                    if let Err(he) = state.db.update_upstream_health(&upstream.id, false, Some(&e.error_summary()), UPSTREAM_FAILURE_THRESHOLD) {
                        warn!(trace_id = %trace_id, error = %he, "Failed to update upstream health after failure");
                    }
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

    // All upstreams exhausted 鈥?log the failure
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
        warn!(trace_id = %trace_id, error = %e, "Failed to insert failure request log");
    }

    // If we never actually attempted any upstream (all were disabled),
    // return 503 instead of 502.
    if !attempted_any {
        return with_request_id(
            error_response::no_available_upstream("all upstreams are disabled"),
            &trace_id,
        );
    }

    with_request_id(
        error_response::all_upstreams_failed(&failed_upstreams_json, last_error.as_deref()),
        &trace_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderName, HeaderValue};

    // 鈹€鈹€ filter_passthrough_headers 娴嬭瘯 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€

    #[test]
    fn test_passthrough_x_ratelimit_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-limit-requests", HeaderValue::from_static("60"));
        headers.insert("x-ratelimit-remaining-tokens", HeaderValue::from_static("1000"));

        let filtered = filter_passthrough_headers(&headers);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.contains_key("x-ratelimit-limit-requests"));
        assert!(filtered.contains_key("x-ratelimit-remaining-tokens"));
    }

    #[test]
    fn test_passthrough_openai_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("openai-organization", HeaderValue::from_static("org-123"));
        headers.insert("openai-processing-ms", HeaderValue::from_static("42"));

        let filtered = filter_passthrough_headers(&headers);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_passthrough_anthropic_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("anthropic-ratelimit-requests", HeaderValue::from_static("100"));

        let filtered = filter_passthrough_headers(&headers);
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn test_filter_out_non_whitelisted_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("server", HeaderValue::from_static("nginx"));
        headers.insert("set-cookie", HeaderValue::from_static("session=abc"));
        headers.insert("x-internal-debug", HeaderValue::from_static("secret"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        let filtered = filter_passthrough_headers(&headers);
        assert!(filtered.is_empty(), "Non-whitelisted headers should be filtered out");
    }

    #[test]
    fn test_passthrough_empty_headers() {
        let headers = HeaderMap::new();
        let filtered = filter_passthrough_headers(&headers);
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_passthrough_mixed_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-limit-requests", HeaderValue::from_static("60"));
        headers.insert("server", HeaderValue::from_static("cloudflare"));
        headers.insert("openai-model", HeaderValue::from_static("gpt-4"));

        let filtered = filter_passthrough_headers(&headers);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.contains_key("x-ratelimit-limit-requests"));
        assert!(filtered.contains_key("openai-model"));
        assert!(!filtered.contains_key("server"));
    }

    // 鈹€鈹€ with_request_id 娴嬭瘯 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€

    #[test]
    fn test_with_request_id_adds_header() {
        let resp = (StatusCode::OK, Json(json!({"ok": true}))).into_response();
        let resp = with_request_id(resp, "abc123");
        assert_eq!(
            resp.headers().get("x-request-id").unwrap(),
            "abc123"
        );
    }

    #[test]
    fn test_with_request_id_preserves_existing_headers() {
        let mut resp = (StatusCode::OK, Json(json!({"ok": true}))).into_response();
        resp.headers_mut().insert("content-type", HeaderValue::from_static("application/json"));
        let resp = with_request_id(resp, "trace-xyz");
        assert_eq!(
            resp.headers().get("x-request-id").unwrap(),
            "trace-xyz"
        );
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json"
        );
    }

    // 鈹€鈹€ build_trace_headers 娴嬭瘯 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€

    #[test]
    fn test_build_trace_headers() {
        let headers = build_trace_headers("test-trace-id");
        assert_eq!(headers.len(), 1);
        assert_eq!(
            headers.get("x-request-id").unwrap(),
            "test-trace-id"
        );
    }

    #[test]
    fn test_build_trace_headers_empty_string() {
        // Empty string is still a valid header value
        let headers = build_trace_headers("");
        assert_eq!(headers.len(), 1);
    }

    // 鈹€鈹€ PASSTHROUGH_HEADER_PREFIXES 瀹屾暣鎬ф祴璇?鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€

    #[test]
    fn test_passthrough_prefixes_cover_expected_ranges() {
        // Ensure all expected prefix patterns are defined
        assert!(PASSTHROUGH_HEADER_PREFIXES.contains(&"x-ratelimit-"));
        assert!(PASSTHROUGH_HEADER_PREFIXES.contains(&"openai-"));
        assert!(PASSTHROUGH_HEADER_PREFIXES.contains(&"anthropic-"));
    }
}
