//! Deterministic synthetic English/Bangla data for the ranking benchmark.
//!
//! Nothing here is user data. Words are generated from fixed root and suffix
//! lists so repeats are reproducible across runs and machines.

use okkhor_core::{Candidate, CandidateSource, Language, RankingFeatures, SuggestionRequest};
use okkhor_learn::{CommitSource, LearningEvent, Smriti};

const CANDIDATES_PER_CASE: usize = 48;
const CASES_PER_LANGUAGE: usize = 64;
const RESULT_LIMIT: usize = 20;
const TRAINING_NOW_MS: u64 = 2_000_000;

const ENGLISH_ROOTS: &[&str] = &[
    "key",
    "board",
    "type",
    "input",
    "word",
    "suggest",
    "rank",
    "learn",
    "smriti",
    "okkhor",
    "bengali",
    "english",
    "window",
    "focus",
    "inject",
    "caret",
    "popup",
    "buffer",
    "layout",
    "dictionary",
];
const ENGLISH_SUFFIXES: &[&str] = &["", "s", "ed", "ing"];

const BANGLA_ROOTS: &[&str] = &[
    "ভালো",
    "বাংলা",
    "অক্ষর",
    "কীবোর্ড",
    "লেখা",
    "শব্দ",
    "বাক্য",
    "স্মৃতি",
    "ইনপুট",
    "সাজেশন",
    "ভাষা",
    "লেআউট",
    "মুখ",
    "হাত",
    "দৃষ্টি",
    "পর্দা",
    "আলো",
    "নাম",
    "গান",
    "সময়",
];
const BANGLA_SUFFIXES: &[&str] = &["", "টি", "ের", "তে", "রা"];

const SOURCES: [CandidateSource; 6] = [
    CandidateSource::Dictionary,
    CandidateSource::Transliteration,
    CandidateSource::UserHistory,
    CandidateSource::Recent,
    CandidateSource::Context,
    CandidateSource::Dictionary,
];

/// One ranking request plus the candidate set handed to `Smriti::rank`.
#[derive(Debug, Clone)]
pub struct BenchCase {
    pub request: SuggestionRequest,
    pub candidates: Vec<Candidate>,
}

/// A trained store and the cases used by the benchmark loop.
#[derive(Debug)]
pub struct SyntheticCorpus {
    pub engine: Smriti,
    pub cases: Vec<BenchCase>,
    pub english_cases: usize,
    pub bangla_cases: usize,
    pub candidates_per_case: usize,
}

impl SyntheticCorpus {
    /// Build the corpus and train the small Smriti store.
    pub fn build(seed: u64) -> Self {
        let english_words = generate_words(ENGLISH_ROOTS, ENGLISH_SUFFIXES);
        let bangla_words = generate_words(BANGLA_ROOTS, BANGLA_SUFFIXES);
        assert!(!english_words.is_empty() && !bangla_words.is_empty());

        let engine = Smriti::new();
        train(&engine, &english_words, Language::English);
        train(&engine, &bangla_words, Language::Bangla);

        let mut rng = Lcg::new(seed | 1);
        let mut cases = Vec::with_capacity(CASES_PER_LANGUAGE * 2);
        for _ in 0..CASES_PER_LANGUAGE {
            cases.push(build_case(&english_words, Language::English, &mut rng));
            cases.push(build_case(&bangla_words, Language::Bangla, &mut rng));
        }

        Self {
            engine,
            cases,
            english_cases: CASES_PER_LANGUAGE,
            bangla_cases: CASES_PER_LANGUAGE,
            candidates_per_case: CANDIDATES_PER_CASE,
        }
    }
}

fn generate_words(roots: &[&str], suffixes: &[&str]) -> Vec<String> {
    let mut words = Vec::with_capacity(roots.len() * suffixes.len());
    for root in roots {
        for suffix in suffixes {
            let word = format!("{root}{suffix}");
            if !word.is_empty() && !words.contains(&word) {
                words.push(word);
            }
        }
    }
    words
}

fn train(engine: &Smriti, words: &[String], language: Language) {
    for (index, word) in words.iter().enumerate() {
        let previous = index.checked_sub(1).map(|previous| words[previous].clone());
        let repeats = 1 + (index % 5) as u64;

        for repeat in 0..repeats {
            engine.observe(LearningEvent::WordCommitted {
                word: word.clone(),
                language,
                previous_word: previous.clone(),
                timestamp_ms: 1_000_000 + index as u64 * 1_000 + repeat,
                source: CommitSource::Typed,
            });
        }

        if index % 3 == 0 {
            engine.observe(LearningEvent::SuggestionShown {
                word: word.clone(),
                language,
                timestamp_ms: 1_000_500 + index as u64,
            });
            engine.observe(LearningEvent::SuggestionAccepted {
                word: word.clone(),
                language,
                previous_word: previous,
                timestamp_ms: 1_000_600 + index as u64,
                rank: 0,
                auto_top: None,
            });
        }

        if index % 7 == 6 {
            let next = words[(index + 1) % words.len()].clone();
            engine.observe(LearningEvent::Correction {
                from: word.clone(),
                to: next,
                language,
                timestamp_ms: 1_000_700 + index as u64,
            });
        }

        if language == Language::Bangla {
            engine.observe(LearningEvent::TransliterationCommitted {
                roman_input: format!("roman-bn-{index}"),
                output: word.clone(),
                language,
                previous_word: None,
                timestamp_ms: 1_000_900 + index as u64,
            });
        }
    }
}

fn build_case(words: &[String], language: Language, rng: &mut Lcg) -> BenchCase {
    let anchor_index = usize::try_from(rng.next() % words.len() as u64).unwrap_or(0);
    let anchor = &words[anchor_index];

    let prefix_len = if language == Language::English {
        anchor.chars().count().clamp(1, 3)
    } else {
        anchor.chars().count().clamp(1, 2)
    };
    let prefix: String = anchor.chars().take(prefix_len).collect();

    let mut chosen: Vec<(usize, String)> = Vec::with_capacity(CANDIDATES_PER_CASE);
    for (index, word) in words.iter().enumerate() {
        if chosen.len() == CANDIDATES_PER_CASE {
            break;
        }
        if word.starts_with(&prefix) {
            chosen.push((index, word.clone()));
        }
    }

    let mut cursor = anchor_index;
    let mut scanned = 0;
    while chosen.len() < CANDIDATES_PER_CASE && scanned < words.len() * 2 {
        if !chosen.iter().any(|(index, _)| *index == cursor) {
            chosen.push((cursor, words[cursor].clone()));
        }
        cursor = (cursor + 1) % words.len();
        scanned += 1;
    }

    let candidates = chosen
        .iter()
        .enumerate()
        .map(|(position, (_, text))| candidate_for(position, text, language))
        .collect();

    let previous = index_before(anchor_index, words.len());
    let context = previous.map(|index| words[index].clone());
    let mut request = SuggestionRequest::new(prefix, language, RESULT_LIMIT)
        .at(TRAINING_NOW_MS)
        .with_context(context);
    if language == Language::Bangla {
        request = request.with_roman_input(format!("roman-bn-{anchor_index}"));
    }

    BenchCase {
        request,
        candidates,
    }
}

fn candidate_for(position: usize, text: &str, language: Language) -> Candidate {
    let mut candidate = Candidate::new(text, language, SOURCES[position % SOURCES.len()]);
    candidate.features = RankingFeatures {
        base_score: 1.0 / (1.0 + position as f32 * 0.05),
        user_frequency: ((position * 3) % 11) as u32,
        recency_score: (CANDIDATES_PER_CASE - position) as f32 / CANDIDATES_PER_CASE as f32,
        context_score: if position == 0 { 0.75 } else { 0.0 },
        acceptance_rate: if position < 3 { 0.5 } else { 0.0 },
        correction_penalty: if position % 17 == 0 { 0.15 } else { 0.0 },
        phonetic_score: ((position * 7) % 10) as f32 / 10.0,
        pair_frequency: if language.is_bengali_script() && position < 5 {
            (position % 4) as u32
        } else {
            0
        },
    };
    candidate
}

fn index_before(index: usize, len: usize) -> Option<usize> {
    if len == 0 {
        None
    } else if index == 0 {
        Some(len - 1)
    } else {
        Some(index - 1)
    }
}

/// Small deterministic generator so a seed reproduces the same cases.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{CANDIDATES_PER_CASE, CASES_PER_LANGUAGE, SyntheticCorpus};
    use okkhor_core::{Language, contains_bengali};

    #[test]
    fn builds_both_languages_and_trains_the_store() {
        let corpus = SyntheticCorpus::build(7);
        assert_eq!(corpus.cases.len(), CASES_PER_LANGUAGE * 2);
        assert_eq!(corpus.english_cases, CASES_PER_LANGUAGE);
        assert_eq!(corpus.bangla_cases, CASES_PER_LANGUAGE);
        assert_eq!(corpus.candidates_per_case, CANDIDATES_PER_CASE);
        assert!(corpus.engine.word_count() > 100);
        assert!(corpus.engine.pair_count() > 50);
        assert!(corpus.engine.bigram_count() > 50);
    }

    #[test]
    fn the_same_seed_produces_the_same_cases() {
        let first = SyntheticCorpus::build(11);
        let second = SyntheticCorpus::build(11);
        assert_eq!(first.cases.len(), second.cases.len());
        for (left, right) in first.cases.iter().zip(&second.cases) {
            assert_eq!(left.request, right.request);
            assert_eq!(left.candidates, right.candidates);
        }
    }

    #[test]
    fn cases_use_candidates_from_both_scripts() {
        let corpus = SyntheticCorpus::build(3);
        let mut english = 0;
        let mut bangla = 0;
        for case in &corpus.cases {
            match case.request.language {
                Language::English => {
                    english += 1;
                    assert!(
                        case.candidates
                            .iter()
                            .all(|candidate| !contains_bengali(&candidate.text))
                    );
                }
                Language::Bangla => {
                    bangla += 1;
                    assert!(
                        case.candidates
                            .iter()
                            .any(|candidate| contains_bengali(&candidate.text))
                    );
                }
            }
            assert!(!case.candidates.is_empty());
        }
        assert_eq!(english, CASES_PER_LANGUAGE);
        assert_eq!(bangla, CASES_PER_LANGUAGE);
    }
}
