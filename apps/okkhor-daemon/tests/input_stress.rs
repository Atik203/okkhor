#![forbid(unsafe_code)]
//! 10,000-event composition-worker stress test.
//!
//! This drives the real `InputSession` state machine with a deterministic fake
//! provider. It does not install the global hook and does not inject text; the
//! hook intentionally ignores injected keys. The owner real-key stress pass is
//! still required by `docs/qa/0.7-checklist.md`.
//!
//! The goal is bounded state and no panic under a long mixed stream of letters,
//! commits, backspaces, selection moves, cancels, and resets. No user text is
//! logged or persisted.

#[allow(dead_code)]
#[path = "../src/input_session.rs"]
mod input_session;

use input_session::{
    BackspaceOutcome, InputSession, MAX_BUFFER_LEN, MAX_CANDIDATES, SuggestionProvider,
};
use okkhor_engine::UserWord;

/// Deterministic provider: a short candidate list built from the prefix.
struct FakeProvider;

impl SuggestionProvider for FakeProvider {
    fn suggestions(
        &self,
        roman: &str,
        limit: usize,
        _previous: Option<&str>,
        _now_ms: u64,
        _user_words: &[UserWord],
    ) -> Vec<String> {
        (0..limit.min(5))
            .map(|index| format!("{roman}{index}"))
            .collect()
    }
}

fn assert_bounds(session: &InputSession<FakeProvider>, iteration: usize) {
    assert!(
        session.buffer().len() <= MAX_BUFFER_LEN,
        "buffer exceeded at iteration {iteration}: {}",
        session.buffer().len()
    );
    assert!(
        session.candidates().len() <= MAX_CANDIDATES,
        "candidates exceeded at iteration {iteration}: {}",
        session.candidates().len()
    );
    if !session.candidates().is_empty() {
        assert!(
            session.selected() < session.candidates().len(),
            "selection {} out of range at iteration {iteration}",
            session.selected()
        );
    }
}

#[test]
fn ten_thousand_mixed_events_keep_state_bounded() {
    let mut session = InputSession::new(FakeProvider);
    let mut commits = 0_usize;
    let started = std::time::Instant::now();

    for iteration in 0..10_000 {
        match iteration % 11 {
            0..=5 => {
                let letter = b'a' + (iteration % 26) as u8;
                session.push_letter(letter);
            }
            6 => {
                if session.commit(true).is_some() {
                    commits += 1;
                }
            }
            7 => {
                if session.backspace() == BackspaceOutcome::Empty {
                    // Pass-through path; nothing to assert here.
                }
            }
            8 => session.move_selection(if iteration % 2 == 0 { 1 } else { -1 }),
            9 => {
                let _ = session.cancel();
            }
            _ => session.reset(),
        }
        assert_bounds(&session, iteration);
    }

    println!(
        "10,000 mixed events: commits={commits} elapsed={:?}",
        started.elapsed()
    );
    assert!(commits > 100, "expected many commits, got {commits}");
}

#[test]
fn ten_thousand_letters_never_exceed_the_buffer_cap() {
    let mut session = InputSession::new(FakeProvider);
    for iteration in 0..10_000 {
        let letter = b'a' + (iteration % 26) as u8;
        session.push_letter(letter);
        assert_bounds(&session, iteration);
    }
    assert_eq!(session.buffer().len(), MAX_BUFFER_LEN);
}
