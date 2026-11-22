//! Clipboard access for the injection fallback.
//!
//! The fallback only overwrites a clipboard that is empty or contains text.
//! Any other format is left untouched and reported to the caller.

use std::mem::size_of;
use std::ptr;
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;

use crate::inject::InjectError;

const FORMAT: u32 = CF_UNICODETEXT.0 as u32;
const OPEN_ATTEMPTS: usize = 10;
const OPEN_RETRY: Duration = Duration::from_millis(5);
const MAX_TEXT_UNITS: usize = 1_048_576;

/// Owns an open clipboard. Closing is best-effort but always attempted.
pub(crate) struct ClipboardGuard;

impl ClipboardGuard {
    pub(crate) fn open() -> Result<Self, InjectError> {
        for attempt in 0..OPEN_ATTEMPTS {
            // SAFETY: opening the clipboard has no pointer preconditions.
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Ok(Self);
            }
            if attempt + 1 < OPEN_ATTEMPTS {
                thread::sleep(OPEN_RETRY);
            }
        }
        Err(InjectError::ClipboardBusy)
    }

    /// True when the clipboard has any format other than Unicode text.
    pub(crate) fn has_non_text_formats(&self) -> bool {
        let mut format = 0;
        loop {
            // SAFETY: EnumClipboardFormats is valid while the clipboard is open.
            format = unsafe { EnumClipboardFormats(format) };
            if format == 0 {
                return false;
            }
            if format != FORMAT {
                return true;
            }
        }
    }

    /// Read the current Unicode text, or `None` when none is present.
    pub(crate) fn read_text(&self) -> Result<Option<String>, InjectError> {
        // SAFETY: the clipboard is open and the format query has no preconditions.
        if unsafe { IsClipboardFormatAvailable(FORMAT) }.is_err() {
            return Ok(None);
        }

        // SAFETY: the clipboard is open; a failed handle is reported.
        let handle = unsafe { GetClipboardData(FORMAT) }.map_err(|error| InjectError::Win32 {
            operation: "GetClipboardData",
            code: error.code().0 as u32,
        })?;
        let hglobal = HGLOBAL(handle.0);
        // SAFETY: the handle is a global memory block owned by the clipboard.
        let ptr = unsafe { GlobalLock(hglobal) } as *const u16;
        if ptr.is_null() {
            return Err(InjectError::ClipboardBusy);
        }

        let mut units = Vec::new();
        for index in 0..MAX_TEXT_UNITS {
            // SAFETY: the block is locked and the loop stops at a NUL or cap.
            let unit = unsafe { *ptr.add(index) };
            if unit == 0 {
                break;
            }
            units.push(unit);
        }
        // SAFETY: balancing the successful GlobalLock above.
        let _ = unsafe { GlobalUnlock(hglobal) };

        Ok(Some(String::from_utf16_lossy(&units)))
    }

    /// Replace the clipboard with Unicode text.
    pub(crate) fn write_text(&self, text: &str) -> Result<(), InjectError> {
        let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = units.len().saturating_mul(size_of::<u16>());

        // SAFETY: allocating a movable global block has no pointer input.
        let hglobal =
            unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) }.map_err(|error| InjectError::Win32 {
                operation: "GlobalAlloc",
                code: error.code().0 as u32,
            })?;

        // SAFETY: the block is owned and large enough for the UTF-16 units.
        let destination = unsafe { GlobalLock(hglobal) } as *mut u16;
        if destination.is_null() {
            // SAFETY: the block is still owned on this failure path.
            let _ = unsafe { GlobalFree(Some(hglobal)) };
            return Err(InjectError::ClipboardBusy);
        }

        // SAFETY: source and destination are valid for units.len() u16 values.
        unsafe {
            ptr::copy_nonoverlapping(units.as_ptr(), destination, units.len());
        }
        // SAFETY: balancing the successful GlobalLock above.
        let _ = unsafe { GlobalUnlock(hglobal) };

        // SAFETY: the clipboard is open; clearing it is required before setting.
        if let Err(error) = unsafe { EmptyClipboard() } {
            // SAFETY: ownership stays with this call when Set has not succeeded.
            let _ = unsafe { GlobalFree(Some(hglobal)) };
            return Err(InjectError::Win32 {
                operation: "EmptyClipboard",
                code: error.code().0 as u32,
            });
        }

        // SAFETY: on success the system owns the global memory; on failure the
        // block is freed below.
        match unsafe { SetClipboardData(FORMAT, Some(HANDLE(hglobal.0))) } {
            Ok(_) => Ok(()),
            Err(error) => {
                // SAFETY: SetClipboardData failed, so ownership did not transfer.
                let _ = unsafe { GlobalFree(Some(hglobal)) };
                Err(InjectError::Win32 {
                    operation: "SetClipboardData",
                    code: error.code().0 as u32,
                })
            }
        }
    }

    /// Clear the clipboard when there was no prior text to restore.
    pub(crate) fn clear(&self) -> Result<(), InjectError> {
        // SAFETY: the clipboard is open.
        unsafe { EmptyClipboard() }.map_err(|error| InjectError::Win32 {
            operation: "EmptyClipboard",
            code: error.code().0 as u32,
        })
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: close is balanced with the successful open in `open`.
        let _ = unsafe { CloseClipboard() };
    }
}
