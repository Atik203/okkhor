use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Relative weights for the personal ranking model.
///
/// These are starting values. Tune them with replay data, not by intuition,
/// and record the result in an ADR.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankingWeights {
    pub base: f32,
    pub user_frequency: f32,
    pub recency: f32,
    pub context: f32,
    pub acceptance: f32,
    pub correction: f32,
    pub phonetic: f32,
    pub pair_frequency: f32,
}

impl Default for RankingWeights {
    fn default() -> Self {
        Self {
            base: 1.0,
            user_frequency: 0.35,
            recency: 0.25,
            context: 0.40,
            acceptance: 0.30,
            correction: 0.50,
            phonetic: 0.20,
            pair_frequency: 0.60,
        }
    }
}

/// Limits and weights for one local learning store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SmritiConfig {
    /// Half-life for recency decay. Recent words fade after this many days.
    pub half_life_days: f32,
    pub max_words: usize,
    pub max_pairs: usize,
    pub max_bigrams: usize,
    pub weights: RankingWeights,
}

impl Default for SmritiConfig {
    fn default() -> Self {
        Self {
            half_life_days: 14.0,
            max_words: 50_000,
            max_pairs: 100_000,
            max_bigrams: 200_000,
            weights: RankingWeights::default(),
        }
    }
}

impl SmritiConfig {
    /// Validate values that would make scoring non-deterministic or unbounded.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !self.half_life_days.is_finite() || self.half_life_days <= 0.0 {
            return Err(ConfigError::NotPositive {
                field: "half_life_days",
            });
        }
        if self.max_words == 0 {
            return Err(ConfigError::NotPositive { field: "max_words" });
        }
        if self.max_pairs == 0 {
            return Err(ConfigError::NotPositive { field: "max_pairs" });
        }
        if self.max_bigrams == 0 {
            return Err(ConfigError::NotPositive {
                field: "max_bigrams",
            });
        }

        let weights = &self.weights;
        for (field, value) in [
            ("base", weights.base),
            ("user_frequency", weights.user_frequency),
            ("recency", weights.recency),
            ("context", weights.context),
            ("acceptance", weights.acceptance),
            ("correction", weights.correction),
            ("phonetic", weights.phonetic),
            ("pair_frequency", weights.pair_frequency),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(ConfigError::InvalidWeight { field });
            }
        }

        Ok(())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ConfigError {
    #[error("{field} must be greater than zero")]
    NotPositive { field: &'static str },
    #[error("{field} must be finite and non-negative")]
    InvalidWeight { field: &'static str },
}

#[cfg(test)]
mod tests {
    use super::{ConfigError, RankingWeights, SmritiConfig};

    #[test]
    fn default_config_is_valid() {
        assert_eq!(SmritiConfig::default().validate(), Ok(()));
    }

    #[test]
    fn rejects_zero_half_life() {
        let config = SmritiConfig {
            half_life_days: 0.0,
            ..SmritiConfig::default()
        };
        assert_eq!(
            config.validate(),
            Err(ConfigError::NotPositive {
                field: "half_life_days"
            })
        );
    }

    #[test]
    fn rejects_negative_weight() {
        let config = SmritiConfig {
            weights: RankingWeights {
                context: -1.0,
                ..RankingWeights::default()
            },
            ..SmritiConfig::default()
        };
        assert_eq!(
            config.validate(),
            Err(ConfigError::InvalidWeight { field: "context" })
        );
    }
}
