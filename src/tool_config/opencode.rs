//! OpenCode config writer (`~/.config/opencode/opencode.json`).
//!
//! OpenCode uses XDG config dirs; on all platforms the file lives at
//! `~/.config/opencode/opencode.json` (or `$XDG_CONFIG_HOME/opencode/opencode.json`).

use crate::error::AppError;
use crate::tool_config::backup::BackupEntry;
use crate::tool_config::detector;
use crate::tool_config::writer::atomic_write;
use crate::tool_config::ToolConfigWriter;
use crate::tool_config::ToolPool;
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
        all_pools: &[ToolPool],
        default_pool_name: &str,
        default_pool_display_name: &str,
        provider_name: &str,
    ) -> Result<(), AppError> {
        self.merge_and_write_config_with_roles_1m(
            original_configs,
            proxy_base_url,
            proxy_api_key,
            all_pools,
            default_pool_name,
            default_pool_display_name,
            provider_name,
            &[],
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
        default_pool_display_name: &str,
        provider_name: &str,
        _model_roles: &[(String, String)],
        roles_1m: &[String],
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

        // Build model list from all pools. Each pool carries its inferred real
        // context window (limit.context) when known; the default pool is forced
        // to 1M when the default-pool 1M flag is set.
        let default_1m = roles_1m.iter().any(|r| r == "default");
        let model_entry = |name_opt: Option<&str>, display: &str, pool_ctx: Option<i32>| -> Value {
            let mut entry = json!({"name": display});
            // The default pool is forced to 1M when the default-pool 1M flag is
            // set; otherwise each pool declares its real context window.
            let ctx = if default_1m && name_opt == Some(default_pool_name) {
                Some(1000000)
            } else {
                pool_ctx
            };
            if let Some(ctx) = ctx {
                // OpenCode 的 limit 结构中 context 与 output 均为必填，
                // 缺 output 会导致配置校验失败（ConfigInvalidError）。
                entry["limit"] = json!({"context": ctx, "output": 8192});
            }
            entry
        };
        let mut models = serde_json::Map::new();
        for pool in all_pools {
            let name_opt = Some(pool.name.as_str());
            models.insert(
                pool.name.clone(),
                model_entry(name_opt, &pool.display_name, pool.context_window),
            );
        }
        if models.is_empty() {
            models.insert(
                default_pool_name.to_string(),
                model_entry(Some(default_pool_name), default_pool_display_name, None),
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
                &[ToolPool::new("gpt-4", "GPT-4"), ToolPool::new("claude-sonnet", "Sonnet")],
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
    fn test_opencode_default_pool_1m_sets_limit_context() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("opencode.json");
        let writer = OpenCodeWriter;
        let original = vec![(cfg.clone(), None)];
        writer
            .merge_and_write_config_with_roles_1m(
                &original,
                "http://127.0.0.1:47339",
                "sk-gw-test",
                &[ToolPool::new("gpt-4", "GPT-4"), ToolPool::new("claude-sonnet", "Sonnet")],
                "gpt-4",
                "GPT-4",
                "llm-api-proxy",
                &[],
                &["default".to_string()],
            )
            .unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let models = &written["provider"]["llm-api-proxy"]["models"];
        // Default pool declares the 1M context window (cc-switch's limit editor).
        assert_eq!(models["gpt-4"]["limit"]["context"], 1000000);
        // OpenCode 要求 limit.output 必填，缺 key 会触发配置校验失败。
        assert!(models["gpt-4"]["limit"]["output"].is_number());
        assert!(models["gpt-4"]["limit"]["output"].as_i64().unwrap() > 0);
        // Other pools keep no window declaration.
        assert!(models["claude-sonnet"].get("limit").is_none());
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
