use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Circuit Breaker for upstream health tracking.
/// 
/// States: CLOSED (healthy) → OPEN (circuit broken) → HALF_OPEN (probing) → CLOSED/OPEN
pub struct CircuitBreaker {
    state: Arc<Mutex<HashMap<String, CBState>>>,
}

#[derive(Debug, Clone, PartialEq)]
enum State {
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
        Self {
            state: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a circuit breaker for an upstream.
    pub fn register(
        &self,
        upstream_id: &str,
        threshold: u32,
        duration_secs: u64,
    ) {
        let mut states = self.state.lock().unwrap();
        states.insert(
            upstream_id.to_string(),
            CBState {
                failure_count: 0,
                last_failure_time: None,
                state: State::Closed,
                threshold,
                duration_secs,
            },
        );
    }

    /// Check if an upstream is allowed to receive requests.
    pub fn allow_request(&self, upstream_id: &str) -> bool {
        let mut states = self.state.lock().unwrap();
        let Some(cb) = states.get_mut(upstream_id) else {
            return true; // no registration → allow
        };

        match cb.state {
            State::Closed => true,
            State::Open => {
                // Check if cooldown has elapsed → transition to half-open
                if let Some(last_fail) = cb.last_failure_time {
                    if last_fail.elapsed().as_secs() >= cb.duration_secs {
                        cb.state = State::HalfOpen;
                        return true;
                    }
                }
                false
            }
            State::HalfOpen => true, // allow one probe request
        }
    }

    /// Record a successful request → reset circuit breaker if in half-open.
    pub fn record_success(&self, upstream_id: &str) {
        let mut states = self.state.lock().unwrap();
        if let Some(cb) = states.get_mut(upstream_id) {
            cb.failure_count = 0;
            cb.state = State::Closed;
            cb.last_failure_time = None;
        }
    }

    /// Record a failed request → may trigger OPEN state.
    pub fn record_failure(&self, upstream_id: &str) {
        let mut states = self.state.lock().unwrap();
        let Some(cb) = states.get_mut(upstream_id) else {
            return;
        };

        cb.failure_count += 1;
        cb.last_failure_time = Some(std::time::Instant::now());

        if cb.failure_count >= cb.threshold {
            cb.state = State::Open;
        } else if cb.state == State::HalfOpen {
            // Probe also failed → go back to open
            cb.state = State::Open;
        }
    }

    /// Get current state for monitoring.
    pub fn get_state(&self, upstream_id: &str) -> State {
        let states = self.state.lock().unwrap();
        states
            .get(upstream_id)
            .map(|cb| cb.state.clone())
            .unwrap_or(State::Closed)
    }

    /// Get failure count for monitoring.
    pub fn get_failure_count(&self, upstream_id: &str) -> u32 {
        let states = self.state.lock().unwrap();
        states
            .get(upstream_id)
            .map(|cb| cb.failure_count)
            .unwrap_or(0)
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

        assert!(cb.allow_request("u1")); // closed
        cb.record_failure("u1");
        cb.record_failure("u1");
        cb.record_failure("u1"); // triggers OPEN

        assert_eq!(cb.get_state("u1"), State::Open);
        assert!(!cb.allow_request("u1")); // should be blocked
    }

    #[test]
    fn test_circuit_recovers_to_half_open() {
        let cb = CircuitBreaker::new();
        cb.register("u1", 2, 1); // 1 second cooldown

        cb.record_failure("u1");
        cb.record_failure("u1");
        assert!(!cb.allow_request("u1"));

        // Wait for cooldown
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
