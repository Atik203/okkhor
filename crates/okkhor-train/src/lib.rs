#![forbid(unsafe_code)]
//! Offline training support: feature schema v1, replay format v1, dataset
//! manifest v1, metrics, a deterministic synthetic replay generator, and a
//! deterministic logistic trainer for the Tier 1 reranker.
//!
//! This crate is an offline developer tool. It is never linked into the typing
//! path, performs file I/O only in its binaries and tests, and uses synthetic
//! or permissively licensed fixtures only. No network, no user data, no legacy
//! execution.

pub mod features;
pub mod generate;
pub mod manifest;
pub mod metrics;
pub mod replay;
pub mod train;

pub use features::{FeatureError, FeatureVectorV1, SCHEMA_VERSION, extract_features};
pub use generate::{GenerationError, generate_cases, render_jsonl, sha256_hex};
pub use manifest::{
    DatasetManifest, MANIFEST_SCHEMA, ManifestError, Split, VerifiedManifest, verify_manifest,
};
pub use metrics::{
    LatencySummary, MetricError, RankedOutcome, ReplayMetrics, SegmentMetrics, compute_metrics,
    percentile_ns,
};
pub use replay::{REPLAY_SCHEMA, ReplayCase, ReplayError, read_replay_str};
pub use train::{
    DEFAULT_SEED, DEFAULT_TRAINED_AT, FEATURE_SCHEMA, MIN_CASES, MODEL_SCHEMA, TrainError,
    TrainMetrics, TrainOptions, TrainedModel, fit_replay,
};
