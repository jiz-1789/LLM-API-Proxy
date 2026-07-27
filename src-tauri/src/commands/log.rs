use serde::{Deserialize, Serialize};
use tauri::State;

use llm_api_proxy_lib::AppState;

// ============================================================================
// Token Usage DTOs
// ============================================================================

/// Token usage response for an upstream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageResponse {
    pub today: llm_api_proxy_lib::db::TokenTotals,
    pub total: llm_api_proxy_lib::db::TokenTotals,
    pub daily: Vec<llm_api_proxy_lib::db::DailyTokenUsage>,
    pub per_model: Vec<llm_api_proxy_lib::db::ModelTokenUsage>,
}

/// Token usage detail response filtered by model (or all models).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDetailResponse {
    pub today: llm_api_proxy_lib::db::TokenTotals,
    pub total: llm_api_proxy_lib::db::TokenTotals,
    pub daily: Vec<llm_api_proxy_lib::db::DailyTokenUsage>,
    pub hourly: Vec<llm_api_proxy_lib::db::HourlyTokenUsage>,
}

// ============================================================================
// Commands
// ============================================================================

/// Get recent request logs with optional filtering.
#[tauri::command]
pub fn get_request_logs(
    start_date: Option<String>,
    end_date: Option<String>,
    pool_name: Option<String>,
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

/// Get token usage stats for an upstream (today + total + 30-day daily breakdown + per-model).
#[tauri::command]
pub fn get_upstream_token_usage(
    upstream_id: String,
    state: State<'_, AppState>,
) -> Result<TokenUsageResponse, String> {
    let today = state
        .db
        .get_upstream_today_tokens(&upstream_id, None)
        .map_err(|e| e.to_string())?;
    let total = state
        .db
        .get_upstream_total_tokens(&upstream_id, None)
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

/// Get token stats for an upstream filtered by model.
#[tauri::command]
pub fn get_upstream_model_detail(
    upstream_id: String,
    model: Option<String>,
    state: State<'_, AppState>,
) -> Result<ModelDetailResponse, String> {
    let model_ref = model.as_deref().filter(|m| !m.is_empty());
    let today = state
        .db
        .get_upstream_today_tokens(&upstream_id, model_ref)
        .map_err(|e| e.to_string())?;
    let total = state
        .db
        .get_upstream_total_tokens(&upstream_id, model_ref)
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
