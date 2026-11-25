# ADR-0024: Embedded global next-word prior (`OKNW`)

- Status: accepted
- Date: 2026-11-25
- Related: ADR-0016, ADR-0021, ADR-0023,
  `docs/superpowers/plans/2026-11-24-0.10-kontho-feel-and-learning.md` (6.3c),
  `docs/learning.md`, `docs/model-roadmap.md`

## Context

The 0.10 ghost hint read next words only from Smriti, so a new user saw nothing
until they had typed the same pair themselves. Trained bigram tables exist for
both languages (Bangla 30.0M tokens, English 65.6M tokens), and the 1B neural
program is multi-day work that also needs an export and a Rust inference path.

The ghost hint runs after a commit on the daemon worker thread, not in the hook,
but it still has to stay inside the personal-rerank budget (< 2 ms p99) and must
never fail typing.

## Decision

### 1. `OKNW` v1 blob

`scripts/build-nextword.py` converts the trained bigram JSON into one compact
blob per language:

```text
header 32 bytes: "OKNW" | u16 version | u16 language | u32 word_count |
                 u32 word_off | u32 entry_count | u32 entry_off |
                 u32 follow_count | u32 follow_off
word table  : 8 bytes each (pool offset u32, length u16, reserved u16)
entries     : 12 bytes each, strictly sorted by previous-word bytes
              (prev id u32, follower start u32, follower count u16, reserved u16)
followers   : u32 word ids, most frequent first
pool        : UTF-8, words de-duplicated across the whole table
```

The builder verifies every previous word round-trips before writing. Both
blobs are committed with a manifest (bytes + SHA-256) and embedded in the
daemon with `include_bytes!`, like `OKS1`.

### 2. Read path

`okkhor_engine::nextword::NextWordPrior` validates the blob once (bounds,
UTF-8, strict ordering) and then answers with a binary search and slice reads:
no I/O, no unsafe, no per-query allocation beyond the returned vector. A bad
blob yields `None` and the caller falls back to local pairs.

The prior is a **global data table**, not code and not a learned-on-device
model. It is identical for every user and contains no user text.

### 3. Merge order

Ghost candidates are built as:

1. local Smriti next words (the user's own pairs), then
2. global prior followers for the same previous word, de-duplicated, capped at
   the caller's limit (5).

The local layer always wins a tie. The prior never changes the selected word on
its own and never injects text; the user still accepts with Tab.

### 4. Deferred: neural next-word model

The trained transformer/MLP artifacts are not shipped in 0.10.0. A neural model
ships later as a signed model update only after quantised export and the p99
gate; until then the ghost hint uses the bigram prior.

## Consequences

- New users get next-word hints immediately; steady-state users keep their
  personal ranking.
- About 25 MB is added to the daemon binary (Bangla 14 MB, English 11 MB).
- A missing or corrupt blob cannot stop typing; it only removes the prior.
- The format is versioned; a future model can grow the same schema (more
  followers, larger vocabulary) without changing the reader contract.
- Regenerating the blob is a documented script run, not a hand edit; the
  manifest hash travels with the file.
