pub mod circuit_breaker;
pub mod round_robin;
pub mod thinking;

/// Trait for pool-level routing decisions.
pub trait PoolRouter {
    /// Select the next upstream to use for a given pool.
    fn select_upstream(&mut self, pool_id: &str) -> Option<String>;

    /// Record that an upstream succeeded (for round-robin progress).
    fn record_success(&mut self, pool_id: &str, upstream_id: &str);

    /// Record that an upstream failed (may trigger circuit breaker).
    fn record_failure(&mut self, pool_id: &str, upstream_id: &str, error: &str);
}

#[cfg(test)]
mod tests {
    use crate::pool::round_robin::RoundRobinRouter;

    #[test]
    fn test_round_robin_selects_sequentially() {
        let router = RoundRobinRouter::new();
        let pool = "test-pool";
        router.add_upstreams(pool, vec!["upstream-a".to_string(), "upstream-b".to_string(), "upstream-c".to_string()]);

        assert_eq!(router.select_upstream(pool), Some("upstream-a".to_string()));
        assert_eq!(router.select_upstream(pool), Some("upstream-b".to_string()));
        assert_eq!(router.select_upstream(pool), Some("upstream-c".to_string()));
        assert_eq!(router.select_upstream(pool), Some("upstream-a".to_string())); // wraps around
    }

    #[test]
    fn test_round_robin_skips_disabled_upstreams() {
        let router = RoundRobinRouter::new();
        let pool = "test-pool";
        router.add_upstreams(pool, vec!["upstream-a".to_string(), "upstream-b".to_string()]);

        // Simulate upstream-a being in circuit breaker
        router.record_failure(pool, "upstream-a", "timeout");
        router.record_failure(pool, "upstream-a", "timeout");
        router.record_failure(pool, "upstream-a", "timeout");

        // Now only upstream-b should be selected
        assert_eq!(router.select_upstream(pool), Some("upstream-b".to_string()));
    }
}
