use crate::error::AiError;
use crate::request::AiRequest;

/// Ceiling accepted by [`AiPolicy::validate`] for `max_tokens`.
pub const MAX_TOKENS_LIMIT: u32 = 8192;

/// Safe-by-default policy for AI actions.
///
/// The default is opt-out: AI is disabled, one request may be in flight, prompts
/// are capped at 20,000 characters, and an empty `allowed_models` list means any
/// model id is accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiPolicy {
    /// Whether the user has opted in to AI actions.
    pub enabled: bool,
    /// Maximum concurrent requests admitted by the service. Clamped to at least
    /// one by [`crate::AiService`].
    pub max_in_flight: usize,
    /// Maximum prompt length in Unicode scalar values.
    pub max_prompt_chars: usize,
    /// Optional allowlist of model ids. Empty means any non-empty model.
    pub allowed_models: Vec<String>,
}

impl Default for AiPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            max_in_flight: 1,
            max_prompt_chars: 20_000,
            allowed_models: Vec::new(),
        }
    }
}

impl AiPolicy {
    /// Validate one request against the policy.
    ///
    /// Checks, in order: opt-in, non-blank prompt, prompt character cap,
    /// non-blank model, allowlist, finite temperature in `0.0..=2.0`, and
    /// `max_tokens` in `1..=8192`. Rejections use [`AiError::Disabled`] for
    /// opt-out and [`AiError::InvalidRequest`] otherwise. Error values never
    /// contain prompt text.
    pub fn validate(&self, request: &AiRequest) -> Result<(), AiError> {
        if !self.enabled {
            return Err(AiError::Disabled);
        }
        if request.prompt.trim().is_empty() {
            return Err(AiError::InvalidRequest);
        }
        if request.prompt.chars().count() > self.max_prompt_chars {
            return Err(AiError::InvalidRequest);
        }
        if request.model.trim().is_empty() {
            return Err(AiError::InvalidRequest);
        }
        if !self.allowed_models.is_empty()
            && !self
                .allowed_models
                .iter()
                .any(|model| model == &request.model)
        {
            return Err(AiError::InvalidRequest);
        }
        if !request.temperature.is_finite() || !(0.0..=2.0).contains(&request.temperature) {
            return Err(AiError::InvalidRequest);
        }
        if !(1..=MAX_TOKENS_LIMIT).contains(&request.max_tokens) {
            return Err(AiError::InvalidRequest);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::AiRequest;

    fn enabled_policy() -> AiPolicy {
        AiPolicy {
            enabled: true,
            ..AiPolicy::default()
        }
    }

    fn valid_request() -> AiRequest {
        AiRequest::new("hello", "gpt-6-luna")
    }

    #[test]
    fn default_is_opt_out() {
        let policy = AiPolicy::default();
        assert!(!policy.enabled);
        assert_eq!(policy.max_in_flight, 1);
        assert_eq!(policy.max_prompt_chars, 20_000);
        assert!(policy.allowed_models.is_empty());
        assert_eq!(policy.validate(&valid_request()), Err(AiError::Disabled));
    }

    #[test]
    fn enabled_policy_accepts_a_valid_request() {
        assert_eq!(enabled_policy().validate(&valid_request()), Ok(()));
    }

    #[test]
    fn rejects_blank_prompt() {
        let mut request = valid_request();
        request.prompt = String::from("   ");
        assert_eq!(
            enabled_policy().validate(&request),
            Err(AiError::InvalidRequest)
        );
    }

    #[test]
    fn rejects_prompt_over_the_character_cap() {
        let mut policy = enabled_policy();
        policy.max_prompt_chars = 4;

        let mut request = valid_request();
        request.prompt = String::from("hello");
        assert_eq!(policy.validate(&request), Err(AiError::InvalidRequest));

        request.prompt = String::from("hell");
        assert_eq!(policy.validate(&request), Ok(()));
    }

    #[test]
    fn prompt_cap_counts_characters_not_bytes() {
        let mut policy = enabled_policy();
        policy.max_prompt_chars = 5;

        let mut request = valid_request();
        request.prompt = String::from("বাংলা");
        assert_eq!(request.prompt.chars().count(), 5);
        assert_eq!(policy.validate(&request), Ok(()));
    }

    #[test]
    fn rejects_blank_model() {
        let mut request = valid_request();
        request.model = String::from("  ");
        assert_eq!(
            enabled_policy().validate(&request),
            Err(AiError::InvalidRequest)
        );
    }

    #[test]
    fn rejects_disallowed_model() {
        let mut policy = enabled_policy();
        policy.allowed_models = vec![String::from("gpt-6-luna")];

        assert_eq!(policy.validate(&valid_request()), Ok(()));

        let mut other = valid_request();
        other.model = String::from("gpt-6.1-terra");
        assert_eq!(policy.validate(&other), Err(AiError::InvalidRequest));
    }

    #[test]
    fn rejects_non_finite_or_out_of_range_temperature() {
        let policy = enabled_policy();
        for temperature in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.1, 2.1] {
            let mut request = valid_request();
            request.temperature = temperature;
            assert_eq!(
                policy.validate(&request),
                Err(AiError::InvalidRequest),
                "temperature {temperature} should be rejected"
            );
        }

        for temperature in [0.0, 1.0, 2.0] {
            let mut request = valid_request();
            request.temperature = temperature;
            assert_eq!(policy.validate(&request), Ok(()));
        }
    }

    #[test]
    fn rejects_out_of_range_max_tokens() {
        let policy = enabled_policy();
        for max_tokens in [0, 8193, u32::MAX] {
            let mut request = valid_request();
            request.max_tokens = max_tokens;
            assert_eq!(
                policy.validate(&request),
                Err(AiError::InvalidRequest),
                "max_tokens {max_tokens} should be rejected"
            );
        }

        for max_tokens in [1, 8192] {
            let mut request = valid_request();
            request.max_tokens = max_tokens;
            assert_eq!(policy.validate(&request), Ok(()));
        }
    }
}
