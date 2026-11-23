# Okkhor fonts

Font binaries are **not** committed to this repository. The release build
fetches them with `scripts/fetch-fonts.ps1` (pinned URLs + SHA256, fails
closed) into this directory, which is git-ignored (`*.ttf`). The daemon
installs them per user at runtime via `okkhor-win::fonts`
(`%LOCALAPPDATA%\Microsoft\Windows\Fonts` + `HKCU\...\Fonts`); no admin
rights are needed and nothing is written outside the user's profile.

## Bundled fonts

| Family | File | Role | License | Source | SHA256 |
| --- | --- | --- | --- | --- | --- |
| Kalpurush | `kalpurush.ttf` | `unicode-ui` — Unicode UI/editor | SIL OFL 1.0 (declared in the font name table; text in `OFL-Kalpurush.txt`) | https://www.omicronlab.com/download/fonts/kalpurush.ttf | `83613A03E53AAD8337F2BE9FB974409A23C1D99B3B4B494567BA9D156C492B01` |
| Kalpurush ANSI | `kalpurush ANSI.ttf` | `bijoy-ansi` — Bijoy-ANSI rendering (default Bijoy font) | SIL OFL 1.0 (same project; text in `OFL-Kalpurush.txt`) | https://www.omicronlab.com/download/fonts/kalpurush%20ANSI.ttf | `BFB06B2360A558424C61405A29C24B09C70487757204814C6DD04621A2A9EEBC` |
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
| SutonnyMJ | Optional, user-installed | Proprietary. Okkhor detects it if installed and offers it as a Bijoy font choice, but never distributes it. The Bijoy byte mapping is identical to Kalpurush ANSI, so official Bijoy forms work with either; only the selected rendering font differs. |
| Siyam Rupali | Optional, user-installed | GPL-licensed. Listed only as a user-installed option to avoid GPL distribution concerns; not bundled. |

## License texts

- `OFL-Kalpurush.txt` — SIL Open Font License 1.0 (as declared by the
  Kalpurush name table; canonical text via SPDX).
- `OFL-NotoSansBengali.txt` — SIL Open Font License 1.1 (verbatim from the
  `google/fonts` `ofl/notosansbengali` directory).
