//! Per-user single-instance guard.

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::HSTRING;

/// Failure to claim the per-user daemon instance.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SingleInstanceError {
    #[error("another Okkhor instance is already running")]
    AlreadyRunning,
    #[error("CreateMutexW failed with code {code}")]
    Win32 { code: i32 },
}

/// Owns the named mutex that prevents a second daemon from starting.
///
/// The mutex name should be session-local (`Local\...`) and stable across
/// launches. Dropping the guard releases the mutex.
pub struct SingleInstance {
    handle: HANDLE,
}

impl SingleInstance {
    pub fn acquire(name: &str) -> Result<Self, SingleInstanceError> {
        let name = HSTRING::from(name);
        // SAFETY: the name is a valid HSTRING; no security attributes are used.
        let handle = unsafe { CreateMutexW(None, false, &name) }.map_err(|error| {
            SingleInstanceError::Win32 {
                code: error.code().0,
            }
        })?;

        // SAFETY: GetLastError is thread-local and has no preconditions.
        let last_error = unsafe { GetLastError() };
        if last_error == ERROR_ALREADY_EXISTS {
            // SAFETY: handle was returned by CreateMutexW and is owned here.
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(SingleInstanceError::AlreadyRunning);
        }

        Ok(Self { handle })
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        // SAFETY: handle is owned by this guard and is closed exactly once.
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SingleInstance, SingleInstanceError};

    #[test]
    fn second_instance_is_rejected_and_released_on_drop() {
        let name = format!("Local\\Okkhor.Test.SingleInstance.{}", std::process::id());

        let first = SingleInstance::acquire(&name).expect("first instance");
        let second = SingleInstance::acquire(&name);
        assert!(matches!(second, Err(SingleInstanceError::AlreadyRunning)));

        drop(first);
        let third = SingleInstance::acquire(&name).expect("released instance");
        drop(third);
    }
}
