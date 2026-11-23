use thiserror::Error;

/// Typed failure for an AI request.
///
/// Variants carry codes and status numbers only. No prompt, response, key, or
/// window title is ever placed in an error value, so errors are safe to log and
/// to return across IPC.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AiError {
    /// The user has not opted in, or AI is switched off in settings.
    #[error("AI features are disabled")]
    Disabled,
    /// The provider needs an API key and none is present in the environment.
    #[error("AI provider API key is missing")]
    KeyMissing,
    /// The request failed policy validation (empty prompt, bad limits, model).
    #[error("invalid AI request")]
    InvalidRequest,
    /// The caller cancelled before or during the provider call.
    #[error("AI request was cancelled")]
    Cancelled,
    /// Too many requests in flight, or the circuit breaker is open.
    #[error("AI service is busy")]
    Busy,
    /// The provider call exceeded the request timeout.
    #[error("AI request timed out")]
    Timeout,
    /// The provider returned a non-success HTTP status.
    #[error("AI provider returned status {status}")]
    Provider {
        /// HTTP status returned by the provider.
        status: u16,
    },
    /// The provider response could not be parsed into a valid response.
    #[error("AI provider returned an invalid response")]
    InvalidResponse,
    /// An unexpected internal failure. Never includes request text.
    #[error("internal AI error")]
    Internal,
}

impl AiError {
    /// Whether this failure should count against the provider circuit breaker.
    ///
    /// Configuration errors (`Disabled`, `KeyMissing`, `InvalidRequest`),
    /// cancellation, local admission backpressure, and a disabled provider do
    /// not indicate provider ill health.
    pub(crate) fn counts_as_breaker_failure(&self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::Provider { .. } | Self::InvalidResponse | Self::Internal
        )
    }
}
