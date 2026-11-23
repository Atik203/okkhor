#![forbid(unsafe_code)]
//! Replay metrics: top-1, top-5, MRR, acceptance rate, keystroke savings,
//! per-language/per-layout segments, and nearest-rank latency.
//!
//! Ranking convention (mirrors the golden-corpus parity rule): a top-1 hit is
//! `ranked.first() == expected.first()`; a top-5 hit is any `expected` entry
//! in the first five ranked suggestions, compared on exact text. MRR uses the
//! rank of the first `expected` hit in the full ranked list (0 when absent).
//! Keystroke savings per case is `max(0, (expected_len - prefix_len) /
//! expected_len)` on a top-1 hit and `0` otherwise, averaged over cases;
//! lengths are in characters. Acceptance rate is the fraction of cases with
//! `accepted.is_some()`.

use std::collections::BTreeMap;
use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MetricError {
    #[error("no outcomes to score")]
    Empty,
}

/// One scored case: the expected answer, the ranker's ordered output, the
/// optional accepted index, prefix/expected lengths, and ranking latency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedOutcome {
    pub case_id: String,
    pub language: String,
    pub layout: String,
    pub expected: Vec<String>,
    pub ranked: Vec<String>,
    pub accepted: bool,
    pub prefix_len_chars: usize,
    pub expected_len_chars: usize,
    pub latency_ns: u64,
}

impl RankedOutcome {
    pub fn rank_of_first_hit(&self) -> Option<usize> {
        self.expected
            .iter()
            .filter_map(|want| self.ranked.iter().position(|got| got == want))
            .min()
            .map(|index| index + 1)
    }

    pub fn top1_hit(&self) -> bool {
        match (self.ranked.first(), self.expected.first()) {
            (Some(first), Some(want)) => first == want,
            _ => false,
        }
    }

    pub fn top5_hit(&self) -> bool {
        self.expected
            .iter()
            .any(|want| self.ranked.iter().take(5).any(|got| got == want))
    }

    pub fn reciprocal_rank(&self) -> f64 {
        self.rank_of_first_hit()
            .map_or(0.0, |rank| 1.0 / rank as f64)
    }

    pub fn keystroke_savings(&self) -> f64 {
        if !self.top1_hit() || self.expected_len_chars == 0 {
            return 0.0;
        }
        let saved = self
            .expected_len_chars
            .saturating_sub(self.prefix_len_chars.min(self.expected_len_chars))
            as f64
            / self.expected_len_chars as f64;
        saved.clamp(0.0, 1.0)
    }
}

/// Nearest-rank latency summary over sorted nanosecond samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatencySummary {
    pub count: usize,
    pub p50_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    pub max_ns: u64,
}

impl LatencySummary {
    pub fn from_nanos(mut samples: Vec<u64>) -> Self {
        samples.sort_unstable();
        Self {
            count: samples.len(),
            p50_ns: percentile_ns(&samples, 50).unwrap_or(0),
            p95_ns: percentile_ns(&samples, 95).unwrap_or(0),
            p99_ns: percentile_ns(&samples, 99).unwrap_or(0),
            max_ns: samples.last().copied().unwrap_or(0),
        }
    }

    pub fn from_durations(samples: &[Duration]) -> Self {
        Self::from_nanos(
            samples
                .iter()
                .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
                .collect(),
        )
    }

    pub fn is_monotonic(&self) -> bool {
        self.p50_ns <= self.p95_ns && self.p95_ns <= self.p99_ns && self.p99_ns <= self.max_ns
    }
}

/// Nearest-rank percentile over a sorted slice. `percent` is clamped to
/// `0..=100`; empty input returns `None`.
pub fn percentile_ns(sorted_nanos: &[u64], percent: u32) -> Option<u64> {
    if sorted_nanos.is_empty() {
        return None;
    }
    let percent = percent.min(100) as usize;
    let rank = (percent * sorted_nanos.len()).div_ceil(100).max(1);
    sorted_nanos.get(rank - 1).copied()
}

/// Aggregate metrics over a set of outcomes.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentMetrics {
    pub cases: usize,
    pub top1: f64,
    pub top5: f64,
    pub mrr: f64,
    pub acceptance_rate: f64,
    pub keystroke_savings: f64,
    pub latency: LatencySummary,
}

/// Full replay report: overall plus per-language and per-layout segments.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayMetrics {
    pub cases: usize,
    pub overall: SegmentMetrics,
    pub by_language: BTreeMap<String, SegmentMetrics>,
    pub by_layout: BTreeMap<String, SegmentMetrics>,
}

fn segment(outcomes: &[&RankedOutcome]) -> SegmentMetrics {
    let cases = outcomes.len();
    if cases == 0 {
        return SegmentMetrics {
            cases: 0,
            top1: 0.0,
            top5: 0.0,
            mrr: 0.0,
            acceptance_rate: 0.0,
            keystroke_savings: 0.0,
            latency: LatencySummary {
                count: 0,
                p50_ns: 0,
                p95_ns: 0,
                p99_ns: 0,
                max_ns: 0,
            },
        };
    }
    let top1 = outcomes.iter().filter(|o| o.top1_hit()).count() as f64 / cases as f64;
    let top5 = outcomes.iter().filter(|o| o.top5_hit()).count() as f64 / cases as f64;
    let mrr = outcomes.iter().map(|o| o.reciprocal_rank()).sum::<f64>() / cases as f64;
    let acceptance_rate = outcomes.iter().filter(|o| o.accepted).count() as f64 / cases as f64;
    let keystroke_savings =
        outcomes.iter().map(|o| o.keystroke_savings()).sum::<f64>() / cases as f64;
    let latency = LatencySummary::from_nanos(outcomes.iter().map(|o| o.latency_ns).collect());
    SegmentMetrics {
        cases,
        top1,
        top5,
        mrr,
        acceptance_rate,
        keystroke_savings,
        latency,
    }
}

/// Score a set of ranked outcomes. Fails closed on empty input.
pub fn compute_metrics(outcomes: &[RankedOutcome]) -> Result<ReplayMetrics, MetricError> {
    if outcomes.is_empty() {
        return Err(MetricError::Empty);
    }
    let all: Vec<&RankedOutcome> = outcomes.iter().collect();
    let overall = segment(&all);

    let mut by_language: BTreeMap<String, SegmentMetrics> = BTreeMap::new();
    for language in ["bangla", "english"] {
        let subset: Vec<&RankedOutcome> = all
            .iter()
            .filter(|o| o.language == language)
            .copied()
            .collect();
        if !subset.is_empty() {
            by_language.insert(language.to_owned(), segment(&subset));
        }
    }

    let mut layouts: Vec<String> = all.iter().map(|o| o.layout.clone()).collect();
    layouts.sort();
    layouts.dedup();
    let mut by_layout: BTreeMap<String, SegmentMetrics> = BTreeMap::new();
    for layout in layouts {
        let subset: Vec<&RankedOutcome> =
            all.iter().filter(|o| o.layout == layout).copied().collect();
        by_layout.insert(layout, segment(&subset));
    }

    Ok(ReplayMetrics {
        cases: outcomes.len(),
        overall,
        by_language,
        by_layout,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(id: &str, expected: &[&str], ranked: &[&str]) -> RankedOutcome {
        RankedOutcome {
            case_id: id.to_owned(),
            language: "bangla".to_owned(),
            layout: "avro".to_owned(),
            expected: expected.iter().map(|s| (*s).to_owned()).collect(),
            ranked: ranked.iter().map(|s| (*s).to_owned()).collect(),
            accepted: false,
            prefix_len_chars: 1,
            expected_len_chars: 4,
            latency_ns: 100,
        }
    }

    #[test]
    fn math_on_hand_built_fixture() {
        // 4 outcomes: top-1 hit (rank 1), hit at rank 2, hit at rank 6
        // (top-5 miss, MRR 1/6), and a complete miss.
        let outcomes = vec![
            outcome("a", &["x"], &["x", "y"]),
            outcome("b", &["y"], &["x", "y"]),
            outcome("c", &["z"], &["a", "b", "c", "d", "e", "z"]),
            outcome("d", &["w"], &["a", "b"]),
        ];
        let metrics = compute_metrics(&outcomes).expect("non-empty");
        assert_eq!(metrics.cases, 4);
        assert!((metrics.overall.top1 - 0.25).abs() < 1e-12);
        assert!((metrics.overall.top5 - 0.5).abs() < 1e-12);
        let want_mrr = (1.0 + 0.5 + 1.0 / 6.0 + 0.0) / 4.0;
        assert!((metrics.overall.mrr - want_mrr).abs() < 1e-12);
    }
}
