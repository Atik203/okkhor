# ADR 0011: Candidate engine (`okkhor-engine`) for 0.4

- Status: Accepted
- Date: 2026-11-23
- Normative detail: [0.4 engine contracts](../design/0.4-engine-contracts.md)
- Plan: [0.4 candidate engine](../superpowers/plans/0.4-candidate-engine.md)

## Context

Phase 2 (Bornomala core, version 0.3) delivered three skeleton crates with a
binding contract (ADR 0010): `okkhor-lang` (layout/encoding/dictionary data and
a synthetic-fixture importer), `okkhor-transliterate` (rule engine and
normalization helpers), and `okkhor-compat` (safe FFI boundary). What 0.3 did
not deliver is the working local suggestion pipeline: nothing yet composes
dictionary prefix lookup, transliteration, and Smriti ranking into the
`< 3 ms p99` candidate path, and there is no approved corpus against which to
measure top-5 parity.

The 0.4 plan therefore adds one crate, `okkhor-engine`, that owns composition
only: it holds references to already-constructed `okkhor-lang` assets, an
`okkhor-transliterate` rule engine, and an `okkhor-learn::Smriti` ranker, and
exposes a single infallible query, `CandidateEngine::suggest`, on the input
worker. AGENTS.md rule 9 requires an ADR before any new crate exists, so this
ADR is the blocking prerequisite for the 0.4 implementation task.

Constraints carried forward:

- ADR 0001 (crate boundaries, `unsafe` confinement) and AGENTS.md rules 1–3
  (typing path: no I/O, network, AI, unbounded allocation, or text logging).
- ADR 0010 (0.3 contracts): `okkhor-core` is not modified in this milestone;
  fixtures are synthetic-only JSON; errors expose `kind()`/`code()` and never
  leak user text, paths, or asset bytes.
- `docs/learning.md` leaves "should transliteration pairs include the active
  layout in the key?" open (0.3 gate G3). Until the lead closes it,
  `SuggestionRequest::layout` stays metadata only.

Owner-granted reference findings (read-only, verified by the lead — the 0.4
tasks never access the legacy folder): legacy `*.bin` dictionaries use a custom
`WTR1` magic + version binary trie matching `word_trie.dll`; `.nmsLayout` /
`.nmsEnco` files are serialized dicts with metadata (`name`, `type`, `lang`,
`dic`, `reverseMap`, `version`, `modification-time`, `developerName`), a
`keyMap`, `compiledKeys`, and regex rules. Importers for those real formats
are explicitly out of scope for 0.4 (synthetic fixtures only); the deferred
real-format importer is recorded as follow-up work below, and any future
importer must be test-validated and must never commit proprietary bytes.

## Decision

Adopt [docs/design/0.4-engine-contracts.md](../design/0.4-engine-contracts.md)
as the binding public contract for `okkhor-engine`. Its essential decisions
are:

1. **One new crate, four allowed dependencies.** `okkhor-engine` depends only
   on `okkhor-core`, `okkhor-lang`, `okkhor-transliterate`, and `okkhor-learn`.
   It never touches `okkhor-compat`, `okkhor-win`, I/O, regex, or the network,
   and carries `#![forbid(unsafe_code)]`. One workspace-member line for this
   crate only (Task 2); all other shared files stay lead-owned.

2. **Single public query.** `CandidateEngine::suggest(&self, request:
   &SuggestionRequest) -> Vec<Suggestion>` is infallible: degradation means
   fewer candidates, never an error or a panic. Construction
   (`CandidateEngine::new`) is the only fallible step and validates the
   per-language budgets once.

3. **Fixed composition order.** Dictionary prefix lookup over `Dictionary`
   entries, then transliteration application for Romanized input, then
   merge + dedup, then `Smriti` rank (`okkhor_core::Ranker::rank`), then
   top-N truncation to `request.limit` clamped by budget. The order is
   normative; reordering requires a new ADR.

4. **Engine owns query-path normalization.** `suggest` normalizes the prefix
   (and `roman_input` when present) exactly once via
   `okkhor_core::normalize_lookup` and passes normalized strings downstream.
   Downstream normalizers are idempotent, so double normalization stays
   correct, but no stage may re-derive a different key.

5. **Per-language budgets.** `EngineConfig` carries a separate `LanguageBudget`
   (`max_prefix_hits`, `max_transliteration_hits`, `max_merged`,
   `max_limit`) for Bangla and English, with documented defaults. Every
   collection on the query path is capped; overlong input (≥ 256 bytes,
   matching the transliteration crate threshold) yields an empty `Vec`.

6. **Determinism.** Identical assets + identical `Smriti` state + identical
   request ⇒ byte-identical output. Tie-breaks follow the implemented
   two-level rule (`score desc, text asc`, stable sort retaining merge
   order — design doc §7.2); no `HashMap` iteration leaks
   into output order; no wall-clock, RNG, or thread timing on the query path
   (`now_ms` comes from the caller).

7. **Golden corpus and parity metric.** The corpus is synthetic-only JSON
   under `crates/okkhor-engine/tests/fixtures/synthetic/`, schema `1`, with
   cases carrying input prefix, language, layout, context, optional
   `roman_input`, and expected top-5. Parity is top-1 hit rate and top-5 hit
   rate over the corpus; latency (p50/p95/p99/max + correctness, release
   build) is reported by the bench task per `docs/performance.md`, never from
   a shared-desktop reference claim.

8. **Uniform error contract.** One `#[non_exhaustive]` `thiserror` enum
   `EngineError` with static `Display` text plus `kind()` and `code()`. Only
   `kind()`/`code()` reach logs; callers never log `Debug` or walk
   `source()`. `suggest` itself never constructs an error.

9. **Real-format importers deferred.** No `WTR1` trie parser and no
   `.nmsLayout`/`.nmsEnco` deserializer in 0.4. The follow-up requires
   test-validated parsers, synthetic-only committed fixtures, and an ADR or
   plan update before any real bytes are read.

## Consequences

Positive:

- The 0.4 implementer has a fixed signature, stage order, budget shape, and
  corpus schema, so no new shared types are invented mid-sprint.
- The bench task (Task 3) can define `--candidates` mode against a frozen
  corpus schema and parity definition.
- The `< 3 ms p99` budget and the privacy rules are enforced structurally
  (caps, no I/O, infallible query) rather than by review vigilance.

Negative:

- Composition flexibility is deliberately frozen; layout-aware ranking,
  trigram context, or a second transliteration source each need a new ADR.
- `layout` staying metadata-only until the Smriti pair-key question closes
  limits layout-specific tuning in 0.4.
- Real legacy dictionaries remain unusable until the deferred importer lands,
  so parity claims stay synthetic-only.

## Alternatives considered

- **Daemon composes the stages inline (no new crate):** rejected; composition
  logic would live in lead-owned daemon code with no contract, no unit owner,
  and no isolated bench target, repeating the 0.3 lesson that unowned seams
  drift.
- **Engine returns `Result<Vec<Suggestion>, EngineError>`:** rejected; the
  typing path must degrade to fewer candidates, never propagate an error to
  the input worker. Fallibility is confined to construction.
- **Engine normalizes per stage instead of once:** rejected; per-stage
  normalization invites key skew between dictionary and pair lookups. One
  owner, one call, idempotent downstream.
- **Ship the real `WTR1`/`.nmsLayout` importer in 0.4:** rejected; formats are
  custom and binary, redistribution rights are unconfirmed, and AGENTS.md rule
  5 forbids guessing at a legacy binary format without a tested parser.
  Synthetic fixtures plus a deferred, test-gated importer is the safe order.
- **One global budget for both languages:** rejected; Bangla transliteration
  fans out to far more candidates than English prefix completion, so a shared
  cap starves one language or over-admits the other.

## Compliance

Task 2 must satisfy design doc §7, run `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo test --workspace`, and report pass/fail with evidence. Any change to a
public signature in this ADR requires a new or superseding ADR and a design-doc
update in the same change. No `okkhor-core` type change ships under this ADR.

## References

- [SDD plan: 0.4 candidate engine](../superpowers/plans/0.4-candidate-engine.md)
- [0.4 engine contracts](../design/0.4-engine-contracts.md)
- [ADR 0010](0010-core-contracts.md) / [0.3 core contracts](../design/0.3-core-contracts.md)
- [ADR 0001](0001-rust-workspace-and-process-boundaries.md)
- [ADR 0005](0005-legacy-reuse-via-compatibility-layer.md)
- [Architecture](../architecture.md), [Roadmap](../roadmap.md),
  [Learning](../learning.md), [Performance](../performance.md),
  [Security and privacy](../security-privacy.md)
