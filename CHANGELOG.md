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
