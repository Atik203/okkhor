//! Redacted, structured daemon logging.
//!
//! The logger accepts only fixed event types and allowlisted fields. It has no
//! API for free-form messages, keystrokes, clipboard text, window titles, or
//! user words. Records are JSON lines under `%LOCALAPPDATA%\Okkhor\logs` and are
//! mirrored to stderr.
//!
//! Keep this module off the typing path. The daemon logs startup, the selected
//! command, and errors only.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

/// Fields a log record is allowed to contain. Anything else is dropped before
/// serialization, so a future caller cannot accidentally add user text.
const ALLOWED_FIELDS: &[&str] = &[
    "ts_ms", "level", "event", "version", "pid", "command", "scope", "kind", "code",
];

const LOG_SUBDIR: &str = "Okkhor";
const LOG_DIR: &str = "logs";
const LOG_FILE: &str = "okkhor.log";

/// Commands that may appear in the `command` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    TrayDemo,
    Daemon,
    AutostartEnable,
    AutostartDisable,
    AutostartStatus,
    FontsStatus,
    FontsInstall,
    FontsUninstall,
}

impl Command {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TrayDemo => "tray-demo",
            Self::Daemon => "daemon",
            Self::AutostartEnable => "autostart-enable",
            Self::AutostartDisable => "autostart-disable",
            Self::AutostartStatus => "autostart-status",
            Self::FontsStatus => "fonts-status",
            Self::FontsInstall => "fonts-install",
            Self::FontsUninstall => "fonts-uninstall",
        }
    }
}

/// Subsystem that produced an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Daemon,
    SingleInstance,
    Hook,
    Hotkey,
    Tray,
    Registry,
    Ipc,
    // G1: constructed by §6.3 daemon wiring (follow-up); allow until then.
    #[allow(dead_code)]
    Lang,
    #[allow(dead_code)]
    Transliterate,
    #[allow(dead_code)]
    Compat,
    // 0.4 §11.3: constructed by engine daemon wiring (follow-up); allow until then.
    #[allow(dead_code)]
    Engine,
}

impl Scope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Daemon => "daemon",
            Self::SingleInstance => "single_instance",
            Self::Hook => "hook",
            Self::Hotkey => "hotkey",
            Self::Tray => "tray",
            Self::Registry => "registry",
            Self::Ipc => "ipc",
            Self::Lang => "lang",
            Self::Transliterate => "transliterate",
            Self::Compat => "compat",
            Self::Engine => "engine",
        }
    }

    /// Parse a stable scope name back into a `Scope`.
    ///
    /// Returns `None` for anything outside the allowlisted set, so a caller
    /// can never smuggle free-form text into the `scope` field (see 0.4
    /// §11.3: the engine construction-failure call site logs
    /// `scope=engine` with `kind()`/`code()`).
    // 0.4 §11.3: used by the engine daemon wiring (follow-up); allow until then.
    #[allow(dead_code)]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "daemon" => Some(Self::Daemon),
            "single_instance" => Some(Self::SingleInstance),
            "hook" => Some(Self::Hook),
            "hotkey" => Some(Self::Hotkey),
            "tray" => Some(Self::Tray),
            "registry" => Some(Self::Registry),
            "ipc" => Some(Self::Ipc),
            "lang" => Some(Self::Lang),
            "transliterate" => Some(Self::Transliterate),
            "compat" => Some(Self::Compat),
            "engine" => Some(Self::Engine),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Level {
    Info,
    Error,
}

impl Level {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Error => "error",
        }
    }
}

/// Process-wide logger. Initialization degrades to stderr when the log file
/// cannot be opened, so logging never prevents the daemon from starting.
pub struct Logger {
    file: Mutex<Option<File>>,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Initialize logging once and return the process-wide logger.
pub fn init() -> &'static Logger {
    LOGGER.get_or_init(|| match open_log_file() {
        Ok(file) => Logger {
            file: Mutex::new(Some(file)),
        },
        Err(_) => {
            eprintln!("okkhor: could not open the log file; logging to stderr only");
            Logger {
                file: Mutex::new(None),
            }
        }
    })
}

impl Logger {
    /// Record daemon startup. The executable path is deliberately not logged
    /// because it can contain the user's name.
    pub fn startup(&self) {
        let mut record = Record::new(Level::Info, "startup");
        record.field("version", json!(env!("CARGO_PKG_VERSION")));
        record.field("pid", json!(std::process::id()));
        self.emit(&record);
    }

    /// Record the command that selected the current mode.
    pub fn command(&self, command: Command) {
        let mut record = Record::new(Level::Info, "command");
        record.field("command", json!(command.as_str()));
        self.emit(&record);
    }

    /// Informational daemon event with an allowlisted scope and event name.
    pub fn info(&self, scope: Scope, event: &'static str) {
        let mut record = Record::new(Level::Info, event);
        record.field("scope", json!(scope.as_str()));
        self.emit(&record);
    }

    /// Record an error without its message. `kind` is a static identifier from
    /// the call site; error text is never serialized because it may embed paths
    /// or target text.
    pub fn error(&self, scope: Scope, kind: &'static str, code: Option<i32>) {
        let mut record = Record::new(Level::Error, "error");
        record.field("scope", json!(scope.as_str()));
        record.field("kind", json!(kind));
        if let Some(code) = code {
            record.field("code", json!(code));
        }
        self.emit(&record);
    }

    fn emit(&self, record: &Record) {
        let line = record.to_json();

        if let Ok(mut slot) = self.file.lock() {
            if let Some(file) = slot.as_mut() {
                // Logging failures are ignored on purpose: a full disk must not
                // stop typing or daemon shutdown.
                let _ = writeln!(file, "{line}");
                let _ = file.flush();
            }
        }
        eprintln!("{line}");
    }
}

/// A single structured event. Field names are filtered through the allowlist.
struct Record {
    fields: Map<String, Value>,
}

impl Record {
    fn new(level: Level, event: &'static str) -> Self {
        let mut fields = Map::new();
        fields.insert("ts_ms".to_owned(), json!(epoch_ms()));
        fields.insert("level".to_owned(), json!(level.as_str()));
        fields.insert("event".to_owned(), json!(event));
        Self { fields }
    }

    fn field(&mut self, name: &'static str, value: Value) {
        if is_allowed_field(name) {
            self.fields.insert(name.to_owned(), value);
        }
    }

    fn to_json(&self) -> String {
        Value::Object(self.fields.clone()).to_string()
    }
}

fn is_allowed_field(name: &str) -> bool {
    ALLOWED_FIELDS.contains(&name)
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn log_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(base)
            .join(LOG_SUBDIR)
            .join(LOG_DIR)
            .join(LOG_FILE),
    )
}

fn open_log_file() -> io::Result<File> {
    let path = log_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "LOCALAPPDATA is not set in this environment",
        )
    })?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::{ALLOWED_FIELDS, Command, Level, Record, Scope, is_allowed_field};
    use serde_json::{Value, json};

    #[test]
    fn sensitive_field_names_are_not_allowed() {
        for name in [
            "text",
            "word",
            "words",
            "sentence",
            "keystrokes",
            "clipboard",
            "clipboard_text",
            "window_title",
            "title",
            "user_words",
        ] {
            assert!(!is_allowed_field(name), "{name} must never be logged");
        }
    }

    #[test]
    fn sensitive_fields_are_dropped_before_serialization() {
        let mut record = Record::new(Level::Info, "startup");
        record.field("text", json!("SECRET-TYPED-TEXT"));
        record.field("window_title", json!("SECRET-WINDOW-TITLE"));
        record.field("clipboard", json!("SECRET-CLIPBOARD"));
        record.field("words", json!(["SECRET-WORD"]));

        let line = record.to_json();
        assert!(!line.contains("SECRET"), "redaction failed: {line}");
        assert!(line.contains("\"event\":\"startup\""));
    }

    #[test]
    fn emitted_records_only_contain_allowlisted_keys() {
        let mut record = Record::new(Level::Error, "error");
        record.field("scope", json!(Scope::Hook.as_str()));
        record.field("kind", json!("hook_start"));
        record.field("code", json!(5));

        let parsed: Value = serde_json::from_str(&record.to_json()).expect("valid JSON");
        let object = parsed.as_object().expect("object");
        for key in object.keys() {
            assert!(is_allowed_field(key), "unexpected field: {key}");
        }
        assert_eq!(object.get("level"), Some(&json!("error")));
        assert_eq!(object.get("scope"), Some(&json!("hook")));
        assert_eq!(object.get("code"), Some(&json!(5)));
    }

    #[test]
    fn field_allowlist_itself_is_auditable() {
        for required in [
            "event", "level", "ts_ms", "command", "scope", "kind", "code",
        ] {
            assert!(ALLOWED_FIELDS.contains(&required), "missing {required}");
        }
    }

    #[test]
    fn command_and_scope_names_are_stable() {
        assert_eq!(Command::Daemon.as_str(), "daemon");
        assert_eq!(Command::AutostartEnable.as_str(), "autostart-enable");
        assert_eq!(Scope::Tray.as_str(), "tray");
        assert_eq!(Scope::Registry.as_str(), "registry");
        assert_eq!(Scope::Ipc.as_str(), "ipc");
        assert_eq!(Scope::Lang.as_str(), "lang");
        assert_eq!(Scope::Transliterate.as_str(), "transliterate");
        assert_eq!(Scope::Compat.as_str(), "compat");
        assert_eq!(Scope::Engine.as_str(), "engine");
    }

    #[test]
    fn scope_names_round_trip_through_parse() {
        for scope in [
            Scope::Daemon,
            Scope::SingleInstance,
            Scope::Hook,
            Scope::Hotkey,
            Scope::Tray,
            Scope::Registry,
            Scope::Ipc,
            Scope::Lang,
            Scope::Transliterate,
            Scope::Compat,
            Scope::Engine,
        ] {
            assert_eq!(Scope::parse(scope.as_str()), Some(scope));
        }
        // Free-form text never parses: the scope field stays allowlisted.
        for rejected in ["", "ENGINE", "engine ", "user text", "hook;drop"] {
            assert_eq!(Scope::parse(rejected), None, "{rejected} must not parse");
        }
    }

    #[test]
    fn engine_errors_map_to_allowed_kind_and_code() {
        // Mirrors the 0.4 §6 `EngineError::kind()` strings and the always-`None`
        // `code()` through the §11.3 call shape
        // `logger.error(Scope::Engine, kind, code)`, without depending on the
        // engine crate: only allowlisted fields may serialize, and no code
        // key appears when `code` is `None`.
        for kind in [
            "engine_invalid_budget",
            "engine_missing_component",
            "engine_unsupported_schema",
            "engine_invalid_golden",
        ] {
            let mut record = Record::new(Level::Error, "error");
            record.field("scope", json!(Scope::Engine.as_str()));
            record.field("kind", json!(kind));

            let parsed: Value = serde_json::from_str(&record.to_json()).expect("valid JSON");
            let object = parsed.as_object().expect("object");
            for key in object.keys() {
                assert!(is_allowed_field(key), "unexpected field: {key}");
            }
            assert_eq!(object.get("scope"), Some(&json!("engine")));
            assert_eq!(object.get("kind"), Some(&json!(kind)));
            assert!(!object.contains_key("code"), "engine code is always None");
        }
    }
}
