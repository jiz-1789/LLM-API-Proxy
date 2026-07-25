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
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("up_{:x}{:08x}", secs, nanos)
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
        return Err(format!(
            "上游返回错误状态: {}",
            resp.status()
        ));
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
    pub circuit_breaker_threshold: i32,
    pub circuit_breaker_duration_seconds: i32,
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
    pub circuit_breaker_threshold: i32,
    pub circuit_breaker_duration_seconds: i32,
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
        circuit_breaker_threshold: p.circuit_breaker_threshold,
        circuit_breaker_duration_seconds: p.circuit_breaker_duration_seconds,
        upstream_count,
        created_at: p.created_at.clone(),
        updated_at: p.updated_at.clone(),
    }
}

/// Generate a unique ID for a pool.
fn generate_pool_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("pool_{:x}{:08x}", secs, nanos)
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
            req.circuit_breaker_threshold,
            req.circuit_breaker_duration_seconds,
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
    status_code: Option<i32>,
    limit: Option<i64>,
    offset: Option<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<llm_api_proxy_lib::db::RequestLogEntry>, String> {
    let filter = llm_api_proxy_lib::db::LogFilter {
        start_date,
        end_date,
        pool_name,
        status_code,
        limit: limit.unwrap_or(50),
        offset: offset.unwrap_or(0),
    };
    state.db.get_recent_logs(&filter).map_err(|e| e.to_string())
}
