use serde::{Deserialize, Serialize};
use tauri::State;

use llm_api_proxy_lib::AppState;

use super::generate_id;

// ============================================================================
// DTO Types
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
    pub last_success_time: Option<String>,
    pub last_error_reason: Option<String>,
    pub recovered_at: Option<String>,
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
    #[serde(default = "super::default_true")]
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

// ============================================================================
// Helpers
// ============================================================================

pub(crate) fn to_vo(u: &llm_api_proxy_lib::db::Upstream) -> UpstreamVO {
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
        last_success_time: u.last_success_time.clone(),
        last_error_reason: u.last_error_reason.clone(),
        recovered_at: u.recovered_at.clone(),
        created_at: u.created_at.clone(),
        updated_at: u.updated_at.clone(),
    }
}

// ============================================================================
// Commands
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
    let url = format!("{}/v1/models", base_url.trim_end_matches('/'));

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
                .filter_map(|item| item.get("id").and_then(|v| v.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();

    Ok(models)
}

/// Fetch available models from an upstream by its ID.
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

    let url = format!("{}/v1/models", upstream.base_url.trim_end_matches('/'));

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
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_upstream() -> llm_api_proxy_lib::db::Upstream {
        llm_api_proxy_lib::db::Upstream {
            id: "up_test_001".to_string(),
            provider_name: "OpenAI".to_string(),
            base_url: "https://api.openai.com".to_string(),
            api_key_encrypted: vec![1, 2, 3, 4],
            selected_model: "gpt-4".to_string(),
            available_models: r#"["gpt-4","gpt-3.5-turbo"]"#.to_string(),
            enabled: true,
            remark: "test upstream".to_string(),
            status: "healthy".to_string(),
            failure_count: 0,
            last_failure_time: None,
            last_success_time: Some("2026-07-28 10:00:00".to_string()),
            last_error_reason: None,
            recovered_at: None,
            created_at: "2026-07-28 09:00:00".to_string(),
            updated_at: "2026-07-28 09:00:00".to_string(),
        }
    }

    #[test]
    fn test_to_vo_masks_api_key() {
        let upstream = make_test_upstream();
        let vo = to_vo(&upstream);
        assert_eq!(vo.api_key_masked, "••••••••");
    }

    #[test]
    fn test_to_vo_empty_key_shows_empty() {
        let mut upstream = make_test_upstream();
        upstream.api_key_encrypted = vec![];
        let vo = to_vo(&upstream);
        assert_eq!(vo.api_key_masked, "");
    }

    #[test]
    fn test_to_vo_parses_available_models() {
        let upstream = make_test_upstream();
        let vo = to_vo(&upstream);
        assert_eq!(vo.available_models, vec!["gpt-4", "gpt-3.5-turbo"]);
    }

    #[test]
    fn test_to_vo_invalid_models_json_falls_back_to_empty() {
        let mut upstream = make_test_upstream();
        upstream.available_models = "not valid json".to_string();
        let vo = to_vo(&upstream);
        assert!(vo.available_models.is_empty());
    }

    #[test]
    fn test_to_vo_maps_all_fields() {
        let upstream = make_test_upstream();
        let vo = to_vo(&upstream);
        assert_eq!(vo.id, "up_test_001");
        assert_eq!(vo.provider_name, "OpenAI");
        assert_eq!(vo.base_url, "https://api.openai.com");
        assert_eq!(vo.selected_model, "gpt-4");
        assert!(vo.enabled);
        assert_eq!(vo.remark, "test upstream");
        assert_eq!(vo.status, "healthy");
        assert_eq!(vo.failure_count, 0);
        assert_eq!(vo.last_success_time.as_deref(), Some("2026-07-28 10:00:00"));
        assert!(vo.last_failure_time.is_none());
        assert!(vo.last_error_reason.is_none());
        assert!(vo.recovered_at.is_none());
    }

    #[test]
    fn test_create_upstream_request_deserialization_defaults() {
        let json = r#"{
            "provider_name": "DeepSeek",
            "base_url": "https://api.deepseek.com",
            "api_key": "sk-test",
            "selected_model": "deepseek-chat"
        }"#;
        let req: CreateUpstreamRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.provider_name, "DeepSeek");
        assert!(req.available_models.is_empty());
        assert!(req.enabled);
        assert!(req.remark.is_empty());
    }

    #[test]
    fn test_create_upstream_request_deserialization_full() {
        let json = r#"{
            "provider_name": "OpenAI",
            "base_url": "https://api.openai.com",
            "api_key": "sk-test",
            "selected_model": "gpt-4",
            "available_models": ["gpt-4", "gpt-3.5-turbo"],
            "enabled": false,
            "remark": "production key"
        }"#;
        let req: CreateUpstreamRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.provider_name, "OpenAI");
        assert_eq!(req.available_models, vec!["gpt-4", "gpt-3.5-turbo"]);
        assert!(!req.enabled);
        assert_eq!(req.remark, "production key");
    }

    #[test]
    fn test_update_upstream_request_deserialization() {
        let json = r#"{
            "provider_name": "Anthropic",
            "base_url": "https://api.anthropic.com",
            "api_key": "sk-new",
            "selected_model": "claude-3",
            "enabled": true
        }"#;
        let req: UpdateUpstreamRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.provider_name, "Anthropic");
        assert_eq!(req.selected_model, "claude-3");
        assert!(req.enabled);
        assert!(req.available_models.is_empty());
        assert!(req.remark.is_empty());
    }

    #[test]
    fn test_update_upstream_request_masked_key_preserved() {
        let json = r#"{
            "provider_name": "OpenAI",
            "base_url": "https://api.openai.com",
            "api_key": "••••••••",
            "selected_model": "gpt-4",
            "enabled": true
        }"#;
        let req: UpdateUpstreamRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.api_key, "••••••••");
    }

    #[test]
    fn test_generate_id_format() {
        let id = generate_id();
        assert!(id.starts_with("up_"));
        assert!(id.len() > "up_".len());
    }

    #[test]
    fn test_generate_id_uniqueness() {
        let id1 = generate_id();
        std::thread::sleep(std::time::Duration::from_millis(1));
        let id2 = generate_id();
        assert_ne!(id1, id2);
    }
}