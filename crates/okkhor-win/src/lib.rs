#![deny(unsafe_op_in_unsafe_fn)]
//! Windows integration boundary.
//!
//! Milestone 0.1 contains only platform-independent types and traits. The
//! actual `WH_KEYBOARD_LL` hook, `SendInput`, caret tracking, and clipboard
//! fallback land in milestone 0.2.
//!
//! All `unsafe` code for Windows belongs in this crate. Every block must carry a
//! `// SAFETY:` comment that names the invariant and the API contract being
//! upheld. No other crate may call Win32 directly.

pub mod inject;
pub mod input;

#[cfg(windows)]
mod single_instance;
#[cfg(windows)]
mod win32;

pub use inject::{InjectError, InjectionMethod, TextInjector};
pub use input::{KeyEvent, KeyPhase, KeyboardHook};

#[cfg(windows)]
pub use single_instance::{SingleInstance, SingleInstanceError};
#[cfg(windows)]
pub use win32::{
    HookError, WindowsInjector, WindowsKeyboardHook, capture_enabled, set_capture_enabled,
    toggle_capture, utf16_units,
};
