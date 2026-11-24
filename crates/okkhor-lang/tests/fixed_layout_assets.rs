//! Golden and schema tests for the embedded fixed-layout assets.
//!
//! Bijoy note (ADR 0021, spec §4.2): `bijoy` is **not** a separate input asset.
//! It uses the National key map in `fixed-national-v1.json` with Okkhor's
//! existing Bijoy output encoding (`okkhor_lang::to_bijoy`). No input table is
//! invented and no reference install path is stored in an asset.

use okkhor_core::text::normalize_nfc;
use serde_json::Value;

const NATIONAL: &str = include_str!("../assets/layouts/fixed-national-v1.json");
const PROBHAT: &str = include_str!("../assets/layouts/fixed-probhat-v1.json");

fn parse(raw: &str) -> Value {
    serde_json::from_str(raw).expect("embedded fixed layout must be valid JSON")
}

/// Every non-null entry in the four modifier maps. A null slot is an
/// intentionally unmapped key/modifier slot, not an output.
fn outputs(value: &Value) -> Vec<&str> {
    ["normal", "shift", "altgr", "shift_altgr"]
        .into_iter()
        .flat_map(|tier| {
            value["keys"][tier]
                .as_object()
                .unwrap_or_else(|| panic!("missing keys.{tier} object"))
                .values()
                .flat_map(|v| match v {
                    Value::String(s) => vec![s.as_str()],
                    Value::Null => Vec::new(),
                    other => panic!("slot must be a string or null, got {other}"),
                })
        })
        .filter(|s| !s.is_empty())
        .collect()
}

fn assert_common_schema(value: &Value, expected_name: &str) {
    assert_eq!(value["kind"], "fixed");
    assert_eq!(value["name"], expected_name);
    assert!(value["version"].as_u64().is_some_and(|v| v >= 1));
    assert!(value["description"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(value["source"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(
        value["source_version"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert!(
        value["verification"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert_eq!(value["developer"], "Okkhor");
    // No reference-install path or temp working-data path may be shipped.
    let raw = value.to_string();
    assert!(
        !raw.contains("avro_portable"),
        "asset leaks a reference path"
    );
    assert!(!raw.contains("Nms_Kontho"), "asset leaks a working path");
    assert!(!raw.contains("AppData"), "asset leaks a working path");

    for slot in outputs(value) {
        assert_eq!(normalize_nfc(slot), slot, "output is not NFC: {slot:?}");
        assert!(!slot.contains('\u{0000}'), "output contains NUL");
    }
}

#[test]
fn national_asset_has_valid_fixed_schema_and_golden_keys() {
    let value = parse(NATIONAL);
    assert_common_schema(&value, "national-v1-fixed");
    assert_eq!(value["rule_count"].as_u64(), Some(203));

    let normal = &value["keys"]["normal"];
    assert_eq!(normal["Key_N_Normal"], "স");
    assert_eq!(normal["Key_H_Normal"], "া");
    assert_eq!(normal["Key_A_Normal"], "ৃ");
    assert_eq!(normal["Key_G_Normal"], "্");
    assert_eq!(normal["Key_1_Normal"], "১");
    assert_eq!(normal["Num1"], "১");
    assert_eq!(normal["Key_OEM3_Normal"], "\u{200c}");
    assert_eq!(normal["Key_B_Normal"], "ন");

    assert_eq!(value["keys"]["shift"]["Key_N_Shift"], "ষ");
    assert_eq!(value["keys"]["altgr"]["Key_S_AltGr"], "উ");
    assert_eq!(value["keys"]["shift_altgr"]["Key_A_ShiftAltGr"], "ৠ");
    // XML entities in the working data are decoded into real characters.
    assert_eq!(value["keys"]["shift"]["Key_7_Shift"], "&");
    assert_eq!(value["keys"]["shift"]["Key_COMMA_Shift"], "<");
    assert_eq!(value["keys"]["shift"]["Key_PERIOD_Shift"], ">");
    assert_eq!(value["keys"]["normal"]["Key_OEM7_Normal"], "'");
    assert_eq!(value["keys"]["shift"]["Key_OEM7_Shift"], "\"");

    // Unmapped slots are represented as null, never as empty strings.
    assert!(value["keys"]["altgr"]["Key_N_AltGr"].is_null());

    // The 203 slots account for every source key/modifier position.
    let slots: usize = ["normal", "shift", "altgr", "shift_altgr"]
        .iter()
        .map(|tier| value["keys"][tier].as_object().unwrap().len())
        .sum();
    assert_eq!(slots, 203);
}

#[test]
fn national_asset_carries_the_documented_compound_sequence() {
    let value = parse(NATIONAL);
    let sequences = value["sequences"].as_array().expect("sequences array");
    let compound = sequences
        .iter()
        .find(|entry| entry["keys"] == serde_json::json!(["Key_G", "Key_S"]))
        .expect("documented g,s compound rule");
    assert_eq!(compound["output"], "উ");
    assert_eq!(normalize_nfc(compound["output"].as_str().unwrap()), "উ");
    assert!(sequences.len() <= 64, "sequence table must stay bounded");
}

#[test]
fn national_asset_documents_the_bijoy_alias() {
    let description = parse(NATIONAL)["description"].as_str().unwrap().to_string();
    assert!(
        description.contains("Bijoy"),
        "National asset must document that Bijoy reuses this key map"
    );
}

#[test]
fn probhat_asset_has_valid_fixed_schema_and_golden_keys() {
    let value = parse(PROBHAT);
    assert_common_schema(&value, "probhat-v1-fixed");
    assert_eq!(value["rule_count"].as_u64(), Some(203));

    let normal = &value["keys"]["normal"];
    assert_eq!(normal["Key_N_Normal"], "ন");
    assert_eq!(normal["Key_H_Normal"], "হ");
    assert_eq!(normal["Key_A_Normal"], "া");
    assert_eq!(normal["Key_1_Normal"], "১");
    assert_eq!(normal["Num1"], "১");
    assert_eq!(normal["Key_OEM2_Normal"], "্");

    assert_eq!(value["keys"]["shift"]["Key_N_Shift"], "ণ");
    assert_eq!(value["keys"]["shift"]["Key_U_Shift"], "উ");
    assert_eq!(value["keys"]["normal"]["Key_PERIOD_Normal"], "।");

    let slots: usize = ["normal", "shift", "altgr", "shift_altgr"]
        .iter()
        .map(|tier| value["keys"][tier].as_object().unwrap().len())
        .sum();
    assert_eq!(slots, 203);
}

#[test]
fn probhat_has_no_compound_sequences() {
    let value = parse(PROBHAT);
    assert_eq!(value["sequences"].as_array().unwrap().len(), 0);
}

#[test]
fn no_separate_bijoy_input_asset_is_shipped() {
    // Bijoy is National keys + the existing Bijoy output encoding. Asserting
    // the absence of a third input asset keeps that decision from drifting.
    let layouts = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/layouts");
    for entry in std::fs::read_dir(layouts).expect("layouts directory") {
        let name = entry.expect("directory entry").file_name();
        let name = name.to_string_lossy();
        assert!(
            !name.contains("bijoy"),
            "Bijoy must not get a separate input asset: {name}"
        );
    }
}
