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

use std::collections::VecDeque;
use std::sync::Arc;

use okkhor_core::Ranker;
use okkhor_engine::{IndexCounts, SuggestRequest, SuggestionEngine, UserWord};

/// Maximum roman letters kept in the composition buffer.
pub const MAX_BUFFER_LEN: usize = 32;
/// Maximum candidates shown in the popup and addressable by digit keys
/// (`1`-`9` and `0` for the tenth row).
pub const MAX_CANDIDATES: usize = 10;

/// Per-composition prefix cache. Backspacing to a prefix the user just saw
/// returns the exact same rows instead of re-ranking them; the entry is
/// dropped on commit/cancel so learned state and a new word start clean.
const PREFIX_CACHE_ENTRIES: usize = 16;

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
/// `.` on the main row (Unicode Bangla maps it to the danda `।`).
pub const VK_OEM_PERIOD: u16 = 0xBE;

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
/// Accepts the `1`-`9` row keys (`0x31`-`0x39`) and `0` for index 9, plus the
/// matching numpad keys (`0x61`-`0x69` and `0x60`). Returns `None` for any
/// other key.
pub fn vk_to_digit_index(vk: u16) -> Option<usize> {
    match vk {
        0x31..=0x39 => Some((vk - 0x31) as usize),
        0x30 => Some(9),
        0x61..=0x69 => Some((vk - 0x61) as usize),
        0x60 => Some(9),
        _ => None,
    }
}

/// Map a candidate index back to the digit that produced it.
///
/// Index `0` is the `1` key … index `8` is `9`, index `9` is `0`. With
/// `bengali`, the result is `১`–`৯`, `০` (U+09E7…U+09E6); otherwise ASCII.
pub fn digit_char(index: usize, bengali: bool) -> char {
    let value = (index + 1) % 10;
    if bengali {
        // '০' is U+09E6, so value 1 maps to U+09E7 ('১').
        char::from_u32(0x09E6 + value as u32).unwrap_or('0')
    } else if value == 0 {
        '0'
    } else {
        char::from(b'0' + value as u8)
    }
}

/// Whether a virtual key is a pure modifier (Shift/Ctrl/Alt/Win/Caps/Num/Scroll).
///
/// Modifier presses must not cancel a composition: Shift is part of normal
/// capital typing, and the chord cases are handled by the letter arm.
pub const fn is_modifier_key(vk: u16) -> bool {
    matches!(
        vk,
        0x10 | 0x11 | 0x12 | 0x14 | 0x5B | 0x5C | 0x90 | 0x91 | 0xA0..=0xA5
    )
}

/// Pure composition session: roman buffer, candidates, selection.
///
/// Bounded: the buffer holds at most [`MAX_BUFFER_LEN`] bytes and the
/// candidate list at most [`MAX_CANDIDATES`] strings. No logging, no I/O.
pub trait SuggestionProvider {
    /// Return the visible candidate rows for `roman`, most likely first.
    fn suggestions(
        &self,
        roman: &str,
        limit: usize,
        previous: Option<&str>,
        now_ms: u64,
        user_words: &[UserWord],
    ) -> Vec<String>;

    /// English prefix completions. Default: none (tests and the empty engine).
    fn english_suggestions(&self, _prefix: &str, _limit: usize) -> Vec<String> {
        Vec::new()
    }
}

pub struct InputSession<P = DefaultEngine> {
    provider: P,
    buffer: String,
    candidates: Vec<String>,
    selected: usize,
    max_candidates: usize,
    previous_word: Option<String>,
    now_ms: u64,
    user_words: Vec<UserWord>,
    /// Rank time frozen at the first letter of this composition. Recency
    /// scoring must not reshuffle the same prefix a few milliseconds later.
    rank_ms: u64,
    /// (roman, previous, limit) -> rows for this composition.
    cache: VecDeque<(String, Option<String>, usize, Vec<String>)>,
    /// True once the user moves the highlight with an arrow key. Only then
    /// does `refresh` follow that row across a keystroke; the automatic top
    /// row stays row 0.
    selection_pinned: bool,
    /// One entry per keystroke, so Backspace is an exact undo.
    history: Vec<Snapshot>,
}

/// State captured before a keystroke.
#[derive(Debug, Clone)]
struct Snapshot {
    buffer: String,
    candidates: Vec<String>,
    selected: usize,
    rank_ms: u64,
    selection_pinned: bool,
}

impl<P: SuggestionProvider> InputSession<P> {
    /// Create an idle session backed by `provider`.
    pub fn new(provider: P) -> Self {
        Self {
            provider,
            buffer: String::with_capacity(MAX_BUFFER_LEN),
            candidates: Vec::with_capacity(MAX_CANDIDATES),
            selected: 0,
            max_candidates: MAX_CANDIDATES,
            previous_word: None,
            now_ms: 0,
            user_words: Vec::new(),
            rank_ms: 0,
            cache: VecDeque::with_capacity(PREFIX_CACHE_ENTRIES),
            selection_pinned: false,
            history: Vec::new(),
        }
    }

    /// Set the logical time passed to the ranker (0 disables recency decay).
    pub fn set_now_ms(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
    }

    /// Replace the user dictionary snapshot used by the provider.
    pub fn set_user_words(&mut self, words: Vec<UserWord>) {
        self.user_words = words;
        self.cache.clear();
    }

    /// English prefix completions for the shared English tracker.
    pub fn english_suggestions(&self, prefix: &str, limit: usize) -> Vec<String> {
        self.provider.english_suggestions(prefix, limit)
    }

    /// Last committed word, used as ranking context.
    pub fn previous_word(&self) -> Option<&str> {
        self.previous_word.as_deref()
    }

    /// Change the visible candidate bound (1-10) from `popup_max_items`.
    ///
    /// The daemon calls this from its loop, off the hook path. Shrinking drops
    /// the tail and keeps the selection in range; the next keystroke rebuilds
    /// the list at the new bound.
    pub fn set_max_candidates(&mut self, max: usize) {
        let max = max.clamp(1, MAX_CANDIDATES);
        if max == self.max_candidates {
            return;
        }
        self.max_candidates = max;
        self.cache.clear();
        if self.candidates.len() > max {
            self.candidates.truncate(max);
            self.selected = self.selected.min(max - 1);
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
        // Snapshot before the change: Backspace becomes an exact undo of the
        // last keystroke instead of a re-run of the engine from a shorter
        // buffer, which could replace the rendered word wholesale.
        if self.history.len() >= MAX_BUFFER_LEN {
            self.history.remove(0);
        }
        self.history.push(Snapshot {
            buffer: self.buffer.clone(),
            candidates: self.candidates.clone(),
            selected: self.selected,
            rank_ms: self.rank_ms,
            selection_pinned: self.selection_pinned,
        });
        if self.buffer.is_empty() {
            // Freeze recency for this word: the same prefix must rank the same
            // when the user backspaces and retypes it. A fresh word also
            // starts with a fresh cache so the previous word cannot leak in.
            self.rank_ms = self.now_ms;
            self.cache.clear();
            self.selection_pinned = false;
        }
        self.buffer.push(lower as char);
        self.refresh();
        true
    }

    /// Handle Backspace: undo exactly one keystroke when possible.
    ///
    /// Restoring the snapshot is what makes the document shrink by one step
    /// instead of jumping to a different word; the engine is not re-run, so a
    /// shorter buffer with no candidates cannot wipe the word.
    pub fn backspace(&mut self) -> BackspaceOutcome {
        if let Some(snapshot) = self.history.pop() {
            self.buffer = snapshot.buffer;
            self.candidates = snapshot.candidates;
            self.selected = snapshot.selected;
            self.rank_ms = snapshot.rank_ms;
            self.selection_pinned = snapshot.selection_pinned;
            if self.buffer.is_empty() {
                self.rank_ms = 0;
                return BackspaceOutcome::BecameEmpty;
            }
            return BackspaceOutcome::StillComposing;
        }
        if self.buffer.pop().is_none() {
            return BackspaceOutcome::Empty;
        }
        if self.buffer.is_empty() {
            self.rank_ms = 0;
            self.selection_pinned = false;
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
        self.selection_pinned = true;
    }

    /// Select an absolute candidate index (digit shortcuts and tests).
    ///
    /// Out-of-range indices are ignored, leaving the current selection.
    pub fn select_index(&mut self, index: usize) {
        if index < self.candidates.len() {
            self.selected = index;
            self.selection_pinned = true;
        }
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
        let text = self.candidates[index].clone();
        let commit = Commit {
            erase_chars: self.buffer.len(),
            text: text.clone(),
            trailing_space,
        };
        self.clear();
        self.previous_word = Some(text);
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

    /// Pause/focus reset: drop the buffer, candidates, selection, and context.
    pub fn reset(&mut self) {
        self.clear();
        self.previous_word = None;
    }

    fn clear(&mut self) {
        self.buffer.clear();
        self.candidates.clear();
        self.selected = 0;
        self.rank_ms = 0;
        self.cache.clear();
        self.selection_pinned = false;
        self.history.clear();
    }

    fn refresh(&mut self) {
        // Keep the highlighted row across a keystroke when that same text is
        // still offered. Kontho selects `wordSofar` and Bongo restores the
        // previous selection; snapping back to row 0 makes an arrow selection
        // feel lost.
        let previous_selection = if self.selection_pinned {
            self.candidates.get(self.selected).cloned()
        } else {
            None
        };
        self.candidates.clear();
        self.selected = 0;
        if self.buffer.is_empty() {
            return;
        }
        let previous = self.previous_word.as_deref();
        let limit = self.max_candidates;
        let cached = self
            .cache
            .iter()
            .find(|(roman, cached_previous, cached_limit, _)| {
                roman == &self.buffer
                    && cached_previous.as_deref() == previous
                    && *cached_limit == limit
            })
            .map(|(_, _, _, rows)| rows.clone());
        if let Some(rows) = cached {
            self.candidates = rows;
            self.restore_selection(previous_selection);
            return;
        }
        let rank_ms = if self.rank_ms == 0 {
            self.now_ms
        } else {
            self.rank_ms
        };
        let rows =
            self.provider
                .suggestions(&self.buffer, limit, previous, rank_ms, &self.user_words);
        self.candidates = rows;
        self.candidates.truncate(limit);
        self.cache.push_front((
            self.buffer.clone(),
            previous.map(str::to_owned),
            limit,
            self.candidates.clone(),
        ));
        self.cache.truncate(PREFIX_CACHE_ENTRIES);
        self.restore_selection(previous_selection);
    }

    fn restore_selection(&mut self, previous: Option<String>) {
        if let Some(text) = previous {
            if let Some(index) = self.candidates.iter().position(|row| *row == text) {
                self.selected = index;
                return;
            }
            // The user's row fell out of the list; do not keep chasing it.
            self.selection_pinned = false;
        }
        self.selected = 0;
    }
}

/// Live engine: the staged suggestion engine plus the shared ranker.
///
/// Construction happens once at daemon startup. A phonetic or index load
/// failure yields [`DefaultEngine::Empty`], which produces no candidates so
/// keys pass through untouched; the daemon logs the static failure kind.
pub enum DefaultEngine {
    /// Live path: embedded index + phonetic table + the injected ranker
    /// (deterministic Smriti or a hybrid reranker).
    Suggest {
        engine: Box<SuggestionEngine>,
        ranker: Arc<dyn Ranker + Send + Sync>,
    },
    /// Load-failure fallback: yields no candidates.
    Empty {
        /// Allowlisted `kind` string for the daemon startup log.
        kind: &'static str,
    },
}

impl std::fmt::Debug for DefaultEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Suggest { engine, .. } => formatter
                .debug_struct("Suggest")
                .field("engine", engine)
                .finish_non_exhaustive(),
            Self::Empty { kind } => formatter.debug_struct("Empty").field("kind", kind).finish(),
        }
    }
}

impl DefaultEngine {
    /// Whether this is the load-failure fallback (yields no candidates).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_fallback(&self) -> bool {
        matches!(self, Self::Empty { .. })
    }

    /// Allowlisted failure kind for the daemon startup log; `None` when the
    /// engine and index loaded. Carries no input, output, or word text.
    pub fn fallback_kind(&self) -> Option<&'static str> {
        match self {
            Self::Suggest { engine, .. } => engine.index_error(),
            Self::Empty { kind } => Some(kind),
        }
    }

    /// Index counts for the startup health check (tests and logs only).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn index_counts(&self) -> Option<IndexCounts> {
        match self {
            Self::Suggest { engine, .. } => {
                engine.index().map(okkhor_engine::SuggestionIndex::counts)
            }
            Self::Empty { .. } => None,
        }
    }
}

impl SuggestionProvider for DefaultEngine {
    fn suggestions(
        &self,
        roman: &str,
        limit: usize,
        previous: Option<&str>,
        now_ms: u64,
        user_words: &[UserWord],
    ) -> Vec<String> {
        match self {
            Self::Suggest { engine, ranker } => {
                let request = SuggestRequest {
                    roman,
                    limit,
                    previous_word: previous,
                    now_ms,
                    user_words,
                };
                engine
                    .suggest_with_ranker(&request, ranker.as_ref())
                    .into_iter()
                    .map(|suggestion| suggestion.text)
                    .collect()
            }
            Self::Empty { .. } => Vec::new(),
        }
    }

    fn english_suggestions(&self, prefix: &str, limit: usize) -> Vec<String> {
        match self {
            Self::Suggest { engine, .. } => engine.suggest_english(prefix, limit),
            Self::Empty { .. } => Vec::new(),
        }
    }
}

/// Build the live engine from the embedded index and phonetic table.
///
/// Never panics: a load failure yields the empty fallback (no candidates,
/// keys pass through) and the daemon logs [`DefaultEngine::fallback_kind`]
/// once at startup.
pub fn default_engine(ranker: Arc<dyn Ranker + Send + Sync>) -> DefaultEngine {
    match SuggestionEngine::from_embedded() {
        Ok(engine) => DefaultEngine::Suggest {
            engine: Box::new(engine),
            ranker,
        },
        Err(_) => DefaultEngine::Empty {
            kind: "engine_load",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BackspaceOutcome, DefaultEngine, InputSession, MAX_BUFFER_LEN, MAX_CANDIDATES,
        SuggestionProvider, default_engine, digit_char, is_modifier_key, vk_to_digit_index,
        vk_to_letter,
    };
    use okkhor_engine::UserWord;
    use okkhor_learn::Smriti;
    use std::sync::Arc;

    fn session() -> InputSession {
        InputSession::new(default_engine(Arc::new(Smriti::new())))
    }

    fn type_text<P: SuggestionProvider>(session: &mut InputSession<P>, text: &str) {
        for byte in text.bytes() {
            assert!(session.push_letter(byte), "letter {byte} accepted");
        }
    }

    /// Stub provider yielding nothing: makes "no candidates" tests independent
    /// of the committed index.
    struct EmptyEngine;

    impl SuggestionProvider for EmptyEngine {
        fn suggestions(
            &self,
            _roman: &str,
            _limit: usize,
            _previous: Option<&str>,
            _now_ms: u64,
            _user_words: &[UserWord],
        ) -> Vec<String> {
            Vec::new()
        }
    }

    /// Stub provider yielding more than the cap: proves truncation is the
    /// session's doing, not an accident of the index.
    struct OverflowEngine;

    impl SuggestionProvider for OverflowEngine {
        fn suggestions(
            &self,
            _roman: &str,
            limit: usize,
            _previous: Option<&str>,
            _now_ms: u64,
            _user_words: &[UserWord],
        ) -> Vec<String> {
            (0..limit + 3).map(|index| format!("ক{index}")).collect()
        }
    }

    /// Stub provider that counts calls, for the per-composition cache.
    struct CountingEngine {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl SuggestionProvider for CountingEngine {
        fn suggestions(
            &self,
            roman: &str,
            _limit: usize,
            _previous: Option<&str>,
            _now_ms: u64,
            _user_words: &[UserWord],
        ) -> Vec<String> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            vec![roman.to_owned()]
        }
    }

    /// Stub provider yielding a fixed candidate list: keeps selection and
    /// commit tests independent of the committed index.
    struct FixedEngine {
        candidates: Vec<String>,
    }

    impl FixedEngine {
        fn new(candidates: &[&str]) -> Self {
            Self {
                candidates: candidates.iter().map(|text| text.to_string()).collect(),
            }
        }
    }

    impl SuggestionProvider for FixedEngine {
        fn suggestions(
            &self,
            _roman: &str,
            limit: usize,
            _previous: Option<&str>,
            _now_ms: u64,
            _user_words: &[UserWord],
        ) -> Vec<String> {
            self.candidates.iter().take(limit).cloned().collect()
        }
    }

    #[test]
    fn default_engine_builds_embedded_index() {
        let engine = default_engine(Arc::new(Smriti::new()));
        assert!(!engine.is_fallback(), "embedded index loads");
        assert_eq!(engine.fallback_kind(), None);
        let counts = engine.index_counts().expect("index counts");
        assert!(counts.bangla > 150_000, "bangla {}", counts.bangla);
        assert!(counts.roman > 100_000, "roman {}", counts.roman);
    }

    #[test]
    fn select_index_sets_an_absolute_row() {
        let engine = FixedEngine::new(&["alpha", "beta", "gamma"]);
        let mut session = InputSession::new(engine);
        type_text(&mut session, "a");
        assert_eq!(session.selected(), 0);
        session.select_index(2);
        assert_eq!(session.selected(), 2);
        session.select_index(99);
        assert_eq!(session.selected(), 2, "out of range is ignored");
        let commit = session.commit(false).expect("commit");
        assert_eq!(commit.text, "gamma");
    }

    #[test]
    fn backspace_reuses_cached_prefix_rows() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut session = InputSession::new(CountingEngine {
            calls: Arc::clone(&calls),
        });
        type_text(&mut session, "bhal");
        let full = session.candidates().to_vec();
        assert_eq!(session.backspace(), BackspaceOutcome::StillComposing);
        assert!(session.push_letter(b'l'));
        assert_eq!(session.candidates(), full.as_slice());
        // b, bh, bha, bhal; the backspace and retype are cache hits.
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 4);
    }

    #[test]
    fn selection_survives_a_keystroke_when_the_row_stays() {
        let mut session = InputSession::new(FixedEngine::new(&["a", "b", "c"]));
        type_text(&mut session, "x");
        session.move_selection(2);
        assert_eq!(session.selected(), 2);
        assert!(session.push_letter(b'y'));
        assert_eq!(session.selected(), 2);
    }

    #[test]
    fn fallback_engine_yields_no_candidates() {
        // Load-failure shape: typing passes through instead of panicking.
        let engine = DefaultEngine::Empty {
            kind: "transliterate_invalid_pattern",
        };
        assert!(engine.is_fallback());
        assert_eq!(
            engine.fallback_kind(),
            Some("transliterate_invalid_pattern")
        );
        assert!(engine.index_counts().is_none());
        let mut session: InputSession<DefaultEngine> = InputSession::new(engine);
        type_text(&mut session, "qqqq");
        assert!(session.candidates().is_empty());
        assert_eq!(session.commit(true), None);
        assert_eq!(session.buffer(), "qqqq");
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
        // < 3 ms p99 in a release build. Debug builds run the dictionary scan
        // unoptimized, so the debug gate is 20 ms; `--release` enforces 3 ms.
        let budget_ms = if cfg!(debug_assertions) { 20 } else { 3 };
        assert!(
            average < std::time::Duration::from_millis(budget_ms),
            "average decision {average:?} exceeds the {budget_ms} ms budget"
        );
    }

    #[test]
    fn letters_build_buffer_and_top_candidate() {
        let mut session = session();
        type_text(&mut session, "bhalo");
        assert_eq!(session.buffer(), "bhalo");
        assert!(session.is_composing());
        assert_eq!(session.selected(), 0);
        // Curated exact match wins for common words so the visible top row is
        // the standard spelling (ভালো), not the table's inherent-vowel form.
        assert_eq!(
            session.candidates().first().map(String::as_str),
            Some("ভালো")
        );
        assert!(session.candidates().iter().any(|word| word == "ভাল"));
    }

    #[test]
    fn live_table_end_to_end_words() {
        for (roman, expected) in [
            ("bangla", "বাংলা"),
            ("tumi", "তুমি"),
            ("kemon", "কেমন"),
            ("amar", "আমার"),
        ] {
            let mut live = session();
            type_text(&mut live, roman);
            let commit = live.commit(false).expect("live table commits");
            assert_eq!(commit.text, expected, "roman {roman:?}");
            assert_eq!(commit.erase_chars, roman.len());
            assert!(!live.is_composing());
        }
    }

    #[test]
    fn backspace_to_one_letter_keeps_real_rows() {
        let mut live = session();
        type_text(&mut live, "bhalo");
        for expected in ["bhal", "bha", "bh", "b"] {
            assert_eq!(live.backspace(), BackspaceOutcome::StillComposing);
            assert!(
                !live.candidates().is_empty(),
                "prefix {expected} lost its suggestions"
            );
        }
    }

    #[test]
    fn two_words_commit_in_sequence() {
        // Owner report: the first word commits, then a later word shows in the
        // list but Space injects only whitespace. If `commit()` ever returns
        // None for the second word, the daemon takes the bare-Space arm.
        let mut live = session();
        for (roman, expected) in [
            ("amar", "আমার"),
            ("kotha", "কথা"),
            ("bhalo", "ভালো"),
            ("tumi", "তুমি"),
        ] {
            type_text(&mut live, roman);
            assert!(
                !live.candidates().is_empty(),
                "{roman} produced no rows before commit"
            );
            let commit = live
                .commit(true)
                .unwrap_or_else(|| panic!("{roman} returned no Commit"));
            assert_eq!(commit.text, expected, "roman {roman:?}");
            assert_eq!(commit.erase_chars, roman.len());
            assert!(!commit.text.chars().all(char::is_whitespace));
        }
    }

    #[test]
    fn live_table_partial_prefix_top_candidate() {
        let mut live = session();
        type_text(&mut live, "bhal");
        assert_eq!(live.candidates().first().map(String::as_str), Some("ভাল"));
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
    fn digit_char_maps_keys_to_ascii_and_bengali_numerals() {
        // Key 1..9 -> index 0..8, key 0 -> index 9.
        assert_eq!(digit_char(0, false), '1');
        assert_eq!(digit_char(8, false), '9');
        assert_eq!(digit_char(9, false), '0');
        assert_eq!(digit_char(0, true), '১');
        assert_eq!(digit_char(8, true), '৯');
        assert_eq!(digit_char(9, true), '০');
    }

    #[test]
    fn modifiers_keep_a_composition_alive() {
        // Shift/Ctrl/Alt/Win/Caps/Num/Scroll presses are not text and must not
        // end a composition; every other unhandled key cancels it.
        for vk in [0x10, 0x11, 0x12, 0x14, 0x5B, 0x5C, 0x90, 0x91, 0xA0, 0xA5] {
            assert!(is_modifier_key(vk), "vk {vk:#04X} is a modifier");
        }
        for vk in [0x41, 0x20, 0x0D, 0x25, 0xBC, 0x2E] {
            assert!(!is_modifier_key(vk), "vk {vk:#04X} is not a modifier");
        }
    }

    #[test]
    fn digits_select_and_commit() {
        // Multi-candidate behavior via stub: the live Avro table yields at
        // most one candidate per buffer.
        let mut session: InputSession<FixedEngine> =
            InputSession::new(FixedEngine::new(&["ভাল", "ভালো", "ভালোবাসা"]));
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
    fn live_session_offers_multiple_curated_candidates() {
        // The rule table yields one form, but the starter list fills rows so
        // the caret popup has a real choice for common words and prefixes.
        let mut exact = session();
        type_text(&mut exact, "korcho");
        assert_eq!(exact.candidates().first().map(String::as_str), Some("করছো"));
        assert!(exact.candidates().len() >= 2);

        let mut prefix = session();
        type_text(&mut prefix, "ko");
        assert!(prefix.candidates().len() >= 5, "{:?}", prefix.candidates());
        assert_eq!(prefix.selected(), 0);
    }

    #[test]
    fn digit_keys_cover_ten_rows_and_stop_at_the_bound() {
        for (vk, expected) in [
            (0x31, 0),
            (0x35, 4),
            (0x39, 8),
            (0x30, 9),
            (0x61, 0),
            (0x65, 4),
            (0x69, 8),
            (0x60, 9),
        ] {
            assert_eq!(vk_to_digit_index(vk), Some(expected), "vk {vk:#x}");
        }
        for vk in [0x41, 0x2D, 0x6A, 0x20] {
            assert_eq!(vk_to_digit_index(vk), None, "vk {vk:#x}");
        }
        // A session with ten candidates commits row ten via the zero key.
        let mut session: InputSession<FixedEngine> = InputSession::new(FixedEngine::new(&[
            "ক০", "ক১", "ক২", "ক৩", "ক৪", "ক৫", "ক৬", "ক৭", "ক৮", "ক৯",
        ]));
        type_text(&mut session, "kh");
        assert_eq!(session.candidates().len(), 10);
        let expected = session.candidates()[9].clone();
        let commit = session.select_digit(9).expect("tenth row commits");
        assert_eq!(commit.text, expected);
    }

    #[test]
    fn digit_out_of_range_leaves_state_untouched() {
        let mut session: InputSession<FixedEngine> =
            InputSession::new(FixedEngine::new(&["ভাল", "ভালো"]));
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
        // Multi-candidate behavior via stub: the live Avro table yields at
        // most one candidate per buffer.
        let mut session: InputSession<FixedEngine> =
            InputSession::new(FixedEngine::new(&["ভাল", "ভালো", "ভালোবাসা"]));
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
        let mut fresh: InputSession<FixedEngine> =
            InputSession::new(FixedEngine::new(&["ভাল", "ভালো", "ভালোবাসা"]));
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
        // Multi-candidate behavior via stub: the live Avro table yields at
        // most one candidate per buffer.
        let mut session: InputSession<FixedEngine> =
            InputSession::new(FixedEngine::new(&["আমি", "আমার"]));
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
        let mut session: InputSession<FixedEngine> =
            InputSession::new(FixedEngine::new(&["ভাল", "ভালো", "ভালোবাসা"]));
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
        // Six through ten: digits 6-9 and 0 on row and numpad.
        for (vk, index) in [
            (0x36, 5),
            (0x37, 6),
            (0x38, 7),
            (0x39, 8),
            (0x30, 9),
            (0x66, 5),
            (0x69, 8),
            (0x60, 9),
        ] {
            assert_eq!(vk_to_digit_index(vk), Some(index), "vk {vk:#x}");
        }
        assert_eq!(vk_to_digit_index(0x41), None);
    }
}
