use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Gateway error: {0}")]
    Gateway(String),

    #[error("Upstream request failed: {0}")]
    UpstreamFailed(String),

    #[error("Authentication failed: invalid API key")]
    Unauthorized,

    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Encryption error: {0}")]
    Crypto(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP client error: {0}")]
    HttpClient(#[from] reqwest::Error),

    #[error("Invalid pool: {0}")]
    InvalidPool(String),

    #[error("All upstreams exhausted")]
    AllUpstreamsExhausted,

    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        AppError::Internal(err.to_string())
    }
}
