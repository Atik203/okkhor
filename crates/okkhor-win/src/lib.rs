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

pub mod dpapi;
pub mod fonts;
pub mod inject;
pub mod input;

#[cfg(windows)]
pub mod autostart;
#[cfg(windows)]
mod caret;
#[cfg(windows)]
mod clipboard;
#[cfg(windows)]
mod hotkey;
#[cfg(windows)]
mod popup;
#[cfg(windows)]
mod quickbar_show;
#[cfg(windows)]
mod single_instance;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod win32;

/// Short feedback sounds (toggle click).
mod sound;

pub use dpapi::{DpapiError, MAX_DPAPI_BYTES, protect, unprotect};
pub use fonts::{
    BUNDLED_FONTS, BijoyFontChoice, BundledFont, EXPECTED_FONT_HASHES, FontError,
    FontInstallationProbe, FontRole, InstallOptions, InstalledFont, RemovedFont, SystemFontProbe,
    font_family_name, font_registry_value_name, install_font_per_user,
    install_font_per_user_with_options, is_font_installed, list_bundled_fonts, resolve_bijoy_font,
    resolve_bijoy_font_with_probe, uninstall_font_per_user, uninstall_font_per_user_with_options,
    user_fonts_dir, validate_font_path, verify_font_hash, verify_staged_font,
};
pub use inject::{InjectError, InjectionMethod, TextInjector};
pub use input::{
    KeyEvent, KeyPhase, KeyboardHook, LLKHF_INJECTED, is_injected_event, should_enqueue,
};

#[cfg(windows)]
pub use autostart::{
    AUTOSTART_KEY, AUTOSTART_VALUE_NAME, AutostartError, DAEMON_ARGUMENT, autostart_command,
    disable as disable_autostart, enable as enable_autostart, is_enabled as is_autostart_enabled,
};
#[cfg(windows)]
pub use caret::{
    CaretAnchor, DpiError, PopupPlacement, caret_anchor, enable_per_monitor_dpi_awareness,
    focused_element_is_password, place_popup,
};
#[cfg(windows)]
pub use hotkey::{GlobalHotkey, HotkeyError, HotkeySpec, ModeBinding, ModeHotkey, PauseHotkey};
#[cfg(windows)]
pub use popup::{MAX_ITEMS, PopupError, PopupMetrics, PopupTheme, SuggestionPopup};
pub use quickbar_show::{
    QUICKBAR_SHOW_EVENT, QuickbarShowError, QuickbarShowWaiter, signal_quickbar_show,
    spawn_quickbar_show_listener,
};
#[cfg(windows)]
pub use single_instance::{SingleInstance, SingleInstanceError};
pub use sound::play_toggle_click;
#[cfg(windows)]
pub use tray::{TrayError, TrayEvent, TrayIcon, TrayMode};
#[cfg(windows)]
pub use win32::{
    HookError, MAX_SEND_BACKSPACES, WindowsInjector, WindowsKeyboardHook, capture_enabled,
    ctrl_or_alt_pressed, foreground_window_id, inject_commit, send_backspaces, send_enter,
    send_tab, set_capture_enabled, set_composition_state, set_english_popup, toggle_capture,
    utf16_units,
};
