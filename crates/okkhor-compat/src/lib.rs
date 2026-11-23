//! Safe FFI boundary for legacy Okkhor adapters (contract §5).
//!
//! Temporary migration aid per ADR 0005: validates DLL paths against an
//! explicit daemon-owned allowlist and exposes per-adapter feature flags.
//! No adapter is on the keydown path; loading happens once at daemon startup.
//!
//! NOTE: there is deliberately no crate-level `#![forbid(unsafe_code)]`.
//! All `unsafe` in this crate is confined to the private `ffi` module on
//! `cfg(windows)`; every `unsafe` block there carries a `// SAFETY:` comment.

pub mod adapter;
pub mod error;
pub mod path;

#[cfg(windows)]
mod ffi; // private: the ONLY module allowed to contain unsafe blocks

pub use adapter::{AdapterHandle, AdapterKind, AdapterRegistry, AdapterStatus};
pub use error::CompatError;
pub use path::{DllPathPolicy, ValidatedDllPath};
