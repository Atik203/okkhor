use serde::{Deserialize, Serialize};

/// Whether a key transition is a press or a release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyPhase {
    Down,
    Up,
}

/// A normalized keyboard event from the low-level hook.
///
/// The hook must fill this without blocking or allocating unbounded data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyEvent {
    pub virtual_key: u16,
    pub scan_code: u16,
    pub phase: KeyPhase,
    pub timestamp_ms: u64,
}

/// Lifecycle for a platform keyboard hook.
pub trait KeyboardHook {
    type Error;

    fn start(&mut self) -> Result<(), Self::Error>;
    fn stop(&mut self);
}

#[cfg(test)]
mod tests {
    use super::{KeyEvent, KeyPhase};

    #[test]
    fn key_event_is_copy_and_comparable() {
        let event = KeyEvent {
            virtual_key: 0x41,
            scan_code: 0x1E,
            phase: KeyPhase::Down,
            timestamp_ms: 42,
        };
        let copy = event;
        assert_eq!(copy, event);
    }

    #[test]
    fn key_event_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<KeyEvent>();
    }
}
