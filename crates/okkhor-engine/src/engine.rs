//! Candidate composition: dictionary prefix lookup, transliteration
//! application, merge + dedup, Smriti ranking, and top-N truncation.
//!
//! Stage order is normative; reordering requires a new ADR:
//!
//! ```text
//! request
//!   │  (0) normalize once: prefix = normalize_lookup(request.prefix),
//!   │      roman = request.roman_input.map(normalize_lookup)
//!   ├─ (1) dictionary prefix lookup over `dictionaries`
//!   ├─ (2) transliteration application for Romanized input
//!   ├─ (3) merge + dedup
//!   ├─ (4) Smriti rank via okkhor_core::Ranker::rank
//!   └─ (5) top-N truncate to clamped limit
//! ```

use okkhor_core::{
    Candidate, CandidateSource, Language, Ranker, Suggestion, SuggestionRequest, normalize_lookup,
};
use okkhor_lang::Dictionary;
use okkhor_learn::Smriti;
use okkhor_transliterate::{RuleTransliterator, TransliterationOptions, TransliteratorEngine};

use crate::config::LanguageBudget;
use crate::{EngineConfig, EngineError};

/// Raw input at or above this byte length yields no suggestions, matching the
/// transliteration crate's overlong threshold.
const MAX_INPUT_BYTES: usize = 256;

/// Borrows already-constructed components; owns no I/O handle. The daemon owns
/// exactly one copy of each asset and chooses its own sharing mechanism.
#[derive(Debug)]
pub struct CandidateEngine<'a> {
    dictionaries: &'a [Dictionary],
    transliterator: &'a RuleTransliterator,
    smriti: &'a Smriti,
    config: EngineConfig,
}

impl<'a> CandidateEngine<'a> {
    /// Borrow already-constructed components. Validates budgets once and
    /// clones nothing large.
    pub fn new(
        dictionaries: &'a [Dictionary],
        transliterator: &'a RuleTransliterator,
        smriti: &'a Smriti,
        config: EngineConfig,
    ) -> Result<Self, EngineError> {
        // `EngineConfig::new` already rejects zero budgets; re-check here so a
        // hand-built config can never widen the query-path allocation.
        for budget in [config.bangla, config.english] {
            if !budget.is_valid() {
                return Err(EngineError::InvalidBudget);
            }
        }
        Ok(Self {
            dictionaries,
            transliterator,
            smriti,
            config,
        })
    }

    /// Infallible query: degradation means fewer candidates, never an error.
    ///
    /// Bounded allocation per query: at most `max_prefix_hits +
    /// max_transliteration_hits` stage candidates, `max_merged` merged
    /// candidates, and `max_limit` returned suggestions. No I/O, no clock
    /// reads, no logging on this path.
    pub fn suggest(&self, request: &SuggestionRequest) -> Vec<Suggestion> {
        // Degenerate queries return before any allocation.
        if request.limit == 0 || request.prefix.len() >= MAX_INPUT_BYTES {
            return Vec::new();
        }
        let budget = LanguageBudget::for_language(&self.config, request.language);
        let limit = request.limit.min(budget.max_limit);
        if limit == 0 {
            // Unreachable: every budget enforces `max_limit >= 1`. Kept so a
            // zero limit can never reach the truncation below.
            return Vec::new();
        }

        // Stage 0 — normalize once; downstream helpers also normalize
        // internally, which is idempotent and stays correct.
        let prefix = normalize_lookup(&request.prefix);
        if prefix.is_empty() {
            return Vec::new();
        }
        let roman = request
            .roman_input
            .as_deref()
            .map(normalize_lookup)
            .filter(|roman| !roman.is_empty());

        // Stage 1 — dictionary prefix lookup.
        let mut staged =
            Vec::with_capacity(budget.max_prefix_hits + budget.max_transliteration_hits);
        dictionary_prefix_lookup(
            self.dictionaries,
            request.language,
            &prefix,
            &budget,
            &mut staged,
        );

        // Stage 2 — transliteration application for Romanized input: either an
        // explicit `roman_input`, or the prefix itself when it is pure ASCII
        // Bangla input. Results arrive ordered by match quality and are never
        // re-sorted within this stage.
        let roman_source: Option<&str> = match roman.as_deref() {
            Some(roman) => Some(roman),
            None if request.language == Language::Bangla && prefix.is_ascii() => {
                Some(prefix.as_str())
            }
            None => None,
        };
        if let Some(roman) = roman_source {
            let options = TransliterationOptions {
                max_candidates: budget.max_transliteration_hits,
                allow_partial: true,
            };
            for output in self.transliterator.transliterate(roman, &options) {
                let mut candidate = output.into_candidate();
                candidate.features.phonetic_score = clamp_finite(candidate.features.phonetic_score);
                staged.push(candidate);
            }
        }

        // Stage 3 — merge + dedup.
        let merged = merge_candidates(staged, &budget);

        // Stage 4 — Smriti rank with the original request so recency and
        // bigram scoring stay deterministic per caller time. `layout` flows
        // through as metadata only; no layout-qualified keys are synthesized.
        let ranked = self.smriti.rank(request, merged);

        // Stage 5 — top-N truncate to the clamped limit, preserving rank order.
        ranked.into_iter().take(limit).collect()
    }

    /// The validated budgets this engine was built with.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }
}

/// Base score for the `position`-th admitted dictionary hit (0-based):
/// `1 / (1 + position)`, i.e. 1.0, 1/2, 1/3, ... Strictly decreasing in
/// `position`, hence monotonic with entry rank (earlier entry = higher score),
/// and always finite for any bounded position.
fn base_score_for(position: usize) -> f32 {
    1.0 / (1.0 + position as f32)
}

/// Engine-stage float hygiene: any non-finite score produced by stages 1–3 is
/// clamped to `0.0` before merge so ordering stays total. (Smriti's internal
/// scoring clamps to `f32::NEG_INFINITY` in `score_with` and is untouched.)
fn clamp_finite(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

/// Stage 1: scan the borrowed dictionaries filtered by `language` and admit
/// entries whose normalized word starts with the normalized prefix, in
/// dictionary order, up to `budget.max_prefix_hits`.
///
/// Dictionary words are normalized at import and the prefix was normalized
/// once in stage 0, so admission is an exact byte-prefix test.
fn dictionary_prefix_lookup(
    dictionaries: &[Dictionary],
    language: Language,
    prefix: &str,
    budget: &LanguageBudget,
    staged: &mut Vec<Candidate>,
) {
    let mut admitted = 0usize;
    for dictionary in dictionaries.iter().filter(|d| d.language == language) {
        for entry in &dictionary.entries {
            if admitted >= budget.max_prefix_hits {
                return;
            }
            if entry.word.as_str().starts_with(prefix) {
                let mut candidate = Candidate::new(
                    entry.word.clone(),
                    dictionary.language,
                    CandidateSource::Dictionary,
                );
                candidate.features.base_score = clamp_finite(base_score_for(admitted));
                staged.push(candidate);
                admitted += 1;
            }
        }
    }
}

/// Stage 3: concatenate stage-1 and stage-2 candidates (dictionary first),
/// then dedup by `(language, text)` keeping the first occurrence — except that
/// a `Transliteration` candidate replaces an earlier `Dictionary` candidate
/// for the same key when its `phonetic_score > 0.0`, carrying the
/// transliteration source forward. Truncates to `budget.max_merged`,
/// dictionary-priority order preserved. Dedup is exact string equality on
/// already-normalized text; no fuzzy collapsing.
///
/// Scratch state is a `Vec` only: no hash map is iterated, so output order is
/// fully deterministic for identical inputs.
fn merge_candidates(staged: Vec<Candidate>, budget: &LanguageBudget) -> Vec<Candidate> {
    let mut merged: Vec<Candidate> = Vec::with_capacity(staged.len().min(budget.max_merged));
    for candidate in staged {
        if let Some(index) = merged
            .iter()
            .position(|kept| kept.language == candidate.language && kept.text == candidate.text)
        {
            if candidate.source == CandidateSource::Transliteration
                && merged[index].source == CandidateSource::Dictionary
                && candidate.features.phonetic_score > 0.0
            {
                merged[index] = candidate;
            }
        } else if merged.len() < budget.max_merged {
            merged.push(candidate);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::{base_score_for, clamp_finite, merge_candidates};
    use crate::config::LanguageBudget;
    use crate::engine::CandidateEngine;
    use crate::{EngineConfig, EngineError};
    use okkhor_core::{
        Candidate, CandidateSource, Language, Ranker, SuggestionRequest, normalize_lookup,
    };
    use okkhor_lang::{Dictionary, DictionaryEntry};
    use okkhor_learn::Smriti;
    use okkhor_transliterate::{
        RULE_SCHEMA, RuleId, RuleSet, RuleTransliterator, TransliterationRule,
    };

    fn dictionary(id: &str, language: Language, words: &[&str]) -> Dictionary {
        let mut entries: Vec<DictionaryEntry> = words
            .iter()
            .map(|word| DictionaryEntry {
                word: (*word).to_owned(),
                frequency: 1,
            })
            .collect();
        entries.sort_by(|a, b| a.word.cmp(&b.word));
        entries.dedup_by(|a, b| a.word == b.word);
        Dictionary {
            id: id.to_owned(),
            language,
            entries,
        }
    }

    fn rule(id: &str, input: &str, output: &str, priority: u16) -> TransliterationRule {
        TransliterationRule {
            id: RuleId::from_code(id).expect("valid test rule id"),
            input: input.to_owned(),
            output: output.to_owned(),
            language: Language::Bangla,
            priority,
        }
    }

    fn transliterator() -> RuleTransliterator {
        let rules = RuleSet {
            schema: RULE_SCHEMA,
            rules: vec![
                rule("r-bha", "bha", "ভালো", 0),
                rule("r-bhalo", "bhalo", "ভালো", 10),
                rule("r-b", "b", "ব", 100),
            ],
        };
        RuleTransliterator::new(rules).expect("valid test rules")
    }

    fn engine<'a>(
        dictionaries: &'a [Dictionary],
        transliterator: &'a RuleTransliterator,
        smriti: &'a Smriti,
        config: EngineConfig,
    ) -> CandidateEngine<'a> {
        CandidateEngine::new(dictionaries, transliterator, smriti, config)
            .expect("valid test engine")
    }

    fn bangla_dict() -> Dictionary {
        dictionary(
            "bn-test",
            Language::Bangla,
            &["ভাই", "ভাত", "ভাবনা", "ভাষা", "ভালো", "কাজ"],
        )
    }

    fn english_dict() -> Dictionary {
        dictionary(
            "en-test",
            Language::English,
            &["held", "helix", "helm", "hello", "help", "hero", "hex"],
        )
    }

    #[test]
    fn constructor_rejects_hand_built_zero_budget() {
        let smriti = Smriti::new();
        let transliterator = transliterator();
        let dictionaries = vec![bangla_dict()];
        // Bypass `EngineConfig::new` validation with a struct literal.
        let config = EngineConfig {
            bangla: LanguageBudget {
                max_prefix_hits: 0,
                max_transliteration_hits: 8,
                max_merged: 24,
                max_limit: 10,
            },
            english: EngineConfig::default().english,
        };
        assert!(matches!(
            CandidateEngine::new(&dictionaries, &transliterator, &smriti, config),
            Err(EngineError::InvalidBudget)
        ));
    }

    #[test]
    fn dictionary_only_query_yields_dictionary_sources() {
        let dictionaries = vec![bangla_dict()];
        let transliterator = transliterator();
        let smriti = Smriti::new();
        let engine = engine(
            &dictionaries,
            &transliterator,
            &smriti,
            EngineConfig::default(),
        );
        // Non-ASCII prefix: no transliteration stage, dictionary only.
        let request = SuggestionRequest::new("ভা", Language::Bangla, 5);
        let suggestions = engine.suggest(&request);
        assert_eq!(suggestions.len(), 5);
        assert!(
            suggestions
                .iter()
                .all(|s| s.source == CandidateSource::Dictionary)
        );
        let texts: Vec<&str> = suggestions.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, vec!["ভাই", "ভাত", "ভাবনা", "ভালো", "ভাষা"]);
    }

    #[test]
    fn transliteration_only_query_yields_transliteration_sources() {
        let dictionaries = vec![bangla_dict()];
        let transliterator = transliterator();
        let smriti = Smriti::new();
        let engine = engine(
            &dictionaries,
            &transliterator,
            &smriti,
            EngineConfig::default(),
        );
        // ASCII Bangla prefix matches no dictionary word: transliteration only.
        let request = SuggestionRequest::new("bha", Language::Bangla, 5);
        let suggestions = engine.suggest(&request);
        assert_eq!(suggestions.len(), 2);
        assert!(
            suggestions
                .iter()
                .all(|s| s.source == CandidateSource::Transliteration)
        );
        assert_eq!(suggestions[0].text, "ভালো");
        assert_eq!(suggestions[1].text, "ব");
    }

    #[test]
    fn mixed_query_combines_both_stages() {
        let dictionaries = vec![bangla_dict()];
        let transliterator = transliterator();
        let smriti = Smriti::new();
        let engine = engine(
            &dictionaries,
            &transliterator,
            &smriti,
            EngineConfig::default(),
        );
        // Dictionary prefix hits plus explicit roman input: "ভালো" dedups to
        // the transliteration source (§5 stage-3 rule) while the other
        // dictionary hits keep theirs. Transliterated "ভালো" (0.2) ties
        // dictionary "ভাষা" (0.2); the text-ascending tie-break orders it
        // first.
        let request = SuggestionRequest::new("ভা", Language::Bangla, 5).with_roman_input("bha");
        let suggestions = engine.suggest(&request);
        let texts: Vec<&str> = suggestions.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, vec!["ভাই", "ভাত", "ভাবনা", "ভালো", "ভাষা"]);
        let dict_hits = suggestions
            .iter()
            .filter(|s| s.source == CandidateSource::Dictionary)
            .count();
        assert_eq!(dict_hits, 4);
        assert_eq!(suggestions[3].source, CandidateSource::Transliteration);
        assert_eq!(suggestions[3].text, "ভালো");
    }

    #[test]
    fn merge_pins_dictionary_first_order_and_replacement_rule() {
        let dict = |text: &str| Candidate::new(text, Language::Bangla, CandidateSource::Dictionary);
        let translit = |text: &str, phonetic: f32| {
            let mut candidate =
                Candidate::new(text, Language::Bangla, CandidateSource::Transliteration);
            candidate.features.phonetic_score = phonetic;
            candidate
        };
        let budget = EngineConfig::default().bangla;
        // Dictionary-first concatenation order is preserved.
        let merged = merge_candidates(vec![dict("ক"), translit("খ", 0.9), dict("গ")], &budget);
        let texts: Vec<&str> = merged.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, vec!["ক", "খ", "গ"]);
        // Transliteration replaces an earlier dictionary hit with phonetic > 0.
        let merged = merge_candidates(vec![dict("ক"), translit("ক", 0.5)], &budget);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].source, CandidateSource::Transliteration);
        // Zero phonetic keeps the dictionary entry.
        let merged = merge_candidates(vec![dict("ক"), translit("ক", 0.0)], &budget);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].source, CandidateSource::Dictionary);
        // Exact duplicate dictionary entries collapse to one.
        let merged = merge_candidates(vec![dict("ক"), dict("ক")], &budget);
        assert_eq!(merged.len(), 1);
        // Cross-language texts are distinct dedup keys.
        let merged = merge_candidates(
            vec![
                dict("abc"),
                Candidate::new("abc", Language::English, CandidateSource::Dictionary),
            ],
            &budget,
        );
        assert_eq!(merged.len(), 2);
        // Later dictionary duplicates never displace the first occurrence.
        let merged = merge_candidates(vec![translit("ক", 0.5), dict("ক")], &budget);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].source, CandidateSource::Transliteration);
    }

    #[test]
    fn normalization_is_owned_once_by_the_engine() {
        let dictionaries = vec![bangla_dict()];
        let transliterator = transliterator();
        let smriti = Smriti::new();
        let engine = engine(
            &dictionaries,
            &transliterator,
            &smriti,
            EngineConfig::default(),
        );
        let clean = engine.suggest(&SuggestionRequest::new("ভা", Language::Bangla, 5));
        // Untrimmed prefix and non-NFC roman input match the clean query.
        let padded = engine.suggest(&SuggestionRequest::new("  ভা  ", Language::Bangla, 5));
        assert_eq!(clean, padded);
        let with_roman = engine
            .suggest(&SuggestionRequest::new("bha", Language::Bangla, 5).with_roman_input("bha"));
        let messy_roman = engine.suggest(
            &SuggestionRequest::new("  BHA ", Language::Bangla, 5).with_roman_input("  BHA "),
        );
        assert_eq!(with_roman, messy_roman);
        // Whitespace-only prefix normalizes to empty: no suggestions.
        assert!(
            engine
                .suggest(&SuggestionRequest::new("   ", Language::Bangla, 5))
                .is_empty()
        );
    }

    #[test]
    fn overlong_and_zero_limit_inputs_yield_empty_vec() {
        let dictionaries = vec![bangla_dict()];
        let transliterator = transliterator();
        let smriti = Smriti::new();
        let engine = engine(
            &dictionaries,
            &transliterator,
            &smriti,
            EngineConfig::default(),
        );
        assert!(
            engine
                .suggest(&SuggestionRequest::new("ভা", Language::Bangla, 0))
                .is_empty()
        );
        let long = "a".repeat(256);
        assert!(
            engine
                .suggest(&SuggestionRequest::new(long, Language::Bangla, 5))
                .is_empty()
        );
        let boundary = "b".repeat(255);
        // 255 bytes is accepted by the engine guard (the transliterator then
        // yields no candidates for the overlong roman key): no panic.
        let _ = engine.suggest(&SuggestionRequest::new(boundary, Language::Bangla, 5));
    }

    #[test]
    fn budgets_cap_each_stage() {
        let dictionaries = vec![bangla_dict()];
        let transliterator = transliterator();
        let smriti = Smriti::new();
        // Dictionary fan-out cap.
        let config = EngineConfig::new(
            LanguageBudget {
                max_prefix_hits: 2,
                max_transliteration_hits: 8,
                max_merged: 24,
                max_limit: 10,
            },
            EngineConfig::default().english,
        )
        .expect("valid test config");
        let capped = engine(&dictionaries, &transliterator, &smriti, config);
        let suggestions = capped.suggest(&SuggestionRequest::new("ভা", Language::Bangla, 5));
        assert_eq!(suggestions.len(), 2);

        // Transliteration fan-out cap.
        let config = EngineConfig::new(
            LanguageBudget {
                max_prefix_hits: 20,
                max_transliteration_hits: 1,
                max_merged: 24,
                max_limit: 10,
            },
            EngineConfig::default().english,
        )
        .expect("valid test config");
        let capped = engine(&dictionaries, &transliterator, &smriti, config);
        let suggestions = capped.suggest(&SuggestionRequest::new("bha", Language::Bangla, 5));
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].text, "ভালো");

        // Limit clamp to max_limit.
        let words: Vec<String> = (0..15).map(|i| format!("word{i:02}")).collect();
        let many_refs: Vec<&str> = words.iter().map(String::as_str).collect();
        let big = vec![dictionary("en-many", Language::English, &many_refs)];
        let capped = engine(&big, &transliterator, &smriti, EngineConfig::default());
        let suggestions = capped.suggest(&SuggestionRequest::new("word", Language::English, 50));
        assert_eq!(suggestions.len(), 10);

        // Merged cap bounds ranker input (English side: the query language).
        let config = EngineConfig::new(
            EngineConfig::default().bangla,
            LanguageBudget {
                max_prefix_hits: 20,
                max_transliteration_hits: 8,
                max_merged: 3,
                max_limit: 10,
            },
        )
        .expect("valid test config");
        let capped = engine(&big, &transliterator, &smriti, config);
        let suggestions = capped.suggest(&SuggestionRequest::new("word", Language::English, 10));
        assert_eq!(suggestions.len(), 3);
    }

    #[test]
    fn suggest_is_deterministic() {
        let dictionaries = vec![bangla_dict(), english_dict()];
        let transliterator = transliterator();
        let smriti = Smriti::new();
        let engine = engine(
            &dictionaries,
            &transliterator,
            &smriti,
            EngineConfig::default(),
        );
        let request = SuggestionRequest::new("ভা", Language::Bangla, 5).at(1_000);
        let first = engine.suggest(&request);
        let second = engine.suggest(&request);
        assert_eq!(first, second);
        assert_eq!(format!("{first:?}"), format!("{second:?}"));
    }

    #[test]
    fn rank_tie_break_and_pathological_scores_are_pinned() {
        // The two-level `(score desc, text asc)` tie-break with stable
        // merge-order retention lives in `Smriti::rank`, which the engine
        // calls without re-sorting: pin its observable behavior here,
        // including equal (score, text) across sources and a NaN feature.
        let smriti = Smriti::new();
        let request = SuggestionRequest::new("x", Language::English, 10);
        let dict =
            |text: &str| Candidate::new(text, Language::English, CandidateSource::Dictionary);
        let translit =
            |text: &str| Candidate::new(text, Language::English, CandidateSource::Transliteration);
        // Same score, different text: ascending text wins.
        let ranked = smriti.rank(&request, vec![dict("b"), dict("a")]);
        assert_eq!(ranked[0].text, "a");
        assert_eq!(ranked[1].text, "b");
        // Same score and same text across sources: stable sort retains the
        // merge order (dictionary hit admitted first).
        let ranked = smriti.rank(&request, vec![dict("same"), translit("same")]);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].source, CandidateSource::Dictionary);
        assert_eq!(ranked[1].source, CandidateSource::Transliteration);
        // NaN base score clamps to NEG_INFINITY inside Smriti and sorts last.
        let mut bad = dict("bad");
        bad.features.base_score = f32::NAN;
        let ranked = smriti.rank(&request, vec![bad, dict("good")]);
        assert_eq!(ranked[0].text, "good");
        assert_eq!(ranked[1].text, "bad");
        assert_eq!(ranked[1].score, f32::NEG_INFINITY);
        // Engine-stage hygiene: non-finite stage scores clamp to 0.0.
        assert_eq!(clamp_finite(f32::NAN), 0.0);
        assert_eq!(clamp_finite(f32::INFINITY), 0.0);
        assert_eq!(clamp_finite(f32::NEG_INFINITY), 0.0);
        assert_eq!(clamp_finite(0.5), 0.5);
    }

    #[test]
    fn base_scores_are_strictly_decreasing_and_finite() {
        let mut previous = f32::INFINITY;
        for position in 0..64 {
            let score = base_score_for(position);
            assert!(score.is_finite());
            assert!(score < previous);
            previous = score;
        }
        assert_eq!(base_score_for(0), 1.0);
    }

    #[test]
    fn english_prefix_lookup_filters_language_and_orders_by_rank() {
        let dictionaries = vec![bangla_dict(), english_dict()];
        let transliterator = transliterator();
        let smriti = Smriti::new();
        let engine = engine(
            &dictionaries,
            &transliterator,
            &smriti,
            EngineConfig::default(),
        );
        // English "hel" must not see Bangla entries; "hero"/"hex" do not match.
        let suggestions = engine.suggest(&SuggestionRequest::new("hel", Language::English, 5));
        let texts: Vec<&str> = suggestions.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, vec!["held", "helix", "hello", "helm", "help"]);
    }

    #[test]
    fn error_display_never_carries_input_text() {
        let input = "secret-prefix-ভা";
        let error = EngineError::InvalidGolden { reason: "bad_id" };
        assert!(!format!("{error}").contains(input));
        // `normalize_lookup` output is data, not an error string: the error
        // type cannot be constructed from it.
        let _ = normalize_lookup(input);
    }
}
