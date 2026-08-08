// ============================================================================
// Re-exports
// ============================================================================

pub mod upstream;
pub mod pool;
pub mod log;
pub mod settings;
pub mod health;
pub mod update;
pub mod shortcut;
pub mod backup;
pub mod diagnostic;
pub mod api_key;
pub mod tool_config;

// ============================================================================
// Shared DTOs & Helpers
// ============================================================================

fn default_true() -> bool {
    true
}

fn default_zh() -> String {
    "zh".to_string()
}

fn default_60() -> u32 {
    60
}

fn default_300() -> u32 {
    300
}

fn default_3() -> u32 {
    3
}

fn default_30_i32() -> i32 {
    30
}

fn default_20000_i64() -> i64 {
    20000
}

fn default_50_f64() -> f64 {
    50.0
}

fn default_10_u32() -> u32 {
    10
}

fn default_30_u32() -> u32 {
    30
}

fn default_7_u32() -> u32 {
    7
}

fn default_5_u32() -> u32 {
    5
}

/// Generate a unique ID for a new upstream record.
pub fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("up_{:x}{:08x}", now.as_secs(), now.subsec_nanos())
}

/// Generate a unique ID for a pool.
pub fn generate_pool_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("pool_{:x}{:08x}", now.as_secs(), now.subsec_nanos())
}
