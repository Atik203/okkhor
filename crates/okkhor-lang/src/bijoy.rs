//! Bijoy (ANSI) encoding conversion.
//!
//! Okkhor holds Bengali text internally as Unicode. Some official workflows
//! still need Bijoy byte text: the same ASCII bytes that, rendered with a
//! Bijoy-compatible font (Kalpurush ANSI by default, SutonnyMJ where
//! installed), show Bengali glyphs. The byte mapping is font-independent, so
//! converting here is correct for either font.
//!
//! Conversion is driven by the ordered rewrite map in
//! `assets/encodings/bijoy-v1.json` (111 rules, owner-supplied legacy data).
//! [`to_bijoy`] applies every rule sequentially in map order with regex
//! replace-all semantics. Order matters: pre-base vowel reorderings such as
//! `(.)ি` -> `d\1` run before the plain consonant rules, which is what turns
//! `কি` into `dj` instead of `jd`.
//!
//! [`from_bijoy`] is a best-effort longest-match inverse over the literal
//! (non-regex) rules only. It is documented as approximate: context-sensitive
//! rules such as `ো` -> `cf` do not round-trip (`cf` decodes to `ো`, not
//! `ো`).
//!
//! Both functions are pure after the first call: the map is parsed from the
//! embedded JSON and every rule compiled exactly once behind a [`OnceLock`].
//! Inputs are bounded at [`MAX_INPUT_CHARS`] Unicode scalar values; anything
//! longer is rejected, and unmapped characters pass through unchanged.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Maximum accepted input length in Unicode scalar values for both
/// directions. Keeps conversion time and allocation bounded on any path.
pub const MAX_INPUT_CHARS: usize = 4096;

/// Number of ordered rewrite rules in the embedded `bijoy-v1` map.
pub const RULE_COUNT: usize = 111;

/// Embedded rewrite map. The JSON file keeps exact legacy rule order in its
/// `rules` array; array order is the application order.
const BIJOY_MAP_JSON: &str = include_str!("../assets/encodings/bijoy-v1.json");

/// Errors from Bijoy conversion.
///
/// `Display` strings are static: they never contain user text or map
/// contents. Use [`BijoyError::kind`] for logging.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum BijoyError {
    /// Input exceeds [`MAX_INPUT_CHARS`] Unicode scalar values.
    #[error("bijoy input exceeds 4096 characters")]
    InputTooLong,
    /// The embedded map JSON could not be parsed (packaging bug).
    #[error("bijoy map is corrupt")]
    MapInvalid,
    /// Rule `index` (0-based map order) failed to compile (packaging bug).
    #[error("bijoy rule is invalid")]
    InvalidRule {
        /// 0-based position in map order.
        index: usize,
    },
}

impl BijoyError {
    /// Stable snake_case identifier for logging. The only logging-safe
    /// projection of the error.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InputTooLong => "bijoy_input_too_long",
            Self::MapInvalid => "bijoy_map_invalid",
            Self::InvalidRule { .. } => "bijoy_invalid_rule",
        }
    }
}

/// One ordered rewrite rule as stored in `bijoy-v1.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BijoyRule {
    pattern: String,
    replacement: String,
}

/// Provenance header of the embedded map file (kept for documentation; only
/// `rules` affects conversion).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BijoyMapFile {
    name: String,
    version: String,
    #[serde(default)]
    rule_count: usize,
    rules: Vec<BijoyRule>,
}

/// A compiled rewrite rule: regex plus a `regex`-crate replacement string.
#[derive(Debug)]
struct CompiledRule {
    regex: Regex,
    replacement: String,
}

/// Compiled forward rules plus the longest-match reverse table.
#[derive(Debug)]
struct CompiledMap {
    forward: Vec<CompiledRule>,
    /// `(bijoy bytes, unicode)` sorted by byte length descending; first
    /// rule wins on duplicate Bijoy forms. Literal rules only.
    reverse: Vec<(Vec<u8>, String)>,
}

/// Internal load failure; [`BijoyError`] is rebuilt from this on each call
/// so the cached value stays `Clone`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MapFailure {
    Json,
    Rule { index: usize },
}

impl MapFailure {
    fn as_error(self) -> BijoyError {
        match self {
            Self::Json => BijoyError::MapInvalid,
            Self::Rule { index } => BijoyError::InvalidRule { index },
        }
    }
}

static COMPILED: OnceLock<Result<CompiledMap, MapFailure>> = OnceLock::new();

fn compiled_map() -> Result<&'static CompiledMap, BijoyError> {
    COMPILED
        .get_or_init(|| {
            let file: BijoyMapFile =
                serde_json::from_str(BIJOY_MAP_JSON).map_err(|_| MapFailure::Json)?;
            compile_map(&file.rules).map_err(|index| MapFailure::Rule { index })
        })
        .as_ref()
        .map_err(|failure| failure.as_error())
}

/// Compile ordered legacy rules. Returns the 0-based index of the first rule
/// that fails to compile; never panics.
fn compile_map(rules: &[BijoyRule]) -> Result<CompiledMap, usize> {
    let mut forward = Vec::with_capacity(rules.len());
    for (index, rule) in rules.iter().enumerate() {
        let regex = Regex::new(&rule.pattern).map_err(|_| index)?;
        let replacement = convert_replacement(&rule.replacement);
        forward.push(CompiledRule { regex, replacement });
    }
    let reverse = build_reverse_table(rules);
    Ok(CompiledMap { forward, reverse })
}

/// Translate a legacy replacement string to `regex`-crate syntax.
///
/// Legacy syntax (as stored in the map): `\1` is capture group 1, `\\` is a
/// literal backslash (so `ৎ` maps to one `\` byte). The `regex` crate uses
/// `$1`/`$name`/`$$` instead and treats `\` as literal, so every `$` must be
/// doubled and every `\`+digits run becomes a braced `${n}` reference.
/// Braces are required (not `$1`): rule 9's `\1A` must expand to group 1
/// plus `A`, while `$1A` would name a non-existent group `1A` and expand
/// to nothing.
///
/// Operates on Unicode scalar values: non-ASCII replacement text (rules
/// 10/11/26/27 emit Bengali) passes through untouched.
fn convert_replacement(legacy: &str) -> String {
    let mut out = String::with_capacity(legacy.len() + 4);
    let mut chars = legacy.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$' {
            out.push_str("$$");
        } else if ch == '\\' {
            match chars.peek() {
                Some(next) if next.is_ascii_digit() => {
                    out.push_str("${");
                    while let Some(digit) = chars.peek() {
                        if digit.is_ascii_digit() {
                            out.push(*digit);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    out.push('}');
                }
                Some('\\') => {
                    out.push('\\');
                    chars.next();
                }
                _ => {
                    out.push('\\');
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// True if the pattern is a plain literal (no regex syntax), so it can join
/// the longest-match reverse table.
fn is_literal_pattern(pattern: &str) -> bool {
    !pattern.bytes().any(|b| {
        matches!(
            b,
            b'(' | b')'
                | b'['
                | b']'
                | b'|'
                | b'\\'
                | b'.'
                | b'*'
                | b'+'
                | b'?'
                | b'^'
                | b'$'
                | b'{'
        )
    })
}

/// Build the longest-match reverse table from literal rules only, in map
/// order (first rule wins on duplicate Bijoy forms), sorted by Bijoy byte
/// length descending.
fn build_reverse_table(rules: &[BijoyRule]) -> Vec<(Vec<u8>, String)> {
    let mut table: Vec<(Vec<u8>, String)> = Vec::new();
    for rule in rules {
        if !is_literal_pattern(&rule.pattern) {
            continue;
        }
        if rule.replacement.bytes().any(|b| b == b'\\') {
            // Group reference: context-sensitive rule, not invertible here.
            continue;
        }
        let key = convert_replacement(&rule.replacement).into_bytes();
        if key.is_empty() {
            continue;
        }
        if table.iter().any(|(existing, _)| *existing == key) {
            continue;
        }
        table.push((key, rule.pattern.clone()));
    }
    // Stable sort: keeps map order among equal-length keys.
    table.sort_by_key(|rule| std::cmp::Reverse(rule.0.len()));
    table
}

/// Rewrite Unicode Bengali text to Bijoy (ANSI) bytes, applying every map
/// rule sequentially in map order.
///
/// Unmapped characters (ASCII punctuation, Latin text, spaces, newlines, and
/// any Bengali character with no rule) pass through unchanged. Rejects
/// inputs longer than [`MAX_INPUT_CHARS`] characters.
pub fn to_bijoy(input: &str) -> Result<String, BijoyError> {
    if input.chars().count() > MAX_INPUT_CHARS {
        return Err(BijoyError::InputTooLong);
    }
    let map = compiled_map()?;
    let mut text = input.to_owned();
    for rule in &map.forward {
        text = rule
            .regex
            .replace_all(&text, rule.replacement.as_str())
            .into_owned();
    }
    Ok(text)
}

/// Best-effort inverse of [`to_bijoy`] by greedy longest match over the
/// literal rules.
///
/// Approximate by design: context-sensitive forward rules (pre-base vowel
/// reordering, reph/ref forms) have no literal inverse, so e.g. `cf`
/// decodes to `ো` rather than `ো`. Where a Bijoy form has several Unicode
/// sources (e.g. `p` from both `ড়` spellings), the first map rule wins.
/// Unmatched bytes pass through unchanged.
/// Rejects inputs longer than [`MAX_INPUT_CHARS`] characters.
pub fn from_bijoy(input: &str) -> Result<String, BijoyError> {
    if input.chars().count() > MAX_INPUT_CHARS {
        return Err(BijoyError::InputTooLong);
    }
    let map = compiled_map()?;
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut pos = 0;
    while pos < bytes.len() {
        let mut matched = false;
        for (key, unicode) in &map.reverse {
            if bytes[pos..].starts_with(key) {
                out.push_str(unicode);
                pos += key.len();
                matched = true;
                break;
            }
        }
        if !matched {
            // Passthrough: copy one whole character (may be multi-byte).
            let rest = &input[pos..];
            let ch = rest.chars().next().unwrap_or('\u{FFFD}');
            out.push(ch);
            pos += ch.len_utf8();
        }
    }
    Ok(out)
}

/// Number of forward rules in the embedded map. Loads (and validates) the
/// map; callers that only need [`to_bijoy`] do not need this.
pub fn rule_count() -> Result<usize, BijoyError> {
    Ok(compiled_map()?.forward.len())
}

#[cfg(test)]
mod tests {
    use super::{BijoyError, MAX_INPUT_CHARS, RULE_COUNT, compiled_map, from_bijoy, to_bijoy};

    /// Every golden below is derived from the map by hand-tracing rule
    /// order; legacy differential testing against the installed reference
    /// is still pending.
    fn golden_cases() -> Vec<(&'static str, &'static str)> {
        vec![
            ("আমি", "Ffdm"),           // map-derived, legacy differential pending
            ("কি", "dj"),              // map-derived, legacy differential pending
            ("মা", "mf"),               // map-derived, legacy differential pending
            ("তুমি", "ksdm"),           // map-derived, legacy differential pending
            ("বাংলা", "hfQVf"),          // map-derived, legacy differential pending
            ("আমার", "Ffmfv"),          // map-derived, legacy differential pending
            ("ঢাকা", "Efjf"),            // map-derived, legacy differential pending
            ("স্কুল", "ngjsV"),          // map-derived, legacy differential pending
            ("ক্ষ", "jgN"),             // map-derived, legacy differential pending
            ("জ্ঞ", "ugI"),             // map-derived, legacy differential pending
            ("্র", "z"),                // map-derived, legacy differential pending
            ("্য", "Z"),                // map-derived, legacy differential pending
            ("ো", "cf"),               // map-derived, legacy differential pending
            ("ৌ", "cX"),               // map-derived, legacy differential pending
            ("ৎ", "\\"),               // map-derived, legacy differential pending
            ("ঃ", "|"),                // map-derived, legacy differential pending
            ("১", "1"),                // map-derived, legacy differential pending
            ("২", "2"),                // map-derived, legacy differential pending
            ("৩", "3"),                // map-derived, legacy differential pending
            ("৪", "4"),                // map-derived, legacy differential pending
            ("৫", "5"),                // map-derived, legacy differential pending
            ("৬", "6"),                // map-derived, legacy differential pending
            ("৭", "7"),                // map-derived, legacy differential pending
            ("৮", "8"),                // map-derived, legacy differential pending
            ("৯", "9"),                // map-derived, legacy differential pending
            ("০", "0"),                // map-derived, legacy differential pending
            ("কে", "cj"),              // map-derived, legacy differential pending
            ("কী", "jD"),              // map-derived, legacy differential pending
            ("র্ক", "jA"),              // map-derived, legacy differential pending
            ("ক্র", "jz"),              // map-derived, legacy differential pending
            ("ক্য", "jZ"),              // map-derived, legacy differential pending
            ("ক্রু", "jsz"),             // map-derived, legacy differential pending
            ("৳", "$"),                // map-derived, legacy differential pending
            ("।", "G"),                // map-derived, legacy differential pending
            ("অ", "F"),                // map-derived, legacy differential pending
            ("আ", "Ff"),               // map-derived, legacy differential pending
            ("চাঁদ", "yf&l"),            // map-derived, legacy differential pending
            ("বাংলাদেশ", "hfQVfclM"),    // map-derived, legacy differential pending
            ("কর্তা", "jkAf"),           // map-derived, legacy differential pending
            ("শিক্ষা", "dMjgNf"),        // map-derived, legacy differential pending
            ("জ্ঞান", "ugIfb"),          // map-derived, legacy differential pending
            ("প্রথম", "rzKm"),          // map-derived, legacy differential pending
            ("দুঃখ", "ls|J"),           // map-derived, legacy differential pending
            ("স্ত্রী", "ngkzD"),         // map-derived, legacy differential pending
            ("অত্যন্ত", "FkZbgk"),       // map-derived, legacy differential pending
            ("বিক্রি", "dhdjz"),        // map-derived, legacy differential pending
            ("কম্পিউটার", "jdmgrgstfv"), // map-derived, legacy differential pending
        ]
    }

    #[test]
    fn map_loads_with_expected_rule_count() {
        let map = compiled_map().expect("embedded bijoy map must load");
        assert_eq!(map.forward.len(), RULE_COUNT);
        assert_eq!(map.forward.len(), 111);
        assert!(!map.reverse.is_empty());
    }

    #[test]
    fn goldens_convert_exactly() {
        for (input, expected) in golden_cases() {
            assert_eq!(
                to_bijoy(input).expect("golden input must convert"),
                expected,
                "to_bijoy({input:?})"
            );
        }
    }

    #[test]
    fn pre_base_reordering_happens_before_consonants() {
        // If consonant rules ran first, কি would become j + d = "jd".
        assert_eq!(to_bijoy("কি").unwrap(), "dj");
        assert_eq!(to_bijoy("দি").unwrap(), "dl");
        assert_eq!(to_bijoy("তুমি").unwrap(), "ksdm");
    }

    #[test]
    fn conjuncts_digits_punctuation() {
        assert_eq!(to_bijoy("ক্ষ").unwrap(), "jgN");
        assert_eq!(to_bijoy("জ্ঞ").unwrap(), "ugI");
        assert_eq!(to_bijoy("১২৩").unwrap(), "123");
        assert_eq!(to_bijoy("“বাংলা”").unwrap(), "~hfQVf\"");
        assert_eq!(to_bijoy("৳১০।").unwrap(), "$10G");
    }

    #[test]
    fn taka_and_chandrabindu() {
        assert_eq!(to_bijoy("৳").unwrap(), "$");
        assert_eq!(to_bijoy("চাঁদ").unwrap(), "yf&l");
    }

    #[test]
    fn empty_input_gives_empty_output() {
        assert_eq!(to_bijoy("").unwrap(), "");
        assert_eq!(from_bijoy("").unwrap(), "");
    }

    #[test]
    fn ascii_passthrough() {
        assert_eq!(to_bijoy("ABC 123").unwrap(), "ABC 123");
        assert_eq!(to_bijoy("আমি OK").unwrap(), "Ffdm OK");
        assert_eq!(to_bijoy("a\nb").unwrap(), "a\nb");
    }

    #[test]
    fn input_bound_is_enforced() {
        let ok_input = "অ".repeat(MAX_INPUT_CHARS);
        assert!(to_bijoy(&ok_input).is_ok());
        assert!(from_bijoy(&ok_input).is_ok());
        let long = "অ".repeat(MAX_INPUT_CHARS + 1);
        assert_eq!(to_bijoy(&long), Err(BijoyError::InputTooLong));
        assert_eq!(from_bijoy(&long), Err(BijoyError::InputTooLong));
    }

    #[test]
    fn conversion_is_deterministic() {
        let input = "আমি তুমি বাংলা স্কুল ক্ষ জ্ঞ ১২৩";
        assert_eq!(to_bijoy(input).unwrap(), to_bijoy(input).unwrap());
    }

    #[test]
    fn reverse_round_trips_simple_forms() {
        assert_eq!(from_bijoy("mf").unwrap(), "মা");
        // Longest match wins: "Ff" (আ) beats "F" (অ).
        assert_eq!(from_bijoy("Ff").unwrap(), "আ");
        assert_eq!(from_bijoy("F").unwrap(), "অ");
        // Bijoy bytes decode even without Unicode context: O = ঘ, K = থ.
        assert_eq!(from_bijoy("OK").unwrap(), "ঘথ");
    }

    #[test]
    fn reverse_first_map_rule_wins_duplicates() {
        // `v` is produced by both ব় (rule 5) and র (rule 92); the first
        // map rule owns the inverse. Same for p (ড় spellings) and W (য়).
        assert_eq!(from_bijoy("v").unwrap(), "ব\u{9bc}");
        assert_eq!(from_bijoy("p").unwrap(), "ড\u{9bc}");
        assert_eq!(from_bijoy("W").unwrap(), "য\u{9bc}");
        // So Ffmfv inverts to আমাব়, not আমার: documented, approximate.
        assert_eq!(from_bijoy("Ffmfv").unwrap(), "আমাব\u{9bc}");
    }

    #[test]
    fn replacement_syntax_conversion_is_exact() {
        use super::convert_replacement;
        // Braced groups: \1A is group 1 plus 'A', never group "1A".
        assert_eq!(convert_replacement("\\1A"), "${1}A");
        assert_eq!(convert_replacement("c\\1"), "c${1}");
        // Legacy \\ is one literal backslash (ৎ maps to a single byte).
        assert_eq!(convert_replacement("\\\\"), "\\");
        // $ must not become a group reference.
        assert_eq!(convert_replacement("$"), "$$");
        // Non-ASCII replacement text passes through untouched.
        assert_eq!(convert_replacement("ো"), "ো");
    }

    #[test]
    fn vowel_sign_before_ra_forms_reorders() {
        // Rules 26/27 move a vowel sign after ্র/্য before ্র/্য collapse.
        // Synthetic inputs (map-derived, legacy differential pending).
        assert_eq!(to_bijoy("া\u{9cd}র").unwrap(), "zf");
        assert_eq!(to_bijoy("া\u{9cd}য").unwrap(), "Zf");
        assert_eq!(to_bijoy("\u{9bf}\u{9cd}র").unwrap(), "zd");
        assert_eq!(to_bijoy("ে\u{9cd}র").unwrap(), "zc");
    }

    #[test]
    fn reverse_is_approximate_for_reordered_vowels() {
        // Forward: ো -> cf. Inverse has no context rule, so cf decodes to
        // the literal parts ে + া. Documented limitation, not a bug.
        assert_eq!(to_bijoy("ো").unwrap(), "cf");
        assert_eq!(from_bijoy("cf").unwrap(), "ো");
    }

    #[test]
    fn reverse_passes_through_unmapped_bytes() {
        // '#' and space are not Bijoy outputs of any rule, so they survive.
        assert_eq!(from_bijoy("Ff #").unwrap(), "আ #");
        assert_eq!(from_bijoy("").unwrap(), "");
    }

    #[test]
    fn invalid_rule_fails_with_typed_error() {
        let bad = vec![super::BijoyRule {
            pattern: "(".to_owned(),
            replacement: "x".to_owned(),
        }];
        let err = super::compile_map(&bad).unwrap_err();
        assert_eq!(err, 0);
        // And the public error type carries the index without panicking.
        let public = BijoyError::InvalidRule { index: 0 };
        assert_eq!(public.kind(), "bijoy_invalid_rule");
    }

    #[test]
    fn error_kinds_are_stable() {
        assert_eq!(BijoyError::InputTooLong.kind(), "bijoy_input_too_long");
        assert_eq!(BijoyError::MapInvalid.kind(), "bijoy_map_invalid");
        assert_eq!(
            BijoyError::InvalidRule { index: 3 }.kind(),
            "bijoy_invalid_rule"
        );
        for error in [
            BijoyError::InputTooLong,
            BijoyError::MapInvalid,
            BijoyError::InvalidRule { index: 0 },
        ] {
            let text = format!("{error}");
            assert!(!text.contains("C:\\"), "leaks path: {text}");
        }
    }

    #[test]
    fn reverse_table_is_longest_first_and_covers_literals() {
        let map = compiled_map().expect("embedded bijoy map must load");
        for (key, _) in &map.reverse {
            assert!(!key.is_empty(), "reverse key must not be empty");
        }
        // Longest-match order: non-increasing key length.
        let mut prev = usize::MAX;
        for (key, _) in &map.reverse {
            assert!(key.len() <= prev, "reverse table not longest-first");
            prev = key.len();
        }
        // Literal multi-char rules invert exactly: ো <-> ো.
        assert_eq!(from_bijoy("ো").unwrap(), "ো");
    }
}
