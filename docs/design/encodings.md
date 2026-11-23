# Encodings: Unicode vs Bijoy ANSI

Okkhor holds Bengali text internally as Unicode. Some official workflows
still require **Bijoy** output: byte text that only reads as Bengali when
rendered with a Bijoy-compatible font (SutonnyMJ by default where staged,
Kalpurush ANSI as fallback). This document describes the two
representations, the conversion model, the font story, and the limits.

## Unicode vs Bijoy ANSI

- **Unicode** (UTF-8/UTF-16) assigns every Bengali letter, vowel sign, and
  digit its own code point (`ক` U+0995, `ি` U+09BF, `১` U+09E7). Any
  Unicode-capable font (Kalpurush, Noto Sans Bengali) shapes it correctly.
- **Bijoy ANSI** reuses plain ASCII bytes for Bengali glyphs: the byte `j`
  *means* `ক`, the byte sequence `d` + consonant *means* a pre-base `ি`,
  and so on. The text is only legible under a Bijoy-layout font
  (SutonnyMJ by default where staged, Kalpurush ANSI as fallback). The byte
  mapping is identical between those fonts, so converting once is
  correct for either; only the rendering font choice differs.

In short: Unicode text carries meaning in the bytes; Bijoy text carries
meaning in **bytes + font**.

## The ordered-rule model

Conversion lives in `okkhor-lang::bijoy` and is driven by
`crates/okkhor-lang/assets/encodings/bijoy-v1.json`: 111 ordered
Unicode→Bijoy rewrite rules (owner-supplied legacy data, publicly
mirrored; provenance fields are in the file).

`to_bijoy` applies every rule **sequentially in map order** with regex
replace-all semantics. Order is load-bearing, not incidental:

- Pre-base vowel reorderings run first: `(.)ি` → `d\1` turns `কি` into
  `dক`, and only then does the plain consonant rule turn it into `dj`.
  Running consonants first would wrongly yield `jd`.
- Reph/ref rules (`র্(.)` → `\1A`) run before the bare `র` rule, so `র্ক`
  becomes `jA`, not `Av`.
- `০`/`ৌ` decompose to `ে` + `া`/`ৗ` early and are finished by the later
  plain `ে`/`া`/`ৗ` rules (`ো` → `cf`, `ৌ` → `cX`).
- Conjunct collapsing (`্র` → `z`, `্য` → `Z`) runs after the vowel-sign
  reorder rules that may consume the surrounding signs (`ক্রু` → `jsz`).
- Digits, `ৎ` → `\`, `ঃ` → `|`, `৳` → `$`, and punctuation map last; they
  never collide with earlier patterns.

Inputs are bounded (4096 Unicode scalar values) and anything unmapped —
Latin text, spaces, newlines — passes through unchanged, so mixed content
converts safely. The map is embedded and compiled once; conversion itself
does no I/O.

`from_bijoy` is a **best-effort longest-match inverse** over the literal
(non-regex) rules only, and is documented as approximate: context-sensitive
forward rules have no literal inverse (`cf` decodes to `ো`, not `ো`), and
where several Unicode sources share one Bijoy form the first map rule wins
(`v` decodes to `ব়`, not `র`). Never use it where exactness matters.

## Font matrix

| Family | Role | License | Distribution |
| --- | --- | --- | --- |
| SutonnyMJ | `bijoy-ansi` — **default Bijoy rendering** | Proprietary © Mustafa Jabbar, Ananda Computers (owner-approved for release builds; see `THIRD_PARTY_LICENSES.md`) | Bundled (staged at build time from `OKKHOR_LEGACY_HOME`, never committed) |
| Kalpurush | `unicode-ui` — Unicode UI/editor | SIL OFL 1.0 | Bundled (fetched at build time) |
| Kalpurush ANSI | `bijoy-ansi` — Bijoy fallback when SutonnyMJ is absent | Avro Free Bangla Font Project (owner-approved; no license embedded — see `THIRD_PARTY_LICENSES.md`) | Bundled (fetched at build time) |
| Noto Sans Bengali | `fallback` — Unicode fallback | SIL OFL 1.1 | Bundled (fetched at build time) |
| Siyam Rupali | Optional Unicode font | GPL | User-installed only; not bundled (GPL) |

`okkhor-win::fonts::resolve_bijoy_font` encodes the preference: SutonnyMJ
when staged, Kalpurush ANSI otherwise, typed non-fatal missing when
neither is present. The app must not fail if SutonnyMJ is missing; the
Bijoy byte mapping is identical across Bijoy fonts, so only the rendering
choice changes.

Binaries are never committed: `scripts/fetch-fonts.ps1` downloads the three
bundled fonts into `assets/fonts/` with pinned URLs, lengths, and SHA256
(fails closed). License texts live next to them (`OFL-Kalpurush.txt`,
`OFL-NotoSansBengali.txt`); see `assets/fonts/README.md`.

## Font install (per user, no admin)

`okkhor-win::fonts` copies a fetched font into
`%LOCALAPPDATA%\Microsoft\Windows\Fonts`, registers it under
`HKCU\Software\Microsoft\Windows NT\CurrentVersion\Fonts` with the value
name taken from the font's own name table (`Kalpurush (TrueType)`, file
stem as fallback), calls `AddFontResourceW`, and broadcasts `WM_FONTCHANGE`.
Idempotent when the same file is already installed. Everything stays
inside the user's profile; elevation is never required. `list_bundled_fonts`
reports the three expected files and their roles without logging paths.

## Official-work workflow

1. Type in Banglish/Avro-phonetic or any Unicode layout → Unicode text.
2. `to_bijoy` rewrites the Unicode text to Bijoy bytes (47 map-derived
   goldens in `okkhor-lang::bijoy` tests; legacy differential testing
   still pending).
3. Render or hand the bytes to the official form under **SutonnyMJ** when
   staged, otherwise **Kalpurush ANSI**. The bytes are identical either
   way; only the rendering font differs, and a missing SutonnyMJ is a
   warning, never a build or runtime failure.

## Limitations

- `from_bijoy` is approximate (see above); round-trips are not guaranteed.
- National and Probhat layouts are not implemented yet; only Avro-style
  phonetic input plus the stored Kontho/Khipro and Banglish layout data
  (`assets/layouts/khipro-v1.json`, `banglish-v1.json`, kept for a later
  engine task) exist.
- Map goldens are derived from the supplied rule map; differential testing
  against the installed legacy reference is still pending.
