//! Optional global next-word model for the ghost hint.
//!
//! The model file is data, not code: a missing or corrupt file leaves the
//! existing bigram + Smriti path untouched. Loading happens on a worker thread
//! because it reads a few hundred megabytes; the typing path only ever sees a
//! ready `Option<&NextWordModel>`.

use std::collections::{HashMap, VecDeque};
use std::path::Path;

use okkhor_core::Language;
use okkhor_engine::nextword::{ModelError, NextWordModel, merge_next_words};
use okkhor_learn::Smriti;

/// Confidence gates from the 0.10 offline sweep: at these levels the hint is
/// right roughly 39 % (Bangla) and 28 % (English) of the time.
///
/// Short context is a special case: after one committed word the model has
/// almost nothing to condition on, so the same gate would hide nearly every
/// hint. The gate is relaxed until the buffer fills.
fn threshold(language: Language, context_len: usize) -> f32 {
    let base = match language {
        Language::Bangla => 0.12,
        Language::English => 0.15,
    };
    match context_len {
        0..=1 => base * 0.35,
        2..=3 => base * 0.7,
        _ => base,
    }
}

/// Models available to the daemon.
#[derive(Default)]
pub struct Models {
    pub bangla: Option<NextWordModel>,
    pub english: Option<NextWordModel>,
}

impl Models {
    /// Load `<dir>/onet-bn-i8.oknm` and `<dir>/onet-en-i8.oknm`.
    ///
    /// Bytes are leaked on purpose: the daemon holds the model for its whole
    /// life, and the reader borrows from the blob.
    pub fn load(dir: &Path) -> (Self, Vec<(&'static str, ModelError)>) {
        let mut models = Self::default();
        let mut failures = Vec::new();
        for (name, language) in [
            ("onet-bn-i8.oknm", Language::Bangla),
            ("onet-en-i8.oknm", Language::English),
        ] {
            let path = dir.join(name);
            if !path.is_file() {
                continue;
            }
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let blob: &'static [u8] = Box::leak(bytes.into_boxed_slice());
                    match NextWordModel::parse(blob) {
                        Ok(model) if model.language() == language => match language {
                            Language::Bangla => models.bangla = Some(model),
                            Language::English => models.english = Some(model),
                        },
                        Ok(_) => failures.push(("model_language", ModelError::BadHeader)),
                        Err(error) => failures.push(("model_parse", error)),
                    }
                }
                Err(_) => failures.push(("model_read", ModelError::Truncated)),
            }
        }
        (models, failures)
    }

    pub fn get(&self, language: Language) -> Option<&NextWordModel> {
        match language {
            Language::Bangla => self.bangla.as_ref(),
            Language::English => self.english.as_ref(),
        }
    }
}

/// The last committed words, newest last.
pub struct Recent {
    words: VecDeque<String>,
    capacity: usize,
}

impl Recent {
    pub fn new(capacity: usize) -> Self {
        Self {
            words: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, word: &str) {
        let word = word.trim();
        if word.is_empty() {
            return;
        }
        if self.words.len() == self.capacity {
            self.words.pop_front();
        }
        self.words.push_back(word.to_owned());
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// Oldest-first context, at most `count` words.
    pub fn context(&self, count: usize) -> Vec<&str> {
        let start = self.words.len().saturating_sub(count);
        self.words.iter().skip(start).map(String::as_str).collect()
    }
}

/// Ghost candidates for one language.
///
/// With a model and some context: model top-50, confidence gate, then a boost
/// for pairs the user has actually typed. Without a model (or before any word
/// is known) the shipped `OKNW` prior and Smriti decide, exactly as before.
pub fn candidates(
    model: Option<&NextWordModel>,
    smriti: &Smriti,
    language: Language,
    recent: &Recent,
    limit: usize,
) -> Vec<String> {
    let previous = recent.context(1).first().copied().unwrap_or("");
    let Some(model) = model else {
        return if previous.is_empty() {
            Vec::new()
        } else {
            merge_next_words(smriti, language, previous, limit)
        };
    };
    if recent.is_empty() {
        return Vec::new();
    }

    let context = recent.context(model.context_len());
    let ids: Vec<u32> = context.iter().map(|word| model.id(word)).collect();
    let ranked = model.predict(&ids, 50);
    let Some((_, top_probability)) = ranked.first() else {
        return Vec::new();
    };
    if *top_probability < threshold(language, context.len()) {
        return Vec::new();
    }

    let local: HashMap<String, u64> = smriti
        .next_words(language, previous, 64)
        .into_iter()
        .collect();
    let mut scored: Vec<(f32, &str)> = ranked
        .iter()
        .filter_map(|(id, probability)| {
            let word = model.word(*id);
            if word.is_empty() || word.starts_with('<') {
                return None;
            }
            let boost = local
                .get(word)
                .map(|count| 0.35 * ((*count as f32) + 1.0).ln())
                .unwrap_or(0.0);
            Some((probability.max(1e-12).ln() + boost, word))
        })
        .collect();
    scored.sort_unstable_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, word)| word.to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_keeps_the_last_words() {
        let mut recent = Recent::new(3);
        recent.push("a");
        recent.push("b");
        recent.push("c");
        recent.push("d");
        assert_eq!(recent.context(3), vec!["b", "c", "d"]);
        assert_eq!(recent.context(1), vec!["d"]);
    }

    #[test]
    fn empty_recent_makes_no_hint() {
        let smriti = Smriti::new();
        let recent = Recent::new(8);
        assert!(candidates(None, &smriti, Language::Bangla, &recent, 5).is_empty());
    }

    #[test]
    fn fallback_uses_the_prior_without_a_model() {
        let smriti = Smriti::new();
        let mut recent = Recent::new(8);
        recent.push("বিধানসভা");
        let words = candidates(None, &smriti, Language::Bangla, &recent, 3);
        assert_eq!(words.first().map(String::as_str), Some("কেন্দ্র"));
    }

    #[test]
    fn missing_model_directory_is_not_an_error() {
        let (models, failures) = Models::load(Path::new("Z:\\definitely-missing"));
        assert!(models.bangla.is_none());
        assert!(failures.is_empty());
    }
}
