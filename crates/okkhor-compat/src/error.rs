//! [`CompatError`] for the `okkhor-compat` crate (contract §5.6).
//!
//! All `Display` strings are static: they never contain user text, fixture
//! contents, full paths, window titles, or proprietary asset bytes. The daemon
//! logger must use only [`CompatError::kind`] and [`CompatError::code`];
//! callers must never log `Debug` output or walk `source()`, since the
//! `#[from]` I/O and JSON variants can embed paths or text.

use thiserror::Error;

use crate::AdapterKind;

/// Errors from DLL path validation, adapter loading, and the loader seam.
///
/// Marked `#[non_exhaustive]` so new variants can be added without a breaking
/// change. No variant stores a path or any user-supplied text.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CompatError {
    /// The requested adapter was not compiled in via its Cargo feature.
    /// Returned before any path check or loader call.
    #[error("adapter is disabled by build features")]
    FeatureDisabled { adapter: AdapterKind },
    /// The requested adapter is not supported on this platform.
    /// (Loader calls only exist on Windows.)
    #[error("adapter is not supported on this platform")]
    UnsupportedPlatform,
    /// A DLL path failed policy validation. `reason` is a static,
    /// machine-readable code, never the offending path. Known codes:
    /// `not_absolute`, `unsupported_prefix`, `dot_component`, `bad_extension`,
    /// `wrong_file_name`, `unresolvable_path`, `metadata_failed`, `not_a_file`,
    /// `free_failed`.
    #[error("DLL path was rejected by policy: {reason}")]
    InvalidDllPath { reason: &'static str },
    /// The canonical DLL path is not under any allowlisted root.
    #[error("DLL path is outside the allowlisted roots")]
    PathOutsideAllowlist,
    /// The OS loader refused the validated DLL. `code` carries the raw OS
    /// error where one exists.
    #[error("DLL load failed")]
    LoadFailed {
        adapter: AdapterKind,
        code: Option<i32>,
    },
    /// A required export was not found in a loaded library.
    #[error("required export was not found")]
    MissingExport { adapter: AdapterKind },
    /// An adapter call exceeded its configured bound.
    #[error("adapter call exceeded its bound")]
    BoundExceeded { adapter: AdapterKind },
    /// An operation required a loaded adapter that is not loaded.
    #[error("adapter is not loaded")]
    NotLoaded { adapter: AdapterKind },
}

impl CompatError {
    /// Stable identifier for the daemon logger's allowlisted `kind` field.
    /// Lowercase snake_case, unique per variant.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::FeatureDisabled { .. } => "compat_feature_disabled",
            Self::UnsupportedPlatform => "compat_unsupported_platform",
            Self::InvalidDllPath { .. } => "compat_invalid_dll_path",
            Self::PathOutsideAllowlist => "compat_path_outside_allowlist",
            Self::LoadFailed { .. } => "compat_load_failed",
            Self::MissingExport { .. } => "compat_missing_export",
            Self::BoundExceeded { .. } => "compat_bound_exceeded",
            Self::NotLoaded { .. } => "compat_not_loaded",
        }
    }

    /// OS error code where one exists, otherwise `None`.
    pub fn code(&self) -> Option<i32> {
        match self {
            Self::LoadFailed { code, .. } => *code,
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_stable_snake_case() {
        let cases: Vec<(CompatError, &str)> = vec![
            (
                CompatError::FeatureDisabled {
                    adapter: AdapterKind::WordTrie,
                },
                "compat_feature_disabled",
            ),
            (
                CompatError::UnsupportedPlatform,
                "compat_unsupported_platform",
            ),
            (
                CompatError::InvalidDllPath {
                    reason: "not_absolute",
                },
                "compat_invalid_dll_path",
            ),
            (
                CompatError::PathOutsideAllowlist,
                "compat_path_outside_allowlist",
            ),
            (
                CompatError::LoadFailed {
                    adapter: AdapterKind::Caret,
                    code: Some(126),
                },
                "compat_load_failed",
            ),
            (
                CompatError::MissingExport {
                    adapter: AdapterKind::Translator,
                },
                "compat_missing_export",
            ),
            (
                CompatError::BoundExceeded {
                    adapter: AdapterKind::SentenceTrie,
                },
                "compat_bound_exceeded",
            ),
            (
                CompatError::NotLoaded {
                    adapter: AdapterKind::EmojiManager,
                },
                "compat_not_loaded",
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.kind(), expected);
        }
    }

    #[test]
    fn code_is_none_except_load_failed() {
        assert_eq!(
            CompatError::LoadFailed {
                adapter: AdapterKind::WordTrie,
                code: Some(126),
            }
            .code(),
            Some(126)
        );
        assert_eq!(
            CompatError::LoadFailed {
                adapter: AdapterKind::WordTrie,
                code: None,
            }
            .code(),
            None
        );
        assert_eq!(
            CompatError::FeatureDisabled {
                adapter: AdapterKind::WordTrie,
            }
            .code(),
            None
        );
        assert_eq!(CompatError::UnsupportedPlatform.code(), None);
        assert_eq!(
            CompatError::InvalidDllPath {
                reason: "not_absolute",
            }
            .code(),
            None
        );
        assert_eq!(CompatError::PathOutsideAllowlist.code(), None);
    }

    #[test]
    fn display_strings_are_static() {
        // Display must never interpolate paths or text: assert the exact strings.
        assert_eq!(
            CompatError::FeatureDisabled {
                adapter: AdapterKind::WordTrie,
            }
            .to_string(),
            "adapter is disabled by build features"
        );
        assert_eq!(
            CompatError::UnsupportedPlatform.to_string(),
            "adapter is not supported on this platform"
        );
        assert_eq!(
            CompatError::InvalidDllPath {
                reason: "not_absolute",
            }
            .to_string(),
            "DLL path was rejected by policy: not_absolute"
        );
        assert_eq!(
            CompatError::PathOutsideAllowlist.to_string(),
            "DLL path is outside the allowlisted roots"
        );
        assert_eq!(
            CompatError::LoadFailed {
                adapter: AdapterKind::WordTrie,
                code: Some(126),
            }
            .to_string(),
            "DLL load failed"
        );
        assert_eq!(
            CompatError::MissingExport {
                adapter: AdapterKind::WordTrie,
            }
            .to_string(),
            "required export was not found"
        );
        assert_eq!(
            CompatError::BoundExceeded {
                adapter: AdapterKind::WordTrie,
            }
            .to_string(),
            "adapter call exceeded its bound"
        );
        assert_eq!(
            CompatError::NotLoaded {
                adapter: AdapterKind::WordTrie,
            }
            .to_string(),
            "adapter is not loaded"
        );
    }
}
