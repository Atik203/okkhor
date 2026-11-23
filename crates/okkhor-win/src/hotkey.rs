//! Global hotkeys: pause chord and Bangla/English mode toggle.
//!
//! Each hotkey is registered on its own thread with a message loop. Windows
//! consumes the chord, so it does not leak into the target application while
//! capture is paused or active.
//!
//! [`GlobalHotkey`] is the reusable primitive (id + modifiers + virtual key)
//! with `start`/`try_next`/`stop`. [`PauseHotkey`] keeps its existing
//! behavior and API on top of it (`Ctrl+Alt+Shift+P`). [`ModeHotkey`]
//! defaults to **F9** and falls back to **Ctrl+Space** when `RegisterHotKey`
//! fails, reporting the active binding. No admin is ever required.
//!
//! The hotkey thread never allocates and never logs: a full event queue drops
//! a repeat via `try_send`.

use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey,
    UnregisterHotKey, VK_F9, VK_P, VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, MSG, PostThreadMessageW, WM_HOTKEY, WM_QUIT,
};

const PAUSE_HOTKEY_ID: i32 = 0x4F4B; // "OK"
const PAUSE_MODIFIERS: HOT_KEY_MODIFIERS =
    HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_ALT.0 | MOD_SHIFT.0 | MOD_NOREPEAT.0);
const PAUSE_VK: u32 = VK_P.0 as u32;

const MODE_HOTKEY_ID: i32 = 0x4F4D; // "OM" (Okkhor mode)
const MODE_F9_VK: u32 = VK_F9.0 as u32;
const MODE_FALLBACK_VK: u32 = VK_SPACE.0 as u32;

#[derive(Debug, thiserror::Error)]
pub enum HotkeyError {
    #[error("the hotkey is already registered")]
    AlreadyStarted,
    #[error("could not start the hotkey thread: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("the hotkey thread exited before it was ready")]
    ThreadGone,
    #[error("Win32 call {operation} failed with code {code}")]
    Win32 { operation: &'static str, code: i32 },
}

/// One global hotkey registration: id + modifiers + virtual key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeySpec {
    /// Per-thread hotkey id. Each hotkey thread owns its own id namespace,
    /// so ids only need to be unique within one thread.
    pub id: i32,
    /// Modifier flags (`MOD_*` bits, usually with `MOD_NOREPEAT`).
    pub modifiers: HOT_KEY_MODIFIERS,
    /// Virtual key (`VK_*` code).
    pub vk: u32,
}

impl HotkeySpec {
    pub const fn new(id: i32, modifiers: HOT_KEY_MODIFIERS, vk: u32) -> Self {
        Self { id, modifiers, vk }
    }
}

/// Which binding the mode hotkey is actually using.
///
/// F9 is the default (ADR 0014 §2). When `RegisterHotKey` for F9 fails
/// (another app claimed it), the daemon falls back to Ctrl+Space and reports
/// it so the UI can show a one-time notice. No admin is involved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModeBinding {
    #[default]
    F9,
    CtrlSpace,
}

impl ModeBinding {
    /// Stable machine-readable name (also the settings spelling).
    pub const fn name(self) -> &'static str {
        match self {
            Self::F9 => "F9",
            Self::CtrlSpace => "Ctrl+Space",
        }
    }

    /// Registration for this binding (same id, different chord).
    pub const fn spec(self) -> HotkeySpec {
        match self {
            Self::F9 => HotkeySpec::new(
                MODE_HOTKEY_ID,
                HOT_KEY_MODIFIERS(MOD_NOREPEAT.0),
                MODE_F9_VK,
            ),
            Self::CtrlSpace => HotkeySpec::new(
                MODE_HOTKEY_ID,
                HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_NOREPEAT.0),
                MODE_FALLBACK_VK,
            ),
        }
    }

    /// Parse a settings/CLI spelling. Case-insensitive, `+`/space tolerant.
    /// Returns `None` for anything outside the allowlisted pair.
    pub fn parse(text: &str) -> Option<Self> {
        let normalized: String = text
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .collect();
        match normalized.as_str() {
            "f9" => Some(Self::F9),
            "ctrlspace" | "controlspace" => Some(Self::CtrlSpace),
            _ => None,
        }
    }

    /// Fallback chain: F9 degrades to Ctrl+Space; Ctrl+Space is terminal.
    pub const fn fallback(self) -> Option<Self> {
        match self {
            Self::F9 => Some(Self::CtrlSpace),
            Self::CtrlSpace => None,
        }
    }
}

/// Reusable global hotkey: one registration on its own message-loop thread.
///
/// `start` can be called again after `stop`; the registration is owned by the
/// hotkey thread and released when that thread exits.
pub struct GlobalHotkey {
    spec: HotkeySpec,
    thread_id: Option<u32>,
    thread: Option<JoinHandle<()>>,
    events: Option<Receiver<()>>,
}

impl GlobalHotkey {
    pub fn new(id: i32, modifiers: HOT_KEY_MODIFIERS, vk: u32) -> Self {
        Self::with_spec(HotkeySpec::new(id, modifiers, vk))
    }

    pub fn with_spec(spec: HotkeySpec) -> Self {
        Self {
            spec,
            thread_id: None,
            thread: None,
            events: None,
        }
    }

    /// The registration this hotkey uses.
    pub fn spec(&self) -> HotkeySpec {
        self.spec
    }

    /// Change the registration. Only takes effect on the next `start`
    /// (a running hotkey keeps its current chord until restarted).
    pub fn set_spec(&mut self, spec: HotkeySpec) {
        self.spec = spec;
    }

    /// Start listening. Registration failures are reported before any events
    /// are delivered.
    pub fn start(&mut self) -> Result<(), HotkeyError> {
        if self.thread.is_some() {
            return Err(HotkeyError::AlreadyStarted);
        }

        let spec = self.spec;
        let (event_tx, event_rx) = mpsc::sync_channel(16);
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("okkhor-hotkey".to_owned())
            .spawn(move || run_hotkey_thread(spec, event_tx, ready_tx))
            .map_err(HotkeyError::Spawn)?;

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
                Err(HotkeyError::ThreadGone)
            }
        }
    }

    /// Poll for a hotkey press without blocking.
    pub fn try_next(&self) -> bool {
        self.events
            .as_ref()
            .is_some_and(|events| events.try_recv().is_ok())
    }

    pub fn is_running(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }

    pub fn stop(&mut self) {
        if let Some(thread_id) = self.thread_id.take() {
            // SAFETY: thread_id came from the live hotkey thread.
            let _ = unsafe { PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.events = None;
    }
}

impl Drop for GlobalHotkey {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A process-local registration for `Ctrl+Alt+Shift+P`.
///
/// `start` can be called again after `stop`; the registration is owned by the
/// hotkey thread and released when that thread exits.
#[derive(Default)]
pub struct PauseHotkey {
    inner: Option<GlobalHotkey>,
    thread_id: Option<u32>,
    thread: Option<JoinHandle<()>>,
    events: Option<Receiver<()>>,
}

impl PauseHotkey {
    pub fn new() -> Self {
        Self::default()
    }

    fn ensure_inner(&mut self) -> &mut GlobalHotkey {
        self.inner
            .get_or_insert_with(|| GlobalHotkey::new(PAUSE_HOTKEY_ID, PAUSE_MODIFIERS, PAUSE_VK))
    }

    /// Start listening. Registration failures are reported before any events
    /// are delivered.
    pub fn start(&mut self) -> Result<(), HotkeyError> {
        if self.thread.is_some() || self.inner.as_ref().is_some_and(|h| h.is_running()) {
            return Err(HotkeyError::AlreadyStarted);
        }
        // Delegate to the shared thread body so pause and mode stay on the
        // same message-loop pattern.
        let spec = HotkeySpec::new(PAUSE_HOTKEY_ID, PAUSE_MODIFIERS, PAUSE_VK);
        let (event_tx, event_rx) = mpsc::sync_channel(16);
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("okkhor-pause-hotkey".to_owned())
            .spawn(move || run_hotkey_thread(spec, event_tx, ready_tx))
            .map_err(HotkeyError::Spawn)?;

        match ready_rx.recv() {
            Ok(Ok(thread_id)) => {
                self.thread_id = Some(thread_id);
                self.thread = Some(thread);
                self.events = Some(event_rx);
                // Keep a matching generic handle for API symmetry; it stays
                // idle while the dedicated fields own the registration.
                let _ = self.ensure_inner();
                Ok(())
            }
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(HotkeyError::ThreadGone)
            }
        }
    }

    /// Poll for a hotkey press without blocking.
    pub fn try_next(&self) -> bool {
        if let Some(events) = self.events.as_ref() {
            if events.try_recv().is_ok() {
                return true;
            }
        }
        self.inner.as_ref().is_some_and(|h| h.try_next())
    }

    pub fn is_running(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
            || self.inner.as_ref().is_some_and(|h| h.is_running())
    }

    pub fn stop(&mut self) {
        if let Some(thread_id) = self.thread_id.take() {
            // SAFETY: thread_id came from the live hotkey thread.
            let _ = unsafe { PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.events = None;
        if let Some(inner) = self.inner.as_mut() {
            inner.stop();
        }
    }
}

impl Drop for PauseHotkey {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Bangla/English mode toggle: F9 by default, Ctrl+Space on conflict.
///
/// `start` tries F9 first; if that registration fails it retries once with
/// Ctrl+Space and reports which binding won. Any other startup failure is
/// returned without a retry.
pub struct ModeHotkey {
    inner: GlobalHotkey,
    binding: ModeBinding,
}

impl ModeHotkey {
    pub fn new() -> Self {
        Self {
            inner: GlobalHotkey::with_spec(ModeBinding::F9.spec()),
            binding: ModeBinding::F9,
        }
    }

    /// Start listening; returns the active binding (F9 or the Ctrl+Space
    /// fallback). Retries only a `RegisterHotKey` conflict, once.
    pub fn start(&mut self) -> Result<ModeBinding, HotkeyError> {
        if self.inner.is_running() {
            return Err(HotkeyError::AlreadyStarted);
        }
        let mut binding = ModeBinding::F9;
        self.inner.set_spec(binding.spec());
        match self.inner.start() {
            Ok(()) => {
                self.binding = binding;
                Ok(binding)
            }
            Err(HotkeyError::Win32 { operation, code }) if operation == "RegisterHotKey" => {
                let Some(next) = binding.fallback() else {
                    return Err(HotkeyError::Win32 { operation, code });
                };
                binding = next;
                // Fresh handle: the failed start joined its thread already.
                self.inner = GlobalHotkey::with_spec(binding.spec());
                self.inner.start()?;
                self.binding = binding;
                Ok(binding)
            }
            Err(other) => Err(other),
        }
    }

    /// Poll for a toggle press without blocking.
    pub fn try_next(&self) -> bool {
        self.inner.try_next()
    }

    pub fn is_running(&self) -> bool {
        self.inner.is_running()
    }

    /// Which binding won at `start` time (F9 unless the fallback fired).
    pub fn binding(&self) -> ModeBinding {
        self.binding
    }

    pub fn stop(&mut self) {
        self.inner.stop();
    }
}

impl Default for ModeHotkey {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ModeHotkey {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_hotkey_thread(
    spec: HotkeySpec,
    event_tx: mpsc::SyncSender<()>,
    ready_tx: mpsc::Sender<Result<u32, HotkeyError>>,
) {
    // SAFETY: GetCurrentThreadId has no preconditions.
    let thread_id = unsafe { GetCurrentThreadId() };
    if thread_id == 0 {
        let _ = ready_tx.send(Err(HotkeyError::ThreadGone));
        return;
    }

    // SAFETY: a null window associates the hotkey with this thread's message
    // queue; MOD_NOREPEAT prevents key repeat.
    if let Err(error) = unsafe { RegisterHotKey(None, spec.id, spec.modifiers, spec.vk) } {
        let _ = ready_tx.send(Err(HotkeyError::Win32 {
            operation: "RegisterHotKey",
            code: error.code().0,
        }));
        return;
    }

    if ready_tx.send(Ok(thread_id)).is_err() {
        // SAFETY: the registration above is owned by this thread.
        let _ = unsafe { UnregisterHotKey(None, spec.id) };
        return;
    }

    let mut message = MSG::default();
    loop {
        // SAFETY: message is a valid local MSG and no window filter is used.
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if result.0 <= 0 {
            break;
        }
        if message.message == WM_HOTKEY && message.wParam.0 as i32 == spec.id {
            // Never block the hotkey thread; a full queue drops a repeat.
            // No allocation, no logging here: this runs beside the key path.
            let _ = event_tx.try_send(());
        }
    }

    // SAFETY: the registration above is owned by this thread.
    let _ = unsafe { UnregisterHotKey(None, spec.id) };
}

#[cfg(test)]
mod tests {
    use super::{HOT_KEY_MODIFIERS, HotkeySpec, MOD_CONTROL, MOD_NOREPEAT, ModeBinding};

    #[test]
    fn f9_is_the_default_binding() {
        assert_eq!(ModeBinding::default(), ModeBinding::F9);
        assert_eq!(ModeBinding::F9.name(), "F9");
        assert_eq!(ModeBinding::CtrlSpace.name(), "Ctrl+Space");
    }

    #[test]
    fn binding_specs_use_distinct_chords() {
        let f9 = ModeBinding::F9.spec();
        let fallback = ModeBinding::CtrlSpace.spec();
        // Same per-thread id namespace; the chord is what differs.
        assert_eq!(f9.id, fallback.id);
        assert_eq!(f9.vk, 0x78); // VK_F9
        assert_eq!(fallback.vk, 0x20); // VK_SPACE
        assert_eq!(f9.modifiers, HOT_KEY_MODIFIERS(MOD_NOREPEAT.0));
        assert_eq!(
            fallback.modifiers,
            HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_NOREPEAT.0)
        );
    }

    #[test]
    fn binding_parse_accepts_only_the_allowlisted_pair() {
        assert_eq!(ModeBinding::parse("F9"), Some(ModeBinding::F9));
        assert_eq!(ModeBinding::parse("f9"), Some(ModeBinding::F9));
        assert_eq!(ModeBinding::parse(" F9 "), Some(ModeBinding::F9));
        assert_eq!(
            ModeBinding::parse("Ctrl+Space"),
            Some(ModeBinding::CtrlSpace)
        );
        assert_eq!(
            ModeBinding::parse("ctrl+space"),
            Some(ModeBinding::CtrlSpace)
        );
        assert_eq!(
            ModeBinding::parse("Control + Space"),
            Some(ModeBinding::CtrlSpace)
        );
        assert_eq!(ModeBinding::parse(""), None);
        assert_eq!(ModeBinding::parse("F11"), None);
        assert_eq!(ModeBinding::parse("Alt+Shift"), None);
        assert_eq!(ModeBinding::parse("Ctrl+Alt+Shift+P"), None);
    }

    #[test]
    fn fallback_chain_is_f9_then_ctrl_space_then_terminal() {
        assert_eq!(ModeBinding::F9.fallback(), Some(ModeBinding::CtrlSpace));
        assert_eq!(ModeBinding::CtrlSpace.fallback(), None);
    }

    #[test]
    fn global_hotkey_keeps_its_spec() {
        let spec = HotkeySpec::new(0x4F4D, HOT_KEY_MODIFIERS(MOD_NOREPEAT.0), 0x78);
        let hotkey = super::GlobalHotkey::with_spec(spec);
        assert_eq!(hotkey.spec(), spec);
    }

    /// Manual smoke: run with `-- --ignored` on a real desktop. Registers F9
    /// for ~3 s. Ignored in CI so no hook or hotkey is ever installed there.
    #[test]
    #[ignore = "manual smoke only: registers a real global hotkey"]
    fn manual_mode_hotkey_registers_f9_or_fallback() {
        let mut hotkey = super::ModeHotkey::new();
        let binding = hotkey.start().expect("register F9 or fallback");
        assert!(matches!(binding, ModeBinding::F9 | ModeBinding::CtrlSpace));
        hotkey.stop();
    }
}
