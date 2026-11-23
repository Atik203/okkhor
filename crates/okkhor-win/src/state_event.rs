//! Cross-process "input state changed" signal.
//!
//! The daemon changes the mode, pause, or encoding; the quick bar lives in the
//! UI process and would otherwise wait for its 15 s poll. A session-local
//! auto-reset event wakes the UI so it can refetch the existing snapshot. The
//! event carries no data - only a wake-up - so no user text can cross it.

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{
    CreateEventW, EVENT_MODIFY_STATE, OpenEventW, SetEvent, WaitForSingleObject,
};
use windows::core::HSTRING;

/// Session-local name. `Local\` keeps it inside the logged-in session.
pub const STATE_CHANGED_EVENT: &str = "Local\\Okkhor.State.Changed.v1";

/// Failure to create the UI-side waiter.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StateChangeError {
    #[error("could not create the state event (code {code})")]
    Create { code: i32 },
}

/// UI-side waiter. Owns the event handle for the UI process lifetime.
pub struct StateChangeWaiter {
    event: HANDLE,
}

// SAFETY: Win32 event handles are process-wide kernel objects and may be
// waited on and closed from a different thread than the one that created them.
// The waiter is moved into its dedicated thread immediately after creation.
unsafe impl Send for StateChangeWaiter {}

impl StateChangeWaiter {
    /// Create (or open) the auto-reset state event.
    pub fn new() -> Result<Self, StateChangeError> {
        let name = HSTRING::from(STATE_CHANGED_EVENT);
        // SAFETY: `name` is a valid HSTRING. A manual-reset=false, initial
        // state false event is created; an existing event is reused.
        let event = unsafe { CreateEventW(None, false, false, &name) }.map_err(|error| {
            StateChangeError::Create {
                code: error.code().0,
            }
        })?;
        Ok(Self { event })
    }

    /// Block until the daemon signals a state change.
    pub fn wait(&self) {
        // SAFETY: `self.event` is live for the lifetime of `self`.
        let _ = unsafe { WaitForSingleObject(self.event, u32::MAX) };
    }
}

impl Drop for StateChangeWaiter {
    fn drop(&mut self) {
        // SAFETY: the handle was created by CreateEventW and is owned here.
        unsafe {
            let _ = CloseHandle(self.event);
        }
    }
}

/// Daemon-side signal. `true` when a UI listener was present.
pub fn signal_state_changed() -> bool {
    let name = HSTRING::from(STATE_CHANGED_EVENT);
    // SAFETY: `name` is a valid HSTRING; EVENT_MODIFY_STATE is the minimum
    // access needed for SetEvent.
    let event = match unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &name) } {
        Ok(event) => event,
        Err(_) => return false,
    };
    // SAFETY: the handle came from OpenEventW and is valid until closed.
    let signalled = unsafe { SetEvent(event) }.is_ok();
    // SAFETY: the handle is owned by this function and closed exactly once.
    unsafe {
        let _ = CloseHandle(event);
    }
    signalled
}

/// Spawn the UI-side listener thread. `on_change` runs after each signal; the
/// caller must only send a Tauri event.
pub fn spawn_state_change_listener<F>(on_change: F) -> Result<(), StateChangeError>
where
    F: Fn() + Send + 'static,
{
    let waiter = StateChangeWaiter::new()?;
    std::thread::Builder::new()
        .name("okkhor-state-change".to_owned())
        .spawn(move || {
            loop {
                waiter.wait();
                on_change();
            }
        })
        .map(|_| ())
        .map_err(|_| StateChangeError::Create { code: 0 })
}

#[cfg(test)]
mod tests {
    use super::{STATE_CHANGED_EVENT, StateChangeError, signal_state_changed};

    #[test]
    fn event_name_is_session_local_and_versioned() {
        assert!(STATE_CHANGED_EVENT.starts_with("Local\\"));
        assert!(STATE_CHANGED_EVENT.ends_with(".v1"));
    }

    #[test]
    fn signal_without_listener_is_false_not_a_panic() {
        assert!(!signal_state_changed());
    }

    #[test]
    fn error_shape_is_static() {
        let error = StateChangeError::Create { code: 5 };
        assert_eq!(
            error.to_string(),
            "could not create the state event (code 5)"
        );
    }
}
