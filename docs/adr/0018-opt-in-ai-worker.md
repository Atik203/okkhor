# ADR 0018: Opt-in AI worker process boundary

- Status: **Proposed** (owner decision D3/D4, 2026-11-24)
- Date: 2026-11-24
- Related: ADR 0002 (AI off the hot path), ADR 0012 (UI IPC and settings),
  `docs/security-privacy.md`, epic #98, #102

## Context

0.7 adds optional AI text actions (for example, "rewrite this paragraph").
ADR 0002 requires AI to stay off the typing path, be cancellable, cached, and
explicitly opt-in. A network client inside the daemon would couple provider
latency and dependencies to the process that owns typing. The UI must not hold
provider keys or talk to a provider directly.

A separate process gives crash isolation, a small daemon binary, and a clear
place to enforce policy. It also adds a protocol and lifecycle surface that must
be designed, not improvised.

## Decision

### 1. Two crates and one process

- `crates/okkhor-ai` (`#![forbid(unsafe_code)]`): provider trait, request and
  response types, typed errors, mock/disabled providers, bounded in-memory
  cache, circuit breaker, single-flight, debounce, cancellation, and policy.
- `apps/okkhor-worker`: a separate no-window process with a bounded job queue,
  health ping, cancellation, and an IPC server on its own pipe/token.
- The daemon is the only client of the worker. The UI talks only to the daemon.

### 2. Data flow

```text
UI "Try a rewrite" click
        -> daemon IPC ai.complete
        -> policy: opt-in? enabled? text explicit? queue space?
        -> enqueue and return { job_id } immediately
        -> worker IPC (own pipe/token)
        -> AiProvider (mock | disabled | OpenAI behind `http`)
UI polls ai.poll(job_id) -> pending | done(text) | error
UI may ai.cancel(job_id)
```

- The request is an explicit user action; there is no passive capture of
  selections, keystrokes, or window contents.
- The UI shows the text that will be sent and requires a click.
- The result is displayed for review. It is never inserted automatically.

### 3. Policy and privacy

- Opt-in only; default disabled.
- Max one in-flight request per installation; 15 s timeout; cancellable.
- Cache is in memory only (64 entries / 256 KiB), keyed by SHA-256 of
  system + prompt + model + prompt version + temperature + max_tokens. No prompt
  or response on disk.
- No prompt, response, key, or window title in logs, errors, crash reports, or
  telemetry.
- API key from `OKKHOR_OPENAI_API_KEY` only; model id from `OKKHOR_AI_MODEL`
  with default `gpt-6-luna` (pricing checked 2026-11-24; ~$0.0002 per short
  rewrite) and `gpt-6.1-terra` as the quality option; neither key nor model is
  persisted, echoed, or shown beyond "set/unset". Gemini remains a future
  provider behind the same trait.
- The provider is behind a Cargo feature (`http`) so the default build is
  offline and CI needs no network.
- Tests use `MockProvider` and a local fake HTTP server only.

### 4. Lifecycle and failure

- The daemon spawns the worker lazily on the first AI request and reaps it on
  shutdown.
- Worker crash, hang, or missing binary returns typed `WorkerDown` / `Timeout`;
  typing and local suggestions are unaffected.
- Circuit breaker transitions are observable through `ai.status` without text.
- The worker starts with the user's token; it does not expose a public port.

### 5. Protocol

- Add `ai.complete`, `ai.poll`, `ai.cancel`, and `ai.status` to the daemon IPC.
- `ai.complete` must fit the existing 5 s request deadline
  (`crates/okkhor-ipc/src/protocol.rs:26`), so it only validates and enqueues;
  it never waits for the provider. `ai.poll` returns the job state.
- Typed params/results with an explicit UTF-16 prompt cap; the 1 MiB frame cap
  still applies. Errors carry codes only, never prompt text.
- The methods are additive, so protocol v1 stays valid; a breaking shape change
  bumps the version and is negotiated in `hello`.
- The worker's own pipe/token is separate from the UI's daemon token. The daemon
  passes the worker token through the child's environment, never argv.

## Consequences

Positive:

- provider/network code cannot delay a keystroke;
- a worker crash is isolated and visible;
- keys stay out of the UI and out of logs;
- the default build remains offline.

Negative:

- an extra process, pipe, token, and lifecycle to test;
- more IPC surface and version negotiation;
- cache and breaker behavior must be tested for determinism;
- provider quality and availability are outside our control.

## Alternatives considered

- **Provider client in the daemon:** rejected; couples network and provider
  dependencies to the typing process.
- **Provider call from the Tauri UI:** rejected; key exposure, CSP/network
  surface, and no central policy.
- **No worker, disabled provider only:** rejected for 0.7 scope; the owner
  asked for the deferred AI work.
- **Background AI suggestions on every keystroke:** rejected by ADR 0002.
