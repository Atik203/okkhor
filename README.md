# Okkhor

Okkhor is a ground-up, Windows-first Bengali/English input engine and keyboard
utility. The goal is a fast, local-first typing experience: immediate
transliteration and suggestions, personalization that learns on the device, and
AI features that never sit in the keystroke path.

> Status: **0.1 scaffold + early 0.2 input layer**. The workspace, learning
> core, storage, Win32 hook, Unicode injector with guarded clipboard fallback,
> single-instance guard, capture pause state, pause hotkey, caret-aware
> non-activating popup scaffold, per-user autostart, redacted daemon logging,
> and a minimal tray icon exist. The Settings tray action, full suggestion
> rendering, UI, speech, and AI workers are roadmap items, not shipped features.

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
apps/okkhor-daemon/      daemon shell, tray/logging wiring, and demos
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

## Documentation

- [Architecture](docs/architecture.md)
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
