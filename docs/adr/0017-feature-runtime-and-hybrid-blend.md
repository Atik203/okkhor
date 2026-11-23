# ADR 0017: Feature runtime crate and hybrid reranker blend

- Status: **Proposed** (owner decision D1/D2, 2026-11-24)
- Date: 2026-11-24
- Related: ADR 0007 (hybrid learning-to-rank), ADR 0015 (offline replay and
  feature schema v1), ADR 0019 (ranker injection and bounded work), epic #98,
  #101
- Implementation note (2026-11-24): the `okkhor-features` crate extraction
  landed (`FeatureVectorV1` + `extract_features`, re-exported by
  `okkhor-train`); the hybrid blend, `okkhor-model`, and ranker injection are
  not implemented yet.

## Context

`FeatureVectorV1` and `extract_features` currently live in `okkhor-train`
(ADR 0015), which is an offline developer crate. It depends on
`okkhor-engine`, `okkhor-lang`, and `okkhor-transliterate` so the replay
harness can build dictionaries and candidates.

The 0.7 Tier 1 reranker is a **shipped** component. If `okkhor-model` depended
on `okkhor-train`, the offline trainer and its language/engine dependencies
would be linked into the app, contradicting the rule that `okkhor-train` is not
shipped. Duplicating the 16-field struct and extraction logic would create two
sources of truth for a schema that must stay byte-stable.

The hybrid blend also needs a pinned contract. A naive
`baseline + model_logit` addition can invert strong deterministic matches
because the two scores are not on the same scale, and it can let the model
dominate before there is any local evidence.

## Decision

### 1. New shipped crate `okkhor-features`

- `crates/okkhor-features` is `#![forbid(unsafe_code)]`, performs no I/O, and
  depends only on `okkhor-core` and `okkhor-learn`.
- It owns:
  - `FeatureVectorV1` (`SCHEMA_VERSION = 1`);
  - stable enum encodings (`candidate_source`, `language`, `layout`,
    `context_language`);
  - `extract_features(&SuggestionRequest, &Candidate, &LearningSnapshot)`.
- `okkhor-train` re-exports the same public names, so existing replay tests and
  CLI code keep compiling.
- `okkhor-model` depends on `okkhor-features`, never on `okkhor-train`.
- A compatibility test pins the old and new extraction outputs byte-for-byte on
  the frozen replay set. Any schema change still requires a version bump and a
  model migration note.

### 2. Hybrid blend contract

Given a candidate pool `P` for one request:

```text
baseline(c)  = the deterministic Smriti/engine score for c
model(c)     = logistic logit from the Tier 1 model
centred(c)   = model(c) - mean(model(x) for x in P)

evidence     = ln(1 + user_word_count)
             + ln(1 + pair_count)
             + 2 * acceptance_rate

confidence   = 0.75 * (1 - exp(-evidence / 10))     # 0.0 .. 0.75

final(c)     = baseline(c) + confidence * centred(c) * MODEL_WEIGHT
```

Rules:

- `MODEL_WEIGHT` is a build-time constant `<= 1.0`; the initial value is `1.0`
  and may only be lowered with A/B evidence.
- `confidence = 0` reproduces the baseline order **exactly**; a golden test
  asserts this.
- Centering removes the model's global offset and prevents a uniformly high
  logit from moving every candidate.
- All inputs are finite; non-finite model output is an error and triggers
  fallback.
- Tier 1 uses a structural work bound (<= 64 candidates x 16 features), not an
  in-engine clock (ADR 0019). A pool over the cap, a missing/invalid model, or
  any non-finite score returns the baseline order. The daemon measures p99
  around the call and records it in `docs/performance.md`.
- The model never adds, removes, or generates candidates. It only reorders the
  existing pool.

### 3. Default and rollout

- Ship the bundled model but keep `--ranker baseline` as the default until the
  A/B report is accepted.
- A/B compares baseline and hybrid on the frozen replay set with top-1/top-5/
  MRR/p99, segmented by language and layout, and states the dataset provenance
  and its limits.
- If hybrid does not beat baseline, ship it disabled and record the negative
  result. Do not tune on the evaluation set.

## Consequences

Positive:

- one shipped source of truth for the feature schema;
- no offline trainer linked into the app;
- a blend that cannot silently override deterministic matches;
- an explicit, testable fallback contract;
- a clean path to a local adapter later.

Negative:

- one more small crate and a one-time move/refactor;
- the compatibility test must run on every schema change;
- the confidence formula is a heuristic until real feedback data exists;
- centering requires scoring the whole pool, not just the top candidate.

## Alternatives considered

- **Depend on `okkhor-train` from `okkhor-model`:** rejected; links offline
  tooling and heavy dependencies into the app.
- **Duplicate the struct and extraction:** rejected; two schemas drift.
- **Move to `okkhor-core`:** rejected; `extract_features` needs
  `LearningSnapshot` from `okkhor-learn`, which already depends on
  `okkhor-core` (cycle).
- **Uncentered additive blend:** rejected; scale mismatch and dominance.
- **Replace baseline with the model:** rejected; violates ADR 0007 and removes
  the explainable fallback.
