//! Hermes config writer (`~/.hermes/config.yaml` on Mac/Linux,
//! `%LOCALAPPDATA%\hermes\config.yaml` on Windows).
//!
//! YAML manipulation is done via string-level section append to avoid adding
//! a YAML dependency. Existing content is preserved verbatim.

use crate::error::AppError;
use crate::tool_config::backup::BackupEntry;
use crate::tool_config::detector;
use crate::tool_config::writer::atomic_write;
use crate::tool_config::ToolConfigWriter;
use crate::tool_config::ToolPool;
use std::path::PathBuf;

pub struct HermesWriter;

const APP_ID: &str = "hermes";
const PROVIDER_ID: &str = "llm-api-proxy";
const DEFAULT_CONTEXT_LENGTH: u64 = 200_000;
const ONE_M_CONTEXT_LENGTH: u64 = 1_000_000;

impl ToolConfigWriter for HermesWriter {
    fn app_id(&self) -> &'static str {
        APP_ID
    }

    fn display_name(&self) -> &'static str {
        "Hermes"
    }

    fn download_url(&self) -> &'static str {
        "https://github.com/VersoriumX/Hermes"
    }

    fn is_installed(&self) -> bool {
        detector::cli_installed("hermes")
    }

    fn config_paths(&self) -> Vec<PathBuf> {
        detector::hermes_config_path().into_iter().collect()
    }

    fn read_original_config(&self) -> Result<Vec<BackupEntry>, AppError> {
        let path = detector::hermes_config_path()
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
        _default_pool_display_name: &str,
        provider_name: &str,
        _model_roles: &[(String, String)],
        roles_1m: &[String],
    ) -> Result<(), AppError> {
        let (path, original) = original_configs
            .first()
            .ok_or_else(|| AppError::Config("缺少原始配置".to_string()))?;

        let mut yaml = original.clone().unwrap_or_default().trim_end().to_string();
        if yaml.is_empty() {
            yaml = String::new();
        }
        if !yaml.ends_with('\n') && !yaml.is_empty() {
            yaml.push('\n');
        }

        let provider_label = if provider_name.trim().is_empty() {
            "LLM-API-Proxy"
        } else {
            provider_name.trim()
        };

        // Model list: one entry per pool. Each pool's context_length defaults
        // to its inferred real context window (fallback), else 200000; the
        // default pool is forced to 1M when the default-pool 1M flag is set.
        let default_1m = roles_1m.iter().any(|r| r == "default");
        let window_of = |name: &str| -> u64 {
            all_pools
                .iter()
                .find(|p| p.name == name)
                .and_then(|p| p.context_window)
                .map(|w| w as u64)
                .unwrap_or(DEFAULT_CONTEXT_LENGTH)
        };
        let mut models_yaml = String::new();
        for pool in all_pools {
            let ctx = if default_1m && pool.name == default_pool_name {
                ONE_M_CONTEXT_LENGTH
            } else {
                window_of(&pool.name)
            };
            models_yaml.push_str(&format!("      {}:\n        context_length: {ctx}\n", pool.name));
        }
        if models_yaml.is_empty() {
            let ctx = if default_1m {
                ONE_M_CONTEXT_LENGTH
            } else {
                DEFAULT_CONTEXT_LENGTH
            };
            models_yaml.push_str(&format!(
                "      {default_pool_name}:\n        context_length: {ctx}\n"
            ));
        }

        // Append a custom provider section with the full model list + default model.
        let block = format!(
            "\ncustom_providers:\n  {PROVIDER_ID}:\n    base_url: \"{proxy_base_url}\"\n    api_key: \"{proxy_api_key}\"\n    name: \"{provider_label}\"\n    models:\n{models_yaml}model:\n  default: \"{default_pool_name}\"\n"
        );
        yaml.push_str(&block);

        atomic_write(path, yaml.as_bytes())
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
    fn test_hermes_merge_creates_yaml() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("config.yaml");
        let writer = HermesWriter;
        let original = vec![(cfg.clone(), None)];
        writer
            .merge_and_write_config(&original, "http://127.0.0.1:47339", "sk-k", &[ToolPool::new("pool-x", "Pool X"), ToolPool::new("pool-y", "Pool Y")], "pool-x", "Pool X", "LLM-API-Proxy")
            .unwrap();
        let yaml = std::fs::read_to_string(&cfg).unwrap();
        assert!(yaml.contains("custom_providers:"));
        assert!(yaml.contains("base_url: \"http://127.0.0.1:47339\""));
        assert!(yaml.contains("default: \"pool-x\""));
        // Full model list: all pools written as models
        assert!(yaml.contains("pool-x:\n        context_length: 200000"), "got: {yaml}");
        assert!(yaml.contains("pool-y:\n        context_length: 200000"), "got: {yaml}");
    }

    #[test]
    fn test_hermes_preserves_existing_content() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("config.yaml");
        std::fs::write(&cfg, "settings:\n  theme: dark\n").unwrap();
        let writer = HermesWriter;
        let original = vec![(cfg.clone(), Some("settings:\n  theme: dark\n".to_string()))];
        writer
            .merge_and_write_config(&original, "http://x", "k", &[], "m", "M", "P")
            .unwrap();
        let yaml = std::fs::read_to_string(&cfg).unwrap();
        assert!(yaml.starts_with("settings:\n  theme: dark\n"));
        assert!(yaml.contains("custom_providers:"));
    }

    #[test]
    fn test_hermes_default_pool_1m_declares_larger_context() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("config.yaml");
        let writer = HermesWriter;
        let original = vec![(cfg.clone(), None)];
        writer
            .merge_and_write_config_with_roles_1m(
                &original,
                "http://127.0.0.1:47339",
                "sk-k",
                &[ToolPool::new("pool-x", "Pool X"), ToolPool::new("pool-y", "Pool Y")],
                "pool-x",
                "Pool X",
                "LLM-API-Proxy",
                &[],
                &["default".to_string()],
            )
            .unwrap();
        let yaml = std::fs::read_to_string(&cfg).unwrap();
        // Default pool declares the 1M window; other pools keep 200K.
        assert!(yaml.contains("pool-x:\n        context_length: 1000000"), "got: {yaml}");
        assert!(yaml.contains("pool-y:\n        context_length: 200000"), "got: {yaml}");
        assert!(yaml.contains("default: \"pool-x\""));
    }

    #[test]
    fn test_hermes_default_pool_1m_toggle_off_keeps_200k() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("config.yaml");
        let writer = HermesWriter;
        let original = vec![(cfg.clone(), None)];
        writer
            .merge_and_write_config_with_roles_1m(
                &original,
                "http://127.0.0.1:47339",
                "sk-k",
                &[ToolPool::new("pool-x", "Pool X")],
                "pool-x",
                "Pool X",
                "LLM-API-Proxy",
                &[],
                &[],
            )
            .unwrap();
        let yaml = std::fs::read_to_string(&cfg).unwrap();
        assert!(yaml.contains("pool-x:\n        context_length: 200000"), "got: {yaml}");
    }
}
