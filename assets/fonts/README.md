# Okkhor fonts

Font binaries are **not** committed to this repository. The release build
stages them with `scripts/fetch-fonts.ps1` (pinned URLs + SHA256, fails
closed; SutonnyMJ is copied from `OKKHOR_LEGACY_HOME/Res/Fonts` when
present) into this directory, which is git-ignored (`*.ttf`). The daemon
installs them per user at runtime via `okkhor-win::fonts`
(`%LOCALAPPDATA%\Microsoft\Windows\Fonts` + `HKCU\...\Fonts`); no admin
rights are needed and nothing is written outside the user's profile.

Bijoy rendering resolution (`okkhor-win::fonts::resolve_bijoy_font`):
**SutonnyMJ** when staged, **Kalpurush ANSI** as fallback, typed
non-fatal missing otherwise — the app keeps working on Unicode paths.

## Bundled fonts

| Family | File | Role | License | Source | SHA256 |
| --- | --- | --- | --- | --- | --- |
| SutonnyMJ | `SutonnyMJ Regular.ttf` | `bijoy-ansi` — **default Bijoy rendering** | Proprietary, © Mustafa Jabbar, Ananda Computers (owner-approved for release builds; binary never committed, staged from `OKKHOR_LEGACY_HOME`) | Owner's legacy installation (`OKKHOR_LEGACY_HOME/Res/Fonts`) | `D58D9A45311447301426649C641FE148010163594AE57A4094AC086B21D52BDC` |
| Kalpurush | `kalpurush.ttf` | `unicode-ui` — Unicode UI/editor | SIL OFL 1.0 (declared in the font name table; text in `OFL-Kalpurush.txt`) | https://www.omicronlab.com/download/fonts/kalpurush.ttf | `83613A03E53AAD8337F2BE9FB974409A23C1D99B3B4B494567BA9D156C492B01` |
| Kalpurush ANSI | `kalpurush ANSI.ttf` | `bijoy-ansi` — Bijoy-ANSI fallback when SutonnyMJ is absent | SIL OFL 1.0 (same project; text in `OFL-Kalpurush.txt`) | https://www.omicronlab.com/download/fonts/kalpurush%20ANSI.ttf | `BFB06B2360A558424C61405A29C24B09C70487757204814C6DD04621A2A9EEBC` |
| Noto Sans Bengali | `NotoSansBengali.ttf` | `fallback` — Unicode fallback | SIL OFL 1.1 (text in `OFL-NotoSansBengali.txt`) | https://raw.githubusercontent.com/google/fonts/main/ofl/notosansbengali/NotoSansBengali%5Bwdth%2Cwght%5D.ttf (variable font) | `dcd42978094e584a849c84a51450eeac40c8826057d566ea6d4b9627a403a05a` |

Attribution: Kalpurush / Kalpurush ANSI — Tanbin Islam Siyam, Avro Font
Development Project, http://www.omicronlab.com (per the fonts' name-table
copyright and designer records; Kalpurush declares OFL 1.0, Version 0.258).
Noto Sans Bengali — The Noto Project Authors
(https://github.com/notofonts/bengali), OFL 1.1.

Sizes at time of pinning: `kalpurush.ttf` 314592 bytes,
`kalpurush ANSI.ttf` 113300 bytes, Noto variable font 463668 bytes. The
fetch script verifies length and hash and refuses to continue on mismatch.

## Optional fonts (never fetched or committed by default)

| Font | Status | Note |
| --- | --- | --- |
| Siyam Rupali | Optional, user-installed | GPL-licensed. Listed only as a user-installed option to avoid GPL distribution concerns; not bundled. |

## License texts

- `OFL-Kalpurush.txt` — SIL Open Font License 1.0 (as declared by the
  Kalpurush name table; canonical text via SPDX).
- `OFL-NotoSansBengali.txt` — SIL Open Font License 1.1 (verbatim from the
  `google/fonts` `ofl/notosansbengali` directory).
