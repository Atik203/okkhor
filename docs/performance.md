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

## Candidate engine bench — 0.4 `--candidates`, shared desktop, 2026-11-23

This block records the first `--candidates` measurement for the 0.4 candidate
engine (`CandidateEngine::suggest` per golden-corpus case, parity via
`check_parity`, rank latency via the shared nearest-rank helper). It does not
replace the measured 0.2 baseline above and is **not a reference-machine
result**: same shared Windows 11 desktop and GNU host toolchain as the 0.2
re-verification block. Fixture byte-sort ordering is re-verified by the
passing golden-corpus integration test (3/3 top-1 and top-5); the committed
fixtures were not touched.

### Observed environment

| Item | Value |
| --- | --- |
| Commit | `7609838ce960bfb1def91bf80b72ca2f7434e640` (`feat/0.4-bench`) |
| Date | 2026-11-23, 16:12–16:20 local |
| OS | Microsoft Windows 11 Home Single Language, 10.0.26300, build 26300, x64 |
| CPU | AMD Ryzen 7 5800H, 8 cores / 16 logical processors |
| RAM | 34,204,610,560 bytes (~31.8 GiB) |
| Storage | SK Hynix HFM512GDHTNI-87A0B and WD PC SN730 SDBPNTY-1T00-1101 (fixed NVMe) |
| Display / power | Balanced plan `381b4222-f694-41f0-9685-ff5bb260df2e` |
| Build profile | `--release`; rustc/cargo 1.100.0 (`0ef1bc386`, 2026-11-09), host `x86_64-pc-windows-gnu` |
| Toolchain caveat | GNU host, not the MSVC production toolchain (`AGENTS.md`); no MSVC release claim is made |

### Commands (all exit code 0)

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run --release -p okkhor-bench -- --candidates --iterations 1 --json
cargo run --release -p okkhor-bench -- --candidates --iterations 2000 --json
cargo run --release -p okkhor-bench -- --candidates --iterations 2000 --json
cargo run --release -p okkhor-bench -- --candidates --smoke --json
cargo run --release -p okkhor-bench -- --candidates --iterations 5 --csv
```

### Parity (all runs)

| Corpus | Cases | Top-1 hits | Top-5 hits | Top-1 rate | Top-5 rate |
| --- | ---: | ---: | ---: | ---: | ---: |
| `crates/okkhor-engine/tests/fixtures/synthetic/corpus.json` (schema 1) | 3 | 3 | 3 | 1.000 | 1.000 |

### Rank latency (release, one `suggest` per timed sample)

| Run | Iterations | Warmup | Timed subset | p50 | p95 | p99 | max | n | Suggestions | Throughput |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Single | 1 | 1 | 3/3 cases | 3.10 us | 3.10 us | 3.10 us | 3.10 us | 1 | 5 | 322,581/sec |
| Full A | 2,000 | 200 | 3/3 cases | 2.00 us | 2.40 us | 4.60 us | 22.10 us | 2,000 | 8,002 | 475,443/sec |
| Full B | 2,000 | 200 | 3/3 cases | 3.40 us | 4.60 us | 4.80 us | 14.30 us | 2,000 | 8,002 | 295,299/sec |
| Smoke | 10 | 10 | 1/3 cases | 2.40 us | 2.70 us | 2.70 us | 2.70 us | 10 | 50 | 418,410/sec |

Observed p99 (~4.6–4.8 us on the full runs) is three orders of magnitude inside
the < 3 ms local-candidate-generation budget. The two full runs disagree on
throughput (475k vs 295k iterations/sec) from shared-desktop scheduling noise;
no reference-machine claim is made. Raw reports (git-ignored):
`bench-out/candidates-1795428751494-18252.json` (single),
`bench-out/candidates-1795428755801-8492.json` (full A),
`bench-out/candidates-1795428772011-240.json` (full B),
`bench-out/candidates-1795428763259-30760.json` (smoke),
`bench-out/candidates-1795428763535-16172.csv` (csv check).

No global hook was installed and no network was used. All text was synthetic
from the committed golden corpus; no real user text and no legacy executable
were used.

## Synthetic replay baseline — 0.6 Task 1, shared desktop, 2026-11-23

First run of the offline replay harness (`okkhor-replay`) on the frozen
synthetic set. **Synthetic-only: these numbers measure harness behavior and
relative movement on hand-authored fixtures, not real-world Bangla/English
quality.** Not a reference-machine result: same shared Windows 11 desktop and
GNU host toolchain as the blocks above. See `docs/design/replay.md` and
ADR 0015.

### Observed environment

| Item | Value |
| --- | --- |
| Code commit | `ee1765bc8755dcbf35e1c229767a18308632e60f` (`feat/0.6-replay`, Task 1) |
| Date | 2026-11-23 local (UTC+06:00) |
| OS | Microsoft Windows 11 Home Single Language, 10.0.26300, build 26300, x64 |
| CPU | AMD Ryzen 7 5800H, 8 cores / 16 logical processors |
| Build profile | `--release`; rustc/cargo 1.100.0 (`0ef1bc386`, 2026-11-09), host `x86_64-pc-windows-gnu` |
| Toolchain caveat | GNU host, not the MSVC production toolchain (`AGENTS.md`); no MSVC release claim is made |
| Dataset | `crates/okkhor-train/data/synthetic-replay.jsonl` (schema 1, 240 cases, seed `20261123`, 74,172 bytes, sha256 `a0d932469fc474332622ee6d4304917ad4069759fa71aae4f15364013b54046f`) |

### Commands (all exit code 0)

```powershell
cargo run -p okkhor-train --bin okkhor-replay -- --generate --out crates/okkhor-train/data/synthetic-replay.jsonl --seed 20261123 --cases 240
cargo run --release -p okkhor-train --bin okkhor-replay -- --replay crates/okkhor-train/data/synthetic-replay.jsonl --engine --json --out bench-out/replay-engine.json
cargo run --release -p okkhor-train --bin okkhor-replay -- --replay crates/okkhor-train/data/synthetic-replay.jsonl --smriti --json --out bench-out/replay-smriti.json
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Generator determinism re-verified: regenerating to a temp path with the same
seed reproduces the identical SHA-256.

### Overall (240 cases)

| Ranker | Top-1 | Top-5 | MRR | Acceptance | Keystroke savings | Rank p50 | Rank p95 | Rank p99 | max |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Engine (`CandidateEngine::suggest`) | 0.6833 | 0.9667 | 0.7952 | 0.7375 | 0.2556 | 3,200 ns | 13,500 ns | 25,300 ns | 27,300 ns |
| Smriti cold-start (closed 5-candidate pool) | 0.2708 | 1.0000 | 0.5096 | 0.7375 | 0.1336 | 9,300 ns | 16,600 ns | 25,900 ns | 67,200 ns |

An earlier pre-commit run on the same tree agreed on all quality metrics to
4 decimals with latency in the same band (engine p50 2,700 ns / p99
15,000 ns; Smriti p50 5,800 ns / p99 30,300 ns); shared-desktop scheduling
noise only. Both rank paths sit three orders of magnitude inside the < 3 ms
local-candidate-generation budget. The numbers above are from the run on the
recorded code commit.

### By language

| Ranker | Segment | n | Top-1 | Top-5 | MRR |
| --- | --- | ---: | ---: | ---: | ---: |
| Engine | bangla | 191 | 0.6806 | 0.9738 | 0.7992 |
| Engine | english | 49 | 0.6939 | 0.9388 | 0.7796 |
| Smriti | bangla | 191 | 0.2775 | 1.0000 | 0.5096 |
| Smriti | english | 49 | 0.2449 | 1.0000 | 0.5095 |

### Reading these numbers honestly

- Acceptance rate (0.7375) is a property of the synthetic labels (75% of cases
  carry `accepted`), not a user measurement; it is identical for both rankers.
- The Smriti harness ranks the case's closed 5-candidate pool, so its top-5
  is 1.0 by construction. The informative deltas are top-1 (+0.41 engine),
  MRR (+0.29 engine), and latency.
- Keystroke savings counts only top-1 hits, so it tracks top-1 by definition.
- Raw reports (git-ignored): `bench-out/replay-engine.json`,
  `bench-out/replay-smriti.json` (+ `.md` renders).

No network was used. No real user text, no downloaded corpus, and no legacy
executable were involved.

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

## 0.6 suggestion engine (development machine)

Measured 2026-11-24 on the owner's Windows 11 desktop, release build, GNU
toolchain. The corpus is the 113-case golden set plus 2,000 prefix queries
(`a`, `am`, `bha`, `bhal`, `k`, `ko`, `kor`, `dho`, `dhonno`, `com`, `valo`)
through the embedded `OKS1` index. Percentiles use the nearest-rank method.

| Path | Corpus | Top-1 | Top-5 | p50 | p95 | p99 | max |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `SuggestionEngine::suggest` (release) | `golden-v1` (113) + 2,000 prefix queries | 92.9% | 100.0% | 0.37 ms | 0.68 ms | 0.80 ms | 0.97 ms |
| `okkhor-bench --suggestions --smoke` | 35 everyday words | 100.0% | 100.0% | 0.28 ms | 0.56 ms | 0.67 ms | 0.68 ms |

Command:

```powershell
.\target\release\okkhor-bench.exe --suggestions --iterations 2000
```

This is a development-machine number, not a reference-machine claim. The
3 ms p99 budget in `AGENTS.md` is met with headroom; re-measure on the release
candidate before tagging.

## 0.7 Tier 1 hybrid reranker A/B — synthetic replay, 2026-11-24

Labeled 0.7 synthetic row. **Synthetic-only, not a real-world quality claim.**
The frozen replay set (`crates/okkhor-train/data/synthetic-replay.jsonl`,
240 cases, seed `20261123`, sha256 `a0d932469fc474332622ee6d4304917ad4069759fa71aae4f15364013b54046f`)
carries no learning snapshot, so every candidate's `confidence` is `0` and the
hybrid blend is an exact baseline identity. These numbers measure the bounded
pipeline and prove the identity/fallback contract; the set was not used for
tuning.

Environment: shared Windows 11 desktop, `--release`, GNU host toolchain (same
caveats as the 0.6 blocks above). Model:
`crates/okkhor-model/assets/reranker-v1.json`, sha256
`e5a6ce0b3b7e5aa506b48af0a123629b2988b5c01d42ea8153e478eaf4ff6440`, fitted from
the same replay with `okkhor-train-fit --trained-at 2026-11-24T00:00:00Z`.

```powershell
cargo run --release -p okkhor-train --bin okkhor-replay -- --replay crates/okkhor-train/data/synthetic-replay.jsonl --hybrid --model crates/okkhor-model/assets/reranker-v1.json --json --out bench-out/replay-hybrid.json
```

`--hybrid` also writes `bench-out/replay-hybrid-baseline.json` on the same
closed candidate pool.

| Ranker | Top-1 | Top-5 | MRR | Rank p50 | p95 | p99 | max |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Baseline (`Smriti`, closed 5-candidate pool) | 0.2708 | 1.0000 | 0.5096 | 4,400 ns | 8,000 ns | 12,100 ns | 23,400 ns |
| Hybrid (`baseline + confidence * centred`, bundled v1) | 0.2708 | 1.0000 | 0.5096 | 11,800 ns | 19,300 ns | 26,100 ns | 52,200 ns |

Segments are identical: Bangla 191 cases top-1 0.2775 / MRR 0.5096; English 49
cases top-1 0.2449 / MRR 0.5095; layouts range top-1 0.2364–0.3182.

Shared-desktop noise: three further hybrid runs gave p99 35.6 µs, 35.8 µs, and
41.9 µs (baseline 11.1–13.0 µs); the table keeps the first recorded run.

Result: **hybrid does not beat baseline on this dataset** — it is identical by
construction (no learning evidence). The hybrid p99 (26.1 µs first run,
≤ 41.9 µs across all four runs) is inside the < 1 ms hybrid gate and the
< 3 ms local-candidate budget. `--ranker baseline` remains the default until an
A/B on real or licensed data shows a gain.

No network was used. No real user text, no downloaded corpus, and no legacy
executable were involved.

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
