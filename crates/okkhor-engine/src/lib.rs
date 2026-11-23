#![forbid(unsafe_code)]
//! Local candidate generation for Okkhor.
//!
//! [`CandidateEngine::suggest`] composes the 0.3 skeleton crates into one
//! bounded, infallible query: dictionary prefix lookup, transliteration
//! application for Romanized input, merge + dedup, [`Smriti`] ranking, and
//! top-N truncation against per-language budgets. The query path performs no
//! I/O, reads no clock, and allocates only within the configured budgets.
//!
//! [`Smriti`]: okkhor_learn::Smriti

pub mod config;
pub mod engine;
pub mod error;
pub mod golden;
pub mod suggest;

pub use config::{EngineConfig, LanguageBudget};
pub use engine::CandidateEngine;
pub use error::EngineError;
pub use golden::{GoldenCase, GoldenCorpus, ParityReport, check_parity};
pub use suggest::{
    IndexCounts, IndexError, MAX_LIMIT, SuggestRequest, SuggestionEngine, SuggestionError,
    SuggestionIndex, UserWord,
};
