# ADR 0010: Core contracts for the 0.3 multi-agent sprint

- Status: Accepted
- Date: 2026-11-23
- Normative detail: [0.3 core contracts](../design/0.3-core-contracts.md)

## Context

Phase 2 (Bornomala core, version 0.3) adds three crates in parallel:
`okkhor-lang` (layout/encoding/dictionary data and importer),
`okkhor-transliterate` (rule engine and normalization helpers), and
`okkhor-compat` (safe FFI boundary, DLL path validation, per-adapter feature
flags). The sprint plan (Task A) requires a blocking contract before Tasks 1–3
start, because independent agents cannot safely guess each other's public
types, error shapes, fixture formats, or integration boundaries.

Prior decisions set the constraints:

- ADR 0001 fixes crate boundaries, separates processes, and confines `unsafe`
  to `okkhor-win` and reviewed FFI modules.
- ADR 0005 permits temporary native DLL reuse through a compatibility crate
  with absolute paths, validated buffers, feature flags, and differential
  tests, with per-DLL replacement later.
- ADR 0008 keeps benchmarks non-hook and CI-safe.
- AGENTS.md forbids I/O, network, AI, unbounded allocation, and text logging on
  the keystroke path, and forbids committing user text, secrets, and
  proprietary assets.

Without a shared contract, the likely failures are accidental cross-crate
dependencies, duplicated or incompatible data types, unsafe code leaking out
of the FFI seam, path traversal in the DLL loader, and fixtures that contain
real user or proprietary data.

## Decision

Adopt [docs/design/0.3-core-contracts.md](../design/0.3-core-contracts.md) as
the binding public contract for Tasks 1–3. Its essential decisions are:

1. **One-way dependency direction.** `okkhor-lang`, `okkhor-transliterate`, and
   `okkhor-compat` each depend only on the existing `okkhor-core`. There are no
   dependencies between them. `okkhor-core` is not modified in this sprint.

2. **Public surfaces are fixed.**
   - `okkhor-lang`: `LayoutId`, `Layout`, `LayoutMapping`, `EncodingId`,
     `EncodingTable`, `EncodingEntry`, `Dictionary`, `DictionaryEntry`,
     `ImportedAssets`, `ImportReport`, `ImportOptions`, `ImportOutcome`,
     `import_fixture_dir`, `import_asset`, and `LangError`.
   - `okkhor-transliterate`: `normalize_roman`, `normalize_bengali`,
     `normalize_key`, `is_bengali_script`, `RuleId`, `TransliterationRule`,
     `RuleSet`, `TransliteratorEngine`, `RuleTransliterator`,
     `TransliterationOptions`, `TransliterationOutput`, and
     `TransliterateError`. `TransliterationOutput::into_candidate()` bridges to
     `okkhor_core::Candidate` with `CandidateSource::Transliteration`.
   - `okkhor-compat`: `AdapterKind`, `AdapterStatus`, `AdapterHandle`,
     `AdapterRegistry`, `DllPathPolicy`, `ValidatedDllPath`, and `CompatError`.

3. **Purity and unsafe confinement.** `okkhor-lang` and
   `okkhor-transliterate` use `#![forbid(unsafe_code)]`. In `okkhor-compat`,
   `unsafe` exists only in a private `cfg(windows)` `ffi` module, with a
   `// SAFETY:` comment on every block; raw pointers never appear in public
   signatures. Loading is startup/shutdown only, never on the hot path.

4. **Uniform error contract.** Each crate exposes one `#[non_exhaustive]`
   `thiserror` enum with static `Display` text plus `kind()` and `code()`
   accessors for the daemon's allowlisted logger. Error text never contains
   user text, fixture contents, full paths, window titles, or asset bytes.

5. **DLL path policy.** Path validation runs before any loader call: feature
   gate first, then absolute-path, no-traversal, extension, adapter/file-name
   match, canonicalization under an explicit allowlisted root, file checks, and
   size caps. All five adapters are disabled by default via Cargo features
   (`word-trie`, `sentence-trie`, `translator`, `caret`, `emoji-manager`).

6. **Fixture contract.** Tests use only synthetic JSON under each crate's
   `tests/fixtures/synthetic/`, with a `schema` field, an 8 MiB per-file cap,
   relative non-traversing manifest paths, and deterministic ordering. No
   legacy files, binaries, pickle data, secrets, or user text.

7. **Daemon integration is lead-owned.** Modules are constructed once at
   startup; the input worker uses only bounded in-memory lookups and
   `okkhor-learn` ranking. The lead adds `lang`, `transliterate`, and `compat`
   logger scopes and extends `self_check`. Tasks 1–3 do not edit the daemon.

## Consequences

Positive:

- Tasks 1–3 can implement independently without merging shared type files or
  inventing incompatible APIs.
- Reviewers have a concrete checklist (design doc §7) for spec compliance.
- The FFI seam and error contract preserve the privacy and hot-path rules from
  AGENTS.md without relying on reviewer vigilance alone.
- Synthetic-only fixtures keep CI free of proprietary assets and user text.

Negative:

- The contract is detailed up front; small deviations require updating the
  design doc and this ADR rather than silent local fixes.
- Per-adapter FFI wrappers remain temporary debt until each DLL is replaced by
  a Rust implementation with parity tests.
- `AdapterHandle` is intentionally `!Send + !Sync` in 0.3, which constrains
  daemon threading until a reviewed change lifts it.
- Adding logger scopes and the ADR index row is deferred to the lead/Task 8,
  so a merge that omits them leaves a small documentation gap.

## Alternatives considered

- **A shared `okkhor-lang-types` crate** for all data types: rejected; it
  creates a fourth dependency hub and violates the "depend only on
  `okkhor-core`" rule.
- **Put rules and layout data in `okkhor-core`**: rejected; core must stay
  I/O-free, algorithm-neutral, and small, and this would enlarge the shared
  file that every agent touches.
- **Let each agent invent its own error type with no shared shape**: rejected;
  the daemon logger needs stable `kind`/`code` values and must never format
  error text.
- **Load DLLs by file name or from the cwd**: rejected; DLL search-order
  hijack is an explicit threat in `docs/security-privacy.md`.
- **Commit real legacy dictionaries/layouts as test fixtures**: rejected;
  ownership is unresolved and the rules forbid proprietary assets.

## Compliance

Tasks 1–3 must satisfy design doc §3.6, §4.7, and §5.7 respectively, run
`cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo test --workspace`, and report pass/fail with evidence. Task 6 reviews
against design doc §7. Any change to a public signature in this ADR requires a
new or superseding ADR and an update to the design doc in the same change.

## References

- [SDD plan: 0.3 multi-agent sprint](../superpowers/plans/0.3-multiagent-sprint.md)
- [0.3 core contracts](../design/0.3-core-contracts.md)
- [ADR 0001](0001-rust-workspace-and-process-boundaries.md)
- [ADR 0005](0005-legacy-reuse-via-compatibility-layer.md)
- [ADR 0008](0008-non-hook-benchmark-harness.md)
- [Architecture](../architecture.md), [Roadmap](../roadmap.md),
  [Security and privacy](../security-privacy.md), [Testing](../testing.md)
