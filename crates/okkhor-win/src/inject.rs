use serde::{Deserialize, Serialize};
use thiserror::Error;

/// How text was inserted into the target application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectionMethod {
    /// Batched `SendInput` with `KEYEVENTF_UNICODE`.
    UnicodeSendInput,
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
