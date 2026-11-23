use serde::{Deserialize, Serialize};

/// `LLKHF_INJECTED` from the Win32 `KBDLLHOOKSTRUCT.flags` field.
///
/// Defined here (instead of importing it from the `windows` crate) so the
/// predicate below stays available on every platform for unit tests. The value
/// matches `Win32::UI::WindowsAndMessaging::LLKHF_INJECTED` (`0x10`; the
/// neighbours are `LLKHF_EXTENDED = 0x01`, `LLKHF_ALTDOWN = 0x20`,
/// `LLKHF_UP = 0x80`).
pub const LLKHF_INJECTED: u32 = 0x0000_0010;

/// Whether a low-level hook event was injected via `SendInput`.
///
/// The hook must skip such events so our own backspace/text injection is never
/// re-captured. Pass-through is unaffected: the callback still forwards every
/// event down the hook chain. Allocation-free by construction.
pub const fn is_injected_event(flags: u32) -> bool {
    (flags & LLKHF_INJECTED) != 0
}

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
    use super::{KeyEvent, KeyPhase, LLKHF_INJECTED, is_injected_event};

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

    #[test]
    fn injected_flag_value_matches_win32() {
        assert_eq!(LLKHF_INJECTED, 0x0000_0010);
    }

    #[test]
    fn real_key_events_are_not_injected() {
        assert!(!is_injected_event(0));
    }

    #[test]
    fn injected_bit_is_detected() {
        assert!(is_injected_event(LLKHF_INJECTED));
    }

    #[test]
    fn other_flag_bits_do_not_count_as_injected() {
        // Neighbouring KBDLLHOOKSTRUCT bits must not trigger the filter:
        // LLKHF_EXTENDED (0x01), LLKHF_ALTDOWN (0x20), LLKHF_UP (0x80).
        for flags in [0x0000_0001u32, 0x0000_0020, 0x0000_0080, 0x0000_00A1] {
            assert!(
                !is_injected_event(flags),
                "flags {flags:#06X} is real input"
            );
        }
        // Combined flags still count as injected.
        assert!(is_injected_event(LLKHF_INJECTED | 0x0000_0080));
    }
}
