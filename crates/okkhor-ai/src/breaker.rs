use std::time::{Duration, Instant};

/// Observable circuit-breaker state.
///
/// Reported through `ai.status` without any request text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    /// Requests flow normally.
    Closed,
    /// Requests are rejected until the cooldown elapses.
    Open,
    /// The cooldown elapsed; one probe is allowed.
    HalfOpen,
}

/// Circuit breaker around provider health.
///
/// Opens after a configurable number of consecutive failures (default 3) and
/// stays open for a cooldown (default 30 s). After the cooldown, the next
/// [`Self::allow`] transitions to [`BreakerState::HalfOpen`]; a success closes
/// the breaker and a failure reopens it.
///
/// This crate is not on the typing path, so using [`Instant`] is acceptable
/// here (unlike the clock-free ranking engine).
#[derive(Debug)]
pub struct CircuitBreaker {
    failure_threshold: u32,
    cooldown: Duration,
    state: BreakerState,
    failures: u32,
    opened_at: Option<Instant>,
}

impl CircuitBreaker {
    /// Defaults: open after 3 failures, 30 s cooldown.
    pub fn new() -> Self {
        Self::with_config(3, Duration::from_secs(30))
    }

    /// Create a breaker with an explicit threshold and cooldown.
    ///
    /// A zero threshold is treated as `1`.
    pub fn with_config(failure_threshold: u32, cooldown: Duration) -> Self {
        Self {
            failure_threshold: failure_threshold.max(1),
            cooldown,
            state: BreakerState::Closed,
            failures: 0,
            opened_at: None,
        }
    }

    /// Whether a request may proceed.
    ///
    /// In `Open`, this returns `false` until the cooldown elapses; the first
    /// call after the cooldown moves the breaker to `HalfOpen` and returns
    /// `true`.
    pub fn allow(&mut self) -> bool {
        match self.state() {
            BreakerState::Closed => true,
            BreakerState::Open => false,
            BreakerState::HalfOpen => {
                self.state = BreakerState::HalfOpen;
                true
            }
        }
    }

    /// Record a successful provider call and close the breaker.
    pub fn record_success(&mut self) {
        self.state = BreakerState::Closed;
        self.failures = 0;
        self.opened_at = None;
    }

    /// Record a provider failure.
    ///
    /// Consecutive `Closed` failures trip the breaker at the threshold. A
    /// failure during `HalfOpen` immediately reopens it.
    pub fn record_failure(&mut self) {
        match self.state() {
            BreakerState::Closed => {
                self.failures = self.failures.saturating_add(1);
                if self.failures >= self.failure_threshold {
                    self.trip();
                }
            }
            BreakerState::HalfOpen => self.trip(),
            BreakerState::Open => {}
        }
    }

    /// Current state. If the cooldown has elapsed, an open breaker reports
    /// `HalfOpen` even before [`Self::allow`] performs the transition.
    pub fn state(&self) -> BreakerState {
        match self.state {
            BreakerState::Open if self.cooldown_elapsed() => BreakerState::HalfOpen,
            other => other,
        }
    }

    /// Consecutive failures recorded since the last success.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    fn trip(&mut self) {
        self.state = BreakerState::Open;
        self.opened_at = Some(Instant::now());
    }

    fn cooldown_elapsed(&self) -> bool {
        self.opened_at
            .is_none_or(|opened_at| opened_at.elapsed() >= self.cooldown)
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    fn short_cooldown_breaker() -> CircuitBreaker {
        CircuitBreaker::with_config(2, Duration::from_millis(10))
    }

    #[test]
    fn starts_closed_and_allows() {
        let mut breaker = CircuitBreaker::new();
        assert_eq!(breaker.state(), BreakerState::Closed);
        assert!(breaker.allow());
    }

    #[test]
    fn opens_after_threshold_failures() {
        let mut breaker = CircuitBreaker::with_config(3, Duration::from_secs(60));

        breaker.record_failure();
        breaker.record_failure();
        assert_eq!(breaker.state(), BreakerState::Closed);
        assert!(breaker.allow());

        breaker.record_failure();
        assert_eq!(breaker.state(), BreakerState::Open);
        assert!(!breaker.allow());
        assert_eq!(breaker.failures(), 3);
    }

    #[test]
    fn default_threshold_is_three() {
        let mut breaker = CircuitBreaker::new();
        breaker.record_failure();
        breaker.record_failure();
        assert_eq!(breaker.state(), BreakerState::Closed);
        breaker.record_failure();
        assert_eq!(breaker.state(), BreakerState::Open);
    }

    #[test]
    fn half_open_after_cooldown_and_success_closes() {
        let mut breaker = short_cooldown_breaker();

        breaker.record_failure();
        breaker.record_failure();
        assert_eq!(breaker.state(), BreakerState::Open);
        assert!(!breaker.allow());

        thread::sleep(Duration::from_millis(25));
        assert_eq!(breaker.state(), BreakerState::HalfOpen);
        assert!(breaker.allow());
        assert_eq!(breaker.state(), BreakerState::HalfOpen);

        breaker.record_success();
        assert_eq!(breaker.state(), BreakerState::Closed);
        assert_eq!(breaker.failures(), 0);
        assert!(breaker.allow());
    }

    #[test]
    fn failure_in_half_open_reopens() {
        let mut breaker = short_cooldown_breaker();
        breaker.record_failure();
        breaker.record_failure();
        thread::sleep(Duration::from_millis(25));

        assert!(breaker.allow());
        breaker.record_failure();

        assert_eq!(breaker.state(), BreakerState::Open);
        assert!(!breaker.allow());
    }

    #[test]
    fn success_resets_the_failure_count() {
        let mut breaker = CircuitBreaker::with_config(3, Duration::from_secs(60));
        breaker.record_failure();
        breaker.record_failure();
        breaker.record_success();
        assert_eq!(breaker.failures(), 0);

        breaker.record_failure();
        breaker.record_failure();
        assert_eq!(breaker.state(), BreakerState::Closed);
    }
}
