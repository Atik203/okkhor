//! Win32 implementation of the input boundary.
//!
//! This is the only module that calls Windows APIs directly. Keep the unsafe
//! surface small and document every block.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, SendInput, VIRTUAL_KEY, VK_CONTROL,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GUITHREADINFO, GetClassNameW, GetForegroundWindow,
    GetGUIThreadInfo, GetMessageW, GetWindowThreadProcessId, HC_ACTION, KBDLLHOOKSTRUCT, MSG,
    PostMessageW, PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WM_CHAR, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::clipboard::ClipboardGuard;
use crate::inject::{InjectError, InjectionMethod, TextInjector};
use crate::input::{KeyEvent, KeyPhase, KeyboardHook};

/// One process-wide hook queue. The daemon starts the hook once for its
/// lifetime; restart requires a new process.
static HOOK_TX: OnceLock<SyncSender<KeyEvent>> = OnceLock::new();

/// Whether the hook forwards events to the consumer. Pass-through is always
/// preserved; disabling capture only stops enqueueing.
static CAPTURE_ENABLED: AtomicBool = AtomicBool::new(true);

/// Enable or disable event capture without uninstalling the hook.
pub fn set_capture_enabled(enabled: bool) {
    CAPTURE_ENABLED.store(enabled, Ordering::Release);
}

/// Whether the hook currently forwards events.
pub fn capture_enabled() -> bool {
    CAPTURE_ENABLED.load(Ordering::Acquire)
}

/// Toggle capture and return the new state.
pub fn toggle_capture() -> bool {
    let previous = CAPTURE_ENABLED.fetch_xor(true, Ordering::AcqRel);
    !previous
}

#[derive(Debug, thiserror::Error)]
pub enum HookError {
    #[error("a keyboard hook is already installed in this process")]
    AlreadyStarted,
    #[error("could not start the hook thread: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("the hook thread exited before it was ready")]
    ThreadGone,
    #[error("Win32 call {operation} failed with code {code}")]
    Win32 { operation: &'static str, code: i32 },
}

/// A global low-level keyboard hook.
///
/// The hook runs on its own thread with a message loop. Events are pushed to a
/// bounded queue with `try_send`; the callback never blocks on a consumer.
///
/// The hook is a one-shot resource per process: `start` can only succeed once,
/// because the callback has no per-instance user data. Create it at daemon
/// startup and keep it for the process lifetime.
#[derive(Default)]
pub struct WindowsKeyboardHook {
    thread_id: Option<u32>,
    thread: Option<JoinHandle<()>>,
    events: Option<Receiver<KeyEvent>>,
}

impl WindowsKeyboardHook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Receive one pending key event without blocking.
    pub fn try_next(&self) -> Option<KeyEvent> {
        self.events.as_ref()?.try_recv().ok()
    }

    pub fn is_running(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }

    /// Stop forwarding events while keeping the hook installed and pass-through.
    pub fn set_capture_enabled(&self, enabled: bool) {
        set_capture_enabled(enabled);
    }

    /// Whether capture is currently enabled.
    pub fn capture_enabled(&self) -> bool {
        capture_enabled()
    }

    /// Toggle capture and return the new state.
    pub fn toggle_capture(&self) -> bool {
        toggle_capture()
    }
}

impl KeyboardHook for WindowsKeyboardHook {
    type Error = HookError;

    fn start(&mut self) -> Result<(), Self::Error> {
        if self.thread.is_some() {
            return Err(HookError::AlreadyStarted);
        }

        set_capture_enabled(true);
        let (event_tx, event_rx) = mpsc::sync_channel(4096);
        HOOK_TX
            .set(event_tx)
            .map_err(|_| HookError::AlreadyStarted)?;

        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("akkhor-keyboard-hook".to_owned())
            .spawn(move || run_hook_thread(ready_tx))
            .map_err(HookError::Spawn)?;

        match ready_rx.recv() {
            Ok(Ok(thread_id)) => {
                self.thread_id = Some(thread_id);
                self.thread = Some(thread);
                self.events = Some(event_rx);
                Ok(())
            }
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(HookError::ThreadGone)
            }
        }
    }

    fn stop(&mut self) {
        if let Some(thread_id) = self.thread_id.take() {
            // SAFETY: the thread id came from GetCurrentThreadId on the live
            // hook thread, and WM_QUIT is safe to post to that message loop.
            let _ = unsafe { PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.events = None;
    }
}

impl Drop for WindowsKeyboardHook {
    fn drop(&mut self) {
        // Ensure the hook thread is stopped and unhooked even on early exit.
        <Self as KeyboardHook>::stop(self);
    }
}

fn run_hook_thread(ready_tx: mpsc::Sender<Result<u32, HookError>>) {
    // SAFETY: GetCurrentThreadId has no preconditions.
    let thread_id = unsafe { GetCurrentThreadId() };
    if thread_id == 0 {
        let _ = ready_tx.send(Err(HookError::ThreadGone));
        return;
    }

    // SAFETY: GetModuleHandleW has no preconditions. A global low-level hook
    // must be associated with the process module that contains the callback.
    let module = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => module,
        Err(error) => {
            let _ = ready_tx.send(Err(HookError::Win32 {
                operation: "GetModuleHandleW",
                code: error.code().0,
            }));
            return;
        }
    };

    // SAFETY: keyboard_proc has the required HOOKPROC signature, module is the
    // current process module, and thread id 0 installs a global hook.
    let hook = match unsafe {
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(module.into()), 0)
    } {
        Ok(hook) => hook,
        Err(error) => {
            let _ = ready_tx.send(Err(HookError::Win32 {
                operation: "SetWindowsHookExW",
                code: error.code().0,
            }));
            return;
        }
    };

    if ready_tx.send(Ok(thread_id)).is_err() {
        // SAFETY: hook was returned by SetWindowsHookExW and is still installed.
        let _ = unsafe { UnhookWindowsHookEx(hook) };
        return;
    }

    let mut message = MSG::default();
    loop {
        // SAFETY: message points to a valid local MSG; no window filter is used.
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

    // SAFETY: hook was installed above and has not been released.
    let _ = unsafe { UnhookWindowsHookEx(hook) };
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, Windows passes a valid KBDLLHOOKSTRUCT pointer
        // in lparam for a WH_KEYBOARD_LL callback. Check for null anyway so a
        // malformed callback cannot dereference it.
        if lparam.0 == 0 {
            // SAFETY: forwarding the event is required for pass-through.
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }
        let data = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let phase = match wparam.0 as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => Some(KeyPhase::Down),
            WM_KEYUP | WM_SYSKEYUP => Some(KeyPhase::Up),
            _ => None,
        };

        if let (Some(queue), Some(phase)) = (HOOK_TX.get(), phase) {
            if CAPTURE_ENABLED.load(Ordering::Relaxed) {
                let _ = queue.try_send(KeyEvent {
                    virtual_key: data.vkCode as u16,
                    scan_code: data.scanCode as u16,
                    phase,
                    timestamp_ms: u64::from(data.time),
                });
            }
        }
    }

    // SAFETY: forwarding the event is required for pass-through behavior.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Text injector based on `SendInput`, with a guarded clipboard fallback.
///
/// Unicode `SendInput` is tried first. If the OS rejects it (for example UIPI),
/// the fallback writes text to the clipboard, sends Ctrl+V, and restores the
/// previous text. The fallback refuses to overwrite non-text clipboard data.
#[derive(Default)]
pub struct WindowsInjector;

impl WindowsInjector {
    /// Inject with Unicode `SendInput` only, never using the clipboard fallback.
    ///
    /// This is for diagnostics and benchmark harnesses that must not touch the
    /// user clipboard. Production injection should use [`TextInjector::inject`],
    /// which falls back to clipboard paste when UIPI blocks `SendInput`.
    pub fn inject_unicode_only(&self, text: &str) -> Result<InjectionMethod, InjectError> {
        if text.is_empty() {
            return Ok(InjectionMethod::UnicodeSendInput);
        }

        let target = foreground_window()?;
        inject_unicode(text)?;
        ensure_focus(target)?;
        Ok(InjectionMethod::UnicodeSendInput)
    }
}

impl TextInjector for WindowsInjector {
    fn inject(&self, text: &str) -> Result<InjectionMethod, InjectError> {
        if text.is_empty() {
            return Ok(InjectionMethod::UnicodeSendInput);
        }

        let target = foreground_window()?;

        // Standard Win32 text controls accept WM_CHAR directly. This avoids the
        // per-event SendInput cost for the most common native targets.
        match inject_wm_char(text, target) {
            Some(Ok(())) => {
                ensure_focus(target)?;
                return Ok(InjectionMethod::WindowMessage);
            }
            Some(Err(error)) => return Err(error),
            None => {}
        }

        match inject_unicode(text) {
            Ok(()) => {
                ensure_focus(target)?;
                Ok(InjectionMethod::UnicodeSendInput)
            }
            Err(InjectError::Blocked) => inject_via_clipboard(text, target),
            Err(error) => Err(error),
        }
    }
}

fn inject_unicode(text: &str) -> Result<(), InjectError> {
    for chunk in utf16_units(text).chunks(256) {
        let inputs = unicode_inputs(chunk);
        // SAFETY: inputs is a valid, fully initialized INPUT slice and the
        // structure size is provided to SendInput.
        let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent as usize != inputs.len() {
            let code = last_error_code();
            // A zero-event failure with ERROR_ACCESS_DENIED is the UIPI case
            // where the clipboard fallback may still work in a normal target.
            if sent == 0 && code == 5 {
                return Err(InjectError::Blocked);
            }
            return Err(InjectError::Win32 {
                operation: "SendInput",
                code,
            });
        }
    }
    Ok(())
}

/// Try the asynchronous `WM_CHAR` fast path for a known Win32 text control.
///
/// Returns `None` when the focused control is not a known text class, or when
/// the very first character could not be queued (so the caller can safely fall
/// back to `SendInput`). A failure after one or more characters returns an
/// error, because retrying would duplicate text.
fn inject_wm_char(text: &str, target: HWND) -> Option<Result<(), InjectError>> {
    let focus = focused_window(target)?;
    let class = class_name(focus)?;
    if !supports_wm_char(&class) {
        return None;
    }

    for (delivered, unit) in text.encode_utf16().enumerate() {
        // SAFETY: focus is a live window from the foreground thread. Posting
        // WM_CHAR keeps the injector call asynchronous for native controls; the
        // caller verifies the readback and other targets use SendInput.
        if let Err(error) =
            unsafe { PostMessageW(Some(focus), WM_CHAR, WPARAM(unit as usize), LPARAM(1)) }
        {
            if delivered == 0 {
                return None;
            }
            return Some(Err(InjectError::Win32 {
                operation: "PostMessageW",
                code: error.code().0 as u32,
            }));
        }
    }

    Some(Ok(()))
}

fn focused_window(target: HWND) -> Option<HWND> {
    // SAFETY: GetWindowThreadProcessId only reads the window's thread id.
    let thread_id = unsafe { GetWindowThreadProcessId(target, None) };
    if thread_id == 0 {
        return None;
    }

    let mut info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..GUITHREADINFO::default()
    };
    // SAFETY: info is a valid GUITHREADINFO with cbSize initialized.
    if unsafe { GetGUIThreadInfo(thread_id, &mut info) }.is_err() {
        return None;
    }
    if info.hwndFocus.0.is_null() {
        None
    } else {
        Some(info.hwndFocus)
    }
}

fn class_name(window: HWND) -> Option<String> {
    let mut buffer = [0u16; 256];
    // SAFETY: buffer is a valid mutable slice and the window belongs to the
    // foreground thread observed above.
    let length = unsafe { GetClassNameW(window, &mut buffer) };
    if length <= 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..length as usize]))
}

fn supports_wm_char(class: &str) -> bool {
    matches!(
        class.to_ascii_lowercase().as_str(),
        "edit" | "richedit20a" | "richedit20w" | "richedit50w" | "richedit60w" | "scintilla"
    )
}

fn inject_via_clipboard(text: &str, target: HWND) -> Result<InjectionMethod, InjectError> {
    ensure_focus(target)?;

    let clipboard = ClipboardGuard::open()?;
    if clipboard.has_non_text_formats() {
        return Err(InjectError::ClipboardNotEmpty);
    }
    let previous = clipboard.read_text()?;
    if let Err(error) = clipboard.write_text(text) {
        drop(clipboard);
        restore_clipboard(previous.as_deref());
        return Err(error);
    }
    drop(clipboard);

    if let Err(error) = ensure_focus(target) {
        restore_clipboard(previous.as_deref());
        return Err(error);
    }

    if let Err(error) = send_ctrl_v() {
        restore_clipboard(previous.as_deref());
        return Err(error);
    }

    // Give the target a chance to consume the paste before restoring the
    // previous clipboard. This path is only used after a failed SendInput.
    thread::sleep(Duration::from_millis(80));
    restore_clipboard(previous.as_deref());
    Ok(InjectionMethod::ClipboardPaste)
}

fn send_ctrl_v() -> Result<(), InjectError> {
    let inputs = [
        virtual_key_input(VK_CONTROL, KEYBD_EVENT_FLAGS(0)),
        virtual_key_input(VIRTUAL_KEY(0x56), KEYBD_EVENT_FLAGS(0)),
        virtual_key_input(VIRTUAL_KEY(0x56), KEYEVENTF_KEYUP),
        virtual_key_input(VK_CONTROL, KEYEVENTF_KEYUP),
    ];
    // SAFETY: inputs is a valid, fully initialized INPUT array.
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        let code = last_error_code();
        if code == 5 {
            Err(InjectError::Blocked)
        } else {
            Err(InjectError::Win32 {
                operation: "SendInput",
                code,
            })
        }
    }
}

fn restore_clipboard(previous: Option<&str>) {
    if let Ok(clipboard) = ClipboardGuard::open() {
        match previous {
            Some(text) => {
                let _ = clipboard.write_text(text);
            }
            None => {
                let _ = clipboard.clear();
            }
        }
    }
}

fn foreground_window() -> Result<HWND, InjectError> {
    // SAFETY: GetForegroundWindow has no pointer preconditions.
    let window = unsafe { GetForegroundWindow() };
    if window.0.is_null() {
        Err(InjectError::FocusChanged)
    } else {
        Ok(window)
    }
}

fn ensure_focus(target: HWND) -> Result<(), InjectError> {
    // SAFETY: GetForegroundWindow has no pointer preconditions.
    let current = unsafe { GetForegroundWindow() };
    if current.0 == target.0 {
        Ok(())
    } else {
        Err(InjectError::FocusChanged)
    }
}

fn last_error_code() -> u32 {
    std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(0)
        .max(0) as u32
}

fn virtual_key_input(vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Encode text as UTF-16 code units, including surrogate pairs.
pub fn utf16_units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn unicode_inputs(units: &[u16]) -> Vec<INPUT> {
    let mut inputs = Vec::with_capacity(units.len().saturating_mul(2));
    for unit in units {
        inputs.push(key_input(*unit, KEYEVENTF_UNICODE));
        inputs.push(key_input(*unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
    }
    inputs
}

fn key_input(scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{WindowsInjector, unicode_inputs, utf16_units};
    use crate::inject::{InjectionMethod, TextInjector};

    #[test]
    fn bengali_is_bmp_encoded() {
        assert_eq!(utf16_units("ভালো").len(), 4);
    }

    #[test]
    fn emoji_uses_a_surrogate_pair() {
        assert_eq!(utf16_units("😀").len(), 2);
    }

    #[test]
    fn empty_input_is_a_noop() {
        let result = WindowsInjector.inject("");
        assert_eq!(result, Ok(InjectionMethod::UnicodeSendInput));
    }

    #[test]
    fn unicode_only_empty_input_is_a_noop() {
        let result = WindowsInjector.inject_unicode_only("");
        assert_eq!(result, Ok(InjectionMethod::UnicodeSendInput));
    }

    #[test]
    fn each_code_unit_has_down_and_up_events() {
        let inputs = unicode_inputs(&[0x09, 0x3F]);
        assert_eq!(inputs.len(), 4);
    }

    #[test]
    fn capture_toggle_round_trips() {
        super::set_capture_enabled(true);
        assert!(super::capture_enabled());
        assert!(!super::toggle_capture());
        assert!(!super::capture_enabled());
        assert!(super::toggle_capture());
        assert!(super::capture_enabled());
    }

    #[test]
    fn wm_char_classes_are_limited_to_known_text_controls() {
        assert!(super::supports_wm_char("Edit"));
        assert!(super::supports_wm_char("RichEdit20W"));
        assert!(super::supports_wm_char("RichEdit50W"));
        assert!(super::supports_wm_char("Scintilla"));
        assert!(!super::supports_wm_char("Chrome_WidgetWin_1"));
        assert!(!super::supports_wm_char("Static"));
    }
}
