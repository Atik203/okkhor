//! Data-home path resolution for the daemon's local files.
//!
//! Layout (contract §2/§3, ADR 0012 §2):
//!
//! - `<home>/settings.json` — plaintext settings; the daemon is the only writer.
//! - `<home>/data/words.bin` — DPAPI-encrypted user words.
//! - `<home>/runtime/ipc.token` — session token (same file as
//!   `okkhor_ipc::auth::default_token_path`).
//!
//! `<home>` is `%LOCALAPPDATA%\Okkhor`, unless `OKKHOR_DATA_HOME` is set, in
//! which case the override wins. The override exists for tests, dev, and
//! portable use only: it reads the daemon process's own environment, so it is
//! a same-user convenience, not a security boundary. Production never sets
//! it.
//!
//! Errors carry no paths: a full path can contain the user's name, which must
//! never reach logs (see `docs/security-privacy.md`).

use std::path::PathBuf;

use thiserror::Error;

/// Environment override for the data home. Tests, dev, and portable use
/// only — never set in production.
pub const DATA_HOME_OVERRIDE_ENV: &str = "OKKHOR_DATA_HOME";
/// Application directory under `%LOCALAPPDATA%`.
const APP_DIR: &str = "Okkhor";

/// Path resolution failures. The messages are static and carry no paths.
#[derive(Debug, Error)]
pub enum PathsError {
    /// Neither the override nor `%LOCALAPPDATA%` is available.
    #[error("data home is not available")]
    NotAvailable,
}

/// Resolve the daemon data home (`%LOCALAPPDATA%\Okkhor`, or
/// `OKKHOR_DATA_HOME` when set and non-empty).
pub fn data_home() -> Result<PathBuf, PathsError> {
    if let Some(dir) = std::env::var_os(DATA_HOME_OVERRIDE_ENV)
        && !dir.is_empty()
    {
        return Ok(PathBuf::from(dir));
    }
    std::env::var_os("LOCALAPPDATA")
        .map(|base| PathBuf::from(base).join(APP_DIR))
        .ok_or(PathsError::NotAvailable)
}

/// Resolve `<home>/settings.json` (contract §2).
pub fn settings_path() -> Result<PathBuf, PathsError> {
    Ok(data_home()?.join("settings.json"))
}

/// Resolve `<home>/data/words.bin` (contract §3).
pub fn words_path() -> Result<PathBuf, PathsError> {
    Ok(data_home()?.join("data").join("words.bin"))
}

/// Resolve `<home>/runtime/ipc.token` (ADR 0012 §2).
///
/// This matches `okkhor_ipc::auth::default_token_path`: the override maps to
/// `<override>/runtime`, otherwise `%LOCALAPPDATA%/Okkhor/runtime`.
pub fn token_path() -> Result<PathBuf, PathsError> {
    Ok(data_home()?.join("runtime").join("ipc.token"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    /// Process environment is global: serialize every test that mutates it.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// Set an env var. Callers must hold `env_lock` (via [`EnvGuard`]).
    fn set_var_locked(key: &str, value: &str) {
        // SAFETY: the caller holds `env_lock`, so no other test in this
        // process mutates the environment concurrently; the values under
        // test are read back synchronously on this thread.
        unsafe {
            std::env::set_var(key, value);
        }
    }

    /// Remove an env var. Callers must hold `env_lock` (via [`EnvGuard`]).
    fn remove_var_locked(key: &str) {
        // SAFETY: same contract as `set_var_locked`.
        unsafe {
            std::env::remove_var(key);
        }
    }

    struct EnvGuard<'a> {
        _lock: std::sync::MutexGuard<'a, ()>,
        saved_override: Option<std::ffi::OsString>,
        saved_local: Option<std::ffi::OsString>,
    }

    impl EnvGuard<'_> {
        fn take() -> Self {
            let lock = env_lock().lock().expect("env lock");
            let saved_override = std::env::var_os(DATA_HOME_OVERRIDE_ENV);
            let saved_local = std::env::var_os("LOCALAPPDATA");
            Self {
                _lock: lock,
                saved_override,
                saved_local,
            }
        }
    }

    impl Drop for EnvGuard<'_> {
        fn drop(&mut self) {
            match &self.saved_override {
                Some(value) => set_var_locked(
                    DATA_HOME_OVERRIDE_ENV,
                    value.to_str().expect("override is UTF-8"),
                ),
                None => remove_var_locked(DATA_HOME_OVERRIDE_ENV),
            }
            match &self.saved_local {
                Some(value) => {
                    set_var_locked("LOCALAPPDATA", value.to_str().expect("profile is UTF-8"))
                }
                None => remove_var_locked("LOCALAPPDATA"),
            }
        }
    }

    #[test]
    fn override_wins_over_profile() {
        let _guard = EnvGuard::take();
        set_var_locked(DATA_HOME_OVERRIDE_ENV, "E:\\TestDataHome");
        set_var_locked("LOCALAPPDATA", "C:\\Users\\someone\\AppData\\Local");

        assert_eq!(
            data_home().expect("home"),
            PathBuf::from("E:\\TestDataHome")
        );
        assert_eq!(
            settings_path().expect("settings"),
            PathBuf::from("E:\\TestDataHome").join("settings.json")
        );
        assert_eq!(
            words_path().expect("words"),
            PathBuf::from("E:\\TestDataHome")
                .join("data")
                .join("words.bin")
        );
        assert_eq!(
            token_path().expect("token"),
            PathBuf::from("E:\\TestDataHome")
                .join("runtime")
                .join("ipc.token")
        );
    }

    #[test]
    fn profile_is_used_without_override() {
        let _guard = EnvGuard::take();
        remove_var_locked(DATA_HOME_OVERRIDE_ENV);
        set_var_locked("LOCALAPPDATA", "C:\\Users\\someone\\AppData\\Local");

        let base = PathBuf::from("C:\\Users\\someone\\AppData\\Local").join("Okkhor");
        assert_eq!(data_home().expect("home"), base);
        // The token path matches `okkhor_ipc::auth::default_token_path`.
        assert_eq!(
            token_path().expect("token"),
            okkhor_ipc::auth::default_token_path().expect("ipc token path")
        );
    }

    #[test]
    fn empty_override_falls_back_to_profile() {
        let _guard = EnvGuard::take();
        set_var_locked(DATA_HOME_OVERRIDE_ENV, "");
        set_var_locked("LOCALAPPDATA", "C:\\Profile\\Local");

        assert_eq!(
            data_home().expect("home"),
            PathBuf::from("C:\\Profile\\Local").join("Okkhor")
        );
    }

    #[test]
    fn neither_set_is_an_error_without_path_text() {
        let _guard = EnvGuard::take();
        remove_var_locked(DATA_HOME_OVERRIDE_ENV);
        remove_var_locked("LOCALAPPDATA");

        let error = format!("{}", data_home().expect_err("must fail"));
        assert!(!error.is_empty());
        // No path (and no user name) may appear in the message.
        assert!(!error.contains("Users"));
        assert!(!error.contains(":\\"));
    }
}
