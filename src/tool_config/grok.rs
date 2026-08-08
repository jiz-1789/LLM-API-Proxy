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
        all_pools: &[(String, String)],
        default_pool_name: &str,
        default_pool_display_name: &str,
        provider_name: &str,
    ) -> Result<(), AppError> {
        let (path, original) = original_configs
            .first()
            .ok_or_else(|| AppError::Config("缺少原始配置".to_string()))?;

        // Provider-scoped key so multiple profiles don't clobber each other.
        let provider_key = provider_name.trim();
        let provider_key = if provider_key.is_empty() {
            "llm-api-proxy".to_string()
        } else {
            provider_key.to_string()
        };

        // Order-preserving upsert: reuse an existing `[model_providers.{key}]`
        // table instead of appending a duplicate section.
        let mut doc = original
            .clone()
            .unwrap_or_default()
            .parse::<toml_edit::DocumentMut>()
            .unwrap_or_default();

        doc["model_provider"] = toml_edit::value(&provider_key);
        doc["model"] = toml_edit::value(default_pool_name);

        if !doc.as_table().contains_key("model_providers") {
            doc["model_providers"] = toml_edit::table();
        }
        let providers = doc
            .get_mut("model_providers")
            .and_then(toml_edit::Item::as_table_mut)
            .ok_or_else(|| AppError::Config("model_providers 必须是 TOML 表".to_string()))?;
        if !providers.contains_key(&provider_key) {
            providers.insert(&provider_key, toml_edit::table());
        }
        let provider = providers
            .get_mut(&provider_key)
            .and_then(toml_edit::Item::as_table_mut)
            .ok_or_else(|| AppError::Config("model_providers.{provider_key} 必须是 TOML 表".to_string()))?;
        provider["name"] = toml_edit::value(provider_name);
        provider["base_url"] = toml_edit::value(proxy_base_url);
        provider["wire_api"] = toml_edit::value("responses");
        provider["requires_openai_auth"] = toml_edit::value(true);

        // Register every pool as a switchable model under this provider so
        // Grok can switch between all proxy pools (not just the default one).
        if !provider.contains_key("models") {
            provider.insert("models", toml_edit::table());
        }
        let models = provider
            .get_mut("models")
            .and_then(toml_edit::Item::as_table_mut)
            .ok_or_else(|| AppError::Config(format!("model_providers.{provider_key}.models 必须是 TOML 表")))?;
        for (name, display) in all_pools {
            let mut desc = toml_edit::Table::new();
            desc.insert("name", toml_edit::value(display.clone()));
            models.insert(name.as_str(), toml_edit::Item::Table(desc));
        }
        if all_pools.is_empty() {
            let mut desc = toml_edit::Table::new();
            desc.insert("name", toml_edit::value(default_pool_display_name.to_string()));
            models.insert(default_pool_name, toml_edit::Item::Table(desc));
        }

        atomic_write(path, doc.to_string().as_bytes())
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
    fn test_grok_writes_all_pool_models_for_switching() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("config.toml");
        let writer = GrokWriter;
        let original = vec![(cfg.clone(), None)];
        writer
            .merge_and_write_config(
                &original,
                "http://127.0.0.1:47339/v1",
                "sk-k",
                &[
                    ("deepseek-v4-pro".to_string(), "DeepSeek V4 Pro".to_string()),
                    ("deepseek-v4-flash".to_string(), "DeepSeek V4 Flash".to_string()),
                ],
                "deepseek-v4-pro",
                "DeepSeek V4 Pro",
                "LLM-API-Proxy",
            )
            .unwrap();
        let toml = std::fs::read_to_string(&cfg).unwrap();
        let parsed = toml.parse::<toml_edit::DocumentMut>().unwrap();
        let models = parsed
            .get("model_providers")
            .and_then(|p| p.get("LLM-API-Proxy"))
            .and_then(|p| p.get("models"))
            .and_then(toml_edit::Item::as_table)
            .expect("model_providers.LLM-API-Proxy.models must exist");
        assert_eq!(models.len(), 2);
        assert_eq!(models["deepseek-v4-pro"]["name"].as_str().unwrap(), "DeepSeek V4 Pro");
        assert_eq!(models["deepseek-v4-flash"]["name"].as_str().unwrap(), "DeepSeek V4 Flash");
        assert_eq!(parsed["model"].as_str().unwrap(), "deepseek-v4-pro");
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

    #[test]
    fn test_grok_reuses_existing_provider_section_no_duplicate() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("config.toml");
        let existing = "model_provider = \"llm-api-proxy\"\nmodel = \"old-model\"\n\n[model_providers.llm-api-proxy]\nname = \"llm-api-proxy\"\nbase_url = \"http://old:8080/v1\"\nwire_api = \"responses\"\n";
        std::fs::write(&cfg, existing).unwrap();
        let writer = GrokWriter;
        let original = vec![(cfg.clone(), Some(existing.to_string()))];
        writer
            .merge_and_write_config(&original, "http://new:47339", "k", &[], "new-pool", "P", "llm-api-proxy")
            .unwrap();
        let toml = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(toml.matches("[model_providers.llm-api-proxy]").count(), 1);
        assert!(toml.contains("base_url = \"http://new:47339\""));
        assert!(toml.contains("model = \"new-pool\""));
        let parsed = toml.parse::<toml_edit::DocumentMut>();
        assert!(parsed.is_ok(), "invalid TOML after upsert: {toml}");
    }
}
