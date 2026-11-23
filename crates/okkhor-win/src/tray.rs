//! Minimal Windows notification-area icon.
//!
//! The tray owns a hidden message-only window on a dedicated thread, so it
//! never touches the keyboard hook or the suggestion popup. The menu is built on
//! demand and contains Bangla/English mode, Pause/Resume, Settings (launches
//! the `okkhor-ui` process; the daemon reports a missing UI with a balloon),
//! and Exit.
//!
//! This is deliberately a scaffold for the 0.2 tray lifecycle: it implements
//! add, remove, tooltip update, balloon, and menu events. Remaining integration:
//!
//! - Re-add the icon when Explorer broadcasts `TaskbarCreated`; that belongs to
//!   the 0.2 tray hardening follow-up.
//! - The daemon currently polls `try_next`; a future version can use the same
//!   channel from an event-driven loop without changing this API.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, HWND_MESSAGE, IDI_APPLICATION, LoadIconW,
    MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
    SetForegroundWindow, TPM_BOTTOMALIGN, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage,
    UnregisterClassW, WM_APP, WM_CLOSE, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONUP,
    WM_NULL, WM_RBUTTONUP, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{PCWSTR, w};

const TRAY_WINDOW_CLASS: PCWSTR = w!("OkkhorTrayWindow.v1");
const TRAY_ICON_ID: u32 = 1;
const WM_TRAY_CALLBACK: u32 = WM_APP + 1;
const WM_TRAY_REFRESH: u32 = WM_APP + 2;
const WM_TRAY_BALLOON: u32 = WM_APP + 3;

const MENU_PAUSE: usize = 1;
const MENU_SETTINGS: usize = 2;
const MENU_EXIT: usize = 3;
const MENU_MODE: usize = 4;

/// Events delivered by the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// The user chose Pause or Resume from the tray menu.
    TogglePause,
    /// The user chose Bangla/English, single-clicked, or picked the mode menu.
    ToggleMode,
    /// The user chose Settings: the daemon launches the settings UI.
    Settings,
    /// The user chose Exit.
    Exit,
}

/// Input mode shown by the tray (ADR 0014 §1). The tray owns display state
/// only; composition gating lives in the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrayMode {
    #[default]
    Bangla,
    English,
}

impl TrayMode {
    /// Stable label for the menu/tooltip (ASCII only; the toast carries Bangla).
    pub const fn label(self) -> &'static str {
        match self {
            Self::Bangla => "Bangla",
            Self::English => "English",
        }
    }

    /// Menu action label from the current mode.
    pub const fn switch_label(self) -> &'static str {
        match self {
            Self::Bangla => "Switch to English",
            Self::English => "Switch to Bangla",
        }
    }

    pub const fn toggle(self) -> Self {
        match self {
            Self::Bangla => Self::English,
            Self::English => Self::Bangla,
        }
    }

    pub const fn is_english(self) -> bool {
        matches!(self, Self::English)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TrayError {
    #[error("a tray icon is already active in this process")]
    AlreadyRunning,
    #[error("could not start the tray thread: {0}")]
    Spawn(#[from] io::Error),
    #[error("the tray thread exited before it was ready")]
    ThreadGone,
    #[error("Win32 call {operation} failed with code {code}")]
    Win32 { operation: &'static str, code: i32 },
}

/// One process-wide tray event channel. The window procedure lives behind an
/// `extern "system"` function pointer and cannot capture state, so the sender is
/// stored here and cleared when the tray thread exits.
static EVENT_TX: Mutex<Option<SyncSender<TrayEvent>>> = Mutex::new(None);
/// Pending balloon text for [`TrayIcon::show_balloon`]. Fixed literals from the
/// daemon only (never user text or paths); the tray thread renders them.
static BALLOON: Mutex<Option<(String, String)>> = Mutex::new(None);
static PAUSED: AtomicBool = AtomicBool::new(false);
static MODE_ENGLISH: AtomicBool = AtomicBool::new(false);

/// A notification-area icon with a hidden message window.
pub struct TrayIcon {
    window: HWND,
    thread: Option<JoinHandle<()>>,
    events: Receiver<TrayEvent>,
}

impl TrayIcon {
    /// Create the tray icon. The icon is visible as soon as this returns.
    pub fn new() -> Result<Self, TrayError> {
        if event_sender().is_some() {
            return Err(TrayError::AlreadyRunning);
        }
        PAUSED.store(false, Ordering::Release);
        MODE_ENGLISH.store(false, Ordering::Release);

        let (event_tx, event_rx) = mpsc::sync_channel(16);
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("okkhor-tray".to_owned())
            .spawn(move || run_tray_thread(event_tx, ready_tx))
            .map_err(TrayError::Spawn)?;

        match ready_rx.recv() {
            Ok(Ok(window)) => Ok(Self {
                // The window handle is sent as isize because raw handles are not
                // Send; it was created on the tray thread and remains valid
                // until that thread processes WM_CLOSE.
                window: HWND(window as *mut core::ffi::c_void),
                thread: Some(thread),
                events: event_rx,
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(TrayError::ThreadGone)
            }
        }
    }

    /// Receive one tray event without blocking.
    pub fn try_next(&self) -> Option<TrayEvent> {
        self.events.try_recv().ok()
    }

    pub fn is_running(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }

    /// Update the menu and tooltip after the capture state changes.
    pub fn set_paused(&self, paused: bool) {
        PAUSED.store(paused, Ordering::Release);
        self.refresh();
    }

    pub fn paused(&self) -> bool {
        PAUSED.load(Ordering::Acquire)
    }

    /// Update the mode label and tooltip after an input-mode change.
    /// Never steals focus: it only posts a refresh to the tray thread.
    pub fn set_mode(&self, mode: TrayMode) {
        MODE_ENGLISH.store(mode.is_english(), Ordering::Release);
        self.refresh();
    }

    pub fn mode(&self) -> TrayMode {
        if MODE_ENGLISH.load(Ordering::Acquire) {
            TrayMode::English
        } else {
            TrayMode::Bangla
        }
    }

    /// Current mode label (`Bangla` / `English`) for status lines.
    pub fn mode_label(&self) -> &'static str {
        self.mode().label()
    }

    /// Ask the tray thread to rebuild its tooltip from the current state.
    pub fn refresh(&self) {
        // SAFETY: the window is owned by the tray thread and is only destroyed
        // by WM_CLOSE, which remove() posts before joining. Posting a private
        // message to a stale handle fails harmlessly.
        unsafe {
            let _ = PostMessageW(Some(self.window), WM_TRAY_REFRESH, WPARAM(0), LPARAM(0));
        }
    }

    /// Show a notification balloon from the tray icon.
    ///
    /// The tray thread renders the text; only fixed daemon literals are ever
    /// passed here — never user text, tokens, or paths. Truncates to the
    /// Win32 balloon limits (63 title / 255 body chars). Never steals focus.
    pub fn show_balloon(&self, title: &str, body: &str) {
        *BALLOON
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((truncate_balloon(title, 63), truncate_balloon(body, 255)));
        // SAFETY: same contract as `refresh` above.
        unsafe {
            let _ = PostMessageW(Some(self.window), WM_TRAY_BALLOON, WPARAM(0), LPARAM(0));
        }
    }

    /// Remove the icon and stop the tray thread.
    pub fn remove(&mut self) {
        if let Some(thread) = self.thread.take() {
            // SAFETY: the window is still owned by the live tray thread.
            unsafe {
                let _ = PostMessageW(Some(self.window), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            let _ = thread.join();
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        self.remove();
    }
}

fn run_tray_thread(event_tx: SyncSender<TrayEvent>, ready_tx: Sender<Result<isize, TrayError>>) {
    // SAFETY: GetModuleHandleW with a null module name returns the current
    // process image handle and has no pointer preconditions.
    let module = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => module,
        Err(error) => {
            let _ = ready_tx.send(Err(last_error("GetModuleHandleW", error.code().0)));
            return;
        }
    };
    let instance = HINSTANCE(module.0);
    let class = WNDCLASSW {
        lpfnWndProc: Some(tray_window_proc),
        hInstance: instance,
        lpszClassName: TRAY_WINDOW_CLASS,
        ..WNDCLASSW::default()
    };
    // SAFETY: the class structure is fully initialized and points to a valid
    // window procedure that remains in this module for the process lifetime.
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        let error = io::Error::last_os_error();
        let code = error.raw_os_error().unwrap_or(0);
        if code != ERROR_CLASS_ALREADY_EXISTS.0 as i32 {
            let _ = ready_tx.send(Err(TrayError::Win32 {
                operation: "RegisterClassW",
                code,
            }));
            return;
        }
    }

    // SAFETY: the class is registered above, the parent is HWND_MESSAGE, and
    // all optional handles and parameters are null. WS_POPUP keeps the window
    // out of the taskbar; it is never shown.
    let window = match unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            TRAY_WINDOW_CLASS,
            w!("Okkhor"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            None,
        )
    } {
        Ok(window) => window,
        Err(error) => {
            // SAFETY: the class was registered above and the window was not
            // created, so unregistering it is valid.
            unsafe {
                let _ = UnregisterClassW(TRAY_WINDOW_CLASS, Some(instance));
            }
            let _ = ready_tx.send(Err(last_error("CreateWindowExW", error.code().0)));
            return;
        }
    };

    {
        let mut slot = lock_events();
        if slot.is_some() {
            drop(slot);
            // SAFETY: window is live and owned by this thread.
            unsafe {
                let _ = DestroyWindow(window);
            }
            // SAFETY: the class was registered above.
            unsafe {
                let _ = UnregisterClassW(TRAY_WINDOW_CLASS, Some(instance));
            }
            let _ = ready_tx.send(Err(TrayError::AlreadyRunning));
            return;
        }
        *slot = Some(event_tx);
    }

    if let Err(error) = add_tray_icon(window) {
        clear_event_sender();
        // SAFETY: window is live and owned by this thread; WM_DESTROY removes
        // the icon even if the add failed.
        unsafe {
            let _ = DestroyWindow(window);
        }
        // SAFETY: the class was registered above.
        unsafe {
            let _ = UnregisterClassW(TRAY_WINDOW_CLASS, Some(instance));
        }
        let _ = ready_tx.send(Err(error));
        return;
    }

    if ready_tx.send(Ok(window.0 as isize)).is_err() {
        clear_event_sender();
        // SAFETY: window is live and owned by this thread; WM_DESTROY removes
        // the icon.
        unsafe {
            let _ = DestroyWindow(window);
            let _ = UnregisterClassW(TRAY_WINDOW_CLASS, Some(instance));
        }
        return;
    }

    let mut message = MSG::default();
    loop {
        // SAFETY: message is a valid local MSG and no window filter is used.
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if result.0 <= 0 {
            break;
        }
        // SAFETY: message was populated by GetMessageW.
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    clear_event_sender();
    // SAFETY: the window has been destroyed and the class belongs to this
    // module.
    unsafe {
        let _ = UnregisterClassW(TRAY_WINDOW_CLASS, Some(instance));
    }
}

fn add_tray_icon(window: HWND) -> Result<(), TrayError> {
    // SAFETY: IDI_APPLICATION is a system icon resource; a null instance asks
    // the system for the shared icon.
    let icon = unsafe { LoadIconW(None, IDI_APPLICATION) }
        .map_err(|error| last_error("LoadIconW", error.code().0))?;

    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: TRAY_ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY_CALLBACK,
        hIcon: icon,
        ..NOTIFYICONDATAW::default()
    };
    write_tooltip(
        &mut data.szTip,
        tooltip_text(
            PAUSED.load(Ordering::Acquire),
            MODE_ENGLISH.load(Ordering::Acquire),
        ),
    );

    // SAFETY: data is a fully initialized NOTIFYICONDATAW whose cbSize matches
    // this ABI version, and the window is live on this thread.
    if unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
        Ok(())
    } else {
        Err(last_error("Shell_NotifyIconW", 0))
    }
}

fn update_tray_icon(window: HWND) {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: TRAY_ICON_ID,
        uFlags: NIF_TIP,
        ..NOTIFYICONDATAW::default()
    };
    write_tooltip(
        &mut data.szTip,
        tooltip_text(
            PAUSED.load(Ordering::Acquire),
            MODE_ENGLISH.load(Ordering::Acquire),
        ),
    );

    // SAFETY: data is fully initialized and only the tooltip is modified.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

fn remove_tray_icon(window: HWND) {
    let data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: TRAY_ICON_ID,
        ..NOTIFYICONDATAW::default()
    };
    // SAFETY: data is fully initialized; deleting an unknown icon is a no-op.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

fn tooltip_text(paused: bool, english: bool) -> &'static str {
    match (english, paused) {
        (false, false) => "Okkhor - Bangla (active)",
        (true, false) => "Okkhor - English (active)",
        (false, true) => "Okkhor - Bangla (paused)",
        (true, true) => "Okkhor - English (paused)",
    }
}

fn write_tooltip(buffer: &mut [u16; 128], text: &str) {
    buffer.fill(0);
    let limit = buffer.len().saturating_sub(1);
    for (slot, unit) in buffer.iter_mut().zip(text.encode_utf16().take(limit)) {
        *slot = unit;
    }
}

/// Truncate balloon text to the Win32 limit, keeping char boundaries.
fn truncate_balloon(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    text.chars().take(max_chars).collect()
}

fn write_balloon_text<const N: usize>(buffer: &mut [u16; N], text: &str) {
    buffer.fill(0);
    let limit = buffer.len().saturating_sub(1);
    for (slot, unit) in buffer.iter_mut().zip(text.encode_utf16().take(limit)) {
        *slot = unit;
    }
}

/// Render a pending balloon (if any) on the tray thread.
fn show_pending_balloon(window: HWND) {
    let pending = BALLOON
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    let Some((title, body)) = pending else {
        return;
    };
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: TRAY_ICON_ID,
        uFlags: NIF_INFO,
        dwInfoFlags: NIIF_INFO,
        ..NOTIFYICONDATAW::default()
    };
    write_balloon_text(&mut data.szInfoTitle, &title);
    write_balloon_text(&mut data.szInfo, &body);
    // SAFETY: data is fully initialized, sizes match this ABI version, and the
    // window is live on this thread.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

unsafe extern "system" fn tray_window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_TRAY_CALLBACK => {
            let notification = (lparam.0 as u32) & 0xFFFF;
            if is_menu_notification(notification) {
                show_menu(window);
            } else if let Some(event) = event_for_notification(notification) {
                send_event(event);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = wparam.0 & 0xFFFF;
            if let Some(event) = event_for_command(id) {
                send_event(event);
            }
            LRESULT(0)
        }
        WM_TRAY_REFRESH => {
            update_tray_icon(window);
            LRESULT(0)
        }
        WM_TRAY_BALLOON => {
            show_pending_balloon(window);
            LRESULT(0)
        }
        WM_CLOSE => {
            // SAFETY: window is live.
            unsafe {
                let _ = DestroyWindow(window);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            remove_tray_icon(window);
            // SAFETY: this is the thread that owns the window message queue.
            unsafe {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        // SAFETY: all other messages follow the default window procedure.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

fn show_menu(window: HWND) {
    // SAFETY: CreatePopupMenu returns a menu owned by this thread.
    let menu = match unsafe { CreatePopupMenu() } {
        Ok(menu) => menu,
        Err(_) => return,
    };

    let mode_label: PCWSTR = if MODE_ENGLISH.load(Ordering::Acquire) {
        w!("Switch to Bangla")
    } else {
        w!("Switch to English")
    };
    let pause_label = if PAUSED.load(Ordering::Acquire) {
        w!("Resume")
    } else {
        w!("Pause")
    };
    // SAFETY: menu is live, the labels are static null-terminated strings, and
    // the command ids are small unique integers.
    unsafe {
        let _ = AppendMenuW(menu, MF_STRING, MENU_MODE, mode_label);
        let _ = AppendMenuW(menu, MF_STRING, MENU_PAUSE, pause_label);
        let _ = AppendMenuW(menu, MF_STRING, MENU_SETTINGS, w!("Settings"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, w!(""));
        let _ = AppendMenuW(menu, MF_STRING, MENU_EXIT, w!("Exit"));
    }

    let mut point = POINT::default();
    // SAFETY: point is valid local storage.
    let _ = unsafe { GetCursorPos(&mut point) };
    // SAFETY: window is live; foreground is required so the menu dismisses
    // correctly after a tray click. The menu itself never takes focus from the
    // target app beyond the standard foreground handoff.
    unsafe {
        let _ = SetForegroundWindow(window);
        let _ = TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            point.x,
            point.y,
            Some(0),
            window,
            None,
        );
        let _ = DestroyMenu(menu);
        // Required by the notification-area documentation so the menu closes
        // when the user clicks elsewhere.
        let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
    }
}

/// Right-click (and its keyboard equivalent) opens the menu; clicks are not
/// delivered as events. Pure for unit tests.
fn is_menu_notification(notification: u32) -> bool {
    notification == WM_RBUTTONUP || notification == WM_CONTEXTMENU
}

/// Click-to-event mapping for the tray callback (pure, no Win32).
/// Single left-click toggles Bangla/English. A double-click arrives as two
/// left-clicks, i.e. two mode toggles (net no change), and never pauses:
/// pause stays available via the tray menu and the pause hotkey, so a
/// double-click can never silently flip the language.
fn event_for_notification(notification: u32) -> Option<TrayEvent> {
    if notification == WM_LBUTTONUP {
        Some(TrayEvent::ToggleMode)
    } else {
        None
    }
}

fn event_for_command(id: usize) -> Option<TrayEvent> {
    match id {
        MENU_MODE => Some(TrayEvent::ToggleMode),
        MENU_PAUSE => Some(TrayEvent::TogglePause),
        MENU_SETTINGS => Some(TrayEvent::Settings),
        MENU_EXIT => Some(TrayEvent::Exit),
        _ => None,
    }
}

fn send_event(event: TrayEvent) {
    if let Some(sender) = event_sender() {
        let _ = sender.try_send(event);
    }
}

fn event_sender() -> Option<SyncSender<TrayEvent>> {
    lock_events().clone()
}

fn lock_events() -> MutexGuard<'static, Option<SyncSender<TrayEvent>>> {
    EVENT_TX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn clear_event_sender() {
    *lock_events() = None;
}

fn last_error(operation: &'static str, code: i32) -> TrayError {
    TrayError::Win32 {
        operation,
        code: if code == 0 {
            io::Error::last_os_error().raw_os_error().unwrap_or(0)
        } else {
            code
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MENU_EXIT, MENU_MODE, MENU_PAUSE, MENU_SETTINGS, TrayEvent, TrayMode, event_for_command,
        event_for_notification, is_menu_notification, tooltip_text, truncate_balloon,
        write_tooltip,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        WM_CONTEXTMENU, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_RBUTTONUP,
    };
    #[test]
    fn pause_menu_toggles_and_exit_exits() {
        assert_eq!(event_for_command(MENU_PAUSE), Some(TrayEvent::TogglePause));
        assert_eq!(event_for_command(MENU_EXIT), Some(TrayEvent::Exit));
    }

    #[test]
    fn mode_menu_item_toggles_mode() {
        assert_eq!(event_for_command(MENU_MODE), Some(TrayEvent::ToggleMode));
    }

    #[test]
    fn settings_opens_the_ui() {
        assert_eq!(event_for_command(MENU_SETTINGS), Some(TrayEvent::Settings));
        assert_eq!(event_for_command(99), None);
    }

    #[test]
    fn left_click_toggles_mode_right_click_opens_menu() {
        assert_eq!(
            event_for_notification(WM_LBUTTONUP),
            Some(TrayEvent::ToggleMode)
        );
        // Right-click never becomes an event: the menu opens instead.
        assert_eq!(event_for_notification(WM_RBUTTONUP), None);
        assert_eq!(event_for_notification(WM_CONTEXTMENU), None);
        assert!(is_menu_notification(WM_RBUTTONUP));
        assert!(is_menu_notification(WM_CONTEXTMENU));
        assert!(!is_menu_notification(WM_LBUTTONUP));
    }

    #[test]
    fn double_click_never_pauses() {
        // Double-click is not mapped: the shell delivers it as two
        // left-clicks, i.e. two mode toggles (net no change). Pause stays
        // available via the tray menu and the pause hotkey only.
        assert_eq!(event_for_notification(WM_LBUTTONDBLCLK), None);
        assert!(!is_menu_notification(WM_LBUTTONDBLCLK));
    }

    #[test]
    fn tray_mode_labels_and_toggle() {
        assert_eq!(TrayMode::Bangla.label(), "Bangla");
        assert_eq!(TrayMode::English.label(), "English");
        assert_eq!(TrayMode::Bangla.toggle(), TrayMode::English);
        assert_eq!(TrayMode::English.toggle(), TrayMode::Bangla);
        assert!(!TrayMode::Bangla.is_english());
        assert!(TrayMode::English.is_english());
        assert_eq!(TrayMode::default(), TrayMode::Bangla);
    }

    #[test]
    fn tooltip_state_is_fixed_text() {
        assert_eq!(tooltip_text(false, false), "Okkhor - Bangla (active)");
        assert_eq!(tooltip_text(false, true), "Okkhor - English (active)");
        assert_eq!(tooltip_text(true, false), "Okkhor - Bangla (paused)");
        assert_eq!(tooltip_text(true, true), "Okkhor - English (paused)");
    }

    #[test]
    fn tooltip_is_truncated_and_nul_terminated() {
        let mut buffer = [0xFFFF_u16; 128];
        let long = "a".repeat(300);
        write_tooltip(&mut buffer, &long);
        assert_eq!(buffer[127], 0);
        assert!(buffer[..127].iter().all(|unit| *unit == u16::from(b'a')));
    }

    #[test]
    fn short_tooltip_clears_the_rest_of_the_buffer() {
        let mut buffer = [0xFFFF_u16; 128];
        write_tooltip(&mut buffer, "Okkhor");
        assert_eq!(&buffer[..6], &[79, 107, 107, 104, 111, 114]);
        assert!(buffer[6..].iter().all(|unit| *unit == 0));
    }

    #[test]
    fn balloon_text_truncates_on_char_boundaries() {
        assert_eq!(truncate_balloon("Okkhor", 63), "Okkhor");
        let long = "অ".repeat(300);
        let truncated = truncate_balloon(&long, 255);
        assert_eq!(truncated.chars().count(), 255);
        let ascii = "a".repeat(100);
        assert_eq!(truncate_balloon(&ascii, 63).len(), 63);
    }
}
