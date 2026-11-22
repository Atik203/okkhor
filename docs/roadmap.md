# Roadmap

## Status

**0.1 scaffold is complete.** It contains the workspace, core types, the Smriti
learning engine, snapshot persistence, the Windows boundary, the daemon shell,
tests, CI, and this documentation.

**0.2 is in progress.** The Win32 low-level hook and Unicode `SendInput`
injector are implemented and building with MSVC; daemon lifecycle, tray,
clipboard fallback, focus/caret, benchmark, and the integration matrix remain.

## Planning assumptions

- Windows 11 x64 first; one codebase, no cross-platform promises before 1.0.
- A small team of 2–3 engineers. A solo implementation will take longer.
- Reuse legacy assets and native DLLs first; replace them one at a time.
- Every phase ends with a measurable gate, not a demo.
- Release `0.x` every four weeks after 0.2; keep a beta channel.
- AI and speech are never on the typing path.

## Phase map

| Phase | Version | Weeks | Outcome |
| --- | --- | --- | --- |
| 0. Foundations | 0.1 | 1–3 | workspace, docs, learning core, storage, CI |
| 1. Input daemon | 0.2 | 3–8 | real hook, hotkeys, injection, tray, single instance |
| 2. Bornomala core | 0.3 | 8–14 | dictionaries, layouts, transliteration, candidates |
| 3. Smriti v1 | 0.4 | 14–20 | persistent learning, ranking, migration, replay |
| 4. UI | 0.5 | 20–28 | settings, on-screen keyboard, word manager, themes |
| 5. Speech and AI | 0.6 | 28–34 | provider workers, cache, cancellation, fallbacks |
| 6. Parity and 1.0 | 1.0 | 34–40 | signed installer, updater, migration, beta hardening |

Weeks are relative to project start, not calendar dates. Add 20–30% buffer for
Windows compatibility and Bengali edge cases.

### Model workstream (parallel)

The learned reranker is a parallel workstream, not a replacement for the input,
language, or UI phases. See [model-roadmap.md](model-roadmap.md) and ADR 0007.

| Stage | Model work |
| --- | --- |
| 0.4 | Smriti v1 baseline; stable feature schema; replay harness |
| 0.6 | Tier 1 local logistic reranker; pure Rust; A/B |
| 1.0 | Global prior (public corpora) + local adapter; signed packages |
| 1.x | Optional Tier 2 ONNX next-word; deadline tuning |
| 2.0 | Federated/DP pilot; Tier 3 transformer for idle only |

## Phase 0 — Foundations (done)

Deliverables:

- Rust workspace and crate boundaries;
- `okkhor-core` language, text, candidate, and ranking primitives;
- `okkhor-learn` Smriti v0 with frequency, recency, acceptance, correction,
  pair, and bigram signals;
- JSON snapshot storage;
- Windows boundary types;
- daemon `self-check`;
- CI, formatting, clippy, tests;
- AGENTS.md, architecture, roadmap, learning, security, testing, compatibility,
  release, glossary, and ADRs.

Exit criteria:

- `cargo fmt --check`, `clippy -D warnings`, and `cargo test --workspace` pass;
- daemon `self-check` demonstrates English, Bangla, transliteration, and
  persistence;
- no proprietary assets committed.

## Phase 1 — Input daemon (0.2)

Deliverables:

- `WH_KEYBOARD_LL` hook on a dedicated thread;
- bounded, lock-free event queue;
- Unicode `SendInput` with clipboard fallback;
- caret/focus tracking and non-activating popup placement;
- global hotkeys and a shortcut editor model;
- tray lifecycle, single instance, autostart;
- graceful pause and exit;
- logging with redaction;
- benchmark harness for keydown -> suggestion and keydown -> injection.

Exit criteria:

- p99 keydown -> injected character under 2 ms on the reference machine;
- no lost or duplicated keys in a 10,000-event stress test;
- typing still works when the UI is closed or crashed;
- elevated-window failure is explicit and does not corrupt text;
- no network or disk I/O in the hook path.

## Phase 2 — Bornomala core (0.3)

Deliverables:

- legacy dictionary/layout importer;
- Avro, National, Probhat, Bijoy, Banglish, and other supported layouts;
- Unicode normalization and Bengali grapheme/cursor rules;
- transliteration candidate generation;
- prefix, fuzzy, phonetic, and context candidate sources;
- top-N ranking with base scores;
- golden corpus and differential harness against the legacy application;
- optional FFI adapters for legacy `word_trie`, `translator`, `caret`, and
  `emoji_manager` DLLs.

Exit criteria:

- top-5 parity with the legacy app at least 95% on the approved corpus;
- local candidate generation p99 under 3 ms;
- cursor movement and deletion correct around conjuncts, virama, and ZWJ/ZWNJ;
- dictionary import is reproducible and documented.

## Phase 3 — Smriti v1 (0.4)

Deliverables:

- persistent user model with encrypted storage;
- English vocabulary, completion, and next-word learning;
- Romanized-English -> Bangla pair learning;
- context bigrams/trigrams and correction signals;
- base + delta index and scheduled compaction;
- import of legacy custom words and, if feasible, sentence history;
- offline replay and evaluation harness;
- pause, export, and clear controls;
- stable feature schema for the future learned reranker.

Exit criteria:

- measurable top-1 improvement over the non-personalized baseline on replay;
- no p99 ranking regression;
- learning data encrypted at rest and absent from logs;
- caps and decay keep storage bounded;
- migration report lists imported, skipped, and unsupported legacy data.

## Phase 4 — UI (0.5)

Deliverables:

- Tauri 2 or Slint application shell;
- native, non-activating suggestion popup;
- settings, language/layout selection, themes, and fonts;
- on-screen keyboard with drag/drop groups, recents, favorites, emoji search;
- word manager, custom words, shortcut editor;
- accessibility, keyboard navigation, and screen-reader labels;
- per-monitor DPI and multi-monitor behavior.

Exit criteria:

- no focus theft in any tested target application;
- UI crash does not stop typing;
- cold start under 1 second on the reference machine;
- 100–200% DPI and multi-monitor checks pass;
- feature parity list is explicit about remaining gaps.

## Phase 5 — Speech and AI (0.6)

Deliverables:

- audio capture for microphone and system loopback;
- `SpeechProvider` abstraction with Google Speech first;
- `AiProvider` abstraction with Gemini first;
- debounce, cancellation, single-flight, cache, circuit breaker;
- visible AI/speech status without blocking typing;
- local fallback path and offline behavior;
- privacy controls for cloud features.

Exit criteria:

- AI/speech outage never delays a keystroke;
- stale responses are discarded;
- cache hit/miss and provider latency are measurable;
- cloud features are opt-in and documented;
- no text in logs or crash reports.

## Phase 6 — Parity and 1.0

Deliverables:

- feature parity review against the legacy app;
- signed installer and portable ZIP;
- signed update manifest, delta updates, rollback;
- settings and learning migration;
- crash reporting, opt-in and redacted;
- SBOM and third-party license inventory;
- support and recovery documentation.

Exit criteria:

- crash-free sessions above 99.5% in beta;
- signed binaries and verified updates on a clean Windows 11 VM;
- rollback tested from the previous release;
- no known P0/P1 privacy or security issues;
- performance gates met on the reference machine.

## Continuous workstreams

| Workstream | Ongoing work |
| --- | --- |
| Input | hook robustness, focus, IME, elevated apps, RDP, touch keyboard |
| Language | Bengali correctness, layouts, dictionaries, transliteration quality |
| Learning | replay metrics, decay, caps, ranking weights, privacy |
| UI | accessibility, themes, on-screen keyboard, performance |
| Platform | signing, installer, updater, crash recovery, diagnostics |
| Quality | golden corpus, fuzzing, differential tests, benchmarks |

## Release gates

A release ships only if:

- format, clippy, and tests pass;
- hot-path p99 is within budget or the regression is justified and documented;
- no new privacy or security issue is open;
- migration from the previous version is tested;
- docs and changelog are updated;
- installer and portable artifacts are signed.

## Risks

| Risk | Mitigation |
| --- | --- |
| Bengali shaping/cursor bugs | golden corpus, property tests, differential testing |
| Windows focus/IME edge cases | matrix testing, explicit failure paths, pause control |
| AV/SmartScreen false positives | code signing, stable publisher, Microsoft submission |
| UI scope | ship the input path first; UI behind a separate process |
| learning quality | replay evaluation, decay, caps, no cloud dependency |
| native DLL reuse | compatibility crate + per-DLL replacement plan |
| scope creep | one phase at a time; ADR for new crates/processes |

## What not to do

- Do not start the UI before the input daemon meets its latency gate.
- Do not put AI or speech on the keydown path.
- Do not rewrite the five native DLLs before measuring the Python overhead.
- Do not copy Python/PyQt structure into Rust.
- Do not deserialize `settings.pkl`.
- Do not promise macOS/Linux before 1.0.
