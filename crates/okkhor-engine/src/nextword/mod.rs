//! Zero-copy reader for the committed `OKNW` next-word priors.
//!
//! The blob is embedded at compile time and validated once. A lookup is a
//! binary search over the previous-word table plus a slice read; there is no
//! I/O, no allocation for the table itself, and no `unsafe`.
//!
//! The prior is a global, language-specific P(next | previous) table trained on
//! licensed corpora. It never replaces Smriti: the caller merges local pairs
//! first, then the prior, and deduplicates.

use std::sync::OnceLock;

use okkhor_core::Language;
use okkhor_learn::Smriti;

pub mod model;

pub use model::{ModelError, NextWordModel};

const BANGLA: &[u8] = include_bytes!("../../assets/nextword/bangla-bigram-v1.bin");
const ENGLISH: &[u8] = include_bytes!("../../assets/nextword/english-bigram-v1.bin");
const MAGIC: &[u8; 4] = b"OKNW";
const VERSION: u16 = 1;
const HEADER_LEN: usize = 32;
const WORD_RECORD: usize = 8;
const ENTRY_RECORD: usize = 12;

/// Parse failures. Messages are static and never include corpus text.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PriorError {
    #[error("next-word prior is truncated")]
    Truncated,
    #[error("next-word prior magic is unsupported")]
    BadMagic,
    #[error("next-word prior version is unsupported (this build reads OKNW v1)")]
    BadVersion,
    #[error("next-word prior contains invalid UTF-8")]
    BadText,
    #[error("next-word prior is not sorted")]
    BadOrder,
}

/// Parsed, read-only next-word prior for one language.
#[derive(Debug)]
pub struct NextWordPrior {
    language: Language,
    blob: &'static [u8],
    words: usize,
    entries: usize,
    word_off: usize,
    entry_off: usize,
    follow_off: usize,
    follow_count: usize,
    pool_off: usize,
}

impl NextWordPrior {
    /// Parse and validate a static blob. Every record is bounds-checked and
    /// the previous-word table is verified to be strictly sorted.
    pub fn parse(blob: &'static [u8]) -> Result<Self, PriorError> {
        if blob.len() < HEADER_LEN {
            return Err(PriorError::Truncated);
        }
        if &blob[0..4] != MAGIC {
            return Err(PriorError::BadMagic);
        }
        let version = u16_at(blob, 4).ok_or(PriorError::Truncated)?;
        if version != VERSION {
            return Err(PriorError::BadVersion);
        }
        let language = match u16_at(blob, 6).ok_or(PriorError::Truncated)? {
            0 => Language::Bangla,
            1 => Language::English,
            _ => return Err(PriorError::BadMagic),
        };
        let words = u32_at(blob, 8).ok_or(PriorError::Truncated)? as usize;
        let word_off = u32_at(blob, 12).ok_or(PriorError::Truncated)? as usize;
        let entries = u32_at(blob, 16).ok_or(PriorError::Truncated)? as usize;
        let entry_off = u32_at(blob, 20).ok_or(PriorError::Truncated)? as usize;
        let follow_count = u32_at(blob, 24).ok_or(PriorError::Truncated)? as usize;
        let follow_off = u32_at(blob, 28).ok_or(PriorError::Truncated)? as usize;

        let words_end = word_off
            .checked_add(
                words
                    .checked_mul(WORD_RECORD)
                    .ok_or(PriorError::Truncated)?,
            )
            .ok_or(PriorError::Truncated)?;
        let entries_end = entry_off
            .checked_add(
                entries
                    .checked_mul(ENTRY_RECORD)
                    .ok_or(PriorError::Truncated)?,
            )
            .ok_or(PriorError::Truncated)?;
        let follow_end = follow_off
            .checked_add(follow_count.checked_mul(4).ok_or(PriorError::Truncated)?)
            .ok_or(PriorError::Truncated)?;
        if words_end > blob.len() || entries_end > blob.len() || follow_end > blob.len() {
            return Err(PriorError::Truncated);
        }

        let prior = Self {
            language,
            blob,
            words,
            entries,
            word_off,
            entry_off,
            follow_off,
            follow_count,
            pool_off: follow_end,
        };

        let mut previous: Option<&str> = None;
        for index in 0..entries {
            let word = prior.previous(index)?;
            if let Some(last) = previous {
                if last >= word {
                    return Err(PriorError::BadOrder);
                }
            }
            let (start, count) = prior.entry_span(index)?;
            if start + count > follow_count {
                return Err(PriorError::Truncated);
            }
            previous = Some(word);
        }
        Ok(prior)
    }

    pub fn language(&self) -> Language {
        self.language
    }

    /// Number of distinct previous words in the table.
    pub fn previous_count(&self) -> usize {
        self.entries
    }

    /// Number of follower rows in the table.
    pub fn follower_count(&self) -> usize {
        self.follow_count
    }

    /// Previous word at `index`, in table order.
    ///
    /// Tooling and benchmarks use this to sample real keys without copying the
    /// table. Returns `None` for an out-of-range index.
    pub fn previous_at(&self, index: usize) -> Option<&'static str> {
        if index >= self.entries {
            return None;
        }
        self.previous(index).ok()
    }

    /// Followers of `previous`, most frequent first, capped at `limit`.
    ///
    /// Returns an empty vector when the pair was never seen. The returned
    /// strings borrow from the embedded blob.
    pub fn followers(&self, previous: &str, limit: usize) -> Vec<&'static str> {
        if limit == 0 || self.entries == 0 {
            return Vec::new();
        }
        let needle = previous.as_bytes();
        let mut low = 0usize;
        let mut high = self.entries;
        while low < high {
            let mid = low + (high - low) / 2;
            let Ok(candidate) = self.previous(mid) else {
                return Vec::new();
            };
            match candidate.as_bytes().cmp(needle) {
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
                std::cmp::Ordering::Equal => {
                    let Ok((start, count)) = self.entry_span(mid) else {
                        return Vec::new();
                    };
                    let take = count.min(limit);
                    let mut out = Vec::with_capacity(take);
                    for offset in 0..take {
                        if let Ok(word) = self.follower(start + offset) {
                            out.push(word);
                        }
                    }
                    return out;
                }
            }
        }
        Vec::new()
    }

    fn previous(&self, index: usize) -> Result<&'static str, PriorError> {
        let id = u32_at(self.blob, self.entry_off + index * ENTRY_RECORD)
            .ok_or(PriorError::Truncated)? as usize;
        self.word(id)
    }

    fn entry_span(&self, index: usize) -> Result<(usize, usize), PriorError> {
        let base = self.entry_off + index * ENTRY_RECORD;
        let start = u32_at(self.blob, base + 4).ok_or(PriorError::Truncated)? as usize;
        let count = u16_at(self.blob, base + 8).ok_or(PriorError::Truncated)? as usize;
        Ok((start, count))
    }

    fn follower(&self, index: usize) -> Result<&'static str, PriorError> {
        let id =
            u32_at(self.blob, self.follow_off + index * 4).ok_or(PriorError::Truncated)? as usize;
        self.word(id)
    }

    fn word(&self, id: usize) -> Result<&'static str, PriorError> {
        if id >= self.words {
            return Err(PriorError::Truncated);
        }
        let base = self.word_off + id * WORD_RECORD;
        let offset = u32_at(self.blob, base).ok_or(PriorError::Truncated)? as usize;
        let length = u16_at(self.blob, base + 4).ok_or(PriorError::Truncated)? as usize;
        let start = self
            .pool_off
            .checked_add(offset)
            .ok_or(PriorError::Truncated)?;
        let end = start.checked_add(length).ok_or(PriorError::Truncated)?;
        if end > self.blob.len() {
            return Err(PriorError::Truncated);
        }
        std::str::from_utf8(&self.blob[start..end]).map_err(|_| PriorError::BadText)
    }
}

/// Embedded prior for `language`, parsed once on first use.
///
/// A corrupt blob yields `None`; callers fall back to local pairs and typing
/// continues.
pub fn global(language: Language) -> Option<&'static NextWordPrior> {
    static BANGLA_PRIOR: OnceLock<Option<NextWordPrior>> = OnceLock::new();
    static ENGLISH_PRIOR: OnceLock<Option<NextWordPrior>> = OnceLock::new();
    let slot = match language {
        Language::Bangla => &BANGLA_PRIOR,
        Language::English => &ENGLISH_PRIOR,
    };
    slot.get_or_init(|| {
        let blob: &'static [u8] = match language {
            Language::Bangla => BANGLA,
            Language::English => ENGLISH,
        };
        NextWordPrior::parse(blob).ok()
    })
    .as_ref()
}

/// Next-word candidates for `previous`: personal pairs first, then the global
/// prior, de-duplicated and capped at `limit`.
///
/// This is the single merge used by the ghost hint and the benchmark. It runs
/// after a commit on the daemon worker, never in the hook.
pub fn merge_next_words(
    smriti: &Smriti,
    language: Language,
    previous: &str,
    limit: usize,
) -> Vec<String> {
    let mut words: Vec<String> = smriti
        .next_words(language, previous, limit)
        .into_iter()
        .map(|(word, _)| word)
        .collect();
    if words.len() < limit {
        if let Some(prior) = global(language) {
            for follower in prior.followers(previous, limit) {
                if words.iter().any(|existing| existing == follower) {
                    continue;
                }
                words.push(follower.to_owned());
                if words.len() >= limit {
                    break;
                }
            }
        }
    }
    words
}

fn u16_at(raw: &[u8], offset: usize) -> Option<u16> {
    let bytes = raw.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn u32_at(raw: &[u8], offset: usize) -> Option<u32> {
    let bytes = raw.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_priors_parse() {
        for language in [Language::Bangla, Language::English] {
            let prior = global(language).expect("embedded prior parses");
            assert!(prior.previous_count() > 1_000, "{language:?}");
        }
    }

    #[test]
    fn unknown_previous_is_empty() {
        let prior = global(Language::Bangla).expect("prior");
        assert!(prior.followers("zzzz-not-a-bangla-word", 5).is_empty());
    }

    #[test]
    fn english_sample_pair_is_ranked() {
        let prior = global(Language::English).expect("prior");
        let words = prior.followers("By", 3);
        assert_eq!(words.first().copied(), Some("the"));
    }

    #[test]
    fn bangla_sample_pair_is_ranked() {
        let prior = global(Language::Bangla).expect("prior");
        assert_eq!(prior.followers("বিধানসভা", 1), vec!["কেন্দ্র"]);
        assert!(prior.follower_count() > 1_000_000);
    }

    #[test]
    fn bad_magic_is_rejected() {
        let blob = Box::leak(vec![0u8; HEADER_LEN].into_boxed_slice());
        assert_eq!(
            NextWordPrior::parse(blob).unwrap_err(),
            PriorError::BadMagic
        );
    }

    #[test]
    fn truncation_is_rejected() {
        static SHORT: &[u8] = b"OKNW";
        assert_eq!(
            NextWordPrior::parse(SHORT).unwrap_err(),
            PriorError::Truncated
        );
    }
}
