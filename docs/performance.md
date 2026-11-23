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
| Character injection, native text control (`WM_CHAR` fast path) | < 2 ms p99 | 3 ms |
| Character injection, universal `SendInput` | < 8 ms p99 on a shared desktop; reference target TBD | 12 ms |
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

## Measured 0.2 baseline

Release build, shared Windows 11 desktop. The 100-iteration baseline and the
10,000-iteration stress run produced the same p99 band:

| Path | English p99 | Bangla p99 | Correctness |
| --- | ---: | ---: | --- |
| Adaptive native EDIT fast path | 1.23 ms | 1.11–1.14 ms | 10,000/10,000 each |
| Forced Unicode `SendInput` | 7.44 ms | 6.64 ms | 100/100 each |
| Ranking, 2,000 iterations | 209 us | 209 us | n/a |

The fast path meets the 2 ms injector-call target for known native text
controls. `SendInput` remains the universal fallback for browser, Electron,
Office, and unknown controls; it is measured separately and does not claim the
2 ms target on this shared desktop. Readback observation was 14–19 ms p99 and is
a diagnostic, not a release claim.

A quiet reference-machine run is still required before calling the universal
path “2 ms.” See ADR 0009 and the 0.2 benchmark reports under `bench-out/`.

## Re-verification block — 0.2 release, shared desktop, 2026-11-23

This block records a **new, independent re-verification**, not a replacement for
the measured 0.2 baseline above and **not a reference-machine result**. It was
run on a shared Windows desktop from the isolated worktree on branch
`docs/0.2-perf`. The injection modes could not acquire window foreground, so
this block does **not** re-measure injection latency or injection correctness.
The existing 0.2 injection baseline remains the only injection-latency record.

### Observed environment

| Item | Value |
| --- | --- |
| Commit | `46122f0c8f591d73c555f802e0aed8b23b7cb05e` (`docs/0.2-perf`) |
| Date | 2026-11-23, 14:34–14:35 (UTC+06:00) |
| OS | Microsoft Windows 11 Home Single Language, 10.0.26300, build 26300, x64 |
| CPU | AMD Ryzen 7 5800H, 8 cores / 16 logical processors |
| RAM | 34,204,610,560 bytes (~31.8 GiB) |
| Storage | SK Hynix HFM512GDHTNI-87A0B and WD PC SN730 SDBPNTY-1T00-1101 (fixed NVMe) |
| Display / power | 1920 x 1080; Balanced plan `381b4222-f694-41f0-9685-ff5bb260df2e` |
| Build profile | `--release`; rustc/cargo 1.100.0 (`0ef1bc386`, 2026-11-09), host `x86_64-pc-windows-gnu` |
| Toolchain caveat | GNU host, not the MSVC production toolchain (`AGENTS.md`); no MSVC release claim is made |
| Session | Interactive session 1, but launched from a headless shell with no console window; `ForegroundLockTimeout=200000` ms |

### Commands (all exit code 0)

```powershell
cargo build --release -p okkhor-bench
cargo run --release -p okkhor-bench -- --ranking --iterations 100 --smoke --json
cargo run --release -p okkhor-bench -- --injection --iterations 100 --json
cargo run --release -p okkhor-bench -- --injection --iterations 100 --sendinput --json
cargo run --release -p okkhor-bench -- --injection --iterations 10000 --json
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### Ranking smoke (100 iterations)

Synthetic corpus seed `87184804171602`; 10 untimed warmups; 48 candidates per
iteration; 128 cases (64 English, 64 Bangla); store 180 words / 100 pairs /
178 bigrams; 2,000 ranked suggestions produced.

| Path | p50 | p95 | p99 | max | n | Correctness |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| `Smriti::rank`, ranking smoke | 94.50 us | 186.40 us | 197.80 us | 219.00 us | 100 | `smoke: ok` — ordered percentiles, both languages, non-empty ranking |

Throughput: 11,458.95 iterations/sec. Raw report:
`bench-out/ranking-1795422863733-23888.json` (git-ignored).

This is a 100-iteration smoke run, **not** a like-for-like comparison with the
2,000-iteration 0.2 baseline p99 of 209 us. No regression or improvement is
claimed from this run.

### Injection runs — deferred, no latency measured

All three injection runs returned `path=event_construction_only`,
`deferred=true`, `all_correct=false`, `correct=0`, `failed=0` (no injection
attempted). Reason, verbatim:

> End-to-end injection is deferred: could not focus the test EDIT window
> (attempt 20: foreground_match=false, focus_match=true); run this mode in an
> interactive Windows session

A retry with a visible console window (`Start-Process -WindowStyle Normal`)
deferred with the same reason. The harness's ~200 ms focus retry window never
saw `foreground_match=true`; this session's `ForegroundLockTimeout` is 200,000
ms, a plausible contributing factor that was not isolated here. Therefore **no
injection p50/p95/p99/max and no readback correctness were observed**, in either
adaptive or `--sendinput` mode. The 0.2 baseline above is unchanged.

The numbers below are the harness's explicitly labeled **UTF-16 event
construction** fallback. They are diagnostic only: they are not Win32 send
latency, not injection latency, and not correctness evidence. `--sendinput` is
ignored on this fallback path, so the two 100-iteration rows differ only by
run-to-run variance.

| Run | Language | p50 | p95 | p99 | max | n | Correctness |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| Adaptive, 100 it | English | 400 ns | 600 ns | 800 ns | 800 ns | 100 | n/a (deferred) |
| Adaptive, 100 it | Bangla | 300 ns | 400 ns | 700 ns | 700 ns | 100 | n/a (deferred) |
| `--sendinput`, 100 it | English | 300 ns | 900 ns | 2.20 us | 2.50 us | 100 | n/a (deferred) |
| `--sendinput`, 100 it | Bangla | 200 ns | 500 ns | 1.70 us | 2.10 us | 100 | n/a (deferred) |
| Stress, 10,000 it | English | 200 ns | 400 ns | 500 ns | 8.70 us | 10,000 | n/a (deferred) |
| Stress, 10,000 it | Bangla | 200 ns | 300 ns | 300 ns | 74.70 us | 10,000 | n/a (deferred) |

Raw reports (git-ignored): `bench-out/injection-1795422868733-20528.json`
(adaptive 100), `bench-out/injection-1795422926634-20708.json` (`--sendinput`
100), `bench-out/injection-1795422935187-24252.json` (stress 10,000), plus the
visible-window retry `bench-out/injection-1795422903129-4448.json`.

No global hook was installed and the user clipboard was not mutated. All text
was synthetic (`keyboard`, `কীবোর্ড`); no real user text and no legacy executable
were used.

### Workspace checks

`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D
warnings`, and `cargo test --workspace` all exited 0; 71 tests passed, 0 failed.

Status: **DONE_WITH_CONCERNS** — ranking smoke re-verified; injection latency
and correctness not re-measured because the harness could not focus its private
EDIT window in this session. A quiet, interactive reference-machine run with an
MSVC release build is still required for release claims.

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

Implemented for the 0.2 input slice:

- `apps/okkhor-bench --ranking` calls `okkhor-learn` directly with a synthetic
  English/Bangla corpus and reports nearest-rank p50/p95/p99, max, sample count,
  and iterations/sec;
- `apps/okkhor-bench --injection` creates a private non-elevated Win32 `EDIT`
  window and measures `WindowsInjector` latency plus Unicode/Bangla readback
  correctness, without a global hook or clipboard mutation;
- `--json` and `--csv` write raw reports under git-ignored `bench-out/`;
- `--smoke` and unit-tested percentile helpers are safe for CI.

See [benchmarks/README.md](../benchmarks/README.md) for commands and the report
schema. The injection mode falls back to an explicitly labeled event-construction
benchmark when the session cannot focus the test window.

Still planned:

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
