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

- This repository is the **0.1 scaffold plus the first 0.2 input slice**.
- `okkhor-win` now contains a real Win32 low-level keyboard hook, a Unicode
  `SendInput` injector with a guarded clipboard fallback, a per-user
  single-instance guard, a capture pause state, a global pause hotkey, and a
  caret-aware non-activating popup scaffold. Tray, full suggestion rendering,
  UI, speech, and AI workers are **not implemented yet**. Do not pretend they
  are.
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
apps/okkhor-daemon/      daemon shell + hook demo; tray pending
crates/okkhor-core/      language, text, candidate, ranking primitives
crates/okkhor-learn/     Smriti: personalization and ranking
crates/okkhor-storage/   snapshot persistence and future redb adapter
crates/okkhor-win/       Win32 hook and Unicode injection boundary
docs/                    architecture, roadmap, learning, performance, security
docs/adr/                architecture decisions
assets/                  import instructions; proprietary binaries are not committed
benchmarks/              harness notes and future corpora
scripts/                 developer checks and importers
```

Planned but not created yet: `okkhor-ui`, `okkhor-ipc`, `okkhor-speech`,
`okkhor-ai`, `okkhor-license`, `okkhor-update`. Create them only when the
roadmap reaches them.

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
- when CI is green and there is no blocking review, **squash-merge into `main`**;
- delete the remote and local branch;
- pull `main`, then verify the resulting `main` CI run;
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
