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

**0.5-1.0 are planned**: `docs/superpowers/plans/0.5-ui.md`,
`0.6-suggestion-quality.md`, `0.9-voice.md`, and `1.0-release.md`. The 0.5 UI
decision is ADR 0004 amended by ADR 0012 (Tauri UI process, named-pipe IPC,
daemon-owned settings, DPAPI-encrypted word store). Dataset rules live in
`docs/design/datasets.md`; heavy corpora are downloaded and processed only on
the cloud GPU, never locally and never committed.

0.6 is the suggestion-quality slice: the full local dictionary, eight rows,
English completions, instant quick-bar state, and a Settings pass (ADR-0016).
The dataset manifests, Tier 1 reranker, and AI worker from
`0.6-model-data-ai.md` move to 0.7/0.8; that plan is retained for the later
work.

0.9 is the **layout engine** phase: Avro, Phonetic and Khipro as alternate
phonetic tables, plus National/Jatiya, Probhat and Bijoy as real fixed
physical-key layouts (ADR 0021). Voice typing and local next-word learning move
to 0.9.x. Keyboard input, language data, the reranker, UI, and release
mechanics come first.

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
| 5. Suggestion quality | 0.6 | 28–36 | `OKS1` dictionary, English completions, eight rows, instant state |
| 6. Hardening, model, AI | 0.7 | 36–44 | IME/RDP/DPI, stress, Tier 1 reranker, opt-in AI worker |
| 7. Parity | 0.8 | 44–48 | real dictionaries/layouts, importers, migration, parity matrix |
| 8. Fixed layouts | 0.9 | 48–50 | National, Probhat, Bijoy; alternate roman tables |
| 9. Local learning and next-word | 0.9.x | 50–53 | accepted/corrected events, bigram strip, replay |
| 10. Voice typing | later | — | after owner daily-use QA; not a 1.0 blocker |
| 11. Release 1.0 | 1.0 | 53–57 | signed installer, updater, SBOM, beta hardening |

Weeks are relative to project start, not calendar dates. Add 20–30% buffer for
Windows compatibility and Bengali edge cases.

### Model workstream (parallel)

The learned reranker is a parallel workstream, not a replacement for the input,
language, or UI phases. See [model-roadmap.md](model-roadmap.md) and ADR 0007.

| Stage | Model work |
| --- | --- |
| 0.4 | candidate engine and deterministic ranking baseline; no learned reranker |
| 0.6 | feature schema v1; replay harness; suggestion dictionary (A/B deferred) |
| 0.7 | dataset manifests; deterministic trainer; Tier 1 logistic reranker; A/B |
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
- Avro, National, Probhat, Bijoy, Phonetic, and other supported layouts;
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

## Phase 5 - Suggestion quality (0.6)

0.6 ships the local dictionary and the surrounding UX. The dataset manifests,
Tier 1 reranker, and AI worker that were originally scheduled here move to 0.7
together with hardening (see `0.6-model-data-ai.md` and
`superpowers/plans/0.7-hardening-and-model.md`); ADR-0016 is the binding decision
for the suggestion index, English completions, and instant state.

Deliverables:

- committed `OKS1` suggestion index built from the Avro database, autocorrect
  dictionary, curated starter list, and `wordfreq` popularity;
- bounded engine stages: curated, user, autocorrect, rule, dictionary exact,
  dictionary prefix, suffix expansion, Smriti rank;
- eight rows by default (max ten), caret-following popup;
- English prefix completions accepted with Tab/click, never auto-replaced;
- quick-bar state event (F9/tray/pause/encoding) with a 5 s fallback poll;
- caret resolution off the key path when measurements justify it;
- Settings sections, responsive at 960/1280/1600 px in light and dark.

Exit criteria:

- top-1 >= 80% and top-5 >= 95% on the 150-case golden corpus;
- suggestion p99 < 3 ms; popup/key-path budgets hold;
- corrupt or missing index falls back to rule output without a panic;
- English never replaces on Space/Enter; exact tokens pass Tab through;
- F9 updates the bar chip within 100 ms;
- owner QA signs off the real-key pass.

### Deferred from Phase 5 (now 0.7)

Feature schema v1 and the frozen replay harness landed in 0.6 (ADR 0015). The
remaining work moved to 0.7 by owner decision (2026-11-24):

- dataset manifests, license review, remote-only corpus handling;
- deterministic logistic trainer and model coefficients;
- Tier 1 deterministic logistic reranker with deadline and fallback;
- A/B report against the deterministic baseline;
- `AiProvider` abstraction, cache, circuit breaker, worker, and AI status;
- privacy controls for cloud text features.

## Phase 6 — Hardening, model, AI (0.7)

Plan of record: `superpowers/plans/0.7-hardening-and-model.md`.

Deliverables:

- 0.6.1 latest-wins caret thread and owner-QA fixes;
- dataset manifests, fail-closed verifier, and deterministic trainer;
- `okkhor-model` Tier 1 reranker with deadline, fallback, and A/B report;
- opt-in `okkhor-ai` worker, provider trait, cache, breaker, and UI panel;
- IME on/off, RDP, elevated windows, multi-monitor, and 100–200% DPI passes;
- 10,000-event hook stress and sleep/resume tests;
- crash isolation for UI and worker;
- accessibility and keyboard-only audit;
- reliability fixes from beta feedback.

Exit criteria:

- no lost or duplicated keys in stress runs;
- typing works with the UI closed, crashed, or not installed;
- elevated-window failure is explicit and never corrupts text;
- hybrid rerank p99 < 1 ms with a proven baseline fallback;
- AI outage, timeout, cancel, or worker kill never delays a keystroke.

## Phase 7 — Parity (0.8)

Deliverables:

- real dictionary and layout import (validated formats, no guessing);
- legacy custom-word and settings import where the format is known;
- feature parity matrix against the legacy app;
- migration report and rollback rehearsal;
- suggestion stability: coalesced popup painting, frozen per-word ranking, and
  a bounded prefix cache;
- raw Latin fallback for unknown phonetic words when the local list has no
  dictionary-backed match.

Exit criteria:

- parity matrix has no silent gaps;
- import is reproducible and documented;
- unsupported legacy data is reported, not silently dropped;
- typing a long word at speed produces no visible flicker, no one-character
  caret jump, and no reordering on backspace;
- an unknown ASCII token remains selectable without leaving Bangla mode.

0.8.0 ships the stability slice and the read-only import/parity report. Full
`WTR1`/`STRI` parsing and pickle-layout export remain open; every gap is named
in `docs/parity/0.8-matrix.md`, so the matrix has no silent gaps but the
migration is not complete. Do not call 0.8 a full-parity release.

## Phase 8 — Fixed layouts (0.9)

0.9 makes every advertised layout real. Avro, Phonetic and Khipro share one
phonetic engine with different compiled rule tables; National, Probhat and
Bijoy use a fixed physical-key engine with modifier and sequence support. The
hook swallows letters in fixed mode so IME composition cannot corrupt the
result. ADR 0021 and `docs/superpowers/specs/2026-11-24-fixed-layout-engine-design.md`
record the design.

Local learning and next-word suggestions are Phase 8b (0.9.x). Voice typing is
deferred until after the owner has used the keyboard for a couple of days and
the typing path is verified seamless in daily use — most users will use the
keyboard, not voice, so voice is not a 1.0 blocker. The voice deliverables and
exit criteria below remain the specification for that later slice.

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
