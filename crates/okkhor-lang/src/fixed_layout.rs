//! Physical-key fixed layouts, parsed once at startup into bounded in-memory
//! tables. Nothing here reads a file, allocates on the typing path, or depends
//! on the daemon.
//!
//! Asset slots are named after physical keys plus a modifier suffix
//! (`Key_A_Normal`, `Key_N_ShiftAltGr`, `Num1`). A null slot is an unmapped
//! key/modifier pair. At parse time the suffix is removed and each output is
//! stored under its bare physical-key name, so [`FixedLayout::resolve`] is a
//! fixed number of `&str` map lookups with no allocation.

use okkhor_core::text::normalize_nfc;

/// Maximum number of keys in one ordered sequence rule.
pub const MAX_SEQUENCE_KEYS: usize = 3;
/// Upper bound on the number of sequence rules in a layout asset.
pub const MAX_SEQUENCES: usize = 64;

/// Static logging reasons for fixed-layout failures. No reason contains user
/// text, a file path, or parsed asset content.
pub const ERROR_REASONS: &[&str] = &[
    "invalid_json",
    "unsupported_kind",
    "invalid_version",
    "rule_count_mismatch",
    "invalid_modifier_name",
    "invalid_key_name",
    "duplicate_key",
    "empty_output",
    "not_nfc",
    "invalid_sequence",
    "sequence_too_long",
    "too_many_sequences",
];

/// Modifier bits carried by a physical key event.
///
/// This is a plain boolean pair so the daemon can convert its own modifier
/// flags at the call site without sharing a type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModifierSet {
    pub shift: bool,
    pub altgr: bool,
}

/// Typed, static reason a fixed-layout asset was rejected.
///
/// Every variant renders to an allowlisted entry in [`ERROR_REASONS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixedLayoutError {
    InvalidJson,
    UnsupportedKind,
    InvalidVersion,
    RuleCountMismatch,
    InvalidModifierName,
    InvalidKeyName,
    DuplicateKey,
    EmptyOutput,
    NotNfc,
    InvalidSequence,
    SequenceTooLong,
    TooManySequences,
}

impl FixedLayoutError {
    /// Allowlisted machine-readable reason, safe for the daemon logger.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::InvalidJson => "invalid_json",
            Self::UnsupportedKind => "unsupported_kind",
            Self::InvalidVersion => "invalid_version",
            Self::RuleCountMismatch => "rule_count_mismatch",
            Self::InvalidModifierName => "invalid_modifier_name",
            Self::InvalidKeyName => "invalid_key_name",
            Self::DuplicateKey => "duplicate_key",
            Self::EmptyOutput => "empty_output",
            Self::NotNfc => "not_nfc",
            Self::InvalidSequence => "invalid_sequence",
            Self::SequenceTooLong => "sequence_too_long",
            Self::TooManySequences => "too_many_sequences",
        }
    }
}

impl std::fmt::Display for FixedLayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for FixedLayoutError {}

/// An embedded, validated fixed layout. Built once at startup; all lookups
/// are bounded, allocation-free reads.
#[derive(Debug)]
pub struct FixedLayout {
    name: String,
    /// Indexed by [`Tier`]; each map is keyed by bare physical-key name.
    keys: [std::collections::HashMap<String, String>; 4],
    sequences: Vec<(Vec<String>, String)>,
}

/// Every physical-key name that may appear in a fixed-layout asset.
const VALID_BASE_KEYS: &[&str] = &[
    "Key_OEM3",
    "Key_1",
    "Key_2",
    "Key_3",
    "Key_4",
    "Key_5",
    "Key_6",
    "Key_7",
    "Key_8",
    "Key_9",
    "Key_0",
    "Key_MINUS",
    "Key_PLUS",
    "Key_Q",
    "Key_W",
    "Key_E",
    "Key_R",
    "Key_T",
    "Key_Y",
    "Key_U",
    "Key_I",
    "Key_O",
    "Key_P",
    "Key_OEM4",
    "Key_OEM6",
    "Key_OEM5",
    "Key_A",
    "Key_S",
    "Key_D",
    "Key_F",
    "Key_G",
    "Key_H",
    "Key_J",
    "Key_K",
    "Key_L",
    "Key_OEM1",
    "Key_OEM7",
    "Key_Z",
    "Key_X",
    "Key_C",
    "Key_V",
    "Key_B",
    "Key_N",
    "Key_M",
    "Key_COMMA",
    "Key_PERIOD",
    "Key_OEM2",
    "Num1",
    "Num2",
    "Num3",
    "Num4",
    "Num5",
    "Num6",
    "Num7",
    "Num8",
    "Num9",
    "Num0",
    "NumAdd",
    "NumDecimal",
    "NumDivide",
    "NumMultiply",
    "NumSubtract",
];

const LETTER_NAMES: [&str; 26] = [
    "Key_A", "Key_B", "Key_C", "Key_D", "Key_E", "Key_F", "Key_G", "Key_H", "Key_I", "Key_J",
    "Key_K", "Key_L", "Key_M", "Key_N", "Key_O", "Key_P", "Key_Q", "Key_R", "Key_S", "Key_T",
    "Key_U", "Key_V", "Key_W", "Key_X", "Key_Y", "Key_Z",
];
const DIGIT_NAMES: [&str; 10] = [
    "Key_0", "Key_1", "Key_2", "Key_3", "Key_4", "Key_5", "Key_6", "Key_7", "Key_8", "Key_9",
];
const NUMPAD_NAMES: [&str; 10] = [
    "Num0", "Num1", "Num2", "Num3", "Num4", "Num5", "Num6", "Num7", "Num8", "Num9",
];

#[derive(Debug, Clone, Copy)]
enum Tier {
    ShiftAltGr,
    AltGr,
    Shift,
    Normal,
}

impl Tier {
    /// Highest priority first: suffix stripping must prefer the longest name.
    const ALL: [Self; 4] = [Self::ShiftAltGr, Self::AltGr, Self::Shift, Self::Normal];

    const fn index(self) -> usize {
        match self {
            Self::ShiftAltGr => 0,
            Self::AltGr => 1,
            Self::Shift => 2,
            Self::Normal => 3,
        }
    }

    const fn suffix(self) -> &'static str {
        match self {
            Self::ShiftAltGr => "_ShiftAltGr",
            Self::AltGr => "_AltGr",
            Self::Shift => "_Shift",
            Self::Normal => "_Normal",
        }
    }

    const fn field(self) -> &'static str {
        match self {
            Self::ShiftAltGr => "shift_altgr",
            Self::AltGr => "altgr",
            Self::Shift => "shift",
            Self::Normal => "normal",
        }
    }
}

fn strip_tier_suffix(slot: &str) -> Option<(Tier, &str)> {
    for tier in Tier::ALL {
        if let Some(base) = slot.strip_suffix(tier.suffix()) {
            return Some((tier, base));
        }
    }
    None
}

fn valid_output(value: &str) -> Result<(), FixedLayoutError> {
    if value.is_empty() {
        return Err(FixedLayoutError::EmptyOutput);
    }
    if normalize_nfc(value).as_str() != value {
        return Err(FixedLayoutError::NotNfc);
    }
    Ok(())
}

/// Parse and validate an embedded fixed-layout asset.
///
/// Validation is bounded and startup-only: it checks the declared slot count,
/// that every modifier tier is known, that every key name is a physical key,
/// and that every non-null output is non-empty NFC. No file is read and no
/// reason string contains parsed content.
pub fn from_asset_json(json: &str) -> Result<FixedLayout, FixedLayoutError> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|_| FixedLayoutError::InvalidJson)?;

    if value["kind"].as_str() != Some("fixed") {
        return Err(FixedLayoutError::UnsupportedKind);
    }
    if !matches!(value["version"].as_u64(), Some(1..)) {
        return Err(FixedLayoutError::InvalidVersion);
    }
    let declared = value["rule_count"]
        .as_u64()
        .ok_or(FixedLayoutError::RuleCountMismatch)? as usize;

    // Reject unknown modifier tiers, then require all four known tiers.
    let keys = value["keys"]
        .as_object()
        .ok_or(FixedLayoutError::InvalidModifierName)?;
    for name in keys.keys() {
        if !Tier::ALL.iter().any(|tier| tier.field() == name) {
            return Err(FixedLayoutError::InvalidModifierName);
        }
    }

    let mut tables = std::array::from_fn(|_| std::collections::HashMap::new());
    let mut slots = 0usize;
    for tier in Tier::ALL {
        let entries = keys
            .get(tier.field())
            .and_then(serde_json::Value::as_object)
            .ok_or(FixedLayoutError::InvalidModifierName)?;
        for (slot, output) in entries {
            slots += 1;
            let base = match strip_tier_suffix(slot) {
                Some((owner, base)) if owner.index() == tier.index() => base,
                Some(_) => return Err(FixedLayoutError::InvalidKeyName),
                // Numpad and other single-tier slots are stored without a suffix.
                None if tier.index() == Tier::Normal.index() => slot.as_str(),
                None => return Err(FixedLayoutError::InvalidKeyName),
            };
            if !VALID_BASE_KEYS.contains(&base) {
                return Err(FixedLayoutError::InvalidKeyName);
            }
            if let serde_json::Value::String(text) = output {
                valid_output(text)?;
                if tables[tier.index()]
                    .insert(base.to_owned(), text.clone())
                    .is_some()
                {
                    return Err(FixedLayoutError::DuplicateKey);
                }
            } else if !output.is_null() {
                return Err(FixedLayoutError::InvalidKeyName);
            }
        }
    }
    if slots != declared {
        return Err(FixedLayoutError::RuleCountMismatch);
    }

    let raw_sequences = value["sequences"]
        .as_array()
        .ok_or(FixedLayoutError::InvalidSequence)?;
    if raw_sequences.len() > MAX_SEQUENCES {
        return Err(FixedLayoutError::TooManySequences);
    }
    let mut sequences = Vec::with_capacity(raw_sequences.len());
    for entry in raw_sequences {
        let names = entry["keys"]
            .as_array()
            .ok_or(FixedLayoutError::InvalidSequence)?;
        if names.len() > MAX_SEQUENCE_KEYS {
            return Err(FixedLayoutError::SequenceTooLong);
        }
        if names.is_empty() {
            return Err(FixedLayoutError::InvalidSequence);
        }
        let mut keys = Vec::with_capacity(names.len());
        for name in names {
            let name = name.as_str().ok_or(FixedLayoutError::InvalidSequence)?;
            if !VALID_BASE_KEYS.contains(&name) {
                return Err(FixedLayoutError::InvalidKeyName);
            }
            keys.push(name.to_owned());
        }
        let output = entry["output"]
            .as_str()
            .ok_or(FixedLayoutError::InvalidSequence)?;
        valid_output(output)?;
        sequences.push((keys, output.to_owned()));
    }

    let name = value["name"].as_str().unwrap_or("fixed").to_owned();
    Ok(FixedLayout {
        name,
        keys: tables,
        sequences,
    })
}

/// Map a Win32 virtual key to its physical-key name.
///
/// Space, Enter, and the numpad separator are deliberately absent: the daemon
/// never feeds them into the fixed layout.
pub fn vk_to_key_name(vk: u16) -> Option<&'static str> {
    match vk {
        0x30..=0x39 => Some(DIGIT_NAMES[(vk - 0x30) as usize]),
        0x41..=0x5A => Some(LETTER_NAMES[(vk - 0x41) as usize]),
        0x60..=0x69 => Some(NUMPAD_NAMES[(vk - 0x60) as usize]),
        0x6A => Some("NumMultiply"),
        0x6B => Some("NumAdd"),
        0x6D => Some("NumSubtract"),
        0x6E => Some("NumDecimal"),
        0x6F => Some("NumDivide"),
        0xC0 => Some("Key_OEM3"),
        0xBA => Some("Key_OEM1"),
        0xBB => Some("Key_PLUS"),
        0xBC => Some("Key_COMMA"),
        0xBD => Some("Key_MINUS"),
        0xBE => Some("Key_PERIOD"),
        0xBF => Some("Key_OEM2"),
        0xDB => Some("Key_OEM4"),
        0xDC => Some("Key_OEM5"),
        0xDD => Some("Key_OEM6"),
        0xDE => Some("Key_OEM7"),
        _ => None,
    }
}

impl FixedLayout {
    /// Human-readable asset name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Resolve one key press, trying modifier tiers in strict priority order:
    /// ShiftAltGr, AltGr, Shift, Normal. Only tiers reachable from the held
    /// modifiers are considered, and a null/unmapped slot falls through.
    pub fn resolve(&self, key: &str, modifiers: ModifierSet) -> Option<&str> {
        let base = strip_tier_suffix(key).map_or(key, |(_, base)| base);
        if !VALID_BASE_KEYS.contains(&base) {
            return None;
        }
        let tiers: &[Tier] = match (modifiers.shift, modifiers.altgr) {
            (true, true) => &[Tier::ShiftAltGr, Tier::AltGr, Tier::Shift, Tier::Normal],
            (false, true) => &[Tier::AltGr, Tier::Shift, Tier::Normal],
            (true, false) => &[Tier::Shift, Tier::Normal],
            (false, false) => &[Tier::Normal],
        };
        for tier in tiers {
            if let Some(output) = self.keys[tier.index()].get(base) {
                return Some(output.as_str());
            }
        }
        None
    }

    /// Match the longest ordered sequence rule: three keys first, then two.
    /// A sequence buffer longer than three keys is rejected outright.
    pub fn resolve_sequence(&self, keys: &[&str]) -> Option<&str> {
        if keys.is_empty() || keys.len() > MAX_SEQUENCE_KEYS {
            return None;
        }
        for len in (2..=keys.len()).rev() {
            let prefix = &keys[..len];
            for (rule, output) in &self.sequences {
                if rule.len() == len && rule.iter().map(String::as_str).eq(prefix.iter().copied()) {
                    return Some(output.as_str());
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const FOUR_TIERS: &str = r#"{
        "kind": "fixed", "version": 1, "rule_count": 4,
        "keys": {
            "normal": {"Key_A_Normal": "N"},
            "shift": {"Key_A_Shift": "S"},
            "altgr": {"Key_A_AltGr": "A"},
            "shift_altgr": {"Key_A_ShiftAltGr": "SA"}
        },
        "sequences": []
    }"#;

    const SHIFT_AND_NORMAL: &str = r#"{
        "kind": "fixed", "version": 1, "rule_count": 2,
        "keys": {
            "normal": {"Key_A_Normal": "N"},
            "shift": {"Key_A_Shift": "S"},
            "altgr": {}, "shift_altgr": {}
        },
        "sequences": []
    }"#;

    const NATIONAL: &str = include_str!("../assets/layouts/fixed-national-v1.json");
    const PROBHAT: &str = include_str!("../assets/layouts/fixed-probhat-v1.json");

    const NONE: ModifierSet = ModifierSet {
        shift: false,
        altgr: false,
    };
    const SHIFT: ModifierSet = ModifierSet {
        shift: true,
        altgr: false,
    };
    const ALTGR: ModifierSet = ModifierSet {
        shift: false,
        altgr: true,
    };
    const SHIFT_ALTGR: ModifierSet = ModifierSet {
        shift: true,
        altgr: true,
    };

    #[test]
    fn vk_to_key_name_maps_letters_digits_numpad_and_oem() {
        assert_eq!(vk_to_key_name(0x41), Some("Key_A"));
        assert_eq!(vk_to_key_name(0x4E), Some("Key_N"));
        assert_eq!(vk_to_key_name(0x5A), Some("Key_Z"));
        assert_eq!(vk_to_key_name(0x30), Some("Key_0"));
        assert_eq!(vk_to_key_name(0x31), Some("Key_1"));
        assert_eq!(vk_to_key_name(0x39), Some("Key_9"));
        assert_eq!(vk_to_key_name(0x60), Some("Num0"));
        assert_eq!(vk_to_key_name(0x69), Some("Num9"));

        assert_eq!(vk_to_key_name(0xC0), Some("Key_OEM3"));
        assert_eq!(vk_to_key_name(0xBA), Some("Key_OEM1"));
        assert_eq!(vk_to_key_name(0xBB), Some("Key_PLUS"));
        assert_eq!(vk_to_key_name(0xBC), Some("Key_COMMA"));
        assert_eq!(vk_to_key_name(0xBD), Some("Key_MINUS"));
        assert_eq!(vk_to_key_name(0xBE), Some("Key_PERIOD"));
        assert_eq!(vk_to_key_name(0xBF), Some("Key_OEM2"));
        assert_eq!(vk_to_key_name(0xDB), Some("Key_OEM4"));
        assert_eq!(vk_to_key_name(0xDC), Some("Key_OEM5"));
        assert_eq!(vk_to_key_name(0xDD), Some("Key_OEM6"));
        assert_eq!(vk_to_key_name(0xDE), Some("Key_OEM7"));
        assert_eq!(vk_to_key_name(0x6A), Some("NumMultiply"));
        assert_eq!(vk_to_key_name(0x6B), Some("NumAdd"));
        assert_eq!(vk_to_key_name(0x6D), Some("NumSubtract"));
        assert_eq!(vk_to_key_name(0x6E), Some("NumDecimal"));
        assert_eq!(vk_to_key_name(0x6F), Some("NumDivide"));

        // Space, Enter and the numpad separator are never fixed-layout keys.
        assert_eq!(vk_to_key_name(0x20), None);
        assert_eq!(vk_to_key_name(0x0D), None);
        assert_eq!(vk_to_key_name(0x6C), None);
        assert_eq!(vk_to_key_name(0x0100), None);
    }

    #[test]
    fn modifier_priority_is_strict() {
        let layout = from_asset_json(FOUR_TIERS).expect("valid asset");
        assert_eq!(layout.resolve("Key_A", NONE), Some("N"));
        assert_eq!(layout.resolve("Key_A", SHIFT), Some("S"));
        assert_eq!(layout.resolve("Key_A", ALTGR), Some("A"));
        assert_eq!(layout.resolve("Key_A", SHIFT_ALTGR), Some("SA"));
    }

    #[test]
    fn altgr_without_entry_falls_back_through_shift_then_normal() {
        let layout = from_asset_json(SHIFT_AND_NORMAL).expect("valid asset");
        assert_eq!(layout.resolve("Key_A", ALTGR), Some("S"));
        assert_eq!(layout.resolve("Key_A", SHIFT_ALTGR), Some("S"));
        assert_eq!(layout.resolve("Key_A", NONE), Some("N"));
    }

    #[test]
    fn unmapped_key_and_unmapped_normal_return_none() {
        let layout = from_asset_json(SHIFT_AND_NORMAL).expect("valid asset");
        assert_eq!(layout.resolve("Key_Z", NONE), None);
        assert_eq!(layout.resolve("Key_Z", SHIFT_ALTGR), None);
        assert_eq!(layout.resolve("Not_A_Key", NONE), None);
        // A bare &str lookup never allocates and never fabricates a character.
        let without_normal =
            SHIFT_AND_NORMAL.replace("\"Key_A_Normal\": \"N\"", "\"Key_A_Normal\": null");
        let layout = from_asset_json(&without_normal).expect("valid asset");
        assert_eq!(layout.resolve("Key_A", NONE), None);
        assert_eq!(layout.resolve("Key_A", SHIFT), Some("S"));
    }

    #[test]
    fn resolve_accepts_suffixed_key_names() {
        let layout = from_asset_json(FOUR_TIERS).expect("valid asset");
        assert_eq!(layout.resolve("Key_A_Normal", NONE), Some("N"));
        assert_eq!(layout.resolve("Key_A_ShiftAltGr", SHIFT_ALTGR), Some("SA"));
    }

    #[test]
    fn longest_sequence_wins_and_three_keys_is_the_cap() {
        let raw = r#"{
            "kind": "fixed", "version": 1, "rule_count": 0,
            "keys": {"normal": {}, "shift": {}, "altgr": {}, "shift_altgr": {}},
            "sequences": [
                {"keys": ["Key_G", "Key_S"], "output": "TWO"},
                {"keys": ["Key_G", "Key_S", "Key_D"], "output": "THREE"}
            ]
        }"#;
        let layout = from_asset_json(raw).expect("valid asset");
        assert_eq!(
            layout.resolve_sequence(&["Key_G", "Key_S", "Key_D"]),
            Some("THREE")
        );
        assert_eq!(layout.resolve_sequence(&["Key_G", "Key_S"]), Some("TWO"));
        assert_eq!(
            layout.resolve_sequence(&["Key_G", "Key_S", "Key_X"]),
            Some("TWO")
        );
        assert_eq!(layout.resolve_sequence(&["Key_G"]), None);
        assert_eq!(
            layout.resolve_sequence(&["Key_G", "Key_S", "Key_D", "Key_A"]),
            None
        );
        assert_eq!(layout.resolve_sequence(&[]), None);
    }

    #[test]
    fn parser_rejects_malformed_assets_with_allowlisted_reasons() {
        assert_eq!(from_asset_json("{").unwrap_err().reason(), "invalid_json");

        let wrong_kind = FOUR_TIERS.replace("\"kind\": \"fixed\"", "\"kind\": \"phonetic\"");
        assert_eq!(
            from_asset_json(&wrong_kind).unwrap_err().reason(),
            "unsupported_kind"
        );

        let bad_count = FOUR_TIERS.replace("\"rule_count\": 4", "\"rule_count\": 5");
        assert_eq!(
            from_asset_json(&bad_count).unwrap_err().reason(),
            "rule_count_mismatch"
        );

        let bad_tier = FOUR_TIERS.replace(
            "\"altgr\": {\"Key_A_AltGr\": \"A\"}",
            "\"ctrl\": {\"Key_A_AltGr\": \"A\"}",
        );
        assert_eq!(
            from_asset_json(&bad_tier).unwrap_err().reason(),
            "invalid_modifier_name"
        );

        let bad_key = FOUR_TIERS.replace("Key_A_Normal", "Key_NOT_A_KEY_Normal");
        assert_eq!(
            from_asset_json(&bad_key).unwrap_err().reason(),
            "invalid_key_name"
        );

        let empty = FOUR_TIERS.replace("\"Key_A_Normal\": \"N\"", "\"Key_A_Normal\": \"\"");
        assert_eq!(
            from_asset_json(&empty).unwrap_err().reason(),
            "empty_output"
        );

        let decomposed = FOUR_TIERS.replace("\"Key_A_Normal\": \"N\"", "\"Key_A_Normal\": \"ো\"");
        assert_eq!(
            from_asset_json(&decomposed).unwrap_err().reason(),
            "not_nfc"
        );

        let four_key_seq = FOUR_TIERS.replace(
            "\"sequences\": []",
            "\"sequences\": [{\"keys\": [\"Key_G\",\"Key_S\",\"Key_D\",\"Key_A\"], \"output\": \"X\"}]",
        );
        assert_eq!(
            from_asset_json(&four_key_seq).unwrap_err().reason(),
            "sequence_too_long"
        );

        for error in [
            FixedLayoutError::InvalidJson,
            FixedLayoutError::UnsupportedKind,
            FixedLayoutError::InvalidVersion,
            FixedLayoutError::RuleCountMismatch,
            FixedLayoutError::InvalidModifierName,
            FixedLayoutError::InvalidKeyName,
            FixedLayoutError::DuplicateKey,
            FixedLayoutError::EmptyOutput,
            FixedLayoutError::NotNfc,
            FixedLayoutError::InvalidSequence,
            FixedLayoutError::SequenceTooLong,
            FixedLayoutError::TooManySequences,
        ] {
            assert!(
                ERROR_REASONS.contains(&error.reason()),
                "reason not allowlisted: {}",
                error.reason()
            );
            assert_eq!(error.to_string(), error.reason());
        }
    }

    #[test]
    fn embedded_national_asset_resolves_golden_keys_and_sequence() {
        let layout = from_asset_json(NATIONAL).expect("national asset");
        assert_eq!(layout.name(), "national-v1-fixed");
        assert_eq!(layout.resolve("Key_N", NONE), Some("স"));
        assert_eq!(layout.resolve("Key_H", NONE), Some("া"));
        assert_eq!(layout.resolve("Key_A", NONE), Some("ৃ"));
        assert_eq!(layout.resolve("Key_G", NONE), Some("্"));
        assert_eq!(layout.resolve("Key_1", NONE), Some("১"));
        assert_eq!(layout.resolve("Num1", NONE), Some("১"));
        assert_eq!(layout.resolve("Key_OEM3", NONE), Some("\u{200c}"));
        assert_eq!(layout.resolve_sequence(&["Key_G", "Key_S"]), Some("উ"));
    }

    #[test]
    fn embedded_probhat_asset_resolves_golden_keys() {
        let layout = from_asset_json(PROBHAT).expect("probhat asset");
        assert_eq!(layout.name(), "probhat-v1-fixed");
        assert_eq!(layout.resolve("Key_N", NONE), Some("ন"));
        assert_eq!(layout.resolve("Key_H", NONE), Some("হ"));
        assert_eq!(layout.resolve("Key_A", NONE), Some("া"));
        assert_eq!(layout.resolve_sequence(&["Key_G", "Key_S"]), None);
    }

    #[test]
    fn outputs_are_nfc() {
        for raw in [NATIONAL, PROBHAT] {
            let layout = from_asset_json(raw).expect("asset");
            for output in [layout.resolve("Key_N", NONE), layout.resolve("Key_A", NONE)] {
                let output = output.expect("mapped key");
                assert_eq!(normalize_nfc(output), output);
            }
        }
    }
}
