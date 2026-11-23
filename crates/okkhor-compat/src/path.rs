//! DLL path validation (contract §5.3).
//!
//! No `unsafe` in this module. Validation runs before any OS loader call and
//! never derives the allowlist from the current working directory, `PATH`, or
//! any untrusted environment variable: the daemon supplies explicit
//! allowlisted roots.
//!
//! Validation rules, in order:
//!
//! 1. `adapter.is_enabled()` is true (feature gate first, cheapest check).
//! 2. The path is absolute; on Windows only a disk or UNC prefix is accepted.
//! 3. No `.` or `..` component in the input path.
//! 4. The extension is `dll` (ASCII case-insensitive).
//! 5. The file name equals `adapter.dll_file_name()` (case-insensitive), so one
//!    adapter cannot load another adapter's library.
//! 6. The canonical path is under at least one canonical allowed root
//!    (`starts_with`), defeating junction and symlink escape.
//! 7. The canonical target is a file; its byte length is within
//!    `max_path_bytes`; its size is within `max_file_bytes`.
//! 8. The canonical path is returned. Errors carry only static reason strings,
//!    logical [`AdapterKind`] values, and optional numeric OS codes —
//!    never a full path.

use std::path::{Component, Path, PathBuf};

use crate::{AdapterKind, CompatError};

/// Daemon-owned policy for DLL path validation.
///
/// `allowed_roots` is empty by default: no DLL can load until the daemon
/// configures an explicit root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DllPathPolicy {
    /// Absolute roots the daemon explicitly allowlists.
    pub allowed_roots: Vec<PathBuf>,
    /// Default 32_768 (Windows extended-length path budget), in bytes.
    pub max_path_bytes: usize,
    /// Default 128 MiB.
    pub max_file_bytes: u64,
}

impl Default for DllPathPolicy {
    fn default() -> Self {
        Self {
            allowed_roots: Vec::new(),
            max_path_bytes: 32_768,
            max_file_bytes: 128 * 1024 * 1024,
        }
    }
}

/// A canonical absolute DLL path that passed every [`DllPathPolicy`] rule.
///
/// The stored path is the canonical path, never the caller's original string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedDllPath {
    path: PathBuf,
    adapter: AdapterKind,
}

impl ValidatedDllPath {
    /// Validate before any OS loader call. Rules are applied in contract §5.3 order.
    pub fn validate(
        path: &Path,
        adapter: AdapterKind,
        policy: &DllPathPolicy,
    ) -> Result<Self, CompatError> {
        // Rule 1: feature gate first; no path check and no filesystem touch
        // when the adapter is not compiled in.
        if !adapter.is_enabled() {
            return Err(CompatError::FeatureDisabled { adapter });
        }
        // Rule 2: absolute only; on Windows only a disk or UNC prefix.
        if !path.is_absolute() {
            return Err(CompatError::InvalidDllPath {
                reason: "not_absolute",
            });
        }
        #[cfg(windows)]
        {
            let has_disk_or_unc = matches!(
                path.components().next(),
                Some(Component::Prefix(prefix))
                    if matches!(
                        prefix.kind(),
                        std::path::Prefix::Disk(_) | std::path::Prefix::UNC(..)
                    )
            );
            if !has_disk_or_unc {
                return Err(CompatError::InvalidDllPath {
                    reason: "unsupported_prefix",
                });
            }
        }
        // Rule 3: no `.` or `..` components in the input path.
        if path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        {
            return Err(CompatError::InvalidDllPath {
                reason: "dot_component",
            });
        }
        // Rule 4: `dll` extension, ASCII case-insensitive.
        let has_dll_extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"));
        if !has_dll_extension {
            return Err(CompatError::InvalidDllPath {
                reason: "bad_extension",
            });
        }
        // Rule 5: file name must equal this adapter's library name, so one
        // adapter cannot load another adapter's library.
        let file_name_matches = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(adapter.dll_file_name()));
        if !file_name_matches {
            return Err(CompatError::InvalidDllPath {
                reason: "wrong_file_name",
            });
        }
        // Rule 6: canonicalize the path and every allowed root; the canonical
        // path must be under at least one canonical root. I/O errors map to
        // static reasons: no path or OS text is stored in any variant.
        let canonical = path
            .canonicalize()
            .map_err(|_| CompatError::InvalidDllPath {
                reason: "unresolvable_path",
            })?;
        let under_allowlist = policy
            .allowed_roots
            .iter()
            .filter_map(|root| root.canonicalize().ok())
            .any(|root| canonical.starts_with(&root));
        if !under_allowlist {
            return Err(CompatError::PathOutsideAllowlist);
        }
        // Rule 7: canonical target is a file; bounded path length and size.
        let metadata = std::fs::metadata(&canonical).map_err(|_| CompatError::InvalidDllPath {
            reason: "metadata_failed",
        })?;
        if !metadata.is_file() {
            return Err(CompatError::InvalidDllPath {
                reason: "not_a_file",
            });
        }
        if canonical.as_os_str().len() > policy.max_path_bytes {
            return Err(CompatError::BoundExceeded { adapter });
        }
        if metadata.len() > policy.max_file_bytes {
            return Err(CompatError::BoundExceeded { adapter });
        }
        // Rule 8: return the canonical path.
        Ok(Self {
            path: canonical,
            adapter,
        })
    }

    /// Canonical absolute path. Never the caller's original string.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The adapter this path was validated for.
    pub fn adapter(&self) -> AdapterKind {
        self.adapter
    }
}
