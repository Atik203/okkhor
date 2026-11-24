# 0021: Fixed-layout keyboard engine

Status: Accepted (0.9). Date: 2026-11-24. Supersedes the "metadata only"
decision recorded in `docs/learning.md` for layout: from 0.9 the layout setting
selects a real engine, but the fixed path still has no suggestion stages.

## Context

0.8.1 advertised one layout because the other codes in 0.8.0 were labels with
no table behind them: the layout value was persisted and echoed in `status`,
but `SuggestRequest` had no layout field and `encode_layout` only produced a
feature integer. A static audit of the owner's reference installs showed that
all three real keyboards genuinely switch tables:

- Avro ships five XML `.avrolayout` fixed-keyboard files with 203 physical
  key/modifier entries each, plus a built-in phonetic engine.
- Kontho ships nine pickle `.nmsLayout` files; `settings.pkl` `currentLayout`
  selects one, and a `phonetic` boolean switches between a roman engine and a
  physical-key map.
- Bongo embeds JSON `National.json`, `Borno.json` and `Probhat.json` with
  `include_str!` and replaces `PhoneticMethod` with `FixedMethod` at runtime.

The owner asked for all layouts to work in one release, more stable and
accurate than those implementations, with edge cases handled.

## Decision

### Two engine paths, one dispatcher

Keep `SuggestionEngine` (Avro, Khipro, Phonetic/Banglish) unchanged as the
phonetic path. Add an independent `okkhor-lang::fixed_layout` module for
National, Probhat and Bijoy. A new daemon `LayoutEngine` selects the path from
`settings.layout` once at startup and falls back to Avro on any load failure.

A single unified engine was rejected: it would put fixed-key semantics inside
the suggestion engine, change many call sites and golden rows, and increase
risk on the typing path that 0.8 just stabilized.

### Physical keys and modifiers, not roman sequences

A fixed layout maps `physical key + modifier` to a Bangla code point. Key
identity is a stable name (`Key_N`, `Num1`, `Key_OEM3`) derived from the Win32
virtual key. Modifier priority is strict: ShiftAltGr, AltGr, Shift, Normal.
Bounded multi-key sequences (at most three keys) are matched longest-first.
All outputs are NFC at build time.

### Swallow the raw letter

The references let the raw key reach the target and later erase it. That
breaks when a Bengali IME composes several keys into one character, and it
caused a real 0.9 bug where a browser child-window swap reset the composition
and left a stray first `a`. In fixed mode the hook now swallows letter keydowns
and their keyups, and the daemon injects only the mapped character. If
injection fails, the original ASCII letter is re-emitted so no keystroke is
lost. Modifier state is queried on the worker, never in the hook callback.

### Data provenance

Layout facts come from public standards and are verified statically against
the reference installs. No pickle is loaded at runtime, no reference file is
copied into git, and no asset is read from disk on the hot path. Every asset
carries provenance fields. Bijoy reuses the National key map with the existing
Bijoy output encoding, matching Bongo's `bijoy` alias; no separate input table
is invented.

### No suggestions in fixed layouts

A fixed layout has no roman prefix to complete. The popup stays hidden and
fixed-layout characters are not learned as transliteration pairs. A later
release may add a next-word strip on committed words without changing the
fixed mapping.

## Consequences

- Every accepted layout code produces real, different behavior after 0.9. The
  selector advertises four of them: Avro (Phonetic), National, Probhat and
  Bijoy. `phonetic` and `khipro` remain valid stored codes with their own Roman
  rule tables but are not offered as separate new-user choices.
- Fixed layouts never depend on the system keyboard language or IME, because
  no raw character is delivered to the target.
- Two code paths must be tested separately; golden tests are required per
  layout and per modifier combination.
- The hook still only timestamps and enqueues; the extra modifier query is on
  the worker, inside the existing hot-path budget.
- Unknown or corrupt layout codes still fall back to Avro, so typing survives
  a bad setting or failed asset.
