//! Grok CLI config writer (`~/.grok/config.toml`).
//! Uses the same Codex-style TOML layout (model_provider + [model_providers.X]).

use crate::error::AppError;
use crate::tool_config::backup::BackupEntry;
use crate::tool_config::detector;
use crate::tool_config::writer::atomic_write;
use crate::tool_config::ToolConfigWriter;
use std::path::PathBuf;

pub struct GrokWriter;

const APP_ID: &str = "grokbuild";

impl ToolConfigWriter for GrokWriter {
    fn app_id(&self) -> &'static str {
        APP_ID
    }

    fn display_name(&self) -> &'static str {
        "Grok CLI"
    }

    fn download_url(&self) -> &'static str {
        "https://github.com/xai-org/grok-build"
    }

    fn is_installed(&self) -> bool {
        detector::cli_installed("grok")
            || detector::config_dir_installed(detector::home_path(".grok"), 2)
    }

    fn config_paths(&self) -> Vec<PathBuf> {
        detector::grok_config_path().into_iter().collect()
    }

    fn read_original_config(&self) -> Result<Vec<BackupEntry>, AppError> {
        let path = detector::grok_config_path()
            .ok_or_else(|| AppError::Config("无法定位用户主目录".to_string()))?;
        let content = std::fs::read_to_string(&path).ok();
        Ok(vec![(path, content)])
    }

    fn merge_and_write_config(
        &self,
        original_configs: &[BackupEntry],
        proxy_base_url: &str,
        _proxy_api_key: &str,
        _all_pools: &[(String, String)],
        default_pool_name: &str,
        _default_pool_display_name: &str,
        provider_name: &str,
    ) -> Result<(), AppError> {
        let (path, original) = original_configs
            .first()
            .ok_or_else(|| AppError::Config("缺少原始配置".to_string()))?;

        let mut toml = original
            .clone()
            .unwrap_or_default()
            .trim_end()
            .to_string();
        if toml.is_empty() {
            toml = String::new();
        }
        if !toml.ends_with('\n') && !toml.is_empty() {
            toml.push('\n');
        }

        // Provider-scoped key so multiple profiles don't clobber each other.
        let provider_key = provider_name.trim();
        let provider_key = if provider_key.is_empty() {
            "llm-api-proxy".to_string()
        } else {
            provider_key.to_string()
        };

        let block = format!(
            "\nmodel_provider = \"{provider_key}\"\nmodel = \"{default_pool_name}\"\n\n[model_providers.{provider_key}]\nname = \"{provider_name}\"\nbase_url = \"{proxy_base_url}\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n",
        );
        toml.push_str(&block);

        atomic_write(path, toml.as_bytes())
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
    fn test_grok_merge_creates_toml() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("config.toml");
        let writer = GrokWriter;
        let original = vec![(cfg.clone(), None)];
        writer
            .merge_and_write_config(
                &original,
                "http://127.0.0.1:47339",
                "sk-k",
                &[],
                "grok-pool",
                "Grok Pool",
                "LLM-API-Proxy",
            )
            .unwrap();
        let toml = std::fs::read_to_string(&cfg).unwrap();
        assert!(toml.contains("model_provider = \"LLM-API-Proxy\""));
        assert!(toml.contains("model = \"grok-pool\""));
        assert!(toml.contains("base_url = \"http://127.0.0.1:47339\""));
    }

    #[test]
    fn test_grok_preserves_existing() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "disable_browser = true\n").unwrap();
        let writer = GrokWriter;
        let original = vec![(cfg.clone(), Some("disable_browser = true\n".to_string()))];
        writer
            .merge_and_write_config(&original, "http://x", "k", &[], "m", "M", "P")
            .unwrap();
        let toml = std::fs::read_to_string(&cfg).unwrap();
        assert!(toml.starts_with("disable_browser = true"));
        assert!(toml.contains("[model_providers.P]"));
    }
}
