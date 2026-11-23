use std::sync::Mutex;

use crate::breaker::{BreakerState, CircuitBreaker};
use crate::cache::{AiCache, AiCacheKey};
use crate::cancel::CancellationToken;
use crate::error::AiError;
use crate::flight::SingleFlight;
use crate::lock::lock;
use crate::policy::AiPolicy;
use crate::provider::AiProvider;
use crate::request::{AiRequest, AiResponse};

/// Orchestrates policy, caching, circuit breaking, single-flight admission, and
/// one provider call.
///
/// Order for [`Self::complete`]:
///
/// 1. validate the request against [`AiPolicy`];
/// 2. return a cached response when present;
/// 3. reject while the circuit breaker is open;
/// 4. admit through single-flight (duplicate key or cap full -> `Busy`);
/// 5. call the provider;
/// 6. cache a successful response and update breaker health.
///
/// No lock is held across the provider call, no prompt or response text is
/// logged, and the cache is memory-only.
pub struct AiService {
    policy: AiPolicy,
    provider: Box<dyn AiProvider + Send + Sync>,
    cache: Mutex<AiCache>,
    breaker: Mutex<CircuitBreaker>,
    in_flight: SingleFlight,
}

impl AiService {
    /// Build a service with the standard cache and breaker.
    pub fn new(policy: AiPolicy, provider: Box<dyn AiProvider + Send + Sync>) -> Self {
        Self::with_components(policy, provider, AiCache::new(), CircuitBreaker::new())
    }

    /// Build a service with explicit cache and breaker bounds.
    ///
    /// Useful for tests and for callers that already own configured components.
    pub fn with_components(
        policy: AiPolicy,
        provider: Box<dyn AiProvider + Send + Sync>,
        cache: AiCache,
        breaker: CircuitBreaker,
    ) -> Self {
        let in_flight = SingleFlight::with_max_in_flight(policy.max_in_flight);
        Self {
            policy,
            provider,
            cache: Mutex::new(cache),
            breaker: Mutex::new(breaker),
            in_flight,
        }
    }

    /// Provider name for status output. Never contains a key or prompt.
    pub fn provider_name(&self) -> &'static str {
        self.provider.name()
    }

    /// The configured policy.
    pub fn policy(&self) -> &AiPolicy {
        &self.policy
    }

    /// Number of cached responses.
    pub fn cache_len(&self) -> usize {
        lock(&self.cache).len()
    }

    /// Response-text bytes currently held by the cache.
    pub fn cache_bytes(&self) -> usize {
        lock(&self.cache).bytes()
    }

    /// Current circuit-breaker state for status output.
    pub fn breaker_state(&self) -> BreakerState {
        lock(&self.breaker).state()
    }

    /// Run one request through policy, cache, breaker, single-flight, and the
    /// provider.
    ///
    /// Returns [`AiError::Busy`] while the breaker is open or admission is full,
    /// and [`AiError::Cancelled`] when the token is cancelled before or during
    /// the call.
    pub fn complete(
        &self,
        request: &AiRequest,
        cancel: &CancellationToken,
    ) -> Result<AiResponse, AiError> {
        self.policy.validate(request)?;
        cancel.check()?;

        let key = AiCacheKey::from_request(request);

        let cached = lock(&self.cache).get(&key);
        if let Some(cached) = cached {
            return Ok(cached);
        }

        {
            let mut breaker = lock(&self.breaker);
            if !breaker.allow() {
                return Err(AiError::Busy);
            }
        }

        let _admission = match self.in_flight.try_acquire(key) {
            Some(guard) => guard,
            None => return Err(AiError::Busy),
        };

        cancel.check()?;

        match self.provider.complete(request, cancel) {
            Ok(response) => {
                if cancel.is_cancelled() {
                    lock(&self.breaker).record_success();
                    return Err(AiError::Cancelled);
                }
                lock(&self.cache).insert(key, response.clone());
                lock(&self.breaker).record_success();
                Ok(response)
            }
            Err(error) => {
                if error.counts_as_breaker_failure() {
                    lock(&self.breaker).record_failure();
                }
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::*;
    use crate::provider::{DisabledProvider, MockProvider};
    use crate::request::AiUsage;

    fn enabled_policy() -> AiPolicy {
        AiPolicy {
            enabled: true,
            ..AiPolicy::default()
        }
    }

    fn request(prompt: &str) -> AiRequest {
        AiRequest::new(prompt, "mock-model")
    }

    fn ok_response(request: &AiRequest) -> AiResponse {
        AiResponse {
            text: request.prompt.to_uppercase(),
            model: request.model.clone(),
            usage: AiUsage {
                input_tokens: 1,
                output_tokens: 1,
            },
            latency_ms: 0,
        }
    }

    struct CountingProvider {
        calls: Arc<AtomicUsize>,
    }

    impl AiProvider for CountingProvider {
        fn name(&self) -> &'static str {
            "counting"
        }

        fn complete(
            &self,
            request: &AiRequest,
            cancel: &CancellationToken,
        ) -> Result<AiResponse, AiError> {
            cancel.check()?;
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ok_response(request))
        }
    }

    struct FailingProvider;

    impl AiProvider for FailingProvider {
        fn name(&self) -> &'static str {
            "failing"
        }

        fn complete(
            &self,
            _request: &AiRequest,
            _cancel: &CancellationToken,
        ) -> Result<AiResponse, AiError> {
            Err(AiError::Provider { status: 500 })
        }
    }

    struct CancellingProvider;

    impl AiProvider for CancellingProvider {
        fn name(&self) -> &'static str {
            "cancelling"
        }

        fn complete(
            &self,
            request: &AiRequest,
            cancel: &CancellationToken,
        ) -> Result<AiResponse, AiError> {
            cancel.cancel();
            Ok(ok_response(request))
        }
    }

    struct GatedProvider {
        started: Mutex<mpsc::Sender<()>>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl AiProvider for GatedProvider {
        fn name(&self) -> &'static str {
            "gated"
        }

        fn complete(
            &self,
            request: &AiRequest,
            cancel: &CancellationToken,
        ) -> Result<AiResponse, AiError> {
            cancel.check()?;
            self.started
                .lock()
                .expect("started lock")
                .send(())
                .expect("signal started");
            self.release
                .lock()
                .expect("release lock")
                .recv()
                .expect("wait for release");
            Ok(ok_response(request))
        }
    }

    #[test]
    fn cache_hit_skips_the_second_provider_call() {
        let calls = Arc::new(AtomicUsize::new(0));
        let service = AiService::new(
            enabled_policy(),
            Box::new(CountingProvider {
                calls: Arc::clone(&calls),
            }),
        );
        let request = request("hello");
        let cancel = CancellationToken::new();

        let first = service.complete(&request, &cancel).expect("first call");
        let second = service.complete(&request, &cancel).expect("second call");

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(first.text, second.text);
        assert_eq!(service.cache_len(), 1);
    }

    #[test]
    fn policy_rejection_never_calls_the_provider() {
        let calls = Arc::new(AtomicUsize::new(0));
        let service = AiService::new(
            enabled_policy(),
            Box::new(CountingProvider {
                calls: Arc::clone(&calls),
            }),
        );
        let cancel = CancellationToken::new();

        assert_eq!(
            service.complete(&request("   "), &cancel),
            Err(AiError::InvalidRequest)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn disabled_policy_returns_disabled() {
        let service = AiService::new(AiPolicy::default(), Box::new(MockProvider));

        assert_eq!(
            service.complete(&request("hello"), &CancellationToken::new()),
            Err(AiError::Disabled)
        );
    }

    #[test]
    fn disabled_provider_returns_disabled() {
        let service = AiService::new(enabled_policy(), Box::new(DisabledProvider));

        assert_eq!(
            service.complete(&request("hello"), &CancellationToken::new()),
            Err(AiError::Disabled)
        );
        assert_eq!(service.provider_name(), "disabled");
    }

    #[test]
    fn open_breaker_rejects_with_busy() {
        let service = AiService::new(enabled_policy(), Box::new(FailingProvider));
        let request = request("hello");
        let cancel = CancellationToken::new();

        for _ in 0..3 {
            assert_eq!(
                service.complete(&request, &cancel),
                Err(AiError::Provider { status: 500 })
            );
        }

        assert_eq!(service.breaker_state(), BreakerState::Open);
        assert_eq!(service.complete(&request, &cancel), Err(AiError::Busy));
        assert_eq!(service.cache_len(), 0);
    }

    #[test]
    fn cancellation_before_the_call_skips_the_provider() {
        let calls = Arc::new(AtomicUsize::new(0));
        let service = AiService::new(
            enabled_policy(),
            Box::new(CountingProvider {
                calls: Arc::clone(&calls),
            }),
        );
        let cancel = CancellationToken::new();
        cancel.cancel();

        assert_eq!(
            service.complete(&request("hello"), &cancel),
            Err(AiError::Cancelled)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(service.cache_len(), 0);
    }

    #[test]
    fn cancellation_after_the_provider_call_discards_the_response() {
        let service = AiService::new(enabled_policy(), Box::new(CancellingProvider));
        let cancel = CancellationToken::new();

        assert_eq!(
            service.complete(&request("hello"), &cancel),
            Err(AiError::Cancelled)
        );
        assert!(cancel.is_cancelled());
        assert_eq!(service.cache_len(), 0);
    }

    #[test]
    fn duplicate_in_flight_request_returns_busy_and_then_completes() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let service = Arc::new(AiService::new(
            enabled_policy(),
            Box::new(GatedProvider {
                started: Mutex::new(started_tx),
                release: Mutex::new(release_rx),
            }),
        ));

        let worker = Arc::clone(&service);
        let worker_request = request("hello");
        let handle =
            thread::spawn(move || worker.complete(&worker_request, &CancellationToken::new()));

        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("provider started");

        assert_eq!(
            service.complete(&request("hello"), &CancellationToken::new()),
            Err(AiError::Busy)
        );

        release_tx.send(()).expect("release provider");
        let response = handle.join().expect("worker thread").expect("completion");
        assert_eq!(response.text, "HELLO");
        assert_eq!(service.cache_len(), 1);
    }
}
