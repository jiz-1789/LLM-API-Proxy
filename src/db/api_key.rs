use crate::error::AppError;
use rusqlite::params;

use super::{ApiKey, Database};

impl Database {
    // ========================================================================
    // API Key CRUD (P2-8: Multi-key access control)
    // ========================================================================

    /// Create a new API key record.
    ///
    /// # Arguments
    /// * `id` - Unique identifier (e.g., `ak_xxx`)
    /// * `key` - The actual API key string (e.g., `sk-gw-xxxx`)
    /// * `name` - Human-readable label
    /// * `allowed_pools` - JSON array of pool IDs (empty array = all pools)
    /// * `expires_at` - Optional expiration timestamp (NULL = never expires)
    pub fn create_api_key(
        &self,
        id: &str,
        key: &str,
        name: &str,
        allowed_pools: &str,
        expires_at: Option<&str>,
    ) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "INSERT INTO api_keys (id, key, name, enabled, allowed_pools, expires_at)
             VALUES (?1, ?2, ?3, 1, ?4, ?5)",
            params![id, key, name, allowed_pools, expires_at],
        )?;
        Ok(())
    }

    /// Get all API keys ordered by creation time (newest first).
    pub fn get_api_keys(&self) -> Result<Vec<ApiKey>, AppError> {
        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, key, name, enabled, allowed_pools, expires_at,
                    last_used_at, created_at, updated_at
             FROM api_keys ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], Self::map_api_key_row)?;
        Self::collect_rows(rows)
    }

    /// Get a single API key by its ID.
    pub fn get_api_key_by_id(&self, id: &str) -> Result<Option<ApiKey>, AppError> {
        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, key, name, enabled, allowed_pools, expires_at,
                    last_used_at, created_at, updated_at
             FROM api_keys WHERE id = ?1",
        )?;
        let result = stmt.query_row(params![id], Self::map_api_key_row);
        match result {
            Ok(k) => Ok(Some(k)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e)),
        }
    }

    /// Look up an API key by its key string (for authentication).
    ///
    /// This is the primary lookup used by the gateway auth layer.
    /// Returns the full key record including `allowed_pools` and `enabled` status.
    pub fn get_api_key_by_key(&self, key: &str) -> Result<Option<ApiKey>, AppError> {
        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, key, name, enabled, allowed_pools, expires_at,
                    last_used_at, created_at, updated_at
             FROM api_keys WHERE key = ?1",
        )?;
        let result = stmt.query_row(params![key], Self::map_api_key_row);
        match result {
            Ok(k) => Ok(Some(k)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e)),
        }
    }

    /// Update an API key's properties (name, enabled, allowed_pools, expires_at).
    ///
    /// The `key` field itself is not updated here; use `regenerate_api_key()` for that.
    pub fn update_api_key(
        &self,
        id: &str,
        name: &str,
        enabled: bool,
        allowed_pools: &str,
        expires_at: Option<&str>,
    ) -> Result<(), AppError> {
        let rows = self.get_conn()?.execute(
            "UPDATE api_keys SET name=?1, enabled=?2, allowed_pools=?3, expires_at=?4,
             updated_at=datetime('now', 'localtime') WHERE id=?5",
            params![name, enabled as i32, allowed_pools, expires_at, id],
        )?;
        if rows == 0 {
            return Err(AppError::NotFound(format!("api key {}", id)));
        }
        Ok(())
    }

    /// Delete an API key by ID.
    pub fn delete_api_key(&self, id: &str) -> Result<(), AppError> {
        self.get_conn()?
            .execute("DELETE FROM api_keys WHERE id=?1", params![id])?;
        Ok(())
    }

    /// Toggle an API key's enabled status.
    pub fn toggle_api_key(&self, id: &str, enabled: bool) -> Result<(), AppError> {
        let rows = self.get_conn()?.execute(
            "UPDATE api_keys SET enabled=?1, updated_at=datetime('now', 'localtime') WHERE id=?2",
            params![enabled as i32, id],
        )?;
        if rows == 0 {
            return Err(AppError::NotFound(format!("api key {}", id)));
        }
        Ok(())
    }

    /// Regenerate the key string for an existing API key ID.
    /// This invalidates the old key string and replaces it with a new one.
    pub fn regenerate_api_key(&self, id: &str, new_key: &str) -> Result<(), AppError> {
        let rows = self.get_conn()?.execute(
            "UPDATE api_keys SET key=?1, updated_at=datetime('now', 'localtime') WHERE id=?2",
            params![new_key, id],
        )?;
        if rows == 0 {
            return Err(AppError::NotFound(format!("api key {}", id)));
        }
        Ok(())
    }

    /// Update the `last_used_at` timestamp for an API key.
    /// Called after successful authentication to track usage.
    /// Errors are non-fatal (best-effort update).
    pub fn update_api_key_last_used(&self, id: &str) -> Result<(), AppError> {
        self.get_conn()?.execute(
            "UPDATE api_keys SET last_used_at=datetime('now', 'localtime') WHERE id=?1",
            params![id],
        )?;
        Ok(())
    }

    /// Count total API keys.
    pub fn count_api_keys(&self) -> Result<i64, AppError> {
        let count: i64 = self.get_read_conn()?.query_row(
            "SELECT COUNT(*) FROM api_keys",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_key(db: &Database, id: &str, key: &str, name: &str) -> ApiKey {
        db.create_api_key(id, key, name, "[]", None).unwrap();
        db.get_api_key_by_id(id).unwrap().unwrap()
    }

    #[test]
    fn test_create_and_get_api_key() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let key = create_test_key(&db, "ak_1", "sk-gw-test123", "测试密钥");
        assert_eq!(key.id, "ak_1");
        assert_eq!(key.key, "sk-gw-test123");
        assert_eq!(key.name, "测试密钥");
        assert!(key.enabled);
        assert_eq!(key.allowed_pools, "[]");
        assert!(key.expires_at.is_none());
        assert!(key.last_used_at.is_none());
    }

    #[test]
    fn test_get_api_key_by_key_string() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        create_test_key(&db, "ak_1", "sk-gw-lookup", "查找测试");

        let found = db.get_api_key_by_key("sk-gw-lookup").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "查找测试");

        let not_found = db.get_api_key_by_key("sk-gw-nonexistent").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_get_api_keys_list() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        create_test_key(&db, "ak_1", "sk-gw-1", "密钥1");
        create_test_key(&db, "ak_2", "sk-gw-2", "密钥2");
        create_test_key(&db, "ak_3", "sk-gw-3", "密钥3");

        let keys = db.get_api_keys().unwrap();
        assert_eq!(keys.len(), 3);
        // All three keys should be present (ordering may vary due to same timestamp)
        let ids: Vec<&str> = keys.iter().map(|k| k.id.as_str()).collect();
        assert!(ids.contains(&"ak_1"));
        assert!(ids.contains(&"ak_2"));
        assert!(ids.contains(&"ak_3"));
    }

    #[test]
    fn test_update_api_key() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        create_test_key(&db, "ak_1", "sk-gw-update", "原始名称");

        db.update_api_key("ak_1", "新名称", false, "[\"pool_1\"]", Some("2026-12-31 23:59:59"))
            .unwrap();

        let updated = db.get_api_key_by_id("ak_1").unwrap().unwrap();
        assert_eq!(updated.name, "新名称");
        assert!(!updated.enabled);
        assert_eq!(updated.allowed_pools, "[\"pool_1\"]");
        assert_eq!(updated.expires_at.as_deref(), Some("2026-12-31 23:59:59"));
    }

    #[test]
    fn test_update_api_key_not_found() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let result = db.update_api_key("nonexistent", "name", true, "[]", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_delete_api_key() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        create_test_key(&db, "ak_1", "sk-gw-delete", "待删除");
        assert_eq!(db.count_api_keys().unwrap(), 1);

        db.delete_api_key("ak_1").unwrap();
        assert_eq!(db.count_api_keys().unwrap(), 0);

        // Deleting non-existent key should not error
        db.delete_api_key("ak_1").unwrap();
    }

    #[test]
    fn test_toggle_api_key() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        create_test_key(&db, "ak_1", "sk-gw-toggle", "切换测试");
        assert!(db.get_api_key_by_id("ak_1").unwrap().unwrap().enabled);

        db.toggle_api_key("ak_1", false).unwrap();
        assert!(!db.get_api_key_by_id("ak_1").unwrap().unwrap().enabled);

        db.toggle_api_key("ak_1", true).unwrap();
        assert!(db.get_api_key_by_id("ak_1").unwrap().unwrap().enabled);
    }

    #[test]
    fn test_regenerate_api_key() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        create_test_key(&db, "ak_1", "sk-gw-old", "原始");

        db.regenerate_api_key("ak_1", "sk-gw-new").unwrap();

        // Old key should no longer be found
        assert!(db.get_api_key_by_key("sk-gw-old").unwrap().is_none());
        // New key should be found
        let new_key = db.get_api_key_by_key("sk-gw-new").unwrap().unwrap();
        assert_eq!(new_key.id, "ak_1");
    }

    #[test]
    fn test_update_last_used() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        create_test_key(&db, "ak_1", "sk-gw-used", "使用测试");
        assert!(db.get_api_key_by_id("ak_1").unwrap().unwrap().last_used_at.is_none());

        db.update_api_key_last_used("ak_1").unwrap();

        let key = db.get_api_key_by_id("ak_1").unwrap().unwrap();
        assert!(key.last_used_at.is_some());
    }

    #[test]
    fn test_unique_key_constraint() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        create_test_key(&db, "ak_1", "sk-gw-duplicate", "第一个");

        // Inserting a second key with the same key string should fail
        let result = db.create_api_key("ak_2", "sk-gw-duplicate", "第二个", "[]", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_api_keys_with_allowed_pools() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db.create_api_key(
            "ak_1",
            "sk-gw-pools",
            "受限密钥",
            "[\"pool_abc\",\"pool_def\"]",
            None,
        )
        .unwrap();

        let key = db.get_api_key_by_id("ak_1").unwrap().unwrap();
        assert_eq!(key.allowed_pools, "[\"pool_abc\",\"pool_def\"]");

        // Verify the JSON can be parsed
        let pools: Vec<String> = serde_json::from_str(&key.allowed_pools).unwrap();
        assert_eq!(pools.len(), 2);
        assert!(pools.contains(&"pool_abc".to_string()));
        assert!(pools.contains(&"pool_def".to_string()));
    }
}
