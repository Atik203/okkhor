# ADR 0015: Offline replay harness and feature schema v1 (`okkhor-train`)

- Status: Accepted
- Date: 2026-11-23

## Context

Milestone 0.6 needs a frozen, reproducible way to compare the Smriti baseline
against the candidate engine (and later a learned reranker) without touching
real user text, the network, or the typing path. ADR 0007 requires replay-first
evaluation with top-1/top-5, MRR, keystroke savings, and p99 latency segmented
by language and layout. `docs/model-roadmap.md` defines a 16-field feature
schema v1; it had no code owner yet.

## Decision

1. **New offline crate `okkhor-train` (not shipped in the app).** Workspace
   member with `#![forbid(unsafe_code)]`. It owns:
   - `FeatureVectorV1` with `SCHEMA_VERSION = 1` and the 16 fields from
     `docs/model-roadmap.md` §"Feature schema v1" in stable declaration order
     (`base_score`, `candidate_source`, `language`, `layout`, `prefix_len`,
     `word_len`, `user_word_count`, `pair_count`, `recency`, `acceptance_rate`,
     `correction_rate`, `bigram_score`, `phonetic_score`, `is_user_vocab`,
     `context_language`, `script_mix`; the optional `trigram_score` is
     explicitly out of v1).
   - `extract_features(&SuggestionRequest, &Candidate, &LearningSnapshot)
     -> Result<FeatureVectorV1, FeatureError>`: no I/O, deterministic, bounded
     allocation (small normalized keys only). Snapshot lookups use new additive
     read-only accessors on `okkhor-learn` (`word_stat`, `pair_stat`,
     `bigram_stat`, `contains_word`); scoring behavior is unchanged.
   - Replay format v1 (JSONL, one case per line): `{schema, case_id, language,
     layout, prefix, context, expected, shown, accepted, candidates}` with
     strict validation (schema, id syntax/uniqueness, length caps, NFC,
     `accepted` in range).
   - Metrics: top-1, top-5, MRR, acceptance rate, keystroke savings,
     per-language/per-layout segments, ranking latency p50/p95/p99 with the
     nearest-rank method shared with `okkhor-bench`.
   - `okkhor-replay` binary: `--generate --out <path> --seed <u64>
     [--cases <n>]` (refuses `< 200`) and `--replay <path>
     (--engine | --smriti) [--json | --markdown] [--out <report>]`, defaulting
     to git-ignored `bench-out/`.
2. **Synthetic-only frozen dataset.**
   `crates/okkhor-train/data/synthetic-replay.jsonl` (240 cases, seed
   `20261123`) is hand-expanded from synthetic word lists and the committed
   synthetic layout ids. No downloads, no user data, no legacy execution. The
   generator is deterministic: same seed yields byte-identical output
   (checksum recorded in `docs/performance.md`). Regeneration overwrites the
   file; any content change is a visible diff.
3. **Baselines reported, not claimed.** The 0.6 baseline table in
   `docs/performance.md` records engine and cold-start Smriti numbers on the
   synthetic set with an explicit **synthetic-only** caveat. The Smriti harness
   ranks the case's closed 5-candidate pool, so its top-5 is 1.0 by
   construction; the informative deltas are top-1/MRR/latency. No real-world
   quality claim is made.

## Consequences

Positive:

- Feature schema has a code owner and a determinism test; schema bumps are
  detectable.
- Task 2 (logistic reranker) can reuse `FeatureVectorV1`, the replay set, and
  the metrics without new data plumbing.
- CI covers parsing, bounds, generator reproducibility, and metric math.

Negative:

- The synthetic set measures harness correctness and relative movement, not
  Bangla/English quality; a real-data eval slice still needs catalog review.
- `okkhor-learn` gains public read-only accessors; they expose aggregate
  counts to offline tools only, not new persistence.

## Alternatives considered

- **Feature extraction inside `okkhor-learn`:** kept the schema in the offline
  crate instead so the typing-path crate stays minimal; `okkhor-learn` only
  exposes the lookups extraction needs.
- **Reusing the golden-corpus format:** the 3-case golden corpus pins engine
  parity; replay needs acceptance indices, candidate pools, and segments, so a
  dedicated v1 format was justified.
- **Downloading a public corpus now:** rejected for this task; remote
  acquisition stays on `uiu-h200` per `docs/design/datasets.md` and needs
  catalog review before anything real enters the repo.

See `docs/design/replay.md`, `docs/model-roadmap.md` (replay notes),
`docs/learning.md` (feature contract), and `docs/performance.md` (baseline).
