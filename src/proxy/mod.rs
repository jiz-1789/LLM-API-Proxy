pub mod client;
pub mod failover;
pub mod model_filter;

use crate::pool::round_robin::RoundRobinRouter;
use crate::pool::circuit_breaker::CircuitBreaker;
use std::sync::{Arc, Mutex};

/// Central proxy engine that coordinates:
/// - Round-robin upstream selection
/// - Circuit breaker health tracking
/// - HTTP forwarding to upstream providers
/// - SSE stream passthrough
/// - Model name replacement
pub struct ProxyEngine {
    pub round_robin: RoundRobinRouter,
    pub circuit_breaker: CircuitBreaker,
}

impl ProxyEngine {
    pub fn new() -> Self {
        Self {
            round_robin: RoundRobinRouter::new(),
            circuit_breaker: CircuitBreaker::new(),
        }
    }

    /// Register a pool with its upstream IDs.
    pub fn register_pool(&self, pool_id: &str, upstream_ids: Vec<String>) {
        self.round_robin.add_upstreams(pool_id, upstream_ids);
    }

    /// Remove a pool registration.
    pub fn unregister_pool(&self, pool_id: &str) {
        let mut states = self.round_robin.state.lock().unwrap();
        states.remove(pool_id);
    }

    /// Add an upstream to a pool.
    pub fn add_upstream_to_pool(&self, pool_id: &str, upstream_id: &str) {
        let mut states = self.round_robin.state.lock().unwrap();
        if let Some(state) = states.get_mut(pool_id) {
            if !state.upstreams.contains(upstream_id) {
                state.upstreams.push(upstream_id.to_string());
                state.enabled.push(true);
            }
        }
    }

    /// Record success/failure for circuit breaker.
    pub fn record_upstream_success(&self, upstream_id: &str) {
        self.circuit_breaker.record_success(upstream_id);
    }

    pub fn record_upstream_failure(&self, upstream_id: &str) {
        self.circuit_breaker.record_failure(upstream_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_engine_registration() {
        let engine = ProxyEngine::new();

        engine.register_pool("pool-1", vec!["upstream-a".to_string(), "upstream-b".to_string()]);
        
        assert_eq!(
            engine.round_robin.select_upstream("pool-1"),
            Some("upstream-a".to_string())
        );
        assert_eq!(
            engine.round_robin.select_upstream("pool-1"),
            Some("upstream-b".to_string())
        );
    }

    #[test]
    fn test_circuit_breaker_integration() {
        let engine = ProxyEngine::new();
        engine.register_pool("pool-1", vec!["u1".to_string()]);

        // All closed initially
        assert!(engine.circuit_breaker.allow_request("u1"));

        // Simulate failures
        for _ in 0..3 {
            engine.record_upstream_failure("u1");
        }

        // Should be open now and blocked
        assert!(!engine.circuit_breaker.allow_request("u1"));

        // Success resets
        engine.record_upstream_success("u1");
        assert!(engine.circuit_breaker.allow_request("u1"));
    }
}
