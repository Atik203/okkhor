//! Golden suggestion corpus: top-1 and top-5 rates for everyday input.

use okkhor_engine::{SuggestRequest, SuggestionEngine};
use okkhor_learn::Smriti;
use serde::Deserialize;

#[derive(Deserialize)]
struct Corpus {
    schema: u32,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: u32,
    roman: String,
    expected: Vec<String>,
}

#[test]
fn golden_top1_and_top5_rates() {
    let raw = include_str!("fixtures/suggestions/golden-v1.json");
    let corpus: Corpus = serde_json::from_str(raw).expect("golden corpus parses");
    assert_eq!(corpus.schema, 1);
    assert!(corpus.cases.len() >= 100, "cases {}", corpus.cases.len());

    let engine = SuggestionEngine::from_embedded().expect("engine");
    assert!(engine.index_error().is_none(), "{:?}", engine.index_error());
    let smriti = Smriti::new();

    let mut top1 = 0usize;
    let mut top5 = 0usize;
    let mut misses = Vec::new();
    for case in &corpus.cases {
        let rows = engine.suggest(&SuggestRequest::new(&case.roman, 8), &smriti);
        let texts: Vec<&str> = rows.iter().map(|row| row.text.as_str()).collect();
        if texts.first().copied() == case.expected.first().map(String::as_str) {
            top1 += 1;
        }
        let hit = case
            .expected
            .iter()
            .any(|expected| texts.iter().take(5).any(|text| text == expected));
        if hit {
            top5 += 1;
        } else {
            misses.push(format!(
                "{} {} -> {:?} (want {:?})",
                case.id, case.roman, texts, case.expected
            ));
        }
    }

    let total = corpus.cases.len() as f64;
    let top1_rate = top1 as f64 / total;
    let top5_rate = top5 as f64 / total;
    eprintln!(
        "golden: top1 {top1}/{} ({:.1}%), top5 {top5}/{} ({:.1}%)",
        corpus.cases.len(),
        top1_rate * 100.0,
        corpus.cases.len(),
        top5_rate * 100.0
    );
    for miss in &misses {
        eprintln!("miss: {miss}");
    }

    assert!(
        top1_rate >= 0.90,
        "top1 {:.1}% below 90%",
        top1_rate * 100.0
    );
    assert!(
        top5_rate >= 0.98,
        "top5 {:.1}% below 98%",
        top5_rate * 100.0
    );
}
