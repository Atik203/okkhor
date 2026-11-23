# Smriti: personal learning

Smriti is Okkhor's on-device personalization engine. Its job is to make the
next word, completion, and transliteration choice better for this user without
putting the network, the UI, or a model in the typing path.

The legacy application already keeps some sentence history and recent words. The
new design goes further: it learns the user's English vocabulary, Bangla
vocabulary, Romanized-English -> Bangla choices, context, and corrections as
separate, inspectable signals.

## Goals

- improve top-1/top-5 suggestions for each user over time;
- learn English words and phrases the dictionary does not contain;
- learn that a given Romanized input maps to a particular Bangla word for this
  user;
- use context and corrections without overreacting to one event;
- keep storage bounded and encrypted;
- make behavior explainable and resettable;
- stay deterministic for replay and tests.

## Non-goals

- cloud training by default;
- storing raw keystrokes or full documents;
- learning from password fields;
- a large neural model in the first release;
- cross-device sync before the local model is stable.

## Signals

| Signal | Source | Use |
| --- | --- | --- |
| Word committed | input worker | frequency, recency, vocabulary |
| Suggestion shown | suggestion popup | denominator for acceptance |
| Suggestion accepted | input worker | positive preference |
| Suggestion ignored | input worker | weak negative, no harsh penalty |
| Correction / backspace | input worker | penalty for the replaced word, learn the replacement |
| Romanized input -> output | transliteration commit | user-specific transliteration pair |
| Previous word | input worker | bigram context |
| Language/layout switch | input worker | context boundary |
| App category | optional, privacy review | weak context, off by default |

Events are emitted only on commit or explicit UI interaction, never per raw
keystroke.

## Event model

```rust
WordCommitted {
    word, language, previous_word, timestamp_ms, source
}
SuggestionShown {
    word, language, timestamp_ms
}
SuggestionAccepted {
    word, language, previous_word, timestamp_ms
}
Correction {
    from, to, language, timestamp_ms
}
TransliterationCommitted {
    roman_input, output, language, previous_word, timestamp_ms
}
```

`source` is one of `Typed`, `AcceptedSuggestion`, `Correction`, `Paste`, or
`Transliteration`. Events are serializable so they can be replayed offline.

## Data model

| Table | Key | Values |
| --- | --- | --- |
| `words` | language + normalized word | count, last_used, shown, accepted, corrections |
| `pairs` | language + normalized roman input + output | count, last_used |
| `bigrams` | language + previous + current | count, last_used |

The current scaffold stores these in memory and persists an opaque snapshot as
JSON. The production adapter will use an embedded transactional store and may
add trigrams or per-app context after replay shows value.

## Ranking

The starting model is linear and explainable:

```text
score = w_base       * base_score
      + w_frequency  * ln(1 + user_frequency)
      + w_recency    * recency_decay
      + w_context    * bigram_score
      + w_acceptance * acceptance_rate
      - w_correction * correction_rate
      + w_phonetic   * phonetic_score
      + w_pair       * ln(1 + pair_frequency)
```

Defaults in the scaffold:

| Weight | Value |
| --- | --- |
| base | 1.00 |
| user frequency | 0.35 |
| recency | 0.25 |
| context | 0.40 |
| acceptance | 0.30 |
| correction | 0.50 |
| phonetic | 0.20 |
| pair frequency | 0.60 |

Rules:

- `user_frequency` is log-scaled so one typo does not dominate.
- `recency_decay` uses a 14-day half-life by default.
- `bigram_score = count / (count + 5)` keeps context useful but bounded.
- `acceptance_rate = accepted / shown`, with no bonus when never shown.
- `correction_rate` rises when a word is repeatedly replaced.
- `pair_frequency` is the user's learned Romanized -> output preference.
- Tune weights with replay data, not intuition. Record changes in an ADR.

### Feature contract v1 (0.6 Task 1, ADR 0015)

The stable 16-field feature vector lives in the shipped
`okkhor-features` crate (ADR 0017) and is re-exported by the offline
`okkhor-train` crate as `FeatureVectorV1` (`SCHEMA_VERSION = 1`), in
`docs/model-roadmap.md` field order. The contract:

- `extract_features(&SuggestionRequest, &Candidate, &LearningSnapshot)` is
  deterministic and performs no I/O; allocation is bounded to small
  normalized keys.
- Learned counters come from read-only snapshot accessors (`word_stat`,
  `pair_stat`, `bigram_stat`, `contains_word`); when the snapshot has no
  entry, the extractor falls back to the candidate's generator hints.
- Recency uses the 14-day half-life decay; rates and scores are clamped to
  finite ranges; empty or overlong inputs are errors, never silent zeros.
- Any field-order, type, or encoding change requires a schema bump and a
  model migration note. Full encoding table and replay format: see
  `docs/design/replay.md`.

### Hybrid blend contract (0.7, ADR 0017/0019)

The Tier 1 reranker only reorders the deterministic candidate pool:

```text
baseline(c) = Smriti score for c
model(c)    = logistic logit from the bundled model
centred(c)  = model(c) - mean(model(x) for x in pool)

evidence    = ln(1 + user_word_count)
            + ln(1 + pair_count)
            + 2 * acceptance_rate

confidence  = 0.75 * (1 - exp(-evidence / 10))    # per candidate, 0.0..0.75

final(c)    = baseline(c) + confidence * centred(c) * MODEL_WEIGHT
```

Rules:

- `confidence = 0` reproduces the baseline order exactly.
- `MODEL_WEIGHT <= 1.0`; the model cannot dominate a strong deterministic match.
- A pool larger than 64, a missing/invalid model, or any non-finite score
  returns the baseline order unchanged.
- The engine does not read a clock; the bound is structural and the daemon
  measures p99 around the call.
- English completions keep their static path in 0.7; the hybrid is Bangla-only.
- `okkhor daemon --ranker hybrid` selects the blend; the default is
  `--ranker baseline`. A model load failure logs `model_load` and keeps typing
  on the baseline. The 2026-11-24 synthetic-only A/B is an exact tie because the
  replay has no learning evidence; see `docs/performance.md`.

## Base + delta

The bundled dictionary is immutable and memory-mapped. User learning is a small
mutable delta:

```text
immutable base index  ← dictionaries, frequency lists, layouts
mutable delta index   ← user words, pairs, bigrams, recency
```

Candidate generation queries both. A background task merges and compacts the
delta, enforces caps, and writes a snapshot. Never rebuild the full trie on a
keystroke.

## English learning

Learn:

- words the user types that are absent from the bundled dictionary;
- word completion frequency and recency;
- next-word bigrams from committed sentences;
- capitalization patterns later, after the base model is stable.

Do not learn from:

- password fields;
- text pasted from the clipboard as if it were typed;
- target content returned by AI or a web page.

## English -> Bangla learning

This is the distinctive signal. When the user types `bhalo` and commits `ভালো`,
store one pair observation `(bn, "bhalo", "ভালো")`. Next time the same Romanized
prefix appears, that output gets a pair-frequency boost.

Also learn:

- which of several valid transliterations this user prefers;
- corrected spellings after a suggestion is accepted;
- Bangla word frequency and context;
- layout-specific behavior, if the user switches layouts often.

Do not collapse different layouts into one key. Include the layout or language
in the key when the same Romanized string can produce different results.

## Online loop

1. Input worker commits a word or a transliteration.
2. It emits a learning event into a bounded queue.
3. A background learner applies the event to memory immediately.
4. Persistence batches every few seconds or every N events.
5. `maintain` runs periodically to decay, cap, and compact.
6. Replay/evaluation can re-run the same events against a candidate model.

The typing thread never waits for steps 3–6.

## Negative feedback

- Suggestion shown but not selected: weak negative; do not punish aggressively.
- Accepted then corrected: stronger negative for the accepted candidate.
- Repeated manual typing of a word despite a suggestion: learn the typed form.
- A correction should also teach the replacement, not only punish the old word.

Avoid feedback loops: cap the user boost, decay stale counts, and keep a small
amount of exploration in positions 2–5 of the list.

## Bounded storage

Defaults:

| Store | Cap |
| --- | --- |
| words | 50,000 |
| pairs | 100,000 |
| bigrams | 200,000 |

Maintenance sorts by count and recency, removes the tail, and decays old counts.
The production store must also enforce a byte budget, not just row counts.

## Cold start

- Bundle frequency lists from the dictionaries.
- Import legacy custom words and shortcuts.
- Import `SentenceHistory.bin` only if its format is understood and tested;
  otherwise start fresh and say so in the migration report.
- Never deserialize `settings.pkl`; use a one-time exporter.

## Evaluation

Offline replay is the primary gate:

- hold out the last N days of events;
- compare personalized vs non-personalized ranking;
- measure top-1, top-5, MRR, and keystroke savings;
- measure p50/p95/p99 ranking latency;
- segment by language, layout, and English vs transliteration;
- inspect false positives and privacy-sensitive examples manually.

Online:

- feature-flag ranking changes;
- beta channel first;
- guardrail metrics: latency, crash rate, clear-data rate, pause usage;
- no text in telemetry, only aggregate counters.

## Privacy

- Local-only by default.
- Encrypt the snapshot with Windows DPAPI or an equivalent OS-protected key.
- No raw keystrokes, no full documents, no clipboard history.
- Do not log words or sentences.
- Provide pause, export, and delete.
- Do not learn from password fields where UI Automation can identify them; when
  uncertain, pause is the user's control.
- If cloud sync is ever added, make it opt-in and end-to-end encrypted, and keep
  it out of the typing path.

## Failure modes

| Failure | Behavior |
| --- | --- |
| snapshot corrupt | start empty, retain the corrupt file for diagnosis, notify |
| schema older | migrate in a copy; never mutate the original in place |
| schema newer | refuse to write; ask the user to update |
| storage full | stop learning writes, keep typing, surface a quiet warning |
| clock skew | clamp recency; do not let future timestamps dominate |
| user clears data | clear memory and disk, then confirm |

## Open questions

- Is per-app context worth the privacy cost?
- Do trigrams improve top-1 enough to justify the storage?
- Should transliteration pairs include the active layout in the key?
  Decision (2026-11-23, gate G3, 0.4): No — `SuggestionRequest.layout` stays
  metadata-only; the pair key stays `(language, normalized roman input, output)`.
  Rationale: no per-layout data yet; revisit with real usage data.
- Is a small logistic reranker better than hand-tuned weights after 30 days of
  replay data?
- Should the UI explain “why this suggestion?” in a diagnostics panel?

The long-term plan for learned ranking is in
[model-roadmap.md](model-roadmap.md) and ADR 0007.
