# 0022: Inline Bangla preedit

Status: Accepted (0.10). Date: 2026-11-24. Supersedes the "inline preedit is
later work" statement in `AGENTS.md` and `README.md`.

## Context

Kontho shows Bangla in the document on every keystroke. Static/Runtime analysis
(`docs/superpowers/analysis/0.10-kontho-inline-preedit.md`) recovered the exact
mechanism:

- `Ui.convertToLayout` runs on every key and writes synchronously.
- It erases `len(previous_bangla) - LCP(previous, next)` and types only the
  suffix.
- Backspace pops one roman character and re-renders.
- Up/Down/Tab rewrite the document through the same diff.
- The popup is coalesced on a 50 ms timer, capped at 10 rows, with the raw roman
  string at row 0.

The reference has structural weaknesses: codepoint-level diffs can split a
grapheme cluster, erase and insert are separate `SendInput` batches, the
`completeWordBySpace` path assumes one previous character, and the composition
state is updated before injection succeeds.

## Decision

Okkhor implements the same feel with a stricter state machine.

### Source of truth

- `roman`: the bounded roman buffer, one per composition.
- `rendered`: the exact UTF-16 string last written into the document.
- Never diff against user-typed text; always diff against `rendered`.

### Per-key edit

1. `next = translate(roman)` from the frozen candidate request.
2. `lcp = common_prefix_chars(rendered, next)`.
3. Clamp `lcp` to a grapheme-cluster boundary: if `next[lcp]` is a combining
   mark, ZWJ or ZWNJ, move `lcp` back to the base character. Bangla marks are
   U+09BC and U+09BE..U+09CD, U+09D7; joiners are U+200C/U+200D.
4. `erase = utf16_len(rendered[lcp..])`, `insert = next[lcp..]`.
5. Emit **one atomic `SendInput` batch**: all backspaces, then all Unicode units.
6. Skip entirely when `erase == 0 && insert.is_empty()`.
7. On a short `SendInput`, mark the composition dirty and resync from the caret;
   never count backspaces to guess.

### Events

- **Backspace** pops the roman buffer and a history snapshot, then recomputes
  the diff. It never sends a raw backspace into the previous word.
- **Space / Enter** freeze the render, clear the buffer, close the popup, and
  emit the single learning event for the word.
- **Tab / Up / Down** change the highlighted row and apply the same diff with
  `next = candidate`; the roman buffer is unchanged so typing resumes.
- **Escape** clears the buffer and leaves the text (Kontho behaviour).
- **Caret move, click, focus change, pause, layout/encoding change, password
  field, IME/bidi field** reset the composition. Never edit an anchor we no
  longer own.

### UTF-16 and bounds

- Erase and insert counts are UTF-16 code units, matching `SendInput` and UIA.
- `roman` is capped (64 chars). At the cap, commit and pass keys through.
- The existing per-key `SendInput` budget and 3 ms candidate deadline still
  apply.

### Setting

`Inline Bangla preedit` is on by default and can be turned off; off restores
the 0.9 commit-on-Space behaviour exactly.

## Consequences

- Bangla appears live per keystroke and Backspace walks it back in order.
- Every keystroke is a document mutation; target-app undo history will contain
  preedit steps. This is accepted and documented.
- Grapheme clamping is stricter than Kontho: a cluster may be erased and
  retyped more often, but a boundary can never split a base and its mark.
- Atomic batches avoid the reference's intermediate-text and autocorrect races.
- English and fixed layouts are unchanged.
- Real-key QA in Notepad, Word, Chrome, VS Code, IME, elevated and RDP remains
  required; tests cannot replace it.
