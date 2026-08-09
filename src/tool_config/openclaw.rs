//! OpenClaw config writer (`~/.openclaw/openclaw.json`).
//!
//! OpenClaw reads custom providers from `models.providers.<id>` with
//! **camelCase** keys (`baseUrl`/`apiKey`/`api`, models as `id`/`name`/
//! `contextWindow`).
//! A fresh file must declare `models.mode = "merge"` so providers accumulate.

use crate::error::AppError;
use crate::tool_config::backup::BackupEntry;
use crate::tool_config::detector;
use crate::tool_config::writer::atomic_write;
use crate::tool_config::ToolConfigWriter;
use crate::tool_config::ToolPool;
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
        all_pools: &[ToolPool],
        default_pool_name: &str,
        _default_pool_display_name: &str,
        provider_name: &str,
    ) -> Result<(), AppError> {
        self.merge_and_write_config_full(
            original_configs,
            proxy_base_url,
            proxy_api_key,
            all_pools,
            default_pool_name,
            provider_name,
        )
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

impl OpenClawWriter {
    /// Core write entry point shared by the trait method and tests.
    fn merge_and_write_config_full(
        &self,
        original_configs: &[BackupEntry],
        proxy_base_url: &str,
        proxy_api_key: &str,
        all_pools: &[ToolPool],
        default_pool_name: &str,
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
        let models = obj.entry("models").or_insert_with(|| {
            // Fresh configs must declare merge mode so providers accumulate.
            json!({ "mode": "merge", "providers": {} })
        });
        if let Some(mobj) = models.as_object_mut() {
            // Ensure models.mode is present and "merge".
            mobj.entry("mode")
                .or_insert_with(|| Value::String("merge".to_string()));
            let providers = mobj.entry("providers").or_insert_with(|| json!({}));
            if let Some(pobj) = providers.as_object_mut() {
                // Register every pool as a switchable model so OpenClaw can pick
                // any proxy pool. Each entry carries its real context window.
                let model_list: Vec<Value> = all_pools
                    .iter()
                    .map(|pool| {
                        let mut entry = json!({
                            "id": pool.name,
                            "name": if pool.display_name.is_empty() {
                                pool.name.clone()
                            } else {
                                pool.display_name.clone()
                            },
                        });
                        if let Some(window) = pool.context_window {
                            entry["contextWindow"] = json!(window);
                        }
                        entry
                    })
                    .collect();
                // camelCase keys are what OpenClaw actually reads.
                pobj.insert(
                    provider_key.clone(),
                    json!({
                        "baseUrl": proxy_base_url,
                        "apiKey": proxy_api_key,
                        "api": "openai-completions",
                        "models": model_list,
                    }),
                );
            }
        }
        // agents.defaults.model -> {primary: "{provider_key}/{default_pool}"}
        // so the agent resolves the default pool through the injected provider.
        let agents = obj.entry("agents").or_insert_with(|| json!({}));
        if let Some(aobj) = agents.as_object_mut() {
            let defaults = aobj.entry("defaults").or_insert_with(|| json!({}));
            if let Some(dobj) = defaults.as_object_mut() {
                dobj.insert(
                    "model".to_string(),
                    json!({
                        "primary": format!("{}/{}", provider_key, default_pool_name),
                        "fallbacks": [],
                    }),
                );
            }
        }

        atomic_write(path, serde_json::to_string_pretty(&root).unwrap_or_default().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_openclaw_writes_camelcase_provider_and_default_model() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("openclaw.json");
        let writer = OpenClawWriter;
        let original = vec![(cfg.clone(), None)];
        writer
            .merge_and_write_config(&original, "http://127.0.0.1:47339/v1", "sk-gw", &[], "grok-4.5", "Grok 4.5", "proxy")
            .unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        // camelCase keys are what OpenClaw reads; snake_case must not appear.
        assert_eq!(written["models"]["providers"]["proxy"]["baseUrl"], "http://127.0.0.1:47339/v1");
        assert_eq!(written["models"]["providers"]["proxy"]["apiKey"], "sk-gw");
        assert_eq!(written["models"]["providers"]["proxy"]["api"], "openai-completions");
        assert!(written["models"]["providers"]["proxy"].get("base_url").is_none());
        assert!(written["models"]["providers"]["proxy"].get("api_key").is_none());
        // Fresh config declares merge mode so providers accumulate.
        assert_eq!(written["models"]["mode"], "merge");
        // No pools → empty model list; default model references provider + pool.
        assert!(written["models"]["providers"]["proxy"]["models"].as_array().unwrap().is_empty());
        assert_eq!(
            written["agents"]["defaults"]["model"]["primary"],
            "proxy/grok-4.5"
        );
        assert!(written["agents"]["defaults"]["model"]["fallbacks"].is_array());
    }

    #[test]
    fn test_openclaw_writes_all_pool_models_with_context_window() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("openclaw.json");
        let writer = OpenClawWriter;
        let original = vec![(cfg.clone(), None)];
        writer
            .merge_and_write_config(
                &original,
                "http://127.0.0.1:47339/v1",
                "sk-gw",
                &[
                    ToolPool::new("deepseek-v4-pro", "DeepSeek V4 Pro"),
                    ToolPool::with_window("deepseek-v4-flash", "DeepSeek V4 Flash", 128000),
                ],
                "deepseek-v4-pro",
                "DeepSeek V4 Pro",
                "LLM-API-Proxy",
            )
            .unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let models = written["models"]["providers"]["LLM-API-Proxy"]["models"].as_array().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["id"], "deepseek-v4-pro");
        assert_eq!(models[0]["name"], "DeepSeek V4 Pro");
        // Pool without a known window has no contextWindow; the other does.
        assert!(models[0].get("contextWindow").is_none());
        assert_eq!(models[1]["id"], "deepseek-v4-flash");
        assert_eq!(models[1]["contextWindow"], 128000);
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
        assert_eq!(written["agents"]["defaults"]["model"]["primary"], "proxy/pool");
        // Existing config without a mode stays merge-friendly too.
        assert_eq!(written["models"]["mode"], "merge");
    }
}