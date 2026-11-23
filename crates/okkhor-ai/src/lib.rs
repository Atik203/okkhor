#![forbid(unsafe_code)]
//! Opt-in AI orchestration core for Okkhor.
//!
//! This crate is deliberately independent of Win32, the daemon, the UI, and the
//! typing path. It provides the provider boundary, request/response types,
//! typed errors, an in-memory bounded cache, a circuit breaker, single-flight
//! admission, a cancellation token, and the [`AiService`] orchestrator described
//! by ADR 0018.
//!
//! Invariants:
//!
//! - no network, disk, or clock-dependent typing-path work;
//! - prompts, responses, and cache keys are never logged;
//! - the cache is memory-only and bounded by entry count and response bytes;
//! - every provider call is cancellable and admitted through policy.
//!
//! The live HTTP provider is intentionally not part of this slice. Callers use
//! [`MockProvider`] or [`DisabledProvider`] until the worker and provider
//! slices land.

mod breaker;
mod cache;
mod cancel;
mod error;
mod flight;
mod lock;
mod policy;
mod provider;
mod request;
mod service;

pub use breaker::{BreakerState, CircuitBreaker};
pub use cache::{AiCache, AiCacheKey};
pub use cancel::CancellationToken;
pub use error::AiError;
pub use flight::{SingleFlight, SingleFlightGuard};
pub use policy::AiPolicy;
pub use provider::{AiProvider, DisabledProvider, MockProvider};
pub use request::{AiRequest, AiResponse, AiUsage};
pub use service::AiService;
