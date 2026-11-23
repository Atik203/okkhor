//! Quick control bar helpers (ADR 0014 §5, as amended 2026-11-23).
//!
//! The bar is a second Tauri window (`quickbar`) in this UI process — not a
//! native daemon window — so typing/daemon independence holds while the UI
//! lives, and the bar disappears if the UI process dies (documented in
//! `docs/ui.md`). Clicking the bar must never steal focus: on Windows the
//! setup path applies `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST`
//! to the bar's HWND (see [`native`]) and shows it with `SW_SHOWNOACTIVATE`.
//!
//! This module keeps every `unsafe` block in the explicitly-reviewed
//! [`native`] submodule with `SAFETY` comments (ADR 0012 §5). Everything else
//! here is pure and unit-tested, including `--quickbar` arg parsing and the
//! top-right/DPI position math.

use serde::Serialize;

/// Logical size of the expanded bar (matches `tauri.conf.json`).
pub const QUICKBAR_LOGICAL_W: u32 = 460;
/// Logical height of the expanded bar (matches `tauri.conf.json`).
pub const QUICKBAR_LOGICAL_H: u32 = 60;
/// Logical size of the collapsed orb (square).
pub const ORB_LOGICAL: u32 = 44;
/// Logical margin from the screen edges.
pub const EDGE_MARGIN_LOGICAL: u32 = 12;

/// Snapshot surfaced to the `/quickbar` view. No user text is ever included:
/// only mode/encoding names, pause state, and bar chrome state.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuickbarState {
    pub input_mode: okkhor_ipc::settings::InputMode,
    pub output_encoding: okkhor_ipc::settings::OutputEncoding,
    pub paused: bool,
    pub available: bool,
    pub collapsed: bool,
    pub opacity: f32,
    /// The `quickbar_visible` setting. The setup task uses it to decide
    /// whether a normal (non-`--quickbar`) launch also shows the bar.
    pub visible: bool,
    /// Saved physical position after a drag (`None` = automatic top-right).
    pub x: Option<i32>,
    pub y: Option<i32>,
}

impl QuickbarState {
    /// Local fallback shown when the daemon is unreachable. Bar chrome stays
    /// interactive (collapse/expand, open settings); mode actions report
    /// `daemon_unavailable` like the rest of the shell. `visible` is false
    /// so a normal launch never pops the bar without a readable setting.
    #[must_use]
    pub fn unavailable(collapsed: bool, opacity: f32) -> Self {
        Self {
            input_mode: okkhor_ipc::settings::InputMode::Bangla,
            output_encoding: okkhor_ipc::settings::OutputEncoding::Unicode,
            paused: false,
            available: false,
            collapsed,
            opacity,
            visible: false,
            x: None,
            y: None,
        }
    }
}

/// True when the process was launched to host the bar: an exact `--quickbar`
/// argument anywhere in the command line (Tauri forwards user args after its
/// own, so position is not fixed).
#[must_use]
pub fn is_quickbar_launch(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--quickbar")
}

/// Top-right position of a window in *physical* pixels, given the monitor's
/// origin/size (physical pixels) and its scale factor. The window's logical
/// size and the logical edge margin are scaled by `scale` and rounded; the
/// result is clamped so the window never leaves the monitor on tiny displays.
///
/// `scale <= 0` or non-finite scales fall back to 1.0 (never panic, never
/// place off-screen).
#[must_use]
pub fn quickbar_top_right_physical(
    monitor_origin: (i32, i32),
    monitor_size: (u32, u32),
    window_logical: (u32, u32),
    scale: f64,
    margin_logical: u32,
) -> (i32, i32) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let win_w = (f64::from(window_logical.0) * scale).round() as i32;
    let win_h = (f64::from(window_logical.1) * scale).round() as i32;
    let margin = (f64::from(margin_logical) * scale).round() as i32;
    let x = monitor_origin
        .0
        .saturating_add(monitor_size.0 as i32)
        .saturating_sub(win_w)
        .saturating_sub(margin);
    let y = monitor_origin.1.saturating_add(margin);
    // Clamp onto the monitor so the bar stays reachable and fully visible on
    // tiny displays: left/top edges, then the bottom edge.
    let x = x.max(monitor_origin.0);
    let max_y = monitor_origin
        .1
        .saturating_add(monitor_size.1 as i32)
        .saturating_sub(win_h)
        .max(monitor_origin.1);
    let y = y.max(monitor_origin.1).min(max_y);
    (x, y)
}

/// Win32 no-activate styling. Windows-only; on other platforms focusing rules
/// are left to the window manager (tests and CI still run there).
#[cfg(windows)]
#[allow(unsafe_code)]
pub mod native {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, HWND_TOPMOST, SET_WINDOW_POS_FLAGS, SHOW_WINDOW_CMD, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    };

    fn ex_style_bits() -> isize {
        (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0) as isize
    }

    fn show_flags() -> SET_WINDOW_POS_FLAGS {
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW
    }

    /// Applies `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST` and pins
    /// the window topmost without activating it.
    pub fn apply_noactivate(hwnd: HWND) {
        // SAFETY: `hwnd` is the live quickbar WebView HWND obtained from
        // Tauri's `window.hwnd()` on the UI thread during setup or a quickbar
        // command. `GetWindowLongPtrW`/`SetWindowLongPtrW` with `GWL_EXSTYLE`
        // only touch this window's style bits; OR-ing preserves all existing
        // bits. `SetWindowPos` with `SWP_NOMOVE | SWP_NOSIZE` changes neither
        // geometry nor Z-order beyond topmost, and `SWP_NOACTIVATE` guarantees
        // the foreground window keeps focus.
        unsafe {
            let previous =
                windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
                hwnd,
                GWL_EXSTYLE,
                previous | ex_style_bits(),
            );
            let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                show_flags(),
            );
        }
    }

    /// Shows the window without activating it (`SW_SHOWNOACTIVATE`).
    pub fn show_noactivate(hwnd: HWND) {
        // SAFETY: `hwnd` is the live quickbar WebView HWND as above;
        // `ShowWindow` with `SW_SHOWNOACTIVATE` only changes visibility and
        // never moves focus. The return value (previous visibility) is
        // intentionally ignored.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    }

    /// Begins a native horizontal resize that follows the physical mouse even
    /// when the pointer leaves this small window. Runs on a helper thread and
    /// never activates the bar; ends on left-button release.
    ///
    /// `start_width_logical` is the bar's current logical width
    /// (320-720); height is held at 60 logical.
    pub fn begin_resize(hwnd: HWND, start_width_logical: u32) {
        use std::time::Duration;
        use windows::Win32::UI::HiDpi::GetDpiForWindow;
        use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
        };

        let raw = hwnd.0 as isize;
        let start_width = start_width_logical.clamp(320, 720);
        std::thread::Builder::new()
            .name("okkhor-quickbar-resize".to_owned())
            .spawn(move || {
                // SAFETY: the HWND was live when the command started and the
                // bar outlives this 15 ms poll loop; pointer-sized handle
                // round-trips through isize only to cross the thread boundary.
                let hwnd = HWND(raw as *mut core::ffi::c_void);
                let mut start = windows::Win32::Foundation::POINT::default();
                // SAFETY: `start` is valid local storage.
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut start);
                }
                // SAFETY: the HWND is live; GetDpiForWindow only reads the
                // window's DPI and never dereferences Rust memory.
                let dpi = unsafe { GetDpiForWindow(hwnd) };
                let scale = if dpi == 0 { 1.0 } else { f64::from(dpi) / 96.0 };
                loop {
                    std::thread::sleep(Duration::from_millis(15));
                    // SAFETY: GetAsyncKeyState is a read-only query.
                    let pressed = unsafe { GetAsyncKeyState(0x01) } < 0;
                    if !pressed {
                        break;
                    }
                    let mut point = windows::Win32::Foundation::POINT::default();
                    // SAFETY: `point` is valid local storage.
                    unsafe {
                        let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point);
                    }
                    let dx = f64::from(point.x - start.x) / scale;
                    let logical = (f64::from(start_width) + dx).round().clamp(320.0, 720.0);
                    let physical_w = (logical * scale).round() as i32;
                    let physical_h = (60.0 * scale).round() as i32;
                    // SAFETY: the HWND is live; SetWindowPos with SWP_NOMOVE
                    // and SWP_NOACTIVATE changes only the size and never takes
                    // focus from the foreground app.
                    unsafe {
                        let _ = SetWindowPos(
                            hwnd,
                            None,
                            0,
                            0,
                            physical_w,
                            physical_h,
                            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                }
            })
            .ok();
    }

    /// Hides the window natively (`SW_HIDE`).
    ///
    /// The bar is shown through raw `ShowWindow`, so Tauri's own visibility
    /// bookkeeping can still believe it is hidden; `Window::hide()` may then
    /// skip the native call. Hiding through the same Win32 path is reliable.
    pub fn hide(hwnd: HWND) {
        // SAFETY: `hwnd` is the live quickbar WebView HWND; `SW_HIDE` only
        // changes visibility and never activates or moves the window.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(
                hwnd,
                windows::Win32::UI::WindowsAndMessaging::SW_HIDE,
            );
        }
    }

    #[allow(dead_code)]
    fn _assert_cmd_repr(cmd: SHOW_WINDOW_CMD) -> i32 {
        cmd.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quickbar_flag_matches_exact_arg_only() {
        let args = vec!["okkhor-ui".to_owned(), "--quickbar".to_owned()];
        assert!(is_quickbar_launch(&args));
        // Tauri forwards its own flags first; position must not matter.
        let args = vec![
            "okkhor-ui".to_owned(),
            "--tauri".to_owned(),
            "--quickbar".to_owned(),
        ];
        assert!(is_quickbar_launch(&args));
        // Prefixes, values, and similarly-named flags must not match.
        for argv in [
            vec!["okkhor-ui"],
            vec!["okkhor-ui", "--quickbar=true"],
            vec!["okkhor-ui", "--quickbar ", "--quickbarx"],
            vec!["okkhor-ui", "-quickbar"],
        ] {
            let owned: Vec<String> = argv.iter().map(std::string::ToString::to_string).collect();
            assert!(!is_quickbar_launch(&owned), "{argv:?}");
        }
    }

    #[test]
    fn top_right_at_scale_one() {
        // 1920×1080 monitor at origin: bar hugs the top-right with margin.
        let (x, y) = quickbar_top_right_physical(
            (0, 0),
            (1920, 1080),
            (QUICKBAR_LOGICAL_W, QUICKBAR_LOGICAL_H),
            1.0,
            EDGE_MARGIN_LOGICAL,
        );
        assert_eq!((x, y), (1920 - 460 - 12, 12));
    }

    #[test]
    fn orb_and_dpi_scaling() {
        // 200% DPI (150% style factor 1.5 on the orb): physical sizes scale.
        let (x, y) = quickbar_top_right_physical(
            (0, 0),
            (1920, 1080),
            (QUICKBAR_LOGICAL_W, QUICKBAR_LOGICAL_H),
            1.5,
            EDGE_MARGIN_LOGICAL,
        );
        assert_eq!((x, y), (1920 - 690 - 18, 18));
        let (ox, oy) = quickbar_top_right_physical(
            (0, 0),
            (1920, 1080),
            (ORB_LOGICAL, ORB_LOGICAL),
            2.0,
            EDGE_MARGIN_LOGICAL,
        );
        assert_eq!((ox, oy), (1920 - 88 - 24, 24));
    }

    #[test]
    fn secondary_monitor_origin_is_respected() {
        let (x, y) = quickbar_top_right_physical(
            (-1920, 0),
            (1920, 1080),
            (QUICKBAR_LOGICAL_W, QUICKBAR_LOGICAL_H),
            1.0,
            EDGE_MARGIN_LOGICAL,
        );
        assert_eq!((x, y), (-1920 + 1920 - 460 - 12, 12));
    }

    #[test]
    fn degenerate_scale_falls_back_to_one_and_clamps() {
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let (x, y) = quickbar_top_right_physical(
                (0, 0),
                (1920, 1080),
                (QUICKBAR_LOGICAL_W, QUICKBAR_LOGICAL_H),
                scale,
                EDGE_MARGIN_LOGICAL,
            );
            assert_eq!((x, y), (1920 - 460 - 12, 12), "scale {scale}");
        }
        // Tiny monitor: clamp onto the origin instead of going negative.
        let (x, y) = quickbar_top_right_physical(
            (100, 100),
            (200, 200),
            (QUICKBAR_LOGICAL_W, QUICKBAR_LOGICAL_H),
            1.0,
            EDGE_MARGIN_LOGICAL,
        );
        assert_eq!((x, y), (100, 112));
    }

    #[test]
    fn short_monitor_clamps_the_bottom_edge() {
        // 200×40 strip: the 60 px bar cannot fit below the margin, so it is
        // pinned to the origin instead of overflowing.
        let (x, y) = quickbar_top_right_physical(
            (100, 100),
            (200, 40),
            (QUICKBAR_LOGICAL_W, QUICKBAR_LOGICAL_H),
            1.0,
            EDGE_MARGIN_LOGICAL,
        );
        assert_eq!((x, y), (100, 100));
    }

    #[test]
    fn unavailable_state_carries_no_text() {
        let state = QuickbarState::unavailable(false, 0.9);
        assert!(!state.available);
        assert!(!state.paused);
        let rendered = serde_json::to_string(&state).expect("serializes");
        assert!(rendered.contains("bangla"));
        assert!(rendered.contains("unicode"));
    }
}
