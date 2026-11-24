#![forbid(unsafe_code)]
//! Core, platform-independent primitives for Okkhor.
//!
//! This crate must stay free of Windows APIs, I/O, and async runtimes so it can
//! be tested deterministically and reused by the daemon, UI, and benchmark
//! harness.

pub mod candidate;
pub mod language;
pub mod preedit;
pub mod text;

pub use candidate::{
    Candidate, CandidateProvider, CandidateSource, Ranker, RankingFeatures, Suggestion,
    SuggestionRequest,
};
pub use language::Language;
pub use preedit::{EditPlan, is_join_mark, plan_edit};
pub use text::{contains_bengali, is_bengali_char, normalize_lookup, normalize_nfc};
