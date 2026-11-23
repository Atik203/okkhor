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
| Injector | `okkhor-win` (`send_backspaces`, `WindowsInjector`) | worker | Unicode `SendInput`, `WM_CHAR` fast path, guarded clipboard fallback |
| Learning queue | `okkhor-daemon` → `okkhor-learn::Smriti` | background | `TransliterationCommitted` only; no I/O, never blocks typing |

## State diagram

```text
                    letter (a-z, no Ctrl/Alt/Win)
   ┌──────┐  ┌─────────────────────────────────────────┐  ┌───────────┐
   │ idle ├─►│              composing                  │─►│ committed │
   │      │  │ buffer += letter; candidates recomputed │  │           │
   └──────┘  │ popup shows numbered list (or hides)    │  └───────────┘
      ▲      └─────────────────────────────────────────┘   erase N + inject
      │         │ arrows │ digits │ backspace │ Esc/Tab │   + optional space
      │         ▼        ▼        ▼           ▼         │   popup hides
      │       select   commit   edit/       cancel:    │
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
| `send_backspaces` fails | no inject; reset + hide | `kind=inject_backspace` + code |
| Text inject fails (after erase) | hide popup; learning skipped | `kind=inject_text` + code |
| Trailing-space inject fails | Bangla already committed; hide | `kind=inject_space` + code |
| Popup create/show fails | typing continues without popup | `kind=popup_create` / `popup_show` + code |
| `caret_anchor` unavailable | popup hides; typing continues | none |
| Learn thread spawn fails | typing continues without learning | `kind=learn_thread` |
| Elevated target blocks `SendInput` | clipboard fallback or `Blocked` code | `kind=inject_backspace` / `inject_text` + code |

No failure path ever logs the buffer, the committed text, key codes, or
window state. The worker loop keeps its 10 ms cadence with no sleep longer
than 1 ms on the key path.

## What is intentionally not here yet

- English composition mode: every letter feeds the Bangla session; type with
  capture paused for English.
- `Enter` on a multi-line target commits instead of inserting a newline while
  composing; `Backspace` past the buffer start is untouched.
- No Smriti rerank of transliteration candidates yet (commits are queued for
  future personalization), no per-word context, no IME on/off awareness.
- The rule table is a small committed starter set; full phonetic coverage and
  the golden-corpus parity harness are tracked separately.
