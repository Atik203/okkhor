//! Alias parity: does the engine return the mapping the licensed table prefers?
//!
//! The suggestion index carries every roman alias with its authored targets, in
//! preferred order. This check samples those rows, asks the live engine for its
//! top row, and compares. A mismatch is not a spelling opinion: it means some
//! other candidate outranked the alias the data itself marks as preferred.
//!
//! The expected value is authoritative in this order:
//! 1. the shipped overlay (our explicit correction),
//! 2. the curated table,
//! 3. the alias's rank-0 target.
//!
//! Sampling is deterministic (every `step`-th row), so two runs are comparable.

use okkhor_engine::{SuggestRequest, SuggestionEngine};
use okkhor_learn::Smriti;

/// One sampled mismatch.
#[derive(Debug, Clone)]
pub struct Miss {
    pub roman: String,
    pub expected: String,
    pub got: String,
}

/// Result of one parity run.
#[derive(Debug, Clone, Default)]
pub struct ParityReport {
    pub checked: usize,
    pub matched: usize,
    pub misses: Vec<Miss>,
}

impl ParityReport {
    pub fn percent(&self) -> f64 {
        if self.checked == 0 {
            0.0
        } else {
            self.matched as f64 * 100.0 / self.checked as f64
        }
    }
}

/// Sample up to `sample` alias rows and compare the engine's top row.
pub fn run(sample: usize) -> Result<ParityReport, String> {
    let sample = sample.max(1);
    let engine = SuggestionEngine::from_embedded().map_err(|error| error.to_string())?;
    let index = engine
        .index()
        .ok_or_else(|| "suggestion index unavailable".to_owned())?;
    let total = index.roman_count();
    let step = (total / sample).max(1);
    let smriti = Smriti::new();

    let mut report = ParityReport::default();
    let mut position = 0usize;
    while position < total {
        let Some(roman) = index.roman_key_at(position) else {
            position += step;
            continue;
        };
        let expected = engine
            .overlay_target(roman)
            .map(str::to_owned)
            .or_else(|| index.curated_exact(roman).map(str::to_owned))
            .or_else(|| {
                index
                    .exact_roman_targets(roman)
                    .first()
                    .map(|text| (*text).to_owned())
            });
        if let Some(expected) = expected {
            let rows = engine.suggest(&SuggestRequest::new(roman, 1), &smriti);
            let got = rows.first().map(|row| row.text.clone()).unwrap_or_default();
            report.checked += 1;
            if got == expected {
                report.matched += 1;
            } else if report.misses.len() < 200 {
                report.misses.push(Miss {
                    roman: roman.to_owned(),
                    expected,
                    got,
                });
            }
        }
        position += step;
    }
    Ok(report)
}
