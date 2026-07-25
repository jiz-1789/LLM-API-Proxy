use serde::{Deserialize, Serialize};

/// Configuration for a single upstream provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamConfig {
    pub id: String,
    pub provider_name: String,
    pub base_url: String,
    pub api_key: String,
    pub selected_model: String,
    pub enabled: bool,
}
