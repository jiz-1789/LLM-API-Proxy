use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Circuit Breaker for upstream health tracking.
pub struct CircuitBreaker {
    state: Arc<Mutex<HashMap<String, CBState>>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug)]
struct CBState {
    failure_count: u32,
    last_failure_time: Option<std::time::Instant>,
    state: State,
    threshold: u32,
    duration_secs: u64,
}

impl CircuitBreaker {
    pub fn new() -> Self {
        Self { state: Arc::new(Mutex::new(HashMap::new())) }
    }

    /// Register an upstream with the circuit breaker if it hasn't been
    /// registered yet. Does NOT reset existing state — safe to call on every
    /// request.
    pub fn ensure_registered(&self, upstream_id: &str, threshold: u32, duration_secs: u64) {
        let mut states = self.state.lock().unwrap();
        if !states.contains_key(upstream_id) {
            states.insert(upstream_id.to_string(), CBState {
                failure_count: 0,
                last_failure_time: None,
                state: State::Closed,
                threshold,
                duration_secs,
            });
        }
    }

    pub fn register(&self, upstream_id: &str, threshold: u32, duration_secs: u64) {
        let mut states = self.state.lock().unwrap();
        states.insert(upstream_id.to_string(), CBState {
            failure_count: 0,
            last_failure_time: None,
            state: State::Closed,
            threshold,
            duration_secs,
        });
    }

    pub fn allow_request(&self, upstream_id: &str) -> bool {
        let mut states = self.state.lock().unwrap();
        let Some(cb) = states.get_mut(upstream_id) else {
            return true; // no registration → allow
        };

        match cb.state {
            State::Closed => true,
            State::Open => {
                if let Some(last_fail) = cb.last_failure_time {
                    if last_fail.elapsed().as_secs() >= cb.duration_secs {
                        cb.state = State::HalfOpen;
                        return true;
                    }
                }
                false
            }
            State::HalfOpen => true,
        }
    }

    pub fn record_success(&self, upstream_id: &str) {
        let mut states = self.state.lock().unwrap();
        if let Some(cb) = states.get_mut(upstream_id) {
            cb.failure_count = 0;
            cb.state = State::Closed;
            cb.last_failure_time = None;
        }
    }

    pub fn record_failure(&self, upstream_id: &str) {
        let mut states = self.state.lock().unwrap();
        let Some(cb) = states.get_mut(upstream_id) else { return; };

        cb.failure_count += 1;
        cb.last_failure_time = Some(std::time::Instant::now());

        if cb.failure_count >= cb.threshold {
            cb.state = State::Open;
        } else if cb.state == State::HalfOpen {
            cb.state = State::Open;
        }
    }

    pub fn get_state(&self, upstream_id: &str) -> State {
        let states = self.state.lock().unwrap();
        states.get(upstream_id).map(|cb| cb.state.clone()).unwrap_or(State::Closed)
    }

    pub fn get_failure_count(&self, upstream_id: &str) -> u32 {
        let states = self.state.lock().unwrap();
        states.get(upstream_id).map(|cb| cb.failure_count).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state_is_closed() {
        let cb = CircuitBreaker::new();
        cb.register("u1", 3, 60);
        assert_eq!(cb.get_state("u1"), State::Closed);
        assert!(cb.allow_request("u1"));
    }

    #[test]
    fn test_circuit_opens_after_threshold() {
        let cb = CircuitBreaker::new();
        cb.register("u1", 3, 60);
        assert!(cb.allow_request("u1"));
        cb.record_failure("u1");
        cb.record_failure("u1");
        cb.record_failure("u1");
        assert_eq!(cb.get_state("u1"), State::Open);
        assert!(!cb.allow_request("u1"));
    }

    #[test]
    fn test_circuit_recovers_to_half_open() {
        let cb = CircuitBreaker::new();
        cb.register("u1", 2, 1);
        cb.record_failure("u1");
        cb.record_failure("u1");
        assert!(!cb.allow_request("u1"));
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(cb.allow_request("u1")); // half-open allows probe
    }

    #[test]
    fn test_success_resets_circuit() {
        let cb = CircuitBreaker::new();
        cb.register("u1", 3, 60);
        cb.record_failure("u1");
        cb.record_failure("u1");
        cb.record_success("u1");
        assert_eq!(cb.get_state("u1"), State::Closed);
        assert_eq!(cb.get_failure_count("u1"), 0);
    }
}
