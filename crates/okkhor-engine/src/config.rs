//! Per-language budgets bounding one candidate-generation query.

use okkhor_core::Language;
use serde::{Deserialize, Serialize};

use crate::error::EngineError;

/// Cap for one stage of the pipeline in one language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageBudget {
    /// Max dictionary prefix hits admitted per query. Default 20.
    pub max_prefix_hits: usize,
    /// Max transliteration outputs admitted per query. Default 8 (Bangla) / 2.
    pub max_transliteration_hits: usize,
    /// Max candidates entering the ranker after merge + dedup. Default 24.
    pub max_merged: usize,
    /// Hard ceiling for `SuggestionRequest::limit`. Default 10.
    pub max_limit: usize,
}

impl LanguageBudget {
    /// Select the budget for `language` from `config`.
    pub fn for_language(config: &EngineConfig, language: Language) -> Self {
        match language {
            Language::Bangla => config.bangla,
            Language::English => config.english,
        }
    }

    /// Every field must be at least 1; validated once at construction so the
    /// query path never checks for zero.
    pub(crate) fn is_valid(&self) -> bool {
        self.max_prefix_hits >= 1
            && self.max_transliteration_hits >= 1
            && self.max_merged >= 1
            && self.max_limit >= 1
    }
}

/// Per-language budgets for [`CandidateEngine`](crate::engine::CandidateEngine).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineConfig {
    pub bangla: LanguageBudget,
    pub english: LanguageBudget,
}

impl EngineConfig {
    /// Validate both budgets; any zero field is [`EngineError::InvalidBudget`].
    pub fn new(bangla: LanguageBudget, english: LanguageBudget) -> Result<Self, EngineError> {
        if !bangla.is_valid() || !english.is_valid() {
            return Err(EngineError::InvalidBudget);
        }
        Ok(Self { bangla, english })
    }
}

impl Default for EngineConfig {
    /// Normative defaults: dictionary fan-out is bounded and cheap, Bangla
    /// romanized input fans out more than English, ranker input stays under
    /// the 2 ms rerank budget, and the UI shows at most the top 10.
    fn default() -> Self {
        Self {
            bangla: LanguageBudget {
                max_prefix_hits: 20,
                max_transliteration_hits: 8,
                max_merged: 24,
                max_limit: 10,
            },
            english: LanguageBudget {
                max_prefix_hits: 20,
                max_transliteration_hits: 2,
                max_merged: 24,
                max_limit: 10,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EngineConfig, LanguageBudget};
    use crate::error::EngineError;
    use okkhor_core::Language;

    fn budget() -> LanguageBudget {
        LanguageBudget {
            max_prefix_hits: 20,
            max_transliteration_hits: 8,
            max_merged: 24,
            max_limit: 10,
        }
    }

    #[test]
    fn default_matches_documented_table() {
        let config = EngineConfig::default();
        assert_eq!(config.bangla.max_prefix_hits, 20);
        assert_eq!(config.bangla.max_transliteration_hits, 8);
        assert_eq!(config.bangla.max_merged, 24);
        assert_eq!(config.bangla.max_limit, 10);
        assert_eq!(config.english.max_prefix_hits, 20);
        assert_eq!(config.english.max_transliteration_hits, 2);
        assert_eq!(config.english.max_merged, 24);
        assert_eq!(config.english.max_limit, 10);
    }

    #[test]
    fn for_language_selects_side() {
        let config = EngineConfig::default();
        assert_eq!(
            LanguageBudget::for_language(&config, Language::Bangla),
            config.bangla
        );
        assert_eq!(
            LanguageBudget::for_language(&config, Language::English),
            config.english
        );
    }

    #[test]
    fn zero_field_is_invalid_budget() {
        let zero = LanguageBudget {
            max_limit: 0,
            ..budget()
        };
        assert!(matches!(
            EngineConfig::new(zero, budget()),
            Err(EngineError::InvalidBudget)
        ));
        assert!(matches!(
            EngineConfig::new(budget(), zero),
            Err(EngineError::InvalidBudget)
        ));
    }

    #[test]
    fn valid_budgets_construct() {
        assert!(EngineConfig::new(budget(), budget()).is_ok());
    }
}
