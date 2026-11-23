//! Tests for the ordered Avro regex pipeline (`PhoneticTransliterator`).
//!
//! Expectations are the legacy table's own outputs (the table wins over
//! Banglish intuition where they differ); see
//! `docs/design/transliteration.md`. The legacy table itself is data
//! extracted read-only from the owner's licensed installation.

use std::time::{Duration, Instant};

use okkhor_core::{CandidateSource, Language};
use okkhor_transliterate::{
    PhoneticTransliterator, TransliterateError, TransliterationOptions, TransliteratorEngine,
};

const GOLDEN_JSON: &str = include_str!("fixtures/synthetic/phonetic_golden.json");

#[derive(Debug, serde::Deserialize)]
struct GoldenFile {
    schema: u32,
    cases: Vec<GoldenCase>,
}

#[derive(Debug, serde::Deserialize)]
struct GoldenCase {
    input: String,
    expected: Vec<String>,
    min_rank: usize,
}

fn engine() -> &'static PhoneticTransliterator {
    PhoneticTransliterator::embedded()
        .as_ref()
        .expect("embedded Avro table compiles")
}

fn opts() -> TransliterationOptions {
    TransliterationOptions::default()
}

#[test]
fn embedded_table_loads_with_250_rules_in_order() {
    let eng = engine();
    assert_eq!(eng.rule_count(), 250);
}

#[test]
fn invalid_pattern_fails_at_load() {
    let err =
        PhoneticTransliterator::from_patterns(vec![("(".to_owned(), "x".to_owned())]).unwrap_err();
    assert!(matches!(err, TransliterateError::InvalidPattern));
    assert_eq!(err.kind(), "transliterate_invalid_pattern");
    assert_eq!(err.code(), None);
}

#[test]
fn rule_count_mismatch_is_rejected() {
    let err = PhoneticTransliterator::from_asset_json(
        r#"{"name":"t","source":"s","rule_count":2,"rules":[{"pattern":"a","replacement":"b"}]}"#,
    )
    .unwrap_err();
    assert!(matches!(
        err,
        TransliterateError::InvalidRule {
            reason: "phonetic_rule_count_mismatch"
        }
    ));
}

#[test]
fn golden_corpus_first_candidate_matches_table() {
    let eng = engine();
    let golden: GoldenFile = serde_json::from_str(GOLDEN_JSON).expect("phonetic golden parses");
    assert_eq!(golden.schema, 1);
    assert!(golden.cases.len() >= 60);
    for case in &golden.cases {
        let outputs = eng.transliterate(&case.input, &opts());
        assert!(!outputs.is_empty(), "empty output for {:?}", case.input);
        let actual: Vec<&str> = outputs.iter().map(|o| o.text.as_str()).collect();
        let rank = outputs
            .iter()
            .position(|o| o.text == case.expected[0])
            .map(|i| i + 1)
            .unwrap_or_else(|| {
                panic!(
                    "input {:?}: expected {:?}, got {:?}",
                    case.input, case.expected[0], actual
                )
            });
        assert!(
            rank <= case.min_rank,
            "input {:?}: expected {:?} within rank {}, got rank {} ({:?})",
            case.input,
            case.expected[0],
            case.min_rank,
            rank,
            outputs.iter().map(|o| &o.text).collect::<Vec<_>>()
        );
    }
}

#[test]
fn required_verification_words() {
    let eng = engine();
    for (input, expected) in [
        ("ami", "আমি"),
        ("amar", "আমার"),
        ("tumi", "তুমি"),
        ("apni", "আপ্নি"),
        ("bhalo", "ভাল"),
        ("bangla", "বাংলা"),
        ("dhonnobad", "ধন্নবাদ"),
        ("kemon", "কেমন"),
        ("acho", "আছ"),
        ("ki", "কি"),
        ("na", "না"),
        ("rong", "রং"),
        ("kkh", "ক্ষ"),
        ("gy", "গ্য"),
        ("sh", "শ"),
        ("Sh", "ষ"),
        ("ng", "ং"),
    ] {
        let outputs = eng.transliterate(input, &opts());
        assert_eq!(
            outputs.len(),
            1,
            "single deterministic output for {input:?}"
        );
        assert_eq!(outputs[0].text, expected, "table output for {input:?}");
        assert_eq!(outputs[0].language, Language::Bangla);
    }
}

#[test]
fn vowel_sign_vs_independent() {
    let eng = engine();
    // Lowercase `o` after a consonant is the inherent vowel (deleted by the
    // table); capital `O` is the ো-kar. `kal` and `kalo` share a table form.
    assert_eq!(eng.transliterate("kal", &opts())[0].text, "কাল");
    assert_eq!(eng.transliterate("kalo", &opts())[0].text, "কাল");
    assert_eq!(eng.transliterate("kalO", &opts())[0].text, "কালো");
    assert_eq!(eng.transliterate("bhalo", &opts())[0].text, "ভাল");
    assert_eq!(eng.transliterate("bhalO", &opts())[0].text, "ভালো");
    assert_eq!(eng.transliterate("ami", &opts())[0].text, "আমি");
    assert_eq!(eng.transliterate("tumi", &opts())[0].text, "তুমি");
}

#[test]
fn conjuncts_produce_hasanta() {
    let eng = engine();
    for word in ["dhakka", "uttor", "ranna", "okkhor"] {
        let text = &eng.transliterate(word, &opts())[0].text;
        assert!(
            text.contains('্'),
            "{word:?} should contain hasanta U+09CD, got {text:?}"
        );
    }
    assert_eq!(eng.transliterate("gyan", &opts())[0].text, "গ্যান");
}

#[test]
fn digits_and_punctuation_map_per_table() {
    let eng = engine();
    assert_eq!(eng.transliterate("123", &opts())[0].text, "১২৩");
    assert_eq!(eng.transliterate(".", &opts())[0].text, "।");
    assert_eq!(
        eng.transliterate("tumi, kemon?", &opts())[0].text,
        "তুমি, কেমন?"
    );
}

#[test]
fn output_converts_to_shared_candidate() {
    let eng = engine();
    let out = eng.transliterate("tumi", &opts())[0].clone();
    assert!((0.0..=1.0).contains(&out.confidence));
    let candidate = out.into_candidate();
    assert_eq!(candidate.text, "তুমি");
    assert_eq!(candidate.language, Language::Bangla);
    assert_eq!(candidate.source, CandidateSource::Transliteration);
    assert_eq!(candidate.features.phonetic_score, 0.95);
}

#[test]
fn empty_and_overlong_inputs_yield_empty_vec() {
    let eng = engine();
    assert!(eng.transliterate("", &opts()).is_empty());
    assert!(eng.transliterate("   ", &opts()).is_empty());
    assert!(eng.transliterate("\t\n ", &opts()).is_empty());
    // 64 chars convert; 65 do not.
    assert_eq!(eng.transliterate(&"a".repeat(64), &opts()).len(), 1);
    assert!(eng.transliterate(&"a".repeat(65), &opts()).is_empty());
    assert!(eng.transliterate(&"a".repeat(256), &opts()).is_empty());
    // max_candidates is respected.
    assert!(
        eng.transliterate(
            "tumi",
            &TransliterationOptions {
                max_candidates: 0,
                ..opts()
            }
        )
        .is_empty()
    );
}

#[test]
fn no_panic_on_arbitrary_unicode() {
    let eng = engine();
    for input in [
        "hello 🌍 বিশ্ব 123",
        "t``",
        ":::",
        "^^^",
        "অ already bangla",
        "\u{200c}\u{200d}",
        "mixed123!@#tumi",
    ] {
        let first = eng.transliterate(input, &opts());
        let second = eng.transliterate(input, &opts());
        assert_eq!(first, second, "deterministic for {input:?}");
        if !input.trim().is_empty() {
            assert!(!first.is_empty(), "non-empty for {input:?}");
        }
    }
}

#[test]
fn synthetic_corpus_is_deterministic_and_nonempty() {
    let eng = engine();
    let parts = [
        "ka", "kha", "ga", "ta", "na", "ma", "ra", "la", "sa", "a", "i", "u", "e", "o", "O", "ng",
        "kk", "tt", "nn", "sh", "Sh", "ch", "th", "dh", "bh", "gy", "kkh", "01", ".", "bhalo",
        "tumi", "r", "y", "w", "z", "t``", "OI", "OU", "rri",
    ];
    let mut count = 0;
    for i in 0..200 {
        let a = parts[i % parts.len()];
        let b = parts[(i * 7 + 3) % parts.len()];
        let c = parts[(i * 13 + 5) % parts.len()];
        let input = if i % 5 == 4 {
            format!("{a} {b} {c}")
        } else {
            format!("{a}{b}{c}")
        };
        let first = eng.transliterate(&input, &opts());
        let second = eng.transliterate(&input, &opts());
        assert_eq!(first, second, "deterministic for {input:?}");
        assert_eq!(first.len(), 1, "single output for {input:?}");
        assert!(!first[0].text.is_empty(), "non-empty output for {input:?}");
        count += 1;
    }
    assert_eq!(count, 200);
}

#[test]
fn p99_conversion_latency_smoke() {
    let eng = engine();
    // Exactly 32 chars of roman input.
    let mut base = String::from("amar sonar bangla ami tomay bhalob");
    base.truncate(32);
    assert_eq!(base.chars().count(), 32);
    let mut samples: Vec<Duration> = Vec::with_capacity(2000);
    for _ in 0..2000 {
        let start = Instant::now();
        let out = eng.transliterate(&base, &opts());
        let elapsed = start.elapsed();
        assert_eq!(out.len(), 1);
        samples.push(elapsed);
    }
    samples.sort_unstable();
    let p99 = samples[samples.len() * 99 / 100];
    #[cfg(debug_assertions)]
    let budget = Duration::from_millis(10);
    #[cfg(not(debug_assertions))]
    let budget = Duration::from_millis(1);
    assert!(
        p99 <= budget,
        "p99 {p99:?} exceeds budget {budget:?} for 32-char conversion"
    );
}
