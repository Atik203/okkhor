use okkhor_core::Language;
use serde::{Deserialize, Serialize};

use crate::error::TransliterateError;

/// Expected value of [`RuleSet::schema`].
pub const RULE_SCHEMA: u32 = 1;

/// Default rule priority used when a rule omits the `priority` field.
const DEFAULT_PRIORITY: u16 = 100;

fn default_priority() -> u16 {
    DEFAULT_PRIORITY
}

/// Logical identifier for one transliteration rule.
///
/// Valid codes match `[a-z0-9_-]` with length 1..=32.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuleId(String);

impl RuleId {
    /// Validate `code` and wrap it, or return `None` when it is not a valid id.
    pub fn from_code(code: &str) -> Option<Self> {
        if code.is_empty()
            || code.len() > 32
            || !code
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        {
            return None;
        }
        Some(Self(code.to_owned()))
    }

    /// The underlying id string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One roman-to-Bangla transliteration rule as parsed from JSON.
///
/// Stored inputs/outputs are raw as parsed; [`crate::RuleTransliterator::new`]
/// applies `normalize_key`, ASCII/length/NFC/language validation, and engine
/// ordering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransliterationRule {
    pub id: RuleId,
    /// Raw ASCII roman prefix as parsed, 1..=32 bytes after normalization.
    pub input: String,
    /// Raw output as parsed, 1..=64 chars after NFC validation.
    pub output: String,
    /// Must be [`Language::Bangla`] in 0.3.
    pub language: Language,
    /// Lower wins on equal-length prefix matches. Default 100.
    #[serde(default = "default_priority")]
    pub priority: u16,
}

/// An ordered set of transliteration rules parsed from a JSON fixture.
///
/// Rules are kept in file order here; [`crate::RuleTransliterator::new`]
/// normalizes, validates, and sorts them into engine order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSet {
    pub schema: u32,
    /// Parsed in file order.
    pub rules: Vec<TransliterationRule>,
}

impl RuleSet {
    /// Parse a rule set from JSON.
    ///
    /// Owns schema checking (`schema == RULE_SCHEMA`, otherwise
    /// [`TransliterateError::UnsupportedSchema`]; a missing `schema` field
    /// reports `found = 0`), JSON shape deserialization, and [`RuleId`]
    /// syntax validation. Preserves file order and does NOT normalize, sort,
    /// dedup, or validate language, length, or NFC — that is owned by
    /// [`crate::RuleTransliterator::new`].
    pub fn from_json_str(json: &str) -> Result<Self, TransliterateError> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        let found = value
            .get("schema")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0);
        if found != RULE_SCHEMA {
            return Err(TransliterateError::UnsupportedSchema {
                found,
                expected: RULE_SCHEMA,
            });
        }
        let ruleset: Self = serde_json::from_value(value)?;
        for rule in &ruleset.rules {
            if RuleId::from_code(rule.id.as_str()).is_none() {
                return Err(TransliterateError::InvalidRule {
                    reason: "invalid_rule_id",
                });
            }
        }
        Ok(ruleset)
    }

    /// Number of rules in file order.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether the set holds no rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}
