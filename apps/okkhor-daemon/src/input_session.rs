//! Pure Bengali composition state machine for the daemon worker.
//!
//! This module has no Win32 calls, no I/O, no threads, and no logging. It owns
//! the roman buffer (bounded to [`MAX_BUFFER_LEN`] ASCII letters), the current
//! candidates (bounded to [`MAX_CANDIDATES`]), and the selection index. All
//! language work goes through [`TransliteratorEngine`]; the daemon only maps
//! hook key codes to the semantic methods here.
//!
//! Privacy: the buffer holds transient keystrokes in memory only. It is never
//! logged, never persisted, and cleared on commit, cancel, and pause.

use okkhor_transliterate::{
    RuleSet, RuleTransliterator, TransliterationOptions, TransliteratorEngine,
};

/// Maximum roman letters kept in the composition buffer.
pub const MAX_BUFFER_LEN: usize = 32;
/// Maximum candidates shown in the popup and addressable by digit keys.
pub const MAX_CANDIDATES: usize = 5;

/// Virtual-key codes handled by the session. These are plain `u16`
/// constants so this module stays free of Win32 types.
pub const VK_BACK: u16 = 0x08;
pub const VK_TAB: u16 = 0x09;
pub const VK_RETURN: u16 = 0x0D;
pub const VK_ESCAPE: u16 = 0x1B;
pub const VK_SPACE: u16 = 0x20;
pub const VK_LEFT: u16 = 0x25;
pub const VK_UP: u16 = 0x26;
pub const VK_RIGHT: u16 = 0x27;
pub const VK_DOWN: u16 = 0x28;

/// A committed composition: what the daemon must erase and inject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// Roman characters to erase. Always equals the buffer length at commit
    /// time; the buffer only ever holds ASCII letters, so bytes == chars.
    pub erase_chars: usize,
    /// Bangla text to inject in place of the roman buffer.
    pub text: String,
    /// Whether the commit was triggered by Space (inject one more space).
    pub trailing_space: bool,
}

/// Outcome of a Backspace press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackspaceOutcome {
    /// The buffer was already empty; the key must pass through untouched.
    Empty,
    /// A character was removed and composition continues.
    StillComposing,
    /// The last character was removed; the session is now idle.
    BecameEmpty,
}

/// Map a virtual-key code to a composition letter (`A`–`Z`, lowercased).
///
/// Returns `None` for anything that is not an ASCII letter key.
pub fn vk_to_letter(vk: u16) -> Option<u8> {
    if (0x41..=0x5A).contains(&vk) {
        Some((vk as u8) + 32)
    } else {
        None
    }
}

/// Map a virtual-key code to a zero-based candidate index.
///
/// Accepts the `1`–`5` row keys (`0x31`–`0x35`) and the numpad `1`–`5` keys
/// (`0x61`–`0x65`). Returns `None` for any other key.
pub fn vk_to_digit_index(vk: u16) -> Option<usize> {
    match vk {
        0x31..=0x35 => Some((vk - 0x31) as usize),
        0x61..=0x65 => Some((vk - 0x61) as usize),
        _ => None,
    }
}

/// Pure composition session: roman buffer, candidates, selection.
///
/// Bounded: the buffer holds at most [`MAX_BUFFER_LEN`] bytes and the
/// candidate list at most [`MAX_CANDIDATES`] strings. No logging, no I/O.
pub struct InputSession<E = RuleTransliterator> {
    engine: E,
    buffer: String,
    candidates: Vec<String>,
    selected: usize,
}

impl<E: TransliteratorEngine> InputSession<E> {
    /// Create an idle session backed by `engine`.
    pub fn new(engine: E) -> Self {
        Self {
            engine,
            buffer: String::with_capacity(MAX_BUFFER_LEN),
            candidates: Vec::with_capacity(MAX_CANDIDATES),
            selected: 0,
        }
    }

    /// Current roman buffer (ASCII letters only).
    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    /// Current candidates, most likely first. Empty when idle.
    pub fn candidates(&self) -> &[String] {
        &self.candidates
    }

    /// Index into [`candidates`](Self::candidates); always `0` when idle.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Whether a composition is in progress.
    pub fn is_composing(&self) -> bool {
        !self.buffer.is_empty()
    }

    /// Append one ASCII letter (case-insensitive) to the buffer.
    ///
    /// Returns `false` without changing state when `letter` is not
    /// `a`–`z`/`A`–`Z` or the buffer is already full.
    pub fn push_letter(&mut self, letter: u8) -> bool {
        let lower = if letter.is_ascii_uppercase() {
            letter + 32
        } else {
            letter
        };
        if !(lower as char).is_ascii_lowercase() {
            return false;
        }
        if self.buffer.len() >= MAX_BUFFER_LEN {
            return false;
        }
        self.buffer.push(lower as char);
        self.refresh();
        true
    }

    /// Handle Backspace: edit the buffer, never beyond it.
    pub fn backspace(&mut self) -> BackspaceOutcome {
        if self.buffer.pop().is_none() {
            return BackspaceOutcome::Empty;
        }
        self.refresh();
        if self.buffer.is_empty() {
            BackspaceOutcome::BecameEmpty
        } else {
            BackspaceOutcome::StillComposing
        }
    }

    /// Move the selection by `delta` steps, wrapping around the list.
    ///
    /// No-op when fewer than two candidates exist.
    pub fn move_selection(&mut self, delta: i32) {
        let len = self.candidates.len();
        if len < 2 {
            return;
        }
        let len = len as i32;
        let next = (self.selected as i32 + delta).rem_euclid(len);
        self.selected = next as usize;
    }

    /// Commit the currently selected candidate (default index 0).
    ///
    /// Returns `None` — leaving all state untouched — when the buffer is
    /// empty or no candidate exists, so the key passes through and the roman
    /// text stays as typed. On success the session returns to idle.
    pub fn commit(&mut self, trailing_space: bool) -> Option<Commit> {
        if self.buffer.is_empty() || self.candidates.is_empty() {
            return None;
        }
        let index = self.selected.min(self.candidates.len() - 1);
        let commit = Commit {
            erase_chars: self.buffer.len(),
            text: self.candidates[index].clone(),
            trailing_space,
        };
        self.clear();
        Some(commit)
    }

    /// Select candidate `index` (0-based, from digit keys 1–5) and commit it.
    ///
    /// Returns `None` — leaving all state untouched — when idle, when no
    /// candidates exist, or when `index` is out of range.
    pub fn select_digit(&mut self, index: usize) -> Option<Commit> {
        if self.buffer.is_empty() || self.candidates.is_empty() || index >= self.candidates.len() {
            return None;
        }
        self.selected = index;
        self.commit(false)
    }

    /// Cancel the composition and return the raw roman buffer unchanged.
    ///
    /// The caller leaves already-typed text untouched and only hides the
    /// popup. Returns `None` when idle.
    pub fn cancel(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            return None;
        }
        let raw = std::mem::replace(&mut self.buffer, String::with_capacity(MAX_BUFFER_LEN));
        self.clear();
        Some(raw)
    }

    /// Pause reset: drop the buffer, candidates, and selection silently.
    pub fn reset(&mut self) {
        self.clear();
    }

    fn clear(&mut self) {
        self.buffer.clear();
        self.candidates.clear();
        self.selected = 0;
    }

    fn refresh(&mut self) {
        self.candidates.clear();
        self.selected = 0;
        if self.buffer.is_empty() {
            return;
        }
        let options = TransliterationOptions {
            max_candidates: MAX_CANDIDATES,
            allow_partial: true,
        };
        for output in self
            .engine
            .transliterate(&self.buffer, &options)
            .into_iter()
            .take(MAX_CANDIDATES)
        {
            self.candidates.push(output.text);
        }
    }
}

/// Build the engine from the rule table committed with this milestone.
///
/// The table is embedded so the daemon needs no dictionary file or I/O on the
/// key path. It covers the Bangla vowels, common consonants, and a small set
/// of frequent words; ranking beyond literal prefix order is future work.
pub fn default_engine() -> RuleTransliterator {
    const RULES_JSON: &str = r#"{
        "schema": 1,
        "rules": [
            { "id": "rule_a", "input": "a", "output": "আ", "language": "bangla", "priority": 100 },
            { "id": "rule_i", "input": "i", "output": "ই", "language": "bangla", "priority": 100 },
            { "id": "rule_u", "input": "u", "output": "উ", "language": "bangla", "priority": 100 },
            { "id": "rule_e", "input": "e", "output": "এ", "language": "bangla", "priority": 100 },
            { "id": "rule_o", "input": "o", "output": "অ", "language": "bangla", "priority": 100 },
            { "id": "rule_k", "input": "k", "output": "ক", "language": "bangla", "priority": 100 },
            { "id": "rule_g", "input": "g", "output": "গ", "language": "bangla", "priority": 100 },
            { "id": "rule_c", "input": "c", "output": "চ", "language": "bangla", "priority": 100 },
            { "id": "rule_j", "input": "j", "output": "জ", "language": "bangla", "priority": 100 },
            { "id": "rule_t", "input": "t", "output": "ত", "language": "bangla", "priority": 100 },
            { "id": "rule_d", "input": "d", "output": "দ", "language": "bangla", "priority": 100 },
            { "id": "rule_n", "input": "n", "output": "ন", "language": "bangla", "priority": 100 },
            { "id": "rule_p", "input": "p", "output": "প", "language": "bangla", "priority": 100 },
            { "id": "rule_b", "input": "b", "output": "ব", "language": "bangla", "priority": 100 },
            { "id": "rule_m", "input": "m", "output": "ম", "language": "bangla", "priority": 100 },
            { "id": "rule_r", "input": "r", "output": "র", "language": "bangla", "priority": 100 },
            { "id": "rule_l", "input": "l", "output": "ল", "language": "bangla", "priority": 100 },
            { "id": "rule_s", "input": "s", "output": "স", "language": "bangla", "priority": 100 },
            { "id": "rule_h", "input": "h", "output": "হ", "language": "bangla", "priority": 100 },
            { "id": "rule_y", "input": "y", "output": "য", "language": "bangla", "priority": 100 },
            { "id": "rule_f", "input": "f", "output": "ফ", "language": "bangla", "priority": 80 },
            { "id": "rule_kh", "input": "kh", "output": "খ", "language": "bangla", "priority": 50 },
            { "id": "rule_ch", "input": "ch", "output": "ছ", "language": "bangla", "priority": 50 },
            { "id": "rule_th", "input": "th", "output": "থ", "language": "bangla", "priority": 50 },
            { "id": "rule_dh", "input": "dh", "output": "ধ", "language": "bangla", "priority": 50 },
            { "id": "rule_ph", "input": "ph", "output": "ফ", "language": "bangla", "priority": 50 },
            { "id": "rule_bh", "input": "bh", "output": "ভ", "language": "bangla", "priority": 50 },
            { "id": "rule_sh", "input": "sh", "output": "শ", "language": "bangla", "priority": 50 },
            { "id": "rule_bha", "input": "bha", "output": "ভা", "language": "bangla", "priority": 50 },
            { "id": "rule_ami", "input": "ami", "output": "আমি", "language": "bangla", "priority": 10 },
            { "id": "rule_tumi", "input": "tumi", "output": "তুমি", "language": "bangla", "priority": 10 },
            { "id": "rule_bhalo", "input": "bhalo", "output": "ভালো", "language": "bangla", "priority": 10 },
            { "id": "rule_kotha", "input": "kotha", "output": "কথা", "language": "bangla", "priority": 10 },
            { "id": "rule_bangla", "input": "bangla", "output": "বাংলা", "language": "bangla", "priority": 10 },
            { "id": "rule_kore", "input": "kore", "output": "করে", "language": "bangla", "priority": 10 },
            { "id": "rule_ache", "input": "ache", "output": "আছে", "language": "bangla", "priority": 10 },
            { "id": "rule_na", "input": "na", "output": "না", "language": "bangla", "priority": 10 },
            { "id": "rule_ki", "input": "ki", "output": "কি", "language": "bangla", "priority": 10 },
            { "id": "rule_tomake", "input": "tomake", "output": "তোমাকে", "language": "bangla", "priority": 10 },
            { "id": "rule_dhonnobad", "input": "dhonnobad", "output": "ধন্যবাদ", "language": "bangla", "priority": 10 }
        ]
    }"#;
    let rules = RuleSet::from_json_str(RULES_JSON).expect("embedded rules are valid");
    RuleTransliterator::new(rules).expect("embedded rules validate")
}

#[cfg(test)]
mod tests {
    use super::{
        BackspaceOutcome, InputSession, MAX_BUFFER_LEN, MAX_CANDIDATES, default_engine,
        vk_to_digit_index, vk_to_letter,
    };
    use okkhor_core::Language;
    use okkhor_transliterate::{
        RuleId, TransliterationOptions, TransliterationOutput, TransliteratorEngine,
    };

    fn session() -> InputSession {
        InputSession::new(default_engine())
    }

    fn type_text<E: TransliteratorEngine>(session: &mut InputSession<E>, text: &str) {
        for byte in text.bytes() {
            assert!(session.push_letter(byte), "letter {byte} accepted");
        }
    }

    /// Stub engine yielding nothing: makes "no candidates" tests independent
    /// of the committed rule table.
    struct EmptyEngine;

    impl TransliteratorEngine for EmptyEngine {
        fn transliterate(
            &self,
            _input: &str,
            _options: &TransliterationOptions,
        ) -> Vec<TransliterationOutput> {
            Vec::new()
        }
    }

    /// Stub engine yielding more than the cap: proves truncation is the
    /// session's doing, not an accident of the rule table.
    struct OverflowEngine;

    impl TransliteratorEngine for OverflowEngine {
        fn transliterate(
            &self,
            _input: &str,
            options: &TransliterationOptions,
        ) -> Vec<TransliterationOutput> {
            (0..options.max_candidates + 3)
                .map(|index| TransliterationOutput {
                    text: format!("ক{index}"),
                    language: Language::Bangla,
                    confidence: 1.0,
                    rule_id: RuleId::from_code("rule_stub").expect("valid stub id"),
                })
                .collect()
        }
    }

    #[test]
    fn default_engine_builds_with_committed_rules() {
        assert!(default_engine().rule_count() > 0);
    }

    #[test]
    fn decision_latency_stays_in_budget() {
        // Unit-level poll-to-decision cost: one letter push including the
        // transliterate refresh, exactly as the worker performs it per
        // keystroke. This is NOT an end-to-end hook-to-commit claim — that
        // needs a real-key manual run (see docs/input-pipeline.md).
        let mut session = session();
        let iterations = 1_000u32;
        let start = std::time::Instant::now();
        for _ in 0..iterations {
            session.reset();
            for byte in *b"bhalo" {
                assert!(session.push_letter(byte));
            }
        }
        let average = start.elapsed() / iterations;
        println!("poll-to-decision average over {iterations} iterations: {average:?}");
        // Generous bound: the AGENTS budget for local candidate generation is
        // < 3 ms p99; a decision here is one push + refresh.
        assert!(
            average < std::time::Duration::from_millis(3),
            "average decision {average:?} exceeds the 3 ms budget"
        );
    }

    #[test]
    fn letters_build_buffer_and_top_candidate() {
        let mut session = session();
        type_text(&mut session, "bhalo");
        assert_eq!(session.buffer(), "bhalo");
        assert!(session.is_composing());
        assert_eq!(session.selected(), 0);
        assert_eq!(
            session.candidates().first().map(String::as_str),
            Some("ভালো")
        );
    }

    #[test]
    fn uppercase_letters_fold_to_lowercase() {
        let mut session = session();
        assert!(session.push_letter(b'A'));
        assert_eq!(session.buffer(), "a");
    }

    #[test]
    fn non_letters_are_rejected() {
        let mut session = session();
        for byte in [b'1', b' ', b'-', 0xE0] {
            assert!(!session.push_letter(byte), "byte {byte} rejected");
        }
        assert!(!session.is_composing());
        assert!(session.candidates().is_empty());
    }

    #[test]
    fn backspace_edits_and_reports_transitions() {
        let mut session = session();
        type_text(&mut session, "bh");
        assert_eq!(session.backspace(), BackspaceOutcome::StillComposing);
        assert_eq!(session.buffer(), "b");
        assert_eq!(session.backspace(), BackspaceOutcome::BecameEmpty);
        assert!(!session.is_composing());
        assert!(session.candidates().is_empty());
        assert_eq!(session.backspace(), BackspaceOutcome::Empty);
    }

    #[test]
    fn backspace_on_empty_buffer_passes_through() {
        let mut session = session();
        assert_eq!(session.backspace(), BackspaceOutcome::Empty);
        assert!(!session.is_composing());
    }

    #[test]
    fn escape_cancels_and_returns_raw_roman() {
        let mut session = session();
        type_text(&mut session, "bha");
        assert_eq!(session.cancel(), Some("bha".to_owned()));
        assert!(!session.is_composing());
        assert!(session.candidates().is_empty());
        assert_eq!(session.selected(), 0);
        assert_eq!(session.cancel(), None);
    }

    #[test]
    fn digits_select_and_commit() {
        let mut session = session();
        type_text(&mut session, "bha");
        assert!(session.candidates().len() >= 3);
        let second = session.candidates()[1].clone();
        let commit = session.select_digit(1).expect("digit 2 commits");
        assert_eq!(commit.text, second);
        assert_eq!(commit.erase_chars, 3);
        assert!(!commit.trailing_space);
        assert!(!session.is_composing());
    }

    #[test]
    fn digit_out_of_range_leaves_state_untouched() {
        let mut session = session();
        type_text(&mut session, "bha");
        let before = session.buffer().to_owned();
        let out_of_range = session.candidates().len();
        assert_eq!(session.select_digit(out_of_range), None);
        assert_eq!(session.buffer(), before);
        assert!(session.is_composing());
        assert_eq!(session.select_digit(99), None);
        assert_eq!(session.buffer(), before);
    }

    #[test]
    fn arrows_cycle_and_wrap() {
        let mut session = session();
        type_text(&mut session, "bha");
        let len = session.candidates().len();
        assert!(len >= 3);
        session.move_selection(1);
        assert_eq!(session.selected(), 1);
        session.move_selection(1);
        assert_eq!(session.selected(), 2);
        session.move_selection(-1);
        assert_eq!(session.selected(), 1);
        // Wrap forward past the end.
        for _ in 0..len {
            session.move_selection(1);
        }
        assert_eq!(session.selected(), 1);
        // Wrap backward from zero to the last candidate.
        let mut fresh: InputSession = InputSession::new(default_engine());
        type_text(&mut fresh, "bha");
        fresh.move_selection(-1);
        assert_eq!(fresh.selected(), fresh.candidates().len() - 1);
    }

    #[test]
    fn arrows_are_noop_without_candidates() {
        // Idle session: nothing to move.
        let mut session = session();
        session.move_selection(1);
        session.move_selection(-1);
        assert_eq!(session.selected(), 0);
        // Composing session whose engine yields nothing: no rule-table
        // coupling, the empty case is explicit.
        let mut empty: InputSession<EmptyEngine> = InputSession::new(EmptyEngine);
        assert!(empty.push_letter(b'q'));
        assert!(empty.is_composing());
        assert!(empty.candidates().is_empty());
        empty.move_selection(1);
        empty.move_selection(-1);
        assert_eq!(empty.selected(), 0);
    }

    #[test]
    fn space_commits_selected_with_trailing_space() {
        let mut session = session();
        type_text(&mut session, "ami");
        session.move_selection(1);
        let expected = session.candidates()[1].clone();
        let commit = session.commit(true).expect("space commits");
        assert_eq!(commit.erase_chars, 3);
        assert_eq!(commit.text, expected);
        assert!(commit.trailing_space);
        assert!(!session.is_composing());
    }

    #[test]
    fn enter_commits_without_trailing_space() {
        let mut session = session();
        type_text(&mut session, "ami");
        let commit = session.commit(false).expect("enter commits");
        assert_eq!(commit.erase_chars, 3);
        assert!(!commit.trailing_space);
    }

    #[test]
    fn empty_commit_returns_none() {
        let mut session = session();
        assert_eq!(session.commit(true), None);
        assert_eq!(session.commit(false), None);
    }

    #[test]
    fn commit_without_candidates_returns_none_and_keeps_buffer() {
        // Fixture-free: the stub engine yields nothing for any input.
        let mut session: InputSession<EmptyEngine> = InputSession::new(EmptyEngine);
        type_text(&mut session, "qx");
        assert!(session.candidates().is_empty());
        assert_eq!(session.commit(true), None);
        // The roman text stays as typed; only the daemon clears the session.
        assert_eq!(session.buffer(), "qx");
    }

    #[test]
    fn erase_count_equals_buffer_length() {
        let mut session = session();
        type_text(&mut session, "kotha");
        let commit = session.commit(true).expect("commits");
        assert_eq!(commit.erase_chars, "kotha".len());
        assert_eq!(commit.text, "কথা");
    }

    #[test]
    fn buffer_is_capped_at_max() {
        let mut session = session();
        for _ in 0..MAX_BUFFER_LEN {
            assert!(session.push_letter(b'a'));
        }
        assert_eq!(session.buffer().len(), MAX_BUFFER_LEN);
        assert!(!session.push_letter(b'a'));
        assert_eq!(session.buffer().len(), MAX_BUFFER_LEN);
    }

    #[test]
    fn candidates_are_capped_at_five() {
        // A stub engine yielding more than the cap proves the truncation is
        // the session's doing, not an accident of the committed rule table.
        let mut session: InputSession<OverflowEngine> = InputSession::new(OverflowEngine);
        assert!(session.push_letter(b'k'));
        assert_eq!(session.candidates().len(), MAX_CANDIDATES);
    }

    #[test]
    fn default_table_stays_within_cap() {
        // Live-table spot check only: asserts the bound, not the exact count,
        // so phonetic-coverage growth cannot flip it.
        let mut session = session();
        assert!(session.push_letter(b'k'));
        assert!(session.candidates().len() <= MAX_CANDIDATES);
    }

    #[test]
    fn pause_reset_clears_everything() {
        let mut session = session();
        type_text(&mut session, "bhalo");
        session.move_selection(2);
        session.reset();
        assert!(!session.is_composing());
        assert_eq!(session.buffer(), "");
        assert!(session.candidates().is_empty());
        assert_eq!(session.selected(), 0);
    }

    #[test]
    fn vk_letter_mapping_covers_a_to_z() {
        assert_eq!(vk_to_letter(0x41), Some(b'a'));
        assert_eq!(vk_to_letter(0x5A), Some(b'z'));
        assert_eq!(vk_to_letter(0x42), Some(b'b'));
        assert_eq!(vk_to_letter(0x30), None);
        assert_eq!(vk_to_letter(0x20), None);
        assert_eq!(vk_to_letter(0x0D), None);
    }

    #[test]
    fn vk_digit_mapping_covers_row_and_numpad() {
        for (vk, index) in [(0x31, 0), (0x32, 1), (0x33, 2), (0x34, 3), (0x35, 4)] {
            assert_eq!(vk_to_digit_index(vk), Some(index));
        }
        for (vk, index) in [(0x61, 0), (0x62, 1), (0x63, 2), (0x64, 3), (0x65, 4)] {
            assert_eq!(vk_to_digit_index(vk), Some(index));
        }
        assert_eq!(vk_to_digit_index(0x30), None);
        assert_eq!(vk_to_digit_index(0x36), None);
        assert_eq!(vk_to_digit_index(0x41), None);
    }
}
