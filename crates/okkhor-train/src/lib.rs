#![forbid(unsafe_code)]
//! Offline training support: feature schema v1, replay format v1, metrics,
//! and a deterministic synthetic replay generator.
//!
//! This crate is an offline developer tool. It is never linked into the typing
//! path, performs file I/O only in the `okkhor-replay` binary and tests, and
//! uses synthetic or permissively licensed fixtures only. No network, no user
//! data, no legacy execution.

pub mod features;
pub mod generate;
pub mod metrics;
pub mod replay;

pub use features::{FeatureError, FeatureVectorV1, SCHEMA_VERSION, extract_features};
pub use generate::{GenerationError, generate_cases, render_jsonl, sha256_hex};
pub use metrics::{
    LatencySummary, MetricError, RankedOutcome, ReplayMetrics, SegmentMetrics, compute_metrics,
    percentile_ns,
};
pub use replay::{REPLAY_SCHEMA, ReplayCase, ReplayError, read_replay_str};
