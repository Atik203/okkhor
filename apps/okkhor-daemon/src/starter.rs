//! Curated starter roman-to-Bangla suggestions.
//!
//! The Avro rule table returns one deterministic form per keystroke, which is
//! correct for most words but leaves the popup with a single row and no way to
//! correct common spellings. This module supplies a small, reviewed starter
//! list: an exact match is offered ahead of the rule output (for example
//! `dhonnobad` -> ধন্যবাদ) and prefix matches fill the remaining rows.
//!
//! Data is static and embedded (`assets/suggestions/bn-starter-v1.json`), so
//! the suggestion path performs no I/O, allocates only the strings it returns,
//! and never touches user text outside the in-memory composition buffer. The
//! list is intentionally small for the 0.5 beta; the imported Avro dictionary
//! replaces it in 0.6. Unknown or malformed data degrades to no starter
//! suggestions - the rule output still works.

use std::sync::OnceLock;

/// One curated roman key and its Bangla form. Order in the JSON file is the
/// priority order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StarterEntry {
    pub roman: String,
    pub output: String,
}

/// Longest roman key accepted from the data file.
const MAX_ROMAN_BYTES: usize = 64;
/// Longest Bangla output accepted from the data file (UTF-8 bytes).
const MAX_OUTPUT_BYTES: usize = 160;
/// Prefix suggestions need at least this many typed ASCII letters; shorter
/// buffers would match almost everything and make the list noise.
const MIN_PREFIX_BYTES: usize = 2;

const RAW: &str = include_str!("../assets/suggestions/bn-starter-v1.json");

/// Parse the embedded list once. Malformed rows are skipped; a malformed
/// document yields an empty list rather than a panic.
fn entries() -> &'static [StarterEntry] {
    static ENTRIES: OnceLock<Vec<StarterEntry>> = OnceLock::new();
    ENTRIES.get_or_init(|| parse(RAW))
}

fn parse(raw: &str) -> Vec<StarterEntry> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    let Some(rows) = value.get("entries").and_then(|entries| entries.as_array()) else {
        return Vec::new();
    };
    let mut parsed = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(roman) = row.get("roman").and_then(|value| value.as_str()) else {
            continue;
        };
        let Some(output) = row.get("output").and_then(|value| value.as_str()) else {
            continue;
        };
        let roman = roman.trim();
        let output = output.trim();
        if roman.is_empty()
            || roman.len() > MAX_ROMAN_BYTES
            || !roman.bytes().all(|byte| byte.is_ascii_lowercase())
            || output.is_empty()
            || output.len() > MAX_OUTPUT_BYTES
        {
            continue;
        }
        parsed.push(StarterEntry {
            roman: roman.to_owned(),
            output: output.to_owned(),
        });
    }
    parsed
}

/// Load and validate the list before the first keystroke (startup only).
pub fn warm() {
    let _ = entries();
}

/// First curated entry whose roman key equals `buffer` exactly.
pub fn exact(buffer: &str) -> Option<String> {
    let buffer = buffer.trim().to_ascii_lowercase();
    if buffer.is_empty() {
        return None;
    }
    entries()
        .iter()
        .find(|entry| entry.roman == buffer)
        .map(|entry| entry.output.clone())
}

/// Prefix completions, excluding exact matches and duplicate outputs, in file
/// priority order. Returns at most `limit` strings.
pub fn prefix(buffer: &str, limit: usize) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let buffer = buffer.trim().to_ascii_lowercase();
    if buffer.len() < MIN_PREFIX_BYTES {
        return Vec::new();
    }
    let mut results: Vec<String> = Vec::with_capacity(limit);
    for entry in entries() {
        if entry.roman == buffer || !entry.roman.starts_with(&buffer) {
            continue;
        }
        if results.iter().any(|existing| existing == &entry.output) {
            continue;
        }
        results.push(entry.output.clone());
        if results.len() >= limit {
            break;
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::{MIN_PREFIX_BYTES, entries, exact, parse, prefix};

    #[test]
    fn embedded_list_is_well_formed_and_ordered() {
        let list = entries();
        assert!(list.len() >= 100, "starter list should be useful");
        assert!(list.iter().all(|entry| !entry.roman.is_empty()));
        assert!(
            list.iter()
                .all(|entry| entry.roman.bytes().all(|byte| byte.is_ascii_lowercase()))
        );
        // Priority order: the first entry is the most common word family.
        assert_eq!(list[0].roman, "amar");
    }

    #[test]
    fn exact_match_corrects_common_spellings() {
        assert_eq!(exact("bhalo").as_deref(), Some("ভালো"));
        assert_eq!(exact("dhonnobad").as_deref(), Some("ধন্যবাদ"));
        assert_eq!(exact("korcho").as_deref(), Some("করছো"));
        // Case and surrounding whitespace are normalized.
        assert_eq!(exact("  Bhalo ").as_deref(), Some("ভালো"));
        assert_eq!(exact("notaword"), None);
    }

    #[test]
    fn prefix_needs_two_letters_and_dedups_outputs() {
        assert_eq!(MIN_PREFIX_BYTES, 2);
        assert!(prefix("k", 5).is_empty());
        let k = prefix("ko", 8);
        assert!(!k.is_empty());
        assert!(k.len() <= 8);
        // করা/করে/করতে/করবো... every entry starts with the typed key.
        assert!(k.iter().any(|word| word == "করে"));
        // roman variants (bhalo/valo) share one output.
        let bh = prefix("bhal", 10);
        assert_eq!(bh.iter().filter(|word| word.as_str() == "ভালো").count(), 1);
    }

    #[test]
    fn malformed_rows_and_documents_are_skipped() {
        let parsed = parse(
            r#"{"entries":[
                {"roman":"amar","output":"আমার"},
                {"roman":"","output":"খালি"},
                {"roman":"UP","output":"বড়"},
                {"roman":"ki","output":""},
                {"roman":"ki"}
            ]}"#,
        );
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].roman, "amar");
        assert!(parse("not json").is_empty());
        assert!(parse(r#"{"entries":{}}"#).is_empty());
    }
}
