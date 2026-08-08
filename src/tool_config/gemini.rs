//! Gemini CLI config writer (`~/.gemini/settings.json`).

use crate::error::AppError;
use crate::tool_config::backup::BackupEntry;
use crate::tool_config::detector;
use crate::tool_config::writer::atomic_write;
use crate::tool_config::ToolConfigWriter;
use crate::tool_config::ToolPool;
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct GeminiWriter;

const APP_ID: &str = "gemini";

impl ToolConfigWriter for GeminiWriter {
    fn app_id(&self) -> &'static str {
        APP_ID
    }

    fn display_name(&self) -> &'static str {
        "Gemini CLI"
    }

    fn download_url(&self) -> &'static str {
        "https://github.com/google-gemini/gemini-cli"
    }

    fn is_installed(&self) -> bool {
        detector::cli_installed("gemini")
    }

    fn config_paths(&self) -> Vec<PathBuf> {
        detector::gemini_settings_path().into_iter().collect()
    }

    fn read_original_config(&self) -> Result<Vec<BackupEntry>, AppError> {
        let path = detector::gemini_settings_path()
            .ok_or_else(|| AppError::Config("无法定位用户主目录".to_string()))?;
        let content = std::fs::read_to_string(&path).ok();
        Ok(vec![(path, content)])
    }

    fn merge_and_write_config(
        &self,
        original_configs: &[BackupEntry],
        proxy_base_url: &str,
        proxy_api_key: &str,
        _all_pools: &[ToolPool],
        default_pool_name: &str,
        _default_pool_display_name: &str,
        _provider_name: &str,
    ) -> Result<(), AppError> {
        let (path, original) = original_configs
            .first()
            .ok_or_else(|| AppError::Config("缺少原始配置".to_string()))?;

        let mut root: Value = match original {
            Some(content) if !content.trim().is_empty() => serde_json::from_str(content)
                .map_err(|e| AppError::Config(format!("解析 settings.json 失败: {e}")))?,
            _ => json!({}),
        };

        let env = root
            .as_object_mut()
            .ok_or_else(|| AppError::Config("settings.json 根节点不是对象".to_string()))?
            .entry("env")
            .or_insert_with(|| json!({}));
        if let Some(env_obj) = env.as_object_mut() {
            env_obj.insert("GEMINI_API_KEY".to_string(), Value::String(proxy_api_key.to_string()));
            // Gemini CLI reads GOOGLE_GEMINI_BASE_URL (cc-switch usage), not
            // GEMINI_BASE_URL — the latter is silently ignored by the CLI.
            env_obj.insert(
                "GOOGLE_GEMINI_BASE_URL".to_string(),
                Value::String(proxy_base_url.to_string()),
            );
            // Default model so the CLI talks to the selected pool without a
            // per-invocation --model flag.
            if !default_pool_name.is_empty() {
                env_obj.insert("GEMINI_MODEL".to_string(), Value::String(default_pool_name.to_string()));
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
    fn test_gemini_writes_base_url_and_default_model() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("settings.json");
        let writer = GeminiWriter;
        let original = vec![(cfg.clone(), None)];
        writer
            .merge_and_write_config(
                &original,
                "http://127.0.0.1:47339",
                "sk-gw",
                &[ToolPool::new("deepseek-v4-pro", "DeepSeek V4 Pro")],
                "deepseek-v4-pro",
                "DeepSeek V4 Pro",
                "LLM-API-Proxy",
            )
            .unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let env = &written["env"];
        assert_eq!(env["GEMINI_API_KEY"], "sk-gw");
        // Gemini CLI reads GOOGLE_GEMINI_BASE_URL (not GEMINI_BASE_URL).
        assert_eq!(env["GOOGLE_GEMINI_BASE_URL"], "http://127.0.0.1:47339");
        assert_eq!(env["GEMINI_MODEL"], "deepseek-v4-pro");
    }

    #[test]
    fn test_gemini_preserves_existing_and_skips_empty_model() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("settings.json");
        std::fs::write(&cfg, r#"{"env":{"GEMINI_MODEL":"old"}}"#).unwrap();
        let writer = GeminiWriter;
        let original = vec![(cfg.clone(), Some(r#"{"env":{"GEMINI_MODEL":"old"}}"#.to_string()))];
        writer
            .merge_and_write_config(&original, "http://x", "k", &[], "", "Pool", "P")
            .unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let env = &written["env"];
        // Existing key preserved, base URL written, empty default model skipped.
        assert_eq!(env["GEMINI_MODEL"], "old");
        assert_eq!(env["GOOGLE_GEMINI_BASE_URL"], "http://x");
    }
}
