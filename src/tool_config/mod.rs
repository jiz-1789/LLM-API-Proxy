//! Tool configuration module: inject the proxy address/API key/model pools
//! into installed AI coding tools' config files.

pub mod backup;
pub mod claude;
pub mod codex;
pub mod detector;
pub mod writer;

pub use backup::BackupEntry;

use crate::db::{Database, ModelCapabilities};
use crate::error::AppError;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

/// Capability-based model routing requirements (used for automatic pool selection).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ModelRequirements {
    pub vision: bool,
    pub function_calling: bool,
    pub audio: bool,
    /// Prefer models with the largest context window (e.g. code agents).
    pub prefer_large_context: bool,
}

/// A tool configuration writer.
pub trait ToolConfigWriter: Send + Sync {
    /// Tool identifier (claude, codex, gemini, ...).
    fn app_id(&self) -> &'static str;
    /// Tool display name.
    fn display_name(&self) -> &'static str;
    /// Download URL for installation guidance.
    fn download_url(&self) -> &'static str;
    /// Detect whether the tool is installed.
    fn is_installed(&self) -> bool;
    /// Config file paths (may be multiple).
    fn config_paths(&self) -> Vec<PathBuf>;
    /// Read existing config file contents for backup.
    fn read_original_config(&self) -> Result<Vec<(PathBuf, Option<String>)>, AppError>;
    /// Merge proxy config into the config files (deep merge, preserve other fields).
    fn merge_and_write_config(
        &self,
        original_configs: &[(PathBuf, Option<String>)],
        proxy_base_url: &str,
        proxy_api_key: &str,
        all_pools: &[(String, String)],
        default_pool_name: &str,
        default_pool_display_name: &str,
        provider_name: &str,
    ) -> Result<(), AppError>;
    /// Restore original config files from backup.
    fn restore_original_config(
        &self,
        original_configs: &[(PathBuf, Option<String>)],
    ) -> Result<(), AppError>;
}

/// Result of enabling a tool.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum EnableResult {
    Ok {
        app_id: String,
        written_at: String,
    },
    NotInstalled {
        app_id: String,
        download_url: String,
    },
}

/// Tool switch manager: orchestrates backup, write, restore, and DB state.
pub struct ToolSwitchManager {
    db: Arc<Database>,
    writers: HashMap<String, Box<dyn ToolConfigWriter>>,
}

impl ToolSwitchManager {
    pub fn new(db: Arc<Database>) -> Self {
        let mut writers: HashMap<String, Box<dyn ToolConfigWriter>> = HashMap::new();
        let claude = claude::ClaudeCodeWriter;
        writers.insert(claude.app_id().to_string(), Box::new(claude));
        let codex = codex::CodexWriter;
        writers.insert(codex.app_id().to_string(), Box::new(codex));
        Self { db, writers }
    }

    /// Register a writer (public for extensibility).
    pub fn register(&mut self, writer: Box<dyn ToolConfigWriter>) {
        writers_insert(&mut self.writers, writer);
    }

    /// Detect installation status of all registered tools.
    pub fn detect_all_tools(&self) -> Vec<crate::db::ToolDetectionResult> {
        self.writers
            .values()
            .map(|w| crate::db::ToolDetectionResult {
                app_id: w.app_id().to_string(),
                display_name: w.display_name().to_string(),
                installed: w.is_installed(),
                config_paths: w.config_paths().iter().map(|p| p.display().to_string()).collect(),
                download_url: w.download_url().to_string(),
            })
            .collect()
    }

    /// Get switch status for all tools, joined with pool name.
    pub fn get_all_switch_status(&self) -> Result<Vec<crate::db::ToolSwitchStatus>, AppError> {
        let configs = self.db.get_all_tool_configs()?;
        let mut out = Vec::new();
        for w in self.writers.values() {
            let cfg = configs.iter().find(|c| c.tool_app_id == w.app_id());
            let pool_name = cfg
                .as_ref()
                .and_then(|c| c.pool_id.as_deref())
                .and_then(|pid| self.db.get_pool_by_id(pid).ok().flatten())
                .map(|p| p.display_name);
            out.push(crate::db::ToolSwitchStatus {
                app_id: w.app_id().to_string(),
                display_name: w.display_name().to_string(),
                installed: w.is_installed(),
                switch_enabled: cfg.map(|c| c.switch_enabled).unwrap_or(false),
                pool_id: cfg.as_ref().and_then(|c| c.pool_id.clone()),
                pool_name,
                api_key_id: cfg.as_ref().and_then(|c| c.api_key_id.clone()),
                provider_name: cfg.map(|c| c.provider_name.clone()).unwrap_or_default(),
                last_written_at: cfg.as_ref().and_then(|c| c.last_written_at.clone()),
            });
        }
        Ok(out)
    }

    /// Enable a tool: backup original config, write proxy config, persist state.
    pub fn enable_tool(
        &self,
        app_id: &str,
        pool_id: &str,
        api_key_id: Option<&str>,
        provider_name: &str,
    ) -> Result<EnableResult, AppError> {
        let writer = self
            .writers
            .get(app_id)
            .ok_or_else(|| AppError::Config(format!("未知工具: {}", app_id)))?;

        if !writer.is_installed() {
            return Ok(EnableResult::NotInstalled {
                app_id: app_id.to_string(),
                download_url: writer.download_url().to_string(),
            });
        }

        let pool = self
            .db
            .get_pool_by_id(pool_id)?
            .ok_or_else(|| AppError::Config(format!("模型池不存在: {}", pool_id)))?;

        // Resolve the gateway API key to write into config.
        let api_key = self.resolve_gateway_api_key(api_key_id)?;
        let base_url = self.gateway_base_url();

        // Gather all pools for multi-model tools.
        let all_pools: Vec<(String, String)> = self
            .db
            .get_pools()
            .unwrap_or_default()
            .into_iter()
            .map(|p| (p.name, p.display_name))
            .collect();

        // Backup original config.
        let original = writer.read_original_config()?;

        writer.merge_and_write_config(
            &original,
            &base_url,
            &api_key,
            &all_pools,
            &pool.name,
            &pool.display_name,
            provider_name,
        )?;

        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let original_json = serde_json::to_string(&original)
            .unwrap_or_else(|_| "[]".to_string());
        let snapshot = serde_json::json!({
            "base_url": base_url,
            "default_pool": pool.name,
            "provider_name": provider_name,
        })
        .to_string();

        self.db.save_tool_config(&crate::db::ToolConfigRecord {
            id: format!("tool_{}", uuid::Uuid::new_v4().simple()),
            tool_app_id: app_id.to_string(),
            pool_id: Some(pool_id.to_string()),
            api_key_id: api_key_id.map(|s| s.to_string()),
            provider_name: provider_name.to_string(),
            switch_enabled: true,
            original_config: original_json,
            config_snapshot: snapshot,
            last_written_at: Some(now.clone()),
            created_at: now.clone(),
            updated_at: now,
        })?;

        info!(tool = app_id, pool = %pool.name, "Tool config enabled");
        Ok(EnableResult::Ok {
            app_id: app_id.to_string(),
            written_at: chrono::Local::now().to_rfc3339(),
        })
    }

    /// Disable a tool: restore original config, clear switch state.
    pub fn disable_tool(&self, app_id: &str) -> Result<(), AppError> {
        let writer = self
            .writers
            .get(app_id)
            .ok_or_else(|| AppError::Config(format!("未知工具: {}", app_id)))?;

        let config = self
            .db
            .get_tool_config(app_id)?
            .ok_or_else(|| AppError::Config(format!("工具 {} 未开启", app_id)))?;

        // Restore original config from backup.
        let original: Vec<(PathBuf, Option<String>)> =
            serde_json::from_str(&config.original_config).unwrap_or_default();
        writer.restore_original_config(&original)?;

        self.db.delete_tool_config(app_id)?;
        info!(tool = app_id, "Tool config disabled, original restored");
        Ok(())
    }

    /// Restore all switch=ON tools at startup (rewrite to ensure freshness).
    pub fn restore_on_startup(&self) -> Result<(), AppError> {
        let configs = self.db.get_tool_configs_by_switch(true)?;
        for config in &configs {
            if let Some(writer) = self.writers.get(&config.tool_app_id) {
                if writer.is_installed() {
                    self.rewrite_config(writer.as_ref(), config)?;
                } else {
                    warn!(tool = %config.tool_app_id, "Tool not installed but switch ON; skipping");
                }
            }
        }
        Ok(())
    }

    /// Restore all tool configs at exit (restore originals).
    pub fn restore_on_exit(&self) -> Result<(), AppError> {
        let configs = self.db.get_tool_configs_by_switch(true)?;
        for config in &configs {
            if let Some(writer) = self.writers.get(&config.tool_app_id) {
                let original: Vec<(PathBuf, Option<String>)> =
                    serde_json::from_str(&config.original_config).unwrap_or_default();
                if let Err(e) = writer.restore_original_config(&original) {
                    warn!(tool = %config.tool_app_id, error = %e, "退出时恢复工具配置失败");
                }
            }
        }
        Ok(())
    }

    /// Rewrite config for a persisted ON record (startup / manual rewrite).
    fn rewrite_config(
        &self,
        writer: &dyn ToolConfigWriter,
        config: &crate::db::ToolConfigRecord,
    ) -> Result<(), AppError> {
        let pool = config
            .pool_id
            .as_deref()
            .and_then(|pid| self.db.get_pool_by_id(pid).ok().flatten())
            .ok_or_else(|| AppError::Config("关联模型池不存在".to_string()))?;
        let api_key = self.resolve_gateway_api_key(config.api_key_id.as_deref())?;
        let base_url = self.gateway_base_url();
        let all_pools: Vec<(String, String)> = self
            .db
            .get_pools()
            .unwrap_or_default()
            .into_iter()
            .map(|p| (p.name, p.display_name))
            .collect();
        let original: Vec<(PathBuf, Option<String>)> =
            serde_json::from_str(&config.original_config).unwrap_or_default();
        writer.merge_and_write_config(
            &original,
            &base_url,
            &api_key,
            &all_pools,
            &pool.name,
            &pool.display_name,
            &config.provider_name,
        )
    }

    /// Update a persisted ON tool's pool/api key and rewrite config.
    pub fn update_tool_config(
        &self,
        app_id: &str,
        pool_id: Option<&str>,
        api_key_id: Option<&str>,
        provider_name: Option<&str>,
    ) -> Result<(), AppError> {
        let mut config = self
            .db
            .get_tool_config(app_id)?
            .ok_or_else(|| AppError::Config(format!("工具 {} 未开启", app_id)))?;
        if let Some(pid) = pool_id {
            config.pool_id = Some(pid.to_string());
        }
        config.api_key_id = api_key_id.map(|s| s.to_string());
        if let Some(p) = provider_name {
            config.provider_name = p.to_string();
        }
        config.switch_enabled = true;
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        config.last_written_at = Some(now.clone());
        config.updated_at = now;
        self.db.save_tool_config(&config)?;

        if let Some(writer) = self.writers.get(app_id) {
            self.rewrite_config(writer.as_ref(), &config)?;
        }
        Ok(())
    }

    /// Resolve the gateway API key (by API key record ID, else the primary key).
    fn resolve_gateway_api_key(&self, api_key_id: Option<&str>) -> Result<String, AppError> {
        if let Some(kid) = api_key_id
            && let Ok(Some(record)) = self.db.get_api_key_by_id(kid)
            && !record.key.is_empty()
        {
            return Ok(record.key);
        }
        // Fall back to the primary gateway key setting.
        self.db
            .get_setting("gateway_api_key")?
            .ok_or_else(|| AppError::Config("未配置 Gateway API Key".to_string()))
    }

    /// The gateway base URL written into tool configs.
    fn gateway_base_url(&self) -> String {
        let addr = self
            .db
            .get_setting("listen_address")
            .ok()
            .flatten()
            .unwrap_or_else(|| "127.0.0.1".to_string());
        let port = self
            .db
            .get_setting("listen_port")
            .ok()
            .flatten()
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(47339);
        format!("http://{}:{}", addr, port)
    }
}

fn writers_insert(
    map: &mut HashMap<String, Box<dyn ToolConfigWriter>>,
    writer: Box<dyn ToolConfigWriter>,
) {
    map.insert(writer.app_id().to_string(), writer);
}

// ============================================================================
// Capability-based model routing (stage 2 integration)
// ============================================================================

/// Pick the best pool for a tool's model requirements.
///
/// Scores each pool by its aggregated capabilities (parsed from
/// `pools.capabilities`) against the given requirements:
/// - vision requirement: +10 if pool supports image input
/// - audio requirement: +10 if pool supports audio input
/// - function calling requirement: +8 if supported
/// - prefer_large_context: +6 for larger context windows (relative)
///
/// Returns the pool's `(name, display_name)` of the best match.
pub fn select_pool_for_requirements(
    db: &Database,
    requirements: ModelRequirements,
) -> Option<(String, String)> {
    let pools = db.get_pools().ok()?;
    let mut best: Option<(i64, (String, String))> = None;
    for pool in &pools {
        let Some(caps) = ModelCapabilities::from_json_str(&pool.capabilities) else {
            continue;
        };
        let mut score: i64 = 0;
        let input = &caps.input_modalities;
        if requirements.vision && input.iter().any(|m| m == "image") {
            score += 10;
        }
        if requirements.audio && input.iter().any(|m| m == "audio") {
            score += 10;
        }
        if requirements.function_calling && caps.supports_function_calling {
            score += 8;
        }
        if requirements.prefer_large_context
            && let Some(ctx) = caps.context_window
        {
            score += (ctx as i64 / 64_000).min(20);
        }
        // Pools that satisfy all requirements always beat those that don't,
        // unless no pool does — then pick the highest-scoring partial match.
        if let Some((s, _)) = best
            && s >= score
        {
            continue;
        }
        best = Some((score, (pool.name.clone(), pool.display_name.clone())));
    }
    best.map(|(_, k)| k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_pool_prefers_capability_match() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        let crypto = crate::crypto::KeyManager::initialize(&std::env::temp_dir()).unwrap();
        let enc = crypto.encrypt_api_key("sk-test").unwrap();

        db.create_upstream("up_a", "OpenAI", "https://a.com", &enc, "gpt-4o", "[]", true, "", "", "openai_chat")
            .unwrap();
        db.create_upstream("up_b", "DeepSeek", "https://b.com", &enc, "deepseek-chat", "[]", true, "", "", "openai_chat")
            .unwrap();

        db.create_pool("pool_vision", "vision-pool", "Vision Pool", 5, false, "off", "", "")
            .unwrap();
        db.create_pool("pool_text", "text-pool", "Text Pool", 5, false, "off", "", "")
            .unwrap();

        // pool_vision uses gpt-4o (has image + tools); pool_text uses deepseek (text only).
        db.add_upstream_to_pool("pool_vision", "up_a", 0, "gpt-4o").unwrap();
        db.add_upstream_to_pool("pool_text", "up_b", 0, "deepseek-chat").unwrap();

        let sel = select_pool_for_requirements(
            &db,
            ModelRequirements {
                vision: true,
                function_calling: true,
                ..Default::default()
            },
        );
        assert!(sel.is_some());
        assert_eq!(sel.unwrap().0, "vision-pool");
    }

    #[test]
    fn test_select_pool_returns_none_when_no_pools() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        assert!(select_pool_for_requirements(&db, ModelRequirements::default()).is_none());
    }

    // A test writer that writes to a temp file.
    struct TempWriter {
        target: PathBuf,
    }
    impl TempWriter {
        fn new(dir: &tempfile::TempDir) -> Self {
            Self {
                target: dir.path().join("tool-config.json"),
            }
        }
    }
    impl ToolConfigWriter for TempWriter {
        fn app_id(&self) -> &'static str {
            "test-tool"
        }
        fn display_name(&self) -> &'static str {
            "Test Tool"
        }
        fn download_url(&self) -> &'static str {
            "https://example.com"
        }
        fn is_installed(&self) -> bool {
            true
        }
        fn config_paths(&self) -> Vec<PathBuf> {
            vec![self.target.clone()]
        }
        fn read_original_config(&self) -> Result<Vec<BackupEntry>, AppError> {
            let content = std::fs::read_to_string(&self.target).ok();
            Ok(vec![(self.target.clone(), content)])
        }
        fn merge_and_write_config(
            &self,
            _original: &[BackupEntry],
            proxy_base_url: &str,
            proxy_api_key: &str,
            _all_pools: &[(String, String)],
            default_pool_name: &str,
            _default_display: &str,
            _provider: &str,
        ) -> Result<(), AppError> {
            let content = serde_json::json!({
                "base_url": proxy_base_url,
                "api_key": proxy_api_key,
                "model": default_pool_name,
                "proxy_injected": true,
            })
            .to_string();
            crate::tool_config::writer::atomic_write(&self.target, content.as_bytes())
        }
        fn restore_original_config(
            &self,
            original: &[BackupEntry],
        ) -> Result<(), AppError> {
            match original.first() {
                Some((path, Some(content))) => {
                    crate::tool_config::writer::atomic_write(path, content.as_bytes())
                }
                Some((path, None)) => {
                    let _ = std::fs::remove_file(path);
                    Ok(())
                }
                None => Ok(()),
            }
        }
    }

    fn setup_manager_with_temp_tool(
        dir: &tempfile::TempDir,
    ) -> (Arc<Database>, ToolSwitchManager, PathBuf) {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.initialize().unwrap();
        let crypto = crate::crypto::KeyManager::initialize(&std::env::temp_dir()).unwrap();
        let enc = crypto.encrypt_api_key("sk-test").unwrap();
        db.create_upstream("up_a", "OpenAI", "https://a.com", &enc, "gpt-4o", "[]", true, "", "", "openai_chat")
            .unwrap();
        db.create_pool("pool_v", "vision-pool", "Vision Pool", 5, false, "off", "", "")
            .unwrap();
        db.add_upstream_to_pool("pool_v", "up_a", 0, "gpt-4o").unwrap();
        // Set gateway key
        db.save_setting("gateway_api_key", "sk-gw-test-key").unwrap();
        db.save_setting("listen_port", "47339").unwrap();

        let mut manager = ToolSwitchManager::new(db.clone());
        manager.register(Box::new(TempWriter::new(dir)));
        let target = dir.path().join("tool-config.json");
        (db, manager, target)
    }

    #[test]
    fn test_enable_then_disable_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let (db, manager, target) = setup_manager_with_temp_tool(&dir);

        // Enable
        let result = manager.enable_tool("test-tool", "pool_v", None, "LLM-API-Proxy").unwrap();
        assert!(matches!(result, EnableResult::Ok { .. }));
        let written = std::fs::read_to_string(&target).unwrap();
        let v: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(v["model"], "vision-pool");
        assert_eq!(v["api_key"], "sk-gw-test-key");

        // State persisted
        let cfg = db.get_tool_config("test-tool").unwrap().unwrap();
        assert!(cfg.switch_enabled);
        assert_eq!(cfg.pool_id.as_deref(), Some("pool_v"));

        // Disable restores original (which was None → file removed)
        manager.disable_tool("test-tool").unwrap();
        assert!(!target.exists());
        assert!(db.get_tool_config("test-tool").unwrap().is_none());
    }

    #[test]
    fn test_enable_not_installed_returns_not_installed() {
        struct NotInstalledWriter;
        impl ToolConfigWriter for NotInstalledWriter {
            fn app_id(&self) -> &'static str {
                "not-installed"
            }
            fn display_name(&self) -> &'static str {
                "N/A"
            }
            fn download_url(&self) -> &'static str {
                "https://example.com"
            }
            fn is_installed(&self) -> bool {
                false
            }
            fn config_paths(&self) -> Vec<PathBuf> {
                vec![]
            }
            fn read_original_config(&self) -> Result<Vec<BackupEntry>, AppError> {
                Ok(vec![])
            }
            fn merge_and_write_config(
                &self,
                _: &[BackupEntry],
                _: &str,
                _: &str,
                _: &[(String, String)],
                _: &str,
                _: &str,
                _: &str,
            ) -> Result<(), AppError> {
                Ok(())
            }
            fn restore_original_config(&self, _: &[BackupEntry]) -> Result<(), AppError> {
                Ok(())
            }
        }

        let db = Arc::new(Database::open_in_memory().unwrap());
        db.initialize().unwrap();
        db.save_setting("gateway_api_key", "sk-gw").unwrap();
        db.create_pool("pool_x", "x", "X", 5, false, "off", "", "").unwrap();
        let mut manager = ToolSwitchManager::new(db.clone());
        manager.register(Box::new(NotInstalledWriter));

        let result = manager.enable_tool("not-installed", "pool_x", None, "P").unwrap();
        assert!(matches!(result, EnableResult::NotInstalled { .. }));
    }
}
