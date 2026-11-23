# Okkhor input pipeline (0.5 composition MVP)

How a Banglish keystroke becomes committed Bangla text, and what happens when
each step fails. For the privacy contract behind every rule here, see
[`security-privacy.md`](../security-privacy.md).

## Components

| Piece | Location | Thread | Allowed to do |
| --- | --- | --- | --- |
| Low-level hook | `okkhor-win/src/win32.rs` (`keyboard_proc`) | hook thread | timestamp + enqueue, skip `LLKHF_INJECTED`; allocation-free, no I/O |
| Composition worker | `okkhor-daemon` (`run_daemon` loop, every 10 ms) | worker | session updates, popup, erase + inject |
| State machine | `okkhor-daemon/src/input_session.rs` | worker (pure) | buffer ≤ 32, candidates ≤ 5, selection; no Win32, no I/O, no logging |
| Suggestion popup | `okkhor-win` (`SuggestionPopup`, caret-aware) | worker | non-activating numbered list, no focus theft |
| Injector | `okkhor-win` (`inject_commit`, `WindowsInjector`) | worker | commits go out as one `SendInput` batch (erase + text + space); `WindowsInjector` remains for diagnostics/`text.insert` with `WM_CHAR`/clipboard paths |
| Learning queue | `okkhor-daemon` → `okkhor-learn::Smriti` | background | bounded `sync_channel(1024)`, `try_send` drop-and-count; `TransliterationCommitted` only; no I/O, never blocks typing |

## State diagram

```text
                    letter (a-z, no Ctrl/Alt/Win)
   ┌──────┐  ┌─────────────────────────────────────────┐  ┌───────────┐
   │ idle ├─►│              composing                  │─►│ committed │
   │      │  │ buffer += letter; candidates recomputed │  │           │
   └──────┘  │ popup shows numbered list (or hides)    │  └───────────┘
      ▲      └─────────────────────────────────────────┘   one SendInput batch:
      │         │ arrows │ digits │ backspace │ Esc/Tab │   erase N + inject
      │         ▼        ▼        ▼           ▼         │   + optional space
      │       select   commit   edit/       cancel:    │   popup hides
      │       wraps    (digit)  pass-thru   raw roman  │
      │                        at empty     stays      │
      │                                               │
      └──────── Space/Enter: commit selected (default 0) ──┘
                no candidates → clear, key passes through

   pause (tray/hotkey), focus change, Ctrl/Alt/Win chord → reset + hide popup
```

## Key handling

- Only key-down events drive the session; key releases pass through.
- Letters are lowercased into the buffer; the hook keeps pass-through, so the
  target application still receives every keystroke (nothing is swallowed).
- Digits `1`–`5` (row and numpad) commit the Nth candidate without trailing
  space. Out-of-range digits are ignored.
- `Left`/`Right`/`Up`/`Down` move the highlight and wrap around.
- `Space` commits the highlight and appends a space; `Enter` commits without
  it. With an empty buffer, or with no candidates, both keys pass through and
  the session clears (roman text stays as typed).
- `Backspace` edits the buffer; at an empty buffer it passes through — the
  daemon never erases beyond the buffer length.
- `Escape`/`Tab` cancel: the raw roman buffer is dropped from memory, the
  popup hides, typed text is untouched, and `Tab` keeps its focus behavior.
- Ctrl/Alt/Win chords end the composition first, so shortcuts such as Ctrl+C
  never grow the buffer (the app receives no letter for them).

## Candidate generation

`InputSession` queries `RuleTransliterator` (the committed rule table embedded
in `input_session.rs`: Bangla vowels, common consonants, frequent words) with
`max_candidates = 5, allow_partial = true`. Full matches (a rule input prefixes
the buffer) order before partial matches (the buffer prefixes a rule input);
within each group the engine's length-then-priority order applies. The
selection resets to index 0 on every buffer change.

## Failure behavior

| Failure | Behavior | Log (allowlisted, no text) |
| --- | --- | --- |
| Foreground window changed since composition started | no erase; reset + hide | `scope=daemon kind=focus_changed` |
| Commit batch fails with zero delivered | nothing erased, nothing injected; reset + hide | `kind=inject_commit` + code |
| Commit batch partially delivered | a roman prefix may already be erased without replacement (inherent to Win32); reset + hide | `kind=inject_commit` + code |
| UIPI-blocked target | `Blocked`: nothing erased, nothing injected; reset + hide | `kind=inject_commit`, no code |
| Popup create/show fails | typing continues without popup | `kind=popup_create` / `popup_show` + code |
| `caret_anchor` unavailable | popup hides; typing continues | none |
| Learn thread spawn fails / queue full | typing continues; drops counted, total printed once at shutdown | `kind=learn_thread` |
| Learn queue full (stalled learn thread) | event dropped + counted; typing never stalls | none (count only, printed at shutdown) |

No failure path ever logs the buffer, the committed text, key codes, or
window state.

## Latency

Two different budgets apply; do not conflate them:

- Hook callback: < 0.5 ms. It only timestamps and enqueues into the bounded
  `sync_channel(4096)` after the pure `should_enqueue` gate — no I/O, no
  allocation beyond the fixed `KeyEvent`, no `SendInput`.
- Worker: polls the queue every 10 ms, so worst-case key-to-commit latency
  grows by one poll quantum plus `SendInput`/popup cost. The 10 ms cadence is
  the design, not a violation: nothing sleeps on the hook thread.

Measured (unit-level only, not end-to-end): state-machine poll-to-decision
averages ≈ 6.8 µs per 5-letter composition round (≈ 1.4 µs per keystroke push +
refresh) in a release build on Ryzen 7 5800H / Windows 11, 2026-11-23
(`decision_latency_stays_in_budget`, `cargo test --release -p okkhor-daemon`).
End-to-end hook→commit latency (poll quantum + one `SendInput` batch + popup)
needs a real-key manual run and has not been measured yet.

## What is intentionally not here yet

- English composition mode: every letter feeds the Bangla session; type with
  capture paused for English.
- `Enter` on a multi-line target commits instead of inserting a newline while
  composing; `Backspace` past the buffer start is untouched.
- No Smriti rerank of transliteration candidates yet (commits are queued for
  future personalization), no per-word context, no IME on/off awareness.
- The rule table is a small committed starter set; full phonetic coverage and
  the golden-corpus parity harness are tracked separately.
- Numpad `1`–`5` commit candidates even with NumLock off (where those keys are
  arrows); USB/HID-level distinction is future work.
- The commit-time focus re-check cannot close the race between the check and
  the `SendInput` batch (standard Win32 TOCTOU); the window makes it narrow,
  and a wrong-window batch is still bounded to the buffer length.
