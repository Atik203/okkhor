//! Fallible-construction errors for the engine.
//!
//! `suggest` never constructs or returns an error. These variants exist only
//! for startup-time construction and golden-corpus loading. `Display` strings
//! are static: they never contain user text, fixture content, or full paths.
//! Callers must log only [`EngineError::kind`] and [`EngineError::code`];
//! never `Debug` formatting and never `source()`.

use thiserror::Error;

/// Construction- and corpus-loading failures.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EngineError {
    /// Any budget field is 0. Returned by `EngineConfig::new` and
    /// `CandidateEngine::new`.
    #[error("engine budget is invalid")]
    InvalidBudget,
    /// Reserved for a future owned-handle constructor. The borrowed `new`
    /// never produces it.
    #[error("engine component is missing")]
    MissingComponent,
    /// Golden-corpus `schema` mismatch; a missing field reports `found = 0`.
    #[error("unsupported golden schema {found}; expected {expected}")]
    UnsupportedSchema { found: u32, expected: u32 },
    /// Golden-corpus shape failure with a static reason string.
    #[error("golden corpus data is malformed: {reason}")]
    InvalidGolden { reason: &'static str },
}

impl EngineError {
    /// Stable identifier for the daemon logger allowlist.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidBudget => "engine_invalid_budget",
            Self::MissingComponent => "engine_missing_component",
            Self::UnsupportedSchema { .. } => "engine_unsupported_schema",
            Self::InvalidGolden { .. } => "engine_invalid_golden",
        }
    }

    /// Always `None` in this crate: no OS call underlies any variant.
    pub fn code(&self) -> Option<i32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::EngineError;

    #[test]
    fn kinds_are_documented_snake_case() {
        assert_eq!(EngineError::InvalidBudget.kind(), "engine_invalid_budget");
        assert_eq!(
            EngineError::MissingComponent.kind(),
            "engine_missing_component"
        );
        assert_eq!(
            EngineError::UnsupportedSchema {
                found: 2,
                expected: 1
            }
            .kind(),
            "engine_unsupported_schema"
        );
        assert_eq!(
            EngineError::InvalidGolden {
                reason: "malformed_json"
            }
            .kind(),
            "engine_invalid_golden"
        );
    }

    #[test]
    fn code_is_always_none() {
        for error in [
            EngineError::InvalidBudget,
            EngineError::MissingComponent,
            EngineError::UnsupportedSchema {
                found: 0,
                expected: 1,
            },
            EngineError::InvalidGolden {
                reason: "malformed_json",
            },
        ] {
            assert_eq!(error.code(), None);
        }
    }

    #[test]
    fn display_is_static() {
        // Static strings only: formatting must never echo caller input.
        assert_eq!(
            format!("{}", EngineError::InvalidBudget),
            "engine budget is invalid"
        );
        assert_eq!(
            format!("{}", EngineError::MissingComponent),
            "engine component is missing"
        );
        assert_eq!(
            format!(
                "{}",
                EngineError::UnsupportedSchema {
                    found: 2,
                    expected: 1
                }
            ),
            "unsupported golden schema 2; expected 1"
        );
        let display = format!(
            "{}",
            EngineError::InvalidGolden {
                reason: "duplicate_case_id"
            }
        );
        assert_eq!(
            display,
            "golden corpus data is malformed: duplicate_case_id"
        );
        assert!(!display.contains("secret-prefix"));
    }
}
