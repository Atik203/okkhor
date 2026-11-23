//! Live suggestion engine (0.6).
//!
//! Stage order is normative for the daemon (ADR-0016): curated exact, user
//! exact, autocorrect exact, rule output and explicit-vowel variant,
//! dictionary exact, dictionary prefix, suffix expansion, merge/dedup,
//! `Smriti` rank, top N. The query is bounded, performs no I/O, reads no
//! clock, and never panics on bad data.

pub mod index;

use okkhor_core::{Candidate, CandidateSource, Language, Ranker, Suggestion, SuggestionRequest};
use okkhor_learn::Smriti;
use okkhor_transliterate::{
    PhoneticTransliterator, TransliterationOptions, TransliterationOutput, TransliteratorEngine,
};

pub use index::{IndexCounts, IndexError, SuggestionIndex};

/// Hard cap on candidates entering the ranker.
pub const MAX_STAGED: usize = 64;
/// Hard cap on returned rows (the popup supports ten digits).
pub const MAX_LIMIT: usize = 10;
/// Rule outputs considered for dictionary expansion.
const EXPAND_OUTPUTS: usize = 2;
/// Prefix hits collected per rule output.
const PREFIX_HITS: usize = 40;
/// Popularity-scan bound for one prefix (falls back to the text range).
const PREFIX_SCAN: usize = 2_000;
/// Longest candidate accepted (UTF-8 bytes).
const MAX_TEXT_BYTES: usize = 96;

/// One user dictionary entry (roman → Bangla).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserWord {
    pub roman: String,
    pub bangla: String,
}

/// Input to one suggestion query.
#[derive(Debug, Clone, Copy)]
pub struct SuggestRequest<'a> {
    pub roman: &'a str,
    pub limit: usize,
    pub previous_word: Option<&'a str>,
    pub now_ms: u64,
    pub user_words: &'a [UserWord],
}

impl<'a> SuggestRequest<'a> {
    pub fn new(roman: &'a str, limit: usize) -> Self {
        Self {
            roman,
            limit,
            previous_word: None,
            now_ms: 0,
            user_words: &[],
        }
    }

    pub fn with_previous(mut self, previous: &'a str) -> Self {
        self.previous_word = Some(previous);
        self
    }

    pub fn at(mut self, now_ms: u64) -> Self {
        self.now_ms = now_ms;
        self
    }

    pub fn with_user_words(mut self, words: &'a [UserWord]) -> Self {
        self.user_words = words;
        self
    }
}

/// Engine construction failures (the index failure is degraded, not fatal).
#[derive(Debug, thiserror::Error)]
pub enum SuggestionError {
    #[error("the phonetic table could not be loaded")]
    Transliterator,
}

/// One live suggestion engine. Owns the parsed index and the phonetic table.
#[derive(Debug)]
pub struct SuggestionEngine {
    index: Option<SuggestionIndex>,
    index_error: Option<&'static str>,
    transliterator: PhoneticTransliterator,
}

impl SuggestionEngine {
    /// Build from the embedded phonetic table and suggestion index. An index
    /// failure keeps the engine usable on the rule output alone.
    pub fn from_embedded() -> Result<Self, SuggestionError> {
        let transliterator = PhoneticTransliterator::from_embedded_avro()
            .map_err(|_| SuggestionError::Transliterator)?;
        let (index, index_error) = match SuggestionIndex::from_embedded() {
            Ok(index) => (Some(index), None),
            Err(error) => (None, Some(error.kind())),
        };
        Ok(Self {
            index,
            index_error,
            transliterator,
        })
    }

    /// Rule-only engine for tests and the corrupt-index fallback path.
    pub fn without_index() -> Result<Self, SuggestionError> {
        let transliterator = PhoneticTransliterator::from_embedded_avro()
            .map_err(|_| SuggestionError::Transliterator)?;
        Ok(Self {
            index: None,
            index_error: Some("index_unavailable"),
            transliterator,
        })
    }

    /// Static kind of the index failure, if any (for the startup log).
    pub fn index_error(&self) -> Option<&'static str> {
        self.index_error
    }

    pub fn index(&self) -> Option<&SuggestionIndex> {
        self.index.as_ref()
    }

    /// Bangla suggestions for a roman buffer.
    pub fn suggest(&self, request: &SuggestRequest<'_>, smriti: &Smriti) -> Vec<Suggestion> {
        let roman = request.roman.trim();
        if roman.is_empty() {
            return Vec::new();
        }
        let limit = request.limit.clamp(1, MAX_LIMIT);
        let mut staged: Vec<Candidate> = Vec::with_capacity(MAX_STAGED);

        if let Some(index) = &self.index {
            self.stage_curated(index, roman, &mut staged);
            self.stage_user_words(request.user_words, roman, &mut staged);
            self.stage_roman(index, roman, &mut staged);
            self.stage_autocorrect(index, roman, &mut staged);
        }

        let outputs = self.transliterate(roman);
        if let Some(index) = &self.index {
            self.stage_rule(index, roman, &outputs, &mut staged);
            self.stage_vowel_variant(index, roman, &mut staged);
            self.stage_suffix(index, roman, &outputs, &mut staged);
        } else {
            self.stage_rule_only(&outputs, &mut staged);
        }

        if staged.is_empty() {
            return Vec::new();
        }

        let mut suggestion_request =
            SuggestionRequest::new(roman, Language::Bangla, limit).at(request.now_ms);
        if let Some(previous) = request.previous_word {
            suggestion_request = suggestion_request.with_context([previous]);
        }
        suggestion_request = suggestion_request.with_roman_input(roman);
        smriti.rank(&suggestion_request, staged)
    }

    /// English prefix completions, lowercase. The caller preserves the token's
    /// capitalization and never auto-replaces.
    pub fn suggest_english(&self, prefix: &str, limit: usize) -> Vec<String> {
        let Some(index) = &self.index else {
            return Vec::new();
        };
        let prefix = prefix.trim().to_ascii_lowercase();
        if prefix.chars().count() < 2 {
            return Vec::new();
        }
        let limit = limit.clamp(1, MAX_LIMIT);
        let mut scored: Vec<(f32, &str)> = index
            .prefix_english(&prefix, MAX_LIMIT * 40, PREFIX_SCAN)
            .into_iter()
            .filter_map(|(position, zipf)| {
                let text = index.english_text(position)?;
                if text == prefix {
                    return None;
                }
                let extra = text.chars().count().saturating_sub(prefix.chars().count());
                let score = 0.50 + popularity(zipf) * 0.45 - (extra as f32 * 0.01);
                Some((score, text))
            })
            .collect();
        scored.sort_by(|left, right| right.0.total_cmp(&left.0).then_with(|| left.1.cmp(right.1)));
        let mut out = Vec::with_capacity(limit);
        for (_, text) in scored {
            if !out.iter().any(|existing| existing == text) {
                out.push(text.to_owned());
            }
            if out.len() >= limit {
                break;
            }
        }
        out
    }

    fn transliterate(&self, roman: &str) -> Vec<TransliterationOutput> {
        self.transliterator.transliterate(
            roman,
            &TransliterationOptions {
                max_candidates: 8,
                allow_partial: true,
            },
        )
    }

    fn stage_curated(&self, index: &SuggestionIndex, roman: &str, staged: &mut Vec<Candidate>) {
        if let Some(output) = index.curated_exact(roman) {
            push(staged, output, 1.30, 1.0, CandidateSource::Dictionary);
        }
        for (_key, output) in index.curated_prefix(roman, 8) {
            push(staged, output, 0.95, 0.70, CandidateSource::Dictionary);
        }
    }

    fn stage_user_words(&self, words: &[UserWord], roman: &str, staged: &mut Vec<Candidate>) {
        for word in words {
            if word.roman == roman {
                push(
                    staged,
                    &word.bangla,
                    1.25,
                    1.0,
                    CandidateSource::UserHistory,
                );
            } else if word.roman.starts_with(roman) {
                push(
                    staged,
                    &word.bangla,
                    0.90,
                    0.70,
                    CandidateSource::UserHistory,
                );
            }
        }
    }

    fn stage_roman(&self, index: &SuggestionIndex, roman: &str, staged: &mut Vec<Candidate>) {
        let lowered = roman.to_ascii_lowercase();
        for value in index.exact_roman(&lowered) {
            let base = match index.exact_bangla(value) {
                Some((_, zipf)) if zipf > 0 => 1.05 + popularity(zipf) * 0.08,
                Some(_) => 0.98,
                None => 0.90,
            };
            push(staged, value, base, 0.85, CandidateSource::Dictionary);
        }
        if lowered.chars().count() < 2 {
            return;
        }
        let mut hits = index.prefix_roman(&lowered, 240, PREFIX_SCAN);
        hits.retain(|(key, _)| *key != lowered);
        let mut scored: Vec<(u16, usize, &str)> = hits
            .into_iter()
            .map(|(key, value)| {
                let zipf = index.exact_bangla(value).map_or(0, |(_, zipf)| zipf);
                (zipf, key.len(), value)
            })
            .collect();
        scored.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(right.2))
        });
        for (zipf, _key_len, value) in scored.into_iter().take(PREFIX_HITS) {
            let base = if zipf > 0 {
                0.68 + popularity(zipf) * 0.22
            } else {
                0.55
            };
            push(staged, value, base, 0.80, CandidateSource::Dictionary);
        }
    }

    fn stage_autocorrect(&self, index: &SuggestionIndex, roman: &str, staged: &mut Vec<Candidate>) {
        let Some(canonical) = index.autocorrect(roman) else {
            return;
        };
        if canonical == roman {
            return;
        }
        for output in self.transliterate(canonical).into_iter().take(3) {
            let base = match index.exact_bangla(&output.text) {
                Some((_, zipf)) => 1.15 + popularity(zipf) * 0.05,
                None => 1.10,
            };
            push(
                staged,
                &output.text,
                base,
                0.90,
                CandidateSource::Transliteration,
            );
        }
    }

    fn stage_rule(
        &self,
        index: &SuggestionIndex,
        roman: &str,
        outputs: &[TransliterationOutput],
        staged: &mut Vec<Candidate>,
    ) {
        for (position, output) in outputs.iter().enumerate() {
            let exact = index.exact_bangla(&output.text);
            let base = match exact {
                Some((_, zipf)) => 1.00 + popularity(zipf) * 0.08,
                None => 0.84,
            };
            push(
                staged,
                &output.text,
                base,
                output.confidence,
                CandidateSource::Transliteration,
            );

            if position >= EXPAND_OUTPUTS || roman.chars().count() < 2 {
                continue;
            }
            let mut hits = index.prefix_bangla(&output.text, PREFIX_HITS * 6, PREFIX_SCAN);
            hits.sort_by_key(|left| std::cmp::Reverse(left.1));
            for (word_index, zipf) in hits.into_iter().take(PREFIX_HITS) {
                let Some(text) = index.bangla_text(word_index) else {
                    continue;
                };
                let extra = text
                    .chars()
                    .count()
                    .saturating_sub(output.text.chars().count());
                let base = (0.58 + popularity(zipf) * 0.28 - extra as f32 * 0.01).clamp(0.25, 0.92);
                push(staged, text, base, 0.60, CandidateSource::Dictionary);
            }
        }
    }

    fn stage_rule_only(&self, outputs: &[TransliterationOutput], staged: &mut Vec<Candidate>) {
        for output in outputs {
            push(
                staged,
                &output.text,
                1.00,
                output.confidence,
                CandidateSource::Transliteration,
            );
        }
    }

    fn stage_vowel_variant(
        &self,
        index: &SuggestionIndex,
        roman: &str,
        staged: &mut Vec<Candidate>,
    ) {
        let Some(stem) = roman.strip_suffix('o') else {
            return;
        };
        if stem.is_empty() {
            return;
        }
        let variant = format!("{stem}O");
        for output in self.transliterate(&variant).into_iter().take(3) {
            let base = match index.exact_bangla(&output.text) {
                Some((_, zipf)) => 1.08 + popularity(zipf) * 0.05,
                None => 0.90,
            };
            push(
                staged,
                &output.text,
                base,
                0.90,
                CandidateSource::Transliteration,
            );
        }
    }

    fn stage_suffix(
        &self,
        index: &SuggestionIndex,
        roman: &str,
        outputs: &[TransliterationOutput],
        staged: &mut Vec<Candidate>,
    ) {
        let Some(suffix) = index.longest_suffix(roman) else {
            return;
        };
        if suffix.is_empty() || starts_with_independent_vowel(suffix) {
            return;
        }
        for output in outputs.iter().take(2) {
            if output.text.ends_with(suffix) {
                continue;
            }
            let text = format!("{}{}", output.text, suffix);
            let base = match index.exact_bangla(&text) {
                Some((_, zipf)) => 0.88 + popularity(zipf) * 0.05,
                None => 0.70,
            };
            push(staged, &text, base, 0.50, CandidateSource::Transliteration);
        }
    }
}

fn push(
    staged: &mut Vec<Candidate>,
    text: &str,
    base: f32,
    phonetic: f32,
    source: CandidateSource,
) {
    if text.is_empty() || text.len() > MAX_TEXT_BYTES {
        return;
    }
    let base = base.clamp(0.0, 2.0);
    let phonetic = phonetic.clamp(0.0, 1.0);
    if let Some(existing) = staged.iter_mut().find(|candidate| candidate.text == text) {
        if base > existing.features.base_score {
            existing.features.base_score = base;
            existing.features.phonetic_score = phonetic;
            existing.source = source;
        }
        return;
    }
    if staged.len() >= MAX_STAGED {
        return;
    }
    let mut candidate = Candidate::new(text, Language::Bangla, source);
    candidate.features.base_score = base;
    candidate.features.phonetic_score = phonetic;
    staged.push(candidate);
}

fn popularity(zipf: u16) -> f32 {
    f32::from(zipf) / 800.0
}

/// Suffixes starting with an independent vowel (`অ`..`ঔ`) are grammatical
/// words, not kars; appending them produces noise such as `করছও`.
fn starts_with_independent_vowel(suffix: &str) -> bool {
    matches!(suffix.chars().next(), Some('\u{0985}'..='\u{0994}'))
}

impl index::IndexError {
    /// Static kind for startup logs (never dictionary content).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Truncated => "index_truncated",
            Self::BadMagic => "index_bad_magic",
            Self::BadVersion => "index_bad_version",
            Self::BadSection => "index_bad_section",
            Self::BadText => "index_bad_text",
            Self::BadOrder => "index_bad_order",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SuggestRequest, SuggestionEngine};
    use okkhor_learn::Smriti;

    fn engine() -> SuggestionEngine {
        SuggestionEngine::from_embedded().expect("engine")
    }

    fn texts(suggestions: &[okkhor_core::Suggestion]) -> Vec<&str> {
        suggestions.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn common_words_are_in_the_top_rows() {
        let engine = engine();
        let smriti = Smriti::new();
        for (roman, expected) in [
            ("amar", "আমার"),
            ("ami", "আমি"),
            ("tumi", "তুমি"),
            ("apni", "আপনি"),
            ("bhalo", "ভালো"),
            ("dhonnobad", "ধন্যবাদ"),
            ("kemon", "কেমন"),
            ("korcho", "করছো"),
            ("bangla", "বাংলা"),
            ("computer", "কম্পিউটার"),
            ("achhi", "আছি"),
            ("ghor", "ঘর"),
            ("mangsho", "মাংস"),
            ("raat", "রাত"),
            ("naam", "নাম"),
            ("jabo", "যাবো"),
            ("koro", "করো"),
            ("korechi", "করেছি"),
        ] {
            let result = engine.suggest(&SuggestRequest::new(roman, 8), &smriti);
            let rows = texts(&result);
            assert!(
                rows.iter().take(5).any(|row| *row == expected),
                "{roman} -> {rows:?}"
            );
        }
    }

    #[test]
    fn limit_is_clamped_and_deterministic() {
        let engine = engine();
        let smriti = Smriti::new();
        let first = engine.suggest(&SuggestRequest::new("bhal", 8), &smriti);
        let second = engine.suggest(&SuggestRequest::new("bhal", 8), &smriti);
        assert_eq!(texts(&first), texts(&second));
        assert!(first.len() <= 8);
        assert!(!first.is_empty());
    }

    #[test]
    fn prefix_expands_the_dictionary() {
        let engine = engine();
        let smriti = Smriti::new();
        let rows = texts(&engine.suggest(&SuggestRequest::new("bhal", 10), &smriti)).join(" ");
        assert!(rows.contains("ভাল"), "{rows}");
    }

    #[test]
    fn user_words_rank_first() {
        use super::UserWord;

        let engine = engine();
        let smriti = Smriti::new();
        let words = [UserWord {
            roman: "myword".to_owned(),
            bangla: "মাইওয়ার্ড".to_owned(),
        }];
        let rows = engine.suggest(
            &SuggestRequest::new("myword", 8).with_user_words(&words),
            &smriti,
        );
        assert_eq!(rows.first().map(|row| row.text.as_str()), Some("মাইওয়ার্ড"));
    }

    #[test]
    fn english_prefix_completions_work() {
        let engine = engine();
        let rows = engine.suggest_english("hel", 8);
        assert!(rows.iter().any(|row| row == "hello"), "{rows:?}");
        assert!(rows.iter().any(|row| row == "help"), "{rows:?}");
        assert!(engine.suggest_english("a", 8).is_empty());
    }

    #[test]
    fn empty_query_returns_nothing() {
        let engine = engine();
        let smriti = Smriti::new();
        assert!(
            engine
                .suggest(&SuggestRequest::new("", 8), &smriti)
                .is_empty()
        );
    }

    #[test]
    fn quality_and_latency_probe() {
        let engine = engine();
        let smriti = Smriti::new();
        let words = [
            "amar",
            "ami",
            "tumi",
            "apni",
            "kemon",
            "acho",
            "achen",
            "achhi",
            "chilo",
            "hobe",
            "holo",
            "hoy",
            "hoye",
            "kore",
            "korte",
            "korechi",
            "korbo",
            "korcho",
            "koro",
            "bhalo",
            "kharap",
            "shundor",
            "valobasha",
            "dhonnobad",
            "bari",
            "ghor",
            "school",
            "college",
            "office",
            "bazar",
            "rasta",
            "desh",
            "shohor",
            "gram",
            "ma",
            "baba",
            "bhai",
            "bon",
            "didi",
            "chele",
            "meye",
            "bondhu",
            "khabar",
            "pani",
            "cha",
            "bhat",
            "mach",
            "mangsho",
            "dim",
            "dudh",
            "aj",
            "kal",
            "shomoy",
            "shokale",
            "bikale",
            "rate",
            "din",
            "raat",
            "onek",
            "khub",
            "kom",
            "beshi",
            "ar",
            "abar",
            "tahole",
            "kintu",
            "tobe",
            "kotha",
            "proshno",
            "uttor",
            "naam",
            "kaj",
            "porichoy",
            "bangla",
            "bangladesh",
            "dhaka",
            "kolkata",
            "computer",
            "mobile",
            "internet",
            "facebook",
            "youtube",
            "google",
            "email",
            "phone",
            "keyboard",
            "mouse",
            "ki",
            "keno",
            "kothay",
            "kobe",
            "kivabe",
            "kar",
            "koto",
            "kon",
            "ha",
            "na",
            "hae",
            "thik",
            "achi",
            "gelam",
            "aschi",
            "jabo",
        ];
        for word in words {
            let suggestions = engine.suggest(&SuggestRequest::new(word, 8), &smriti);
            let rows = texts(&suggestions);
            eprintln!("{word:>12} -> {}", rows.join(" | "));
        }

        let prefixes = [
            "a", "am", "bha", "bhal", "k", "ko", "kor", "dho", "dhonno", "com", "valo",
        ];
        let iterations = 2_000usize;
        let mut samples = Vec::with_capacity(iterations);
        for index in 0..iterations {
            let prefix = prefixes[index % prefixes.len()];
            let start = std::time::Instant::now();
            let _ = engine.suggest(&SuggestRequest::new(prefix, 8), &smriti);
            samples.push(start.elapsed().as_secs_f64() * 1_000.0);
        }
        samples.sort_by(|a, b| a.total_cmp(b));
        let p = |q: f64| samples[((samples.len() - 1) as f64 * q) as usize];
        eprintln!(
            "suggest latency ms: p50={:.3} p95={:.3} p99={:.3} max={:.3}",
            p(0.50),
            p(0.95),
            p(0.99),
            samples[samples.len() - 1]
        );
    }
}
