//! Caret anchoring and popup placement.

use std::mem::size_of;

use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromWindow,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GUITHREADINFO, GetCursorPos, GetForegroundWindow, GetGUIThreadInfo, GetSystemMetrics,
    GetWindowThreadProcessId, SM_CXSCREEN, SM_CYSCREEN,
};

/// The caret rectangle in physical screen coordinates and its monitor work area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaretAnchor {
    pub rect: RECT,
    pub dpi: u32,
    pub work_area: RECT,
}

/// Where the popup should be placed, in physical screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopupPlacement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub above: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum DpiError {
    #[error("could not set per-monitor DPI awareness: code {code}")]
    SetProcessDpiAwarenessContext { code: i32 },
}

/// Enable Per-Monitor V2 DPI awareness. Call once before creating windows.
pub fn enable_per_monitor_dpi_awareness() -> Result<(), DpiError> {
    // SAFETY: the context is a documented system constant and the call has no
    // pointer preconditions. It is expected to fail if the host already set a
    // process-wide context.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }.map_err(
        |error| DpiError::SetProcessDpiAwarenessContext {
            code: error.code().0,
        },
    )
}

/// Read the foreground caret, or a mouse/screen fallback.
///
/// The returned coordinates are physical pixels because the process should be
/// Per-Monitor V2 DPI aware.
pub fn caret_anchor() -> Option<CaretAnchor> {
    // SAFETY: GetForegroundWindow has no pointer preconditions.
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0.is_null() {
        return Some(fallback_anchor());
    }

    // SAFETY: GetWindowThreadProcessId only reads the window's thread id.
    let thread_id = unsafe { GetWindowThreadProcessId(foreground, None) };
    if thread_id == 0 {
        return Some(fallback_anchor());
    }

    let mut info = GUITHREADINFO {
        cbSize: size_of::<GUITHREADINFO>() as u32,
        ..GUITHREADINFO::default()
    };
    // SAFETY: info is a valid GUITHREADINFO with cbSize initialized.
    if unsafe { GetGUIThreadInfo(thread_id, &mut info) }.is_err() {
        return Some(fallback_anchor());
    }

    let caret_window = if !info.hwndCaret.0.is_null() {
        info.hwndCaret
    } else {
        info.hwndFocus
    };
    if caret_window.0.is_null() {
        return Some(fallback_anchor());
    }

    let mut point = POINT {
        x: info.rcCaret.left,
        y: info.rcCaret.bottom,
    };
    // SAFETY: caret_window is a live window and point is a valid POINT.
    if !unsafe { ClientToScreen(caret_window, &mut point) }.as_bool() {
        return Some(fallback_anchor());
    }

    // SAFETY: foreground is a live window; zero is handled below.
    let dpi = match unsafe { GetDpiForWindow(foreground) } {
        0 => 96,
        dpi => dpi,
    };
    let caret_height = (info.rcCaret.bottom - info.rcCaret.top).max(1);

    // SAFETY: MonitorFromWindow has no pointer preconditions.
    let monitor = unsafe { MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST) };
    let work_area = monitor_work_area(monitor).unwrap_or_else(screen_work_area);

    Some(CaretAnchor {
        rect: RECT {
            left: point.x,
            top: point.y - caret_height,
            right: point.x + 1,
            bottom: point.y,
        },
        dpi,
        work_area,
    })
}

/// Place a popup below the caret, flipping above or clamping when needed.
pub fn place_popup(anchor: &CaretAnchor, width: i32, height: i32) -> PopupPlacement {
    let work = anchor.work_area;
    let work_width = (work.right - work.left).max(1);
    let work_height = (work.bottom - work.top).max(1);
    let width = width.clamp(1, work_width);
    let height = height.clamp(1, work_height);
    let gap = ((4_i64 * i64::from(anchor.dpi)) / 96).max(2) as i32;

    let below_y = anchor.rect.bottom.saturating_add(gap);
    let above_y = anchor.rect.top.saturating_sub(gap).saturating_sub(height);
    let below_fits = below_y.saturating_add(height) <= work.bottom;
    let above_fits = above_y >= work.top;
    let above = !below_fits && above_fits;

    let max_x = (work.right - width).max(work.left);
    let max_y = (work.bottom - height).max(work.top);
    let x = anchor.rect.left.clamp(work.left, max_x);
    let y = if above { above_y } else { below_y }.clamp(work.top, max_y);

    PopupPlacement {
        x,
        y,
        width,
        height,
        above,
    }
}

fn monitor_work_area(monitor: HMONITOR) -> Option<RECT> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..MONITORINFO::default()
    };
    // SAFETY: info is a valid MONITORINFO with cbSize initialized.
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        Some(info.rcWork)
    } else {
        None
    }
}

fn screen_work_area() -> RECT {
    // SAFETY: GetSystemMetrics has no pointer preconditions.
    let width = unsafe { GetSystemMetrics(SM_CXSCREEN) }.max(1);
    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) }.max(1);
    RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    }
}

fn fallback_anchor() -> CaretAnchor {
    let mut point = POINT::default();
    // SAFETY: point is a valid POINT.
    let has_cursor = unsafe { GetCursorPos(&mut point) }.is_ok();
    if !has_cursor {
        point = POINT { x: 0, y: 0 };
    }
    let work_area = screen_work_area();
    CaretAnchor {
        rect: RECT {
            left: point.x,
            top: point.y.saturating_sub(18),
            right: point.x.saturating_add(1),
            bottom: point.y,
        },
        dpi: 96,
        work_area,
    }
}

#[cfg(test)]
mod tests {
    use super::{CaretAnchor, place_popup};
    use windows::Win32::Foundation::RECT;

    fn anchor(left: i32, top: i32, right: i32, bottom: i32, work: RECT) -> CaretAnchor {
        CaretAnchor {
            rect: RECT {
                left,
                top,
                right,
                bottom,
            },
            dpi: 96,
            work_area: work,
        }
    }

    fn work() -> RECT {
        RECT {
            left: 0,
            top: 0,
            right: 800,
            bottom: 600,
        }
    }

    #[test]
    fn places_below_the_caret_when_it_fits() {
        let placement = place_popup(&anchor(100, 100, 101, 120, work()), 200, 100);
        assert_eq!(placement.x, 100);
        assert_eq!(placement.y, 124);
        assert!(!placement.above);
    }

    #[test]
    fn flips_above_when_below_does_not_fit() {
        let placement = place_popup(&anchor(100, 550, 101, 580, work()), 200, 100);
        assert!(placement.above);
        assert_eq!(placement.y, 446);
    }

    #[test]
    fn clamps_to_the_right_edge() {
        let placement = place_popup(&anchor(780, 100, 781, 120, work()), 200, 100);
        assert_eq!(placement.x, 600);
    }

    #[test]
    fn clamps_oversized_popup_to_work_area() {
        let placement = place_popup(&anchor(50, 50, 51, 70, work()), 2_000, 2_000);
        assert_eq!(placement.width, 800);
        assert_eq!(placement.height, 600);
        assert_eq!(placement.x, 0);
        assert_eq!(placement.y, 0);
    }

    #[test]
    fn dpi_scales_the_gap() {
        let mut anchor = anchor(100, 100, 101, 120, work());
        anchor.dpi = 192;
        let placement = place_popup(&anchor, 200, 100);
        assert_eq!(placement.y, 128);
    }
}
