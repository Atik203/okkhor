# Dataset policy and catalog

Binding policy for every dataset used by Okkhor. The live catalog with
verified licenses, sizes, and download commands is
`akkhor-sdd-reports/datasets-catalog.md` (kept out of git until the license
review is recorded here).

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

## Remote-only data handling

- Heavy downloads and processing run only on `uiu-h200`, inside the project
  space, under `tmux`/`screen`. The local workstation never stores full corpora.
- Local repo keeps only: manifests, checksums, small frozen fixtures
  (≤ 2 MB total), and trained coefficients.
- The remote project space stays under the 200 GB quota; check `df -h` and
  `du -sh` before a download and record the size in the manifest.
- Never print or commit hostnames, usernames, tokens, or absolute remote paths.
  Reports use `<project-root>/data/<name>`.

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
- One manifest per file; a collection has a parent manifest listing children.
- Any transformation records an input → output checksum chain.
- Evaluation files are frozen and never used for training; the split is by
  `case_id` hash, not by row order.

## Preprocessing

- Unicode NFC for Bangla, preserve ZWJ/ZWNJ, strip control characters except
  tab/newline in text corpora.
- Script and language tagging; Romanized text is tagged `roman`, Bangla
  `bn`, English `en`.
- Deduplicate exact and normalized rows; report duplicate counts.
- Run a conservative PII scan (emails, phone numbers, URLs, long digit runs)
  and drop those rows from evaluation sets; do not attempt to "clean" user text
  into training data.
- Record tokenizer/normalizer version in the manifest; training and inference
  must use the same normalization.

## Dataset selection criteria

1. Explicit license; permissive preferred.
2. Human-produced or clearly labeled synthetic; synthetic data is not used to
   claim real-world quality.
3. Bangla script correct (NFC, conjuncts, ZWJ/ZWNJ) and Romanization scheme
   documented (Avro/National/Banglish).
4. Size proportionate to value: the Tier 1 reranker needs features, not a
   trillion tokens; start ≤ 2 GB for training and ≤ 500 MB for evaluation.
5. Reproducible: stable revision/commit id, checksum, and a download command
   that works with the configured HF token.
6. No overlap with the evaluation set; contamination check recorded.

## Recommended set

Verified 2026-11-23 via the Hugging Face API (license field, gating, last
modified). Sizes are estimates; the manifest records the exact checksum.

| Task | Dataset | License | Size | Use |
| --- | --- | --- | --- | --- |
| Transliteration | `aplycaebous/BanglaTLit` (canonical: `farhanishmam/BanglaTLit`) | MIT | ~150 MB | train + eval |
| Transliteration | Dakshina `bn` (canonical GitHub) | CC-BY-SA-4.0 | ~10 MB | train + eval |
| Spelling/correction | `mehedihasanbijoy/BanglaSEC` | MIT | ~350 MB | correction features + eval |
| Dictionary/frequency | `wikimedia/wikipedia` bn | CC-BY-SA-3.0 + GFDL | ~1 GB | frequency prior, seed dictionary |
| Dictionary/frequency | `wordfreq` (en + bn) | code Apache-2.0, data CC-BY-SA-4.0 | ~30 MB | frequency prior, seed list |
| Code-mixed context | `CodeMixBench/CodeMixBench` (beneng) | Apache-2.0 | ~50 MB | language-ID / script-mix eval |
| Frequency sample | `allenai/c4` multilingual bn, sampled ≤ 5 GB | ODC-BY | ≤ 5 GB | frequency prior only |
| Golden Bangla eval | `openlanguagedata/flores_plus` bn | CC-BY-SA-4.0, gated (auto) | ~5 MB | fluency/coverage eval |
| Golden segmentation | `UniversalDependencies/UD_Bengali-{BRU,PUD}` | CC-BY-SA-4.0 | small | word boundaries, cursor rules |

Total remote budget ≈ 10 GB including working space. CPU-only; the H200 slice is
not required for Tier 1 training.

### License decisions

- MIT/Apache datasets may train shipped coefficients with attribution.
- CC-BY-SA datasets may train models, but derived dictionary/frequency
  artifacts need a recorded legal review before bundling; evaluation use is
  fine. Attribute Wikipedia/wordfreq/Dakshina/FLORES+/UD in
  `THIRD_PARTY_LICENSES.md`.
- ODC-BY mC4 may contribute derived counts with attribution; never ship raw
  text.
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

After each download: write the manifest, compute `sha256`, check `df -h`, and
record row counts before ingestion.

## Rejection log

Record datasets that were considered and rejected, with the reason (license,
quality, contamination, size). This prevents re-litigating the same source.

| Dataset | Reason |
| --- | --- |
| legacy application assets | oracle only; owner rights, never a corpus |
| settings.pkl and Python pickles | untrusted serialization; never parse |
| scraped social text without license | no license, privacy risk |
