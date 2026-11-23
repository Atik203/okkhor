# Roadmap

## Status

**0.1 scaffold is complete.** It contains the workspace, core types, the Smriti
learning engine, snapshot persistence, the Windows boundary, the daemon shell,
tests, CI, and this documentation.

**0.2.0 is code-complete and tagged from the final release branch.** It includes
the Win32 low-level hook, Unicode `SendInput` injector with guarded clipboard
fallback and a native-control `WM_CHAR` fast path, single-instance guard,
capture pause state, global pause hotkey, caret-aware non-activating popup
scaffold, per-user autostart, redacted daemon logging, a minimal tray icon, and
a non-hook benchmark harness for ranking and both injection paths.

**0.3 and 0.4 are code-complete** (no version bump yet). They add the contract
set (`docs/design/0.3-core-contracts.md`, ADR 0010/0011), the `okkhor-lang`,
`okkhor-transliterate`, `okkhor-compat`, and `okkhor-engine` crates, the
synthetic golden corpus, engine parity benchmarking, the 0.3/0.4 QA and
performance records, and the G1–G4 gates.

**0.5–1.0 are planned**: `docs/superpowers/plans/0.5-ui.md`,
`0.6-model-data-ai.md`, `0.9-voice.md`, and `1.0-release.md`. The 0.5 UI
decision is ADR 0004 amended by ADR 0012 (Tauri UI process, named-pipe IPC,
daemon-owned settings, DPAPI-encrypted word store). Dataset rules live in
`docs/design/datasets.md`; heavy corpora are downloaded and processed only on
the cloud GPU, never locally and never committed.

Voice typing is the **last** feature phase (0.9). Keyboard input, language
data, the reranker, UI, and release mechanics come first.

Manual Windows QA execution remains in issue #8. The full suggestion-rendering
UI, hook-level end-to-end benchmark, and tray hardening are follow-ups.

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
| 5. Language data, model, AI text | 0.6 | 28–36 | datasets, Tier 1 reranker, AI text worker |
| 6. Hardening | 0.7 | 36–40 | IME/RDP/DPI, stress, accessibility, reliability |
| 7. Parity | 0.8 | 40–44 | real dictionaries/layouts, importers, migration, parity matrix |
| 8. Voice typing | 0.9 | 44–46 | opt-in dictation, last feature phase |
| 9. Release 1.0 | 1.0 | 46–50 | signed installer, updater, SBOM, beta hardening |

Weeks are relative to project start, not calendar dates. Add 20–30% buffer for
Windows compatibility and Bengali edge cases.

### Model workstream (parallel)

The learned reranker is a parallel workstream, not a replacement for the input,
language, or UI phases. See [model-roadmap.md](model-roadmap.md) and ADR 0007.

| Stage | Model work |
| --- | --- |
| 0.4 | candidate engine and deterministic ranking baseline; no learned reranker |
| 0.6 | feature schema v1; replay harness; Tier 1 logistic reranker; A/B |
| 1.0 | global prior from licensed corpora + local adapter; checksummed packages |
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
- non-hook benchmark harness for Smriti ranking and injector latency;
- hook-level keydown -> suggestion and keydown -> injection measurement.

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

## Phase 5 — Language data, model, and AI text (0.6)

Deliverables:

- dataset manifests, license review, and remote-only corpus handling;
- feature schema v1 and a frozen replay harness;
- Tier 1 deterministic logistic reranker with deadline and fallback;
- A/B report against the deterministic baseline;
- `AiProvider` abstraction, in-memory cache, circuit breaker, and worker;
- visible AI status without blocking typing;
- privacy controls for cloud features (text only; no microphone).

Exit criteria:

- model miss or AI outage never delays a keystroke;
- hybrid rerank p99 under 1 ms with proven fallback;
- no raw text or audio in logs;
- dataset licenses and checksums recorded;
- no claim beyond the evaluated dataset's provenance.

## Phase 6 — Hardening (0.7)

Deliverables:

- IME on/off, RDP, elevated windows, multi-monitor, and 100–200% DPI passes;
- 10,000-event hook stress and sleep/resume tests;
- crash isolation for UI and worker;
- accessibility and keyboard-only audit;
- reliability fixes from beta feedback.

Exit criteria:

- no lost or duplicated keys in stress runs;
- typing works with the UI closed, crashed, or not installed;
- elevated-window failure is explicit and never corrupts text.

## Phase 7 — Parity (0.8)

Deliverables:

- real dictionary and layout import (validated formats, no guessing);
- legacy custom-word and settings import where the format is known;
- feature parity matrix against the legacy app;
- migration report and rollback rehearsal.

Exit criteria:

- parity matrix has no silent gaps;
- import is reproducible and documented;
- unsupported legacy data is reported, not silently dropped.

## Phase 8 — Voice typing (0.9)

Voice is the last feature phase: keyboard input is the primary path.

Deliverables:

- opt-in microphone capture with explicit start/stop;
- `SpeechProvider` abstraction with Google Speech first;
- worker isolation, cancellation, 30 s cap;
- transcript panel with explicit insert only (no auto-injection);
- privacy controls and no audio/transcript persistence.

Exit criteria:

- speech outage never delays a keystroke;
- stale responses are discarded;
- cloud features are opt-in and documented;
- no audio or transcript in logs or crash reports.

## Phase 9 — Release 1.0

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
