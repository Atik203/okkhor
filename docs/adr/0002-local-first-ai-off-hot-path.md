# ADR 0002: Local-first typing and AI off the hot path

- Status: Accepted
- Date: 2026-11-23

## Context

The legacy app uses Google Speech (`recognize_google`) and Gemini
(`gemini-flash-latest`) for speech and AI text actions. Those calls are
network-bound. A keyboard app cannot wait for them while the user types, and a
cloud outage must not stop input.

## Decision

- Local dictionaries, transliteration, candidate generation, and Smriti ranking
  always decide quick suggestions.
- The hook callback and input worker perform no network, disk, AI, or UI work.
- AI requests are debounced (250–300 ms idle), cancellable, cached, and
  single-flight.
- Speech and AI run in a separate worker with bounded queues.
- A provider outage degrades to local behavior with a visible, non-blocking
  status.
- Cloud features require explicit user action or a clearly labeled mode.

## Consequences

Positive:

- predictable typing latency;
- offline typing works;
- AI cost and latency are isolated;
- stale AI responses can be discarded safely.

Negative:

- AI cannot be used to “rescue” a bad local suggestion in the same keystroke;
- two code paths (local vs AI) must be maintained;
- caching and cancellation add complexity.

## Alternatives considered

- AI-first suggestions: unacceptable latency and privacy risk.
- AI in the input worker with a timeout: still risks p99 stalls and makes
  offline behavior fragile.
