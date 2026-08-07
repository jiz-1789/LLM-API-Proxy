//! OpenCode config writer (`~/.config/opencode/opencode.json`).
//!
//! OpenCode uses XDG config dirs; on all platforms the file lives at
//! `~/.config/opencode/opencode.json` (or `$XDG_CONFIG_HOME/opencode/opencode.json`).

use crate::error::AppError;
use crate::tool_config::backup::BackupEntry;
use crate::tool_config::detector;
use crate::tool_config::writer::atomic_write;
use crate::tool_config::ToolConfigWriter;
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct OpenCodeWriter;

const APP_ID: &str = "opencode";

impl ToolConfigWriter for OpenCodeWriter {
    fn app_id(&self) -> &'static str {
        APP_ID
    }

    fn display_name(&self) -> &'static str {
        "OpenCode"
    }

    fn download_url(&self) -> &'static str {
        "https://opencode.ai/docs/"
    }

    fn is_installed(&self) -> bool {
        detector::cli_installed("opencode")
    }

    fn config_paths(&self) -> Vec<PathBuf> {
        detector::opencode_config_paths()
    }

    fn read_original_config(&self) -> Result<Vec<BackupEntry>, AppError> {
        let paths = detector::opencode_config_paths();
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
        default_pool_display_name: &str,
        provider_name: &str,
    ) -> Result<(), AppError> {
        let (path, original) = original_configs
            .first()
            .ok_or_else(|| AppError::Config("缺少原始配置".to_string()))?;

        let mut root: Value = match original {
            Some(content) if !content.trim().is_empty() => serde_json::from_str(content)
                .map_err(|e| AppError::Config(format!("解析 opencode.json 失败: {e}")))?,
            _ => json!({}),
        };

        let provider_key = provider_name.trim();
        let provider_key = if provider_key.is_empty() {
            "llm-api-proxy".to_string()
        } else {
            provider_key.to_string()
        };

        // Build model list from all pools.
        let mut models = serde_json::Map::new();
        for (name, display) in all_pools {
            models.insert(
                name.clone(),
                json!({"name": display.clone()}),
            );
        }
        if models.is_empty() {
            models.insert(
                default_pool_name.to_string(),
                json!({"name": default_pool_display_name}),
            );
        }

        let obj = root
            .as_object_mut()
            .ok_or_else(|| AppError::Config("opencode.json 根节点不是对象".to_string()))?;
        // provider.provider_key = { baseURL, apiKey, models }
        let provider = obj
            .entry("provider")
            .or_insert_with(|| json!({}));
        if let Some(pobj) = provider.as_object_mut() {
            pobj.insert(
                provider_key,
                json!({
                    "baseURL": proxy_base_url,
                    "apiKey": proxy_api_key,
                    "models": Value::Object(models),
                }),
            );
        }

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
    fn test_opencode_merge_preserves_existing_and_adds_provider() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("opencode.json");
        std::fs::write(&cfg, r#"{"theme":"dark","provider":{"existing":{"baseURL":"https://x.com"}}}"#).unwrap();

        let writer = OpenCodeWriter;
        let original = vec![(
            cfg.clone(),
            Some(r#"{"theme":"dark","provider":{"existing":{"baseURL":"https://x.com"}}}"#.to_string()),
        )];

        writer
            .merge_and_write_config(
                &original,
                "http://127.0.0.1:47339",
                "sk-gw-test",
                &[("gpt-4".to_string(), "GPT-4".to_string()), ("claude-sonnet".to_string(), "Sonnet".to_string())],
                "gpt-4",
                "GPT-4",
                "llm-api-proxy",
            )
            .unwrap();

        let written: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        // Preserved
        assert_eq!(written["theme"], "dark");
        assert_eq!(written["provider"]["existing"]["baseURL"], "https://x.com");
        // Injected provider
        assert_eq!(written["provider"]["llm-api-proxy"]["baseURL"], "http://127.0.0.1:47339");
        assert_eq!(written["provider"]["llm-api-proxy"]["apiKey"], "sk-gw-test");
        // All pools written as models
        assert_eq!(written["provider"]["llm-api-proxy"]["models"]["gpt-4"]["name"], "GPT-4");
        assert_eq!(written["provider"]["llm-api-proxy"]["models"]["claude-sonnet"]["name"], "Sonnet");
    }

    #[test]
    fn test_opencode_creates_new_file() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("opencode.json");
        let writer = OpenCodeWriter;
        let original = vec![(cfg.clone(), None)];
        writer
            .merge_and_write_config(&original, "http://x", "k", &[], "pool", "Pool", "llm-api-proxy")
            .unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(written["provider"]["llm-api-proxy"]["models"]["pool"]["name"], "Pool");
    }

    #[test]
    fn test_opencode_restore() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("opencode.json");
        std::fs::write(&cfg, "{}").unwrap();
        let writer = OpenCodeWriter;
        let original = vec![(cfg.clone(), Some(r#"{"original":true}"#.to_string()))];
        writer.restore_original_config(&original).unwrap();
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), r#"{"original":true}"#);
    }
}
