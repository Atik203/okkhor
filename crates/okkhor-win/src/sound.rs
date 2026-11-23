//! Short UI feedback sounds.
//!
//! The 0.5 toggle click is a tiny generated WAV embedded in the binary, so the
//! portable artifact needs no extra asset next to the exe and playback cannot
//! fail because of a missing file. Playback is asynchronous
//! (`SND_ASYNC | SND_MEMORY`) and never blocks the keyboard path.

/// Play the short Bangla/English toggle click.
///
/// No-op on non-Windows hosts. Playback failures are deliberately ignored:
/// a missing sound device must never affect typing or the mode switch.
pub fn play_toggle_click() {
    #[cfg(windows)]
    imp::play_toggle_click();
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
    use windows::core::PCWSTR;

    /// Generated 18 ms click (mono, 22.05 kHz, 16-bit PCM).
    const TOGGLE_CLICK_WAV: &[u8] = include_bytes!("../../../assets/sounds/toggle-click.wav");

    pub fn play_toggle_click() {
        // SAFETY: `TOGGLE_CLICK_WAV` is a valid in-memory WAV image that lives
        // for the whole process (static bytes). With `SND_MEMORY` the pointer
        // is read as sound data, not as a NUL-terminated string. The call only
        // queues asynchronous playback and does not retain the pointer beyond
        // the call in a way Rust must manage.
        unsafe {
            let _ = PlaySoundW(
                PCWSTR(TOGGLE_CLICK_WAV.as_ptr().cast()),
                None,
                SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
            );
        }
    }
}
