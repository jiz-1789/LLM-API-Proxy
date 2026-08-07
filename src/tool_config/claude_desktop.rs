//! Claude Desktop config writer.
//!
//! On Windows the config lives at `%APPDATA%\Claude\claude_desktop_config.json`.
//! Claude Desktop does not use env vars; it uses `baseUrl`/`apiKey` at the
//! top level. We deep-merge into the existing file, preserving user settings.

use crate::error::AppError;
use crate::tool_config::backup::BackupEntry;
use crate::tool_config::detector;
use crate::tool_config::writer::atomic_write;
use crate::tool_config::ToolConfigWriter;
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct ClaudeDesktopWriter;

const APP_ID: &str = "claude-desktop";

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
        all_pools: &[(String, String)],
        default_pool_name: &str,
        _default_pool_display_name: &str,
        _provider_name: &str,
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

        // Build modelRoutes: default pool maps to the Claude standard model IDs
        // (sonnet/opus/haiku), every other pool gets a `pool-{name}` route so the
        // user can switch pools from within Claude Desktop.
        let mut routes: Vec<Value> = Vec::new();
        if !default_pool_name.is_empty() {
            routes.push(json!({ "routeId": "claude-sonnet-5", "upstreamModel": default_pool_name, "supports1m": false }));
            routes.push(json!({ "routeId": "claude-opus-5", "upstreamModel": default_pool_name, "supports1m": false }));
            routes.push(json!({ "routeId": "claude-haiku-4-5", "upstreamModel": default_pool_name, "supports1m": false }));
        }
        for (pool_name, _) in all_pools {
            if pool_name != default_pool_name {
                routes.push(json!({
                    "routeId": format!("pool-{}", pool_name),
                    "upstreamModel": pool_name,
                    "supports1m": false,
                }));
            }
        }
        obj.insert("modelRoutes".to_string(), Value::Array(routes));

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
            .merge_and_write_config(&original, "http://127.0.0.1:47339", "sk-gw", &[
                ("claude-sonnet-4-5".to_string(), "Sonnet".to_string()),
                ("gpt-4o".to_string(), "GPT-4o".to_string()),
            ], "claude-sonnet-4-5", "Sonnet", "x")
            .unwrap();

        let written: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        // Preserved
        assert_eq!(written["mcpServers"]["server1"]["command"], "echo");
        // Injected
        assert_eq!(written["baseUrl"], "http://127.0.0.1:47339");
        assert_eq!(written["apiKey"], "sk-gw");
        assert_eq!(written["mode"], "proxy");
        // modelRoutes: default pool -> standard IDs; other pools -> pool-{name}
        let routes = written["modelRoutes"].as_array().unwrap();
        assert_eq!(routes.len(), 4);
        assert_eq!(routes[0]["routeId"], "claude-sonnet-5");
        assert_eq!(routes[0]["upstreamModel"], "claude-sonnet-4-5");
        assert_eq!(routes[1]["routeId"], "claude-opus-5");
        assert_eq!(routes[2]["routeId"], "claude-haiku-4-5");
        assert_eq!(routes[3]["routeId"], "pool-gpt-4o");
        assert_eq!(routes[3]["upstreamModel"], "gpt-4o");
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
    }
}
