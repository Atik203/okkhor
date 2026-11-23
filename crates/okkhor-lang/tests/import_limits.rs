//! Entry-limit enforcement at exactly the cap, one over, and via the
//! per-fixture byte budget.

mod common;

use okkhor_lang::{ImportOptions, SkipReason, error::LangError, import_fixture_dir};

use common::{temp_dir, test_options, write_file};

fn dictionary_with(tag: &str, count: usize) -> std::path::PathBuf {
    let dir = temp_dir(tag);
    let mut entries = String::from("[");
    for i in 0..count {
        if i > 0 {
            entries.push_str(", ");
        }
        entries.push_str(&format!("{{\"word\": \"w{i:05}\", \"frequency\": {i}}}"));
    }
    entries.push(']');
    write_file(
        &dir,
        "manifest.json",
        r#"{"schema": 1, "assets": [{"kind": "dictionary", "path": "d.json"}]}"#,
    );
    write_file(
        &dir,
        "d.json",
        &format!(
            "{{\"schema\": 1, \"id\": \"many\", \"language\": \"english\", \"entries\": {entries}}}"
        ),
    );
    dir
}

#[test]
fn exactly_at_cap_imports() {
    let dir = dictionary_with("cap-exact", 1_000);
    let outcome = import_fixture_dir(&dir, &test_options()).expect("import at cap");
    assert_eq!(outcome.report.imported, 1);
    assert_eq!(outcome.report.skipped, 0);
    assert_eq!(outcome.assets.dictionaries["many"].entries.len(), 1_000);
}

#[test]
fn one_over_cap_is_limit_exceeded() {
    let dir = dictionary_with("cap-over", 1_001);
    let outcome = import_fixture_dir(&dir, &test_options()).expect("continues");
    assert_eq!(outcome.report.imported, 0);
    assert_eq!(outcome.report.skipped, 1);
    assert_eq!(
        outcome.report.unsupported[0].reason,
        SkipReason::LimitExceeded
    );

    let strict = ImportOptions {
        strict: true,
        ..test_options()
    };
    match import_fixture_dir(&dir, &strict) {
        Err(LangError::LimitExceeded) => {}
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

#[test]
fn zero_cap_rejects_any_entry() {
    let dir = dictionary_with("cap-zero", 1);
    let zero = ImportOptions {
        max_entries_per_asset: 0,
        ..test_options()
    };
    let outcome = import_fixture_dir(&dir, &zero).expect("continues");
    assert_eq!(outcome.report.skipped, 1);
    assert_eq!(
        outcome.report.unsupported[0].reason,
        SkipReason::LimitExceeded
    );
}

#[test]
fn small_custom_cap_scales() {
    let dir = dictionary_with("cap-small", 3);
    let tiny = ImportOptions {
        max_entries_per_asset: 2,
        ..test_options()
    };
    let outcome = import_fixture_dir(&dir, &tiny).expect("continues");
    assert_eq!(outcome.report.skipped, 1);
    assert_eq!(
        outcome.report.unsupported[0].reason,
        SkipReason::LimitExceeded
    );
}
