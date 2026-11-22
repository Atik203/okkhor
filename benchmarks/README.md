# Benchmarks

The 0.2 harness lives in `apps/okkhor-bench`. It measures the ranking and
injection boundaries directly, without installing a global keyboard hook, using
the network, reading user data, or mutating the user clipboard.

Run it from the repository root:

```powershell
$env:PATH = "$HOME\.cargo\bin;$env:PATH"

# CI-friendly ranking smoke
cargo run -p okkhor-bench -- --ranking --iterations 100 --smoke

# A longer ranking run with a JSON report in bench-out/
cargo run --release -p okkhor-bench -- --ranking --iterations 5000 --json

# Manual injection check against a private EDIT window
cargo run --release -p okkhor-bench -- --injection --iterations 20 --json
```

`bench-out/` is git-ignored. Reports are named with a Unix-millisecond stamp and
the process id so repeated runs do not overwrite each other.

## Ranking mode

`--ranking` builds deterministic synthetic English and Bangla cases from
`--seed` (default `0x4f4b4b484f52`), trains a small in-memory Smriti store, and
times `Smriti::rank` for each measured iteration. Candidate cloning and warmup
happen outside the timed section.

The report contains:

- iteration, warmup, candidate, case, and store counts;
- count, p50, p95, p99, max, and total nanoseconds;
- iterations per second.

Percentiles use the nearest-rank method: the p-th percentile is the sample at
`ceil(p / 100 * count)`, clamped to the last sample. `--smoke` runs 100
iterations, checks that the percentiles are ordered, and verifies that both
languages and a non-empty ranked result were produced. The percentile helper and
the smoke path have unit tests, so `cargo test --workspace` covers CI without a
desktop.

## Injection mode

`--injection` creates a private, non-elevated Win32 `EDIT` window on its own
message-pump thread, focuses it, calls `WindowsInjector` with a synthetic English
word and a synthetic Bangla word, then reads the text back and compares it
exactly.

- No global keyboard hook is installed.
- The harness calls the strict `WindowsInjector::inject_unicode_only` entry
  point, which cannot select clipboard paste. If the injector ever returns
  `ClipboardPaste`, the run is marked failed.
- The test window is owned by the benchmark process and is destroyed on exit.
- The reported `inject_latency` is time inside
  `WindowsInjector::inject_unicode_only`. The separate `readback_latency` is the
  observed time until the edit control's text matched; it includes target
  scheduling and polling, so it is diagnostic only.
- If the window cannot become foreground, the report is marked `deferred`,
  includes the exact reason, and contains a pure UTF-16 event-construction
  benchmark instead. That fallback does not prove Win32 send latency.

Injection mode is manual because it needs an interactive desktop. It is not part
of the required CI contract. Do not treat a number from a shared, remote, or
hook-heavy session as a release claim; measure on the quiet reference machine
described in [docs/performance.md](../docs/performance.md).

## Current scope and deferred scenarios

Implemented for 0.2:

- synthetic English/Bangla ranking latency;
- injector latency and Unicode/Bangla readback correctness;
- non-hook JSON/CSV reports and a CI-safe smoke run.

Still deferred:

- keydown -> local suggestion and keydown -> visible text through the real
  global hook (disposable VM or explicit owner approval);
- top-20 UI update and popup latency;
- 1-hour idle and typing memory;
- storage compaction pauses;
- AI/speech worker contention;
- ETW/WPA traces and a replay runner.

## Rules

- record OS build, CPU, RAM, power plan, build profile, and commit;
- use release builds for release claims, never a debug build;
- compare the same corpus and commit range;
- report p50/p95/p99, max, and sample count, never only an average;
- run on a quiet Windows 11 machine, not while Windows Update or a virus scan is
  active;
- keep raw results under `bench-out/`, not in version control.
