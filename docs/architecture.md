# Architecture

## Context

Okkhor is a Windows-first Bengali/English input engine. It replaces a
PyInstaller/PyQt5 application with a native Rust daemon, a local learning
engine, and a responsive UI. The legacy application is a behavioral oracle, not
a runtime dependency.

## Goals

- sub-millisecond hook handling and predictable p99 typing latency;
- local-first transliteration and suggestions;
- personalization learned on the device;
- AI and speech as background enhancements only;
- a signed, portable Windows binary with a small installer;
- Bengali correctness and accessibility as first-class requirements;
- a staged hybrid global/local learning-to-rank model that never blocks typing.

## Non-goals for 1.0

- macOS/Linux support;
- a cloud account requirement for typing;
- training a large language model locally;
- reproducing every legacy screen before the input path is fast and reliable;
- rewriting licensed third-party dictionaries or fonts.

## Process model

```text
┌──────────────────────────── okkhor.exe ────────────────────────────┐
│                                                                    │
│  WH_KEYBOARD_LL thread                                             │
│        │ bounded, lock-free queue                                  │
│        ▼                                                           │
│  input worker: layout, language, buffer, commit state              │
│        │                                                           │
│        ├── Bornomala candidate generation (dictionary + phonetics) │
│        ├── Smriti personal ranking (memory + user model)           │
│        ├── native suggestion popup (no focus steal)                │
│        └── text injector (SendInput, clipboard fallback)           │
│                                                                    │
│  background: storage writer, cache compaction, maintenance         │
└───────────────┬───────────────────────────────┬────────────────────┘
                │ authenticated named pipe      │ restartable worker
                ▼                               ▼
┌───────────────────────────┐      ┌─────────────────────────────────┐
│ okkhor-ui.exe             │      │ okkhor-worker.exe               │
│ Tauri/Slint settings,     │      │ speech + AI, bounded queues,    │
│ on-screen keyboard,       │      │ cancellation, cache, no typing  │
│ word manager              │      │ path involvement                │
└───────────────────────────┘      └─────────────────────────────────┘
```

### Why separate processes

- A UI crash, WebView failure, speech provider crash, or AI timeout must not stop
  typing.
- The daemon can start at login and stay small.
- The UI can be lazy-loaded only when the user opens settings.
- The worker can be restarted or disabled independently.

## Hot path

1. The low-level hook receives keydown/keyup, timestamps it, and enqueues it.
   It does not allocate, log, lock, or call the language engine.
2. The input worker updates the buffer and language/layout state.
3. The suggestion engine (`okkhor-engine::suggest`) returns local candidates
   from the embedded `OKS1` index (curated corrections, the direct roman
   dictionary, the Avro word list, autocorrect, suffixes, `wordfreq`
   popularity) plus the phonetic table. From 0.9 the daemon selects the table
   from `settings.layout`: Avro, Phonetic and Khipro are alternate roman
   tables; National, Probhat and Bijoy take a fixed physical-key path that
   bypasses suggestions and injects mapped characters directly (ADR 0021).
4. Smriti reranks them using local statistics and request context.
5. The popup shows the top N without activating.
6. A commit is injected with batched Unicode `SendInput`, or clipboard paste
   when the target cannot accept Unicode.
7. The learning event is queued for background persistence.

The path never performs network I/O, disk I/O, AI calls, or UI-thread waits.

## Crate boundaries

| Crate | Responsibility | Must not |
| --- | --- | --- |
| `okkhor-core` | language, text normalization, candidate/ranking types | touch Win32, I/O, async |
| `okkhor-features` | shipped v1 feature vector and deterministic extraction | do I/O, read a clock, or use `unsafe` |
| `okkhor-learn` | Smriti statistics, baseline scoring, snapshots, local adapter (1.0) | do I/O or logging |
| `okkhor-storage` | snapshot load/save, future redb adapter | contain ranking policy |
| `okkhor-win` | hook, injection, caret, focus, clipboard | contain language policy |
| `okkhor-daemon` | composition, lifecycle, tray, workers | own algorithm internals |
| `okkhor-engine` | embedded `OKS1` suggestion stages, ranker input | perform I/O, read a clock, or use `unsafe` |
| `okkhor-model` (0.7) | model parse/validate, logistic scoring, bounded hybrid blend | read a clock, generate candidates, or use `unsafe` |
| `okkhor-train` (dev) | dataset manifests, replay and training tooling | ship in a release |
| `okkhor-bench` (dev) | synthetic ranking/suggestion/injection measurements | install hooks, persist user data, or ship in a release |
| `okkhor-ui` | settings, word manager, on-screen keyboard, quick bar | sit on the hot path |
| `okkhor-ipc` | authenticated local messages, settings, words, AI job control | expose raw text broadly |
| `okkhor-speech` (planned) | audio capture, provider adapters | block typing |
| `okkhor-ai` (0.7) | OpenAI/other providers, cache, cancellation, policy | decide quick suggestions or hold a key |
| `okkhor-worker` (0.7) | isolated AI job process | touch the hook or the popup |

## Data flow

```text
legacy assets ──importer──► bundled dictionaries/layouts ──► Bornomala
user typing ──► events ──► Smriti (memory) ──► batched snapshot ──► storage
suggestion shown/accepted ──► learning queue ──► Smriti
AI request ──► worker ──► cache ──► optional UI badge
```

## Storage and caching

- **L1:** in-memory prefix cache, top words, recent words, current context.
- **L2:** memory-mapped dictionaries and a compact user snapshot.
- **L3:** AI/speech result cache keyed by input hash, language, prompt/model
  version, and engine version.
- **Base + delta:** immutable bundled index plus a mutable user delta; merge on
  a maintenance schedule, never on a keystroke.
- **Invalidation:** language/layout switch, dictionary update, model/prompt
  change, user data clear, app version change.

## IPC

- Local named pipe with a per-user ACL and a random session token.
- Binary framing (`postcard` or equivalent), not JSON, for suggestion batches.
- Bounded request/response sizes and timeouts.
- The UI cannot ask the daemon to inject arbitrary text without an explicit
  local user action.

## Windows integration

- `SetWindowsHookExW(WH_KEYBOARD_LL)` on a dedicated thread.
- `SendInput` with `KEYEVENTF_UNICODE`; clipboard fallback for long text or
  incompatible targets.
- `GetGUIThreadInfo`/caret APIs and per-monitor DPI v2 for popup placement.
- `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` for the suggestion popup.
- Single instance via a named mutex and a message-only window.
- A `Shell_NotifyIconW` tray icon on a hidden message-only window with Pause,
  a disabled Settings placeholder, and Exit.
- Per-user autostart through
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`; the value is
  `"<absolute path>" daemon` and is written without elevation.
- Elevated windows: a non-elevated process cannot inject into them; report the
  limitation instead of silently failing.
- IME on/off, RDP, touch keyboard, and multi-monitor are part of the test matrix.

## Speech and AI

- Speech and AI run in a separate worker with bounded queues.
- AI requests are debounced, cancellable, cached, and never gate local
  suggestions.
- Provider traits keep Google Speech and Gemini replaceable.
- A cloud outage degrades to local behavior with a visible, non-blocking status.

## Observability

- Structured, redacted JSON-lines events from the daemon.
- The daemon logger accepts only allowlisted fields (`event`, `level`,
  `version`, `pid`, `command`, `scope`, `kind`, `code`, `ts_ms`) and has no API
  for free-form text. It writes to `%LOCALAPPDATA%\Okkhor\logs\okkhor.log` and
  stderr.
- A `tracing`-based span layer can replace the local logger when latency
  instrumentation lands; the redaction contract stays the same.
- Latency histograms for hook, candidate generation, ranking, and injection.
- ETW/WPA traces for p99 investigations.
- No user text in logs, crash reports, or telemetry.
- Opt-in crash reporting only after a privacy review.

## Failure modes

| Failure | Required behavior |
| --- | --- |
| storage write fails | keep typing; retry with backoff; surface a quiet warning |
| dictionary missing/corrupt | fall back to bundled minimum; do not crash |
| AI timeout | discard stale result; keep local suggestions |
| UI crash | daemon keeps typing; UI restarts |
| hook removed by Windows | detect and reinstall; if reinstall fails, tell the user |
| focus lost | do not inject into the wrong window; abort and report |
| injection blocked | clipboard fallback or explicit failure, never silent corruption |

## Open decisions

- Tauri vs Slint for the main UI; native popup is likely either way.
- `redb` vs another embedded store for the user delta.
- IPC framing and authentication details.
- Whether per-app context is worth the privacy cost.
- Whether a small local reranker is justified after replay data exists.
