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
| `#/settings` | Settings | every contract §2 field, live validation, dirty guard, Save/Reset, toasts |
| `#/keyboard` | On-screen keyboard | **coming in 0.5.1** placeholder |
| `#/words` | Word manager | **coming in 0.5.1** placeholder |
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
`ipc_list_layouts`, `ipc_insert_text`, `ipc_set_paused` — all async, typed
`UiError { code, message }`, no `unwrap` on user paths.

- Token: `%LOCALAPPDATA%\Okkhor\runtime\ipc.token` via
  `okkhor-ipc::auth` helpers (`OKKHOR_DATA_HOME` override honored for tests).
- Error mapping (`src/error.rs`): pipe/token/connection failures →
  `daemon_unavailable`; daemon `error` responses pass the protocol `ErrorCode`
  name through verbatim; local protocol violations → `internal`. Messages are
  static; no tokens, paths, or user text.

## UI rules (contract §5)

- **No focus theft**: the settings window may activate; the popup/toast never
  do. The UI creates no `WS_EX_TOPMOST` windows of its own.
- Theme follows system by default; accent and font scale apply live from
  settings (preview is local state until Save).
- Accessibility: landmarks, labelled controls, visible focus rings, Escape
  closes dialogs, Enter submits, `aria-live` sonner toasts, no color-only
  status, honors `prefers-reduced-motion`.
- DPI 100/150/200: resizable window (960×640 default, 720×520 min), no clipped
  controls; window size persistence (if added) lives in `localStorage` only.
- On-screen keyboard (0.5.1) must route through `text.insert` (512 UTF-16
  cap); recents stay session-only — no raw text in `localStorage`.
