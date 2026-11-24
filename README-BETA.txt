Okkhor 0.9.0 beta - Windows 11 x64 portable
===========================================

This is an UNSIGNED beta unless the release owner signed it with a trusted
Authenticode certificate. It has no installer and no auto-update. It is not
yet 1.0: the installer, updater, and a signed release are later milestones.
Everything runs per-user; no admin is required. The build uses the MSVC
toolchain, the production Windows toolchain, and links the C runtime
statically: no separate Microsoft Visual C++ Redistributable is needed.

1. EXTRACT
----------
Unzip the folder anywhere you can write, e.g. Desktop or Documents. Keep the
folder layout:

  Okkhor-0.9.0-win64/
    Okkhor.exe          THE APP - double-click this one
    bin/
      okkhor-engine.exe resident input engine (started by Okkhor.exe)
      okkhor-worker.exe isolated AI worker (spawned only when AI is used)
    fonts/              Kalpurush, Kalpurush ANSI, Noto Sans Bengali, SutonnyMJ
    LICENSES/           third-party notices and font licenses
    okkhor.ico          app icon
    README.txt          this file
    BUILD-INFO.json     exact version, commit, date, hashes

Okkhor.exe is the only thing you need to run. It starts the typing engine in
the background (no console window) and then shows Settings. If the engine is
already running it simply reconnects.

2. WINDOWS SMARTSCREEN AND SIGNATURE
------------------------------------
This build is code-signed only if the release owner attached a trusted
Authenticode certificate. An unsigned build makes Windows show "Windows
protected your PC" the first time you run it:

  More info  ->  Run anyway

That warning is about the missing signature and download reputation, not a
detected bug in Okkhor, and it disappears only for a build signed with a
trusted code-signing certificate. A self-signed certificate does not help on
other machines.

Before running it, verify the download. Compare the zip hash with
SHA256SUMS.txt:

  Get-FileHash .\Okkhor-0.9.0-win64-portable.zip -Algorithm SHA256

The value must match SHA256SUMS.txt exactly, character for character. Only
trust a zip that matches.

If this is a signed build, also check the signature:

  Get-AuthenticodeSignature .\Okkhor.exe | Format-List Status, SignerCertificate

Status must be Valid and the signer must be the expected publisher. An
"Insecure" or "NotSigned" result means the file was modified or this is an
unsigned beta.

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
- Type phonetic English and press Space to commit Bangla. The caret list shows eight
  rows by default (up to ten); digits 1-9 and 0 select a row, arrows move,
  Backspace edits, Esc keeps the roman text.
- In English mode, three letters open a quiet completion list. Tab accepts the
  highlighted word, a click accepts any row, Esc hides it. Space, Enter, and
  punctuation never replace what you typed. Password fields are suppressed.
- **F9** toggles Bangla/English and plays a short click (disable in Settings
  > Hotkeys).
- Tray: left-click toggles language; right-click opens the menu.
- **Ctrl+Alt+Shift+P** pauses/resumes capture.
- Quick bar: language, output encoding (Unicode/Bijoy), pause, on-screen
  keyboard, settings, hide to tray, quit. It never takes focus.
- Settings > Words, Keyboard, Show quick bar, theme, font scale, autostart.
- Closing Settings leaves the engine (and typing) running; quit from the tray
  or the bar's power button.

6. AI REWRITE (OPT-IN)
----------------------
Settings > AI is off by default. To use it:

1. Set an OpenAI key in your user environment:
     setx OKKHOR_OPENAI_API_KEY "sk-..."
   or put `OKKHOR_OPENAI_API_KEY=sk-...` in a local `.env` file next to
   Okkhor.exe (or start Okkhor from a folder that has one). Never commit it.
2. Optionally set the model:
     setx OKKHOR_AI_MODEL "gpt-6-luna"
3. Restart Okkhor, enable AI in Settings, Save, then paste text into the AI
   card and click **Try a rewrite**.

The default `gpt-6-luna` costs roughly $0.0002 for a short rewrite (checked
2026-11-24). It is a reasoning model and accepts only the default temperature
of 1.0; Okkhor sends that. `gpt-6.1-terra` is the quality option. The panel shows only
whether a key is set; it never displays the key. Prompts and responses are
memory-only and are never logged or inserted automatically.

7. WHAT IS IN 0.9.0
-------------------
- Suggestions appear from the FIRST character. Dictionary-backed rows are no
  longer skipped for one-letter prefixes, and English completions no longer
  wait for a three-letter token.
- Backspace keeps the list alive. Returning to a shorter prefix restores the
  exact same rows from the bounded prefix cache, forwards and backwards.
- The popup stays put and does less work. Identical rows and selection cause
  no clone, no text measurement, no window move and no repaint; repeated row
  widths are cached. A real change still repaints once, coalesced to 30 ms.
- A blank word can never replace a typed word: a commit whose text is empty or
  only invisible characters is rejected instead of erasing the roman buffer.
- All layouts now change typing output. Avro, Phonetic and Khipro use one
  phonetic engine with different compiled rule tables; the selector offers a
  single default "Avro (Phonetic)" style, while `phonetic` and `khipro` remain
  valid saved settings. National/Jatiya, Probhat and Bijoy are fixed
  physical-key layouts with Shift/AltGr and conjunct sequences. In fixed mode
  the raw letter is swallowed, the popup stays hidden, and output encoding
  (including Bijoy) still applies. Changing layout in Settings takes effect
  immediately; no restart is needed.
- The first letter no longer disappears in browsers: a child-window swap in
  the same process no longer resets the composition. The English popup no
  longer flickers while a caret sample is pending. The card sits closer under
  the caret. The tray uses Okkhor's own icon, and the settings UI connects
  sooner after launch.

0.8.1 features remain:
- Stable suggestion popup: updated in place instead of re-shown, painted
  through a memory DC, coalesced to one visible update per 30 ms, and latched
  through a transient caret-sample miss.
- Recency is frozen for one composition, a bounded prefix cache restores the
  exact rows after backspace-and-retype, and arrow selection follows the same
  row across later keystrokes.
- Raw Latin fallback: Bangla mode keeps the typed Latin token as the last row,
  so unknown phonetic words such as `also` are selectable without a rule guess.

0.7 features remain:
- Dedicated latest-wins caret thread: typing no longer waits on UI Automation;
  the popup follows a newer caret without repainting.
- English popup waits for a fresh caret/password sample before showing, so a
  previous field cannot leak a completion into a password box.
- Shipped `okkhor-features` and `okkhor-model` crates: one v1 feature schema and
  a checked, embedded Tier 1 reranker. `--ranker baseline` remains the default;
  `--ranker hybrid` is opt-in and falls back exactly on error.
- Opt-in AI rewrite: isolated `okkhor-worker.exe`, OpenAI provider, job
  poll/cancel/status, Settings panel, no automatic insertion.
- Dataset manifest v1 + fail-closed verifier + deterministic logistic trainer.
- 10,000-event session stress test and the `scripts/qa-0.7.ps1` gate.
- 0.6 features remain: full local dictionary, eight rows, English completions,
  instant quick-bar state, encrypted learning, Avro/Khipro/Phonetic layouts,
  Unicode/Bijoy output, settings, on-screen keyboard, tray, and quick bar.

Bijoy note: Bijoy output is ANSI text meant for documents that use the
SutonnyMJ font. A browser tab or modern editor without SutonnyMJ shows Latin
letters; use Unicode there.

8. KNOWN LIMITS (read before filing bugs)
-----------------------------------------
- UNSIGNED unless the owner signed this build: SmartScreen will warn; a trusted
  Authenticode certificate is required to remove it. Do not distribute outside
  the test group.
- No installer/uninstaller; delete the folder to remove the app.
- The hybrid reranker is trained on a synthetic replay only and is default-off;
  it makes no real-world quality claim. Real-corpus A/B is owner-gated.
- AI is opt-in and needs a key; without one the panel reports "key not set".
  A provider outage or worker crash never affects typing.
- Windows IME/RDP/elevated/DPI rows are manual; see docs/qa/0.7-matrix.md.
- Real-key QA is the owner's pass because the hook ignores injected keys.

9. VERSION IDENTITY
-------------------
`.\bin\okkhor-engine.exe --version` must print `okkhor 0.9.0`, matching
BUILD-INFO.json and the folder name. If it does not, stop and rebuild from the
tag.

10. REPORTING
-------------
Include BUILD-INFO.json, `winver`, the exact keys pressed, and the relevant
redacted log lines from %LOCALAPPDATA%\Okkhor\logs. Never paste private text,
prompts, or keys.
