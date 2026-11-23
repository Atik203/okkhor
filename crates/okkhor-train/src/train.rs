#![forbid(unsafe_code)]
//! Deterministic logistic trainer for the Tier 1 candidate reranker.
//!
//! Trains on replay cases (`docs/design/replay.md`): one binary label per
//! candidate, positive for the accepted/shown answer or `expected[0]`, all
//! others negative. The 80/20 split is by SHA-256 of `case_id`, never row
//! order, and standardization uses training-split statistics only. Full-batch
//! gradient descent with a seeded shuffle makes the model JSON byte-identical
//! for the same seed, replay, and `trained_at`.
//!
//! Offline only: no network, no wall clock in the deterministic core, no new
//! dependencies. Synthetic or permissively licensed replay data only.

use okkhor_core::{Candidate, CandidateSource, SuggestionRequest};
use okkhor_learn::Smriti;
use serde::Serialize;
use thiserror::Error;

use crate::features::extract_features;
use crate::generate::{SplitMix64, sha256_hex};
use crate::replay::{ReplayCase, ReplayError, read_replay_str};

/// Expected value of [`TrainedModel::schema`].
pub const MODEL_SCHEMA: u32 = 1;
/// Feature schema the weights are trained against.
pub const FEATURE_SCHEMA: &str = "v1";
/// Default shuffle seed used by the CLI.
pub const DEFAULT_SEED: u64 = 20261123;
/// Default `trained_at`; fixed so repeated runs are byte-identical.
pub const DEFAULT_TRAINED_AT: &str = "1970-01-01T00:00:00Z";
/// Refuse to fit on fewer replay cases than this.
pub const MIN_CASES: usize = 200;

pub const MODEL_NAME: &str = "okkhor-reranker";
const MODEL_NOTES: &str =
    "trained on replay cases; synthetic fixtures only, not a real-world quality claim";

const NUM_FEATURES: usize = 16;
const ITERATIONS: usize = 500;
const LEARNING_RATE: f64 = 0.05;
const L2: f64 = 1e-4;
const TRAIN_THRESHOLD: u64 = 80;
const MIN_VARIANCE: f64 = 1e-12;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TrainError {
    #[error("need at least {min} replay cases, got {got}")]
    TooFewCases { got: usize, min: usize },
    #[error("no reusable cases after label mapping ({skipped} skipped)")]
    NoUsableCases { skipped: usize },
    #[error("training split is empty")]
    EmptyTrainSplit,
    #[error("training split has no negative candidates")]
    NoNegatives,
    #[error("feature extraction failed for case {case_id}: {source}")]
    Feature {
        case_id: String,
        #[source]
        source: crate::features::FeatureError,
    },
    #[error("training produced non-finite parameters")]
    NonFiniteParameters,
    #[error("cannot serialize model: {reason}")]
    Serialize { reason: String },
    #[error(transparent)]
    Replay(#[from] ReplayError),
}

/// CLI/API knobs. `trained_at` is the only non-deterministic field, so it is
/// caller-supplied rather than read from the clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainOptions {
    pub seed: u64,
    pub trained_at: String,
}

impl Default for TrainOptions {
    fn default() -> Self {
        Self {
            seed: DEFAULT_SEED,
            trained_at: DEFAULT_TRAINED_AT.to_owned(),
        }
    }
}

impl TrainOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    pub fn with_trained_at(mut self, trained_at: impl Into<String>) -> Self {
        self.trained_at = trained_at.into();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrainMetrics {
    pub train_cases: usize,
    pub validation_cases: usize,
    pub skipped_cases: usize,
    pub positives: usize,
    pub negatives: usize,
    pub weight_pos: f32,
    pub validation_top1: f64,
    pub validation_top5: f64,
    pub validation_mrr: f64,
    pub final_loss: f32,
}

/// Serializable model artifact. Field order is part of the byte contract.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrainedModel {
    pub schema: u32,
    pub name: String,
    pub version: String,
    pub feature_schema: String,
    pub weights: Vec<f32>,
    pub bias: f32,
    pub means: Vec<f32>,
    pub scales: Vec<f32>,
    pub trained_at: String,
    pub dataset_sha256: String,
    pub metrics: TrainMetrics,
    pub notes: String,
}

impl TrainedModel {
    /// Pretty JSON, stable field and vector order.
    pub fn to_json(&self) -> Result<String, TrainError> {
        serde_json::to_string_pretty(self).map_err(|error| TrainError::Serialize {
            reason: error.to_string(),
        })
    }
}

struct PoolItem {
    text: String,
    features: [f32; NUM_FEATURES],
}

struct LabeledCase {
    items: Vec<PoolItem>,
    positive: usize,
    train: bool,
}

#[derive(Clone, Copy)]
struct Standardizer {
    means: [f32; NUM_FEATURES],
    scales: [f32; NUM_FEATURES],
}

impl Standardizer {
    fn fit(cases: &[LabeledCase]) -> Self {
        let mut sums = [0.0_f64; NUM_FEATURES];
        let mut squares = [0.0_f64; NUM_FEATURES];
        let mut count = 0_usize;
        for case in cases.iter().filter(|case| case.train) {
            for item in &case.items {
                count += 1;
                for (sum, value) in sums.iter_mut().zip(item.features.iter()) {
                    *sum += f64::from(*value);
                }
                for (square, value) in squares.iter_mut().zip(item.features.iter()) {
                    *square += f64::from(*value) * f64::from(*value);
                }
            }
        }
        let mut standardizer = Self {
            means: [0.0; NUM_FEATURES],
            scales: [1.0; NUM_FEATURES],
        };
        if count == 0 {
            return standardizer;
        }
        let denominator = count as f64;
        for ((mean, scale), (sum, square)) in standardizer
            .means
            .iter_mut()
            .zip(standardizer.scales.iter_mut())
            .zip(sums.iter().zip(squares.iter()))
        {
            let center = *sum / denominator;
            let variance = (*square / denominator - center * center).max(0.0);
            *mean = center as f32;
            let deviation = variance.sqrt();
            *scale = if deviation.is_finite() && variance > MIN_VARIANCE {
                deviation as f32
            } else {
                1.0
            };
        }
        standardizer
    }

    fn transform(&self, features: &[f32; NUM_FEATURES]) -> [f32; NUM_FEATURES] {
        let mut out = [0.0_f32; NUM_FEATURES];
        for ((slot, value), (mean, scale)) in out
            .iter_mut()
            .zip(features.iter())
            .zip(self.means.iter().zip(self.scales.iter()))
        {
            *slot = (*value - *mean) / *scale;
        }
        out
    }
}

fn is_train_case(case_id: &str) -> bool {
    let digest = sha256_hex(case_id.as_bytes());
    let mut bucket = 0_u64;
    for byte in digest.as_bytes().iter().take(16) {
        let digit = u64::from(match *byte {
            b'0'..=b'9' => *byte - b'0',
            b'a'..=b'f' => *byte - b'a' + 10,
            _ => return false,
        });
        bucket = (bucket << 4) | digit;
    }
    bucket % 100 < TRAIN_THRESHOLD
}

fn request_for(case: &ReplayCase) -> SuggestionRequest {
    // Same synthesis as `okkhor-replay`: an ASCII Bangla prefix is also the
    // Roman input that produced it.
    let roman_input = match case.language {
        okkhor_core::Language::Bangla if case.prefix.is_ascii() && !case.prefix.is_empty() => {
            Some(case.prefix.clone())
        }
        _ => None,
    };
    SuggestionRequest {
        prefix: case.prefix.clone(),
        language: case.language,
        layout: case.layout.clone(),
        context: case.context.clone(),
        roman_input,
        now_ms: 0,
        limit: 5,
    }
}

/// Build the labeled pools, dropping cases whose positive answer is absent
/// from `candidates` (counted, never guessed).
fn build_cases(document: &str) -> Result<(Vec<LabeledCase>, usize), TrainError> {
    let cases = read_replay_str(document)?;
    if cases.len() < MIN_CASES {
        return Err(TrainError::TooFewCases {
            got: cases.len(),
            min: MIN_CASES,
        });
    }
    let snapshot = Smriti::new().snapshot();
    let mut labeled = Vec::with_capacity(cases.len());
    let mut skipped = 0_usize;
    for case in &cases {
        let positive_text = match case.accepted {
            Some(index) => case.shown.get(index).cloned(),
            None => case.expected.first().cloned(),
        };
        let positive =
            positive_text.and_then(|text| case.candidates.iter().position(|t| *t == text));
        let Some(positive) = positive else {
            skipped += 1;
            continue;
        };
        let request = request_for(case);
        let mut items = Vec::with_capacity(case.candidates.len());
        for (index, text) in case.candidates.iter().enumerate() {
            let mut candidate =
                Candidate::new(text.clone(), case.language, CandidateSource::Dictionary);
            candidate.features.base_score = 1.0 / (1.0 + index as f32);
            let features = extract_features(&request, &candidate, &snapshot).map_err(|source| {
                TrainError::Feature {
                    case_id: case.case_id.clone(),
                    source,
                }
            })?;
            items.push(PoolItem {
                text: text.clone(),
                features: features.to_array(),
            });
        }
        labeled.push(LabeledCase {
            items,
            positive,
            train: is_train_case(&case.case_id),
        });
    }
    if labeled.is_empty() {
        return Err(TrainError::NoUsableCases { skipped });
    }
    Ok((labeled, skipped))
}

fn shuffle(order: &mut [usize], rng: &mut SplitMix64) {
    for index in (1..order.len()).rev() {
        let other = (rng.next_u64() % (index as u64 + 1)) as usize;
        order.swap(index, other);
    }
}

fn sigmoid(value: f64) -> f64 {
    if value >= 0.0 {
        1.0 / (1.0 + (-value).exp())
    } else {
        let exp = value.exp();
        exp / (1.0 + exp)
    }
}

fn logit(weights: &[f32; NUM_FEATURES], bias: f32, features: &[f32; NUM_FEATURES]) -> f64 {
    weights
        .iter()
        .zip(features.iter())
        .map(|(weight, feature)| f64::from(*weight) * f64::from(*feature))
        .sum::<f64>()
        + f64::from(bias)
}

fn train_weights(
    cases: &[LabeledCase],
    standardizer: &Standardizer,
    seed: u64,
    weight_pos: f32,
) -> Result<([f32; NUM_FEATURES], f32, f32), TrainError> {
    let mut weights = [0.0_f32; NUM_FEATURES];
    let mut bias = 0.0_f32;
    let mut order: Vec<usize> = cases
        .iter()
        .enumerate()
        .filter(|(_, case)| case.train)
        .map(|(index, _)| index)
        .collect();
    let examples: f64 = cases
        .iter()
        .filter(|case| case.train)
        .map(|case| case.items.len())
        .sum::<usize>() as f64;
    let mut rng = SplitMix64::new(seed);

    for _ in 0..ITERATIONS {
        shuffle(&mut order, &mut rng);
        let mut gradients = [0.0_f64; NUM_FEATURES];
        let mut bias_gradient = 0.0_f64;
        for &case_index in &order {
            let case = &cases[case_index];
            for (item_index, item) in case.items.iter().enumerate() {
                let features = standardizer.transform(&item.features);
                let probability = sigmoid(logit(&weights, bias, &features));
                let label = if item_index == case.positive {
                    1.0
                } else {
                    0.0
                };
                let item_weight = if item_index == case.positive {
                    f64::from(weight_pos)
                } else {
                    1.0
                };
                let error = item_weight * (probability - label);
                for (gradient, feature) in gradients.iter_mut().zip(features.iter()) {
                    *gradient += error * f64::from(*feature);
                }
                bias_gradient += error;
            }
        }
        for (weight, gradient) in weights.iter_mut().zip(gradients.iter()) {
            let scaled = gradient / examples + L2 * f64::from(*weight);
            *weight = (f64::from(*weight) - LEARNING_RATE * scaled) as f32;
        }
        bias = (f64::from(bias) - LEARNING_RATE * (bias_gradient / examples)) as f32;
    }

    if weights.iter().any(|weight| !weight.is_finite()) || !bias.is_finite() {
        return Err(TrainError::NonFiniteParameters);
    }

    let mut loss = 0.0_f64;
    let mut total_weight = 0.0_f64;
    for case in cases.iter().filter(|case| case.train) {
        for (item_index, item) in case.items.iter().enumerate() {
            let features = standardizer.transform(&item.features);
            let probability = sigmoid(logit(&weights, bias, &features)).clamp(1e-12, 1.0 - 1e-12);
            let label = if item_index == case.positive {
                1.0
            } else {
                0.0
            };
            let item_weight = if item_index == case.positive {
                f64::from(weight_pos)
            } else {
                1.0
            };
            loss -=
                item_weight * (label * probability.ln() + (1.0 - label) * (1.0 - probability).ln());
            total_weight += item_weight;
        }
    }
    let final_loss = if total_weight > 0.0 {
        (loss / total_weight) as f32
    } else {
        0.0
    };
    Ok((weights, bias, final_loss))
}

struct EvalSummary {
    cases: usize,
    top1: f64,
    top5: f64,
    mrr: f64,
}

fn evaluate(
    cases: &[LabeledCase],
    weights: &[f32; NUM_FEATURES],
    bias: f32,
    standardizer: &Standardizer,
) -> EvalSummary {
    let mut scored_cases = 0_usize;
    let mut top1 = 0_usize;
    let mut top5 = 0_usize;
    let mut reciprocal_rank = 0.0_f64;
    for case in cases.iter().filter(|case| !case.train) {
        scored_cases += 1;
        let mut ranked: Vec<(f32, &str)> = case
            .items
            .iter()
            .map(|item| {
                let features = standardizer.transform(&item.features);
                (logit(weights, bias, &features) as f32, item.text.as_str())
            })
            .collect();
        ranked.sort_by(|left, right| {
            right
                .0
                .partial_cmp(&left.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.1.cmp(right.1))
        });
        let positive = case.items[case.positive].text.as_str();
        if let Some(rank) = ranked.iter().position(|(_, text)| *text == positive) {
            if rank == 0 {
                top1 += 1;
            }
            if rank < 5 {
                top5 += 1;
            }
            reciprocal_rank += 1.0 / (rank as f64 + 1.0);
        }
    }
    if scored_cases == 0 {
        return EvalSummary {
            cases: 0,
            top1: 0.0,
            top5: 0.0,
            mrr: 0.0,
        };
    }
    let denominator = scored_cases as f64;
    EvalSummary {
        cases: scored_cases,
        top1: top1 as f64 / denominator,
        top5: top5 as f64 / denominator,
        mrr: reciprocal_rank / denominator,
    }
}

/// Fit a model from a replay JSONL document.
pub fn fit_replay(document: &str, options: &TrainOptions) -> Result<TrainedModel, TrainError> {
    let (cases, skipped) = build_cases(document)?;
    let train_cases = cases.iter().filter(|case| case.train).count();
    if train_cases == 0 {
        return Err(TrainError::EmptyTrainSplit);
    }
    let positives = train_cases;
    let negatives: usize = cases
        .iter()
        .filter(|case| case.train)
        .map(|case| case.items.len().saturating_sub(1))
        .sum();
    if negatives == 0 {
        return Err(TrainError::NoNegatives);
    }
    let weight_pos = negatives as f32 / positives as f32;
    let standardizer = Standardizer::fit(&cases);
    let (weights, bias, final_loss) =
        train_weights(&cases, &standardizer, options.seed, weight_pos)?;
    let evaluation = evaluate(&cases, &weights, bias, &standardizer);

    Ok(TrainedModel {
        schema: MODEL_SCHEMA,
        name: MODEL_NAME.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        feature_schema: FEATURE_SCHEMA.to_owned(),
        weights: weights.to_vec(),
        bias,
        means: standardizer.means.to_vec(),
        scales: standardizer.scales.to_vec(),
        trained_at: options.trained_at.clone(),
        dataset_sha256: sha256_hex(document.as_bytes()),
        metrics: TrainMetrics {
            train_cases,
            validation_cases: evaluation.cases,
            skipped_cases: skipped,
            positives,
            negatives,
            weight_pos,
            validation_top1: evaluation.top1,
            validation_top5: evaluation.top5,
            validation_mrr: evaluation.mrr,
            final_loss,
        },
        notes: MODEL_NOTES.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::FeatureVectorV1;
    use crate::generate::{generate_cases, render_jsonl};

    const SYNTHETIC_SEED: u64 = 20261123;

    fn synthetic_document(count: usize) -> String {
        render_jsonl(&generate_cases(SYNTHETIC_SEED, count).expect("generate"))
    }

    #[test]
    fn rejects_fewer_than_two_hundred_cases() {
        let document = synthetic_document(MIN_CASES - 1);
        assert_eq!(
            fit_replay(&document, &TrainOptions::default()),
            Err(TrainError::TooFewCases {
                got: MIN_CASES - 1,
                min: MIN_CASES
            })
        );
    }

    #[test]
    fn same_inputs_produce_byte_identical_json() {
        let document = synthetic_document(240);
        let options = TrainOptions::default()
            .with_seed(7)
            .with_trained_at("2026-11-23T00:00:00Z");
        let first = fit_replay(&document, &options).expect("fit");
        let second = fit_replay(&document, &options).expect("fit again");
        assert_eq!(first, second);
        assert_eq!(
            first.to_json().expect("json"),
            second.to_json().expect("json")
        );
        assert_eq!(first.dataset_sha256, sha256_hex(document.as_bytes()));
    }

    #[test]
    fn shuffle_depends_on_seed() {
        let mut first: Vec<usize> = (0..240).collect();
        let mut second: Vec<usize> = (0..240).collect();
        shuffle(&mut first, &mut SplitMix64::new(1));
        shuffle(&mut second, &mut SplitMix64::new(2));
        assert_ne!(first, second);
    }

    #[test]
    fn label_mapping_miss_is_counted() {
        let mut cases = generate_cases(SYNTHETIC_SEED, 210).expect("generate");
        let target = cases[0].expected[0].clone();
        cases[0].candidates.retain(|text| *text != target);
        assert!(!cases[0].candidates.contains(&target));
        let document = render_jsonl(&cases);
        let model = fit_replay(&document, &TrainOptions::default()).expect("fit");
        assert_eq!(model.metrics.skipped_cases, 1);
        assert_eq!(
            model.metrics.train_cases + model.metrics.validation_cases,
            cases.len() - 1
        );
    }

    #[test]
    fn split_is_by_case_id_and_not_row_order() {
        let cases = generate_cases(SYNTHETIC_SEED, 240).expect("generate");
        let forward: Vec<bool> = cases
            .iter()
            .map(|case| (case.case_id.as_str(), is_train_case(&case.case_id)))
            .map(|(_, train)| train)
            .collect();
        let backward: Vec<bool> = cases
            .iter()
            .rev()
            .map(|case| is_train_case(&case.case_id))
            .collect();
        let mut reversed_forward = forward.clone();
        reversed_forward.reverse();
        assert_eq!(reversed_forward, backward);

        let train = forward.iter().filter(|train| **train).count();
        let fraction = train as f64 / forward.len() as f64;
        assert!(
            (0.65..=0.95).contains(&fraction),
            "unexpected train fraction {fraction}"
        );
    }

    #[test]
    fn metrics_and_shapes_are_bounded() {
        let document = synthetic_document(240);
        let model = fit_replay(&document, &TrainOptions::default()).expect("fit");
        assert_eq!(model.weights.len(), NUM_FEATURES);
        assert_eq!(model.means.len(), NUM_FEATURES);
        assert_eq!(model.scales.len(), NUM_FEATURES);
        assert_eq!(model.feature_schema, FEATURE_SCHEMA);
        assert!(model.weights.iter().all(|weight| weight.is_finite()));
        assert!(model.metrics.validation_cases > 0);
        assert!((0.0..=1.0).contains(&model.metrics.validation_top1));
        assert!((0.0..=1.0).contains(&model.metrics.validation_top5));
        assert!((0.0..=1.0).contains(&model.metrics.validation_mrr));
        assert!(model.metrics.weight_pos > 0.0);
    }

    #[test]
    fn feature_vectors_are_pinned_to_sixteen() {
        let document = synthetic_document(MIN_CASES);
        let cases = read_replay_str(&document).expect("replay");
        let snapshot = Smriti::new().snapshot();
        let case = &cases[0];
        let request = request_for(case);
        let candidate = Candidate::new(
            case.candidates[0].clone(),
            case.language,
            CandidateSource::Dictionary,
        );
        let features = extract_features(&request, &candidate, &snapshot).expect("features");
        let array: [f32; NUM_FEATURES] = features.to_array();
        assert_eq!(array.len(), FeatureVectorV1::field_names().len());
    }
}
