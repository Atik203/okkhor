//! Caret anchoring and popup placement.

use std::mem::size_of;
use std::sync::{Arc, Condvar, Mutex};

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

/// One published caret resolve: the anchor, the password probe, and the
/// generation that requested it. `Copy` so only this value crosses threads;
/// the popup HWND/GDI state never leaves the daemon worker.
#[derive(Debug, Clone, Copy)]
pub struct CaretSample {
    pub anchor: CaretAnchor,
    pub password: bool,
    pub generation: u64,
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
    /// Whether COM was initialized on this thread. `CoInitializeEx` is
    /// idempotent but returning `S_FALSE` on every resolve still costs a COM
    /// call, so initialize once per thread.
    static COM_INITIALIZED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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

/// Initialize COM once for the current thread. MTA is right for a worker that
/// never pumps UI messages; an already-initialized apartment (`S_FALSE` or
/// `RPC_E_CHANGED_MODE`) still supports UIA client calls.
fn ensure_com_initialized() {
    COM_INITIALIZED.with(|initialized| {
        if initialized.get() {
            return;
        }
        // SAFETY: a null reserved pointer is the documented form. The result
        // is deliberately ignored: a thread already in another apartment can
        // still call UIA, and no out parameter is written.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        initialized.set(true);
    });
}

/// Lazily create (once per thread) the UIA client.
#[cfg(windows)]
fn uia_automation() -> Option<IUIAutomation> {
    ensure_com_initialized();
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

/// Latest-wins caret resolver.
///
/// One named thread owns COM/UIA and resolves `caret_anchor()` plus the
/// password probe. The daemon requests a resolve without blocking and later
/// reads the newest published [`CaretSample`]; requests that arrive while a
/// resolve is in flight coalesce to the newest generation, and a result whose
/// generation was superseded is dropped. Only `CaretSample` (`Copy`) crosses
/// threads, so the popup HWND and GDI state stay on the daemon worker.
pub struct CaretTracker {
    shared: Arc<TrackerShared>,
    worker: Option<std::thread::JoinHandle<()>>,
}

struct TrackerShared {
    state: Mutex<TrackerState>,
    signal: Condvar,
}

struct TrackerState {
    running: bool,
    generation: u64,
    pending: Option<u64>,
    latest: Option<CaretSample>,
}

impl CaretTracker {
    /// Spawn the production resolver (UIA caret plus password probe).
    pub fn new() -> std::io::Result<Self> {
        Self::spawn()
    }

    /// Spawn the production resolver (UIA caret plus password probe).
    pub fn spawn() -> std::io::Result<Self> {
        Self::spawn_with(|| {
            (
                caret_anchor().unwrap_or_else(fallback_anchor),
                focused_element_is_password(),
            )
        })
    }

    /// Spawn with a deterministic resolver. Test seam only: production code
    /// must use [`CaretTracker::spawn`] so UIA stays on this one thread.
    pub fn spawn_with<F>(resolver: F) -> std::io::Result<Self>
    where
        F: Fn() -> (CaretAnchor, bool) + Send + 'static,
    {
        let shared = Arc::new(TrackerShared {
            state: Mutex::new(TrackerState {
                running: true,
                generation: 0,
                pending: None,
                latest: None,
            }),
            signal: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("okkhor-caret".to_owned())
            .spawn(move || run_tracker(worker_shared, resolver))?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    /// Ask for a new resolve without blocking. If a resolve is already
    /// pending, the newer generation replaces it (latest wins). Returns the
    /// queued generation, or `0` when the tracker has stopped.
    pub fn request(&self) -> u64 {
        let mut state = lock_state(&self.shared);
        if !state.running {
            return 0;
        }
        state.generation = state.generation.wrapping_add(1);
        state.pending = Some(state.generation);
        let generation = state.generation;
        drop(state);
        self.shared.signal.notify_one();
        generation
    }

    /// The newest published sample, or `None` before the first resolve.
    pub fn latest(&self) -> Option<CaretSample> {
        lock_state(&self.shared).latest
    }

    /// Stop the resolver and wait for the thread to exit. Safe to call twice.
    pub fn stop(&mut self) {
        {
            let mut state = lock_state(&self.shared);
            state.running = false;
            state.pending = None;
        }
        self.shared.signal.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for CaretTracker {
    fn drop(&mut self) {
        // Signal but do not join: a UI Automation call can hang, and joining
        // here would block daemon shutdown. The OS reclaims the thread when
        // the process exits; `stop()` remains the explicit join used by tests.
        {
            let mut state = lock_state(&self.shared);
            state.running = false;
            state.pending = None;
        }
        self.shared.signal.notify_all();
    }
}

impl TrackerState {
    fn take_pending(&mut self) -> Option<u64> {
        if !self.running {
            return None;
        }
        self.pending.take()
    }
}

fn lock_state(shared: &TrackerShared) -> std::sync::MutexGuard<'_, TrackerState> {
    shared
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn run_tracker<F>(shared: Arc<TrackerShared>, resolver: F)
where
    F: Fn() -> (CaretAnchor, bool),
{
    // COM is apartment-thread-affine: initialize it once on this thread, then
    // every resolve reuses the thread-local UIA client.
    ensure_com_initialized();
    // If the resolver panics, mark the tracker stopped so later requests do
    // not queue forever behind a dead thread.
    let _guard = RunningGuard(Arc::clone(&shared));
    loop {
        let generation = {
            let mut state = lock_state(&shared);
            loop {
                if let Some(generation) = state.take_pending() {
                    break generation;
                }
                if !state.running {
                    return;
                }
                state = shared
                    .signal
                    .wait(state)
                    .unwrap_or_else(|error| error.into_inner());
            }
        };
        let (anchor, password) = resolver();
        let mut state = lock_state(&shared);
        // Publish a completed resolve even if a newer one is already queued.
        // The popup would rather use the previous field's caret for one frame
        // than lose the anchor entirely and hide; the newer sample replaces it
        // as soon as it lands.
        if state.running
            && state
                .latest
                .as_ref()
                .is_none_or(|sample| sample.generation < generation)
        {
            state.latest = Some(CaretSample {
                anchor,
                password,
                generation,
            });
        }
    }
}

/// Marks the shared state stopped when the worker exits, including on panic.
struct RunningGuard(Arc<TrackerShared>);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        {
            let mut state = lock_state(&self.0);
            state.running = false;
            state.pending = None;
        }
        self.0.signal.notify_all();
    }
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
    // Keep the card just under the caret: a small vertical gap and a small
    // leftward inset so the caret sits at the card's leading edge instead of
    // the card appearing further to the right of the typing position.
    let gap = ((2_i64 * i64::from(anchor.dpi)) / 96).max(1) as i32;
    let inset = ((2_i64 * i64::from(anchor.dpi)) / 96).max(1) as i32;

    let below_y = anchor.rect.bottom.saturating_add(gap);
    let above_y = anchor.rect.top.saturating_sub(gap).saturating_sub(height);
    let below_fits = below_y.saturating_add(height) <= work.bottom;
    let above_fits = above_y >= work.top;
    let above = !below_fits && above_fits;

    let max_x = (work.right - width).max(work.left);
    let max_y = (work.bottom - height).max(work.top);
    let x = anchor
        .rect
        .left
        .saturating_sub(inset)
        .clamp(work.left, max_x);
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
    use super::{CaretAnchor, CaretSample, CaretTracker, place_popup};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::RECT;

    const TIMEOUT: Duration = Duration::from_secs(2);

    fn test_anchor() -> CaretAnchor {
        CaretAnchor {
            rect: RECT {
                left: 100,
                top: 100,
                right: 101,
                bottom: 120,
            },
            dpi: 96,
            work_area: RECT {
                left: 0,
                top: 0,
                right: 800,
                bottom: 600,
            },
        }
    }

    /// A tracker whose first resolve blocks until the test releases it, so the
    /// test can deterministically queue competing generations.
    struct GatedTracker {
        tracker: CaretTracker,
        started: mpsc::Receiver<()>,
        release: mpsc::Sender<()>,
    }

    fn gated_tracker() -> GatedTracker {
        let (started_tx, started) = mpsc::channel();
        let (release, release_rx) = mpsc::channel::<()>();
        let tracker = CaretTracker::spawn_with(move || {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
            (test_anchor(), false)
        })
        .expect("caret thread starts");
        GatedTracker {
            tracker,
            started,
            release,
        }
    }

    fn wait_for_generation(tracker: &CaretTracker, generation: u64) -> CaretSample {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(sample) = tracker.latest() {
                if sample.generation == generation {
                    return sample;
                }
            }
            assert!(
                Instant::now() < deadline,
                "no sample for generation {generation}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn latest_is_none_before_the_first_resolve() {
        let mut tracker =
            CaretTracker::spawn_with(|| (test_anchor(), false)).expect("caret thread starts");
        assert!(tracker.latest().is_none());
        tracker.stop();
    }

    #[test]
    fn coalesces_to_the_newest_pending_generation() {
        let mut harness = gated_tracker();
        let _ = harness.tracker.request();
        harness
            .started
            .recv_timeout(TIMEOUT)
            .expect("first resolve starts");
        let _ = harness.tracker.request();
        let _ = harness.tracker.request();
        harness.release.send(()).expect("release first resolve");
        harness
            .started
            .recv_timeout(TIMEOUT)
            .expect("coalesced resolve starts");
        harness.release.send(()).expect("release coalesced resolve");
        let sample = wait_for_generation(&harness.tracker, 3);
        assert_eq!(sample.generation, 3);
        harness.tracker.stop();
    }

    #[test]
    fn superseded_result_still_publishes_an_anchor() {
        let mut harness = gated_tracker();
        let _ = harness.tracker.request();
        harness
            .started
            .recv_timeout(TIMEOUT)
            .expect("first resolve starts");
        let _ = harness.tracker.request();
        harness.release.send(()).expect("release stale resolve");
        harness
            .started
            .recv_timeout(TIMEOUT)
            .expect("newer resolve starts");
        // The first resolve finished after generation 2 was requested. Its
        // anchor is still a better fallback than no anchor at all; the newer
        // sample replaces it when it lands.
        let stale = harness.tracker.latest().expect("stale anchor published");
        assert_eq!(stale.generation, 1);
        harness.release.send(()).expect("release newer resolve");
        let sample = wait_for_generation(&harness.tracker, 2);
        assert_eq!(sample.generation, 2);
        harness.tracker.stop();
    }

    #[test]
    fn request_returns_the_queued_generation() {
        let mut tracker =
            CaretTracker::spawn_with(|| (test_anchor(), false)).expect("caret thread starts");
        assert_eq!(tracker.request(), 1);
        assert_eq!(tracker.request(), 2);
        tracker.stop();
        assert_eq!(tracker.request(), 0);
    }

    #[test]
    fn stop_joins_and_ignores_late_requests() {
        let mut tracker =
            CaretTracker::spawn_with(|| (test_anchor(), false)).expect("caret thread starts");
        let _ = tracker.request();
        tracker.stop();
        let _ = tracker.request();
        tracker.stop();
    }

    #[test]
    fn slow_resolver_does_not_panic_or_block_stop() {
        let (started_tx, started_rx) = mpsc::channel();
        let mut tracker = CaretTracker::spawn_with(move || {
            let _ = started_tx.send(());
            std::thread::sleep(Duration::from_millis(30));
            (test_anchor(), true)
        })
        .expect("caret thread starts");
        let _ = tracker.request();
        started_rx
            .recv_timeout(TIMEOUT)
            .expect("slow resolve starts");
        let stop_started = Instant::now();
        tracker.stop();
        assert!(stop_started.elapsed() < TIMEOUT);
    }

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
        assert_eq!(placement.x, 98);
        assert_eq!(placement.y, 122);
        assert!(!placement.above);
    }

    #[test]
    fn flips_above_when_below_does_not_fit() {
        let placement = place_popup(&anchor(100, 550, 101, 580, work()), 200, 100);
        assert!(placement.above);
        assert_eq!(placement.y, 448);
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
        assert_eq!(placement.y, 124);
    }
}
