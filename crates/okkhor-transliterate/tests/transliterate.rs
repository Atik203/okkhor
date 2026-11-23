//! Integration tests for `okkhor-transliterate` (contract §4.7).
//!
//! Fixtures are synthetic only and loaded with `include_str!`; no legacy
//! dictionary or transliteration table is read.

use okkhor_core::{CandidateSource, Language};
use okkhor_transliterate::{
    RULE_SCHEMA, RuleId, RuleSet, RuleTransliterator, TransliterateError, TransliterationOptions,
    TransliteratorEngine, is_bengali_script, normalize_bengali, normalize_key, normalize_roman,
};

const RULES_JSON: &str = include_str!("fixtures/synthetic/rules.json");
const GOLDEN_JSON: &str = include_str!("fixtures/synthetic/golden.json");

/// Private golden-corpus type: `golden.json` has no required public type.
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

fn engine_from(json: &str) -> RuleTransliterator {
    RuleTransliterator::new(RuleSet::from_json_str(json).expect("valid rules")).expect("valid")
}

fn two_rules_reversed() -> &'static str {
    // File order is the reverse of engine order: the shorter input is listed
    // first, so `from_json_str` must preserve it while `new` sorts it.
    r#"{
        "schema": 1,
        "rules": [
            { "id": "rule_bha", "input": "bha", "output": "Xbha", "language": "bangla", "priority": 100 },
            { "id": "rule_bhalo", "input": "bhalo", "output": "Xbhalo", "language": "bangla", "priority": 50 }
        ]
    }"#
}

#[test]
fn fixtures_parse_and_schema_is_one() {
    let rules = RuleSet::from_json_str(RULES_JSON).expect("rules.json parses");
    assert_eq!(rules.schema, RULE_SCHEMA);
    assert_eq!(rules.len(), 2);
    assert!(!rules.is_empty());

    let golden: GoldenFile = serde_json::from_str(GOLDEN_JSON).expect("golden.json parses");
    assert_eq!(golden.schema, 1);
    assert_eq!(golden.cases.len(), 2);
}

#[test]
fn schema_mismatch_is_rejected() {
    let err = RuleSet::from_json_str(r#"{"schema": 2, "rules": []}"#).unwrap_err();
    assert!(matches!(
        err,
        TransliterateError::UnsupportedSchema {
            found: 2,
            expected: RULE_SCHEMA
        }
    ));
    assert_eq!(err.kind(), "transliterate_unsupported_schema");
    assert_eq!(err.code(), None);
}

#[test]
fn missing_schema_reports_found_zero() {
    let err = RuleSet::from_json_str(r#"{"rules": []}"#).unwrap_err();
    assert!(matches!(
        err,
        TransliterateError::UnsupportedSchema {
            found: 0,
            expected: RULE_SCHEMA
        }
    ));
}

#[test]
fn duplicate_rule_id_is_rejected() {
    let err = RuleTransliterator::new(
        RuleSet::from_json_str(
            r#"{
                "schema": 1,
                "rules": [
                    { "id": "rule_dup", "input": "a", "output": "Xa", "language": "bangla", "priority": 100 },
                    { "id": "rule_dup", "input": "b", "output": "Xb", "language": "bangla", "priority": 100 }
                ]
            }"#,
        )
        .expect("shape parses"),
    )
    .unwrap_err();
    assert!(matches!(err, TransliterateError::DuplicateRuleId));
    assert_eq!(err.kind(), "transliterate_duplicate_rule");
}

#[test]
fn non_ascii_input_is_rejected() {
    let err = RuleTransliterator::new(
        RuleSet::from_json_str(
            r#"{
                "schema": 1,
                "rules": [
                    { "id": "rule_bn", "input": "abc", "output": "X", "language": "bangla", "priority": 100 }
                ]
            }"#,
        )
        .expect("shape parses"),
    );
    // Sanity: the ASCII control above passes; the Bengali input below fails.
    assert!(err.is_ok());
    let err = RuleTransliterator::new(
        RuleSet::from_json_str(
            "{\"schema\": 1, \"rules\": [{\"id\": \"rule_bn\", \"input\": \"\\u09ad\\u09be\", \"output\": \"X\", \"language\": \"bangla\", \"priority\": 100}]}",
        )
        .expect("shape parses"),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        TransliterateError::InvalidRule {
            reason: "non_ascii_input"
        }
    ));
    assert_eq!(err.kind(), "transliterate_invalid_rule");
}

#[test]
fn non_bangla_language_is_rejected() {
    let err = RuleTransliterator::new(
        RuleSet::from_json_str(
            r#"{
                "schema": 1,
                "rules": [
                    { "id": "rule_en", "input": "abc", "output": "X", "language": "english", "priority": 100 }
                ]
            }"#,
        )
        .expect("shape parses"),
    )
    .unwrap_err();
    assert!(matches!(err, TransliterateError::UnsupportedLanguage));
    assert_eq!(err.kind(), "transliterate_unsupported_language");
}

#[test]
fn overlong_rule_input_is_rejected() {
    let long = "a".repeat(33);
    let json = format!(
        "{{\"schema\": 1, \"rules\": [{{\"id\": \"rule_long\", \"input\": \"{long}\", \"output\": \"X\", \"language\": \"bangla\", \"priority\": 100}}]}}"
    );
    let err =
        RuleTransliterator::new(RuleSet::from_json_str(&json).expect("shape parses")).unwrap_err();
    assert!(matches!(err, TransliterateError::InputTooLong));
    assert_eq!(err.kind(), "transliterate_input_too_long");
}

#[test]
fn empty_whitespace_and_overlong_input_yield_empty_vec() {
    let engine = engine_from(RULES_JSON);
    let options = TransliterationOptions::default();
    assert!(engine.transliterate("", &options).is_empty());
    assert!(engine.transliterate("   \t\n ", &options).is_empty());
    assert!(engine.transliterate(&"a".repeat(255), &options).is_empty());
    assert!(engine.transliterate(&"a".repeat(256), &options).is_empty());
    assert!(engine.transliterate(&"a".repeat(4096), &options).is_empty());
}

#[test]
fn from_json_preserves_file_order_and_new_sorts() {
    // Ownership split (§4.3): `from_json_str` keeps file order (short first),
    // while the engine matches longest-first regardless of file order.
    let rules = RuleSet::from_json_str(two_rules_reversed()).expect("parses");
    assert_eq!(rules.rules[0].input, "bha");
    assert_eq!(rules.rules[1].input, "bhalo");

    let engine = RuleTransliterator::new(rules).expect("valid");
    assert_eq!(engine.rule_count(), 2);
    let options = TransliterationOptions::default();
    let outputs = engine.transliterate("bhalo", &options);
    assert_eq!(outputs.len(), 2);
    assert_eq!(outputs[0].text, "Xbhalo");
    assert_eq!(outputs[1].text, "Xbha");
}

#[test]
fn duplicate_normalized_inputs_are_rejected() {
    let err = RuleTransliterator::new(
        RuleSet::from_json_str(
            r#"{
                "schema": 1,
                "rules": [
                    { "id": "rule_one", "input": "BHA", "output": "X1", "language": "bangla", "priority": 100 },
                    { "id": "rule_two", "input": "bha", "output": "X2", "language": "bangla", "priority": 100 }
                ]
            }"#,
        )
        .expect("shape parses"),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        TransliterateError::InvalidRule {
            reason: "duplicate_rule_input"
        }
    ));
}

#[test]
fn longest_match_wins() {
    let engine = engine_from(RULES_JSON);
    let options = TransliterationOptions::default();
    let outputs = engine.transliterate("bhalo", &options);
    assert!(outputs.len() >= 2);
    assert_eq!(outputs[0].text, "ভালো");
    assert_eq!(outputs[0].rule_id, RuleId::from_code("rule_bhalo").unwrap());
}

#[test]
fn priority_tie_break() {
    // Both rules partially match "a" with equal-length inputs; the lower
    // priority value must come first.
    let engine = engine_from(
        r#"{
            "schema": 1,
            "rules": [
                { "id": "rule_hi", "input": "ab", "output": "Xhi", "language": "bangla", "priority": 200 },
                { "id": "rule_lo", "input": "ac", "output": "Xlo", "language": "bangla", "priority": 10 }
            ]
        }"#,
    );
    let outputs = engine.transliterate("a", &TransliterationOptions::default());
    assert_eq!(outputs.len(), 2);
    assert_eq!(outputs[0].text, "Xlo");
    assert_eq!(outputs[1].text, "Xhi");
    assert!(outputs[0].confidence > outputs[1].confidence);
}

#[test]
fn id_tie_break() {
    // Equal length and priority: ascending rule id wins.
    let engine = engine_from(
        r#"{
            "schema": 1,
            "rules": [
                { "id": "rule_zed", "input": "ab", "output": "Xzed", "language": "bangla", "priority": 100 },
                { "id": "rule_aye", "input": "ac", "output": "Xaye", "language": "bangla", "priority": 100 }
            ]
        }"#,
    );
    let outputs = engine.transliterate("a", &TransliterationOptions::default());
    assert_eq!(outputs.len(), 2);
    assert_eq!(outputs[0].rule_id.as_str(), "rule_aye");
    assert_eq!(outputs[1].rule_id.as_str(), "rule_zed");
}

#[test]
fn allow_partial_switch() {
    let engine = engine_from(RULES_JSON);
    let partial = engine.transliterate("bha", &TransliterationOptions::default());
    assert_eq!(partial.len(), 2);
    assert_eq!(partial[0].text, "ভা");

    let full_only = engine.transliterate(
        "bha",
        &TransliterationOptions {
            allow_partial: false,
            ..TransliterationOptions::default()
        },
    );
    assert_eq!(full_only.len(), 1);
    assert_eq!(full_only[0].text, "ভা");
}

#[test]
fn max_candidates_caps_output() {
    let engine = engine_from(RULES_JSON);
    let options = TransliterationOptions {
        max_candidates: 1,
        ..TransliterationOptions::default()
    };
    let outputs = engine.transliterate("bhalo", &options);
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].text, "ভালো");

    let none = engine.transliterate(
        "bhalo",
        &TransliterationOptions {
            max_candidates: 0,
            ..TransliterationOptions::default()
        },
    );
    assert!(none.is_empty());
}

#[test]
fn normalization_helpers() {
    assert_eq!(normalize_roman("  BHalo "), "bhalo");
    assert_eq!(normalize_key("  BHalo "), "bhalo");
    assert_eq!(normalize_bengali("ভালো"), "ভালো");
    // Non-ASCII letters are left unchanged by key normalization.
    assert_eq!(normalize_key("ভালো"), "ভালো");
    assert!(is_bengali_script("hello বিশ্ব"));
    assert!(!is_bengali_script("hello world"));
}

#[test]
fn case_insensitive_matching() {
    let engine = engine_from(
        r#"{
            "schema": 1,
            "rules": [
                { "id": "rule_up", "input": "BHA", "output": "Xup", "language": "bangla", "priority": 100 }
            ]
        }"#,
    );
    let options = TransliterationOptions {
        allow_partial: false,
        ..TransliterationOptions::default()
    };
    let outputs = engine.transliterate("bha", &options);
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].text, "Xup");
}

#[test]
fn non_nfc_output_is_rejected() {
    // "e" + combining acute is not NFC ("é" is).
    let err = RuleTransliterator::new(
        RuleSet::from_json_str(
            "{\"schema\": 1, \"rules\": [{\"id\": \"rule_dec\", \"input\": \"aa\", \"output\": \"e\\u0301\", \"language\": \"bangla\", \"priority\": 100}]}",
        )
        .expect("shape parses"),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        TransliterateError::InvalidRule {
            reason: "non_nfc_output"
        }
    ));
}

#[test]
fn output_converts_to_shared_candidate() {
    let engine = engine_from(RULES_JSON);
    let outputs = engine.transliterate("bhalo", &TransliterationOptions::default());
    let confidence = outputs[0].confidence;
    assert!((0.0..=1.0).contains(&confidence));
    let candidate = outputs[0].clone().into_candidate();
    assert_eq!(candidate.text, "ভালো");
    assert_eq!(candidate.language, Language::Bangla);
    assert_eq!(candidate.source, CandidateSource::Transliteration);
    assert_eq!(candidate.features.phonetic_score, confidence);
}

#[test]
fn golden_cases_pass() {
    let engine = engine_from(RULES_JSON);
    let golden: GoldenFile = serde_json::from_str(GOLDEN_JSON).expect("golden parses");
    let options = TransliterationOptions::default();
    for case in &golden.cases {
        let outputs = engine.transliterate(&case.input, &options);
        let rank = outputs
            .iter()
            .position(|o| o.text == case.expected[0])
            .map(|i| i + 1)
            .expect("expected output is produced");
        assert!(
            rank <= case.min_rank,
            "input {:?}: expected {:?} within rank {}, got rank {}",
            case.input,
            case.expected[0],
            case.min_rank,
            rank
        );
    }
}

#[test]
fn error_display_leaks_no_input_text() {
    let hostile = "qzxwHOSTILEkvbn";
    let json = format!(
        "{{\"schema\": 1, \"rules\": [{{\"id\": \"rule_h\", \"input\": \"{hostile}\", \"output\": \"X\", \"language\": \"english\", \"priority\": 100}}]}}"
    );
    let err =
        RuleTransliterator::new(RuleSet::from_json_str(&json).expect("shape parses")).unwrap_err();
    assert!(matches!(err, TransliterateError::UnsupportedLanguage));
    assert!(!format!("{err}").contains(hostile));

    let dup = RuleTransliterator::new(
        RuleSet::from_json_str(&format!(
            "{{\"schema\": 1, \"rules\": [\
                {{\"id\": \"rule_h\", \"input\": \"a\", \"output\": \"X\", \"language\": \"bangla\", \"priority\": 100}}, \
                {{\"id\": \"rule_h\", \"input\": \"{hostile}\", \"output\": \"Y\", \"language\": \"bangla\", \"priority\": 100}}]}}"
        ))
        .expect("shape parses"),
    )
    .unwrap_err();
    assert!(!format!("{dup}").contains(hostile));

    assert!(!format!("{}", TransliterateError::InputTooLong).contains(hostile));
    assert!(
        !format!(
            "{}",
            TransliterateError::InvalidRule {
                reason: "non_ascii_input"
            }
        )
        .contains(hostile)
    );
}
