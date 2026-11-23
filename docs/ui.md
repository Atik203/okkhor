# Okkhor settings UI (0.5)

Separate-process settings window: **Tauri 2 + React 19 + TypeScript + Vite** in
`apps/okkhor-ui`. A UI crash must never stop typing — the daemon owns input,
this app only observes and configures it through IPC.

Binding contracts: `docs/design/0.5-ui-contracts.md` (§2 settings, §4
protocol, §5 UI behavior), ADR 0012 (UI process, IPC, settings ownership),
ADR 0013 (encodings/fonts), ADR 0014 (input modes, hotkeys, zero-config
fonts).

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
- Daemon absent: every Rust command returns typed `daemon_unavailable`; the
  WebView shows a persistent banner, renders read-only, and offers Retry.
- No network at runtime: CSP is `default-src 'self'` plus the minimum Tauri
  needs (`ipc:`/`http://ipc.localhost` for invoke, `asset:`/`data:` images,
  local inline styles). No remote fonts, assets, or telemetry.
- The suggestion popup stays native Win32 (`okkhor-win`); the WebView never
  renders per-keystroke suggestions and creates no always-on-top windows.

## Routes

| Hash | Page | Notes |
| --- | --- | --- |
| `#/` | Status | version, paused toggle, mode, language/layout, encoding, word count, autostart, "UI is separate from typing" note |
| `#/settings` | Settings | every contract §2 field (incl. quickbar X/Y + Automatic toggle), live validation, dirty guard, Save/Reset, toasts |
| `#/keyboard` | On-screen keyboard | layout selector, Unicode key table + Shift layer, reorderable groups, favorites, session recents, bundled emoji search |
| `#/words` | Word manager | paginated table (50 default), search, add/remove, CSV/JSON import preview + export download |
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

## On-screen keyboard (`#/keyboard`)

- Layout selector sourced from `ipc_list_layouts` (Avro, Khipro, Banglish,
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
  do. The UI creates no `WS_EX_TOPMOST` windows of its own.
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
