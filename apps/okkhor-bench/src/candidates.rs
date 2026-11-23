//! Golden-corpus candidate benchmark: `CandidateEngine::suggest` per case.
//!
//! The corpus is the committed synthetic fixture
//! (`crates/okkhor-engine/tests/fixtures/synthetic/corpus.json`), embedded
//! with `include_str!` so the benchmark always measures the committed corpus
//! and never duplicates it. The engine components mirror the golden-corpus
//! integration test in `crates/okkhor-engine/tests/golden.rs`: two small
//! sorted dictionaries, a two-rule transliterator, an empty `Smriti`, and
//! default budgets. A unit test pins per-case output to the committed file,
//! so dictionary/rule drift fails loudly instead of silently diverging.
//!
//! Correctness reuses [`okkhor_engine::check_parity`] (top-1/top-5 hit rate
//! over the full corpus). Latency samples come from repeated timed
//! `suggest` calls cycling through the corpus cases; each timed call builds
//! the same request the parity helper builds
//! (`limit = min(5, budget max_limit)`, `now_ms = 0` for determinism).

use std::hint::black_box;
use std::time::Instant;

use okkhor_core::{Language, SuggestionRequest};
use okkhor_engine::{
    CandidateEngine, EngineConfig, GoldenCorpus, LanguageBudget, ParityReport, check_parity,
};
use okkhor_lang::{Dictionary, DictionaryEntry};
use okkhor_learn::Smriti;
use okkhor_transliterate::{RULE_SCHEMA, RuleId, RuleSet, RuleTransliterator, TransliterationRule};

use crate::stats::LatencySummary;

/// Committed synthetic golden corpus. Read, never duplicated.
pub const GOLDEN_CORPUS_JSON: &str =
    include_str!("../../../crates/okkhor-engine/tests/fixtures/synthetic/corpus.json");

/// Measured output of one candidate benchmark run.
#[derive(Debug, Clone)]
pub struct CandidatesRun {
    pub iterations: usize,
    pub warmup_iterations: usize,
    pub subset_cases: usize,
    pub corpus_schema: u32,
    pub parity: ParityReport,
    pub latency: LatencySummary,
    pub suggestions_total: usize,
}

/// Components borrowed by the [`CandidateEngine`]. The engine borrows them,
/// so the owner must outlive the run.
pub struct FixtureComponents {
    pub dictionaries: Vec<Dictionary>,
    pub transliterator: RuleTransliterator,
    pub smriti: Smriti,
    pub config: EngineConfig,
}

impl FixtureComponents {
    /// Build the fixture engine components (mirrors the `okkhor-engine`
    /// golden-corpus integration test).
    pub fn build() -> Self {
        let dictionaries = vec![
            dictionary(
                "bn-fixture",
                Language::Bangla,
                &["ভাই", "ভাত", "ভাবনা", "ভাষা", "ভালো", "কাজ"],
            ),
            dictionary(
                "en-fixture",
                Language::English,
                &["held", "helix", "helm", "hello", "help", "hero", "hex"],
            ),
        ];
        let rules = RuleSet {
            schema: RULE_SCHEMA,
            rules: vec![
                TransliterationRule {
                    id: RuleId::from_code("r-bha").expect("valid fixture rule id"),
                    input: "bha".to_owned(),
                    output: "ভালো".to_owned(),
                    language: Language::Bangla,
                    priority: 0,
                },
                TransliterationRule {
                    id: RuleId::from_code("r-b").expect("valid fixture rule id"),
                    input: "b".to_owned(),
                    output: "ব".to_owned(),
                    language: Language::Bangla,
                    priority: 100,
                },
            ],
        };
        let transliterator = RuleTransliterator::new(rules).expect("valid fixture rules");
        Self {
            dictionaries,
            transliterator,
            smriti: Smriti::new(),
            config: EngineConfig::default(),
        }
    }

    pub fn engine(&self) -> Result<CandidateEngine<'_>, okkhor_engine::EngineError> {
        CandidateEngine::new(
            &self.dictionaries,
            &self.transliterator,
            &self.smriti,
            self.config,
        )
    }
}

/// Parse and validate the committed golden corpus.
pub fn load_corpus() -> Result<GoldenCorpus, okkhor_engine::EngineError> {
    GoldenCorpus::from_json_str(GOLDEN_CORPUS_JSON)
}

/// Build the parity request for one golden case (mirrors `check_parity`).
///
/// The limit uses the same `min(5, budget max_limit)` clamp as the metric so
/// warmup and timed calls measure the identical query shape.
fn request_for_case(
    engine: &CandidateEngine,
    case: &okkhor_engine::GoldenCase,
) -> SuggestionRequest {
    let budget = LanguageBudget::for_language(engine.config(), case.language);
    SuggestionRequest {
        prefix: case.prefix.clone(),
        language: case.language,
        layout: case.layout.clone(),
        context: case.context.clone(),
        roman_input: case.roman_input.clone(),
        now_ms: 0,
        limit: 5usize.min(budget.max_limit),
    }
}

/// Run `iterations` measured `suggest` calls after `warmup` untimed calls.
///
/// The timed calls cycle through `cases`; `subset` caps how many leading
/// corpus cases the timed loop uses (`None` means all cases). Parity always
/// covers the full corpus via [`check_parity`].
pub fn run(
    engine: &CandidateEngine,
    corpus: &GoldenCorpus,
    iterations: usize,
    warmup: usize,
    subset: Option<usize>,
) -> CandidatesRun {
    assert!(!corpus.cases.is_empty(), "corpus must contain cases");
    let timed_cases = subset
        .map(|limit| limit.min(corpus.cases.len()).max(1))
        .unwrap_or(corpus.cases.len());
    let cases = &corpus.cases[..timed_cases];

    for index in 0..warmup {
        let request = request_for_case(engine, &cases[index % timed_cases]);
        let ranked = engine.suggest(&request);
        black_box(&ranked);
    }

    let mut samples = Vec::with_capacity(iterations);
    let mut suggestions_total = 0;
    for index in 0..iterations {
        let request = request_for_case(engine, &cases[index % timed_cases]);
        let started = Instant::now();
        let ranked = engine.suggest(&request);
        let elapsed = started.elapsed();
        black_box(&ranked);
        suggestions_total += ranked.len();
        samples.push(elapsed);
    }

    CandidatesRun {
        iterations,
        warmup_iterations: warmup,
        subset_cases: timed_cases,
        corpus_schema: corpus.schema,
        parity: check_parity(engine, corpus),
        latency: LatencySummary::from_samples(samples),
        suggestions_total,
    }
}

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

#[cfg(test)]
mod tests {
    use super::{FixtureComponents, load_corpus, run};

    #[test]
    fn committed_corpus_parses_and_hits_every_case() {
        let corpus = load_corpus().expect("fixture must validate");
        assert_eq!(corpus.schema, 1);
        assert!(!corpus.cases.is_empty());
        let components = FixtureComponents::build();
        let engine = components.engine().expect("valid fixture engine");
        let result = run(&engine, &corpus, 6, 2, None);
        assert_eq!(result.parity.cases, corpus.cases.len() as u64);
        assert_eq!(result.parity.top1_hits, result.parity.cases);
        assert_eq!(result.parity.top5_hits, result.parity.cases);
        assert_eq!(result.parity.top1_rate, 1.0);
        assert_eq!(result.parity.top5_rate, 1.0);
    }

    #[test]
    fn candidates_run_is_bounded_and_monotonic() {
        let corpus = load_corpus().expect("fixture must validate");
        let components = FixtureComponents::build();
        let engine = components.engine().expect("valid fixture engine");
        let result = run(&engine, &corpus, 8, 2, Some(1));
        assert_eq!(result.latency.count, 8);
        assert_eq!(result.subset_cases, 1);
        assert!(result.latency.is_monotonic());
        assert!(result.suggestions_total > 0);
    }

    #[test]
    fn per_case_results_are_pinned_to_the_committed_corpus() {
        use okkhor_core::CandidateSource;

        // The fixture engine above mirrors
        // `crates/okkhor-engine/tests/golden.rs`. Pin its observable behavior
        // to the committed corpus file: any dictionary/rule drift that
        // alters per-case output fails here and forces the mirror update.
        // (Corpus growth must update this pin alongside the mirror.)
        let corpus = load_corpus().expect("fixture must validate");
        assert_eq!(corpus.cases.len(), 3, "corpus growth must update this pin");
        let components = FixtureComponents::build();
        let engine = components.engine().expect("valid fixture engine");
        let expected: &[(&str, &[&str], CandidateSource)] = &[
            (
                "bn_bha_prefix",
                &["ভাই", "ভাত", "ভাবনা", "ভালো", "ভাষা"],
                CandidateSource::Dictionary,
            ),
            (
                "en_hel_prefix",
                &["held", "helix", "hello", "helm", "help"],
                CandidateSource::Dictionary,
            ),
            (
                "bn_roman_bha",
                &["ভালো", "ব"],
                CandidateSource::Transliteration,
            ),
        ];
        for (case, (id, texts, source)) in corpus.cases.iter().zip(expected) {
            assert_eq!(&case.id, id);
            let request = super::request_for_case(&engine, case);
            let suggestions = engine.suggest(&request);
            let actual: Vec<&str> = suggestions.iter().map(|s| s.text.as_str()).collect();
            assert_eq!(actual, *texts, "suggestion texts for case {id}");
            assert!(
                suggestions.iter().all(|s| s.source == *source),
                "suggestion sources for case {id}"
            );
        }
    }
}
