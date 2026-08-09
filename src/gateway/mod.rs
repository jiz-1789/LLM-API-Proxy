pub mod auth;
pub mod convert;
pub mod error_response;
pub mod health;
pub mod rate_limit;
pub mod stream;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::io::StreamReader;
use tracing::{info, warn};

use crate::crypto::KeyManager;
use crate::db::Database;
use crate::error::AppError;
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
        .route("/v1/responses", post(handle_responses))
        // Native client-format endpoints
        .route("/v1/messages", post(handle_messages))
        .route("/v1beta/models/{*rest}", post(handle_gemini_generate))
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
///
/// While the Claude Desktop switch is ON, the route IDs written into the 3P
/// profile are listed first, enriched with the fields Claude Desktop reads
/// (`type` / `created_at` / `supports1m`, Anthropic model-list format) so it
/// can show the 1M-context option. OpenAI clients ignore the extra fields.
async fn handle_models(State(state): State<GatewayState>) -> impl IntoResponse {
    match state.db.get_pools() {
        Ok(pools) => {
            let mut data: Vec<Value> = Vec::new();
            // Roles with 1M-context enabled, persisted in the tool config
            // snapshot when the Desktop switch was saved.
            let mut one_m_roles: Vec<String> = Vec::new();
            if let Ok(Some(cfg)) =
                state
                    .db
                    .get_tool_config(crate::tool_config::claude_desktop::APP_ID)
                && let Ok(snapshot) =
                    serde_json::from_str::<Value>(&cfg.config_snapshot)
                && let Some(roles) = snapshot.get("roles_1m").and_then(|v| v.as_array())
            {
                one_m_roles = roles
                    .iter()
                    .filter_map(|r| r.as_str().map(str::to_string))
                    .collect();
            }
            // Claude Desktop route IDs first (while the Desktop switch is ON),
            // then every pool as a plain model. Pools mapped to a 1M-enabled
            // role also carry `supports1m` — Claude Desktop issues requests
            // with the plain pool id it picked from the model list, so the
            // pool entry itself must declare the 1M capability.
            if let Ok(Some(map_json)) = state
                .db
                .get_setting(crate::tool_config::claude_desktop::ROUTE_MAP_SETTING_KEY)
                && let Ok(map) = serde_json::from_str::<serde_json::Map<String, Value>>(&map_json)
            {
                for route_id in map.keys() {
                    let role = crate::tool_config::claude_desktop::ROLE_ROUTE_IDS
                        .iter()
                        .find(|(_, rid)| rid == route_id)
                        .map(|(role, _)| *role);
                    let one_m = role
                        .map(|r| one_m_roles.iter().any(|x| x == r))
                        .unwrap_or(false);
                    data.push(desktop_model_item(route_id, one_m));
                }
            }
            // Pools behind a 1M-enabled role mapping.
            let mut one_m_pools: HashSet<String> = HashSet::new();
            if let Ok(Some(cfg)) =
                state
                    .db
                    .get_tool_config(crate::tool_config::claude_desktop::APP_ID)
                && let Ok(snapshot) =
                    serde_json::from_str::<Value>(&cfg.config_snapshot)
                && let Some(roles) = snapshot.get("model_roles").and_then(|v| v.as_array())
            {
                for pair in roles {
                    let Some(role) = pair.get(0).and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let Some(pool) = pair.get(1).and_then(|v| v.as_str()) else {
                        continue;
                    };
                    if one_m_roles.iter().any(|r| r == role) {
                        one_m_pools.insert(pool.to_string());
                    }
                }
            }
            data.extend(pools.iter().map(|pool| {
                let mut item = json!({
                    "id": pool.name,
                    "object": "model",
                    "owned_by": "llm-api-proxy"
                });
                if one_m_pools.contains(&pool.name) {
                    item["type"] = json!("model");
                    item["created_at"] = json!("2024-01-01T00:00:00Z");
                    item["supports1m"] = json!(true);
                }
                item
            }));

            let first_id = data
                .first()
                .and_then(|m| m.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let last_id = data
                .last()
                .and_then(|m| m.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string);
            Json(json!({
                "data": data,
                "has_more": false,
                "first_id": first_id,
                "last_id": last_id,
            }))
            .into_response()
        }
        Err(e) => error_response::internal_error(&format!("database error: {}", e)),
    }
}

/// One model entry for the `/v1/models` list in Anthropic-compatible shape
/// (what Claude Desktop reads), keeping OpenAI fields (`object`) harmless.
fn desktop_model_item(route_id: &str, supports_1m: bool) -> Value {
    json!({
        "id": route_id,
        "object": "model",
        "owned_by": "llm-api-proxy",
        "type": "model",
        "created_at": "2024-01-01T00:00:00Z",
        "supports1m": supports_1m,
    })
}

/// Resolve a Claude Desktop route ID (`claude-sonnet-5`, ...) to its mapped
/// pool via the persisted route map setting. Returns `(pool_name, route_id)`.
/// `Ok(None)` when no map is active or the model is not a route ID.
fn resolve_claude_desktop_route(
    db: &Database,
    model: &str,
) -> Result<Option<(String, String)>, AppError> {
    let Some(map_json) = db.get_setting(crate::tool_config::claude_desktop::ROUTE_MAP_SETTING_KEY)?
    else {
        return Ok(None);
    };
    let Ok(map) = serde_json::from_str::<serde_json::Map<String, Value>>(&map_json) else {
        return Ok(None);
    };
    let Some(pool_name) = map.get(model).and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    Ok(Some((pool_name.to_string(), model.to_string())))
}

/// Strip the Claude Desktop 1M-context marker (`[1m]`, case-insensitive, with
/// optional surrounding whitespace) from a model name. Returns the trimmed
/// base name when the marker is present, otherwise the input unchanged.
fn strip_one_m_marker(model: &str) -> String {
    let trimmed = model.trim();
    let lower = trimmed.to_ascii_lowercase();
    if let Some(pos) = lower.rfind("[1m]") {
        let prefix = &trimmed[..pos];
        let tail = &trimmed[pos + 4..];
        return format!("{prefix}{tail}").trim().to_string();
    }
    trimmed.to_string()
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

/// Extract token usage from a **native** upstream response (used in passthrough
/// mode where the body is Responses/Anthropic/Gemini, not Chat). Uses the same
/// 0,0,0 fallback as `extract_usage` when no usage is present.
fn extract_native_usage(resp: &serde_json::Value, upstream_format: &str) -> (i64, i64, i64) {
    match upstream_format {
        // Responses / Anthropic both report `usage.input_tokens` / `output_tokens`.
        convert::FORMAT_OPENAI_RESPONSES | convert::FORMAT_ANTHROPIC => {
            if let Some(usage) = resp.get("usage") {
                let prompt = usage.get("input_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
                let completion = usage.get("output_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
                let total = usage.get("total_tokens").and_then(|v| v.as_i64()).unwrap_or(prompt + completion);
                (prompt, completion, total)
            } else {
                (0, 0, 0)
            }
        }
        // Gemini native embeds usage under `usageMetadata`.
        convert::FORMAT_GEMINI_NATIVE => {
            if let Some(meta) = resp.get("usageMetadata") {
                let prompt = meta.get("promptTokenCount").and_then(|v| v.as_i64()).unwrap_or(0);
                let completion = meta.get("candidatesTokenCount").and_then(|v| v.as_i64()).unwrap_or(0);
                (prompt, completion, prompt + completion)
            } else {
                (0, 0, 0)
            }
        }
        _ => extract_usage(resp),
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
///
/// POST /v1/chat/completions — OpenAI Chat Completions endpoint.
async fn handle_chat_completions(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    process_completion(state, headers, body, ResponseFormat::Chat).await
}

/// POST /v1/responses — OpenAI Responses API endpoint.
async fn handle_responses(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    process_completion(state, headers, body, ResponseFormat::Responses).await
}

/// POST /v1/messages — Anthropic Messages API endpoint.
///
/// Anthropic-format requests (from Claude Code, Claude Desktop, etc.) are
/// normalized to the internal Chat format, processed, then converted back to
/// an Anthropic Messages response.
async fn handle_messages(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    process_completion(state, headers, body, ResponseFormat::Anthropic).await
}

/// POST /v1beta/models/{*rest} — Gemini Native API endpoint.
///
/// The path tail is `{model}:generateContent` or `{model}:streamGenerateContent`.
/// Gemini-format requests are normalized to internal Chat, processed, then
/// converted back to a Gemini Native response.
async fn handle_gemini_generate(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(rest): Path<String>,
    Json(body): Json<Value>,
) -> Response<Body> {
    // Parse `{model}:generateContent` (or `:streamGenerateContent`) from the tail.
    let model = rest
        .split(':')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if model.is_empty() {
        return with_request_id(
            error_response::invalid_request("invalid Gemini model path", "invalid_request"),
            "gemini-request",
        );
    }

    let mut body = body;
    // Ensure the body carries the model (Gemini may omit it from the body).
    if let Some(obj) = body.as_object_mut() {
        obj.insert("model".to_string(), Value::String(model.clone()));
    }

    process_completion(state, headers, body, ResponseFormat::GeminiNative)
        .await
        .into_response()
}

/// Which response schema the client expects.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ResponseFormat {
    Chat,
    Responses,
    Anthropic,
    GeminiNative,
}

impl ResponseFormat {
    /// The wire format name this endpoint speaks (matches upstream `api_format`).
    fn api_format(&self) -> &'static str {
        match self {
            ResponseFormat::Chat => convert::FORMAT_OPENAI_CHAT,
            ResponseFormat::Responses => convert::FORMAT_OPENAI_RESPONSES,
            ResponseFormat::Anthropic => convert::FORMAT_ANTHROPIC,
            ResponseFormat::GeminiNative => convert::FORMAT_GEMINI_NATIVE,
        }
    }
}

/// Shared core handler for chat-style completion requests.
async fn process_completion(
    state: GatewayState,
    headers: HeaderMap,
    mut body: Value,
    response_format: ResponseFormat,
) -> impl IntoResponse {
    let start_time = Instant::now();

    // Keep the raw client body. When a client endpoint's wire format matches
    // the upstream's api_format we pass it through untouched (no Chat round-trip).
    let client_body = body.clone();

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

    // Normalize client-facing native formats to the internal Chat format.
    match response_format {
        ResponseFormat::Responses => {
            match convert::normalize_responses_input(&body) {
                Ok(chat_body) => body = chat_body,
                Err(e) => {
                    return with_request_id(
                        error_response::invalid_request(&format!("invalid responses request: {}", e), "invalid_request"),
                        &trace_id,
                    );
                }
            }
        }
        ResponseFormat::Anthropic => {
            match convert::normalize_anthropic_input(&body) {
                Ok(chat_body) => body = chat_body,
                Err(e) => {
                    return with_request_id(
                        error_response::invalid_request(&format!("invalid anthropic request: {}", e), "invalid_request"),
                        &trace_id,
                    );
                }
            }
        }
        ResponseFormat::GeminiNative => {
            match convert::normalize_gemini_input(&body) {
                Ok(chat_body) => body = chat_body,
                Err(e) => {
                    return with_request_id(
                        error_response::invalid_request(&format!("invalid gemini request: {}", e), "invalid_request"),
                        &trace_id,
                    );
                }
            }
        }
        ResponseFormat::Chat => {}
    }

    let model_raw = match body.get("model").and_then(|m| m.as_str()) {
        Some(m) => m.to_string(),
        None => {
            return with_request_id(
                error_response::invalid_request("Missing required field: model", "missing_model"),
                &trace_id,
            );
        }
    };

    // Claude Desktop appends a `[1m]` marker to the model name while its
    // 1M-context beta is active (e.g. `claude-sonnet-5[1m]`). Strip it before
    // route/pool lookup so the request resolves like its base model.
    let model = strip_one_m_marker(&model_raw);

    let is_stream = body.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    info!(
        trace_id = %trace_id,
        model = %model,
        stream = is_stream,
        "Received chat completion request"
    );

    // Claude Desktop route alias: while the Desktop switch is ON, clients
    // (Claude Desktop) send fixed route IDs (`claude-sonnet-5`, ...) instead
    // of pool names. Resolve the route back to its mapped pool; the route ID
    // is echoed back to the client in place of the pool's display name.
    let route_alias = resolve_claude_desktop_route(&state.db, &model).ok().flatten();

    // Find matching pool by model name
    let pool = match state.db.get_pool_by_name(
        route_alias
            .as_ref()
            .map(|(pool_name, _)| pool_name.as_str())
            .unwrap_or(&model),
    ) {
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

    // Model name returned to the client: the route ID for Claude Desktop
    // alias requests, otherwise the pool's display name.
    let response_model = route_alias
        .as_ref()
        .map(|(_, route_id)| route_id.clone())
        .unwrap_or_else(|| pool.display_name.clone());

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
    // Last upstream 429 (rate limit) response, kept so that when every
    // upstream is rate-limited the client gets the 429 back instead of a
    // generic 502 (failover already tried, but all exhausted).
    let mut last_ratelimit: Option<(u16, Value)> = None;

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
        //
        // When the client endpoint's wire format matches the upstream's
        // api_format, forward the raw client body (no Chat round-trip).
        let upstream_format = upstream.api_format.as_str();
        let passthrough = response_format.api_format() == upstream_format;
        let mut request_body = if passthrough {
            client_body.clone()
        } else {
            body.clone()
        };

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
            // In passthrough mode the params must be expressed in the
            // client wire format (Responses/Anthropic/Gemini native).
            let params = if passthrough {
                thinking::map_thinking_params_for_client_format(&thinking_params, upstream_format)
            } else {
                thinking_params
            };
            thinking::merge_thinking_params(&mut request_body, &params);
        }

        // For streaming requests, ensure we get usage info in the final chunk.
        // (Only relevant for Chat-based upstreams; Conversations first pass-through uses native usage.)
        if is_stream
            && upstream_format == convert::FORMAT_OPENAI_CHAT
            && let Some(obj) = request_body.as_object_mut()
        {
            obj.insert(
                "stream_options".to_string(),
                json!({ "include_usage": true }),
            );
        }

        // Convert request body to the upstream's native API format if needed.
        // In passthrough mode (client wire format == upstream format), the raw
        // client body is already in the correct format, so no conversion.
        if !passthrough && convert::needs_request_conversion(upstream_format) {
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
                    let tu_status = upstream_response.status().as_u16() as i32;
                    let byte_stream = upstream_response.bytes_stream();
                    let display_name = response_model.clone();
                    let upstream_format = upstream.api_format.clone();
                    let db_clone = state.db.clone();
                    let log_id_clone = log_id.clone();
                    let trace_id_clone = trace_id.clone();
                    let tu_pool = pool.name.clone();
                    let tu_upstream = upstream.id.clone();
                    let tu_model = model_str.to_string();
                    let tu_request_id = request_id.clone();
                    let passthrough_flag = passthrough;
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
                        // Passthrough: forward the native stream verbatim (model/usage/error only).
                        // Only applies to native client formats; Chat client + Chat upstream
                        // uses the existing Chat chunk path (model replace via process_sse_chunk).
                        let use_passthrough_stream =
                            passthrough_flag && upstream_format != convert::FORMAT_OPENAI_CHAT;
                        let use_native_converter = !use_passthrough_stream
                            && convert::needs_request_conversion(&upstream_format);
                        let mut passthrough_converter =
                            convert::PassthroughStreamConverter::new(&upstream_format);
                        let mut native_converter =
                            convert::NativeStreamConverter::new(&upstream_format);
                        // Client-side converters for native response formats.
                        // Stateful so the final completion event can be deferred
                        // until the trailing usage chunk arrives, and streaming
                        // tool calls can be accumulated across chunks.
                        let mut responses_conv =
                            convert::openai_responses::ResponsesStreamConverter::new(&display_name);
                        let mut anthropic_conv =
                            convert::AnthropicStreamConverter::new(&display_name);
                        let mut gemini_conv =
                            convert::GeminiStreamConverter::new(&display_name);
                        let responses_stream = response_format == ResponseFormat::Responses;
                        let anthropic_client_stream = response_format == ResponseFormat::Anthropic;
                        let gemini_client_stream = response_format == ResponseFormat::GeminiNative;
                        let idle_timeout = std::time::Duration::from_secs(DEFAULT_STREAM_IDLE_TIMEOUT_SECS);

                        loop {
                            // Wrap next_line() with an idle timeout.
                            // If no data arrives within the timeout, terminate the stream.
                            match tokio::time::timeout(idle_timeout, lines.next_line()).await {
                                Ok(Ok(Some(line))) => {
                                    let mut outputs: Vec<String> = Vec::new();
                                    if use_passthrough_stream {
                                        let converted =
                                            passthrough_converter.process(&line, &display_name);
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
                                    } else if use_native_converter {
                                        let converted = native_converter.process(&line, &display_name);
                                        if let Some(u) = converted.usage {
                                            last_usage = Some(u);
                                        }
                                        if stream_error.is_none() && converted.error.is_some() {
                                            stream_error = converted.error.clone();
                                        }
                                        // The native upstream was translated to Chat
                                        // format. If the CLIENT also speaks a native
                                        // format (different from the upstream's), the
                                        // Chat chunks must be converted a second time
                                        // into the client's native events instead of
                                        // being forwarded raw.
                                        if responses_stream || anthropic_client_stream || gemini_client_stream {
                                            let mut native_done = converted.done;
                                            for out in converted.lines {
                                                if let Some(payload) =
                                                    out.strip_prefix("data: ").map(|s| s.trim())
                                                {
                                                    if payload == "[DONE]" {
                                                        native_done = true;
                                                        continue;
                                                    }
                                                    let (c_lines, c_usage, c_err) =
                                                        if responses_stream {
                                                            responses_conv.process(payload)
                                                        } else if anthropic_client_stream {
                                                            anthropic_conv.process(payload)
                                                        } else {
                                                            gemini_conv.process(payload)
                                                        };
                                                    if let Some(u) = c_usage {
                                                        last_usage = Some(u);
                                                    }
                                                    if stream_error.is_none()
                                                        && let Some(msg) = c_err
                                                    {
                                                        stream_error = Some(msg);
                                                    }
                                                    outputs.extend(c_lines);
                                                }
                                            }
                                            if native_done {
                                                let tail = if responses_stream {
                                                    responses_conv.finish()
                                                } else if anthropic_client_stream {
                                                    anthropic_conv.finish()
                                                } else {
                                                    gemini_conv.finish()
                                                };
                                                outputs.extend(tail);
                                                for out in outputs {
                                                    if tx.send(Ok(Bytes::from(out))).await.is_err() {
                                                        break;
                                                    }
                                                }
                                                break;
                                            }
                                        } else {
                                            outputs = converted.lines;
                                            if converted.done {
                                                for out in outputs {
                                                    if tx.send(Ok(Bytes::from(out))).await.is_err() {
                                                        break;
                                                    }
                                                }
                                                break;
                                            }
                                        }
                                    } else if let Some(json_str) = line.strip_prefix("data: ") {
                                        let trimmed = json_str.trim();
                                        if trimmed == "[DONE]" {
                                            // Native client streams: the converter
                                            // emits its own terminal `[DONE]` after the
                                            // deferred completion event — flush it here
                                            // and do NOT forward the raw marker again.
                                            if responses_stream {
                                                outputs.extend(responses_conv.finish());
                                            } else if anthropic_client_stream {
                                                outputs.extend(anthropic_conv.finish());
                                            } else if gemini_client_stream {
                                                outputs.extend(gemini_conv.finish());
                                            } else {
                                                outputs.push("data: [DONE]\n\n".to_string());
                                            }
                                        } else if responses_stream {
                                            // Convert OpenAI Chat chunk -> Responses events
                                            let (resp_lines, usage, error_msg) =
                                                responses_conv.process(trimmed);
                                            if let Some(u) = usage {
                                                last_usage = Some(u);
                                            }
                                            if stream_error.is_none()
                                                && let Some(msg) = error_msg
                                            {
                                                stream_error = Some(msg);
                                            }
                                            outputs.extend(resp_lines);
                                        } else if anthropic_client_stream {
                                            // Convert OpenAI Chat chunk -> Anthropic SSE events
                                            let (anth_lines, usage, error_msg) =
                                                anthropic_conv.process(trimmed);
                                            if let Some(u) = usage {
                                                last_usage = Some(u);
                                            }
                                            if stream_error.is_none()
                                                && let Some(msg) = error_msg
                                            {
                                                stream_error = Some(msg);
                                            }
                                            outputs.extend(anth_lines);
                                        } else if gemini_client_stream {
                                            // Convert OpenAI Chat chunk -> Gemini SSE data
                                            let (gem_lines, usage, error_msg) =
                                                gemini_conv.process(trimmed);
                                            if let Some(u) = usage {
                                                last_usage = Some(u);
                                            }
                                            if stream_error.is_none()
                                                && let Some(msg) = error_msg
                                            {
                                                stream_error = Some(msg);
                                            }
                                            outputs.extend(gem_lines);
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
                                Ok(Ok(None)) => {
                                    // Stream ended without a `[DONE]` marker: flush
                                    // any deferred completion event for native clients.
                                    if responses_stream {
                                        for out in responses_conv.finish() {
                                            if tx.send(Ok(Bytes::from(out))).await.is_err() {
                                                break;
                                            }
                                        }
                                    } else if anthropic_client_stream {
                                        for out in anthropic_conv.finish() {
                                            if tx.send(Ok(Bytes::from(out))).await.is_err() {
                                                break;
                                            }
                                        }
                                    } else if gemini_client_stream {
                                        for out in gemini_conv.finish() {
                                            if tx.send(Ok(Bytes::from(out))).await.is_err() {
                                                break;
                                            }
                                        }
                                    }
                                    break; // stream ended
                                }
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
                        if last_usage.is_none() && use_passthrough_stream {
                            let (p, c, t) = passthrough_converter.final_usage();
                            if p > 0 || c > 0 {
                                last_usage = Some((p, c, t));
                            }
                        }
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
                        // Record standalone token usage for the streamed request.
                        if let Some((prompt, completion, total)) = last_usage
                            && total > 0
                        {
                            let tu_id = format!("tu_{}", uuid::Uuid::new_v4().simple());
                            if let Err(e) = db_clone.record_token_usage(&crate::db::TokenUsageRecord {
                                id: tu_id,
                                request_id: tu_request_id,
                                pool_name: Some(tu_pool),
                                upstream_id: Some(tu_upstream),
                                model: Some(tu_model),
                                prompt_tokens: prompt,
                                completion_tokens: completion,
                                total_tokens: total,
                                status_code: if stream_error.is_some() { 500 } else { tu_status },
                                created_at: String::new(),
                            }) {
                                warn!(trace_id = %trace_id_clone, error = %e, "Failed to record token usage");
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
                    if let Some((passthrough_status, passthrough_body)) = e.passthrough_response()
                        && !matches!(e, UpstreamError::RateLimited { .. })
                    {
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
                    // Keep the last 429 so that if every upstream is
                    // rate-limited we can pass it through instead of a 502.
                    if let UpstreamError::RateLimited { status, body, .. } = &e {
                        last_ratelimit = Some((*status, body.clone().unwrap_or_else(|| json!({}))));
                    }
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

                    // Convert native upstream response back to OpenAI Chat format.
                    // In passthrough mode the response is already in the client
                    // format, so keep it verbatim.
                    let mut resp_body = if passthrough {
                        response.body
                    } else if convert::needs_response_conversion(&upstream.api_format) {
                        convert::convert_response_to_client(
                            &response.body,
                            &upstream.api_format,
                            &response_model,
                        )
                    } else {
                        response.body
                    };
                    // Replace model name in response with the pool's display name.
                    // Native formats carry a top-level model field (except Gemini);
                    // Chat responses must always carry one.
                    let add_model = passthrough
                        && upstream_format == convert::FORMAT_OPENAI_CHAT
                        || !passthrough;
                    if add_model
                        && let Some(obj) = resp_body.as_object_mut()
                    {
                        obj.insert(
                            "model".to_string(),
                            Value::String(response_model.clone()),
                        );
                    } else if passthrough
                        && let Some(obj) = resp_body.as_object_mut()
                        && obj.get("model").is_some()
                    {
                        obj.insert(
                            "model".to_string(),
                            Value::String(response_model.clone()),
                        );
                    }

                    // Convert the OpenAI Chat response to the client's native
                    // format when the client called a native endpoint.
                    // Skipped in passthrough mode: the response is already in the
                    // client's format.
                    if !passthrough {
                        match response_format {
                            ResponseFormat::Responses => {
                                resp_body = convert::normalize_responses_output(&resp_body, &response_model);
                            }
                            ResponseFormat::Anthropic => {
                                resp_body = convert::normalize_anthropic_output(&resp_body, &response_model);
                            }
                            ResponseFormat::GeminiNative => {
                                resp_body = convert::normalize_gemini_output(&resp_body, &response_model);
                            }
                            ResponseFormat::Chat => {}
                        }
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

                    // Extract token usage from response (native keys in passthrough mode)
                    let (prompt_tokens, completion_tokens, total_tokens) = if passthrough {
                        extract_native_usage(&resp_body, upstream_format)
                    } else {
                        extract_usage(&resp_body)
                    };

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

                    // Record standalone token usage (decoupled from request_logs so
                    // log cleanup never distorts statistics).
                    if total_tokens > 0 {
                        let tu_id = format!("tu_{}", uuid::Uuid::new_v4().simple());
                        if let Err(e) = state.db.record_token_usage(&crate::db::TokenUsageRecord {
                            id: tu_id,
                            request_id: request_id.clone(),
                            pool_name: Some(pool.name.clone()),
                            upstream_id: Some(upstream.id.clone()),
                            model: Some(model_str.to_string()),
                            prompt_tokens,
                            completion_tokens,
                            total_tokens,
                            status_code: response.status_code,
                            created_at: String::new(),
                        }) {
                            warn!(trace_id = %trace_id, error = %e, "Failed to record token usage");
                        }
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
                    if let Some((passthrough_status, passthrough_body)) = e.passthrough_response()
                        && !matches!(e, UpstreamError::RateLimited { .. })
                    {
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
                    // Keep the last 429 so that if every upstream is
                    // rate-limited we can pass it through instead of a 502.
                    if let UpstreamError::RateLimited { status, body, .. } = &e {
                        last_ratelimit = Some((*status, body.clone().unwrap_or_else(|| json!({}))));
                    }
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

    // Every upstream failed. If the last failure was a 429 (rate limit),
    // pass it through so the client can back off — the real status carries
    // more meaning than a generic 502.
    if let Some((status, body)) = last_ratelimit {
        return with_request_id(
            error_response::passthrough_upstream_error(status, &body, None),
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
    use axum::http::HeaderValue;

    // 鈹€鈹€ filter_passthrough_headers 娴嬭瘯 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€

    #[test]
    fn test_desktop_model_item_anthropic_shape() {
        let item = desktop_model_item("claude-sonnet-5", true);
        assert_eq!(item["id"], json!("claude-sonnet-5"));
        assert_eq!(item["type"], json!("model"));
        assert_eq!(item["created_at"], json!("2024-01-01T00:00:00Z"));
        assert_eq!(item["supports1m"], json!(true));
        let no_1m = desktop_model_item("claude-haiku-4-5", false);
        assert_eq!(no_1m["supports1m"], json!(false));
    }

    #[test]
    fn test_desktop_model_item_keeps_openai_fields() {
        let item = desktop_model_item("claude-opus-5", false);
        assert_eq!(item["object"], json!("model"));
        assert_eq!(item["owned_by"], json!("llm-api-proxy"));
    }

    #[test]
    fn test_strip_one_m_marker_variants() {
        assert_eq!(strip_one_m_marker("claude-sonnet-5[1m]"), "claude-sonnet-5");
        assert_eq!(strip_one_m_marker("claude-opus-4-8 [1M]"), "claude-opus-4-8");
        assert_eq!(strip_one_m_marker("claude-haiku-4-5[1m] "), "claude-haiku-4-5");
        assert_eq!(strip_one_m_marker("claude-sonnet-5"), "claude-sonnet-5");
        assert_eq!(strip_one_m_marker("deepseek-v4-flash-free"), "deepseek-v4-flash-free");
        assert_eq!(strip_one_m_marker("gpt-5[1m]"), "gpt-5");
    }

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

    // 鈹€鈹€ Claude Desktop route alias resolution 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€

    fn test_db_with_pool() -> (Arc<Database>, String) {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.initialize().unwrap();
        let crypto = crate::crypto::KeyManager::initialize(&std::env::temp_dir()).unwrap();
        let enc = crypto.encrypt_api_key("sk-test").unwrap();
        db.create_upstream("up_a", "OpenAI", "https://a.com", &enc, "gpt-4o", "[]", true, "", "", "openai_chat")
            .unwrap();
        db.create_pool("pool_a", "pool-a", "Pool A", 5, false, "off", "", "")
            .unwrap();
        db.add_upstream_to_pool("pool_a", "up_a", 0, "gpt-4o").unwrap();
        (db, "pool-a".to_string())
    }

    #[test]
    fn test_resolve_route_no_map_returns_none() {
        let (db, _) = test_db_with_pool();
        let resolved = resolve_claude_desktop_route(&db, "claude-sonnet-5").unwrap();
        assert!(resolved.is_none());
    }

    #[test]
    fn test_resolve_route_active_map_translates_to_pool() {
        let (db, _) = test_db_with_pool();
        db.save_setting(
            crate::tool_config::claude_desktop::ROUTE_MAP_SETTING_KEY,
            r#"{"claude-sonnet-5":"pool-a","claude-opus-5":"pool-a"}"#,
        )
        .unwrap();
        let resolved = resolve_claude_desktop_route(&db, "claude-sonnet-5").unwrap();
        assert_eq!(resolved, Some(("pool-a".to_string(), "claude-sonnet-5".to_string())));
        // Non-route models are untouched.
        assert!(resolve_claude_desktop_route(&db, "gpt-4o").unwrap().is_none());
        // Unknown route IDs are untouched.
        assert!(resolve_claude_desktop_route(&db, "claude-sonnet-9").unwrap().is_none());
    }

    #[test]
    fn test_resolve_route_ignores_corrupt_map() {
        let (db, _) = test_db_with_pool();
        db.save_setting(
            crate::tool_config::claude_desktop::ROUTE_MAP_SETTING_KEY,
            "not-json",
        )
        .unwrap();
        assert!(resolve_claude_desktop_route(&db, "claude-sonnet-5").unwrap().is_none());
    }

    #[test]
    fn test_handle_models_includes_route_ids_when_map_active() {
        let (db, _) = test_db_with_pool();
        let state = GatewayState {
            db: db.clone(),
            proxy_client: Arc::new(UpstreamClient::new()),
            crypto: Arc::new(
                crate::crypto::KeyManager::initialize(&std::env::temp_dir()).unwrap(),
            ),
            rr_counters: Arc::new(Mutex::new(HashMap::new())),
            rate_limiter: RateLimiter::new(RateLimitConfig::default()),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        // Without the map: pools only.
        let resp = rt.block_on(handle_models(State(state.clone()))).into_response();
        let body = rt
            .block_on(axum::body::to_bytes(resp.into_body(), usize::MAX))
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let ids: Vec<&str> = json["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["id"].as_str())
            .collect();
        assert_eq!(ids, vec!["pool-a"]);

        // With the map active: route IDs first, then pools.
        db.save_setting(
            crate::tool_config::claude_desktop::ROUTE_MAP_SETTING_KEY,
            r#"{"claude-sonnet-5":"pool-a","claude-haiku-4-5":"pool-a"}"#,
        )
        .unwrap();
        let resp = rt.block_on(handle_models(State(state))).into_response();
        let body = rt
            .block_on(axum::body::to_bytes(resp.into_body(), usize::MAX))
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let mut ids: Vec<&str> = json["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["id"].as_str())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["claude-haiku-4-5", "claude-sonnet-5", "pool-a"]);
    }
}
