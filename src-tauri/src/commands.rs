use serde::{Deserialize, Serialize};
use tauri::State;

use llm_api_proxy_lib::AppState;

// ============================================================================
// DTO Types (frontend <-> backend)
// ============================================================================

/// Upstream info returned to the frontend (API key masked).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamVO {
    pub id: String,
    pub provider_name: String,
    pub base_url: String,
    pub api_key_masked: String,
    pub selected_model: String,
    pub available_models: Vec<String>,
    pub enabled: bool,
    pub remark: String,
    pub status: String,
    pub failure_count: i32,
    pub last_failure_time: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Request body for creating a new upstream.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateUpstreamRequest {
    pub provider_name: String,
    pub base_url: String,
    pub api_key: String,
    pub selected_model: String,
    #[serde(default)]
    pub available_models: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub remark: String,
}

/// Request body for updating an upstream.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateUpstreamRequest {
    pub provider_name: String,
    pub base_url: String,
    pub api_key: String,
    pub selected_model: String,
    #[serde(default)]
    pub available_models: Vec<String>,
    pub enabled: bool,
    #[serde(default)]
    pub remark: String,
}

fn default_true() -> bool {
    true
}

// ============================================================================
// Helpers
// ============================================================================

fn to_vo(u: &llm_api_proxy_lib::db::Upstream) -> UpstreamVO {
    let has_key = !u.api_key_encrypted.is_empty();
    let available_models: Vec<String> = serde_json::from_str(&u.available_models)
        .unwrap_or_default();
    UpstreamVO {
        id: u.id.clone(),
        provider_name: u.provider_name.clone(),
        base_url: u.base_url.clone(),
        api_key_masked: if has_key { "••••••••" } else { "" }.to_string(),
        selected_model: u.selected_model.clone(),
        available_models,
        enabled: u.enabled,
        remark: u.remark.clone(),
        status: u.status.clone(),
        failure_count: u.failure_count,
        last_failure_time: u.last_failure_time.clone(),
        created_at: u.created_at.clone(),
        updated_at: u.updated_at.clone(),
    }
}

/// Generate a unique ID for a new upstream record.
fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("up_{:x}{:08x}", now.as_secs(), now.subsec_nanos())
}

// ============================================================================
// Tauri Commands
// ============================================================================

/// List all upstreams (API keys masked).
#[tauri::command]
pub fn list_upstreams(state: State<'_, AppState>) -> Result<Vec<UpstreamVO>, String> {
    state
        .db
        .get_upstreams()
        .map(|list| list.iter().map(to_vo).collect())
        .map_err(|e| e.to_string())
}

/// Get a single upstream by ID (API key masked).
#[tauri::command]
pub fn get_upstream(id: String, state: State<'_, AppState>) -> Result<UpstreamVO, String> {
    state
        .db
        .get_upstream_by_id(&id)
        .map_err(|e| e.to_string())?
        .map(|u| to_vo(&u))
        .ok_or_else(|| format!("上游 {} 不存在", id))
}

/// Create a new upstream. The plaintext API key is encrypted before storage.
#[tauri::command]
pub fn create_upstream(
    req: CreateUpstreamRequest,
    state: State<'_, AppState>,
) -> Result<UpstreamVO, String> {
    let encrypted = state.crypto.encrypt_api_key(&req.api_key)?;
    let id = generate_id();
    let models_json = serde_json::to_string(&req.available_models).unwrap_or_else(|_| "[]".to_string());

    state
        .db
        .create_upstream(
            &id,
            &req.provider_name,
            &req.base_url,
            &encrypted,
            &req.selected_model,
            &models_json,
            req.enabled,
            &req.remark,
        )
        .map_err(|e| e.to_string())?;

    state
        .db
        .get_upstream_by_id(&id)
        .map_err(|e| e.to_string())?
        .map(|u| to_vo(&u))
        .ok_or_else(|| "创建后未能读取上游记录".to_string())
}

/// Update an existing upstream. If `api_key` is "••••••••" (masked), the existing key is kept.
#[tauri::command]
pub fn update_upstream(
    id: String,
    req: UpdateUpstreamRequest,
    state: State<'_, AppState>,
) -> Result<UpstreamVO, String> {
    // If the user didn't change the API key (sent the mask back), keep the existing encrypted key.
    let encrypted = if req.api_key == "••••••••" {
        let existing = state
            .db
            .get_upstream_by_id(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("上游 {} 不存在", id))?;
        existing.api_key_encrypted
    } else {
        state.crypto.encrypt_api_key(&req.api_key)?
    };
    let models_json = serde_json::to_string(&req.available_models).unwrap_or_else(|_| "[]".to_string());

    state
        .db
        .update_upstream(
            &id,
            &req.provider_name,
            &req.base_url,
            &encrypted,
            &req.selected_model,
            &models_json,
            req.enabled,
            &req.remark,
        )
        .map_err(|e| e.to_string())?;

    state
        .db
        .get_upstream_by_id(&id)
        .map_err(|e| e.to_string())?
        .map(|u| to_vo(&u))
        .ok_or_else(|| format!("上游 {} 不存在", id))
}

/// Delete an upstream by ID.
#[tauri::command]
pub fn delete_upstream(id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.db.delete_upstream(&id).map_err(|e| e.to_string())
}

/// Toggle an upstream's enabled state.
#[tauri::command]
pub fn toggle_upstream(
    id: String,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .db
        .toggle_upstream(&id, enabled)
        .map_err(|e| e.to_string())
}

/// Fetch the list of available models from an upstream provider's `/v1/models` endpoint.
#[tauri::command]
pub async fn fetch_upstream_models(
    base_url: String,
    api_key: String,
) -> Result<Vec<String>, String> {
    let url = format!(
        "{}/v1/models",
        base_url.trim_end_matches('/')
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let resp = client
        .get(&url)
        .bearer_auth(&api_key)
        .send()
        .await
        .map_err(|e| format!("请求上游失败: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("上游返回错误状态: {} — {}", status, body));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析响应失败: {}", e))?;

    // OpenAI /v1/models format: { "data": [ { "id": "gpt-4", ... }, ... ] }
    let models: Vec<String> = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("id").and_then(|v| v.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();

    Ok(models)
}

/// Fetch available models from an upstream by its ID.
/// Uses the stored (encrypted) API key, decrypted at runtime.
/// This allows fetching models when editing an existing upstream
/// without re-entering the API key.
#[tauri::command]
pub async fn fetch_upstream_models_by_id(
    id: String,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let upstream = state
        .db
        .get_upstream_by_id(&id)
        .map_err(|e| format!("数据库查询失败: {}", e))?
        .ok_or_else(|| "上游不存在".to_string())?;

    if upstream.api_key_encrypted.is_empty() {
        return Err("该上游未配置 API Key".to_string());
    }

    let api_key = state
        .crypto
        .decrypt_api_key(&upstream.api_key_encrypted)
        .map_err(|e| format!("API Key 解密失败: {}", e))?;

    let url = format!(
        "{}/v1/models",
        upstream.base_url.trim_end_matches('/')
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let resp = client
        .get(&url)
        .bearer_auth(&api_key)
        .send()
        .await
        .map_err(|e| format!("请求上游失败: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("上游返回错误状态: {} — {}", status, body));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析响应失败: {}", e))?;

    let models: Vec<String> = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    item.get("id")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(models)
}

// ============================================================================
// Pool Management Commands
// ============================================================================

/// Entry for associating an upstream with a pool.
#[derive(Debug, Clone, Deserialize)]
pub struct PoolUpstreamEntry {
    pub upstream_id: String,
    pub model: String,
}

/// Request body for creating a pool.
#[derive(Debug, Clone, Deserialize)]
pub struct CreatePoolRequest {
    pub name: String,
    pub display_name: String,
    #[serde(default = "default_concurrency")]
    pub max_concurrency: i32,
    #[serde(default)]
    pub thinking_enabled: bool,
    #[serde(default)]
    pub upstreams: Vec<PoolUpstreamEntry>,
}

/// Request body for updating a pool.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdatePoolRequest {
    pub display_name: String,
    pub max_concurrency: i32,
    pub thinking_enabled: bool,
}

fn default_concurrency() -> i32 {
    5
}

/// Pool info returned to the frontend, enriched with upstream count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolVO {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub round_robin_strategy: String,
    pub failover_enabled: bool,
    pub timeout_seconds: i32,
    pub max_concurrency: i32,
    pub thinking_enabled: bool,
    pub upstream_count: usize,
    pub created_at: String,
    pub updated_at: String,
}

fn pool_to_vo(p: &llm_api_proxy_lib::db::Pool, upstream_count: usize) -> PoolVO {
    PoolVO {
        id: p.id.clone(),
        name: p.name.clone(),
        display_name: p.display_name.clone(),
        round_robin_strategy: p.round_robin_strategy.clone(),
        failover_enabled: p.failover_enabled,
        timeout_seconds: p.timeout_seconds,
        max_concurrency: p.max_concurrency,
        thinking_enabled: p.thinking_enabled,
        upstream_count,
        created_at: p.created_at.clone(),
        updated_at: p.updated_at.clone(),
    }
}

/// Generate a unique ID for a pool.
fn generate_pool_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("pool_{:x}{:08x}", now.as_secs(), now.subsec_nanos())
}

/// List all pools with their upstream counts.
#[tauri::command]
pub fn list_pools(state: State<'_, AppState>) -> Result<Vec<PoolVO>, String> {
    let pools = state.db.get_pools().map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    for p in &pools {
        let upstreams = state.db.get_pool_upstreams(&p.id).unwrap_or_default();
        result.push(pool_to_vo(p, upstreams.len()));
    }
    Ok(result)
}

/// Get a single pool by ID.
#[tauri::command]
pub fn get_pool(id: String, state: State<'_, AppState>) -> Result<PoolVO, String> {
    let p = state
        .db
        .get_pool_by_id(&id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("号池 {} 不存在", id))?;
    let upstreams = state.db.get_pool_upstreams(&p.id).unwrap_or_default();
    Ok(pool_to_vo(&p, upstreams.len()))
}

/// Create a new pool and optionally associate upstreams.
#[tauri::command]
pub fn create_pool(
    req: CreatePoolRequest,
    state: State<'_, AppState>,
) -> Result<PoolVO, String> {
    let id = generate_pool_id();
    state
        .db
        .create_pool(&id, &req.name, &req.display_name, req.max_concurrency, req.thinking_enabled)
        .map_err(|e| e.to_string())?;

    for (i, entry) in req.upstreams.iter().enumerate() {
        state
            .db
            .add_upstream_to_pool(&id, &entry.upstream_id, i as i32, &entry.model)
            .map_err(|e| e.to_string())?;
    }

    let p = state
        .db
        .get_pool_by_id(&id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "创建后未能读取号池记录".to_string())?;
    Ok(pool_to_vo(&p, req.upstreams.len()))
}

/// Update pool configuration.
#[tauri::command]
pub fn update_pool(
    id: String,
    req: UpdatePoolRequest,
    state: State<'_, AppState>,
) -> Result<PoolVO, String> {
    state
        .db
        .update_pool(
            &id,
            &req.display_name,
            req.max_concurrency,
            req.thinking_enabled,
        )
        .map_err(|e| e.to_string())?;

    let p = state
        .db
        .get_pool_by_id(&id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("号池 {} 不存在", id))?;
    let upstreams = state.db.get_pool_upstreams(&p.id).unwrap_or_default();
    Ok(pool_to_vo(&p, upstreams.len()))
}

/// Delete a pool (cascade removes upstream associations).
#[tauri::command]
pub fn delete_pool(id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.db.delete_pool(&id).map_err(|e| e.to_string())
}

/// Add an upstream to a pool, specifying which model to use.
#[tauri::command]
pub fn add_upstream_to_pool(
    pool_id: String,
    upstream_id: String,
    model: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let existing = state.db.get_pool_upstreams(&pool_id).unwrap_or_default();
    let sort_order = existing.len() as i32;
    state
        .db
        .add_upstream_to_pool(&pool_id, &upstream_id, sort_order, &model)
        .map_err(|e| e.to_string())
}

/// Remove an upstream from a pool.
#[tauri::command]
pub fn remove_upstream_from_pool(
    pool_id: String,
    upstream_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .db
        .remove_upstream_from_pool(&pool_id, &upstream_id)
        .map_err(|e| e.to_string())
}

/// Get upstreams associated with a pool.
#[tauri::command]
pub fn get_pool_upstreams(
    pool_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<llm_api_proxy_lib::db::PoolUpstreamInfo>, String> {
    state
        .db
        .get_pool_upstreams(&pool_id)
        .map_err(|e| e.to_string())
}

/// Reorder upstreams within a pool (accepts ordered list of upstream IDs).
#[tauri::command]
pub fn reorder_pool_upstreams(
    pool_id: String,
    upstream_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .db
        .reorder_pool_upstreams(&pool_id, &upstream_ids)
        .map_err(|e| e.to_string())
}

// ============================================================================
// Dashboard & Stats Commands
// ============================================================================

/// Get aggregate statistics for the dashboard.
#[tauri::command]
pub fn get_stats(state: State<'_, AppState>) -> Result<llm_api_proxy_lib::db::Stats, String> {
    state.db.get_stats().map_err(|e| e.to_string())
}

/// Get gateway settings for dashboard display.
#[tauri::command]
pub fn get_gateway_info(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let settings = &state.settings;
    Ok(serde_json::json!({
        "listen_address": settings.listen_address,
        "listen_port": settings.listen_port,
        "gateway_url": settings.gateway_url(),
        "openai_base_url": settings.gateway_base_path(),
        "log_level": settings.log_level,
        "gateway_enabled": settings.gateway_enabled,
    }))
}

// ============================================================================
// Request Logs Commands
// ============================================================================

/// Get recent request logs with optional filtering.
#[tauri::command]
pub fn get_request_logs(
    start_date: Option<String>,
    end_date: Option<String>,
    pool_name: Option<String>,
    // Status code prefix for range filtering (2 = 2xx, 4 = 4xx, 5 = 5xx).
    status_prefix: Option<i32>,
    limit: Option<i64>,
    offset: Option<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<llm_api_proxy_lib::db::RequestLogEntry>, String> {
    let filter = llm_api_proxy_lib::db::LogFilter {
        start_date,
        end_date,
        pool_name,
        status_prefix,
        limit: limit.unwrap_or(50),
        offset: offset.unwrap_or(0),
    };
    state.db.get_recent_logs(&filter).map_err(|e| e.to_string())
}

// ============================================================================
// Token Usage Commands
// ============================================================================

/// Token usage response for an upstream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageResponse {
pub today: llm_api_proxy_lib::db::TokenTotals,
pub total: llm_api_proxy_lib::db::TokenTotals,
pub daily: Vec<llm_api_proxy_lib::db::DailyTokenUsage>,
pub per_model: Vec<llm_api_proxy_lib::db::ModelTokenUsage>,
}

/// Get token usage stats for an upstream (today + total + 30-day daily breakdown + per-model).
#[tauri::command]
pub fn get_upstream_token_usage(
upstream_id: String,
state: State<'_, AppState>,
) -> Result<TokenUsageResponse, String> {
let today = state
.db
.get_upstream_today_tokens(&upstream_id)
.map_err(|e| e.to_string())?;
let total = state
.db
.get_upstream_total_tokens(&upstream_id)
.map_err(|e| e.to_string())?;
let daily = state
.db
.get_upstream_token_stats(&upstream_id, 30)
.map_err(|e| e.to_string())?;
let per_model = state
.db
.get_upstream_model_token_stats(&upstream_id)
.map_err(|e| e.to_string())?;
Ok(TokenUsageResponse { today, total, daily, per_model })
}

/// Reset token stats for an upstream by deleting all its request logs.
#[tauri::command]
pub fn reset_upstream_token_stats(
upstream_id: String,
state: State<'_, AppState>,
) -> Result<i64, String> {
state
.db
.reset_upstream_token_stats(&upstream_id)
.map_err(|e| e.to_string())
}

/// Token usage detail response filtered by model (or all models).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDetailResponse {
pub today: llm_api_proxy_lib::db::TokenTotals,
pub total: llm_api_proxy_lib::db::TokenTotals,
pub daily: Vec<llm_api_proxy_lib::db::DailyTokenUsage>,
pub hourly: Vec<llm_api_proxy_lib::db::HourlyTokenUsage>,
}

/// Get token stats for an upstream filtered by model.
/// When model is None or empty, returns aggregated stats for all models.
#[tauri::command]
pub fn get_upstream_model_detail(
upstream_id: String,
model: Option<String>,
state: State<'_, AppState>,
) -> Result<ModelDetailResponse, String> {
let model_ref = model.as_deref().filter(|m| !m.is_empty());
let today = state
.db
.get_upstream_today_tokens_filtered(&upstream_id, model_ref)
.map_err(|e| e.to_string())?;
let total = state
.db
.get_upstream_total_tokens_filtered(&upstream_id, model_ref)
.map_err(|e| e.to_string())?;
let daily = state
.db
.get_upstream_token_stats_filtered(&upstream_id, model_ref, 30)
.map_err(|e| e.to_string())?;
let hourly = state
.db
.get_upstream_hourly_stats(&upstream_id, model_ref)
.map_err(|e| e.to_string())?;
Ok(ModelDetailResponse { today, total, daily, hourly })
}

// ============================================================================
// Health Check Commands
// ============================================================================

/// Result of a health check for a single upstream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResult {
    pub upstream_id: String,
    pub provider_name: String,
    pub status: String, // "ok", "timeout", "error"
    pub latency_ms: Option<u64>,
    pub message: Option<String>,
}

/// Internal helper: test connectivity to an upstream.
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

// ============================================================================
// Settings Commands
// ============================================================================

/// Settings data for the frontend settings page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsVO {
    pub listen_address: String,
    pub listen_port: u16,
    pub api_key: String,
    pub log_level: String,
    pub theme: String,
    #[serde(default = "default_true")]
    pub minimize_to_tray: bool,
}

/// Get current gateway settings.
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<SettingsVO, String> {
    let db = &state.db;
    Ok(SettingsVO {
        listen_address: db.get_setting("listen_address").ok().flatten()
            .unwrap_or_else(|| "127.0.0.1".to_string()),
        listen_port: db.get_setting("listen_port").ok().flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(47339),
        api_key: db.get_setting("gateway_api_key").ok().flatten()
            .unwrap_or_else(|| "sk-gateway-key".to_string()),
        log_level: db.get_setting("log_level").ok().flatten()
            .unwrap_or_else(|| "info".to_string()),
        theme: db.get_setting("theme").ok().flatten()
            .unwrap_or_else(|| "dark".to_string()),
        minimize_to_tray: db.get_setting("minimize_to_tray").ok().flatten()
            .map(|v| v == "true")
            .unwrap_or(true),
    })
}

/// Update gateway settings (persisted to database).
/// Note: listen_address and listen_port changes require app restart to take effect.
#[tauri::command]
pub fn update_settings(req: SettingsVO, state: State<'_, AppState>) -> Result<(), String> {
    // Update in-memory cache immediately, before any DB operations
    state.set_minimize_to_tray(req.minimize_to_tray);
    tracing::info!("AtomicBool cache updated: minimize_to_tray={}", req.minimize_to_tray);

    let db = &state.db;
    db.save_setting("listen_address", &req.listen_address).map_err(|e| e.to_string())?;
    db.save_setting("listen_port", &req.listen_port.to_string()).map_err(|e| e.to_string())?;
    db.save_setting("gateway_api_key", &req.api_key).map_err(|e| e.to_string())?;
    db.save_setting("log_level", &req.log_level).map_err(|e| e.to_string())?;
    db.save_setting("theme", &req.theme).map_err(|e| e.to_string())?;
    db.save_setting("minimize_to_tray", &req.minimize_to_tray.to_string()).map_err(|e| e.to_string())?;
    Ok(())
}

/// Set minimize-to-tray preference immediately (updates in-memory cache + DB).
/// Called directly when the toggle switch changes, independent of full settings save.
#[tauri::command]
pub fn set_minimize_to_tray(value: bool, state: State<'_, AppState>) -> Result<(), String> {
    state.set_minimize_to_tray(value);
    state.db.save_setting("minimize_to_tray", &value.to_string()).map_err(|e| e.to_string())?;
    tracing::info!("minimize_to_tray={} saved", value);
    Ok(())
}

/// Update only the theme setting, without overwriting other settings
/// (such as minimize_to_tray). This avoids race conditions where
/// toggleTheme calls update_settings with a stale minimize_to_tray value.
#[tauri::command]
pub fn set_theme(theme: String, state: State<'_, AppState>) -> Result<(), String> {
    state.db.save_setting("theme", &theme).map_err(|e| e.to_string())?;
    tracing::info!("theme={} saved", theme);
    Ok(())
}

/// Open a URL in the system's default browser.
/// Only http/https URLs are allowed to prevent command injection.
#[tauri::command]
pub fn open_external_url(url: String) -> Result<(), String> {
    // Validate URL scheme to prevent command injection
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("仅允许打开 http:// 或 https:// 开头的链接".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", &url])
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&url)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&url)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Read text from the system clipboard (bypasses WebView2 permission prompts).
#[tauri::command]
pub fn read_clipboard() -> Result<String, String> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|e| format!("Failed to access clipboard: {}", e))?;
    let text = clipboard.get_text()
        .map_err(|e| format!("Failed to read clipboard: {}", e))?;
    Ok(text)
}

/// Show a native file save dialog and write the given content to the chosen file.
/// Returns the chosen file path, or an error if the user cancelled or writing failed.
#[tauri::command]
pub async fn save_file_dialog(
    filename: String,
    content: String,
) -> Result<String, String> {
    let file_handle = rfd::AsyncFileDialog::new()
        .set_file_name(&filename)
        .add_filter("CSV 文件", &["csv"])
        .add_filter("所有文件", &["*"])
        .save_file()
        .await
        .ok_or_else(|| "用户取消了保存".to_string())?;

    let path = file_handle.path().to_path_buf();
    std::fs::write(&path, content.as_bytes())
        .map_err(|e| format!("写入文件失败: {}", e))?;

    Ok(path.to_string_lossy().to_string())
}
