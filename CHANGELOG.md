# Changelog

All notable changes to Okkhor are recorded here. The project follows Semantic
Versioning once it reaches 1.0.

## [0.5.0] - 2026-11-23

### Added

- Avro phonetic engine ported from the legacy 250-rule table, with Khipro and
  Banglish layout data.
- Unicode and SutonnyMJ/Bijoy ANSI output using the verified `ANSI 1` table;
  `okkhor daemon --encoding bijoy`.
- Bundled Kalpurush, Kalpurush ANSI, Noto Sans Bengali, and SutonnyMJ (staged
  from the owner's licensed installation); per-user install/status/uninstall.
- Tauri settings UI, on-screen keyboard, word manager, and a non-activating
  quick control bar with drag-to-move, horizontal resize (320-720 px),
  hide-to-tray/restore, and a two-click quit that shuts the daemon down too.
- Caret-anchored suggestion popup with up to ten numbered rows, light/dark
  theme, mouse and keyboard selection, and a curated starter list that adds
  exact corrections plus prefix alternatives to the Avro rule output.
- F9 Bangla/English toggle with Ctrl+Space fallback, tray left-click toggle,
  non-activating toast, and the existing pause hotkey; a short click plays on
  each mode change and can be disabled with `toggle_sound`.
- Daemon IPC v1: status, settings, words, text insertion, and capture control
  over an owner-only named pipe with a rotating token.
- Encrypted word store (DPAPI) and daemon-owned settings store.
- Offline replay/feature-schema foundation (`okkhor-train`, ADR 0015).

### Changed

- The typing path uses the real Avro table (replacing the 0.2 fixture rules).
- Per-user fonts are installed without admin; Bijoy prefers SutonnyMJ and
  falls back to Kalpurush ANSI.

### Known limitations

- Unsigned beta, no installer/updater.
- Alternatives come from a small curated starter list; the imported Avro
  dictionary and the learned reranker are later milestones.
- No speech/AI actions yet; no clipboard fallback or elevated-window injection.
- Manual real-key QA remains owner-gated; see `docs/qa/`.

## [Unreleased]

### Added

- 0.5 daemon IPC server (Task 3): `okkhor daemon` serves contract §4 on
  `\\.\pipe\okkhor-ipc-v1` (dedicated thread + runtime, max 4 clients, 10 s
  read timeout, 5 s deadline, owner-only DACL) unless `--no-ipc`. The token
  file is created/rotated at startup and removed on clean shutdown; setup
  failures degrade to typing-without-IPC. Handlers: `status` (incl. live
  `input_mode`/`output_encoding`), `settings.get/update` with live-apply
  (mode, encoding, pause/capture, popup enable, learning, autostart;
  hotkeys re-register where feasible else `Deferred`, fonts validated not
  installed), `layouts.list` (Avro/Khipro/Banglish/Bijoy), `words.*`
  (caps/dupes enforced), `text.insert` (512 UTF-16, 20/s, recorder-tested,
  injection-locked against commits), `capture.set`, `ai.complete` →
  `Unsupported`. Tray Settings launches `okkhor-ui.exe` (detached,
  `OKKHOR_UI_PATH` override, balloon + log code when missing); `daemon --ui`
  launches it once at startup. Unit + `ipc_roundtrip` pipe tests; no hook
  install, no SendInput, no text in logs/errors.
- Store review advisories: `Debug`/log output redacted (no user paths from
  either store), and atomic-replace failures surface loudly as a typed error
  plus one allowlisted log code instead of a silent remove-and-retry.
- 0.5 encrypted store (Task 2): `okkhor-win::dpapi` (current-user
  `CryptProtectData`/`CryptUnprotectData`, `CRYPTPROTECT_UI_FORBIDDEN`, typed
  errors, 64 MiB cap), `okkhor-storage::EncryptedJsonRepository` over a
  `Protector` trait (`OKE1` envelope, atomic replace, 8 MiB cap,
  `NotFound`/`Corrupt`/`Unsupported`, deterministic fake protector), and
  daemon `paths` (`settings.json`, `data/words.bin`, `runtime/ipc.token` with
  `OKKHOR_DATA_HOME` test override), `settings_store` (defaults, clamping,
  `update(patch)`), and `words_store` (ids, 20,000 cap, duplicates, search,
  CSV/JSON import/export with 5,000-row import cap) modules. The daemon loop
  and IPC handlers are Task 3 and are not wired yet.

### Fixed

- Associate the global low-level keyboard hook with the process module handle
  instead of passing a null module handle. Hook QA with real keys passes.

### Added

- New `okkhor-lang` crate (0.3 sprint, Task 1): layout/encoding/dictionary
  data types (`Layout`, `EncodingTable`, `Dictionary`) plus a synthetic-fixture
  importer (`import_fixture_dir`, `import_asset`) with `FixtureEnvelope`
  schema enforcement, deterministic `BTreeMap` ordering, and daemon-safe
  `LangError::kind()`/`code()` reporting. No transliteration, FFI, or Win32.
- New `okkhor-transliterate` crate (0.3 sprint): rule-based transliteration
  engine (`RuleTransliterator`, `RuleSet` parsed from synthetic JSON and
  validated/sorted at construction, literal prefix matching) with
  normalization helpers and daemon-safe `TransliterateError::kind()`/`code()`
  reporting. Full phonetic/conjunct behavior is a later milestone.
- New `okkhor-compat` crate (0.3 sprint): safe legacy-DLL boundary with a
  private `cfg(windows)` FFI module, DLL path validation against the
  daemon allowlist (`DllPathPolicy`, `ValidatedDllPath`), per-adapter Cargo
  feature flags, a fixed 5-slot `AdapterRegistry`, and
  `CompatError::kind()`/`code()` reporting with static `Display` strings.
  Tests use a stubbed loader seam and synthetic nonce files; no real DLL is
  ever opened.
- New `okkhor-engine` crate (0.4 sprint): candidate generation pipeline
  (`CandidateEngine::suggest`: normalize once, dictionary prefix lookup,
  transliteration application, merge/dedup, Smriti rank, top-N truncate) with
  per-language budgets, a synthetic golden corpus (`GoldenCorpus`,
  `check_parity` top-1/top-5), and `EngineError::kind()`/`code()` reporting.
  No `unsafe`, no I/O on the query path.
- `okkhor-bench --candidates` mode (0.4 sprint): runs the golden corpus
  through `CandidateEngine::suggest`, reports parity (`check_parity` with
  `limit = min(5, budget max_limit)`) plus rank latency p50/p95/p99/max, and
  writes JSON/CSV under `bench-out/`. First shared-desktop measurement is
  recorded in `docs/performance.md`.
- 0.3 core contracts (`docs/design/0.3-core-contracts.md`) with ADR 0010, and
  0.4 engine contracts (`docs/design/0.4-engine-contracts.md`) with ADR 0011.
- Lead merge gates G1–G4 closed: daemon logger scopes `lang`,
  `transliterate`, `compat` (G1); ADR 0010 index row (G2); layout stays
  metadata-only in the Smriti pair key (G3, `docs/learning.md`); compat
  naming clarified as `okkhor-compat` in ADR 0005 (G4).
- QA-only `okkhor inject <text>` command with a focus delay and forced
  `SendInput` option for manual live-app QA.

## [0.2.0] - 2026-11-23

### Added

- Win32 low-level keyboard hook (`WindowsKeyboardHook`) with a dedicated hook
  thread, message loop, bounded event queue, and pass-through behavior.
- Unicode `SendInput` text injector (`WindowsInjector`) with UTF-16 surrogate
  handling and 256-code-unit batching.
- `okkhor hook-demo` manual smoke test for the hook.
- Per-user single-instance guard (`SingleInstance`) with a named mutex.
- Capture pause state (`set_capture_enabled`, `toggle_capture`) that preserves
  pass-through and stops enqueueing without uninstalling the hook.
- Global `Ctrl+Alt+Shift+P` pause hotkey registered on a dedicated message-loop
  thread, with `hook-demo` wiring.
- Clipboard fallback with focus checks, text-only clipboard preservation,
  Ctrl+V injection, and previous-text restore.
- UIPI/`ERROR_ACCESS_DENIED` handling and explicit blocked/focus/clipboard
  errors.
- Caret anchoring via `GetGUIThreadInfo`/`ClientToScreen` with a mouse/screen
  fallback, DPI-scaled placement, and work-area clamping.
- A non-activating `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` popup scaffold and
  `okkhor popup-demo`.
- Hook cleanup on drop and a single-instance guard around `hook-demo`.
- Per-user autostart through `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
  with a quoted executable path and the `daemon` argument, plus
  `okkhor autostart enable|disable|status`.
- A minimal `Shell_NotifyIconW` tray (`TrayIcon`) on a hidden message-only
  window with Pause/Resume, a disabled Settings placeholder, Exit, and
  `okkhor tray-demo`.
- The resident `okkhor daemon` mode wiring the hook, pause hotkey, tray, and
  single-instance guard together.
- Redacted JSON-lines daemon logging under `%LOCALAPPDATA%\Okkhor\logs` and
  stderr with an allowlisted field set for startup, command, and error events.
- Strict `WindowsInjector::inject_unicode_only` entry point for diagnostics and
  benchmarks that must never use the clipboard fallback.
- Native-control `WM_CHAR` fast path (`Edit`, `RichEdit*`, `Scintilla`) with
  `SendInput` as the universal fallback and measured 1.1–1.3 ms inject p99.
- `okkhor-bench --sendinput` to measure the universal fallback separately.
- `apps/okkhor-bench` with deterministic synthetic English/Bangla ranking
  measurements, nearest-rank p50/p95/p99/max reporting, and git-ignored
  JSON/CSV output under `bench-out/`.
- Injection benchmark against a private non-elevated Win32 `EDIT` window through
  `WindowsInjector`, including Unicode/Bangla readback correctness and a
  deferred UTF-16 event-construction fallback when the session cannot focus a
  test window.
- CI-safe `--smoke` mode and unit-tested percentile/report helpers.
- ADR 0008 documenting the non-hook benchmark boundary.

### Not yet

- the Settings tray action, `TaskbarCreated` re-add after Explorer restart, and
  the settings UI;
- full suggestion-list rendering and interaction;
- hook-level keydown -> suggestion and keydown -> visible-text benchmarks;
- Windows integration matrix and signing.

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
