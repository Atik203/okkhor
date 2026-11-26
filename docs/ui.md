# Okkhor settings UI (0.5)

Separate-process settings window: **Tauri 2 + React 19 + TypeScript + Vite** in
`apps/okkhor-ui`. A UI crash must never stop typing — the daemon owns input,
this app only observes and configures it through IPC.

Binding contracts: `docs/design/0.5-ui-contracts.md` (§2 settings, §4
protocol, §5 UI behavior), ADR 0012 (UI process, IPC, settings ownership),
ADR 0013 (encodings/fonts), ADR 0014 (input modes, hotkeys, zero-config
fonts), ADR 0016 (suggestion index, English completions, instant state).

## 0.6 additions

- Settings is split into General, Typing, Suggestions, Appearance,
  Words & Learning, and About. The layout is two columns at >=1024 px and a
  single column below; every section is checked at 960, 1280, and 1600 px in
  light and dark.
- Suggestions: popup toggle, rows 5/8/10 (max 10), English completions toggle,
  and a "Tab accepts the highlighted word" hint.
- The inert AI/speech toggles are removed from the main page; the stored
  fields remain for forward compatibility.
- The quick bar listens for `okkhor:state-changed` and refetches its snapshot;
  the 5 s poll is a fallback only.
- Save stays explicit with a sticky footer; toggles and selects are still
  applied by Save so a half-edited page never reaches the daemon.

## Architecture

```text
WebView (React) ──invoke──▶ src-tauri (Rust commands) ──named pipe──▶ daemon
                              │  JSON frames, 1 MiB cap
                              │  hello { protocol: 1, token }
   reads %LOCALAPPDATA%\Okkhor\runtime\ipc.token for the handshake
```

- The daemon is the **single writer** of `settings.json`. The UI never writes
  the file directly — all reads/writes go through `settings.get` /
  `settings.update` IPC.
- Daemon absent: `Okkhor.exe` (this UI) starts the engine next to itself
  (`bin\okkhor-engine.exe`, or a sibling for dev/0.5.0 layouts) through the
  `daemon_ensure` command, then re-probes status. The engine stays a separate
  process: if it still cannot start, every Rust command returns typed
  `daemon_unavailable` and the WebView shows a persistent banner, renders
  read-only, and offers **Start Okkhor**. A missing `bin\` folder returns
  `engine_missing` with static layout text. `OKKHOR_ENGINE_PATH` overrides
  the engine path for dev/QA.
- No network at runtime: CSP is `default-src 'self'` plus the minimum Tauri
  needs (`ipc:`/`http://ipc.localhost` for invoke, `asset:`/`data:` images,
  local inline styles). No remote fonts, assets, or telemetry.
- The suggestion popup stays native Win32 (`okkhor-win`); the WebView never
  renders per-keystroke suggestions. The only always-on-top WebView is the
  second `quickbar` window (see "Quick bar" below); the settings window is a
  normal window.

## Routes

| Hash | Page | Notes |
| --- | --- | --- |
| `#/` | Status | version, paused toggle, mode, language/layout, encoding, word count, autostart, "UI is separate from typing" note |
| `#/settings` | Settings | every contract §2 field (incl. quickbar X/Y + Automatic toggle), live validation, dirty guard, Save/Reset, toasts |
| `#/keyboard` | On-screen keyboard | layout selector, Unicode key table + Shift layer, reorderable groups, favorites, session recents, bundled emoji search |
| `#/words` | Word manager | paginated table (50 default), search, add/remove, CSV/JSON import preview + export download |
| `#/quickbar` | Quick bar | compact 340×52 bar (see below); also auto-hosted by the `quickbar` window with no nav chrome |
| `#/about` | About | versions, data locations, privacy summary, licenses pointer |

Unknown hashes fall back to `#/` (status). First run shows no wizard and no
font picker: fonts install per-user in the background (Kalpurush default, F9
toggle) and the advanced Fonts section on the settings page stays collapsed.

## Build / dev / test

Requires Node 22+ and npm (the repo pins `package-lock.json`; never commit
`node_modules`, `dist`, or `src-tauri/target`).

```powershell
cd apps/okkhor-ui
npm ci
npm run dev      # Vite dev server (Tauri devUrl http://localhost:1420)
npm run build    # tsc + vite build -> dist/
npm test -- --run
```

Rust shell (its own workspace — root `cargo` commands never touch it):

```powershell
cd apps/okkhor-ui/src-tauri
cargo fmt --check
cargo clippy -- --deny warnings
cargo test
```

## Rust commands (`src-tauri/src/main.rs`)

`ipc_status`, `ipc_get_settings`, `ipc_update_settings`,
`ipc_list_layouts`, `ipc_insert_text`, `ipc_set_paused`,
`ipc_list_words`, `ipc_add_word`, `ipc_remove_word`,
`ipc_import_words`, `ipc_export_words` — all async, typed
`UiError { code, message }`, no `unwrap` on user paths.

## Quick bar (`#/quickbar`, second Tauri window)

- Per the ADR 0014 amendment (2026-11-23), the bar is the **`quickbar`
  window in this UI process** — not a native daemon window. Typing/daemon
  independence holds while the UI lives (the daemon never links UI code),
  but **the bar disappears if the UI process dies**, unlike a daemon window.
- Launch: `okkhor-ui --quickbar` shows only the bar and keeps `main` hidden
  until "Open settings" (`quickbar_open_settings` shows `main` and navigates
  it to `/settings` via `okkhor:navigate`). A normal launch shows `main`
  normally and additionally shows the bar when `quickbar_visible` is set
  (resolved after settings load). Both windows start `visible: false` so no
  frame can show before its styles are applied.
- No focus, always on top: `decorations: false`, `transparent: true`,
  `skipTaskbar: true`, `alwaysOnTop: true`, `focus: false`; on Windows the
  setup path applies `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST`
  to the bar HWND *before* showing it with `SW_SHOWNOACTIVATE`. Clicking
  chips, pause, keyboard, settings, or collapse never activates the window.
  The bar is the single approved exception to contract §5's no-always-on-top
  rule (lead ruling, 2026-11-23); no other UI window may be always-on-top.
- Geometry: fixed 340×52 (compact typing surface), collapsible to a 44×44 orb
  (double-click the grip to collapse; click the orb to expand). The bar is not
  resizable: a fixed size keeps the controls in one predictable place instead
  of letting the window grow into a control panel. The bar starts top-right of
  the primary monitor with per-monitor DPI scaling, or at the last saved drag
  position when it is still on a monitor.
- Moving: manual pointer-capture drag (`setPosition`) on the left grip and the
  empty spacer — *not* `data-tauri-drag-region`. Windows' modal move loop does
  not reliably start for a `WS_EX_NOACTIVATE` window; explicit positioning
  works and never activates the bar. Buttons never start a drag. The new
  position persists through `settings.update` (`quickbar_x`/`quickbar_y`).
  While mounted, the bar locks `html`/`body`/`#root` to 100% with
  `overflow: hidden` and a transparent background, so no scrollbar or bottom
  line can appear.
- Contents: mode chip (`বাংলা`/`EN`), pause, add-last-word, and settings;
  status dot (text-announced, never color-only) for daemon availability;
  centered notice pill on pause/add/error, two-second snapshot timeout when the
  daemon is unreachable. Everything rare (encoding, on-screen keyboard,
  hide-to-tray, quit) lives in Settings or the tray, so the bar stays a typing
  surface with five targets. Each notice renders exactly once and is announced
  once (`role="status"`). No text capture: the view renders static labels and
  never reads keystrokes. A short click plays on mode changes unless
  `toggle_sound` is off.
- Hide to tray: `quickbar_hide` hides the window and sets `quickbar_visible`
  to false; the process stays alive. The tray menu's **Show quick bar** signals
  a session-local named event (`Local\Okkhor.QuickBar.Show.v1`) so the running
  UI re-shows the bar no-activate (`quickbar_show`). If no UI is listening,
  the daemon launches the UI at the artifact root with `--quickbar` (sibling
  `okkhor-ui.exe` in dev/0.5.0 layouts).
- Quit lives in the tray and Settings; the bar no longer carries a power
  button next to the typing controls. The tray/quit path is unchanged:
  authenticated IPC `shutdown`, then the UI exits.
- Resize: removed. The bar is a fixed 340×52 compact surface; the old
  right-edge handle and its native `SetWindowPos` poll loop are gone, so no
  pointer loop can outlive the window.
- Settings: "Show quick bar" toggle and the 0.5–1.0 opacity slider write
  the existing `quickbar_*` fields through `settings.update`. Daemon down:
  mode/encoding/pause return `daemon_unavailable` (chips disable, banner
  rules apply) while hide-to-tray/quit/open-settings/open-keyboard stay local.
- Limitations: the bar cannot outlive the UI process; it does not
  auto-follow the active monitor while visible (it is re-homed only when
  shown off-screen); snap-to-edges is not implemented.
## Suggestion popup (native Win32, `okkhor-win`)

The caret-adjacent suggestion list is a native, custom-painted Win32 window
(`crates/okkhor-win/src/popup.rs`) — not WebView, not a `STATIC` text box.
The daemon feeds it `Vec<String>` candidates plus the highlight index; the
window owns sizing, painting, placement, and click queuing.

### List design

- Up to 10 rows (`popup_max_items`, 1-10; default 5). The Avro rule table
  yields one deterministic form per keystroke, so the session adds a curated
  starter list: an exact match first (corrects common spellings such as
  `dhonnobad` -> ধন্যবাদ), then the rule output, then prefix completions.
  Fewer candidates shrink the card; an empty list hides it. One row per
  candidate, row height 30px at 96 DPI (DPI-scaled), 8px outer padding, 22px
  index-badge column, card width clamped to 180-380px (96-DPI basis, scaled),
  6px corner radius, 1px border, teal selection fill, badges `1`-`9`/`0`
  matching the digit keys.
- Long words ellipsis (`DT_END_ELLIPSIS`); single-line, vertically centered,
  `NOPREFIX` (a `&` in a word never becomes an accelerator).
- GDI `DrawTextW` with a Bengali-capable font: first installed match of
  Kalpurush → Noto Sans Bengali → Nirmala UI → Segoe UI, 15pt,
  `CLEARTYPE_QUALITY`; regular rows use the normal weight, the highlighted
  row uses bold + white text. Fonts recreate on DPI change (`WM_DPICHANGED`).
- Theme follows the Windows apps preference
  (`HKCU\...\Personalize\AppsUseLightTheme`, light on any read failure):
  light card `rgb(220,228,245)`/black text, dark card `rgb(1,22,39)`/light
  text — inspired by the legacy suggestion-window themes, not pixel copies.
  No config file; unit tests inject the registry value instead of reading it.

### Placement rules

- Below the caret with a 2px (DPI-scaled) gap; flips above when the card would
  leave the monitor work area. When the caret is near the bottom of the screen
  the card flips up, and the row count is capped to the larger side's available
  height so the tail can never be clamped over the line being typed. If neither
  side holds even one row (a tiny window), it takes the larger side and clamps.
  Clamps horizontally; per-monitor DPI via the caret anchor
  (`crates/okkhor-win/src/caret.rs`, `place_popup`, `popup_space`).
- Never covers the caret line: `show_candidates` caps `rows` from
  `popup_space` before layout, so the card shrinks instead of overlapping.
  The window is `WS_POPUP | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST`,
  the class registers once, `WM_MOUSEACTIVATE` returns `MA_NOACTIVATE`, and
  showing uses `SW_SHOWNOACTIVATE` — it never takes focus and never steals
  typing.

### Keyboard / mouse, focus, blocking

- Keyboard is unchanged: digits `1`-`9` and `0` commit (ten rows), arrows
  move the highlight, Space/Enter commit, Esc/Tab cancel, Backspace edits
  (`docs/input-pipeline.md`).
- Clicking a row queues its index; the daemon drains it with
  `try_next_click()` (which pumps only this window's messages, never blocks)
  and commits through the same `select_digit` path as the digit keys —
  out-of-range rows are ignored, clicks with no composition do nothing.
- Hide on commit, cancel, pause, focus change, or empty candidates. No text
  is ever logged (metrics carry geometry only). Typing never blocks on the
  popup: a failed create/show degrades to typing without a popup.

### DPI / multi-monitor

- All metrics scale from the caret anchor DPI; the card re-lays out and
  re-places itself on `WM_DPICHANGED` from the stored anchor. Multi-monitor
  follows from the anchor's work area; per-monitor correctness still needs a
  real multi-DPI manual pass.

### Known gaps

- Clicking outside the card does not dismiss it immediately; the composition
  resets on the next keystroke (focus-change guard) and the card hides then.
- No animation, no IME-mode awareness, no accessibility role beyond a plain
  window (screen-reader naming is future work).
- Multi-DPI and dark-theme-while-running coverage is manual only
  (`popup-demo` + the ignored smoke test in `popup.rs`).

## How the UI talks to the daemon (IPC v1)

Binding contract: `docs/design/0.5-ui-contracts.md` §4, ADR 0012 §2.
Pipe: `\\.\pipe\okkhor-ipc-v1`, max 4 clients, 1 MiB frames, 10 s read
timeout, 5 s per-request deadline.

1. Read the session token from `%LOCALAPPDATA%\Okkhor\runtime\ipc.token`
   (`OKKHOR_DATA_HOME` override honored). The daemon rotates it on every
   start and removes it on clean shutdown; the value is never logged.
2. Send exactly one `hello` first: `{"hello":{"protocol":1,"token":"…"}}`.
   A wrong protocol is a typed `ProtocolMismatch` ("restart Okkhor"); a wrong
   token is `Unauthorized`.
3. Requests are one tagged object per frame. Results:
   - `status` → daemon version, `paused`, `language`/`layout`, `input_mode`,
     `output_encoding`, `popup_visible`, `words_total`, `learning_enabled`,
     `autostart` (full field list is pinned by the IPC `Status` shape).
   - `settings.get` / `settings.update` (partial patch; the daemon validates,
     clamps, persists atomically, and live-applies what it can — see below).
   - `layouts.list` → Avro/Khipro/Phonetic/Bijoy with display names.
   - `words.list` (search + `offset`/`limit`, limit clamped to 200),
     `words.add` (duplicate triple rejected), `words.remove`,
     `words.import` (CSV `roman,output,language` or JSON, 5,000 rows max),
     `words.export` (either format, 1 MiB cap).
   - `text.insert` (explicit clicks only, 512 UTF-16 units, 20/s budget).
   - `capture.set` → flips pause, returns fresh `status`.
   - `ai.complete` → typed `Unsupported` in 0.5 (accepted, never executed).
4. Errors are `{"error":{"code":"…","message":"…"}}` with static messages —
   never user text, tokens, or paths. Unknown top-level keys are
   `UnknownMethod`; oversized frames are `FrameTooLarge`.

Live-apply (the daemon never lies about it): input mode, output encoding,
pause/capture, popup enable, learning flag, and autostart take effect
immediately. Hotkey changes re-register where feasible (F9 / Ctrl+Space and
the default pause chord); anything else is persisted and reported deferred.
Fonts are validated, not installed. `popup_max_items` is persisted for the
UI's own rendering; the native popup keeps the engine's 5-candidate cap.

## On-screen keyboard (`#/keyboard`)

- Layout selector sourced from `ipc_list_layouts` (Avro, Khipro, Phonetic,
  Bijoy, …), falling back to a bundled code list when the daemon is
  unreachable. The selector is display-only and does not persist: the hardware
  layout is owned by daemon settings and switched in Settings. The table below
  always inserts Unicode text.
- The grid is a static Unicode Bangla table (vowels, consonants, vowel
  signs/marks, digits, punctuation/symbols, common conjuncts) with a Shift
  layer for digits/symbols. Per-layout roman keystroke maps are intentionally
  not bundled (no invented mappings).
- Clicking a key calls `ipc_insert_text` with the key's Unicode text, capped
  by the daemon at 512 UTF-16 units. A "Bijoy bytes" note explains that Bijoy
  output is chosen via Output encoding in Settings.
- Groups are collapsible; group order is reorderable by drag-and-drop and by
  keyboard-operable up/down buttons, persisted in `localStorage` under the
  versioned, non-sensitive key `okkhor.osk.v1.groupOrder` (ids only — no user
  text). Favorites (pinned single keys, capped) persist under
  `okkhor.osk.v1.favorites`; recents are session-only in-memory state and are
  never persisted. (Lead ruling 2026-11-23, recorded in contract §5: UI
  preferences stay out of the settings document and word store.)
- A bundled emoji list (~120 entries with English/Bengali keywords, no
  network) supports search and click-to-insert through the same path.
- Daemon unavailable: all keys disable with an explanation; every insert shows
  a toast only on error. All keys are real `<button>` elements with labels, a
  visible focus ring, and full tab order.

## Word manager (`#/words`)

- Paginated table (page size from `word_manager_page_size`, default 50),
  search, add form (roman/output/language) with client validation mirroring
  contract §3 (the daemon re-validates authoritatively), remove with a
  confirm dialog, CSV/JSON import with a first-20-rows preview plus
  imported/skipped estimates, and CSV/JSON export as a client-side blob
  download (no Tauri dialog plugin added, keeping capabilities minimal).
- Duplicate/too-many-words/control-character failures render inline as static
  text; unknown daemon codes render a safe generic message (issue #72).
  Words are never written to logs and never leave the encrypted store except
  through an explicit user export download.

## UI rules (contract §5)

- Token: `%LOCALAPPDATA%\Okkhor\runtime\ipc.token` via
  `okkhor-ipc::auth` helpers (`OKKHOR_DATA_HOME` override honored for tests).
- Error mapping (`src/error.rs`): pipe/token/connection failures →
  `daemon_unavailable`; daemon `error` responses pass the protocol `ErrorCode`
  name through verbatim as `code` with the code's static message (never the
  daemon's message string); unrecognized codes and local protocol violations
  → `internal`/generic. Messages are static; no tokens, paths, or user text.
- **No focus theft**: the settings window may activate; the popup/toast never
  do. The `quickbar` window additionally carries `WS_EX_NOACTIVATE |
  WS_EX_TOOLWINDOW | WS_EX_TOPMOST`, is shown with `SW_SHOWNOACTIVATE`, and
  declines focus (`focus: false`), so clicking it never steals typing focus.
- Theme follows system by default; accent and font scale apply live from
  settings (preview is local state until Save).
- Accessibility: landmarks, labelled controls, visible focus rings, Escape
  closes dialogs, Enter submits, `aria-live` sonner toasts, no color-only
  status, honors `prefers-reduced-motion`.
- DPI 100/150/200: resizable window (960×640 default, 720×520 min), no clipped
  controls; window size persistence (if added) lives in `localStorage` only.
- On-screen keyboard routes inserts through `text.insert` (512 UTF-16
  cap); recents stay session-only — no raw text in `localStorage` (only
  non-sensitive group order and pinned single-character favorites).

## Layout rules (responsive shell)

- App shell (`src/app.tsx`): fixed 240 px sidebar (`w-60 shrink-0`) plus a
  single scroll region (`main.overflow-auto`) whose child centers content with
  `mx-auto w-full max-w-6xl px-4 py-6 sm:px-6`. The page never scrolls
  horizontally; the sidebar never shrinks. 960×640 stays fully usable and
  720×520 is the supported minimum.
- Every grid that holds cards/table wrappers carries `min-w-0` on itself and
  `[&>*]:min-w-0` (or explicit `min-w-0` on children) so wide tables/keys can
  scroll internally (`overflow-x-auto` on the table wrapper) instead of
  pushing the page wider.
- Status: `md:grid-cols-2` — status card left, words/startup + note stacked
  right.
- Settings: one form grid, `lg:grid-cols-2` — group cards flow into two
  columns; the sticky Save/Reset bar is `lg:col-span-2`.
- Words: `xl:grid-cols-5` — word-list table card `xl:col-span-3` left, a rail
  (`xl:col-span-2`) stacking Add + Import + Export right; rail forms collapse
  to one column (`xl:grid-cols-1`). Below `xl` everything stacks.
- Keyboard: header card full width, key-group sections in `lg:grid-cols-2`;
  Favorites/Recents/Emoji stay full width below.
- About stays `max-w-2xl`. The quickbar route/window keeps its fixed 340×52
  layout — layout work must not touch it.

## Theme tokens

- Light: page `neutral-100`, cards/sidebar white, borders `neutral-200`,
  body text `neutral-900`, muted `neutral-500`. Dark: page `neutral-950`,
  cards/sidebar `neutral-900`, borders `neutral-800`, body text
  `neutral-100`, muted `neutral-400`. Muted-on-surface pairs hold ≥ 4.5:1;
  red/green/amber message text uses the 700 (light) / 400 (dark) steps.
- Accent: teal-700 fill with white text in **both** modes (≥ 5:1); the
  settings accent preset tints only the selected sidebar item
  (`rgb(var(--accent)/0.12)` wash + ring) and the global focus-visible ring.
- Font: `"Segoe UI", "Kalpurush", "Noto Sans Bengali", "Hind Siliguri",
  system-ui, sans-serif` on `body` — Latin from Segoe UI, Bangla from the
  resident Kalpurush/Noto fonts (stack only, no bundled font files).
- Scrollbars are thin and theme-aware (`scrollbar-width: thin` + WebKit
  rules) and never reserve layout space.
- Tables: header row `neutral-50`/`neutral-900`, row hover
  `neutral-50`/`neutral-800/50`, long Bangla strings wrap
  (`overflow-wrap: anywhere` on `td`).
- Layout decisions are pinned by `src/test/layout.test.tsx` (shell width,
  container cap, per-page breakpoints, table/rail spans).
