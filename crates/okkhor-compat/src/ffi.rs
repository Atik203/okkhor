//! Private Windows loader seam (contract §5.5).
//!
//! The ONLY module in this crate allowed to contain `unsafe` blocks.
//! Every function below is `pub(super)` and unsafe to call; no public API
//! exposes these symbols and raw pointers never cross into safe code.
//!
//! This module compiles on `cfg(windows)` only. Under `cfg(test)` the
//! [`crate::AdapterRegistry`] uses a stubbed seam instead, so tests never
//! open a real DLL.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt as _;

use windows::Win32::Foundation::{FreeLibrary, GetLastError, HMODULE};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::core::{PCSTR, PCWSTR};

use crate::{CompatError, ValidatedDllPath};

/// Load a validated DLL with locked-down search flags.
///
/// Under `cfg(test)` the registry uses a stubbed seam instead, so this
/// function is live code only in production builds.
///
/// # Safety
///
/// Call only with a [`ValidatedDllPath`] (absolute, canonical, allowlisted,
/// bounded), once per resulting handle, on the loading thread, at
/// startup/shutdown — never from the keyboard hook or the per-keystroke
/// candidate path. The caller owns the returned handle and must free it
/// exactly once via [`free_library`] on the same thread.
#[cfg_attr(test, allow(dead_code))]
pub(super) unsafe fn load_library(path: &ValidatedDllPath) -> Result<*mut c_void, CompatError> {
    let adapter = path.adapter();
    // Explicit UTF-16 conversion with a NUL terminator; the validated path is
    // within `max_path_bytes`, so this adds at most a bounded trailer.
    let mut wide: Vec<u16> = path.path().as_os_str().encode_wide().collect();
    wide.push(0);
    // SAFETY: `wide` outlives the call and holds a NUL-terminated UTF-16
    // buffer; search flags exclude the cwd and relative lookup, so only the
    // canonical absolute path can load. No Rust reference crosses into the
    // loader, and no panic crosses the FFI boundary.
    let module = unsafe {
        LoadLibraryExW(
            PCWSTR(wide.as_ptr()),
            None,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    }
    .map_err(|_| CompatError::LoadFailed {
        adapter,
        // SAFETY: `GetLastError` reads only thread-local OS error state set by
        // the failed load above; it touches no Rust memory.
        code: Some(unsafe { GetLastError().0 } as i32),
    })?;
    Ok(module.0)
}

/// Free a handle previously returned by [`load_library`].
///
/// # Safety
///
/// Call only with a non-null handle owned by a live [`crate::AdapterHandle`],
/// exactly once, on the loading thread. Unload failures surface as
/// `InvalidDllPath { reason: "free_failed" }`: the seam carries no adapter
/// context, so the registry re-attaches it (see `platform_unload`).
///
/// Under `cfg(test)` the registry uses a stubbed seam instead, so this
/// function is live code only in production builds.
#[cfg_attr(test, allow(dead_code))]
pub(super) unsafe fn free_library(handle: *mut c_void) -> Result<(), CompatError> {
    // SAFETY: the caller guarantees `handle` is a non-null live module handle
    // from `load_library`, freed exactly once on the owning thread.
    unsafe { FreeLibrary(HMODULE(handle)) }.map_err(|_| CompatError::InvalidDllPath {
        reason: "free_failed",
    })
}

/// Resolve an export from a live handle.
///
/// Kept per the §5.5 contract for the adapter call surface arriving in later
/// milestones; currently unused because `AdapterHandle` exposes no
/// adapter-specific methods yet (see the scaffold TODO in `adapter.rs`).
///
/// # Safety
///
/// Call only with a live handle owned by a [`crate::AdapterHandle`] and a
/// NUL-terminated ASCII export-name buffer that outlives the call. Symbol
/// names are compile-time constants from this crate, never user-supplied. An
/// invalid name buffer or a missing export yields `Ok(None)`; no panic
/// crosses the boundary.
#[allow(dead_code)]
pub(super) unsafe fn resolve_proc(
    handle: *mut c_void,
    name: &[u8],
) -> Result<Option<*mut c_void>, CompatError> {
    let is_nul_terminated_ascii = name.len() >= 2
        && name[name.len() - 1] == 0
        && name[..name.len() - 1]
            .iter()
            .all(|byte| byte.is_ascii() && *byte != 0);
    if !is_nul_terminated_ascii || handle.is_null() {
        return Ok(None);
    }
    // SAFETY: `handle` is live per the caller contract; `name` points to a
    // valid NUL-terminated ASCII buffer for the duration of the call.
    let proc = unsafe { GetProcAddress(HMODULE(handle), PCSTR(name.as_ptr())) };
    Ok(proc.map(|far_proc| far_proc as *mut c_void))
}
