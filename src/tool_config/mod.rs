//! Tool configuration module: inject the proxy address/API key/model pools
//! into installed AI coding tools' config files.

pub mod backup;
pub mod claude;
pub mod claude_desktop;
pub mod codex;
pub mod detector;
pub mod env_check;
pub mod gateway;
pub mod gemini;
pub mod grok;
pub mod hermes;
pub mod openclaw;
pub mod opencode;
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

/// A pool as seen by tool config writers: real pool name, display name, and
/// the pool's inferred real context window (from `pool.capabilities`).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolPool {
    pub name: String,
    pub display_name: String,
    pub context_window: Option<i32>,
}

impl ToolPool {
    /// A pool without a known context window.
    pub fn new(name: &str, display_name: &str) -> Self {
        Self {
            name: name.to_string(),
            display_name: display_name.to_string(),
            context_window: None,
        }
    }

    /// A pool with its inferred real context window.
    pub fn with_window(name: &str, display_name: &str, context_window: i32) -> Self {
        Self {
            name: name.to_string(),
            display_name: display_name.to_string(),
            context_window: Some(context_window),
        }
    }
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
    #[allow(clippy::too_many_arguments)]
    fn merge_and_write_config(
        &self,
        original_configs: &[(PathBuf, Option<String>)],
        proxy_base_url: &str,
        proxy_api_key: &str,
        all_pools: &[ToolPool],
        default_pool_name: &str,
        default_pool_display_name: &str,
        provider_name: &str,
    ) -> Result<(), AppError>;
    /// Merge proxy config with an explicit role→pool model mapping.
    ///
    /// `model_roles` is a list of `(role, pool_name)` pairs (e.g. Claude Code's
    /// Sonnet/Opus/Fable/Haiku/Subagent slots). Tools without role slots ignore
    /// it via the default implementation.
    #[allow(clippy::too_many_arguments)]
    fn merge_and_write_config_with_roles(
        &self,
        original_configs: &[(PathBuf, Option<String>)],
        proxy_base_url: &str,
        proxy_api_key: &str,
        all_pools: &[ToolPool],
        default_pool_name: &str,
        default_pool_display_name: &str,
        provider_name: &str,
        _model_roles: &[(String, String)],
    ) -> Result<(), AppError> {
        self.merge_and_write_config(
            original_configs,
            proxy_base_url,
            proxy_api_key,
            all_pools,
            default_pool_name,
            default_pool_display_name,
            provider_name,
        )
    }
    /// Merge proxy config with an explicit role→pool mapping AND 1M-context
    /// role flags (Claude Code / Claude Desktop declare 1M per role).
    ///
    /// `roles_1m` lists the roles that must declare a 1M context window. Tools
    /// without 1M support ignore it via the default implementation.
    #[allow(clippy::too_many_arguments)]
    fn merge_and_write_config_with_roles_1m(
        &self,
        original_configs: &[(PathBuf, Option<String>)],
        proxy_base_url: &str,
        proxy_api_key: &str,
        all_pools: &[ToolPool],
        default_pool_name: &str,
        default_pool_display_name: &str,
        provider_name: &str,
        model_roles: &[(String, String)],
        _roles_1m: &[String],
    ) -> Result<(), AppError> {
        self.merge_and_write_config_with_roles(
            original_configs,
            proxy_base_url,
            proxy_api_key,
            all_pools,
            default_pool_name,
            default_pool_display_name,
            provider_name,
            model_roles,
        )
    }
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

/// Canonical display order for the tool tiles (matches registration order).
const TOOL_DISPLAY_ORDER: [&str; 8] = [
    "claude",
    "claude-desktop",
    "codex",
    "gemini",
    "grokbuild",
    "opencode",
    "openclaw",
    "hermes",
];

/// Tool switch manager: orchestrates backup, write, restore, and DB state.
pub struct ToolSwitchManager {
    db: Arc<Database>,
    writers: HashMap<String, Box<dyn ToolConfigWriter>>,
}

impl ToolSwitchManager {
    /// Writers in the canonical display order; unknown app ids go last.
    fn ordered_writers(&self) -> Vec<&dyn ToolConfigWriter> {
        let mut ws: Vec<&Box<dyn ToolConfigWriter>> = self.writers.values().collect();
        ws.sort_by_key(|w| {
            TOOL_DISPLAY_ORDER
                .iter()
                .position(|id| *id == w.app_id())
                .unwrap_or(usize::MAX)
        });
        ws.into_iter().map(|w| w.as_ref()).collect()
    }
    pub fn new(db: Arc<Database>) -> Self {
        let mut writers: HashMap<String, Box<dyn ToolConfigWriter>> = HashMap::new();
        let claude = claude::ClaudeCodeWriter;
        writers.insert(claude.app_id().to_string(), Box::new(claude));
        let claude_desktop = claude_desktop::ClaudeDesktopWriter;
        writers.insert(claude_desktop.app_id().to_string(), Box::new(claude_desktop));
        let codex = codex::CodexWriter;
        writers.insert(codex.app_id().to_string(), Box::new(codex));
        let gemini = gemini::GeminiWriter;
        writers.insert(gemini.app_id().to_string(), Box::new(gemini));
        let grok = grok::GrokWriter;
        writers.insert(grok.app_id().to_string(), Box::new(grok));
        let opencode = opencode::OpenCodeWriter;
        writers.insert(opencode.app_id().to_string(), Box::new(opencode));
        let openclaw = openclaw::OpenClawWriter;
        writers.insert(openclaw.app_id().to_string(), Box::new(openclaw));
        let hermes = hermes::HermesWriter;
        writers.insert(hermes.app_id().to_string(), Box::new(hermes));
        Self { db, writers }
    }

    /// Register a writer (public for extensibility).
    pub fn register(&mut self, writer: Box<dyn ToolConfigWriter>) {
        writers_insert(&mut self.writers, writer);
    }

    /// Detect installation status of all registered tools.
    pub fn detect_all_tools(&self) -> Vec<crate::db::ToolDetectionResult> {
        self.ordered_writers()
            .iter()
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
        for w in self.ordered_writers() {
            let cfg = configs.iter().find(|c| c.tool_app_id == w.app_id());
            let pool_name = cfg
                .as_ref()
                .and_then(|c| c.pool_id.as_deref())
                .and_then(|pid| self.db.get_pool_by_id(pid).ok().flatten())
                .map(|p| p.display_name);
            // Restore the persisted role→pool mapping (if any) from the snapshot.
            let model_roles: Vec<(String, String)> = cfg
                .as_ref()
                .and_then(|c| {
                    serde_json::from_str::<serde_json::Value>(&c.config_snapshot)
                        .ok()
                        .and_then(|v| {
                            v.get("model_roles")
                                .and_then(|r| serde_json::from_value(r.clone()).ok())
                        })
                })
                .unwrap_or_default();
            // Restore the persisted 1M-context role flags (Claude Code / Desktop).
            let model_roles_1m: Vec<String> = cfg
                .as_ref()
                .and_then(|c| {
                    serde_json::from_str::<serde_json::Value>(&c.config_snapshot)
                        .ok()
                        .and_then(|v| {
                            v.get("roles_1m")
                                .and_then(|r| serde_json::from_value(r.clone()).ok())
                        })
                })
                .unwrap_or_default();
            out.push(crate::db::ToolSwitchStatus {
                app_id: w.app_id().to_string(),
                display_name: w.display_name().to_string(),
                installed: w.is_installed(),
                switch_enabled: cfg.map(|c| c.switch_enabled).unwrap_or(false),
                pool_id: cfg.as_ref().and_then(|c| c.pool_id.clone()),
                pool_name,
                api_key_id: cfg.as_ref().and_then(|c| c.api_key_id.clone()),
                provider_name: cfg.map(|c| c.provider_name.clone()).unwrap_or_default(),
                model_roles,
                model_roles_1m,
                last_written_at: cfg.as_ref().and_then(|c| c.last_written_at.clone()),
            });
        }
        Ok(out)
    }

    /// Enable a tool: backup original config, write proxy config, persist state.
    ///
    /// `model_roles` is an optional `(role, pool_name)` mapping for tools with
    /// role slots (e.g. Claude Code); empty for tools without them.
    /// `roles_1m` lists the roles with 1M-context enabled (Claude Code/Desktop).
    pub fn enable_tool(
        &self,
        app_id: &str,
        pool_id: &str,
        api_key_id: Option<&str>,
        provider_name: &str,
        model_roles: &[(String, String)],
        roles_1m: &[String],
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
        let base_url = self.gateway_base_url_for_tool(app_id);

        // Gather all pools for multi-model tools. Each pool carries its
        // inferred real context window (from pool.capabilities) so writers can
        // declare the true window instead of a hardcoded value.
        let all_pools: Vec<ToolPool> = self
            .db
            .get_pools()
            .unwrap_or_default()
            .into_iter()
            .map(|p| {
                let ctx = ModelCapabilities::from_json_str(&p.capabilities)
                    .and_then(|c| c.context_window);
                match ctx {
                    Some(window) => ToolPool::with_window(&p.name, &p.display_name, window),
                    None => ToolPool::new(&p.name, &p.display_name),
                }
            })
            .collect();

        // Backup original config.
        let original = writer.read_original_config()?;

        // Write secondary on-disk backups (crash-recovery insurance).
        for (path, content) in &original {
            if let Err(e) = backup::write_secondary_backup(path, content.as_deref()) {
                tracing::warn!(path = %path.display(), error = %e, "写入二级备份失败");
            }
        }

        writer.merge_and_write_config_with_roles_1m(
            &original,
            &base_url,
            &api_key,
            &all_pools,
            &pool.name,
            &pool.display_name,
            provider_name,
            model_roles,
            roles_1m,
        )?;

        // Claude Desktop: persist the route→pool map so the gateway can
        // resolve the fixed route IDs (claude-sonnet-5, ...) Claude Desktop
        // sends back to the correct pool.
        if app_id == crate::tool_config::claude_desktop::APP_ID {
            self.save_claude_desktop_route_map(model_roles, &pool.name)?;
        }

        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let original_json = serde_json::to_string(&original)
            .unwrap_or_else(|_| "[]".to_string());
        let snapshot = serde_json::json!({
            "base_url": base_url,
            "default_pool": pool.name,
            "provider_name": provider_name,
            "model_roles": model_roles,
            "roles_1m": roles_1m,
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

        // Clean up secondary on-disk backups.
        for (path, _) in &original {
            backup::remove_secondary_backup(path);
        }

        self.db.delete_tool_config(app_id)?;
        // Claude Desktop: clear the persisted route map so the gateway stops
        // advertising/resolving the fixed route IDs once the switch is off.
        if app_id == crate::tool_config::claude_desktop::APP_ID {
            let _ = self
                .db
                .delete_setting(crate::tool_config::claude_desktop::ROUTE_MAP_SETTING_KEY);
        }
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
                } else {
                    // Clean restore → remove secondary backups so the next
                    // startup doesn't treat this as a crash.
                    for (path, _) in &original {
                        backup::remove_secondary_backup(path);
                    }
                }
            }
        }
        Ok(())
    }

    /// Detect abnormal termination from the previous run and recover.
    ///
    /// If a `.llm-proxy-backup` file is found next to any managed config file,
    /// the app was killed before the exit hook could restore the original.
    /// Restore it now, then re-inject all switch=ON tools.
    pub fn recover_from_crash(&self) -> Result<(), AppError> {
        for writer in self.writers.values() {
            for path in writer.config_paths() {
                if backup::restore_from_secondary_backup(&path)? {
                    warn!(
                        tool = %writer.app_id(),
                        path = %path.display(),
                        "检测到异常退出备份，正在恢复原始配置"
                    );
                }
            }
        }
        self.restore_on_startup()
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
        let base_url = self.gateway_base_url_for_tool(&config.tool_app_id);
        let all_pools: Vec<ToolPool> = self
            .db
            .get_pools()
            .unwrap_or_default()
            .into_iter()
            .map(|p| {
                let ctx = ModelCapabilities::from_json_str(&p.capabilities)
                    .and_then(|c| c.context_window);
                match ctx {
                    Some(window) => ToolPool::with_window(&p.name, &p.display_name, window),
                    None => ToolPool::new(&p.name, &p.display_name),
                }
            })
            .collect();
        let original: Vec<(PathBuf, Option<String>)> =
            serde_json::from_str(&config.original_config).unwrap_or_default();
        // Restore the role→pool mapping persisted in the snapshot (if any).
        let model_roles: Vec<(String, String)> = serde_json::from_str(&config.config_snapshot)
            .ok()
            .and_then(|v: serde_json::Value| {
                v.get("model_roles")
                    .and_then(|r| serde_json::from_value(r.clone()).ok())
            })
            .unwrap_or_default();
        // Restore the 1M-context role flags persisted in the snapshot (if any).
        let roles_1m: Vec<String> = serde_json::from_str(&config.config_snapshot)
            .ok()
            .and_then(|v: serde_json::Value| {
                v.get("roles_1m")
                    .and_then(|r| serde_json::from_value(r.clone()).ok())
            })
            .unwrap_or_default();
        writer.merge_and_write_config_with_roles_1m(
            &original,
            &base_url,
            &api_key,
            &all_pools,
            &pool.name,
            &pool.display_name,
            &config.provider_name,
            &model_roles,
            &roles_1m,
        )?;

        // Claude Desktop: re-persist the route→pool map after a rewrite.
        if config.tool_app_id == crate::tool_config::claude_desktop::APP_ID {
            self.save_claude_desktop_route_map(&model_roles, &pool.name)?;
        }
        Ok(())
    }

    /// Update a persisted ON tool's pool/api key and rewrite config.
    ///
    /// `model_roles` replaces the persisted role→pool mapping when provided.
    /// `roles_1m` replaces the persisted 1M-context role flags when provided.
    pub fn update_tool_config(
        &self,
        app_id: &str,
        pool_id: Option<&str>,
        api_key_id: Option<&str>,
        provider_name: Option<&str>,
        model_roles: Option<&[(String, String)]>,
        roles_1m: Option<&[String]>,
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
        if let Ok(mut snapshot) = serde_json::from_str::<serde_json::Value>(&config.config_snapshot)
            && let Some(obj) = snapshot.as_object_mut()
        {
            if let Some(roles) = model_roles {
                // Persist the new role mapping into the snapshot so startup
                // rewrites keep using it.
                obj.insert("model_roles".to_string(), serde_json::json!(roles));
            }
            if let Some(roles) = roles_1m {
                // Persist the new 1M-context role flags.
                obj.insert("roles_1m".to_string(), serde_json::json!(roles));
            }
            config.config_snapshot = snapshot.to_string();
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

    /// Resolve the credential to write into a tool config.
    ///
    /// Explicit API key records take precedence. Otherwise fall back to the
    /// dedicated tool gateway token (generated on first use) instead of the
    /// primary `gateway_api_key`, so the primary key is never embedded in a
    /// tool's config file.
    fn resolve_gateway_api_key(&self, api_key_id: Option<&str>) -> Result<String, AppError> {
        if let Some(kid) = api_key_id
            && let Ok(Some(record)) = self.db.get_api_key_by_id(kid)
            && !record.key.is_empty()
        {
            return Ok(record.key);
        }
        crate::tool_config::gateway::get_or_create_tool_token(&self.db)
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

    /// Tool-appropriate gateway base URL.
    ///
    /// - Anthropic-family tools (Claude Code / Claude Desktop) get the bare
    ///   origin: their SDKs append `/v1/messages` themselves.
    /// - OpenAI-compatible tools (Codex, Grok, OpenCode, OpenClaw, Hermes)
    ///   get `{origin}/v1`: they append `/responses` or `/chat/completions`.
    /// - Gemini CLI gets the bare origin: it appends
    ///   `/v1beta/models/{model}:generateContent` itself.
    fn gateway_base_url_for_tool(&self, app_id: &str) -> String {
        let origin = self.gateway_base_url();
        match app_id {
            "claude" | "claude-desktop" | "gemini" => origin,
            _ => format!("{origin}/v1"),
        }
    }

    /// Persist the Claude Desktop route→pool map as a setting.
    ///
    /// Maps the fixed role route IDs Claude Desktop sends
    /// (`claude-sonnet-5`, `claude-opus-5`, ...) to their mapped pool, falling
    /// back to the default pool for unmapped roles. The gateway reads this to
    /// resolve route-alias requests and to advertise route IDs in `/v1/models`.
    fn save_claude_desktop_route_map(
        &self,
        model_roles: &[(String, String)],
        default_pool_name: &str,
    ) -> Result<(), AppError> {
        use crate::tool_config::claude_desktop;
        let mut map = serde_json::Map::new();
        for (role, route_id) in claude_desktop::ROLE_ROUTE_IDS {
            let pool = model_roles
                .iter()
                .find(|(r, _)| r == role)
                .map(|(_, m)| m.as_str())
                .unwrap_or(default_pool_name);
            map.insert(route_id.to_string(), serde_json::Value::String(pool.to_string()));
        }
        let json = serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string());
        self.db.save_setting(claude_desktop::ROUTE_MAP_SETTING_KEY, &json)?;
        Ok(())
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
            _all_pools: &[ToolPool],
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
        let result = manager.enable_tool("test-tool", "pool_v", None, "LLM-API-Proxy", &[], &[]).unwrap();
        assert!(matches!(result, EnableResult::Ok { .. }));
        let written = std::fs::read_to_string(&target).unwrap();
        let v: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(v["model"], "vision-pool");
        // No explicit API key record → the dedicated tool gateway token is
        // written instead of the primary gateway_api_key.
        assert_eq!(
            v["api_key"],
            crate::tool_config::gateway::get_or_create_tool_token(&db).unwrap()
        );

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
                _: &[ToolPool],
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

        let result = manager.enable_tool("not-installed", "pool_x", None, "P", &[], &[]).unwrap();
        assert!(matches!(result, EnableResult::NotInstalled { .. }));
    }

    #[test]
    fn test_manager_registers_all_8_tools() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.initialize().unwrap();
        let manager = ToolSwitchManager::new(db.clone());
        let detections = manager.detect_all_tools();
        let ids: Vec<String> = detections.iter().map(|d| d.app_id.clone()).collect();
        let expected = [
            "claude",
            "claude-desktop",
            "codex",
            "gemini",
            "grokbuild",
            "opencode",
            "openclaw",
            "hermes",
        ];
        for id in expected {
            assert!(
                ids.iter().any(|i| i == id),
                "tool {} not registered; got {:?}",
                id,
                ids
            );
        }
        assert_eq!(detections.len(), 8);
    }

    #[test]
    fn test_manager_detect_order_is_stable() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.initialize().unwrap();
        let manager = ToolSwitchManager::new(db.clone());

        let order = TOOL_DISPLAY_ORDER.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mut runs = Vec::new();
        for _ in 0..5 {
            let detections = manager.detect_all_tools();
            let ids: Vec<String> = detections.iter().map(|d| d.app_id.clone()).collect();
            runs.push(ids);
        }
        for run in &runs {
            assert_eq!(run, &order, "tool tile order must be stable across runs");
        }
    }

    #[test]
    fn test_gateway_base_url_for_tool_family_split() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.initialize().unwrap();
        db.save_setting("listen_address", "127.0.0.1").unwrap();
        db.save_setting("listen_port", "47339").unwrap();
        let manager = ToolSwitchManager::new(db.clone());

        // Anthropic-family tools and Gemini use the bare origin.
        assert_eq!(
            manager.gateway_base_url_for_tool("claude"),
            "http://127.0.0.1:47339"
        );
        assert_eq!(
            manager.gateway_base_url_for_tool("claude-desktop"),
            "http://127.0.0.1:47339"
        );
        assert_eq!(
            manager.gateway_base_url_for_tool("gemini"),
            "http://127.0.0.1:47339"
        );
        // OpenAI-compatible tools get the /v1 suffix.
        for id in ["codex", "grokbuild", "opencode", "openclaw", "hermes"] {
            assert_eq!(
                manager.gateway_base_url_for_tool(id),
                "http://127.0.0.1:47339/v1",
                "tool {id} should get the /v1 base URL"
            );
        }
    }

    #[test]
    fn test_secondary_backup_crash_recovery_restores_original() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("tool-config.json");
        // User's original config exists.
        std::fs::write(&target, r#"{"user":true}"#).unwrap();

        // Enable writes secondary backup + injected config.
        let (db, manager, target) = setup_manager_with_temp_tool(&dir);
        manager.enable_tool("test-tool", "pool_v", None, "LLM-API-Proxy", &[], &[]).unwrap();
        let backup_path = backup::secondary_backup_path(&target);
        assert!(backup_path.exists());
        assert_eq!(std::fs::read_to_string(&backup_path).unwrap(), r#"{"user":true}"#);

        // Simulate abnormal exit: backup file still present, config injected.
        // A fresh manager (as on startup) recovers the original.
        let mut manager2 = ToolSwitchManager::new(db.clone());
        manager2.register(Box::new(TempWriter::new(&dir)));
        manager2.recover_from_crash().unwrap();
        // Original restored then re-injected because switch is ON.
        let written = std::fs::read_to_string(&target).unwrap();
        let v: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(v["model"], "vision-pool");
        // Secondary backup consumed.
        assert!(!backup_path.exists());
    }

    #[test]
    fn test_secondary_backup_absent_marker_deletes_injected_file() {
        let dir = tempfile::tempdir().unwrap();
        let (db, manager, target) = setup_manager_with_temp_tool(&dir);
        // Original config did NOT exist.
        manager.enable_tool("test-tool", "pool_v", None, "LLM-API-Proxy", &[], &[]).unwrap();
        assert!(target.exists());
        let backup_path = backup::secondary_backup_path(&target);
        assert_eq!(
            std::fs::read_to_string(&backup_path).unwrap(),
            backup::ABSENT_MARKER
        );

        // Crash recovery with switch OFF should leave the file deleted.
        // Set switch OFF first by deleting the record (simulate disabled tool).
        db.delete_tool_config("test-tool").unwrap();
        let mut manager2 = ToolSwitchManager::new(db.clone());
        manager2.register(Box::new(TempWriter::new(&dir)));
        manager2.recover_from_crash().unwrap();
        // No record → restore_on_startup does nothing → file removed by recovery.
        assert!(!target.exists());
        assert!(!backup_path.exists());
    }

    #[test]
    fn test_restore_on_exit_cleans_secondary_backup() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tool-config.json"), r#"{"user":true}"#).unwrap();
        let (_db, manager, target) = setup_manager_with_temp_tool(&dir);
        manager.enable_tool("test-tool", "pool_v", None, "LLM-API-Proxy", &[], &[]).unwrap();
        let backup_path = backup::secondary_backup_path(&target);
        assert!(backup_path.exists());

        // Normal exit restores original AND removes the secondary backup.
        manager.restore_on_exit().unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), r#"{"user":true}"#);
        assert!(!backup_path.exists());
    }

    // A writer that records the role→pool mapping it received, so tests can
    // assert snapshot persistence without parsing config files.
    struct RoleCapturingWriter {
        target: PathBuf,
        last_roles: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    }
    impl ToolConfigWriter for RoleCapturingWriter {
        fn app_id(&self) -> &'static str {
            "role-tool"
        }
        fn display_name(&self) -> &'static str {
            "Role Tool"
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
            _proxy_base_url: &str,
            _proxy_api_key: &str,
            _all_pools: &[ToolPool],
            default_pool_name: &str,
            _default_display: &str,
            _provider: &str,
        ) -> Result<(), AppError> {
            crate::tool_config::writer::atomic_write(
                &self.target,
                format!(r#"{{"model":"{default_pool_name}"}}"#).as_bytes(),
            )
        }
        fn merge_and_write_config_with_roles(
            &self,
            original: &[BackupEntry],
            proxy_base_url: &str,
            proxy_api_key: &str,
            all_pools: &[ToolPool],
            default_pool_name: &str,
            default_display: &str,
            provider: &str,
            model_roles: &[(String, String)],
        ) -> Result<(), AppError> {
            *self.last_roles.lock().unwrap() = model_roles.to_vec();
            self.merge_and_write_config(
                original,
                proxy_base_url,
                proxy_api_key,
                all_pools,
                default_pool_name,
                default_display,
                provider,
            )
        }
        fn restore_original_config(&self, _original: &[BackupEntry]) -> Result<(), AppError> {
            Ok(())
        }
    }

    #[test]
    fn test_update_tool_config_persists_model_roles() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.initialize().unwrap();
        let crypto = crate::crypto::KeyManager::initialize(&std::env::temp_dir()).unwrap();
        let enc = crypto.encrypt_api_key("sk-test").unwrap();
        db.create_upstream("up_a", "OpenAI", "https://a.com", &enc, "gpt-4o", "[]", true, "", "", "openai_chat")
            .unwrap();
        db.create_pool("pool_v", "vision-pool", "Vision Pool", 5, false, "off", "", "")
            .unwrap();
        db.add_upstream_to_pool("pool_v", "up_a", 0, "gpt-4o").unwrap();
        db.save_setting("gateway_api_key", "sk-gw-test-key").unwrap();
        db.save_setting("listen_port", "47339").unwrap();

        let roles = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
        let mut manager = ToolSwitchManager::new(db.clone());
        let writer = RoleCapturingWriter {
            target: dir.path().join("role-config.json"),
            last_roles: roles.clone(),
        };
        manager.register(Box::new(writer));

        // Enable with a role mapping.
        manager
            .enable_tool(
                "role-tool",
                "pool_v",
                None,
                "LLM-API-Proxy",
                &[("sonnet".to_string(), "vision-pool".to_string())],
                &["sonnet".to_string()],
            )
            .unwrap();
        assert_eq!(
            *roles.lock().unwrap(),
            vec![("sonnet".to_string(), "vision-pool".to_string())]
        );

        // Simulate a fresh manager (startup rewrite path): roles must be
        // restored from the snapshot, not lost.
        roles.lock().unwrap().clear();
        let mut manager2 = ToolSwitchManager::new(db.clone());
        manager2.register(Box::new(RoleCapturingWriter {
            target: dir.path().join("role-config.json"),
            last_roles: roles.clone(),
        }));
        manager2.restore_on_startup().unwrap();
        assert_eq!(
            *roles.lock().unwrap(),
            vec![("sonnet".to_string(), "vision-pool".to_string())]
        );

        // update_tool_config with no new roles keeps the persisted mapping.
        manager2
            .update_tool_config("role-tool", None, None, None, None, None)
            .unwrap();
        assert_eq!(
            *roles.lock().unwrap(),
            vec![("sonnet".to_string(), "vision-pool".to_string())]
        );

        // update_tool_config with a new mapping replaces it.
        roles.lock().unwrap().clear();
        manager2
            .update_tool_config(
                "role-tool",
                None,
                None,
                None,
                Some(&[("opus".to_string(), "vision-pool".to_string())]),
                None,
            )
            .unwrap();
        assert_eq!(
            *roles.lock().unwrap(),
            vec![("opus".to_string(), "vision-pool".to_string())]
        );
    }
}
