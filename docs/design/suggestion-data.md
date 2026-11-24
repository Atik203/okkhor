# Suggestion data (0.6, index v2 in 0.10)

The live daemon loads one embedded binary index, `OKS1` version 2, generated at
development time. The query path performs no I/O, no SQLite, and no network.
ADR-0023 records the v2 format and migration.

## Sources

| Source | Use | Count |
| --- | --- | --- |
| Avro Keyboard 5.6.0 `Database.db3` | Bangla word list (47 onset tables) | 159,426 raw |
| `Database.db3` → `Suffix` | roman suffix → Bangla suffix | 722 kept |
| `autodict.dct` | roman autocorrect pairs | 1,161 kept |
| `bn-starter-v1.json` | curated exact forms | 236 |
| `wordfreq` `bn` top 50k | common forms missing from Avro; zipf popularity | 47,827 added |
| `wordfreq` `en` top 60k | English completions; zipf popularity | 58,928 |
| Nms `bangla_dict.bin` | full WTR1 roman→Bangla value table | 135,409 keys, 151,110 pairs |

The union is **184,775 Bangla words** (72,315 with non-zero popularity),
**58,928 English words**, and **151,110 roman→Bangla pairs**. The index is
11,134,034 bytes (~10.6 MiB); the manifest records the source SHA256s, counts,
the rejected key list, and the `wordfreq` version.

## WTR1 import

`bangla_dict.bin` is the documented WTR1 container, not a trie walk:

```text
"WTR1" | u8 phonetic | 6 x pstr header
filter  : u32 n, n x (pstr key, pstr value)
value   : u32 n, n x (pstr alias, u32 m, m x pstr target)
main    : u32 n, n x pstr
suffix  : u8 is_dict, dict(like filter) or list(u32 n, n x pstr)
recent  : u32 0x31524652, u32 n, n x u32 index   (old form: u32 n, n x pstr)
unknown : u32 n, n x pstr
pstr = u16 byte-length + UTF-8 bytes
```

The importer parses this layout with bounds checks on every read, requires the
file to be consumed exactly to the last byte, and fails the build on SHA-256
drift (`554949a4…b2af79`). The observed Bangla file has `phonetic = 1`,
`main = 0`, 135,425 aliases, 151,126 targets, and 142 suffixes.

Rules:

1. Apply the four `filter_dic` nukta mappings (`ড়→ড়`, `ঢ়→ঢ়`, `য়→য়`,
   `ব়→র`) and then NFC.
2. Keep every target of an alias (cap 16; observed maximum is 13), in stored
   order. That order is the only preference signal in the format; there is no
   frequency field.
3. Reject alias keys outside `[a-z]{1,24}`. The 16 rejected keys (for example
   `c:`, `du:sahos`, `prodhanot:`) are listed in the manifest and counted as
   `rejected_keys`.
4. `recent_known` (555 indices) and `recent_unknown` (271 words) are the
   packager's personal state. They are never imported; only the two counts are
   recorded as provenance.
5. Known-mapping assertions (`dhonnobad`, `bhalo`, `ghor`, `mangsho`, `jabo`,
   `koro`, `korechi`, `computer`, …) still fail the build on layout drift.

Raw targets 151,126 minus the 16 targets owned by rejected keys is 151,110
imported pairs. The 0.10 plan's `≥151,120` line was 10 above that arithmetic
maximum; the manifest records both raw and imported counts so the difference
stays visible.

## Regeneration

```powershell
$env:PATH = "$HOME\.cargo\bin;C:\msys64\mingw64\bin;$env:PATH"
python -m pip install -r scripts/requirements-suggest.txt
python scripts/build-suggestion-index.py --avro-home ..\avro_portable `
  --nms-dict "..\Nms Kontho x64 8.002\Dictionaries\Langs\Bangla\Dics\bangla_dict.bin"
.\scripts\verify-suggestion-index.ps1
```

The generator is deterministic: same sources + same `wordfreq` version →
byte-identical index. It never writes an absolute source path into the
manifest. Rebuild and rerun the verification after changing a source. The
committed file keeps the historical `suggest-v1.bin` name; its header is
`OKS1` version 2.

## `OKS1` layout

Little-endian throughout.

```text
magic  "OKS1" (u32) | version u16 = 2 | section_count u16 | reserved u32
section table: { id u32, offset u32, len u32, count u32 } * section_count
1  POOL        UTF-8 bytes shared by all sections
2  CURATED     { roman_off u32, roman_len u16, out_off u32, out_len u16 }
3  BN          { text_off u32, text_len u16, zipf u16 } sorted by UTF-8 bytes
4  BN_POP      u32 indices into BN, sorted by (zipf desc, text)
5  AUTOC       { key_off u32, key_len u16, val_off u32, val_len u16 }
6  SUFFIX      { roman_off u32, roman_len u16, bangla_off u32, bangla_len u16 }
7  EN          { text_off u32, text_len u16, zipf u16 } sorted by UTF-8 bytes
8  EN_POP      u32 indices into EN, sorted by (zipf desc, text)
9  META        UTF-8 JSON (counts, sources, wordfreq version)
10 ROMAN       { key_off u32, key_len u16, rank u8, out_len u16, out_off u32 }
               13 bytes; sorted by (key bytes, rank)
```

`zipf` is `round(zipf_frequency * 100)` clamped to 0–800. Words are NFC,
single-word, and contain only Bengali letters/marks plus ZWJ/ZWNJ. ROMAN keys
are lowercase ASCII, 1–24 bytes; every target of an alias is one row with
`rank = 0` for the preferred form. A repeated target therefore stays a
separate row, matching the multi-target count in the source.

The loader validates the magic, version, section bounds, UTF-8, key shape, the
`(key, rank)` sort order, and rank `< 16`; any failure disables the dictionary
and keeps the rule-only path. A v1 blob is rejected as `BadVersion`.

## Live use of ranks

0.10 A1 imports the ranks but does not change live ordering yet: the frozen
golden gate must stay at top1 92.9 % and top5 100 %. `SuggestionIndex::
exact_roman` returns the preferred (rank 0) target, and
`SuggestionIndex::exact_roman_targets` exposes every row. Phase 3 (B1/B3) uses
the ranks as a bounded tie-break/weight and owns any ranking change.

## Why these sources

- Avro's words match the Avro phonetic rule table already shipped; no
  legacy binary parser is needed.
- `wordfreq` supplies the popularity that the Avro database lacks and fills
  common forms such as `করছো` that the database omits.
- The curated list keeps the 0.5 corrections (e.g. `dhonnobad`) at the top
  even if a source changes.
- The autocorrect list is a bounded correction layer, not an auto-replace
  rule; it only adds candidates.
- The Nms value table covers words the 250-rule table spells differently
  (`ghor` → ঘর, `mangsho` → মাংস, `jabo` → যাবো) and supplies the authored
  alternatives for ambiguous keys (`bari` → বাড়ি, বারি, বাড়ী, বারী); the
  Avro list still supplies words the Nms dictionary lacks.
