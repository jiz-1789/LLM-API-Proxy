//! OpenClaw config writer (`~/.openclaw/openclaw.json`).

use crate::error::AppError;
use crate::tool_config::backup::BackupEntry;
use crate::tool_config::detector;
use crate::tool_config::writer::atomic_write;
use crate::tool_config::ToolConfigWriter;
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct OpenClawWriter;

const APP_ID: &str = "openclaw";

impl ToolConfigWriter for OpenClawWriter {
    fn app_id(&self) -> &'static str {
        APP_ID
    }

    fn display_name(&self) -> &'static str {
        "OpenClaw"
    }

    fn download_url(&self) -> &'static str {
        "https://github.com/openclaw/openclaw"
    }

    fn is_installed(&self) -> bool {
        detector::cli_installed("openclaw")
    }

    fn config_paths(&self) -> Vec<PathBuf> {
        detector::openclaw_config_path().into_iter().collect()
    }

    fn read_original_config(&self) -> Result<Vec<BackupEntry>, AppError> {
        let path = detector::openclaw_config_path()
            .ok_or_else(|| AppError::Config("无法定位用户主目录".to_string()))?;
        let content = std::fs::read_to_string(&path).ok();
        Ok(vec![(path, content)])
    }

    fn merge_and_write_config(
        &self,
        original_configs: &[BackupEntry],
        proxy_base_url: &str,
        proxy_api_key: &str,
        _all_pools: &[(String, String)],
        default_pool_name: &str,
        _default_pool_display_name: &str,
        provider_name: &str,
    ) -> Result<(), AppError> {
        let (path, original) = original_configs
            .first()
            .ok_or_else(|| AppError::Config("缺少原始配置".to_string()))?;

        let mut root: Value = match original {
            Some(content) if !content.trim().is_empty() => serde_json::from_str(content)
                .map_err(|e| AppError::Config(format!("解析 openclaw.json 失败: {e}")))?,
            _ => json!({}),
        };

        let provider_key = provider_name.trim();
        let provider_key = if provider_key.is_empty() {
            "llm-api-proxy".to_string()
        } else {
            provider_key.to_string()
        };

        let obj = root
            .as_object_mut()
            .ok_or_else(|| AppError::Config("openclaw.json 根节点不是对象".to_string()))?;
        let models = obj
            .entry("models")
            .or_insert_with(|| json!({}));
        if let Some(mobj) = models.as_object_mut() {
            let providers = mobj.entry("providers").or_insert_with(|| json!({}));
            if let Some(pobj) = providers.as_object_mut() {
                pobj.insert(
                    provider_key.clone(),
                    json!({
                        "baseURL": proxy_base_url,
                        "apiKey": proxy_api_key,
                    }),
                );
            }
        }
        // agents.defaults.model -> "{provider_key}/{default_pool}" so the
        // agent resolves the pool through the injected provider.
        let agents = obj.entry("agents").or_insert_with(|| json!({}));
        if let Some(aobj) = agents.as_object_mut() {
            let defaults = aobj.entry("defaults").or_insert_with(|| json!({}));
            if let Some(dobj) = defaults.as_object_mut() {
                dobj.insert(
                    "model".to_string(),
                    Value::String(format!("{}/{}", provider_key, default_pool_name)),
                );
            }
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
    fn test_openclaw_writes_provider_and_default_model() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("openclaw.json");
        let writer = OpenClawWriter;
        let original = vec![(cfg.clone(), None)];
        writer
            .merge_and_write_config(&original, "http://127.0.0.1:47339", "sk-gw", &[], "grok-4.5", "Grok 4.5", "proxy")
            .unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(written["models"]["providers"]["proxy"]["baseURL"], "http://127.0.0.1:47339");
        assert_eq!(written["models"]["providers"]["proxy"]["apiKey"], "sk-gw");
        // Default model references the provider key + pool name.
        assert_eq!(written["agents"]["defaults"]["model"], "proxy/grok-4.5");
    }

    #[test]
    fn test_openclaw_preserves_existing() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("openclaw.json");
        std::fs::write(&cfg, r#"{"agents":{"defaults":{"temperature":0.7}}}"#).unwrap();
        let writer = OpenClawWriter;
        let original = vec![(
            cfg.clone(),
            Some(r#"{"agents":{"defaults":{"temperature":0.7}}}"#.to_string()),
        )];
        writer
            .merge_and_write_config(&original, "http://x", "k", &[], "pool", "Pool", "proxy")
            .unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(written["agents"]["defaults"]["temperature"], 0.7);
        assert_eq!(written["agents"]["defaults"]["model"], "proxy/pool");
    }
}
