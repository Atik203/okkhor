#![forbid(unsafe_code)]
//! Deterministic QA for the inline-preedit document flow.
//!
//! Drives the real `InputSession` plus the real `plan_edit` diff against a
//! string document, exactly like the daemon does, with a fake transliterator.
//! It does not install the hook and does not inject text (the hook ignores
//! injected keys), so the owner real-key pass is still required; what this
//! catches is boundary corruption: words merging after Space/danda, wrong
//! candidate on commit, and leftovers after Backspace/Esc.

#[allow(dead_code)]
#[path = "../src/input_session.rs"]
mod input_session;

use input_session::{BackspaceOutcome, InputSession, SuggestionProvider};
use okkhor_core::plan_edit;
use okkhor_engine::UserWord;

/// Deterministic transliteration: a fixed dictionary plus a visible marker for
/// unknown roman input, so a merge ("amarbhalo") shows up as a different word
/// instead of silently producing valid Bangla.
struct Stub;

const WORDS: [(&str, &str); 6] = [
    ("amar", "আমার"),
    ("bhalo", "ভালো"),
    ("tumi", "তুমি"),
    ("kemon", "কেমন"),
    ("acho", "আছো"),
    ("desh", "দেশ"),
];

impl SuggestionProvider for Stub {
    fn suggestions(
        &self,
        roman: &str,
        limit: usize,
        _previous: Option<&str>,
        _now_ms: u64,
        _user_words: &[UserWord],
    ) -> Vec<String> {
        let lower = roman.to_ascii_lowercase();
        let mut rows: Vec<String> = Vec::new();
        for (key, value) in WORDS {
            if lower == *key {
                rows.push((*value).to_owned());
            }
        }
        // A merged buffer is not a dictionary word; expose it verbatim.
        if rows.is_empty() {
            rows.push(format!("!{roman}!"));
        }
        // A second, different candidate so selection has something to move to.
        rows.push(format!("~{roman}~"));
        rows.truncate(limit);
        rows
    }
}

struct Sim {
    session: InputSession<Stub>,
    doc: String,
    rendered: String,
}

impl Sim {
    fn new() -> Self {
        Self {
            session: InputSession::new(Stub),
            doc: String::new(),
            rendered: String::new(),
        }
    }

    fn render(&mut self) {
        let next = if self.session.is_composing() {
            self.session
                .candidates()
                .get(self.session.selected())
                .cloned()
                .unwrap_or_default()
        } else {
            String::new()
        };
        let plan = plan_edit(&self.rendered, &next);
        let keep = self.doc.chars().count().saturating_sub(plan.erase_utf16);
        self.doc = self.doc.chars().take(keep).collect();
        self.doc.push_str(&plan.insert);
        self.rendered = next;
    }

    fn letter(&mut self, letter: char) {
        if self.session.push_letter(letter as u8) {
            self.render();
        }
    }

    fn word(&mut self, text: &str) {
        for letter in text.chars() {
            self.letter(letter);
        }
    }

    fn space(&mut self) {
        self.session.commit(true);
        self.rendered.clear();
        self.doc.push(' ');
    }

    fn danda(&mut self) {
        if self.session.is_composing() {
            self.session.commit(false);
            self.rendered.clear();
        }
        self.doc.push('।');
    }

    fn tab(&mut self) {
        self.session.commit(false);
        self.rendered.clear();
    }

    fn escape(&mut self) {
        self.session.cancel();
        self.rendered.clear();
    }

    fn backspace(&mut self) {
        if self.session.backspace() != BackspaceOutcome::Empty {
            self.render();
        }
    }

    fn invariant(&self, context: &str) {
        if !self.rendered.is_empty() {
            assert!(
                self.doc.ends_with(&self.rendered),
                "{context}: document {:?} does not end with rendered {:?}",
                self.doc,
                self.rendered
            );
        }
        assert!(
            !self.doc.contains("!!"),
            "{context}: nested unknown marker in {:?}",
            self.doc
        );
    }
}

#[test]
fn two_words_do_not_merge_after_space() {
    let mut sim = Sim::new();
    sim.word("amar");
    sim.invariant("after amar");
    sim.space();
    sim.word("bhalo");
    sim.space();
    assert_eq!(sim.doc, "আমার ভালো ");
    assert!(!sim.doc.contains("amarbhalo"));
}

#[test]
fn danda_finishes_the_word_without_eating_it() {
    let mut sim = Sim::new();
    sim.word("amar");
    sim.danda();
    assert_eq!(sim.doc, "আমার।");
}

#[test]
fn danda_after_two_words_keeps_both() {
    let mut sim = Sim::new();
    sim.word("amar");
    sim.space();
    sim.word("bhalo");
    sim.danda();
    assert_eq!(sim.doc, "আমার ভালো।");
}

#[test]
fn digit_selects_the_chosen_candidate() {
    let mut sim = Sim::new();
    sim.word("amar");
    sim.session.select_index(1);
    sim.render();
    sim.tab();
    assert_eq!(sim.doc, "~amar~");
}

#[test]
fn backspace_walks_back_without_leaving_text() {
    let mut sim = Sim::new();
    sim.word("amar");
    sim.backspace();
    sim.backspace();
    sim.invariant("after backspaces");
    sim.word("ar");
    sim.invariant("after retype");
    sim.space();
    assert_eq!(sim.doc, "আমার ");
}

#[test]
fn escape_keeps_the_rendered_word_and_starts_clean() {
    let mut sim = Sim::new();
    sim.word("amar");
    sim.escape();
    sim.invariant("after escape");
    assert_eq!(sim.doc, "আমার");
    sim.word("bhalo");
    sim.invariant("second word");
    sim.space();
    assert_eq!(sim.doc, "আমারভালো ", "a second word starts next to the first");
}

#[test]
fn unknown_input_is_never_silently_merged() {
    let mut sim = Sim::new();
    sim.word("amar");
    sim.space();
    sim.word("bhalo");
    sim.space();
    sim.word("xyz");
    sim.space();
    assert!(sim.doc.starts_with("আমার ভালো "));
    assert!(
        sim.doc.contains("xyz"),
        "raw marker echoes the buffer: {}",
        sim.doc
    );
}

/// 5,000 mixed events with a structural invariant after every one.
#[test]
fn stress_mixed_events_keep_the_document_consistent() {
    let mut sim = Sim::new();
    let mut state: u64 = 0x4F4B_4B48;
    let alphabet = [
        'a', 'm', 'r', 'b', 'h', 'l', 'o', 't', 'u', 'i', 'k', 'e', 'n', 'c', 'd', 's',
    ];
    for step in 0..5_000 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        match (state >> 33) % 10 {
            0..=5 => {
                let letter = alphabet[(state as usize >> 8) % alphabet.len()];
                sim.letter(letter);
            }
            6 => sim.space(),
            7 => sim.danda(),
            8 => {
                if !sim.session.is_composing() {
                    sim.word("amar");
                }
                sim.backspace();
            }
            _ => {
                if sim.session.is_composing() {
                    sim.session.move_selection(1);
                    sim.render();
                } else {
                    sim.word("desh");
                }
            }
        }
        sim.invariant(&format!("step {step}"));
        assert!(sim.doc.len() < 20_000, "document growth is bounded");
    }
}
