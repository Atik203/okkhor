use std::collections::HashMap;
use std::sync::RwLock;

use okkhor_core::{Candidate, Language, Ranker, Suggestion, SuggestionRequest, normalize_lookup};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::{ConfigError, SmritiConfig};
use crate::event::{CommitSource, LearningEvent};

/// Schema version for persisted learning snapshots.
pub const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// Aggregate statistics for one word in one language.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordStat {
    pub count: u64,
    pub last_used_ms: u64,
    pub shown: u64,
    pub accepted: u64,
    pub corrections: u64,
}

/// Aggregate statistics for one Romanized-input -> output pair.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairStat {
    pub count: u64,
    pub last_used_ms: u64,
}

/// Aggregate statistics for one adjacent word pair.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BigramStat {
    pub count: u64,
    pub last_used_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct WordKey {
    language: Language,
    text: String,
}

impl WordKey {
    fn new(language: Language, text: &str) -> Option<Self> {
        let text = normalize_lookup(text);
        if text.is_empty() {
            None
        } else {
            Some(Self { language, text })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct PairKey {
    language: Language,
    roman_input: String,
    output: String,
}

impl PairKey {
    fn new(language: Language, roman_input: &str, output: &str) -> Option<Self> {
        let roman_input = normalize_lookup(roman_input).to_lowercase();
        let output = normalize_lookup(output);
        if roman_input.is_empty() || output.is_empty() {
            None
        } else {
            Some(Self {
                language,
                roman_input,
                output,
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct BigramKey {
    language: Language,
    first: String,
    second: String,
}

#[derive(Debug, Default)]
struct Inner {
    words: HashMap<WordKey, WordStat>,
    pairs: HashMap<PairKey, PairStat>,
    bigrams: HashMap<BigramKey, BigramStat>,
}

/// An opaque, serializable snapshot of the learning store.
///
/// Fields are intentionally private. Persistence code should store the whole
/// snapshot; analytics should go through exported, anonymized aggregates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearningSnapshot {
    schema_version: u32,
    words: Vec<(WordKey, WordStat)>,
    pairs: Vec<(PairKey, PairStat)>,
    bigrams: Vec<(BigramKey, BigramStat)>,
}

impl LearningSnapshot {
    pub const SCHEMA_VERSION: u32 = SNAPSHOT_SCHEMA_VERSION;

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn word_count(&self) -> usize {
        self.words.len()
    }

    pub fn pair_count(&self) -> usize {
        self.pairs.len()
    }

    pub fn bigram_count(&self) -> usize {
        self.bigrams.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty() && self.pairs.is_empty() && self.bigrams.is_empty()
    }

    /// Look up aggregate stats for one normalized word.
    ///
    /// Returns `None` for empty input or an unknown word. Used by the
    /// offline feature extractor (`okkhor-train`); never on the typing path.
    pub fn word_stat(&self, language: Language, word: &str) -> Option<WordStat> {
        let key = WordKey::new(language, word)?;
        self.words
            .iter()
            .find(|(existing, _)| *existing == key)
            .map(|(_, stat)| stat.clone())
    }

    /// Whether the snapshot already contains `word` for `language`.
    pub fn contains_word(&self, language: Language, word: &str) -> bool {
        self.word_stat(language, word)
            .is_some_and(|stat| stat.count > 0)
    }

    /// Look up one Romanized-input -> output pair.
    pub fn pair_stat(
        &self,
        language: Language,
        roman_input: &str,
        output: &str,
    ) -> Option<PairStat> {
        let key = PairKey::new(language, roman_input, output)?;
        self.pairs
            .iter()
            .find(|(existing, _)| *existing == key)
            .map(|(_, stat)| stat.clone())
    }

    /// Look up one adjacent word-pair count (`previous` -> `current`).
    pub fn bigram_stat(
        &self,
        language: Language,
        previous: &str,
        current: &str,
    ) -> Option<BigramStat> {
        let previous_key = WordKey::new(language, previous)?;
        let current_key = WordKey::new(language, current)?;
        self.bigrams
            .iter()
            .find(|(existing, _)| {
                existing.language == language
                    && existing.first == previous_key.text
                    && existing.second == current_key.text
            })
            .map(|(_, stat)| stat.clone())
    }
}

#[derive(Debug, Error)]
pub enum LearnError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("unsupported snapshot schema {found}; expected {expected}")]
    UnsupportedSchema { found: u32, expected: u32 },
}

/// The local personalization engine.
#[derive(Debug)]
pub struct Smriti {
    config: SmritiConfig,
    inner: RwLock<Inner>,
}

impl Default for Smriti {
    fn default() -> Self {
        Self::new()
    }
}

impl Smriti {
    /// Create an engine with the default configuration.
    pub fn new() -> Self {
        let config = SmritiConfig::default();
        debug_assert!(config.validate().is_ok());
        Self {
            config,
            inner: RwLock::new(Inner::default()),
        }
    }

    /// Create an engine with a validated configuration.
    pub fn with_config(config: SmritiConfig) -> Result<Self, LearnError> {
        config.validate()?;
        Ok(Self {
            config,
            inner: RwLock::new(Inner::default()),
        })
    }

    /// Restore a persisted snapshot after validating its schema.
    pub fn from_snapshot(
        snapshot: LearningSnapshot,
        config: SmritiConfig,
    ) -> Result<Self, LearnError> {
        config.validate()?;
        if snapshot.schema_version != SNAPSHOT_SCHEMA_VERSION {
            return Err(LearnError::UnsupportedSchema {
                found: snapshot.schema_version,
                expected: SNAPSHOT_SCHEMA_VERSION,
            });
        }
        Ok(Self {
            config,
            inner: RwLock::new(Inner {
                words: snapshot.words.into_iter().collect(),
                pairs: snapshot.pairs.into_iter().collect(),
                bigrams: snapshot.bigrams.into_iter().collect(),
            }),
        })
    }

    pub fn config(&self) -> &SmritiConfig {
        &self.config
    }

    /// Apply one committed observation. This is intentionally synchronous and
    /// cheap enough to call from a background learning queue.
    pub fn observe(&self, event: LearningEvent) {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        match event {
            LearningEvent::WordCommitted {
                word,
                language,
                previous_word,
                timestamp_ms,
                source,
            } => {
                record_word(
                    &mut inner,
                    language,
                    &word,
                    previous_word.as_deref(),
                    timestamp_ms,
                    source,
                );
            }
            LearningEvent::SuggestionShown {
                word,
                language,
                timestamp_ms: _,
            } => {
                if let Some(key) = WordKey::new(language, &word) {
                    let stat = inner.words.entry(key).or_default();
                    stat.shown = stat.shown.saturating_add(1);
                }
            }
            LearningEvent::SuggestionAccepted {
                word,
                language,
                previous_word,
                timestamp_ms,
            } => {
                if let Some(key) = WordKey::new(language, &word) {
                    let stat = inner.words.entry(key).or_default();
                    stat.shown = stat.shown.saturating_add(1);
                }
                record_word(
                    &mut inner,
                    language,
                    &word,
                    previous_word.as_deref(),
                    timestamp_ms,
                    CommitSource::AcceptedSuggestion,
                );
            }
            LearningEvent::Correction {
                from,
                to,
                language,
                timestamp_ms,
            } => {
                if let Some(key) = WordKey::new(language, &from) {
                    let stat = inner.words.entry(key).or_default();
                    stat.corrections = stat.corrections.saturating_add(1);
                }
                record_word(
                    &mut inner,
                    language,
                    &to,
                    None,
                    timestamp_ms,
                    CommitSource::Correction,
                );
            }
            LearningEvent::TransliterationCommitted {
                roman_input,
                output,
                language,
                previous_word,
                timestamp_ms,
            } => {
                record_word(
                    &mut inner,
                    language,
                    &output,
                    previous_word.as_deref(),
                    timestamp_ms,
                    CommitSource::Transliteration,
                );
                if let Some(key) = PairKey::new(language, &roman_input, &output) {
                    let stat = inner.pairs.entry(key).or_default();
                    stat.count = stat.count.saturating_add(1);
                    stat.last_used_ms = stat.last_used_ms.max(timestamp_ms);
                }
            }
        }
    }

    /// Rank candidates with local statistics. No I/O, network, or allocation of
    /// unbounded data is allowed here.
    pub fn score(&self, candidate: &Candidate, request: &SuggestionRequest) -> f32 {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.score_with(candidate, request, &inner)
    }

    /// Score every candidate under one read lock, preserving input order.
    ///
    /// Additive bulk API for the hybrid reranker: centering needs the whole
    /// staged pool's baseline scores before truncation. Bounded by the staged
    /// pool cap; performs no I/O and reads no clock.
    pub fn score_pool(&self, request: &SuggestionRequest, candidates: &[Candidate]) -> Vec<f32> {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        candidates
            .iter()
            .map(|candidate| self.score_with(candidate, request, &inner))
            .collect()
    }

    /// Persistable snapshot for `okkhor-storage`.
    pub fn snapshot(&self) -> LearningSnapshot {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        LearningSnapshot {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            words: inner
                .words
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            pairs: inner
                .pairs
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            bigrams: inner
                .bigrams
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        }
    }

    /// Drop everything. Used by the privacy "clear learning data" control.
    pub fn clear(&self) {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.words.clear();
        inner.pairs.clear();
        inner.bigrams.clear();
    }

    /// Enforce configured cardinality caps. Call from a maintenance task, not
    /// from the typing path.
    pub fn maintain(&self) {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        prune(&mut inner.words, self.config.max_words, |stat| {
            (stat.count, stat.last_used_ms)
        });
        prune(&mut inner.pairs, self.config.max_pairs, |stat| {
            (stat.count, stat.last_used_ms)
        });
        prune(&mut inner.bigrams, self.config.max_bigrams, |stat| {
            (stat.count, stat.last_used_ms)
        });
    }

    pub fn word_count(&self) -> usize {
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .words
            .len()
    }

    pub fn pair_count(&self) -> usize {
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pairs
            .len()
    }

    /// Look up one word's aggregate stats. Additive query for the offline
    /// feature extractor; the typing path uses [`Smriti::score`] instead.
    pub fn word_stat(&self, language: Language, word: &str) -> Option<WordStat> {
        let key = WordKey::new(language, word)?;
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .words
            .get(&key)
            .cloned()
    }

    /// Whether `word` has a positive learned count for `language`.
    pub fn contains_word(&self, language: Language, word: &str) -> bool {
        self.word_stat(language, word)
            .is_some_and(|stat| stat.count > 0)
    }

    /// Look up one Romanized-input -> output pair.
    pub fn pair_stat(
        &self,
        language: Language,
        roman_input: &str,
        output: &str,
    ) -> Option<PairStat> {
        let key = PairKey::new(language, roman_input, output)?;
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pairs
            .get(&key)
            .cloned()
    }

    /// Look up one adjacent word-pair (`previous` -> `current`).
    pub fn bigram_stat(
        &self,
        language: Language,
        previous: &str,
        current: &str,
    ) -> Option<BigramStat> {
        let previous_key = WordKey::new(language, previous)?;
        let current_key = WordKey::new(language, current)?;
        let key = BigramKey {
            language,
            first: previous_key.text,
            second: current_key.text,
        };
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .bigrams
            .get(&key)
            .cloned()
    }

    /// Most frequent words that followed `previous` for this user. The caller
    /// uses this for the next-word strip; it returns an empty list when the
    /// pair was never seen and never blocks the typing path.
    pub fn next_words(
        &self,
        language: Language,
        previous: &str,
        limit: usize,
    ) -> Vec<(String, u64)> {
        let Some(previous_key) = WordKey::new(language, previous) else {
            return Vec::new();
        };
        let inner = self
            .inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut rows: Vec<(String, u64)> = inner
            .bigrams
            .iter()
            .filter(|(key, _)| key.language == language && key.first == previous_key.text)
            .map(|(key, stat)| (key.second.clone(), stat.count))
            .collect();
        rows.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        rows.truncate(limit);
        rows
    }

    pub fn bigram_count(&self) -> usize {
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .bigrams
            .len()
    }

    fn score_with(&self, candidate: &Candidate, request: &SuggestionRequest, inner: &Inner) -> f32 {
        let Some(word_key) = WordKey::new(candidate.language, &candidate.text) else {
            return f32::NEG_INFINITY;
        };
        let stat = inner.words.get(&word_key);

        let user_frequency =
            candidate.features.user_frequency as f32 + stat.map_or(0.0, |s| s.count as f32);
        let learned_recency = stat.map_or(0.0, |s| {
            recency_score(s.last_used_ms, request.now_ms, self.config.half_life_days)
        });
        let recency = (candidate.features.recency_score + learned_recency).clamp(0.0, 1.0);
        let acceptance = stat.map_or(candidate.features.acceptance_rate, acceptance_rate);
        let correction = stat.map_or(candidate.features.correction_penalty, correction_rate);
        let context = candidate.features.context_score + bigram_score(inner, candidate, request);
        let pair_count = request
            .roman_input
            .as_deref()
            .and_then(|roman_input| PairKey::new(candidate.language, roman_input, &candidate.text))
            .and_then(|key| inner.pairs.get(&key))
            .map_or(0.0, |stat| stat.count as f32);
        let pair_frequency = candidate.features.pair_frequency as f32 + pair_count;

        let weights = &self.config.weights;
        let score = weights.base * candidate.features.base_score
            + weights.user_frequency * user_frequency.ln_1p()
            + weights.recency * recency
            + weights.context * context
            + weights.acceptance * acceptance
            - weights.correction * correction
            + weights.phonetic * candidate.features.phonetic_score
            + weights.pair_frequency * pair_frequency.ln_1p();

        if score.is_finite() {
            score
        } else {
            f32::NEG_INFINITY
        }
    }
}

impl Ranker for Smriti {
    fn rank(&self, request: &SuggestionRequest, candidates: Vec<Candidate>) -> Vec<Suggestion> {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let mut suggestions: Vec<Suggestion> = candidates
            .into_iter()
            .map(|candidate| {
                let score = self.score_with(&candidate, request, &inner);
                Suggestion {
                    text: candidate.text,
                    language: candidate.language,
                    score,
                    source: candidate.source,
                }
            })
            .collect();

        suggestions.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.text.cmp(&right.text))
        });
        suggestions.truncate(request.limit.max(1));
        suggestions
    }
}

fn record_word(
    inner: &mut Inner,
    language: Language,
    word: &str,
    previous_word: Option<&str>,
    timestamp_ms: u64,
    source: CommitSource,
) {
    let Some(word_key) = WordKey::new(language, word) else {
        return;
    };
    let word_text = word_key.text.clone();
    let stat = inner.words.entry(word_key).or_default();
    stat.count = stat.count.saturating_add(1);
    stat.last_used_ms = stat.last_used_ms.max(timestamp_ms);
    if source == CommitSource::AcceptedSuggestion {
        stat.accepted = stat.accepted.saturating_add(1);
    }

    let Some(previous) = previous_word.and_then(|value| WordKey::new(language, value)) else {
        return;
    };
    let key = BigramKey {
        language,
        first: previous.text,
        second: word_text,
    };
    let bigram = inner.bigrams.entry(key).or_default();
    bigram.count = bigram.count.saturating_add(1);
    bigram.last_used_ms = bigram.last_used_ms.max(timestamp_ms);
}

fn bigram_score(inner: &Inner, candidate: &Candidate, request: &SuggestionRequest) -> f32 {
    let Some(previous) = request.context.last() else {
        return 0.0;
    };
    let Some(previous_key) = WordKey::new(candidate.language, previous) else {
        return 0.0;
    };
    let Some(word_key) = WordKey::new(candidate.language, &candidate.text) else {
        return 0.0;
    };
    let key = BigramKey {
        language: candidate.language,
        first: previous_key.text,
        second: word_key.text,
    };
    inner
        .bigrams
        .get(&key)
        .map_or(0.0, |stat| stat.count as f32 / (stat.count as f32 + 5.0))
}

fn acceptance_rate(stat: &WordStat) -> f32 {
    if stat.shown == 0 {
        0.0
    } else {
        (stat.accepted as f32 / stat.shown as f32).clamp(0.0, 1.0)
    }
}

fn correction_rate(stat: &WordStat) -> f32 {
    let denominator = stat.count as f32 + stat.corrections as f32 + 1.0;
    (stat.corrections as f32 / denominator).clamp(0.0, 1.0)
}

fn recency_score(last_used_ms: u64, now_ms: u64, half_life_days: f32) -> f32 {
    if last_used_ms == 0 || now_ms == 0 || now_ms <= last_used_ms {
        return if last_used_ms == 0 { 0.0 } else { 1.0 };
    }
    let age_ms = now_ms.saturating_sub(last_used_ms) as f32;
    let half_life_ms = half_life_days * 86_400_000.0;
    if half_life_ms <= 0.0 {
        return 0.0;
    }
    (0.5_f32).powf(age_ms / half_life_ms).clamp(0.0, 1.0)
}

fn prune<K, V, S>(map: &mut HashMap<K, V>, cap: usize, score: S)
where
    K: Eq + std::hash::Hash + Clone,
    S: Fn(&V) -> (u64, u64),
{
    if map.len() <= cap {
        return;
    }

    let mut ranked: Vec<(K, (u64, u64))> = map
        .iter()
        .map(|(key, value)| (key.clone(), score(value)))
        .collect();
    ranked.sort_by_key(|left| std::cmp::Reverse(left.1));
    for (key, _) in ranked.into_iter().skip(cap) {
        map.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::{LearnError, Smriti, SmritiConfig};
    use crate::event::{CommitSource, LearningEvent};
    use okkhor_core::{
        Candidate, CandidateSource, Language, Ranker, RankingFeatures, SuggestionRequest,
    };

    fn bangla_candidate(text: &str, base_score: f32) -> Candidate {
        let mut candidate = Candidate::new(text, Language::Bangla, CandidateSource::Dictionary);
        candidate.features.base_score = base_score;
        candidate
    }

    fn commit(smriti: &Smriti, word: &str, timestamp_ms: u64) {
        smriti.observe(LearningEvent::WordCommitted {
            word: word.to_owned(),
            language: Language::Bangla,
            previous_word: None,
            timestamp_ms,
            source: CommitSource::Typed,
        });
    }

    fn request(now_ms: u64) -> SuggestionRequest {
        SuggestionRequest::new("ভা", Language::Bangla, 5).at(now_ms)
    }

    #[test]
    fn repeated_words_get_a_frequency_boost() {
        let smriti = Smriti::new();
        for _ in 0..10 {
            commit(&smriti, "ভালো", 1_000);
        }

        let ranked = smriti.rank(
            &request(2_000),
            vec![bangla_candidate("ভাল", 0.0), bangla_candidate("ভালো", 0.0)],
        );

        assert_eq!(ranked[0].text, "ভালো");
    }

    #[test]
    fn recent_words_beat_stale_words_at_equal_frequency() {
        let smriti = Smriti::new();
        let now = 100 * 86_400_000_u64;
        commit(&smriti, "নতুন", now - 3_600_000);
        commit(&smriti, "পুরোনো", now - 40 * 86_400_000);

        let ranked = smriti.rank(
            &request(now),
            vec![bangla_candidate("পুরোনো", 0.0), bangla_candidate("নতুন", 0.0)],
        );

        assert_eq!(ranked[0].text, "নতুন");
    }

    #[test]
    fn accepted_suggestions_raise_acceptance_rate() {
        let smriti = Smriti::new();
        for _ in 0..10 {
            smriti.observe(LearningEvent::SuggestionShown {
                word: "ভালো".to_owned(),
                language: Language::Bangla,
                timestamp_ms: 1_000,
            });
        }
        smriti.observe(LearningEvent::SuggestionAccepted {
            word: "ভালো".to_owned(),
            language: Language::Bangla,
            previous_word: None,
            timestamp_ms: 2_000,
        });

        let ranked = smriti.rank(
            &request(3_000),
            vec![bangla_candidate("ভালো", 0.0), bangla_candidate("ভাল", 0.0)],
        );

        assert_eq!(ranked[0].text, "ভালো");
        assert!(ranked[0].score > ranked[1].score);
    }

    #[test]
    fn transliteration_pairs_boost_the_chosen_output() {
        let smriti = Smriti::new();
        for _ in 0..5 {
            smriti.observe(LearningEvent::TransliterationCommitted {
                roman_input: "bhalo".to_owned(),
                output: "ভালো".to_owned(),
                language: Language::Bangla,
                previous_word: None,
                timestamp_ms: 1_000,
            });
        }

        let ranked = smriti.rank(
            &SuggestionRequest::new("ভা", Language::Bangla, 5)
                .with_roman_input("bhalo")
                .at(2_000),
            vec![bangla_candidate("ভাল", 0.0), bangla_candidate("ভালো", 0.0)],
        );

        assert_eq!(ranked[0].text, "ভালো");
    }

    #[test]
    fn next_words_returns_the_most_frequent_followers() {
        let smriti = Smriti::new();
        for _ in 0..3 {
            smriti.observe(LearningEvent::WordCommitted {
                word: "ভালো".to_owned(),
                language: Language::Bangla,
                previous_word: Some("আমি".to_owned()),
                timestamp_ms: 1_000,
                source: CommitSource::Typed,
            });
        }
        smriti.observe(LearningEvent::WordCommitted {
            word: "খারাপ".to_owned(),
            language: Language::Bangla,
            previous_word: Some("আমি".to_owned()),
            timestamp_ms: 1_000,
            source: CommitSource::Typed,
        });
        let rows = smriti.next_words(Language::Bangla, "আমি", 3);
        assert_eq!(rows.first().map(|(word, _)| word.as_str()), Some("ভালো"));
        assert!(rows.iter().any(|(word, _)| word == "খারাপ"));
        assert!(smriti.next_words(Language::Bangla, "নেই", 3).is_empty());
    }

    #[test]
    fn context_bigrams_boost_expected_next_words() {
        let smriti = Smriti::new();
        commit(&smriti, "আমি", 1_000);
        for _ in 0..10 {
            smriti.observe(LearningEvent::WordCommitted {
                word: "ভালো".to_owned(),
                language: Language::Bangla,
                previous_word: Some("আমি".to_owned()),
                timestamp_ms: 2_000,
                source: CommitSource::Typed,
            });
        }

        let ranked = smriti.rank(
            &SuggestionRequest::new("ভা", Language::Bangla, 5)
                .with_context(["আমি"])
                .at(3_000),
            vec![bangla_candidate("খারাপ", 0.0), bangla_candidate("ভালো", 0.0)],
        );

        assert_eq!(ranked[0].text, "ভালো");
    }

    #[test]
    fn corrections_penalize_the_replaced_word() {
        let smriti = Smriti::new();
        for _ in 0..5 {
            commit(&smriti, "ভাল", 1_000);
            smriti.observe(LearningEvent::Correction {
                from: "ভাল".to_owned(),
                to: "ভালো".to_owned(),
                language: Language::Bangla,
                timestamp_ms: 2_000,
            });
        }

        let ranked = smriti.rank(
            &request(3_000),
            vec![bangla_candidate("ভাল", 0.0), bangla_candidate("ভালো", 0.0)],
        );

        assert_eq!(ranked[0].text, "ভালো");
    }

    #[test]
    fn snapshots_restore_the_same_ranking() {
        let smriti = Smriti::new();
        for _ in 0..10 {
            commit(&smriti, "ভালো", 1_000);
        }
        let snapshot = smriti.snapshot();
        assert!(snapshot.word_count() > 0);

        let restored =
            Smriti::from_snapshot(snapshot, SmritiConfig::default()).expect("valid snapshot");
        let ranked = restored.rank(
            &request(2_000),
            vec![bangla_candidate("ভাল", 0.0), bangla_candidate("ভালো", 0.0)],
        );
        assert_eq!(ranked[0].text, "ভালো");
    }

    #[test]
    fn maintain_enforces_word_caps() {
        let config = SmritiConfig {
            max_words: 2,
            ..SmritiConfig::default()
        };
        let smriti = Smriti::with_config(config).expect("valid config");
        for word in ["এক", "দুই", "তিন"] {
            commit(&smriti, word, 1_000);
        }
        assert_eq!(smriti.word_count(), 3);
        smriti.maintain();
        assert_eq!(smriti.word_count(), 2);
    }

    #[test]
    fn empty_words_are_ignored() {
        let smriti = Smriti::new();
        commit(&smriti, "   ", 1_000);
        assert_eq!(smriti.word_count(), 0);
    }

    #[test]
    fn invalid_snapshot_schema_is_rejected() {
        let smriti = Smriti::new();
        let mut snapshot = smriti.snapshot();
        // Only the schema guard is under test; the snapshot stays otherwise valid.
        snapshot.schema_version = 999;
        let error = Smriti::from_snapshot(snapshot, SmritiConfig::default())
            .expect_err("schema must be rejected");
        assert!(matches!(
            error,
            LearnError::UnsupportedSchema { found: 999, .. }
        ));
    }

    #[test]
    fn ranking_features_are_preserved() {
        let smriti = Smriti::new();
        let mut candidate = bangla_candidate("ভালো", 1.0);
        candidate.features = RankingFeatures {
            base_score: 1.0,
            user_frequency: 3,
            recency_score: 0.5,
            context_score: 0.25,
            acceptance_rate: 0.5,
            correction_penalty: 0.0,
            phonetic_score: 0.75,
            pair_frequency: 2,
        };
        let score = smriti.score(&candidate, &request(2_000));
        assert!(score.is_finite());
        assert!(score > 0.0);
    }

    #[test]
    fn score_pool_preserves_input_order_and_matches_score() {
        let smriti = Smriti::new();
        for _ in 0..5 {
            commit(&smriti, "ভালো", 1_000);
        }
        let candidates = vec![
            bangla_candidate("খারাপ", 0.0),
            bangla_candidate("ভালো", 0.0),
            bangla_candidate("ভাল", 0.0),
        ];
        let pool = smriti.score_pool(&request(2_000), &candidates);
        assert_eq!(pool.len(), candidates.len());
        for (index, candidate) in candidates.iter().enumerate() {
            assert_eq!(pool[index], smriti.score(candidate, &request(2_000)));
        }
    }

    #[test]
    fn score_pool_empty_pool_is_empty() {
        let smriti = Smriti::new();
        assert!(smriti.score_pool(&request(2_000), &[]).is_empty());
    }

    #[test]
    fn score_pool_ties_keep_input_order() {
        let smriti = Smriti::new();
        let candidates = vec![
            bangla_candidate("গ", 0.0),
            bangla_candidate("খ", 0.0),
            bangla_candidate("ক", 0.0),
        ];
        let pool = smriti.score_pool(&request(2_000), &candidates);
        // Identical candidates with no learned history tie exactly.
        assert!(pool.windows(2).all(|pair| pair[0] == pair[1]));
        // `rank` still applies its pinned (score desc, text asc) tie-break.
        let ranked = smriti.rank(&request(2_000), candidates);
        let texts: Vec<&str> = ranked.iter().map(|row| row.text.as_str()).collect();
        assert_eq!(texts, ["ক", "খ", "গ"]);
    }
}
