//! Ordered regex transliteration pipeline for Avro Phonetic.
//!
//! The rule table is derived from the `keyMap` of the legacy
//! `Avro Phonetic (Custom)` layout (modified Avro Phonetic by Mehdi Hasan,
//! OmicronLab, adapted by Nabil for Nms Kontho). The data was extracted
//! read-only from the owner's licensed installation into
//! `assets/avro-v1.json` (ordered array, file order preserved); no legacy
//! code was copied or executed.
//!
//! Table semantics (confirmed against a prototype port of the table):
//! input is processed left-to-right, one whitespace-delimited chunk at a
//! time. At each position every rule is tried anchored at that position;
//! the longest match wins, with table order breaking ties. Context rules
//! such as `([কখ...])a` inspect the **last already-built output
//! character**, not the raw input: on a match the last output character is
//! replaced by the expanded replacement (so `ক` + `a` becomes `কা`). `^`
//! rules only match at chunk start. The matched input length is consumed.
//! Unmatched characters (punctuation gaps, digits already covered by the
//! table, emoji, Bengali input) pass through unchanged.
//!
//! This means lowercase `o` after a consonant is the inherent vowel (it is
//! deleted: `bhalo` → ভাল), while capital `O` is the ো-kar (`bhalO` →
//! ভালো). Likewise `gy` → গ্য (জ্ঞ comes from `gg`/`jNG`). These are Avro
//! behaviors, not bugs; dictionary correction for words like আপনি/ধন্যবাদ
//! is a later task and must not be folded into this table.
//!
//! Matching itself uses the `regex` crate (character classes, quantifiers,
//! escaped literals, anchors, capture groups). Rule compilation happens
//! once at load; conversion performs no I/O.
//!
//! This module is additive: [`crate::RuleTransliterator`] and the schema-1
//! fixtures are untouched.

use std::sync::OnceLock;

use okkhor_core::Language;
use regex::{Captures, Regex};
use serde::Deserialize;

use crate::engine::{TransliterationOptions, TransliterationOutput, TransliteratorEngine};
use crate::error::TransliterateError;
use crate::rules::RuleId;

/// Hard cap on roman input length (chars). Longer input yields no output.
pub const MAX_PHONETIC_INPUT_CHARS: usize = 64;
/// Hard cap on transliterated output length (chars).
pub const MAX_PHONETIC_OUTPUT_CHARS: usize = 256;
/// Safety cap on compiled rules; the bundled table holds 250.
pub const MAX_PHONETIC_RULES: usize = 4096;
/// Confidence for the deterministic table output.
pub const PHONETIC_CONFIDENCE: f32 = 0.95;

/// Embedded ordered rule table (`assets/avro-v1.json`).
const EMBEDDED_JSON: &str = include_str!("../assets/avro-v1.json");

/// One row of the asset file; order in the file is application order.
#[derive(Debug, Deserialize)]
struct AssetRule {
    pattern: String,
    replacement: String,
}

/// Asset file envelope. `rule_count` is validated against `rules.len()`.
/// `name`/`source` are provenance only (kept so the file stays
/// self-describing); they are intentionally not used at runtime.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AssetFile {
    name: String,
    source: String,
    rule_count: usize,
    rules: Vec<AssetRule>,
}

/// How one match option is anchored against the input/output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchMode {
    /// Match the option against the remaining input at this position.
    Input,
    /// Match the option against (last output char + remaining input).
    /// The option starts with a group (`([..])`, `(.)`) that consumes the
    /// last output character.
    OutputPrefixed,
    /// The option is `rr([..])`: the input must start with `rr` and the
    /// last output character must be in the option's class.
    RrPrefixed,
}

/// One top-level alternative of a table pattern, with its own anchor mode.
#[derive(Debug, Clone)]
struct MatchOption {
    regex: Regex,
    mode: MatchMode,
    /// Option contains an unescaped `^`: only valid at chunk start.
    start_only: bool,
}

/// One table row in application order with its converted replacement.
#[derive(Debug, Clone)]
struct CompiledRule {
    options: Vec<MatchOption>,
    /// Replacement in `regex`-crate syntax (`$1`, `$$`).
    replacement: String,
}

/// First-byte dispatch: `(rule index, option index)` lists keyed by the
/// first input byte an option can match. Matching only ever consults the
/// bucket for `input[pos]` (plus the always-tried fallback), so conversion
/// costs a handful of regex probes per character instead of one per rule.
/// Buckets are built in table order, preserving longest-match-first with
/// table-order tie-breaking.
#[derive(Debug, Clone, Default)]
struct Dispatch {
    buckets: Vec<Vec<(usize, usize)>>,
    fallback: Vec<(usize, usize)>,
}

impl Dispatch {
    fn build(rules: &[CompiledRule], firsts: &[Vec<Option<u8>>]) -> Self {
        let mut buckets: Vec<Vec<(usize, usize)>> = vec![Vec::new(); 256];
        let mut fallback = Vec::new();
        for (rule_idx, rule) in rules.iter().enumerate() {
            for (opt_idx, _) in rule.options.iter().enumerate() {
                let entry = (rule_idx, opt_idx);
                match firsts[rule_idx][opt_idx] {
                    Some(b) => buckets[b as usize].push(entry),
                    None => fallback.push(entry),
                }
            }
        }
        Self { buckets, fallback }
    }
}

/// First input byte `option` can consume, if statically known.
///
/// - `rr(` options consume a literal `rr` from the input.
/// - Output-prefixed options (`(...)...`) consume their suffix from the
///   input; the group matches the previous output character.
/// - Other options consume their own leading literal (after `^` and after
///   unescaping `\X` to `X`).
fn option_first_byte(option: &str, mode: MatchMode) -> Option<u8> {
    let text = match mode {
        MatchMode::RrPrefixed => return Some(b'r'),
        MatchMode::OutputPrefixed => {
            let close = option.find(')')?;
            option.get(close + 1..)?
        }
        MatchMode::Input => option.strip_prefix('^').unwrap_or(option),
    };
    first_byte_of(text)
}

/// First byte of a literal run, resolving one `\X` escape.
fn first_byte_of(text: &str) -> Option<u8> {
    let mut chars = text.chars();
    let mut first = chars.next()?;
    if first == '\\' {
        first = chars.next()?;
    }
    let mut buf = [0u8; 4];
    Some(first.encode_utf8(&mut buf).as_bytes()[0])
}

/// Convert table replacement syntax (`\1`..`\9` captures) to `regex`-crate
/// syntax (`$1`..`$9`), escaping literal `$` first.
fn convert_replacement(table: &str) -> String {
    let mut out = String::with_capacity(table.len() + 4);
    let mut chars = table.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '$' => out.push_str("$$"),
            '\\' => {
                let mut rest = chars.clone();
                match rest.next() {
                    Some(d) if d.is_ascii_digit() => {
                        chars.next();
                        out.push('$');
                        out.push(d);
                    }
                    _ => out.push('\\'),
                }
            }
            _ => out.push(ch),
        }
    }
    out
}

/// Expand a converted replacement using `capture(i)` for `$i`
/// (`$$` yields `$`; an unset group yields `""`).
fn expand_replacement(converted: &str, capture: &dyn Fn(usize) -> Option<String>) -> String {
    let mut out = String::with_capacity(converted.len() + 8);
    let mut chars = converted.chars();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            out.push(ch);
            continue;
        }
        match chars.clone().next() {
            Some('$') => {
                chars.next();
                out.push('$');
            }
            Some(d) if d.is_ascii_digit() => {
                chars.next();
                let mut num = (d as u8 - b'0') as usize;
                while let Some(e) = chars.clone().next() {
                    if !e.is_ascii_digit() {
                        break;
                    }
                    chars.next();
                    num = num
                        .saturating_mul(10)
                        .saturating_add((e as u8 - b'0') as usize);
                }
                if let Some(text) = capture(num) {
                    out.push_str(&text);
                }
            }
            _ => out.push('$'),
        }
    }
    out
}

/// Expand with regex captures (unset groups yield `""`).
fn expand_captures(converted: &str, caps: &Captures<'_>) -> String {
    expand_replacement(converted, &|i| caps.get(i).map(|m| m.as_str().to_owned()))
}

/// Split a pattern on top-level `|` (ignoring `\|` escapes and `|` nested
/// inside `(...)` or `[...]`).
fn split_alternatives(pattern: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth_paren = 0usize;
    let mut depth_bracket = 0usize;
    let mut escaped = false;
    let mut start = 0usize;
    for (i, ch) in pattern.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '(' if depth_bracket == 0 => depth_paren += 1,
            ')' if depth_bracket == 0 && depth_paren > 0 => depth_paren -= 1,
            '[' if depth_paren == 0 => depth_bracket += 1,
            ']' if depth_paren == 0 && depth_bracket > 0 => depth_bracket -= 1,
            '|' if depth_paren == 0 && depth_bracket == 0 => {
                parts.push(&pattern[start..i]);
                start = i + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&pattern[start..]);
    parts
}

/// Whether `option` contains an unescaped `^` anchor.
fn has_anchor(option: &str) -> bool {
    let mut escaped = false;
    for ch in option.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
        } else if ch == '^' {
            return true;
        }
    }
    false
}

/// Compile one top-level alternative of a table pattern.
fn compile_option(option: &str) -> Result<MatchOption, TransliterateError> {
    let mode = if option.starts_with("rr(") {
        MatchMode::RrPrefixed
    } else if option.starts_with('(') {
        MatchMode::OutputPrefixed
    } else {
        MatchMode::Input
    };
    let anchored = format!("^(?:{option})");
    let regex = Regex::new(&anchored).map_err(|_| TransliterateError::InvalidPattern)?;
    Ok(MatchOption {
        regex,
        mode,
        start_only: has_anchor(option),
    })
}

/// Ordered regex transliteration engine (Avro phonetic table).
///
/// Owns the compiled rule order. `transliterate` is pure, deterministic,
/// and infallible: overlong or blank input yields an empty `Vec`.
#[derive(Debug, Clone)]
pub struct PhoneticTransliterator {
    rules: Vec<CompiledRule>,
    dispatch: Dispatch,
}

impl PhoneticTransliterator {
    /// Compile rules from `(pattern, replacement)` pairs in order.
    ///
    /// Fails with [`TransliterateError::InvalidPattern`] on the first
    /// pattern that does not compile. Accepts at most
    /// [`MAX_PHONETIC_RULES`] rules.
    pub fn from_patterns<I>(patterns: I) -> Result<Self, TransliterateError>
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut rules: Vec<CompiledRule> = Vec::new();
        let mut firsts: Vec<Vec<Option<u8>>> = Vec::new();
        for (pattern, replacement) in patterns {
            if rules.len() >= MAX_PHONETIC_RULES {
                return Err(TransliterateError::InvalidRule {
                    reason: "too_many_phonetic_rules",
                });
            }
            let mut options = Vec::new();
            let mut option_firsts = Vec::new();
            for alternative in split_alternatives(&pattern) {
                let option = compile_option(alternative)?;
                option_firsts.push(option_first_byte(alternative, option.mode));
                options.push(option);
            }
            if options.is_empty() {
                return Err(TransliterateError::InvalidRule {
                    reason: "empty_phonetic_pattern",
                });
            }
            firsts.push(option_firsts);
            rules.push(CompiledRule {
                options,
                replacement: convert_replacement(&replacement),
            });
        }
        if rules.is_empty() {
            return Err(TransliterateError::InvalidRule {
                reason: "empty_phonetic_table",
            });
        }
        let dispatch = Dispatch::build(&rules, &firsts);
        Ok(Self { rules, dispatch })
    }

    /// Load a rule table from an asset JSON string (`avro-v1.json` shape).
    pub fn from_asset_json(json: &str) -> Result<Self, TransliterateError> {
        let asset: AssetFile = serde_json::from_str(json)?;
        if asset.rule_count != asset.rules.len() {
            return Err(TransliterateError::InvalidRule {
                reason: "phonetic_rule_count_mismatch",
            });
        }
        Self::from_patterns(asset.rules.into_iter().map(|r| (r.pattern, r.replacement)))
    }

    /// Compile the embedded Avro table. Rule compilation happens here, off
    /// the typing path; conversion itself performs no I/O.
    pub fn from_embedded_avro() -> Result<Self, TransliterateError> {
        Self::from_asset_json(EMBEDDED_JSON)
    }

    /// Process-wide shared instance compiled once via [`OnceLock`].
    ///
    /// Returns a shared reference so failed compilations can be reported
    /// without cloning the error. The bundled table is covered by a load
    /// test.
    pub fn embedded() -> &'static Result<Self, TransliterateError> {
        static CELL: OnceLock<Result<PhoneticTransliterator, TransliterateError>> = OnceLock::new();
        CELL.get_or_init(Self::from_embedded_avro)
    }

    /// Number of compiled rules in application order.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Ranked candidates for `input`. The table pipeline is deterministic,
    /// so this yields at most one output: the table form.
    pub fn suggest(
        &self,
        input: &str,
        options: &TransliterationOptions,
    ) -> Vec<TransliterationOutput> {
        if options.max_candidates == 0 || input.len() >= 256 {
            return Vec::new();
        }
        if input.chars().count() > MAX_PHONETIC_INPUT_CHARS {
            return Vec::new();
        }
        if input.trim().is_empty() {
            return Vec::new();
        }
        let mut out = String::with_capacity(input.len().saturating_mul(3).min(1024));
        // Whitespace-delimited chunks so `^` anchors mean word start and
        // spacing is preserved exactly.
        for chunk in input.split_inclusive(|c: char| c.is_whitespace()) {
            let boundary = chunk
                .find(|c: char| c.is_whitespace())
                .unwrap_or(chunk.len());
            // `boundary` from `find` is always a character boundary.
            let (core, space) = chunk.split_at(boundary);
            if !core.is_empty() {
                out.push_str(&self.convert_word(core));
            }
            out.push_str(space);
        }
        if out.is_empty() {
            return Vec::new();
        }
        // Hard output cap at a character boundary (never reached for
        // <=64-char input; a safety bound, not a shaping step).
        if out.chars().count() > MAX_PHONETIC_OUTPUT_CHARS {
            let cut = out
                .char_indices()
                .nth(MAX_PHONETIC_OUTPUT_CHARS)
                .map(|(i, _)| i)
                .unwrap_or(out.len());
            out.truncate(cut);
        }
        // Static id: the literal is a valid `[a-z0-9_-]` code by inspection.
        let rule_id = RuleId::from_code("avro-main").expect("static rule id is valid");
        vec![TransliterationOutput {
            text: out,
            language: Language::Bangla,
            confidence: PHONETIC_CONFIDENCE,
            rule_id,
        }]
    }

    /// Convert one whitespace-free chunk left-to-right.
    ///
    /// At each position the longest matching rule wins (table order breaks
    /// ties); context rules inspect the last output character. Every match
    /// consumes at least one input character, so this always terminates.
    fn convert_word(&self, word: &str) -> String {
        // Bounded: `word` is at most the 64-char input.
        let mut out = String::with_capacity(word.len().saturating_mul(3).min(512));
        let mut pos = 0usize;
        // Last consumed *input* character. Avro's `r`-after-consonant rule is a
        // ra-phala; after a vowel it must stay a plain `র`, otherwise `kor…`
        // renders as `ক্র…` while the dictionary word has not matched yet.
        let mut prev_input: Option<char> = None;
        while pos < word.len() {
            if let Some((consumed, replacement)) = self.match_at(word, pos, &out, prev_input) {
                debug_assert!(consumed > 0);
                if consumed == 0 {
                    break;
                }
                // Pop the last output char when the match consumed it as a
                // context prefix; the expanded replacement carries it.
                if replacement.pop_context {
                    out.pop();
                }
                out.push_str(&replacement.text);
                let consumed_text = &word[pos..pos + consumed];
                prev_input = consumed_text.chars().next_back();
                pos += consumed;
                if out.len() > 4096 {
                    break;
                }
            } else {
                // Passthrough for characters the table does not cover.
                // `pos` is always a character boundary (matches only ever
                // consume ASCII pattern text), so this cannot panic.
                let ch = word[pos..].chars().next().unwrap_or('\u{FFFD}');
                out.push(ch);
                prev_input = Some(ch);
                pos += ch.len_utf8();
            }
        }
        out
    }

    /// Try the bucketed rules anchored at `pos`; longest consumption wins,
    /// table order breaks ties. Returns consumed input bytes plus the
    /// expanded replacement and whether it absorbed the last output char.
    fn match_at(
        &self,
        word: &str,
        pos: usize,
        out: &str,
        prev_input: Option<char>,
    ) -> Option<(usize, Applied)> {
        let rest = &word[pos..];
        let last = out.chars().next_back();
        let mut best: Option<(usize, Applied, usize, usize)> = None;
        // Candidate options come only from this input byte's bucket (plus
        // the fallback list), each in table order.
        if let Some(bucket) = word
            .as_bytes()
            .get(pos)
            .map(|b| &self.dispatch.buckets[*b as usize])
        {
            for &(rule_idx, opt_idx) in bucket.iter().chain(self.dispatch.fallback.iter()) {
                Self::consider(
                    &self.rules,
                    rule_idx,
                    opt_idx,
                    pos,
                    rest,
                    last,
                    prev_input,
                    &mut best,
                );
            }
        }
        best.map(|(consumed, applied, _, _)| (consumed, applied))
    }

    /// Try one candidate option; keep it when it beats the running best.
    #[allow(clippy::too_many_arguments)]
    fn consider(
        rules: &[CompiledRule],
        rule_idx: usize,
        opt_idx: usize,
        pos: usize,
        rest: &str,
        last: Option<char>,
        prev_input: Option<char>,
        best: &mut Option<(usize, Applied, usize, usize)>,
    ) {
        let rule = &rules[rule_idx];
        let option = &rule.options[opt_idx];
        if option.start_only && pos != 0 {
            return;
        }
        let found: Option<(usize, String)> = match option.mode {
            MatchMode::Input => {
                let caps = match option.regex.captures(rest) {
                    Some(caps) => caps,
                    None => return,
                };
                let m = match caps.get(0) {
                    Some(m) => m,
                    None => return,
                };
                if m.start() != 0 || m.end() == 0 {
                    None
                } else {
                    Some((m.end(), expand_captures(&rule.replacement, &caps)))
                }
            }
            MatchMode::OutputPrefixed => {
                let last = match last {
                    Some(last) => last,
                    None => return,
                };
                let mut synthetic = String::with_capacity(last.len_utf8() + rest.len());
                synthetic.push(last);
                synthetic.push_str(rest);
                let caps = match option.regex.captures(&synthetic) {
                    Some(caps) => caps,
                    None => return,
                };
                let m = match caps.get(0) {
                    Some(m) => m,
                    None => return,
                };
                let consumed = match m.end().checked_sub(last.len_utf8()) {
                    Some(consumed) => consumed,
                    None => return,
                };
                if m.start() != 0 || consumed == 0 {
                    None
                } else {
                    Some((consumed, expand_captures(&rule.replacement, &caps)))
                }
            }
            MatchMode::RrPrefixed => {
                let last = match last {
                    Some(last) => last,
                    None => return,
                };
                if !rest.starts_with("rr") {
                    return;
                }
                let mut synthetic = String::with_capacity(2 + last.len_utf8());
                synthetic.push_str("rr");
                synthetic.push(last);
                let caps = match option.regex.captures(&synthetic) {
                    Some(caps) => caps,
                    None => return,
                };
                let m = match caps.get(0) {
                    Some(m) => m,
                    None => return,
                };
                if m.start() != 0 || m.end() != synthetic.len() {
                    None
                } else {
                    Some((2, expand_captures(&rule.replacement, &caps)))
                }
            }
        };
        if let Some((consumed, text)) = found {
            // Avro: a consonant followed by `r` is ra-phala only when the `r`
            // does not follow a vowel. `kor…` must stay `কর…`, not `ক্র…`.
            if is_vowel(prev_input) && text.contains("\u{09CD}\u{09B0}") {
                return;
            }
            let pop_context = option.mode != MatchMode::Input;
            // Buckets iterate in table order, so a strictly-longer test
            // keeps the earliest rule on ties; the tuple check states the
            // same intent explicitly.
            let replace = best.as_ref().is_none_or(|b| {
                consumed > b.0 || (consumed == b.0 && (rule_idx, opt_idx) < (b.2, b.3))
            });
            if replace {
                *best = Some((consumed, Applied { text, pop_context }, rule_idx, opt_idx));
            }
        }
    }
}

/// An expanded replacement plus whether it absorbed the last output char.
struct Applied {
    text: String,
    pop_context: bool,
}

impl TransliteratorEngine for PhoneticTransliterator {
    fn transliterate(
        &self,
        input: &str,
        options: &TransliterationOptions,
    ) -> Vec<TransliterationOutput> {
        self.suggest(input, options)
    }
}


/// True for the roman vowels Avro treats as syllable carriers.
fn is_vowel(input: Option<char>) -> bool {
    matches!(
        input,
        Some('a' | 'e' | 'i' | 'o' | 'u' | 'A' | 'E' | 'I' | 'O' | 'U')
    )
}

#[cfg(test)]
mod reph_tests {
    use super::*;

    #[test]
    fn ra_phala_does_not_follow_a_vowel() {
        let engine = PhoneticTransliterator::from_embedded_avro().expect("table");
        let options = TransliterationOptions::default();
        for (input, expected) in [
            ("korte", "\u{0995}\u{09b0}\u{09a4}\u{09c7}"),
            ("kortechi", "\u{0995}\u{09b0}\u{09a4}\u{09c7}\u{099b}\u{09bf}"),
            ("korchi", "\u{0995}\u{09b0}\u{099b}\u{09bf}"),
            ("kormo", "\u{0995}\u{09b0}\u{09ae}"),
        ] {
            let out = engine.transliterate(input, &options);
            assert_eq!(
                out.first().map(|row| row.text.as_str()),
                Some(expected),
                "{input}"
            );
        }
    }
}
