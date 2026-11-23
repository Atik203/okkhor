//! Shared helpers for the `okkhor-lang` integration tests.
//!
//! Each integration target compiles this module separately and uses only a
//! subset, so dead-code lints are silenced here.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use okkhor_lang::{ImportOptions, ImportedAssets};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Root of the committed synthetic fixtures.
pub fn synthetic_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synthetic")
}

/// Test import options: the contract-mandated 1,000-entry cap.
pub fn test_options() -> ImportOptions {
    ImportOptions {
        max_entries_per_asset: 1_000,
        ..ImportOptions::default()
    }
}

/// Fresh empty temp directory for one test.
pub fn temp_dir(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("okkhor-lang-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Write `content` to `rel` (a `/`-separated relative path) under `dir`.
pub fn write_file(dir: &Path, rel: &str, content: &str) -> PathBuf {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dirs");
    }
    std::fs::write(&path, content).expect("write fixture file");
    path
}

/// Write a manifest plus one asset file, returning the directory root.
pub fn single_asset_dir(tag: &str, kind: &str, filename: &str, asset_json: &str) -> PathBuf {
    let dir = temp_dir(tag);
    write_file(&dir, "manifest.json", &manifest_json(kind, filename));
    write_file(&dir, filename, asset_json);
    dir
}

fn manifest_json(kind: &str, filename: &str) -> String {
    format!("{{\"schema\": 1, \"assets\": [{{\"kind\": \"{kind}\", \"path\": \"{filename}\"}}]}}")
}

/// Copy the committed synthetic tree into a temp dir for mutation tests.
pub fn synthetic_copy(tag: &str) -> PathBuf {
    fn copy_dir(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).expect("create dir");
        for entry in std::fs::read_dir(src).expect("read dir") {
            let entry = entry.expect("dir entry");
            let to = dst.join(entry.file_name());
            if entry.file_type().expect("file type").is_dir() {
                copy_dir(&entry.path(), &to);
            } else {
                std::fs::copy(entry.path(), &to).expect("copy file");
            }
        }
    }
    let dir = temp_dir(tag);
    copy_dir(&synthetic_root(), &dir);
    dir
}

/// Assert the daemon-safe projection of an outcome: no error text is logged.
pub fn assert_report_is_log_safe(report: &okkhor_lang::ImportReport) {
    for item in &report.unsupported {
        assert!(!item.name.contains('\\'));
        assert!(!item.name.contains(':'));
    }
}

#[allow(dead_code)]
pub fn empty_assets() -> ImportedAssets {
    ImportedAssets::default()
}
