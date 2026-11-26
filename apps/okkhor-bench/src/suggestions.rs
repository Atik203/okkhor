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
    // Everyday nouns, verbs, adjectives. The expected column is the standard
    // spelling; a miss here is a rule or index gap worth reviewing, not proof
    // of a bug (some words have accepted variants).
    ("amra", "আমরা"),
    ("tara", "তারা"),
    ("se", "সে"),
    ("ki", "কী"),
    ("keno", "কেন"),
    ("kothay", "কোথায়"),
    ("kobe", "কবে"),
    ("korche", "করছে"),
    ("korchi", "করছি"),
    ("korbo", "করবো"),
    ("korlam", "করলাম"),
    ("khai", "খাই"),
    ("khela", "খেলা"),
    ("jai", "যাই"),
    ("jete", "যেতে"),
    ("asbo", "আসবো"),
    ("dekhi", "দেখি"),
    ("dekho", "দেখো"),
    ("bolo", "বলো"),
    ("shono", "শোনো"),
    ("likhi", "লিখি"),
    ("poRi", "পড়ি"),
    ("ghumi", "ঘুমি"),
    ("bari", "বাড়ি"),
    ("baba", "বাবা"),
    ("bon", "বোন"),
    ("chele", "ছেলে"),
    ("meye", "মেয়ে"),
    ("bondhu", "বন্ধু"),
    ("manush", "মানুষ"),
    ("pani", "পানি"),
    ("bhat", "ভাত"),
    ("mach", "মাছ"),
    ("dim", "ডিম"),
    ("doodh", "দুধ"),
    ("cha", "চা"),
    ("mishti", "মিষ্টি"),
    ("khabar", "খাবার"),
    ("bazar", "বাজার"),
    ("dokan", "দোকান"),
    ("taka", "টাকা"),
    ("kaj", "কাজ"),
    ("school", "স্কুল"),
    ("boi", "বই"),
    ("kolom", "কলম"),
    ("kagoj", "কাগজ"),
    ("phone", "ফোন"),
    ("chobi", "ছবি"),
    ("gaan", "গান"),
    ("din", "দিন"),
    ("shokal", "সকাল"),
    ("dupur", "দুপুর"),
    ("kal", "কাল"),
    ("mash", "মাস"),
    ("bochor", "বছর"),
    ("kharap", "খারাপ"),
    ("kom", "কম"),
    ("beshi", "বেশি"),
    ("shundor", "সুন্দর"),
    ("choto", "ছোট"),
    ("boro", "বড়"),
    ("notun", "নতুন"),
    ("purano", "পুরানো"),
    ("shohoj", "সহজ"),
    ("kothin", "কঠিন"),
    ("ekhon", "এখন"),
    ("pore", "পরে"),
    ("age", "আগে"),
    ("shohor", "শহর"),
    ("gram", "গ্রাম"),
    ("rasta", "রাস্তা"),
    ("nodi", "নদী"),
    ("pahar", "পাহাড়"),
    ("office", "অফিস"),
    ("hospital", "হাসপাতাল"),
    ("ek", "এক"),
    ("dui", "দুই"),
    ("tin", "তিন"),
    ("char", "চার"),
    ("panch", "পাঁচ"),
    ("choy", "ছয়"),
    ("shat", "সাত"),
    ("aath", "আট"),
    ("dosh", "দশ"),
    ("hajar", "হাজার"),
    ("lakh", "লাখ"),
    ("boyosh", "বয়স"),
    ("prosno", "প্রশ্ন"),
    ("uttor", "উত্তর"),
    ("golpo", "গল্প"),
    ("khobor", "খবর"),
    ("chithi", "চিঠি"),
    ("mukh", "মুখ"),
    ("chokh", "চোখ"),
    ("matha", "মাথা"),
    ("pet", "পেট"),
    ("mon", "মন"),
    ("valobasha", "ভালোবাসা"),
    ("biye", "বিয়ে"),
    ("poribar", "পরিবার"),
    ("sontan", "সন্তান"),
    ("druto", "দ্রুত"),
    ("taratari", "তাড়াতাড়ি"),
];

#[derive(Debug, Clone, Default)]
pub struct SuggestionReport {
    pub top1: usize,
    pub top5: usize,
    pub total: usize,
    /// `(roman, expected, got)` for top-1 misses, capped for printing.
    pub misses: Vec<(String, String, String)>,
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
    let mut misses = Vec::new();
    for (roman, expected) in WORDS {
        let rows = engine.suggest(&SuggestRequest::new(roman, 8), &smriti);
        let texts: Vec<&str> = rows.iter().map(|row| row.text.as_str()).collect();
        let got = texts.first().copied().unwrap_or("");
        if got == *expected {
            top1 += 1;
        } else if misses.len() < 40 {
            misses.push(((*roman).to_owned(), (*expected).to_owned(), got.to_owned()));
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
        misses,
        latency: Some(LatencySummary::from_samples(samples)),
    })
}
