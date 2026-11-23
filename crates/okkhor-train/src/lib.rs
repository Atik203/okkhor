#![forbid(unsafe_code)]
//! Offline training support: replay format v1, dataset manifest v1, metrics, a
//! deterministic synthetic replay generator, and a deterministic logistic
//! trainer for the Tier 1 reranker. Feature schema v1 itself lives in the
//! shipped `okkhor-features` crate and is re-exported here for replay and
//! training code.
//!
//! This crate is an offline developer tool. It is never linked into the typing
//! path, performs file I/O only in its binaries and tests, and uses synthetic
//! or permissively licensed fixtures only. No network, no user data, no legacy
//! execution.

pub mod generate;
pub mod manifest;
pub mod metrics;
pub mod replay;
pub mod train;

/// Re-export of the shipped feature schema so replay and training code keeps
/// its existing import path. The implementation lives in `okkhor-features`.
pub mod features {
    pub use okkhor_features::*;
}

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
