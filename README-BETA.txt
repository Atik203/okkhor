Okkhor 0.6.0 beta - Windows 11 x64 portable
===========================================

This is an UNSIGNED beta. It has no installer and no auto-update. It is not
yet 1.0: speech, AI actions, the learned reranker, and installers are later
milestones. Everything runs per-user; no admin is required.

1. EXTRACT
----------
Unzip the folder anywhere you can write, e.g. Desktop or Documents. Keep the
folder layout:

  Okkhor-0.6.0-win64/
    Okkhor.exe          THE APP - double-click this one
    bin/
      okkhor-engine.exe resident input engine (started by Okkhor.exe)
    fonts/              Kalpurush, Kalpurush ANSI, Noto Sans Bengali, SutonnyMJ
    LICENSES/           third-party notices and font licenses
    okkhor.ico          app icon
    README.txt          this file
    BUILD-INFO.json     exact version, commit, date, hashes

Okkhor.exe is the only thing you need to run. It starts the typing engine in
the background (no console window) and then shows Settings. If the engine is
already running it simply reconnects.

2. WINDOWS SMARTSCREEN
----------------------
Because this beta is not code-signed yet, Windows may show "Windows protected
your PC" the first time you run it:

  More info  ->  Run anyway

That warning is expected for unsigned software and disappears once 1.0 ships
with an Authenticode signature. If you prefer to verify the download first,
compare the zip hash with SHA256SUMS.txt:

  Get-FileHash .\Okkhor-0.6.0-win64-portable.zip -Algorithm SHA256

The value must match SHA256SUMS.txt exactly. Only trust a zip that matches.

3. INSTALL FONTS (per-user, no admin)
-------------------------------------
From a terminal in this folder (the engine keeps its command line in bin\):

  .\bin\okkhor-engine.exe fonts status
  .\bin\okkhor-engine.exe fonts install

`fonts install` copies the bundled fonts into your profile and registers them
under HKCU. It is idempotent. SutonnyMJ is preferred for Bijoy output;
Kalpurush ANSI is the fallback. To remove one later:

  .\bin\okkhor-engine.exe fonts uninstall "SutonnyMJ Regular.ttf"

4. START
--------
Double-click Okkhor.exe. That is all.

Optional:
  Okkhor.exe --quickbar        # open just the small always-on-top bar
  bin\okkhor-engine.exe daemon # start only the engine (tray + typing)
  bin\okkhor-engine.exe shutdown

The engine creates a per-user IPC token under
%LOCALAPPDATA%\Okkhor\runtime\ipc.token; the UI reads it automatically.

5. USE
------
- Type Banglish and press Space to commit Bangla. The caret list shows eight
  rows by default (up to ten); digits 1-9 and 0 select a row, arrows move,
  Backspace edits, Esc keeps the roman text. Curated corrections, the direct
  roman dictionary, and the Avro word list all feed the same ranked list.
- In English mode, three letters open a quiet completion list. Tab accepts the
  highlighted word, a click accepts any row, Esc hides it. Space, Enter, and
  punctuation never replace what you typed.
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
- Settings > Output encoding: Unicode (browsers/editors) or Bijoy (official
  forms; select SutonnyMJ in the document).
- Settings > Words, Keyboard, Show quick bar, theme, font scale, autostart.
- Closing Settings leaves the engine (and typing) running; quit from the tray
  or the bar's power button.

6. WHAT IS IN 0.6.0
-------------------
- Single-entry-point launch: Okkhor.exe starts the engine automatically, so
  the read-only "engine is not running" state only appears if startup fails.
- Commit keys (Space/Enter/digits/arrows) are captured while composing, so the
  roman word is replaced exactly: `amar` + Space gives `আমার `.
- The suggestion list follows the text caret in browsers and editors (UI
  Automation), including Chrome, Edge, Slack, and VS Code.
- Bangla Unicode typing: `.` gives the Bengali danda `।`, and digits `0`-`9`
  give `০`-`৯`; press F9 for English when you need ASCII punctuation or
  numbers. Both stay ASCII in English and Bijoy.
- The quick bar is visible on first launch (it was hidden by a startup race).
- 0.5 features: Avro phonetic engine (250 rules), Khipro and Banglish layouts,
  Unicode or SutonnyMJ/Bijoy output, encrypted word store, Tauri settings UI,
  on-screen keyboard, caret suggestion popup, daemon IPC, and the quick bar.

Bijoy note: Bijoy output is ANSI text meant for documents that use the
SutonnyMJ font. A browser tab or modern editor without SutonnyMJ shows Latin
letters; use Unicode there. The font is bundled and can be installed with
`bin\okkhor-engine.exe fonts install`.

7. KNOWN LIMITS (read before filing bugs)
-----------------------------------------
- UNSIGNED: SmartScreen will warn. Do not distribute outside the test group.
- No installer/uninstaller; delete the folder to remove the app. Per-user
  fonts remain until `fonts uninstall`.
- The first popup in a new window may take one extra frame while UI Automation
  resolves the caret; the latest-wins caret thread is a 0.6.1 polish item.
- No installer, updater, speech, or AI rewrite yet (1.0 / later).
- Autostart launches the engine directly; it is a console subsystem binary,
  so a console window can flash at sign-in. Use the tray instead if that
  bothers you until 1.0.
- Real-key QA rows are still being executed; see docs/qa/ in the repository.

8. VERSION IDENTITY
-------------------
`.\bin\okkhor-engine.exe --version` must print `okkhor 0.6.0`, matching
BUILD-INFO.json and the folder name. If it does not, stop and rebuild from
the tag.

9. REPORTING
------------
Include BUILD-INFO.json, `winver`, the exact keys pressed, and the relevant
redacted log lines from %LOCALAPPDATA%\Okkhor\logs. Never paste private text.
