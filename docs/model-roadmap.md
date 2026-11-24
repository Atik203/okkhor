# Model roadmap: hybrid global/local learning-to-rank

This document is the long-term plan for Okkhor's suggestion model. It is
subordinate to the hard rules in `AGENTS.md` and ADR 0007: **typing never waits
on a model, and raw user text never leaves the device by default.**

## Why a custom model

Smriti v1 is a transparent count/recency/context ranker. It is fast and private,
but it cannot generalize as well as a learned reranker for:

- English words and phrases missing from the bundled dictionary;
- Bangla spelling and word-choice preferences;
- Romanized-English -> Bangla transliteration variants;
- next-word prediction in mixed English/Bangla text;
- cold start for a new user.

A learned model improves **quality**, not raw lookup speed. The trie still finds
candidates; the model only reranks them.

## Model tiers

| Tier | Model | Runtime | Target latency | Use |
|---|---|---|---|---|
| 0 | dictionary/trie + base scores | Rust | < 3 ms | candidate generation |
| 1 | linear/logistic reranker | pure Rust | < 1 ms | always-on personal ranking |
| 2 | small GBDT/MLP | ONNX Runtime | 1–3 ms | optional, deadline-bound |
| 3 | small transformer | ONNX/candle | 10–100 ms | idle next-word/AI assist only |
| 4 | cloud LLM | HTTP | 300 ms+ | explicit user action only |

Tier 3 and 4 must never block the keypress path. If a tier misses its deadline,
the result is discarded and the next lower tier is used.

## Architecture

```text
candidate generator
  ├─ dictionary
  ├─ transliteration
  ├─ user vocabulary
  └─ context/recency
        ↓
feature extraction (schema v1)
        ↓
global prior (shipped, signed)
        +
local adapter (on-device, private)
        ↓
deadline-bound rerank
        ↓
top-N suggestions
```

### Global prior

Trained from:

- license-compatible public Bangla and English text;
- synthetic typing data generated from layouts and transliteration rules;
- later, opt-in aggregate statistics or model updates with differential privacy.

It provides cold-start quality, common n-grams, and general transliteration
preferences. It must not memorize private text.

### Local adapter

Trained on-device from:

- committed words and phrases;
- accepted, ignored, and corrected suggestions;
- Romanized input -> chosen Bangla output;
- previous-word context and language/layout switches;
- user custom words and shortcuts.

It learns the user's idiolect, named entities, spelling, and code-mixing style.
The raw data stays local.

### Blend

```text
final = global(features)
      + confidence(n) * local_adapter(features)
```

`confidence(n)` grows with the amount of local evidence, so a new user relies on
the global prior and a long-time user relies more on their adapter. Both remain
optional and explainable.

## Feature schema v1

Keep the schema small, stable, and cheap to compute. Every feature must be
available before inference without I/O.

| Feature | Type | Source |
|---|---|---|
| `base_score` | f32 | dictionary/candidate generator |
| `candidate_source` | enum | dictionary, transliteration, user, context |
| `language` | enum | en/bn |
| `layout` | enum/id | active layout |
| `prefix_len` | u16 | current buffer |
| `word_len` | u16 | candidate |
| `user_word_count` | u32 | Smriti |
| `pair_count` | u32 | Smriti transliteration pairs |
| `recency` | f32 | Smriti decay |
| `acceptance_rate` | f32 | Smriti shown/accepted |
| `correction_rate` | f32 | Smriti corrections |
| `bigram_score` | f32 | previous word -> candidate |
| `trigram_score` | f32 | optional, phase 1.x |
| `phonetic_score` | f32 | Bornomala |
| `is_user_vocab` | bool | user dictionary |
| `context_language` | enum | previous tokens |
| `script_mix` | f32 | English/Bangla ratio in context |

Rules:

- No feature may require a network call, disk read, or UI query.
- Schema changes require a version bump and a model migration note.
- Training and inference must use the same normalization and tokenization.
- Never include raw text in a feature vector sent to a global trainer.

Replay notes (0.6 Task 1, ADR 0015; amended by ADR 0017): the schema is
implemented as `FeatureVectorV1` (`SCHEMA_VERSION = 1`) in the shipped
`okkhor-features` crate and re-exported by the offline `okkhor-train` crate, in
the exact field order of the table above (the optional `trigram_score` is out of
v1). Categorical fields use stable codes (`candidate_source`:
dictionary 0, transliteration 1, user-history 2, recent 3, context 4, AI 5,
raw-input 6. Raw-input is an additive code for the always-selectable Latin
fallback (ADR 0020); the shipped synthetic model was trained without it and
the default baseline ranker does not depend on it.
`language`: English 0, Bangla 1; `layout`: none 0, avro 1, bijoy 2, national
3, probhat 4, phonetic 5, khipro 6, unibijoy 7, other 8; `context_language`:
none 0, English 1, Bangla 2, mixed 3). Extraction is
`extract_features(&SuggestionRequest, &Candidate, &LearningSnapshot)` with no
I/O. The frozen synthetic replay set and the engine/Smriti baselines are
documented in `docs/design/replay.md` with numbers in `docs/performance.md`;
all 0.6 results carry the **synthetic-only** caveat.

## Data strategy

### Default

No data leaves the device. Local learning only.

### Public corpora

Use license-compatible Bangla/English text for the global prior. Record the
source, license, preprocessing, and checksum in the dataset manifest.

### Synthetic typing data

Generate Romanized input and layout variants from public text and the supported
layouts. This gives cold-start pairs without collecting user keystrokes.

### Opt-in aggregate learning

Only with explicit consent and a visible benefit:

- aggregate feature statistics or model updates, never raw text;
- differential privacy with a documented budget;
- secure aggregation or a trusted aggregator;
- no passwords, financial text, or sensitive fields;
- easy opt-out and delete;
- publish a model card and data statement.

### Federated learning

A post-1.0 experiment, not a launch feature. It requires:

- secure aggregation;
- DP or robust aggregation against poisoning;
- update authentication;
- membership-inference and memorization testing;
- a clear threat model and legal review.

## Training pipeline

```text
encrypted local events
        ↓
replay + feature extraction
        ↓
train (offline; Python or Rust)
        ↓
holdout evaluation + bias segments
        ↓
export coefficients or ONNX
        ↓
sign + checksum + model manifest
        ↓
ship in beta, A/B, rollback
```

- Training code lives in `okkhor-train` (offline, not shipped).
- Start with logistic regression; compare against Smriti v1.
- Try a small GBDT/MLP only if Tier 1 plateaus.
- Export ONNX only for Tier 2+.
- Model artifacts are signed and versioned; never load an unsigned model.
- Keep a frozen replay set for regression tests.

## Model manifest

Illustrative shape:

```json
{
  "schema": 1,
  "name": "okkhor-reranker",
  "version": "1.0.0",
  "feature_schema": "v1",
  "languages": ["en", "bn"],
  "runtime": "onnx",
  "file": "reranker.onnx",
  "sha256": "<sha256>",
  "signature": "<ed25519>",
  "size_bytes": 0,
  "trained_at": "2026-11-23T00:00:00Z",
  "metrics": {
    "top1": 0.0,
    "top5": 0.0,
    "p99_ms": 0.0
  }
}
```

The 0.7 runtime verifies the SHA-256 checksum before loading; signing is a 1.0
feature. On failure, stay on the deterministic baseline and report a
non-blocking error.

## Rust integration

Status (0.7 model slice): `okkhor-features` and `okkhor-model` exist. The
bundled 16-feature logistic model loads from an embedded, checksum-verified
asset; `HybridRanker` implements the centred, evidence-scaled blend with a
structural 64-candidate bound. The daemon exposes `--ranker hybrid|baseline`
and **defaults to `baseline`**. On the frozen synthetic replay the hybrid is an
exact baseline identity (no learning evidence), so no quality gain is claimed.

Crates:

- `okkhor-features`: shipped, dependency-light `FeatureVectorV1` and
  `extract_features` (ADR 0017); re-exported by `okkhor-train`.
- `okkhor-model`: checksum-verified logistic inference, the `HybridRanker`
  blend, bounded work and baseline fallback (ADR 0017/0019).
- `okkhor-learn`: Smriti baseline, `score_pool`, and later the local adapter.
- `okkhor-train`: offline training and replay (not shipped; its
  `okkhor-replay --hybrid --model` runs the A/B).
- `okkhor-core`: `Ranker`/`Candidate`/`SuggestionRequest` traits and types.

Runtime shape:

```rust
struct HybridRanker {
    baseline: Arc<Smriti>,
    model: LogisticModel, // bundled, checksum-verified v1
    model_weight: f32,    // <= 1.0
    // local: Option<LocalAdapter>, // 1.0
}
```

Rules:

- `rank()` never blocks; a model miss or invalid score returns the baseline
  result.
- No clock read inside the engine: Tier 1 uses a structural work bound
  (ADR 0019), and the daemon measures p99 around the call.
- The model only reorders the existing pool; it never adds or removes
  candidates.
- 0.7 loads the bundled model once at startup behind an `Arc`; atomic
  hot-swapping and signed packages are 1.0 work.
- Cache key includes prefix, context, language, layout, feature schema, and
  model version.
- Inference memory is preallocated; no unbounded allocation per keystroke.
- A model update is staged, validated, and rolled back on failure.

## Evaluation

Offline:

- top-1, top-5, MRR;
- keystroke savings;
- acceptance rate;
- p50/p95/p99 inference latency;
- model size and cold-start time;
- per-language, per-layout, and dialect/code-mixing segments;
- privacy leakage and memorization checks.

Online:

- feature flag and beta channel;
- guardrails: latency, crash rate, delete/pause usage;
- no text in telemetry;
- rollback if top-1 or p99 regresses.

## Staged roadmap

| Stage | Work |
|---|---|
| 0.4 | Smriti v1 counts/linear; stable feature schema; replay harness |
| 0.6 | suggestion dictionary and English completions; feature schema and replay landed (reranker deferred) |
| 0.7 | dataset manifests/trainer; Tier 1 local logistic reranker implemented behind `--ranker hybrid`, default `baseline`; synthetic-only A/B |
| 1.0 | Global prior from public corpora; local adapter; signed model packages |
| 1.x | Optional Tier 2 GBDT/MLP for next-word; cache and deadline tuning |
| 2.0 | Federated/DP pilot; Tier 3 transformer for idle suggestions |
| Never | Cloud LLM on every keystroke; raw text upload; one global model replacing local personalization |

## Risks

| Risk | Mitigation |
|---|---|
| model latency | deadline, fallback, Tier 1 first |
| model size | small feature schema; quantize/prune; no transformer on hot path |
| privacy | local-only default; DP; no raw text; delete/export |
| bias | segment metrics; public data audit; local adaptation |
| poisoning | signed updates; anomaly checks; robust aggregation |
| overfitting | holdout replay; decay; caps; exploration limited |
| complexity | one model tier at a time; ADR before a new runtime |

## What not to build

- a chatbot inside the IME;
- an LLM call per keystroke;
- a global model trained on raw user text;
- a 100 MB transformer in the input path;
- a custom crypto scheme for model or license signing;
- federated learning before the local model is excellent.
