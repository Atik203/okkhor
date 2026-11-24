#![forbid(unsafe_code)]
//! Layout, encoding, and dictionary data types plus a synthetic-fixture
//! importer.
//!
//! This crate is pure data and startup-time import logic. It performs no I/O
//! on the typing path: [`importer::import_fixture_dir`] runs once at daemon
//! startup and the runtime calls ([`Layout::lookup`], [`Dictionary::get`]) are
//! bounded in-memory lookups on the input worker.
//!
//! All fixtures are synthetic JSON committed under
//! `tests/fixtures/synthetic/`. No legacy files, binaries, or proprietary
//! assets are read.

pub mod assets;
pub mod bijoy;
pub mod error;
pub mod fixed_layout;
pub mod importer;
pub mod sutonnymj;

pub use assets::{
    AssetKind, Dictionary, DictionaryEntry, EncodingEntry, EncodingId, EncodingTable, ImportReport,
    ImportedAsset, ImportedAssets, Layout, LayoutId, LayoutMapping, SkipReason, UnsupportedAsset,
};
pub use bijoy::{
    BijoyError, MAX_INPUT_CHARS, REVERSE_RULE_COUNT, RULE_COUNT, from_bijoy, reverse_rule_count,
    rule_count, to_bijoy,
};
pub use error::LangError;
pub use importer::{
    FixtureEnvelope, ImportOptions, ImportOutcome, Manifest, ManifestEntry, import_asset,
    import_fixture_dir,
};
