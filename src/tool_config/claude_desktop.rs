//! Claude Desktop config writer.
//!
//! On Windows the config lives at `%APPDATA%\Claude\claude_desktop_config.json`.
//! Claude Desktop does not use env vars; it uses `baseUrl`/`apiKey` at the
//! top level. We deep-merge into the existing file, preserving user settings.
//!
//! 1M-context roles (via `roles_1m`) are declared two ways:
//! - `modelRoutes` entries carry `supports1m` per role route
//! - `inferenceModels` entries (name + labelOverride + supports1m) advertise
//!   the 1M capability for each pool to Claude Desktop's model picker
//!
//! The gateway reads the same roles from the persisted tool-config snapshot
//! (`roles_1m` / `model_roles`) and a `ROUTE_MAP_SETTING_KEY` setting to
//! resolve the fixed route IDs (`claude-sonnet-5`, ...) Claude Desktop sends
//! back to the correct pool.

use crate::error::AppError;
use crate::tool_config::backup::BackupEntry;
use crate::tool_config::detector;
use crate::tool_config::writer::atomic_write;
use crate::tool_config::ToolConfigWriter;
use crate::tool_config::ToolPool;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::PathBuf;

pub struct ClaudeDesktopWriter;

pub const APP_ID: &str = "claude-desktop";

/// Settings key holding the persisted route→pool map (`{"claude-sonnet-5":"pool-a"}`).
/// The gateway reads it to resolve Claude Desktop's fixed route IDs back to pools.
pub const ROUTE_MAP_SETTING_KEY: &str = "claude_desktop_route_map";

/// The fixed role route IDs Claude Desktop's model menu uses, in display order
/// (sonnet/opus/fable/haiku). `(role, route_id)` pairs.
pub const ROLE_ROUTE_IDS: &[(&str, &str)] = &[
    ("sonnet", "claude-sonnet-5"),
    ("opus", "claude-opus-5"),
    ("fable", "claude-fable-5"),
    ("haiku", "claude-haiku-4-5"),
];

impl ToolConfigWriter for ClaudeDesktopWriter {
    fn app_id(&self) -> &'static str {
        APP_ID
    }

    fn display_name(&self) -> &'static str {
        "Claude Desktop"
    }

    fn download_url(&self) -> &'static str {
        "https://claude.com/download"
    }

    fn is_installed(&self) -> bool {
        detector::claude_desktop_config_paths()
            .iter()
            .any(|p| p.exists() || p.parent().map(|d| d.exists()).unwrap_or(false))
    }

    fn config_paths(&self) -> Vec<PathBuf> {
        detector::claude_desktop_config_paths()
    }

    fn read_original_config(&self) -> Result<Vec<BackupEntry>, AppError> {
        // Backup the first existing config file (prefer the canonical one).
        let paths = detector::claude_desktop_config_paths();
        let path = paths
            .iter()
            .find(|p| p.exists())
            .cloned()
            .or_else(|| paths.first().cloned())
            .ok_or_else(|| AppError::Config("无法定位配置目录".to_string()))?;
        let content = std::fs::read_to_string(&path).ok();
        Ok(vec![(path, content)])
    }

    fn merge_and_write_config(
        &self,
        original_configs: &[BackupEntry],
        proxy_base_url: &str,
        proxy_api_key: &str,
        all_pools: &[ToolPool],
        default_pool_name: &str,
        default_pool_display_name: &str,
        provider_name: &str,
    ) -> Result<(), AppError> {
        self.merge_and_write_config_with_roles(
            original_configs,
            proxy_base_url,
            proxy_api_key,
            all_pools,
            default_pool_name,
            default_pool_display_name,
            provider_name,
            &[],
        )
    }

    fn merge_and_write_config_with_roles(
        &self,
        original_configs: &[BackupEntry],
        proxy_base_url: &str,
        proxy_api_key: &str,
        all_pools: &[ToolPool],
        default_pool_name: &str,
        default_pool_display_name: &str,
        provider_name: &str,
        model_roles: &[(String, String)],
    ) -> Result<(), AppError> {
        self.merge_and_write_config_with_roles_1m(
            original_configs,
            proxy_base_url,
            proxy_api_key,
            all_pools,
            default_pool_name,
            default_pool_display_name,
            provider_name,
            model_roles,
            &[],
        )
    }

    fn merge_and_write_config_with_roles_1m(
        &self,
        original_configs: &[BackupEntry],
        proxy_base_url: &str,
        proxy_api_key: &str,
        all_pools: &[ToolPool],
        default_pool_name: &str,
        _default_pool_display_name: &str,
        _provider_name: &str,
        model_roles: &[(String, String)],
        roles_1m: &[String],
    ) -> Result<(), AppError> {
        let (path, original) = original_configs
            .first()
            .ok_or_else(|| AppError::Config("缺少原始配置".to_string()))?;

        let mut root: Value = match original {
            Some(content) if !content.trim().is_empty() => serde_json::from_str(content)
                .map_err(|e| AppError::Config(format!("解析 claude_desktop_config.json 失败: {e}")))?,
            _ => json!({}),
        };

        let obj = root
            .as_object_mut()
            .ok_or_else(|| AppError::Config("配置文件根节点不是对象".to_string()))?;
        obj.insert("baseUrl".to_string(), Value::String(proxy_base_url.to_string()));
        obj.insert("apiKey".to_string(), Value::String(proxy_api_key.to_string()));
        obj.insert("mode".to_string(), Value::String("proxy".to_string()));

        // The mapped pool for a role: explicit mapping, else the default pool.
        let pool_for_role = |role: &str| -> String {
            model_roles
                .iter()
                .find(|(r, _)| r == role)
                .map(|(_, m)| m.clone())
                .unwrap_or_else(|| default_pool_name.to_string())
        };
        // A role declares 1M when explicitly flagged, or (via the "default"
        // marker) when it follows the default pool and the default-pool 1M
        // checkbox is set. Roles with an explicit mapping to a non-default
        // pool are never affected by the default flag.
        let supports_1m_for_role = |role: &str| -> bool {
            if roles_1m.iter().any(|r| r == role) {
                return true;
            }
            let default_1m = roles_1m.iter().any(|r| r == "default");
            default_1m
                && pool_for_role(role) == default_pool_name
                && !model_roles.iter().any(|(r, _)| r == role)
        };

        // modelRoutes: the fixed role routes (route ID → mapped pool), then a
        // `pool-{name}` route for every pool not covered by a role mapping so
        // the user can still switch to it inside Claude Desktop.
        let mut routes: Vec<Value> = Vec::new();
        for (role, route_id) in ROLE_ROUTE_IDS {
            routes.push(json!({
                "routeId": route_id,
                "upstreamModel": pool_for_role(role),
                "supports1m": supports_1m_for_role(role),
            }));
        }
        let covered: HashSet<String> = model_roles
            .iter()
            .map(|(_, m)| m.clone())
            .chain(std::iter::once(default_pool_name.to_string()))
            .collect();
        for pool in all_pools {
            if pool.name == default_pool_name || covered.contains(&pool.name) {
                continue;
            }
            routes.push(json!({
                "routeId": format!("pool-{}", pool.name),
                "upstreamModel": pool.name,
                "supports1m": false,
            }));
        }
        obj.insert("modelRoutes".to_string(), Value::Array(routes));

        // inferenceModels: the model picker list. Each pool appears with its
        // real display name and 1M capability (pool behind a 1M role, or the
        // default pool when its unmapped roles follow the default 1M flag).
        let inference_models =
            build_inference_models(all_pools, default_pool_name, model_roles, roles_1m);
        obj.insert("inferenceModels".to_string(), inference_models);

        atomic_write(path, serde_json::to_string_pretty(&root).unwrap_or_default().as_bytes())
    }

    fn restore_original_config(&self, original_configs: &[BackupEntry]) -> Result<(), AppError> {
        match original_configs.first() {
            Some((path, Some(content))) => atomic_write(path, content.as_bytes()),
            Some((path, None)) => {
                let _ = std::fs::remove_file(path);
                Ok(())
            }
            None => Ok(()),
        }
    }
}

/// Build the `inferenceModels` array Claude Desktop's model picker reads.
///
/// One entry per pool: `name` = pool name, `labelOverride` = real display name
/// (from `ToolPool`), `supports1m` = whether the pool declares 1M context.
fn build_inference_models(
    all_pools: &[ToolPool],
    default_pool_name: &str,
    model_roles: &[(String, String)],
    roles_1m: &[String],
) -> Value {
    let display_of = |name: &str| -> String {
        all_pools
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.display_name.clone())
            .unwrap_or_else(|| name.to_string())
    };
    let pool_supports_1m = |pool: &str| -> bool {
        model_roles
            .iter()
            .any(|(role, mapped)| mapped == pool && roles_1m.iter().any(|r| r == role))
            || (pool == default_pool_name && roles_1m.iter().any(|r| r == "default"))
    };
    let models: Vec<Value> = all_pools
        .iter()
        .map(|pool| {
            let mut item = json!({
                "name": pool.name,
                "labelOverride": display_of(&pool.name),
                "supports1m": pool_supports_1m(&pool.name),
            });
            // Bare names for plain pools keep the list readable; only pools
            // with a display override or 1M flag need the rich object shape.
            if !pool_supports_1m(&pool.name) && pool.display_name == pool.name {
                item = Value::String(pool.name.clone());
            }
            item
        })
        .collect();
    Value::Array(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_claude_desktop_merge() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("claude_desktop_config.json");
        std::fs::write(&cfg, r#"{"mcpServers":{"server1":{"command":"echo"}}}"#).unwrap();

        let writer = ClaudeDesktopWriter;
        let original = vec![(
            cfg.clone(),
            Some(r#"{"mcpServers":{"server1":{"command":"echo"}}}"#.to_string()),
        )];

        writer
            .merge_and_write_config(
                &original,
                "http://127.0.0.1:47339",
                "sk-gw",
                &[
                    ToolPool::new("claude-sonnet-4-5", "Sonnet"),
                    ToolPool::new("gpt-4o", "GPT-4o"),
                ],
                "claude-sonnet-4-5",
                "Sonnet",
                "x",
            )
            .unwrap();

        let written: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        // Preserved
        assert_eq!(written["mcpServers"]["server1"]["command"], "echo");
        // Injected
        assert_eq!(written["baseUrl"], "http://127.0.0.1:47339");
        assert_eq!(written["apiKey"], "sk-gw");
        assert_eq!(written["mode"], "proxy");
        // modelRoutes: role routes + pool-{name} for the non-default pool.
        let routes = written["modelRoutes"].as_array().unwrap();
        assert_eq!(routes.len(), 5);
        assert_eq!(routes[0]["routeId"], "claude-sonnet-5");
        assert_eq!(routes[0]["upstreamModel"], "claude-sonnet-4-5");
        assert_eq!(routes[1]["routeId"], "claude-opus-5");
        assert_eq!(routes[2]["routeId"], "claude-fable-5");
        assert_eq!(routes[3]["routeId"], "claude-haiku-4-5");
        assert_eq!(routes[4]["routeId"], "pool-gpt-4o");
        assert_eq!(routes[4]["upstreamModel"], "gpt-4o");
    }

    #[test]
    fn test_claude_desktop_1m_roles_declare_supports1m() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("claude_desktop_config.json");
        let writer = ClaudeDesktopWriter;
        let original = vec![(cfg.clone(), None)];

        writer
            .merge_and_write_config_with_roles_1m(
                &original,
                "http://127.0.0.1:47339",
                "sk-gw",
                &[
                    ToolPool::new("deepseek-v4-pro", "DeepSeek V4 Pro"),
                    ToolPool::new("deepseek-v4-flash", "DeepSeek V4 Flash"),
                ],
                "deepseek-v4-pro",
                "DeepSeek V4 Pro",
                "LLM-API-Proxy",
                &[
                    ("sonnet".to_string(), "deepseek-v4-pro".to_string()),
                    ("haiku".to_string(), "deepseek-v4-flash".to_string()),
                ],
                &["sonnet".to_string(), "haiku".to_string()],
            )
            .unwrap();

        let written: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let routes = written["modelRoutes"].as_array().unwrap();
        // Sonnet/haiku routes map to their pools and declare 1M; unmapped roles
        // fall back to the default pool without 1M.
        let sonnet = routes.iter().find(|r| r["routeId"] == "claude-sonnet-5").unwrap();
        assert_eq!(sonnet["upstreamModel"], "deepseek-v4-pro");
        assert_eq!(sonnet["supports1m"], true);
        let haiku = routes.iter().find(|r| r["routeId"] == "claude-haiku-4-5").unwrap();
        assert_eq!(haiku["upstreamModel"], "deepseek-v4-flash");
        assert_eq!(haiku["supports1m"], true);
        let opus = routes.iter().find(|r| r["routeId"] == "claude-opus-5").unwrap();
        assert_eq!(opus["upstreamModel"], "deepseek-v4-pro");
        assert_eq!(opus["supports1m"], false);
        // Both pools listed in inferenceModels with their real display names.
        let models = written["inferenceModels"].as_array().unwrap();
        let pro = models.iter().find(|m| m.get("name").and_then(Value::as_str) == Some("deepseek-v4-pro")).unwrap();
        assert_eq!(pro["labelOverride"], "DeepSeek V4 Pro");
        assert_eq!(pro["supports1m"], true);
        let flash = models.iter().find(|m| m.get("name").and_then(Value::as_str) == Some("deepseek-v4-flash")).unwrap();
        assert_eq!(flash["labelOverride"], "DeepSeek V4 Flash");
        assert_eq!(flash["supports1m"], true);
    }

    #[test]
    fn test_claude_desktop_default_pool_1m_flag() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("claude_desktop_config.json");
        let writer = ClaudeDesktopWriter;
        let original = vec![(cfg.clone(), None)];

        writer
            .merge_and_write_config_with_roles_1m(
                &original,
                "http://x",
                "k",
                &[ToolPool::new("pool-a", "Pool A")],
                "pool-a",
                "Pool A",
                "LLM-API-Proxy",
                &[],
                &["default".to_string()],
            )
            .unwrap();

        let written: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        // All role routes follow the default pool; the default 1M flag makes
        // every route (unmapped roles) declare 1M.
        for route in written["modelRoutes"].as_array().unwrap() {
            assert_eq!(route["upstreamModel"], "pool-a");
            assert_eq!(route["supports1m"], true);
        }
        // The default pool's inferenceModels entry declares 1M.
        let models = written["inferenceModels"].as_array().unwrap();
        let entry = models.iter().find(|m| m.get("name").and_then(Value::as_str) == Some("pool-a")).unwrap();
        assert_eq!(entry["supports1m"], true);
    }

    #[test]
    fn test_claude_desktop_creates_new() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("claude_desktop_config.json");
        let writer = ClaudeDesktopWriter;
        let original = vec![(cfg.clone(), None)];
        writer
            .merge_and_write_config(&original, "http://x", "k", &[], "p", "P", "x")
            .unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(written["baseUrl"], "http://x");
        assert_eq!(written["mode"], "proxy");
    }

    #[test]
    fn test_claude_desktop_restore() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("claude_desktop_config.json");
        std::fs::write(&cfg, "{\"injected\":true}").unwrap();
        let writer = ClaudeDesktopWriter;
        let original = vec![(cfg.clone(), Some(r#"{"original":true}"#.to_string()))];
        writer.restore_original_config(&original).unwrap();
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), r#"{"original":true}"#);
    }
}
