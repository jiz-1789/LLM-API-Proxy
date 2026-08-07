//! Shared test helpers for integration tests.
//!
//! Provides `TestEnv` which bundles all the dependencies needed
//! to run gateway integration tests: in-memory database, crypto key manager,
//! upstream client, rate limit config, and the axum router.

use std::sync::Arc;

use crate::crypto::KeyManager;
use crate::db::Database;
use crate::gateway;
use crate::gateway::rate_limit::RateLimitConfig;
use crate::proxy::failover::UpstreamClient;

/// Test API key used for gateway authentication in tests.
pub const TEST_API_KEY: &str = "sk-test-gateway-key-12345";

/// Test environment for gateway integration tests.
///
/// Holds references to the database, crypto, and router needed to
/// send test requests through the gateway. The wiremock mock server
/// is managed separately by each test.
pub struct TestEnv {
    pub db: Arc<Database>,
    pub crypto: Arc<KeyManager>,
    pub router: axum::Router,
    /// Keep temp dir alive so the key file persists for the test duration.
    _temp_dir: tempfile::TempDir,
}

impl TestEnv {
    /// Create a new test environment with an in-memory database and
    /// a real key manager backed by a temp directory.
    pub async fn new() -> Self {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let crypto = Arc::new(KeyManager::initialize(temp_dir.path()).unwrap());

        let db = Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        // Set the gateway API key for authentication
        db.save_setting("gateway_api_key", TEST_API_KEY).unwrap();

        let db = Arc::new(db);
        let proxy_client = Arc::new(UpstreamClient::new());

        // Disable rate limiting for tests
        let rate_limit_config = RateLimitConfig {
            enabled: false,
            ..Default::default()
        };

        let router = gateway::create_router(
            db.clone(),
            proxy_client,
            crypto.clone(),
            rate_limit_config,
        );

        Self {
            db,
            crypto,
            router,
            _temp_dir: temp_dir,
        }
    }

    /// Create an upstream in the database pointing to the given base URL.
    /// Returns the upstream ID.
    pub fn create_upstream(&self, provider_name: &str, base_url: &str) -> String {
        let id = format!("up_test_{}", uuid::Uuid::new_v4().simple());
        let encrypted = self.crypto.encrypt_api_key("sk-upstream-test-key").unwrap();
        self.db
            .create_upstream(
                &id,
                provider_name,
                base_url,
                &encrypted,
                "test-model",
                r#"["test-model"]"#,
                true,
                "",
            )
            .unwrap();
        id
    }

    /// Create a disabled upstream (enabled=false).
    pub fn create_disabled_upstream(&self, provider_name: &str, base_url: &str) -> String {
        let id = format!("up_test_{}", uuid::Uuid::new_v4().simple());
        let encrypted = self.crypto.encrypt_api_key("sk-upstream-test-key").unwrap();
        self.db
            .create_upstream(
                &id,
                provider_name,
                base_url,
                &encrypted,
                "test-model",
                r#"["test-model"]"#,
                false,
                "",
            )
            .unwrap();
        id
    }

    /// Create a pool with the given model name and associate upstreams.
    pub fn create_pool(&self, name: &str, display_name: &str, upstream_ids: &[String]) {
        let pool_id = format!("pool_test_{}", uuid::Uuid::new_v4().simple());
        self.db
            .create_pool(&pool_id, name, display_name, 5, false, "off", "")
            .unwrap();

        for (i, uid) in upstream_ids.iter().enumerate() {
            self.db
                .add_upstream_to_pool(&pool_id, uid, i as i32, "")
                .unwrap();
        }
    }

    /// Create a pool with failover disabled.
    pub fn create_pool_no_failover(&self, name: &str, display_name: &str, upstream_ids: &[String]) {
        let pool_id = format!("pool_test_{}", uuid::Uuid::new_v4().simple());
        self.db
            .create_pool(&pool_id, name, display_name, 5, false, "off", "")
            .unwrap();

        for (i, uid) in upstream_ids.iter().enumerate() {
            self.db
                .add_upstream_to_pool(&pool_id, uid, i as i32, "")
                .unwrap();
        }
    }

    /// Send a chat completion request through the gateway router.
    pub async fn send_chat_request(
        &self,
        model: &str,
        stream: bool,
    ) -> axum::response::Response {
        use tower::ServiceExt;
        let body = if stream {
            serde_json::json!({
                "model": model,
                "messages": [{"role": "user", "content": "hello"}],
                "stream": true
            })
        } else {
            serde_json::json!({
                "model": model,
                "messages": [{"role": "user", "content": "hello"}]
            })
        };

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", TEST_API_KEY))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        self.router.clone().oneshot(request).await.unwrap()
    }

    /// Send a chat completion request without authentication.
    pub async fn send_chat_request_no_auth(&self, model: &str) -> axum::response::Response {
        use tower::ServiceExt;
        let body = serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": "hello"}]
        });

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        self.router.clone().oneshot(request).await.unwrap()
    }

    /// Send a chat completion request without a model field.
    pub async fn send_chat_request_no_model(&self) -> axum::response::Response {
        use tower::ServiceExt;
        let body = serde_json::json!({
            "messages": [{"role": "user", "content": "hello"}]
        });

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", TEST_API_KEY))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        self.router.clone().oneshot(request).await.unwrap()
    }
}
