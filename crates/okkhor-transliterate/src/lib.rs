//! Pure transliteration rule engine and normalization helpers.
//!
//! This crate depends only on [`okkhor_core`]. It performs no I/O, no FFI, and
//! no regex matching. Rule data is parsed from synthetic JSON fixtures with
//! [`RuleSet::from_json_str`], validated and sorted by [`RuleTransliterator::new`],
//! and matched literally by [`TransliteratorEngine::transliterate`].
//!
//! Full phonetic/conjunct behavior is a later milestone.
//! TODO(Task 2, milestone 0.3/0.4): replace literal prefix matching with the
//! phonetic engine once the golden corpus lands.

#![forbid(unsafe_code)]

pub mod engine;
pub mod error;
pub mod normalize;
pub mod phonetic;
pub mod rules;

pub use engine::{
    RuleTransliterator, TransliterationOptions, TransliterationOutput, TransliteratorEngine,
};
pub use error::TransliterateError;
pub use normalize::{is_bengali_script, normalize_bengali, normalize_key, normalize_roman};
pub use phonetic::{MAX_PHONETIC_INPUT_CHARS, MAX_PHONETIC_OUTPUT_CHARS, PhoneticTransliterator};
pub use rules::{RULE_SCHEMA, RuleId, RuleSet, TransliterationRule};
