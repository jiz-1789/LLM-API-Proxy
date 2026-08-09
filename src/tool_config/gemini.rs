//! Gemini CLI config writer.
//!
//! Gemini CLI reads credentials from `~/.gemini/.env` (`GEMINI_API_KEY`,
//! `GOOGLE_GEMINI_BASE_URL`, `GEMINI_MODEL`). The `env` JSON object inside `settings.json` is NOT read by the CLI,
//! so it must not be used as the write target. Additionally `settings.json`
//! gets `security.auth.selectedType = "gemini-api-key"` so the CLI prefers the
//! injected API key over OAuth.

use crate::error::AppError;
use crate::tool_config::backup::BackupEntry;
use crate::tool_config::detector;
use crate::tool_config::writer::atomic_write;
use crate::tool_config::ToolConfigWriter;
use crate::tool_config::ToolPool;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub struct GeminiWriter;

const APP_ID: &str = "gemini";
const ENV_API_KEY: &str = "GEMINI_API_KEY";
const ENV_BASE_URL: &str = "GOOGLE_GEMINI_BASE_URL";
const ENV_MODEL: &str = "GEMINI_MODEL";

/// The `.env` file lives next to `settings.json` (both under `~/.gemini/`).
fn env_path_for(settings_path: &std::path::Path) -> PathBuf {
    settings_path.with_file_name(".env")
}

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
        let mut out: Vec<PathBuf> = detector::gemini_settings_path().into_iter().collect();
        if let Some(settings) = detector::gemini_settings_path() {
            out.push(env_path_for(&settings));
        }
        out
    }

    fn read_original_config(&self) -> Result<Vec<BackupEntry>, AppError> {
        let mut out = Vec::new();
        if let Some(path) = detector::gemini_settings_path() {
            let content = std::fs::read_to_string(&path).ok();
            let env = env_path_for(&path);
            let env_content = std::fs::read_to_string(&env).ok();
            out.push((path, content));
            out.push((env, env_content));
        }
        Ok(out)
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
        let (settings_path, settings_original) = original_configs
            .first()
            .ok_or_else(|| AppError::Config("缺少原始配置".to_string()))?;
        let env_original = original_configs
            .get(1)
            .map(|(_, c)| c.as_deref().unwrap_or_default())
            .unwrap_or_default();

        // 1) `.env`: preserve existing entries, upsert the three proxy vars.
        let mut env: BTreeMap<String, String> = parse_env(env_original);
        env.insert(ENV_API_KEY.to_string(), proxy_api_key.to_string());
        env.insert(ENV_BASE_URL.to_string(), proxy_base_url.to_string());
        if !default_pool_name.is_empty() {
            env.insert(ENV_MODEL.to_string(), default_pool_name.to_string());
        }
        let mut env_text = String::new();
        for (k, v) in &env {
            env_text.push_str(&format!("{k}={v}\n"));
        }
        let env_path = env_path_for(settings_path);
        if let Some(parent) = env_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AppError::Config(format!("创建 Gemini 配置目录失败: {e}")))?;
        }
        atomic_write(&env_path, env_text.as_bytes())?;

        // 2) `settings.json`: switch the CLI auth type to API-key mode so it
        //    prefers the injected key over OAuth.
        let mut root: Value = match settings_original {
            Some(content) if !content.trim().is_empty() => serde_json::from_str(content)
                .map_err(|e| AppError::Config(format!("解析 settings.json 失败: {e}")))?,
            _ => json!({}),
        };
        if let Some(obj) = root.as_object_mut() {
            let security = obj.entry("security").or_insert_with(|| json!({}));
            if let Some(sec) = security.as_object_mut() {
                let auth = sec.entry("auth").or_insert_with(|| json!({}));
                if let Some(auth_obj) = auth.as_object_mut() {
                    auth_obj.insert(
                        "selectedType".to_string(),
                        Value::String("gemini-api-key".to_string()),
                    );
                }
            }
        }
        atomic_write(
            settings_path,
            serde_json::to_string_pretty(&root).unwrap_or_default().as_bytes(),
        )
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

/// Parse a `.env` file into a key→value map (lax: skips blank/comment/malformed lines).
fn parse_env(content: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            if !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                map.insert(key.to_string(), value.trim().to_string());
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_parse_env_lax() {
        let map = parse_env("GEMINI_API_KEY=sk-1\n\n# comment\nGEMINI_MODEL=gemini-2.5\nBROKEN LINE\n");
        assert_eq!(map.get("GEMINI_API_KEY").unwrap(), "sk-1");
        assert_eq!(map.get("GEMINI_MODEL").unwrap(), "gemini-2.5");
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn test_gemini_writes_env_and_settings() {
        let dir = TempDir::new().unwrap();
        // Manually simulate detector path: settings.json in a temp dir.
        let settings = dir.path().join("settings.json");
        let env = dir.path().join(".env");
        let writer = GeminiWriter;
        // The writer derives .env from the settings path; construct the
        // original entries accordingly.
        let original = vec![(settings.clone(), None), (env.clone(), None)];
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
        // .env content: three proxy vars present.
        let env_text = std::fs::read_to_string(&env).unwrap();
        assert!(env_text.contains("GEMINI_API_KEY=sk-gw\n"));
        assert!(env_text.contains("GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:47339\n"));
        assert!(env_text.contains("GEMINI_MODEL=deepseek-v4-pro\n"));
        // settings.json: selectedType switched to API-key mode.
        let settings_json: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(
            settings_json["security"]["auth"]["selectedType"],
            "gemini-api-key"
        );
    }

    #[test]
    fn test_gemini_preserves_existing_env_and_skips_empty_model() {
        let dir = TempDir::new().unwrap();
        let settings = dir.path().join("settings.json");
        let env = dir.path().join(".env");
        let writer = GeminiWriter;
        let existing_env = "# existing\nGEMINI_MODEL=old-model\nGEMINI_API_KEY=old-key\n";
        let original = vec![
            (settings.clone(), None),
            (env.clone(), Some(existing_env.to_string())),
        ];
        writer
            .merge_and_write_config(&original, "http://x", "k", &[], "", "Pool", "P")
            .unwrap();
        let env_text = std::fs::read_to_string(&env).unwrap();
        assert!(env_text.contains("GEMINI_MODEL=old-model\n"));
        assert!(env_text.contains("GOOGLE_GEMINI_BASE_URL=http://x\n"));
        assert!(env_text.contains("GEMINI_API_KEY=k\n"));
    }

    #[test]
    fn test_gemini_restore_removes_created_files() {
        let dir = TempDir::new().unwrap();
        let settings = dir.path().join("settings.json");
        let env = dir.path().join(".env");
        let writer = GeminiWriter;
        let original = vec![(settings.clone(), None), (env.clone(), None)];
        writer
            .merge_and_write_config(&original, "http://x", "k", &[], "m", "M", "P")
            .unwrap();
        assert!(env.exists());
        assert!(settings.exists());
        writer.restore_original_config(&original).unwrap();
        assert!(!env.exists());
        assert!(!settings.exists());
    }
}