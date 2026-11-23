#![forbid(unsafe_code)]
//! Persistence for Okkhor learning snapshots.
//!
//! The first implementation is a small JSON repository so the learning core
//! can be exercised end-to-end. The production adapter will use an embedded
//! database (`redb` or equivalent) behind the same boundary.
//!
//! This crate must never deserialize Python pickle. Legacy settings require a
//! separate one-time exporter.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use okkhor_learn::LearningSnapshot;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("storage I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("storage JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported storage format {found}; expected {expected}")]
    UnsupportedFormat { found: u32, expected: u32 },
    /// The file does not exist yet. Callers that treat a missing file as an
    /// empty store handle this; everything else propagates it.
    #[error("stored data was not found")]
    NotFound,
    /// Magic, version, ciphertext, or JSON failed validation. The message is
    /// static: it never echoes file bytes, which may be user text.
    #[error("stored data is corrupt")]
    Corrupt,
    /// The protector cannot run on this platform (e.g. DPAPI off Windows).
    #[error("encrypted storage is not supported on this platform")]
    Unsupported,
    /// Plaintext or file exceeds the repository size cap.
    #[error("stored data exceeds the size limit")]
    TooLarge,
    /// Protection failed. The OS error is retained as the source for a
    /// numeric code only; it carries no protected bytes.
    #[error("protection failed")]
    Protect(#[source] std::io::Error),
}

pub mod encrypted;

pub use encrypted::{
    ENVELOPE_MAGIC, ENVELOPE_VERSION, EncryptedJsonRepository, FakeProtector,
    MAX_ENCRYPTED_JSON_BYTES, Protector,
};

/// Atomically replace `path` with `bytes`: write to a sibling temporary file
/// (same directory, so the rename stays on one filesystem) and rename over
/// the target. The temporary file holds exactly `bytes` — callers writing
/// secrets must pass ciphertext, never plaintext.
pub(crate) fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
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
            // Windows can refuse to replace an existing file (AV locks, ACL
            // timing). Remove and retry once; the new bytes are preserved if
            // the fallback also fails, and the old file is untouched until a
            // rename succeeds.
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
        .unwrap_or_else(|| "okkhor-store".into());
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    path.with_file_name(name)
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredSnapshot {
    format_version: u32,
    snapshot: LearningSnapshot,
}

/// A single-file JSON snapshot repository.
///
/// Writes go to a sibling temporary file and are renamed into place. This is
/// adequate for the scaffold; the production adapter should use a
/// transaction-capable embedded store.
#[derive(Debug, Clone)]
pub struct JsonFileRepository {
    path: PathBuf,
}

impl JsonFileRepository {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load a snapshot, or `None` when the file does not exist yet.
    pub fn load(&self) -> Result<Option<LearningSnapshot>, StorageError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };

        let stored: StoredSnapshot = serde_json::from_slice(&bytes)?;
        if stored.format_version != FORMAT_VERSION {
            return Err(StorageError::UnsupportedFormat {
                found: stored.format_version,
                expected: FORMAT_VERSION,
            });
        }
        Ok(Some(stored.snapshot))
    }

    /// Persist a snapshot atomically enough for the scaffold.
    pub fn save(&self, snapshot: &LearningSnapshot) -> Result<(), StorageError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let stored = StoredSnapshot {
            format_version: FORMAT_VERSION,
            snapshot: snapshot.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&stored)?;
        let temp_path = self.temp_path();
        fs::write(&temp_path, bytes)?;

        match fs::rename(&temp_path, &self.path) {
            Ok(()) => Ok(()),
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::AlreadyExists | ErrorKind::PermissionDenied
                ) =>
            {
                // Windows can refuse to replace an existing file. Preserve the
                // new bytes if the fallback also fails.
                fs::remove_file(&self.path)?;
                fs::rename(&temp_path, &self.path)?;
                Ok(())
            }
            Err(error) => {
                let _ = fs::remove_file(&temp_path);
                Err(error.into())
            }
        }
    }

    /// Remove the snapshot file if it exists.
    pub fn clear(&self) -> Result<(), StorageError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn temp_path(&self) -> PathBuf {
        let mut name = self
            .path
            .file_name()
            .map(|value| value.to_os_string())
            .unwrap_or_else(|| "learning.json".into());
        name.push(format!(".{}.tmp", std::process::id()));
        self.path.with_file_name(name)
    }
}

#[cfg(test)]
mod tests {
    use super::{FORMAT_VERSION, JsonFileRepository, StorageError, StoredSnapshot};
    use okkhor_core::Language;
    use okkhor_learn::{CommitSource, LearningEvent, Smriti};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir()
            .join(format!("okkhor-storage-{}-{unique}", std::process::id()))
            .join("learning.json")
    }

    fn sample_snapshot() -> okkhor_learn::LearningSnapshot {
        let smriti = Smriti::new();
        smriti.observe(LearningEvent::WordCommitted {
            word: "ভালো".to_owned(),
            language: Language::Bangla,
            previous_word: None,
            timestamp_ms: 1_000,
            source: CommitSource::Typed,
        });
        smriti.snapshot()
    }

    #[test]
    fn saves_loads_and_clears() {
        let path = temp_path();
        let repository = JsonFileRepository::new(&path);
        let snapshot = sample_snapshot();

        assert_eq!(repository.load().expect("empty load"), None);
        repository.save(&snapshot).expect("save");
        let loaded = repository.load().expect("load").expect("snapshot");
        assert_eq!(loaded.word_count(), snapshot.word_count());
        assert!(repository.path().exists());

        repository.clear().expect("clear");
        assert!(!repository.path().exists());
        assert_eq!(repository.load().expect("load after clear"), None);

        let _ = fs::remove_dir_all(path.parent().expect("temp parent"));
    }

    #[test]
    fn rejects_unknown_format() {
        let path = temp_path();
        fs::create_dir_all(path.parent().expect("temp parent")).expect("mkdir");
        let fixture = StoredSnapshot {
            format_version: FORMAT_VERSION + 1,
            snapshot: sample_snapshot(),
        };
        fs::write(
            &path,
            serde_json::to_vec(&fixture).expect("serialize fixture"),
        )
        .expect("write fixture");

        let repository = JsonFileRepository::new(&path);
        let error = repository.load().expect_err("format must be rejected");
        assert!(matches!(
            error,
            StorageError::UnsupportedFormat {
                expected: FORMAT_VERSION,
                ..
            }
        ));

        let _ = fs::remove_dir_all(path.parent().expect("temp parent"));
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let path = temp_path();
        let repository = JsonFileRepository::new(&path);
        assert_eq!(repository.load().expect("missing load"), None);
        repository.clear().expect("missing clear");
    }
}
