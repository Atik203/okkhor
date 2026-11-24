use serde::{Deserialize, Serialize};

/// `LLKHF_INJECTED` from the Win32 `KBDLLHOOKSTRUCT.flags` field.
///
/// Defined here (instead of importing it from the `windows` crate) so the
/// predicate below stays available on every platform for unit tests. The value
/// matches `Win32::UI::WindowsAndMessaging::LLKHF_INJECTED` (`0x10`; the
/// neighbours are `LLKHF_EXTENDED = 0x01`, `LLKHF_ALTDOWN = 0x20`,
/// `LLKHF_UP = 0x80`).
pub const LLKHF_INJECTED: u32 = 0x0000_0010;

/// Virtual key of the full stop (`.` on the main row).
pub const VK_OEM_PERIOD: u16 = 0xBE;

/// Whether a `.` key-down must be swallowed to emit the Bengali danda (।).
///
/// Only for Bangla **Unicode** output without Shift: `>` stays available and
/// English mode keeps the ASCII full stop. In Bijoy mode the period passes
/// through (legacy documents own their punctuation).
pub const fn should_swallow_danda(
    capture_enabled: bool,
    bangla_mode: bool,
    bangla_unicode: bool,
    chord_pressed: bool,
    shift_pressed: bool,
    injected: bool,
    vk: u16,
) -> bool {
    capture_enabled
        && bangla_mode
        && bangla_unicode
        && !chord_pressed
        && !shift_pressed
        && !injected
        && vk == VK_OEM_PERIOD
}

/// Virtual key of Tab.
pub const VK_TAB_KEY: u16 = 0x09;

/// Whether the hook must swallow Tab for an English completion.
///
/// Only when capture is on, the active mode is English, the completion popup
/// is visible, no Ctrl/Alt/Win chord is held, and the event is real. The daemon
/// re-emits Tab if the accept path fails, so a swallowed Tab is never lost
/// silently.
pub const fn should_swallow_tab(
    capture_enabled: bool,
    english_mode: bool,
    popup_visible: bool,
    chord_pressed: bool,
    injected: bool,
    vk: u16,
) -> bool {
    capture_enabled
        && english_mode
        && popup_visible
        && !chord_pressed
        && !injected
        && vk == VK_TAB_KEY
}

/// Whether a digit key is a candidate selector or a Bengali numeral.
///
/// Row `0`-`9` and numpad `0`-`9`. While composing these are commit keys
/// (handled by [`should_swallow`]); while idle Bangla Unicode turns them into
/// `০`-`৯`, so they must be swallowed and re-emitted by the daemon too.
pub const fn is_digit_key(vk: u16) -> bool {
    matches!(vk, 0x30..=0x39 | 0x60..=0x69)
}

/// Whether an idle digit must be swallowed for Bangla Unicode numerals.
pub const fn should_swallow_numeral(
    capture_enabled: bool,
    bangla_mode: bool,
    bangla_unicode: bool,
    composing: bool,
    chord_pressed: bool,
    injected: bool,
    vk: u16,
) -> bool {
    capture_enabled
        && bangla_mode
        && bangla_unicode
        && !composing
        && !chord_pressed
        && !injected
        && is_digit_key(vk)
}

/// Whether a low-level hook event was injected via `SendInput`.
///
/// The hook must skip such events so our own backspace/text injection is never
/// re-captured. Pass-through is unaffected: the callback still forwards every
/// event down the hook chain. Allocation-free by construction.
pub const fn is_injected_event(flags: u32) -> bool {
    (flags & LLKHF_INJECTED) != 0
}

/// Whether the hook callback should enqueue this event for the worker.
///
/// False when capture is paused or the event was injected (`LLKHF_INJECTED`),
/// so our own `SendInput` erase/inject can never be re-captured and feed the
/// composition loop. This is the testable seam for the anti-loop filter: the
/// callback itself only forwards the decision's inputs. Pass-through is
/// decided by the caller — this predicate gates the queue only, never the
/// `CallNextHookEx` forward. Allocation-free by construction.
pub const fn should_enqueue(capture_enabled: bool, flags: u32) -> bool {
    capture_enabled && !is_injected_event(flags)
}

/// Virtual keys the daemon handles while a composition is active, and whose
/// physical press must therefore **not** reach the target application.
///
/// Letters and Backspace stay visible: one target character per buffer
/// character keeps the erase count exact. Space/Enter/digits commit a
/// candidate; the arrows move the highlight. If those reached the target they
/// would append characters (space, digit) or move the caret, so the commit's
/// backspaces would delete the wrong text — the exact "amar my word" bug where
/// the roman text survived next to the Bangla.
///
/// Coverage is numeric so the predicate stays platform-free for tests:
/// Enter/numpad Enter `0x0D`, Space `0x20`, arrows `0x25..=0x28`, row digits
/// `0x30..=0x39`, numpad digits `0x60..=0x69`.
pub const fn is_composition_key(vk: u16) -> bool {
    matches!(vk, 0x0D | 0x20 | 0x25..=0x28 | 0x30..=0x39 | 0x60..=0x69)
}

/// Whether the low-level hook must swallow this key-down event.
///
/// Swallowing happens only for a real Bangla composition: capture on, Bangla
/// mode, a non-empty buffer, no Ctrl/Alt/Win chord, a key from
/// [`is_composition_key`], and never for our own injected events (the commit
/// re-emits Space/Enter through `SendInput`; swallowing that would lose the
/// keypress and set up a phantom key-up). The event is still enqueued for the
/// worker — the hook swallows delivery to the target, not the decision itself.
pub const fn should_swallow(
    capture_enabled: bool,
    bangla_mode: bool,
    composing: bool,
    chord_pressed: bool,
    injected: bool,
    vk: u16,
) -> bool {
    capture_enabled
        && bangla_mode
        && composing
        && !chord_pressed
        && !injected
        && is_composition_key(vk)
}

/// Whether the hook must swallow an ASCII letter key-down in fixed-layout mode.
///
/// Virtual keys that a fixed layout can map. Space, Enter, Backspace, the
/// arrows and the modifier keys are deliberately absent: those must keep their
/// normal behavior. The set mirrors `okkhor_lang::fixed_layout::vk_to_key_name`.
pub const fn is_fixed_layout_key(vk: u16) -> bool {
    matches!(
        vk,
        0x30..=0x39
            | 0x41..=0x5A
            | 0x60..=0x6B
            | 0x6D..=0x6F
            | 0xC0
            | 0xBA..=0xBF
            | 0xDB..=0xDE
    )
}

/// In fixed mode the daemon injects the mapped character for every physical
/// key, so the raw key must never also reach the target. Swallowing letters
/// alone duplicated punctuation, Shift+punctuation and AltGr+digits.
/// Phonetic mode (`fixed_layout = false`) never swallows these keys because
/// the roman characters are the composition buffer and must stay visible. The
/// matching key-up is swallowed by the hook's last-swallowed-VK path.
pub const fn should_swallow_letters(
    capture_enabled: bool,
    bangla_mode: bool,
    fixed_layout: bool,
    chord_pressed: bool,
    injected: bool,
    vk: u16,
) -> bool {
    capture_enabled
        && bangla_mode
        && fixed_layout
        && !chord_pressed
        && !injected
        && is_fixed_layout_key(vk)
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
    use super::{
        KeyEvent, KeyPhase, LLKHF_INJECTED, VK_OEM_PERIOD, VK_TAB_KEY, is_composition_key,
        is_injected_event, should_enqueue, should_swallow, should_swallow_danda,
        should_swallow_letters, should_swallow_numeral, should_swallow_tab,
    };

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

    #[test]
    fn enqueue_matrix_real_vs_injected_and_capture_on_vs_off() {
        // Real keystroke, capture on: the only case that reaches the worker.
        assert!(should_enqueue(true, 0));
        // Our own SendInput (backspaces/text) while composing: never re-captured.
        assert!(!should_enqueue(true, LLKHF_INJECTED));
        // Injected flag combined with other bits is still skipped.
        assert!(!should_enqueue(true, LLKHF_INJECTED | 0x0000_0080));
        // Paused capture drops everything, real or injected.
        assert!(!should_enqueue(false, 0));
        assert!(!should_enqueue(false, LLKHF_INJECTED));
        // Neighbouring real-input bits still enqueue when capture is on.
        for flags in [0x0000_0001u32, 0x0000_0020, 0x0000_0080] {
            assert!(
                should_enqueue(true, flags),
                "flags {flags:#06X} must enqueue"
            );
        }
    }

    #[test]
    fn composition_keys_are_the_commit_and_arrow_set() {
        for vk in [
            0x0D, // Enter
            0x20, // Space
            0x25, 0x26, 0x27, 0x28, // arrows
            0x30, 0x35, 0x39, // row digits
            0x60, 0x65, 0x69, // numpad digits
        ] {
            assert!(is_composition_key(vk), "vk {vk:#04X} must be a commit key");
        }
        // Letters and Backspace stay visible and stay in sync one-for-one.
        for vk in [0x41, 0x5A, 0x08, 0x09, 0x1B] {
            assert!(!is_composition_key(vk), "vk {vk:#04X} must pass through");
        }
    }

    #[test]
    fn swallow_matrix_requires_a_live_bangla_composition() {
        let space = 0x20;
        // The only case that swallows: all conditions true.
        assert!(should_swallow(true, true, true, false, false, space));
        // Paused capture, English mode, idle buffer, or a chord: pass through.
        assert!(!should_swallow(false, true, true, false, false, space));
        assert!(!should_swallow(true, false, true, false, false, space));
        assert!(!should_swallow(true, true, false, false, false, space));
        assert!(!should_swallow(true, true, true, true, false, space));
        // Our own injected commit keys are never swallowed.
        assert!(!should_swallow(true, true, true, false, true, 0x0D));
        // A letter is never swallowed even mid-composition.
        assert!(!should_swallow(true, true, true, false, false, 0x41));
    }

    #[test]
    fn every_mapped_fixed_key_is_swallowed() {
        // The whole ASCII letter VK range, including both boundaries.
        for vk in 0x41..=0x5A {
            assert!(
                should_swallow_letters(true, true, true, false, false, vk),
                "vk {vk:#04X} must be swallowed in fixed mode"
            );
        }
        // Phonetic mode (fixed_layout = false) NEVER swallows letters, not even
        // while a composition buffer is active.
        assert!(!should_swallow_letters(
            true, true, false, false, false, 0x41
        ));
        // Every other gate alone is enough to pass the key through.
        assert!(!should_swallow_letters(
            false, true, true, false, false, 0x41
        )); // capture paused
        assert!(!should_swallow_letters(
            true, false, true, false, false, 0x41
        )); // English mode
        assert!(!should_swallow_letters(true, true, true, true, false, 0x41)); // Ctrl/Alt/Win chord
        assert!(!should_swallow_letters(true, true, true, false, true, 0x41)); // injected event
        // Digits, numpad and OEM punctuation are mapped by fixed layouts and
        // must be swallowed too, otherwise the target receives them twice.
        for vk in [0x30, 0x39, 0x60, 0x69, 0xBA, 0xBE, 0xC0, 0xDB] {
            assert!(
                should_swallow_letters(true, true, true, false, false, vk),
                "vk {vk:#04X} is mapped by fixed layouts"
            );
        }
        // Commit keys and modifiers are never swallowed.
        for vk in [0x40, 0x5B, 0x08, 0x09, 0x0D, 0x20, 0x10, 0x12] {
            assert!(
                !should_swallow_letters(true, true, true, false, false, vk),
                "vk {vk:#04X} must pass through"
            );
        }
        // The predicate is usable in a constant context from the hook.
        const { assert!(should_swallow_letters(true, true, true, false, false, 0x41)) };
    }

    #[test]
    fn danda_swallow_matrix() {
        // Bangla Unicode, unshifted, no chord: the `.` becomes `।`.
        assert!(should_swallow_danda(
            true,
            true,
            true,
            false,
            false,
            false,
            VK_OEM_PERIOD
        ));
        // Shift keeps `>`, English mode keeps `.`, Bijoy keeps legacy bytes.
        assert!(!should_swallow_danda(
            true,
            true,
            true,
            false,
            true,
            false,
            VK_OEM_PERIOD
        ));
        assert!(!should_swallow_danda(
            true,
            false,
            true,
            false,
            false,
            false,
            VK_OEM_PERIOD
        ));
        assert!(!should_swallow_danda(
            true,
            true,
            false,
            false,
            false,
            false,
            VK_OEM_PERIOD
        ));
        // Chords, paused capture, injected events, and other keys pass.
        assert!(!should_swallow_danda(
            true,
            true,
            true,
            true,
            false,
            false,
            VK_OEM_PERIOD
        ));
        assert!(!should_swallow_danda(
            false,
            true,
            true,
            false,
            false,
            false,
            VK_OEM_PERIOD
        ));
        assert!(!should_swallow_danda(
            true,
            true,
            true,
            false,
            false,
            true,
            VK_OEM_PERIOD
        ));
        assert!(!should_swallow_danda(
            true, true, true, false, false, false, 0x41
        ));
    }

    #[test]
    fn numeral_swallow_matrix() {
        // Idle Bangla Unicode: digits become Bengali numerals.
        assert!(should_swallow_numeral(
            true, true, true, false, false, false, 0x31
        ));
        assert!(should_swallow_numeral(
            true, true, true, false, false, false, 0x60
        ));
        // While composing they are commit keys, not numerals.
        assert!(!should_swallow_numeral(
            true, true, true, true, false, false, 0x31
        ));
        // English, Bijoy, chords, injected, paused, letters: pass through.
        assert!(!should_swallow_numeral(
            true, false, true, false, false, false, 0x31
        ));
        assert!(!should_swallow_numeral(
            true, true, false, false, false, false, 0x31
        ));
        assert!(!should_swallow_numeral(
            true, true, true, false, true, false, 0x31
        ));
        assert!(!should_swallow_numeral(
            true, true, true, false, false, true, 0x31
        ));
        assert!(!should_swallow_numeral(
            false, true, true, false, false, false, 0x31
        ));
        assert!(!should_swallow_numeral(
            true, true, true, false, false, false, 0x41
        ));
    }

    #[test]
    fn tab_is_swallowed_only_for_a_visible_english_popup() {
        assert!(should_swallow_tab(
            true, true, true, false, false, VK_TAB_KEY
        ));
        // Bangla mode: Tab cancels composition, not an English accept.
        assert!(!should_swallow_tab(
            true, false, true, false, false, VK_TAB_KEY
        ));
        // Hidden popup: Tab must move focus normally.
        assert!(!should_swallow_tab(
            true, true, false, false, false, VK_TAB_KEY
        ));
        // Chords and injected events pass.
        assert!(!should_swallow_tab(
            true, true, true, true, false, VK_TAB_KEY
        ));
        assert!(!should_swallow_tab(
            true, true, true, false, true, VK_TAB_KEY
        ));
        // Paused capture passes.
        assert!(!should_swallow_tab(
            false, true, true, false, false, VK_TAB_KEY
        ));
        // Other keys pass.
        assert!(!should_swallow_tab(true, true, true, false, false, 0x41));
    }
}
