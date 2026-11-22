# Changelog

All notable changes to Akkhor are recorded here. The project follows Semantic
Versioning once it reaches 1.0.

## [0.1.0] — scaffold

### Added

- Rust workspace with `akkhor-core`, `akkhor-learn`, `akkhor-storage`,
  `akkhor-win`, and `akkhor-daemon`.
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
