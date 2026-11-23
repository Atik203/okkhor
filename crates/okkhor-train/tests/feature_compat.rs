#![forbid(unsafe_code)]
//! Byte-compatibility gate for the v1 feature schema.
//!
//! Runs `extract_features` over the frozen synthetic replay set in stable
//! case/candidate order, serializes every `to_array()` value as little-endian
//! f32 bytes, and pins the SHA-256 of the concatenation. The expected value was
//! recorded from the pre-extraction implementation in `okkhor-train` and must
//! not change when the schema moves to the shipped `okkhor-features` crate.
//!
//! This is synthetic-only data; it is a compatibility check, not a quality
//! claim.

use okkhor_core::{Candidate, CandidateSource, Language, SuggestionRequest};
use okkhor_learn::Smriti;
use okkhor_train::{ReplayCase, extract_features, read_replay_str, sha256_hex};

/// SHA-256 of every `to_array()` value (little-endian f32) over the frozen set.
const EXPECTED_HASH: &str = "2a81d2f9853a26a5194930327e194dde4fc24a51831fcfd014dc12daab9926e9";

fn request_for(case: &ReplayCase) -> SuggestionRequest {
    let roman = match case.language {
        Language::Bangla if case.prefix.is_ascii() && !case.prefix.is_empty() => {
            Some(case.prefix.clone())
        }
        _ => None,
    };
    SuggestionRequest {
        prefix: case.prefix.clone(),
        language: case.language,
        layout: case.layout.clone(),
        context: case.context.clone(),
        roman_input: roman,
        now_ms: 0,
        limit: 5,
    }
}

fn hash_feature_bytes(document: &str) -> (String, usize, usize) {
    let cases = read_replay_str(document).expect("frozen replay set parses");
    let snapshot = Smriti::new().snapshot();
    let mut bytes = Vec::new();
    let mut candidates = 0usize;
    for case in &cases {
        let request = request_for(case);
        for (index, text) in case.candidates.iter().enumerate() {
            let mut candidate =
                Candidate::new(text.clone(), case.language, CandidateSource::Dictionary);
            candidate.features.base_score = 1.0 / (1.0 + index as f32);
            let vector = extract_features(&request, &candidate, &snapshot).expect("extract");
            for value in vector.to_array() {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            candidates += 1;
        }
    }
    (sha256_hex(&bytes), cases.len(), candidates)
}

#[test]
fn feature_bytes_match_the_frozen_v1_schema() {
    let document = include_str!("../data/synthetic-replay.jsonl");
    let (hash, cases, candidates) = hash_feature_bytes(document);
    println!("feature compat hash={hash} cases={cases} candidates={candidates}");
    assert_eq!(cases, 240, "frozen replay set size changed");
    assert_eq!(
        hash, EXPECTED_HASH,
        "feature extraction bytes changed; schema v1 is frozen"
    );
}
