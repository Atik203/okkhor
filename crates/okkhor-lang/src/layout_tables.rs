//! Compile stored phonetic rule tables into the regex asset shape consumed by
//! `PhoneticTransliterator::from_asset_json`. Conversion happens once at
//! startup; the typed path only uses the compiled table.
//!
//! The stored tables use `{key, output}` where `key` is already a regex
//! pattern and `output` is the literal Bangla replacement. The transliterator
//! expects `{pattern, replacement}` plus a provenance envelope.

use serde_json::{Value, json};

use crate::fixed_layout::FixedLayoutError;

/// Stored Khipro phonetic rules (288 entries, public layout facts).
pub const KHIPRO_RULES: &str = include_str!("../assets/layouts/khipro-v1.json");
/// Stored Phonetic/Banglish phonetic rules (256 entries).
pub const PHONETIC_RULES: &str = include_str!("../assets/layouts/phonetic-v1.json");

/// Convert a stored `{key, output}` table into the transliterator's asset JSON.
///
/// `rule_count` is rechecked against the actual rule array so a truncated or
/// hand-edited asset fails at startup instead of silently changing typing.
/// Strip look-around groups that Rust's `regex` engine does not support.
///
/// The stored tables contain three such rules. Their guards only refine when
/// a punctuation character converts; the conversion itself is the same, so
/// `(?<!\d)\.(?!\d)` becomes `\.` and `(?<!/)/(?!/)` becomes `/`. Dropping
/// the whole rule would lose a real mapping; keeping the plain form preserves
/// it at the cost of a slightly broader match.
fn sanitize_pattern(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '(' && matches!(chars.peek(), Some('?')) {
            chars.next();
            let mut group = String::from("(?");
            let mut lookaround = false;
            for next in chars.by_ref() {
                group.push(next);
                if next == '<' || next == '=' || next == '!' {
                    lookaround = group.starts_with("(?<")
                        || group.starts_with("(?!")
                        || group.starts_with("(?=");
                }
                if next == ')' {
                    break;
                }
            }
            if lookaround {
                continue;
            }
            out.push_str(&group);
        } else {
            out.push(ch);
        }
    }
    out
}

pub fn layout_rules_to_asset_json(source: &str) -> Result<String, FixedLayoutError> {
    let table: Value = serde_json::from_str(source).map_err(|_| FixedLayoutError::InvalidJson)?;
    let rules = table
        .get("rules")
        .and_then(Value::as_array)
        .ok_or(FixedLayoutError::InvalidJson)?;
    let declared = table
        .get("rule_count")
        .and_then(Value::as_u64)
        .ok_or(FixedLayoutError::RuleCountMismatch)?;
    if declared != rules.len() as u64 {
        return Err(FixedLayoutError::RuleCountMismatch);
    }
    let name = table
        .get("name")
        .and_then(Value::as_str)
        .ok_or(FixedLayoutError::InvalidJson)?;
    let source_name = table.get("source").and_then(Value::as_str).unwrap_or(name);

    let converted: Vec<Value> = rules
        .iter()
        .map(|rule| {
            let pattern = rule
                .get("key")
                .and_then(Value::as_str)
                .ok_or(FixedLayoutError::InvalidJson)?;
            let replacement = rule
                .get("output")
                .and_then(Value::as_str)
                .ok_or(FixedLayoutError::InvalidJson)?;
            // An empty replacement is a deliberate deletion rule (Khipro has
            // one), so only an empty pattern is invalid here.
            let sanitized = sanitize_pattern(pattern);
            if sanitized.is_empty() {
                return Err(FixedLayoutError::EmptyOutput);
            }
            Ok(json!({ "pattern": sanitized, "replacement": replacement }))
        })
        .collect::<Result<Vec<_>, FixedLayoutError>>()?;

    serde_json::to_string(&json!({
        "name": name,
        "source": source_name,
        "rule_count": converted.len(),
        "rules": converted,
    }))
    .map_err(|_| FixedLayoutError::InvalidJson)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_tables_convert_to_the_regex_asset_shape() {
        for source in [KHIPRO_RULES, PHONETIC_RULES] {
            let converted = layout_rules_to_asset_json(source).expect("converted table");
            let value: Value = serde_json::from_str(&converted).expect("valid JSON");
            let rules = value["rules"].as_array().expect("rules");
            assert_eq!(value["rule_count"].as_u64(), Some(rules.len() as u64));
            assert!(rules.iter().all(|rule| rule.get("pattern").is_some()
                && rule.get("replacement").is_some()
                && rule.get("key").is_none()));
        }
    }

    #[test]
    fn khipro_conversion_keeps_the_golden_rule() {
        let converted = layout_rules_to_asset_json(KHIPRO_RULES).expect("converted table");
        assert!(converted.contains("nftff"));
        assert!(converted.contains("ণ্ঠ"));
    }

    #[test]
    fn lookaround_is_converted_to_a_plain_pattern() {
        assert_eq!(sanitize_pattern(r"(?<!\d)\.(?!\d)"), r"\.");
        assert_eq!(sanitize_pattern(r"(?<!/)/(?!/)"), "/");
        assert_eq!(sanitize_pattern(r"[of]|(?<!;);(?!;)"), "[of]|;");
        // Non-capturing groups are ordinary regex and must survive.
        assert_eq!(sanitize_pattern(r"(?:a)b"), r"(?:a)b");
    }

    #[test]
    fn malformed_tables_are_rejected() {
        assert!(layout_rules_to_asset_json("not json").is_err());
        assert!(
            layout_rules_to_asset_json(r#"{"name":"x","source":"x","rule_count":2,"rules":[]}"#)
                .is_err()
        );
    }
}
