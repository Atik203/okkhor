use std::collections::HashSet;

use okkhor_core::{Candidate, CandidateSource, Language, normalize_nfc};
use serde::{Deserialize, Serialize};

use crate::error::TransliterateError;
use crate::normalize::normalize_key;
use crate::rules::{RuleId, RuleSet};

/// Options controlling one transliteration call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransliterationOptions {
    /// Hard cap on returned outputs; default 8.
    pub max_candidates: usize,
    /// Emit a candidate when only a prefix of a rule matched; default true.
    pub allow_partial: bool,
}

impl Default for TransliterationOptions {
    fn default() -> Self {
        Self {
            max_candidates: 8,
            allow_partial: true,
        }
    }
}

/// One transliteration result before ranking.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransliterationOutput {
    pub text: String,
    pub language: Language,
    /// 0.0..=1.0; derived from rule priority only, never from user data.
    pub confidence: f32,
    pub rule_id: RuleId,
}

impl TransliterationOutput {
    /// Bridge into the existing shared candidate/ranking path.
    ///
    /// Sets `source = CandidateSource::Transliteration` and
    /// `features.phonetic_score = confidence`.
    pub fn into_candidate(self) -> Candidate {
        let mut candidate =
            Candidate::new(self.text, self.language, CandidateSource::Transliteration);
        candidate.features.phonetic_score = self.confidence;
        candidate
    }
}

/// Pure, deterministic transliteration over an immutable rule view.
pub trait TransliteratorEngine {
    /// Transliterate `input` with no I/O. Outputs are ordered by match quality.
    ///
    /// Infallible: overlong input yields an empty `Vec`, never an error.
    fn transliterate(
        &self,
        input: &str,
        options: &TransliterationOptions,
    ) -> Vec<TransliterationOutput>;
}

/// A single rule after the normalization, validation, and sorting owned by
/// [`RuleTransliterator::new`].
#[derive(Debug, Clone)]
struct EngineRule {
    id: RuleId,
    /// Normalized lookup key: 1..=32 ASCII bytes.
    input: String,
    /// NFC-validated output: 1..=64 chars.
    output: String,
    language: Language,
    priority: u16,
    confidence: f32,
}

/// Confidence derived from rule priority only: lower priority wins and scores
/// closer to 1.0. Always in (0.0, 1.0].
fn confidence_for(priority: u16) -> f32 {
    1.0 / (1.0 + f32::from(priority) / 100.0)
}

/// Literal-prefix transliteration engine (scaffold for 0.3).
///
/// Full phonetic/conjunct behavior is a later milestone.
/// TODO(Task 2, milestone 0.3/0.4): extend matching beyond literal prefixes.
#[derive(Debug, Clone)]
pub struct RuleTransliterator {
    /// Immutable rules in engine order: `input` length descending, `priority`
    /// ascending, `id` ascending.
    rules: Vec<EngineRule>,
}

impl RuleTransliterator {
    /// Validate, normalize, and sort `rules` into the immutable engine view.
    ///
    /// Owns: `normalize_key` on each `input`; the explicit ASCII-only check
    /// (non-ASCII input is `InvalidRule { reason: "non_ascii_input" }`); the
    /// NFC check and 1..=64 char check on `output`; the
    /// `language == Language::Bangla` check; duplicate normalized `id`
    /// (`DuplicateRuleId`); duplicate normalized `input` under a different id
    /// (`InvalidRule { reason: "duplicate_rule_input" }`); and sorting.
    /// `transliterate` sees only the resulting view. This is the only step
    /// that returns [`TransliterateError`].
    pub fn new(rules: RuleSet) -> Result<Self, TransliterateError> {
        let mut seen_ids: HashSet<&str> = HashSet::new();
        let mut seen_inputs: HashSet<String> = HashSet::new();
        let mut engine_rules: Vec<EngineRule> = Vec::with_capacity(rules.rules.len());

        for rule in &rules.rules {
            if crate::rules::RuleId::from_code(rule.id.as_str()).is_none() {
                return Err(TransliterateError::InvalidRule {
                    reason: "invalid_rule_id",
                });
            }
            let input = normalize_key(&rule.input);
            if input.is_empty() {
                return Err(TransliterateError::InvalidRule {
                    reason: "empty_rule_input",
                });
            }
            if !input.is_ascii() {
                return Err(TransliterateError::InvalidRule {
                    reason: "non_ascii_input",
                });
            }
            if input.len() > 32 {
                return Err(TransliterateError::InputTooLong);
            }
            if normalize_nfc(&rule.output) != rule.output {
                return Err(TransliterateError::InvalidRule {
                    reason: "non_nfc_output",
                });
            }
            if rule.output.chars().count() == 0 || rule.output.chars().count() > 64 {
                return Err(TransliterateError::InvalidRule {
                    reason: "invalid_output_length",
                });
            }
            if rule.language != Language::Bangla {
                return Err(TransliterateError::UnsupportedLanguage);
            }
            if !seen_ids.insert(rule.id.as_str()) {
                return Err(TransliterateError::DuplicateRuleId);
            }
            if !seen_inputs.insert(input.clone()) {
                return Err(TransliterateError::InvalidRule {
                    reason: "duplicate_rule_input",
                });
            }
            engine_rules.push(EngineRule {
                id: rule.id.clone(),
                input,
                output: rule.output.clone(),
                language: rule.language,
                priority: rule.priority,
                confidence: confidence_for(rule.priority),
            });
        }

        engine_rules.sort_by(|a, b| {
            b.input
                .len()
                .cmp(&a.input.len())
                .then_with(|| a.priority.cmp(&b.priority))
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });

        Ok(Self {
            rules: engine_rules,
        })
    }

    /// Number of validated rules in engine order.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

impl TransliteratorEngine for RuleTransliterator {
    fn transliterate(
        &self,
        input: &str,
        options: &TransliterationOptions,
    ) -> Vec<TransliterationOutput> {
        // Bounded-input guard: >=256-byte input yields no candidates. This
        // method is infallible: no error is constructed and no panic occurs.
        if input.len() >= 256 {
            return Vec::new();
        }
        let key = normalize_key(input);
        if key.is_empty() || key.len() >= 256 {
            return Vec::new();
        }

        let mut outputs: Vec<TransliterationOutput> = Vec::new();
        let mut seen_texts: HashSet<&str> = HashSet::new();

        // Full matches first (the rule input is a prefix of the typed key),
        // then partial matches (the typed key is a proper prefix of the rule
        // input). Each group follows engine order; identical outputs keep the
        // best (first) rule.
        for pass in 0..2 {
            if pass == 1 && !options.allow_partial {
                break;
            }
            for rule in &self.rules {
                let matched = if pass == 0 {
                    key.starts_with(rule.input.as_str())
                } else {
                    rule.input.starts_with(key.as_str())
                };
                if !matched || !seen_texts.insert(rule.output.as_str()) {
                    continue;
                }
                outputs.push(TransliterationOutput {
                    text: rule.output.clone(),
                    language: rule.language,
                    confidence: rule.confidence,
                    rule_id: rule.id.clone(),
                });
            }
        }

        outputs.truncate(options.max_candidates);
        outputs
    }
}
