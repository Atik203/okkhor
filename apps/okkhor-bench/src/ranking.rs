//! Ranking microbenchmark: N measured `Smriti::rank` calls.

use std::hint::black_box;
use std::time::Instant;

use okkhor_core::Ranker;

use crate::corpus::SyntheticCorpus;
use crate::stats::LatencySummary;

/// Measured output of one ranking benchmark run.
#[derive(Debug, Clone)]
pub struct RankingRun {
    pub iterations: usize,
    pub warmup_iterations: usize,
    pub candidates_per_iteration: usize,
    pub english_cases: usize,
    pub bangla_cases: usize,
    pub store_words: usize,
    pub store_pairs: usize,
    pub store_bigrams: usize,
    pub ranked_total: usize,
    pub latency: LatencySummary,
}

/// Run `iterations` measured calls after `warmup` untimed calls.
///
/// Candidate cloning happens before each timed section so the sample reflects
/// ranking work, not corpus setup.
pub fn run(corpus: &SyntheticCorpus, iterations: usize, warmup: usize) -> RankingRun {
    assert!(!corpus.cases.is_empty(), "corpus must contain cases");
    let case_count = corpus.cases.len();

    for index in 0..warmup {
        let case = &corpus.cases[index % case_count];
        let ranked = corpus.engine.rank(&case.request, case.candidates.clone());
        black_box(&ranked);
    }

    let mut samples = Vec::with_capacity(iterations);
    let mut ranked_total = 0;
    for index in 0..iterations {
        let case = &corpus.cases[(warmup + index) % case_count];
        let candidates = case.candidates.clone();
        let started = Instant::now();
        let ranked = corpus.engine.rank(&case.request, candidates);
        let elapsed = started.elapsed();
        black_box(&ranked);
        ranked_total += ranked.len();
        samples.push(elapsed);
    }

    RankingRun {
        iterations,
        warmup_iterations: warmup,
        candidates_per_iteration: corpus.candidates_per_case,
        english_cases: corpus.english_cases,
        bangla_cases: corpus.bangla_cases,
        store_words: corpus.engine.word_count(),
        store_pairs: corpus.engine.pair_count(),
        store_bigrams: corpus.engine.bigram_count(),
        ranked_total,
        latency: LatencySummary::from_samples(samples),
    }
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::corpus::SyntheticCorpus;

    #[test]
    fn ranking_smoke_is_bounded_and_monotonic() {
        let corpus = SyntheticCorpus::build(5);
        let result = run(&corpus, 16, 4);
        assert_eq!(result.latency.count, 16);
        assert!(result.latency.is_monotonic());
        assert!(result.ranked_total > 0);
        assert!(result.ranked_total <= 16 * 20);
        assert_eq!(result.store_words, corpus.engine.word_count());
    }
}
