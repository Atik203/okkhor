# ADR 0019: Ranker injection and bounded hybrid work in the live engine

- Status: **Proposed** (owner decision D2, 2026-11-24)
- Date: 2026-11-24
- Related: ADR 0011 (candidate engine), ADR 0016 (live suggestion stages),
  ADR 0017 (feature runtime and blend), epic #98, #101

## Context

The live suggestion engine currently owns the final ranking call:

```text
SuggestionEngine::suggest(&SuggestionRequest, &Smriti)
    -> stage candidates
    -> Smriti::rank(...)
    -> truncate to limit
```

`Smriti::rank` scores, tie-breaks `(score desc, text asc)`, and truncates
(`crates/okkhor-learn/src/store.rs:530-558`). `SuggestionEngine` holds no
ranker and takes `&Smriti` directly
(`crates/okkhor-engine/src/suggest/mod.rs:129`).

ADR 0017 adds a `HybridRanker` that rescales model scores against the
deterministic baseline. It needs two things the current shape does not provide:

1. **A ranker injection point.** The engine must call a `Ranker` implementation
   chosen by the daemon, not a hard-coded `Smriti`.
2. **Baseline scores for the whole pool.** Centering requires every candidate's
   baseline score before truncation. `Smriti::rank` already truncates, and the
   only bulk-preserving API is a per-candidate `score()` that takes the read lock
   each call.

ADR 0011 forbids wall-clock reads, RNG, and thread timing on the query path. A
naive `< 1 ms` deadline implemented with `Instant` inside the engine would
violate that contract. The Tier 1 model is small enough to bound structurally
instead.

ADR 0011 also says a public signature or composition-order change needs a
superseding ADR. This is that ADR for the ranker injection.

## Decision

### 1. Inject a `Ranker` into the live engine

- Add an additive entry point:

  ```rust
  impl SuggestionEngine {
      pub fn suggest_with_ranker(
          &self,
          request: &SuggestionRequest,
          ranker: &dyn Ranker,
      ) -> Vec<Suggestion>;
  }
  ```

  Keep the existing `suggest(&request, &Smriti)` as a thin wrapper for the 0.4
  parity harness and existing tests.
- The daemon constructs either an `Arc<Smriti>` baseline or an
  `Arc<HybridRanker>` and passes `&dyn Ranker` at the call site.
- `CandidateEngine` (golden/replay/bench parity) keeps its `&Smriti` shape; its
  metrics remain the reference baseline.
- The engine crate does **not** depend on `okkhor-model`. `HybridRanker` lives
  in `okkhor-model` and is passed in by the daemon, so ADR 0011's dependency
  list stays intact.

### 2. Add a bulk baseline-score API

Add to `okkhor-learn`:

```rust
impl Smriti {
    /// Score every candidate under one read lock, preserving input order.
    pub fn score_pool(&self, request: &SuggestionRequest, candidates: &[Candidate]) -> Vec<f32>;
}
```

- One lock acquisition, bounded by the staged pool cap (64).
- `HybridRanker` uses these scores as `baseline(c)`, centres the model scores,
  applies the ADR 0017 blend, sorts with the same tie-break as `Smriti::rank`,
  and truncates.
- `score_pool` is additive; `Smriti::rank` behavior and tests are unchanged.

### 3. Bound the work, not the clock

- Tier 1 work is at most `64 candidates x 16 features` = 1,024 multiply-adds
  plus a small sort. It is deterministic and allocation-bounded.
- The engine and `okkhor-model` do **not** read a clock. The bound is
  structural:
  - pool > 64 -> return baseline order;
  - any non-finite baseline or model score -> return baseline order;
  - missing/invalid model -> return baseline order.
- The daemon measures the actual p99 around the call and records it in
  `docs/performance.md`. A measured regression is fixed by tuning the work
  envelope, not by adding a timer to the engine.

### 4. Scope

- The hybrid reranks Bangla candidates only. English completions keep their
  existing static path (`suggest_english`) in 0.7.
- The model never adds, removes, or generates candidates.
- The default remains `baseline` until the A/B report is accepted.

## Consequences

Positive:

- one injection point and one bulk-score API, both additive;
- ADR 0011's no-clock and dependency rules stay intact;
- centering gets the full pool without a second sort;
- the 0.4 parity harness remains the reference;
- baseline fallback is a simple early return.

Negative:

- `SuggestionEngine` gains a second public entry point;
- `Smriti` gains a small public API surface;
- the hybrid is Bangla-only in 0.7;
- the p99 gate depends on measurement outside the engine, not an in-engine
  timer.

## Alternatives considered

- **Hard-code `HybridRanker` in `okkhor-engine`:** rejected; adds a model
  dependency to the engine and violates ADR 0011's crate boundary.
- **Call `Smriti::rank` then rescore the top N:** rejected; centering needs the
  whole pool, and the truncated tail could have won after blending.
- **Use `Instant` inside the engine:** rejected; violates ADR 0011's no-clock
  query contract and makes tests non-deterministic.
- **Add a timer thread/watchdog around ranking:** rejected; a synchronous
  bounded calculation cannot be preempted safely, and a watchdog adds a thread
  and a lock to the query path.
- **A new ranking crate:** rejected; `okkhor-model` is the right owner and the
  engine only needs a `Ranker` reference.
