# Architecture

## Context

Akkhor is a Windows-first Bengali/English input engine. It replaces a
PyInstaller/PyQt5 application with a native Rust daemon, a local learning
engine, and a responsive UI. The legacy Kontho build is a behavioral oracle, not
a runtime dependency.

## Goals

- sub-millisecond hook handling and predictable p99 typing latency;
- local-first transliteration and suggestions;
- personalization learned on the device;
- AI and speech as background enhancements only;
- a signed, portable Windows binary with a small installer;
- Bengali correctness and accessibility as first-class requirements.

## Non-goals for 1.0

- macOS/Linux support;
- a cloud account requirement for typing;
- training a large language model locally;
- reproducing every legacy screen before the input path is fast and reliable;
- rewriting licensed third-party dictionaries or fonts.

## Process model

```text
┌──────────────────────────── akkhor.exe ────────────────────────────┐
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
│ akkhor-ui.exe             │      │ akkhor-worker.exe               │
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
3. Bornomala returns local candidates from the dictionary, layouts, phonetics,
   recency, and user deltas.
4. Smriti reranks them using local statistics and request context.
5. The popup shows the top N without activating.
6. A commit is injected with batched Unicode `SendInput`, or clipboard paste
   when the target cannot accept Unicode.
7. The learning event is queued for background persistence.

The path never performs network I/O, disk I/O, AI calls, or UI-thread waits.

## Crate boundaries

| Crate | Responsibility | Must not |
| --- | --- | --- |
| `akkhor-core` | language, text normalization, candidate/ranking types | touch Win32, I/O, async |
| `akkhor-learn` | Smriti statistics, scoring, snapshots | do I/O or logging |
| `akkhor-storage` | snapshot load/save, future redb adapter | contain ranking policy |
| `akkhor-win` | hook, injection, caret, focus, clipboard | contain language policy |
| `akkhor-daemon` | composition, lifecycle, tray, workers | own algorithm internals |
| `akkhor-ui` (planned) | settings, word manager, on-screen keyboard | sit on the hot path |
| `akkhor-ipc` (planned) | authenticated local messages | expose raw text broadly |
| `akkhor-speech` (planned) | audio capture, provider adapters | block typing |
| `akkhor-ai` (planned) | Gemini/other providers, cache, cancellation | decide quick suggestions |

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

- `tracing` for structured, redacted events.
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
