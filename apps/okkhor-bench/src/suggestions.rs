//! Suggestion-engine quality and latency over a small everyday corpus.

use std::time::{Duration, Instant};

use okkhor_engine::{SuggestRequest, SuggestionEngine};
use okkhor_learn::Smriti;

use crate::stats::LatencySummary;

const WORDS: &[(&str, &str)] = &[
    ("amar", "আমার"),
    ("ami", "আমি"),
    ("tumi", "তুমি"),
    ("apni", "আপনি"),
    ("kemon", "কেমন"),
    ("acho", "আছো"),
    ("achhi", "আছি"),
    ("bhalo", "ভালো"),
    ("dhonnobad", "ধন্যবাদ"),
    ("korcho", "করছো"),
    ("kore", "করে"),
    ("korte", "করতে"),
    ("ghor", "ঘর"),
    ("mangsho", "মাংস"),
    ("raat", "রাত"),
    ("naam", "নাম"),
    ("jabo", "যাবো"),
    ("koro", "করো"),
    ("korechi", "করেছি"),
    ("bangla", "বাংলা"),
    ("computer", "কম্পিউটার"),
    ("internet", "ইন্টারনেট"),
    ("keyboard", "কিবোর্ড"),
    ("kotha", "কথা"),
    ("bhai", "ভাই"),
    ("ma", "মা"),
    ("desh", "দেশ"),
    ("shomoy", "সময়"),
    ("aj", "আজ"),
    ("onek", "অনেক"),
    ("khub", "খুব"),
    ("theke", "থেকে"),
    ("jonno", "জন্য"),
    ("sathe", "সাথে"),
    ("holo", "হলো"),
];

#[derive(Debug, Clone, Default)]
pub struct SuggestionReport {
    pub top1: usize,
    pub top5: usize,
    pub total: usize,
    pub latency: Option<LatencySummary>,
}

pub fn run(iterations: usize, warmup: usize) -> Result<SuggestionReport, String> {
    let engine = SuggestionEngine::from_embedded().map_err(|error| error.to_string())?;
    if let Some(kind) = engine.index_error() {
        return Err(format!("suggestion index unavailable: {kind}"));
    }
    let smriti = Smriti::new();

    let mut top1 = 0usize;
    let mut top5 = 0usize;
    for (roman, expected) in WORDS {
        let rows = engine.suggest(&SuggestRequest::new(roman, 8), &smriti);
        let texts: Vec<&str> = rows.iter().map(|row| row.text.as_str()).collect();
        if texts.first().copied() == Some(*expected) {
            top1 += 1;
        }
        if texts.iter().take(5).any(|text| text == expected) {
            top5 += 1;
        }
    }

    for index in 0..warmup {
        let (roman, _) = WORDS[index % WORDS.len()];
        let _ = engine.suggest(&SuggestRequest::new(roman, 8), &smriti);
    }

    let mut samples = Vec::with_capacity(iterations);
    for index in 0..iterations {
        let (roman, _) = WORDS[index % WORDS.len()];
        let start = Instant::now();
        let _ = engine.suggest(&SuggestRequest::new(roman, 8), &smriti);
        samples.push(start.elapsed().max(Duration::from_nanos(1)));
    }

    Ok(SuggestionReport {
        top1,
        top5,
        total: WORDS.len(),
        latency: Some(LatencySummary::from_samples(samples)),
    })
}
