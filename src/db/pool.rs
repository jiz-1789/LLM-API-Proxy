use crate::error::AppError;
use rusqlite::params;

use super::{Database, Pool, PoolUpstreamInfo};

impl Database {
    // ========================================================================
    // Pool CRUD
    // ========================================================================

    /// Create a new pool record.
    pub fn create_pool(
        &self,
        id: &str,
        name: &str,
        display_name: &str,
        max_concurrency: i32,
        thinking_enabled: bool,
        thinking_level: &str,
        thinking_custom_params: &str,
        capabilities: &str,
    ) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "INSERT INTO pools (id, name, display_name, max_concurrency, thinking_enabled,
                                thinking_level, thinking_custom_params, capabilities)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                name,
                display_name,
                max_concurrency,
                thinking_enabled as i32,
                thinking_level,
                thinking_custom_params,
                capabilities
            ],
        )?;
        Ok(())
    }

    /// Get all pools ordered by creation time (newest first).
    pub fn get_pools(&self) -> Result<Vec<Pool>, AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, display_name, round_robin_strategy, failover_enabled,
                    timeout_seconds, max_concurrency, thinking_enabled,
                    thinking_level, thinking_custom_params, capabilities,
                    created_at, updated_at
             FROM pools ORDER BY created_at DESC"
        )?;
        let rows = stmt.query_map([], Self::map_pool_row)?;
        Self::collect_rows(rows)
    }

    /// Get a single pool by its ID.
    pub fn get_pool_by_id(&self, id: &str) -> Result<Option<Pool>, AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, display_name, round_robin_strategy, failover_enabled,
                    timeout_seconds, max_concurrency, thinking_enabled,
                    thinking_level, thinking_custom_params, capabilities,
                    created_at, updated_at
             FROM pools WHERE id = ?1"
        )?;
        let result = stmt.query_row(params![id], Self::map_pool_row);
        match result {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e)),
        }
    }

    /// Get a pool by its unique model name.
    pub fn get_pool_by_name(&self, name: &str) -> Result<Option<Pool>, AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, display_name, round_robin_strategy, failover_enabled,
                    timeout_seconds, max_concurrency, thinking_enabled,
                    thinking_level, thinking_custom_params, capabilities,
                    created_at, updated_at
             FROM pools WHERE name = ?1"
        )?;
        let result = stmt.query_row(params![name], Self::map_pool_row);
        match result {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e)),
        }
    }

    /// Update pool configuration.
    pub fn update_pool(
        &self,
        id: &str,
        display_name: &str,
        max_concurrency: i32,
        thinking_enabled: bool,
        thinking_level: &str,
        thinking_custom_params: &str,
        capabilities: &str,
    ) -> Result<(), AppError> {
        let rows = self.get_conn()?.execute(
            "UPDATE pools SET display_name=?1, max_concurrency=?2, thinking_enabled=?3,
             thinking_level=?4, thinking_custom_params=?5, capabilities=?6,
             updated_at=datetime('now', 'localtime') WHERE id=?7",
            params![
                display_name,
                max_concurrency,
                thinking_enabled as i32,
                thinking_level,
                thinking_custom_params,
                capabilities,
                id
            ],
        )?;
        if rows == 0 {
            return Err(AppError::NotFound(format!("pool {}", id)));
        }
        Ok(())
    }

    /// 重新计算池级能力（池内所有上游能力的并集）并写回。
    pub fn recompute_pool_capabilities(&self, pool_id: &str) -> Result<(), AppError> {
        let infos = self.get_pool_upstreams(pool_id)?;
        let mut merged = super::ModelCapabilities::default();
        for info in &infos {
            let caps = if info.capabilities.trim().is_empty() {
                crate::gateway::convert::capabilities::infer_capabilities(&info.model)
            } else {
                super::ModelCapabilities::from_json_str(&info.capabilities)
                    .unwrap_or_else(|| crate::gateway::convert::capabilities::infer_capabilities(&info.model))
            };
            merged = merged.union(&caps);
        }
        let json = merged.to_json_str();
        self.get_conn()?.execute(
            "UPDATE pools SET capabilities=?1, updated_at=datetime('now', 'localtime') WHERE id=?2",
            params![json, pool_id],
        )?;
        Ok(())
    }

    /// 重新计算所有池的池级能力。
    pub fn recompute_all_pool_capabilities(&self) -> Result<(), AppError> {
        let pools = self.get_pools()?;
        for pool in &pools {
            self.recompute_pool_capabilities(&pool.id)?;
        }
        Ok(())
    }

    /// Delete a pool by ID (cascade removes pool_upstreams associations).
    pub fn delete_pool(&self, id: &str) -> Result<(), AppError> {
        self.get_conn()?.execute("DELETE FROM pools WHERE id=?", params![id])?;
        Ok(())
    }

    // ========================================================================
    // Pool-Upstream Association
    // ========================================================================

    /// Associate an upstream with a pool at the given sort order, specifying which model to use.
    pub fn add_upstream_to_pool(
        &self,
        pool_id: &str,
        upstream_id: &str,
        sort_order: i32,
        model: &str,
    ) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "INSERT INTO pool_upstreams (pool_id, upstream_id, sort_order, model, capabilities)
             SELECT ?1, ?2, ?3, ?4, capabilities FROM upstreams WHERE id=?2",
            params![pool_id, upstream_id, sort_order, model],
        )?;
        self.recompute_pool_capabilities(pool_id)?;
        Ok(())
    }

    /// Remove an upstream from a pool.
    pub fn remove_upstream_from_pool(&self, pool_id: &str, upstream_id: &str) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "DELETE FROM pool_upstreams WHERE pool_id=?1 AND upstream_id=?2",
            params![pool_id, upstream_id],
        )?;
        self.recompute_pool_capabilities(pool_id)?;
        Ok(())
    }

    /// Update a pool-upstream association: model and/or per-upstream thinking
    /// level override. An empty override means "follow the pool level".
    pub fn update_pool_upstream(
        &self,
        pool_id: &str,
        upstream_id: &str,
        model: Option<&str>,
        thinking_level_override: Option<&str>,
    ) -> Result<(), AppError> {
        let conn = self.get_conn()?;
        let has_model = model.is_some();
        let has_override = thinking_level_override.is_some();
        if has_model && has_override {
            conn.execute(
                "UPDATE pool_upstreams SET model=?3, thinking_level_override=?4
                 WHERE pool_id=?1 AND upstream_id=?2",
                params![pool_id, upstream_id, model.unwrap_or(""), thinking_level_override.unwrap_or("")],
            )?;
        } else if has_model {
            conn.execute(
                "UPDATE pool_upstreams SET model=?3 WHERE pool_id=?1 AND upstream_id=?2",
                params![pool_id, upstream_id, model.unwrap_or("")],
            )?;
        } else if has_override {
            conn.execute(
                "UPDATE pool_upstreams SET thinking_level_override=?3
                 WHERE pool_id=?1 AND upstream_id=?2",
                params![pool_id, upstream_id, thinking_level_override.unwrap_or("")],
            )?;
        }
        Ok(())
    }

    /// Get all upstreams for a pool, ordered by sort_order.
    pub fn get_pool_upstreams(&self, pool_id: &str) -> Result<Vec<PoolUpstreamInfo>, AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT u.id, u.provider_name, pu.model, pu.sort_order, pu.thinking_level_override, pu.capabilities
             FROM pool_upstreams pu
             JOIN upstreams u ON u.id = pu.upstream_id
             WHERE pu.pool_id=?1
             ORDER BY pu.sort_order ASC"
        )?;
        let rows = stmt.query_map(params![pool_id], |row| {
            Ok(PoolUpstreamInfo {
                upstream_id: row.get(0)?,
                provider_name: row.get(1)?,
                model: row.get(2)?,
                sort_order: row.get(3)?,
                thinking_level_override: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                capabilities: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            })
        })?;
        Self::collect_rows(rows)
    }

    /// Reorder upstreams in a pool by providing the full desired order.
    pub fn reorder_pool_upstreams(
        &self,
        pool_id: &str,
        ordered_upstream_ids: &[String],
    ) -> Result<(), AppError> {
        self.with_transaction(|conn| {
            for (idx, uid) in ordered_upstream_ids.iter().enumerate() {
                conn.execute(
                    "UPDATE pool_upstreams SET sort_order=?1 WHERE pool_id=?2 AND upstream_id=?3",
                    params![idx as i32, pool_id, uid],
                )?;
            }
            Ok(())
        })
    }

    /// Check if a pool exists by ID.
    pub fn pool_exists(&self, id: &str) -> Result<bool, AppError> {
        let count: i64 = self.get_conn()?.query_row(
            "SELECT COUNT(*) FROM pools WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Count total pools.
    pub fn count_pools(&self) -> Result<i64, AppError> {
        let count: i64 = self.get_conn()?.query_row(
            "SELECT COUNT(*) FROM pools", [], |row| row.get(0)
        )?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ModelCapabilities;

    #[test]
    fn test_pool_capabilities_roundtrip_and_recompute() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        let crypto = crate::crypto::KeyManager::initialize(&std::env::temp_dir()).unwrap();
        let enc = crypto.encrypt_api_key("sk-test").unwrap();

        db.create_upstream("up_a", "OpenAI", "https://a.com", &enc, "gpt-4o", "[]", true, "", "", "openai_chat")
            .unwrap();
        db.create_upstream("up_b", "DeepSeek", "https://b.com", &enc, "deepseek-chat", "[]", true, "", "", "openai_chat")
            .unwrap();

        db.create_pool("pool_1", "p1", "P1", 5, false, "off", "", "")
            .unwrap();
        db.add_upstream_to_pool("pool_1", "up_a", 0, "gpt-4o")
            .unwrap();
        db.add_upstream_to_pool("pool_1", "up_b", 1, "deepseek-chat")
            .unwrap();

        // Pool capabilities auto-aggregated (union) after add_upstream_to_pool.
        let pool = db.get_pool_by_id("pool_1").unwrap().unwrap();
        let caps = ModelCapabilities::from_json_str(&pool.capabilities).unwrap();
        assert!(caps.input_modalities.contains(&"text".to_string()));
        assert!(caps.input_modalities.contains(&"image".to_string()));
        assert!(caps.supports_function_calling);

        // Removing an upstream recomputes and narrows capabilities.
        db.remove_upstream_from_pool("pool_1", "up_a").unwrap();
        let pool = db.get_pool_by_id("pool_1").unwrap().unwrap();
        let caps = ModelCapabilities::from_json_str(&pool.capabilities).unwrap();
        assert!(!caps.input_modalities.contains(&"image".to_string()));

        // Explicit recompute is idempotent and round-trips.
        db.recompute_all_pool_capabilities().unwrap();
        let pool = db.get_pool_by_id("pool_1").unwrap().unwrap();
        let caps = ModelCapabilities::from_json_str(&pool.capabilities).unwrap();
        assert!(caps.supports_function_calling);
        assert_eq!(caps.input_modalities, vec!["text".to_string()]);
    }

    #[test]
    fn test_explicit_capabilities_are_respected_in_recompute() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        let crypto = crate::crypto::KeyManager::initialize(&std::env::temp_dir()).unwrap();
        let enc = crypto.encrypt_api_key("sk-test").unwrap();

        // A custom capability JSON stored on the upstream overrides inference.
        let explicit = serde_json::json!({
            "input_modalities": ["text", "audio"],
            "output_modalities": ["text"],
            "supports_function_calling": true,
            "supports_streaming": true,
            "context_window": null,
            "max_output_tokens": null
        })
        .to_string();

        db.create_upstream("up_c", "Custom", "https://c.com", &enc, "some-model", "[]", true, "", &explicit, "openai_chat")
            .unwrap();
        db.create_pool("pool_2", "p2", "P2", 5, false, "off", "", "")
            .unwrap();
        db.add_upstream_to_pool("pool_2", "up_c", 0, "some-model")
            .unwrap();

        let pool = db.get_pool_by_id("pool_2").unwrap().unwrap();
        let caps = ModelCapabilities::from_json_str(&pool.capabilities).unwrap();
        assert!(caps.input_modalities.contains(&"audio".to_string()));
        assert!(!caps.input_modalities.contains(&"image".to_string()));
    }

    #[test]
    fn test_update_pool_upstream_model_and_override() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        let crypto = crate::crypto::KeyManager::initialize(&std::env::temp_dir()).unwrap();
        let enc = crypto.encrypt_api_key("sk-test").unwrap();

        db.create_upstream("up_a", "OpenAI", "https://a.com", &enc, "gpt-4o", "[]", true, "", "", "openai_chat")
            .unwrap();
        db.create_pool("pool_1", "p1", "P1", 5, false, "off", "", "")
            .unwrap();
        db.add_upstream_to_pool("pool_1", "up_a", 0, "gpt-4o")
            .unwrap();

        // Default: override empty (follows pool level).
        let ups = db.get_pool_upstreams("pool_1").unwrap();
        assert_eq!(ups[0].thinking_level_override, "");

        // Set only override.
        db.update_pool_upstream("pool_1", "up_a", None, Some("high"))
            .unwrap();
        let ups = db.get_pool_upstreams("pool_1").unwrap();
        assert_eq!(ups[0].thinking_level_override, "high");

        // Set both model + override.
        db.update_pool_upstream("pool_1", "up_a", Some("gpt-4o-mini"), Some("max"))
            .unwrap();
        let ups = db.get_pool_upstreams("pool_1").unwrap();
        assert_eq!(ups[0].model, "gpt-4o-mini");
        assert_eq!(ups[0].thinking_level_override, "max");

        // Clear override back to follow-pool.
        db.update_pool_upstream("pool_1", "up_a", None, Some(""))
            .unwrap();
        let ups = db.get_pool_upstreams("pool_1").unwrap();
        assert_eq!(ups[0].thinking_level_override, "");
    }
}
