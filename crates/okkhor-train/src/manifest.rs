#![forbid(unsafe_code)]
//! Dataset manifest v1: validation and file verification.
//!
//! One manifest per data file, named `<data-stem>.manifest.json` beside the
//! data file (`<data-stem>.jsonl|.txt|.csv|.tsv`). The manifest records the
//! license, provenance, SHA-256, and row count; [`verify_manifest`] re-hashes
//! the data file and re-counts its rows. Offline only: no network, no panics.
//! See `docs/design/datasets.md` §"Manifest v1".

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::generate::sha256_hex;

/// Expected value of [`DatasetManifest::schema`].
pub const MANIFEST_SCHEMA: u32 = 1;

/// Maximum note length in characters.
pub const MAX_NOTES_CHARS: usize = 1024;

/// Data-file extensions tried when resolving a manifest, in order.
const DATA_EXTENSIONS: [&str; 4] = ["jsonl", "txt", "csv", "tsv"];

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ManifestError {
    #[error("cannot read manifest {path}: {reason}")]
    ReadManifest { path: String, reason: String },
    #[error("malformed manifest JSON: {reason}")]
    Malformed { reason: String },
    #[error("unsupported schema {found}; expected {expected}")]
    UnsupportedSchema { found: u32, expected: u32 },
    #[error("field `{field}` must not be blank")]
    BlankField { field: &'static str },
    #[error("license `{value}` is unknown and cannot be used")]
    UnknownLicense { value: String },
    #[error("sha256 must be 64 lowercase hex chars, got `{value}`")]
    InvalidSha256 { value: String },
    #[error("languages must list at least one language")]
    NoLanguages,
    #[error("unknown split `{value}`")]
    UnknownSplit { value: String },
    #[error("notes exceed {max} characters")]
    NotesTooLong { max: usize },
    #[error("manifest name `{name}` does not follow `<data-stem>.manifest.json`")]
    ManifestName { name: String },
    #[error("no data file found for manifest; expected one of {expected}")]
    DataMissing { expected: String },
    #[error("cannot read data file {path}: {reason}")]
    ReadData { path: String, reason: String },
    #[error("data file {path} is not valid UTF-8")]
    DataNotUtf8 { path: String },
    #[error("sha256 mismatch: manifest has {expected}, file is {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("row mismatch: manifest has {expected}, file has {actual}")]
    RowMismatch { expected: u64, actual: u64 },
}

/// Fixed partition of a dataset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Split {
    Train,
    Validation,
    Test,
    Eval,
}

impl Split {
    pub fn as_str(self) -> &'static str {
        match self {
            Split::Train => "train",
            Split::Validation => "validation",
            Split::Test => "test",
            Split::Eval => "eval",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "train" => Some(Split::Train),
            "validation" => Some(Split::Validation),
            "test" => Some(Split::Test),
            "eval" => Some(Split::Eval),
            _ => None,
        }
    }
}

/// A validated dataset manifest v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetManifest {
    pub schema: u32,
    pub name: String,
    pub version: String,
    pub license: String,
    pub source: String,
    pub sha256: String,
    pub rows: u64,
    pub languages: Vec<String>,
    pub split: Split,
    #[serde(default)]
    pub notes: String,
}

/// Fields as they appear on disk, before typed validation.
#[derive(Debug, Deserialize)]
struct RawManifest {
    #[serde(default)]
    schema: u32,
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    license: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    sha256: String,
    #[serde(default)]
    rows: u64,
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default)]
    split: String,
    #[serde(default)]
    notes: String,
}

fn is_lowercase_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl DatasetManifest {
    /// Parse and validate one manifest document.
    pub fn from_json(json: &str) -> Result<Self, ManifestError> {
        let raw: RawManifest =
            serde_json::from_str(json).map_err(|error| ManifestError::Malformed {
                reason: error.to_string(),
            })?;
        let split = Split::parse(raw.split.trim()).ok_or_else(|| ManifestError::UnknownSplit {
            value: raw.split.clone(),
        })?;
        let manifest = DatasetManifest {
            schema: raw.schema,
            name: raw.name,
            version: raw.version,
            license: raw.license,
            source: raw.source,
            sha256: raw.sha256,
            rows: raw.rows,
            languages: raw.languages,
            split,
            notes: raw.notes,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validate every v1 rule without touching the data file.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema != MANIFEST_SCHEMA {
            return Err(ManifestError::UnsupportedSchema {
                found: self.schema,
                expected: MANIFEST_SCHEMA,
            });
        }
        for (field, value) in [
            ("name", &self.name),
            ("version", &self.version),
            ("license", &self.license),
            ("source", &self.source),
        ] {
            if value.trim().is_empty() {
                return Err(ManifestError::BlankField { field });
            }
        }
        if self.license.trim().eq_ignore_ascii_case("unknown") {
            return Err(ManifestError::UnknownLicense {
                value: self.license.clone(),
            });
        }
        if !is_lowercase_hex(&self.sha256) {
            return Err(ManifestError::InvalidSha256 {
                value: self.sha256.clone(),
            });
        }
        if self.languages.is_empty() {
            return Err(ManifestError::NoLanguages);
        }
        if self.languages.iter().any(|lang| lang.trim().is_empty()) {
            return Err(ManifestError::BlankField { field: "languages" });
        }
        if self.notes.chars().count() > MAX_NOTES_CHARS {
            return Err(ManifestError::NotesTooLong {
                max: MAX_NOTES_CHARS,
            });
        }
        Ok(())
    }
}

/// A manifest plus the data file it was checked against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedManifest {
    pub manifest: DatasetManifest,
    pub data_path: PathBuf,
    pub sha256: String,
    pub rows: u64,
}

/// Resolve `<data-stem>` next to the manifest and return the first existing
/// data file with a known extension.
fn resolve_data_path(manifest_path: &Path) -> Result<PathBuf, ManifestError> {
    let name = manifest_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let stem = name
        .strip_suffix(".manifest.json")
        .or_else(|| name.strip_suffix(".json"))
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| ManifestError::ManifestName {
            name: name.to_owned(),
        })?;
    let directory = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let mut expected = Vec::with_capacity(DATA_EXTENSIONS.len());
    for extension in DATA_EXTENSIONS {
        let candidate = directory.join(format!("{stem}.{extension}"));
        if candidate.is_file() {
            return Ok(candidate);
        }
        expected.push(candidate.display().to_string());
    }
    Err(ManifestError::DataMissing {
        expected: expected.join(", "),
    })
}

/// Non-empty line count; CSV/TSV subtract one header line (saturating).
pub fn count_rows(text: &str, extension: &str) -> u64 {
    let non_empty = text.lines().filter(|line| !line.trim().is_empty()).count() as u64;
    match extension.to_ascii_lowercase().as_str() {
        "csv" | "tsv" => non_empty.saturating_sub(1),
        _ => non_empty,
    }
}

/// Read a manifest, resolve its data file, re-hash it, count its rows, and
/// compare both against the manifest.
pub fn verify_manifest(path: impl AsRef<Path>) -> Result<VerifiedManifest, ManifestError> {
    let manifest_path = path.as_ref();
    let document =
        fs::read_to_string(manifest_path).map_err(|error| ManifestError::ReadManifest {
            path: manifest_path.display().to_string(),
            reason: error.to_string(),
        })?;
    let manifest = DatasetManifest::from_json(&document)?;

    let data_path = resolve_data_path(manifest_path)?;
    let bytes = fs::read(&data_path).map_err(|error| ManifestError::ReadData {
        path: data_path.display().to_string(),
        reason: error.to_string(),
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|_| ManifestError::DataNotUtf8 {
        path: data_path.display().to_string(),
    })?;

    let actual_sha256 = sha256_hex(&bytes);
    if actual_sha256 != manifest.sha256 {
        return Err(ManifestError::ChecksumMismatch {
            expected: manifest.sha256.clone(),
            actual: actual_sha256,
        });
    }

    let extension = data_path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let actual_rows = count_rows(text, extension);
    if actual_rows != manifest.rows {
        return Err(ManifestError::RowMismatch {
            expected: manifest.rows,
            actual: actual_rows,
        });
    }

    Ok(VerifiedManifest {
        manifest,
        data_path,
        sha256: actual_sha256,
        rows: actual_rows,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "okkhor-train-manifest-{tag}-{}-{id}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const ROWS: &str = "{\"id\":\"a\"}\n{\"id\":\"b\"}\n";

    fn manifest(sha256: String, rows: u64) -> DatasetManifest {
        DatasetManifest {
            schema: MANIFEST_SCHEMA,
            name: "unit-corpus".to_owned(),
            version: "1.0.0".to_owned(),
            license: "CC0-1.0".to_owned(),
            source: "synthetic".to_owned(),
            sha256,
            rows,
            languages: vec!["bn".to_owned()],
            split: Split::Train,
            notes: "unit fixture".to_owned(),
        }
    }

    /// Write the data file and manifest; returns the manifest path.
    fn write_fixture(dir: &TempDir, manifest: &DatasetManifest) -> PathBuf {
        fs::write(dir.0.join("unit-corpus.jsonl"), ROWS).expect("data file");
        let path = dir.0.join("unit-corpus.manifest.json");
        fs::write(
            &path,
            serde_json::to_string_pretty(manifest).expect("manifest serializes"),
        )
        .expect("manifest file");
        path
    }

    fn valid_manifest() -> DatasetManifest {
        manifest(sha256_hex(ROWS.as_bytes()), 2)
    }

    #[test]
    fn committed_fixture_verifies() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("data")
            .join("fixtures")
            .join("mini-corpus.manifest.json");
        let verified = verify_manifest(&path).expect("committed fixture verifies");
        assert_eq!(verified.manifest.name, "mini-corpus");
        assert_eq!(verified.manifest.split, Split::Train);
        assert_eq!(verified.rows, 16);
        assert_eq!(verified.rows, verified.manifest.rows);
    }

    #[test]
    fn missing_license_is_rejected() {
        let json = r#"{"schema":1,"name":"n","version":"1","source":"s","sha256":"0000000000000000000000000000000000000000000000000000000000000000","rows":1,"languages":["bn"],"split":"train"}"#;
        assert_eq!(
            DatasetManifest::from_json(json),
            Err(ManifestError::BlankField { field: "license" })
        );
    }

    #[test]
    fn blank_license_is_rejected() {
        let mut bad = valid_manifest();
        bad.license = "   ".to_owned();
        assert_eq!(
            bad.validate(),
            Err(ManifestError::BlankField { field: "license" })
        );
    }

    #[test]
    fn unknown_license_is_rejected_case_insensitively() {
        let mut bad = valid_manifest();
        bad.license = "UNKNOWN".to_owned();
        assert_eq!(
            bad.validate(),
            Err(ManifestError::UnknownLicense {
                value: "UNKNOWN".to_owned()
            })
        );
    }

    #[test]
    fn uppercase_checksum_is_rejected() {
        let mut bad = valid_manifest();
        bad.sha256 = bad.sha256.to_uppercase();
        assert_eq!(
            bad.validate(),
            Err(ManifestError::InvalidSha256 {
                value: bad.sha256.clone()
            })
        );
    }

    #[test]
    fn unsupported_schema_is_rejected() {
        let mut bad = valid_manifest();
        bad.schema = 2;
        assert_eq!(
            bad.validate(),
            Err(ManifestError::UnsupportedSchema {
                found: 2,
                expected: MANIFEST_SCHEMA
            })
        );
    }

    #[test]
    fn unknown_split_is_rejected() {
        let json = r#"{"schema":1,"name":"n","version":"1","license":"CC0-1.0","source":"s","sha256":"0000000000000000000000000000000000000000000000000000000000000000","rows":1,"languages":["bn"],"split":"holdout"}"#;
        assert_eq!(
            DatasetManifest::from_json(json),
            Err(ManifestError::UnknownSplit {
                value: "holdout".to_owned()
            })
        );
    }

    #[test]
    fn bad_checksum_is_rejected() {
        let dir = TempDir::new("checksum");
        let bad = manifest("0".repeat(64), 2);
        let path = write_fixture(&dir, &bad);
        assert!(matches!(
            verify_manifest(&path),
            Err(ManifestError::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn row_mismatch_is_rejected() {
        let dir = TempDir::new("rows");
        let bad = manifest(sha256_hex(ROWS.as_bytes()), 3);
        let path = write_fixture(&dir, &bad);
        assert_eq!(
            verify_manifest(&path),
            Err(ManifestError::RowMismatch {
                expected: 3,
                actual: 2
            })
        );
    }

    #[test]
    fn csv_and_tsv_subtract_one_header() {
        assert_eq!(count_rows("a,b\n1,2\n3,4\n", "csv"), 2);
        assert_eq!(count_rows("header\n", "tsv"), 0);
        assert_eq!(count_rows("one\ntwo\n", "jsonl"), 2);
        assert_eq!(count_rows("one\n\ntwo\n", "bin"), 2);
    }
}
