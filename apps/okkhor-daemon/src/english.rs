//! English completion state for the daemon (0.6).
//!
//! Tracks the Latin token at the caret and the prefix completions shown for
//! it. It never edits text on its own: only an explicit Tab/click produces an
//! [`EnglishCommit`]. Space, Enter, punctuation, and focus changes reset the
//! token, so English typing semantics stay exactly as before.

/// Maximum token length tracked (the engine accepts at most 32).
pub const MAX_TOKEN_LEN: usize = 32;
/// Shortest token that offers completions. A single letter is enough: the
/// engine bounds one-letter prefix scans, and hiding until three letters made
/// Backspace look broken to the owner.
pub const MIN_TOKEN_LEN: usize = 1;

/// A committed English completion: erase the token, inject `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnglishCommit {
    pub erase_chars: usize,
    pub text: String,
}

/// English token + popup state. Pure: no Win32, I/O, or logging.
#[derive(Debug, Default)]
pub struct EnglishSession {
    token: String,
    suggestions: Vec<String>,
    selected: usize,
    window: isize,
}

impl EnglishSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop the token and the list.
    pub fn reset(&mut self) {
        self.token.clear();
        self.suggestions.clear();
        self.selected = 0;
        self.window = 0;
    }

    /// Hide the list but keep the token so typing can re-open it.
    pub fn dismiss(&mut self) {
        self.suggestions.clear();
        self.selected = 0;
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn suggestions(&self) -> &[String] {
        &self.suggestions
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn window(&self) -> isize {
        self.window
    }

    pub fn is_visible(&self) -> bool {
        !self.suggestions.is_empty()
    }

    /// Append one ASCII letter and refresh the list. `suggest` is the engine
    /// query; the caller passes a closure so this module stays platform-free.
    pub fn push_letter(
        &mut self,
        letter: u8,
        limit: usize,
        window: isize,
        suggest: impl Fn(&str, usize) -> Vec<String>,
    ) -> bool {
        if !letter.is_ascii_alphabetic() || self.token.len() >= MAX_TOKEN_LEN {
            return false;
        }
        if self.token.is_empty() {
            self.window = window;
        }
        self.token.push(letter as char);
        self.refresh(limit, suggest);
        true
    }

    /// Remove the last character and refresh (or hide when empty).
    pub fn backspace(&mut self, limit: usize, suggest: impl Fn(&str, usize) -> Vec<String>) {
        self.token.pop();
        if self.token.is_empty() {
            self.suggestions.clear();
            self.selected = 0;
            self.window = 0;
            return;
        }
        self.refresh(limit, suggest);
    }

    /// Move the highlighted row (wrapping). No-op with fewer than two rows.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn move_selection(&mut self, delta: i32) {
        if self.suggestions.len() < 2 {
            return;
        }
        let len = self.suggestions.len() as i32;
        self.selected = (self.selected as i32 + delta).rem_euclid(len) as usize;
    }

    /// Accept the highlighted row and return the erase+inject operation.
    pub fn accept(&mut self) -> Option<EnglishCommit> {
        if self.suggestions.is_empty() || self.token.is_empty() {
            return None;
        }
        let index = self.selected.min(self.suggestions.len() - 1);
        let text = self.suggestions[index].clone();
        let commit = EnglishCommit {
            erase_chars: self.token.chars().count(),
            text,
        };
        self.reset();
        Some(commit)
    }

    /// Accept a clicked row by index.
    pub fn accept_index(&mut self, index: usize) -> Option<EnglishCommit> {
        if index >= self.suggestions.len() {
            return None;
        }
        self.selected = index;
        self.accept()
    }

    fn refresh(&mut self, limit: usize, suggest: impl Fn(&str, usize) -> Vec<String>) {
        self.selected = 0;
        if self.token.len() < MIN_TOKEN_LEN {
            self.suggestions.clear();
            return;
        }
        let capitalized = self
            .token
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_uppercase());
        self.suggestions = suggest(&self.token, limit)
            .into_iter()
            .filter(|suggestion| !suggestion.eq_ignore_ascii_case(&self.token))
            .take(limit)
            .map(|suggestion| {
                if capitalized {
                    let mut chars = suggestion.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                        None => suggestion,
                    }
                } else {
                    suggestion
                }
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(rows: &'static [&'static str]) -> impl Fn(&str, usize) -> Vec<String> {
        move |_prefix, limit| {
            rows.iter()
                .take(limit)
                .map(|row| (*row).to_owned())
                .collect()
        }
    }

    #[test]
    fn single_letter_token_opens_a_popup() {
        let mut session = EnglishSession::new();
        assert!(session.push_letter(b'h', 8, 1, fake(&["hello"])));
        assert!(session.is_visible());
        assert_eq!(session.suggestions(), ["hello"]);
        assert!(session.push_letter(b'e', 8, 1, fake(&["hello"])));
        assert!(session.is_visible());
        assert!(session.push_letter(b'l', 8, 1, fake(&["hello", "help"])));
        assert!(session.is_visible());
        assert_eq!(session.suggestions(), ["hello", "help"]);
    }

    #[test]
    fn exact_token_hides_the_list_and_tab_passes() {
        let mut session = EnglishSession::new();
        let exact = |_prefix: &str, _limit: usize| vec!["hello".to_owned()];
        session.push_letter(b'h', 8, 1, exact);
        session.push_letter(b'e', 8, 1, exact);
        session.push_letter(b'l', 8, 1, exact);
        assert!(session.is_visible());
        session.push_letter(b'l', 8, 1, exact);
        // "hell" -> "hello" is still a completion.
        assert!(session.is_visible());
        session.push_letter(b'o', 8, 1, exact);
        // "hello" -> "hello" is not a completion.
        assert!(!session.is_visible());
    }

    #[test]
    fn accept_returns_the_token_length_and_resets() {
        let mut session = EnglishSession::new();
        for letter in *b"hel" {
            session.push_letter(letter, 8, 42, fake(&["hello", "help"]));
        }
        session.move_selection(1);
        let commit = session.accept().expect("commit");
        assert_eq!(commit.erase_chars, 3);
        assert_eq!(commit.text, "help");
        assert!(!session.is_visible());
        assert!(session.token().is_empty());
    }

    #[test]
    fn backspace_keeps_completions_until_the_token_is_empty() {
        let mut session = EnglishSession::new();
        for letter in *b"hel" {
            session.push_letter(letter, 8, 1, fake(&["hello"]));
        }
        session.backspace(8, fake(&["hello"]));
        assert!(session.is_visible(), "he should still offer completions");
        session.backspace(8, fake(&["hello"]));
        assert!(session.is_visible(), "h should still offer completions");
        session.backspace(8, fake(&["hello"]));
        assert!(!session.is_visible());
        assert!(session.token().is_empty());
    }

    #[test]
    fn capitalization_is_preserved() {
        let mut session = EnglishSession::new();
        session.push_letter(b'H', 8, 1, fake(&["hello"]));
        session.push_letter(b'e', 8, 1, fake(&["hello"]));
        session.push_letter(b'l', 8, 1, fake(&["hello"]));
        assert_eq!(session.suggestions(), ["Hello"]);
    }
}
