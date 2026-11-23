//! Duplicate handling across all four `allow_duplicates` x `strict`
//! combinations, for duplicate asset ids, mapping keys, encoding codes, and
//! dictionary words. The first occurrence in manifest/array order always wins.

mod common;

use okkhor_lang::{ImportOptions, SkipReason, error::LangError, import_fixture_dir};

use common::{temp_dir, test_options, write_file};

fn options(allow_duplicates: bool, strict: bool) -> ImportOptions {
    ImportOptions {
        allow_duplicates,
        strict,
        ..test_options()
    }
}

fn layout_asset(id: &str, label: &str, mappings: &str) -> String {
    format!(
        "{{\"schema\": 1, \"id\": \"{id}\", \"label\": \"{label}\", \"language\": \"bangla\", \
         \"mappings\": [{mappings}]}}"
    )
}

fn mapping(key: &str, output: &str) -> String {
    format!("{{\"key\": \"{key}\", \"output\": \"{output}\"}}")
}

/// Manifest dir with two layout files sharing one id; labels differ so the
/// winner is observable.
fn dup_id_dir(tag: &str) -> std::path::PathBuf {
    let dir = temp_dir(tag);
    write_file(
        &dir,
        "manifest.json",
        r#"{"schema": 1, "assets": [
            {"kind": "layout", "path": "first.json"},
            {"kind": "layout", "path": "second.json"}]}"#,
    );
    write_file(
        &dir,
        "first.json",
        &layout_asset("dup", "FIRST", &mapping("a", "আ")),
    );
    write_file(
        &dir,
        "second.json",
        &layout_asset("dup", "SECOND", &mapping("a", "আ")),
    );
    dir
}

#[test]
fn duplicate_asset_id_matrix() {
    // (false, false): second asset skipped, first retained.
    let dir = dup_id_dir("dup-id-ff");
    let outcome = import_fixture_dir(&dir, &options(false, false)).expect("continues");
    assert_eq!(outcome.report.imported, 1);
    assert_eq!(outcome.report.skipped, 1);
    assert_eq!(outcome.report.unsupported.len(), 1);
    assert_eq!(
        outcome.report.unsupported[0].reason,
        SkipReason::DuplicateId
    );
    assert_eq!(outcome.report.unsupported[0].name, "second.json");
    let layout = &outcome.assets.layouts[&okkhor_lang::LayoutId::from_code("dup").unwrap()];
    assert_eq!(layout.label, "FIRST");

    // (false, true): fatal, no partial outcome.
    let dir = dup_id_dir("dup-id-ft");
    match import_fixture_dir(&dir, &options(false, true)) {
        Err(LangError::DuplicateId) => {}
        other => panic!("expected DuplicateId, got {other:?}"),
    }

    // (true, false) and (true, true): silent drop, first retained.
    for (allow, strict) in [(true, false), (true, true)] {
        let dir = dup_id_dir(&format!("dup-id-t{strict}"));
        let outcome = import_fixture_dir(&dir, &options(allow, strict)).expect("continues");
        assert_eq!(outcome.report.imported, 1, "allow={allow} strict={strict}");
        assert_eq!(outcome.report.skipped, 0, "allow={allow} strict={strict}");
        assert!(outcome.report.unsupported.is_empty());
        let layout = &outcome.assets.layouts[&okkhor_lang::LayoutId::from_code("dup").unwrap()];
        assert_eq!(layout.label, "FIRST");
        let _ = allow;
    }
}

/// Manifest dir with one layout whose two mappings share a key; outputs
/// differ so the winner is observable.
fn dup_key_dir(tag: &str) -> std::path::PathBuf {
    let dir = temp_dir(tag);
    write_file(
        &dir,
        "manifest.json",
        r#"{"schema": 1, "assets": [{"kind": "layout", "path": "k.json"}]}"#,
    );
    let mappings = format!("{}, {}", mapping("a", "FIRST"), mapping("a", "SECOND"));
    write_file(&dir, "k.json", &layout_asset("keys", "k", &mappings));
    dir
}

#[test]
fn duplicate_mapping_key_matrix() {
    let id = okkhor_lang::LayoutId::from_code("keys").unwrap();

    // (false, false): whole asset skipped with DuplicateId.
    let dir = dup_key_dir("dup-key-ff");
    let outcome = import_fixture_dir(&dir, &options(false, false)).expect("continues");
    assert_eq!(outcome.report.imported, 0);
    assert_eq!(outcome.report.skipped, 1);
    assert_eq!(
        outcome.report.unsupported[0].reason,
        SkipReason::DuplicateId
    );
    assert!(!outcome.assets.layouts.contains_key(&id));

    // (false, true): fatal DuplicateId.
    let dir = dup_key_dir("dup-key-ft");
    match import_fixture_dir(&dir, &options(false, true)) {
        Err(LangError::DuplicateId) => {}
        other => panic!("expected DuplicateId, got {other:?}"),
    }

    // (true, *): first occurrence wins, silently.
    for strict in [false, true] {
        let dir = dup_key_dir(&format!("dup-key-t{strict}"));
        let outcome = import_fixture_dir(&dir, &options(true, strict)).expect("continues");
        assert_eq!(outcome.report.imported, 1);
        assert_eq!(outcome.report.skipped, 0);
        assert_eq!(outcome.assets.layouts[&id].lookup("a"), Some("FIRST"));
    }
}

fn dup_code_dir(tag: &str) -> std::path::PathBuf {
    let dir = temp_dir(tag);
    write_file(
        &dir,
        "manifest.json",
        r#"{"schema": 1, "assets": [{"kind": "encoding", "path": "e.json"}]}"#,
    );
    write_file(
        &dir,
        "e.json",
        r#"{"schema": 1, "id": "codes",
            "entries": [{"code": 65, "unicode": "FIRST"}, {"code": 65, "unicode": "SECOND"}]}"#,
    );
    dir
}

#[test]
fn duplicate_encoding_code_matrix() {
    let id = okkhor_lang::EncodingId::from_code("codes").unwrap();

    let dir = dup_code_dir("dup-code-ff");
    let outcome = import_fixture_dir(&dir, &options(false, false)).expect("continues");
    assert_eq!(outcome.report.skipped, 1);
    assert_eq!(
        outcome.report.unsupported[0].reason,
        SkipReason::DuplicateId
    );
    assert!(!outcome.assets.encodings.contains_key(&id));

    let dir = dup_code_dir("dup-code-ft");
    match import_fixture_dir(&dir, &options(false, true)) {
        Err(LangError::DuplicateId) => {}
        other => panic!("expected DuplicateId, got {other:?}"),
    }

    for strict in [false, true] {
        let dir = dup_code_dir(&format!("dup-code-t{strict}"));
        let outcome = import_fixture_dir(&dir, &options(true, strict)).expect("continues");
        assert_eq!(outcome.report.skipped, 0);
        assert_eq!(outcome.assets.encodings[&id].lookup(65), Some("FIRST"));
    }
}

fn dup_word_dir(tag: &str) -> std::path::PathBuf {
    let dir = temp_dir(tag);
    write_file(
        &dir,
        "manifest.json",
        r#"{"schema": 1, "assets": [{"kind": "dictionary", "path": "d.json"}]}"#,
    );
    write_file(
        &dir,
        "d.json",
        r#"{"schema": 1, "id": "words", "language": "bangla",
            "entries": [{"word": "ভালো", "frequency": 1},
                        {"word": "  ভালো  ", "frequency": 2}]}"#,
    );
    dir
}

#[test]
fn duplicate_dictionary_word_matrix() {
    // The two words differ only by surrounding whitespace, which the importer
    // normalizes away: they are duplicates with the first occurrence winning.

    let dir = dup_word_dir("dup-word-ff");
    let outcome = import_fixture_dir(&dir, &options(false, false)).expect("continues");
    assert_eq!(outcome.report.skipped, 1);
    assert_eq!(
        outcome.report.unsupported[0].reason,
        SkipReason::DuplicateId
    );
    assert!(!outcome.assets.dictionaries.contains_key("words"));

    let dir = dup_word_dir("dup-word-ft");
    match import_fixture_dir(&dir, &options(false, true)) {
        Err(LangError::DuplicateId) => {}
        other => panic!("expected DuplicateId, got {other:?}"),
    }

    for strict in [false, true] {
        let dir = dup_word_dir(&format!("dup-word-t{strict}"));
        let outcome = import_fixture_dir(&dir, &options(true, strict)).expect("continues");
        assert_eq!(outcome.report.skipped, 0);
        assert_eq!(
            outcome.assets.dictionaries["words"]
                .get("ভালো")
                .unwrap()
                .frequency,
            1
        );
    }
}

#[test]
fn invalid_id_is_reported_not_confused_with_duplicates() {
    let dir = temp_dir("invalid-id");
    write_file(
        &dir,
        "manifest.json",
        r#"{"schema": 1, "assets": [{"kind": "layout", "path": "bad.json"}]}"#,
    );
    write_file(
        &dir,
        "bad.json",
        r#"{"schema": 1, "id": "NOT VALID", "label": "x", "language": "bangla",
            "mappings": [{"key": "a", "output": "আ"}]}"#,
    );
    let outcome = import_fixture_dir(&dir, &options(false, false)).expect("continues");
    assert_eq!(outcome.report.skipped, 1);
    assert_eq!(outcome.report.unsupported[0].reason, SkipReason::InvalidId);
    match import_fixture_dir(&dir, &options(false, true)) {
        Err(LangError::InvalidData { reason }) => assert_eq!(reason, "invalid_layout_id"),
        other => panic!("expected InvalidData, got {other:?}"),
    }
}

#[test]
fn malformed_data_is_reported() {
    let dir = temp_dir("malformed");
    write_file(
        &dir,
        "manifest.json",
        r#"{"schema": 1, "assets": [{"kind": "encoding", "path": "e.json"}]}"#,
    );
    write_file(
        &dir,
        "e.json",
        r#"{"schema": 1, "id": "bad", "entries": [{"code": 1114112, "unicode": "X"}]}"#,
    );
    let outcome = import_fixture_dir(&dir, &options(false, false)).expect("continues");
    assert_eq!(outcome.report.skipped, 1);
    assert_eq!(
        outcome.report.unsupported[0].reason,
        SkipReason::MalformedData
    );
    match import_fixture_dir(&dir, &options(false, true)) {
        Err(LangError::InvalidData { reason }) => assert_eq!(reason, "code_out_of_range"),
        other => panic!("expected InvalidData, got {other:?}"),
    }
}
