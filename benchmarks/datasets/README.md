# Dataset pipeline fixtures

Committed fixtures for `scripts/prepare-datasets.py`. They are small, synthetic,
and CC0-1.0. No licensed corpus bulk, no user text, and no download step lives
here.

Policy and the operator runbook:

- [docs/design/datasets.md](../../docs/design/datasets.md)
- [docs/superpowers/analysis/0.10-dataset-pipeline.md](../../docs/superpowers/analysis/0.10-dataset-pipeline.md)

## Layout

```text
benchmarks/datasets/fixtures/
  synthetic/                     operator input: source manifest + tiny corpora
    dataset-sources.json
    synthetic-pairs.tsv          12 roman -> Bangla pairs
    synthetic-bn.txt             Bangla sentences with a duplicate and an email
    synthetic-holdout.txt        forced evaluation rows
  expected/                      generated output, committed for CI verification
    collection.json
    train/                       per-source train split + schema-1 manifests
    eval/                        hashed and forced eval rows
    freq/                        Bangla token counts, per-million, zipf buckets
  rejected-missing-license/      must fail closed
```

The fixtures exercise the rules that matter:

- train/eval split by `sha256(case_id)`, not row order;
- a forced frozen `role: eval` source;
- duplicate-row removal, blank-line skipping, and PII row dropping;
- deterministic output: building twice produces byte-identical trees;
- schema-1 manifests that `okkhor-train-dataset --verify` accepts.

## Commands

Run from the repository root:

```powershell
python -m py_compile scripts/prepare-datasets.py

python scripts/prepare-datasets.py `
  --input benchmarks/datasets/fixtures/synthetic `
  --out $env:TEMP\okkhor-dataset-preview --dry-run

python scripts/prepare-datasets.py `
  --input benchmarks/datasets/fixtures/synthetic `
  --out benchmarks/datasets/fixtures/expected --force

python scripts/prepare-datasets.py `
  --out benchmarks/datasets/fixtures/expected --verify

# must exit 1 and name the missing licence / attribution
python scripts/prepare-datasets.py `
  --input benchmarks/datasets/fixtures/rejected-missing-license `
  --out $env:TEMP\okkhor-dataset-rejected --dry-run
```

Expected fixture result: 16 train rows, 3 eval rows, 31 Bangla tokens, 27
distinct tokens, all in zipf bucket 4. The generated `expected/` tree is
committed; a deliberate regeneration must be reviewed as a diff (hashes change
with content, never with time).

## Rules

- Fixtures stay tiny (well under 2 MB) and synthetic or permissively licensed.
- Never commit licensed corpus rows, download caches, tokens, or user data.
- The manifests are the contract; do not hand-edit `expected/`. Change the
  inputs or the tool, regenerate, then review the diff.
- Every committed manifest must pass both `--verify` and
  `okkhor-train-dataset --verify`.
