# ADR 0007: Hybrid global/local learning-to-rank

- Status: Accepted
- Date: 2026-11-23

## Context

Smriti v1 ranks local candidates with explicit counts, recency, acceptance,
correction, transliteration-pair, and bigram features. That is fast, explainable,
and private. Over time, a learned reranker can improve quality further,
especially for:

- English words and phrases missing from the bundled dictionary;
- Bangla word choice and spelling preferred by one user;
- Romanized-English -> Bangla transliteration preferences;
- next-word prediction in mixed English/Bangla text;
- cold start for new users.

A single large model is the wrong shape for a keyboard. It adds latency, size,
privacy risk, and training complexity, and it cannot learn a private idiolect
without local adaptation. Collecting raw text from all users is unacceptable.

## Decision

Adopt a **two-tier learning-to-rank system**:

```text
local candidate generation
        ↓
fixed feature schema
        ↓
global prior + local user adapter
        ↓
top-N suggestions, deadline-bound
```

Specific decisions:

1. **Candidate generation stays deterministic.** The dictionary/trie/phonetic
   engine remains the source of candidates. The model reranks; it does not
   replace the engine.
2. **Global prior + local adapter.** The global model is trained from
   license-compatible public corpora and, later, opt-in aggregate feedback. The
   local adapter is trained on-device from the user's committed words,
   accepted/ignored suggestions, corrections, and transliteration choices.
3. **Tiered inference.**
   - Tier 1: pure-Rust linear/logistic reranker, sub-millisecond.
   - Tier 2: optional small GBDT/MLP via ONNX, 1–3 ms, deadline-bound.
   - Tier 3: transformer only after idle, for next-word/AI assist; never on the
     keypress path.
   - Cloud AI only for explicit actions, never for quick suggestions.
4. **Deadline and fallback.** A model that misses its budget is skipped. Smriti
   v1 remains the fallback and the reference. Typing never waits.
5. **Signed, versioned model packages.** A manifest declares feature schema,
   model version, checksum, signature, size, and supported languages. Model
   swaps are atomic and rollback-capable.
6. **Cache keys include model version.** Scores are cached by prefix, context,
   language, layout, and model version; invalidation is explicit.
7. **Privacy default is local-only.** No raw keystrokes, text, or clipboard
   leave the device. Global learning requires explicit opt-in, aggregation,
   and differential privacy. Federated learning is a post-1.0 experiment.
8. **Evaluation is replay-first.** Changes ship only with offline top-1/top-5,
   MRR, keystroke-savings, and p99 results, segmented by language and layout.

## Consequences

Positive:

- better cold start than a purely local model;
- personalization without uploading text;
- strict latency isolation and fallback;
- a clear path from linear scoring to small learned models;
- model quality becomes measurable and rollbackable.

Negative:

- two models to train, version, and evaluate;
- a feature store and replay pipeline are required;
- global data collection needs legal, privacy, and security review;
- model packaging and signing add release complexity;
- a learned model can be less explainable than counts.

## Alternatives considered

- **One global model only:** ignores private idiolect and creates a central
  privacy target.
- **One local model only:** poor cold start and no shared Bangla/English prior.
- **LLM on every keystroke:** unacceptable latency, cost, privacy, and
  reliability.
- **Raw text collection from all users:** rejected; incompatible with a
  keyboard's trust boundary.
- **Federated learning from day one:** too complex and too early; start with
  local adaptation and aggregate public/opt-in data.

## Implementation notes

- Add `okkhor-model` when phase 0.6 starts; keep it separate from `okkhor-core`
  and `okkhor-win`.
- Add `okkhor-train` as an offline tool, not a shipped binary.
- Define a stable feature schema before training the first model.
- Keep the linear model pure Rust; use ONNX Runtime only for Tier 2.
- Use `arc-swap` for atomic model replacement and a bounded inference queue.
- Add an ADR before enabling federated learning or any raw-data collection.

See [model-roadmap.md](../model-roadmap.md) for the staged plan.
