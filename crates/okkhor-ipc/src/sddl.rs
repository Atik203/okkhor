//! Owner-only security descriptor hardening (Windows only).
//!
//! This is the single `unsafe` module in `okkhor-ipc`. Everything else in the
//! crate is `#![forbid(unsafe_code)]`. The `unsafe` here is unavoidable:
//! passing an owner-only DACL to the Win32 security-descriptor and pipe APIs
//! requires `unsafe fn` in the `windows` crate and an `unsafe fn` in tokio.
//!
//! The pipe DACL is present **at creation**: [`create_server_instance`]
//! builds a `SECURITY_ATTRIBUTES` from
//! `ConvertStringSecurityDescriptorToSecurityDescriptorW` and hands it to
//! `CreateNamedPipeW` through tokio's
//! `ServerOptions::create_with_security_attributes_raw`. There is no window in
//! which another local user can open the instance under a weaker DACL —
//! post-creation `SetSecurityInfo` hardening cannot close that window,
//! because a client that opens the instance before the DACL is tightened
//! keeps its granted access afterwards. (An earlier revision did
//! create-then-harden; the false "harden before connect()" comment it carried
//! is removed by this change.)
//!
//! The token file uses [`apply_owner_only_dacl_to_path`]
//! (`SetNamedSecurityInfoW`), which opens the object itself with the access
//! it needs.

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows::Win32::Foundation::{ERROR_SUCCESS, HLOCAL, LocalFree, WIN32_ERROR};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION, SE_FILE_OBJECT,
    SetNamedSecurityInfoW,
};
use windows::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, PSECURITY_DESCRIPTOR,
    SECURITY_ATTRIBUTES,
};
use windows::core::BOOL;

use crate::auth::TokenFileError;

/// Owner + System full access (ADR 0012 §2).
pub const OWNER_ONLY_SDDL: &str = "D:(A;;GA;;;SY)(A;;GA;;;OW)";

/// Failures while building or applying the security descriptor.
#[derive(Debug, thiserror::Error)]
pub enum SddlError {
    #[error("security descriptor call failed: {0}")]
    Win(#[from] windows::core::Error),
    #[error("the converted descriptor has no DACL")]
    MissingDacl,
    #[error("SetSecurityInfo failed with code {0}")]
    SetSecurity(u32),
}

/// An SDDL-derived security descriptor that frees itself via `LocalFree`.
struct OwnerDescriptor(PSECURITY_DESCRIPTOR);

impl OwnerDescriptor {
    fn from_sddl(sddl: &str) -> Result<Self, SddlError> {
        let wide: windows::core::HSTRING = sddl.into();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `wide` is a valid NUL-terminated UTF-16 string owned by this
        // scope; `descriptor` is a valid out-pointer for the returned
        // descriptor, which this struct's `Drop` frees exactly once with
        // `LocalFree`. `SDDL_REVISION` (1) is the only revision the converter
        // accepts.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &wide,
                SDDL_REVISION,
                &mut descriptor,
                None,
            )
        }?;
        Ok(Self(descriptor))
    }

    /// Borrow the DACL embedded in the converted descriptor. The pointer is
    /// valid for the lifetime of `self`.
    fn dacl(&self) -> Result<*const ACL, SddlError> {
        let mut present = BOOL::default();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut defaulted = BOOL::default();
        // SAFETY: `self.0` is a valid absolute-format descriptor from
        // `from_sddl`, alive for this call; the three out-pointers are valid
        // locals. The returned `dacl` points inside the descriptor, so it
        // stays valid while `self` is alive.
        unsafe { GetSecurityDescriptorDacl(self.0, &mut present, &mut dacl, &mut defaulted) }?;
        if present.as_bool() && !dacl.is_null() {
            Ok(dacl as *const ACL)
        } else {
            Err(SddlError::MissingDacl)
        }
    }
}

impl Drop for OwnerDescriptor {
    fn drop(&mut self) {
        // SAFETY: the pointer came from
        // `ConvertStringSecurityDescriptorToSecurityDescriptorW`, which
        // documents `LocalFree` as its deallocator; this `Drop` runs exactly
        // once per successful conversion.
        unsafe {
            LocalFree(Some(HLOCAL(self.0.0)));
        }
    }
}

/// A `SECURITY_ATTRIBUTES` bundle that owns its descriptor.
///
/// `SECURITY_ATTRIBUTES.lpSecurityDescriptor` borrows the descriptor, so both
/// live in one struct: the raw pointer handed to `CreateNamedPipeW` stays
/// valid while `self` is alive.
pub struct OwnerSecurityAttributes {
    _descriptor: OwnerDescriptor,
    attributes: SECURITY_ATTRIBUTES,
}

impl OwnerSecurityAttributes {
    fn new() -> Result<Self, SddlError> {
        let descriptor = OwnerDescriptor::from_sddl(OWNER_ONLY_SDDL)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0.0,
            bInheritHandle: BOOL(0),
        };
        Ok(Self {
            _descriptor: descriptor,
            attributes,
        })
    }

    fn as_raw(&mut self) -> *mut std::ffi::c_void {
        &mut self.attributes as *mut SECURITY_ATTRIBUTES as *mut std::ffi::c_void
    }
}

/// Create one pipe instance with the owner-only DACL present at creation.
///
/// All other instance settings (`first_pipe_instance`, duplex, byte mode,
/// buffer sizes, remote-client rejection) come from `options` unchanged —
/// only the creation call differs from `ServerOptions::create`.
pub fn create_server_instance(
    options: &ServerOptions,
    pipe_name: &str,
) -> std::io::Result<NamedPipeServer> {
    let mut security = OwnerSecurityAttributes::new().map_err(std::io::Error::other)?;
    // SAFETY: `security` owns a valid `SECURITY_ATTRIBUTES` whose descriptor
    // pointer stays alive for this call; tokio documents that `attrs` must be
    // null or point at a valid `SECURITY_ATTRIBUTES` for the duration of the
    // call, which holds here. The returned handle is a fresh pipe instance
    // owned by the `NamedPipeServer`.
    unsafe { options.create_with_security_attributes_raw(pipe_name, security.as_raw()) }
}

/// Apply the owner-only DACL to a file by path.
///
/// `SetNamedSecurityInfoW` opens the object itself with the access it needs,
/// which a plain `std` file handle does not carry. Used for the token file
/// (0600-equivalent intent).
pub(crate) fn apply_owner_only_dacl_to_path(path: &std::path::Path) -> Result<(), TokenFileError> {
    use std::os::windows::ffi::OsStrExt as _;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let descriptor = OwnerDescriptor::from_sddl(OWNER_ONLY_SDDL)
        .map_err(|error| TokenFileError::Io(std::io::Error::other(format!("{error}"))))?;
    let dacl = descriptor
        .dacl()
        .map_err(|error| TokenFileError::Io(std::io::Error::other(format!("{error}"))))?;
    // SAFETY: `wide` is a valid NUL-terminated UTF-16 path owned by this
    // scope; `dacl` points inside `descriptor` (alive for this call). Only
    // the DACL is replaced; owner, group, and SACL are untouched (`None`).
    let status: WIN32_ERROR = unsafe {
        SetNamedSecurityInfoW(
            windows::core::PCWSTR(wide.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(dacl),
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(TokenFileError::Io(std::io::Error::other(
            SddlError::SetSecurity(status.0),
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_sddl_converts() {
        let descriptor = OwnerDescriptor::from_sddl(OWNER_ONLY_SDDL);
        assert!(descriptor.is_ok());
    }

    #[test]
    fn invalid_sddl_rejected() {
        assert!(OwnerDescriptor::from_sddl("bogus").is_err());
    }

    #[test]
    fn file_dacl_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "okkhor-ipc-sddl-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("probe.txt");
        std::fs::write(&path, "probe").expect("writes");
        apply_owner_only_dacl_to_path(&path).expect("hardens");
        std::fs::remove_dir_all(&dir).ok();
    }
}
