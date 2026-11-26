# Okkhor input pipeline (0.6 composition + suggestions, 0.9 fixed layouts)

## 0.9 fixed-layout path

When `settings.layout` is `national`, `probhat` or `bijoy`, the input worker
takes the fixed path (ADR 0021): the hook swallows letter keydowns and their
keyups, the worker reads Shift/AltGr, and the physical key is mapped to a
Bangla code point. There is no roman buffer, no popup and no erase; one
`SendInput` batch inserts the mapped text. Keys that can start a conjunct are
held for at most 30 ms; a matching next key replaces them, a timeout flushes
them, and focus changes, pause or mode changes clear the buffer. If injection
fails, the original ASCII letter is re-emitted so no keystroke is lost.

How a Phonetic keystroke becomes committed Bangla text, and what happens when
each step fails. For the privacy contract behind every rule here, see
[`security-privacy.md`](../security-privacy.md). The suggestion index and
English completion rules are in ADR-0016.

## 0.6 additions

- **Suggestion engine** (`okkhor-engine::suggest`) replaces the session's
  ad-hoc starter + rule merge. Stage order: curated exact, user exact, roman
  exact/prefix, autocorrect exact, rule output/vowel variant, dictionary
  exact, dictionary prefix, suffix expansion, Smriti rank.
- **Rows:** `popup_max_items` defaults to 8 (max 10); digits `1`-`9` and `0`
  select the tenth.
- **English completions:** a separate token tracker in the daemon; the popup
  shows after three letters. `Tab` accepts only while visible, `Esc` dismisses,
  a click accepts. Space/Enter/punctuation never replace.
- **State:** the daemon signals `Local\Okkhor.State.Changed.v1` on mode, pause,
  encoding, and bar visibility changes; the quick bar refetches its snapshot.
- **Caret:** resolved on a dedicated latest-wins thread when the measurement
  gate passes; the key path never blocks on UI Automation.

## Components

| Piece | Location | Thread | Allowed to do |
| --- | --- | --- | --- |
| Low-level hook | `okkhor-win/src/win32.rs` (`keyboard_proc`) | hook thread | timestamp + enqueue, skip `LLKHF_INJECTED`; allocation-free, no I/O |
| Composition worker | `okkhor-daemon` (`run_daemon` loop, every 5 ms) | worker | session updates, popup, erase + inject |
| State machine | `okkhor-daemon/src/input_session.rs` | worker (pure) | buffer ≤ 32, candidates ≤ 10 (default 8), selection; no Win32, no I/O, no logging |
| Suggestion engine | `okkhor-engine/src/suggest` | worker (pure) | dictionary/prefix/autocorrect stages over the embedded `OKS1` index; bounded, no I/O, no clock |
| Suggestion popup | `okkhor-win` (`SuggestionPopup`, caret-aware) | worker | non-activating numbered list, no focus theft |
| Injector | `okkhor-win` (`inject_commit`, `WindowsInjector`) | worker | commits go out as one `SendInput` batch (erase + text + space); `WindowsInjector` remains for diagnostics/`text.insert` with `WM_CHAR`/clipboard paths |
| Learning queue | `okkhor-daemon` → `okkhor-learn::Smriti` | background | bounded `sync_channel(1024)`, `try_send` drop-and-count; transliteration, accepted-suggestion, and correction events; encrypted `learning.bin` snapshots; no key-path I/O |

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

- Only key-down events drive the session; key releases follow their key-down's
  swallow decision.
- Letters keep the case the user typed (`shift XOR caps`). Avro reads `T`, `D`,
  `N`, `S`, `R` and the digraphs built from them as different keys, so folding
  to lowercase would make ট, ড, ণ, শ and ড় unreachable. Non-semantic capitals
  are normalized to their lowercase key before the rule matcher sees them
  (ADR 0027). Letters and Backspace keep pass-through, because one target
  character per buffer character keeps the erase count exact.
- Commit keys — Space, Enter, digits `0`–`9` (row and numpad), and the arrows —
  are **swallowed while a composition is active**. They either append a
  character (space, digit) or move the caret, so letting them reach the target
  would make the commit's backspaces erase the wrong text. The worker re-emits
  the key where it belongs: Space via `trailing_space`, Enter via
  `send_enter`, an out-of-range digit as literal text.
- Any other key (punctuation, Home/End, Delete, function keys) cancels the
  composition and acts normally, so an unhandled key can never shift the
  target text out of sync with the buffer. Modifier presses (Shift/Ctrl/Alt/
  Win/Caps/Num/Scroll) do not cancel; Shift is normal capital typing.
- Digits `1`–`9` and `0` commit the matching candidate without trailing
  space; out-of-range digits are re-emitted as text after the composition ends.
- `Left`/`Right`/`Up`/`Down` move the highlight and wrap around; they never
  move the target caret.
- `Space` commits the highlight and appends exactly one space; `Enter` commits
  and then emits a real newline. With no candidates, the roman text stays and
  the key is re-emitted so it is never lost.
- `Backspace` edits the buffer; at an empty buffer it passes through — the
  daemon never erases beyond the buffer length.
- `Escape`/`Tab` cancel: the raw roman buffer is dropped from memory, the
  popup hides, typed text is untouched, and `Tab` keeps its focus behavior.
- Ctrl/Alt/Win chords end the composition first, so shortcuts such as Ctrl+C
  never grow the buffer (the app receives no letter for them).

## Bangla conventions (Unicode output)

Bengali convention applies only in Bangla **Unicode** mode; English and Bijoy
keep the ASCII keys.

- An unshifted `.` becomes the Bengali danda `।` (U+0964). It commits a live
  composition first, then emits the danda. Shift+`.` still types `>`, so the
  hook checks Shift before swallowing.
- Idle digits `0`-`9` (row and numpad) become `০`-`৯`. While composing they
  remain candidate selectors; an out-of-range digit is re-emitted as the
  Bengali numeral after the composition ends.
- Both remaps are published to the hook with the mode/encoding state. They are
  off in English and in Bijoy (legacy documents own their punctuation and
  digits).

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
- Worker: polls the queue every 5 ms (0.5.1: was 10 ms), so worst-case key-to-commit latency
  grows by one poll quantum plus `SendInput`/popup cost. The short cadence is
  the design, not a violation: nothing sleeps on the hook thread.

Measured (unit-level only, not end-to-end): state-machine poll-to-decision
averages ≈ 6.8 µs per 5-letter composition round (≈ 1.4 µs per keystroke push +
refresh) in a release build on Ryzen 7 5800H / Windows 11, 2026-11-23
(`decision_latency_stays_in_budget`, `cargo test --release -p okkhor-daemon`).
End-to-end hook→commit latency (poll quantum + one `SendInput` batch + popup)
needs a real-key manual run and has not been measured yet.

## Mode behavior (ADR 0014 §1–§3)

`InputMode` (`bangla` | `english`, `--mode` flag) gates the session; pause
(`Ctrl+Alt+Shift+P` / tray) stays a separate capture switch checked first.
The CLI flag selects the initial mode only; persisting the last mode across
restarts (ADR 0014 §1) arrives with the settings-store wiring (Task 3) and is
not claimed here.

- Bangla (default): letters feed `InputSession` (buffer ≤ 32, candidates ≤ 5)
  as before.
- English: no composition — the toggle clears the buffer, hides the popup,
  and the worker drains the key queue without touching the session, so keys
  pass through untouched. Pause keeps working in either mode.
- Toggle sources: `F9` (falls back to `Ctrl+Space` when `RegisterHotKey` for
  `F9` fails; the active binding is printed once at startup and a
  `scope=hotkey kind=hotkey_fallback` record is logged), tray left-click
  (`WM_LBUTTONUP`), or the `Switch to …` tray menu item. A double-click is
  two left-clicks (net no mode change) and never pauses; pause is available
  via the tray menu and the pause hotkey.
- Each toggle clears/hides, updates the tray label + tooltip
  (`Okkhor - Bangla/English (active/paused)`), shows a non-activating
  `বাংলা` / `English` toast near the caret for ~800 ms, and logs only the
  allowlisted `scope=daemon kind=mode_toggle` code — never text. The toast
  expiry is checked once per poll, so toggling never blocks the key path.

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
