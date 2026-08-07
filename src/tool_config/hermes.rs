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
use std::path::PathBuf;

pub struct HermesWriter;

const APP_ID: &str = "hermes";
const PROVIDER_ID: &str = "llm-api-proxy";

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
        detector::hermes_config_path()
            .map(|p| p.exists())
            .unwrap_or(false)
            || detector::which_in_path("hermes").is_some()
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
        _all_pools: &[(String, String)],
        default_pool_name: &str,
        _default_pool_display_name: &str,
        provider_name: &str,
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

        // Append a custom provider section + default model.
        let block = format!(
            "\ncustom_providers:\n  {PROVIDER_ID}:\n    base_url: \"{proxy_base_url}\"\n    api_key: \"{proxy_api_key}\"\n    name: \"{provider_label}\"\nmodel:\n  default: \"{default_pool_name}\"\n"
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
            .merge_and_write_config(&original, "http://127.0.0.1:47339", "sk-k", &[], "pool-x", "Pool X", "LLM-API-Proxy")
            .unwrap();
        let yaml = std::fs::read_to_string(&cfg).unwrap();
        assert!(yaml.contains("custom_providers:"));
        assert!(yaml.contains("base_url: \"http://127.0.0.1:47339\""));
        assert!(yaml.contains("default: \"pool-x\""));
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
}
