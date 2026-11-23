use crate::cancel::CancellationToken;
use crate::error::AiError;
use crate::request::{AiRequest, AiResponse, AiUsage};

/// Boundary between the orchestration core and a concrete AI backend.
///
/// Implementations must be cheap to call from a worker thread, must honour
/// [`CancellationToken`] before starting work, and must never log or persist
/// request or response text. A live HTTP implementation belongs behind a
/// feature in a later slice; this crate ships only [`MockProvider`] and
/// [`DisabledProvider`].
pub trait AiProvider {
    /// Stable provider name for status output. Never includes a key.
    fn name(&self) -> &'static str;

    /// Perform one completion.
    ///
    /// The provider should return [`AiError::Cancelled`] when cancellation is
    /// observed and [`AiError::Timeout`] when the request deadline expires.
    fn complete(
        &self,
        request: &AiRequest,
        cancel: &CancellationToken,
    ) -> Result<AiResponse, AiError>;
}

/// Deterministic offline provider used by tests and local demos.
///
/// The response text is the prompt converted with Unicode uppercase. The model
/// id is echoed from the request, and token counts are a stable estimate based
/// on character count. No clock or randomness is involved.
#[derive(Debug, Clone, Copy, Default)]
pub struct MockProvider;

impl AiProvider for MockProvider {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn complete(
        &self,
        request: &AiRequest,
        cancel: &CancellationToken,
    ) -> Result<AiResponse, AiError> {
        cancel.check()?;

        let text = request.prompt.to_uppercase();
        let input_tokens = request
            .system
            .as_deref()
            .map_or(0, estimate_tokens)
            .saturating_add(estimate_tokens(&request.prompt));
        let usage = AiUsage {
            input_tokens,
            output_tokens: estimate_tokens(&text),
        };

        Ok(AiResponse {
            text,
            model: request.model.clone(),
            usage,
            latency_ms: 0,
        })
    }
}

/// Provider for the opt-out and key-missing states.
///
/// It never performs work and always returns [`AiError::Disabled`], so an
/// accidental call without opt-in cannot reach a network client.
#[derive(Debug, Clone, Copy, Default)]
pub struct DisabledProvider;

impl AiProvider for DisabledProvider {
    fn name(&self) -> &'static str {
        "disabled"
    }

    fn complete(
        &self,
        _request: &AiRequest,
        _cancel: &CancellationToken,
    ) -> Result<AiResponse, AiError> {
        Err(AiError::Disabled)
    }
}

fn estimate_tokens(text: &str) -> u32 {
    let chars = text.chars().count();
    u32::try_from(chars.div_ceil(4)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_provider_is_deterministic() {
        let provider = MockProvider;
        let request = AiRequest::new("hello world", "mock-model");
        let cancel = CancellationToken::new();

        let first = provider.complete(&request, &cancel).expect("mock succeeds");
        let second = provider.complete(&request, &cancel).expect("mock succeeds");

        assert_eq!(first, second);
        assert_eq!(first.text, "HELLO WORLD");
        assert_eq!(first.model, "mock-model");
        assert_eq!(first.latency_ms, 0);
        assert!(first.usage.input_tokens > 0);
        assert!(first.usage.output_tokens > 0);
    }

    #[test]
    fn mock_provider_honours_cancellation() {
        let provider = MockProvider;
        let request = AiRequest::new("hello", "mock-model");
        let cancel = CancellationToken::new();
        cancel.cancel();

        assert_eq!(
            provider.complete(&request, &cancel),
            Err(AiError::Cancelled)
        );
    }

    #[test]
    fn disabled_provider_always_returns_disabled() {
        let provider = DisabledProvider;
        let request = AiRequest::new("hello", "mock-model");
        let cancel = CancellationToken::new();

        assert_eq!(provider.complete(&request, &cancel), Err(AiError::Disabled));
        assert_eq!(provider.name(), "disabled");
    }
}
