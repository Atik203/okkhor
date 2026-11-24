# 0020: Suggestion stability and raw Latin fallback

Status: Accepted (0.8)
Date: 2026-11-24

## Context

After the 0.7.0 beta the owner reported four user-visible problems: the popup
flickers and jumps while typing, suggestions reorder on almost every keystroke,
the popup can stay hidden after a Space, and an unknown phonetic word has no
usable row (for example `also` only produces a rule-guessed Bangla word).

Static inspection of the two reference keyboards clarified the useful parts:

- The Kontho suggestion window updates an already-visible list in place. It
  never calls `show()` again while visible, it only calls `SetWindowPos` when
  the computed position actually changed, and it paints through a memory DC.
- The Bongo Windows HUD also blits a memory DC, keeps the last caret position
  when a probe fails, and its candidate generator keeps the raw Latin stem as
  a selectable row and caches candidate lists by word.

Neither implementation is copied. Their observable behavior is the evidence
for the decisions below.

## Decision

### Visible list stability

- The daemon still computes candidates on every key. Commits (Space, Enter,
  digits, clicks) always read the current session, never the painted rows.
- The visible card is coalesced to at most one update per 30 ms and flushed by
  the daemon loop. This is a paint debounce, not an engine debounce.
- A visible card is updated in place: no repeat `ShowWindow`, no
  `SWP_SHOWWINDOW`, and `SetWindowPos` runs only when the target rectangle
  changed. The first show inserts the card into the topmost band
  (`HWND_TOPMOST`), matching both reference implementations; later moves leave
  the z-order alone.
- The card width does not shrink while it stays visible; it only grows to fit
  a longer row.
- `WM_PAINT` draws into a compatible memory bitmap and blits once.
- `WM_ERASEBKGND` remains suppressed.

### Caret and latching

- The caret worker publishes a completed sample even when a newer request is
  already queued. A one-frame-old anchor is better than no anchor.
- The daemon stores the last successful anchor and uses it when a probe fails.
- When a fresh sample lands while a composition is active, the popup is
  re-evaluated and re-shown without waiting for another keystroke.

### Ranking stability

- Recency time is frozen at the first letter of a composition. The same prefix
  must rank the same when the user backspaces and retypes it.
- A bounded per-composition prefix cache (16 entries, keyed by roman, previous
  word, and visible limit) returns the exact rows previously shown. It is
  cleared on commit, cancel, word-list change, and limit change.
- The highlight follows the selected text across a keystroke only after the
  user moves it with an arrow key. The automatic top row stays row 0, so Space
  still commits the best Bangla candidate rather than the raw Latin fallback.

### Raw input fallback

- The engine adds the Latin token with `CandidateSource::RawInput` (encoded as
  6 in the feature vector).
- If ranking truncation would drop it, it replaces the last row. It never
  changes the top row, so the golden top-1 rate is unchanged.
- When no dictionary-backed Bangla candidate exists, up to three English prefix
  completions are added below the Bangla rows. This is a fallback, not a second
  language mode.
- A Latin commit is not recorded as a Bangla transliteration pair. Learning
  only observes outputs that contain Bengali script.

## Consequences

- Typing fast produces at most ~33 visible updates per second, no position
  jitter from a redundant move, and no hide/show churn.
- A transient UI Automation failure no longer hides the card for the rest of a
  word.
- The last row may be the typed Latin word. Pressing Space still commits the
  top row; the raw token is reachable with the arrow keys, a digit, or a click.
- The feature vector gains one source code (6). The hybrid reranker was trained
  without it and remains default-off; the baseline ranker treats it as a normal
  candidate hint.
- The popup paint budget is now bounded by the 30 ms coalescing interval in
  addition to the existing popup-show budget.
