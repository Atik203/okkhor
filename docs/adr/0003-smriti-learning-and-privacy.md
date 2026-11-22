# ADR 0003: Smriti learning and privacy

- Status: Accepted
- Date: 2026-11-23

## Context

The legacy app keeps some sentence history and recent words. Okkhor needs to do
better: learn English vocabulary, Bangla vocabulary, and the user's preferred
Romanized-English -> Bangla mappings. A keyboard app that learns from typing is
also a privacy-sensitive product.

## Decision

- Learn only from committed words, accepted suggestions, corrections, and
  transliteration commits.
- Never persist raw keystrokes, documents, or clipboard history.
- Keep the model local-only by default.
- Store word, pair, and bigram aggregates plus a bounded recent context.
- Use a base + delta index so learning never rebuilds the dictionary on a
  keystroke.
- Encrypt the snapshot at rest with DPAPI or an OS-protected key.
- Provide pause, export, and clear controls.
- Evaluate with offline replay before changing weights or features.

## Consequences

Positive:

- user-specific suggestions improve over time;
- privacy boundary is explicit and testable;
- model behavior can be replayed and explained;
- storage stays bounded.

Negative:

- no cross-device sync in v1;
- more state and migration work;
- ranking quality depends on sufficient local data;
- privacy review is required for any future cloud feature.

## Alternatives considered

- Cloud learning: rejected for privacy and latency.
- No personalization: simpler, but loses a key advantage over the legacy app.
- Full-document history: rejected; too sensitive and unnecessary.
