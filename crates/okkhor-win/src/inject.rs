use serde::{Deserialize, Serialize};
use thiserror::Error;

/// How text was inserted into the target application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectionMethod {
    /// Batched `SendInput` with `KEYEVENTF_UNICODE`.
    UnicodeSendInput,
    /// `WM_CHAR` fast path for known Win32 text controls.
    WindowMessage,
    /// Clipboard write followed by Ctrl+V.
    ClipboardPaste,
}

/// Errors raised by the injection layer.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum InjectError {
    #[error("text injection is not available on this host")]
    Unsupported,
    #[error("the target window rejected input (UIPI or focus lost)")]
    Blocked,
    #[error("the target window changed focus before injection")]
    FocusChanged,
    #[error("the clipboard is busy; another application holds it")]
    ClipboardBusy,
    #[error("the clipboard contains non-text data and will not be overwritten")]
    ClipboardNotEmpty,
    #[error("Win32 call {operation} failed with code {code}")]
    Win32 { operation: &'static str, code: u32 },
}

/// Inserts text into the foreground application.
///
/// Implementations must not steal focus and must choose Unicode `SendInput`
/// first, falling back to clipboard paste only when necessary.
pub trait TextInjector {
    fn inject(&self, text: &str) -> Result<InjectionMethod, InjectError>;
}
