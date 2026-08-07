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
        detector::codex_installed()
    }

    fn config_paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Some(p) = detector::codex_home_path(AUTH_REL) {
            paths.push(p);
        }
        if let Some(p) = detector::codex_home_path(CONFIG_REL) {
            paths.push(p);
        }
        paths
    }

    fn read_original_config(&self) -> Result<Vec<BackupEntry>, AppError> {
        let auth = detector::codex_home_path(AUTH_REL)
            .ok_or_else(|| AppError::Config("无法定位用户主目录".to_string()))?;
        let config = detector::codex_home_path(CONFIG_REL)
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

        // config.toml: order-preserving upsert of the custom provider section.
        // Preserves all existing keys/tables (model_catalog_json, notify,
        // mcp_servers, etc.) and reuses the existing `[model_providers.custom]`
        // table instead of appending a duplicate section.
        let mut doc = config_original
            .clone()
            .unwrap_or_default()
            .parse::<toml_edit::DocumentMut>()
            .unwrap_or_default();

        doc["model_provider"] = toml_edit::value("custom");
        doc["model"] = toml_edit::value(default_pool_name);

        if !doc.as_table().contains_key("model_providers") {
            doc["model_providers"] = toml_edit::table();
        }
        let providers = doc
            .get_mut("model_providers")
            .and_then(toml_edit::Item::as_table_mut)
            .ok_or_else(|| AppError::Config("model_providers 必须是 TOML 表".to_string()))?;
        if !providers.contains_key("custom") {
            providers.insert("custom", toml_edit::table());
        }
        let provider = providers
            .get_mut("custom")
            .and_then(toml_edit::Item::as_table_mut)
            .ok_or_else(|| AppError::Config("model_providers.custom 必须是 TOML 表".to_string()))?;
        provider["name"] = toml_edit::value(provider_name);
        provider["base_url"] = toml_edit::value(proxy_base_url);
        provider["wire_api"] = toml_edit::value("responses");
        provider["requires_openai_auth"] = toml_edit::value(true);

        let toml = doc.to_string();

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
        std::fs::write(&config, "sandbox_mode = \"danger-full-access\"\n").unwrap();

        let writer = CodexWriter;
        let original = vec![
            (auth.clone(), None),
            (
                config.clone(),
                Some("sandbox_mode = \"danger-full-access\"\n".to_string()),
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
        // Original unrelated root key preserved
        assert!(toml.contains("sandbox_mode = \"danger-full-access\""));
        // Proxy provider upserted
        assert!(toml.contains("[model_providers.custom]"));
        assert!(toml.contains("model = \"my-pool\""));
        assert!(toml.contains("model_provider = \"custom\""));
        // No duplicate section
        let occurrences = toml.matches("[model_providers.custom]").count();
        assert_eq!(occurrences, 1, "duplicate [model_providers.custom] found: {toml}");
    }

    #[test]
    fn test_codex_merge_reuses_existing_custom_section_no_duplicate() {
        // Simulate a real user config that already has [model_providers.custom]
        // (e.g. written by cc-switch): the proxy must overwrite fields in place,
        // not append a second identical section.
        let dir = TempDir::new().unwrap();
        let auth = dir.path().join("auth.json");
        let config = dir.path().join("config.toml");
        let existing = r#"model = "gpt-5.4"
model_provider = "custom"
model_catalog_json = "cc-switch-model-catalog.json"

[model_providers.custom]
name = "custom"
wire_api = "responses"
requires_openai_auth = true
base_url = "http://127.0.0.1:57321/v1"
"#;
        std::fs::write(&config, existing).unwrap();

        let writer = CodexWriter;
        let original = vec![
            (auth.clone(), None),
            (config.clone(), Some(existing.to_string())),
        ];

        writer
            .merge_and_write_config(
                &original,
                "http://127.0.0.1:47339",
                "sk-gw-proxy",
                &[],
                "grok-4.5",
                "Grok 4.5",
                "LLM-API-Proxy",
            )
            .unwrap();

        let toml = std::fs::read_to_string(&config).unwrap();
        // Exactly one [model_providers.custom] section
        assert_eq!(toml.matches("[model_providers.custom]").count(), 1);
        // base_url overwritten in place, model_catalog_json preserved
        assert!(toml.contains("base_url = \"http://127.0.0.1:47339\""));
        assert!(toml.contains("model_catalog_json = \"cc-switch-model-catalog.json\""));
        assert!(toml.contains("model = \"grok-4.5\""));
        // It must parse as valid TOML (no duplicate-key error)
        let parsed = toml.parse::<toml_edit::DocumentMut>();
        assert!(parsed.is_ok(), "config.toml no longer valid TOML: {toml}");
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
