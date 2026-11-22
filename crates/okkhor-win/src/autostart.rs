//! Per-user autostart through the Windows `Run` key.
//!
//! Okkhor registers a single value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. The value points at the
//! current executable followed by the daemon argument, so the same binary can
//! also be used for the self-check and manual demos.
//!
//! Tests cover the command string and constants only. They never read or write
//! the real registry.

use std::path::Path;

use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE,
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::core::{PCWSTR, w};

/// Registry key that stores per-user startup programs.
pub const AUTOSTART_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Value name owned by Okkhor.
pub const AUTOSTART_VALUE_NAME: &str = "Okkhor";
/// Argument that tells the daemon binary to stay resident.
pub const DAEMON_ARGUMENT: &str = "daemon";

const AUTOSTART_KEY_W: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const AUTOSTART_VALUE_NAME_W: PCWSTR = w!("Okkhor");

/// Failure to inspect or change the per-user autostart entry.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AutostartError {
    #[error("the executable path is not absolute")]
    NotAbsolute,
    #[error("the executable path is not valid UTF-8")]
    NotUnicode,
    #[error("the executable path contains a NUL character")]
    Nul,
    #[error("Win32 call {operation} failed with code {code}")]
    Win32 { operation: &'static str, code: i32 },
}

/// Build the command line stored in the `Run` value.
///
/// The executable is always quoted and its embedded backslashes and quotes are
/// escaped with the same rules `CreateProcessW` uses for arguments. The daemon
/// argument is appended so a normal login starts the resident process rather
/// than printing help.
pub fn autostart_command(executable: &Path) -> Result<String, AutostartError> {
    if !executable.is_absolute() {
        return Err(AutostartError::NotAbsolute);
    }
    let executable = executable.to_str().ok_or(AutostartError::NotUnicode)?;
    if executable.contains('\0') {
        return Err(AutostartError::Nul);
    }

    Ok(format!("{} {DAEMON_ARGUMENT}", quote_argument(executable)))
}

/// Register `executable` as the per-user startup command.
///
/// This writes `HKCU\...\Run\Okkhor`; it never elevates and never touches
/// machine-wide state.
pub fn enable(executable: &Path) -> Result<(), AutostartError> {
    let command = autostart_command(executable)?;
    let key = open_run_key(KEY_SET_VALUE)?;

    let mut wide: Vec<u16> = command.encode_utf16().collect();
    wide.push(0);
    // Windows targets are little-endian, and the registry API copies the bytes
    // before returning, so this owned buffer is not retained.
    let bytes: Vec<u8> = wide.iter().flat_map(|unit| unit.to_le_bytes()).collect();

    // SAFETY: key is an open HKEY owned by `key`, the value name and data are
    // valid buffers that live through the call, and REG_SZ data includes the
    // terminating NUL.
    let status =
        unsafe { RegSetValueExW(key.0, AUTOSTART_VALUE_NAME_W, None, REG_SZ, Some(&bytes)) };
    if status.is_err() {
        return Err(win32("RegSetValueExW", status.0));
    }
    Ok(())
}

/// Remove the Okkhor value. Returns `false` when it was not present.
pub fn disable() -> Result<bool, AutostartError> {
    let key = match open_run_key(KEY_SET_VALUE) {
        Ok(key) => key,
        Err(AutostartError::Win32 { code, .. }) if code == ERROR_FILE_NOT_FOUND.0 as i32 => {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };

    // SAFETY: key is an open HKEY owned by `key` and the value name is a valid
    // null-terminated string.
    let status = unsafe { RegDeleteValueW(key.0, AUTOSTART_VALUE_NAME_W) };
    if status == ERROR_FILE_NOT_FOUND {
        Ok(false)
    } else if status.is_err() {
        Err(win32("RegDeleteValueW", status.0))
    } else {
        Ok(true)
    }
}

/// Whether the per-user autostart value exists.
pub fn is_enabled() -> Result<bool, AutostartError> {
    let key = match open_run_key(KEY_QUERY_VALUE) {
        Ok(key) => key,
        Err(AutostartError::Win32 { code, .. }) if code == ERROR_FILE_NOT_FOUND.0 as i32 => {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };

    let mut value_type = REG_VALUE_TYPE::default();
    let mut size = 0_u32;
    // SAFETY: key is an open HKEY owned by `key`; the type and size out
    // parameters point to valid local storage. No data buffer is requested, so
    // a successful call only reports the stored type and size.
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            AUTOSTART_VALUE_NAME_W,
            None,
            Some(&mut value_type),
            None,
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    if status.is_err() {
        return Err(win32("RegQueryValueExW", status.0));
    }
    if value_type != REG_SZ || size < 2 {
        return Ok(false);
    }
    Ok(true)
}

/// QUOTE a single Windows command-line argument.
///
/// Backslashes are doubled when they precede a quote or the closing quote; an
/// embedded quote is escaped. This matches the argument grammar used by
/// `CommandLineToArgvW` and the C runtime.
fn quote_argument(argument: &str) -> String {
    let mut quoted = String::with_capacity(argument.len() + 2);
    quoted.push('"');
    let mut backslashes = 0_usize;
    for character in argument.chars() {
        match character {
            '\\' => {
                backslashes = backslashes.saturating_add(1);
                quoted.push('\\');
            }
            '"' => {
                for _ in 0..=backslashes {
                    quoted.push('\\');
                }
                quoted.push('"');
                backslashes = 0;
            }
            other => {
                backslashes = 0;
                quoted.push(other);
            }
        }
    }
    for _ in 0..backslashes {
        quoted.push('\\');
    }
    quoted.push('"');
    quoted
}

fn open_run_key(access: REG_SAM_FLAGS) -> Result<RegistryKey, AutostartError> {
    let mut key = HKEY::default();
    // SAFETY: HKEY_CURRENT_USER is a predefined root, the subkey is a valid
    // null-terminated string, and `key` points to valid local storage.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            AUTOSTART_KEY_W,
            Some(0),
            access,
            &mut key,
        )
    };
    if status.is_err() {
        Err(win32("RegOpenKeyExW", status.0))
    } else {
        Ok(RegistryKey(key))
    }
}

fn win32(operation: &'static str, code: u32) -> AutostartError {
    AutostartError::Win32 {
        operation,
        code: code as i32,
    }
}

/// Owns an open registry key and closes it on every exit path.
struct RegistryKey(HKEY);

impl Drop for RegistryKey {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by RegOpenKeyExW and is closed once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AUTOSTART_KEY, AUTOSTART_VALUE_NAME, AutostartError, DAEMON_ARGUMENT, autostart_command,
    };
    use std::path::Path;

    #[test]
    fn command_quotes_a_path_with_spaces_and_appends_the_daemon_argument() {
        let command =
            autostart_command(Path::new(r"C:\Program Files\Okkhor\okkhor.exe")).expect("command");
        assert_eq!(command, r#""C:\Program Files\Okkhor\okkhor.exe" daemon"#);
        assert!(command.ends_with(DAEMON_ARGUMENT));
    }

    #[test]
    fn command_doubles_trailing_backslashes_before_the_closing_quote() {
        let command = autostart_command(Path::new(r"C:\Okkhor\")).expect("command");
        assert_eq!(command, r#""C:\Okkhor\\" daemon"#);
    }

    #[test]
    fn command_escapes_embedded_quotes() {
        let command = autostart_command(Path::new(r#"C:\Okkhor"odd.exe"#)).expect("command");
        assert_eq!(command, r#""C:\Okkhor\"odd.exe" daemon"#);
    }

    #[test]
    fn command_rejects_relative_paths() {
        let error = autostart_command(Path::new(r"target\debug\okkhor.exe")).expect_err("relative");
        assert_eq!(error, AutostartError::NotAbsolute);
    }

    #[test]
    fn registry_location_is_the_per_user_run_key() {
        assert_eq!(
            AUTOSTART_KEY,
            r"Software\Microsoft\Windows\CurrentVersion\Run"
        );
        assert_eq!(AUTOSTART_VALUE_NAME, "Okkhor");
    }

    #[test]
    fn command_has_no_registry_side_effects() {
        // This is a guard for the test suite: only the pure command builder is
        // exercised here. `enable`, `disable`, and `is_enabled` are integration
        // paths and are never called from unit tests.
        let command = autostart_command(Path::new(r"C:\Okkhor\okkhor.exe")).expect("command");
        assert!(command.starts_with('"'));
    }
}
