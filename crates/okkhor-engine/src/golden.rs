//! Synthetic golden corpus: schema, validation, and the parity metric.
//!
//! All fixtures are authored JSON committed under
//! `crates/okkhor-engine/tests/fixtures/synthetic/`. No legacy binaries,
//! proprietary assets, or real user text are read.

use okkhor_core::{Language, SuggestionRequest, normalize_lookup, normalize_nfc};
use serde::{Deserialize, Serialize};

use crate::config::LanguageBudget;
use crate::engine::CandidateEngine;
use crate::error::EngineError;

/// Expected value of [`GoldenCorpus::schema`].
pub const GOLDEN_SCHEMA: u32 = 1;

/// Maximum number of cases accepted from one file.
const MAX_CASES: usize = 10_000;
/// Case `id` (and `layout`) shape: `[a-z0-9_-]`, 1..=64 bytes.
const MAX_ID_LEN: usize = 64;
/// Raw `prefix` / `roman_input` cap in bytes.
const MAX_PREFIX_BYTES: usize = 255;
/// Maximum previous words carried per case.
const MAX_CONTEXT_WORDS: usize = 8;
/// `expected_top5` holds 1..=5 strings.
const MAX_EXPECTED: usize = 5;

/// One synthetic golden case: input plus the expected ranking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldenCase {
    pub id: String,
    pub prefix: String,
    pub roman_input: Option<String>,
    pub language: Language,
    pub layout: Option<String>,
    pub context: Vec<String>,
    pub expected_top5: Vec<String>,
}

/// A validated synthetic golden corpus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldenCorpus {
    pub schema: u32,
    pub cases: Vec<GoldenCase>,
}

impl GoldenCorpus {
    /// Parse and validate: schema check first (a missing `schema` field
    /// reports `found = 0`), then payload shape, id syntax + uniqueness,
    /// length caps, language codes, and NFC validation of expected strings.
    /// Never best-effort parsed.
    pub fn from_json_str(json: &str) -> Result<Self, EngineError> {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|_| EngineError::InvalidGolden {
                reason: "malformed_json",
            })?;
        let found = value
            .get("schema")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0);
        if found != GOLDEN_SCHEMA {
            return Err(EngineError::UnsupportedSchema {
                found,
                expected: GOLDEN_SCHEMA,
            });
        }
        let corpus: Self =
            serde_json::from_value(value).map_err(|_| EngineError::InvalidGolden {
                reason: "malformed_corpus",
            })?;
        corpus.validate()?;
        Ok(corpus)
    }

    fn validate(&self) -> Result<(), EngineError> {
        if self.cases.is_empty() || self.cases.len() > MAX_CASES {
            return Err(EngineError::InvalidGolden {
                reason: "invalid_case_count",
            });
        }
        let mut seen: Vec<&str> = Vec::with_capacity(self.cases.len());
        for case in &self.cases {
            if !valid_id(&case.id) {
                return Err(EngineError::InvalidGolden {
                    reason: "invalid_case_id",
                });
            }
            if seen.contains(&case.id.as_str()) {
                return Err(EngineError::InvalidGolden {
                    reason: "duplicate_case_id",
                });
            }
            seen.push(case.id.as_str());
            if case.prefix.len() > MAX_PREFIX_BYTES {
                return Err(EngineError::InvalidGolden {
                    reason: "prefix_too_long",
                });
            }
            if case
                .roman_input
                .as_ref()
                .is_some_and(|roman| roman.len() > MAX_PREFIX_BYTES)
            {
                return Err(EngineError::InvalidGolden {
                    reason: "roman_input_too_long",
                });
            }
            if case.context.len() > MAX_CONTEXT_WORDS {
                return Err(EngineError::InvalidGolden {
                    reason: "too_many_context_words",
                });
            }
            if case.expected_top5.is_empty() || case.expected_top5.len() > MAX_EXPECTED {
                return Err(EngineError::InvalidGolden {
                    reason: "invalid_expected_count",
                });
            }
            for expected in &case.expected_top5 {
                if expected.is_empty() || normalize_nfc(expected) != *expected {
                    return Err(EngineError::InvalidGolden {
                        reason: "expected_not_nfc",
                    });
                }
            }
            if case.layout.as_ref().is_some_and(|layout| !valid_id(layout)) {
                return Err(EngineError::InvalidGolden {
                    reason: "invalid_layout",
                });
            }
        }
        Ok(())
    }
}

/// `[a-z0-9_-]`, 1..=64 bytes. Mirrors the layout/rule id rules in the 0.3
/// crates so fixture ids stay portable.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
}

/// Top-1 / top-5 hit counts and rates over a corpus.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ParityReport {
    pub cases: u64,
    pub top1_hits: u64,
    pub top5_hits: u64,
    pub top1_rate: f64,
    pub top5_rate: f64,
}

/// Run every corpus case through `engine.suggest` with
/// `limit = min(5, budget max_limit)` and score ranks 1 and 1–5.
///
/// A top-1 hit is `suggestions.first() == expected_top5.first()`; a top-5 hit
/// is any `expected_top5` entry appearing in the first five suggestions, both
/// compared on exact normalized text. An empty corpus yields `0.0` rates with
/// `cases == 0` (no division by zero). Test/bench helper only: never on the
/// typing path.
pub fn check_parity(engine: &CandidateEngine, corpus: &GoldenCorpus) -> ParityReport {
    let mut top1_hits = 0u64;
    let mut top5_hits = 0u64;
    for case in &corpus.cases {
        let budget = LanguageBudget::for_language(engine.config(), case.language);
        let limit = 5usize.min(budget.max_limit);
        let request = SuggestionRequest {
            prefix: case.prefix.clone(),
            language: case.language,
            layout: case.layout.clone(),
            context: case.context.clone(),
            roman_input: case.roman_input.clone(),
            now_ms: 0,
            limit,
        };
        let suggestions = engine.suggest(&request);
        let expected: Vec<String> = case
            .expected_top5
            .iter()
            .map(|text| normalize_lookup(text))
            .collect();
        let actual: Vec<String> = suggestions
            .iter()
            .take(5)
            .map(|suggestion| normalize_lookup(&suggestion.text))
            .collect();
        if actual.first() == expected.first() {
            top1_hits += 1;
        }
        if expected.iter().any(|text| actual.contains(text)) {
            top5_hits += 1;
        }
    }
    let cases = corpus.cases.len() as u64;
    ParityReport {
        cases,
        top1_hits,
        top5_hits,
        top1_rate: rate(top1_hits, cases),
        top5_rate: rate(top5_hits, cases),
    }
}

fn rate(hits: u64, cases: u64) -> f64 {
    if cases == 0 {
        0.0
    } else {
        hits as f64 / cases as f64
    }
}

#[cfg(test)]
mod tests {
    use super::{GOLDEN_SCHEMA, GoldenCorpus, check_parity, rate};
    use crate::error::EngineError;

    const CASE: &str = r#"{"id":"case-one","prefix":"hel","language":"english","context":[],"expected_top5":["hello"]}"#;

    fn corpus_json(case: &str) -> String {
        format!(r#"{{"schema":{GOLDEN_SCHEMA},"cases":[{case}]}}"#)
    }

    #[test]
    fn schema_constant_is_one() {
        assert_eq!(GOLDEN_SCHEMA, 1);
    }

    #[test]
    fn missing_schema_reports_found_zero() {
        let error = GoldenCorpus::from_json_str(r#"{"cases":[]}"#).expect_err("schema required");
        assert!(matches!(
            error,
            EngineError::UnsupportedSchema {
                found: 0,
                expected: 1
            }
        ));
    }

    #[test]
    fn schema_bump_is_rejected() {
        let error =
            GoldenCorpus::from_json_str(r#"{"schema":2,"cases":[]}"#).expect_err("bump rejected");
        assert!(matches!(
            error,
            EngineError::UnsupportedSchema {
                found: 2,
                expected: 1
            }
        ));
    }

    #[test]
    fn malformed_json_is_invalid_golden() {
        assert!(matches!(
            GoldenCorpus::from_json_str("{oops"),
            Err(EngineError::InvalidGolden { .. })
        ));
    }

    #[test]
    fn empty_cases_rejected() {
        assert!(matches!(
            GoldenCorpus::from_json_str(&corpus_json("")).map(|_| ()),
            Err(EngineError::InvalidGolden { .. })
        ));
    }

    #[test]
    fn bad_ids_rejected() {
        for id in ["", "UPPER", "has space", "toolong", "dot.id"] {
            let id = if id == "toolong" {
                "a".repeat(65)
            } else {
                id.to_owned()
            };
            let case = format!(
                r#"{{"id":"{id}","prefix":"hel","language":"english","context":[],"expected_top5":["hello"]}}"#
            );
            assert!(
                GoldenCorpus::from_json_str(&corpus_json(&case)).is_err(),
                "id must be rejected: {id}"
            );
        }
    }

    #[test]
    fn duplicate_ids_rejected() {
        let json = corpus_json(&format!("{CASE},{CASE}"));
        assert!(matches!(
            GoldenCorpus::from_json_str(&json),
            Err(EngineError::InvalidGolden { .. })
        ));
    }

    #[test]
    fn overlong_prefix_rejected() {
        let prefix = "a".repeat(256);
        let case = format!(
            r#"{{"id":"long","prefix":"{prefix}","language":"english","context":[],"expected_top5":["hello"]}}"#
        );
        assert!(matches!(
            GoldenCorpus::from_json_str(&corpus_json(&case)),
            Err(EngineError::InvalidGolden { .. })
        ));
    }

    #[test]
    fn legacy_language_codes_rejected() {
        // Serde `Language` accepts snake_case only: never "bn"/"en".
        for code in ["bn", "en"] {
            let case = format!(
                r#"{{"id":"lang","prefix":"x","language":"{code}","context":[],"expected_top5":["y"]}}"#
            );
            assert!(
                GoldenCorpus::from_json_str(&corpus_json(&case)).is_err(),
                "code must be rejected: {code}"
            );
        }
    }

    #[test]
    fn non_nfc_expected_rejected() {
        // e + combining acute is not NFC.
        let case = "{\"id\":\"nfc\",\"prefix\":\"hel\",\"language\":\"english\",\"context\":[],\"expected_top5\":[\"cafe\\u0301\"]}";
        assert!(matches!(
            GoldenCorpus::from_json_str(&corpus_json(case)),
            Err(EngineError::InvalidGolden { .. })
        ));
    }

    #[test]
    fn empty_expected_rejected() {
        let case =
            r#"{"id":"empty","prefix":"hel","language":"english","context":[],"expected_top5":[]}"#;
        assert!(matches!(
            GoldenCorpus::from_json_str(&corpus_json(case)),
            Err(EngineError::InvalidGolden { .. })
        ));
    }

    #[test]
    fn too_much_context_rejected() {
        let context: Vec<String> = (0..9).map(|i| format!("\"w{i}\"")).collect();
        let case = format!(
            r#"{{"id":"ctx","prefix":"hel","language":"english","context":[{}],"expected_top5":["hello"]}}"#,
            context.join(",")
        );
        assert!(matches!(
            GoldenCorpus::from_json_str(&corpus_json(&case)),
            Err(EngineError::InvalidGolden { .. })
        ));
    }

    #[test]
    fn valid_corpus_parses_with_optional_fields() {
        let json = format!(
            r#"{{"schema":{GOLDEN_SCHEMA},"cases":[{CASE},{{"id":"case_two","prefix":"bha","roman_input":"bha","language":"bangla","layout":"avro","context":["আমি"],"expected_top5":["ভালো","ব"]}}]}}"#
        );
        let corpus = GoldenCorpus::from_json_str(&json).expect("valid corpus");
        assert_eq!(corpus.cases.len(), 2);
        assert_eq!(corpus.cases[1].roman_input.as_deref(), Some("bha"));
        assert_eq!(corpus.cases[1].layout.as_deref(), Some("avro"));
        assert_eq!(corpus.cases[1].context, vec!["আমি"]);
    }

    #[test]
    fn invalid_layout_rejected() {
        let case = r#"{"id":"lay","prefix":"hel","language":"english","layout":"AVRO","context":[],"expected_top5":["hello"]}"#;
        assert!(matches!(
            GoldenCorpus::from_json_str(&corpus_json(case)),
            Err(EngineError::InvalidGolden { .. })
        ));
    }

    #[test]
    fn empty_corpus_report_is_zero_without_panic() {
        use crate::engine::CandidateEngine;
        use crate::{EngineConfig, GoldenCase};
        use okkhor_learn::Smriti;
        use okkhor_transliterate::{RULE_SCHEMA, RuleSet, RuleTransliterator};

        let transliterator = RuleTransliterator::new(RuleSet {
            schema: RULE_SCHEMA,
            rules: Vec::new(),
        })
        .expect("empty rule set is valid");
        let smriti = Smriti::new();
        let dictionaries = Vec::new();
        let engine = CandidateEngine::new(
            &dictionaries,
            &transliterator,
            &smriti,
            EngineConfig::default(),
        )
        .expect("valid engine");
        // Hand-built empty corpus: the loader requires >= 1 case, but the
        // metric itself must not divide by zero.
        let corpus = GoldenCorpus {
            schema: GOLDEN_SCHEMA,
            cases: Vec::<GoldenCase>::new(),
        };
        let report = check_parity(&engine, &corpus);
        assert_eq!(report.cases, 0);
        assert_eq!(report.top1_hits, 0);
        assert_eq!(report.top5_hits, 0);
        assert_eq!(report.top1_rate, 0.0);
        assert_eq!(report.top5_rate, 0.0);
    }

    #[test]
    fn rate_handles_zero_cases() {
        assert_eq!(rate(0, 0), 0.0);
        assert_eq!(rate(1, 2), 0.5);
    }
}
