pub mod error;
pub mod failover;
pub mod model_filter;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_modules_accessible() {
        // Smoke test: ensure proxy submodules are accessible
        let _ = error::UpstreamError::ConnectionFailed {
            detail: "test".to_string(),
        };
    }
}
