# ADR 0001: Rust workspace and process boundaries

- Status: Accepted
- Date: 2026-11-23

## Context

The legacy app is a PyInstaller/Python 3.13/PyQt5 application. Its keyboard
hook, injection, UI, dictionaries, and cloud calls share one process. A UI or
provider failure can affect typing, and the Python runtime makes startup and
memory heavier than necessary.

Akkhor is a ground-up Windows-first rewrite. It must remain testable and
maintainable for years.

## Decision

Use a Rust workspace with explicit boundaries:

- `akkhor-core`: pure language and ranking primitives, no I/O, no Windows;
- `akkhor-learn`: Smriti personalization, no I/O;
- `akkhor-storage`: persistence boundary;
- `akkhor-win`: all Windows input/injection/caret/focus code;
- `akkhor-daemon`: composition and lifecycle;
- future `akkhor-ui`, `akkhor-ipc`, `akkhor-speech`, `akkhor-ai` as separate
  crates/processes.

Run the daemon and UI as separate processes. Run speech/AI in a restartable
worker. Keep `unsafe` confined to `akkhor-win` and reviewed FFI modules.

## Consequences

Positive:

- crash isolation and independent restart;
- deterministic core tests without Windows;
- clear review boundaries for unsafe code;
- the UI can be lazy-loaded and replaced;
- AI/speech cannot block typing.

Negative:

- IPC and process supervision add complexity;
- shared state must be explicit;
- packaging and signing cover multiple binaries.

## Alternatives considered

- Single process with threads: simpler, but a UI/provider crash can stop typing.
- Python with Rust extensions only: faster to start, but keeps the runtime and
  packaging problems the rewrite is meant to remove.
- C#/WPF: viable for UI, but less attractive for the low-level hook and a small
  dependency surface.
