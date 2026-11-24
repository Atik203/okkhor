//! Next-word ghost benchmark: the real merge path (Smriti + `OKNW` prior).

use std::time::Instant;

use okkhor_core::Language;
use okkhor_engine::nextword::{NextWordPrior, global, merge_next_words};
use okkhor_learn::Smriti;

use crate::stats::LatencySummary;

/// One language's results.
pub struct LanguageReport {
    pub language: &'static str,
    pub previous: usize,
    pub followers: usize,
    pub hits: usize,
    pub samples: usize,
    pub latency: LatencySummary,
}

/// Both languages plus the one-time parse cost.
pub struct NextWordReport {
    pub parse_ns: u64,
    pub bangla: LanguageReport,
    pub english: LanguageReport,
}

/// Measure the parse cost and the merge latency over deterministic samples of
/// real previous words from the embedded tables.
pub fn run(iterations: usize, warmup: usize) -> Result<NextWordReport, String> {
    let started = Instant::now();
    let bangla = global(Language::Bangla).ok_or("Bangla next-word prior failed to parse")?;
    let english = global(Language::English).ok_or("English next-word prior failed to parse")?;
    let parse_ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    let smriti = Smriti::new();

    Ok(NextWordReport {
        parse_ns,
        bangla: measure(
            "bangla",
            Language::Bangla,
            bangla,
            &smriti,
            iterations,
            warmup,
        ),
        english: measure(
            "english",
            Language::English,
            english,
            &smriti,
            iterations,
            warmup,
        ),
    })
}

fn measure(
    label: &'static str,
    language: Language,
    prior: &'static NextWordPrior,
    smriti: &Smriti,
    iterations: usize,
    warmup: usize,
) -> LanguageReport {
    let previous = prior.previous_count();
    let followers = prior.follower_count();
    let sample = |index: usize| {
        if previous == 0 {
            ""
        } else {
            prior
                .previous_at((index.wrapping_mul(7_919)) % previous)
                .unwrap_or("")
        }
    };

    for index in 0..warmup {
        let _ = merge_next_words(smriti, language, sample(index), 3);
    }

    let mut samples = Vec::with_capacity(iterations);
    let mut hits = 0usize;
    for index in 0..iterations {
        let word = sample(index);
        let started = Instant::now();
        let words = merge_next_words(smriti, language, word, 3);
        samples.push(started.elapsed());
        if !words.is_empty() {
            hits += 1;
        }
    }

    LanguageReport {
        language: label,
        previous,
        followers,
        hits,
        samples: samples.len(),
        latency: LatencySummary::from_samples(samples),
    }
}
