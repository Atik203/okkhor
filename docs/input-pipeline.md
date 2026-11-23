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

`InputSession` queries `PhoneticTransliterator` (the real Avro phonetic table:
250 ordered regex rules embedded in `crates/okkhor-transliterate/assets/` and
compiled once at startup via `from_embedded_avro`, off the typing path) with
`max_candidates = 5, allow_partial = true`. The table is deterministic, so a
buffer yields at most one candidate — the table form — and the selection
resets to index 0 on every buffer change. Lowercase `o` after a consonant is
the inherent vowel (deleted: `bhalo` → ভাল), while capital `O` is the ো-kar
(`bhalO` → ভালো); dictionary-based corrections (আপনি, ধন্যবাদ) come later.
If the embedded table somehow fails to load, the daemon logs one allowlisted
startup error (`scope=transliterate` + the failure kind, never text) and runs
with an empty engine (no candidates, keys pass through) instead of panicking.

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

## Mode behavior (ADR 0014 §1–§3)

`InputMode` (`bangla` | `english`, default `bangla`, `--mode` flag) gates the
session; pause (`Ctrl+Alt+Shift+P` / tray) stays a separate capture switch
checked first.

- Bangla: letters feed `InputSession` (buffer ≤ 32, candidates ≤ 5) as before.
- English: no composition — the toggle clears the buffer, hides the popup,
  and the worker drains the key queue without touching the session, so keys
  pass through untouched. Pause keeps working in either mode.
- Toggle sources: `F9` (falls back to `Ctrl+Space` when `RegisterHotKey` for
  `F9` fails; the active binding is printed once at startup and a
  `scope=hotkey kind=hotkey_fallback` record is logged), tray left-click
  (`WM_LBUTTONUP`), or the `Switch to …` tray menu item. Double-click still
  toggles pause.
- Each toggle clears/hides, updates the tray label + tooltip
  (`Okkhor - Bangla/English (active/paused)`), shows a non-activating
  `বাংলা` / `English` toast near the caret for ~800 ms, and logs only the
  allowlisted `scope=daemon kind=mode_toggle` code — never text. The toast
  expiry is checked once per 10 ms poll, so toggling never blocks the key path.

## What is intentionally not here yet

- `Enter` on a multi-line target commits instead of inserting a newline while
  composing; `Backspace` past the buffer start is untouched.
- No Smriti rerank of transliteration candidates yet (commits are queued for
  future personalization), no per-word context, no IME on/off awareness.
- The candidate table is the real embedded Avro phonetic table (deterministic
  single candidate per buffer); dictionary-based corrections (আপনি, ধন্যবাদ)
  and the golden-corpus parity harness are tracked separately.
- Numpad `1`–`5` commit candidates even with NumLock off (where those keys are
  arrows); USB/HID-level distinction is future work.
- The commit-time focus re-check cannot close the race between the check and
  the `SendInput` batch (standard Win32 TOCTOU); the window makes it narrow,
  and a wrong-window batch is still bounded to the buffer length.
