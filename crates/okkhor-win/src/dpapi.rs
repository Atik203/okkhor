//! DPAPI (Data Protection API) boundary for user-scoped secrets.
//!
//! Current-user scope via `CryptProtectData` / `CryptUnprotectData` with
//! `CRYPTPROTECT_UI_FORBIDDEN`, so no UI may ever appear on the typing path.
//! Errors are typed and carry no plaintext: callers learn only whether the
//! operation failed, never what the bytes contained.
//!
//! Non-Windows builds expose the same signatures but always return
//! [`DpapiError::Unsupported`], so the workspace still builds elsewhere.

use std::io;

use thiserror::Error;

/// Hard cap for a single DPAPI input (64 MiB). The word store caps far below
/// this (8 MiB); this bound only stops absurd allocations.
pub const MAX_DPAPI_BYTES: usize = 64 * 1024 * 1024;

/// Typed DPAPI failures. No variant carries plaintext or ciphertext.
#[derive(Debug, Error)]
pub enum DpapiError {
    /// Input exceeds [`MAX_DPAPI_BYTES`].
    #[error("data exceeds the DPAPI size limit")]
    TooLarge,
    /// `CryptProtectData` failed. The OS error is retained as the source for
    /// a numeric code only; it carries no protected bytes.
    #[error("DPAPI protect failed")]
    ProtectFailed(#[source] io::Error),
    /// `CryptUnprotectData` failed: tampered blob, wrong entropy, or a blob
    /// from another user. Deliberately source-free so callers cannot
    /// distinguish the cause.
    #[error("DPAPI unprotect failed")]
    UnprotectFailed,
    /// Called on a platform without DPAPI.
    #[error("DPAPI is only available on Windows")]
    Unsupported,
}

/// Protect `plaintext` for the current user, bound to `entropy`.
///
/// The same `entropy` must be supplied to [`unprotect`]; it need not be
/// secret, it only scopes the blob to this application.
#[cfg(windows)]
pub fn protect(plaintext: &[u8], entropy: &[u8]) -> Result<Vec<u8>, DpapiError> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
    };
    use windows::core::PCWSTR;

    if plaintext.len() > MAX_DPAPI_BYTES || entropy.len() > MAX_DPAPI_BYTES {
        return Err(DpapiError::TooLarge);
    }
    // Verified against the OS: `CryptProtectData` rejects zero-length input
    // with `E_INVALIDARG`, so empty plaintext fails here with a typed error
    // instead of a surprising OS round-trip. (Empty *entropy* is fine: NULL
    // optional entropy is the documented default.)
    if plaintext.is_empty() {
        return Err(DpapiError::ProtectFailed(io::Error::new(
            io::ErrorKind::InvalidInput,
            "plaintext is empty",
        )));
    }

    // Input blobs borrow the caller's slices; DPAPI copies synchronously.
    let input = CRYPT_INTEGER_BLOB {
        cbData: plaintext.len() as u32,
        pbData: if plaintext.is_empty() {
            std::ptr::null_mut()
        } else {
            plaintext.as_ptr() as *mut u8
        },
    };
    let entropy_blob = CRYPT_INTEGER_BLOB {
        cbData: entropy.len() as u32,
        pbData: if entropy.is_empty() {
            std::ptr::null_mut()
        } else {
            entropy.as_ptr() as *mut u8
        },
    };
    let entropy_arg = if entropy.is_empty() {
        None
    } else {
        Some(std::ptr::addr_of!(entropy_blob))
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    // SAFETY: `input` points at the live `plaintext` slice (or is null with a
    // zero length, which DPAPI accepts without dereferencing); `entropy_arg`
    // likewise; `output` is a valid out-pointer; no prompt struct plus the
    // flag means no UI may appear; the call is synchronous and copies before
    // returning.
    unsafe {
        CryptProtectData(
            &input,
            PCWSTR::null(),
            entropy_arg,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    }
    .map_err(|error| DpapiError::ProtectFailed(io::Error::from_raw_os_error(error.code().0)))?;

    if output.pbData.is_null() {
        return Err(DpapiError::ProtectFailed(io::Error::other(
            "DPAPI returned no output",
        )));
    }
    // SAFETY: on success DPAPI allocates `cbData` readable bytes at `pbData`
    // with `LocalAlloc`; the pointer/length pair is valid for exactly one
    // read here, and `cbData` fits in memory by the cap above.
    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData as *const u8, output.cbData as usize) }
            .to_vec();
    // SAFETY: `output.pbData` came from `CryptProtectData`, whose documented
    // deallocator is `LocalFree`; it is freed exactly once here and never
    // aliased afterwards.
    unsafe {
        LocalFree(Some(HLOCAL(output.pbData as *mut std::ffi::c_void)));
    }
    Ok(bytes)
}

/// Reverse [`protect`]: decrypt `ciphertext` with the same `entropy`.
///
/// Any tampering, wrong entropy, or foreign-user blob fails closed with
/// [`DpapiError::UnprotectFailed`].
#[cfg(windows)]
pub fn unprotect(ciphertext: &[u8], entropy: &[u8]) -> Result<Vec<u8>, DpapiError> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
    };

    // Ciphertext tracks plaintext size (DPAPI adds only a small header), so
    // the same cap bounds the allocation on both directions.
    if ciphertext.len() > MAX_DPAPI_BYTES || entropy.len() > MAX_DPAPI_BYTES {
        return Err(DpapiError::TooLarge);
    }

    let input = CRYPT_INTEGER_BLOB {
        cbData: ciphertext.len() as u32,
        pbData: if ciphertext.is_empty() {
            std::ptr::null_mut()
        } else {
            ciphertext.as_ptr() as *mut u8
        },
    };
    let entropy_blob = CRYPT_INTEGER_BLOB {
        cbData: entropy.len() as u32,
        pbData: if entropy.is_empty() {
            std::ptr::null_mut()
        } else {
            entropy.as_ptr() as *mut u8
        },
    };
    let entropy_arg = if entropy.is_empty() {
        None
    } else {
        Some(std::ptr::addr_of!(entropy_blob))
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    // SAFETY: `input` points at the live `ciphertext` slice (or is null with
    // a zero length); `entropy_arg` likewise; `output` is a valid
    // out-pointer; no prompt struct plus the flag means no UI may appear; the
    // call is synchronous.
    let status = unsafe {
        CryptUnprotectData(
            &input,
            None,
            entropy_arg,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    // Drop the OS error: its code distinguishes tamper from wrong-entropy,
    // and callers must treat both identically (fail closed, no detail).
    if status.is_err() {
        return Err(DpapiError::UnprotectFailed);
    }

    if output.pbData.is_null() {
        return Err(DpapiError::UnprotectFailed);
    }
    // SAFETY: on success DPAPI allocates `cbData` readable bytes at `pbData`
    // with `LocalAlloc`; the pointer/length pair is valid for exactly one
    // read here, and `cbData` fits in memory by the cap above.
    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData as *const u8, output.cbData as usize) }
            .to_vec();
    // SAFETY: `output.pbData` came from `CryptUnprotectData`, whose documented
    // deallocator is `LocalFree`; it is freed exactly once here and never
    // aliased afterwards.
    unsafe {
        LocalFree(Some(HLOCAL(output.pbData as *mut std::ffi::c_void)));
    }
    Ok(bytes)
}

#[cfg(not(windows))]
/// Non-Windows stub: DPAPI does not exist here.
pub fn protect(_plaintext: &[u8], _entropy: &[u8]) -> Result<Vec<u8>, DpapiError> {
    if _plaintext.len() > MAX_DPAPI_BYTES || _entropy.len() > MAX_DPAPI_BYTES {
        return Err(DpapiError::TooLarge);
    }
    Err(DpapiError::Unsupported)
}

#[cfg(not(windows))]
/// Non-Windows stub: DPAPI does not exist here.
pub fn unprotect(_ciphertext: &[u8], _entropy: &[u8]) -> Result<Vec<u8>, DpapiError> {
    if _ciphertext.len() > MAX_DPAPI_BYTES || _entropy.len() > MAX_DPAPI_BYTES {
        return Err(DpapiError::TooLarge);
    }
    Err(DpapiError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn non_windows_is_unsupported() {
        assert!(matches!(
            protect(b"secret", b"entropy"),
            Err(DpapiError::Unsupported)
        ));
        assert!(matches!(
            unprotect(b"secret", b"entropy"),
            Err(DpapiError::Unsupported)
        ));
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_still_enforces_size_cap() {
        let big = vec![0u8; MAX_DPAPI_BYTES + 1];
        assert!(matches!(
            protect(&big, b"entropy"),
            Err(DpapiError::TooLarge)
        ));
        assert!(matches!(
            unprotect(&big, b"entropy"),
            Err(DpapiError::TooLarge)
        ));
    }

    #[test]
    fn errors_carry_no_plaintext() {
        let error = DpapiError::UnprotectFailed;
        let rendered = format!("{error}");
        assert!(!rendered.is_empty());
        assert!(!rendered.contains("secret"));
        let io_error = DpapiError::ProtectFailed(io::Error::other("os-detail"));
        assert!(!format!("{io_error}").contains("secret"));
    }

    #[cfg(windows)]
    #[test]
    fn round_trip() {
        let plaintext = b"okkhor secret words";
        let blob = protect(plaintext, b"test-entropy").expect("protects");
        assert!(!blob.is_empty());
        assert_ne!(blob.as_slice(), plaintext.as_slice());
        let back = unprotect(&blob, b"test-entropy").expect("unprotects");
        assert_eq!(back, plaintext);
    }

    #[cfg(windows)]
    #[test]
    fn tampered_blob_is_rejected() {
        let secret = b"tamper-me-please";
        let mut blob = protect(secret, b"test-entropy").expect("protects");
        let index = blob.len() / 2;
        blob[index] ^= 0x01;
        let error = unprotect(&blob, b"test-entropy").expect_err("tamper must fail");
        assert!(matches!(error, DpapiError::UnprotectFailed));
        // The failure carries no trace of the secret.
        assert!(!format!("{error:?}").contains("tamper"));
    }

    #[cfg(windows)]
    #[test]
    fn truncated_blob_is_rejected() {
        let blob = protect(b"truncate-me", b"test-entropy").expect("protects");
        let short = &blob[..blob.len() / 2];
        assert!(matches!(
            unprotect(short, b"test-entropy"),
            Err(DpapiError::UnprotectFailed)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn empty_input_is_rejected_not_silent() {
        // The OS rejects zero-length input (`E_INVALIDARG`); the wrapper
        // surfaces that as a typed error instead of attempting the call.
        let error = protect(b"", b"test-entropy").expect_err("empty plaintext must fail");
        assert!(
            matches!(error, DpapiError::ProtectFailed(_)),
            "typed failure, got {error:?}"
        );
        assert!(!format!("{error}").contains("secret"));
        assert!(matches!(
            unprotect(b"", b"test-entropy"),
            Err(DpapiError::UnprotectFailed)
        ));
        // Empty entropy is fine (NULL optional entropy is the default).
        let blob = protect(b"plain", b"").expect("protects without entropy");
        assert_eq!(unprotect(&blob, b"").expect("unprotects"), b"plain");
    }

    #[cfg(windows)]
    #[test]
    fn one_mib_input_round_trips() {
        let plaintext = vec![0x41u8; 1024 * 1024];
        let blob = protect(&plaintext, b"test-entropy").expect("protects 1 MiB");
        let back = unprotect(&blob, b"test-entropy").expect("unprotects 1 MiB");
        assert_eq!(back, plaintext);
    }

    #[cfg(windows)]
    #[test]
    fn wrong_entropy_fails() {
        let blob = protect(b"entropy-bound", b"correct-entropy").expect("protects");
        assert!(matches!(
            unprotect(&blob, b"wrong-entropy"),
            Err(DpapiError::UnprotectFailed)
        ));
        assert!(matches!(
            unprotect(&blob, b""),
            Err(DpapiError::UnprotectFailed)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn distinct_entropy_gives_distinct_blobs() {
        let plaintext = b"same plaintext";
        let first = protect(plaintext, b"entropy-one").expect("protects");
        let second = protect(plaintext, b"entropy-two").expect("protects");
        // Random IVs already separate repeated protects; different entropy
        // must never alias either.
        assert_ne!(first, second);
        assert_eq!(
            unprotect(&first, b"entropy-one").expect("unprotects"),
            plaintext
        );
        assert_eq!(
            unprotect(&second, b"entropy-two").expect("unprotects"),
            plaintext
        );
    }

    #[cfg(windows)]
    #[test]
    fn oversized_input_is_rejected_before_dpapi() {
        let big = vec![0u8; MAX_DPAPI_BYTES + 1];
        assert!(matches!(protect(&big, b"e"), Err(DpapiError::TooLarge)));
        assert!(matches!(unprotect(&big, b"e"), Err(DpapiError::TooLarge)));
    }
}
