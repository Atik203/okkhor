# ADR-0016: Suggestion index, English completions, and instant state

- Status: accepted
- Date: 2026-11-24
- Supersedes: the live-path stage order in ADR-0011 for 0.6 (the parity
  harness keeps ADR-0011 semantics)
- Related: ADR-0002, ADR-0003, ADR-0014

## Context

0.5.1 ships a 229-entry curated list plus the Avro rule table. That gives one
rule output and a few corrections, not a dictionary. Users see five rows and
no English suggestions. The quick bar polls its mode chip every 15 s, and the
popup resolves the caret through UI Automation on the key path, which adds a
visible delay in Chromium.

The owner supplied the Avro Keyboard 5.6.0 portable build and the Nms Kontho
installation. Analysis found:

- Avro `Database.db3` (SQLite): 159,426 unique Bangla words in 47 onset
  tables plus a 749-row suffix table; no frequency column.
- Avro `autodict.dct`: 2,481 roman→roman autocorrect pairs.
- Nms `bangla_dict.bin`: a `WTR1` binary trie with 135,410 roman→Bangla
  pairs; a validated parser is deferred (0.4 contract).
- `wordfreq` provides Bengali/English zipf popularity and a top-word list
  that fills common forms missing from the Avro database.

## Decision

### 1. One committed suggestion index

Build `crates/okkhor-engine/assets/suggest/suggest-v1.bin` from the Avro
database, the suffix table, the autocorrect dictionary, the curated starter
list, and `wordfreq` top lists for `bn` and `en`. The index is deterministic,
versioned (`OKS1`), NFC-normalized, and carries a manifest with source
SHA256s, counts, and the wordfreq version. It is embedded with
`include_bytes!` and parsed once at daemon startup into a zero-copy view.
No source path is committed.

`WTR1` is not parsed in 0.6. The Avro data matches the Avro phonetic rules
already shipped, so it is the lower-risk source.

### 2. Stage order

For a roman buffer, the live engine uses:

1. curated exact (starter list);
2. user dictionary exact;
3. autocorrect exact;
4. rule output exact and explicit-vowel variant (`o` → `O`);
5. dictionary exact;
6. dictionary prefix, bounded to the 40 most popular completions;
7. suffix expansion if it improves the golden corpus;
8. merge/dedup, `Smriti::rank`, top N.

The query is bounded (≤64 staged candidates, ≤8 rule outputs), performs no
I/O, reads no clock, and has no `unsafe`. Corrupt data fails closed to the
rule-only path.

### 3. Ranking

Static score is a documented combination of match class, zipf popularity, and
completion length. `Smriti` adds learned frequency, recency, acceptance,
correction penalty, and bigram context. The base weight stays 1.0 so the
static order is meaningful even with an empty learning store.

### 4. English completions

English mode tracks the current Latin token and shows the same popup after
three letters. It never auto-replaces: Space/Enter/punctuation pass through.
Tab accepts only while the popup is visible; Esc dismisses; a click accepts
the clicked row. The popup hides when the top suggestion equals the token, so
Tab behaves normally for a fully typed word. The feature is a setting
(default on). Password fields, pause, chords, and focus changes suppress it.

### 5. Instant state

A session-local auto-reset event `Local\Okkhor.State.Changed.v1` wakes the UI
when the mode, pause, encoding, or bar visibility changes. It carries no data;
the UI refetches the existing snapshot. Polling remains a 5 s fallback.

### 6. Caret resolution

UI Automation resolution moves to a dedicated latest-wins thread with a
deadline. The key path uses the last known anchor and never blocks on UIA.
The popup window itself is unchanged (non-activating, per-monitor DPI).

## Consequences

- The daemon grows by the embedded index (target ≤8 MiB) and a small parse
  cost at startup, off the key path.
- Regenerating the index requires the owner-provided sources and the pinned
  wordfreq version; the committed manifest makes drift visible.
- The old `CandidateEngine` remains the 0.4 parity harness; the live daemon
  uses `SuggestionEngine`.
- Public distribution of the derived dictionary data remains an owner
  decision; 0.6 is a personal/QA beta and the manifest records provenance.
- If the caret tracker does not improve measurements, only the theme/paint
  fixes ship and the negative result is recorded.

