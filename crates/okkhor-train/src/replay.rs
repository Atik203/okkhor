#![forbid(unsafe_code)]
//! Replay format v1: one JSON object per line, UTF-8, `\\n` terminated.
//!
//! ```json
//! {"schema":1,"case_id":"bn_0007","language":"bangla","layout":"avro",
//!  "prefix":"ভা","context":[],"expected":["ভালো"],
//!  "shown":["ভালো","ভাই"],"accepted":0,
//!  "candidates":["ভালো","ভাই","ভাত"]}
//! ```
//!
//! Rules: `schema == 1`; `case_id` matches `[a-z0-9_-]` 1..=64 and is unique
//! per file; `prefix` is 1..=255 bytes; `context` holds <= 8 words;
//! `expected` holds 1..=5 NFC strings; `shown`/`candidates` are non-empty,
//! bounded, NFC; `accepted` is `null` or a valid index into `shown`.

use std::collections::HashSet;

use okkhor_core::{Language, normalize_nfc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Expected value of [`ReplayCase::schema`].
pub const REPLAY_SCHEMA: u32 = 1;

const MAX_ID_LEN: usize = 64;
const MAX_PREFIX_BYTES: usize = 255;
const MAX_CONTEXT_WORDS: usize = 8;
const MAX_EXPECTED: usize = 5;
const MAX_SHOWN: usize = 32;
const MAX_CANDIDATES: usize = 64;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ReplayError {
    #[error("unsupported schema {found}; expected {expected}")]
    UnsupportedSchema { found: u32, expected: u32 },
    #[error("malformed replay line: {reason}")]
    Malformed { reason: &'static str },
    #[error("invalid case {case_id}: {reason}")]
    InvalidCase {
        case_id: String,
        reason: &'static str,
    },
    #[error("duplicate case id: {case_id}")]
    Duplicate { case_id: String },
}

/// One replay case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayCase {
    pub schema: u32,
    pub case_id: String,
    pub language: Language,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    pub prefix: String,
    #[serde(default)]
    pub context: Vec<String>,
    pub expected: Vec<String>,
    #[serde(default)]
    pub shown: Vec<String>,
    #[serde(default)]
    pub accepted: Option<usize>,
    #[serde(default)]
    pub candidates: Vec<String>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

impl ReplayCase {
    pub fn validate(&self) -> Result<(), ReplayError> {
        if self.schema != REPLAY_SCHEMA {
            return Err(ReplayError::UnsupportedSchema {
                found: self.schema,
                expected: REPLAY_SCHEMA,
            });
        }
        if !valid_id(&self.case_id) {
            return Err(ReplayError::InvalidCase {
                case_id: self.case_id.clone(),
                reason: "invalid_case_id",
            });
        }
        if self.prefix.is_empty() || self.prefix.len() > MAX_PREFIX_BYTES {
            return Err(ReplayError::InvalidCase {
                case_id: self.case_id.clone(),
                reason: "invalid_prefix",
            });
        }
        if self.context.len() > MAX_CONTEXT_WORDS {
            return Err(ReplayError::InvalidCase {
                case_id: self.case_id.clone(),
                reason: "too_many_context_words",
            });
        }
        if self.expected.is_empty() || self.expected.len() > MAX_EXPECTED {
            return Err(ReplayError::InvalidCase {
                case_id: self.case_id.clone(),
                reason: "invalid_expected_count",
            });
        }
        for text in &self.expected {
            if text.is_empty() || normalize_nfc(text) != *text {
                return Err(ReplayError::InvalidCase {
                    case_id: self.case_id.clone(),
                    reason: "expected_not_nfc",
                });
            }
        }
        if self.shown.is_empty() || self.shown.len() > MAX_SHOWN {
            return Err(ReplayError::InvalidCase {
                case_id: self.case_id.clone(),
                reason: "invalid_shown_count",
            });
        }
        for text in self.shown.iter().chain(self.candidates.iter()) {
            if text.is_empty() || normalize_nfc(text) != *text {
                return Err(ReplayError::InvalidCase {
                    case_id: self.case_id.clone(),
                    reason: "text_not_nfc",
                });
            }
        }
        if self.candidates.is_empty() || self.candidates.len() > MAX_CANDIDATES {
            return Err(ReplayError::InvalidCase {
                case_id: self.case_id.clone(),
                reason: "invalid_candidates_count",
            });
        }
        if let Some(index) = self.accepted {
            if index >= self.shown.len() {
                return Err(ReplayError::InvalidCase {
                    case_id: self.case_id.clone(),
                    reason: "accepted_out_of_range",
                });
            }
        }
        if self.layout.as_ref().is_some_and(|layout| !valid_id(layout)) {
            return Err(ReplayError::InvalidCase {
                case_id: self.case_id.clone(),
                reason: "invalid_layout",
            });
        }
        Ok(())
    }
}

/// Parse one JSONL line into a validated [`ReplayCase`].
pub fn parse_line(line: &str) -> Result<ReplayCase, ReplayError> {
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|_| ReplayError::Malformed {
            reason: "malformed_json",
        })?;
    let found = value
        .get("schema")
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0);
    if found != REPLAY_SCHEMA {
        return Err(ReplayError::UnsupportedSchema {
            found,
            expected: REPLAY_SCHEMA,
        });
    }
    let case: ReplayCase = serde_json::from_value(value).map_err(|_| ReplayError::Malformed {
        reason: "malformed_case",
    })?;
    case.validate()?;
    Ok(case)
}

/// Parse a whole JSONL document, enforcing unique `case_id`s.
pub fn read_replay_str(document: &str) -> Result<Vec<ReplayCase>, ReplayError> {
    let mut cases = Vec::new();
    let mut seen = HashSet::new();
    for (index, line) in document.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let case = parse_line(line).map_err(|error| match error {
            ReplayError::InvalidCase { case_id, reason } => ReplayError::InvalidCase {
                case_id: if case_id.is_empty() {
                    format!("line_{index}")
                } else {
                    case_id
                },
                reason,
            },
            other => other,
        })?;
        if !seen.insert(case.case_id.clone()) {
            return Err(ReplayError::Duplicate {
                case_id: case.case_id.clone(),
            });
        }
        cases.push(case);
    }
    if cases.is_empty() {
        return Err(ReplayError::Malformed {
            reason: "empty_replay",
        });
    }
    Ok(cases)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(case_id: &str) -> String {
        format!(
            "{{\"schema\":1,\"case_id\":\"{case_id}\",\"language\":\"bangla\",\"layout\":\"avro\",\"prefix\":\"ভা\",\"context\":[],\"expected\":[\"ভালো\"],\"shown\":[\"ভালো\"],\"accepted\":0,\"candidates\":[\"ভালো\"]}}"
        )
    }

    #[test]
    fn round_trip() {
        let case = parse_line(&line("bn_0001")).expect("valid");
        assert_eq!(case.case_id, "bn_0001");
        let back = serde_json::to_string(&case).expect("serialize");
        assert_eq!(parse_line(&back).expect("reparse"), case);
    }
}
