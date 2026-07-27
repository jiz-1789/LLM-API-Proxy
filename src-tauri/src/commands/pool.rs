use serde::{Deserialize, Serialize};
use tauri::State;

use llm_api_proxy_lib::AppState;

use super::generate_pool_id;

// ============================================================================
// DTO Types
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

// ============================================================================
// Helpers
// ============================================================================

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

// ============================================================================
// Commands
// ============================================================================

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
