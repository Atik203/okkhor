# AGENTS.md — Okkhor engineering contract

Okkhor is a Windows-first Bengali/English input engine and keyboard utility.
This file is binding for every LLM agent and human contributor. Read it before
changing code, then read the documents it points to.

## Read first

1. `README.md`
2. `docs/architecture.md`
3. `docs/roadmap.md`
4. `docs/performance.md`
5. `docs/learning.md`
6. `docs/model-roadmap.md`
7. `docs/security-privacy.md`
8. The ADR that covers the area you are changing (`docs/adr/`)

If a request conflicts with these rules, stop and ask. Do not silently widen the
scope.

## Current state

- This repository is **0.10 in progress**. The live typing path is the Win32
  hook, the resident daemon, the Avro phonetic table, and the embedded `OKS1`
  v2 suggestion engine (135k aliases, 151k roman pairs, preferred rank,
  curated corrections, `wordfreq` popularity). English mode offers prefix completions (Tab/click,
  never auto-replace). The quick bar receives pushed mode/pause/encoding
  state. Learning is persisted to an encrypted `data/learning.bin`. Caret and
  password UI Automation run on a dedicated latest-wins thread.
- 0.5 shipped the Tauri UI, named-pipe IPC, daemon-owned settings,
  DPAPI-encrypted word store, on-screen keyboard, quick bar, tray, autostart,
  and the packaged `Okkhor.exe` + `bin\okkhor-engine.exe` layout. 0.6 added the
  full local dictionary, eight rows, English completions, and instant state.
  0.7 adds the caret thread, the Tier 1 reranker, the AI rewrite worker, and the
  QA/hardening slices. 0.8 adds the coalesced/in-place suggestion card, frozen
  per-word ranking with a prefix cache, sticky selection, the raw Latin
  fallback, the read-only legacy import report/parity matrix, the MSVC release
  build, and the code-signing hook. 0.8.1 adds first-character suggestions,
  Backspace-stable rows, a no-op popup fast path, and a blank-commit guard.
  0.9 adds the real layout engine (ADR 0021): Avro/Phonetic/Khipro as alternate
  roman tables and National/Probhat/Bijoy as fixed physical-key layouts. 0.10
  adds inline Bangla preedit (ADR 0022), index v2 (ADR 0023), ghost next-word
  hints, and real shown/accepted/correction learning. Global next-word models
  are trained on licensed 1B-token corpora and distilled for shipping; personal
  patterns stay local. Punctuation-commit choices and 1B model integration
  remain in progress.
- The **Tier 1 logistic reranker exists** in `okkhor-model` and is selectable
  with `okkhor daemon --ranker hybrid`; the **default is `--ranker baseline`**
  (deterministic Smriti) until an A/B on real or licensed data is accepted. On
  the frozen synthetic replay the hybrid is an exact baseline identity (no
  learning evidence) and does not claim a quality gain. A missing or invalid
  bundled model logs `model_load` and falls back to baseline; typing continues.
- The **opt-in AI worker exists**: `apps/okkhor-worker` owns the OpenAI client,
  the daemon proxies jobs, and Settings exposes an explicit rewrite action.
  Prompts/responses are memory-only; the worker is spawned only when AI is
  enabled and a key is present. Speech, the installer, updater, and code signing
  are **not implemented yet**. 0.7 is a personal/QA beta; 1.0 needs the installer
  and signature. Do not claim those exist.
- Real-key Windows QA (caret, English Tab/password, IME/RDP/elevated/DPI) is
  still the owner's manual pass; the hook intentionally ignores injected keys.
- The legacy application is a read-only reference installation supplied by the
  owner. It is not part of this repository; see `docs/compatibility.md`.
- The legacy application is a read-only reference installation supplied by the
  owner. It is not part of this repository; see `docs/compatibility.md`.
- **Never run the legacy executable or its installer.** Use static analysis or
  the exported asset corpus only.
- **Never modify, move, rename, or delete anything under the legacy folder.**

## Non-negotiable rules

### 1. Typing is sacred

The keydown-to-character path MUST NOT:

- perform network I/O
- perform disk I/O
- call an AI model
- wait on a UI thread
- take an unbounded lock
- allocate without a bound
- log user text
- depend on a garbage collector or interpreter

The hook callback should only timestamp and enqueue an event. All language work
happens on a dedicated worker. AI and speech are background features, never
gates on typing.

### 2. Local-first, AI off the hot path

- Local dictionary, transliteration, ranking, and learning always decide the
  quick suggestions.
- AI may improve an explicit action (proofread, rewrite, translate) or a slow
  idle suggestion. It must be cancellable and cached.
- Any learned model must have a hard inference deadline and fall back to the
  deterministic Smriti baseline. A model miss is not a typing stall.
- A cloud outage must not stop typing or local suggestions.
- Never send text to a remote service without an explicit, informed user action.

### 3. Privacy

A keyboard app sees everything. Treat that as a security boundary.

- Do not persist raw keystrokes.
- Do not log words, sentences, clipboard contents, or window titles.
- Learning data is local-only by default and encrypted at rest.
- Global model training requires explicit opt-in, aggregation, and differential
  privacy. Never upload raw text.
- Provide pause, export, and delete controls.
- Do not learn from password fields where the platform can identify them; when
  uncertain, the pause control is the user's escape hatch.
- Telemetry is opt-in, aggregate, and documented. No text in telemetry, ever.

### 4. Windows first, explicit boundaries

- Target Windows 11 x64 first.
- `unsafe` is allowed only in `okkhor-win` and explicitly reviewed FFI modules.
  Every `unsafe` block needs a `// SAFETY:` comment naming the invariant.
- `okkhor-core`, `okkhor-learn`, and `okkhor-storage` are `#![forbid(unsafe_code)]`.
- Do not use a convenience crate for the production keyboard hook. Use the
  `windows` crate directly so behavior is inspectable.
- Preserve focus. Suggestion windows must not activate.
- Handle per-monitor DPI, multi-monitor, IME on/off, and elevated-window failure
  explicitly.

### 5. Correctness before cleverness

- Bengali shaping, conjuncts, virama, ZWJ/ZWNJ, cursor movement, and word
  boundaries need tests.
- Build a golden corpus before replacing a legacy component.
- Differential-test new behavior against the legacy app while it is still the
  oracle.
- Do not guess at a legacy binary format. Write a parser with tests or keep the
  old component behind FFI.

### 6. Reuse assets, not architecture

- Reuse dictionaries, layouts, encodings, fonts, images, sounds, and metadata
  only through an importer, and only when the user has the rights to them.
- Reuse legacy native DLLs through a compatibility crate while migrating, then
  replace them one at a time with tests.
- Do not deserialize `settings.pkl`. Python pickle is untrusted input. Use a
  one-time exporter or import known JSON.
- Do not copy Python/PyQt structure into Rust. This is a ground-up design.

### 7. Performance is a gate

Every hot-path change needs a before/after measurement. Use the budgets in
`docs/performance.md`. A change that improves average latency but worsens p99 is
not automatically acceptable.

### 8. Dependencies and licensing

- Prefer the standard library and small, maintained crates.
- Every new dependency needs a reason in the PR and a license check.
- Avoid GPL/AGPL/non-commercial dependencies unless the owner explicitly
  approves a licensing change.
- Do not add a dependency for something the standard library or a small local
  module can do.
- Do not commit secrets, tokens, certificates, user data, or proprietary assets.

### 9. Scope control

- Implement the current roadmap milestone, not the whole roadmap.
- One vertical slice per change is better than many half-finished modules.
- Do not add a crate, process, protocol, or database without an ADR.
- Do not rename public types or crates without updating docs and consumers.
- Mark unfinished code as `scaffold` or `TODO` with an owner and a roadmap
  milestone. Never ship a placeholder that looks finished.

## Repository map

```text
apps/okkhor-bench/       non-hook ranking/suggestion/injection benchmarks
apps/okkhor-daemon/      resident daemon, tray/logging wiring, and demos
apps/okkhor-ui/          Tauri settings + quick bar + on-screen keyboard
apps/okkhor-worker/      isolated opt-in AI worker process
crates/okkhor-ai/        provider boundary, cache, breaker, single-flight, policy
crates/okkhor-core/      language, text, candidate, ranking primitives
crates/okkhor-features/  shipped v1 feature vector and deterministic extraction
crates/okkhor-ipc/       named-pipe protocol, settings, words
crates/okkhor-engine/    suggestion engine + embedded OKS1 index
crates/okkhor-train/     offline dataset/replay tooling (not shipped)
crates/okkhor-learn/     Smriti: personalization and ranking
crates/okkhor-model/     Tier 1 logistic reranker runtime + bounded hybrid blend
crates/okkhor-storage/   snapshot persistence and future redb adapter
crates/okkhor-win/       Win32 hook and Unicode injection boundary
crates/okkhor-lang/      0.3 layouts, encodings, dictionaries + fixture importer
crates/okkhor-transliterate/  0.3 rule-based transliteration engine
crates/okkhor-compat/    0.3 legacy-DLL compatibility boundary
crates/okkhor-engine/    0.4 candidate generation + golden-corpus parity
docs/                    architecture, roadmap, learning, performance, security
docs/adr/                architecture decisions
assets/                  import instructions; proprietary binaries are not committed
benchmarks/              harness notes and future corpora
scripts/                 developer checks and importers
```

Planned but not created yet: `okkhor-speech`, `okkhor-license`,
`okkhor-update`. Create them only when the roadmap reaches them.

## Build and test

### Local Windows

The current dev machine is bootstrapped with the user-level GNU toolchain and
MSYS2 mingw64. MSVC is the production toolchain and should be used before
shipping native Windows code.

```powershell
# One-time current shell setup if cargo is not on PATH
$env:PATH = "$HOME\.cargo\bin;C:\msys64\mingw64\bin;$env:PATH"

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run --package okkhor-daemon -- self-check
```

Or run the wrapper:

```powershell
.\scripts\check.ps1
```

### CI

CI runs on `windows-latest` with stable MSVC. The contract is:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
```

## University GPU (uiu-h200) — model training only

Scope: this resource is for this project's model-training work only. These
rules bind every agent and session. If an action is not explicitly allowed
here, ask the user for permission first.

- Access: SSH via the local alias `uiu-h200` only. Do not expose ports or
  create another connection method.
- GPU: H200 MIG `1g.18gb` slice, 18 GB VRAM maximum. Use only this slice;
  never request or use a larger profile.
- Driver / CUDA: NVIDIA driver 580.178.04, CUDA 13.5. Do not change,
  overwrite, reinstall, or upgrade either; do not replace system libraries.
- Do not change: GPU clocks, power limits, MIG settings, SSH daemon, host
  keys, account name, login method, or other users' `authorized_keys`.
- Storage: 200 GB maximum. Check `df -h` and `du -sh` before large writes;
  stay inside the project space; do not change mounts, quotas, or permissions;
  do not delete other users' files.
- Python: use a project-local venv or conda environment only. No system-wide
  packages and no `sudo` without administrator approval.
- Jobs: run long jobs under `tmux` or `screen`; stop idle GPU jobs. No mining,
  public services, port scanning, or unrelated workloads.
- Secrets: never commit a server address, hostname, username, account ID,
  password, SSH key, or host fingerprint. The only allowed identifiers are
  `uiu-h200`, `H200 1g.18gb`, driver `580.178.04`, and `CUDA 13.5`.
- Policy: follow the university's monitoring, acceptable-use, and
  data-handling policies. Training data must still satisfy the privacy rules
  above: no raw keystrokes, no user text, synthetic or licensed corpora only.

## Definition of done

A change is done only when:

- the smallest useful slice works;
- new behavior has unit tests or a golden test;
- hot-path changes have before/after numbers;
- `cargo fmt`, `clippy`, and tests pass;
- user-visible behavior is documented;
- architecture or policy changes have an ADR;
- no new `unsafe` lacks a safety comment;
- no new secret, user data, or proprietary asset is committed.

## Performance budgets

| Path | Budget |
| --- | --- |
| Hook callback | < 0.5 ms, no I/O |
| Local candidate generation | < 3 ms p99 |
| Personal rerank | < 2 ms p99 |
| Top-20 UI update | < 8 ms |
| Popup show | < 10 ms |
| AI suggestion | only after 250–300 ms idle |
| Disk persistence | batched, never per keystroke |

These are starting budgets. Change them only with measurements and an ADR.

## Documentation rules

- `docs/architecture.md` is the system map.
- `docs/roadmap.md` is the plan of record.
- `docs/learning.md` is the personalization contract.
- `docs/performance.md` defines measurement.
- `docs/security-privacy.md` defines the data boundary.
- ADRs record irreversible or cross-cutting decisions.
- Update the relevant document in the same change as the code.
- Do not create a second source of truth. Link instead of duplicating.

## Working with the legacy app

- Use static inspection, exported dictionaries, and the golden corpus.
- Do not execute the legacy binary or installer.
- Do not copy its Python bytecode into this repository.
- When comparing behavior, run the comparison harness in a disposable VM or
  with explicit owner approval.
- Record differences as compatibility issues, not as silent “improvements.”

## PR and merge policy

The owner's standing rule:

- every change goes through a pull request;
- every PR links an existing issue (`Issue: #N`);
- use `Refs #N` while work is partial; use `Closes #N` only when the issue's
  acceptance criteria are met;
- progress notes, decisions, test results, and follow-ups belong on the issue,
  not only in the PR;
- when CI is green and there is no blocking review, **squash-merge into `main`**;
- delete the remote and local branch;
- pull `main`, then verify the resulting `main` CI run;
- close the linked issue only after merge verification and only if its criteria
  are met;
- do not leave a green, reviewed PR open.

If a change is intentionally draft or needs owner review, say so in the PR and
keep it open.

## Agent behavior

- Ask before deleting files, changing the toolchain, adding a native dependency,
  changing licensing, or touching the owner-provided legacy installation.
- Prefer a small, reviewable diff over a large speculative refactor.
- Run the checks yourself; do not claim success without output.
- If a check cannot run, say exactly what is missing and what was verified.
- Leave the repository in a buildable state.
- When in doubt, follow `docs/roadmap.md` and the nearest ADR.
