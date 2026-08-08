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
    #[serde(default = "default_thinking_level")]
    pub thinking_level: String,
    #[serde(default)]
    pub thinking_custom_params: String,
    #[serde(default)]
    pub upstreams: Vec<PoolUpstreamEntry>,
}

/// Request body for updating a pool.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdatePoolRequest {
    pub display_name: String,
    pub max_concurrency: i32,
    pub thinking_enabled: bool,
    #[serde(default = "default_thinking_level")]
    pub thinking_level: String,
    #[serde(default)]
    pub thinking_custom_params: String,
}

fn default_concurrency() -> i32 {
    5
}

fn default_thinking_level() -> String {
    "off".to_string()
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
    pub thinking_level: String,
    pub thinking_custom_params: String,
    /// Pool-level aggregated capabilities (JSON), v14.
    pub capabilities: String,
    pub upstream_count: usize,
    pub created_at: String,
    pub updated_at: String,
}

// ============================================================================
// Helpers
// ============================================================================

pub(crate) fn pool_to_vo(p: &llm_api_proxy_lib::db::Pool, upstream_count: usize) -> PoolVO {
    PoolVO {
        id: p.id.clone(),
        name: p.name.clone(),
        display_name: p.display_name.clone(),
        round_robin_strategy: p.round_robin_strategy.clone(),
        failover_enabled: p.failover_enabled,
        timeout_seconds: p.timeout_seconds,
        max_concurrency: p.max_concurrency,
        thinking_enabled: p.thinking_enabled,
        thinking_level: p.thinking_level.clone(),
        thinking_custom_params: p.thinking_custom_params.clone(),
        capabilities: p.capabilities.clone(),
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
        .create_pool(
            &id,
            &req.name,
            &req.display_name,
            req.max_concurrency,
            req.thinking_enabled,
            &req.thinking_level,
            &req.thinking_custom_params,
            "",
        )
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
            &req.thinking_level,
            &req.thinking_custom_params,
            "",
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

/// Update a pool-upstream association: model and/or per-upstream thinking
/// level override (empty override = follow the pool level).
#[tauri::command]
pub fn update_pool_upstream(
    pool_id: String,
    upstream_id: String,
    model: Option<String>,
    thinking_level_override: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .db
        .update_pool_upstream(
            &pool_id,
            &upstream_id,
            model.as_deref(),
            thinking_level_override.as_deref(),
        )
        .map_err(|e| e.to_string())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_pool() -> llm_api_proxy_lib::db::Pool {
        llm_api_proxy_lib::db::Pool {
            id: "pool_test_001".to_string(),
            name: "gpt-4-pool".to_string(),
            display_name: "GPT-4".to_string(),
            round_robin_strategy: "round_robin".to_string(),
            failover_enabled: true,
            timeout_seconds: 30,
            max_concurrency: 5,
            thinking_enabled: false,
            thinking_level: "high".to_string(),
            thinking_custom_params: String::new(),
            capabilities: String::new(),
            created_at: "2026-07-28 09:00:00".to_string(),
            updated_at: "2026-07-28 09:00:00".to_string(),
        }
    }

    #[test]
    fn test_pool_to_vo_maps_all_fields() {
        let pool = make_test_pool();
        let vo = pool_to_vo(&pool, 3);
        assert_eq!(vo.id, "pool_test_001");
        assert_eq!(vo.name, "gpt-4-pool");
        assert_eq!(vo.display_name, "GPT-4");
        assert_eq!(vo.round_robin_strategy, "round_robin");
        assert!(vo.failover_enabled);
        assert_eq!(vo.timeout_seconds, 30);
        assert_eq!(vo.max_concurrency, 5);
        assert!(!vo.thinking_enabled);
        assert_eq!(vo.thinking_level, "high");
        assert_eq!(vo.upstream_count, 3);
        assert_eq!(vo.created_at, "2026-07-28 09:00:00");
    }

    #[test]
    fn test_pool_to_vo_zero_upstreams() {
        let pool = make_test_pool();
        let vo = pool_to_vo(&pool, 0);
        assert_eq!(vo.upstream_count, 0);
    }

    #[test]
    fn test_create_pool_request_deserialization_defaults() {
        let json = r#"{
            "name": "test-pool",
            "display_name": "Test Pool"
        }"#;
        let req: CreatePoolRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "test-pool");
        assert_eq!(req.display_name, "Test Pool");
        assert_eq!(req.max_concurrency, 5); // default
        assert!(!req.thinking_enabled); // default
        assert_eq!(req.thinking_level, "off"); // default
        assert!(req.thinking_custom_params.is_empty()); // default
        assert!(req.upstreams.is_empty()); // default
    }

    #[test]
    fn test_create_pool_request_deserialization_full() {
        let json = r#"{
            "name": "prod-pool",
            "display_name": "Production",
            "max_concurrency": 10,
            "thinking_enabled": true,
            "thinking_level": "max",
            "thinking_custom_params": "{\"x\":1}",
            "upstreams": [
                {"upstream_id": "up_001", "model": "gpt-4"},
                {"upstream_id": "up_002", "model": "gpt-4"}
            ]
        }"#;
        let req: CreatePoolRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "prod-pool");
        assert_eq!(req.max_concurrency, 10);
        assert!(req.thinking_enabled);
        assert_eq!(req.thinking_level, "max");
        assert_eq!(req.thinking_custom_params, "{\"x\":1}");
        assert_eq!(req.upstreams.len(), 2);
        assert_eq!(req.upstreams[0].upstream_id, "up_001");
        assert_eq!(req.upstreams[0].model, "gpt-4");
        assert_eq!(req.upstreams[1].upstream_id, "up_002");
    }

    #[test]
    fn test_update_pool_request_deserialization() {
        let json = r#"{
            "display_name": "Updated Name",
            "max_concurrency": 20,
            "thinking_enabled": true,
            "thinking_level": "low",
            "thinking_custom_params": ""
        }"#;
        let req: UpdatePoolRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.display_name, "Updated Name");
        assert_eq!(req.max_concurrency, 20);
        assert!(req.thinking_enabled);
        assert_eq!(req.thinking_level, "low");
    }

    #[test]
    fn test_update_pool_request_thinking_level_default() {
        // Backward compat: old clients without thinking_level fields
        let json = r#"{
            "display_name": "Legacy",
            "max_concurrency": 5,
            "thinking_enabled": false
        }"#;
        let req: UpdatePoolRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.thinking_level, "off");
        assert!(req.thinking_custom_params.is_empty());
    }

    #[test]
    fn test_pool_upstream_entry_deserialization() {
        let json = r#"{"upstream_id": "up_abc", "model": "claude-3"}"#;
        let entry: PoolUpstreamEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.upstream_id, "up_abc");
        assert_eq!(entry.model, "claude-3");
    }

    #[test]
    fn test_generate_pool_id_format() {
        let id = generate_pool_id();
        assert!(id.starts_with("pool_"));
        assert!(id.len() > "pool_".len());
    }

    #[test]
    fn test_generate_pool_id_uniqueness() {
        let id1 = generate_pool_id();
        std::thread::sleep(std::time::Duration::from_millis(1));
        let id2 = generate_pool_id();
        assert_ne!(id1, id2);
    }
}
