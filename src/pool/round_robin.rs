use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Round-robin load balancer for pool upstream selection.
/// 
/// Maintains a per-pool counter that advances on each call to `select_upstream`.
/// Thread-safe via internal Arc<Mutex<>>.
pub struct RoundRobinRouter {
    state: Arc<Mutex<HashMap<String, PoolState>>>,
}

#[derive(Debug)]
struct PoolState {
    upstreams: Vec<String>,
    enabled: Vec<bool>,
    index: usize,
}

impl RoundRobinRouter {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register upstreams for a pool.
    pub fn add_upstreams(&self, pool_id: &str, upstreams: Vec<String>) {
        let mut states = self.state.lock().unwrap();
        states.insert(
            pool_id.to_string(),
            PoolState {
                upstreams,
                enabled: vec![true; 10], // placeholder, will be updated later
                index: 0,
            },
        );
    }

    /// Select the next enabled upstream for a pool using round-robin.
    pub fn select_upstream(&self, pool_id: &str) -> Option<String> {
        let mut states = self.state.lock().unwrap();
        let state = states.get_mut(pool_id)?;

        if state.upstreams.is_empty() {
            return None;
        }

        let n = state.upstreams.len();
        let start_idx = state.index;

        for i in 0..n {
            let idx = (start_idx + i) % n;
            if state.enabled[idx] {
                state.index = (idx + 1) % n;
                return Some(state.upstreams[idx].clone());
            }
        }

        None
    }

    /// Update enabled status of an upstream within a pool.
    pub fn set_upstream_enabled(&self, pool_id: &str, upstream_id: &str, enabled: bool) {
        let mut states = self.state.lock().unwrap();
        if let Some(state) = states.get_mut(pool_id) {
            if let Some(pos) = state.upstreams.iter().position(|u| u == upstream_id) {
                state.enabled[pos] = enabled;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_robin_cycles_through_upstreams() {
        let router = RoundRobinRouter::new();
        let pool = "pool-1";

        router.add_upstreams(
            pool,
            vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
            ],
        );

        assert_eq!(router.select_upstream(pool), Some("a".to_string()));
        assert_eq!(router.select_upstream(pool), Some("b".to_string()));
        assert_eq!(router.select_upstream(pool), Some("c".to_string()));
        assert_eq!(router.select_upstream(pool), Some("a".to_string()));
    }

    #[test]
    fn test_skips_disabled_upstreams() {
        let router = RoundRobinRouter::new();
        let pool = "pool-2";

        router.add_upstreams(
            pool,
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
        );

        // Disable "b"
        router.set_upstream_enabled(pool, "b", false);

        assert_eq!(router.select_upstream(pool), Some("a".to_string()));
        assert_eq!(router.select_upstream(pool), Some("c".to_string()));
        assert_eq!(router.select_upstream(pool), Some("a".to_string()));
    }

    #[test]
    fn test_empty_pool_returns_none() {
        let router = RoundRobinRouter::new();
        assert_eq!(router.select_upstream("nonexistent"), None);
    }
}
