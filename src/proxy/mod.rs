pub mod client;
pub mod failover;
pub mod model_filter;

use crate::pool::round_robin::RoundRobinRouter;

/// Central proxy engine that coordinates:
/// - Round-robin upstream selection
/// - HTTP forwarding to upstream providers
/// - SSE stream passthrough
/// - Model name replacement
pub struct ProxyEngine {
    pub round_robin: RoundRobinRouter,
}

impl ProxyEngine {
    pub fn new() -> Self {
        Self {
            round_robin: RoundRobinRouter::new(),
        }
    }

    /// Register a pool with its upstream IDs.
    pub fn register_pool(&self, pool_id: &str, upstream_ids: Vec<String>) {
        self.round_robin.add_upstreams(pool_id, upstream_ids);
    }

    /// Remove a pool registration.
    pub fn unregister_pool(&self, pool_id: &str) {
        self.round_robin.remove_pool(pool_id);
    }

    /// Add an upstream to a pool.
    pub fn add_upstream_to_pool(&self, pool_id: &str, upstream_id: &str) {
        self.round_robin.add_upstream_to_pool(pool_id, upstream_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_engine_registration() {
        let engine = ProxyEngine::new();
        engine.register_pool("pool-1", vec!["upstream-a".to_string(), "upstream-b".to_string()]);
        
        assert_eq!(engine.round_robin.select_upstream("pool-1"), Some("upstream-a".to_string()));
        assert_eq!(engine.round_robin.select_upstream("pool-1"), Some("upstream-b".to_string()));
    }
}
