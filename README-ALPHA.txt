Okkhor 0.5.0-alpha.1 — portable internal alpha (Windows 11 x64)
==================================================================

This is an INTERNAL test build. It is unsigned, has no installer, no UI,
and no auto-update. Everything runs per-user; nothing needs admin rights.

1. EXTRACT
----------
Unzip the portable folder anywhere you can write, e.g. Desktop or
Documents. Keep the folder layout intact:

  Okkhor-<version>-win64/
    okkhor.exe            the daemon (run from a terminal)
    fonts/                staged font files (may be empty, see below)
    okkhor.ico            app icon (cosmetic)
    THIRD_PARTY_LICENSES.md
    OFL-*.txt             font license texts (when present)
    README-ALPHA.txt      this file
    BUILD-INFO.json       exact build identity (version, commit, date)

2. INSTALL FONTS (per-user, no admin)
--------------------------------------
From a terminal inside the extracted folder:

  .\okkhor.exe fonts status     read-only check: staged files, roles,
                               per-user install state, Bijoy resolution
  .\okkhor.exe fonts install   copy staged fonts to your own profile
                               (%LOCALAPPDATA%\Microsoft\Windows\Fonts)
                               and register them under HKCU; idempotent —
                               run it again any time, it only fills gaps

To remove one font later:

  .\okkhor.exe fonts uninstall kalpurush.ttf

3. RUN THE DAEMON
-----------------
Unicode output (default, for browsers, editors, chat):

  .\okkhor.exe daemon

Bijoy output (legacy ANSI bytes for official forms; needs a
Bijoy-compatible font such as SutonnyMJ selected in the document):

  .\okkhor.exe daemon --encoding bijoy

While it runs:
  - Type Banglish (roman letters) and press Space to commit Bangla.
  - Keys 1-5 pick a numbered candidate; arrows move; Esc keeps the
    roman text; Backspace edits the roman buffer.
  - Ctrl+Alt+Shift+P pauses/resumes capture (tray icon matches).
  - Right-click the tray icon for pause/exit.

Check the engine without typing:

  .\okkhor.exe self-check       should print: self-check: ok

4. WHAT WORKS IN THIS ALPHA
----------------------------
  - Unicode commit path (erase + inject in one batch).
  - Bijoy commit path (--encoding bijoy; trailing space stays ASCII;
    conversion failure falls back to Unicode and is logged as a code).
  - Per-user font install / status / uninstall; SutonnyMJ preferred,
    Kalpurush ANSI fallback, typed non-fatal "none available".
  - Pause hotkey + tray; single-instance guard; redacted JSON-lines logs
    under %LOCALAPPDATA%\Okkhor\logs (no text is ever logged).
  - Autostart enable/disable/status (per-user login entry).

5. HONEST LIMITATIONS (read before filing bugs)
-----------------------------------------------
  - No UI, no quick bar, no F9 Bangla/English toggle yet. The daemon
    starts in Bangla composition; pause with the hotkey to type English.
  - One top candidate per buffer: digits/arrows work against the live
    list, which the current table usually fills with a single form.
  - Table-exact forms win: where the phonetic table differs from
    intuition (e.g. dictionary words like "apni"/"dhonnobad"), the table
    form is committed. Dictionary correction is a later milestone.
  - No clipboard fallback and no elevated-window support: injection
    into an elevated (admin) window fails explicitly instead of
    corrupting text; retype after focusing a normal window.
  - The build is UNSIGNED: Windows SmartScreen will warn. This is
    expected for an internal alpha; do not distribute outside the team.
  - Portable only: there is no installer and no uninstaller. Delete the
    folder to remove the program; per-user fonts stay until you run
    `fonts uninstall`, and logs stay under %LOCALAPPDATA%\Okkhor.
  - See docs/alpha.md for scope and docs/qa/0.5-alpha-checklist.md for
    the release QA matrix (no tag ships until every P0 row passes).

6. REPORTING A PROBLEM
-----------------------
Include: BUILD-INFO.json (version + commit), Windows build (winver),
the exact keys typed, and the relevant log lines. Never paste private
text you typed — logs contain none, and neither should the report.
