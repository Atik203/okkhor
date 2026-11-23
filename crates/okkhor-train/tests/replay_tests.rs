#![forbid(unsafe_code)]
//! Task 1 acceptance tests: schema order, determinism, bounds, replay
//! parsing, generator reproducibility, and metric math.

use okkhor_core::{Candidate, CandidateSource, Language, SuggestionRequest};
use okkhor_learn::{LearningEvent, LearningSnapshot, Smriti, SmritiConfig};
use okkhor_train::{
    FeatureVectorV1, RankedOutcome, SCHEMA_VERSION, compute_metrics, extract_features,
    generate_cases, percentile_ns, read_replay_str, render_jsonl, sha256_hex,
};

fn snapshot_with_word() -> LearningSnapshot {
    let smriti = Smriti::new();
    smriti.observe(LearningEvent::WordCommitted {
        word: "ভালো".to_owned(),
        language: Language::Bangla,
        previous_word: Some("আমি".to_owned()),
        timestamp_ms: 1_000,
        source: okkhor_learn::CommitSource::Typed,
    });
    smriti.snapshot()
}

fn request() -> SuggestionRequest {
    SuggestionRequest::new("ভা", Language::Bangla, 5)
        .with_layout("avro")
        .with_context(["আমি"])
        .at(2_000)
}

fn candidate() -> Candidate {
    let mut candidate = Candidate::new("ভালো", Language::Bangla, CandidateSource::Dictionary);
    candidate.features.base_score = 0.5;
    candidate.features.phonetic_score = 0.25;
    candidate
}

#[test]
fn schema_version_is_one_and_field_order_is_pinned() {
    assert_eq!(SCHEMA_VERSION, 1);
    assert_eq!(
        FeatureVectorV1::field_names(),
        [
            "base_score",
            "candidate_source",
            "language",
            "layout",
            "prefix_len",
            "word_len",
            "user_word_count",
            "pair_count",
            "recency",
            "acceptance_rate",
            "correction_rate",
            "bigram_score",
            "phonetic_score",
            "is_user_vocab",
            "context_language",
            "script_mix",
        ]
    );
}

#[test]
fn extraction_is_deterministic_and_byte_identical() {
    let snapshot = snapshot_with_word();
    let first = extract_features(&request(), &candidate(), &snapshot).expect("extract");
    let second = extract_features(&request(), &candidate(), &snapshot).expect("extract");
    assert_eq!(first, second);
    assert_eq!(first.to_array(), second.to_array());
    let json_first = serde_json::to_string(&first.to_array()).expect("json");
    let json_second = serde_json::to_string(&second.to_array()).expect("json");
    assert_eq!(json_first, json_second);
}

#[test]
fn extraction_reflects_learned_counts() {
    let empty = Smriti::new().snapshot();
    let full = snapshot_with_word();
    let req = request();
    let cand = candidate();
    let base = extract_features(&req, &cand, &empty).expect("empty");
    let learned = extract_features(&req, &cand, &full).expect("full");
    assert_eq!(base.user_word_count, 0);
    assert_eq!(learned.user_word_count, 1);
    assert!(learned.is_user_vocab);
    assert!(!base.is_user_vocab);
    assert!(learned.reciprocal_check_recency_positive());
}

trait RecencyCheck {
    fn reciprocal_check_recency_positive(&self) -> bool;
}

impl RecencyCheck for FeatureVectorV1 {
    fn reciprocal_check_recency_positive(&self) -> bool {
        self.recency > 0.0
    }
}

#[test]
fn extraction_bounds_reject_empty_and_overlong() {
    let snapshot = Smriti::new().snapshot();
    let empty_candidate = Candidate::new("", Language::Bangla, CandidateSource::Dictionary);
    assert!(extract_features(&request(), &empty_candidate, &snapshot).is_err());
    let long_prefix = SuggestionRequest::new("a".repeat(257), Language::English, 5);
    assert!(extract_features(&long_prefix, &candidate(), &snapshot).is_err());
    let long_word = Candidate::new(
        "a".repeat(129),
        Language::English,
        CandidateSource::Dictionary,
    );
    assert!(
        extract_features(
            &SuggestionRequest::new("a", Language::English, 5),
            &long_word,
            &snapshot
        )
        .is_err()
    );
}

#[test]
fn extraction_clamps_non_finite_scores() {
    let snapshot = Smriti::new().snapshot();
    let mut bad = candidate();
    bad.features.base_score = f32::NAN;
    bad.features.phonetic_score = f32::INFINITY;
    let vector = extract_features(&request(), &bad, &snapshot).expect("clamped");
    assert!(vector.base_score.is_finite());
    assert!((0.0..=1.0).contains(&vector.phonetic_score));
    assert_eq!(vector.candidate_source, 0);
    assert_eq!(vector.language, 1);
    assert_eq!(vector.layout, 1);
}

#[test]
fn replay_parse_accepts_valid_and_rejects_bad() {
    let cases = generate_cases(7, 8).expect("cases");
    let document = render_jsonl(&cases);
    let parsed = read_replay_str(&document).expect("valid replay parses");
    assert_eq!(parsed.len(), 8);

    // Missing schema reports unsupported (found = 0).
    let missing: String = "{\"case_id\":\"x\",\"language\":\"bangla\",\"prefix\":\"a\",\"expected\":[\"a\"],\"shown\":[\"a\"],\"candidates\":[\"a\"]}\n".to_owned();
    assert!(read_replay_str(&missing).is_err());

    // Duplicate ids rejected.
    let line = document.lines().next().expect("one line").to_owned();
    let duplicated = format!("{line}\n{line}\n");
    assert!(read_replay_str(&duplicated).is_err());

    // Bad accepted index rejected.
    let mut bad = cases[0].clone();
    bad.accepted = Some(999);
    let bad_line = serde_json::to_string(&bad).expect("json");
    assert!(read_replay_str(&format!("{bad_line}\n")).is_err());

    // Empty document rejected.
    assert!(read_replay_str("").is_err());
}

#[test]
fn generator_is_deterministic_with_checksum() {
    let first = generate_cases(20261123, 240).expect("gen");
    let second = generate_cases(20261123, 240).expect("gen");
    let third = generate_cases(999, 240).expect("gen");
    let first_doc = render_jsonl(&first);
    let second_doc = render_jsonl(&second);
    let third_doc = render_jsonl(&third);
    assert_eq!(first_doc, second_doc);
    assert_eq!(
        sha256_hex(first_doc.as_bytes()),
        sha256_hex(second_doc.as_bytes())
    );
    assert_ne!(first_doc, third_doc);
    assert!(first.len() >= 200);
    // Case ids unique and well-formed.
    let mut ids: Vec<&str> = first.iter().map(|c| c.case_id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), first.len());
}

#[test]
fn metric_math_on_hand_built_fixture() {
    let outcomes = vec![
        RankedOutcome {
            case_id: "a".into(),
            language: "bangla".into(),
            layout: "avro".into(),
            expected: vec!["x".into()],
            ranked: vec!["x".into(), "y".into()],
            accepted: true,
            prefix_len_chars: 1,
            expected_len_chars: 4,
            latency_ns: 100,
        },
        RankedOutcome {
            case_id: "b".into(),
            language: "bangla".into(),
            layout: "avro".into(),
            expected: vec!["y".into()],
            ranked: vec!["x".into(), "y".into()],
            accepted: false,
            prefix_len_chars: 1,
            expected_len_chars: 4,
            latency_ns: 200,
        },
        RankedOutcome {
            case_id: "c".into(),
            language: "english".into(),
            layout: "none".into(),
            expected: vec!["z".into()],
            ranked: vec![
                "a".into(),
                "b".into(),
                "c".into(),
                "d".into(),
                "e".into(),
                "z".into(),
            ],
            accepted: false,
            prefix_len_chars: 1,
            expected_len_chars: 4,
            latency_ns: 300,
        },
        RankedOutcome {
            case_id: "d".into(),
            language: "english".into(),
            layout: "none".into(),
            expected: vec!["w".into()],
            ranked: vec!["a".into(), "b".into()],
            accepted: true,
            prefix_len_chars: 1,
            expected_len_chars: 4,
            latency_ns: 400,
        },
    ];
    let metrics = compute_metrics(&outcomes).expect("metrics");
    assert_eq!(metrics.cases, 4);
    assert!((metrics.overall.top1 - 0.25).abs() < 1e-12);
    assert!((metrics.overall.top5 - 0.5).abs() < 1e-12);
    let want_mrr = (1.0 + 0.5 + 1.0 / 6.0) / 4.0;
    assert!((metrics.overall.mrr - want_mrr).abs() < 1e-12);
    assert!((metrics.overall.acceptance_rate - 0.5).abs() < 1e-12);
    // Only the top-1 hit saves keystrokes: (4-1)/4 = 0.75, averaged over 4.
    assert!((metrics.overall.keystroke_savings - 0.75 / 4.0).abs() < 1e-12);
}

#[test]
fn segment_filtering_splits_language_and_layout() {
    let outcomes = vec![
        RankedOutcome {
            case_id: "bn1".into(),
            language: "bangla".into(),
            layout: "avro".into(),
            expected: vec!["x".into()],
            ranked: vec!["x".into()],
            accepted: true,
            prefix_len_chars: 1,
            expected_len_chars: 2,
            latency_ns: 10,
        },
        RankedOutcome {
            case_id: "en1".into(),
            language: "english".into(),
            layout: "none".into(),
            expected: vec!["y".into()],
            ranked: vec!["q".into()],
            accepted: false,
            prefix_len_chars: 1,
            expected_len_chars: 2,
            latency_ns: 20,
        },
    ];
    let metrics = compute_metrics(&outcomes).expect("metrics");
    assert_eq!(metrics.by_language["bangla"].top1, 1.0);
    assert_eq!(metrics.by_language["english"].top1, 0.0);
    assert_eq!(metrics.by_layout["avro"].cases, 1);
    assert_eq!(metrics.by_layout["none"].cases, 1);
}

#[test]
fn empty_corpus_fails_closed() {
    assert!(compute_metrics(&[]).is_err());
}

#[test]
fn p99_uses_nearest_rank() {
    let samples: Vec<u64> = (1..=100).collect();
    assert_eq!(percentile_ns(&samples, 50), Some(50));
    assert_eq!(percentile_ns(&samples, 95), Some(95));
    assert_eq!(percentile_ns(&samples, 99), Some(99));
    assert_eq!(percentile_ns(&[], 99), None);
    assert_eq!(percentile_ns(&[10], 99), Some(10));
}

#[test]
fn snapshot_round_trip_preserves_features() {
    let smriti = Smriti::with_config(SmritiConfig::default()).expect("config");
    for _ in 0..3 {
        smriti.observe(LearningEvent::WordCommitted {
            word: "hello".to_owned(),
            language: Language::English,
            previous_word: None,
            timestamp_ms: 500,
            source: okkhor_learn::CommitSource::Typed,
        });
    }
    let snapshot = smriti.snapshot();
    assert!(snapshot.contains_word(Language::English, "hello"));
    assert_eq!(
        snapshot
            .word_stat(Language::English, "hello")
            .expect("stat")
            .count,
        3
    );
}
