# ADR 0009: Native-control `WM_CHAR` fast path

- Status: Accepted
- Date: 2026-11-23

## Context

`SendInput` with `KEYEVENTF_UNICODE` is the universal injection path, but the
benchmark measured cross-process injection p99 at **5.8–7.5 ms** for short
English/Bangla strings on this shared desktop. That is above the 0.2 target of
2 ms.

A synchronous `SendMessageTimeoutW(WM_CHAR)` fast path was tested first and was
**slower** (21–27 ms) because it waits for the target thread. An asynchronous
`PostMessageW(WM_CHAR)` path for known native text controls measured
**p99 1.1–1.3 ms** with 100/100 readback correctness in the private EDIT
benchmark.

## Decision

Use a bounded fast path for known native text controls:

- detect the focused control with `GetGUIThreadInfo`;
- read its class with `GetClassNameW`;
- use `PostMessageW(WM_CHAR)` only for `Edit`, `RichEdit20A/W`,
  `RichEdit50W`, `RichEdit60W`, and `Scintilla`;
- keep Unicode `SendInput` as the universal fallback for every other target;
- keep the clipboard fallback only after `SendInput` is blocked;
- never use the clipboard in the fast path;
- if the first posted character fails, fall back to `SendInput`; after a partial
  post, return an error rather than duplicate text.

## Consequences

Positive:

- native edit controls meet the 2 ms injector-call target;
- no global hook and no clipboard mutation;
- the universal path remains available for browser/Electron/Office targets;
- benchmark reports the method used, so results are not conflated.

Negative:

- `PostMessageW` is asynchronous; visible text can lag the injector call and the
  target may process messages later;
- focus can change between the check and message processing;
- class-name allowlisting is deliberately narrow; unknown controls use the
  slower `SendInput` path;
- readback is required in the benchmark to detect silent message loss.

## Alternatives considered

- Keep only `SendInput`: universal but misses the 2 ms target on this machine.
- Synchronous `WM_CHAR`: measured slower.
- Clipboard-only: worse latency and destructive to clipboard state.
- Post `WM_CHAR` to every control: rejected; unreliable and potentially
  corrupting for custom targets.
