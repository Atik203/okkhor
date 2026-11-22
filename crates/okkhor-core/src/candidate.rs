use crate::Language;
use serde::{Deserialize, Serialize};

/// Where a suggestion came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateSource {
    Dictionary,
    Transliteration,
    UserHistory,
    Recent,
    Context,
    Ai,
}

/// Precomputed hints supplied by a candidate generator.
///
/// `okkhor-learn` adds persisted user statistics on top of these values. Keep
/// them as hints, not as a second copy of the learned state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RankingFeatures {
    pub base_score: f32,
    pub user_frequency: u32,
    pub recency_score: f32,
    pub context_score: f32,
    pub acceptance_rate: f32,
    pub correction_penalty: f32,
    pub phonetic_score: f32,
    pub pair_frequency: u32,
}

/// One candidate before personal ranking.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub text: String,
    pub language: Language,
    pub source: CandidateSource,
    pub features: RankingFeatures,
}

impl Candidate {
    pub fn new(text: impl Into<String>, language: Language, source: CandidateSource) -> Self {
        Self {
            text: text.into(),
            language,
            source,
            features: RankingFeatures::default(),
        }
    }
}

/// A ranked suggestion ready for the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    pub text: String,
    pub language: Language,
    pub score: f32,
    pub source: CandidateSource,
}

/// Input to candidate generation and ranking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestionRequest {
    pub prefix: String,
    pub language: Language,
    pub layout: Option<String>,
    /// Previous committed words, oldest first. Keep this short and bounded.
    pub context: Vec<String>,
    /// The Romanized input that produced the current prefix, when applicable.
    pub roman_input: Option<String>,
    /// Logical time for deterministic recency scoring. `0` disables decay.
    pub now_ms: u64,
    pub limit: usize,
}

impl SuggestionRequest {
    pub fn new(prefix: impl Into<String>, language: Language, limit: usize) -> Self {
        Self {
            prefix: prefix.into(),
            language,
            layout: None,
            context: Vec::new(),
            roman_input: None,
            now_ms: 0,
            limit,
        }
    }

    pub fn with_layout(mut self, layout: impl Into<String>) -> Self {
        self.layout = Some(layout.into());
        self
    }

    pub fn with_context<I, S>(mut self, context: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.context = context.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_roman_input(mut self, roman_input: impl Into<String>) -> Self {
        self.roman_input = Some(roman_input.into());
        self
    }

    pub fn at(mut self, now_ms: u64) -> Self {
        self.now_ms = now_ms;
        self
    }
}

/// Produces local candidates. Implementations must be fast and side-effect free
/// on the typing path.
pub trait CandidateProvider {
    fn candidates(&self, request: &SuggestionRequest) -> Vec<Candidate>;
}

/// Ranks candidates. The implementation must not perform I/O or network calls.
pub trait Ranker {
    fn rank(&self, request: &SuggestionRequest, candidates: Vec<Candidate>) -> Vec<Suggestion>;
}
