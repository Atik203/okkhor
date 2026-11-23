Okkhor 0.5.0 beta — Windows 11 x64 portable
===========================================

This is an UNSIGNED beta. It has no installer and no auto-update. It is not
yet 1.0: speech, AI actions, the learned reranker, and installers are later
milestones. Everything runs per-user; no admin is required.

1. EXTRACT
----------
Unzip the folder anywhere you can write, e.g. Desktop or Documents. Keep the
folder layout:

  Okkhor-0.5.0-win64/
    okkhor.exe          the resident input engine (tray + keyboard hook)
    okkhor-ui.exe       settings, on-screen keyboard, word manager, quick bar
    fonts/              Kalpurush, Kalpurush ANSI, Noto Sans Bengali, SutonnyMJ
    LICENSES/           third-party notices and font licenses
    okkhor.ico          app icon
    README.txt          this file
    BUILD-INFO.json     exact version, commit, date, hashes

2. INSTALL FONTS (per-user, no admin)
-------------------------------------
From a terminal in this folder:

  .\okkhor.exe fonts status
  .\okkhor.exe fonts install

`fonts install` copies the bundled fonts into your profile and registers them
under HKCU. It is idempotent. SutonnyMJ is preferred for Bijoy output;
Kalpurush ANSI is the fallback. To remove one later:

  .\okkhor.exe fonts uninstall "SutonnyMJ Regular.ttf"

3. START
--------
Start the engine first:

  .\okkhor.exe daemon

Then the UI and/or quick bar:

  .\okkhor-ui.exe --quickbar     # small always-on-top control pill
  .\okkhor-ui.exe                # full Settings window

The daemon creates a per-user IPC token under
%LOCALAPPDATA%\Okkhor\runtime\ipc.token; the UI reads it automatically.

4. USE
------
- Type Banglish and press Space to commit Bangla. Digits 1-9 and 0 select a
  candidate (up to ten); arrows move; Backspace edits; Esc keeps the roman
  text. The caret list offers a curated correction first, then alternatives.
- **F9** toggles Bangla/English and plays a short click (disable in Settings
  > Hotkeys). If F9 is already taken, Okkhor falls back to
  Ctrl+Space and tells you once.
- Tray: left-click toggles language; right-click opens the menu
  (language, pause, show quick bar, settings, exit). Double-click no longer
  pauses.
- **Ctrl+Alt+Shift+P** pauses/resumes capture.
- Quick bar: language, output encoding (Unicode/Bijoy), pause, on-screen
  keyboard, settings, hide to tray, quit. Drag the grip/spacer to move; drag
  the right edge to resize; double-click the grip to collapse. It never takes
  focus. Use the tray's Show quick bar to bring it back after hiding.
- Settings → Output encoding: Unicode (browsers/editors) or Bijoy (official
  forms; select SutonnyMJ in the document).
- Settings → Words, Keyboard, Show quick bar, theme, font scale, autostart.
- The quick bar is a UI-process window; if the UI exits, the bar disappears
  but typing keeps working.

5. WHAT IS IN 0.5
-----------------
- Avro phonetic engine from the legacy rule table (250 rules) + Khipro and
  Banglish layout data.
- Unicode or SutonnyMJ/Bijoy ANSI output (ANSI 1 table, render-verified).
- Encrypted word store (DPAPI) and settings store.
- Tauri settings UI, on-screen keyboard, word manager, quick bar.
- F9/tray language toggle, non-activating toast, pause hotkey.
- Daemon IPC: status, settings, words, insert, capture.
- Autostart enable/disable/status.

6. KNOWN LIMITS (read before filing bugs)
-----------------------------------------
- UNSIGNED: SmartScreen will warn. Do not distribute outside the test group.
- No installer/uninstaller; delete the folder to remove the app. Per-user
  fonts remain until `fonts uninstall`.
- One top candidate per buffer; dictionary-based correction (আপনি,
  ধন্যবাদ) lands after 0.5.
- No speech, AI rewrite, or learned reranker yet (0.6+).
- No clipboard fallback and no elevated-window injection; failures are
  explicit, never silent corruption.
- Quick bar position is primary-monitor top-right in 0.5; X/Y persistence,
  snap, and follow-monitor are follow-ups.
- Manual real-key QA rows are still being executed; see
  docs/qa/0.5-alpha-checklist.md in the repository.

7. VERSION IDENTITY
-------------------
`.\okkhor.exe --version` must print `okkhor 0.5.0`, matching BUILD-INFO.json
and the folder name. If it does not, stop and rebuild from the tag.

8. REPORTING
------------
Include BUILD-INFO.json, `winver`, the exact keys pressed, and the relevant
redacted log lines from %LOCALAPPDATA%\Okkhor\logs. Never paste private text.
