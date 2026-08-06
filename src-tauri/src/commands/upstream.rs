use serde::{Deserialize, Serialize};
use tauri::State;

use llm_api_proxy_lib::AppState;
use llm_api_proxy_lib::proxy::url_util::{build_models_url, send_test_chat_request};

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

/// Reveal the decrypted API key for an upstream (for eye toggle in UI).
/// Returns the plaintext API key.
#[tauri::command]
pub fn reveal_api_key(id: String, state: State<'_, AppState>) -> Result<String, String> {
    let upstream = state
        .db
        .get_upstream_by_id(&id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("上游 {} 不存在", id))?;

    if upstream.api_key_encrypted.is_empty() {
        return Err("该上游未配置 API Key".to_string());
    }

    state
        .crypto
        .decrypt_api_key(&upstream.api_key_encrypted)
        .map_err(|e| format!("API Key 解密失败: {}", e))
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
    let url = build_models_url(&base_url);

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

    let url = build_models_url(&upstream.base_url);

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
// Chat-based connection test
// ============================================================================

/// Result of a chat-based connection test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTestResult {
    pub success: bool,
    pub latency_ms: u64,
    pub model_used: String,
    pub message: Option<String>,
}

/// Test upstream connectivity by sending a minimal chat completion request.
///
/// Uses the provided `model` to send `{"messages":[{"role":"user","content":"hi"}], "max_tokens":1}`
/// to the upstream's `/chat/completions` endpoint. This verifies end-to-end
/// connectivity: network, auth, model validity, and response format.
#[tauri::command]
pub async fn test_upstream_chat(
    base_url: String,
    api_key: String,
    model: String,
) -> Result<ChatTestResult, String> {
    let start = std::time::Instant::now();
    match send_test_chat_request(&base_url, &api_key, &model, 30).await {
        Ok(latency) => Ok(ChatTestResult {
            success: true,
            latency_ms: latency,
            model_used: model,
            message: None,
        }),
        Err(e) => Ok(ChatTestResult {
            success: false,
            latency_ms: start.elapsed().as_millis() as u64,
            model_used: model,
            message: Some(e),
        }),
    }
}

/// Test upstream connectivity by sending a minimal chat completion request
/// using the upstream's first available model.
///
/// Looks up the upstream by ID, decrypts its API key, and uses `selected_model`
/// (or the first entry in `available_models`) to send a test request.
#[tauri::command]
pub async fn test_upstream_chat_by_id(
    id: String,
    state: State<'_, AppState>,
) -> Result<ChatTestResult, String> {
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

    // Use selected_model, or fall back to first available model
    let model = if !upstream.selected_model.is_empty() {
        upstream.selected_model.clone()
    } else {
        let models: Vec<String> = serde_json::from_str(&upstream.available_models).unwrap_or_default();
        models
            .into_iter()
            .next()
            .ok_or_else(|| "该上游未配置模型".to_string())?
    };

    let start = std::time::Instant::now();
    match send_test_chat_request(&upstream.base_url, &api_key, &model, 30).await {
        Ok(latency) => Ok(ChatTestResult {
            success: true,
            latency_ms: latency,
            model_used: model,
            message: None,
        }),
        Err(e) => Ok(ChatTestResult {
            success: false,
            latency_ms: start.elapsed().as_millis() as u64,
            model_used: model,
            message: Some(e),
        }),
    }
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

    #[test]
    fn test_chat_test_result_serialization_success() {
        let result = ChatTestResult {
            success: true,
            latency_ms: 150,
            model_used: "gpt-4".to_string(),
            message: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"latency_ms\":150"));
        assert!(json.contains("\"model_used\":\"gpt-4\""));
        assert!(json.contains("\"message\":null"));
    }

    #[test]
    fn test_chat_test_result_serialization_failure() {
        let result = ChatTestResult {
            success: false,
            latency_ms: 3000,
            model_used: "deepseek-chat".to_string(),
            message: Some("HTTP 401 — unauthorized".to_string()),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"success\":false"));
        assert!(json.contains("\"message\":\"HTTP 401 — unauthorized\""));
    }

    #[test]
    fn test_chat_test_result_deserialization() {
        let json = r#"{"success":true,"latency_ms":42,"model_used":"claude-3","message":null}"#;
        let result: ChatTestResult = serde_json::from_str(json).unwrap();
        assert!(result.success);
        assert_eq!(result.latency_ms, 42);
        assert_eq!(result.model_used, "claude-3");
        assert!(result.message.is_none());
    }
}