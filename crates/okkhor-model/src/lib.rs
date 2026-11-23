#![forbid(unsafe_code)]
//! Tier 1 reranker runtime: a checked 16-feature logistic model and the
//! bounded hybrid blend from ADR 0017/0019.
//!
//! This crate is on the suggestion path, so it performs no I/O, reads no
//! clock, and is bounded structurally (<= 64 candidates x 16 features). The
//! bundled model is embedded at build time and checksum-verified in memory;
//! a missing or invalid model always falls back to the deterministic
//! `Smriti` baseline.

mod hybrid;
mod model;

pub use hybrid::{CONFIDENCE_CAP, HybridRanker, MAX_POOL, MODEL_WEIGHT, SmritiFeatureSource};
pub use model::{
    BUNDLED_MODEL_JSON, BUNDLED_MODEL_SHA256, FEATURE_SCHEMA, LogisticModel, MAX_MAGNITUDE,
    MODEL_SCHEMA, ModelError, NUM_FEATURES, load_bundled,
};
