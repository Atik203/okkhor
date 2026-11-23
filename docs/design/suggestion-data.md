# Suggestion data (0.6)

The live daemon loads one embedded binary index, `OKS1`, generated at
development time. The query path performs no I/O, no SQLite, and no network.

## Sources

| Source | Use | Count |
| --- | --- | --- |
| Avro Keyboard 5.6.0 `Database.db3` | Bangla word list (47 onset tables) | 159,426 raw; 159k kept |
| `Database.db3` → `Suffix` | roman suffix → Bangla suffix | 722 kept |
| `autodict.dct` | roman autocorrect pairs | 1,161 kept |
| `bn-starter-v1.json` | curated exact forms | 229 |
| `wordfreq` `bn` top 50k | common forms missing from Avro; zipf popularity | 47,827 |
| `wordfreq` `en` top 60k | English completions; zipf popularity | 58,928 |

The union is **184,775 Bangla words** (72,315 with non-zero popularity) and
**58,928 English words**. The index is ~7.7 MiB; the manifest records the
source SHA256s and the `wordfreq` version.

The Nms Kontho `WTR1` phonetic trie is **not** parsed in 0.6. Its 135,410
roman→Bangla pairs would add spelling variants, but the format is a binary
trie and AGENTS.md requires a test-validated parser, not a guess. It remains a
0.8 option.

## Regeneration

```powershell
$env:PATH = "$HOME\.cargo\bin;C:\msys64\mingw64\bin;$env:PATH"
python -m pip install -r scripts/requirements-suggest.txt
python scripts/build-suggestion-index.py --avro-home ..\avro_portable
.\scripts\verify-suggestion-index.ps1
```

The generator is deterministic: same sources + same `wordfreq` version →
byte-identical index. It never writes an absolute source path into the
manifest. Rebuild and rerun the verification after changing a source.

## `OKS1` layout

Little-endian throughout.

```text
magic  "OKS1" (u32) | version u16 | section_count u16 | reserved u32
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
```

`zipf` is `round(zipf_frequency * 100)` clamped to 0–800. Words are NFC,
single-word, and contain only Bengali letters/marks plus ZWJ/ZWNJ. The loader
validates the magic, version, section bounds, UTF-8, and sort order; any
failure disables the dictionary and keeps the rule-only path.

## Why these sources

- Avro's words match the Avro phonetic rule table already shipped; no
  legacy binary parser is needed.
- `wordfreq` supplies the popularity that the Avro database lacks and fills
  common forms such as `করছো` that the database omits.
- The curated list keeps the 0.5 corrections (e.g. `dhonnobad`) at the top
  even if a source changes.
- The autocorrect list is a bounded correction layer, not an auto-replace
  rule; it only adds candidates.
