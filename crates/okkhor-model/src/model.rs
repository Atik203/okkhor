//! Checked loading and scored inference for the 16-feature v1 logistic model.
//!
//! The model is data, not code: every read is validated (checksum, schema,
//! vector lengths, finite and bounded parameters) before it can score. The
//! shipped artifact is embedded with [`include_bytes!`], so inference never
//! touches the disk.

use std::fmt::Write;

use okkhor_features::FeatureVectorV1;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Expected value of the model `schema` field.
pub const MODEL_SCHEMA: u32 = 1;
/// Feature schema the weights must be trained against.
pub const FEATURE_SCHEMA: &str = "v1";
/// Number of features in the pinned v1 schema.
pub const NUM_FEATURES: usize = 16;
/// Reject parameters whose magnitude exceeds this bound.
pub const MAX_MAGNITUDE: f32 = 1000.0;

/// The bundled, synthetic-trained model. Small JSON, committed on purpose.
pub const BUNDLED_MODEL_JSON: &[u8] = include_bytes!("../assets/reranker-v1.json");

/// SHA-256 of [`BUNDLED_MODEL_JSON`]; verified once at startup.
pub const BUNDLED_MODEL_SHA256: &str =
    "e5a6ce0b3b7e5aa506b48af0a123629b2988b5c01d42ea8153e478eaf4ff6440";

#[derive(Debug, Error, Clone, PartialEq)]
pub enum ModelError {
    #[error("model checksum mismatch")]
    ChecksumMismatch,
    #[error("malformed model json: {message}")]
    Malformed { message: String },
    #[error("unsupported model schema {found}; expected {expected}")]
    UnsupportedSchema { found: u32, expected: u32 },
    #[error("unsupported feature schema {found}; expected {expected}")]
    UnsupportedFeatureSchema {
        found: String,
        expected: &'static str,
    },
    #[error("model field {field} must have {expected} values, got {got}")]
    WrongVectorLength {
        field: &'static str,
        expected: usize,
        got: usize,
    },
    #[error("model parameter {field}[{index}] is not finite")]
    NonFinite { field: &'static str, index: usize },
    #[error("model parameter {field}[{index}] exceeds +/-{max}")]
    Magnitude {
        field: &'static str,
        index: usize,
        max: f32,
    },
    #[error("model scale[{index}] must be finite and positive")]
    NonPositiveScale { index: usize },
    #[error("feature value {index} is not finite")]
    NonFiniteFeature { index: usize },
    #[error("model produced a non-finite score")]
    NonFiniteScore,
}

#[derive(Debug, Deserialize)]
struct ModelJson {
    schema: u32,
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    feature_schema: String,
    weights: Vec<f32>,
    bias: f32,
    means: Vec<f32>,
    scales: Vec<f32>,
    #[serde(default)]
    trained_at: String,
    #[serde(default)]
    dataset_sha256: String,
    #[serde(default)]
    metrics: serde_json::Value,
    #[serde(default)]
    notes: String,
}

/// A validated linear model over [`FeatureVectorV1`].
#[derive(Debug, Clone, PartialEq)]
pub struct LogisticModel {
    name: String,
    version: String,
    weights: [f32; NUM_FEATURES],
    bias: f32,
    means: [f32; NUM_FEATURES],
    scales: [f32; NUM_FEATURES],
    trained_at: String,
    dataset_sha256: String,
    metrics: serde_json::Value,
    notes: String,
}

impl LogisticModel {
    /// Parse and validate a model, after checking the bytes against
    /// `expected_sha256` (lowercase hex).
    pub fn from_json_bytes(bytes: &[u8], expected_sha256: &str) -> Result<Self, ModelError> {
        let actual = sha256_hex(bytes);
        if !actual.eq_ignore_ascii_case(expected_sha256.trim()) {
            return Err(ModelError::ChecksumMismatch);
        }
        let json: ModelJson =
            serde_json::from_slice(bytes).map_err(|error| ModelError::Malformed {
                message: error.to_string(),
            })?;
        if json.schema != MODEL_SCHEMA {
            return Err(ModelError::UnsupportedSchema {
                found: json.schema,
                expected: MODEL_SCHEMA,
            });
        }
        if json.feature_schema != FEATURE_SCHEMA {
            return Err(ModelError::UnsupportedFeatureSchema {
                found: json.feature_schema,
                expected: FEATURE_SCHEMA,
            });
        }
        let weights = validated_vector("weights", json.weights)?;
        let means = validated_vector("means", json.means)?;
        let scales = validated_vector("scales", json.scales)?;
        for (index, scale) in scales.iter().enumerate() {
            if !scale.is_finite() || *scale <= 0.0 {
                return Err(ModelError::NonPositiveScale { index });
            }
        }
        if !json.bias.is_finite() {
            return Err(ModelError::NonFinite {
                field: "bias",
                index: 0,
            });
        }
        if json.bias.abs() > MAX_MAGNITUDE {
            return Err(ModelError::Magnitude {
                field: "bias",
                index: 0,
                max: MAX_MAGNITUDE,
            });
        }

        Ok(Self {
            name: json.name,
            version: json.version,
            weights,
            bias: json.bias,
            means,
            scales,
            trained_at: json.trained_at,
            dataset_sha256: json.dataset_sha256,
            metrics: json.metrics,
            notes: json.notes,
        })
    }

    /// Standardize each feature with the stored means/scales, then compute
    /// `weights . standardized + bias`.
    pub fn score(&self, features: &FeatureVectorV1) -> Result<f32, ModelError> {
        let values = features.to_array();
        let mut logit = f64::from(self.bias);
        for index in 0..NUM_FEATURES {
            let value = values[index];
            if !value.is_finite() {
                return Err(ModelError::NonFiniteFeature { index });
            }
            let standardized = (value - self.means[index]) / self.scales[index];
            logit += f64::from(self.weights[index]) * f64::from(standardized);
        }
        let logit = logit as f32;
        if logit.is_finite() {
            Ok(logit)
        } else {
            Err(ModelError::NonFiniteScore)
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn trained_at(&self) -> &str {
        &self.trained_at
    }

    pub fn dataset_sha256(&self) -> &str {
        &self.dataset_sha256
    }

    pub fn metrics(&self) -> &serde_json::Value {
        &self.metrics
    }

    pub fn notes(&self) -> &str {
        &self.notes
    }

    pub fn bias(&self) -> f32 {
        self.bias
    }

    pub fn weights(&self) -> &[f32; NUM_FEATURES] {
        &self.weights
    }
}

/// Load and verify the embedded model.
pub fn load_bundled() -> Result<LogisticModel, ModelError> {
    LogisticModel::from_json_bytes(BUNDLED_MODEL_JSON, BUNDLED_MODEL_SHA256)
}

fn validated_vector(
    field: &'static str,
    values: Vec<f32>,
) -> Result<[f32; NUM_FEATURES], ModelError> {
    if values.len() != NUM_FEATURES {
        return Err(ModelError::WrongVectorLength {
            field,
            expected: NUM_FEATURES,
            got: values.len(),
        });
    }
    for (index, value) in values.iter().enumerate() {
        if !value.is_finite() {
            return Err(ModelError::NonFinite { field, index });
        }
        if value.abs() > MAX_MAGNITUDE {
            return Err(ModelError::Magnitude {
                field,
                index,
                max: MAX_MAGNITUDE,
            });
        }
    }
    let mut array = [0.0_f32; NUM_FEATURES];
    array.copy_from_slice(&values);
    Ok(array)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundled() -> LogisticModel {
        load_bundled().expect("bundled model is valid")
    }

    fn sample_vector() -> FeatureVectorV1 {
        FeatureVectorV1 {
            base_score: 1.0,
            candidate_source: 0,
            language: 1,
            layout: 1,
            prefix_len: 2,
            word_len: 4,
            user_word_count: 0,
            pair_count: 0,
            recency: 0.0,
            acceptance_rate: 0.0,
            correction_rate: 0.0,
            bigram_score: 0.0,
            phonetic_score: 0.8,
            is_user_vocab: false,
            context_language: 0,
            script_mix: 0.0,
        }
    }

    fn mutated(update: impl FnOnce(&mut serde_json::Value)) -> (Vec<u8>, String) {
        let mut value: serde_json::Value =
            serde_json::from_slice(BUNDLED_MODEL_JSON).expect("bundled json");
        update(&mut value);
        let bytes = serde_json::to_vec(&value).expect("serialize");
        let sha = sha256_hex(&bytes);
        (bytes, sha)
    }

    #[test]
    fn bundled_model_loads_and_matches_the_pinned_checksum() {
        assert_eq!(sha256_hex(BUNDLED_MODEL_JSON), BUNDLED_MODEL_SHA256);
        let model = bundled();
        assert_eq!(model.name(), "okkhor-reranker");
        assert_eq!(model.weights().len(), NUM_FEATURES);
    }

    #[test]
    fn checksum_mismatch_is_rejected() {
        assert_eq!(
            LogisticModel::from_json_bytes(BUNDLED_MODEL_JSON, "deadbeef"),
            Err(ModelError::ChecksumMismatch)
        );
    }

    #[test]
    fn unsupported_model_schema_is_rejected() {
        let (bytes, sha) = mutated(|value| value["schema"] = serde_json::json!(2));
        assert_eq!(
            LogisticModel::from_json_bytes(&bytes, &sha),
            Err(ModelError::UnsupportedSchema {
                found: 2,
                expected: MODEL_SCHEMA
            })
        );
    }

    #[test]
    fn unsupported_feature_schema_is_rejected() {
        let (bytes, sha) = mutated(|value| value["feature_schema"] = serde_json::json!("v2"));
        assert_eq!(
            LogisticModel::from_json_bytes(&bytes, &sha),
            Err(ModelError::UnsupportedFeatureSchema {
                found: "v2".to_owned(),
                expected: FEATURE_SCHEMA
            })
        );
    }

    #[test]
    fn wrong_vector_length_is_rejected() {
        let (bytes, sha) = mutated(|value| {
            value["weights"].as_array_mut().expect("weights").pop();
        });
        assert_eq!(
            LogisticModel::from_json_bytes(&bytes, &sha),
            Err(ModelError::WrongVectorLength {
                field: "weights",
                expected: NUM_FEATURES,
                got: NUM_FEATURES - 1
            })
        );
    }

    #[test]
    fn oversized_and_non_finite_weights_are_rejected() {
        let (bytes, sha) = mutated(|value| value["weights"][0] = serde_json::json!(2000.0));
        assert_eq!(
            LogisticModel::from_json_bytes(&bytes, &sha),
            Err(ModelError::Magnitude {
                field: "weights",
                index: 0,
                max: MAX_MAGNITUDE
            })
        );

        let (bytes, sha) = mutated(|value| value["weights"][0] = serde_json::json!(1e39));
        assert_eq!(
            LogisticModel::from_json_bytes(&bytes, &sha),
            Err(ModelError::NonFinite {
                field: "weights",
                index: 0
            })
        );
    }

    #[test]
    fn non_positive_scale_is_rejected() {
        let (bytes, sha) = mutated(|value| value["scales"][2] = serde_json::json!(0.0));
        assert_eq!(
            LogisticModel::from_json_bytes(&bytes, &sha),
            Err(ModelError::NonPositiveScale { index: 2 })
        );
    }

    #[test]
    fn golden_score_vector_is_pinned() {
        let score = bundled().score(&sample_vector()).expect("score");
        assert_eq!(score, 0.28476113);
    }

    #[test]
    fn non_finite_features_are_rejected() {
        let mut vector = sample_vector();
        vector.script_mix = f32::NAN;
        assert_eq!(
            bundled().score(&vector),
            Err(ModelError::NonFiniteFeature { index: 15 })
        );
    }
}
