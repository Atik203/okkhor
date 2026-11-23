use std::collections::HashSet;
use std::sync::Mutex;

use crate::cache::AiCacheKey;
use crate::lock::lock;

/// Admission control that allows at most one active operation per cache key.
///
/// Duplicate keys are rejected immediately with `None`; the service maps that
/// to [`crate::AiError::Busy`]. Keys are SHA-256 digests, so no user text is
/// retained. The guard releases its slot on drop, including on panic.
#[derive(Debug)]
pub struct SingleFlight {
    active: Mutex<HashSet<AiCacheKey>>,
    max_in_flight: usize,
}

/// RAII admission permit returned by [`SingleFlight::try_acquire`].
///
/// Dropping the guard removes the key from the active set.
#[derive(Debug)]
pub struct SingleFlightGuard<'a> {
    flight: &'a SingleFlight,
    key: AiCacheKey,
}

impl SingleFlight {
    /// Create admission control without a global in-flight cap.
    pub fn new() -> Self {
        Self::with_max_in_flight(usize::MAX)
    }

    /// Create admission control with a global in-flight cap.
    ///
    /// A cap of `0` is treated as `1` so a misconfiguration cannot deadlock the
    /// service silently.
    pub fn with_max_in_flight(max_in_flight: usize) -> Self {
        Self {
            active: Mutex::new(HashSet::new()),
            max_in_flight: max_in_flight.max(1),
        }
    }

    /// Try to admit an operation for `key`.
    ///
    /// Returns `None` when the key is already active or the global cap is full.
    pub fn try_acquire(&self, key: AiCacheKey) -> Option<SingleFlightGuard<'_>> {
        let mut active = lock(&self.active);
        if active.len() >= self.max_in_flight || active.contains(&key) {
            return None;
        }
        active.insert(key);
        drop(active);
        Some(SingleFlightGuard { flight: self, key })
    }

    /// Number of active operations.
    pub fn active(&self) -> usize {
        lock(&self.active).len()
    }

    /// Whether `key` is currently active.
    pub fn is_active(&self, key: &AiCacheKey) -> bool {
        lock(&self.active).contains(key)
    }
}

impl Default for SingleFlight {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SingleFlightGuard<'_> {
    fn drop(&mut self) {
        lock(&self.flight.active).remove(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::AiRequest;

    fn key(prompt: &str) -> AiCacheKey {
        AiCacheKey::from_request(&AiRequest::new(prompt, "mock-model"))
    }

    #[test]
    fn duplicate_key_is_rejected_until_guard_drops() {
        let flight = SingleFlight::new();
        let first_key = key("first");

        let guard = flight.try_acquire(first_key);
        assert!(guard.is_some());
        assert_eq!(flight.active(), 1);
        assert!(flight.is_active(&first_key));

        assert!(flight.try_acquire(first_key).is_none());
        assert_eq!(flight.active(), 1);

        drop(guard);
        assert_eq!(flight.active(), 0);
        assert!(!flight.is_active(&first_key));
        assert!(flight.try_acquire(first_key).is_some());
    }

    #[test]
    fn distinct_keys_can_be_active_together() {
        let flight = SingleFlight::new();
        let first = flight.try_acquire(key("first"));
        let second = flight.try_acquire(key("second"));

        assert!(first.is_some());
        assert!(second.is_some());
        assert_eq!(flight.active(), 2);
    }

    #[test]
    fn global_in_flight_cap_is_enforced() {
        let flight = SingleFlight::with_max_in_flight(1);
        let guard = flight.try_acquire(key("first"));

        assert!(guard.is_some());
        assert!(flight.try_acquire(key("second")).is_none());

        drop(guard);
        assert!(flight.try_acquire(key("second")).is_some());
    }
}
