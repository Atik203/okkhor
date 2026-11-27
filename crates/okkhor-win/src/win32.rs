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
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, SendInput, VIRTUAL_KEY, VK_BACK,
    VK_CAPITAL, VK_CONTROL, VK_LEFT, VK_LWIN, VK_MENU, VK_RETURN, VK_RIGHT, VK_RMENU, VK_RWIN,
    VK_SHIFT, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CallNextHookEx, DispatchMessageW, GUITHREADINFO, GetClassNameW,
    GetForegroundWindow, GetGUIThreadInfo, GetMessageW, GetWindowThreadProcessId, HC_ACTION,
    KBDLLHOOKSTRUCT, MSG, PostMessageW, PostThreadMessageW, SW_SHOW, SetForegroundWindow,
    SetWindowsHookExW, ShowWindow, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_CHAR,
    WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::clipboard::ClipboardGuard;
use crate::inject::{InjectError, InjectionMethod, TextInjector};
use crate::input::{KeyEvent, KeyPhase, KeyboardHook};

/// Whether the inline preedit currently has rendered Bangla in the document.
///
/// Set by the daemon after every render. Backspace is swallowed only when this
/// is true, because then the composition owns the on-screen word.
static INLINE_RENDERED: AtomicBool = AtomicBool::new(false);

/// Update the inline-rendered flag used by the Backspace swallow decision.
pub fn set_inline_rendered(rendered: bool) {
    INLINE_RENDERED.store(rendered, Ordering::Release);
}

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

/// Whether the active input mode composes Bangla (published by the daemon).
static BANGLA_MODE: AtomicBool = AtomicBool::new(true);

/// Whether a composition buffer is non-empty (published by the daemon).
static COMPOSING: AtomicBool = AtomicBool::new(false);

/// Whether the English completion popup is visible (published by the daemon).
static ENGLISH_POPUP: AtomicBool = AtomicBool::new(false);

/// Virtual key of the last key-down the hook swallowed, so its key-up is
/// swallowed too. Zero means none pending.
/// One flag per virtual key whose key-down this hook swallowed. A single
/// "last VK" slot lost track when two keys (for example Down then Tab) were
/// swallowed before either release, which could leave a key looking stuck.
static SWALLOWED_DOWN: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];

/// Inline Bangla preedit swallows letters so the roman key never reaches the
/// document; the daemon renders the Bangla form itself.
static INLINE_PREEDIT: AtomicBool = AtomicBool::new(false);

/// A ghost next-word hint is visible and owns Tab/Up/Down.
static GHOST_POPUP: AtomicBool = AtomicBool::new(false);

/// Whether fixed-layout mode is active. The hook drops ASCII letter keydowns
/// (and their matching keyups via `LAST_SWALLOWED_VK`) so the daemon can inject
/// the mapped Bangla character instead of letting an IME compose the raw ASCII.
static SWALLOW_LETTERS: AtomicBool = AtomicBool::new(false);

/// Publish fixed-layout mode for the hook. Strictly scoped: callers must clear
/// it on a mode or layout switch so the Avro phonetic path keeps its letters.
pub fn set_swallow_letters(enabled: bool) {
    SWALLOW_LETTERS.store(enabled, Ordering::Release);
}

/// Publish inline Bangla preedit for the hook. Only letters are hidden; the
/// daemon injects the rendered Bangla form.
pub fn set_swallow_inline(enabled: bool) {
    INLINE_PREEDIT.store(enabled, Ordering::Release);
}

/// Publish ghost next-word visibility for the hook.
pub fn set_ghost_popup(visible: bool) {
    GHOST_POPUP.store(visible, Ordering::Release);
}

/// Publish the composition state the hook needs to decide swallowing.
///
/// Called by the daemon after every key batch (and on mode/encoding changes).
/// Relaxed stores: a one-poll stale value can only make a key pass through; it
/// is corrected by the next publish.
pub fn set_composition_state(bangla_mode: bool, composing: bool, bangla_unicode: bool) {
    BANGLA_MODE.store(bangla_mode, Ordering::Release);
    COMPOSING.store(composing, Ordering::Release);
    BANGLA_UNICODE.store(bangla_unicode, Ordering::Release);
}

/// Publish whether the English completion popup is visible so the hook can
/// swallow Tab only while an accept is possible.
/// Whether the hook is currently swallowing Tab/arrow keys for the English
/// completion list. The daemon uses it to decide whether a swallowed key must
/// be re-emitted, avoiding a double Tab when the key had already passed through.
pub fn english_popup_visible() -> bool {
    ENGLISH_POPUP.load(Ordering::Relaxed)
}

pub fn set_english_popup(visible: bool) {
    ENGLISH_POPUP.store(visible, Ordering::Release);
}

/// Whether Bangla **Unicode** output is active (danda and Bengali numerals).
static BANGLA_UNICODE: AtomicBool = AtomicBool::new(true);

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

    /// Publish the Bangla/composing state used for key swallowing.
    pub fn set_composition_state(&self, bangla_mode: bool, composing: bool, bangla_unicode: bool) {
        set_composition_state(bangla_mode, composing, bangla_unicode);
    }

    /// Publish fixed-layout mode so the hook swallows raw letter keys.
    pub fn set_swallow_letters(&self, enabled: bool) {
        set_swallow_letters(enabled);
    }

    /// Publish the English completion visibility for Tab swallowing.
    pub fn set_english_popup(&self, visible: bool) {
        set_english_popup(visible);
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
            let capture = CAPTURE_ENABLED.load(Ordering::Relaxed);
            let virtual_key = data.vkCode as u16;
            // Anti-loop gate, decided by the pure `should_enqueue` seam
            // (matrix-tested in `input.rs`): our own SendInput erase/inject
            // carries LLKHF_INJECTED and is never re-captured.
            if crate::input::should_enqueue(capture, data.flags.0) {
                let _ = queue.try_send(KeyEvent {
                    virtual_key,
                    scan_code: data.scanCode as u16,
                    phase,
                    timestamp_ms: u64::from(data.time),
                    chord: ctrl_or_alt_pressed(),
                });
            }
            // Commit keys (Space/Enter/digits/arrows) must not reach the
            // target while composing: the app already holds the roman letters,
            // and an extra space/digit or a caret move would make the commit's
            // backspaces erase the wrong characters. Letters and Backspace
            // still pass through one-for-one with the buffer.
            let swallow = match phase {
                KeyPhase::Down => {
                    let injected = crate::input::is_injected_event(data.flags.0);
                    let chord = ctrl_or_alt_pressed();
                    let bangla = BANGLA_MODE.load(Ordering::Relaxed);
                    let composing = COMPOSING.load(Ordering::Relaxed);
                    let unicode = BANGLA_UNICODE.load(Ordering::Relaxed);
                    let english_popup = ENGLISH_POPUP.load(Ordering::Relaxed);
                    let fixed = SWALLOW_LETTERS.load(Ordering::Relaxed);
                    let inline = INLINE_PREEDIT.load(Ordering::Relaxed);
                    let inline_rendered = INLINE_RENDERED.load(Ordering::Relaxed);
                    let ghost = GHOST_POPUP.load(Ordering::Relaxed);
                    // AltGr is a layout modifier, not a shortcut chord. Only
                    // the fixed-layout letter predicate uses this relaxed test;
                    // the phonetic predicates keep the original chord rule.
                    let fixed_chord = chord && !altgr_pressed();
                    let decision = !injected
                        && (crate::input::should_swallow(
                            capture,
                            bangla,
                            composing,
                            chord,
                            false,
                            virtual_key,
                        ) || crate::input::should_swallow_letters(
                            capture,
                            bangla,
                            fixed,
                            fixed_chord,
                            false,
                            virtual_key,
                        ) || crate::input::should_swallow_backspace(
                            capture,
                            bangla,
                            inline,
                            inline_rendered,
                            composing,
                            chord,
                            virtual_key,
                        ) || crate::input::should_swallow_inline_letters(
                            capture,
                            bangla,
                            inline,
                            chord,
                            false,
                            virtual_key,
                        ) || crate::input::should_swallow_ghost(
                            capture,
                            ghost,
                            chord,
                            false,
                            virtual_key,
                        ) || crate::input::should_swallow_danda(
                            capture,
                            bangla,
                            unicode,
                            chord,
                            shift_pressed(),
                            false,
                            virtual_key,
                        ) || crate::input::should_swallow_numeral(
                            capture,
                            bangla,
                            unicode,
                            composing,
                            chord,
                            false,
                            virtual_key,
                        ) || crate::input::should_swallow_tab(
                            capture,
                            !bangla,
                            english_popup,
                            chord,
                            false,
                            virtual_key,
                        ) || crate::input::should_swallow_english_arrows(
                            capture,
                            !bangla,
                            english_popup,
                            chord,
                            false,
                            virtual_key,
                        ));
                    if decision && (virtual_key as usize) < SWALLOWED_DOWN.len() {
                        SWALLOWED_DOWN[virtual_key as usize].store(true, Ordering::Relaxed);
                    }
                    decision
                }
                // Swallow the matching release so the target never sees an
                // unmatched key-up after we hid its key-down. Injected events
                // are never swallowed (the commit's own re-emitted keys).
                KeyPhase::Up => {
                    !crate::input::is_injected_event(data.flags.0)
                        && (virtual_key as usize) < SWALLOWED_DOWN.len()
                        && SWALLOWED_DOWN[virtual_key as usize].swap(false, Ordering::Relaxed)
                }
            };
            if swallow {
                return LRESULT(1);
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

/// Upper bound for one [`send_backspaces`] call.
///
/// The composition buffer never exceeds 32 characters; the cap leaves headroom
/// while keeping the `SendInput` batch bounded.
pub const MAX_SEND_BACKSPACES: usize = 64;

/// Build the commit event batch without calling Win32, so its exact shape is
/// unit-testable. Backspaces come first as VK down/up pairs, then one
/// `KEYEVENTF_UNICODE` down/up pair per UTF-16 code unit.
fn build_commit_inputs(backspaces: usize, units: &[u16]) -> Vec<INPUT> {
    let mut inputs = Vec::with_capacity(
        backspaces
            .saturating_mul(2)
            .saturating_add(units.len().saturating_mul(2)),
    );
    for _ in 0..backspaces {
        inputs.push(virtual_key_input(VK_BACK, KEYBD_EVENT_FLAGS(0)));
        inputs.push(virtual_key_input(VK_BACK, KEYEVENTF_KEYUP));
    }
    for unit in units {
        inputs.push(key_input(*unit, KEYEVENTF_UNICODE));
        inputs.push(key_input(*unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
    }
    inputs
}

/// Upper bound for one [`send_caret_steps`] call.
///
/// A rendered word is at most a few hundred UTF-16 units; the composition
/// buffer is capped at 32 roman characters, so the projected offset stays far
/// below this. The cap keeps the batch bounded no matter what state is passed.
pub const MAX_CARET_STEPS: usize = 64;

/// Build one arrow-key batch without calling Win32, so its shape is testable.
fn build_caret_inputs(left: bool, count: usize) -> Vec<INPUT> {
    let vk = if left { VK_LEFT } else { VK_RIGHT };
    // Arrow keys are extended keys; targets that use the scan code (some games
    // and older controls) need the flag on both the press and the release.
    let flags = KEYEVENTF_EXTENDEDKEY;
    let mut inputs = Vec::with_capacity(count.saturating_mul(2));
    for _ in 0..count {
        inputs.push(virtual_key_input(vk, flags));
        inputs.push(virtual_key_input(vk, flags | KEYEVENTF_KEYUP));
    }
    inputs
}

/// Move the document caret `count` positions left or right in one batch.
///
/// Only the inline preedit caret projection uses this: it walks the caret into
/// the rendered word to show the composition point, and back to the word end
/// before the next edit. The events carry `LLKHF_INJECTED`, so the hook skips
/// them and they never re-enter the composition. A count of zero touches no
/// Win32 API; larger counts clamp to [`MAX_CARET_STEPS`]. A failed batch
/// reports the same typed error as [`inject_commit`], and the caller must then
/// treat the document caret as unknown instead of guessing.
pub fn send_caret_steps(left: bool, count: usize) -> Result<(), InjectError> {
    let count = count.min(MAX_CARET_STEPS);
    if count == 0 {
        return Ok(());
    }
    let inputs = build_caret_inputs(left, count);
    // SAFETY: inputs is a valid, fully initialized INPUT slice and the
    // structure size is provided to SendInput.
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) } as usize;
    if sent == inputs.len() {
        Ok(())
    } else {
        let code = last_error_code();
        if sent == 0 && code == 5 {
            Err(InjectError::Blocked)
        } else {
            Err(InjectError::Win32 {
                operation: "SendInput",
                code,
            })
        }
    }
}

/// Commit an erase plus replacement text in one `SendInput` batch.
///
/// Builds a single `INPUT` array — `backspaces` Backspace down/up pairs
/// followed by `text` (plus one space when `trailing_space`) as
/// `KEYEVENTF_UNICODE` down/up pairs — and delivers it in one `SendInput`
/// call, which Windows processes in order. A zero return means nothing was
/// inserted, so erase and inject fail together; a nonzero partial send still
/// delivers a prefix (inherent to Win32) and is reported with its code.
///
/// Unlike [`TextInjector::inject`] there is no `WM_CHAR` fast path and no
/// clipboard fallback: neither can share the single batch. A UIPI-blocked
/// target reports [`InjectError::Blocked`] with nothing erased and nothing
/// injected. `backspaces` clamps to [`MAX_SEND_BACKSPACES`]; the commit text
/// stays bounded by design (the daemon only passes rule outputs of at most 64
/// chars plus one space). A fully empty commit is a no-op that touches no
/// Win32 API. The resulting events carry `LLKHF_INJECTED` and are skipped by
/// the hook filter.
pub fn inject_commit(
    backspaces: usize,
    text: &str,
    trailing_space: bool,
) -> Result<(), InjectError> {
    let backspaces = backspaces.min(MAX_SEND_BACKSPACES);
    let mut units = utf16_units(text);
    if trailing_space {
        units.push(0x20);
    }
    if backspaces == 0 && units.is_empty() {
        return Ok(());
    }
    let inputs = build_commit_inputs(backspaces, &units);
    // SAFETY: inputs is a valid, fully initialized INPUT slice and the
    // structure size is provided to SendInput.
    let mut sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) } as usize;
    // A partial batch can deliver the backspaces but not the replacement,
    // which left an erased word looking like spaces. Retry only the unsent
    // tail once; event order is preserved, so this cannot duplicate a prefix.
    if sent > 0 && sent < inputs.len() {
        let rest = &inputs[sent..];
        // SAFETY: `rest` is a valid, fully initialized tail of `inputs`.
        let more = unsafe { SendInput(rest, std::mem::size_of::<INPUT>() as i32) } as usize;
        sent += more;
    }
    if sent == inputs.len() {
        Ok(())
    } else {
        let code = last_error_code();
        if sent == 0 && code == 5 {
            Err(InjectError::Blocked)
        } else {
            Err(InjectError::Win32 {
                operation: "SendInput",
                code,
            })
        }
    }
}

/// Inject one mapped fixed-layout character.
///
/// The hook swallowed the raw letter, so there is nothing to erase: zero
/// backspaces, no trailing space, one `SendInput` batch, one Unicode down/up
/// pair for a BMP character. Supplementary characters need one pair per UTF-16
/// code unit, which [`inject_commit`] handles. No `WM_CHAR` fast path and no
/// clipboard fallback: the character is delivered exactly once or not at all,
/// so the daemon can re-emit the raw ASCII letter on failure.
pub fn inject_char(ch: char) -> Result<(), InjectError> {
    inject_commit(0, &ch.to_string(), false)
}

/// Send `count` Backspace key presses via `SendInput`.
///
/// A count of zero is a no-op that touches no Win32 API; larger counts are
/// clamped to [`MAX_SEND_BACKSPACES`]. Shares the single-batch path with
/// [`inject_commit`]. Like all injection, the resulting events carry
/// `LLKHF_INJECTED` and are skipped by the hook filter.
pub fn send_backspaces(count: usize) -> Result<(), InjectError> {
    inject_commit(count, "", false)
}

/// Send one Enter press (down + up) after a commit.
///
/// Enter is a swallowed composition key: the daemon must re-emit the newline
/// the target never received. Sharing `SendInput` keeps the ordering after
/// [`inject_commit`] deterministic. Failures use the same typed mapping.
pub fn send_enter() -> Result<(), InjectError> {
    let inputs = [
        virtual_key_input(VK_RETURN, KEYBD_EVENT_FLAGS(0)),
        virtual_key_input(VK_RETURN, KEYEVENTF_KEYUP),
    ];
    // SAFETY: inputs is a valid, fully initialized INPUT slice and the
    // structure size is provided to SendInput.
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        let code = last_error_code();
        if sent == 0 && code == 5 {
            Err(InjectError::Blocked)
        } else {
            Err(InjectError::Win32 {
                operation: "SendInput",
                code,
            })
        }
    }
}

/// Re-emit a swallowed Tab (English accept failed, or the popup hid during
/// the accept). Uses the same `SendInput` path as Enter.
pub fn send_tab() -> Result<(), InjectError> {
    let inputs = [
        virtual_key_input(VK_TAB, KEYBD_EVENT_FLAGS(0)),
        virtual_key_input(VK_TAB, KEYEVENTF_KEYUP),
    ];
    // SAFETY: inputs is a valid, fully initialized INPUT slice and the
    // structure size is provided to SendInput.
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        let code = last_error_code();
        if sent == 0 && code == 5 {
            Err(InjectError::Blocked)
        } else {
            Err(InjectError::Win32 {
                operation: "SendInput",
                code,
            })
        }
    }
}

/// Identifier of the current foreground window for the composition guard.
///
/// The daemon records this when a composition starts and refuses to erase when
/// it changes before commit, so a focus switch can never delete text in the
/// wrong window. A null handle maps to `0`, which never equals a live window.
pub fn foreground_window_id() -> isize {
    // SAFETY: GetForegroundWindow has no pointer preconditions.
    unsafe { GetForegroundWindow() }.0 as isize
}

/// Process id that owns the foreground window. Browsers swap HWNDs for the
/// address bar and its autocomplete popup while a word is being typed; the
/// process stays the same, so comparing PIDs avoids resetting a composition
/// because of a child-window switch. Returns 0 when there is no foreground
/// window, which never compares equal to a live composition's PID.
pub fn foreground_window_pid() -> u32 {
    // SAFETY: GetForegroundWindow and GetWindowThreadProcessId have no pointer
    // preconditions; the out-pointer is valid for the duration of the call.
    unsafe {
        let window = GetForegroundWindow();
        if window.0.is_null() {
            return 0;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(window, Some(&mut pid));
        pid
    }
}

/// Modifier snapshot used by the fixed-layout resolver.
///
/// Both fields are plain booleans so the daemon can convert this into the
/// layout engine's own modifier set without any Win32 dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModifierFlags {
    pub shift: bool,
    pub altgr: bool,
}

/// Read Shift and AltGr for a fixed-layout key on the worker thread.
///
/// This is deliberately not called from the hook callback: the callback only
/// timestamps and enqueues. Only `VK_RMENU` is AltGr; plain left Alt remains a
/// shortcut modifier so Alt+F and similar app shortcuts still work.
pub fn modifier_state() -> ModifierFlags {
    // SAFETY: GetAsyncKeyState with a virtual-key code has no pointer
    // preconditions; a negative return means the key is currently down.
    unsafe {
        ModifierFlags {
            shift: GetAsyncKeyState(i32::from(VK_SHIFT.0)) < 0,
            altgr: altgr_pressed(),
        }
    }
}

/// Show a window and bring it to the foreground through Win32.
///
/// Tauri's `show()` can leave a window that was created hidden (settings in a
/// `--quickbar` launch) still invisible: the WebView exists and answers IPC,
/// but no `ShowWindow` ever runs. This is the fallback the settings and tray
/// paths call after the Tauri calls; failures are ignored because focus can be
/// refused by the foreground lock without meaning the window is hidden.
pub fn show_window_foreground(hwnd: HWND) {
    // SAFETY: hwnd is a live top-level window owned by this process; ShowWindow
    // only changes its visibility state.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Whether Caps Lock is toggled on.
///
/// `GetAsyncKeyState`'s low bit is the toggle state for toggle keys, which is
/// the only state readable off the window's own thread. The composition case is
/// `shift XOR caps`, matching what the target application would receive.
pub fn caps_lock_on() -> bool {
    // SAFETY: GetAsyncKeyState with a virtual-key code has no pointer
    // preconditions.
    unsafe { (GetAsyncKeyState(i32::from(VK_CAPITAL.0)) & 1) != 0 }
}

/// Whether the AltGr modifier is held. Ctrl+Alt on Windows is reported as
/// both keys down, so a general "chord" test would wrongly treat an AltGr
/// letter as a shortcut and let it reach the target unmodified.
pub fn altgr_pressed() -> bool {
    // SAFETY: GetAsyncKeyState with a virtual-key code has no pointer
    // preconditions; a negative return means the key is currently down.
    // SAFETY: GetAsyncKeyState with a virtual-key code has no pointer
    // preconditions; a negative return means the key is currently down.
    unsafe { GetAsyncKeyState(i32::from(VK_RMENU.0)) < 0 }
}

/// Whether Ctrl, Alt, or the Windows key is currently held.
///
/// The composition loop calls this before treating a letter as composition
/// input, so shortcuts such as Ctrl+C never grow the roman buffer (the target
/// application receives no letter for those chords, and erasing afterwards
/// would delete the user's real text).
pub fn ctrl_or_alt_pressed() -> bool {
    // SAFETY: GetAsyncKeyState with a virtual-key code has no pointer
    // preconditions; a negative return means the key is currently down.
    unsafe {
        GetAsyncKeyState(i32::from(VK_CONTROL.0)) < 0
            || GetAsyncKeyState(i32::from(VK_MENU.0)) < 0
            || GetAsyncKeyState(i32::from(VK_LWIN.0)) < 0
            || GetAsyncKeyState(i32::from(VK_RWIN.0)) < 0
    }
}

/// Whether Shift is currently held (used to keep `>` a real `>`).
pub fn shift_pressed() -> bool {
    // SAFETY: GetAsyncKeyState has no pointer preconditions.
    unsafe { GetAsyncKeyState(i32::from(VK_SHIFT.0)) < 0 }
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
    use std::sync::atomic::Ordering;

    use super::{WindowsInjector, unicode_inputs, utf16_units};
    use crate::inject::{InjectionMethod, TextInjector};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT_KEYBOARD, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY,
        VK_BACK, VK_LEFT, VK_RIGHT,
    };

    /// Read the keyboard arm of an INPUT built by this module.
    fn keyboard_fields(
        input: &windows::Win32::UI::Input::KeyboardAndMouse::INPUT,
    ) -> (u16, u16, u32) {
        // SAFETY: every INPUT in these batches is built by `virtual_key_input`
        // or `key_input`, both of which initialize the `ki` union arm.
        unsafe {
            let ki = &input.Anonymous.ki;
            (ki.wVk.0, ki.wScan, ki.dwFlags.0)
        }
    }

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
    fn zero_backspaces_is_a_noop() {
        assert!(super::send_backspaces(0).is_ok());
    }

    #[test]
    fn empty_commit_is_a_noop() {
        // Early-returns before any Win32 call; safe without a hook or focus.
        assert!(super::inject_commit(0, "", false).is_ok());
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

    #[test]
    fn inject_char_batch_is_one_unicode_down_up_pair() {
        // Same batch `inject_char` sends: 0 backspaces, one char, no space.
        let inputs = super::build_commit_inputs(0, &utf16_units("a"));
        assert_eq!(inputs.len(), 2);
        let (down_vk, down_scan, down_flags) = keyboard_fields(&inputs[0]);
        let (up_vk, up_scan, up_flags) = keyboard_fields(&inputs[1]);
        assert_eq!(down_vk, VIRTUAL_KEY(0).0);
        assert_eq!(up_vk, VIRTUAL_KEY(0).0);
        assert_eq!(down_scan, u16::from(b'a'));
        assert_eq!(up_scan, u16::from(b'a'));
        assert_eq!(down_flags, KEYEVENTF_UNICODE.0);
        assert_eq!(up_flags, (KEYEVENTF_UNICODE | KEYEVENTF_KEYUP).0);
        assert_eq!(inputs[0].r#type, INPUT_KEYBOARD);
        assert_eq!(inputs[1].r#type, INPUT_KEYBOARD);
        // Zero backspaces: the fixed path never erases the swallowed letter.
        assert!(
            inputs
                .iter()
                .all(|input| keyboard_fields(input).0 != VK_BACK.0)
        );
    }

    #[test]
    fn caret_batch_is_extended_arrow_down_up_pairs() {
        let inputs = super::build_caret_inputs(true, 3);
        assert_eq!(inputs.len(), 6);
        for pair in inputs.chunks(2) {
            let (down_vk, down_scan, down_flags) = keyboard_fields(&pair[0]);
            let (up_vk, up_scan, up_flags) = keyboard_fields(&pair[1]);
            assert_eq!(down_vk, VK_LEFT.0);
            assert_eq!(up_vk, VK_LEFT.0);
            assert_eq!(down_scan, 0);
            assert_eq!(up_scan, 0);
            // Arrows are extended keys on both edges of the press.
            assert_eq!(down_flags, KEYEVENTF_EXTENDEDKEY.0);
            assert_eq!(up_flags, (KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP).0);
        }
        let right = super::build_caret_inputs(false, 1);
        assert_eq!(keyboard_fields(&right[0]).0, VK_RIGHT.0);
    }

    #[test]
    fn empty_caret_batch_is_a_noop() {
        assert!(super::build_caret_inputs(true, 0).is_empty());
    }

    #[test]
    fn bengali_char_batch_uses_its_code_unit() {
        let inputs = super::build_commit_inputs(0, &utf16_units("স"));
        assert_eq!(inputs.len(), 2);
        assert_eq!(keyboard_fields(&inputs[0]).1, 0x09B8);
        assert_eq!(keyboard_fields(&inputs[1]).1, 0x09B8);
    }

    #[test]
    fn empty_char_batch_is_a_noop() {
        assert!(super::build_commit_inputs(0, &[]).is_empty());
        // A char always has at least one UTF-16 unit; the empty path is the
        // zero-backspace, empty-text commit, which touches no Win32 API.
        assert!(super::inject_commit(0, "", false).is_ok());
    }

    #[test]
    fn swallow_letters_flag_round_trips() {
        super::set_swallow_letters(true);
        assert!(super::SWALLOW_LETTERS.load(Ordering::Relaxed));
        super::set_swallow_letters(false);
        assert!(!super::SWALLOW_LETTERS.load(Ordering::Relaxed));
    }

    #[test]
    fn modifier_flags_expose_shift_and_altgr() {
        let flags: super::ModifierFlags = super::modifier_state();
        // Live values depend on the keyboard at test time; only the shape is
        // asserted here.
        let _ = (flags.shift, flags.altgr);
        let explicit = super::ModifierFlags {
            shift: true,
            altgr: false,
        };
        assert!(explicit.shift);
        assert!(!explicit.altgr);
    }
}
