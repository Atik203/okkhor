use okkhor_core::{contains_bengali, normalize_lookup, normalize_nfc};

/// Normalize roman (Latin-script) input: ASCII lowercase, then trim and
/// collapse whitespace via [`normalize_lookup`].
///
/// Pure, deterministic, and bounded by input length.
pub fn normalize_roman(input: &str) -> String {
    normalize_lookup(&input.to_ascii_lowercase())
}

/// Normalize Bengali-script text to NFC.
///
/// Preserves whitespace, ZWJ/ZWNJ, and case. Pure, deterministic, and bounded
/// by input length.
pub fn normalize_bengali(input: &str) -> String {
    normalize_nfc(input)
}

/// Canonical rule-lookup key: NFC, trim/collapse whitespace, ASCII lowercase.
///
/// Non-ASCII letters are left unchanged (apart from NFC composition). Used by
/// the rule importer and the engine so callers pass the raw key token.
pub fn normalize_key(input: &str) -> String {
    normalize_lookup(&input.to_ascii_lowercase())
}

/// Whether the input contains at least one Bengali-script character.
///
/// Delegates to [`okkhor_core::contains_bengali`].
pub fn is_bengali_script(input: &str) -> bool {
    contains_bengali(input)
}
