//! Claude Code config writer (`~/.claude/settings.json`).
//!
//! Writes `env.ANTHROPIC_BASE_URL`, `env.ANTHROPIC_AUTH_TOKEN` and the model
//! env vars into the existing settings.json, preserving all other fields.

use crate::error::AppError;
use crate::tool_config::detector;
use crate::tool_config::writer::atomic_write;
use crate::tool_config::backup::BackupEntry;
use crate::tool_config::{ToolConfigWriter};
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct ClaudeCodeWriter;

const APP_ID: &str = "claude";
const SETTINGS_REL: &str = ".claude/settings.json";

impl ToolConfigWriter for ClaudeCodeWriter {
    fn app_id(&self) -> &'static str {
        APP_ID
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    fn download_url(&self) -> &'static str {
        "https://docs.anthropic.com/en/docs/claude-code/setup"
    }

    fn is_installed(&self) -> bool {
        detector::claude_settings_path()
            .map(|p| p.exists())
            .unwrap_or(false)
            || detector::which_in_path("claude").is_some()
            || detector::which_in_path("claude.cmd").is_some()
    }

    fn config_paths(&self) -> Vec<PathBuf> {
        detector::home_path(SETTINGS_REL).into_iter().collect()
    }

    fn read_original_config(&self) -> Result<Vec<BackupEntry>, AppError> {
        let path = detector::home_path(SETTINGS_REL)
            .ok_or_else(|| AppError::Config("无法定位用户主目录".to_string()))?;
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
        _provider_name: &str,
    ) -> Result<(), AppError> {
        let (path, original) = original_configs
            .first()
            .ok_or_else(|| AppError::Config("缺少原始配置".to_string()))?;

        // Deep-merge into existing settings.json if present.
        let mut root: Value = match original {
            Some(content) if !content.trim().is_empty() => {
                serde_json::from_str(content)
                    .map_err(|e| AppError::Config(format!("解析现有 settings.json 失败: {e}")))? 
            }
            _ => json!({}),
        };

        let env = root
            .as_object_mut()
            .ok_or_else(|| AppError::Config("settings.json 根节点不是对象".to_string()))?
            .entry("env")
            .or_insert_with(|| json!({}));
        if let Some(env_obj) = env.as_object_mut() {
            env_obj.insert("ANTHROPIC_BASE_URL".to_string(), Value::String(proxy_base_url.to_string()));
            env_obj.insert("ANTHROPIC_AUTH_TOKEN".to_string(), Value::String(proxy_api_key.to_string()));
            env_obj.insert("ANTHROPIC_MODEL".to_string(), Value::String(default_pool_name.to_string()));
            env_obj.insert(
                "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(),
                Value::String(default_pool_name.to_string()),
            );
            env_obj.insert(
                "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
                Value::String(default_pool_name.to_string()),
            );
            env_obj.insert(
                "ANTHROPIC_DEFAULT_OPUS_MODEL".to_string(),
                Value::String(default_pool_name.to_string()),
            );
        }

        atomic_write(path, serde_json::to_string_pretty(&root).unwrap_or_default().as_bytes())
    }

    fn restore_original_config(&self, original_configs: &[BackupEntry]) -> Result<(), AppError> {
        let (path, original) = original_configs
            .first()
            .ok_or_else(|| AppError::Config("缺少原始配置".to_string()))?;
        match original {
            Some(content) => atomic_write(path, content.as_bytes()),
            None => {
                // Original didn't exist → remove our written file.
                let _ = std::fs::remove_file(path);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_writer_with_home(dir: &TempDir) -> ClaudeCodeWriter {
        // Override home by setting HOME/USERPROFILE env for detection in test.
        let _ = dir;
        ClaudeCodeWriter
    }

    #[test]
    fn test_claude_merge_preserves_existing_fields() {
        let dir = TempDir::new().unwrap();
        let settings = dir.path().join("settings.json");
        std::fs::write(
            &settings,
            r#"{"permissions": {"allow": ["Bash(claude)"]}, "env": {"EXISTING": "keep"}}"#,
        )
        .unwrap();

        let writer = make_writer_with_home(&dir);
        let original = vec![(settings.clone(), Some(r#"{"permissions": {"allow": ["Bash(claude)"]}, "env": {"EXISTING": "keep"}}"#.to_string()))];

        writer
            .merge_and_write_config(
                &original,
                "http://127.0.0.1:47339",
                "sk-gw-test",
                &[("gpt-4".to_string(), "GPT-4".to_string())],
                "gpt-4",
                "GPT-4",
                "LLM-API-Proxy",
            )
            .unwrap();

        let written: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        // Preserved
        assert_eq!(written["permissions"]["allow"][0], "Bash(claude)");
        assert_eq!(written["env"]["EXISTING"], "keep");
        // Injected
        assert_eq!(written["env"]["ANTHROPIC_BASE_URL"], "http://127.0.0.1:47339");
        assert_eq!(written["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-gw-test");
        assert_eq!(written["env"]["ANTHROPIC_MODEL"], "gpt-4");
    }

    #[test]
    fn test_claude_creates_new_file_when_absent() {
        let dir = TempDir::new().unwrap();
        let settings = dir.path().join("settings.json");

        let writer = make_writer_with_home(&dir);
        let original = vec![(settings.clone(), None)];

        writer
            .merge_and_write_config(
                &original,
                "http://127.0.0.1:47339",
                "sk-key",
                &[],
                "my-pool",
                "My Pool",
                "LLM-API-Proxy",
            )
            .unwrap();

        let written: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(written["env"]["ANTHROPIC_BASE_URL"], "http://127.0.0.1:47339");
    }

    #[test]
    fn test_claude_restore_original() {
        let dir = TempDir::new().unwrap();
        let settings = dir.path().join("settings.json");
        std::fs::write(&settings, "{\"env\":{\"ANTHROPIC_BASE_URL\":\"http://proxy\"}}").unwrap();

        let writer = make_writer_with_home(&dir);
        let original = vec![(settings.clone(), Some(r#"{"original": true}"#.to_string()))];
        writer.restore_original_config(&original).unwrap();
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), r#"{"original": true}"#);
    }

    #[test]
    fn test_claude_restore_removes_when_absent_original() {
        let dir = TempDir::new().unwrap();
        let settings = dir.path().join("settings.json");
        std::fs::write(&settings, "{\"env\":{}}").unwrap();

        let writer = make_writer_with_home(&dir);
        let original = vec![(settings.clone(), None)];
        writer.restore_original_config(&original).unwrap();
        assert!(!settings.exists());
    }

    #[test]
    fn test_app_id_and_display() {
        let w = ClaudeCodeWriter;
        assert_eq!(w.app_id(), "claude");
        assert_eq!(w.display_name(), "Claude Code");
    }
}
