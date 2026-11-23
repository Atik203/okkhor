//! The bounded hybrid blend from ADR 0017/0019.
//!
//! `HybridRanker` reads baseline scores for the whole staged pool under one
//! read lock, extracts v1 features directly from the live `Smriti` store (no
//! snapshot clone), centres the model logits, and adds a per-candidate,
//! evidence-scaled correction to the baseline score. Any error, an oversized
//! pool, or a non-finite value returns the deterministic baseline order.

use std::sync::Arc;

use okkhor_core::{Candidate, Language, Ranker, Suggestion, SuggestionRequest};
use okkhor_features::{FeatureSource, FeatureVectorV1, extract_features};
use okkhor_learn::{BigramStat, PairStat, Smriti, WordStat};

use crate::model::LogisticModel;

/// Structural work bound: larger pools fall back to the baseline order.
pub const MAX_POOL: usize = 64;
/// Blend weight for the centred model score. Must stay `<= 1.0`.
pub const MODEL_WEIGHT: f32 = 1.0;
/// Confidence ceiling; the model never fully overrides the baseline.
pub const CONFIDENCE_CAP: f32 = 0.75;

/// Borrowed [`FeatureSource`] over the live Smriti store.
///
/// Each lookup takes the store's read lock for one hash-map get, so the live
/// path never clones the full snapshot.
#[derive(Debug, Clone, Copy)]
pub struct SmritiFeatureSource<'a>(pub &'a Smriti);

impl FeatureSource for SmritiFeatureSource<'_> {
    fn word_stat(&self, language: Language, word: &str) -> Option<WordStat> {
        self.0.word_stat(language, word)
    }

    fn pair_stat(&self, language: Language, roman_input: &str, output: &str) -> Option<PairStat> {
        self.0.pair_stat(language, roman_input, output)
    }

    fn bigram_stat(&self, language: Language, previous: &str, current: &str) -> Option<BigramStat> {
        self.0.bigram_stat(language, previous, current)
    }
}

/// Baseline Smriti scores plus a centred logistic correction.
#[derive(Debug, Clone)]
pub struct HybridRanker {
    baseline: Arc<Smriti>,
    model: LogisticModel,
    model_weight: f32,
}

impl HybridRanker {
    pub fn new(baseline: Arc<Smriti>, model: LogisticModel) -> Self {
        Self {
            baseline,
            model,
            model_weight: MODEL_WEIGHT,
        }
    }

    /// Override the blend weight. Clamped to `0.0..=1.0` so a configuration
    /// mistake cannot let the model dominate the deterministic baseline.
    pub fn with_model_weight(mut self, model_weight: f32) -> Self {
        self.model_weight = if model_weight.is_finite() {
            model_weight.clamp(0.0, MODEL_WEIGHT)
        } else {
            MODEL_WEIGHT
        };
        self
    }

    pub fn model(&self) -> &LogisticModel {
        &self.model
    }

    pub fn model_weight(&self) -> f32 {
        self.model_weight
    }
}

impl Ranker for HybridRanker {
    fn rank(&self, request: &SuggestionRequest, candidates: Vec<Candidate>) -> Vec<Suggestion> {
        if candidates.len() > MAX_POOL || candidates.is_empty() {
            return self.baseline.rank(request, candidates);
        }

        let baseline_scores = self.baseline.score_pool(request, &candidates);
        if baseline_scores.iter().any(|score| !score.is_finite()) {
            return self.baseline.rank(request, candidates);
        }

        let source = SmritiFeatureSource(self.baseline.as_ref());
        let mut model_scores = Vec::with_capacity(candidates.len());
        let mut confidences = Vec::with_capacity(candidates.len());
        for candidate in &candidates {
            let features = match extract_features(request, candidate, &source) {
                Ok(features) => features,
                Err(_) => return self.baseline.rank(request, candidates),
            };
            match self.model.score(&features) {
                Ok(score) if score.is_finite() => model_scores.push(score),
                _ => return self.baseline.rank(request, candidates),
            }
            confidences.push(confidence(&features));
        }

        let count = model_scores.len() as f32;
        let mean = model_scores.iter().copied().sum::<f32>() / count;
        if !mean.is_finite() {
            return self.baseline.rank(request, candidates);
        }

        let mut final_scores = Vec::with_capacity(candidates.len());
        for index in 0..candidates.len() {
            let centred = model_scores[index] - mean;
            let score = baseline_scores[index] + confidences[index] * centred * self.model_weight;
            if !score.is_finite() {
                return self.baseline.rank(request, candidates);
            }
            final_scores.push(score);
        }

        let mut suggestions: Vec<Suggestion> = candidates
            .into_iter()
            .zip(final_scores)
            .map(|(candidate, score)| Suggestion {
                text: candidate.text,
                language: candidate.language,
                score,
                source: candidate.source,
            })
            .collect();

        suggestions.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.text.cmp(&right.text))
        });
        suggestions.truncate(request.limit.max(1));
        suggestions
    }
}

/// Per-candidate confidence in `0.0..=0.75`.
///
/// `confidence = 0` when the candidate has no learned evidence, which makes
/// the blend an exact baseline identity.
fn confidence(features: &FeatureVectorV1) -> f32 {
    let evidence = (1.0 + features.user_word_count as f32).ln()
        + (1.0 + features.pair_count as f32).ln()
        + 2.0 * features.acceptance_rate.clamp(0.0, 1.0);
    if !evidence.is_finite() || evidence <= 0.0 {
        return 0.0;
    }
    (CONFIDENCE_CAP * (1.0 - (-evidence / 10.0).exp())).clamp(0.0, CONFIDENCE_CAP)
}

#[cfg(test)]
mod tests {
    use super::*;
    use okkhor_core::CandidateSource;

    fn hybrid() -> HybridRanker {
        HybridRanker::new(
            Arc::new(Smriti::new()),
            crate::model::load_bundled().expect("bundled model"),
        )
    }

    fn candidates(count: usize) -> Vec<Candidate> {
        (0..count)
            .map(|index| {
                let mut candidate = Candidate::new(
                    format!("শব্দ{index:02}"),
                    Language::Bangla,
                    CandidateSource::Dictionary,
                );
                candidate.features.base_score = 1.0 - index as f32 * 0.01;
                candidate
            })
            .collect()
    }

    fn request(limit: usize) -> SuggestionRequest {
        SuggestionRequest::new("শব", Language::Bangla, limit)
    }

    #[test]
    fn confidence_zero_reproduces_the_baseline_exactly() {
        let ranker = hybrid();
        let baseline = ranker.baseline.rank(&request(10), candidates(5));
        let hybrid = ranker.rank(&request(10), candidates(5));
        assert_eq!(hybrid, baseline);

        let features = FeatureVectorV1 {
            base_score: 0.0,
            candidate_source: 0,
            language: 1,
            layout: 0,
            prefix_len: 1,
            word_len: 2,
            user_word_count: 0,
            pair_count: 0,
            recency: 0.0,
            acceptance_rate: 0.0,
            correction_rate: 0.0,
            bigram_score: 0.0,
            phonetic_score: 0.0,
            is_user_vocab: false,
            context_language: 0,
            script_mix: 0.0,
        };
        assert_eq!(confidence(&features), 0.0);
    }

    #[test]
    fn pool_over_the_cap_falls_back_to_baseline() {
        let ranker = hybrid();
        let baseline = ranker.baseline.rank(&request(10), candidates(MAX_POOL + 1));
        let hybrid = ranker.rank(&request(10), candidates(MAX_POOL + 1));
        assert_eq!(hybrid, baseline);
    }

    #[test]
    fn empty_pool_returns_nothing() {
        let ranker = hybrid();
        assert!(ranker.rank(&request(10), Vec::new()).is_empty());
    }

    #[test]
    fn non_finite_baseline_falls_back_to_baseline() {
        let ranker = hybrid();
        let mut pool = candidates(3);
        // Empty text has no word key, so the baseline score is non-finite.
        pool.push(Candidate::new(
            "",
            Language::Bangla,
            CandidateSource::Dictionary,
        ));
        let baseline = ranker.baseline.rank(&request(10), pool.clone());
        let hybrid = ranker.rank(&request(10), pool);
        assert_eq!(hybrid, baseline);
    }

    #[test]
    fn ranking_is_deterministic() {
        let ranker = hybrid();
        let first = ranker.rank(&request(10), candidates(8));
        let second = ranker.rank(&request(10), candidates(8));
        assert_eq!(first, second);
    }

    #[test]
    fn model_weight_is_capped_at_one() {
        let ranker = hybrid().with_model_weight(4.0).with_model_weight(f32::NAN);
        assert_eq!(ranker.model_weight(), MODEL_WEIGHT);
    }
}
