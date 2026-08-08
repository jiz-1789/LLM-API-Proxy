use serde::Deserialize;
use tauri::State;

use llm_api_proxy_lib::AppState;
use llm_api_proxy_lib::tool_config::{EnableResult, ModelRequirements};

// ============================================================================
// Commands
// ============================================================================

/// Detect installation status of all registered tools.
#[tauri::command]
pub fn detect_all_tools(
    state: State<'_, AppState>,
) -> Result<Vec<llm_api_proxy_lib::db::ToolDetectionResult>, String> {
    Ok(state.tool_switch_manager.detect_all_tools())
}

/// Get the switch status of all tools (joined with pool name).
#[tauri::command]
pub fn get_tool_switches(
    state: State<'_, AppState>,
) -> Result<Vec<llm_api_proxy_lib::db::ToolSwitchStatus>, String> {
    state
        .tool_switch_manager
        .get_all_switch_status()
        .map_err(|e| e.to_string())
}

/// Role→pool mapping for tools with role slots (e.g. Claude Code / Desktop).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ModelRoleMapping {
    /// (role, pool_name) pairs, e.g. ("sonnet", "deepseek-v4-pro").
    pub roles: Vec<(String, String)>,
    /// Roles with 1M-context enabled (Claude Desktop only).
    #[serde(default)]
    pub roles_1m: Vec<String>,
}

/// Enable a tool switch: backup + write proxy config.
#[tauri::command]
pub fn enable_tool_switch(
    state: State<'_, AppState>,
    app_id: String,
    pool_id: String,
    api_key_id: Option<String>,
    provider_name: Option<String>,
    model_roles: Option<ModelRoleMapping>,
) -> Result<EnableResult, String> {
    // Keep the owned mapping alive while passing a slice into the manager.
    let mapping = model_roles.unwrap_or_default();
    state
        .tool_switch_manager
        .enable_tool(
            &app_id,
            &pool_id,
            api_key_id.as_deref(),
            provider_name.as_deref().unwrap_or("LLM-API-Proxy"),
            mapping.roles.as_slice(),
            mapping.roles_1m.as_slice(),
        )
        .map_err(|e| e.to_string())
}

/// Disable a tool switch: restore original config.
#[tauri::command]
pub fn disable_tool_switch(state: State<'_, AppState>, app_id: String) -> Result<(), String> {
    state
        .tool_switch_manager
        .disable_tool(&app_id)
        .map_err(|e| e.to_string())
}

/// Update an enabled tool's associated pool/API key and rewrite config.
#[tauri::command]
pub fn update_tool_config(
    state: State<'_, AppState>,
    app_id: String,
    pool_id: Option<String>,
    api_key_id: Option<String>,
    provider_name: Option<String>,
    model_roles: Option<ModelRoleMapping>,
) -> Result<(), String> {
    // Keep the owned mapping alive while passing a slice into the manager.
    let mapping = model_roles.unwrap_or_default();
    state
        .tool_switch_manager
        .update_tool_config(
            &app_id,
            pool_id.as_deref(),
            api_key_id.as_deref(),
            provider_name.as_deref(),
            Some(mapping.roles.as_slice()),
            Some(mapping.roles_1m.as_slice()),
        )
        .map_err(|e| e.to_string())
}

/// Detect all environment-variable conflicts that could override injected config.
#[tauri::command]
pub fn detect_env_conflicts() -> Vec<llm_api_proxy_lib::tool_config::env_check::EnvConflict> {
    llm_api_proxy_lib::tool_config::env_check::detect_conflicts()
}

/// Remove conflicting environment variables (backing them up first).
#[tauri::command]
pub fn cleanup_env_conflicts(
    conflicts: Vec<llm_api_proxy_lib::tool_config::env_check::EnvConflict>,
) -> Result<Option<String>, String> {
    llm_api_proxy_lib::tool_config::env_check::cleanup_conflicts(&conflicts)
        .map(|p| p.map(|p| p.display().to_string()))
        .map_err(|e| e.to_string())
}

/// Restore environment variables from a previously created backup file.
#[tauri::command]
pub fn restore_env_backup(backup_path: String) -> Result<(), String> {
    llm_api_proxy_lib::tool_config::env_check::restore_env_backup(
        std::path::Path::new(&backup_path),
    )
    .map_err(|e| e.to_string())
}

// ============================================================================
// Capability-based pool suggestion (stage 2 integration)
// ============================================================================

/// Requirements for capability-based model routing.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SuggestPoolRequest {
    pub vision: bool,
    pub audio: bool,
    pub function_calling: bool,
    pub prefer_large_context: bool,
}

/// Suggest the best pool for the given capability requirements.
#[tauri::command]
pub fn suggest_pool(
    state: State<'_, AppState>,
    req: SuggestPoolRequest,
) -> Result<Option<(String, String)>, String> {
    let requirements = ModelRequirements {
        vision: req.vision,
        audio: req.audio,
        function_calling: req.function_calling,
        prefer_large_context: req.prefer_large_context,
    };
    Ok(llm_api_proxy_lib::tool_config::select_pool_for_requirements(
        &state.db,
        requirements,
    ))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    /// Replica of Tauri's generated per-command args struct (camelCase keys),
    /// mirroring `enable_tool_switch` / `update_tool_config`.
    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ToolSwitchArgs {
        app_id: String,
        pool_id: Option<String>,
        api_key_id: Option<String>,
        provider_name: Option<String>,
        model_roles: Option<ModelRoleMapping>,
    }

    #[test]
    fn test_tool_switch_args_claude_desktop_payload() {
        // Exact payload shape the frontend sends for Claude Desktop (roles +
        // roles_1m, apiKeyId explicitly null).
        let json = r#"{
            "appId": "claude-desktop",
            "poolId": "pool-abc",
            "apiKeyId": null,
            "providerName": "LLM-API-Proxy",
            "modelRoles": {
                "roles": [["sonnet", "vision-pool"]],
                "roles_1m": ["sonnet"]
            }
        }"#;
        let args: ToolSwitchArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.app_id, "claude-desktop");
        assert_eq!(args.pool_id.as_deref(), Some("pool-abc"));
        assert!(args.api_key_id.is_none());
        let mapping = args.model_roles.unwrap();
        assert_eq!(mapping.roles, vec![("sonnet".to_string(), "vision-pool".to_string())]);
        assert_eq!(mapping.roles_1m, vec!["sonnet".to_string()]);
    }

    #[test]
    fn test_tool_switch_args_empty_roles_ok() {
        let json = r#"{
            "appId": "claude-desktop",
            "poolId": "pool-abc",
            "apiKeyId": null,
            "providerName": "LLM-API-Proxy",
            "modelRoles": {"roles": [], "roles_1m": []}
        }"#;
        let args: ToolSwitchArgs = serde_json::from_str(json).unwrap();
        let mapping = args.model_roles.unwrap();
        assert!(mapping.roles.is_empty());
        assert!(mapping.roles_1m.is_empty());
    }

    #[test]
    fn test_tool_switch_args_missing_model_roles_is_none() {
        // Non-role tools send no modelRoles at all.
        let json = r#"{
            "appId": "codex",
            "poolId": "pool-abc",
            "apiKeyId": null,
            "providerName": "LLM-API-Proxy"
        }"#;
        let args: ToolSwitchArgs = serde_json::from_str(json).unwrap();
        assert!(args.model_roles.is_none());
    }

    #[test]
    fn test_tool_switch_args_null_required_string_is_error() {
        // Documents the failure mode the user hit: a null String arg must be
        // rejected with "invalid type: null, expected a string".
        let json = r#"{
            "appId": null,
            "poolId": "pool-abc",
            "apiKeyId": null,
            "providerName": "LLM-API-Proxy",
            "modelRoles": null
        }"#;
        let err = serde_json::from_str::<ToolSwitchArgs>(json).unwrap_err();
        assert!(
            err.to_string().contains("invalid type: null, expected a string"),
            "unexpected error: {err}"
        );
    }
}
