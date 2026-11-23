#![forbid(unsafe_code)]
//! Deterministic synthetic replay generator.
//!
//! The word lists below are hand-authored synthetic fixtures (common Bangla
//! and English words plus romanizations), not user data and not a downloaded
//! corpus. The generator expands them with a seeded SplitMix64 stream into
//! replay cases; the same `--seed` always yields identical bytes. Layouts come
//! from the committed synthetic layout ids (`avro`, `bijoy`, `national`,
//! `probhat`) plus `none`.

use okkhor_core::{Language, normalize_nfc};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::replay::{REPLAY_SCHEMA, ReplayCase};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum GenerationError {
    #[error("need at least 1 case, got {got}")]
    TooFew { got: usize },
}

/// SplitMix64: small, deterministic, no external dependency.
///
/// Shared with the trainer so shuffling and generation use one PRNG.
#[derive(Debug, Clone)]
pub(crate) struct SplitMix64(u64);

impl SplitMix64 {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub(crate) fn below(&mut self, bound: usize) -> usize {
        assert!(bound > 0);
        (self.next_u64() % bound as u64) as usize
    }
}

const BN_WORDS: &[&str] = &[
    "আমি",
    "তুমি",
    "সে",
    "ভালো",
    "ভাল",
    "ভাই",
    "ভাত",
    "ভাবনা",
    "ভাষা",
    "কাজ",
    "বই",
    "পড়া",
    "লেখা",
    "খাওয়া",
    "যাওয়া",
    "আসা",
    "বাংলা",
    "ইংরেজি",
    "কথা",
    "গান",
    "বন্ধু",
    "পরিবার",
    "সময়",
    "দিন",
    "রাত",
    "সকাল",
    "বিকেল",
    "পানি",
    "ভাত",
    "মাছ",
    "বাড়ি",
    "স্কুল",
    "শহর",
    "গ্রাম",
    "রাস্তা",
    "আলো",
    "অন্ধকার",
    "সুন্দর",
    "নতুন",
    "পুরোনো",
    "বড়",
    "ছোট",
    "লাল",
    "নীল",
    "সবুজ",
];

const BN_ROMAN: &[(&str, &str)] = &[
    ("bhalo", "ভালো"),
    ("bhai", "ভাই"),
    ("bhaat", "ভাত"),
    ("bhasha", "ভাষা"),
    ("aami", "আমি"),
    ("tumi", "তুমি"),
    ("boi", "বই"),
    ("kotha", "কথা"),
    ("gaan", "গান"),
    ("bondhu", "বন্ধু"),
    ("shomoy", "সময়"),
    ("paani", "পানি"),
    ("baari", "বাড়ি"),
    ("shohor", "শহর"),
    ("alo", "আলো"),
    ("notun", "নতুন"),
];

const EN_WORDS: &[&str] = &[
    "hello",
    "held",
    "helix",
    "helm",
    "help",
    "hero",
    "hex",
    "world",
    "word",
    "work",
    "write",
    "water",
    "light",
    "night",
    "morning",
    "evening",
    "book",
    "read",
    "school",
    "city",
    "road",
    "time",
    "day",
    "friend",
    "family",
    "song",
    "house",
    "small",
    "large",
    "green",
    "blue",
    "morning",
    "keyboard",
    "suggest",
    "typing",
    "learn",
    "context",
    "honest",
    "report",
    "frozen",
    "synthetic",
    "replay",
    "layout",
    "prefix",
    "accept",
];

const BN_CONTEXT: &[&str] = &["আমি", "তুমি", "সে", "আজ", "কাল", "এখানে", "ওখানে", "খুব"];
const EN_CONTEXT: &[&str] = &["i", "you", "the", "a", "today", "very", "here", "now"];
const LAYOUTS: &[Option<&str>] = &[
    Some("avro"),
    Some("bijoy"),
    Some("national"),
    Some("probhat"),
    None,
];

/// Generate `count` deterministic replay cases from `seed`.
pub fn generate_cases(seed: u64, count: usize) -> Result<Vec<ReplayCase>, GenerationError> {
    if count == 0 {
        return Err(GenerationError::TooFew { got: 0 });
    }
    let mut rng = SplitMix64::new(seed);
    let mut cases = Vec::with_capacity(count);
    for index in 0..count {
        cases.push(one_case(&mut rng, index));
    }
    Ok(cases)
}

fn one_case(rng: &mut SplitMix64, index: usize) -> ReplayCase {
    // Alternate languages with jitter so both segments are covered.
    let bangla = if index % 2 == 0 {
        true
    } else {
        rng.below(100) < 60
    };
    if bangla {
        bangla_case(rng, index)
    } else {
        english_case(rng, index)
    }
}

fn pick<'a>(rng: &mut SplitMix64, pool: &'a [&'a str]) -> &'a str {
    pool[rng.below(pool.len())]
}

fn bangla_case(rng: &mut SplitMix64, index: usize) -> ReplayCase {
    let use_roman = rng.below(100) < 40;
    let (prefix, target) = if use_roman {
        let (roman, word) = BN_ROMAN[rng.below(BN_ROMAN.len())];
        let cut = 1 + rng.below(roman.len().max(1).min(roman.len()));
        let prefix: String = roman.chars().take(cut.max(1)).collect();
        (prefix, word.to_owned())
    } else {
        let word = pick(rng, BN_WORDS);
        let chars: Vec<char> = word.chars().collect();
        let cut = 1 + rng.below(chars.len().max(2) - 1);
        let prefix: String = chars[..cut.min(chars.len())].iter().collect();
        (prefix, word.to_owned())
    };
    let layout = LAYOUTS[rng.below(LAYOUTS.len())].map(str::to_owned);
    let context = bangla_context(rng);
    let distractors = bangla_distractors(rng, &target, 4);
    let (shown, accepted) = arrange(rng, &target, distractors);
    let mut candidates = shown.clone();
    candidates.sort();
    candidates.dedup();
    ReplayCase {
        schema: REPLAY_SCHEMA,
        case_id: format!("bn_{index:04}"),
        language: Language::Bangla,
        layout,
        prefix: normalize_nfc(&prefix),
        context,
        expected: vec![normalize_nfc(&target)],
        shown,
        accepted,
        candidates,
    }
}

fn english_case(rng: &mut SplitMix64, index: usize) -> ReplayCase {
    let word = pick(rng, EN_WORDS);
    let chars: Vec<char> = word.chars().collect();
    let cut = 1 + rng.below(chars.len().max(2) - 1);
    let prefix: String = chars[..cut.min(chars.len())].iter().collect();
    let layout = LAYOUTS[rng.below(LAYOUTS.len())].map(str::to_owned);
    let context = english_context(rng);
    let distractors = english_distractors(rng, word, 4);
    let (shown, accepted) = arrange(rng, word, distractors);
    let mut candidates = shown.clone();
    candidates.sort();
    candidates.dedup();
    ReplayCase {
        schema: REPLAY_SCHEMA,
        case_id: format!("en_{index:04}"),
        language: Language::English,
        layout,
        prefix,
        context,
        expected: vec![word.to_owned()],
        shown,
        accepted,
        candidates,
    }
}

fn bangla_context(rng: &mut SplitMix64) -> Vec<String> {
    let count = rng.below(3);
    (0..count)
        .map(|_| pick(rng, BN_CONTEXT).to_owned())
        .collect()
}

fn english_context(rng: &mut SplitMix64) -> Vec<String> {
    let count = rng.below(3);
    (0..count)
        .map(|_| pick(rng, EN_CONTEXT).to_owned())
        .collect()
}

fn bangla_distractors(rng: &mut SplitMix64, target: &str, count: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(count);
    let mut guard = 0;
    while out.len() < count && guard < 64 {
        guard += 1;
        let word = pick(rng, BN_WORDS);
        if word != target && !out.iter().any(|w: &String| w == word) {
            out.push(word.to_owned());
        }
    }
    out
}

fn english_distractors(rng: &mut SplitMix64, target: &str, count: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(count);
    let mut guard = 0;
    while out.len() < count && guard < 64 {
        guard += 1;
        let word = pick(rng, EN_WORDS);
        if word != target && !out.iter().any(|w: &String| w == word) {
            out.push(word.to_owned());
        }
    }
    out
}

/// Deterministically rotate the expected answer through the shown list;
/// ~75% of cases carry an `accepted` index (the expected position).
fn arrange(
    rng: &mut SplitMix64,
    target: &str,
    mut distractors: Vec<String>,
) -> (Vec<String>, Option<usize>) {
    let mut shown = vec![target.to_owned()];
    shown.append(&mut distractors);
    let rotation = rng.below(shown.len().max(1));
    shown.rotate_right(rotation);
    let position = shown.iter().position(|w| w == target);
    let accepted = if rng.below(100) < 75 { position } else { None };
    (shown, accepted)
}

/// Render cases as JSONL with one object per line plus a trailing newline.
pub fn render_jsonl(cases: &[ReplayCase]) -> String {
    let mut out = String::new();
    for case in cases {
        out.push_str(&serde_json::to_string(case).expect("replay case serializes"));
        out.push('\n');
    }
    out
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_cases() {
        assert!(generate_cases(1, 0).is_err());
    }
}
