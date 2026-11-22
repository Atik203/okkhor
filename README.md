# Akkhor

Akkhor is a ground-up, Windows-first Bengali/English input engine and keyboard
utility. The goal is a fast, local-first typing experience: immediate
transliteration and suggestions, personalization that learns on the device, and
AI features that never sit in the keystroke path.

> Status: **0.1 scaffold**. The workspace, learning core, storage, and daemon
> shell exist. The native keyboard hook, text injection, UI, speech, and AI
> workers are roadmap items, not shipped features.

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
6. The legacy Kontho app is a behavioral oracle, not a runtime dependency.

## Repository layout

```text
apps/akkhor-daemon/      daemon/tray binary (scaffold)
crates/akkhor-core/      language, text, candidate, ranking primitives
crates/akkhor-learn/     Smriti personalization and ranking
crates/akkhor-storage/   snapshot persistence
crates/akkhor-win/       Windows input/injection boundary
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
cargo run --package akkhor-daemon -- self-check
```

Or:

```powershell
.\scripts\check.ps1
```

## Documentation

- [Architecture](docs/architecture.md)
- [Roadmap](docs/roadmap.md)
- [Personal learning (Smriti)](docs/learning.md)
- [Performance](docs/performance.md)
- [Security and privacy](docs/security-privacy.md)
- [Testing](docs/testing.md)
- [Legacy compatibility](docs/compatibility.md)
- [Release process](docs/release.md)
- [Glossary](docs/glossary.md)
- [Agent rules](AGENTS.md)

## Legacy application

The legacy Kontho build is kept read-only at
`E:\Nms_Kontho\Nms Kontho x64 8.002` for analysis and differential testing. It is
not part of this repository, and its binaries and assets are not redistributed
here. See `docs/compatibility.md`.

## License

License is **TBD**. Do not add or assume an open-source license until the owner
chooses one. Third-party asset and dependency licenses must be reviewed before
distribution.
