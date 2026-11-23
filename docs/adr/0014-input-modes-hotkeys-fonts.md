# ADR 0014: Input modes, one-key toggle, and zero-config fonts

- Status: Accepted
- Date: 2026-11-23
- Relates to: ADR 0004 (UI), ADR 0012 (UI process and settings), ADR 0013
  (encodings and fonts)

## Context

Okkhor must be the easiest Bangla keyboard to start using: install, type, done.
Users should not have to choose fonts, open settings, or remember a chord before
their first sentence. At the same time, English typing must be one action away,
and Bijoy output must be available for official forms.

A single global function key is the requested interaction model. Function keys
are heavily used by applications (F11 fullscreen, F12 devtools, F5 refresh,
F10 menus), so the default must be a key that is rarely claimed and must be
configurable.

## Decision

### 1. Two input modes, one output encoding choice

- `input_mode`: `bangla` (phonetic composition) or `english` (pass-through).
  Default after install: `bangla`.
- `output_encoding`: `unicode` (default) or `bijoy` (legacy ANSI bytes rendered
  with Kalpurush ANSI / SutonnyMJ).
- Input mode and output encoding are independent: Banglish → Unicode and
  Banglish → Bijoy are both first-class.
- The last mode and encoding persist across restarts. Composition state never
  persists.

### 2. Default hotkeys

| Action | Default | Notes |
| --- | --- | --- |
| Toggle Bangla/English | **F9** | single key, global; configurable |
| Pause/resume capture | **Ctrl+Alt+Shift+P** | existing behavior; configurable |
| Cancel composition | **Esc** | in-app key, not global |
| Commit | **Space / Enter / 1–5** | composition only |

F9 is the default because F11 conflicts with fullscreen in browsers and F10/F12
are commonly claimed by applications. F9 is not free in every app (e.g. mail
send), so startup conflict detection is mandatory: if `RegisterHotKey` fails,
fall back to `Ctrl+Space`, show a one-time notification, and keep the setting
editable.

No default global hotkey is assigned to output encoding; it lives in the tray
menu and UI to avoid stealing a second function key.

### 3. Feedback and controls

- Toggling shows a non-activating toast (`বাংলা` / `English`) for ~800 ms and
  updates the tray tooltip; no sound by default.
- Tray left-click toggles the input mode. Tray right-click menu:
  Bangla/English, Output Unicode/Bijoy, Pause, Settings, Exit.
- The daemon UI (Tauri) exposes the same controls; no hidden state.
- Popup and toast never take focus.

### 4. Zero-config fonts

- On first run, install per-user (no admin) **Kalpurush** (Unicode),
  **SutonnyMJ** (default Bijoy, staged from the owner's licensed
  installation), and **Kalpurush ANSI** (Bijoy fallback). **Noto Sans
  Bengali** is the fallback.
- Defaults are selected automatically; there is no font picker on first run.
- Advanced settings may change the Unicode font and the Bijoy font:
  **SutonnyMJ** is the default when staged or already installed (detected),
  **Kalpurush ANSI** otherwise. SutonnyMJ is staged at build time from the
  owner's licensed installation and never committed to git.
- If per-user font installation fails, load the fonts privately for Okkhor's
  own windows, keep Noto Sans Bengali as the fallback, and notify once. Never
  block typing on a font problem.
- Font install is idempotent and reversible from Settings → Fonts.

### 5. Quick control bar

- A compact **native** bar owned by the daemon (not the Tauri UI), so it remains
  available when the settings window is closed or crashed.
- Default: a 300×40 px pill, top-right of the primary monitor, always-on-top,
  never activating, no taskbar entry, ~90% opacity, 8 px corners.
- Contents: mode chip (`বাংলা`/`EN`), encoding chip (`ইউনিকোড`/`বিজয়`), pause,
  on-screen keyboard, settings, collapse. Microphone is added in 0.9.
- Clicking the mode chip toggles Bangla/English; the encoding chip cycles
  Unicode/Bijoy; settings/OSK open the Tauri UI at that route; pause toggles
  capture; collapse shrinks the bar to a 40×40 orb.
- The bar is draggable from its background (not the buttons), snaps to screen
  edges, remembers position, and follows the active monitor's DPI.
- Double-clicking the orb restores the bar; right-clicking shows the tray menu.
- The bar never takes focus and never shows user text.
- Theme follows system light/dark plus the accent; Bangla labels use Kalpurush.
- Settings: `quickbar_visible` (default true), `quickbar_collapsed` (false),
  `quickbar_x`/`quickbar_y` (null = auto), `quickbar_opacity` (0.5–1.0).

### 6. Failure behavior

| Failure | Behavior |
| --- | --- |
| Hotkey already registered | fall back to `Ctrl+Space`, notify once, keep setting editable |
| Font copy/registration fails | private font for Okkhor UI, Noto fallback, notify once |
| SutonnyMJ absent | Kalpurush ANSI is used; no prompt |
| Daemon not running | UI shows a banner and opens a "Start Okkhor" action |
| UI closed/crashed | typing and hotkeys keep working |

### 6. Accessibility

- Mode and encoding are exposed as text, not color alone.
- All tray/UI actions are reachable by keyboard in the Tauri window.
- The toast is announced through `aria-live` in the UI and never steals focus
  on the desktop.
- High-contrast and 100–200% DPI are part of the UI QA matrix.

## Consequences

Positive:

- first-run success without configuration;
- one key to switch languages, one click for Bijoy;
- no dependency on a proprietary font for the default path;
- consistent behavior between tray, hotkey, and UI.

Negative:

- a global F9 can collide with an application; mitigated by conflict
  detection and fallback;
- two font families to ship and register;
- mode state must be visible enough that users do not type into the wrong mode.

## Alternatives considered

- **F11 default.** Rejected: fullscreen conflict in browsers and media apps.
- **Alt+Shift / Win+Space.** Rejected: conflicts with Windows language/IME
  switching and is a chord, not a single key.
- **Font picker on first run.** Rejected: adds friction and a decision users
  are not equipped to make.
- **SutonnyMJ bundled by default.** Adopted 2026-11-23 by owner decision:
  SutonnyMJ ships in release artifacts (staged from the owner's licensed
  installation, never committed) as the default Bijoy font, with Kalpurush
  ANSI as fallback; see the ADR 0013 amendment.
- **Separate Bijoy-only mode with automatic font switching in the target app.**
  Rejected for 1.0: an external app's font cannot be changed reliably; the user
  chooses the font in the document, Okkhor guarantees the bytes.
