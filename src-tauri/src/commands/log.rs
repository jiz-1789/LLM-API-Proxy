use serde::{Deserialize, Serialize};
use tauri::State;

use llm_api_proxy_lib::AppState;

// ============================================================================
// Token Usage DTOs
// ============================================================================

/// Request statistics entry for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestStatsVO {
    pub upstream_id: Option<String>,
    pub upstream_name: Option<String>,
    pub model: Option<String>,
    pub total_count: i64,
    pub success_count: i64,
    pub error_count: i64,
    pub success_rate: f64,
    pub avg_response_time_ms: f64,
    pub p95_response_time_ms: i32,
    pub p99_response_time_ms: i32,
}

impl From<llm_api_proxy_lib::db::RequestStatsEntry> for RequestStatsVO {
    fn from(e: llm_api_proxy_lib::db::RequestStatsEntry) -> Self {
        Self {
            upstream_id: e.upstream_id,
            upstream_name: e.upstream_name,
            model: e.model,
            total_count: e.total_count,
            success_count: e.success_count,
            error_count: e.error_count,
            success_rate: e.success_rate,
            avg_response_time_ms: e.avg_response_time_ms,
            p95_response_time_ms: e.p95_response_time_ms,
            p99_response_time_ms: e.p99_response_time_ms,
        }
    }
}

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
#[allow(clippy::too_many_arguments)]
pub fn get_request_logs(
    start_date: Option<String>,
    end_date: Option<String>,
    pool_name: Option<String>,
    upstream_id: Option<String>,
    model: Option<String>,
    status_prefix: Option<i32>,
    limit: Option<i64>,
    offset: Option<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<llm_api_proxy_lib::db::RequestLogEntry>, String> {
    let filter = llm_api_proxy_lib::db::LogFilter {
        start_date,
        end_date,
        pool_name,
        upstream_id,
        model,
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

/// Export request logs as CSV or JSON string.
/// Supports filtering by time range, pool, upstream, model, and status code.
/// Returns the formatted content; the frontend uses `save_file_dialog` to write it.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn export_request_logs(
    format: String,
    start_date: Option<String>,
    end_date: Option<String>,
    pool_name: Option<String>,
    upstream_id: Option<String>,
    model: Option<String>,
    status_prefix: Option<i32>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let filter = llm_api_proxy_lib::db::LogFilter {
        start_date,
        end_date,
        pool_name,
        upstream_id,
        model,
        status_prefix,
        limit: 10000,
        offset: 0,
    };
    let logs = state.db.get_recent_logs(&filter).map_err(|e| e.to_string())?;

    // Resolve upstream names
    let upstreams = state.db.get_upstreams().map_err(|e| e.to_string())?;
    let upstream_map: std::collections::HashMap<String, String> = upstreams
        .iter()
        .map(|u| (u.id.clone(), u.provider_name.clone()))
        .collect();

    match format.as_str() {
        "json" => {
            let json_entries: Vec<serde_json::Value> = logs
                .iter()
                .map(|l| {
                    let upstream_name = l.upstream_id.as_ref().and_then(|id| upstream_map.get(id)).cloned();
                    serde_json::json!({
                        "id": l.id,
                        "request_id": l.request_id,
                        "created_at": l.created_at,
                        "pool_name": l.pool_name,
                        "upstream_id": l.upstream_id,
                        "upstream_name": upstream_name,
                        "model": l.model,
                        "method": l.method,
                        "endpoint": l.endpoint,
                        "status_code": l.status_code,
                        "response_time_ms": l.response_time_ms,
                        "is_streaming": l.is_streaming,
                        "prompt_tokens": l.prompt_tokens,
                        "completion_tokens": l.completion_tokens,
                        "total_tokens": l.total_tokens,
                        "failed_upstreams": l.failed_upstreams,
                    })
                })
                .collect();
            serde_json::to_string_pretty(&json_entries).map_err(|e| e.to_string())
        }
        _ => {
            // CSV format (default)
            let mut csv = String::from("\u{FEFF}"); // BOM for Excel UTF-8
            csv.push_str("time,pool,upstream_id,upstream_name,model,method,endpoint,status_code,response_time_ms,is_streaming,prompt_tokens,completion_tokens,total_tokens,failed_upstreams\n");
            for l in &logs {
                let upstream_name = l.upstream_id.as_ref().and_then(|id| upstream_map.get(id)).map(|s| s.as_str()).unwrap_or("");
                let row = [
                    l.created_at.as_str(),
                    l.pool_name.as_deref().unwrap_or(""),
                    l.upstream_id.as_deref().unwrap_or(""),
                    upstream_name,
                    l.model.as_deref().unwrap_or(""),
                    l.method.as_str(),
                    l.endpoint.as_str(),
                    &l.status_code.to_string(),
                    &l.response_time_ms.to_string(),
                    if l.is_streaming { "true" } else { "false" },
                    &l.prompt_tokens.to_string(),
                    &l.completion_tokens.to_string(),
                    &l.total_tokens.to_string(),
                    &l.failed_upstreams,
                ];
                let escaped: Vec<String> = row.iter().map(|s| {
                    let s = String::from(*s);
                    if s.contains(',') || s.contains('"') || s.contains('\n') {
                        format!("\"{}\"", s.replace('"', "\"\""))
                    } else {
                        s
                    }
                }).collect();
                csv.push_str(&escaped.join(","));
                csv.push('\n');
            }
            Ok(csv)
        }
    }
}

/// Get aggregated request statistics grouped by upstream + model.
/// Returns success rate, avg/P95/P99 latency for each group.
#[tauri::command]
pub fn get_request_stats(
    start_date: Option<String>,
    end_date: Option<String>,
    pool_name: Option<String>,
    upstream_id: Option<String>,
    model: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<RequestStatsVO>, String> {
    let filter = llm_api_proxy_lib::db::StatsFilter {
        start_date,
        end_date,
        pool_name,
        upstream_id,
        model,
    };
    let mut entries = state.db.get_request_stats(&filter).map_err(|e| e.to_string())?;

    // Resolve upstream names
    let upstreams = state.db.get_upstreams().map_err(|e| e.to_string())?;
    let upstream_map: std::collections::HashMap<String, String> = upstreams
        .iter()
        .map(|u| (u.id.clone(), u.provider_name.clone()))
        .collect();
    for entry in &mut entries {
        if let Some(id) = &entry.upstream_id {
            entry.upstream_name = upstream_map.get(id).cloned();
        }
    }

    Ok(entries.into_iter().map(Into::into).collect())
}

/// Failover event entry for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverEventVO {
    pub id: String,
    pub request_id: String,
    pub created_at: String,
    pub pool_name: Option<String>,
    pub upstream_id: Option<String>,
    pub upstream_name: Option<String>,
    pub model: Option<String>,
    pub status_code: i32,
    pub response_time_ms: i32,
    pub is_streaming: bool,
    pub failed_upstreams: Vec<llm_api_proxy_lib::db::FailedUpstreamEntry>,
    pub total_attempts: i32,
}

impl From<llm_api_proxy_lib::db::FailoverEvent> for FailoverEventVO {
    fn from(e: llm_api_proxy_lib::db::FailoverEvent) -> Self {
        Self {
            id: e.id,
            request_id: e.request_id,
            created_at: e.created_at,
            pool_name: e.pool_name,
            upstream_id: e.upstream_id,
            upstream_name: e.upstream_name,
            model: e.model,
            status_code: e.status_code,
            response_time_ms: e.response_time_ms,
            is_streaming: e.is_streaming,
            failed_upstreams: e.failed_upstreams,
            total_attempts: e.total_attempts,
        }
    }
}

/// Response for failover events query with pagination info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverEventsResponse {
    pub events: Vec<FailoverEventVO>,
    pub total: i64,
}

/// Get failover events (request logs that had upstream failures).
/// Supports filtering by time range and pool name, with pagination.
#[tauri::command]
pub fn get_failover_events(
    start_date: Option<String>,
    end_date: Option<String>,
    pool_name: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    state: State<'_, AppState>,
) -> Result<FailoverEventsResponse, String> {
    let limit = limit.unwrap_or(50).min(500);
    let offset = offset.unwrap_or(0);

    let mut events = state.db
        .get_failover_events(
            start_date.as_deref(),
            end_date.as_deref(),
            pool_name.as_deref(),
            limit,
            offset,
        )
        .map_err(|e| e.to_string())?;

    // Resolve upstream names
    let upstreams = state.db.get_upstreams().map_err(|e| e.to_string())?;
    let upstream_map: std::collections::HashMap<String, String> = upstreams
        .iter()
        .map(|u| (u.id.clone(), u.provider_name.clone()))
        .collect();
    for event in &mut events {
        if let Some(id) = &event.upstream_id {
            event.upstream_name = upstream_map.get(id).cloned();
        }
    }

    let total = state.db
        .count_failover_events(
            start_date.as_deref(),
            end_date.as_deref(),
            pool_name.as_deref(),
        )
        .map_err(|e| e.to_string())?;

    Ok(FailoverEventsResponse {
        events: events.into_iter().map(Into::into).collect(),
        total,
    })
}
