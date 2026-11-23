use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::AiError;

/// A cloneable cancellation flag shared by a caller and a provider call.
///
/// `CancellationToken::clone` shares the same flag. Cancellation is monotonic:
/// once cancelled, it stays cancelled.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    flag: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Create a fresh, uncancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark the operation cancelled.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Return [`AiError::Cancelled`] if cancellation has been requested.
    pub fn check(&self) -> Result<(), AiError> {
        if self.is_cancelled() {
            Err(AiError::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_token_is_not_cancelled() {
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());
        assert_eq!(token.check(), Ok(()));
    }

    #[test]
    fn clones_share_the_flag() {
        let token = CancellationToken::new();
        let clone = token.clone();
        token.cancel();
        assert!(clone.is_cancelled());
        assert_eq!(clone.check(), Err(AiError::Cancelled));
    }
}
