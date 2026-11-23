//! Plaintext settings store (contract §2).
//!
//! The daemon is the single writer of `settings.json`. Unknown fields are
//! ignored on read and dropped on write; ranged fields are clamped; anything
//! else invalid fails with a typed error and never panics. Writes are atomic
//! (sibling temporary file + rename): a failed write leaves the previous
//! file intact.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use okkhor_ipc::error::ErrorCode;
use okkhor_ipc::settings::{Settings, SettingsError, SettingsPatch};
use thiserror::Error;

/// Settings load/update failures. Messages are static; they never include
/// file content, so hand-edited garbage cannot leak through errors or logs.
#[derive(Debug, Error)]
pub enum SettingsStoreError {
    /// Filesystem failure (OS message only, never file content).
    #[error("settings storage I/O failed")]
    Io(#[from] std::io::Error),
    /// The file is not valid JSON or misses required fields.
    #[error("settings file is corrupt")]
    Corrupt,
    /// `schema` is not 1.
    #[error("unsupported schema version")]
    UnsupportedSchema,
    /// `layout` is not a known layout code.
    #[error("unknown layout")]
    InvalidLayout,
    /// A hotkey failed to parse.
    #[error("invalid hotkey")]
    InvalidHotkey,
    /// Any other settings value failed validation.
    #[error("invalid settings value")]
    InvalidSettings,
    /// The data home could not be resolved.
    #[error("data home is not available")]
    NoDataHome,
}

impl From<SettingsError> for SettingsStoreError {
    fn from(error: SettingsError) -> Self {
        match error {
            SettingsError::UnsupportedSchema => Self::UnsupportedSchema,
            SettingsError::InvalidLayout => Self::InvalidLayout,
            SettingsError::InvalidHotkey => Self::InvalidHotkey,
            SettingsError::InvalidFontScale
            | SettingsError::InvalidFontFamily
            | SettingsError::InvalidValue => Self::InvalidSettings,
        }
    }
}

impl From<crate::paths::PathsError> for SettingsStoreError {
    fn from(_: crate::paths::PathsError) -> Self {
        Self::NoDataHome
    }
}

impl SettingsStoreError {
    /// Protocol code for the IPC layer (Task 3).
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::Io(_) | Self::Corrupt | Self::NoDataHome => ErrorCode::Internal,
            Self::UnsupportedSchema => ErrorCode::UnsupportedSchema,
            Self::InvalidLayout => ErrorCode::InvalidLayout,
            Self::InvalidHotkey => ErrorCode::InvalidHotkey,
            Self::InvalidSettings => ErrorCode::InvalidSettings,
        }
    }
}

/// Owned settings document plus the file it persists to.
#[derive(Debug)]
pub struct SettingsStore {
    path: PathBuf,
    settings: Settings,
}

impl SettingsStore {
    /// Load `path`, or start from defaults when the file is missing (the
    /// defaults are persisted immediately so the UI read-only fallback and
    /// the next daemon start see a file).
    ///
    /// A present file is parsed, clamped, and validated; ranged values that
    /// fall outside contract bounds are clamped and written back, while
    /// structurally invalid files fail with a typed error and are left
    /// untouched (fail closed).
    pub fn load(path: &Path) -> Result<Self, SettingsStoreError> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let store = Self {
                    path: path.to_owned(),
                    settings: Settings::default(),
                };
                store.save()?;
                return Ok(store);
            }
            Err(error) => return Err(error.into()),
        };
        // Unknown fields are ignored by `Settings` deserialization.
        let parsed: Settings = serde_json::from_slice(&bytes).map_err(|_| {
            // Drop the serde error: its position info is useless here, and the
            // message must never echo file content.
            SettingsStoreError::Corrupt
        })?;
        let clamped = parsed.clamped();
        clamped.validate()?;
        let store = Self {
            path: path.to_owned(),
            settings: clamped.clone(),
        };
        // Self-heal drift (e.g. an out-of-range hand edit) only when the
        // normalized value actually differs; unknown fields are otherwise
        // kept on disk until the next real write drops them.
        if clamped != parsed {
            store.save()?;
        }
        Ok(store)
    }

    /// Load from the default location (`<home>/settings.json`).
    pub fn load_default() -> Result<Self, SettingsStoreError> {
        Self::load(&crate::paths::settings_path()?)
    }

    /// File this store persists to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The current validated settings.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Merge `patch`, validate the merged value, persist atomically, and
    /// return the new settings. On any failure the previous file and the
    /// previous in-memory value are both untouched.
    pub fn update(&mut self, patch: &SettingsPatch) -> Result<Settings, SettingsStoreError> {
        let merged = self.settings.apply(patch)?;
        let bytes =
            serde_json::to_vec_pretty(&merged).map_err(|_| SettingsStoreError::InvalidSettings)?;
        write_atomic(&self.path, &bytes)?;
        self.settings = merged.clone();
        Ok(merged)
    }

    fn save(&self) -> Result<(), SettingsStoreError> {
        let bytes = serde_json::to_vec_pretty(&self.settings)
            .map_err(|_| SettingsStoreError::InvalidSettings)?;
        write_atomic(&self.path, &bytes)
    }
}

/// Atomically replace `path` with `bytes` via a sibling temporary file and
/// rename, creating parents as needed.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), SettingsStoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = sibling_temp_path(path);
    fs::write(&temp_path, bytes)?;

    match fs::rename(&temp_path, path) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::AlreadyExists | ErrorKind::PermissionDenied
            ) =>
        {
            // Windows can refuse to replace an existing file (AV locks). The
            // new bytes are preserved when the fallback also fails, and the
            // old file is untouched until a rename succeeds.
            fs::remove_file(path)?;
            fs::rename(&temp_path, path)?;
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temp_path);
            Err(error.into())
        }
    }
}

fn sibling_temp_path(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut name = path
        .file_name()
        .map(|value| value.to_os_string())
        .unwrap_or_else(|| "settings.json".into());
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use okkhor_ipc::settings::{
        Accent, FontBijoy, FontUnicode, InputMode, Language, OutputEncoding, Theme,
    };
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "okkhor-settings-{}-{}-{}-{}",
            name,
            std::process::id(),
            unique,
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn missing_file_loads_defaults_and_persists() {
        let dir = temp_dir("defaults");
        let path = dir.join("settings.json");
        let store = SettingsStore::load(&path).expect("loads defaults");
        assert_eq!(store.settings(), &Settings::default());
        assert!(path.exists(), "defaults are persisted");

        // A second load reads the persisted file back identically.
        let again = SettingsStore::load(&path).expect("reloads");
        assert_eq!(again.settings(), &Settings::default());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn wrong_schema_is_rejected_without_touching_the_file() {
        let dir = temp_dir("schema");
        let path = dir.join("settings.json");
        let mut doc = serde_json::to_value(Settings::default()).expect("serializes");
        doc["schema"] = serde_json::json!(2);
        std::fs::write(&path, serde_json::to_vec(&doc).expect("doc")).expect("plants");
        let before = std::fs::read(&path).expect("reads planted");

        let error = SettingsStore::load(&path).expect_err("schema 2 must fail");
        assert!(matches!(error, SettingsStoreError::UnsupportedSchema));
        assert_eq!(error.code(), ErrorCode::UnsupportedSchema);
        assert_eq!(std::fs::read(&path).expect("untouched"), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn out_of_range_values_clamp_and_heal() {
        let dir = temp_dir("clamp");
        let path = dir.join("settings.json");
        let mut doc = serde_json::to_value(Settings::default()).expect("serializes");
        doc["font_scale"] = serde_json::json!(99.0);
        doc["popup_max_items"] = serde_json::json!(0);
        doc["word_manager_page_size"] = serde_json::json!(500);
        doc["quickbar_opacity"] = serde_json::json!(0.0);
        std::fs::write(&path, serde_json::to_vec(&doc).expect("doc")).expect("plants");

        let store = SettingsStore::load(&path).expect("clamps on load");
        assert_eq!(store.settings().font_scale, 1.6);
        assert_eq!(store.settings().popup_max_items, 1);
        assert_eq!(store.settings().word_manager_page_size, 200);
        assert_eq!(store.settings().quickbar_opacity, 0.5);

        // The healed file loads again without further changes.
        let healed = std::fs::read(&path).expect("reads healed");
        let again = SettingsStore::load(&path).expect("reloads healed");
        assert_eq!(again.settings(), store.settings());
        assert_eq!(std::fs::read(&path).expect("stable"), healed);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_values_fail_closed() {
        for (field, value) in [
            ("layout", serde_json::json!("not-a-layout")),
            ("toggle_hotkey", serde_json::json!("bogus")),
            ("pause_hotkey", serde_json::json!("Ctrl")),
        ] {
            let dir = temp_dir("invalid");
            let path = dir.join("settings.json");
            let mut doc = serde_json::to_value(Settings::default()).expect("serializes");
            doc[field] = value;
            std::fs::write(&path, serde_json::to_vec(&doc).expect("doc")).expect("plants");
            let before = std::fs::read(&path).expect("reads planted");

            let error = SettingsStore::load(&path).expect_err("must reject");
            assert!(
                matches!(
                    error,
                    SettingsStoreError::InvalidLayout
                        | SettingsStoreError::InvalidHotkey
                        | SettingsStoreError::InvalidSettings
                ),
                "{field}: {error:?}"
            );
            assert_eq!(std::fs::read(&path).expect("untouched"), before);
            std::fs::remove_dir_all(&dir).ok();
        }
    }

    #[test]
    fn garbage_is_corrupt_not_a_panic() {
        let dir = temp_dir("garbage");
        let path = dir.join("settings.json");
        for garbage in [
            b"not json at all".to_vec(),
            b"{\"schema\":1,".to_vec(),
            b"[1,2,3]".to_vec(),
            b"null".to_vec(),
            vec![0xFF, 0xFE, 0x00],
        ] {
            std::fs::write(&path, &garbage).expect("plants garbage");
            let error = SettingsStore::load(&path).expect_err("garbage must fail");
            assert!(
                matches!(
                    error,
                    SettingsStoreError::Corrupt | SettingsStoreError::InvalidSettings
                ),
                "{garbage:?}: {error:?}"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_fields_ignored_on_read_and_dropped_on_write() {
        let dir = temp_dir("unknown");
        let path = dir.join("settings.json");
        let mut doc = serde_json::to_value(Settings::default()).expect("serializes");
        doc["future_field"] = serde_json::json!("dropped");
        std::fs::write(&path, serde_json::to_vec(&doc).expect("doc")).expect("plants");

        let mut store = SettingsStore::load(&path).expect("ignores unknown");
        // No rewrite was needed (values already normalized), so the unknown
        // field is still on disk — but the next real write drops it.
        store
            .update(&SettingsPatch {
                theme: Some(Theme::Dark),
                ..SettingsPatch::default()
            })
            .expect("updates");
        let raw = std::fs::read_to_string(&path).expect("reads file");
        assert!(!raw.contains("future_field"), "dropped on write");
        assert!(raw.contains("\"dark\""));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn update_merges_validates_and_persists() {
        let dir = temp_dir("update");
        let path = dir.join("settings.json");
        let mut store = SettingsStore::load(&path).expect("loads");

        let merged = store
            .update(&SettingsPatch {
                language: Some(Language::En),
                layout: Some("bijoy".to_owned()),
                font_scale: Some(9.0),
                popup_max_items: Some(0),
                input_mode: Some(InputMode::English),
                output_encoding: Some(OutputEncoding::Bijoy),
                font_unicode: Some(FontUnicode::NotoSansBengali),
                font_bijoy: Some(FontBijoy::KalpurushAnsi),
                accent: Some(Accent::Rose),
                ..SettingsPatch::default()
            })
            .expect("merges");
        assert_eq!(merged.language, Language::En);
        assert_eq!(merged.layout, "bijoy");
        assert_eq!(merged.font_scale, 1.6);
        assert_eq!(merged.popup_max_items, 1);

        // Persisted: a fresh load sees the merged document.
        let again = SettingsStore::load(&path).expect("reloads");
        assert_eq!(again.settings(), &merged);

        // Rejected patches change neither the file nor memory.
        let before = std::fs::read(&path).expect("reads file");
        let error = store
            .update(&SettingsPatch {
                layout: Some("nope".to_owned()),
                ..SettingsPatch::default()
            })
            .expect_err("bad layout");
        assert!(matches!(error, SettingsStoreError::InvalidLayout));
        assert_eq!(store.settings(), &merged);
        assert_eq!(std::fs::read(&path).expect("untouched"), before);

        assert!(matches!(
            store
                .update(&SettingsPatch {
                    font_scale: Some(f32::NAN),
                    ..SettingsPatch::default()
                })
                .expect_err("NaN"),
            SettingsStoreError::InvalidSettings
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn error_codes_map_to_protocol() {
        assert_eq!(SettingsStoreError::Corrupt.code(), ErrorCode::Internal);
        assert_eq!(
            SettingsStoreError::InvalidLayout.code(),
            ErrorCode::InvalidLayout
        );
        assert_eq!(
            SettingsStoreError::InvalidHotkey.code(),
            ErrorCode::InvalidHotkey
        );
        assert_eq!(
            SettingsStoreError::InvalidSettings.code(),
            ErrorCode::InvalidSettings
        );
    }
}
