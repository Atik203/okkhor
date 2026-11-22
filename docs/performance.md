# Performance

Okkhor is a latency product. Throughput matters less than predictable p99
behavior on a normal Windows desktop under normal load.

## Budgets

| Path | Target | Hard limit |
| --- | --- | --- |
| Hook callback | < 0.5 ms | 1 ms |
| Local candidate generation | < 3 ms p99 | 5 ms |
| Smriti rerank | < 2 ms p99 | 3 ms |
| Top-20 UI update | < 8 ms | 16 ms |
| Popup show | < 10 ms | 20 ms |
| Character injection | < 2 ms p99 | 5 ms |
| Cold daemon start | < 300 ms | 1 s |
| Idle working set | < 40 MB | 80 MB |
| AI suggestion | only after 250–300 ms idle | never blocks |

AI/speech are excluded from the typing budget. Their latency is measured
separately and reported with provider and network conditions.

## What to measure

- keydown -> hook enqueue
- keydown -> local candidates ready
- keydown -> ranked top-20
- keydown -> text visible in the target
- suggestion popup visibility
- English and Bangla bursts, including layout switches
- 1-hour idle and 1-hour typing memory
- storage write pause and compaction pause
- AI/speech worker contention with typing
- startup with warm and cold caches

Report p50, p95, p99, max, and sample count. Never report only the average.

## Measurement protocol

- Use a quiet Windows 11 reference machine on the balanced power plan.
- Record OS build, CPU, RAM, storage type, display scaling, and build profile.
- Compare the same commit range and the same corpus.
- Use release builds with symbols for profiling and stripped builds for size.
- Pin the process to a performance core only for diagnosis, not for release
  claims.
- Repeat each scenario at least 5 times; report variance.
- Reboot or clear caches for cold-start measurements.
- Do not benchmark on a machine running Windows Update or a virus scan.

## Harness

Planned components:

- a small Rust harness that calls `okkhor-core`/`okkhor-learn` directly for
  engine microbenchmarks;
- an end-to-end harness that sends synthetic key events through the real hook
  and measures visible text;
- ETW/WPA traces for scheduling, stalls, and Win32 calls;
- `criterion` benches for candidate generation and ranking;
- a replay runner for learning events with latency histograms.

The end-to-end harness must run in a disposable VM or with explicit approval,
because it exercises global input hooks.

## Scenarios

| Scenario | Corpus |
| --- | --- |
| English burst | 1,000 common English words with corrections |
| Bangla burst | 1,000 Banglish -> Bangla commits across layouts |
| Mixed | 30-second English/Bangla switches |
| Long word | 64-character prefix, fuzzy candidates |
| Noisy context | large bigram history, many user words |
| Cold start | empty user model, first 100 keystrokes |
| Storage pressure | 50k words, 100k pairs, 200k bigrams |
| AI contention | Gemini request in flight while typing |

## Release gates

- No release if p99 keydown -> injection regresses by more than 10% without an
  accepted ADR.
- No release if any budget is exceeded on the reference machine.
- No release if memory grows unbounded in a one-hour typing run.
- No release if AI/speech can block the input worker.
- A performance improvement that worsens p99 is not automatically accepted.

## Profiling tools

- Windows Performance Recorder / Windows Performance Analyzer (ETW)
- `cargo flamegraph` or `perf`-style sampling where available
- `tracing` spans with microsecond timestamps
- `criterion` for microbenchmarks
- custom histogram export for p50/p95/p99

## Baseline

Before replacing a legacy component, record its behavior on the same corpus.
The legacy app is an oracle, not a performance target. If the new path is slower
but more correct, document the trade-off explicitly.
