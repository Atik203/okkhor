#![forbid(unsafe_code)]
//! Smriti is Okkhor's local personalization engine.
//!
//! It learns from committed words, accepted suggestions, corrections, and
//! Romanized-to-Bangla choices. It never stores raw keystrokes and never
//! performs I/O; persistence is supplied by `okkhor-storage`.
//!
//! Keep this crate deterministic: callers pass timestamps in events and
//! requests, so tests and replay tools can reproduce the exact ranking.

mod config;
mod event;
mod store;

pub use config::{ConfigError, RankingWeights, SmritiConfig};
pub use event::{CommitSource, LearningEvent};
pub use store::{BigramStat, LearnError, LearningSnapshot, PairStat, Smriti, WordStat};
