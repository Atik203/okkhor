#![forbid(unsafe_code)]
//! Shipped feature schema v1: stable, ordered, cheap to compute, no I/O.
//!
//! This crate is the single source of truth for `FeatureVectorV1` and
//! `extract_features`. It is a shipped component: it performs no I/O and reads
//! no clock, so inference can call it on the suggestion path. The offline
//! `okkhor-train` crate re-exports these names for replay and training.
//!
//! Field order is normative and matches `docs/model-roadmap.md` §"Feature
//! schema v1" (minus the optional `trigram_score`, which is explicitly out of
//! v1). Any insertion, removal, or reorder requires a schema bump and a model
//! migration note. See ADR 0015 and ADR 0017.

use okkhor_core::{Candidate, CandidateSource, Language, SuggestionRequest, contains_bengali};
use okkhor_learn::{BigramStat, LearningSnapshot, PairStat, WordStat};
use thiserror::Error;

/// Read-only view of the local learning statistics that feature extraction
/// needs.
///
/// Implemented by [`LearningSnapshot`] for offline/replay use and by
/// `Smriti` (in `okkhor-model`) for the live path, so a ranker can read the
/// shared store without cloning a snapshot per keystroke.
pub trait FeatureSource {
    fn word_stat(&self, language: Language, word: &str) -> Option<WordStat>;
    fn pair_stat(&self, language: Language, roman_input: &str, output: &str) -> Option<PairStat>;
    fn bigram_stat(&self, language: Language, previous: &str, current: &str) -> Option<BigramStat>;
}

impl FeatureSource for LearningSnapshot {
    fn word_stat(&self, language: Language, word: &str) -> Option<WordStat> {
        LearningSnapshot::word_stat(self, language, word)
    }

    fn pair_stat(&self, language: Language, roman_input: &str, output: &str) -> Option<PairStat> {
        LearningSnapshot::pair_stat(self, language, roman_input, output)
    }

    fn bigram_stat(&self, language: Language, previous: &str, current: &str) -> Option<BigramStat> {
        LearningSnapshot::bigram_stat(self, language, previous, current)
    }
}

/// Feature schema version. Bump on any field order/type change.
pub const SCHEMA_VERSION: u32 = 1;

/// Canonical field names in declaration order.
pub const FIELD_NAMES: [&str; 16] = [
    "base_score",
    "candidate_source",
    "language",
    "layout",
    "prefix_len",
    "word_len",
    "user_word_count",
    "pair_count",
    "recency",
    "acceptance_rate",
    "correction_rate",
    "bigram_score",
    "phonetic_score",
    "is_user_vocab",
    "context_language",
    "script_mix",
];

/// Default recency half-life in days, mirroring `SmritiConfig::default`.
const RECENCY_HALF_LIFE_DAYS: f32 = 14.0;

/// Hard input caps so extraction stays bounded.
const MAX_PREFIX_CHARS: usize = 256;
const MAX_WORD_CHARS: usize = 128;
const MAX_CONTEXT_WORDS: usize = 8;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FeatureError {
    #[error("candidate text is empty after normalization")]
    EmptyCandidate,
    #[error("prefix exceeds 256 chars")]
    PrefixTooLong,
    #[error("candidate word exceeds 128 chars")]
    WordTooLong,
}

/// Stable encoding of [`CandidateSource`].
pub fn encode_source(source: CandidateSource) -> u8 {
    match source {
        CandidateSource::Dictionary => 0,
        CandidateSource::Transliteration => 1,
        CandidateSource::UserHistory => 2,
        CandidateSource::Recent => 3,
        CandidateSource::Context => 4,
        CandidateSource::Ai => 5,
    }
}

/// Stable encoding of [`Language`]: English = 0, Bangla = 1.
pub fn encode_language(language: Language) -> u8 {
    match language {
        Language::English => 0,
        Language::Bangla => 1,
    }
}

/// Stable layout encoding for the active layout id.
///
/// `None`/empty/unknown map to `0`; known 0.3 codes map to fixed ids so a
/// model trained today decodes the same bytes tomorrow.
pub fn encode_layout(layout: Option<&str>) -> u8 {
    match layout.map(str::trim).map(str::to_lowercase).as_deref() {
        None | Some("") => 0,
        Some("avro") => 1,
        Some("bijoy") => 2,
        Some("national") => 3,
        Some("probhat") => 4,
        Some("banglish") => 5,
        Some("khipro") => 6,
        Some("unibijoy") => 7,
        Some(_) => 8,
    }
}

/// Context language encoding: 0 = none, 1 = English only, 2 = Bangla, 3 = mixed.
pub fn encode_context_language(context: &[String]) -> u8 {
    if context.is_empty() {
        return 0;
    }
    let mut saw_bengali = false;
    let mut saw_other = false;
    for word in context.iter().take(MAX_CONTEXT_WORDS) {
        if contains_bengali(word) {
            saw_bengali = true;
        } else if !word.trim().is_empty() {
            saw_other = true;
        }
    }
    match (saw_bengali, saw_other) {
        (false, false) => 0,
        (false, true) => 1,
        (true, false) => 2,
        (true, true) => 3,
    }
}

/// Fraction of Bengali-script characters in the joined context, 0.0..=1.0.
pub fn script_mix_ratio(context: &[String]) -> f32 {
    let mut bengali = 0u64;
    let mut total = 0u64;
    for word in context.iter().take(MAX_CONTEXT_WORDS) {
        for ch in word.chars() {
            if ch.is_whitespace() {
                continue;
            }
            total += 1;
            if okkhor_core::is_bengali_char(ch) {
                bengali += 1;
            }
        }
    }
    if total == 0 {
        0.0
    } else {
        (bengali as f32 / total as f32).clamp(0.0, 1.0)
    }
}

fn clamp01(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn clamp_finite(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

fn recency_decay(last_used_ms: u64, now_ms: u64) -> f32 {
    if last_used_ms == 0 {
        return 0.0;
    }
    if now_ms == 0 || now_ms <= last_used_ms {
        return 1.0;
    }
    let age_ms = now_ms.saturating_sub(last_used_ms) as f32;
    let half_life_ms = RECENCY_HALF_LIFE_DAYS * 86_400_000.0;
    (0.5_f32).powf(age_ms / half_life_ms).clamp(0.0, 1.0)
}

/// The 16-field v1 feature vector, in stable declaration order.
///
/// All floats are finite; integer fields saturate. No raw text is stored.
#[derive(Debug, Clone, PartialEq)]
pub struct FeatureVectorV1 {
    pub base_score: f32,
    pub candidate_source: u8,
    pub language: u8,
    pub layout: u8,
    pub prefix_len: u16,
    pub word_len: u16,
    pub user_word_count: u32,
    pub pair_count: u32,
    pub recency: f32,
    pub acceptance_rate: f32,
    pub correction_rate: f32,
    pub bigram_score: f32,
    pub phonetic_score: f32,
    pub is_user_vocab: bool,
    pub context_language: u8,
    pub script_mix: f32,
}

impl FeatureVectorV1 {
    /// Numeric view in field order, for trainers. Enums/bools become `f32`.
    pub fn to_array(&self) -> [f32; 16] {
        [
            self.base_score,
            f32::from(self.candidate_source),
            f32::from(self.language),
            f32::from(self.layout),
            f32::from(self.prefix_len),
            f32::from(self.word_len),
            self.user_word_count as f32,
            self.pair_count as f32,
            self.recency,
            self.acceptance_rate,
            self.correction_rate,
            self.bigram_score,
            self.phonetic_score,
            if self.is_user_vocab { 1.0 } else { 0.0 },
            f32::from(self.context_language),
            self.script_mix,
        ]
    }

    /// Field names in the same order as [`FeatureVectorV1::to_array`].
    pub fn field_names() -> [&'static str; 16] {
        FIELD_NAMES
    }
}

/// Extract v1 features for one (`request`, `candidate`) pair against any
/// [`FeatureSource`]. No I/O, deterministic, bounded allocation (a few small
/// normalized keys bounded by the input lengths).
pub fn extract_features<S: FeatureSource + ?Sized>(
    request: &SuggestionRequest,
    candidate: &Candidate,
    snapshot: &S,
) -> Result<FeatureVectorV1, FeatureError> {
    let prefix_chars = request.prefix.chars().count();
    if prefix_chars > MAX_PREFIX_CHARS {
        return Err(FeatureError::PrefixTooLong);
    }
    let word_chars = candidate.text.chars().count();
    if word_chars == 0
        || candidate.text.trim().is_empty()
        || okkhor_core::normalize_lookup(&candidate.text).is_empty()
    {
        return Err(FeatureError::EmptyCandidate);
    }
    if word_chars > MAX_WORD_CHARS {
        return Err(FeatureError::WordTooLong);
    }

    let word_stat = snapshot.word_stat(candidate.language, &candidate.text);
    let user_word_count = word_stat
        .as_ref()
        .map_or(candidate.features.user_frequency, |stat| {
            u32::try_from(stat.count).unwrap_or(u32::MAX)
        });

    let pair_count = request
        .roman_input
        .as_deref()
        .and_then(|roman| snapshot.pair_stat(candidate.language, roman, &candidate.text))
        .map_or(candidate.features.pair_frequency, |stat| {
            u32::try_from(stat.count).unwrap_or(u32::MAX)
        });

    let recency = word_stat
        .as_ref()
        .map_or(0.0, |stat| recency_decay(stat.last_used_ms, request.now_ms));

    let acceptance_rate =
        word_stat
            .as_ref()
            .map_or(clamp01(candidate.features.acceptance_rate), |stat| {
                if stat.shown == 0 {
                    0.0
                } else {
                    (stat.accepted as f32 / stat.shown as f32).clamp(0.0, 1.0)
                }
            });

    let correction_rate =
        word_stat
            .as_ref()
            .map_or(clamp01(candidate.features.correction_penalty), |stat| {
                let denominator = stat.count as f32 + stat.corrections as f32 + 1.0;
                (stat.corrections as f32 / denominator).clamp(0.0, 1.0)
            });

    let bigram_score = request
        .context
        .last()
        .and_then(|previous| snapshot.bigram_stat(candidate.language, previous, &candidate.text))
        .map_or(clamp01(candidate.features.context_score), |stat| {
            (stat.count as f32 / (stat.count as f32 + 5.0)).clamp(0.0, 1.0)
        });

    Ok(FeatureVectorV1 {
        base_score: clamp_finite(candidate.features.base_score, 0.0),
        candidate_source: encode_source(candidate.source),
        language: encode_language(candidate.language),
        layout: encode_layout(request.layout.as_deref()),
        prefix_len: u16::try_from(prefix_chars.min(u16::MAX as usize)).unwrap_or(u16::MAX),
        word_len: u16::try_from(word_chars.min(u16::MAX as usize)).unwrap_or(u16::MAX),
        user_word_count,
        pair_count,
        recency,
        acceptance_rate,
        correction_rate,
        bigram_score,
        phonetic_score: clamp01(candidate.features.phonetic_score),
        is_user_vocab: word_stat.is_some_and(|stat| stat.count > 0),
        context_language: encode_context_language(&request.context),
        script_mix: script_mix_ratio(&request.context),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use okkhor_core::{CandidateSource, Language};

    #[test]
    fn field_order_is_pinned() {
        assert_eq!(
            FeatureVectorV1::field_names(),
            [
                "base_score",
                "candidate_source",
                "language",
                "layout",
                "prefix_len",
                "word_len",
                "user_word_count",
                "pair_count",
                "recency",
                "acceptance_rate",
                "correction_rate",
                "bigram_score",
                "phonetic_score",
                "is_user_vocab",
                "context_language",
                "script_mix",
            ]
        );
    }

    #[test]
    fn encodings_are_stable() {
        assert_eq!(encode_source(CandidateSource::Dictionary), 0);
        assert_eq!(encode_source(CandidateSource::Transliteration), 1);
        assert_eq!(encode_layout(None), 0);
        assert_eq!(encode_layout(Some("avro")), 1);
        assert_eq!(encode_layout(Some("AVRO")), 1);
        assert_eq!(encode_language(Language::English), 0);
        assert_eq!(encode_language(Language::Bangla), 1);
    }
}
