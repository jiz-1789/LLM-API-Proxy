//! Gateway token for tool config injection.
//!
//! Tool configs write a dedicated `llm-proxy-{uuid}` token instead of the
//! primary `gateway_api_key`, so the primary key never ends up embedded in a
//! tool's config file (which may be synced/read by other tools). The gateway
//! accepts this token alongside the primary key (see `gateway/auth.rs`).

use crate::db::Database;
use crate::error::AppError;

/// Settings key holding the tool gateway token.
pub const TOOL_TOKEN_SETTING_KEY: &str = "tool_gateway_token";

/// Return the persistent tool gateway token, generating and persisting it on
/// first use. The token is stable across restarts so tool configs stay valid.
pub fn get_or_create_tool_token(db: &Database) -> Result<String, AppError> {
    if let Some(token) = db.get_setting(TOOL_TOKEN_SETTING_KEY)? {
        let trimmed = token.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    let token = format!("llm-proxy-{}", uuid::Uuid::new_v4().simple());
    db.save_setting(TOOL_TOKEN_SETTING_KEY, &token)?;
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_token_stable_across_calls() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        let a = get_or_create_tool_token(&db).unwrap();
        let b = get_or_create_tool_token(&db).unwrap();
        assert_eq!(a, b);
        assert!(a.starts_with("llm-proxy-"));
    }

    #[test]
    fn test_tool_token_persisted() {
        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();
        let token = get_or_create_tool_token(&db).unwrap();
        assert_eq!(
            db.get_setting(TOOL_TOKEN_SETTING_KEY).unwrap().unwrap(),
            token
        );
    }
}
