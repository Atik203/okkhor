//! Every committed fixture parses as JSON and deserializes through
//! `FixtureEnvelope` with `schema == FIXTURE_SCHEMA`; schema bumps (including
//! a missing `schema` field) are rejected at both manifest and asset level.

mod common;

use okkhor_lang::{
    AssetKind, Dictionary, EncodingTable, FixtureEnvelope, ImportOptions, Layout, Manifest,
    assets::FIXTURE_SCHEMA, error::LangError, import_asset, import_fixture_dir,
};
use serde_json::Value;

use common::{single_asset_dir, synthetic_copy, synthetic_root, test_options};

const FIXTURE_FILES: [&str; 4] = [
    "manifest.json",
    "layouts/avro.json",
    "encodings/legacy_ascii.json",
    "dictionaries/bn_basic.json",
];

#[test]
fn every_committed_fixture_parses_as_json() {
    let root = synthetic_root();
    for file in FIXTURE_FILES {
        let text = std::fs::read_to_string(root.join(file)).expect("read fixture");
        let value: Value = serde_json::from_str(&text).expect("fixture parses as JSON");
        assert!(value.is_object(), "{file} must be a JSON object");
    }
}

#[test]
fn every_committed_fixture_has_schema_one_envelope() {
    let root = synthetic_root();
    for file in FIXTURE_FILES {
        let text = std::fs::read_to_string(root.join(file)).expect("read fixture");
        let envelope: FixtureEnvelope<Value> =
            serde_json::from_str(&text).expect("fixture has an envelope");
        assert_eq!(envelope.schema, FIXTURE_SCHEMA, "{file}");
    }
}

#[test]
fn manifest_payload_matches_manifest_type() {
    let text =
        std::fs::read_to_string(synthetic_root().join("manifest.json")).expect("read manifest");
    let envelope: FixtureEnvelope<Manifest> =
        serde_json::from_str(&text).expect("manifest envelope");
    assert_eq!(envelope.schema, FIXTURE_SCHEMA);
    assert_eq!(envelope.payload.assets.len(), 3);
    assert_eq!(envelope.payload.assets[0].path, "layouts/avro.json");
}

#[test]
fn asset_payloads_match_typed_forms() {
    let root = synthetic_root();
    let layout_text = std::fs::read_to_string(root.join("layouts/avro.json")).expect("read layout");
    let layout_envelope: FixtureEnvelope<Layout> =
        serde_json::from_str(&layout_text).expect("layout envelope");
    assert_eq!(layout_envelope.payload.id.as_str(), "avro");

    let encoding_text =
        std::fs::read_to_string(root.join("encodings/legacy_ascii.json")).expect("read table");
    let encoding_envelope: FixtureEnvelope<EncodingTable> =
        serde_json::from_str(&encoding_text).expect("encoding envelope");
    assert_eq!(encoding_envelope.payload.id.as_str(), "synthetic_ascii");

    let dictionary_text =
        std::fs::read_to_string(root.join("dictionaries/bn_basic.json")).expect("read dict");
    let dictionary_envelope: FixtureEnvelope<Dictionary> =
        serde_json::from_str(&dictionary_text).expect("dictionary envelope");
    assert_eq!(dictionary_envelope.payload.id, "bn_basic");
}

#[test]
fn fixture_language_strings_use_bangla_not_codes() {
    let root = synthetic_root();
    for file in ["layouts/avro.json", "dictionaries/bn_basic.json"] {
        let text = std::fs::read_to_string(root.join(file)).expect("read fixture");
        assert!(
            text.contains("\"bangla\""),
            "{file} must use \"bangla\", not a code"
        );
        assert!(!text.contains("\"bn\""), "{file} must not use \"bn\"");
    }
}

#[test]
fn fixture_encoding_codes_are_decimal() {
    let text = std::fs::read_to_string(synthetic_root().join("encodings/legacy_ascii.json"))
        .expect("read table");
    assert!(text.contains("65"), "decimal code point present");
    assert!(!text.contains("0x"), "no hexadecimal literals");
}

#[test]
fn manifest_schema_bump_is_rejected() {
    let dir = synthetic_copy("manifest-bump");
    let manifest = std::fs::read_to_string(dir.join("manifest.json")).expect("read manifest");
    std::fs::write(
        dir.join("manifest.json"),
        manifest.replace("\"schema\":1", "\"schema\": 2"),
    )
    .expect("rewrite manifest");
    match import_fixture_dir(&dir, &test_options()) {
        Err(LangError::UnsupportedSchema { found, expected }) => {
            assert_eq!(found, 2);
            assert_eq!(expected, FIXTURE_SCHEMA);
        }
        other => panic!("expected UnsupportedSchema, got {other:?}"),
    }
}

#[test]
fn manifest_missing_schema_reports_found_zero() {
    let dir = synthetic_copy("manifest-noschema");
    let manifest = std::fs::read_to_string(dir.join("manifest.json")).expect("read manifest");
    std::fs::write(
        dir.join("manifest.json"),
        manifest.replace("\"schema\":1,", ""),
    )
    .expect("rewrite manifest");
    match import_fixture_dir(&dir, &test_options()) {
        Err(LangError::UnsupportedSchema { found, expected }) => {
            assert_eq!(found, 0);
            assert_eq!(expected, FIXTURE_SCHEMA);
        }
        other => panic!("expected UnsupportedSchema(found = 0), got {other:?}"),
    }
}

#[test]
fn asset_schema_bump_is_skipped_non_strict() {
    let dir = synthetic_copy("asset-bump");
    let layout = std::fs::read_to_string(dir.join("layouts/avro.json")).expect("read layout");
    std::fs::write(
        dir.join("layouts/avro.json"),
        layout.replace("\"schema\":1", "\"schema\": 2"),
    )
    .expect("rewrite layout");
    let outcome = import_fixture_dir(&dir, &test_options()).expect("import continues");
    assert_eq!(outcome.report.imported, 2);
    assert_eq!(outcome.report.skipped, 1);
    let skip = &outcome.report.unsupported[0];
    assert_eq!(skip.reason, okkhor_lang::SkipReason::UnsupportedSchema);
    assert!(
        !outcome
            .assets
            .layouts
            .contains_key(&okkhor_lang::LayoutId::from_code("avro").unwrap())
    );
    // Strict escalates with the envelope's `found` value.
    let strict = ImportOptions {
        strict: true,
        ..test_options()
    };
    match import_fixture_dir(&dir, &strict) {
        Err(LangError::UnsupportedSchema { found, expected }) => {
            assert_eq!(found, 2);
            assert_eq!(expected, FIXTURE_SCHEMA);
        }
        other => panic!("expected UnsupportedSchema, got {other:?}"),
    }
}

#[test]
fn asset_missing_schema_reports_found_zero() {
    let dir = single_asset_dir(
        "asset-noschema",
        "layout",
        "avro.json",
        r#"{"id": "avro", "label": "x", "language": "bangla",
            "mappings": [{"key": "a", "output": "আ"}]}"#,
    );
    let outcome = import_fixture_dir(&dir, &test_options()).expect("import continues");
    assert_eq!(outcome.report.skipped, 1);
    assert_eq!(
        outcome.report.unsupported[0].reason,
        okkhor_lang::SkipReason::UnsupportedSchema
    );
    let path = dir.join("avro.json");
    match import_asset(AssetKind::Layout, &path, &test_options()) {
        Err(LangError::UnsupportedSchema { found, .. }) => assert_eq!(found, 0),
        other => panic!("expected UnsupportedSchema(found = 0), got {other:?}"),
    }
}
