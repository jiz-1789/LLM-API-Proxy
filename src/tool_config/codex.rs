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
use crate::tool_config::ToolPool;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct CodexWriter;

const APP_ID: &str = "codex";
const AUTH_REL: &str = "auth.json";
const CONFIG_REL: &str = "config.toml";

/// Model catalog file written next to `config.toml` and referenced via the
/// top-level `model_catalog_json` field. Codex (>=0.144) drives its `/model`
/// picker from this external catalog, so every pool shows up as a switchable
/// model with its real display name (the `[model_providers.custom.models]`
/// table alone does not populate the picker).
const MODEL_CATALOG_FILENAME: &str = "llm-api-proxy-model-catalog.json";

/// Slug cloned from `~/.codex/models_cache.json` when available, so catalog
/// entries carry the exact field set of the user's Codex version.
const MODEL_CATALOG_TEMPLATE_SLUG: &str = "gpt-5.5";

/// Bundled fallback template. Field set mirrors the catalog shape
/// Codex successfully loads (observed on Codex desktop 26.721:
/// an entry missing `model_messages` / `default_verbosity` / reasoning levels
/// gets filtered from the `/model` picker while `model_catalog_json` is still
/// honored — the picker ends up empty). Native `/responses` style: no
/// freeform `apply_patch` / `web_search` custom tools — our gateway is a
/// plain OpenAI-compatible `/responses` proxy, so Codex edits via
/// `shell_type="shell_command"`. Neutral `base_instructions` (identity only).
const STATIC_CATALOG_TEMPLATE: &str = r#"{
  "slug": "template",
  "display_name": "template",
  "description": "template",
  "base_instructions": "You are Codex, a coding agent. You and the user share the same workspace and collaborate to achieve the user's goals.",
  "default_reasoning_level": "medium",
  "supported_reasoning_levels": [
    { "effort": "low", "description": "Fast responses with lighter reasoning" },
    { "effort": "medium", "description": "Balances speed and reasoning depth for everyday tasks" },
    { "effort": "high", "description": "Greater reasoning depth for complex problems" },
    { "effort": "xhigh", "description": "Extra high reasoning depth for complex problems" }
  ],
  "shell_type": "shell_command",
  "visibility": "list",
  "supported_in_api": true,
  "priority": 0,
  "supports_reasoning_summaries": true,
  "default_reasoning_summary": "none",
  "support_verbosity": true,
  "default_verbosity": "low",
  "model_messages": {
    "instructions_template": "You are Codex, a coding agent. You and the user share the same workspace and collaborate to achieve the user's goals.",
    "instructions_variables": {}
  },
  "truncation_policy": { "mode": "tokens", "limit": 10000 },
  "supports_parallel_tool_calls": true,
  "supports_image_detail_original": true,
  "context_window": 272000,
  "max_context_window": 272000,
  "effective_context_window_percent": 95,
  "experimental_supported_tools": [],
  "input_modalities": ["text", "image"],
  "supports_search_tool": true,
  "auto_compact_token_limit": 128000
}"#;

/// Clone the `gpt-5.5` entry from Codex's `models_cache.json` so generated
/// catalog entries carry the field set of the user's installed Codex version.
/// Returns `None` when the cache or the template entry is missing.
fn catalog_template_from_models_cache(codex_dir: &Path) -> Option<Value> {
    let path = codex_dir.join("models_cache.json");
    if !path.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let catalog: Value = serde_json::from_str(&text).ok()?;
    catalog
        .get("models")?
        .as_array()?
        .iter()
        .find(|model| {
            model.get("slug").and_then(|slug| slug.as_str()) == Some(MODEL_CATALOG_TEMPLATE_SLUG)
        })
        .cloned()
}

/// Backfill any field the cloned cache entry is missing from the bundled
/// static template (existing values always win), so entries cloned from an
/// older `models_cache.json` still carry the full field set newer Codex
/// builds require (`model_messages`, `default_verbosity`, reasoning levels,
/// parser-required `base_instructions` / `supports_reasoning_summaries`).
fn fill_required_template_fields(template: &mut Value) {
    let Ok(static_template) = serde_json::from_str::<Value>(STATIC_CATALOG_TEMPLATE) else {
        return;
    };
    let (Some(template_obj), Some(static_obj)) =
        (template.as_object_mut(), static_template.as_object())
    else {
        return;
    };
    for (key, value) in static_obj {
        if !template_obj.contains_key(key) {
            template_obj.insert(key.clone(), value.clone());
        }
    }
}

/// Strip any key that would make Codex emit a freeform/custom tool
/// (`apply_patch`, `web_search`) our plain `/responses` gateway cannot serve;
/// edits flow through `shell_type="shell_command"` instead. `model_messages`
/// is kept — newer Codex builds filter entries that lack it.
fn sanitize_catalog_entry(entry: &mut Value) {
    let Some(obj) = entry.as_object_mut() else {
        return;
    };
    for key in ["apply_patch_tool_type", "web_search_tool_type", "tools"] {
        obj.remove(key);
    }
    obj.insert("shell_type".to_string(), json!("shell_command"));
}

/// Load the catalog template: prefer the user's own `models_cache.json`
/// gpt-5.5 entry (field set matches their Codex version), fall back to the
/// bundled static template.
fn load_catalog_template(codex_dir: &Path) -> Value {
    if let Some(mut template) = catalog_template_from_models_cache(codex_dir) {
        fill_required_template_fields(&mut template);
        sanitize_catalog_entry(&mut template);
        return template;
    }
    let mut template: Value =
        serde_json::from_str(STATIC_CATALOG_TEMPLATE).unwrap_or_else(|_| json!({}));
    sanitize_catalog_entry(&mut template);
    template
}

/// Build the external model catalog: one entry per pool, cloned from the
/// template with `slug` = pool name and `display_name` = pool display name.
fn build_model_catalog(
    codex_dir: &Path,
    all_pools: &[ToolPool],
    default_pool_name: &str,
    default_pool_display_name: &str,
) -> Value {
    let template = load_catalog_template(codex_dir);
    let pools: Vec<ToolPool> = if all_pools.is_empty() {
        vec![ToolPool::new(default_pool_name, default_pool_display_name)]
    } else {
        all_pools.to_vec()
    };
    let entries: Vec<Value> = pools
        .iter()
        .enumerate()
        .map(|(index, pool)| {
            let mut entry = template.clone();
            let display_name = if pool.display_name.is_empty() {
                pool.name.clone()
            } else {
                pool.display_name.clone()
            };
            if let Some(obj) = entry.as_object_mut() {
                obj.insert("slug".to_string(), json!(pool.name));
                obj.insert("display_name".to_string(), json!(display_name));
                obj.insert("description".to_string(), json!(display_name));
                obj.insert("priority".to_string(), json!(1000 + index));
                obj.insert("additional_speed_tiers".to_string(), json!([]));
                obj.insert("service_tiers".to_string(), json!([]));
                obj.insert("availability_nux".to_string(), Value::Null);
                obj.insert("upgrade".to_string(), Value::Null);
                // Real context window from pool capabilities (fallback), so
                // Codex compacts at the pool's true window instead of the
                // static template's gpt-5.5 272K placeholder.
                if let Some(window) = pool.context_window {
                    obj.insert("context_window".to_string(), json!(window));
                    obj.insert("max_context_window".to_string(), json!(window));
                }
            }
            entry
        })
        .collect();
    json!({ "models": entries })
}

/// Resolve the catalog path: always next to `config.toml`, so tests using
/// temp paths exercise the exact same write/cleanup logic as production.
fn catalog_path_for(config_path: &Path) -> Option<PathBuf> {
    config_path
        .parent()
        .map(|dir| dir.join(MODEL_CATALOG_FILENAME))
}

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
        all_pools: &[ToolPool],
        default_pool_name: &str,
        default_pool_display_name: &str,
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
        // Newer Codex (desktop app) authenticates via this bearer token instead
        // of auth.json; write both so both old and new Codex work.
        provider["experimental_bearer_token"] = toml_edit::value(proxy_api_key);

        // Register every pool as a switchable model under this provider so
        // Codex can switch between all proxy pools (not just the default one).
        if !provider.contains_key("models") {
            provider.insert("models", toml_edit::table());
        }
        let models = provider
            .get_mut("models")
            .and_then(toml_edit::Item::as_table_mut)
            .ok_or_else(|| AppError::Config("model_providers.custom.models 必须是 TOML 表".to_string()))?;
        for pool in all_pools {
            let mut desc = toml_edit::Table::new();
            desc.insert("name", toml_edit::value(pool.display_name.clone()));
            models.insert(pool.name.as_str(), toml_edit::Item::Table(desc));
        }
        if all_pools.is_empty() {
            let mut desc = toml_edit::Table::new();
            desc.insert("name", toml_edit::value(default_pool_display_name.to_string()));
            models.insert(default_pool_name, toml_edit::Item::Table(desc));
        }

        // External model catalog: Codex's `/model` picker is driven by
        // `model_catalog_json`, so generate one catalog entry per pool (real
        // pool name as slug, real display name in the picker) and point Codex
        // at it. Written next to config.toml so restore can clean it up.
        let codex_dir = config_path
            .parent()
            .ok_or_else(|| AppError::Config("config.toml 缺少父目录".to_string()))?;
        let catalog = build_model_catalog(
            codex_dir,
            all_pools,
            default_pool_name,
            default_pool_display_name,
        );
        let catalog_content = serde_json::to_string_pretty(&catalog).unwrap_or_default();
        let catalog_path = catalog_path_for(config_path)
            .ok_or_else(|| AppError::Config("无法定位 config.toml 目录".to_string()))?;
        atomic_write(&catalog_path, catalog_content.as_bytes())?;
        doc["model_catalog_json"] = toml_edit::value(MODEL_CATALOG_FILENAME);

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
        // Remove the catalog file we generated (only our own, named file —
        // third-party catalogs stay untouched).
        for (path, _) in original_configs {
            if path.file_name().and_then(|n| n.to_str()) == Some(CONFIG_REL)
                && let Some(catalog_path) = catalog_path_for(path)
                && catalog_path.exists()
            {
                let _ = std::fs::remove_file(&catalog_path);
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
        // Bearer token written for newer Codex desktop authentication
        assert!(toml.contains("experimental_bearer_token = \"sk-gw-test\""));
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
        // (e.g. written by another tool): the proxy must overwrite fields in place,
        // not append a second identical section.
        let dir = TempDir::new().unwrap();
        let auth = dir.path().join("auth.json");
        let config = dir.path().join("config.toml");
        let existing = r#"model = "gpt-5.4"
model_provider = "custom"
model_catalog_json = "third-party-model-catalog.json"

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
        // base_url overwritten in place; model_catalog_json taken over by the
        // proxy's own catalog (Codex `/model` picker must list our pools).
        assert!(toml.contains("base_url = \"http://127.0.0.1:47339\""));
        assert!(toml.contains("model_catalog_json = \"llm-api-proxy-model-catalog.json\""));
        assert!(toml.contains("model = \"grok-4.5\""));
        // Bearer token added for newer Codex authentication
        assert!(toml.contains("experimental_bearer_token = \"sk-gw-proxy\""));
        // It must parse as valid TOML (no duplicate-key error)
        let parsed = toml.parse::<toml_edit::DocumentMut>();
        assert!(parsed.is_ok(), "config.toml no longer valid TOML: {toml}");
    }

    #[test]
    fn test_codex_writes_all_pool_models_for_switching() {
        let dir = TempDir::new().unwrap();
        let auth = dir.path().join("auth.json");
        let config = dir.path().join("config.toml");

        let writer = CodexWriter;
        let original = vec![(auth.clone(), None), (config.clone(), None)];

        writer
            .merge_and_write_config(
                &original,
                "http://127.0.0.1:47339/v1",
                "sk-gw-test",
                &[
                    ToolPool::new("deepseek-v4-pro", "DeepSeek V4 Pro"),
                    ToolPool::new("deepseek-v4-flash", "DeepSeek V4 Flash"),
                ],
                "deepseek-v4-pro",
                "DeepSeek V4 Pro",
                "LLM-API-Proxy",
            )
            .unwrap();

        let toml = std::fs::read_to_string(&config).unwrap();
        let parsed = toml.parse::<toml_edit::DocumentMut>().unwrap();
        // Both pools registered under [model_providers.models] for switching.
        let models = parsed
            .get("model_providers")
            .and_then(|p| p.get("custom"))
            .and_then(|p| p.get("models"))
            .and_then(toml_edit::Item::as_table)
            .expect("model_providers.custom.models must exist");
        assert_eq!(models.len(), 2);
        assert_eq!(
            models["deepseek-v4-pro"]["name"].as_str().unwrap(),
            "DeepSeek V4 Pro"
        );
        assert_eq!(
            models["deepseek-v4-flash"]["name"].as_str().unwrap(),
            "DeepSeek V4 Flash"
        );
        // Default model untouched.
        assert_eq!(parsed["model"].as_str().unwrap(), "deepseek-v4-pro");
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
    fn test_codex_merge_writes_model_catalog_json() {
        let dir = TempDir::new().unwrap();
        let auth = dir.path().join("auth.json");
        let config = dir.path().join("config.toml");

        let writer = CodexWriter;
        let original = vec![(auth.clone(), None), (config.clone(), None)];

        writer
            .merge_and_write_config(
                &original,
                "http://127.0.0.1:47339/v1",
                "sk-gw-test",
                &[
                    ToolPool::new("deepseek-v4-pro", "DeepSeek V4 Pro"),
                    ToolPool::new("deepseek-v4-flash", "DeepSeek V4 Flash"),
                ],
                "deepseek-v4-pro",
                "DeepSeek V4 Pro",
                "LLM-API-Proxy",
            )
            .unwrap();

        // config.toml points at the generated catalog
        let toml = std::fs::read_to_string(&config).unwrap();
        assert!(
            toml.contains("model_catalog_json = \"llm-api-proxy-model-catalog.json\""),
            "missing model_catalog_json field: {toml}"
        );

        // Catalog file exists next to config.toml with one entry per pool
        let catalog_path = dir.path().join("llm-api-proxy-model-catalog.json");
        assert!(catalog_path.exists(), "catalog file not written");
        let catalog: Value =
            serde_json::from_str(&std::fs::read_to_string(&catalog_path).unwrap()).unwrap();
        let models = catalog["models"].as_array().expect("catalog.models array");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["slug"], "deepseek-v4-pro");
        assert_eq!(models[0]["display_name"], "DeepSeek V4 Pro");
        assert_eq!(models[1]["slug"], "deepseek-v4-flash");
        assert_eq!(models[1]["display_name"], "DeepSeek V4 Flash");
        // No freeform custom tool declarations (plain /responses proxy)
        for model in models {
            assert!(
                model.get("apply_patch_tool_type").is_none(),
                "freeform apply_patch must be stripped"
            );
            assert_eq!(model["shell_type"], "shell_command");
            assert!(model.get("base_instructions").is_some(), "required field");
            assert!(
                model.get("supports_reasoning_summaries").is_some(),
                "required field"
            );
            // Full field set newer Codex builds require in the picker
            assert!(model.get("model_messages").is_some(), "model_messages");
            assert!(
                model.get("default_verbosity").is_some(),
                "default_verbosity"
            );
            assert!(
                model["supported_reasoning_levels"].is_array(),
                "supported_reasoning_levels"
            );
        }
    }

    #[test]
    fn test_codex_catalog_uses_models_cache_template_when_present() {
        let dir = TempDir::new().unwrap();
        let auth = dir.path().join("auth.json");
        let config = dir.path().join("config.toml");

        // Simulate Codex's own cache: a gpt-5.5 entry with a version-specific
        // field set (here: a custom marker field the static template lacks).
        let cache = json!({
            "models": [
                {
                    "slug": "gpt-5.5",
                    "display_name": "GPT-5.5",
                    "context_window": 272000,
                    "supports_reasoning_summaries": true,
                    "marker": "user-codex-version"
                }
            ]
        });
        std::fs::write(
            dir.path().join("models_cache.json"),
            serde_json::to_string(&cache).unwrap(),
        )
        .unwrap();

        let writer = CodexWriter;
        let original = vec![(auth.clone(), None), (config.clone(), None)];
        writer
            .merge_and_write_config(
                &original,
                "http://127.0.0.1:47339/v1",
                "sk-gw-test",
                &[ToolPool::new("my-pool", "My Pool")],
                "my-pool",
                "My Pool",
                "LLM-API-Proxy",
            )
            .unwrap();

        let catalog_path = dir.path().join("llm-api-proxy-model-catalog.json");
        let catalog: Value =
            serde_json::from_str(&std::fs::read_to_string(&catalog_path).unwrap()).unwrap();
        let entry = &catalog["models"][0];
        // Cloned from the user's cache (marker survives, context window kept)
        assert_eq!(entry["marker"], "user-codex-version");
        assert_eq!(entry["context_window"], 272000);
        // Required fields backfilled, slug/display name overridden
        assert_eq!(entry["slug"], "my-pool");
        assert_eq!(entry["display_name"], "My Pool");
        assert_eq!(
            entry["base_instructions"].as_str().unwrap(),
            "You are Codex, a coding agent. You and the user share the same workspace and collaborate to achieve the user's goals."
        );
        // Full field set backfilled from the static template
        assert!(entry.get("model_messages").is_some(), "model_messages");
        assert!(entry.get("default_verbosity").is_some(), "default_verbosity");
        assert!(
            entry["supported_reasoning_levels"].is_array(),
            "supported_reasoning_levels"
        );
        // Freeform custom-tool keys stripped
        assert!(entry.get("apply_patch_tool_type").is_none());
        assert_eq!(entry["shell_type"], "shell_command");
    }

    #[test]
    fn test_codex_catalog_uses_real_pool_context_window() {
        let dir = TempDir::new().unwrap();
        let catalog = build_model_catalog(
            dir.path(),
            &[
                ToolPool::new("deepseek-v4-flash", "DeepSeek V4 Flash"),
                ToolPool::with_window("gpt-5.2", "GPT 5.2", 131072),
            ],
            "gpt-5.2",
            "GPT 5.2",
        );
        let models = catalog["models"].as_array().unwrap();
        // Pool with a real context window overrides the template's 272K
        // placeholder; pools without one keep using the template value.
        assert_eq!(models[0]["context_window"], 272000);
        assert_eq!(models[1]["context_window"], 131072);
        assert_eq!(models[1]["max_context_window"], 131072);
    }

    #[test]
    fn test_codex_restore_removes_own_catalog_keeps_third_party() {
        let dir = TempDir::new().unwrap();
        let auth = dir.path().join("auth.json");
        let config = dir.path().join("config.toml");
        std::fs::write(&auth, "{}").unwrap();
        std::fs::write(&config, "model_provider = \"custom\"").unwrap();

        let own_catalog = dir.path().join("llm-api-proxy-model-catalog.json");
        let third_party = dir.path().join("third-party-model-catalog.json");
        std::fs::write(&own_catalog, "{\"models\":[]}").unwrap();
        std::fs::write(&third_party, "{\"models\":[]}").unwrap();

        let writer = CodexWriter;
        let original = vec![
            (auth.clone(), Some("{}".to_string())),
            (
                config.clone(),
                Some("model_provider = \"openai\"".to_string()),
            ),
        ];
        writer.restore_original_config(&original).unwrap();

        assert!(!own_catalog.exists(), "own catalog must be removed");
        assert!(
            third_party.exists(),
            "third-party catalog must be left untouched"
        );
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "model_provider = \"openai\""
        );
    }

    #[test]
    fn test_codex_catalog_empty_pools_falls_back_to_default() {
        let dir = TempDir::new().unwrap();
        let catalog = build_model_catalog(
            dir.path(),
            &[],
            "default-pool",
            "Default Pool",
        );
        let models = catalog["models"].as_array().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["slug"], "default-pool");
        assert_eq!(models[0]["display_name"], "Default Pool");
    }

    #[test]
    fn test_codex_ids() {
        let w = CodexWriter;
        assert_eq!(w.app_id(), "codex");
        assert_eq!(w.display_name(), "Codex CLI");
    }
}
