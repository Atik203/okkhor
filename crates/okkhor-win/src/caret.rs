//! Caret anchoring and popup placement.

use std::mem::size_of;

use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromWindow,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::System::Ole::SafeArrayDestroy;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern2, UIA_TextPattern2Id,
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

/// Read the foreground caret, or a screen fallback.
///
/// Sources, in order:
///
/// 1. `GetGUIThreadInfo`: exact caret for classic Win32 edit controls
///    (Notepad, dialogs). Fast and allocation-free.
/// 2. UI Automation `TextPattern2::GetCaretRange`: Chromium (Chrome, Edge,
///    Electron, Slack, VS Code) exposes no GUI caret — the old code used
///    `hwndFocus` with a zero rectangle, which pinned the popup to the page's
///    top-left corner. UIA gives the real screen rectangle and follows the
///    caret as the text grows.
/// 3. The mouse position, so a popup never lands at (0,0).
///
/// Coordinates are physical pixels because the process is Per-Monitor V2 DPI
/// aware.
pub fn caret_anchor() -> Option<CaretAnchor> {
    // SAFETY: GetForegroundWindow has no pointer preconditions.
    let foreground = unsafe { GetForegroundWindow() };
    let dpi = if foreground.0.is_null() {
        96
    } else {
        // SAFETY: foreground is a live window; zero is handled below.
        match unsafe { GetDpiForWindow(foreground) } {
            0 => 96,
            dpi => dpi,
        }
    };
    let work_area = if foreground.0.is_null() {
        screen_work_area()
    } else {
        // SAFETY: MonitorFromWindow has no pointer preconditions.
        let monitor = unsafe { MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST) };
        monitor_work_area(monitor).unwrap_or_else(screen_work_area)
    };

    if !foreground.0.is_null() {
        if let Some(rect) = gui_caret_rect(foreground) {
            return Some(anchor_from_rect(rect, dpi, work_area));
        }
        if let Some(rect) = uia_caret_rect(foreground) {
            return Some(anchor_from_rect(rect, dpi, work_area));
        }
    }
    Some(fallback_anchor())
}

/// Screen rectangle of the classic Win32 caret, when the focused control has
/// one. Returns `None` for Chromium-style clients that only report a focus
/// window with an empty caret rectangle.
fn gui_caret_rect(foreground: windows::Win32::Foundation::HWND) -> Option<RECT> {
    // SAFETY: GetWindowThreadProcessId only reads the window's thread id.
    let thread_id = unsafe { GetWindowThreadProcessId(foreground, None) };
    if thread_id == 0 {
        return None;
    }
    let mut info = GUITHREADINFO {
        cbSize: size_of::<GUITHREADINFO>() as u32,
        ..GUITHREADINFO::default()
    };
    // SAFETY: info is a valid GUITHREADINFO with cbSize initialized.
    if unsafe { GetGUIThreadInfo(thread_id, &mut info) }.is_err() {
        return None;
    }
    // Only a real caret window carries `rcCaret`; `hwndFocus` alone is the
    // whole page/view, so using it would place the popup at its (0,0).
    if info.hwndCaret.0.is_null() {
        return None;
    }
    let height = info.rcCaret.bottom - info.rcCaret.top;
    if height <= 0 {
        return None;
    }
    let mut point = POINT {
        x: info.rcCaret.left,
        y: info.rcCaret.bottom,
    };
    // SAFETY: hwndCaret is a live window and point is a valid POINT.
    if !unsafe { ClientToScreen(info.hwndCaret, &mut point) }.as_bool() {
        return None;
    }
    Some(RECT {
        left: point.x,
        top: point.y - height,
        right: point.x + 1,
        bottom: point.y,
    })
}

thread_local! {
    /// UIA client object per thread; COM apartments are thread-affine.
    static UIA: std::cell::RefCell<Option<IUIAutomation>> =
        const { std::cell::RefCell::new(None) };
    /// Focused element cache: (foreground hwnd, taken-at, element).
    ///
    /// `GetFocusedElement` walks the whole UIA tree in Chromium and can cost
    /// tens of milliseconds per keystroke. The caret range itself is cheap, so
    /// the element is reused for a short window (covers a typing burst) and
    /// re-resolved after a click or focus change.
    static UIA_FOCUS: std::cell::RefCell<
        Option<(isize, std::time::Instant, windows::Win32::UI::Accessibility::IUIAutomationElement)>,
    > = const { std::cell::RefCell::new(None) };
}

/// Lazily create (once per thread) the UIA client.
#[cfg(windows)]
fn uia_automation() -> Option<IUIAutomation> {
    // SAFETY: a null reserved pointer is the documented form. MTA is right for
    // a worker that never pumps UI messages; an already-initialized apartment
    // (S_FALSE or RPC_E_CHANGED_MODE) still supports UIA client calls.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    UIA.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            // SAFETY: CUIAutomation is the documented CLSID and the out
            // interface is a valid IUIAutomation.
            *slot = unsafe {
                CoCreateInstance(
                    &CUIAutomation,
                    None::<&windows::core::IUnknown>,
                    CLSCTX_INPROC_SERVER,
                )
            }
            .ok();
        }
        slot.clone()
    })
}

/// Screen rectangle of the UIA text caret (Chromium, Electron, WinUI, WPF).
///
/// `TextPattern2::GetCaretRange` is the precise source; `TextPattern::
/// GetSelection` covers clients that only implement the base pattern. Both
/// return client-area-independent screen coordinates in physical pixels.
#[cfg(windows)]
fn uia_caret_rect(foreground: windows::Win32::Foundation::HWND) -> Option<RECT> {
    let uia = uia_automation()?;
    let focused = focused_element(&uia, foreground)?;

    // SAFETY: same as above; the pattern may be unsupported (Err) or unusual.
    if let Ok(pattern) =
        unsafe { focused.GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id) }
    {
        let mut active = windows::core::BOOL(0);
        // SAFETY: active is a valid BOOL storage.
        if let Ok(range) = unsafe { pattern.GetCaretRange(&mut active) } {
            if let Some(rect) = range_screen_rect(&uia, &range) {
                return Some(rect);
            }
        }
    }

    // SAFETY: base TextPattern selection. The first range is the caret when
    // the selection is collapsed; empty selections return no ranges.
    let base: windows::Win32::UI::Accessibility::IUIAutomationTextPattern = unsafe {
        focused.GetCurrentPatternAs(windows::Win32::UI::Accessibility::UIA_TextPatternId)
    }
    .ok()?;
    let selection = unsafe { base.GetSelection() }.ok()?;
    let count = unsafe { selection.Length() }.ok()?;
    for index in 0..count.max(0) {
        if let Ok(range) = unsafe { selection.GetElement(index) } {
            if let Some(rect) = range_screen_rect(&uia, &range) {
                return Some(rect);
            }
        }
    }
    None
}

/// Whether the focused control is a password field.
///
/// Uses the same cached UIA element as the caret path, so a typing burst does
/// not re-walk the tree. Unknown/unsupported results return `false` (the daemon
/// still has the pause control as the escape hatch).
#[cfg(windows)]
pub fn focused_element_is_password() -> bool {
    // SAFETY: GetForegroundWindow has no pointer preconditions.
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0.is_null() {
        return false;
    }
    let Some(uia) = uia_automation() else {
        return false;
    };
    let Some(element) = focused_element(&uia, foreground) else {
        return false;
    };
    // SAFETY: UIA property read; failure is a typed HRESULT and maps to false.
    unsafe { element.CurrentIsPassword() }
        .map(|value| value.as_bool())
        .unwrap_or(false)
}

/// Focused element for `foreground`, cached briefly to keep typing smooth.
fn focused_element(
    uia: &IUIAutomation,
    foreground: windows::Win32::Foundation::HWND,
) -> Option<windows::Win32::UI::Accessibility::IUIAutomationElement> {
    let key = foreground.0 as isize;
    UIA_FOCUS.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some((cached, at, element)) = slot.as_ref() {
            if *cached == key && at.elapsed() < std::time::Duration::from_millis(500) {
                return Some(element.clone());
            }
        }
        // SAFETY: UIA client call; failure is a typed HRESULT.
        let element = unsafe { uia.GetFocusedElement() }.ok()?;
        *slot = Some((key, std::time::Instant::now(), element.clone()));
        Some(element)
    })
}

/// First screen rectangle of a UIA text range, or `None` when empty.
#[cfg(windows)]
fn range_screen_rect(
    uia: &IUIAutomation,
    range: &windows::Win32::UI::Accessibility::IUIAutomationTextRange,
) -> Option<RECT> {
    // SAFETY: returns an owned SAFEARRAY of doubles (left, top, width, height).
    let array = unsafe { range.GetBoundingRectangles() }.ok()?;
    if array.is_null() {
        return None;
    }
    let mut rects: *mut RECT = std::ptr::null_mut();
    // SAFETY: array is a live SAFEARRAY returned by UIA; rects receives a
    // CoTaskMemAlloc allocation, freed below.
    let count = unsafe { uia.SafeArrayToRectNativeArray(array, &mut rects) }.unwrap_or(0);
    // SAFETY: we own the SAFEARRAY returned by GetBoundingRectangles.
    let _ = unsafe { SafeArrayDestroy(array) };
    let first = if count > 0 && !rects.is_null() {
        // SAFETY: count > 0 means at least one RECT was written.
        Some(unsafe { *rects })
    } else {
        None
    };
    if !rects.is_null() {
        // SAFETY: the allocation came from SafeArrayToRectNativeArray.
        unsafe { CoTaskMemFree(Some(rects as *const core::ffi::c_void)) };
    }
    let rect = first?;
    if rect.bottom <= rect.top || rect.right < rect.left {
        return None;
    }
    Some(rect)
}

fn anchor_from_rect(rect: RECT, dpi: u32, work_area: RECT) -> CaretAnchor {
    CaretAnchor {
        rect: RECT {
            left: rect.left,
            top: rect.top,
            right: (rect.left + 1).max(rect.right),
            bottom: rect.bottom,
        },
        dpi,
        work_area,
    }
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
