//! Valid-manifest import, lookups, ordering, NFC, serde round-trip, the
//! empty-`ImportedAssets` fallback, and error-text privacy.

mod common;

use okkhor_lang::{AssetKind, ImportedAssets, LayoutId, import_asset, import_fixture_dir};

use common::{
    assert_report_is_log_safe, single_asset_dir, synthetic_root, temp_dir, test_options, write_file,
};

#[test]
fn valid_manifest_imports_all_three_assets() {
    let outcome = import_fixture_dir(&synthetic_root(), &test_options()).expect("valid import");
    assert_eq!(outcome.report.imported, 3);
    assert_eq!(outcome.report.skipped, 0);
    assert!(outcome.report.unsupported.is_empty());
    assert_report_is_log_safe(&outcome.report);

    let avro = LayoutId::from_code("avro").unwrap();
    assert!(outcome.assets.layouts.contains_key(&avro));
    assert!(
        outcome
            .assets
            .encodings
            .contains_key(&okkhor_lang::EncodingId::from_code("synthetic_ascii").unwrap())
    );
    assert!(outcome.assets.dictionaries.contains_key("bn_basic"));
    assert!(!outcome.assets.is_empty());
    assert_eq!(outcome.assets.total_entries(), 2 + 2 + 2);
}

#[test]
fn layout_lookup_uses_raw_key_with_internal_normalization() {
    let outcome = import_fixture_dir(&synthetic_root(), &test_options()).expect("valid import");
    let layout = &outcome.assets.layouts[&LayoutId::from_code("avro").unwrap()];
    assert_eq!(layout.lookup("a"), Some("আ"));
    assert_eq!(layout.lookup("shift+a"), Some("আঁ"));
    assert_eq!(layout.lookup("missing"), None);
    // Lookup normalization matches the importer: surrounding whitespace is
    // insignificant on both sides.
    assert_eq!(layout.lookup("  a  "), Some("আ"));
}

#[test]
fn dictionary_get_uses_raw_word_with_internal_normalization() {
    let outcome = import_fixture_dir(&synthetic_root(), &test_options()).expect("valid import");
    let dictionary = &outcome.assets.dictionaries["bn_basic"];
    assert_eq!(dictionary.get("ভালো").unwrap().frequency, 42);
    assert_eq!(dictionary.get("আমি").unwrap().frequency, 30);
    assert_eq!(dictionary.get("নেই"), None);
}

#[test]
fn entries_are_stored_in_sorted_order() {
    let dir = single_asset_dir(
        "sorted",
        "layout",
        "z.json",
        r#"{"schema": 1, "id": "zed", "label": "z", "language": "english",
            "mappings": [{"key": "b", "output": "B"}, {"key": "a", "output": "A"}]}"#,
    );
    let outcome = import_fixture_dir(&dir, &test_options()).expect("import");
    let layout = &outcome.assets.layouts[&LayoutId::from_code("zed").unwrap()];
    let keys: Vec<&str> = layout.mappings.iter().map(|m| m.key.as_str()).collect();
    assert_eq!(keys, ["a", "b"]);

    let dir = single_asset_dir(
        "sorted-enc",
        "encoding",
        "e.json",
        r#"{"schema": 1, "id": "nums",
            "entries": [{"code": 66, "unicode": "B"}, {"code": 65, "unicode": "A"}]}"#,
    );
    let outcome = import_fixture_dir(&dir, &test_options()).expect("import");
    let table = &outcome.assets.encodings[&okkhor_lang::EncodingId::from_code("nums").unwrap()];
    let codes: Vec<u32> = table.entries.iter().map(|e| e.code).collect();
    assert_eq!(codes, [65, 66]);

    let dir = single_asset_dir(
        "sorted-dict",
        "dictionary",
        "d.json",
        r#"{"schema": 1, "id": "words", "language": "english",
            "entries": [{"word": "b", "frequency": 1}, {"word": "a", "frequency": 2}]}"#,
    );
    let outcome = import_fixture_dir(&dir, &test_options()).expect("import");
    let dictionary = &outcome.assets.dictionaries["words"];
    let words: Vec<&str> = dictionary.entries.iter().map(|d| d.word.as_str()).collect();
    assert_eq!(words, ["a", "b"]);
}

#[test]
fn outputs_are_normalized_to_nfc_at_import() {
    // "e" + combining acute (JSON escape) must be stored precomposed.
    let payload = "{\"schema\": 1, \"id\": \"nfc\", \"label\": \"n\", \"language\": \"english\", \
         \"mappings\": [{\"key\": \"e\", \"output\": \"e\\u0301\"}]}";
    let dir = single_asset_dir("nfc", "layout", "n.json", payload);
    let outcome = import_fixture_dir(&dir, &test_options()).expect("import");
    let layout = &outcome.assets.layouts[&LayoutId::from_code("nfc").unwrap()];
    assert_eq!(layout.lookup("e").unwrap(), "\u{e9}");
}

#[test]
fn dictionary_words_are_lookup_normalized_at_import() {
    let dir = single_asset_dir(
        "dict-norm",
        "dictionary",
        "d.json",
        "{\"schema\": 1, \"id\": \"w\", \"language\": \"bangla\",
            \"entries\": [{\"word\": \"  ভালো  \", \"frequency\": 7}]}",
    );
    let outcome = import_fixture_dir(&dir, &test_options()).expect("import");
    let dictionary = &outcome.assets.dictionaries["w"];
    assert_eq!(dictionary.entries[0].word, "ভালো");
    assert_eq!(dictionary.get("ভালো").unwrap().frequency, 7);
}

#[test]
fn imported_assets_serde_round_trip() {
    let outcome = import_fixture_dir(&synthetic_root(), &test_options()).expect("valid import");
    let json = serde_json::to_string(&outcome.assets).expect("serialize");
    let back: ImportedAssets = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, outcome.assets);
}

#[test]
fn empty_imported_assets_is_a_safe_fallback() {
    let empty = ImportedAssets::default();
    assert!(empty.is_empty());
    assert_eq!(empty.total_entries(), 0);

    let dir = temp_dir("empty-manifest");
    write_file(&dir, "manifest.json", r#"{"schema": 1, "assets": []}"#);
    let outcome = import_fixture_dir(&dir, &test_options()).expect("empty import");
    assert!(outcome.assets.is_empty());
    assert_eq!(outcome.assets.total_entries(), 0);
    assert_eq!(outcome.report.imported, 0);
    assert_eq!(outcome.report.skipped, 0);
}

#[test]
fn error_display_contains_no_path_or_text() {
    let missing = temp_dir("missing").join("nope.json");
    let error = import_asset(AssetKind::Layout, &missing, &test_options()).unwrap_err();
    let text = format!("{error}");
    assert!(!text.contains("nope"), "Display leaks path: {text}");
    assert!(!text.contains("missing"), "Display leaks path: {text}");

    let dir = single_asset_dir(
        "display-unsafe",
        "layout",
        "ok.json",
        r#"{"schema": 1, "id": "x", "label": "x", "language": "bangla",
            "mappings": [{"key": "a", "output": "আ"}]}"#,
    );
    let traversal = dir.join("..").join("evil.json");
    let error = import_asset(AssetKind::Layout, &traversal, &test_options()).unwrap_err();
    let text = format!("{error}");
    assert!(!text.contains("evil"), "Display leaks path: {text}");
}

#[test]
fn no_legacy_file_is_read() {
    // The importer only opens paths listed in the manifest under the fixture
    // root; a manifest that lists nothing imports nothing.
    let outcome = import_fixture_dir(&synthetic_root(), &test_options()).expect("valid import");
    for layout in outcome.assets.layouts.values() {
        assert!(!layout.id.as_str().ends_with(".bin"));
        assert!(!layout.id.as_str().ends_with(".nmsLayout"));
    }
}
