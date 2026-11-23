//! Encrypted-at-rest JSON persistence.
//!
//! [`EncryptedJsonRepository`] stores any serializable document through a
//! [`Protector`] (DPAPI in production, a deterministic fake in tests). The
//! on-disk layout is:
//!
//! ```text
//! b"OKE1" | u32 LE envelope version (= 1) | protector ciphertext
//! ```
//!
//! Writes are atomic (sibling temporary file + rename) and the temporary file
//! holds ciphertext only — plaintext is never written to disk. Loads fail
//! closed: a missing file reports [`StorageError::NotFound`], anything else
//! wrong (magic, version, ciphertext, JSON) reports [`StorageError::Corrupt`]
//! without loading partial data or touching the original file.

use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::StorageError;

/// Envelope magic prefix (`OKE1` = Okkhor Encrypted v1).
pub const ENVELOPE_MAGIC: &[u8; 4] = b"OKE1";
/// Envelope format version, stored as little-endian `u32` after the magic.
pub const ENVELOPE_VERSION: u32 = 1;
/// Maximum plaintext JSON bytes per document (8 MiB).
pub const MAX_ENCRYPTED_JSON_BYTES: usize = 8 * 1024 * 1024;
/// Magic (4) + version (4) prefix length.
const ENVELOPE_HEADER_LEN: usize = 8;
/// Headroom for protector expansion (DPAPI adds only a small header) before
/// a file is rejected without attempting decryption.
const CIPHERTEXT_HEADROOM: usize = 64 * 1024;

/// Encrypts/decrypts repository bytes. Production uses DPAPI (current-user
/// scope); tests use [`FakeProtector`]. Implementations must fail closed on
/// tampering and never include protected bytes in errors.
pub trait Protector: Send + Sync {
    /// Encrypt `plaintext`.
    fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, StorageError>;
    /// Decrypt `ciphertext`, or fail (mapped to [`StorageError::Corrupt`]
    /// unless the platform itself is unsupported).
    fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, StorageError>;
}

/// Deterministic test-only protector (repeating-key XOR).
///
/// This provides **no confidentiality or integrity**: it exists so unit tests
/// exercise envelope handling, atomicity, and caps without DPAPI. The
/// repository layer still rejects header tampering and malformed JSON, so
/// tamper tests stay meaningful; ciphertext-flip tests target the first
/// plaintext byte (always `{` for JSON objects), where any single-bit flip
/// breaks parsing. Never use in production.
#[derive(Debug, Clone, Copy, Default)]
pub struct FakeProtector;

impl FakeProtector {
    /// A fixed, non-secret mask. It is not a key, only a deterministic mask.
    const MASK: &'static [u8] = b"okkhor-test-only-fake-mask";

    fn mask(bytes: &[u8]) -> Vec<u8> {
        bytes
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ Self::MASK[index % Self::MASK.len()])
            .collect()
    }
}

impl Protector for FakeProtector {
    fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, StorageError> {
        Ok(Self::mask(plaintext))
    }

    fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, StorageError> {
        Ok(Self::mask(ciphertext))
    }
}

/// An encrypted JSON document at `path`, sealed through `P`.
///
/// The repository never logs or returns document bytes: errors are limited to
/// [`StorageError`] variants with static messages.
pub struct EncryptedJsonRepository<T, P> {
    path: PathBuf,
    protector: P,
    marker: PhantomData<T>,
}

// Manual `Debug`: the repository holds no document bytes in memory, but the
// bound `T: Serialize + DeserializeOwned` should not require `T: Debug`, and
// the path alone is enough to identify the store.
impl<T, P> std::fmt::Debug for EncryptedJsonRepository<T, P> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EncryptedJsonRepository")
            .field("path", &self.path)
            .finish()
    }
}

impl<T, P> EncryptedJsonRepository<T, P> {
    /// Open the repository at `path` with `protector`. Nothing is read until
    /// [`EncryptedJsonRepository::load`] or written until
    /// [`EncryptedJsonRepository::save`].
    pub fn new(path: impl Into<PathBuf>, protector: P) -> Self {
        Self {
            path: path.into(),
            protector,
            marker: PhantomData,
        }
    }

    /// File this repository reads and writes.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl<T, P> EncryptedJsonRepository<T, P>
where
    T: Serialize + DeserializeOwned,
    P: Protector,
{
    /// Load and decrypt the document.
    ///
    /// Returns [`StorageError::NotFound`] when the file is missing and
    /// [`StorageError::Corrupt`] for any tampering, truncation, version
    /// mismatch, or JSON failure. The original file is never modified or
    /// partially loaded.
    pub fn load(&self) -> Result<T, StorageError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(StorageError::NotFound);
            }
            Err(error) => return Err(error.into()),
        };
        // Reject absurd files before paying for decryption.
        if bytes.len() > MAX_ENCRYPTED_JSON_BYTES + CIPHERTEXT_HEADROOM {
            return Err(StorageError::TooLarge);
        }
        let ciphertext = split_envelope(&bytes)?;
        let plaintext = self.protector.unprotect(ciphertext).map_err(|error| {
            // A protector that cannot run here stays `Unsupported`; every
            // other failure (tamper, wrong entropy) is corruption.
            match error {
                StorageError::Unsupported => StorageError::Unsupported,
                _ => StorageError::Corrupt,
            }
        })?;
        if plaintext.len() > MAX_ENCRYPTED_JSON_BYTES {
            return Err(StorageError::TooLarge);
        }
        serde_json::from_slice(&plaintext).map_err(|_| StorageError::Corrupt)
    }

    /// Encrypt and atomically persist `value`.
    ///
    /// The plaintext is serialized, size-checked, encrypted, then written to
    /// a sibling temporary file (ciphertext only) and renamed over the
    /// target. A failed save leaves any previous file intact.
    pub fn save(&self, value: &T) -> Result<(), StorageError> {
        let plaintext = serde_json::to_vec(value).map_err(|_| StorageError::Corrupt)?;
        if plaintext.len() > MAX_ENCRYPTED_JSON_BYTES {
            return Err(StorageError::TooLarge);
        }
        let ciphertext = self.protector.protect(&plaintext)?;
        let mut envelope = Vec::with_capacity(ENVELOPE_HEADER_LEN + ciphertext.len());
        envelope.extend_from_slice(ENVELOPE_MAGIC);
        envelope.extend_from_slice(&ENVELOPE_VERSION.to_le_bytes());
        envelope.extend_from_slice(&ciphertext);
        crate::atomic_replace(&self.path, &envelope)
    }
}

/// Split `OKE1 | version | ciphertext`, rejecting anything else as corrupt.
fn split_envelope(bytes: &[u8]) -> Result<&[u8], StorageError> {
    if bytes.len() < ENVELOPE_HEADER_LEN {
        return Err(StorageError::Corrupt);
    }
    if &bytes[..4] != ENVELOPE_MAGIC {
        return Err(StorageError::Corrupt);
    }
    let mut version = [0u8; 4];
    version.copy_from_slice(&bytes[4..8]);
    if u32::from_le_bytes(version) != ENVELOPE_VERSION {
        return Err(StorageError::Corrupt);
    }
    Ok(&bytes[ENVELOPE_HEADER_LEN..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StorageError;
    use serde::{Deserialize, Serialize};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Doc {
        schema: u32,
        name: String,
        count: u64,
    }

    impl Doc {
        fn sample() -> Self {
            Self {
                schema: 1,
                name: "sample".to_owned(),
                count: 7,
            }
        }
    }

    /// Protector that always fails, to prove failed saves leave the old file
    /// intact (no overwrite before a successful write).
    struct FailingProtector;

    impl Protector for FailingProtector {
        fn protect(&self, _plaintext: &[u8]) -> Result<Vec<u8>, StorageError> {
            Err(StorageError::Protect(std::io::Error::other(
                "injected failure",
            )))
        }

        fn unprotect(&self, _ciphertext: &[u8]) -> Result<Vec<u8>, StorageError> {
            Err(StorageError::Protect(std::io::Error::other(
                "injected failure",
            )))
        }
    }

    /// Protector that refuses to run, to prove `Unsupported` propagates
    /// distinctly from `Corrupt`.
    struct UnsupportedProtector;

    impl Protector for UnsupportedProtector {
        fn protect(&self, _plaintext: &[u8]) -> Result<Vec<u8>, StorageError> {
            Err(StorageError::Unsupported)
        }

        fn unprotect(&self, _ciphertext: &[u8]) -> Result<Vec<u8>, StorageError> {
            Err(StorageError::Unsupported)
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "okkhor-encrypted-{}-{}-{}-{}",
            name,
            std::process::id(),
            unique,
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn temp_leftovers(dir: &Path) -> Vec<PathBuf> {
        match std::fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| {
                    path.as_os_str()
                        .to_str()
                        .is_some_and(|name| name.contains(".tmp"))
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    #[test]
    fn round_trip() {
        let dir = temp_dir("round-trip");
        let repository = EncryptedJsonRepository::new(dir.join("doc.bin"), FakeProtector);
        repository.save(&Doc::sample()).expect("saves");

        // The file is an opaque envelope, not plaintext JSON.
        let raw = std::fs::read(repository.path()).expect("reads raw");
        assert!(raw.starts_with(ENVELOPE_MAGIC));
        assert!(!raw.windows(6).any(|window| window == b"sample"));

        let back: Doc = repository.load().expect("loads");
        assert_eq!(back, Doc::sample());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_is_not_found() {
        let dir = temp_dir("missing");
        let repository: EncryptedJsonRepository<Doc, FakeProtector> =
            EncryptedJsonRepository::new(dir.join("absent.bin"), FakeProtector);
        assert!(matches!(repository.load(), Err(StorageError::NotFound)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tampered_magic_is_corrupt() {
        let dir = temp_dir("magic");
        let path = dir.join("doc.bin");
        let repository = EncryptedJsonRepository::new(&path, FakeProtector);
        repository.save(&Doc::sample()).expect("saves");

        let mut raw = std::fs::read(&path).expect("reads raw");
        raw[0] ^= 0xFF;
        std::fs::write(&path, &raw).expect("writes tampered");

        let repository: EncryptedJsonRepository<Doc, FakeProtector> =
            EncryptedJsonRepository::new(&path, FakeProtector);
        assert!(matches!(repository.load(), Err(StorageError::Corrupt)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tampered_ciphertext_is_corrupt() {
        let dir = temp_dir("cipher");
        let path = dir.join("doc.bin");
        let repository = EncryptedJsonRepository::new(&path, FakeProtector);
        repository.save(&Doc::sample()).expect("saves");

        // Flip the first ciphertext byte: the first plaintext byte is `{`
        // (compact JSON object), and every single-bit flip of `{` breaks
        // parsing or the target-type check, so this is deterministic.
        let mut raw = std::fs::read(&path).expect("reads raw");
        raw[ENVELOPE_HEADER_LEN] ^= 0x20;
        std::fs::write(&path, &raw).expect("writes tampered");

        let repository: EncryptedJsonRepository<Doc, FakeProtector> =
            EncryptedJsonRepository::new(&path, FakeProtector);
        assert!(matches!(repository.load(), Err(StorageError::Corrupt)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn truncated_file_is_corrupt() {
        let dir = temp_dir("truncated");
        let path = dir.join("doc.bin");
        let repository = EncryptedJsonRepository::new(&path, FakeProtector);
        repository.save(&Doc::sample()).expect("saves");

        let raw = std::fs::read(&path).expect("reads raw");
        std::fs::write(&path, &raw[..raw.len() / 2]).expect("truncates");

        let repository: EncryptedJsonRepository<Doc, FakeProtector> =
            EncryptedJsonRepository::new(&path, FakeProtector);
        assert!(matches!(repository.load(), Err(StorageError::Corrupt)));
        // A bare header with no payload is corrupt, not a document.
        std::fs::write(&path, b"OKE1-short").expect("writes short");
        assert!(matches!(repository.load(), Err(StorageError::Corrupt)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_envelope_version_is_corrupt() {
        let dir = temp_dir("version");
        let path = dir.join("doc.bin");
        let repository = EncryptedJsonRepository::new(&path, FakeProtector);
        repository.save(&Doc::sample()).expect("saves");

        let mut raw = std::fs::read(&path).expect("reads raw");
        raw[4..8].copy_from_slice(&99u32.to_le_bytes());
        std::fs::write(&path, &raw).expect("writes future version");

        let repository: EncryptedJsonRepository<Doc, FakeProtector> =
            EncryptedJsonRepository::new(&path, FakeProtector);
        // Future envelopes fail closed: never partially loaded.
        assert!(matches!(repository.load(), Err(StorageError::Corrupt)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn non_json_payload_is_corrupt() {
        let dir = temp_dir("garbage");
        let path = dir.join("not-json.bin");
        // Valid header, but the "ciphertext" unmasks to non-JSON bytes.
        let mut envelope = Vec::new();
        envelope.extend_from_slice(ENVELOPE_MAGIC);
        envelope.extend_from_slice(&ENVELOPE_VERSION.to_le_bytes());
        envelope.extend_from_slice(&[0x01, 0x02, 0x03, 0x04]);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, &envelope).expect("writes garbage");

        let repository: EncryptedJsonRepository<Doc, FakeProtector> =
            EncryptedJsonRepository::new(&path, FakeProtector);
        assert!(matches!(repository.load(), Err(StorageError::Corrupt)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn failed_write_keeps_old_file_and_leaves_no_temp() {
        let dir = temp_dir("atomic");
        let path = dir.join("doc.bin");
        EncryptedJsonRepository::new(&path, FakeProtector)
            .save(&Doc::sample())
            .expect("saves v1");
        let before = std::fs::read(&path).expect("reads v1");

        let failing = EncryptedJsonRepository::new(&path, FailingProtector);
        assert!(matches!(
            failing.save(&Doc {
                schema: 1,
                name: "v2".to_owned(),
                count: 2,
            }),
            Err(StorageError::Protect(_))
        ));

        // The original is byte-identical and still loads.
        assert_eq!(std::fs::read(&path).expect("reads intact"), before);
        let back: Doc = EncryptedJsonRepository::new(&path, FakeProtector)
            .load()
            .expect("loads v1");
        assert_eq!(back, Doc::sample());
        assert!(temp_leftovers(&dir).is_empty(), "no temp files left behind");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unsupported_protector_propagates() {
        let dir = temp_dir("unsupported");
        let path = dir.join("doc.bin");
        // Plant a valid envelope with the working protector so the load
        // reaches `unprotect` (a broken header would report `Corrupt` first).
        EncryptedJsonRepository::new(&path, FakeProtector)
            .save(&Doc::sample())
            .expect("plants valid envelope");

        let repository: EncryptedJsonRepository<Doc, UnsupportedProtector> =
            EncryptedJsonRepository::new(&path, UnsupportedProtector);
        assert!(matches!(
            repository.save(&Doc::sample()),
            Err(StorageError::Unsupported)
        ));
        assert!(matches!(repository.load(), Err(StorageError::Unsupported)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn oversized_document_is_rejected() {
        let dir = temp_dir("size-cap");
        let path = dir.join("doc.bin");
        let repository = EncryptedJsonRepository::new(&path, FakeProtector);
        let big = Doc {
            schema: 1,
            name: "x".repeat(MAX_ENCRYPTED_JSON_BYTES),
            count: 0,
        };
        assert!(matches!(repository.save(&big), Err(StorageError::TooLarge)));
        // Nothing was written.
        assert!(!path.exists());

        // An oversized file is rejected before decryption is attempted.
        let mut envelope = Vec::new();
        envelope.extend_from_slice(ENVELOPE_MAGIC);
        envelope.extend_from_slice(&ENVELOPE_VERSION.to_le_bytes());
        envelope.resize(MAX_ENCRYPTED_JSON_BYTES + CIPHERTEXT_HEADROOM + 1, 0xAA);
        std::fs::write(&path, &envelope).expect("plants oversized");
        assert!(matches!(repository.load(), Err(StorageError::TooLarge)));
        std::fs::remove_dir_all(&dir).ok();
    }
}
