# Dataset policy and catalog

Binding policy for every dataset used by Okkhor. The machine-readable record for
a prepared corpus is the generated `collection.json` plus one schema-1 manifest
per data file, produced by `scripts/prepare-datasets.py`. A research catalog
(`akkhor-sdd-reports/datasets-catalog.md`, outside git) is background material,
not a source of truth.

No corpus is committed. The repository keeps manifests, checksums, and small
synthetic fixtures only; bulk licensed data stays in the project space on
`uiu-h200`.

## Scope

Datasets support four jobs:

| Job | Used for | Ships to users |
| --- | --- | --- |
| Dictionary / frequency prior | bundled word list, base scores, n-grams | yes, as derived aggregates |
| Transliteration pairs | training/evaluating Romanized → Bangla preferences | derived weights; raw data only if license allows |
| Evaluation | frozen replay and golden corpora | small fixtures only |
| Model training | Tier 1 reranker coefficients | coefficients only |

Out of scope before 1.0: training a transformer, speech datasets, and any
corpus collected from user keystrokes.

## License tiers

| Tier | Examples | May ship weights? | May ship rows? | Use |
| --- | --- | --- | --- | --- |
| Permissive | CC0, CC-BY, MIT, Apache-2.0 | yes, with attribution | yes, with attribution | train, eval, dictionary |
| Share-alike | CC-BY-SA, ODbL | yes, keep the same license and attribution | yes, same license | train, eval, dictionary |
| Non-commercial | CC-BY-NC, NC-SA | **no** without written legal review | **no** | evaluation only, not in release |
| Unknown / no license | scraped, "research only" | no | no | do not use |
| Proprietary | legacy assets, purchased dictionaries | only with owner rights | only with owner rights | importer, not committed |

Rules:

- A dataset without an explicit license is rejected by the manifest verifier.
- Non-commercial data may inform an internal experiment but must not produce
  shipped weights without a recorded legal decision. When in doubt, exclude it.
- Attribution and license text for every shipped dataset/derived model go into
  `THIRD_PARTY_LICENSES.md` and the About screen.
- The legacy application's assets are never treated as a training corpus; they
  remain a behavioral oracle only.

## Preparing a dataset (0.10)

`scripts/prepare-datasets.py` is the supported importer. It is standard-library
only, offline, and never reads user data. It turns operator-supplied corpus
files into normalized splits plus schema-1 manifests.

Input layout (never committed):

```text
<corpus-dir>/
  dataset-sources.json
  <name>.tsv | .csv | .jsonl | .txt
```

`dataset-sources.json` (schema 1):

```json
{
  "schema": 1,
  "name": "phase5-licensed",
  "version": "1.0.0",
  "eval_percent": 10,
  "eval_required": true,
  "frequency": {
    "license": "CC-BY-SA-3.0",
    "source": "wikimedia/wikipedia bn",
    "attribution": "Bengali Wikipedia contributors, CC BY-SA 3.0 and GFDL"
  },
  "files": [
    {
      "path": "banglatlit.tsv",
      "name": "banglatlit",
      "kind": "pairs",
      "role": "hash",
      "license": "MIT",
      "source": "aplycaebous/BanglaTLit",
      "attribution": "BanglaTLit dataset, MIT License",
      "languages": ["roman", "bn"],
      "notes": "roman -> Bangla pairs"
    }
  ]
}
```

Rules:

- `license`, `source`, and `languages` are mandatory on every file; a licence
  that requires attribution also requires a non-blank `attribution`.
- Licences outside the approved SPDX list are rejected. Adding one is a legal
  decision recorded in this file first.
- Non-commercial licences are accepted only with `"role": "eval"`.
- `kind` is `pairs`, `sentences`, or `corrections`; `role` is `hash` (default),
  `train`, or `eval`.
- `path` must be relative, inside the input directory, and have an allowed
  extension. Input and output directories must be disjoint.
- The frequency block is required when Bangla tokens are present. It names the
  licence and attribution for the derived counts, which may differ from the
  per-file licences.

Row shapes: `pairs`/`corrections` use `input`/`roman` and
`target`/`bangla`; `sentences` use `text`/`sentence`. JSONL/CSV/TSV may also
carry `case_id` and `language`; `.txt` is one sentence per line. Unknown
languages and malformed `case_id` values are rejected.

Commands:

```powershell
python -m py_compile scripts/prepare-datasets.py

# validate and show the plan; writes nothing
python scripts/prepare-datasets.py --input <corpus-dir> --out <out-dir> --dry-run

# write splits, frequency artifact, manifests, collection.json
python scripts/prepare-datasets.py --input <corpus-dir> --out <out-dir>

# re-hash, re-count, re-check the split and recompute the frequency artifact
python scripts/prepare-datasets.py --out <out-dir> --verify
```

Output layout:

```text
<out-dir>/
  collection.json                       collection index, input hashes, split rule,
                                        licence summary, attributions, token stats
  train/<name>.jsonl                    rows sorted by case_id
  train/<name>.manifest.json            schema 1, split=train
  eval/<name>.jsonl                     frozen holdout rows
  eval/<name>.manifest.json             schema 1, split=eval
  freq/bn-tokens.tsv                    token, count, per_million, zipf, bucket
  freq/bn-tokens.manifest.json          schema 1, split=train
```

Every manifest must also pass `okkhor-train-dataset --verify <manifest>`
(`cargo run -p okkhor-train --bin okkhor-train-dataset -- --verify <manifest>`).
The committed example lives in `benchmarks/datasets/fixtures/`.

## Remote-only data handling

- Heavy downloads and full-corpus processing run only on `uiu-h200`, inside the
  project space, under `tmux`/`screen`. The local workstation never stores full
  corpora.
- `scripts/prepare-datasets.py` reads a local directory only. Point it at the
  remote project copy when that is where the corpus lives; it never downloads.
- Local repo keeps only: manifests, checksums, small frozen fixtures
  (≤ 2 MB total), and trained coefficients.
- The remote project space stays under the 200 GB quota; check `df -h` and
  `du -sh` before a download and record the size in the manifest.
- Never print or commit hostnames, usernames, tokens, or absolute remote paths.
  Reports use `<project-root>/data/<name>`.

## No raw user text

- The pipeline accepts only files named by an operator manifest. It does not
  read `data/learning.bin`, daemon stores, logs, clipboard, or telemetry.
- Corpus rows are never committed. Only derived splits, aggregate counts,
  checksums, and small synthetic fixtures enter git.
- PII-shaped rows (email, URL, long digit run, phone) are dropped before any
  split output, and the count is recorded; trained artifacts must never be
  presented as user data.
- Global training data is licensed or synthetic only. The opt-in aggregate
  contribution remains deferred (0.10 owner decision B).

## Manifest v1

```json
{
  "schema": 1,
  "name": "synthetic-bootstrap-v1",
  "version": "1.0.0",
  "license": "CC0-1.0",
  "source": "generated",
  "sha256": "<64 hex>",
  "rows": 0,
  "languages": ["bn", "en"],
  "split": "train",
  "notes": "synthetic romanization from bundled layouts"
}
```

Rules:

- `license` and `source` are mandatory; `sha256` and `rows` are verified.
- The data file sits beside the manifest and is named by stripping
  `.manifest.json` from the manifest name: `mini-corpus.manifest.json` verifies
  `mini-corpus.jsonl` (also tried: `.txt`, `.csv`, `.tsv`, in that order).
- Row count: `.jsonl` and `.txt` count non-empty lines; `.csv` and `.tsv` count
  non-empty lines minus one header line (saturating at zero); any unknown
  extension counts non-empty lines.
- One manifest per file; a collection has a `collection.json` index listing
  children, their manifest/data hashes, the split rule, licence summary,
  attributions, and frequency statistics. `collection.json` is an index, not a
  schema-1 manifest: schema 1 describes exactly one data file.
- Any transformation records an input → output checksum chain.
- Evaluation files are frozen and never used for training; the split is by
  `case_id` hash, not by row order.
- Committed fixtures are synthetic or permissively licensed, live under
  `crates/okkhor-train/data/fixtures/` and `benchmarks/datasets/fixtures/`,
  stay within 2 MB in total, and must pass
  `okkhor-train-dataset --verify` in CI. Currently:
  `mini-corpus.jsonl` + `mini-corpus.manifest.json` (16 rows, CC0-1.0,
  synthetic) and the `fixture-synthetic` collection (train/eval/frequency
  artifacts, CC0-1.0, synthetic).

## Preprocessing

- Unicode NFC for every row; ZWJ/ZWNJ preserved; other control/format
  characters stripped; whitespace collapsed to single spaces and trimmed.
- Script and language tagging; Romanized text is tagged `roman`, Bangla `bn`,
  English `en`. Unknown tags are rejected.
- Deduplicate exact and normalized rows within and across sources; report both
  counts. A row that survives is emitted once.
- Run a conservative PII scan (emails, phone numbers, URLs, long digit runs)
  and drop those rows from both splits; record the count. Never "clean" user
  text into a corpus.
- Bangla tokens are runs of U+0980–U+09FF plus ZWJ/ZWNJ containing at least one
  `Lo` character.
- Record the normalizer/tokenizer identity in the manifest notes
  (`prepare-datasets/1`); training and inference must use the same
  normalization. A behavior change bumps the tool version.

## Train/eval split

- A row's `case_id` is supplied by the source or derived as the first 16 hex
  characters of SHA-256 over source name, kind, and the normalized payload.
- The bucket is the first 8 bytes of SHA-256(`case_id`), big-endian, modulo 100.
  Default: rows below 10 are `eval`; the source manifest may set
  `eval_percent` from 1 to 50.
- Per-file `role` overrides the hash: `train` puts every row in train, `eval`
  puts every row in the frozen holdout. `hash` is the default.
- The same `case_id` appearing in two source files is contamination and fails
  closed. A `case_id` in both train and eval is refused by `--verify`.
- Evaluation rows are never counted into the frequency prior and are never used
  for training. Re-running the pipeline must reproduce the same eval rows.
- A ship-gate holdout is a dedicated `role: eval` source of at least 1,000
  licensed cases, separate from any tuning data.

## Determinism

The same input bytes, tool version, and arguments produce byte-identical
output:

- no timestamps, hostnames, absolute paths, random ids, or git metadata in the
  dataset manifests;
- rows sorted by `case_id`, compact JSON with sorted keys, UTF-8;
- manifests are two-space JSON with sorted keys and a trailing newline;
- the frequency TSV is sorted by count descending then token, with fixed
  decimal formatting;
- `collection.json` records input and output SHA-256 values, and `--verify`
  recomputes all of them;
- the operator rebuilds twice and compares hashes before uploading any split.

Model packages, not dataset manifests, record the git commit, hyperparameters,
seed, and metrics (see `docs/model-roadmap.md`).

## Frequency prior

The frequency artifact is derated from aggregated counts only:

- `per_million = count / train_tokens * 1,000,000`;
- `zipf = round(log10(per_million), 2)`; `bucket = floor(zipf)`;
- TSV columns: `token`, `count`, `per_million`, `zipf`, `bucket`;
- counts come from the train split only, never eval;
- `--min-bn-count` and `--top-tokens` control emission and are recorded in
  `collection.json`;
- `total`, `distinct`, emitted rows, top-1000 coverage, and bucket counts are
  recorded.

The D3 target is ≥120,000 distinct Bangla words covered by the index and prior.
This tool reports counts and coverage; it does not claim index coverage, which
is measured against the built index during evaluation.

Wikimedia-derived counts are CC-BY-SA-3.0 with GFDL attribution and require the
recorded legal review before they replace the shipped `wordfreq` popularity
prior. Evaluation use is unaffected.

## Dataset selection criteria

1. Explicit license; permissive preferred.
2. Human-produced or clearly labeled synthetic; synthetic data is not used to
   claim real-world quality.
3. Bangla script correct (NFC, conjuncts, ZWJ/ZWNJ) and Romanization scheme
   documented (Avro/National/Phonetic).
4. Size proportionate to value: the Tier 1 reranker needs features, not a
   trillion tokens; start ≤ 2 GB for training and ≤ 500 MB for evaluation.
5. Reproducible: stable revision/commit id, checksum, and a download command
   that works with the configured HF token.
6. No overlap with the evaluation set; contamination check recorded.
7. Prepared by `scripts/prepare-datasets.py`; manifests pass `--verify` and
   `okkhor-train-dataset --verify`; no hand-edited checksums.

## 0.10 Phase 5 sources

Exact identifiers and attribution for the licensed/synthetic set the 0.10
pipeline prepares. Every row below becomes a `files[]` entry in
`dataset-sources.json` with the stated licence and attribution.

| Role | Source | Licence | Required attribution | Use |
| --- | --- | --- | --- | --- |
| Frequency prior | `wikimedia/wikipedia` `bn` dump (Wikimedia) | CC-BY-SA-3.0 + GFDL | Wikipedia contributors, Bengali Wikipedia, CC BY-SA 3.0 and GFDL, via Wikimedia dumps | token counts, seed vocabulary, derived prior |
| Roman pairs | `aplycaebous/BanglaTLit` (canonical `farhanishmam/BanglaTLit`) | MIT | BanglaTLit dataset, MIT License; keep the copyright and licence notice | train + eval pair preferences |
| Roman pairs | Dakshina `bn`, `google-research-datasets/dakshina` | CC-BY-SA-4.0 | Dakshina `bn` split, Google Research, CC BY-SA 4.0 | train + eval pair preferences |
| Corrections | `mehedihasanbijoy/BanglaSEC` | MIT; NC-labelled portions evaluation-only | BanglaSEC dataset, MIT License; NC portions pending legal review | correction features; NC parts eval only |
| Coverage | Synthetic romanisation through Okkhor's own layouts (ADR 0021) | project-generated; recorded as CC0-1.0 | Okkhor synthetic romanisation from bundled layouts; no third-party text | cold-start pairs, coverage, fixtures |

Details:

- `wikimedia/wikipedia` bn is dual-licensed CC-BY-SA-3.0 and GFDL. The manifest
  `license` field records `CC-BY-SA-3.0` (the share-alike term we rely on) and
  `attribution` names both licences; the derived frequency artifact keeps the
  same attribution and remains share-alike.
- BanglaSEC is tagged MIT, but parts are non-commercial. Split NC content into
  its own input file declared with the NC licence and `"role": "eval"`. Never
  declare NC rows as MIT; the tool cannot detect that for you.
- Synthetic rows are generated from bundled layouts and may never contain
  copied third-party text. Synthetic results are labelled synthetic and do not
  support real-world quality claims.
- Raw corpus rows stay outside git. Only manifests, checksums, aggregate
  counts, and tiny synthetic fixtures are committed.
- Attribution entries must be added to `THIRD_PARTY_LICENSES.md` and the About
  screen before any derived artifact ships, in the same change as the artifact.

### Other approved sources

| Task | Dataset | Licence | Use |
| --- | --- | --- | --- |
| Dictionary/frequency | `wordfreq` (en + bn) | code Apache-2.0, data CC-BY-SA-4.0 | frequency prior, seed list |
| Code-mixed context | `CodeMixBench/CodeMixBench` (beneng) | Apache-2.0 | language-ID / script-mix eval |
| Frequency sample | `allenai/c4` multilingual bn, sampled ≤ 5 GB | ODC-BY | frequency prior only |
| Golden Bangla eval | `openlanguagedata/flores_plus` bn | CC-BY-SA-4.0, gated (auto) | fluency/coverage eval |
| Golden segmentation | `UniversalDependencies/UD_Bengali-{BRU,PUD}` | CC-BY-SA-4.0 | word boundaries, cursor rules |

Total remote budget ≈ 10 GB including working space. Tier 1 is CPU-sized; the
GPU is only for a Tier 2 candidate that passes the gate.

### Licence decisions

- MIT/Apache datasets may train shipped coefficients with attribution.
- CC-BY-SA datasets may train models, but derived dictionary/frequency
  artifacts need a recorded legal review before bundling; evaluation use is
  fine. Attribute Wikipedia/wordfreq/Dakshina/FLORES+/UD in
  `THIRD_PARTY_LICENSES.md`.
- ODC-BY mC4 may contribute derived counts with attribution; never ship raw
  text.
- Non-commercial data is evaluation-only and cannot produce a frequency prior or
  trainable split; the pipeline enforces `role: eval` and rejects a non-
  commercial frequency block.
- Unknown licences are rejected. Adding a licence to the tool allowlist is a
  legal decision recorded here first.
- `ai4bharat/Aksharantar` (license tag `cc`, exact terms unverified),
  `ai4bharat/IndicCorpV2` (truncated license statement), Bengali news corpora,
  XLSum-bn (NC-SA), LinCE (no license), and Norvig/LDC-derived lists are
  **evaluation-only pending legal review** and must not produce shipped
  artifacts.
- Rejected outright: `SKNahin/bengali-transliteration-data` (no license),
  `google-10000-english` (no license), BanglaWordNet (no licensed copy found).

### Remote environment status (checked 2026-11-23)

- `uiu-h200` reachable; MIG `1g.18gb` present, 18 GB VRAM, driver `580.178.04`.
- ~165 GB free inside the 200 GB project quota; HF cache already ~21 GB from
  another project.
- GPU Python: a project venv already has torch 2.14.0+cu130 with CUDA
  available; the default interpreter has no torch. `nvcc` is absent; wheels are
  enough.
- Hugging Face is authenticated locally but **not** on the remote (no token
  file, `hf` CLI missing). Downloads must pass the token per command from the
  local environment over SSH, or install `huggingface_hub` in an Okkhor
  project-local venv; never write the token into the repo or a report.
- `tmux` and `screen` exist for long jobs.

### Remote download workflow

Run on `uiu-h200` inside the Okkhor project space, under `tmux`; never on the
local workstation and never into git.

```bash
# one-time, project-local
python3 -m venv <project-root>/okkhor/venv
<project-root>/okkhor/venv/bin/pip install -U "huggingface_hub[cli]" wordfreq
export HF_HOME=<project-root>/okkhor/hf-cache

# datasets (token comes from the SSH stdin environment, never a file)
hf download aplycaebous/BanglaTLit --repo-type dataset --local-dir <project-root>/okkhor/data/banglatlit
hf download mehedihasanbijoy/BanglaSEC --repo-type dataset --local-dir <project-root>/okkhor/data/banglasec
hf download wikimedia/wikipedia --repo-type dataset --local-dir <project-root>/okkhor/data/wikipedia_bn --include "bn/*"
hf download CodeMixBench/CodeMixBench --repo-type dataset --local-dir <project-root>/okkhor/data/codemixbench
hf download allenai/c4 --repo-type dataset --local-dir <project-root>/okkhor/data/mc4_bn --include "multilingual/c4-bn.*"   # cap 5 GB
hf download openlanguagedata/flores_plus --repo-type dataset --local-dir <project-root>/okkhor/data/flores --include "*ben_Beng*"
git clone --depth 1 --filter blob:none --sparse https://github.com/google-research-datasets/dakshina.git <project-root>/okkhor/data/dakshina
git -C <project-root>/okkhor/data/dakshina sparse-checkout set bn
```

After each download: check `df -h`/`du -sh`, write the source manifest entry with
the real filenames and row counts, then run `scripts/prepare-datasets.py` on the
corpus directory and verify the output twice:

```bash
python scripts/prepare-datasets.py --input <corpus-dir> --out <project-root>/okkhor/data/prepared/<name> --dry-run
python scripts/prepare-datasets.py --input <corpus-dir> --out <project-root>/okkhor/data/prepared/<name>
python scripts/prepare-datasets.py --out <project-root>/okkhor/data/prepared/<name> --verify
```

Only `collection.json`, the split JSONL files, the frequency TSV, and the
schema-1 manifests go to the training machine. The raw download stays in the
remote project space and never enters git.

## Rejection log

Record datasets that were considered and rejected, with the reason (license,
quality, contamination, size). This prevents re-litigating the same source.

| Dataset | Reason |
| --- | --- |
| legacy application assets | oracle only; owner rights, never a corpus |
| settings.pkl and Python pickles | untrusted serialization; never parse |
| scraped social text without license | no license, privacy risk |
| daemon learning store / user text | private user data; never a corpus |
| licence not on the approved list | fails closed until a recorded legal decision |
| non-commercial data in a train role | evaluation-only; pipeline rejects it |
