use crate::error::AppError;
use rusqlite::params;

use super::Database;

impl Database {
    // ========================================================================
    // Rate Limit State Persistence
    // ========================================================================

    /// Load all rate limit state entries from the database.
    /// Returns a vector of (client_ip, count, window_start_unix_secs).
    pub fn load_rate_limit_state(&self) -> Result<Vec<(String, u32, i64)>, AppError> {
        let conn = self.get_read_conn()?;
        let mut stmt = conn.prepare(
            "SELECT client_ip, count, window_start FROM rate_limit_state",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        Self::collect_rows(rows)
    }

    /// Save (upsert) a batch of rate limit state entries.
    /// Each entry is (client_ip, count, window_start_unix_secs).
    pub fn save_rate_limit_state(
        &self,
        entries: &[(String, u32, i64)],
    ) -> Result<(), AppError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "INSERT INTO rate_limit_state (client_ip, count, window_start)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(client_ip) DO UPDATE SET count=?2, window_start=?3",
        )?;
        for (ip, count, window_start) in entries {
            stmt.execute(params![ip, count, window_start])?;
        }
        Ok(())
    }

    /// Clear all rate limit state entries.
    pub fn clear_rate_limit_state(&self) -> Result<(), AppError> {
        self.get_conn()?
            .execute("DELETE FROM rate_limit_state", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        db
    }

    #[test]
    fn test_load_empty_rate_limit_state() {
        let db = test_db();
        let state = db.load_rate_limit_state().unwrap();
        assert!(state.is_empty());
    }

    #[test]
    fn test_save_and_load_rate_limit_state() {
        let db = test_db();
        let entries = vec![
            ("1.2.3.4".to_string(), 5u32, 1700000000i64),
            ("5.6.7.8".to_string(), 10u32, 1700000100i64),
        ];
        db.save_rate_limit_state(&entries).unwrap();

        let loaded = db.load_rate_limit_state().unwrap();
        assert_eq!(loaded.len(), 2);

        // Find entries by IP
        let e1 = loaded.iter().find(|(ip, _, _)| ip == "1.2.3.4").unwrap();
        assert_eq!(e1.1, 5);
        assert_eq!(e1.2, 1700000000);

        let e2 = loaded.iter().find(|(ip, _, _)| ip == "5.6.7.8").unwrap();
        assert_eq!(e2.1, 10);
        assert_eq!(e2.2, 1700000100);
    }

    #[test]
    fn test_upsert_rate_limit_state() {
        let db = test_db();
        // Insert
        db.save_rate_limit_state(&[("1.2.3.4".to_string(), 3u32, 1700000000i64)])
            .unwrap();
        // Upsert (update existing)
        db.save_rate_limit_state(&[("1.2.3.4".to_string(), 7u32, 1700000500i64)])
            .unwrap();

        let loaded = db.load_rate_limit_state().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].1, 7); // count updated
        assert_eq!(loaded[0].2, 1700000500); // window_start updated
    }

    #[test]
    fn test_clear_rate_limit_state() {
        let db = test_db();
        db.save_rate_limit_state(&[
            ("1.2.3.4".to_string(), 5u32, 1700000000i64),
            ("5.6.7.8".to_string(), 10u32, 1700000100i64),
        ])
        .unwrap();
        assert_eq!(db.load_rate_limit_state().unwrap().len(), 2);

        db.clear_rate_limit_state().unwrap();
        assert!(db.load_rate_limit_state().unwrap().is_empty());
    }

    #[test]
    fn test_save_empty_batch() {
        let db = test_db();
        let entries: Vec<(String, u32, i64)> = vec![];
        db.save_rate_limit_state(&entries).unwrap();
        assert!(db.load_rate_limit_state().unwrap().is_empty());
    }
}
