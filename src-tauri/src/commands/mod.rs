use serde::{Deserialize, Serialize};
use tauri::State;

use llm_api_proxy_lib::AppState;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

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

fn default_5_i32() -> i32 {
    5
}

fn default_200_i64() -> i64 {
    200
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
