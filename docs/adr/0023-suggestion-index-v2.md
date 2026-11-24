# ADR-0023: Suggestion index v2 and the full WTR1 import

- Status: accepted
- Date: 2026-11-25
- Supersedes: the `WTR1` leaf-scanner description in ADR-0016 and the ROMAN
  record in `docs/design/suggestion-data.md` (0.6 text)
- Related: ADR-0016, ADR-0021, `docs/superpowers/analysis/0.10-suggestions-and-data.md`,
  `docs/superpowers/plans/2026-11-24-0.10-kontho-feel-and-learning.md` (Phase 2)

## Context

The 0.6 importer treated `bangla_dict.bin` as a byte stream and accepted
complete-looking leaf records (`u16 key_len`, ASCII key, `u32 m`, `u16
value_len`, value). It read the `u32` as a count of one target and kept only
the first target, so 21,232 of 135,425 aliases (15.7 %) were unreachable and
**no** multi-target alias carried more than one row. Most of the loss came from
decomposed nukta targets (`য + ়`, `ড + ়`, `ঢ + ়`), which the file expects the
loader to normalize through `filter_dic` before NFC.

Static analysis of the installed 8.002 files established the complete WTR1
layout from the application's own `dic_bin_reader` docstring and confirmed it
by parsing all three dictionaries to the last byte. The format has no
frequency field: `value_dic` is `alias → ordered targets`, so stored order is
the only preference signal, and `recent_known`/`recent_unknown` are the
packager's personal user state.

## Decision

### 1. Bounds-checked WTR1 parser

`scripts/build-suggestion-index.py` parses exactly:

```text
"WTR1" | u8 phonetic | 6 x pstr header
filter  : u32 n, n x (pstr, pstr)
value   : u32 n, n x (pstr alias, u32 m, m x pstr target)
main    : u32 n, n x pstr
suffix  : u8 is_dict, dict(like filter) or list(u32 n, n x pstr)
recent  : u32 0x31524652, u32 n, n x u32 index   (old form: u32 n, n x pstr)
unknown : u32 n, n x pstr
pstr = u16 byte-length + UTF-8 bytes
```

Every read is length-checked and reports the absolute offset; the parse must
consume the file exactly, with no trailing bytes. The Nms Bangla SHA-256 is
pinned (`554949a4…b2af79`); drift fails the build. `phonetic = 1`, `main = 0`,
and the expected `filter_dic` entry are asserted. The known-mapping set from
0.6 stays.

### 2. Normalization and key policy

Apply `filter_dic` (four nukta mappings) and then NFC. Keep **every** target
of every alias, in stored order, capped at 16 (observed maximum 13). Targets
are not deduplicated: the source's duplicate targets stay separate rows so the
"multi-target aliases with ≥2 rows" count matches the file.

Keys outside `[a-z]{1,24}` are rejected: 16 aliases (`c:`, `du:sahos`,
`prodhanot:`, …). The manifest records both the count and the sorted key list
as `rejected_keys`.

`recent_known` (555) and `recent_unknown` (271) are **not** imported. Only
their counts appear in the manifest, as provenance.

Observed result: 135,425 aliases, 151,126 stored targets, 16 rejected keys,
135,409 keys, 151,110 pairs, 13,192 aliases with ≥2 rows. The plan's
`≥151,120` pair line was 10 above the arithmetic maximum implied by its own
16-key rejection (151,126 − 16 = 151,110); the importer therefore gates on
`≥151,100` and records `roman_targets_raw` alongside `roman_pairs` so the
difference is never silent.

### 3. ROMAN v2 and OKS1 version 2

```text
ROMAN row (13 bytes):
  u32 key_off | u16 key_len | u8 preferred_rank | u16 out_len | u32 out_off
OKS1 header: magic "OKS1" | version u16 = 2 | section_count 10 | reserved
sort key: (key bytes, preferred_rank), rank strictly increasing per key
```

The 13-byte size is the sum of the listed fields (4+2+1+2+4). The plan's
"15 B" estimate was arithmetic drift; no reserved field is needed because the
loader reads each field explicitly. One alias contributes one row per target,
with `rank = 0` for the preferred form.

The loader validates:

- `OKS1` magic, `version == 2`, ten sections, section bounds;
- ROMAN record length exactly `count * 13`;
- keys are lowercase ASCII, 1–24 bytes;
- `rank < 16`;
- rows sorted by `(key bytes, rank)`;
- UTF-8 in the pool, and the existing BN/EN sort and popularity rules.

A v1 blob fails as `BadVersion` with the message "suggestion index version is
unsupported (this build reads OKS1 v2)". The engine keeps its existing
degraded path (`index_bad_version`, rule output only); typing is unaffected.

### 4. Manifest fields

`crates/okkhor-engine/assets/suggest/manifest.json` and the embedded META JSON
add:

| Field | Meaning |
| --- | --- |
| `schema` | `2` (manifest schema) |
| `index_version` | `2` |
| `roman_record_bytes` | `13` |
| `counts.roman_pairs` | imported rows (151,110) |
| `counts.roman_keys` | distinct keys (135,409) |
| `counts.roman_aliases` | source aliases (135,425) |
| `counts.roman_multi_target_aliases` | 13,192 |
| `counts.roman_targets_raw` | source targets (151,126) |
| `counts.rejected_keys` | 16 |
| `rejected_keys` | sorted list of rejected alias keys |
| `counts.recent_known` / `counts.recent_unknown` | 555 / 271, counts only |
| `sources.nms_phonetic.sha256` | pinned digest |

The index bytes and SHA-256 stay in `bytes`/`sha256` and are checked by
`scripts/verify-suggestion-index.ps1`.

### 5. Live behavior in 0.10 A1

Phase 2 imports the data and format only. The frozen golden gate must not move,
so `SuggestionIndex::exact_roman` keeps returning the preferred (rank 0)
target for staging. A new `SuggestionIndex::exact_roman_targets` returns every
target in rank order; the golden tests pin `bari` (4 rows, `বাড়ি` first) and
`cha` (rank 0 = `ছা`) through it, and `bon` still ranks `বোন` top-1 in the
engine. Phase 3 B1/B3 owns the preferred-rank tie-break/weight and any change
to live ordering.

## Consequences

- The index grows from 10,221,520 to 11,134,034 bytes (+0.87 MiB). Size is
  recorded, not a 0.10 gate. Startup validation walks 151,110 ROMAN rows once.
- Alias coverage rises from 114,193 to 135,409 keys; every authored alternative
  is now in the artifact and reachable through the index API. The live card
  exposes them when Phase 3 enables rank-aware ordering.
- The three targets with a matra as the first character (`a → া`, …) and other
  odd source targets remain, because "keep every target" is the owner's rule.
  Ranking, not import, is the place to suppress them.
- The 16 rejected keys and the exact raw/imported target counts are visible in
  the manifest instead of silently disappearing.
- No user data is added: recents are counts only.

## Rollback

The index is embedded, so rollback is a code rollback: revert the commit and
rebuild. The previous `suggest-v1.bin` (OKS1 v1, 10,221,520 bytes) is restored
byte-for-byte from git; the loader is version-gated, so a v2 binary refuses a
v1 file as `BadVersion` and vice versa. A rollback also restores the 0.6
`exact_roman` behavior and golden baseline (105/113 top-1, 113/113 top-5).
No user data, learning store, or on-disk migration is involved.
