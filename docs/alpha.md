# Okkhor 0.5 alpha — scope, build/run, limits

Internal portable alpha of the merged daemon. No UI, no quick bar, no F9
toggle, no IPC wiring yet — this build proves the commit path, the Bijoy
path, per-user fonts, and repeatable packaging.

## Scope

In scope for the alpha:

- `okkhor daemon [--encoding unicode|bijoy]` — resident composition with
  hook, pause hotkey (`Ctrl+Alt+Shift+P`), and tray (ADR 0013, ADR 0014).
- Unicode (default) and Bijoy commit paths. Bijoy converts once at commit
  with `okkhor_lang::bijoy::to_bijoy`; the trailing space stays ASCII; a
  conversion failure injects Unicode and logs the allowlisted
  `encoding_fallback` code (never text).
- `okkhor fonts status|install|uninstall <name>` — per-user install into
  `%LOCALAPPDATA%\Microsoft\Windows\Fonts` + `HKCU\...\Fonts`, no admin,
  idempotent, reversible. Fonts resolve from `<exe-dir>\fonts` first, else
  the dev checkout's `assets/fonts`. Bijoy resolution: SutonnyMJ, else
  Kalpurush ANSI, else typed non-fatal none (Unicode keeps working).
- `okkhor self-check`, `okkhor autostart <enable|disable|status>`.
- `scripts/build-alpha.ps1` — repeatable release build + portable ZIP.

Explicitly out of scope: Tauri UI, quick bar, F9 toggle, IPC, speech/AI,
learned model, installer/uninstaller, clipboard fallback, elevated-window
injection.

## Build / run

Prerequisites: stable Rust toolchain (MSVC for shipping; the repo's GNU
checkout builds too), Windows 11 x64.

```powershell
# checks (fmt, clippy, tests, self-check)
.\scripts\check.ps1

# stage fonts (pinned URLs + SHA256; SutonnyMJ from OKKHOR_LEGACY_HOME)
.\scripts\fetch-fonts.ps1

# portable artifact (versioned dir + zip + SHA256SUMS.txt)
.\scripts\build-alpha.ps1 -Version 0.5.0-alpha.1
```

Run from the extracted artifact:

```powershell
.\okkhor.exe fonts status
.\okkhor.exe fonts install
.\okkhor.exe daemon
.\okkhor.exe daemon --encoding bijoy
```

`BUILD-INFO.json` in the artifact records version, git commit, UTC date,
`rustc` version, and target triple.

## Known limits

- No F9 or tray toggle for Bangla/English yet; pause to type English.
- Single-candidate lists from the live table in most cases.
- Table-exact transliteration wins over dictionary intuition; dictionary
  correction (আপনি, ধন্যবাদ) is a later milestone.
- No clipboard fallback; elevated targets fail explicitly, never silently.
- Unsigned build: expect a SmartScreen warning; internal distribution only.
- Portable: delete the folder to remove; fonts/logs persist per-user until
  removed explicitly.

## QA pointer

Release rule: **no tag and no public artifact until every P0 row passes.**
The matrix is `docs/qa/0.5-alpha-checklist.md` — record build commit, OS
build, tester, and date per run. Automated rows (fmt, clippy, tests,
self-check) are covered by CI and `scripts/check.ps1`; interactive rows
need a real keyboard on a real Windows desktop.
