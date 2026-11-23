#![forbid(unsafe_code)]
//! Protocol error codes for the 0.5 IPC API (contract §4).
//!
//! Every code serializes as its PascalCase name, e.g. `"InvalidLayout"`.
//! Messages paired with these codes are static text; they never include user
//! text, tokens, or paths.

use serde::{Deserialize, Serialize};

/// Typed error codes returned in `{"error":{"code":..,"message":..}}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorCode {
    /// A frame exceeded [`crate::MAX_FRAME_BYTES`].
    FrameTooLarge,
    /// The peer closed the connection mid-frame or a read timed out.
    Truncated,
    /// A request exceeded the per-request deadline.
    Timeout,
    /// Hello token missing, malformed, or wrong.
    Unauthorized,
    /// Hello protocol version is not [`crate::PROTOCOL_VERSION`].
    ProtocolMismatch,
    /// The top-level request key is not a known 0.5 method.
    UnknownMethod,
    /// The request was not a single-key object or its params failed to parse.
    InvalidRequest,
    /// `settings.schema` (or words file `schema`) is not 1.
    UnsupportedSchema,
    /// `layout` is not one of the known layout codes.
    InvalidLayout,
    /// `pause_hotkey` failed to parse.
    InvalidHotkey,
    /// Any other settings value failed validation.
    InvalidSettings,
    /// A word field failed validation (length, language, control characters).
    InvalidWord,
    /// A `(language, roman, output)` triple already exists.
    DuplicateWord,
    /// An import exceeds [`crate::MAX_IMPORT_ROWS`] rows or the store exceeds
    /// [`crate::MAX_WORDS_TOTAL`] words.
    TooManyWords,
    /// `text.insert` exceeds 512 UTF-16 code units.
    TextTooLong,
    /// An export payload would exceed [`crate::MAX_FRAME_BYTES`].
    ExportTooLarge,
    /// `ai.complete` in 0.5: accepted by the protocol, never executed.
    Unsupported,
    /// Catch-all for handler failures that must not leak detail.
    Internal,
}

impl ErrorCode {
    /// Static message for a code. Contains no user text, tokens, or paths.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::FrameTooLarge => "frame exceeds the 1 MiB limit",
            Self::Truncated => "connection closed or read timed out",
            Self::Timeout => "request exceeded the deadline",
            Self::Unauthorized => "missing or incorrect token",
            Self::ProtocolMismatch => "protocol version mismatch; restart Okkhor",
            Self::UnknownMethod => "unknown method",
            Self::InvalidRequest => "malformed request",
            Self::UnsupportedSchema => "unsupported schema version",
            Self::InvalidLayout => "unknown layout",
            Self::InvalidHotkey => "invalid hotkey",
            Self::InvalidSettings => "invalid settings value",
            Self::InvalidWord => "invalid word",
            Self::DuplicateWord => "duplicate word",
            Self::TooManyWords => "word cap exceeded",
            Self::TextTooLong => "text exceeds the insert limit",
            Self::ExportTooLarge => "export exceeds the size limit",
            Self::Unsupported => "not available in this milestone",
            Self::Internal => "internal error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_as_pascal_case() {
        for code in [
            ErrorCode::FrameTooLarge,
            ErrorCode::Truncated,
            ErrorCode::Timeout,
            ErrorCode::Unauthorized,
            ErrorCode::ProtocolMismatch,
            ErrorCode::UnknownMethod,
            ErrorCode::InvalidRequest,
            ErrorCode::UnsupportedSchema,
            ErrorCode::InvalidLayout,
            ErrorCode::InvalidHotkey,
            ErrorCode::InvalidSettings,
            ErrorCode::InvalidWord,
            ErrorCode::DuplicateWord,
            ErrorCode::TooManyWords,
            ErrorCode::TextTooLong,
            ErrorCode::ExportTooLarge,
            ErrorCode::Unsupported,
            ErrorCode::Internal,
        ] {
            let json = serde_json::to_string(&code).expect("code serializes");
            let back: ErrorCode = serde_json::from_str(&json).expect("code parses");
            assert_eq!(back, code);
        }
        assert_eq!(
            serde_json::to_string(&ErrorCode::InvalidLayout).expect("serializes"),
            "\"InvalidLayout\""
        );
    }

    #[test]
    fn messages_are_static_and_text_free() {
        // Every message must be a fixed string with no room for user text.
        assert!(!ErrorCode::Unauthorized.message().is_empty());
        assert!(!ErrorCode::Unsupported.message().is_empty());
    }
}
