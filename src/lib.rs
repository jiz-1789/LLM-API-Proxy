// LLM-API-Proxy Gateway Library

pub mod config;
pub mod crypto;
pub mod db;
pub mod error;
pub mod gateway;
pub mod pool;
pub mod proxy;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Shared application state available to all components.
#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<config::GatewaySettings>,
    pub db: Arc<db::Database>,
    pub crypto: Arc<crypto::KeyManager>,
    /// Signal to trigger graceful shutdown of the gateway server.
    shutdown: Arc<tokio::sync::Notify>,
    /// Cached minimize-to-tray preference (avoids DB reads in window event handlers).
    minimize_to_tray: Arc<AtomicBool>,
}

impl AppState {
    pub fn new(
        settings: config::GatewaySettings,
        db: Arc<db::Database>,
        crypto: Arc<crypto::KeyManager>,
        shutdown: Arc<tokio::sync::Notify>,
        minimize_to_tray: bool,
    ) -> Self {
        Self {
            settings: Arc::new(settings),
            db,
            crypto,
            shutdown,
            minimize_to_tray: Arc::new(AtomicBool::new(minimize_to_tray)),
        }
    }

    /// Signal the gateway server to shut down gracefully and release the port.
    pub fn shutdown(&self) {
        tracing::info!("Shutting down gateway server...");
        self.shutdown.notify_one();
    }

    /// Get the cached minimize-to-tray preference.
    pub fn get_minimize_to_tray(&self) -> bool {
        self.minimize_to_tray.load(Ordering::SeqCst)
    }

    /// Update the cached minimize-to-tray preference.
    pub fn set_minimize_to_tray(&self, value: bool) {
        self.minimize_to_tray.store(value, Ordering::SeqCst);
    }
}

/// Initialize backend services (database, crypto, gateway server).
/// Returns the shared [`AppState`] for use by Tauri commands or other consumers.
///
/// This function:
/// 1. Initializes tracing/logging
/// 2. Creates the data directory
/// 3. Opens and migrates the SQLite database
/// 4. Initializes the AES-256-GCM key manager
/// 5. Loads gateway settings from the database
/// 6. Starts the HTTP gateway server on a background thread
pub fn initialize_backend() -> anyhow::Result<AppState> {
    // Initialize tracing subscriber (respects RUST_LOG env var)
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    let data_dir = config::GatewaySettings::data_dir();
    std::fs::create_dir_all(&data_dir)?;
    tracing::info!("Data directory: {:?}", data_dir);

    // Open and initialize database with migrations
    let db = db::Database::open(&config::GatewaySettings::db_path())?;
    db.initialize()?;
    tracing::info!("Database initialized");

    // Initialize crypto key manager (once, shared everywhere)
    let crypto = Arc::new(crypto::KeyManager::initialize(&data_dir)?);
    tracing::info!("Crypto key manager ready");

    // Load or create default settings, then persist defaults
    let settings = load_settings(&db);
    db.save_setting("listen_port", &settings.listen_port.to_string())?;
    db.save_setting("listen_address", &settings.listen_address)?;
    db.save_setting("gateway_api_key", &settings.api_key)?;

    // Wrap in Arc shared by both gateway server and Tauri commands
    let db_arc = Arc::new(db);

    // Load minimize-to-tray from the shared connection (not a separate one)
    let minimize = load_minimize_to_tray(&db_arc);

    // Shutdown signal for the gateway server
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let shutdown_for_server = shutdown.clone();
    let db_for_gw = db_arc.clone();

    // Start HTTP gateway server on a background thread
    let listen_addr = settings.listen_address.clone();
    let listen_port = settings.listen_port;
    let proxy_client = Arc::new(proxy::failover::UpstreamClient::new());
    let crypto_for_gw = crypto.clone();

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("Failed to create tokio runtime: {}", e);
                return;
            }
        };
        rt.block_on(async move {
            let rate_limit_config = load_rate_limit_config(&db_for_gw);
            let router = gateway::create_router(db_for_gw, proxy_client, crypto_for_gw, rate_limit_config);
            let addr = format!("{}:{}", listen_addr, listen_port);
            tracing::info!("Gateway server listening on {}", addr);
            let listener = match tokio::net::TcpListener::bind(&addr).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!("Failed to bind gateway port {}: {}", addr, e);
                    return;
                }
            };
            if let Err(e) = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    shutdown_for_server.notified().await;
                    tracing::info!("Gateway server received shutdown signal");
                })
                .await
            {
                tracing::error!("Gateway server error: {}", e);
            }
            tracing::info!("Gateway server stopped, port released");
        });
    });

    // Share the SAME database connection for both gateway and Tauri commands
    let state = AppState::new(settings, db_arc.clone(), crypto, shutdown, minimize);
    Ok(state)
}

/// Generate a random API key in the format `sk-gw-<32 hex chars>`.
fn generate_api_key() -> String {
    format!(
        "sk-gw-{}",
        uuid::Uuid::new_v4().to_string().replace('-', "")
    )
}

/// Load gateway settings from the database, falling back to defaults.
fn load_settings(db: &db::Database) -> config::GatewaySettings {
    let listen_address = db
        .get_setting("listen_address")
        .ok()
        .flatten()
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let listen_port: u16 = db
        .get_setting("listen_port")
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(47339);
    let api_key = db
        .get_setting("gateway_api_key")
        .ok()
        .flatten()
        .unwrap_or_else(generate_api_key);
    let log_level = db
        .get_setting("log_level")
        .ok()
        .flatten()
        .unwrap_or_else(|| "info".to_string());

    config::GatewaySettings {
        listen_address,
        listen_port,
        api_key,
        log_level,
        ..Default::default()
    }
}

/// Load minimize-to-tray setting from the database, falling back to true (default).
pub fn load_minimize_to_tray(db: &db::Database) -> bool {
    db.get_setting("minimize_to_tray")
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(true)
}

/// Load rate limiter configuration from the settings table.
///
/// Settings keys:
/// - `rate_limit_enabled` (default: true)
/// - `rate_limit_max_requests` (default: 60)
/// - `rate_limit_window_seconds` (default: 60)
/// - `rate_limit_trust_xff` (default: false)
fn load_rate_limit_config(db: &db::Database) -> gateway::rate_limit::RateLimitConfig {
    let enabled = db
        .get_setting("rate_limit_enabled")
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(true);

    let max_requests = db
        .get_setting("rate_limit_max_requests")
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);

    let window_seconds = db
        .get_setting("rate_limit_window_seconds")
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);

    let trust_forwarded_for = db
        .get_setting("rate_limit_trust_xff")
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(false);

    gateway::rate_limit::RateLimitConfig {
        enabled,
        max_requests,
        window_seconds,
        trust_forwarded_for,
    }
}
