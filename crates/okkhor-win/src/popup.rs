//! A minimal non-activating popup window.
//!
//! The production UI will replace the `STATIC` content with a rendered
//! suggestion list. The window contract is what matters here: no activation,
//! no focus theft, and placement in physical pixels.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SetWindowPos, SetWindowTextW, ShowWindow, WS_BORDER, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{HSTRING, w};

use crate::caret::PopupPlacement;

#[derive(Debug, thiserror::Error)]
pub enum PopupError {
    #[error("Win32 call {operation} failed with code {code}")]
    Win32 { operation: &'static str, code: i32 },
}

/// A top-level, non-activating window that shows plain text near the caret.
pub struct SuggestionPopup {
    window: HWND,
}

impl SuggestionPopup {
    pub fn new() -> Result<Self, PopupError> {
        // SAFETY: the system STATIC class is a valid window class; all optional
        // handles and parameters are null, and no activation is requested.
        let window = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                w!("STATIC"),
                w!(""),
                WS_POPUP | WS_BORDER,
                0,
                0,
                1,
                1,
                None,
                None,
                None,
                None,
            )
        }
        .map_err(|error| PopupError::Win32 {
            operation: "CreateWindowExW",
            code: error.code().0,
        })?;

        Ok(Self { window })
    }

    /// Update the text and show the window at a physical-pixel placement.
    pub fn show(&mut self, text: &str, placement: PopupPlacement) -> Result<(), PopupError> {
        let text = HSTRING::from(text);
        // SAFETY: window is a live window owned by this popup; HSTRING is valid.
        unsafe { SetWindowTextW(self.window, &text) }.map_err(|error| PopupError::Win32 {
            operation: "SetWindowTextW",
            code: error.code().0,
        })?;

        // SAFETY: window is live; the placement is in physical pixels.
        unsafe {
            SetWindowPos(
                self.window,
                None,
                placement.x,
                placement.y,
                placement.width,
                placement.height,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW,
            )
        }
        .map_err(|error| PopupError::Win32 {
            operation: "SetWindowPos",
            code: error.code().0,
        })?;

        // SWP_SHOWWINDOW already shows it; this explicit path documents the
        // no-activate intent and covers shell quirks.
        // SAFETY: window is live.
        unsafe {
            let _ = ShowWindow(self.window, SW_SHOWNOACTIVATE);
        }
        Ok(())
    }

    pub fn hide(&self) {
        // SAFETY: window is live until Drop.
        unsafe {
            let _ = ShowWindow(self.window, SW_HIDE);
        }
    }
}

impl Drop for SuggestionPopup {
    fn drop(&mut self) {
        // SAFETY: window is owned by this popup and destroyed exactly once.
        unsafe {
            let _ = DestroyWindow(self.window);
        }
    }
}
