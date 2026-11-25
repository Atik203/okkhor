# ADR-0025: SIMD boundary for the next-word runtime

- Status: accepted
- Date: 2026-11-25
- Related: ADR-0024, `docs/superpowers/plans/2026-11-25-nextword-integration.md`

## Context

The `OKNM` model's projection layer is the hot loop: one prediction multiplies
a `vocab × d_model` matrix (up to 200k × 768) against one activation vector.
The reference implementation in `okkhor-engine` is scalar, and `okkhor-engine`
is `#![forbid(unsafe_code)]`, so intrinsics cannot live there.

Measured on the dev machine with the 512-d export: scalar prediction took
**73 ms**; the isolated int16 SIMD projection reaches **18 GMAC/s**, which puts
the projection itself at ~5 ms.

## Decision

- New crate `okkhor-simd` owns the intrinsics behind a **safe API**:
  `dot_i8_f32`, `dot_f32_bytes`, `project_i8_f32`, `project_f32_bytes`.
- Inputs are little-endian byte rows and `&[f32]` activations; all loads are
  unaligned, so callers inherit no alignment precondition.
- Runtime detection (`is_x86_feature_detected!("avx2")`) selects the kernel; a
  scalar twin always exists and is the fallback on other targets.
- The int8 projection quantizes the activation vector once per call to int16
  with a per-vector scale (`30000 / max|x|`) and uses `vpmaddwd`, which does 16
  multiply-accumulates per instruction.
- Every `unsafe` block carries a `// SAFETY:` comment naming the bounds
  invariant, and `unsafe_op_in_unsafe_fn` stays denied workspace-wide.
- Correctness is enforced by tests that compare each kernel with its scalar twin
  at lengths 1..513 and at the model width (512) for whole-block projection.

## Consequences

- The engine stays safe Rust and keeps a portable fallback.
- Block projection (one call per matrix) is required; a per-row call is slower
  than scalar because a `#[target_feature]` function cannot be inlined into a
  plain caller.
- The encoder stores its matrices as int8 with per-row scales: f32 weights made
  the encoder memory-bound (4 B/MAC versus 1 B/MAC, ~4.9 versus ~19.6 GMAC/s).
- Measured on the 512-d export: scalar 73 ms, block projection 50 ms, int8
  encoder + single-pass softmax **17.8 ms p50 / 18.3 ms p99**, inside the 20 ms
  gate. Golden vectors still match exactly.
