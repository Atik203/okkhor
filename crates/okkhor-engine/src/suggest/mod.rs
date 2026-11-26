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

/// Shipped spelling corrections. Format and policy: see the file header.
const CURATED_OVERLAY: &str = include_str!("../../assets/suggest/curated-overlay.tsv");

/// Parse `roman<TAB>bangla` rows, skipping comments and blank lines.
fn parse_overlay(source: &str) -> Vec<(String, String)> {
    source
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (roman, bangla) = line.split_once('\t')?;
            let (roman, bangla) = (roman.trim(), bangla.trim());
            if roman.is_empty() || bangla.is_empty() {
                return None;
            }
            Some((roman.to_owned(), bangla.to_owned()))
        })
        .collect()
}

/// One live suggestion engine. Owns the parsed index and the phonetic table.

#[derive(Debug)]
pub struct SuggestionEngine {
    index: Option<SuggestionIndex>,
    index_error: Option<&'static str>,
    transliterator: PhoneticTransliterator,
    /// Shipped roman -> Bangla corrections for words the rule table and the
    /// licensed aliases both get wrong. Sorted, bounded, built once.
    overlay: Vec<(String, String)>,
}

impl SuggestionEngine {
    /// Build from the embedded phonetic table and suggestion index. An index
    /// failure keeps the engine usable on the rule output alone.
    pub fn from_embedded() -> Result<Self, SuggestionError> {
        let transliterator = PhoneticTransliterator::from_embedded_avro()
            .map_err(|_| SuggestionError::Transliterator)?;
        Ok(Self::with_transliterator(transliterator))
    }

    /// Build from an alternate embedded rule table (Khipro, Phonetic/Banglish)
    /// and the shared suggestion index. Table compilation happens once at
    /// startup; the index is unchanged because it is roman-shaped, not
    /// layout-specific. A malformed table is a typed error, never a panic.
    pub fn from_embedded_table(table_json: &str) -> Result<Self, SuggestionError> {
        let transliterator = PhoneticTransliterator::from_asset_json(table_json)
            .map_err(|_| SuggestionError::Transliterator)?;
        Ok(Self::with_transliterator(transliterator))
    }

    fn with_transliterator(transliterator: PhoneticTransliterator) -> Self {
        let (index, index_error) = match SuggestionIndex::from_embedded() {
            Ok(index) => (Some(index), None),
            Err(error) => (None, Some(error.kind())),
        };
        Self {
            index,
            index_error,
            transliterator,
            overlay: parse_overlay(CURATED_OVERLAY),
        }
    }

    /// Rule-only engine for tests and the corrupt-index fallback path.
    pub fn without_index() -> Result<Self, SuggestionError> {
        let transliterator = PhoneticTransliterator::from_embedded_avro()
            .map_err(|_| SuggestionError::Transliterator)?;
        Ok(Self {
            index: None,
            index_error: Some("index_unavailable"),
            transliterator,
            overlay: parse_overlay(CURATED_OVERLAY),
        })
    }

    /// Shipped correction for `roman`, if any. Quality tools use this as the
    /// authoritative expectation; the engine already stages it first.
    pub fn overlay_target(&self, roman: &str) -> Option<&str> {
        self.overlay
            .iter()
            .find(|(key, _)| key == roman)
            .map(|(_, value)| value.as_str())
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
        self.suggest_with_ranker(request, smriti)
    }

    /// Bangla suggestions for a roman buffer, with an injected ranker.
    ///
    /// The daemon passes the deterministic `Smriti` baseline or a hybrid
    /// reranker; the staging body is identical and the injected ranker owns
    /// only the final ordering and truncation.
    pub fn suggest_with_ranker(
        &self,
        request: &SuggestRequest<'_>,
        ranker: &dyn Ranker,
    ) -> Vec<Suggestion> {
        let roman = request.roman.trim();
        if roman.is_empty() {
            return Vec::new();
        }
        let limit = request.limit.clamp(1, MAX_LIMIT);
        let mut staged: Vec<Candidate> = Vec::with_capacity(MAX_STAGED);

        self.stage_overlay(roman, &mut staged);
        // Avro capitals are semantic (T -> ট, D -> ড, R -> ড়, ...). The index
        // keys are lowercase, so a capitalized key must not be answered with
        // the lowercase word: the rule path owns this query.
        let semantic_capital = roman
            .chars()
            .any(|ch| matches!(ch, 'T' | 'D' | 'N' | 'S' | 'R' | 'I' | 'U' | 'O' | 'Y'));
        if let Some(index) = &self.index {
            if !semantic_capital {
                self.stage_curated(index, roman, &mut staged);
            }
            self.stage_user_words(request.user_words, roman, &mut staged);
            if !semantic_capital {
                self.stage_roman(index, roman, &mut staged);
                self.stage_typo(index, roman, &mut staged);
                self.stage_autocorrect(index, roman, &mut staged);
            }
        }

        let outputs = self.transliterate(roman);
        if let Some(index) = &self.index {
            self.stage_rule(index, roman, &outputs, &mut staged);
            self.stage_vowel_variant(index, roman, &mut staged);
            self.stage_suffix(index, roman, &outputs, &mut staged);
        } else {
            self.stage_rule_only(&outputs, &mut staged);
        }

        // Keep the Latin token selectable, like the legacy keyboard apps. A
        // dictionary-backed Bangla list ranks it low; when the only Bangla
        // output is a rule guess, the raw token (and a few English prefixes)
        // becomes the honest fallback instead of a nonsense word.
        if roman.is_ascii() {
            let has_dictionary = staged.iter().any(|candidate| {
                matches!(
                    candidate.source,
                    CandidateSource::Dictionary | CandidateSource::UserHistory
                )
            });
            // Keep it low: the Bangla/rule rows stay the primary product and
            // the raw token is the always-available escape hatch (promoted to
            // the last row after ranking if truncation would drop it).
            let raw_base = 0.45;
            keep_room(&mut staged, roman);
            push_lang(
                &mut staged,
                roman,
                Language::English,
                raw_base,
                0.30,
                CandidateSource::RawInput,
            );
            if !has_dictionary && !roman.is_empty() {
                for (position, text) in self.suggest_english(roman, 3).into_iter().enumerate() {
                    keep_room(&mut staged, &text);
                    let base = 0.50 - (position as f32 * 0.02);
                    push_lang(
                        &mut staged,
                        &text,
                        Language::English,
                        base,
                        0.25,
                        CandidateSource::RawInput,
                    );
                }
            }
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
        let mut ranked = ranker.rank(&suggestion_request, staged);
        // Last-resort escape hatch: keep the Latin token on the last row even
        // when a full Bangla list outranks it. Kontho and Bongo both keep the
        // typed word selectable; it costs one row and never changes the top.
        if roman.is_ascii()
            && limit >= 2
            && !ranked.iter().any(|suggestion| suggestion.text == roman)
        {
            let raw = Suggestion {
                text: roman.to_owned(),
                language: Language::English,
                score: 0.0,
                source: CandidateSource::RawInput,
            };
            if ranked.len() >= limit {
                if let Some(last) = ranked.last_mut() {
                    *last = raw;
                }
            } else {
                ranked.push(raw);
            }
        }
        ranked
    }

    /// English prefix completions, lowercase. The caller preserves the token's
    /// capitalization and never auto-replaces.
    pub fn suggest_english(&self, prefix: &str, limit: usize) -> Vec<String> {
        let Some(index) = &self.index else {
            return Vec::new();
        };
        let prefix = prefix.trim().to_ascii_lowercase();
        // One letter is enough; an empty prefix would scan the whole English
        // index, so keep that case empty. The scan and hit caps below bound a
        // one-letter lookup to the same budget as a longer prefix.
        if prefix.is_empty() {
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

    /// Stage the shipped corrections first: they carry the curated score and
    /// resolve words where both the rule table and the licensed aliases give a
    /// different everyday spelling.
    fn stage_overlay(&self, roman: &str, staged: &mut Vec<Candidate>) {
        for (key, value) in &self.overlay {
            if key == roman {
                push(staged, value, 1.30, 1.0, CandidateSource::Dictionary);
                break;
            }
        }
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
                // An explicit user mapping always wins over curated and
                // dictionary rows. This is the owner's `ammar -> আমার` case:
                // the user added it, so it must be the first row.
                push(
                    staged,
                    &word.bangla,
                    1.98,
                    1.0,
                    CandidateSource::UserHistory,
                );
            } else if word.roman.starts_with(roman) {
                // Prefix matches from the user's own list still outrank the
                // generic dictionary prefix stage.
                push(
                    staged,
                    &word.bangla,
                    1.10,
                    0.80,
                    CandidateSource::UserHistory,
                );
            }
        }
    }

    /// Recover the common "typed two letters in the wrong order" case.
    ///
    /// Only adjacent swaps of a 3–11 character roman key are considered, and
    /// the pool is capped so this can never dominate the list. A real exact
    /// match still outranks it.
    fn stage_typo(&self, index: &SuggestionIndex, roman: &str, staged: &mut Vec<Candidate>) {
        let lowered = roman.to_ascii_lowercase();
        let chars: Vec<char> = lowered.chars().collect();
        if !(3..=11).contains(&chars.len()) {
            return;
        }
        let mut added = 0usize;
        for position in 0..chars.len() - 1 {
            if chars[position] == chars[position + 1] {
                continue;
            }
            let mut swapped = chars.clone();
            swapped.swap(position, position + 1);
            let variant: String = swapped.into_iter().collect();
            for value in index.exact_roman_targets(&variant).into_iter().take(2) {
                let base = if position == 0 { 0.88 } else { 0.92 };
                push(staged, value, base, 0.55, CandidateSource::Dictionary);
                added += 1;
                if added >= 16 {
                    return;
                }
            }
        }
    }

    fn stage_roman(&self, index: &SuggestionIndex, roman: &str, staged: &mut Vec<Candidate>) {
        let lowered = roman.to_ascii_lowercase();
        // v2 keeps every target of an alias in preferred-rank order. Rank 0
        // keeps the old exact score; later ranks lose a small bounded amount
        // so the authored preference is visible without letting a rare
        // alternative displace a clearly better row.
        for (rank, value) in index.exact_roman_targets(&lowered).into_iter().enumerate() {
            let base = match index.exact_bangla(value) {
                Some((_, zipf)) if zipf > 0 => 1.05 + popularity(zipf) * 0.08,
                Some(_) => 0.98,
                None => 0.90,
            };
            let penalty = 0.015 * rank.min(4) as f32;
            push(
                staged,
                value,
                (base - penalty).max(0.30),
                0.85,
                CandidateSource::Dictionary,
            );
        }
        // Include the first letter. `prefix_roman` is bounded by 240 hits and
        // a 2000-entry scan, so a one-letter prefix stays inside the latency
        // budget while giving the owner dictionary rows immediately.
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
        _roman: &str,
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

            if position >= EXPAND_OUTPUTS {
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
    push_lang(staged, text, Language::Bangla, base, phonetic, source);
}

/// Make room for the raw-input fallback when every stage has filled the
/// bounded pool. The row that would otherwise be truncated is the lowest
/// value one; the fallback is the user's escape hatch.
fn keep_room(staged: &mut Vec<Candidate>, text: &str) {
    if staged.len() >= MAX_STAGED && !staged.iter().any(|candidate| candidate.text == text) {
        staged.pop();
    }
}

fn push_lang(
    staged: &mut Vec<Candidate>,
    text: &str,
    language: Language,
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
            existing.language = language;
        }
        return;
    }
    if staged.len() >= MAX_STAGED {
        return;
    }
    let mut candidate = Candidate::new(text, language, source);
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
    use super::{SuggestRequest, SuggestionEngine, UserWord};
    use okkhor_learn::Smriti;

    fn engine() -> SuggestionEngine {
        SuggestionEngine::from_embedded().expect("engine")
    }

    fn texts(suggestions: &[okkhor_core::Suggestion]) -> Vec<&str> {
        suggestions.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn capitalized_keys_reach_the_rules() {
        let engine = engine();
        let smriti = Smriti::new();
        for (roman, expected) in [
            ("Taka", "\u{099f}\u{09be}\u{0995}\u{09be}"),
            ("Th", "\u{09a0}"),
            ("D", "\u{09a1}"),
            ("R", "\u{09dc}"),
            ("Sh", "\u{09b7}"),
        ] {
            let rows = engine.suggest(&SuggestRequest::new(roman, 4), &smriti);
            assert_eq!(
                rows.first().map(|row| row.text.as_str()),
                Some(expected),
                "{roman}"
            );
        }
    }
    #[test]
    fn shipped_overlay_rows_win() {
        let engine = engine();
        let smriti = Smriti::new();
        assert!(!engine.overlay.is_empty(), "overlay parses");
        for (roman, bangla) in &engine.overlay {
            let rows = engine.suggest(&SuggestRequest::new(roman, 4), &smriti);
            assert_eq!(
                rows.first().map(|row| row.text.as_str()),
                Some(bangla.as_str()),
                "{roman} should render as {bangla}"
            );
        }
    }

    #[test]
    #[ignore = "diagnostic: prints rows for a roman key"]
    fn debug_alias_rows() {
        let engine = engine();
        let smriti = Smriti::new();
        for roman in ["chokh", "aath", "chithi", "kotha", "shohoj"] {
            let exact = engine
                .index
                .as_ref()
                .map(|index| index.exact_roman_targets(roman))
                .unwrap_or_default();
            let curated = engine
                .index
                .as_ref()
                .and_then(|index| index.curated_exact(roman));
            println!("--- {roman} exact={exact:?} curated={curated:?}");
            let rows = engine.suggest(&SuggestRequest::new(roman, 5), &smriti);
            for row in &rows {
                println!("  {} [{:?}]", row.text, row.source);
            }
        }
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
    fn unknown_ascii_word_keeps_the_raw_token_selectable() {
        let engine = engine();
        let smriti = Smriti::new();
        let ranked = engine.suggest(&SuggestRequest::new("also", 10), &smriti);
        let rows = texts(&ranked);
        assert!(rows.contains(&"also"), "raw token missing from {rows:?}");
    }

    #[test]
    fn known_bangla_word_keeps_a_bangla_top_row() {
        let engine = engine();
        let smriti = Smriti::new();
        let ranked = engine.suggest(&SuggestRequest::new("amar", 10), &smriti);
        let rows = texts(&ranked);
        assert_eq!(rows.first().copied(), Some("আমার"), "{rows:?}");
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
        assert!(
            !engine.suggest_english("a", 8).is_empty(),
            "one-letter prefixes must offer completions"
        );
        assert!(engine.suggest_english("", 8).is_empty());
    }

    #[test]
    fn single_char_prefix_has_dictionary_rows() {
        let engine = engine();
        let smriti = Smriti::new();
        let rows = engine.suggest(&SuggestRequest::new("k", 8), &smriti);
        assert!(!rows.is_empty(), "the first letter must produce rows");
        assert!(
            rows.iter()
                .any(|row| row.source == okkhor_core::CandidateSource::Dictionary),
            "a one-letter prefix must offer a dictionary-backed Bangla row: {rows:?}"
        );
    }

    #[test]
    fn adjacent_transposition_recovers_the_intended_word() {
        let engine = engine();
        let rows = engine.suggest(&SuggestRequest::new("amra", 10), &Smriti::new());
        assert!(
            rows.iter().any(|row| row.text == "আমার"),
            "amra should offer amar through a transposition: {rows:?}"
        );
        // The exact word still ranks above the typo repair.
        assert_eq!(
            rows.first().map(|row| row.text.as_str()),
            Some("আমরা"),
            "the exact word must stay on top: {rows:?}"
        );
    }

    #[test]
    fn preferred_rank_keeps_the_authored_first_target_on_top() {
        let engine = engine();
        let rows = engine.suggest(&SuggestRequest::new("bari", 10), &Smriti::new());
        let first_dictionary = rows
            .iter()
            .find(|row| row.source == okkhor_core::CandidateSource::Dictionary)
            .map(|row| row.text.as_str());
        assert_eq!(
            first_dictionary,
            Some("বাড়ি"),
            "rank 0 must stay first among dictionary rows: {rows:?}"
        );
        assert!(
            rows.iter()
                .filter(|row| row.text == "বাড়ি" || row.text == "বারি")
                .count()
                <= 2,
            "multi-target aliases must not flood the list"
        );
    }

    #[test]
    fn an_explicit_user_mapping_outranks_curated_and_dictionary_rows() {
        let engine = engine();
        let smriti = Smriti::new();
        let words = [UserWord {
            roman: "ammar".to_owned(),
            bangla: "আমার".to_owned(),
        }];
        let rows = engine.suggest(
            &SuggestRequest::new("ammar", 8).with_user_words(&words),
            &smriti,
        );
        assert_eq!(
            rows.first().map(|row| row.text.as_str()),
            Some("আমার"),
            "the user's own mapping must be row 1: {rows:?}"
        );
        assert_eq!(
            rows.first().map(|row| row.source),
            Some(okkhor_core::CandidateSource::UserHistory)
        );
    }

    #[test]
    fn alternate_embedded_table_compiles_and_keeps_the_index() {
        let table = r#"{
            "name": "khipro-test",
            "source": "test",
            "rule_count": 1,
            "rules": [{ "pattern": "nftff", "replacement": "ণ্ঠ" }]
        }"#;
        let engine = SuggestionEngine::from_embedded_table(table).expect("alternate table");
        assert_eq!(engine.index_error(), None);
        let rows = engine.suggest(&SuggestRequest::new("amar", 8), &Smriti::new());
        assert!(!rows.is_empty(), "the shared index still works");
    }

    #[test]
    fn malformed_alternate_table_is_a_typed_error() {
        let table = r#"{"name":"bad","source":"test","rule_count":9,"rules":[]}"#;
        assert!(SuggestionEngine::from_embedded_table(table).is_err());
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
