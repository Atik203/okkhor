use thiserror::Error;

/// Errors from the transliteration rule engine.
///
/// All `Display` strings are static: they never embed user text, rule text, or
/// paths. The daemon logger may only record [`TransliterateError::kind`] and
/// [`TransliterateError::code`]; callers must not log `Debug` output or walk
/// `source()`, since `Json` can carry embedded text.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TransliterateError {
    #[error("rule JSON is invalid")]
    Json(#[from] serde_json::Error),
    #[error("unsupported rule schema {found}; expected {expected}")]
    UnsupportedSchema { found: u32, expected: u32 },
    #[error("invalid transliteration rule: {reason}")]
    InvalidRule { reason: &'static str },
    #[error("duplicate rule id")]
    DuplicateRuleId,
    #[error("rule input exceeds configured length")]
    InputTooLong,
    #[error("unsupported language for transliteration")]
    UnsupportedLanguage,
}

impl TransliterateError {
    /// Stable identifier for the daemon logger's allowlisted `kind` field.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Json(_) => "transliterate_json",
            Self::UnsupportedSchema { .. } => "transliterate_unsupported_schema",
            Self::InvalidRule { .. } => "transliterate_invalid_rule",
            Self::DuplicateRuleId => "transliterate_duplicate_rule",
            Self::InputTooLong => "transliterate_input_too_long",
            Self::UnsupportedLanguage => "transliterate_unsupported_language",
        }
    }

    /// OS error code where one exists, otherwise `None`.
    ///
    /// This crate performs no I/O, so this is always `None`.
    pub fn code(&self) -> Option<i32> {
        None
    }
}
