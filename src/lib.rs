// LLM-API-Proxy Gateway Library

pub mod config;
pub mod crypto;
pub mod db;
pub mod error;
pub mod gateway;
pub mod pool;
pub mod proxy;

use std::sync::Arc;

/// Shared application state available to all components.
#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<config::GatewaySettings>,
    pub db: Arc<db::Database>,
    pub crypto: Arc<crypto::KeyManager>,
}

impl AppState {
    pub fn new(
        settings: config::GatewaySettings,
        db: db::Database,
        crypto: crypto::KeyManager,
    ) -> Self {
        Self {
            settings: Arc::new(settings),
            db: Arc::new(db),
            crypto: Arc::new(crypto),
        }
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

    // Initialize crypto key manager
    let _crypto = crypto::KeyManager::initialize(&data_dir)?;
    tracing::info!("Crypto key manager ready");

    // Load or create default settings
    let settings = load_settings(&db);
    db.save_setting("listen_port", &settings.listen_port.to_string())?;
    db.save_setting("listen_address", &settings.listen_address)?;

    // Start HTTP gateway server on a background thread
    let listen_addr = settings.listen_address.clone();
    let listen_port = settings.listen_port;
    let db_arc = Arc::new(db);
    let proxy_client = Arc::new(proxy::failover::UpstreamClient::new());
    let crypto_for_gw = Arc::new(crypto::KeyManager::initialize(&data_dir)?);

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(async move {
            let router = gateway::create_router(db_arc.clone(), proxy_client, crypto_for_gw);
            let addr = format!("{}:{}", listen_addr, listen_port);
            tracing::info!("Gateway server listening on {}", addr);
            let listener = tokio::net::TcpListener::bind(&addr)
                .await
                .expect("failed to bind gateway port");
            axum::serve(listener, router)
                .await
                .expect("gateway server error");
        });
    });

    // Re-extract db from Arc for AppState (we need the inner db)
    // Since Arc<Database> doesn't easily give us Database back, we'll
    // reconstruct it. For now, we open a second connection for AppState.
    // In a future refactor, AppState should hold Arc<Database> directly.
    let db2 = db::Database::open(&config::GatewaySettings::db_path())?;

    let crypto2 = crypto::KeyManager::initialize(&data_dir)?;
    let state = AppState::new(settings, db2, crypto2);
    Ok(state)
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
        .unwrap_or_else(|| "sk-gateway-key".to_string());
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
