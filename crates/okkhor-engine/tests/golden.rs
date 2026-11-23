//! Golden-corpus integration test: every committed synthetic fixture parses
//! and validates, and `check_parity` scores exact top-1/top-5 hits on the
//! hand-computed miniature corpus.

use okkhor_core::Language;
use okkhor_engine::{CandidateEngine, EngineConfig, GoldenCorpus, check_parity};
use okkhor_lang::{Dictionary, DictionaryEntry};
use okkhor_learn::Smriti;
use okkhor_transliterate::{RULE_SCHEMA, RuleId, RuleSet, RuleTransliterator, TransliterationRule};

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

fn fixture_engine() -> (Vec<Dictionary>, RuleTransliterator, Smriti, EngineConfig) {
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
                id: RuleId::from_code("r-bha").expect("valid rule id"),
                input: "bha".to_owned(),
                output: "ভালো".to_owned(),
                language: Language::Bangla,
                priority: 0,
            },
            TransliterationRule {
                id: RuleId::from_code("r-b").expect("valid rule id"),
                input: "b".to_owned(),
                output: "ব".to_owned(),
                language: Language::Bangla,
                priority: 100,
            },
        ],
    };
    let transliterator = RuleTransliterator::new(rules).expect("valid fixture rules");
    (
        dictionaries,
        transliterator,
        Smriti::new(),
        EngineConfig::default(),
    )
}

#[test]
fn committed_fixtures_parse_and_hit_top1_and_top5() {
    let json = include_str!("fixtures/synthetic/corpus.json");
    let corpus = GoldenCorpus::from_json_str(json).expect("fixture must validate");
    assert_eq!(corpus.schema, 1);
    assert!(!corpus.cases.is_empty());

    let (dictionaries, transliterator, smriti, config) = fixture_engine();
    let engine = CandidateEngine::new(&dictionaries, &transliterator, &smriti, config)
        .expect("valid fixture engine");
    let report = check_parity(&engine, &corpus);
    assert_eq!(report.cases, corpus.cases.len() as u64);
    assert_eq!(report.top1_hits, report.cases);
    assert_eq!(report.top5_hits, report.cases);
    assert_eq!(report.top1_rate, 1.0);
    assert_eq!(report.top5_rate, 1.0);
}

#[test]
fn schema_bump_fixture_is_rejected() {
    let json = include_str!("fixtures/synthetic/corpus.json");
    let bumped = json.replacen("\"schema\": 1", "\"schema\": 2", 1);
    assert_ne!(bumped, json);
    let error = GoldenCorpus::from_json_str(&bumped).expect_err("schema bump rejected");
    assert_eq!(
        format!("{error}"),
        "unsupported golden schema 2; expected 1"
    );
}
