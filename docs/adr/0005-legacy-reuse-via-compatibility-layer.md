# ADR 0005: Legacy reuse via a compatibility layer

- Status: Accepted
- Date: 2026-11-23

## Context

The legacy build contains useful dictionaries, layouts, encodings, fonts, and
five native DLLs. It also contains Python bytecode, PyQt UI code, and a
`settings.pkl` file. A clean rewrite should not throw away useful data, but it
must not inherit unsafe packaging or an opaque runtime.

## Decision

- Reuse data assets through importers, subject to ownership/license review.
- Reuse native DLLs only through a future `legacy-compat` crate (working name;
  implemented as `okkhor-compat` everywhere else) with absolute
  paths, validated buffers, feature flags, and differential tests.
- Replace each DLL with a Rust implementation when parity is proven.
- Do not reuse Python bytecode, PyQt code, AHK hotkeys, or the PyInstaller
  runtime.
- Do not deserialize `settings.pkl` in Rust; use a one-time exporter.
- Keep the legacy app side-by-side as an oracle until 1.0.

## Consequences

Positive:

- faster path to language parity;
- lower risk than rewriting every dictionary format first;
- clear removal plan for temporary FFI.

Negative:

- the compatibility crate carries unsafe code and licensing obligations;
- temporary DLLs complicate packaging and signing;
- the legacy app must be available for differential tests.

## Alternatives considered

- Rewrite every format before shipping: slower and riskier.
- Depend on the legacy app at runtime: rejected; defeats the rewrite.
- Ignore legacy data: loses layouts, dictionaries, and user history.
