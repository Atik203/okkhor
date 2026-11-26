# 0026: Mid-word composition caret

Status: Accepted (0.10). Date: 2026-11-26. Amends ADR 0022 for caret movement
that the composition itself performs.

## Context

ADR 0022 defines inline preedit as one rendered word with a roman buffer behind
it. Every erase count in `plan_edit` is measured from the end of that word,
which is why the composition has always ended with the document caret after the
last character.

The owner's requirement is Kontho-feel correction: Left/Right inside a word,
then a letter, and the change lands where the caret is. The arrows are already
swallowed while composing (`is_composition_key` covers `0x25..=0x28`), so the
buffer cursor can move without the target application seeing the key. What was
missing is the visible half: the buffer cursor moved, the document caret did
not, and the user had no way to see where the next letter would land.

ADR 0022 §57 says an *external* caret move resets the composition. That stays
true. This decision covers caret movement the composition performs itself, on
an anchor it still owns.

## Decision

### Tracked projection

`preedit_caret_back` is the number of UTF-16 units between the document caret
and the end of `preedit_rendered`. Zero is the only position the erase math
assumes, and it is also the resting state.

### Parking before every mutation

Anything that rewrites the rendered word — a per-key diff, a finalize batch, a
cancel — first calls `park_preedit_caret`, which sends Right × `preedit_caret_back`
in one batch and zeroes the offset. If parking fails, the document is **not**
touched: the position is unknown, and erasing from an unknown caret is exactly
the corruption class this ADR exists to prevent. The composition keeps its
buffer and retries on the next key.

### Projection after every render

After the diff, `caret_back_units(next, prefix)` returns the UTF-16 length of
the rendered right-hand half. `prefix` is the provider's rendering of
`roman[..cursor]`, i.e. the same pipeline that produced the word on screen.
When that prefix is a prefix of `next`, the offset is exact and the caret is
walked left into the word. When transliteration is not prefix-stable
(`k` → `ক` but `ki` → `কি`), it returns 0 and the caret stays at the word end.
A wrong caret is worse than no caret, so the fallback is deliberately the safe
one.

### Chord guard

An injected arrow while Ctrl or Alt is physically down arrives as a word- or
paragraph-level chord, not a one-unit move. `park_preedit_caret` therefore
refuses to inject under a chord and reports an unknown position. Callers that
end the composition under a chord drop the offset instead of injecting:
`safe to forget, never safe to guess`.

### Bounds and privacy

- Arrow batches clamp to `MAX_CARET_STEPS` (64) and share the `SendInput`
  boundary with commits; they carry `LLKHF_INJECTED`, so the hook skips them.
- No new I/O and no new allocation on the hook: the extra prefix request runs
  on the composition worker, inside the existing 3 ms candidate budget.
- The projection never logs text, only allowlisted error codes.

## Consequences

- Left/Right inside a Bangla word now shows the composition point, and typing
  edits there instead of at the end.
- A mid-word keystroke costs one extra bounded `SendInput` batch and one extra
  candidate request; the word end is restored before anything is erased, so the
  existing erase invariants are unchanged.
- Target applications that consume arrow keys themselves (some games, custom
  controls) can desync the projection. Parking is the guard: a target that
  swallows our Right batch leaves the caret inside the word, and the next
  mutation is skipped rather than applied to the wrong offset.
- Prefix-unstable transliterations keep the caret at the word end; they remain
  editable in the buffer, just without the visible cue.
- Undo history in the target still records each preedit step, as ADR 0022
  already accepts.
