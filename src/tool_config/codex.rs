//! Codex CLI config writer (`~/.codex/auth.json` + `~/.codex/config.toml`).
//!
//! Codex uses two files:
//! - `auth.json`: stores `OPENAI_API_KEY`
//! - `config.toml`: declares a custom model provider pointing at the proxy

use crate::error::AppError;
use crate::tool_config::detector;
use crate::tool_config::writer::{atomic_write, atomic_write_multi};
use crate::tool_config::backup::BackupEntry;
use crate::tool_config::ToolConfigWriter;
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct CodexWriter;

const APP_ID: &str = "codex";
const AUTH_REL: &str = ".codex/auth.json";
const CONFIG_REL: &str = ".codex/config.toml";

impl ToolConfigWriter for CodexWriter {
    fn app_id(&self) -> &'static str {
        APP_ID
    }

    fn display_name(&self) -> &'static str {
        "Codex CLI"
    }

    fn download_url(&self) -> &'static str {
        "https://github.com/openai/codex"
    }

    fn is_installed(&self) -> bool {
        detector::cli_installed("codex")
    }

    fn config_paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Some(p) = detector::home_path(AUTH_REL) {
            paths.push(p);
        }
        if let Some(p) = detector::home_path(CONFIG_REL) {
            paths.push(p);
        }
        paths
    }

    fn read_original_config(&self) -> Result<Vec<BackupEntry>, AppError> {
        let auth = detector::home_path(AUTH_REL)
            .ok_or_else(|| AppError::Config("无法定位用户主目录".to_string()))?;
        let config = detector::home_path(CONFIG_REL)
            .ok_or_else(|| AppError::Config("无法定位用户主目录".to_string()))?;
        Ok(vec![
            (auth.clone(), std::fs::read_to_string(&auth).ok()),
            (config.clone(), std::fs::read_to_string(&config).ok()),
        ])
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
        let (auth_path, auth_original) = original_configs
            .first()
            .ok_or_else(|| AppError::Config("缺少 auth.json 配置".to_string()))?;
        let (config_path, config_original) = original_configs
            .get(1)
            .ok_or_else(|| AppError::Config("缺少 config.toml 配置".to_string()))?;

        // auth.json: deep-merge OPENAI_API_KEY
        let mut auth_root: Value = match auth_original {
            Some(content) if !content.trim().is_empty() => serde_json::from_str(content)
                .map_err(|e| AppError::Config(format!("解析 auth.json 失败: {e}")))?,
            _ => json!({}),
        };
        if let Some(obj) = auth_root.as_object_mut() {
            obj.insert("OPENAI_API_KEY".to_string(), Value::String(proxy_api_key.to_string()));
        }
        let auth_content = serde_json::to_string_pretty(&auth_root).unwrap_or_default();

        // config.toml: preserve existing, rewrite/insert model_provider section
        let mut toml = config_original
            .clone()
            .unwrap_or_default()
            .trim_end()
            .to_string();
        if toml.is_empty() {
            toml = String::new();
        }

        // Ensure config.toml ends with newline before appending
        if !toml.ends_with('\n') && !toml.is_empty() {
            toml.push('\n');
        }

        let provider_block = format!(
            "\nmodel_provider = \"custom\"\nmodel = \"{}\"\n\n[model_providers.custom]\nname = \"{}\"\nbase_url = \"{}\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n",
            default_pool_name, provider_name, proxy_base_url
        );
        toml.push_str(&provider_block);

        atomic_write_multi(&[
            (auth_path.clone(), auth_content),
            (config_path.clone(), toml),
        ])
    }

    fn restore_original_config(&self, original_configs: &[BackupEntry]) -> Result<(), AppError> {
        for (path, content) in original_configs {
            match content {
                Some(content) => atomic_write(path, content.as_bytes())?,
                None => {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // Build a writer backed by explicit temp paths by monkey-patching home.
    // detector::home_path reads dirs::home_dir(), which we can't easily
    // override; instead we test the merge logic directly via a small wrapper.
    #[test]
    fn test_codex_merge_creates_both_files() {
        let dir = TempDir::new().unwrap();
        let auth = dir.path().join("auth.json");
        let config = dir.path().join("config.toml");

        // Since home_path is fixed to real home, write to temp via a direct
        // merge call by constructing original entries pointing at temp paths.
        let writer = CodexWriter;
        let original = vec![
            (auth.clone(), None),
            (config.clone(), None),
        ];

        writer
            .merge_and_write_config(
                &original,
                "http://127.0.0.1:47339",
                "sk-gw-test",
                &[],
                "gpt-4-pool",
                "GPT-4",
                "LLM-API-Proxy",
            )
            .unwrap();

        // auth.json
        let auth_val: Value =
            serde_json::from_str(&std::fs::read_to_string(&auth).unwrap()).unwrap();
        assert_eq!(auth_val["OPENAI_API_KEY"], "sk-gw-test");

        // config.toml
        let toml = std::fs::read_to_string(&config).unwrap();
        assert!(toml.contains("model_provider = \"custom\""));
        assert!(toml.contains("model = \"gpt-4-pool\""));
        assert!(toml.contains("base_url = \"http://127.0.0.1:47339\""));
        assert!(toml.contains("wire_api = \"responses\""));
        assert!(toml.contains("name = \"LLM-API-Proxy\""));
    }

    #[test]
    fn test_codex_merge_preserves_existing_config_toml() {
        let dir = TempDir::new().unwrap();
        let auth = dir.path().join("auth.json");
        let config = dir.path().join("config.toml");
        std::fs::write(&config, "model_provider = \"openai\"\n").unwrap();

        let writer = CodexWriter;
        let original = vec![
            (auth.clone(), None),
            (
                config.clone(),
                Some("model_provider = \"openai\"\n".to_string()),
            ),
        ];

        writer
            .merge_and_write_config(
                &original,
                "http://127.0.0.1:47339",
                "sk-k",
                &[],
                "my-pool",
                "My Pool",
                "P",
            )
            .unwrap();

        let toml = std::fs::read_to_string(&config).unwrap();
        // Original line preserved
        assert!(toml.starts_with("model_provider = \"openai\""));
        // Proxy block appended
        assert!(toml.contains("[model_providers.custom]"));
        assert!(toml.contains("model = \"my-pool\""));
    }

    #[test]
    fn test_codex_restore_restores_files() {
        let dir = TempDir::new().unwrap();
        let auth = dir.path().join("auth.json");
        let config = dir.path().join("config.toml");
        std::fs::write(&auth, "{\"OPENAI_API_KEY\":\"tmp\"}").unwrap();
        std::fs::write(&config, "model_provider = \"custom\"").unwrap();

        let writer = CodexWriter;
        let original = vec![
            (auth.clone(), Some(r#"{"OPENAI_API_KEY":"original"}"#.to_string())),
            (config.clone(), Some("model_provider = \"openai\"".to_string())),
        ];
        writer.restore_original_config(&original).unwrap();
        assert_eq!(
            std::fs::read_to_string(&auth).unwrap(),
            r#"{"OPENAI_API_KEY":"original"}"#
        );
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "model_provider = \"openai\""
        );
    }

    #[test]
    fn test_codex_restore_removes_files_when_absent_original() {
        let dir = TempDir::new().unwrap();
        let auth = dir.path().join("auth.json");
        std::fs::write(&auth, "{}").unwrap();
        let writer = CodexWriter;
        let original = vec![(auth.clone(), None)];
        writer.restore_original_config(&original).unwrap();
        assert!(!auth.exists());
    }

    #[test]
    fn test_codex_ids() {
        let w = CodexWriter;
        assert_eq!(w.app_id(), "codex");
        assert_eq!(w.display_name(), "Codex CLI");
    }
}
