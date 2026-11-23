# Okkhor

Okkhor is a ground-up, Windows-first Bengali/English input engine and keyboard
utility. The goal is a fast, local-first typing experience: immediate
transliteration and suggestions, personalization that learns on the device, and
AI features that never sit in the keystroke path.

> Status: **0.2.0 code-complete**. The workspace, learning core, storage, Win32
> hook, Unicode injector with guarded clipboard fallback and native-control
> `WM_CHAR` fast path, single-instance guard, capture pause state, pause hotkey,
> caret-aware non-activating popup scaffold, per-user autostart, redacted daemon
> logging, a minimal tray icon, and a non-hook benchmark harness exist. Manual
> Windows QA is pending in issue #8; full suggestion rendering, UI, speech, and
> AI workers are later milestones.

## Why Rust

- low, predictable input latency without an interpreter or GC
- a small, signed Windows binary instead of a bundled Python runtime
- explicit Windows API boundaries for hooks, injection, caret, and focus
- safe concurrency for the suggestion, learning, and AI workers
- a clean base for long-term maintenance

## Product principles

1. Typing never waits on disk, network, UI, or AI.
2. Local dictionaries and local learning decide quick suggestions.
3. AI is a background enhancement with cancellation and caching.
4. Personal learning is local, encrypted, visible, and deletable.
5. Bengali correctness is a first-class requirement, not an afterthought.
6. The legacy application is a behavioral oracle, not a runtime dependency.

## Repository layout

```text
apps/okkhor-bench/       non-hook ranking and injection benchmark harness
apps/okkhor-daemon/      resident daemon, tray/logging wiring, and demos
crates/okkhor-core/      language, text, candidate, ranking primitives
crates/okkhor-learn/     Smriti personalization and ranking
crates/okkhor-storage/   snapshot persistence
crates/okkhor-win/       Win32 hook and Unicode injection boundary
docs/                    architecture, roadmap, learning, performance, security
docs/adr/                architecture decisions
assets/                  asset import instructions; proprietary files excluded
benchmarks/              benchmark harness notes and future corpora
scripts/                 developer checks
```

## Quick start

Prerequisites: Rust stable (1.85+), Cargo, and a Windows linker. The current
development machine uses a user-level GNU toolchain with MSYS2 mingw64; CI and
production use MSVC.

```powershell
$env:PATH = "$HOME\.cargo\bin;C:\msys64\mingw64\bin;$env:PATH"

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run --package okkhor-daemon -- self-check
```

Or:

```powershell
.\scripts\check.ps1
```

## What works now (0.5 input pipeline)

The daemon (`okkhor daemon`) now types Bangla end to end on Windows:

- Type Banglish letters (`bhalo`) — a non-activating popup near the caret shows
  the Bangla candidate from the real embedded Avro phonetic table, refreshed
  per keystroke (up to 5 numbered candidates; the deterministic table yields
  one). Lowercase `o` after a consonant is the inherent vowel (`bhalo` →
  ভাল); capital `O` gives the ো-kar (`bhalO` → ভালো).
- `Space` erases the roman buffer and commits the selected candidate plus a
  space; `Enter` commits without the space. The highlight defaults to the first
  candidate; `1`–`5` commit a numbered candidate, arrow keys cycle with wrap,
  `Backspace` edits, `Esc`/`Tab` cancel and leave typed text untouched.
- The pause hotkey (`Ctrl+Alt+Shift+P`) and the tray toggle cancel any
  composition and hide the popup. While paused, everything passes through, so
  **English typing requires pausing capture**.
- Injected keystrokes (`LLKHF_INJECTED`) are never re-captured, so our own
  erase/inject `SendInput` cannot feed the loop. Nothing is swallowed: the hook
  keeps pass-through for every key.
- Safety: a commit (erase + Bangla text + optional space) goes out as one
  `SendInput` batch. If the batch fails with zero delivered, nothing is erased
  or injected; a nonzero partial batch can still erase a roman prefix without
  replacement (inherent to Win32 — retry means retyping the word). If the
  foreground window changed since the buffer started, nothing is sent at all.
  Failures log an allowlisted code only, never text. See
  `docs/input-pipeline.md` for the state diagram and the full failure table.

Known limitations: `Enter` while composing commits instead of newline;
`Backspace` at an empty buffer passes through (never erases beyond the
buffer); Ctrl/Alt/Win chords cancel the composition; candidates come from the
real Avro table only, so dictionary-based corrections (আপনি, ধন্যবাদ) come
later; learning commits are queued to a background `Smriti` with no rerank yet.

## Documentation

- [Architecture](docs/architecture.md)
- [Input pipeline (composition MVP)](docs/input-pipeline.md)
- [Roadmap](docs/roadmap.md)
- [Personal learning (Smriti)](docs/learning.md)
- [Model roadmap](docs/model-roadmap.md)
- [Performance](docs/performance.md)
- [Security and privacy](docs/security-privacy.md)
- [Testing](docs/testing.md)
- [Legacy compatibility](docs/compatibility.md)
- [Release process](docs/release.md)
- [Glossary](docs/glossary.md)
- [Agent rules](AGENTS.md)

## Legacy application

The legacy application is kept read-only for analysis and differential testing.
It is not part of this repository, and its binaries and assets are not
redistributed here. See `docs/compatibility.md`.

## License

License is **TBD**. Do not add or assume an open-source license until the owner
chooses one. Third-party asset and dependency licenses must be reviewed before
distribution.
