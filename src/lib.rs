// LLM-API-Proxy Gateway Library
pub mod config;
pub mod crypto;
pub mod db;
pub mod error;
pub mod gateway;
pub mod pool;
pub mod proxy;

use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<config::GatewaySettings>,
    pub db: Arc<db::Database>,
    pub crypto: Arc<crypto::KeyManager>,
}

impl AppState {
    pub fn new(settings: config::GatewaySettings, db: db::Database, crypto: crypto::KeyManager) -> Self {
        Self {
            settings: Arc::new(settings),
            db: Arc::new(db),
            crypto: Arc::new(crypto),
        }
    }
}
