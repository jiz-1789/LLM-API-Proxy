//! Claude Code config writer (`~/.claude/settings.json`).
//!
//! Writes `env.ANTHROPIC_BASE_URL`, `env.ANTHROPIC_AUTH_TOKEN` and the model
//! env vars into the existing settings.json, preserving all other fields.
//!
//! 1M-context roles (via `roles_1m`) are declared the Claude Code way: the
//! model value carries a `[1M]` suffix (see cc-switch `ONE_M_CONTEXT_MARKER`),
//! and when any role is marked 1M the client-side context window is raised
//! with `CLAUDE_CODE_MAX_CONTEXT_TOKENS` / `CLAUDE_CODE_AUTO_COMPACT_WINDOW`
//! — without those, Claude Code caps non-`claude-` model ids at 200K.

use crate::error::AppError;
use crate::tool_config::detector;
use crate::tool_config::writer::atomic_write;
use crate::tool_config::backup::BackupEntry;
use crate::tool_config::{ToolConfigWriter, ToolPool};
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct ClaudeCodeWriter;

const APP_ID: &str = "claude";
const SETTINGS_REL: &str = ".claude/settings.json";
const ONE_M_SUFFIX: &str = "[1M]";
const ONE_M_CONTEXT_TOKENS: &str = "1000000";

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
        detector::cli_installed("claude")
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
        _all_pools: &[ToolPool],
        default_pool_name: &str,
        _default_pool_display_name: &str,
        _provider_name: &str,
    ) -> Result<(), AppError> {
        self.merge_and_write_config_with_roles(
            original_configs,
            proxy_base_url,
            proxy_api_key,
            _all_pools,
            default_pool_name,
            _default_pool_display_name,
            _provider_name,
            &[],
        )
    }

    fn merge_and_write_config_with_roles(
        &self,
        original_configs: &[BackupEntry],
        proxy_base_url: &str,
        proxy_api_key: &str,
        all_pools: &[ToolPool],
        default_pool_name: &str,
        default_pool_display_name: &str,
        provider_name: &str,
        model_roles: &[(String, String)],
    ) -> Result<(), AppError> {
        self.merge_and_write_config_with_roles_1m(
            original_configs,
            proxy_base_url,
            proxy_api_key,
            all_pools,
            default_pool_name,
            default_pool_display_name,
            provider_name,
            model_roles,
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
        default_pool_display_name: &str,
        _provider_name: &str,
        model_roles: &[(String, String)],
        roles_1m: &[String],
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
            // Claude Code warns when both AUTH_TOKEN and API_KEY are set
            // ("Both ANTHROPIC_AUTH_TOKEN and ANTHROPIC_API_KEY set").
            // We take over auth via AUTH_TOKEN, so drop any stale API_KEY to
            // keep exactly one credential active.
            env_obj.remove("ANTHROPIC_API_KEY");
            env_obj.insert("ANTHROPIC_MODEL".to_string(), Value::String(default_pool_name.to_string()));
            // Role slots: any role without an explicit mapping falls back to
            // the default pool so /model always offers a working entry.
            // Roles marked in `roles_1m` get the Claude Code `[1M]` suffix,
            // which the gateway strips before pool lookup.
            let pool_for_role = |role: &str| -> String {
                model_roles
                    .iter()
                    .find(|(r, _)| r == role)
                    .map(|(_, m)| m.clone())
                    .unwrap_or_else(|| default_pool_name.to_string())
            };
            // Real context window for a pool (used as a safe fallback instead
            // of hard-coding 200K / 1M).
            let window_for_pool = |pool: &str| -> Option<i32> {
                all_pools
                    .iter()
                    .find(|p| p.name == pool)
                    .and_then(|p| p.context_window)
            };
            // /model menu friendly name: the mapped pool's display name (cc-switch
            // mirrors this with the ANTHROPIC_DEFAULT_*_MODEL_NAME env pair).
            let display_for_role = |role: &str| -> String {
                let pool = pool_for_role(role);
                if pool == default_pool_name {
                    default_pool_display_name.to_string()
                } else {
                    all_pools
                        .iter()
                        .find(|p| p.name == pool)
                        .map(|p| p.display_name.clone())
                        .unwrap_or_else(|| pool.clone())
                }
            };
            let role_env = |role: &str| -> String {
                let pool = pool_for_role(role);
                if roles_1m.iter().any(|r| r == role) {
                    format!("{pool}{ONE_M_SUFFIX}")
                } else {
                    pool
                }
            };
            let insert_role = |env_obj: &mut serde_json::Map<String, Value>, role: &str| {
                let key = format!("ANTHROPIC_DEFAULT_{}_MODEL", role.to_ascii_uppercase());
                let name_key = format!("ANTHROPIC_DEFAULT_{}_MODEL_NAME", role.to_ascii_uppercase());
                env_obj.insert(key, Value::String(role_env(role)));
                env_obj.insert(name_key, Value::String(display_for_role(role)));
            };
            insert_role(env_obj, "haiku");
            insert_role(env_obj, "sonnet");
            insert_role(env_obj, "opus");
            insert_role(env_obj, "fable");
            env_obj.insert(
                "ANTHROPIC_DEFAULT_SUBAGENT_MODEL".to_string(),
                Value::String(role_env("subagent")),
            );
            // Any 1M-marked role raises the client-side context window to 1M.
            // Otherwise fall back to the default pool's real context window
            // (inferred from capabilities) instead of assuming Claude Code's
            // 200K default; clear when unknown so the safe 200K default applies.
            let window_tokens: Option<String> = if !roles_1m.is_empty() {
                Some(ONE_M_CONTEXT_TOKENS.to_string())
            } else {
                window_for_pool(default_pool_name).map(|w| w.to_string())
            };
            match window_tokens {
                Some(tokens) => {
                    env_obj.insert(
                        "CLAUDE_CODE_MAX_CONTEXT_TOKENS".to_string(),
                        Value::String(tokens.clone()),
                    );
                    env_obj.insert(
                        "CLAUDE_CODE_AUTO_COMPACT_WINDOW".to_string(),
                        Value::String(tokens),
                    );
                }
                None => {
                    env_obj.remove("CLAUDE_CODE_MAX_CONTEXT_TOKENS");
                    env_obj.remove("CLAUDE_CODE_AUTO_COMPACT_WINDOW");
                }
            }
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
                &[ToolPool::new("gpt-4", "GPT-4")],
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
    fn test_claude_writes_role_model_mapping() {
        let dir = TempDir::new().unwrap();
        let settings = dir.path().join("settings.json");

        let writer = make_writer_with_home(&dir);
        let original = vec![(settings.clone(), None)];

        writer
            .merge_and_write_config_with_roles(
                &original,
                "http://127.0.0.1:47339",
                "sk-gw-test",
                &[
                    ToolPool::new("deepseek-v4-pro", "DeepSeek V4 Pro"),
                    ToolPool::new("deepseek-v4-flash", "DeepSeek V4 Flash"),
                ],
                "deepseek-v4-pro",
                "DeepSeek V4 Pro",
                "LLM-API-Proxy",
                &[
                    ("sonnet".to_string(), "deepseek-v4-pro".to_string()),
                    ("opus".to_string(), "deepseek-v4-pro".to_string()),
                    ("haiku".to_string(), "deepseek-v4-flash".to_string()),
                ],
            )
            .unwrap();

        let written: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        let env = &written["env"];
        assert_eq!(env["ANTHROPIC_MODEL"], "deepseek-v4-pro");
        assert_eq!(env["ANTHROPIC_DEFAULT_SONNET_MODEL"], "deepseek-v4-pro");
        assert_eq!(env["ANTHROPIC_DEFAULT_OPUS_MODEL"], "deepseek-v4-pro");
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "deepseek-v4-flash");
        // Roles without explicit mapping fall back to the default pool.
        assert_eq!(env["ANTHROPIC_DEFAULT_FABLE_MODEL"], "deepseek-v4-pro");
        assert_eq!(env["ANTHROPIC_DEFAULT_SUBAGENT_MODEL"], "deepseek-v4-pro");
        // /model menu display names mirror the mapped pools' display names.
        assert_eq!(env["ANTHROPIC_DEFAULT_SONNET_MODEL_NAME"], "DeepSeek V4 Pro");
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME"], "DeepSeek V4 Flash");
        // Unmapped roles fall back to the default pool's display name.
        assert_eq!(env["ANTHROPIC_DEFAULT_FABLE_MODEL_NAME"], "DeepSeek V4 Pro");
    }

    #[test]
    fn test_claude_1m_roles_append_suffix_and_raise_context_window() {
        let dir = TempDir::new().unwrap();
        let settings = dir.path().join("settings.json");

        let writer = make_writer_with_home(&dir);
        let original = vec![(settings.clone(), None)];

        writer
            .merge_and_write_config_with_roles_1m(
                &original,
                "http://127.0.0.1:47339",
                "sk-gw-test",
                &[ToolPool::new("deepseek-v4-pro", "DeepSeek V4 Pro")],
                "deepseek-v4-pro",
                "DeepSeek V4 Pro",
                "LLM-API-Proxy",
                &[
                    ("sonnet".to_string(), "deepseek-v4-pro".to_string()),
                    ("haiku".to_string(), "deepseek-v4-flash".to_string()),
                ],
                &["sonnet".to_string(), "haiku".to_string()],
            )
            .unwrap();

        let written: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        let env = &written["env"];
        // Marked roles carry the [1M] suffix (gateway strips it on request).
        assert_eq!(env["ANTHROPIC_DEFAULT_SONNET_MODEL"], "deepseek-v4-pro[1M]");
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "deepseek-v4-flash[1M]");
        // Unmarked roles keep the bare pool name.
        assert_eq!(env["ANTHROPIC_DEFAULT_OPUS_MODEL"], "deepseek-v4-pro");
        assert_eq!(env["ANTHROPIC_DEFAULT_FABLE_MODEL"], "deepseek-v4-pro");
        assert_eq!(env["ANTHROPIC_DEFAULT_SUBAGENT_MODEL"], "deepseek-v4-pro");
        assert_eq!(env["ANTHROPIC_MODEL"], "deepseek-v4-pro");
        // Any 1M role raises the client-side context window.
        assert_eq!(env["CLAUDE_CODE_MAX_CONTEXT_TOKENS"], "1000000");
        assert_eq!(env["CLAUDE_CODE_AUTO_COMPACT_WINDOW"], "1000000");
    }

    #[test]
    fn test_claude_no_1m_clears_stale_context_window_env() {
        let dir = TempDir::new().unwrap();
        let settings = dir.path().join("settings.json");
        std::fs::write(
            &settings,
            r#"{"env": {"CLAUDE_CODE_MAX_CONTEXT_TOKENS": "1000000", "CLAUDE_CODE_AUTO_COMPACT_WINDOW": "1000000"}}"#,
        )
        .unwrap();

        let writer = make_writer_with_home(&dir);
        let original = vec![(settings.clone(), Some(std::fs::read_to_string(&settings).unwrap()))];

        writer
            .merge_and_write_config_with_roles_1m(
                &original,
                "http://127.0.0.1:47339",
                "sk-gw-test",
                &[],
                "deepseek-v4-pro",
                "DeepSeek V4 Pro",
                "LLM-API-Proxy",
                &[],
                &[],
            )
            .unwrap();

        let written: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        let env = &written["env"];
        // No role marked 1M → stale context-window env from a previous write
        // is removed, falling back to Claude Code's safe 200K default.
        assert!(env.get("CLAUDE_CODE_MAX_CONTEXT_TOKENS").is_none());
        assert!(env.get("CLAUDE_CODE_AUTO_COMPACT_WINDOW").is_none());
        // Suffix-free role values overwrite stale [1M] ones.
        assert_eq!(env["ANTHROPIC_DEFAULT_SONNET_MODEL"], "deepseek-v4-pro");
    }

    #[test]
    fn test_claude_auth_conflict_removes_stale_api_key() {
        let dir = TempDir::new().unwrap();
        let settings = dir.path().join("settings.json");
        // User previously configured ANTHROPIC_API_KEY; we take over auth via
        // ANTHROPIC_AUTH_TOKEN and must drop the stale key to avoid the
        // "Both AUTH_TOKEN and API_KEY set" warning.
        std::fs::write(
            &settings,
            r#"{"env":{"ANTHROPIC_API_KEY":"sk-user","ANTHROPIC_BASE_URL":"https://user.com"}}"#,
        )
        .unwrap();

        let writer = make_writer_with_home(&dir);
        let original = vec![(settings.clone(), Some(std::fs::read_to_string(&settings).unwrap()))];

        writer
            .merge_and_write_config(
                &original,
                "http://127.0.0.1:47339",
                "sk-gw-test",
                &[ToolPool::new("gpt-4", "GPT-4")],
                "gpt-4",
                "GPT-4",
                "LLM-API-Proxy",
            )
            .unwrap();

        let written: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        let env = &written["env"];
        assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], "sk-gw-test");
        assert!(env.get("ANTHROPIC_API_KEY").is_none(), "stale API_KEY must be removed");
        assert_eq!(env["ANTHROPIC_BASE_URL"], "http://127.0.0.1:47339");
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
