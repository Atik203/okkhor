use thiserror::Error;

/// Errors from layout/encoding/dictionary import and id validation.
///
/// `Display` strings are static: they never contain user text, fixture
/// contents, or file paths. The daemon logger must use [`LangError::kind`]
/// and [`LangError::code`] only; callers must not log `Debug` output or walk
/// `source()`, because the `Io` and `Json` variants can embed paths or text.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LangError {
    /// A fixture file could not be read or its metadata could not be queried.
    #[error("fixture I/O failed")]
    Io(#[from] std::io::Error),
    /// A manifest or asset file is not syntactically valid JSON.
    ///
    /// Returned only when a single file is parsed directly (`manifest.json`
    /// via [`crate::import_fixture_dir`], or any file via
    /// [`crate::import_asset`]). Inside [`crate::import_fixture_dir`],
    /// per-asset syntax failures are recorded as
    /// [`crate::SkipReason::MalformedData`] skips instead.
    #[error("fixture JSON is invalid")]
    Json(#[from] serde_json::Error),
    /// The `schema` field is missing (`found = 0`) or does not equal
    /// [`crate::assets::FIXTURE_SCHEMA`].
    #[error("unsupported fixture schema {found}; expected {expected}")]
    UnsupportedSchema { found: u32, expected: u32 },
    /// A manifest path is absolute, escapes its fixture root (including via
    /// symlink/junction), or contains `.`, `..`, or separator tricks.
    #[error("fixture path was rejected")]
    UnsafePath,
    /// An asset id, mapping key, encoding code, or dictionary word is
    /// repeated and duplicates are not allowed.
    #[error("duplicate asset identifier")]
    DuplicateId,
    /// A fixture file exceeds the 8 MiB byte cap or an asset exceeds
    /// [`crate::ImportOptions::max_entries_per_asset`].
    #[error("asset exceeds configured entry limit")]
    LimitExceeded,
    /// A fixture parsed as JSON but failed semantic validation. `reason` is a
    /// static machine-readable code such as `"invalid_layout_id"`.
    #[error("asset contains malformed data: {reason}")]
    InvalidData { reason: &'static str },
}

impl LangError {
    /// Stable snake_case identifier for the daemon logger's allowlisted
    /// `kind` field. This is the only logging-safe projection of the error.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Io(_) => "lang_io",
            Self::Json(_) => "lang_invalid_json",
            Self::UnsupportedSchema { .. } => "lang_unsupported_schema",
            Self::UnsafePath => "lang_unsafe_path",
            Self::DuplicateId => "lang_duplicate_id",
            Self::LimitExceeded => "lang_limit_exceeded",
            Self::InvalidData { .. } => "lang_invalid_data",
        }
    }

    /// OS error code where one exists (`Io`), otherwise `None`.
    pub fn code(&self) -> Option<i32> {
        match self {
            Self::Io(error) => error.raw_os_error(),
            Self::Json(_)
            | Self::UnsupportedSchema { .. }
            | Self::UnsafePath
            | Self::DuplicateId
            | Self::LimitExceeded
            | Self::InvalidData { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LangError;

    #[test]
    fn kinds_are_stable_snake_case() {
        assert_eq!(
            LangError::UnsupportedSchema {
                found: 2,
                expected: 1
            }
            .kind(),
            "lang_unsupported_schema"
        );
        assert_eq!(LangError::UnsafePath.kind(), "lang_unsafe_path");
        assert_eq!(LangError::DuplicateId.kind(), "lang_duplicate_id");
        assert_eq!(LangError::LimitExceeded.kind(), "lang_limit_exceeded");
        assert_eq!(
            LangError::InvalidData { reason: "x" }.kind(),
            "lang_invalid_data"
        );
        assert_eq!(LangError::code(&LangError::UnsafePath), None);
    }

    #[test]
    fn display_is_static_and_leaks_no_path() {
        let errors = [
            LangError::UnsafePath,
            LangError::DuplicateId,
            LangError::LimitExceeded,
            LangError::InvalidData { reason: "x" },
            LangError::UnsupportedSchema {
                found: 9,
                expected: 1,
            },
        ];
        for error in errors {
            let text = format!("{error}");
            assert!(!text.contains("C:\\"), "leaks path: {text}");
            assert!(!text.contains('/'), "leaks path: {text}");
        }
    }
}
