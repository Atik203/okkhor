# Okkhor

Okkhor is a ground-up, Windows-first Bengali/English input engine and keyboard
utility. The goal is a fast, local-first typing experience: immediate
transliteration and suggestions, personalization that learns on the device, and
AI features that never sit in the keystroke path.

> Status: **0.9.1 code-complete**. The workspace has the full local dictionary,
> English completions, a caret-following suggestion popup, Smriti
> personalization, the Tier 1 reranker (default-off), an opt-in isolated AI
> rewrite worker, the 0.8 popup-stability/raw-fallback work, and the 0.9 layout
> engine: Avro/Phonetic/Khipro as alternate roman tables plus National,
> Probhat and Bijoy as real fixed physical-key layouts (ADR 0021). The portable
> Windows build uses MSVC and has a code-signing hook. The installer, updater,
> code signing certificate, speech, and inline preedit remain later milestones;
> real-key QA is the owner's pass in `docs/qa/0.9-checklist.md`.

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

## What works now (0.6 input pipeline)

The daemon (`okkhor daemon`) now types Bangla end to end on Windows:

- Type phonetic English letters (`bhalo`) - a non-activating popup near the caret shows
  the Bangla candidate from the real embedded Avro phonetic table, refreshed
  per keystroke (up to 5 numbered candidates; the deterministic table yields
  one). Lowercase `o` after a consonant is the inherent vowel (`bhalo` →
  ভাল); capital `O` gives the ো-kar (`bhalO` → ভালো).
- `Space` erases the roman buffer and commits the selected candidate plus a
  space; `Enter` commits without the space. The highlight defaults to the first
  candidate; `1`–`5` commit a numbered candidate, arrow keys cycle with wrap,
  `Backspace` edits, `Esc`/`Tab` cancel and leave typed text untouched.
- **Bangla/English mode toggle**: `F9` flips between Bangla composition and
  English pass-through (falls back to `Ctrl+Space` if `F9` is already claimed,
  reported once at startup); left-clicking the tray icon or picking the
  `Switch to …` tray menu item does the same. A toggle clears the buffer,
  hides the popup, updates the tray label/tooltip (`Okkhor - Bangla/English`),
  and shows a non-activating toast (`বাংলা` / `English`) for ~800 ms without
  stealing focus. In English mode keys pass through untouched — pause is now a
  separate capture switch. Start in English with `okkhor daemon --mode english`
  (default `--mode bangla`); the flag selects the initial mode only — last-mode
  persistence arrives with the settings-store wiring and is not claimed yet.
  A double-click is two left-clicks (net no mode change) and never pauses.
- The pause hotkey (`Ctrl+Alt+Shift+P`) and the tray pause item cancel any
  composition and hide the popup. While paused, everything passes through.
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

## Settings, UI, and IPC (0.5 Task 3)

The resident daemon owns settings, user words, and a local control pipe; a
separate settings UI observes and configures it. Full contract:
`docs/design/0.5-ui-contracts.md`, transport/settings ownership:
`docs/adr/0012-ui-ipc-and-settings.md`, UI notes: `docs/ui.md`.

- Run: `okkhor daemon [--encoding unicode|bijoy] [--mode bangla|english]
  [--no-ipc] [--ui]`. The CLI flags seed a fresh data home only — afterwards
  the persisted last mode wins (the tray/F9 toggle persists too).
  `--no-ipc` disables the settings-UI pipe (typing, tray, and hotkeys keep
  working); `--ui` launches the settings UI once at startup. Either way a
  store/token failure degrades to typing-without-IPC, never a dead daemon.
- The tray menu has Bangla/English, Pause/Resume, **Settings** (launches the
  UI at the artifact root as `Okkhor.exe`, then a sibling `okkhor-ui.exe`,
  `OKKHOR_UI_PATH` override; a missing UI shows a tray balloon), and Exit.
  Token file:
  `%LOCALAPPDATA%\Okkhor\runtime\ipc.token` (rotated per start, removed on
  clean shutdown; `OKKHOR_DATA_HOME` override for tests/dev/portable).
- `status` reports version, `paused`, `language`/`layout`, `input_mode`,
  `output_encoding`, `popup_visible`, `words_total`, `learning_enabled`,
  `autostart`. `settings.update` validates/clamps, persists atomically, and
  live-applies mode, encoding, pause/capture, popup enable, learning, and
  autostart; hotkeys re-register where feasible (F9 / Ctrl+Space, default
  pause chord) and are reported deferred otherwise — never claimed live when
  they are not. Fonts are validated, not installed.
- `text.insert` is explicit UI clicks only (512 UTF-16 units, 20/s budget,
  same injector as commits, serialized against them — a mid-commit insert
  fails loudly instead of interleaving). `ai.complete` answers `Unsupported`.
- No user text, tokens, or paths in logs or errors; store `Debug` output is
  redacted; an atomic-replace failure surfaces as a typed error plus one
  allowlisted log code instead of a silent fallback.

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
