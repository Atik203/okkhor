//! Win32 implementation of the input boundary.
//!
//! This is the only module that calls Windows APIs directly. Keep the unsafe
//! surface small and document every block.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, MSG,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

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

    // SAFETY: keyboard_proc has the required HOOKPROC signature and remains
    // valid for the lifetime of this process. A global WH_KEYBOARD_LL hook uses
    // a null module handle and thread id 0.
    let hook = match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0) } {
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

/// Unicode text injector based on `SendInput`.
///
/// The clipboard fallback is intentionally left for the follow-up slice; this
/// implementation returns an explicit Win32 error rather than silently
/// switching methods.
#[derive(Default)]
pub struct WindowsInjector;

impl TextInjector for WindowsInjector {
    fn inject(&self, text: &str) -> Result<InjectionMethod, InjectError> {
        if text.is_empty() {
            return Ok(InjectionMethod::UnicodeSendInput);
        }

        for chunk in utf16_units(text).chunks(256) {
            let inputs = unicode_inputs(chunk);
            // SAFETY: inputs is a valid, fully initialized INPUT slice and the
            // structure size is provided to SendInput.
            let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
            if sent as usize != inputs.len() {
                let code = std::io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(0)
                    .max(0) as u32;
                return Err(InjectError::Win32 {
                    operation: "SendInput",
                    code,
                });
            }
        }

        Ok(InjectionMethod::UnicodeSendInput)
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
}
