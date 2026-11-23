use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use okkhor_core::{normalize_lookup, normalize_nfc};
use serde::Deserialize;
use serde_json::Value;

use crate::assets::{
    AssetKind, Dictionary, DictionaryEntry, EncodingEntry, EncodingId, EncodingTable,
    FIXTURE_SCHEMA, ImportReport, ImportedAsset, ImportedAssets, Layout, LayoutId, LayoutMapping,
    SkipReason, UnsupportedAsset,
};
use crate::error::LangError;

/// Maximum bytes read from any single fixture file (manifest or asset).
const MAX_FIXTURE_BYTES: u64 = 8 * 1024 * 1024;
/// Maximum Unicode scalar value accepted for an encoding code.
const MAX_CODE_POINT: u32 = 0x10FFFF;
/// Maximum bytes of a normalized layout key token.
const MAX_KEY_BYTES: usize = 16;

/// Envelope for `manifest.json` and every per-asset fixture file.
///
/// `schema` is read first from a `serde_json::Value`; only after it equals
/// [`FIXTURE_SCHEMA`](crate::assets::FIXTURE_SCHEMA) is the payload
/// deserialized into its typed form. The public data types intentionally have
/// no `schema` field, so a future schema bump cannot be silently ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FixtureEnvelope<T> {
    pub schema: u32,
    #[serde(flatten)]
    pub payload: T,
}

/// Typed shape of a manifest payload (`schema` is consumed by the envelope).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    /// `schema` is consumed by `FixtureEnvelope`, not repeated here.
    pub assets: Vec<ManifestEntry>,
}

/// One entry of [`Manifest`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ManifestEntry {
    pub kind: AssetKind,
    /// Relative, `/`-separated, no `.` or `..` components.
    pub path: String,
}

/// Lenient manifest shape used by the importer so that an unknown `kind`
/// string becomes an [`SkipReason::UnsupportedKind`] skip instead of aborting
/// the whole manifest parse.
#[derive(Debug, Deserialize)]
struct RawManifest {
    assets: Vec<RawManifestEntry>,
}

/// Lenient manifest entry used by the importer (see [`RawManifest`]).
#[derive(Debug, Deserialize)]
struct RawManifestEntry {
    kind: String,
    path: String,
}

/// Importer configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOptions {
    /// Hard cap on entries per asset; [`SkipReason::LimitExceeded`] (or
    /// [`LangError::LimitExceeded`] under `strict`) when breached.
    pub max_entries_per_asset: usize,
    /// When true, the FIRST occurrence in manifest or array order wins and
    /// later duplicates are discarded silently. When false, duplicates are
    /// reported (non-strict) or fatal (strict); see the behavior matrix in
    /// [`import_fixture_dir`].
    pub allow_duplicates: bool,
    /// When true, any skipped asset aborts the whole import with the mapped
    /// [`LangError`]; see the behavior matrix in [`import_fixture_dir`].
    pub strict: bool,
}

impl Default for ImportOptions {
    /// Production default: 100,000 entries per asset, duplicates rejected,
    /// non-strict. Test fixtures use `max_entries_per_asset = 1,000`, set
    /// explicitly so the cap is exercised deterministically.
    fn default() -> Self {
        Self {
            max_entries_per_asset: 100_000,
            allow_duplicates: false,
            strict: false,
        }
    }
}

/// Result of [`import_fixture_dir`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOutcome {
    pub assets: ImportedAssets,
    pub report: ImportReport,
}

/// Per-asset problem found while the manifest as a whole is still readable.
///
/// Fatal environment failures ([`LangError::Io`]) bypass this type and abort
/// the import directly.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingSkip {
    reason: SkipReason,
    /// Static code carried into `LangError::InvalidData` under `strict`.
    detail: &'static str,
    /// Envelope schema value, for `UnsupportedSchema` escalation.
    schema_found: Option<u32>,
}

impl PendingSkip {
    fn new(reason: SkipReason, detail: &'static str) -> Self {
        Self {
            reason,
            detail,
            schema_found: None,
        }
    }

    fn schema(found: u32) -> Self {
        Self {
            reason: SkipReason::UnsupportedSchema,
            detail: "unsupported_schema",
            schema_found: Some(found),
        }
    }

    /// Map to the fatal error used under `strict = true`.
    fn strict_error(&self) -> LangError {
        match self.reason {
            SkipReason::UnsupportedSchema => LangError::UnsupportedSchema {
                found: self.schema_found.unwrap_or(0),
                expected: FIXTURE_SCHEMA,
            },
            SkipReason::PathRejected => LangError::UnsafePath,
            SkipReason::LimitExceeded => LangError::LimitExceeded,
            SkipReason::DuplicateId => LangError::DuplicateId,
            SkipReason::UnsupportedKind | SkipReason::InvalidId | SkipReason::MalformedData => {
                LangError::InvalidData {
                    reason: self.detail,
                }
            }
        }
    }
}

/// Internal result of processing one manifest entry.
enum EntryAction {
    /// Asset validated and ready to insert (dedupe against `seen` ids still
    /// pending at the call site).
    Ready(PendingAsset),
}

/// A validated asset plus its manifest identity.
enum PendingAsset {
    Layout(LayoutId, Layout),
    Encoding(EncodingId, EncodingTable),
    Dictionary(String, Dictionary),
}

/// Import every asset listed in `<root>/manifest.json`.
///
/// Root and all manifest paths are canonicalized; traversal (including
/// symlink/junction escape) is rejected. Never panics on malformed input:
/// under `strict = false` each problem asset is recorded in
/// [`ImportReport::unsupported`] and the import continues; under
/// `strict = true` the first problem aborts with the mapped [`LangError`].
///
/// Duplicate and strict behavior: "duplicate" means (a) an asset id repeated
/// within a collection, or (b) a mapping key, encoding code, or dictionary
/// word repeated inside one asset. The first occurrence in manifest or array
/// order always wins.
///
/// | `allow_duplicates` | `strict` | Behavior |
/// | --- | --- | --- |
/// | `false` (default) | `false` (default) | Whole duplicate asset is skipped; an `UnsupportedAsset` with `SkipReason::DuplicateId` is appended, `skipped` is incremented, and the import continues. |
/// | `false` | `true` | Duplicate is fatal: `LangError::DuplicateId` is returned immediately; no partial `ImportOutcome` is returned. |
/// | `true` | `false` | Duplicate occurrences are discarded silently (no `unsupported` entry, no error); the first occurrence is retained. |
/// | `true` | `true` | Same as `true`/`false`: an explicitly allowed duplicate is not a skip, so `strict` has nothing to escalate. |
///
/// Other skip reasons map to errors under `strict = true` as follows:
/// `UnsupportedSchema` → `LangError::UnsupportedSchema`,
/// `PathRejected` → `LangError::UnsafePath`,
/// `LimitExceeded` → `LangError::LimitExceeded`,
/// `DuplicateId` → `LangError::DuplicateId`, and
/// `UnsupportedKind` | `InvalidId` | `MalformedData` →
/// `LangError::InvalidData` with a static reason string.
/// The returned `unsupported` list is sorted by `(kind, name, reason)`.
pub fn import_fixture_dir(
    root: &Path,
    options: &ImportOptions,
) -> Result<ImportOutcome, LangError> {
    let canonical_root = fs::canonicalize(root).map_err(LangError::Io)?;
    let manifest_path = canonical_root.join("manifest.json");
    let manifest_bytes = read_capped(&manifest_path).map_err(|error| match error {
        ReadError::Io(error) => LangError::Io(error),
        ReadError::TooLarge => LangError::LimitExceeded,
    })?;
    let manifest_value: Value = serde_json::from_slice(&manifest_bytes).map_err(LangError::Json)?;
    let manifest_missing_schema = manifest_value.get("schema").is_none();
    let manifest_envelope: FixtureEnvelope<Value> = serde_json::from_value(manifest_value)
        .map_err(|error| {
            if manifest_missing_schema {
                LangError::UnsupportedSchema {
                    found: 0,
                    expected: FIXTURE_SCHEMA,
                }
            } else {
                LangError::Json(error)
            }
        })?;
    if manifest_envelope.schema != FIXTURE_SCHEMA {
        return Err(LangError::UnsupportedSchema {
            found: manifest_envelope.schema,
            expected: FIXTURE_SCHEMA,
        });
    }
    let raw: RawManifest =
        serde_json::from_value(manifest_envelope.payload).map_err(LangError::Json)?;

    let mut assets = ImportedAssets::default();
    let mut unsupported: Vec<UnsupportedAsset> = Vec::new();
    let mut imported: u32 = 0;
    let mut seen_layouts: BTreeSet<LayoutId> = BTreeSet::new();
    let mut seen_encodings: BTreeSet<EncodingId> = BTreeSet::new();
    let mut seen_dictionaries: BTreeSet<String> = BTreeSet::new();

    for entry in &raw.assets {
        let entry_kind = AssetKind::from_code(entry.kind.as_str());
        match process_entry(&canonical_root, entry, entry_kind, options) {
            Ok(EntryAction::Ready(pending)) => {
                let duplicate = match &pending {
                    PendingAsset::Layout(id, _) => !seen_layouts.insert(id.clone()),
                    PendingAsset::Encoding(id, _) => !seen_encodings.insert(id.clone()),
                    PendingAsset::Dictionary(id, _) => !seen_dictionaries.insert(id.clone()),
                };
                if duplicate {
                    if options.allow_duplicates {
                        // Explicitly allowed: silent drop, first occurrence retained.
                        continue;
                    }
                    handle_skip(
                        &mut unsupported,
                        options,
                        entry,
                        entry_kind,
                        &PendingSkip::new(SkipReason::DuplicateId, "duplicate_asset_id"),
                    )?;
                    continue;
                }
                match pending {
                    PendingAsset::Layout(id, layout) => {
                        assets.layouts.insert(id, layout);
                    }
                    PendingAsset::Encoding(id, table) => {
                        assets.encodings.insert(id, table);
                    }
                    PendingAsset::Dictionary(id, dictionary) => {
                        assets.dictionaries.insert(id, dictionary);
                    }
                }
                imported += 1;
            }
            Err(WorkError::Skip(skip)) => {
                handle_skip(&mut unsupported, options, entry, entry_kind, &skip)?;
            }
            Err(WorkError::Fatal(error)) => return Err(error),
        }
    }

    unsupported.sort_by(|left, right| {
        (
            left.kind,
            &left.name,
            skip_rank(left.reason),
            &left.reason_detail(),
        )
            .cmp(&(
                right.kind,
                &right.name,
                skip_rank(right.reason),
                &right.reason_detail(),
            ))
    });
    let skipped = unsupported.len() as u32;
    Ok(ImportOutcome {
        assets,
        report: ImportReport {
            imported,
            skipped,
            unsupported,
        },
    })
}

/// Import one asset file.
///
/// `path` may be absolute or relative to the process working directory, but
/// must not contain `.` or `..` components. There is no skip report here:
/// every problem is returned as a [`LangError`]. `strict` has no additional
/// effect on this single-asset API; `allow_duplicates` still controls whether
/// repeated keys/codes/words are discarded silently (`true`) or rejected with
/// [`LangError::DuplicateId`] (`false`).
pub fn import_asset(
    kind: AssetKind,
    path: &Path,
    options: &ImportOptions,
) -> Result<ImportedAsset, LangError> {
    reject_dot_components(path)?;
    let canonical = fs::canonicalize(path).map_err(LangError::Io)?;
    let bytes = read_capped(&canonical).map_err(|error| match error {
        ReadError::Io(error) => LangError::Io(error),
        ReadError::TooLarge => LangError::LimitExceeded,
    })?;
    let value: Value = serde_json::from_slice(&bytes).map_err(LangError::Json)?;
    let payload = split_asset_envelope(value).map_err(|skip| skip.strict_error())?;
    build_asset(kind, payload, options)
        .map(|pending| match pending {
            PendingAsset::Layout(_, layout) => ImportedAsset::Layout(layout),
            PendingAsset::Encoding(_, table) => ImportedAsset::Encoding(table),
            PendingAsset::Dictionary(_, dictionary) => ImportedAsset::Dictionary(dictionary),
        })
        .map_err(|skip| skip.strict_error())
}

/// Failure of [`process_entry`] that is not a skippable asset problem.
enum WorkError {
    Skip(PendingSkip),
    Fatal(LangError),
}

impl From<PendingSkip> for WorkError {
    fn from(skip: PendingSkip) -> Self {
        Self::Skip(skip)
    }
}

/// Read one manifest entry: resolve its path, parse the envelope, validate
/// the payload, and apply within-asset duplicate handling.
fn process_entry(
    canonical_root: &Path,
    entry: &RawManifestEntry,
    entry_kind: Option<AssetKind>,
    options: &ImportOptions,
) -> Result<EntryAction, WorkError> {
    let kind = entry_kind.ok_or(PendingSkip::new(
        SkipReason::UnsupportedKind,
        "unsupported_kind",
    ))?;
    let canonical = resolve_under_root(canonical_root, entry.path.as_str())?;
    let bytes = match read_capped(&canonical) {
        Ok(bytes) => bytes,
        Err(ReadError::Io(error)) => return Err(WorkError::Fatal(LangError::Io(error))),
        Err(ReadError::TooLarge) => {
            return Err(PendingSkip::new(SkipReason::LimitExceeded, "fixture_too_large").into());
        }
    };
    let value: Value = match serde_json::from_slice(bytes.as_slice()) {
        Ok(value) => value,
        Err(_) => {
            return Err(PendingSkip::new(SkipReason::MalformedData, "invalid_json").into());
        }
    };
    let payload = split_asset_envelope(value)?;
    let pending = build_asset(kind, payload, options)?;
    Ok(EntryAction::Ready(pending))
}

/// Record a skip under `strict = false`, or escalate under `strict = true`.
/// `entry_kind` is `None` only for unknown kind strings; the report uses
/// `AssetKind::Layout` as a placeholder so the entry keeps a stable shape.
fn handle_skip(
    unsupported: &mut Vec<UnsupportedAsset>,
    options: &ImportOptions,
    entry: &RawManifestEntry,
    entry_kind: Option<AssetKind>,
    skip: &PendingSkip,
) -> Result<(), LangError> {
    if options.strict {
        return Err(skip.strict_error());
    }
    unsupported.push(UnsupportedAsset {
        kind: entry_kind.unwrap_or(AssetKind::Layout),
        name: entry.path.clone(),
        reason: skip.reason,
    });
    Ok(())
}

/// Verify the envelope `schema` and return the payload. The public data types
/// have no `schema` field, so the envelope is the only source of `found`: a
/// future schema bump cannot be silently ignored.
fn split_asset_envelope(value: Value) -> Result<Value, PendingSkip> {
    let missing_schema = value.get("schema").is_none();
    match serde_json::from_value::<FixtureEnvelope<Value>>(value) {
        Ok(envelope) if envelope.schema == FIXTURE_SCHEMA => Ok(envelope.payload),
        Ok(envelope) => Err(PendingSkip::schema(envelope.schema)),
        Err(_) if missing_schema => Err(PendingSkip::schema(0)),
        Err(_) => Err(PendingSkip::new(SkipReason::MalformedData, "invalid_json")),
    }
}

/// Validate one asset payload into its stored form (sorted, deduplicated,
/// normalized). Within-asset duplicates follow the duplicate matrix: the
/// first occurrence in array order always wins.
fn build_asset(
    kind: AssetKind,
    payload: Value,
    options: &ImportOptions,
) -> Result<PendingAsset, PendingSkip> {
    match kind {
        AssetKind::Layout => {
            build_layout(payload, options).map(|(id, layout)| PendingAsset::Layout(id, layout))
        }
        AssetKind::Encoding => {
            build_encoding(payload, options).map(|(id, table)| PendingAsset::Encoding(id, table))
        }
        AssetKind::Dictionary => build_dictionary(payload, options)
            .map(|(id, dictionary)| PendingAsset::Dictionary(id, dictionary)),
    }
}

fn typed<T: serde::de::DeserializeOwned>(payload: Value) -> Result<T, PendingSkip> {
    serde_json::from_value(payload)
        .map_err(|_| PendingSkip::new(SkipReason::MalformedData, "malformed_data"))
}

fn build_layout(
    payload: Value,
    options: &ImportOptions,
) -> Result<(LayoutId, Layout), PendingSkip> {
    let raw: Layout = typed(payload)?;
    let id = LayoutId::validated(raw.id.as_str())
        .map_err(|_| PendingSkip::new(SkipReason::InvalidId, "invalid_layout_id"))?;
    if raw.mappings.len() > options.max_entries_per_asset {
        return Err(PendingSkip::new(
            SkipReason::LimitExceeded,
            "too_many_mappings",
        ));
    }
    let mut mappings: Vec<LayoutMapping> = Vec::with_capacity(raw.mappings.len());
    for mapping in raw.mappings {
        let key = normalize_lookup(mapping.key.as_str());
        if key.is_empty() || key.len() > MAX_KEY_BYTES || !key.is_ascii() {
            return Err(PendingSkip::new(
                SkipReason::MalformedData,
                "invalid_mapping_key",
            ));
        }
        mappings.push(LayoutMapping {
            key,
            output: normalize_nfc(mapping.output.as_str()),
        });
    }
    mappings.sort_by(|left, right| left.key.cmp(&right.key));
    let mut deduped: Vec<LayoutMapping> = Vec::with_capacity(mappings.len());
    for mapping in mappings {
        let duplicate = deduped
            .last()
            .is_some_and(|last: &LayoutMapping| last.key == mapping.key);
        if duplicate {
            if options.allow_duplicates {
                continue;
            }
            return Err(PendingSkip::new(
                SkipReason::DuplicateId,
                "duplicate_mapping_key",
            ));
        }
        deduped.push(mapping);
    }
    Ok((
        id.clone(),
        Layout {
            id,
            label: raw.label,
            language: raw.language,
            mappings: deduped,
        },
    ))
}

fn build_encoding(
    payload: Value,
    options: &ImportOptions,
) -> Result<(EncodingId, EncodingTable), PendingSkip> {
    let raw: EncodingTable = typed(payload)?;
    let id = EncodingId::validated(raw.id.as_str())
        .map_err(|_| PendingSkip::new(SkipReason::InvalidId, "invalid_encoding_id"))?;
    if raw.entries.len() > options.max_entries_per_asset {
        return Err(PendingSkip::new(
            SkipReason::LimitExceeded,
            "too_many_entries",
        ));
    }
    let mut entries: Vec<EncodingEntry> = Vec::with_capacity(raw.entries.len());
    for entry in raw.entries {
        if entry.code > MAX_CODE_POINT {
            return Err(PendingSkip::new(
                SkipReason::MalformedData,
                "code_out_of_range",
            ));
        }
        let unicode = normalize_nfc(entry.unicode.as_str());
        if unicode.is_empty() {
            return Err(PendingSkip::new(
                SkipReason::MalformedData,
                "empty_unicode_value",
            ));
        }
        entries.push(EncodingEntry {
            code: entry.code,
            unicode,
        });
    }
    entries.sort_by_key(|entry| entry.code);
    let mut deduped: Vec<EncodingEntry> = Vec::with_capacity(entries.len());
    for entry in entries {
        let duplicate = deduped
            .last()
            .is_some_and(|last: &EncodingEntry| last.code == entry.code);
        if duplicate {
            if options.allow_duplicates {
                continue;
            }
            return Err(PendingSkip::new(
                SkipReason::DuplicateId,
                "duplicate_encoding_code",
            ));
        }
        deduped.push(entry);
    }
    Ok((
        id.clone(),
        EncodingTable {
            id,
            entries: deduped,
        },
    ))
}

fn build_dictionary(
    payload: Value,
    options: &ImportOptions,
) -> Result<(String, Dictionary), PendingSkip> {
    let raw: Dictionary = typed(payload)?;
    if raw.id.is_empty() {
        return Err(PendingSkip::new(
            SkipReason::InvalidId,
            "empty_dictionary_id",
        ));
    }
    if raw.entries.len() > options.max_entries_per_asset {
        return Err(PendingSkip::new(
            SkipReason::LimitExceeded,
            "too_many_entries",
        ));
    }
    let mut entries: Vec<DictionaryEntry> = Vec::with_capacity(raw.entries.len());
    for entry in raw.entries {
        let word = normalize_lookup(entry.word.as_str());
        if word.is_empty() {
            return Err(PendingSkip::new(
                SkipReason::MalformedData,
                "empty_dictionary_word",
            ));
        }
        entries.push(DictionaryEntry {
            word,
            frequency: entry.frequency,
        });
    }
    entries.sort_by(|left, right| left.word.cmp(&right.word));
    let mut deduped: Vec<DictionaryEntry> = Vec::with_capacity(entries.len());
    for entry in entries {
        let duplicate = deduped
            .last()
            .is_some_and(|last: &DictionaryEntry| last.word == entry.word);
        if duplicate {
            if options.allow_duplicates {
                continue;
            }
            return Err(PendingSkip::new(
                SkipReason::DuplicateId,
                "duplicate_dictionary_word",
            ));
        }
        deduped.push(entry);
    }
    Ok((
        raw.id.clone(),
        Dictionary {
            id: raw.id,
            language: raw.language,
            entries: deduped,
        },
    ))
}

/// Deterministic rank for [`SkipReason`] used when sorting the report.
fn skip_rank(reason: SkipReason) -> u8 {
    match reason {
        SkipReason::UnsupportedKind => 0,
        SkipReason::UnsupportedSchema => 1,
        SkipReason::InvalidId => 2,
        SkipReason::DuplicateId => 3,
        SkipReason::LimitExceeded => 4,
        SkipReason::MalformedData => 5,
        SkipReason::PathRejected => 6,
    }
}

trait ReasonDetail {
    fn reason_detail(&self) -> &'static str;
}

impl ReasonDetail for UnsupportedAsset {
    fn reason_detail(&self) -> &'static str {
        match self.reason {
            SkipReason::UnsupportedKind => "unsupported_kind",
            SkipReason::UnsupportedSchema => "unsupported_schema",
            SkipReason::InvalidId => "invalid_id",
            SkipReason::DuplicateId => "duplicate_id",
            SkipReason::LimitExceeded => "limit_exceeded",
            SkipReason::MalformedData => "malformed_data",
            SkipReason::PathRejected => "path_rejected",
        }
    }
}

/// Reject `.` and `..` path components lexically (no filesystem access).
fn reject_dot_components(path: &Path) -> Result<(), LangError> {
    use std::path::Component;
    for component in path.components() {
        match component {
            Component::CurDir | Component::ParentDir => return Err(LangError::UnsafePath),
            _ => {}
        }
    }
    Ok(())
}

/// Resolve a manifest `path` under `canonical_root`.
///
/// Rejects absolute paths, backslash/drive tricks, and `.`/`..` components
/// lexically, then canonicalizes and requires the result to stay under the
/// fixture root (defeats symlink/junction escape) and to be a file.
fn resolve_under_root(canonical_root: &Path, raw: &str) -> Result<PathBuf, PendingSkip> {
    let rejected = || PendingSkip::new(SkipReason::PathRejected, "path_rejected");
    if raw.is_empty() || raw.starts_with('/') || raw.starts_with('\\') {
        return Err(rejected());
    }
    if raw.len() >= 2 && raw.as_bytes()[1] == b':' {
        return Err(rejected());
    }
    if raw.contains('\\') || raw.contains(':') {
        return Err(rejected());
    }
    for component in raw.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(rejected());
        }
    }
    let joined = canonical_root.join(raw);
    let canonical = fs::canonicalize(&joined).map_err(|_| rejected())?;
    if !canonical.starts_with(canonical_root) {
        return Err(rejected());
    }
    if !canonical.is_file() {
        return Err(rejected());
    }
    Ok(canonical)
}

enum ReadError {
    Io(std::io::Error),
    TooLarge,
}

/// Read a fixture file, enforcing the 8 MiB per-file byte cap.
fn read_capped(path: &Path) -> Result<Vec<u8>, ReadError> {
    let metadata = fs::metadata(path).map_err(ReadError::Io)?;
    if metadata.len() > MAX_FIXTURE_BYTES {
        return Err(ReadError::TooLarge);
    }
    let bytes = fs::read(path).map_err(ReadError::Io)?;
    if bytes.len() as u64 > MAX_FIXTURE_BYTES {
        return Err(ReadError::TooLarge);
    }
    Ok(bytes)
}
