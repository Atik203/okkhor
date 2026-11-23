# Third-party licenses

This file records the provenance and license of third-party material that
Okkhor stages, bundles, or derives from at build time. No third-party
binaries are committed to this repository; fonts are staged into
`assets/fonts/` (git-ignored) by `scripts/fetch-fonts.ps1`, and layout /
encoding data is imported from owner-supplied JSON (see
`crates/okkhor-lang/assets/`).

## Fonts

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

### Kalpurush / Kalpurush ANSI — SIL Open Font License 1.0

- **Copyright/design:** Tanbin Islam Siyam, Avro Font Development Project,
  http://www.omicronlab.com (per the fonts' name-table records;
  Kalpurush declares OFL 1.0, Version 0.258).
- **License text:** `assets/fonts/OFL-Kalpurush.txt`.
- Kalpurush is the Unicode UI/editor font; Kalpurush ANSI is the fallback
  Bijoy-ANSI rendering font.

### Noto Sans Bengali — SIL Open Font License 1.1

- **Copyright:** The Noto Project Authors
  (https://github.com/notofonts/bengali).
- **License text:** `assets/fonts/OFL-NotoSansBengali.txt`
  (verbatim from the `google/fonts` `ofl/notosansbengali` directory).
- Unicode fallback font. Variable font build; family name unchanged.

## Layout / encoding data

- `crates/okkhor-lang/assets/encodings/bijoy-v1.json` (111 ordered rules),
  `assets/layouts/khipro-v1.json` (288 rules), and
  `assets/layouts/banglish-v1.json` (256 rules) carry per-file provenance
  (`source`, `developer`, `github_link`) and the license note
  "owner-supplied legacy data, publicly mirrored".
