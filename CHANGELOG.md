# Changelog

All notable changes to Okkhor are recorded here. The project follows Semantic
Versioning once it reaches 1.0.

## [Unreleased]

### Added

- Win32 low-level keyboard hook (`WindowsKeyboardHook`) with a dedicated hook
  thread, message loop, bounded event queue, and pass-through behavior.
- Unicode `SendInput` text injector (`WindowsInjector`) with UTF-16 surrogate
  handling and 256-code-unit batching.
- `okkhor hook-demo` manual smoke test for the hook.

### Not yet

- daemon lifecycle, tray, single instance, pause hotkey;
- clipboard fallback and focus/caret-aware popup placement;
- benchmark harness, Windows integration matrix, signing.

## [0.1.0] — scaffold

### Added

- Rust workspace with `okkhor-core`, `okkhor-learn`, `okkhor-storage`,
  `okkhor-win`, and `okkhor-daemon`.
- Language, text normalization, candidate, and ranking primitives.
- Smriti learning engine with frequency, recency, acceptance, correction,
  transliteration-pair, and bigram signals.
- JSON snapshot persistence with schema validation.
- Windows input/injection boundary types.
- Daemon `self-check` covering English, Bangla, transliteration, and
  persistence.
- CI, formatting, clippy, tests, and developer check script.
- Architecture, roadmap, learning, performance, security, testing,
  compatibility, release, and glossary documentation.
- AGENTS.md engineering contract and ADRs.
- ADR 0007 and the long-term hybrid global/local model roadmap.

### Not included

- keyboard hook, text injection, tray, UI, speech, AI workers, installer.
