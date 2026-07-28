use crate::crypto::KeyManager;
use crate::db::Database;
use crate::error::AppError;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};

// ============================================================================
// Export / Import Data Structures
// ============================================================================

/// Top-level configuration export structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigExport {
    pub version: String,
    pub exported_at: String,
    pub schema_version: i32,
    pub upstreams: Vec<ExportUpstream>,
    pub pools: Vec<ExportPool>,
    pub settings: HashMap<String, String>,
}

/// An upstream in export format (API key in plaintext for portability).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportUpstream {
    pub id: String,
    pub provider_name: String,
    pub base_url: String,
    pub api_key: String,
    pub selected_model: String,
    pub available_models: Vec<String>,
    pub enabled: bool,
    pub remark: String,
}

/// A pool in export format, with nested upstream associations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportPool {
    pub name: String,
    pub display_name: String,
    pub max_concurrency: i32,
    pub thinking_enabled: bool,
    pub upstreams: Vec<ExportPoolUpstream>,
}

/// An upstream association within a pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportPoolUpstream {
    pub upstream_id: String,
    pub sort_order: i32,
    pub model: String,
}

/// Import mode: incremental (add/update) or full (replace all).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    Incremental,
    Full,
}

/// Request body for the import_config command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportRequest {
    pub config: ConfigExport,
    pub mode: ImportMode,
}

/// Result of a config import operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub upstreams_added: usize,
    pub upstreams_updated: usize,
    pub pools_added: usize,
    pub pools_updated: usize,
    pub settings_imported: usize,
    pub warnings: Vec<String>,
}

// ============================================================================
// Export
// ============================================================================

/// Export all configuration (upstreams, pools, settings) to a portable JSON structure.
/// API keys are decrypted to plaintext for cross-machine portability.
pub fn export_config(db: &Database, crypto: &KeyManager) -> Result<ConfigExport, AppError> {
    let schema_version = db.get_schema_version()?;

    // Export upstreams (decrypt API keys)
    let upstreams = db.get_upstreams()?;
    let mut export_upstreams = Vec::with_capacity(upstreams.len());
    for u in &upstreams {
        let api_key = if u.api_key_encrypted.is_empty() {
            String::new()
        } else {
            crypto
                .decrypt_api_key(&u.api_key_encrypted)
                .unwrap_or_else(|e| {
                    warn!("Failed to decrypt API key for {}: {}", u.provider_name, e);
                    String::new()
                })
        };
        let available_models: Vec<String> =
            serde_json::from_str(&u.available_models).unwrap_or_default();

        export_upstreams.push(ExportUpstream {
            id: u.id.clone(),
            provider_name: u.provider_name.clone(),
            base_url: u.base_url.clone(),
            api_key,
            selected_model: u.selected_model.clone(),
            available_models,
            enabled: u.enabled,
            remark: u.remark.clone(),
        });
    }

    // Export pools with nested upstream associations
    let pools = db.get_pools()?;
    let mut export_pools = Vec::with_capacity(pools.len());
    for p in &pools {
        let pool_upstreams = db.get_pool_upstreams(&p.id)?;
        let mut export_pool_upstreams = Vec::with_capacity(pool_upstreams.len());
        for pu in pool_upstreams {
            export_pool_upstreams.push(ExportPoolUpstream {
                upstream_id: pu.upstream_id.clone(),
                sort_order: pu.sort_order,
                model: pu.model.clone(),
            });
        }
        export_pools.push(ExportPool {
            name: p.name.clone(),
            display_name: p.display_name.clone(),
            max_concurrency: p.max_concurrency,
            thinking_enabled: p.thinking_enabled,
            upstreams: export_pool_upstreams,
        });
    }

    // Export settings (all key-value pairs)
    let settings_list = db.get_all_settings()?;
    let settings: HashMap<String, String> = settings_list.into_iter().collect();

    Ok(ConfigExport {
        version: "1.0".to_string(),
        exported_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        schema_version,
        upstreams: export_upstreams,
        pools: export_pools,
        settings,
    })
}

// ============================================================================
// Import
// ============================================================================

/// Import configuration from a JSON structure.
/// - `Incremental`: add new items, update existing (matched by provider_name+base_url for upstreams, by name for pools)
/// - `Full`: delete all existing items first, then import all from the file
pub fn import_config(
    db: &Database,
    crypto: &KeyManager,
    config: &ConfigExport,
    mode: &ImportMode,
) -> Result<ImportResult, AppError> {
    let mut result = ImportResult {
        upstreams_added: 0,
        upstreams_updated: 0,
        pools_added: 0,
        pools_updated: 0,
        settings_imported: 0,
        warnings: Vec::new(),
    };

    // Validate all upstreams before importing
    for u in &config.upstreams {
        validate_upstream(u)?;
    }
    // Validate all pools before importing
    for p in &config.pools {
        validate_pool(p)?;
    }

    match mode {
        ImportMode::Full => {
            import_full(db, crypto, config, &mut result)?;
        }
        ImportMode::Incremental => {
            import_incremental(db, crypto, config, &mut result)?;
        }
    }

    info!(
        "Config import complete: +{} ~{} upstreams, +{} ~{} pools, {} settings",
        result.upstreams_added,
        result.upstreams_updated,
        result.pools_added,
        result.pools_updated,
        result.settings_imported
    );

    Ok(result)
}

/// Full import: delete all existing data, then create everything from the config.
fn import_full(
    db: &Database,
    crypto: &KeyManager,
    config: &ConfigExport,
    result: &mut ImportResult,
) -> Result<(), AppError> {
    db.with_transaction(|conn| {
        // Delete in FK-safe order: pool_upstreams → pools → upstreams
        conn.execute("DELETE FROM pool_upstreams", [])?;
        conn.execute("DELETE FROM pools", [])?;
        conn.execute("DELETE FROM upstreams", [])?;

        // Import upstreams with new IDs
        let mut id_map: HashMap<String, String> = HashMap::new();
        for u in &config.upstreams {
            let new_id = generate_id("up");
            let encrypted = if u.api_key.is_empty() {
                Vec::new()
            } else {
                crypto.encrypt_api_key(&u.api_key).map_err(|e| {
                    AppError::Crypto(format!("加密 API Key 失败: {}", e))
                })?
            };
            let models_json = serde_json::to_string(&u.available_models)
                .unwrap_or_else(|_| "[]".to_string());

            conn.execute(
                "INSERT INTO upstreams (id, provider_name, base_url, api_key_encrypted,
                 selected_model, available_models, enabled, remark)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    new_id,
                    u.provider_name,
                    u.base_url,
                    encrypted,
                    u.selected_model,
                    models_json,
                    u.enabled as i32,
                    u.remark,
                ],
            )?;
            id_map.insert(u.id.clone(), new_id);
            result.upstreams_added += 1;
        }

        // Import pools with new IDs
        for p in &config.pools {
            let new_pool_id = generate_id("pool");
            conn.execute(
                "INSERT INTO pools (id, name, display_name, max_concurrency, thinking_enabled)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    new_pool_id,
                    p.name,
                    p.display_name,
                    p.max_concurrency,
                    p.thinking_enabled as i32,
                ],
            )?;

            // Create upstream associations
            for pu in &p.upstreams {
                let resolved_id = id_map.get(&pu.upstream_id);
                if let Some(real_id) = resolved_id {
                    conn.execute(
                        "INSERT INTO pool_upstreams (pool_id, upstream_id, sort_order, model)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![new_pool_id, real_id, pu.sort_order, pu.model],
                    )?;
                } else {
                    warn!(
                        "Skipping pool_upstream: upstream_id {} not found in import data",
                        pu.upstream_id
                    );
                }
            }
            result.pools_added += 1;
        }

        // Import settings (overwrite all)
        for (key, value) in &config.settings {
            conn.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value=?2, updated_at=datetime('now', 'localtime')",
                params![key, value],
            )?;
            result.settings_imported += 1;
        }

        Ok(())
    })?;

    Ok(())
}

/// Incremental import: add new items, update existing by natural key.
fn import_incremental(
    db: &Database,
    crypto: &KeyManager,
    config: &ConfigExport,
    result: &mut ImportResult,
) -> Result<(), AppError> {
    db.with_transaction(|conn| {
        let mut id_map: HashMap<String, String> = HashMap::new();

        // Import upstreams — match by provider_name + base_url
        for u in &config.upstreams {
            // Check if upstream exists by provider_name + base_url
            let existing_id: Option<String> = conn
                .query_row(
                    "SELECT id FROM upstreams WHERE provider_name=?1 AND base_url=?2",
                    params![u.provider_name, u.base_url],
                    |row| row.get(0),
                )
                .ok();

            let encrypted = if u.api_key.is_empty() {
                Vec::new()
            } else {
                crypto.encrypt_api_key(&u.api_key).map_err(|e| {
                    AppError::Crypto(format!("加密 API Key 失败: {}", e))
                })?
            };
            let models_json = serde_json::to_string(&u.available_models)
                .unwrap_or_else(|_| "[]".to_string());

            if let Some(existing) = existing_id {
                // Update existing upstream
                conn.execute(
                    "UPDATE upstreams SET provider_name=?1, base_url=?2, api_key_encrypted=?3,
                     selected_model=?4, available_models=?5, enabled=?6, remark=?7,
                     updated_at=datetime('now', 'localtime')
                     WHERE id=?8",
                    params![
                        u.provider_name,
                        u.base_url,
                        encrypted,
                        u.selected_model,
                        models_json,
                        u.enabled as i32,
                        u.remark,
                        existing,
                    ],
                )?;
                id_map.insert(u.id.clone(), existing);
                result.upstreams_updated += 1;
            } else {
                // Create new upstream
                let new_id = generate_id("up");
                conn.execute(
                    "INSERT INTO upstreams (id, provider_name, base_url, api_key_encrypted,
                     selected_model, available_models, enabled, remark)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        &new_id,
                        u.provider_name,
                        u.base_url,
                        encrypted,
                        u.selected_model,
                        models_json,
                        u.enabled as i32,
                        u.remark,
                    ],
                )?;
                id_map.insert(u.id.clone(), new_id);
                result.upstreams_added += 1;
            }
        }

        // Import pools — match by name
        for p in &config.pools {
            let existing_pool_id: Option<String> = conn
                .query_row(
                    "SELECT id FROM pools WHERE name=?1",
                    params![p.name],
                    |row| row.get(0),
                )
                .ok();

            if let Some(existing) = existing_pool_id {
                // Update existing pool
                conn.execute(
                    "UPDATE pools SET display_name=?1, max_concurrency=?2, thinking_enabled=?3,
                     updated_at=datetime('now', 'localtime') WHERE id=?4",
                    params![
                        p.display_name,
                        p.max_concurrency,
                        p.thinking_enabled as i32,
                        existing,
                    ],
                )?;
                // Clear old upstream associations and rebuild
                conn.execute(
                    "DELETE FROM pool_upstreams WHERE pool_id=?1",
                    params![existing],
                )?;
                for pu in &p.upstreams {
                    if let Some(real_id) = id_map.get(&pu.upstream_id) {
                        conn.execute(
                            "INSERT INTO pool_upstreams (pool_id, upstream_id, sort_order, model)
                             VALUES (?1, ?2, ?3, ?4)",
                            params![existing, real_id, pu.sort_order, pu.model],
                        )?;
                    } else {
                        warn!(
                            "Skipping pool_upstream: upstream_id {} not found",
                            pu.upstream_id
                        );
                    }
                }
                result.pools_updated += 1;
            } else {
                // Create new pool
                let new_pool_id = generate_id("pool");
                conn.execute(
                    "INSERT INTO pools (id, name, display_name, max_concurrency, thinking_enabled)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        &new_pool_id,
                        p.name,
                        p.display_name,
                        p.max_concurrency,
                        p.thinking_enabled as i32,
                    ],
                )?;
                for pu in &p.upstreams {
                    if let Some(real_id) = id_map.get(&pu.upstream_id) {
                        conn.execute(
                            "INSERT INTO pool_upstreams (pool_id, upstream_id, sort_order, model)
                             VALUES (?1, ?2, ?3, ?4)",
                            params![&new_pool_id, real_id, pu.sort_order, pu.model],
                        )?;
                    } else {
                        warn!(
                            "Skipping pool_upstream: upstream_id {} not found",
                            pu.upstream_id
                        );
                    }
                }
                result.pools_added += 1;
            }
        }

        // Import settings (overwrite, don't delete existing)
        for (key, value) in &config.settings {
            conn.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value=?2, updated_at=datetime('now', 'localtime')",
                params![key, value],
            )?;
            result.settings_imported += 1;
        }

        Ok(())
    })?;

    Ok(())
}

// ============================================================================
// Validation
// ============================================================================

/// Validate an upstream before importing.
fn validate_upstream(u: &ExportUpstream) -> Result<(), AppError> {
    if u.provider_name.trim().is_empty() {
        return Err(AppError::Config("上游名称不能为空".to_string()));
    }
    if u.base_url.trim().is_empty() {
        return Err(AppError::Config("上游 base_url 不能为空".to_string()));
    }
    if !u.base_url.starts_with("http://") && !u.base_url.starts_with("https://") {
        return Err(AppError::Config(format!(
            "上游 base_url 必须以 http:// 或 https:// 开头: {}",
            u.base_url
        )));
    }
    Ok(())
}

/// Validate a pool before importing.
fn validate_pool(p: &ExportPool) -> Result<(), AppError> {
    if p.name.trim().is_empty() {
        return Err(AppError::Config("池名称不能为空".to_string()));
    }
    if p.display_name.trim().is_empty() {
        return Err(AppError::Config("池显示名称不能为空".to_string()));
    }
    Ok(())
}

/// Generate a unique ID with a given prefix.
///
/// Format: `{prefix}_{timestamp_hex}{nanos_hex}{random_hex}`
/// The 4-byte random suffix prevents collisions when two IDs are generated
/// within the same nanosecond.
fn generate_id(prefix: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mut rand_bytes = [0u8; 4];
    let _ = getrandom::getrandom(&mut rand_bytes);
    let rand_hex = u32::from_le_bytes(rand_bytes);
    format!(
        "{}_{:x}{:08x}{:08x}",
        prefix,
        now.as_secs(),
        now.subsec_nanos(),
        rand_hex
    )
}

// ============================================================================
// Serialization helpers
// ============================================================================

impl ConfigExport {
    /// Serialize to a pretty-printed JSON string.
    pub fn to_json(&self) -> Result<String, AppError> {
        serde_json::to_string_pretty(self)
            .map_err(|e| AppError::Internal(format!("JSON 序列化失败: {}", e)))
    }
}

impl ImportRequest {
    /// Parse from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, AppError> {
        serde_json::from_str(json)
            .map_err(|e| AppError::Internal(format!("JSON 解析失败: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_export() -> ConfigExport {
        ConfigExport {
            version: "1.0".to_string(),
            exported_at: "2026-07-28 10:00:00".to_string(),
            schema_version: 8,
            upstreams: vec![ExportUpstream {
                id: "up_test1".to_string(),
                provider_name: "OpenAI".to_string(),
                base_url: "https://api.openai.com".to_string(),
                api_key: "sk-test-api-key-12345".to_string(),
                selected_model: "gpt-4".to_string(),
                available_models: vec!["gpt-4".to_string(), "gpt-3.5-turbo".to_string()],
                enabled: true,
                remark: "test".to_string(),
            }],
            pools: vec![ExportPool {
                name: "gpt-4-pool".to_string(),
                display_name: "GPT-4".to_string(),
                max_concurrency: 5,
                thinking_enabled: false,
                upstreams: vec![ExportPoolUpstream {
                    upstream_id: "up_test1".to_string(),
                    sort_order: 0,
                    model: "gpt-4".to_string(),
                }],
            }],
            settings: {
                let mut m = HashMap::new();
                m.insert("listen_port".to_string(), "47339".to_string());
                m
            },
        }
    }

    #[test]
    fn test_config_export_serialization() {
        let export = make_test_export();
        let json = export.to_json().unwrap();
        assert!(json.contains("\"version\": \"1.0\""));
        assert!(json.contains("\"OpenAI\""));
        assert!(json.contains("\"gpt-4-pool\""));
    }

    #[test]
    fn test_config_export_deserialization() {
        let json = r#"{
            "version": "1.0",
            "exported_at": "2026-07-28",
            "schema_version": 10,
            "upstreams": [],
            "pools": [],
            "settings": {}
        }"#;
        let export: ConfigExport = serde_json::from_str(json).unwrap();
        assert_eq!(export.version, "1.0");
        assert_eq!(export.schema_version, 10);
        assert!(export.upstreams.is_empty());
        assert!(export.pools.is_empty());
    }

    #[test]
    fn test_import_request_deserialization() {
        let json = r#"{
            "config": {
                "version": "1.0",
                "exported_at": "2026-07-28",
                "schema_version": 10,
                "upstreams": [],
                "pools": [],
                "settings": {}
            },
            "mode": "incremental"
        }"#;
        let req = ImportRequest::from_json(json).unwrap();
        assert!(matches!(req.mode, ImportMode::Incremental));
    }

    #[test]
    fn test_import_request_full_mode() {
        let json = r#"{
            "config": {
                "version": "1.0",
                "exported_at": "2026-07-28",
                "schema_version": 10,
                "upstreams": [],
                "pools": [],
                "settings": {}
            },
            "mode": "full"
        }"#;
        let req = ImportRequest::from_json(json).unwrap();
        assert!(matches!(req.mode, ImportMode::Full));
    }

    #[test]
    fn test_validate_upstream_valid() {
        let u = make_test_export().upstreams[0].clone();
        assert!(validate_upstream(&u).is_ok());
    }

    #[test]
    fn test_validate_upstream_empty_name() {
        let mut u = make_test_export().upstreams[0].clone();
        u.provider_name = "".to_string();
        assert!(validate_upstream(&u).is_err());
    }

    #[test]
    fn test_validate_upstream_empty_url() {
        let mut u = make_test_export().upstreams[0].clone();
        u.base_url = "".to_string();
        assert!(validate_upstream(&u).is_err());
    }

    #[test]
    fn test_validate_upstream_invalid_url() {
        let mut u = make_test_export().upstreams[0].clone();
        u.base_url = "ftp://bad.protocol".to_string();
        assert!(validate_upstream(&u).is_err());
    }

    #[test]
    fn test_validate_pool_valid() {
        let p = make_test_export().pools[0].clone();
        assert!(validate_pool(&p).is_ok());
    }

    #[test]
    fn test_validate_pool_empty_name() {
        let mut p = make_test_export().pools[0].clone();
        p.name = "".to_string();
        assert!(validate_pool(&p).is_err());
    }

    #[test]
    fn test_validate_pool_empty_display_name() {
        let mut p = make_test_export().pools[0].clone();
        p.display_name = "".to_string();
        assert!(validate_pool(&p).is_err());
    }

    #[test]
    fn test_export_config_from_db() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("test_key", "test_value").unwrap();

        // Create a mock key manager
        let temp_dir = tempfile::tempdir().unwrap();
        let crypto = KeyManager::initialize(temp_dir.path()).unwrap();

        let export = export_config(&db, &crypto).unwrap();
        assert_eq!(export.schema_version, 10);
        assert!(export.settings.contains_key("test_key"));
        assert!(export.upstreams.is_empty());
        assert!(export.pools.is_empty());
    }

    #[test]
    fn test_import_full_creates_upstreams_and_pools() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let crypto = KeyManager::initialize(temp_dir.path()).unwrap();

        // Add some existing data first
        let encrypted = crypto.encrypt_api_key("sk-existing").unwrap();
        db.create_upstream("up_existing", "Existing", "https://existing.com", &encrypted, "model", "[]", true, "").unwrap();

        let config = make_test_export();
        let result = import_config(&db, &crypto, &config, &ImportMode::Full).unwrap();

        assert_eq!(result.upstreams_added, 1);
        assert_eq!(result.pools_added, 1);
        assert!(result.settings_imported > 0);

        // Verify old data was deleted
        let upstreams = db.get_upstreams().unwrap();
        assert_eq!(upstreams.len(), 1); // Only the imported one
        assert_eq!(upstreams[0].provider_name, "OpenAI");

        // Verify pool was created
        let pools = db.get_pools().unwrap();
        assert_eq!(pools.len(), 1);
        assert_eq!(pools[0].name, "gpt-4-pool");

        // Verify pool_upstreams association
        let pool_upstreams = db.get_pool_upstreams(&pools[0].id).unwrap();
        assert_eq!(pool_upstreams.len(), 1);
    }

    #[test]
    fn test_import_incremental_updates_existing() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let crypto = KeyManager::initialize(temp_dir.path()).unwrap();

        // Create an existing upstream that matches the import
        let encrypted = crypto.encrypt_api_key("sk-old-key").unwrap();
        db.create_upstream("up_existing", "OpenAI", "https://api.openai.com", &encrypted, "gpt-3.5-turbo", "[]", false, "old remark").unwrap();

        let config = make_test_export();
        let result = import_config(&db, &crypto, &config, &ImportMode::Incremental).unwrap();

        assert_eq!(result.upstreams_updated, 1);
        assert_eq!(result.upstreams_added, 0);
        assert_eq!(result.pools_added, 1);

        // Verify the existing upstream was updated
        let upstreams = db.get_upstreams().unwrap();
        assert_eq!(upstreams.len(), 1);
        assert_eq!(upstreams[0].selected_model, "gpt-4");
        assert!(upstreams[0].enabled);

        // Verify pool was created
        let pools = db.get_pools().unwrap();
        assert_eq!(pools.len(), 1);
    }

    #[test]
    fn test_import_incremental_creates_new() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let crypto = KeyManager::initialize(temp_dir.path()).unwrap();

        let config = make_test_export();
        let result = import_config(&db, &crypto, &config, &ImportMode::Incremental).unwrap();

        assert_eq!(result.upstreams_added, 1);
        assert_eq!(result.upstreams_updated, 0);
        assert_eq!(result.pools_added, 1);

        // Verify upstream was created with correct data
        let upstreams = db.get_upstreams().unwrap();
        assert_eq!(upstreams.len(), 1);
        assert_eq!(upstreams[0].provider_name, "OpenAI");

    // Verify API key was re-encrypted and can be decrypted
    let decrypted = crypto.decrypt_api_key(&upstreams[0].api_key_encrypted).unwrap();
    assert_eq!(decrypted, "sk-test-api-key-12345");
    }

    #[test]
    fn test_import_full_deletes_old_pools() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let crypto = KeyManager::initialize(temp_dir.path()).unwrap();

        // Create existing pool
        db.create_pool("pool_old", "old-pool", "Old Pool", 5, false).unwrap();

        let config = make_test_export();
        let result = import_config(&db, &crypto, &config, &ImportMode::Full).unwrap();

        assert_eq!(result.pools_added, 1);

        // Verify old pool was deleted
        let pools = db.get_pools().unwrap();
        assert_eq!(pools.len(), 1);
        assert_eq!(pools[0].name, "gpt-4-pool");
    }

    #[test]
    fn test_import_settings_overwrite() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.save_setting("listen_port", "11111").unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let crypto = KeyManager::initialize(temp_dir.path()).unwrap();

        let config = make_test_export();
        let result = import_config(&db, &crypto, &config, &ImportMode::Full).unwrap();

        assert!(result.settings_imported > 0);

        // Verify setting was overwritten
        let port = db.get_setting("listen_port").unwrap();
        assert_eq!(port.as_deref(), Some("47339"));
    }

    #[test]
    fn test_import_incremental_preserves_unmatched_data() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let crypto = KeyManager::initialize(temp_dir.path()).unwrap();

        // Create existing data that doesn't match the import
        let encrypted = crypto.encrypt_api_key("sk-other").unwrap();
        db.create_upstream("up_other", "OtherProvider", "https://other.com", &encrypted, "model-x", "[]", true, "").unwrap();
        db.create_pool("pool_other", "other-pool", "Other Pool", 3, false).unwrap();

        let config = make_test_export();
        let result = import_config(&db, &crypto, &config, &ImportMode::Incremental).unwrap();

        assert_eq!(result.upstreams_added, 1);
        assert_eq!(result.upstreams_updated, 0);

        // Verify old data was preserved
        let upstreams = db.get_upstreams().unwrap();
        assert_eq!(upstreams.len(), 2); // Old + new

        let pools = db.get_pools().unwrap();
        assert_eq!(pools.len(), 2); // Old + new
    }

    #[test]
    fn test_import_rejects_invalid_upstream() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let crypto = KeyManager::initialize(temp_dir.path()).unwrap();

        let mut config = make_test_export();
        config.upstreams[0].provider_name = "".to_string();

        let result = import_config(&db, &crypto, &config, &ImportMode::Incremental);
        assert!(result.is_err());
    }

    #[test]
    fn test_import_rejects_invalid_pool() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let crypto = KeyManager::initialize(temp_dir.path()).unwrap();

        let mut config = make_test_export();
        config.pools[0].name = "".to_string();

        let result = import_config(&db, &crypto, &config, &ImportMode::Incremental);
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_id_uniqueness() {
        let id1 = generate_id("up");
        std::thread::sleep(std::time::Duration::from_millis(1));
        let id2 = generate_id("up");
        assert_ne!(id1, id2);
        assert!(id1.starts_with("up_"));
        assert!(id2.starts_with("up_"));
    }

    #[test]
    fn test_config_export_with_empty_api_key() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        // Create upstream with empty encrypted key
        db.create_upstream("up_empty", "Empty", "https://empty.com", &[], "model", "[]", true, "").unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let crypto = KeyManager::initialize(temp_dir.path()).unwrap();

        let export = export_config(&db, &crypto).unwrap();
        assert_eq!(export.upstreams.len(), 1);
        assert!(export.upstreams[0].api_key.is_empty());
    }
}
