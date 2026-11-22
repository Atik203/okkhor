# ADR 0008: Non-hook benchmark harness

- Status: Accepted
- Date: 2026-11-23

## Context

The 0.2 input layer needs repeatable measurements for two budgets:

- Smriti personal reranking (`< 2 ms p99` hard limit `3 ms`);
- character injection (`< 2 ms p99` hard limit `5 ms`).

The existing benchmark contract asked for a keydown-to-suggestion and
keydown-to-injection harness, but that harness would install a global keyboard
hook. Global hooks must run in a disposable VM or with explicit owner approval,
so they are not suitable for CI or for everyday development. CI also cannot
depend on an interactive desktop.

The project needs a smaller harness that measures the real library and injector
boundaries without a hook, without network access, without clipboard mutation,
and without real user text.

## Decision

- Add `apps/okkhor-bench` as a workspace member. It is a developer tool, not a
  daemon component and not part of the shipped input path.
- `--ranking` builds a deterministic synthetic English/Bangla corpus, trains a
  small in-memory Smriti store, and times `Smriti::rank` calls. It reports
  count, p50, p95, p99, max, total time, and iterations/sec. Percentiles use the
  nearest-rank method and are unit-tested.
- `--injection` creates a private, non-elevated Win32 `EDIT` window, focuses it,
  calls `WindowsInjector::inject_unicode_only`, reads the synthetic text back,
  and reports injector latency plus correctness. It installs no keyboard hook
  and cannot select the clipboard fallback.
- If the test window cannot be focused (headless session, foreground denial),
  the injection mode reports the exact reason and runs a pure UTF-16
  event-construction benchmark. That fallback is explicitly labeled and is not
  presented as Win32 send latency.
- `--smoke` runs a short ranking measurement and validates the report shape. Unit
  tests cover the percentile math, synthetic corpus, ranking path, and CLI, so
  CI needs no interactive desktop.
- JSON and CSV reports are written under `bench-out/`, which is git-ignored.
- The benchmark app uses the `windows` crate only for its private test window.
  All `unsafe` blocks in that developer-only module carry `SAFETY` comments. The
  production Win32 boundary remains in `okkhor-win`.

## Consequences

Positive:

- ranking and injector latency can be measured on a normal desktop and repeated;
- CI can verify the reporting logic and a ranking smoke run without a hook;
- no global hook, clipboard mutation, network, or user data is involved;
- the synthetic corpus is reproducible from a seed and is committed.

Negative:

- the hook-level keydown-to-suggestion and keydown-to-visible-text scenarios
  still require a separate disposable-VM harness;
- the injection benchmark times the injector call; target scheduling and
  readback are reported separately as diagnostics;
- the private test window is extra Win32 code that must be reviewed like other
  FFI modules even though it is never shipped.

## Alternatives considered

- Run the real global hook in CI: rejected because CI has no interactive
  desktop and a global hook is a security-sensitive test.
- Use `criterion` immediately: useful later, but the project needs a named
  report with CI-friendly percentiles and JSON/CSV output now. The custom
  harness keeps the dependency surface small.
- Inject into Notepad or another user application: rejected. A private window
  avoids touching user documents and makes cleanup deterministic.
