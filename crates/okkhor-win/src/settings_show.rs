//! Cross-process "show Settings" signal.
//!
//! The tray lives in the daemon but the settings window lives in the UI
//! process. A session-local named event bridges them without a second
//! protocol: the UI creates the event and waits on it in a dedicated thread;
//! the daemon sets it when the tray menu asks for Settings. When no UI is
//! listening, `OpenEventW` fails and the daemon launches the UI instead.
//!
//! The event carries no data - only a wake-up - so no user text can cross it.

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{
    CreateEventW, EVENT_MODIFY_STATE, OpenEventW, SetEvent, WaitForSingleObject,
};
use windows::core::HSTRING;

/// Session-local name. `Local\` keeps it inside the logged-in session.
pub const SETTINGS_SHOW_EVENT: &str = "Local\\Okkhor.Settings.Show.v1";

/// Failure to create the UI-side waiter.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SettingsShowError {
    #[error("could not create the settings event (code {code})")]
    Create { code: i32 },
}

/// UI-side waiter for the settings event.
pub struct SettingsShowWaiter {
    event: HANDLE,
}

// SAFETY: Win32 event handles are process-wide kernel objects and may be
// waited on and closed from a different thread than the one that created them.
unsafe impl Send for SettingsShowWaiter {}

impl SettingsShowWaiter {
    /// Create (or open) the auto-reset settings event.
    pub fn new() -> Result<Self, SettingsShowError> {
        let name = HSTRING::from(SETTINGS_SHOW_EVENT);
        // SAFETY: `name` is a valid HSTRING; the event is auto-reset with an
        // initial state of false, and an existing event is reopened.
        let event = unsafe { CreateEventW(None, false, false, &name) }.map_err(|error| {
            SettingsShowError::Create {
                code: error.code().0,
            }
        })?;
        Ok(Self { event })
    }

    /// Block until the daemon asks for Settings.
    pub fn wait(&self) {
        // SAFETY: `self.event` is live for the lifetime of `self`; an infinite
        // wait only blocks this dedicated thread.
        let _ = unsafe { WaitForSingleObject(self.event, u32::MAX) };
    }
}

impl Drop for SettingsShowWaiter {
    fn drop(&mut self) {
        // SAFETY: the handle was created by CreateEventW and is owned here.
        unsafe {
            let _ = CloseHandle(self.event);
        }
    }
}

/// Daemon-side signal. `true` when a UI listener was present.
pub fn signal_settings_show() -> bool {
    let name = HSTRING::from(SETTINGS_SHOW_EVENT);
    // SAFETY: `name` is a valid HSTRING; EVENT_MODIFY_STATE is the minimum
    // access needed for SetEvent.
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

/// Spawn the UI-side listener thread.
pub fn spawn_settings_show_listener<F>(on_show: F) -> Result<(), SettingsShowError>
where
    F: Fn() + Send + 'static,
{
    let waiter = SettingsShowWaiter::new()?;
    std::thread::Builder::new()
        .name("okkhor-settings-show".to_owned())
        .spawn(move || {
            loop {
                waiter.wait();
                on_show();
            }
        })
        .map(|_| ())
        .map_err(|_| SettingsShowError::Create { code: 0 })
}

#[cfg(test)]
mod tests {
    use super::{SETTINGS_SHOW_EVENT, signal_settings_show};

    #[test]
    fn event_name_is_session_local_and_versioned() {
        assert!(SETTINGS_SHOW_EVENT.starts_with("Local\\"));
        assert!(SETTINGS_SHOW_EVENT.ends_with(".v1"));
    }

    #[test]
    fn signal_without_listener_is_false_not_a_panic() {
        assert!(!signal_settings_show());
    }
}
