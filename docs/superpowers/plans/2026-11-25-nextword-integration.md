# Next-word integration plan (0.10.x)

Goal: put the trained next-word models into the ghost hint so Bangla and
English move from the shipped bigram (**12.4 / 31.1** Bangla, **16.9 / 31.3**
English) to the transformer range (**~24 / ~45**), with personal reranking, a
confidence gate and phrase hints.

Owner: Atik. Training continues in parallel; none of these phases needs the GPU.

## Container: `OKNM` v1

The app is Rust and never loads PyTorch.

```text
magic "OKNM" | u16 version | u32 header_len | header JSON | pad to 64
vocabulary blob (UTF-8 words, newline separated, id order)
tensor blocks, each aligned to 64 bytes (offset in header)
```

Header: `language`, `context`, `vocab`, `embed`, `d_model`, `layers`, `heads`,
`ff`, `dtype`, `tensors[{name, shape, dtype, offset, bytes}]`.

`scripts/export-nextword-model.py` writes it, `--quantize-output int8` stores
the vocabulary projection as int8 with one scale per row, and every export also
writes `golden.json` (fixed inputs + PyTorch logits) for the Rust test.

## Phase A — export and offline evidence (done)

- Export validated on the 512-d Bangla model: 299 MB, int8 output, every tensor
  round-trips, golden vectors written.
- Evaluation harness: `scripts/eval-nextword-model.py` on the real held-out
  tails (CPU only).

Measured on 5,000 held-out windows:

| Model | top-1 | top-3 | top-5 |
| --- | ---: | ---: | ---: |
| Bangla 512-d | 18.90 % | 29.52 % | 34.80 % |
| English 512-d | 17.76 % | 30.28 % | 35.60 % |

Confidence curve (Bangla / English precision at that threshold):

| Threshold | Coverage Bn | Precision Bn | Coverage En | Precision En |
| ---: | ---: | ---: | ---: | ---: |
| 0.10 | 52.5 % | 31.9 % | 67.6 % | 23.4 % |
| 0.15 | 38.7 % | 39.0 % | 51.8 % | 28.1 % |
| 0.20 | 30.5 % | 45.0 % | 39.0 % | 32.3 % |
| 0.25 | 25.0 % | 49.4 % | 30.0 % | 37.3 % |
| 0.40 | 15.6 % | 62.2 % | 15.0 % | 51.4 % |

**Decision from the data:** blending the *global* bigram into the model lowers
accuracy at every alpha (Bangla 18.9 → 16.5 at α=1.5). The global prior is
already inside the model; only *personal* evidence should rerank, and only for
pairs the user actually typed. So:

- confidence gate **τ = 0.12** for Bangla, **0.15** for English (≈ 45–52 %
  coverage, precision roughly doubled over the base rate);
- Smriti boost applies only when `count(prev, word) ≥ 1`, multiplier
  `+0.35 · log1p(count)`, never a dense blend.

Acceptance: export/load round-trip is bit-exact for f32 and within ±1 LSB for
int8; harness runs offline with no GPU.

## Phase B — Rust runtime

- `okkhor-engine::nextword::model`: parse `OKNM`, validate bounds and SHA-256
  against the manifest, expose `predict(context_ids, k) -> [(word_id, p)]`.
- Layout: embeddings, 6 × (LayerNorm, MHA, LayerNorm, FFN), projection.
  Attention is over ≤ 16 positions; no KV cache needed.
- int8 matmul for the projection (the 200k × d cost), f32/AXV2 for the rest;
  a portable fallback path keeps non-AVX2 machines working.
- Golden test: logits within 1e-3 of `golden.json`; p99 prediction ≤ 20 ms on
  the dev machine for the 512-d model.
- No `unsafe` outside a reviewed SIMD module with `// SAFETY:` comments.

Acceptance: golden match, latency gate, corrupt blob → `None` and the ghost
falls back to the bigram prior.

## Phase C — ghost integration and personal rerank

- Keep a ring buffer of the last 8 committed words in the daemon session
  (today only `previous_word` is stored).
- On commit: model top-50 → apply the confidence gate → Smriti boost for known
  pairs → take 5 → show the ghost.
- Model load happens once on a worker thread; never on the hook.
- Missing/corrupt model, or a language without a model, keeps the `OKNW` prior.

Acceptance: ghost latency p99 ≤ 25 ms end to end; a fresh profile still gets
hints; a missing model logs once and falls back.

## Phase D — phrase hints

- Suggest `w1 w2` when `P(w1) ≥ τ` and `P(w2 | context, w1) ≥ 0.25`.
- Second forward pass with the top-1 appended (≤ 40 ms total), only while idle.
- Tab inserts both words; Esc/typing dismisses as today.

Acceptance: no double insert, no phrase longer than two words, and a phrase
never appears without the first word being accepted.

## Phase E — gates

- Offline A/B: model+gate vs bigram on the frozen holdout; publish top-1/3/5,
  precision@shown and coverage.
- Latency: p99 for prediction, ghost show, and the full post-commit path.
- Real-key QA rows in `docs/qa/0.10-checklist.md`: hint correctness, cycling,
  phrase accept, model-missing fallback, password fields.
- Ship as a signed model update (`0.10.x`), not a reinstall.

## Order and effort

| Phase | Effort | Depends on |
| --- | --- | --- |
| A | done | — |
| B | 4–7 days | A |
| C | 2–3 days | B |
| D | 1–2 days | C |
| E | 2 days | C, D |

The 1B teachers replace the 512-d artifacts in the same pipeline once their
metrics land; the student (distilled 512-d int8) is what ships.
