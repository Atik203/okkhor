//! Rerank opportunity: how often is the intended word already in the popup?
//!
//! The context reranker only helps when the transliterator already offers the
//! intended word somewhere in the candidate list but ranks it below another
//! word. This tool samples licensed aliases, asks the live engine for its rows,
//! and writes one JSON line per case so a teacher model can score them later.
//!
//! Each line is:
//! `{"roman":"...","intended":"...","candidates":["...",...],"baseline_rank":N}`
//!
//! `baseline_rank` is the zero-based position the current engine gives the
//! alias's preferred target, or -1 when it is not in the list at all. Cases
//! with rank > 0 are the reranker's opportunity; -1 cases cannot be fixed by
//! reordering.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use okkhor_engine::{SuggestRequest, SuggestionEngine};
use okkhor_learn::Smriti;

/// Summary of one dataset build.
#[derive(Debug, Clone, Default)]
pub struct DatasetReport {
    /// Aliases that had an authoritative target.
    pub sampled: usize,
    /// Target already first: nothing to rerank.
    pub first: usize,
    /// Target present but ranked below first: the reranker's room.
    pub displaced: usize,
    /// Target absent from the candidate list: reordering cannot help.
    pub absent: usize,
}

impl DatasetReport {
    pub fn displaced_percent(&self) -> f64 {
        if self.sampled == 0 {
            0.0
        } else {
            self.displaced as f64 * 100.0 / self.sampled as f64
        }
    }

    pub fn present_percent(&self) -> f64 {
        if self.sampled == 0 {
            0.0
        } else {
            (self.first + self.displaced) as f64 * 100.0 / self.sampled as f64
        }
    }
}

/// Write up to `sample` cases as JSON lines and return the opportunity summary.
pub fn write_dataset(path: &Path, sample: usize, limit: usize) -> Result<DatasetReport, String> {
    let sample = sample.max(1);
    let engine = SuggestionEngine::from_embedded().map_err(|error| error.to_string())?;
    let index = engine
        .index()
        .ok_or_else(|| "suggestion index unavailable".to_owned())?;
    let total = index.roman_count();
    let step = (total / sample).max(1);
    let smriti = Smriti::new();

    let file = File::create(path).map_err(|error| error.to_string())?;
    let mut writer = BufWriter::new(file);
    let mut report = DatasetReport::default();

    let mut position = 0usize;
    while position < total {
        let Some(roman) = index.roman_key_at(position) else {
            position += step;
            continue;
        };
        let intended = engine
            .overlay_target(roman)
            .map(str::to_owned)
            .or_else(|| index.curated_exact(roman).map(str::to_owned))
            .or_else(|| {
                index
                    .exact_roman_targets(roman)
                    .first()
                    .map(|text| (*text).to_owned())
            });
        if let Some(intended) = intended {
            let rows = engine.suggest(&SuggestRequest::new(roman, 8), &smriti);
            let candidates: Vec<String> = rows.into_iter().map(|row| row.text).collect();
            let rank = candidates
                .iter()
                .position(|text| text == &intended)
                .map(|value| value as i32)
                .unwrap_or(-1);
            report.sampled += 1;
            match rank {
                0 => report.first += 1,
                -1 => report.absent += 1,
                _ => report.displaced += 1,
            }
            if rank > 0 && report.displaced <= limit {
                let line = serde_json::json!({
                    "roman": roman,
                    "intended": intended,
                    "candidates": candidates,
                    "baseline_rank": rank,
                });
                writeln!(writer, "{line}").map_err(|error| error.to_string())?;
            }
        }
        position += step;
    }
    writer.flush().map_err(|error| error.to_string())?;
    Ok(report)
}
