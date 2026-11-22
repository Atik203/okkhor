//! Latency samples and nearest-rank percentiles.
//!
//! The benchmark reports p50/p95/p99 and max instead of an average. The
//! percentile helper is pure and unit-tested, so CI can verify the reporting
//! logic without a desktop or a real injection target.

use std::time::Duration;

use serde::Serialize;

/// A duration summary in nanoseconds.
///
/// Percentiles use the nearest-rank method: the p-th percentile is the value at
/// `ceil(p / 100 * count)`, clamped to the last sample. This keeps results
/// reproducible and avoids interpolation assumptions for latency tails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LatencySummary {
    pub count: usize,
    pub p50_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    pub max_ns: u64,
    pub total_ns: u64,
}

impl LatencySummary {
    /// Build a summary from measured durations. Sorting happens once here so
    /// the report path never reorders samples.
    pub fn from_samples(samples: impl IntoIterator<Item = Duration>) -> Self {
        let mut nanos: Vec<u64> = samples.into_iter().map(duration_nanos).collect();
        nanos.sort_unstable();

        let total_ns = nanos
            .iter()
            .map(|value| u128::from(*value))
            .sum::<u128>()
            .min(u128::from(u64::MAX)) as u64;

        Self {
            count: nanos.len(),
            p50_ns: percentile_ns(&nanos, 50).unwrap_or(0),
            p95_ns: percentile_ns(&nanos, 95).unwrap_or(0),
            p99_ns: percentile_ns(&nanos, 99).unwrap_or(0),
            max_ns: nanos.last().copied().unwrap_or(0),
            total_ns,
        }
    }

    /// Samples per second, computed from the measured time only.
    ///
    /// Warmup and candidate cloning are deliberately excluded.
    pub fn iterations_per_second(&self) -> f64 {
        if self.total_ns == 0 || self.count == 0 {
            0.0
        } else {
            self.count as f64 * 1_000_000_000.0 / self.total_ns as f64
        }
    }

    /// Whether the reported tail values are ordered as a caller expects.
    pub fn is_monotonic(&self) -> bool {
        self.p50_ns <= self.p95_ns && self.p95_ns <= self.p99_ns && self.p99_ns <= self.max_ns
    }
}

/// Nearest-rank percentile over a sorted slice of nanosecond samples.
///
/// `percent` is clamped to `0..=100`. An empty slice returns `None`.
pub fn percentile_ns(sorted_nanos: &[u64], percent: u32) -> Option<u64> {
    if sorted_nanos.is_empty() {
        return None;
    }

    let percent = percent.min(100) as usize;
    let rank = (percent * sorted_nanos.len()).div_ceil(100).max(1);
    sorted_nanos.get(rank - 1).copied()
}

fn duration_nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{LatencySummary, percentile_ns};

    fn nanos(values: &[u64]) -> Vec<u64> {
        let mut values = values.to_vec();
        values.sort_unstable();
        values
    }

    #[test]
    fn nearest_rank_percentiles_are_exact() {
        let samples = nanos(&(1..=100).collect::<Vec<_>>());
        assert_eq!(percentile_ns(&samples, 0), Some(1));
        assert_eq!(percentile_ns(&samples, 50), Some(50));
        assert_eq!(percentile_ns(&samples, 95), Some(95));
        assert_eq!(percentile_ns(&samples, 99), Some(99));
        assert_eq!(percentile_ns(&samples, 100), Some(100));
    }

    #[test]
    fn small_sets_clamp_to_available_samples() {
        assert_eq!(percentile_ns(&[10], 50), Some(10));
        assert_eq!(percentile_ns(&[10], 99), Some(10));
        assert_eq!(percentile_ns(&[10, 20], 50), Some(10));
        assert_eq!(percentile_ns(&[10, 20], 95), Some(20));
        assert_eq!(percentile_ns(&[], 50), None);
    }

    #[test]
    fn percentiles_are_monotonic_for_every_length() {
        for length in 0..=128 {
            let samples = nanos(&(0..length as u64).collect::<Vec<_>>());
            let summary = LatencySummary::from_samples(
                samples.iter().map(|value| Duration::from_nanos(*value)),
            );
            assert_eq!(summary.count, length as usize);
            assert!(summary.is_monotonic(), "length {length}");
            if length > 0 {
                assert_eq!(summary.max_ns, length as u64 - 1);
            }
        }
    }

    #[test]
    fn summary_counts_and_total_are_consistent() {
        let summary = LatencySummary::from_samples([
            Duration::from_nanos(30),
            Duration::from_nanos(10),
            Duration::from_nanos(20),
        ]);
        assert_eq!(summary.count, 3);
        assert_eq!(summary.total_ns, 60);
        assert_eq!(summary.p50_ns, 20);
        assert_eq!(summary.p95_ns, 30);
        assert_eq!(summary.p99_ns, 30);
        assert_eq!(summary.max_ns, 30);
        assert!((summary.iterations_per_second() - 50_000_000.0).abs() < 0.001);
    }

    #[test]
    fn empty_summary_is_zeroed() {
        let summary = LatencySummary::from_samples(Vec::<Duration>::new());
        assert_eq!(summary.count, 0);
        assert_eq!(summary.max_ns, 0);
        assert_eq!(summary.total_ns, 0);
        assert_eq!(summary.iterations_per_second(), 0.0);
        assert!(summary.is_monotonic());
    }
}
