# Encodings: Unicode vs SutonnyMJ ANSI

Okkhor holds Bengali text internally as Unicode. Some official workflows
still require **SutonnyMJ ANSI** output: byte text that only reads as Bengali
when rendered with the SutonnyMJ font. This document describes the two
representations, the conversion model, the font story, and the limits.

## Keyboard layout ≠ font encoding

These are different objects; do not confuse them:

- A **keyboard layout** says which key types which character (positional
  input). `Bangla to Bijoy layout` (111 rules) is such a layout. It is kept
  in-tree as `crates/okkhor-lang/assets/encodings/bijoy-keyboard-v1.json`
  for reference only and is **never** used as the output encoder.
- A **font encoding** says which byte sequence renders which glyph under a
  legacy font. `ANSI 1` ("for SutoniMJ font", 256 ordered rules) is the
  actual SutonnyMJ font encoding. `okkhor-lang::sutonnymj::to_bijoy`
  (re-exported as `okkhor-lang::bijoy` for call-site compatibility) applies
  exactly that table, stored as
  `crates/okkhor-lang/assets/encodings/sutonnymj-ansi-v1.json`.

The public function names keep the historic `bijoy` prefix; they mean
Unicode ↔ SutonnyMJ ANSI.

## Unicode vs SutonnyMJ ANSI

- **Unicode** (UTF-8/UTF-16) assigns every Bengali letter, vowel sign, and
  digit its own code point (`ক` U+0995, `ি` U+09BF, `১` U+09E7). Any
  Unicode-capable font (Kalpurush, Noto Sans Bengali) shapes it correctly.
- **SutonnyMJ ANSI** reuses CP1252-range bytes for Bengali glyphs: the byte
  `K` *means* `ক`, `w` in front of a consonant *means* a pre-base `ি`
  (`কি` → `wK`), `©` (U+00A9) is a reph position marker consumed mid-table,
  and high characters such as `‡` (U+2021, pre-base `ে` marker), `‹`
  (U+2039, part of the `স্ক` ligature `¯‹`), and `¯` (U+00AF) are ordinary
  table values the SutonnyMJ font renders as Bangla glyphs. The text is only
  legible under SutonnyMJ — unlike the old keyboard map, this table is
  **not** claimed to be identical across Bijoy fonts, so the converter
  targets SutonnyMJ specifically.

In short: Unicode text carries meaning in the bytes; SutonnyMJ text carries
meaning in **bytes + the SutonnyMJ font**.

## The ordered-rule model

Conversion lives in `okkhor-lang::sutonnymj` and is driven by
`crates/okkhor-lang/assets/encodings/sutonnymj-ansi-v1.json`: 256 ordered
Unicode→SutonnyMJ rewrite rules derived from the legacy `ANSI 1` table
(owner-supplied legacy data, publicly mirrored; provenance fields are in
the file, only `keyMap` order and entries were extracted).

`to_bijoy` applies every rule **sequentially in table order** with regex
replace-all semantics. Order is load-bearing, not incidental:

- `ো`/`ৌ` decompose to `ে` + `া`/`ৗ` first; the later pre-base `ে`/`ৈ`/`ি`
  reorderings (`([cons])(©?)ে` → `‡\1\2`) then move the vowel sign in front
  of its consonant cluster, so `কি` becomes `wক` and only then `wK`.
  A leading `‡` is marked `†` by the legacy `^‡` rule, so bare `ো`
  converts to `†v` and `কে` to `†K`.
- Reph rules (`র্([cons])` → `\1©`) emit the `©` marker before the bare `র`
  rule runs, and the later `(…)(©?)ে/ি/ৈ` rules consume it (`র্ক` → `K©`).
- The `ৃ`/`ু`/`ূ` families and the `্র…` families carry a negative
  lookbehind, `(?<!্)`, so they do not fire inside a larger cluster. The
  guard is load-bearing: in `স্কুল` the `ু` rule matches `স্ক` + `ু`
  (giving `স্কzল`) because the `স` has no preceding virama; without the
  guard the cluster would split and the `স্ক` ligature (`¯‹`) would never
  form. Final: `¯‹zj`.
- Conjunct collapsing (`্র` → `ª`, `্য` → `¨`) and the large ligature table
  (`ক্ষ` → `¶`, `জ্ঞ` → `Á`, `স্ত্র` → `¯¿`, …) run after the vowel-sign
  rules that may consume the surrounding signs.
- Digits, `ৎ` → `r`, `ং` → `s`, `৳` → `$`, `।` → `|`, and punctuation map
  last; they never collide with earlier patterns.

The engine is `fancy-regex` (MIT, pure Rust): the table needs negative
lookbehind plus `\1`/`\2` backreferences, which the `regex` crate omits.
Translating the lookbehind away (capture-and-re-emit prefixes) would change
match boundaries for overlapping sequential rewrites, so the table is
applied verbatim instead. Replacement expansion is manual (`\N` → group
text, everything else literal), so `$`, `\`, and `©` can never be misread
as engine directives. Tables are embedded, compiled once, and conversion
does no I/O.

Inputs are bounded (4096 Unicode scalar values) and anything unmapped —
Latin text, spaces, newlines — passes through unchanged, so mixed content
converts safely.

`from_bijoy` is the ordered inverse for paste/import, driven by
`crates/okkhor-lang/assets/encodings/sutonnymj-ansi-reverse-v1.json`
(676 rules, derived from the legacy `ANSI 1 to Unicode` table): every rule
applies sequentially in table order, exactly like the forward direction.
The legacy `K → ক` rewrite runs before the `w([cluster])(©?) → \1\2ি`
pattern, and that cascade is what decodes `wK` back to `কি` — all eight
render goldens round-trip exactly. Exactness is still not guaranteed for
every input: line-start `†`/`ˆ` forms decode to `‡`/`‰` plus the vowel sign
rather than recomposing `ো`/`ৌ`, and the legacy `স্প্ল` entry (whose value
ends in a real newline) is preserved as shipped. Never use it where
exactness matters.

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
neither is present. The app must not fail if SutonnyMJ is missing; only
SutonnyMJ renders the converter's byte output exactly as verified, so a
missing SutonnyMJ is a warning, not a build or runtime failure.

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
2. `to_bijoy` rewrites the Unicode text to SutonnyMJ bytes (8
   render-verified goldens plus conjunct, `ো`/`ৌ`, digit, and round-trip
   cases in `okkhor-lang::sutonnymj` tests; legacy differential testing
   against the installed reference is still pending).
3. Render or hand the bytes to the official form under **SutonnyMJ** when
   staged, otherwise **Kalpurush ANSI** as a degraded fallback. The bytes
   are produced for SutonnyMJ; a missing SutonnyMJ is a warning, never a
   build or runtime failure.

## Limitations

- `from_bijoy` is the ordered legacy inverse (see above); round-trips are
  exact for the eight render goldens but not guaranteed in general.
- National and Probhat layouts are not implemented yet; only Avro-style
  phonetic input plus the stored Kontho/Khipro and Banglish layout data
  (`assets/layouts/khipro-v1.json`, `banglish-v1.json`, kept for a later
  engine task) exist.
- Table goldens are render-verified against SutonnyMJ for the eight core
  words; differential testing against the installed legacy reference is
  still pending.
