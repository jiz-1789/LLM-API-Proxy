use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};

use llm_api_proxy_lib::AppState;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

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

fn default_zh() -> String {
    "zh".to_string()
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
    #[serde(default = "default_zh")]
    pub language: String,
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
        language: db.get_setting("language").ok().flatten()
            .unwrap_or_else(|| "zh".to_string()),
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
    db.save_setting("language", &req.language).map_err(|e| e.to_string())?;
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

// ============================================================================
// Update Check
// ============================================================================

/// Release info returned to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckResult {
    pub has_update: bool,
    pub current_version: String,
    pub latest_version: String,
    pub release_notes: String,
    pub published_at: String,
    pub source: String,
    pub github_release_url: String,
    pub github_download_url: String,
    pub gitee_release_url: String,
    pub gitee_download_url: String,
}

/// Parsed release info from a single source (GitHub or Gitee).
struct ParsedRelease {
    latest_version: String,
    release_url: String,
    download_url: String,
    release_notes: String,
    published_at: String,
}

/// Extract the portable exe download URL from a JSON assets array.
fn extract_portable_download_url(json: &serde_json::Value) -> String {
    json.get("assets")
        .and_then(|a| a.as_array())
        .and_then(|assets| {
            assets.iter().find_map(|asset| {
                let name = asset.get("name")?.as_str()?;
                if name.contains("portable") && name.ends_with(".exe") {
                    asset.get("browser_download_url")?.as_str()
                } else {
                    None
                }
            })
        })
        .unwrap_or("")
        .to_string()
}

/// Fetch the latest release from GitHub API.
async fn fetch_github_release(client: &reqwest::Client) -> Result<ParsedRelease, String> {
    let url = "https://api.github.com/repos/jiz-1789/LLM-API-Proxy/releases/latest";
    let resp = client
        .get(url)
        .header("User-Agent", "LLM-API-Proxy")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("请求 GitHub API 失败: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API 返回错误: HTTP {}", resp.status()));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析 GitHub API 响应失败: {}", e))?;

    let tag = json.get("tag_name").and_then(|v| v.as_str()).unwrap_or("");
    let latest_version = tag.trim_start_matches('v').to_string();
    let release_url = json
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://github.com/jiz-1789/LLM-API-Proxy/releases")
        .to_string();
    let download_url = extract_portable_download_url(&json);
    let release_notes = json.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let published_at = json.get("published_at").and_then(|v| v.as_str()).unwrap_or("").to_string();

    Ok(ParsedRelease {
        latest_version,
        release_url,
        download_url,
        release_notes,
        published_at,
    })
}

/// Fetch the latest release from Gitee API.
async fn fetch_gitee_release(client: &reqwest::Client) -> Result<ParsedRelease, String> {
    let url = "https://gitee.com/api/v5/repos/yilichenaiosi/LLM-API-Proxy/releases/latest";
    let resp = client
        .get(url)
        .header("User-Agent", "LLM-API-Proxy")
        .send()
        .await
        .map_err(|e| format!("请求 Gitee API 失败: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Gitee API 返回错误: HTTP {}", resp.status()));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析 Gitee API 响应失败: {}", e))?;

    let tag = json.get("tag_name").and_then(|v| v.as_str()).unwrap_or("");
    let latest_version = tag.trim_start_matches('v').to_string();
    let release_url = json
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://gitee.com/yilichenaiosi/LLM-API-Proxy/releases")
        .to_string();
    let download_url = extract_portable_download_url(&json);
    let release_notes = json.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();
    // Gitee uses "created_at" instead of "published_at"
    let published_at = json
        .get("created_at")
        .or_else(|| json.get("published_at"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(ParsedRelease {
        latest_version,
        release_url,
        download_url,
        release_notes,
        published_at,
    })
}

/// Check for updates from GitHub (primary) and Gitee (fallback).
/// Tries GitHub first; if it fails, falls back to Gitee.
/// Always attempts to fetch Gitee download URL for the dual-button UI.
#[tauri::command]
pub async fn check_for_updates() -> Result<UpdateCheckResult, String> {
    let current_version = env!("CARGO_PKG_VERSION");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    // Try GitHub first
    let github_result = fetch_github_release(&client).await;

    // Always try Gitee (for the Gitee download URL)
    let gitee_result = fetch_gitee_release(&client).await;

    // Determine which source to use for version info
    let (primary, source) = match (&github_result, &gitee_result) {
        (Ok(gh), _) => (gh.clone(), "github"),
        (Err(_), Ok(gitee)) => (gitee.clone(), "gitee"),
        (Err(gh_err), Err(_)) => {
            return Err(format!(
                "GitHub 和 Gitee 均无法访问: {}",
                gh_err
            ));
        }
    };

    let has_update = compare_versions(&primary.latest_version, current_version);

    // Extract URLs from whichever source succeeded
    let github_release_url = github_result.as_ref().map(|r| r.release_url.clone()).unwrap_or_default();
    let github_download_url = github_result.as_ref().map(|r| r.download_url.clone()).unwrap_or_default();
    let gitee_release_url = gitee_result.as_ref().map(|r| r.release_url.clone()).unwrap_or_default();
    let gitee_download_url = gitee_result.as_ref().map(|r| r.download_url.clone()).unwrap_or_default();

    Ok(UpdateCheckResult {
        has_update,
        current_version: current_version.to_string(),
        latest_version: primary.latest_version.clone(),
        release_notes: primary.release_notes.clone(),
        published_at: primary.published_at.clone(),
        source: source.to_string(),
        github_release_url,
        github_download_url,
        gitee_release_url,
        gitee_download_url,
    })
}

/// Compare two semver strings. Returns true if `latest` > `current`.
fn compare_versions(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> {
        s.split('.')
            .filter_map(|p| p.parse::<u32>().ok())
            .collect()
    };
    let l = parse(latest);
    let c = parse(current);
    for i in 0..l.len().max(c.len()) {
        let lv = l.get(i).copied().unwrap_or(0);
        let cv = c.get(i).copied().unwrap_or(0);
        if lv > cv {
            return true;
        }
        if lv < cv {
            return false;
        }
    }
    false
}

/// Download progress payload sent to the frontend via Tauri events.
#[derive(Clone, Serialize)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: u64,
    pub percentage: f64,
}

/// Check if a pending update (downloaded but not yet applied) exists.
/// Also cleans up any stale partial download file.
#[tauri::command]
pub fn check_pending_update() -> Result<bool, String> {
    let current_exe = std::env::current_exe()
        .map_err(|e| format!("获取当前程序路径失败: {}", e))?;
    let exe_dir = current_exe.parent()
        .ok_or("无法获取程序目录")?;

    // Clean up stale partial download if exists
    let downloading = exe_dir.join("_update_downloading.exe");
    if downloading.exists() {
        let _ = std::fs::remove_file(&downloading);
    }

    let pending = exe_dir.join("_update_pending.exe");
    Ok(pending.exists())
}

/// Download the new portable exe (download only, does not exit the app).
/// Streams the download with progress events via Tauri events.
/// The file is saved as _update_pending.exe on completion.
#[tauri::command]
pub async fn download_update(
    download_url: String,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    // Validate URL scheme
    if !download_url.starts_with("https://") {
        return Err("下载地址无效，仅支持 HTTPS".to_string());
    }

    // Get current exe path and directory
    let current_exe = std::env::current_exe()
        .map_err(|e| format!("获取当前程序路径失败: {}", e))?;
    let exe_dir = current_exe.parent()
        .ok_or("无法获取程序目录")?;

    // Download to _update_downloading.exe first, rename to _update_pending.exe on success
    let downloading_path = exe_dir.join("_update_downloading.exe");
    let pending_path = exe_dir.join("_update_pending.exe");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let resp = client
        .get(&download_url)
        .header("User-Agent", "LLM-API-Proxy")
        .send()
        .await
        .map_err(|e| format!("下载失败: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("下载失败: HTTP {}", resp.status()));
    }

    let total_size = resp.content_length().unwrap_or(0);

    // Stream the download with progress reporting
    let mut file = std::fs::File::create(&downloading_path)
        .map_err(|e| format!("创建临时文件失败: {}", e))?;

    use std::io::Write;
    let mut resp = resp;
    let mut downloaded: u64 = 0;
    let mut last_report: u64 = 0;

    loop {
        let chunk = resp.chunk()
            .await
            .map_err(|e| {
                // Clean up partial download on error
                let _ = std::fs::remove_file(&downloading_path);
                format!("读取下载数据失败: {}", e)
            })?
            .unwrap_or_default();

        if chunk.is_empty() {
            break;
        }

        file.write_all(&chunk)
            .map_err(|e| {
                let _ = std::fs::remove_file(&downloading_path);
                format!("写入临时文件失败: {}", e)
            })?;

        downloaded += chunk.len() as u64;

        // Report progress at most every 100KB to avoid flooding the event bus
        if downloaded - last_report >= 102_400 || (total_size > 0 && downloaded == total_size) {
            last_report = downloaded;
            let percentage = if total_size > 0 {
                (downloaded as f64 / total_size as f64) * 100.0
            } else {
                0.0
            };
            let _ = app_handle.emit("update-progress", DownloadProgress {
                downloaded,
                total: total_size,
                percentage,
            });
        }
    }

    drop(file); // Ensure the file handle is closed before rename

    tracing::info!("Download complete: {} bytes", downloaded);

    // Rename downloading file to pending file
    std::fs::rename(&downloading_path, &pending_path)
        .map_err(|e| {
            let _ = std::fs::remove_file(&downloading_path);
            format!("重命名下载文件失败: {}", e)
        })?;

    Ok(())
}

/// Apply a pending update: create a batch updater script, shut down the
/// gateway, and exit the app. The batch script replaces the exe and
/// restarts the new version. This works regardless of minimize-to-tray
/// settings — the app fully exits.
#[tauri::command]
pub fn apply_update(app_handle: tauri::AppHandle) -> Result<(), String> {
    let current_exe = std::env::current_exe()
        .map_err(|e| format!("获取当前程序路径失败: {}", e))?;
    let exe_dir = current_exe.parent()
        .ok_or("无法获取程序目录")?;

    // Verify the pending update file exists
    let pending_path = exe_dir.join("_update_pending.exe");
    if !pending_path.exists() {
        return Err("未找到待安装的更新文件，请重新下载".to_string());
    }

    let exe_name = current_exe.file_name()
        .and_then(|n| n.to_str())
        .ok_or("无法获取程序文件名")?;

    let exe_path = current_exe.to_string_lossy().to_string();

    let batch_content = format!(
        "@echo off\r\n\
         :: Wait for the app to fully exit\r\n\
         timeout /t 2 /nobreak >nul\r\n\
         :: Retry loop: rename old exe to .bak to verify it is unlocked\r\n\
         :retry\r\n\
         ren \"{exe_name}\" \"{exe_name}.bak\" 2>nul\r\n\
         if errorlevel 1 (\r\n\
             timeout /t 1 /nobreak >nul\r\n\
             goto retry\r\n\
         )\r\n\
         :: Move the new exe into place\r\n\
         move /y \"_update_pending.exe\" \"{exe_name}\"\r\n\
         :: Delete the old exe backup\r\n\
         del \"{exe_name}.bak\" 2>nul\r\n\
         :: Ensure a desktop shortcut exists (create if missing)\r\n\
         powershell -NoProfile -Command \"$desktop=[Environment]::GetFolderPath('Desktop'); $lnk=Join-Path $desktop 'LLM-API-Proxy.lnk'; if (-not (Test-Path $lnk)) {{ $ws=New-Object -ComObject WScript.Shell; $s=$ws.CreateShortcut($lnk); $s.TargetPath='{exe_path}'; $s.WorkingDirectory='{exe_dir}'; $s.IconLocation='{exe_path},0'; $s.Description='LLM-API-Proxy'; $s.Save() }}\" 2>nul\r\n\
         :: Refresh Windows icon cache to fix desktop shortcuts\r\n\
         ie4uinit.exe -show 2>nul\r\n\
         :: Start the new version\r\n\
         start \"\" \"{exe_name}\"\r\n\
         :: Clean up this batch script\r\n\
         del \"%~f0\"\r\n",
        exe_name = exe_name,
        exe_path = exe_path,
        exe_dir = exe_dir.to_string_lossy(),
    );

    let batch_path = exe_dir.join("_updater.bat");
    std::fs::write(&batch_path, batch_content)
        .map_err(|e| format!("创建更新脚本失败: {}", e))?;

    tracing::info!("Update script created at {}, exiting app to apply update", batch_path.display());

    // Gracefully shut down the gateway server before exiting
    let state = app_handle.state::<AppState>();
    state.shutdown();

    // Run the batch script in a detached process
    std::process::Command::new("cmd")
        .args(["/c", "start", "", "/b", "_updater.bat"])
        .current_dir(exe_dir)
        .creation_flags(0x00000008) // DETACHED_PROCESS
        .spawn()
        .map_err(|e| format!("启动更新脚本失败: {}", e))?;

    // Exit the app so the batch script can replace the exe
    app_handle.exit(0);

    Ok(())
}
