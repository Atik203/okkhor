# Third-party licenses

This file records the provenance and license of third-party material that
Okkhor stages, bundles, or derives from at build time. No third-party
binaries are committed to this repository; fonts are staged into
`assets/fonts/` (git-ignored) by `scripts/fetch-fonts.ps1`, and layout /
encoding data is imported from owner-supplied JSON (see
`crates/okkhor-lang/assets/`).

## Fonts

### Kalpurush — SIL Open Font License 1.0

- **Copyright/design:** Tanbin Islam Siyam, Avro Font Development Project,
  http://www.omicronlab.com (per the font's name-table records; Kalpurush
  declares OFL 1.0, Version 0.258).
- **License text:** `assets/fonts/OFL-Kalpurush.txt` (SPDX-canonical OFL 1.0
  text; upstream verbatim unobtainable — omicronlab.com font/license pages
  return 404 as of 2026-11-23).
- Unicode UI/editor font.

### SutonnyMJ — proprietary, owner-approved for release builds

- **Copyright:** SutonnyMJ © Mustafa Jabbar, Ananda Computers
  (per the font name table: "Copyright Mustafa Jabbar, Ananda Computers,
  8/6 Segun Bagicha, Dhaka-1000, Bangladesh.").
- **Source:** the owner's legacy installation
  (`OKKHOR_LEGACY_HOME/Res/Fonts/SutonnyMJ Regular.ttf`, 73656 bytes,
  SHA256 `D58D9A45311447301426649C641FE148010163594AE57A4094AC086B21D52BDC`).
- **Terms:** proprietary. The owner accepts the redistribution decision for
  Okkhor release builds. The binary is staged at build time and never
  committed to git. It is Okkhor's default Bijoy rendering font, with
  Kalpurush ANSI as fallback.

### Kalpurush ANSI — Avro Free Bangla Font Project, owner-approved

- **License:** Avro Free Bangla Font Project with the owner's redistribution
  approval for Okkhor release builds. The font embeds **no** license
  declaration (verified 2026-11-23: its name table carries family
  "Kalpurush ANSI" and subfamily only — no copyright, license, or
  description records). It is therefore **not** recorded as OFL; the
  `OFL-Kalpurush.txt` file covers Kalpurush only.
- **Source:** https://www.omicronlab.com/download/fonts/kalpurush%20ANSI.ttf
  (113300 bytes, SHA256
  `BFB06B2360A558424C61405A29C24B09C70487757204814C6DD04621A2A9EEBC`).
- Fallback Bijoy-ANSI rendering font (SutonnyMJ is the default).

### Noto Sans Bengali — SIL Open Font License 1.1

- **Copyright:** The Noto Project Authors
  (https://github.com/notofonts/bengali).
- **License text:** `assets/fonts/OFL-NotoSansBengali.txt`
  (verbatim from the `google/fonts` `ofl/notosansbengali` directory).
- Unicode fallback font. Variable font build; family name unchanged.

## Layout / encoding data

- `crates/okkhor-lang/assets/encodings/sutonnymj-ansi-v1.json` (256 ordered
  forward rules, from legacy `ANSI 1` v1.4 "for SutoniMJ font") and
  `crates/okkhor-lang/assets/encodings/sutonnymj-ansi-reverse-v1.json`
  (676 ordered reverse rules, from legacy `ANSI 1 to Unicode` v1.12) carry
  per-file provenance (`source`, `source_version`, `developer`,
  `developer_contact`, `github_link`) and the license note
  "owner-supplied legacy data, publicly mirrored". Only the legacy
  `keyMap` order and entries were extracted; no other legacy-app metadata
  was copied.
- `crates/okkhor-lang/assets/encodings/bijoy-keyboard-v1.json` (111 rules,
  `Bangla to Bijoy layout`) is a **keyboard layout**, kept for reference
  only and never used as the output encoder.
- `assets/layouts/khipro-v1.json` (288 rules) and
  `assets/layouts/phonetic-v1.json` (256 rules) carry per-file provenance
  and the same license note.

## Runtime dependencies with license relevance

- `fancy-regex` 0.20 (MIT, pure Rust, no native code) powers the
  SutonnyMJ converter: the ANSI 1 tables require negative lookbehind
  (`(?<!্)`) and `\1`/`\2` backreferences, which the `regex` crate omits.
  Replacements are expanded manually from match captures, so table text can
  never be misread as engine directives.
