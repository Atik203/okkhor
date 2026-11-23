use std::fmt;
use std::time::Duration;

/// One explicit AI text action.
///
/// The prompt is user-supplied text and must never be logged or persisted by
/// this crate. [`Debug`] is redacted and reports only lengths.
#[derive(Clone, PartialEq)]
pub struct AiRequest {
    /// Text the user explicitly submitted.
    pub prompt: String,
    /// Optional system/instruction text.
    pub system: Option<String>,
    /// Provider model identifier, for example `gpt-6-luna`.
    pub model: String,
    /// Upper bound on generated tokens.
    pub max_tokens: u32,
    /// Sampling temperature. Must be finite and within `0.0..=2.0`.
    pub temperature: f32,
    /// Caller-side timeout for the provider request.
    pub timeout: Duration,
    /// Version of the prompt template, so edits invalidate cached responses.
    pub prompt_version: String,
}

impl AiRequest {
    /// Build a request with conservative defaults.
    ///
    /// Defaults: no system prompt, `max_tokens = 512`, `temperature = 0.2`,
    /// `timeout = 15 s`, `prompt_version = "v1"`.
    pub fn new(prompt: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            system: None,
            model: model.into(),
            max_tokens: 512,
            temperature: 0.2,
            timeout: Duration::from_secs(15),
            prompt_version: String::from("v1"),
        }
    }

    /// Attach a system/instruction prompt.
    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }
}

impl fmt::Debug for AiRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiRequest")
            .field("prompt_chars", &self.prompt.chars().count())
            .field(
                "system_chars",
                &self.system.as_ref().map(|system| system.chars().count()),
            )
            .field("model", &self.model)
            .field("max_tokens", &self.max_tokens)
            .field("temperature", &self.temperature)
            .field("timeout", &self.timeout)
            .field("prompt_version", &self.prompt_version)
            .finish()
    }
}

/// Token accounting reported by a provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AiUsage {
    /// Tokens sent to the provider.
    pub input_tokens: u32,
    /// Tokens produced by the provider.
    pub output_tokens: u32,
}

/// A successful provider response.
///
/// The response text is user-visible content and must never be logged.
/// [`Debug`] is redacted and reports only lengths.
#[derive(Clone, PartialEq, Eq)]
pub struct AiResponse {
    /// Generated text. Never inserted automatically by this crate.
    pub text: String,
    /// Model that produced the response.
    pub model: String,
    /// Token usage, if the provider reported it.
    pub usage: AiUsage,
    /// Provider-measured latency in milliseconds. Tests may leave this at `0`.
    pub latency_ms: u64,
}

impl fmt::Debug for AiResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiResponse")
            .field("text_chars", &self.text.chars().count())
            .field("model", &self.model)
            .field("usage", &self.usage)
            .field("latency_ms", &self.latency_ms)
            .finish()
    }
}
