//! Unicode ↔ SutonnyMJ ANSI conversion.
//!
//! Okkhor holds Bengali text internally as Unicode. Some official workflows
//! still need SutonnyMJ byte text: the same ASCII/CP1252-range bytes that,
//! rendered with the SutonnyMJ font, show Bangla glyphs. Converting here is
//! correct for SutonnyMJ; Kalpurush ANSI shares the legacy Bijoy byte layout
//! for the old keyboard map but is **not** byte-identical to this table, so
//! this module targets SutonnyMJ only.
//!
//! # Keyboard layout ≠ font encoding
//!
//! These are different objects and the 0.5 code confused them:
//!
//! - A **keyboard layout** says which key types which character (positional
//!   input). `assets/encodings/bijoy-keyboard-v1.json` (`Bangla to Bijoy
//!   layout`, 111 rules) is such a layout. It is kept for reference only and
//!   is never used as the output encoder.
//! - A **font encoding** says which byte sequence renders which glyph under a
//!   legacy font. `assets/encodings/sutonnymj-ansi-v1.json` (`ANSI 1`, "for
//!   SutoniMJ font", 256 rules) is the actual SutonnyMJ font encoding, and
//!   [`to_bijoy`] applies exactly that table. ("Bijoy" in [`to_bijoy`] /
//!   [`from_bijoy`] / [`BijoyError`] is the historic public name for this
//!   byte output; it is retained so existing call sites keep compiling. It
//!   means SutonnyMJ ANSI.)
//!
//! # The ANSI 1 rule model
//!
//! Conversion is driven by the ordered rewrite tables in
//! `assets/encodings/sutonnymj-ansi-v1.json` (256 Unicode→ANSI rules) and
//! `assets/encodings/sutonnymj-ansi-reverse-v1.json` (676 ANSI→Unicode
//! rules), both derived from the owner-supplied legacy `.nmsEnco` data
//! (developer Md Nabil Mostofa, publicly mirrored; provenance fields are in
//! the files, only `keyMap` order and entries were extracted).
//!
//! [`to_bijoy`] applies every forward rule **sequentially in table order**
//! with regex replace-all semantics. Order is load-bearing:
//!
//! - `ো`/`ৌ` decompose to `ে` + `া`/`ৗ` first; the later `ে`/`ৈ`/`ি`
//!   pre-base reorderings (`([cons])(©?)ে` → `‡\1\2`) then move the vowel
//!   sign in front of its consonant cluster (`কি` → `wক` → `wK`).
//! - The reph marker `©` (U+00A9, an intermediate only) is emitted by the
//!   `র্…` rules (`র্([cons])` → `\1©`) and consumed by the later
//!   `(…)(©?)ে/ি/ৈ` rules, so no `©` leaks into finished output.
//! - `্র`/`্য`/`্ব`/`্ল` collapsing and the big conjunct table run after the
//!   vowel-sign rules that may consume surrounding signs (`স্কুল` → `স্কzল`
//!   via the `ু` lookbehind rule → `¯‹zj`).
//! - Digits, `৳` → `$`, `।` → `|`, and punctuation map last.
//!
//! [`from_bijoy`] applies every reverse rule the same way (sequentially in
//! table order) for paste/import. The reverse table is the legacy inverse,
//! so cached `K → ক` rewrites run before the `w([cons])(©?) → \1\2ি`
//! patterns; that cascade is what makes `wK` decode back to `কি`.
//!
//! # Why `fancy-regex`
//!
//! The forward table needs two features the `regex` crate deliberately omits:
//! negative lookbehind (`(?<!্)` guards the `ৃ`/`ু`/`ূ`/`্র…` rules so they
//! do not fire inside a larger cluster) and `\1`/`\2` backreferences in
//! replacements. Translating the lookbehind away would mean re-encoding the
//! legacy guard as capture-and-re-emit prefixes, which changes match
//! boundaries for overlapping sequential rewrites and risks diverging from
//! the legacy engine (e.g. the `স্কুল` → `¯‹zj` trace above depends on the
//! exact guard). `fancy-regex` (MIT, pure Rust, no native code) supports
//! both constructs with the same semantics as the legacy engine, so the
//! table is applied verbatim. Both directions share the one engine.
//! Replacement expansion is done manually from `fancy_regex::Captures`
//! (`\1`…`\99` → group text, everything else literal) rather than through
//! replacement-string syntax, so `$`, `\`, and `©` in the tables can never
//! be misread as engine directives.
//!
//! # Value encoding note
//!
//! Table values are CP1252-range byte characters stored as Unicode (`‡`
//! U+2021, `‹` U+2039, `¯` U+00AF, `Ô` U+00D4, …). They are emitted and
//! matched as-is; no transcoding happens here. The one exception is the
//! legacy `স্প্ল` entry whose value ends in a real newline (`¯c­\n`); it is
//! preserved exactly as shipped and documented, not silently "fixed".
//!
//! Both functions are pure after the first call: the tables are parsed from
//! the embedded JSON and every rule compiled exactly once behind
//! [`OnceLock`]s. Inputs are bounded at [`MAX_INPUT_CHARS`] Unicode scalar
//! values; anything longer is rejected, and unmapped characters pass through
//! unchanged.

use std::sync::OnceLock;

use fancy_regex::{Captures, Regex};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Maximum accepted input length in Unicode scalar values for both
/// directions. Keeps conversion time and allocation bounded on any path.
pub const MAX_INPUT_CHARS: usize = 4096;

/// Number of ordered rewrite rules in the embedded forward
/// (`sutonnymj-ansi-v1`, Unicode→SutonnyMJ ANSI) table.
pub const RULE_COUNT: usize = 256;

/// Number of ordered rewrite rules in the embedded reverse
/// (`sutonnymj-ansi-reverse-v1`, SutonnyMJ ANSI→Unicode) table.
pub const REVERSE_RULE_COUNT: usize = 676;

/// Embedded forward rewrite table. The JSON file keeps exact legacy
/// `keyMap` order in its `rules` array; array order is the application
/// order.
const FORWARD_JSON: &str = include_str!("../assets/encodings/sutonnymj-ansi-v1.json");
/// Embedded reverse rewrite table, same ordering contract as
/// [`FORWARD_JSON`].
const REVERSE_JSON: &str = include_str!("../assets/encodings/sutonnymj-ansi-reverse-v1.json");

/// Errors from SutonnyMJ conversion.
///
/// `Display` strings are static: they never contain user text or table
/// contents. Use [`BijoyError::kind`] for logging.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum BijoyError {
    /// Input exceeds [`MAX_INPUT_CHARS`] Unicode scalar values.
    #[error("bijoy input exceeds 4096 characters")]
    InputTooLong,
    /// An embedded table JSON could not be parsed (packaging bug), or a
    /// match failed at runtime (unreachable for these tables; fail closed).
    #[error("bijoy map is corrupt")]
    MapInvalid,
    /// Rule `index` (0-based table order) failed to compile (packaging bug).
    #[error("bijoy rule is invalid")]
    InvalidRule {
        /// 0-based position in table order.
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

/// One ordered rewrite rule as stored in the `sutonnymj-ansi-*` files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SutonnyRule {
    pattern: String,
    replacement: String,
}

/// Provenance header of an embedded table file (kept for documentation; only
/// `rules` affects conversion).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SutonnyTableFile {
    name: String,
    version: String,
    #[serde(default)]
    rule_count: usize,
    rules: Vec<SutonnyRule>,
}

/// A compiled rewrite rule: regex plus its legacy replacement string.
/// Replacements are expanded manually (see [`expand_replacement`]), never
/// passed through engine replacement syntax.
#[derive(Debug)]
struct CompiledRule {
    regex: Regex,
    replacement: String,
}

/// Compiled forward and reverse tables.
#[derive(Debug)]
struct CompiledTables {
    forward: Vec<CompiledRule>,
    reverse: Vec<CompiledRule>,
}

/// Internal load failure; [`BijoyError`] is rebuilt from this on each call
/// so the cached value stays `Clone`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MapFailure {
    Json,
    Rule { index: usize },
    Runtime,
}

impl MapFailure {
    fn as_error(self) -> BijoyError {
        match self {
            Self::Json => BijoyError::MapInvalid,
            Self::Runtime => BijoyError::MapInvalid,
            Self::Rule { index } => BijoyError::InvalidRule { index },
        }
    }
}

static COMPILED: OnceLock<Result<CompiledTables, MapFailure>> = OnceLock::new();

fn compiled_tables() -> Result<&'static CompiledTables, BijoyError> {
    COMPILED
        .get_or_init(|| {
            let forward_file: SutonnyTableFile =
                serde_json::from_str(FORWARD_JSON).map_err(|_| MapFailure::Json)?;
            let reverse_file: SutonnyTableFile =
                serde_json::from_str(REVERSE_JSON).map_err(|_| MapFailure::Json)?;
            let forward =
                compile_rules(&forward_file.rules).map_err(|index| MapFailure::Rule { index })?;
            // Reverse-table indices are reported in the same `index` field;
            // the two tables are compiled separately, so an index always
            // refers to the table that failed (forward is loaded first).
            let reverse =
                compile_rules(&reverse_file.rules).map_err(|index| MapFailure::Rule { index })?;
            Ok(CompiledTables { forward, reverse })
        })
        .as_ref()
        .map_err(|failure| failure.as_error())
}

/// Compile ordered legacy rules. Returns the 0-based index of the first rule
/// that fails to compile; never panics.
fn compile_rules(rules: &[SutonnyRule]) -> Result<Vec<CompiledRule>, usize> {
    let mut compiled = Vec::with_capacity(rules.len());
    for (index, rule) in rules.iter().enumerate() {
        let regex = Regex::new(&rule.pattern).map_err(|_| index)?;
        compiled.push(CompiledRule {
            regex,
            replacement: rule.replacement.clone(),
        });
    }
    Ok(compiled)
}

/// Expand a legacy replacement string against one match.
///
/// Legacy syntax: `\1`…`\99` is capture group N, everything else (including
/// `$`, `©`, and a lone trailing `\`) is literal. An out-of-range or
/// unmatched group expands to empty, matching standard replace-all
/// behavior.
fn expand_replacement(caps: &Captures<'_, str>, replacement: &str) -> String {
    let mut out = String::with_capacity(replacement.len() + 8);
    let mut chars = replacement.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.peek() {
                Some(next) if next.is_ascii_digit() => {
                    let mut num = String::new();
                    while let Some(digit) = chars.peek() {
                        if digit.is_ascii_digit() {
                            num.push(*digit);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    let group: usize = num.parse().unwrap_or(usize::MAX);
                    if let Ok(n) = u32::try_from(group) {
                        if let Some(m) = caps.get(n as usize) {
                            out.push_str(m.as_str());
                        }
                    }
                }
                _ => {
                    // Lone backslash (or `\` + non-digit): literal. The
                    // shipped tables only use `\1`/`\2`, so this is purely
                    // defensive and keeps a future table edit total.
                    out.push('\\');
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Apply one compiled rule left-to-right with replace-all semantics,
/// expanding legacy `\N` groups manually. A runtime match failure is
/// reported so the caller can fail closed; never panics, including on
/// zero-width matches (progress is forced by consuming one character).
fn apply_rule(text: &str, rule: &CompiledRule) -> Result<String, MapFailure> {
    let mut out = String::with_capacity(text.len() + 8);
    let mut pos = 0;
    while pos <= text.len() {
        let caps = rule
            .regex
            .captures_from_pos(text, pos)
            .map_err(|_| MapFailure::Runtime)?;
        let Some(caps) = caps else {
            out.push_str(&text[pos..]);
            break;
        };
        let whole = caps.get(0).ok_or(MapFailure::Runtime)?;
        let (start, end) = (whole.start(), whole.end());
        out.push_str(&text[pos..start]);
        out.push_str(&expand_replacement(&caps, &rule.replacement));
        if end > start {
            pos = end;
        } else if start < text.len() {
            // Zero-width match: emit one scalar to guarantee progress.
            let ch = text[start..].chars().next().unwrap_or('\u{FFFD}');
            out.push(ch);
            pos = start + ch.len_utf8();
        } else {
            break;
        }
    }
    Ok(out)
}

/// Apply a whole ordered table, one rule after another.
fn apply_table(text: &str, rules: &[CompiledRule]) -> Result<String, MapFailure> {
    let mut current = text.to_owned();
    for rule in rules {
        current = apply_rule(&current, rule)?;
    }
    Ok(current)
}

/// Rewrite Unicode Bengali text to SutonnyMJ ANSI bytes, applying every
/// forward-table rule sequentially in table order.
///
/// Unmapped characters (ASCII punctuation, Latin text, spaces, newlines, and
/// any Bengali character with no rule) pass through unchanged. Rejects
/// inputs longer than [`MAX_INPUT_CHARS`] characters.
pub fn to_bijoy(input: &str) -> Result<String, BijoyError> {
    if input.chars().count() > MAX_INPUT_CHARS {
        return Err(BijoyError::InputTooLong);
    }
    let tables = compiled_tables()?;
    apply_table(input, &tables.forward).map_err(|failure| failure.as_error())
}

/// Inverse of [`to_bijoy`] for paste/import: rewrites SutonnyMJ ANSI bytes
/// back to Unicode, applying every reverse-table rule sequentially in table
/// order.
///
/// The reverse table is the legacy inverse, so most text round-trips, but
/// exactness is not guaranteed for every input: line-start `†`/`ˆ` forms
/// (legacy `^‡`/`^‰` rules) decode to `‡`/`‰` plus the vowel sign rather
/// than recomposing `ো`/`ৌ`, bare `w`/`z` (reordered `ি`/`ু` markers without
/// a following cluster yet) pass through until a later rule supplies the
/// cluster, and the legacy `স্প্ল` newline quirk survives as-is. Unmatched
/// bytes pass through unchanged. Rejects inputs longer than
/// [`MAX_INPUT_CHARS`] characters.
pub fn from_bijoy(input: &str) -> Result<String, BijoyError> {
    if input.chars().count() > MAX_INPUT_CHARS {
        return Err(BijoyError::InputTooLong);
    }
    let tables = compiled_tables()?;
    apply_table(input, &tables.reverse).map_err(|failure| failure.as_error())
}

/// Number of forward rules in the embedded table. Loads (and validates) the
/// tables; callers that only need [`to_bijoy`] do not need this.
pub fn rule_count() -> Result<usize, BijoyError> {
    Ok(compiled_tables()?.forward.len())
}

/// Number of reverse rules in the embedded table. Loads (and validates) the
/// tables; callers that only need [`from_bijoy`] do not need this.
pub fn reverse_rule_count() -> Result<usize, BijoyError> {
    Ok(compiled_tables()?.reverse.len())
}

#[cfg(test)]
mod tests {
    use super::{
        BijoyError, MAX_INPUT_CHARS, REVERSE_RULE_COUNT, RULE_COUNT, compiled_tables, from_bijoy,
        reverse_rule_count, rule_count, to_bijoy,
    };

    /// The eight SutonnyMJ render-verified goldens (checked by rendering
    /// with `assets/fonts/SutonnyMJ Regular.ttf`; see
    /// `akkhor-sdd-reports/sutonnymj-ansi1-check.png`). Each was additionally
    /// hand-traced through the ordered table before being pinned here.
    fn render_goldens() -> Vec<(&'static str, &'static str)> {
        vec![
            ("আমার", "Avgvi"),
            ("কি", "wK"),
            ("মা", "gv"),
            ("তুমি", "Zzwg"),
            ("বাংলা", "evsjv"),
            ("ভালো", "fv‡jv"),
            ("ঢাকা", "XvKv"),
            ("স্কুল", "¯‹zj"),
        ]
    }

    #[test]
    fn tables_load_with_expected_rule_counts() {
        let tables = compiled_tables().expect("embedded sutonnymj tables must load");
        assert_eq!(tables.forward.len(), RULE_COUNT);
        assert_eq!(tables.forward.len(), 256);
        assert_eq!(tables.reverse.len(), REVERSE_RULE_COUNT);
        assert_eq!(tables.reverse.len(), 676);
        assert_eq!(rule_count().unwrap(), 256);
        assert_eq!(reverse_rule_count().unwrap(), 676);
    }

    #[test]
    fn render_goldens_convert_exactly() {
        for (input, expected) in render_goldens() {
            assert_eq!(
                to_bijoy(input).expect("golden input must convert"),
                expected,
                "to_bijoy({input:?})"
            );
        }
    }

    #[test]
    fn lookbehind_guards_cluster_vowels() {
        // (?<!্) keeps the ি/ু rules from firing inside a larger cluster:
        // স্কুল routes through স্ক+ু (¯‹zj), it must NOT split স্ক apart.
        assert_eq!(to_bijoy("স্কুল").unwrap(), "¯‹zj");
        // ... while a plain CV ি still reorders in front (wK, not Kw).
        assert_eq!(to_bijoy("কি").unwrap(), "wK");
        assert_eq!(to_bijoy("দি").unwrap(), "w`");
        assert_eq!(to_bijoy("তুমি").unwrap(), "Zzwg");
    }

    #[test]
    fn conjuncts_collapse() {
        assert_eq!(to_bijoy("ক্ষ").unwrap(), "¶");
        assert_eq!(to_bijoy("জ্ঞ").unwrap(), "Á");
        assert_eq!(to_bijoy("ক্ক").unwrap(), "°");
        assert_eq!(to_bijoy("স্ত্র").unwrap(), "¯¿");
        assert_eq!(to_bijoy("ত্র").unwrap(), "Î");
        assert_eq!(to_bijoy("ন্ড").unwrap(), "Û");
        // Reph marker: র্ক carries the © through to the byte form …
        assert_eq!(to_bijoy("র্ক").unwrap(), "K©");
        // … and the reverse table consumes it back.
        assert_eq!(from_bijoy("K©").unwrap(), "র্ক");
    }

    #[test]
    fn o_kar_and_ou_kar_decompose_then_mark_line_start() {
        // ো → ে + া, then the legacy ^‡ rule marks a leading ‡ with †.
        assert_eq!(to_bijoy("ো").unwrap(), "†v");
        assert_eq!(to_bijoy("ৌ").unwrap(), "†Š");
        assert_eq!(to_bijoy("কে").unwrap(), "†K");
        assert_eq!(to_bijoy("কী").unwrap(), "Kx");
    }

    #[test]
    fn digits_taka_danda() {
        assert_eq!(to_bijoy("০১২৩৪৫৬৭৮৯").unwrap(), "0123456789");
        assert_eq!(to_bijoy("৳").unwrap(), "$");
        assert_eq!(to_bijoy("।").unwrap(), "|");
        assert_eq!(from_bijoy("$").unwrap(), "৳");
        assert_eq!(from_bijoy("|").unwrap(), "।");
        assert_eq!(from_bijoy("0123456789").unwrap(), "০১২৩৪৫৬৭৮৯");
        assert_eq!(to_bijoy("৳১০।").unwrap(), "$10|");
    }

    #[test]
    fn empty_input_gives_empty_output() {
        assert_eq!(to_bijoy("").unwrap(), "");
        assert_eq!(from_bijoy("").unwrap(), "");
    }

    #[test]
    fn ascii_passthrough() {
        assert_eq!(to_bijoy("ABC 123").unwrap(), "ABC 123");
        assert_eq!(to_bijoy("a\nb").unwrap(), "a\nb");
        // Unmapped symbols survive both directions.
        assert_eq!(from_bijoy(" # ").unwrap(), " # ");
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
    fn no_panic_on_arbitrary_unicode() {
        for input in [
            "日本語テスト",
            "🎉🇧🇩👨‍👩‍👧",
            "مرحبا",
            "é́",
            "\u{200d}",
            "© ‡ ¯ ‹ Ô",
            "(a|b).*+?^$[]{}",
            "\\1 \\2",
            "কি©‡",
            "\n\t\r",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaকি",
        ] {
            assert!(to_bijoy(input).is_ok(), "to_bijoy({input:?})");
            assert!(from_bijoy(input).is_ok(), "from_bijoy({input:?})");
        }
    }

    #[test]
    fn conversion_is_deterministic() {
        let input = "আমার কি মা তুমি বাংলা ভালো ঢাকা স্কুল ক্ষ জ্ঞ ১২৩";
        assert_eq!(to_bijoy(input).unwrap(), to_bijoy(input).unwrap());
        let encoded = to_bijoy(input).unwrap();
        assert_eq!(from_bijoy(&encoded).unwrap(), from_bijoy(&encoded).unwrap());
    }

    #[test]
    fn round_trip_render_goldens_exactly() {
        // The legacy reverse table inverts all eight render goldens
        // exactly (the K → ক rewrite runs before the w([cluster]) pattern,
        // so wK cascades back to কি). Pinned: any table reorder breaks this.
        for (unicode, ansi) in render_goldens() {
            assert_eq!(from_bijoy(ansi).unwrap(), unicode, "round-trip {ansi:?}");
        }
    }

    #[test]
    fn reverse_basics() {
        // Longest forms win by table order: "Av" (আ) precedes "A" (অ).
        assert_eq!(from_bijoy("Av").unwrap(), "আ");
        assert_eq!(from_bijoy("A").unwrap(), "অ");
        assert_eq!(from_bijoy("g").unwrap(), "ম");
        assert_eq!(from_bijoy("K").unwrap(), "ক");
        // Line-start † decodes to ‡ + vowel (legacy ^‡ information loss);
        // ো itself does not recompose. Documented, not a bug.
        assert_eq!(from_bijoy("†v").unwrap(), "‡া");
    }

    #[test]
    fn legacy_backref_expansion_is_exact() {
        // \2© keeps group 2 plus the marker; \1\2 keeps both groups.
        assert_eq!(to_bijoy("র্ক").unwrap(), "K©");
        assert_eq!(to_bijoy("কর্তা").unwrap(), "KZ©v");
        assert_eq!(to_bijoy("শিক্ষা").unwrap(), "wk¶v");
    }

    #[test]
    fn invalid_rule_fails_with_typed_error() {
        let bad = vec![super::SutonnyRule {
            pattern: "(".to_owned(),
            replacement: "x".to_owned(),
        }];
        let err = super::compile_rules(&bad).unwrap_err();
        assert_eq!(err, 0);
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
}
