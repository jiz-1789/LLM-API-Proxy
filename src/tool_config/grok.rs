//! Grok CLI config writer (`~/.grok/config.toml`).
//!
//! Grok Build reads a top-level `[models]` table plus one `[model."<name>"]`
//! table per model profile (`models.default` + `model.<default>.{model,base_url,
//! name,api_key,api_backend,context_window}`). One profile is generated per proxy pool so
//! the CLI can switch between all pools; `models.default` points at the active
//! pool.

use crate::error::AppError;
use crate::tool_config::backup::BackupEntry;
use crate::tool_config::detector;
use crate::tool_config::writer::atomic_write;
use crate::tool_config::ToolConfigWriter;
use crate::tool_config::ToolPool;
use std::path::PathBuf;

pub struct GrokWriter;

const APP_ID: &str = "grokbuild";
const DEFAULT_API_BACKEND: &str = "responses";
const DEFAULT_CONTEXT_WINDOW: i64 = 500_000;

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
        proxy_api_key: &str,
        all_pools: &[ToolPool],
        default_pool_name: &str,
        default_pool_display_name: &str,
        _provider_name: &str,
    ) -> Result<(), AppError> {
        let (path, original) = original_configs
            .first()
            .ok_or_else(|| AppError::Config("缺少原始配置".to_string()))?;

        // Order-preserving upsert: reuse an existing `[model."{pool}"]` table
        // instead of appending a duplicate section.
        let mut doc = original
            .clone()
            .unwrap_or_default()
            .parse::<toml_edit::DocumentMut>()
            .unwrap_or_default();

        // One profile per pool; fall back to the default pool when no pools
        // are known (e.g. no pools registered yet).
        let mut profiles: Vec<(String, String, Option<i32>)> = all_pools
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    if p.display_name.is_empty() {
                        p.name.clone()
                    } else {
                        p.display_name.clone()
                    },
                    p.context_window,
                )
            })
            .collect();
        if profiles.is_empty() {
            profiles.push((
                default_pool_name.to_string(),
                default_pool_display_name.to_string(),
                None,
            ));
        }

        if !doc.as_table().contains_key("models") {
            doc["models"] = toml_edit::table();
        }
        doc["models"]["default"] = toml_edit::value(&profiles[0].0);
        // On existing configs, always point `default` at the currently
        // selected pool even if a stale default still references a removed
        // profile.
        let profiles_len = profiles.len();

        // Models live under `[model."<profile>"]`.
        if !doc.as_table().contains_key("model") {
            doc["model"] = toml_edit::table();
        }
        let model_root = doc
            .get_mut("model")
            .and_then(toml_edit::Item::as_table_mut)
            .ok_or_else(|| AppError::Config("model 必须是 TOML 表".to_string()))?;
        for (name, display, window) in profiles {
            if !model_root.contains_key(&name) {
                model_root.insert(&name, toml_edit::Item::Table(toml_edit::Table::new()));
            }
            let table = model_root
                .get_mut(&name)
                .and_then(toml_edit::Item::as_table_mut)
                .ok_or_else(|| AppError::Config("model.{name} 必须是 TOML 表".to_string()))?;
            table.insert("model", toml_edit::value(&name));
            table.insert("base_url", toml_edit::value(proxy_base_url));
            table.insert("name", toml_edit::value(display));
            table.insert("api_key", toml_edit::value(proxy_api_key));
            table.insert("api_backend", toml_edit::value(DEFAULT_API_BACKEND));
            let window = window
                .map(i64::from)
                .filter(|w| *w > 0)
                .unwrap_or(DEFAULT_CONTEXT_WINDOW);
            table.insert("context_window", toml_edit::value(window));
        }
        debug_assert!(profiles_len > 0, "at least one profile must be written");

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
    fn test_grok_creates_official_model_table_shape() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("config.toml");
        let writer = GrokWriter;
        let original = vec![(cfg.clone(), None)];
        writer
            .merge_and_write_config(
                &original,
                "http://127.0.0.1:47339/v1",
                "sk-k",
                &[ToolPool::new("grok-pool", "Grok Pool")],
                "grok-pool",
                "Grok Pool",
                "LLM-API-Proxy",
            )
            .unwrap();
        let toml = std::fs::read_to_string(&cfg).unwrap();
        assert!(toml.contains("[models]"));
        assert!(toml.contains("default = \"grok-pool\""));
        let parsed = toml.parse::<toml_edit::DocumentMut>().unwrap();
        let model = parsed["model"]["grok-pool"].as_table().expect("model profile");
        assert_eq!(model["model"].as_str().unwrap(), "grok-pool");
        assert_eq!(model["base_url"].as_str().unwrap(), "http://127.0.0.1:47339/v1");
        assert_eq!(model["name"].as_str().unwrap(), "Grok Pool");
        assert_eq!(model["api_key"].as_str().unwrap(), "sk-k");
        assert_eq!(model["api_backend"].as_str().unwrap(), "responses");
        assert_eq!(model["context_window"].as_integer().unwrap(), 500_000);
        // Old Codex-style keys must NOT be written.
        assert!(!toml.contains("model_provider"));
        assert!(!toml.contains("wire_api"));
        assert!(!toml.contains("experimental_bearer_token"));
    }

    #[test]
    fn test_grok_writes_all_pool_profiles() {
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
                    ToolPool::new("deepseek-v4-pro", "DeepSeek V4 Pro"),
                    ToolPool::with_window("deepseek-v4-flash", "DeepSeek V4 Flash", 128000),
                ],
                "deepseek-v4-pro",
                "DeepSeek V4 Pro",
                "LLM-API-Proxy",
            )
            .unwrap();
        let toml = std::fs::read_to_string(&cfg).unwrap();
        let parsed = toml.parse::<toml_edit::DocumentMut>().unwrap();
        let pro = parsed["model"]["deepseek-v4-pro"].as_table().unwrap();
        assert_eq!(pro["model"].as_str().unwrap(), "deepseek-v4-pro");
        assert_eq!(pro["name"].as_str().unwrap(), "DeepSeek V4 Pro");
        let flash = parsed["model"]["deepseek-v4-flash"].as_table().unwrap();
        assert_eq!(flash["name"].as_str().unwrap(), "DeepSeek V4 Flash");
        // Known window is used; unknown falls back to the Grok default.
        assert_eq!(flash["context_window"].as_integer().unwrap(), 128_000);
        assert_eq!(pro["context_window"].as_integer().unwrap(), 500_000);
    }

    #[test]
    fn test_grok_preserves_existing_other_sections() {
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
        assert!(toml.contains("[model.m]") || toml.contains("[model.\"m\"]"));
    }

    #[test]
    fn test_grok_reuses_existing_profile_table_no_duplicate() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("config.toml");
        let existing = "[models]\ndefault = \"old-model\"\n\n[model.old-model]\nmodel = \"old-model\"\nbase_url = \"http://old:8080/v1\"\napi_key = \"old-key\"\napi_backend = \"responses\"\ncontext_window = 500000\n";
        std::fs::write(&cfg, existing).unwrap();
        let writer = GrokWriter;
        let original = vec![(cfg.clone(), Some(existing.to_string()))];
        writer
            .merge_and_write_config(&original, "http://new:47339", "k", &[], "new-pool", "P", "P")
            .unwrap();
        let toml = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(
            toml.matches("[model.\"new-pool\"]").count()
                + toml.matches("[model.new-pool]").count(),
            1
        );
        assert!(toml.contains("base_url = \"http://new:47339\""));
        assert!(toml.contains("default = \"new-pool\""));
        let parsed = toml.parse::<toml_edit::DocumentMut>();
        assert!(parsed.is_ok(), "invalid TOML after upsert: {toml}");
    }
}