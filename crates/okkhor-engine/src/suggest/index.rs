//! Zero-copy reader for the committed `OKS1` suggestion index.
//!
//! The blob is embedded at compile time and validated once at startup. Every
//! lookup is a slice read plus UTF-8 decoding; there is no I/O on the query
//! path and no `unsafe`.

use std::str;

const RAW: &[u8] = include_bytes!("../../assets/suggest/suggest-v1.bin");
const MAGIC: &[u8; 4] = b"OKS1";
const VERSION: u16 = 2;
const SECTION_COUNT: usize = 10;
const HEADER_LEN: usize = 12;
const TABLE_ENTRY: usize = 16;
const BN_RECORD: usize = 8;
const PAIR_RECORD: usize = 12;
const ROMAN_RECORD: usize = 13;
const POP_RECORD: usize = 4;
/// Every target of one alias is its own row; the importer caps a group at 16.
const MAX_ROMAN_RANK_ROWS: usize = 16;

const SECTION_POOL: usize = 1;
const SECTION_CURATED: usize = 2;
const SECTION_BN: usize = 3;
const SECTION_BN_POP: usize = 4;
const SECTION_AUTOC: usize = 5;
const SECTION_SUFFIX: usize = 6;
const SECTION_EN: usize = 7;
const SECTION_EN_POP: usize = 8;
const SECTION_META: usize = 9;
const SECTION_ROMAN: usize = 10;

/// Index parse/validation failures. Messages are static; they never include
/// dictionary content.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IndexError {
    #[error("suggestion index is truncated")]
    Truncated,
    #[error("suggestion index magic is unsupported")]
    BadMagic,
    #[error("suggestion index version is unsupported (this build reads OKS1 v2)")]
    BadVersion,
    #[error("suggestion index section table is invalid")]
    BadSection,
    #[error("suggestion index contains invalid UTF-8")]
    BadText,
    #[error("suggestion index is not sorted")]
    BadOrder,
}

/// Counts reported by the index, useful for startup logs and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IndexCounts {
    pub bangla: usize,
    pub english: usize,
    pub curated: usize,
    pub autocorrect: usize,
    pub suffix: usize,
    pub roman: usize,
}

#[derive(Debug, Clone, Copy)]
struct Section {
    offset: usize,
    len: usize,
    count: usize,
}

/// Parsed, read-only suggestion dictionary.
#[derive(Debug)]
pub struct SuggestionIndex {
    pool: &'static [u8],
    curated: &'static [u8],
    bn: &'static [u8],
    bn_pop: &'static [u8],
    autoc: &'static [u8],
    suffix: &'static [u8],
    en: &'static [u8],
    en_pop: &'static [u8],
    roman: &'static [u8],
    meta: &'static [u8],
    counts: IndexCounts,
}

impl SuggestionIndex {
    /// Parse the embedded index. Validation walks every string once at
    /// startup; failures are static and never panic.
    pub fn from_embedded() -> Result<Self, IndexError> {
        Self::parse(RAW)
    }

    /// Parse and validate an arbitrary blob (tests and tooling).
    pub fn parse(raw: &'static [u8]) -> Result<Self, IndexError> {
        if raw.len() < HEADER_LEN + SECTION_COUNT * TABLE_ENTRY {
            return Err(IndexError::Truncated);
        }
        if &raw[0..4] != MAGIC {
            return Err(IndexError::BadMagic);
        }
        let version = u16_at(raw, 4).ok_or(IndexError::Truncated)?;
        let sections = usize::from(u16_at(raw, 6).ok_or(IndexError::Truncated)?);
        if version != VERSION {
            return Err(IndexError::BadVersion);
        }
        if sections != SECTION_COUNT {
            return Err(IndexError::BadSection);
        }

        let mut table = [Section {
            offset: 0,
            len: 0,
            count: 0,
        }; SECTION_COUNT];
        let minimum = HEADER_LEN + SECTION_COUNT * TABLE_ENTRY;
        for index in 0..SECTION_COUNT {
            let base = HEADER_LEN + index * TABLE_ENTRY;
            let id = u32_at(raw, base).ok_or(IndexError::Truncated)? as usize;
            let offset = u32_at(raw, base + 4).ok_or(IndexError::Truncated)? as usize;
            let len = u32_at(raw, base + 8).ok_or(IndexError::Truncated)? as usize;
            let count = u32_at(raw, base + 12).ok_or(IndexError::Truncated)? as usize;
            if !(SECTION_POOL..=SECTION_ROMAN).contains(&id) {
                return Err(IndexError::BadSection);
            }
            if offset < minimum || offset.saturating_add(len) > raw.len() {
                return Err(IndexError::BadSection);
            }
            table[id - 1] = Section { offset, len, count };
        }

        let pool = slice(raw, table[SECTION_POOL - 1])?;
        let curated = slice(raw, table[SECTION_CURATED - 1])?;
        let bn = slice(raw, table[SECTION_BN - 1])?;
        let bn_pop = slice(raw, table[SECTION_BN_POP - 1])?;
        let autoc = slice(raw, table[SECTION_AUTOC - 1])?;
        let suffix = slice(raw, table[SECTION_SUFFIX - 1])?;
        let en = slice(raw, table[SECTION_EN - 1])?;
        let en_pop = slice(raw, table[SECTION_EN_POP - 1])?;
        let roman = slice(raw, table[SECTION_ROMAN - 1])?;
        let meta = slice(raw, table[SECTION_META - 1])?;

        let counts = IndexCounts {
            bangla: table[SECTION_BN - 1].count,
            english: table[SECTION_EN - 1].count,
            curated: table[SECTION_CURATED - 1].count,
            autocorrect: table[SECTION_AUTOC - 1].count,
            suffix: table[SECTION_SUFFIX - 1].count,
            roman: table[SECTION_ROMAN - 1].count,
        };
        expect_len(bn, counts.bangla, BN_RECORD)?;
        expect_len(en, counts.english, BN_RECORD)?;
        expect_len(bn_pop, counts.bangla, POP_RECORD)?;
        expect_len(en_pop, counts.english, POP_RECORD)?;
        expect_len(curated, counts.curated, PAIR_RECORD)?;
        expect_len(autoc, counts.autocorrect, PAIR_RECORD)?;
        expect_len(suffix, counts.suffix, PAIR_RECORD)?;
        expect_len(roman, counts.roman, ROMAN_RECORD)?;
        if table[SECTION_META - 1].count != 1 {
            return Err(IndexError::BadSection);
        }
        str::from_utf8(meta).map_err(|_| IndexError::BadText)?;

        let index = Self {
            pool,
            curated,
            bn,
            bn_pop,
            autoc,
            suffix,
            en,
            en_pop,
            roman,
            meta,
            counts,
        };
        index.validate_text()?;
        index.validate_order()?;
        index.validate_popularity()?;
        Ok(index)
    }

    pub fn counts(&self) -> IndexCounts {
        self.counts
    }

    pub fn meta_json(&self) -> &'static str {
        str::from_utf8(self.meta).unwrap_or("{}")
    }

    fn validate_text(&self) -> Result<(), IndexError> {
        for index in 0..self.counts.curated {
            self.curated_pair(index)?;
        }
        for index in 0..self.counts.autocorrect {
            self.autoc_pair(index)?;
        }
        for index in 0..self.counts.suffix {
            self.suffix_pair(index)?;
        }
        for index in 0..self.counts.roman {
            let (key, rank, _value) = self.roman_row(index)?;
            if !valid_roman_key(key) || usize::from(rank) >= MAX_ROMAN_RANK_ROWS {
                return Err(IndexError::BadSection);
            }
        }
        for index in 0..self.counts.bangla {
            self.bn_text(index)?;
        }
        for index in 0..self.counts.english {
            self.en_text(index)?;
        }
        Ok(())
    }

    fn validate_order(&self) -> Result<(), IndexError> {
        let mut previous: Option<&str> = None;
        for index in 0..self.counts.bangla {
            let text = self.bn_text(index)?;
            if let Some(previous) = previous {
                if previous.as_bytes() >= text.as_bytes() {
                    return Err(IndexError::BadOrder);
                }
            }
            previous = Some(text);
        }
        let mut previous: Option<(&str, u8)> = None;
        for index in 0..self.counts.roman {
            let (key, rank, _value) = self.roman_row(index)?;
            if let Some((previous_key, previous_rank)) = previous {
                let key_order = previous_key.as_bytes().cmp(key.as_bytes());
                let ordered = match key_order {
                    std::cmp::Ordering::Less => true,
                    std::cmp::Ordering::Equal => previous_rank < rank,
                    std::cmp::Ordering::Greater => false,
                };
                if !ordered {
                    return Err(IndexError::BadOrder);
                }
            }
            previous = Some((key, rank));
        }
        let mut previous: Option<&str> = None;
        for index in 0..self.counts.english {
            let text = self.en_text(index)?;
            if let Some(previous) = previous {
                if previous.as_bytes() >= text.as_bytes() {
                    return Err(IndexError::BadOrder);
                }
            }
            previous = Some(text);
        }
        Ok(())
    }

    fn validate_popularity(&self) -> Result<(), IndexError> {
        for position in 0..self.counts.bangla {
            let index = self.bn_pop_index(position)?;
            if index >= self.counts.bangla {
                return Err(IndexError::BadSection);
            }
        }
        for position in 0..self.counts.english {
            let index = self.en_pop_index(position)?;
            if index >= self.counts.english {
                return Err(IndexError::BadSection);
            }
        }
        Ok(())
    }

    /// Exact roman→Bangla curated correction (the 0.5 starter list).
    pub fn curated_exact(&self, roman: &str) -> Option<&str> {
        for index in 0..self.counts.curated {
            let (key, value) = self.curated_pair(index).ok()?;
            if key == roman {
                return Some(value);
            }
        }
        None
    }

    /// Curated roman prefix completions, in file order (priority order).
    pub fn curated_prefix(&self, roman: &str, max: usize) -> Vec<(&str, &str)> {
        let mut out = Vec::new();
        for index in 0..self.counts.curated {
            if out.len() >= max {
                break;
            }
            let Ok((key, value)) = self.curated_pair(index) else {
                continue;
            };
            if key != roman && key.starts_with(roman) {
                out.push((key, value));
            }
        }
        out
    }

    /// Exact Bangla word lookup.
    pub fn exact_bangla(&self, word: &str) -> Option<(usize, u16)> {
        let mut low = 0usize;
        let mut high = self.counts.bangla;
        while low < high {
            let middle = low + (high - low) / 2;
            let text = self.bn_text(middle).ok()?;
            match text.as_bytes().cmp(word.as_bytes()) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => {
                    return Some((middle, self.bn_zipf(middle).ok()?));
                }
            }
        }
        None
    }

    /// Bangla prefix completions in popularity order. A common prefix fills
    /// the rows from the first few thousand entries; a rare prefix falls back
    /// to the text-sorted range when the scan budget expires.
    pub fn prefix_bangla(&self, prefix: &str, max: usize, scan_budget: usize) -> Vec<(usize, u16)> {
        self.prefix_bangla_text(prefix, max, scan_budget)
    }

    fn prefix_bangla_text(
        &self,
        prefix: &str,
        max: usize,
        scan_budget: usize,
    ) -> Vec<(usize, u16)> {
        let mut low = 0usize;
        let mut high = self.counts.bangla;
        while low < high {
            let middle = low + (high - low) / 2;
            let Some(text) = self.bn_text(middle).ok() else {
                break;
            };
            if text.as_bytes() < prefix.as_bytes() {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let mut out = Vec::new();
        let mut scanned = 0usize;
        while low < self.counts.bangla && out.len() < max && scanned < scan_budget {
            let Ok(text) = self.bn_text(low) else {
                break;
            };
            if !text.starts_with(prefix) {
                break;
            }
            if let Ok(zipf) = self.bn_zipf(low) {
                out.push((low, zipf));
            }
            low += 1;
            scanned += 1;
        }
        out
    }

    /// English prefix completions in popularity order, with the same
    /// text-range fallback as [`Self::prefix_bangla`].
    pub fn prefix_english(
        &self,
        prefix: &str,
        max: usize,
        scan_budget: usize,
    ) -> Vec<(usize, u16)> {
        self.prefix_english_text(prefix, max, scan_budget)
    }

    fn prefix_english_text(
        &self,
        prefix: &str,
        max: usize,
        scan_budget: usize,
    ) -> Vec<(usize, u16)> {
        let mut low = 0usize;
        let mut high = self.counts.english;
        while low < high {
            let middle = low + (high - low) / 2;
            let Some(text) = self.en_text(middle).ok() else {
                break;
            };
            if text.as_bytes() < prefix.as_bytes() {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let mut out = Vec::new();
        let mut scanned = 0usize;
        while low < self.counts.english && out.len() < max && scanned < scan_budget {
            let Ok(text) = self.en_text(low) else {
                break;
            };
            if !text.starts_with(prefix) {
                break;
            }
            if let Ok(zipf) = self.en_zipf(low) {
                out.push((low, zipf));
            }
            low += 1;
            scanned += 1;
        }
        out
    }

    /// Autocorrect exact match, case-sensitive first then ASCII-insensitive.
    pub fn autocorrect(&self, roman: &str) -> Option<&str> {
        for index in 0..self.counts.autocorrect {
            let Ok((key, value)) = self.autoc_pair(index) else {
                continue;
            };
            if key == roman {
                return Some(value);
            }
        }
        for index in 0..self.counts.autocorrect {
            let Ok((key, value)) = self.autoc_pair(index) else {
                continue;
            };
            if key.eq_ignore_ascii_case(roman) {
                return Some(value);
            }
        }
        None
    }

    /// Longest roman suffix in the suffix table that `roman` ends with.
    pub fn longest_suffix(&self, roman: &str) -> Option<&str> {
        let mut best: Option<(&str, &str)> = None;
        for index in 0..self.counts.suffix {
            let Ok((key, value)) = self.suffix_pair(index) else {
                continue;
            };
            if roman.ends_with(key) {
                let better = best.is_none_or(|(current, _)| key.len() > current.len());
                if better {
                    best = Some((key, value));
                }
            }
        }
        best.map(|(_, value)| value)
    }

    /// Preferred roman→Bangla target (rank 0) from the imported Nms
    /// phonetic dictionary.
    ///
    /// Live staging keeps its 0.9 ordering until Phase 3 adds the bounded
    /// preferred-rank signal; [`Self::exact_roman_targets`] exposes every
    /// target of the v2 index now.
    pub fn exact_roman(&self, key: &str) -> Vec<&str> {
        let mut targets = self.exact_roman_targets(key);
        targets.truncate(1);
        targets
    }

    /// Every target of an exact roman key, in preferred-rank order (rank 0
    /// first). A multi-target alias contributes one row per target.
    pub fn exact_roman_targets(&self, key: &str) -> Vec<&str> {
        let mut low = 0usize;
        let mut high = self.counts.roman;
        while low < high {
            let middle = low + (high - low) / 2;
            let Some((candidate, _, _)) = self.roman_row(middle).ok() else {
                break;
            };
            if candidate.as_bytes() < key.as_bytes() {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let mut out = Vec::new();
        while low < self.counts.roman && out.len() < MAX_ROMAN_RANK_ROWS {
            let Ok((candidate, _rank, value)) = self.roman_row(low) else {
                break;
            };
            if candidate != key {
                break;
            }
            out.push(value);
            low += 1;
        }
        out
    }

    /// Roman prefix completions from the imported phonetic dictionary, in
    /// (key bytes, rank) order.
    pub fn prefix_roman(&self, key: &str, max: usize, scan_budget: usize) -> Vec<(&str, &str)> {
        let mut low = 0usize;
        let mut high = self.counts.roman;
        while low < high {
            let middle = low + (high - low) / 2;
            let Some((candidate, _, _)) = self.roman_row(middle).ok() else {
                break;
            };
            if candidate.as_bytes() < key.as_bytes() {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let mut out = Vec::new();
        let mut scanned = 0usize;
        while low < self.counts.roman && out.len() < max && scanned < scan_budget {
            let Ok((candidate, _rank, value)) = self.roman_row(low) else {
                break;
            };
            if !candidate.starts_with(key) {
                break;
            }
            out.push((candidate, value));
            low += 1;
            scanned += 1;
        }
        out
    }

    pub fn bangla_text(&self, index: usize) -> Option<&str> {
        self.bn_text(index).ok()
    }

    pub fn bangla_zipf(&self, index: usize) -> Option<u16> {
        self.bn_zipf(index).ok()
    }

    pub fn english_text(&self, index: usize) -> Option<&str> {
        self.en_text(index).ok()
    }

    fn curated_pair(&self, index: usize) -> Result<(&str, &str), IndexError> {
        let base = index * PAIR_RECORD;
        let key_offset = u32_at(self.curated, base).ok_or(IndexError::Truncated)? as usize;
        let key_len = u16_at(self.curated, base + 4).ok_or(IndexError::Truncated)? as usize;
        let value_offset = u32_at(self.curated, base + 6).ok_or(IndexError::Truncated)? as usize;
        let value_len = u16_at(self.curated, base + 10).ok_or(IndexError::Truncated)? as usize;
        Ok((
            self.text(key_offset, key_len)?,
            self.text(value_offset, value_len)?,
        ))
    }

    fn autoc_pair(&self, index: usize) -> Result<(&str, &str), IndexError> {
        let base = index * PAIR_RECORD;
        let key_offset = u32_at(self.autoc, base).ok_or(IndexError::Truncated)? as usize;
        let key_len = u16_at(self.autoc, base + 4).ok_or(IndexError::Truncated)? as usize;
        let value_offset = u32_at(self.autoc, base + 6).ok_or(IndexError::Truncated)? as usize;
        let value_len = u16_at(self.autoc, base + 10).ok_or(IndexError::Truncated)? as usize;
        Ok((
            self.text(key_offset, key_len)?,
            self.text(value_offset, value_len)?,
        ))
    }

    fn suffix_pair(&self, index: usize) -> Result<(&str, &str), IndexError> {
        let base = index * PAIR_RECORD;
        let key_offset = u32_at(self.suffix, base).ok_or(IndexError::Truncated)? as usize;
        let key_len = u16_at(self.suffix, base + 4).ok_or(IndexError::Truncated)? as usize;
        let value_offset = u32_at(self.suffix, base + 6).ok_or(IndexError::Truncated)? as usize;
        let value_len = u16_at(self.suffix, base + 10).ok_or(IndexError::Truncated)? as usize;
        Ok((
            self.text(key_offset, key_len)?,
            self.text(value_offset, value_len)?,
        ))
    }

    fn roman_row(&self, index: usize) -> Result<(&str, u8, &str), IndexError> {
        let base = index * ROMAN_RECORD;
        let key_offset = u32_at(self.roman, base).ok_or(IndexError::Truncated)? as usize;
        let key_len = u16_at(self.roman, base + 4).ok_or(IndexError::Truncated)? as usize;
        let rank = *self.roman.get(base + 6).ok_or(IndexError::Truncated)?;
        let value_len = u16_at(self.roman, base + 7).ok_or(IndexError::Truncated)? as usize;
        let value_offset = u32_at(self.roman, base + 9).ok_or(IndexError::Truncated)? as usize;
        Ok((
            self.text(key_offset, key_len)?,
            rank,
            self.text(value_offset, value_len)?,
        ))
    }

    fn bn_text(&self, index: usize) -> Result<&str, IndexError> {
        let base = index * BN_RECORD;
        let offset = u32_at(self.bn, base).ok_or(IndexError::Truncated)? as usize;
        let len = u16_at(self.bn, base + 4).ok_or(IndexError::Truncated)? as usize;
        self.text(offset, len)
    }

    fn bn_zipf(&self, index: usize) -> Result<u16, IndexError> {
        let base = index * BN_RECORD;
        u16_at(self.bn, base + 6).ok_or(IndexError::Truncated)
    }

    fn bn_pop_index(&self, position: usize) -> Result<usize, IndexError> {
        u32_at(self.bn_pop, position * POP_RECORD)
            .map(|value| value as usize)
            .ok_or(IndexError::Truncated)
    }

    fn en_text(&self, index: usize) -> Result<&str, IndexError> {
        let base = index * BN_RECORD;
        let offset = u32_at(self.en, base).ok_or(IndexError::Truncated)? as usize;
        let len = u16_at(self.en, base + 4).ok_or(IndexError::Truncated)? as usize;
        self.text(offset, len)
    }

    fn en_zipf(&self, index: usize) -> Result<u16, IndexError> {
        let base = index * BN_RECORD;
        u16_at(self.en, base + 6).ok_or(IndexError::Truncated)
    }

    fn en_pop_index(&self, position: usize) -> Result<usize, IndexError> {
        u32_at(self.en_pop, position * POP_RECORD)
            .map(|value| value as usize)
            .ok_or(IndexError::Truncated)
    }

    fn text(&self, offset: usize, len: usize) -> Result<&str, IndexError> {
        let bytes = self
            .pool
            .get(offset..offset.saturating_add(len))
            .ok_or(IndexError::BadSection)?;
        str::from_utf8(bytes).map_err(|_| IndexError::BadText)
    }
}

fn valid_roman_key(key: &str) -> bool {
    (1..=24).contains(&key.len()) && key.bytes().all(|byte| byte.is_ascii_lowercase())
}

fn slice(raw: &'static [u8], section: Section) -> Result<&'static [u8], IndexError> {
    raw.get(section.offset..section.offset + section.len)
        .ok_or(IndexError::BadSection)
}

fn expect_len(section: &[u8], count: usize, record: usize) -> Result<(), IndexError> {
    if count.saturating_mul(record) != section.len() {
        return Err(IndexError::BadSection);
    }
    Ok(())
}

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

#[cfg(test)]
mod tests {
    use super::{IndexError, SuggestionIndex};

    #[test]
    fn embedded_index_validates() {
        let index = SuggestionIndex::from_embedded().expect("embedded index");
        let counts = index.counts();
        assert!(counts.bangla > 150_000, "bangla {}", counts.bangla);
        assert!(counts.english > 40_000, "english {}", counts.english);
        assert!(counts.curated > 200);
        assert!(counts.autocorrect > 1_000);
        assert!(counts.suffix > 700);
        assert!(counts.roman >= 151_000, "roman {}", counts.roman);
    }

    #[test]
    fn v1_index_is_rejected_as_bad_version() {
        let mut raw = vec![0u8; 256];
        raw[0..4].copy_from_slice(b"OKS1");
        raw[4..6].copy_from_slice(&1u16.to_le_bytes());
        raw[6..8].copy_from_slice(&10u16.to_le_bytes());
        let leaked: &'static [u8] = Box::leak(raw.into_boxed_slice());
        assert_eq!(
            SuggestionIndex::parse(leaked).unwrap_err(),
            IndexError::BadVersion
        );
    }

    #[test]
    fn roman_rows_keep_every_target_in_rank_order() {
        let index = SuggestionIndex::from_embedded().expect("embedded index");
        let bari = index.exact_roman_targets("bari");
        assert!(bari.len() >= 4, "bari rows {bari:?}");
        assert_eq!(bari.first().copied(), Some("বাড়ি"));
        assert_eq!(index.exact_roman("bari"), vec!["বাড়ি"]);
        assert_eq!(index.exact_roman_targets("cha").first().copied(), Some("ছা"));
        assert_eq!(
            index.exact_roman_targets("bon").first().copied(),
            Some("বন")
        );
    }

    #[test]
    fn known_words_and_prefixes() {
        let index = SuggestionIndex::from_embedded().expect("embedded index");
        assert!(index.exact_bangla("আমার").is_some());
        assert!(index.exact_bangla("ভালো").is_some());
        assert!(index.exact_bangla("ধন্যবাদ").is_some());
        assert!(index.exact_bangla("কম্পিউটার").is_some());
        let hits = index.prefix_bangla("ভাল", 400, 5_000);
        assert!(
            hits.iter()
                .any(|(i, _)| index.bangla_text(*i) == Some("ভালো"))
        );
    }

    #[test]
    fn curated_and_autocorrect() {
        let index = SuggestionIndex::from_embedded().expect("embedded index");
        assert_eq!(index.curated_exact("dhonnobad"), Some("ধন্যবাদ"));
        assert_eq!(index.curated_exact("korcho"), Some("করছো"));
        assert_eq!(index.autocorrect("abdomen"), Some("oZabDOmen"));
    }

    #[test]
    fn nms_roman_pairs_cover_rule_gaps() {
        let index = SuggestionIndex::from_embedded().expect("embedded index");
        for (key, value) in [
            ("ghor", "ঘর"),
            ("mangsho", "মাংস"),
            ("koro", "করো"),
            ("korechi", "করেছি"),
            ("jabo", "যাবো"),
            ("hae", "হ্যাঁ"),
        ] {
            assert!(
                index.exact_roman(key).contains(&value),
                "{key} -> {:?}",
                index.exact_roman(key)
            );
        }
        let hits = index.prefix_roman("bhal", 200, 5_000);
        assert!(hits.iter().any(|(_, value)| *value == "ভালো"));
    }

    #[test]
    fn bad_magic_fails_closed() {
        static BAD: &[u8] = b"NOPE0000000000000000000000000000";
        assert!(SuggestionIndex::parse(BAD).is_err());
    }
}
