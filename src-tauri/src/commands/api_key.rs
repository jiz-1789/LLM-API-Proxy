use serde::{Deserialize, Serialize};
use tauri::State;

use llm_api_proxy_lib::AppState;

// ============================================================================
// DTO Types
// ============================================================================

/// API Key VO for frontend display.
///
/// The `key` field is masked for security: only the first 8 and last 4
/// characters are shown (e.g., `sk-gw-ab...wxyz`).
/// The full key is only returned when explicitly requested (e.g., on creation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyVO {
    pub id: String,
    /// Masked key for display (e.g., `sk-gw-ab...wxyz`).
    /// The full key is only shown on creation/regeneration.
    pub key: String,
    pub name: String,
    pub enabled: bool,
    /// JSON array of pool IDs. Empty array = all pools allowed.
    pub allowed_pools: String,
    pub expires_at: Option<String>,
    pub last_used_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Request payload for creating a new API key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateApiKeyRequest {
    pub name: String,
    /// JSON array of pool IDs. Empty array = all pools allowed.
    #[serde(default = "default_empty_array")]
    pub allowed_pools: String,
    /// Optional expiration timestamp (NULL = never expires).
    /// Format: `YYYY-MM-DD HH:MM:SS`
    pub expires_at: Option<String>,
}

/// Request payload for updating an existing API key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateApiKeyRequest {
    pub name: String,
    pub enabled: bool,
    /// JSON array of pool IDs. Empty array = all pools allowed.
    pub allowed_pools: String,
    /// Optional expiration timestamp (NULL = never expires).
    pub expires_at: Option<String>,
}

fn default_empty_array() -> String {
    "[]".to_string()
}

/// Mask an API key string for display.
///
/// Shows the first 8 and last 4 characters, masking the middle.
/// For short keys, masks everything except the first 4 characters.
fn mask_key(key: &str) -> String {
    if key.len() <= 12 {
        return format!("{}••••••••", &key[..key.len().min(4)]);
    }
    format!("{}••••••••{}", &key[..8], &key[key.len() - 4..])
}

/// Generate a unique ID for a new API key record.
///
/// Format: `ak_{timestamp_hex}{nanos_hex}{random_hex}`
/// The 4-byte random suffix (from UUID v4) prevents collisions when two IDs
/// are generated within the same nanosecond on fast machines.
fn generate_api_key_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // Use uuid v4 as a source of randomness (already a dependency)
    let rand_hex = (uuid::Uuid::new_v4().as_u128() & 0xFFFF_FFFF) as u32;
    format!(
        "ak_{:x}{:08x}{:08x}",
        now.as_secs(),
        now.subsec_nanos(),
        rand_hex
    )
}

/// Generate a random API key string in the format `sk-gw-<32 hex chars>`.
fn generate_key_string() -> String {
    format!(
        "sk-gw-{}",
        uuid::Uuid::new_v4().to_string().replace('-', "")
    )
}

/// Convert an ApiKey database record to a VO with masked key.
impl From<llm_api_proxy_lib::db::ApiKey> for ApiKeyVO {
    fn from(k: llm_api_proxy_lib::db::ApiKey) -> Self {
        Self {
            id: k.id,
            key: mask_key(&k.key),
            name: k.name,
            enabled: k.enabled,
            allowed_pools: k.allowed_pools,
            expires_at: k.expires_at,
            last_used_at: k.last_used_at,
            created_at: k.created_at,
            updated_at: k.updated_at,
        }
    }
}

// ============================================================================
// Commands
// ============================================================================

/// List all API keys (with masked key values).
#[tauri::command]
pub fn list_api_keys(state: State<'_, AppState>) -> Result<Vec<ApiKeyVO>, String> {
    let keys = state.db.get_api_keys().map_err(|e| e.to_string())?;
    Ok(keys.into_iter().map(Into::into).collect())
}

/// Create a new API key.
/// Returns the full (unmasked) key string — this is the only time the full key is shown.
#[tauri::command]
pub fn create_api_key(
    req: CreateApiKeyRequest,
    state: State<'_, AppState>,
) -> Result<ApiKeyVO, String> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err("密钥名称不能为空".to_string());
    }

    // Validate allowed_pools is valid JSON
    let pools: Vec<String> = serde_json::from_str(&req.allowed_pools)
        .map_err(|_| "allowed_pools 不是有效的 JSON 数组".to_string())?;

    // Validate that referenced pool IDs exist (if any)
    if !pools.is_empty() {
        for pool_id in &pools {
            if !state.db.pool_exists(pool_id).map_err(|e| e.to_string())? {
                return Err(format!("号池不存在: {}", pool_id));
            }
        }
    }

    let id = generate_api_key_id();
    let key = generate_key_string();
    let allowed_pools = serde_json::to_string(&pools).unwrap_or_else(|_| "[]".to_string());

    state
        .db
        .create_api_key(&id, &key, name, &allowed_pools, req.expires_at.as_deref())
        .map_err(|e| e.to_string())?;

    // Record audit entry
    state
        .db
        .insert_config_change("api_key_created", None, &format!("创建密钥: {}", name))
        .map_err(|e| e.to_string())?;

    // Return the full key (unmasked) so the user can copy it on creation
    let api_key = state.db.get_api_key_by_id(&id).map_err(|e| e.to_string())?;
    let api_key = api_key.ok_or("创建后无法找到密钥")?;

    // Return VO with full key (not masked) for initial display
    Ok(ApiKeyVO {
        id: api_key.id,
        key: api_key.key, // Full key on creation
        name: api_key.name,
        enabled: api_key.enabled,
        allowed_pools: api_key.allowed_pools,
        expires_at: api_key.expires_at,
        last_used_at: api_key.last_used_at,
        created_at: api_key.created_at,
        updated_at: api_key.updated_at,
    })
}

/// Update an existing API key's properties.
#[tauri::command]
pub fn update_api_key(
    id: String,
    req: UpdateApiKeyRequest,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err("密钥名称不能为空".to_string());
    }

    // Validate allowed_pools is valid JSON
    let pools: Vec<String> = serde_json::from_str(&req.allowed_pools)
        .map_err(|_| "allowed_pools 不是有效的 JSON 数组".to_string())?;

    // Validate that referenced pool IDs exist (if any)
    if !pools.is_empty() {
        for pool_id in &pools {
            if !state.db.pool_exists(pool_id).map_err(|e| e.to_string())? {
                return Err(format!("号池不存在: {}", pool_id));
            }
        }
    }

    let allowed_pools = serde_json::to_string(&pools).unwrap_or_else(|_| "[]".to_string());

    state
        .db
        .update_api_key(
            &id,
            name,
            req.enabled,
            &allowed_pools,
            req.expires_at.as_deref(),
        )
        .map_err(|e| e.to_string())?;

    // Record audit entry
    state
        .db
        .insert_config_change(
            "api_key_updated",
            None,
            &format!("更新密钥: {} ({})", name, id),
        )
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Delete an API key.
#[tauri::command]
pub fn delete_api_key(id: String, state: State<'_, AppState>) -> Result<(), String> {
    // Get the key name for audit before deleting
    let key = state.db.get_api_key_by_id(&id).map_err(|e| e.to_string())?;
    let key_name = key.map(|k| k.name).unwrap_or_default();

    state.db.delete_api_key(&id).map_err(|e| e.to_string())?;

    // Record audit entry
    state
        .db
        .insert_config_change(
            "api_key_deleted",
            None,
            &format!("删除密钥: {} ({})", key_name, id),
        )
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Toggle an API key's enabled status.
#[tauri::command]
pub fn toggle_api_key(
    id: String,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .db
        .toggle_api_key(&id, enabled)
        .map_err(|e| e.to_string())?;

    // Record audit entry
    let action = if enabled { "启用" } else { "禁用" };
    state
        .db
        .insert_config_change("api_key_toggled", None, &format!("{}密钥: {}", action, id))
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Regenerate an API key's key string.
/// Returns the new full (unmasked) key string.
#[tauri::command]
pub fn regenerate_api_key(
    id: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let new_key = generate_key_string();

    state
        .db
        .regenerate_api_key(&id, &new_key)
        .map_err(|e| e.to_string())?;

    // Record audit entry
    state
        .db
        .insert_config_change("api_key_regenerated", None, &format!("重新生成密钥: {}", id))
        .map_err(|e| e.to_string())?;

    Ok(new_key)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_key_long() {
        let masked = mask_key("sk-gw-abcdefghijwxyz");
        assert!(masked.starts_with("sk-gw-ab"));
        assert!(masked.ends_with("wxyz"));
        assert!(masked.contains("••••••••"));
    }

    #[test]
    fn test_mask_key_short() {
        let masked = mask_key("sk-gw");
        assert!(masked.starts_with("sk-g"));
        assert!(masked.contains("••••••••"));
    }

    #[test]
    fn test_mask_key_empty() {
        let masked = mask_key("");
        // Empty key: no prefix visible, just mask
        assert!(masked.contains("••••••••"));
    }

    #[test]
    fn test_mask_key_boundary_12_chars() {
        let masked = mask_key("123456789012");
        // 12 chars: uses short mask (first 4 + mask)
        assert_eq!(masked, "1234••••••••");
    }

    #[test]
    fn test_mask_key_13_chars() {
        let masked = mask_key("1234567890123");
        // 13 chars: first 8 + mask + last 4
        assert_eq!(masked, "12345678••••••••0123");
    }

    #[test]
    fn test_generate_key_string_format() {
        let key = generate_key_string();
        assert!(key.starts_with("sk-gw-"));
        assert_eq!(key.len(), "sk-gw-".len() + 32); // 32 hex chars from UUID
    }

    #[test]
    fn test_generate_api_key_id_format() {
        let id = generate_api_key_id();
        assert!(id.starts_with("ak_"));
    }

    #[test]
    fn test_api_key_vo_from_masks_key() {
        let api_key = llm_api_proxy_lib::db::ApiKey {
            id: "ak_1".to_string(),
            key: "sk-gw-abcdefghijklmnopwxyz".to_string(),
            name: "测试".to_string(),
            enabled: true,
            allowed_pools: "[]".to_string(),
            expires_at: None,
            last_used_at: None,
            created_at: "2026-07-28 10:00:00".to_string(),
            updated_at: "2026-07-28 10:00:00".to_string(),
        };
        let vo: ApiKeyVO = api_key.into();
        assert_eq!(vo.id, "ak_1");
        assert!(vo.key.starts_with("sk-gw-ab"));
        assert!(vo.key.ends_with("wxyz"));
        assert!(!vo.key.contains("cdefghijklmnop")); // middle is masked
    }

    #[test]
    fn test_create_api_key_request_default_allowed_pools() {
        let json = r#"{"name": "测试"}"#;
        let req: CreateApiKeyRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.allowed_pools, "[]");
        assert!(req.expires_at.is_none());
    }

    #[test]
    fn test_create_api_key_request_with_pools() {
        let json = r#"{"name": "受限", "allowed_pools": "[\"pool_1\", \"pool_2\"]"}"#;
        let req: CreateApiKeyRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "受限");
        assert_eq!(req.allowed_pools, "[\"pool_1\", \"pool_2\"]");
    }
}
