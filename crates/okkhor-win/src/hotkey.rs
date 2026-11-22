//! Global pause hotkey.
//!
//! The hotkey is registered on its own thread with a message loop. Windows
//! consumes the chord, so it does not leak into the target application while
//! capture is paused or active.

use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey,
    UnregisterHotKey, VK_P,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, MSG, PostThreadMessageW, WM_HOTKEY, WM_QUIT,
};

const HOTKEY_ID: i32 = 0x4F4B; // "OK"
const MODIFIERS: HOT_KEY_MODIFIERS =
    HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_ALT.0 | MOD_SHIFT.0 | MOD_NOREPEAT.0);

#[derive(Debug, thiserror::Error)]
pub enum HotkeyError {
    #[error("the pause hotkey is already registered")]
    AlreadyStarted,
    #[error("could not start the hotkey thread: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("the hotkey thread exited before it was ready")]
    ThreadGone,
    #[error("Win32 call {operation} failed with code {code}")]
    Win32 { operation: &'static str, code: i32 },
}

/// A process-local registration for `Ctrl+Alt+Shift+P`.
///
/// `start` can be called again after `stop`; the registration is owned by the
/// hotkey thread and released when that thread exits.
#[derive(Default)]
pub struct PauseHotkey {
    thread_id: Option<u32>,
    thread: Option<JoinHandle<()>>,
    events: Option<Receiver<()>>,
}

impl PauseHotkey {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start listening. Registration failures are reported before any events
    /// are delivered.
    pub fn start(&mut self) -> Result<(), HotkeyError> {
        if self.thread.is_some() {
            return Err(HotkeyError::AlreadyStarted);
        }

        let (event_tx, event_rx) = mpsc::sync_channel(16);
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("okkhor-pause-hotkey".to_owned())
            .spawn(move || run_hotkey_thread(event_tx, ready_tx))
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

impl Drop for PauseHotkey {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_hotkey_thread(
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
    // queue; the chord is fixed and MOD_NOREPEAT prevents key repeat.
    if let Err(error) = unsafe { RegisterHotKey(None, HOTKEY_ID, MODIFIERS, u32::from(VK_P.0)) } {
        let _ = ready_tx.send(Err(HotkeyError::Win32 {
            operation: "RegisterHotKey",
            code: error.code().0,
        }));
        return;
    }

    if ready_tx.send(Ok(thread_id)).is_err() {
        // SAFETY: the registration above is owned by this thread.
        let _ = unsafe { UnregisterHotKey(None, HOTKEY_ID) };
        return;
    }

    let mut message = MSG::default();
    loop {
        // SAFETY: message is a valid local MSG and no window filter is used.
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if result.0 <= 0 {
            break;
        }
        if message.message == WM_HOTKEY && message.wParam.0 as i32 == HOTKEY_ID {
            // Never block the hotkey thread; a full queue drops a repeat.
            let _ = event_tx.try_send(());
        }
    }

    // SAFETY: the registration above is owned by this thread.
    let _ = unsafe { UnregisterHotKey(None, HOTKEY_ID) };
}
