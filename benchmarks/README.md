# Benchmarks

The benchmark harness is planned for milestone 0.2, once the input daemon
exists. Until then, this directory documents the contract.

Required scenarios:

- cold and warm daemon startup;
- keydown -> local suggestion, p50/p95/p99;
- keydown -> injected character, p50/p95/p99;
- top-20 suggestion update;
- 1,000-word English typing burst;
- 1,000-word Bangla transliteration burst;
- mixed English/Bangla context switches;
- 1-hour idle and 1-hour typing memory;
- AI/speech worker contention with typing.

Rules:

- benchmarks run on a quiet Windows 11 machine, not CI;
- record OS build, CPU, RAM, power plan, and build profile;
- compare against the previous release and the legacy app where possible;
- never benchmark with a debug build for release claims;
- store raw results under `bench-out/` (git-ignored);
- summarize only p50/p95/p99 and worst-case, not averages alone.
