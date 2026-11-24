use std::collections::BTreeMap;

use okkhor_core::Language;
use serde::{Deserialize, Serialize};

use crate::error::LangError;

/// Schema version accepted by every fixture file in this crate.
pub const FIXTURE_SCHEMA: u32 = 1;

/// Maximum length in bytes of a layout or rule id code.
const MAX_ID_LEN: usize = 32;

/// Kind of asset listed in a fixture manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Layout,
    Encoding,
    Dictionary,
}

impl AssetKind {
    /// Stable string code used in manifests (`"layout"`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Layout => "layout",
            Self::Encoding => "encoding",
            Self::Dictionary => "dictionary",
        }
    }

    /// Parse a manifest kind string.
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "layout" => Some(Self::Layout),
            "encoding" => Some(Self::Encoding),
            "dictionary" => Some(Self::Dictionary),
            _ => None,
        }
    }
}

/// Whether an id code matches `[a-z0-9_-]`, length 1..=32 bytes.
fn valid_id_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= MAX_ID_LEN
        && code.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
}

/// Layout identifier, e.g. `"avro"`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LayoutId(String);

impl LayoutId {
    /// Known 0.3 layout codes. Custom ids are allowed but `is_known()` is false.
    pub const AVRO: &'static str = "avro";
    pub const NATIONAL: &'static str = "national";
    pub const PROBHAT: &'static str = "probhat";
    pub const BIJOY: &'static str = "bijoy";
    /// Canonical code for romanized Bangla phonetic input.
    pub const PHONETIC: &'static str = "phonetic";
    /// Legacy alias; accepted on input but never emitted as canonical.
    pub const BANGLISH: &'static str = "banglish";
    pub const KHIPRO: &'static str = "khipro";

    /// Validates `[a-z0-9_-]`, length 1..=32 bytes. Mirrors
    /// `Language::from_code`.
    pub fn from_code(code: &str) -> Option<Self> {
        valid_id_code(code).then(|| Self(code.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is one of the known 0.3 layout codes.
    pub fn is_known(&self) -> bool {
        matches!(
            self.0.as_str(),
            Self::AVRO
                | Self::NATIONAL
                | Self::PROBHAT
                | Self::BIJOY
                | Self::PHONETIC
                | Self::BANGLISH
                | Self::KHIPRO
        )
    }

    pub(crate) fn validated(code: &str) -> Result<Self, LangError> {
        Self::from_code(code).ok_or(LangError::InvalidData {
            reason: "invalid_layout_id",
        })
    }
}

/// One key-to-output rule inside a layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutMapping {
    /// Synthetic key token, ASCII, 1..=16 bytes after lookup normalization
    /// (e.g. `"a"`, `"shift+a"`).
    pub key: String,
    /// NFC output string; normalized at import with
    /// `okkhor_core::normalize_nfc`.
    pub output: String,
}

/// A keyboard layout: an ordered map from key tokens to NFC output strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub id: LayoutId,
    pub label: String,
    pub language: Language,
    /// Sorted by `key`; no duplicate keys.
    pub mappings: Vec<LayoutMapping>,
}

impl Layout {
    /// Binary search over sorted `mappings`. Normalizes `key` internally with
    /// `okkhor_core::normalize_lookup`; the importer applies the same
    /// normalization to stored keys, so callers pass the raw key token.
    /// Performs no I/O.
    pub fn lookup(&self, key: &str) -> Option<&str> {
        let normalized = okkhor_core::normalize_lookup(key);
        self.mappings
            .binary_search_by(|mapping| mapping.key.as_str().cmp(&normalized))
            .ok()
            .map(|index| self.mappings[index].output.as_str())
    }
}

/// Encoding table identifier, e.g. `"synthetic_ascii"`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EncodingId(String);

impl EncodingId {
    /// Same `[a-z0-9_-]`, 1..=32 byte validation as [`LayoutId`].
    pub fn from_code(code: &str) -> Option<Self> {
        valid_id_code(code).then(|| Self(code.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn validated(code: &str) -> Result<Self, LangError> {
        Self::from_code(code).ok_or(LangError::InvalidData {
            reason: "invalid_encoding_id",
        })
    }
}

/// One numeric code point mapped to an NFC Unicode string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncodingEntry {
    /// Numeric code point or legacy byte value, 0..=0x10FFFF.
    pub code: u32,
    /// NFC string; normalized at import with `okkhor_core::normalize_nfc`.
    pub unicode: String,
}

/// An encoding table: an ordered map from numeric codes to NFC strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncodingTable {
    pub id: EncodingId,
    /// Sorted by `code`; no duplicate codes.
    pub entries: Vec<EncodingEntry>,
}

impl EncodingTable {
    /// Binary search over sorted `entries`. Performs no I/O.
    pub fn lookup(&self, code: u32) -> Option<&str> {
        self.entries
            .binary_search_by(|entry| entry.code.cmp(&code))
            .ok()
            .map(|index| self.entries[index].unicode.as_str())
    }
}

/// One dictionary word with its corpus frequency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictionaryEntry {
    /// Normalized with `okkhor_core::normalize_lookup` at import.
    pub word: String,
    pub frequency: u32,
}

/// A word dictionary for one language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dictionary {
    pub id: String,
    pub language: Language,
    /// Sorted by normalized `word`; no duplicates.
    pub entries: Vec<DictionaryEntry>,
}

impl Dictionary {
    /// Normalizes `word` internally with `okkhor_core::normalize_lookup`;
    /// the importer applies the same normalization to stored words, so
    /// callers pass the raw lookup string. Performs no I/O.
    pub fn get(&self, word: &str) -> Option<&DictionaryEntry> {
        let normalized = okkhor_core::normalize_lookup(word);
        self.entries
            .binary_search_by(|entry| entry.word.as_str().cmp(&normalized))
            .ok()
            .map(|index| &self.entries[index])
    }
}

/// Everything imported from one fixture directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedAssets {
    pub layouts: BTreeMap<LayoutId, Layout>,
    pub encodings: BTreeMap<EncodingId, EncodingTable>,
    pub dictionaries: BTreeMap<String, Dictionary>,
}

impl ImportedAssets {
    pub fn is_empty(&self) -> bool {
        self.layouts.is_empty() && self.encodings.is_empty() && self.dictionaries.is_empty()
    }

    /// Total number of mappings, encoding entries, and dictionary entries.
    pub fn total_entries(&self) -> usize {
        self.layouts
            .values()
            .map(|layout| layout.mappings.len())
            .sum::<usize>()
            + self
                .encodings
                .values()
                .map(|table| table.entries.len())
                .sum::<usize>()
            + self
                .dictionaries
                .values()
                .map(|dictionary| dictionary.entries.len())
                .sum::<usize>()
    }
}

/// Why one manifest asset was skipped (non-strict import).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    UnsupportedKind,
    UnsupportedSchema,
    InvalidId,
    DuplicateId,
    LimitExceeded,
    MalformedData,
    PathRejected,
}

/// One asset that was skipped during a non-strict import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsupportedAsset {
    pub kind: AssetKind,
    /// Logical name from the manifest, never a full path.
    pub name: String,
    pub reason: SkipReason,
}

/// Counts and per-asset skip details for one import run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    pub imported: u32,
    pub skipped: u32,
    pub unsupported: Vec<UnsupportedAsset>,
}

/// One successfully imported asset file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportedAsset {
    Layout(Layout),
    Encoding(EncodingTable),
    Dictionary(Dictionary),
}

#[cfg(test)]
mod tests {
    use super::{EncodingId, LayoutId};

    #[test]
    fn layout_id_validation() {
        assert_eq!(LayoutId::from_code("avro").unwrap().as_str(), "avro");
        assert!(LayoutId::from_code("avro").unwrap().is_known());
        assert!(!LayoutId::from_code("custom-layout_2").unwrap().is_known());
        assert_eq!(LayoutId::from_code(""), None);
        assert_eq!(LayoutId::from_code("Avro"), None);
        assert_eq!(LayoutId::from_code("a b"), None);
        assert_eq!(LayoutId::from_code("a".repeat(33).as_str()), None);
        assert!(LayoutId::from_code("a".repeat(32).as_str()).is_some());
    }

    #[test]
    fn encoding_id_validation() {
        assert!(EncodingId::from_code("synthetic_ascii").is_some());
        assert_eq!(EncodingId::from_code("UPPER"), None);
        assert_eq!(EncodingId::from_code(""), None);
    }
}
