// LLM-API-Proxy Gateway Library

pub mod alert;
pub mod config;
pub mod config_io;
pub mod crypto;
pub mod db;
pub mod diagnostic;
pub mod error;
pub mod gateway;
pub mod pool;
pub mod probe;
pub mod proxy;
pub mod tool_config;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Shared application state available to all components.
#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<config::GatewaySettings>,
    pub db: Arc<db::Database>,
    pub crypto: Arc<crypto::KeyManager>,
    /// Tool switch manager for injecting proxy config into AI coding tools.
    pub tool_switch_manager: Arc<tool_config::ToolSwitchManager>,
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
        let db_for_tools = db.clone();
        Self {
            settings: Arc::new(settings),
            db,
            crypto,
            tool_switch_manager: Arc::new(tool_config::ToolSwitchManager::new(db_for_tools)),
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

    // Check for pending database restore (applied before opening the database)
    if db::backup::apply_pending_restore() {
        tracing::info!("Database restore applied, continuing with restored database");
    }

    // Open and initialize database with migrations
    let db = db::Database::open(&config::GatewaySettings::db_path())?;
    db.initialize()?;
    tracing::info!("Database initialized");

    // Initialize crypto key manager (once, shared everywhere)
    let crypto = Arc::new(crypto::KeyManager::initialize(&data_dir)?);
    tracing::info!("Crypto key manager ready");

    // Load settings from DB, falling back to defaults for missing keys.
    // Only persist defaults for keys that don't exist yet (first-run initialization).
    // This prevents overwriting user-saved values on restart.
    let settings = load_settings(&db);
    if db.get_setting("listen_address")?.is_none() {
        db.save_setting("listen_address", &settings.listen_address)?;
    }
    if db.get_setting("listen_port")?.is_none() {
        db.save_setting("listen_port", &settings.listen_port.to_string())?;
    }
    if db.get_setting("gateway_api_key")?.is_none() {
        db.save_setting("gateway_api_key", &settings.api_key)?;
    }

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
            let router = gateway::create_router(db_for_gw.clone(), proxy_client, crypto_for_gw.clone(), rate_limit_config);
            let addr = format!("{}:{}", listen_addr, listen_port);
            tracing::info!("Gateway server listening on {}", addr);
            let listener = match tokio::net::TcpListener::bind(&addr).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!("Failed to bind gateway port {}: {}", addr, e);
                    return;
                }
            };

            // Start background upstream probe task (if enabled)
            let probe_config = probe::load_probe_config(&db_for_gw);
            probe::start_probe_task(db_for_gw.clone(), crypto_for_gw, probe_config);

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

    // Start background auto-backup task (if enabled)
    db::backup::start_auto_backup_task(db_arc.clone());

    // Start background log cleanup task (periodic, every 30 minutes)
    start_log_cleanup_task(db_arc.clone());

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
fn load_rate_limit_config(db: &db::Database) -> gateway::rate_limit::RateLimitConfig {
    let settings = config::RateLimitSettings::load(db);
    gateway::rate_limit::RateLimitConfig {
        enabled: settings.enabled,
        max_requests: settings.max_requests,
        window_seconds: settings.window_seconds as u64,
        trust_forwarded_for: settings.trust_forwarded_for,
    }
}

/// Start a background task that periodically cleans up old request logs.
///
/// Runs every 30 minutes. Uses the configured retention policy
/// (`log_retention_days` and `log_max_entries` from the settings table).
/// This complements the startup cleanup in `Database::initialize()`,
/// ensuring long-running instances don't exceed log limits between restarts.
fn start_log_cleanup_task(db: Arc<db::Database>) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::warn!("Failed to create tokio runtime for log cleanup: {}", e);
                return;
            }
        };
        rt.block_on(async move {
            let interval = std::time::Duration::from_secs(1800); // 30 minutes
            loop {
                tokio::time::sleep(interval).await;
                let retention = config::LogRetentionSettings::load(&db);
                match db.cleanup_old_logs(retention.retention_days, retention.max_entries) {
                    Ok(deleted) if deleted > 0 => {
                        tracing::info!("Periodic log cleanup: removed {} old entries", deleted);
                    }
                    Ok(_) => {} // nothing to delete
                    Err(e) => {
                        tracing::warn!("Periodic log cleanup failed: {}", e);
                    }
                }
            }
        });
    });
}
