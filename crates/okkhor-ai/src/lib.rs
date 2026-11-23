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
//! The live HTTP provider (`OpenAiProvider`) is behind the non-default `http`
//! Cargo feature so the default build stays offline. Callers use
//! [`MockProvider`] or [`DisabledProvider`] otherwise; the worker and daemon
//! wiring land in later slices.

mod breaker;
mod cache;
mod cancel;
mod error;
mod flight;
mod lock;
#[cfg(feature = "http")]
mod openai;
mod policy;
mod provider;
mod request;
mod service;

pub use breaker::{BreakerState, CircuitBreaker};
pub use cache::{AiCache, AiCacheKey};
pub use cancel::CancellationToken;
pub use error::AiError;
pub use flight::{SingleFlight, SingleFlightGuard};
#[cfg(feature = "http")]
pub use openai::{OpenAiConfig, OpenAiProvider};
pub use policy::AiPolicy;
pub use provider::{AiProvider, DisabledProvider, MockProvider};
pub use request::{AiRequest, AiResponse, AiUsage};
pub use service::AiService;
