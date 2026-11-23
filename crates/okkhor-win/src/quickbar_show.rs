//! Cross-process "show quick bar" signal.
//!
//! The tray lives in the daemon but the bar lives in the UI process. A
//! session-local named event bridges them without a second protocol: the UI
//! creates the event (auto-reset) and waits on it in a dedicated thread; the
//! daemon opens it and sets it when the tray menu asks for the bar. When the
//! UI is not running, `OpenEventW` fails and the daemon falls back to launching
//! the UI with `--quickbar`.
//!
//! The event carries no data - only a wake-up - so no user text can cross it.

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{
    CreateEventW, EVENT_MODIFY_STATE, OpenEventW, SetEvent, WaitForSingleObject,
};
use windows::core::HSTRING;

/// Session-local name. `Local\` keeps it inside the logged-in session.
pub const QUICKBAR_SHOW_EVENT: &str = "Local\\Okkhor.QuickBar.Show.v1";

/// Failure to create the UI-side waiter.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum QuickbarShowError {
    #[error("could not create the quick bar event (code {code})")]
    Create { code: i32 },
}

/// UI-side waiter. Owns the event handle for the lifetime of the UI process;
/// the handle is closed when the listener thread exits.
pub struct QuickbarShowWaiter {
    event: HANDLE,
}

// SAFETY: Win32 event handles are process-wide kernel objects and may be
// waited on and closed from a different thread than the one that created them.
// `spawn_quickbar_show_listener` moves the waiter into its dedicated thread
// immediately after creation and never shares it.
unsafe impl Send for QuickbarShowWaiter {}

impl QuickbarShowWaiter {
    /// Create (or open) the auto-reset show event.
    pub fn new() -> Result<Self, QuickbarShowError> {
        let name = HSTRING::from(QUICKBAR_SHOW_EVENT);
        // SAFETY: `name` is a valid HSTRING. A manual-reset=false, initial
        // state false event is created; if it already exists, Windows returns
        // the existing handle, which is what a restarted UI wants.
        let event = unsafe { CreateEventW(None, false, false, &name) }.map_err(|error| {
            QuickbarShowError::Create {
                code: error.code().0,
            }
        })?;
        Ok(Self { event })
    }

    /// Block until the daemon asks for the bar. Returns after one signal; the
    /// auto-reset event is ready for the next request.
    pub fn wait(&self) {
        // SAFETY: `self.event` is live for the lifetime of `self`; waiting with
        // an infinite timeout only blocks this dedicated thread and returns on
        // process teardown when the handle is closed.
        let _ = unsafe { WaitForSingleObject(self.event, u32::MAX) };
    }
}

impl Drop for QuickbarShowWaiter {
    fn drop(&mut self) {
        // SAFETY: the handle was created by CreateEventW and is owned here; it
        // is closed exactly once.
        unsafe {
            let _ = CloseHandle(self.event);
        }
    }
}

/// Daemon-side signal. `true` when a UI listener was present and the event was
/// set; `false` when the UI is not running (caller launches it instead).
pub fn signal_quickbar_show() -> bool {
    let name = HSTRING::from(QUICKBAR_SHOW_EVENT);
    // SAFETY: `name` is a valid HSTRING; EVENT_MODIFY_STATE is the minimum
    // access needed for SetEvent. `false` keeps inherited-handle semantics
    // unchanged.
    let event = match unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &name) } {
        Ok(event) => event,
        Err(_) => return false,
    };
    // SAFETY: the handle came from OpenEventW and is valid until closed below.
    let signalled = unsafe { SetEvent(event) }.is_ok();
    // SAFETY: the handle is owned by this function and closed exactly once.
    unsafe {
        let _ = CloseHandle(event);
    }
    signalled
}

/// Spawn the UI-side listener thread. `on_show` runs on that thread after each
/// signal; the caller must only send a Tauri event (no window calls that need
/// the UI thread).
pub fn spawn_quickbar_show_listener<F>(on_show: F) -> Result<(), QuickbarShowError>
where
    F: Fn() + Send + 'static,
{
    let waiter = QuickbarShowWaiter::new()?;
    std::thread::Builder::new()
        .name("okkhor-quickbar-show".to_owned())
        .spawn(move || {
            loop {
                waiter.wait();
                on_show();
            }
        })
        .map(|_| ())
        .map_err(|_| QuickbarShowError::Create { code: 0 })
}

#[cfg(test)]
mod tests {
    use super::{QUICKBAR_SHOW_EVENT, QuickbarShowError, signal_quickbar_show};

    #[test]
    fn event_name_is_session_local_and_versioned() {
        assert!(QUICKBAR_SHOW_EVENT.starts_with("Local\\"));
        assert!(QUICKBAR_SHOW_EVENT.ends_with(".v1"));
    }

    #[test]
    fn signal_without_listener_is_false_not_a_panic() {
        // This test process never creates the event, so the daemon-side path
        // must report "not running" instead of panicking.
        assert!(!signal_quickbar_show());
    }

    #[test]
    fn error_shape_is_static() {
        let error = QuickbarShowError::Create { code: 5 };
        assert_eq!(
            error.to_string(),
            "could not create the quick bar event (code 5)"
        );
    }
}
