# ADR 0012: UI process, IPC, settings ownership, and encrypted user store

- Status: Accepted
- Date: 2026-11-23
- Supersedes: none
- Amends: ADR 0004 (UI shell), `docs/architecture.md` IPC note

## Context

ADR 0004 proposed Tauri 2 for the settings/on-screen-keyboard window and a
separate native, non-activating suggestion popup. The native popup already
exists in `okkhor-win`; the UI process, its transport, and the owner of user
settings do not.

The architecture document says IPC may use binary framing "for suggestion
batches". That framing predates the 0.5 design: suggestions are rendered by the
native popup inside the daemon, so no per-keystroke batch crosses the process
boundary. What crosses is low-rate control traffic (status, settings, word
manager, explicit on-screen-keyboard insertion).

The security contract requires: per-user named-pipe ACL, a random session
token, caller validation, bounded request/response sizes and timeouts, and
encrypted-at-rest user text. `docs/security-privacy.md` names DPAPI.

## Decision

### 1. Shell

- Accept ADR 0004. The UI is Tauri 2 + React 19 + TypeScript + Vite, in
  `apps/okkhor-ui`, a **separate process** from the daemon.
- UI stack: Tailwind CSS v4, shadcn/ui on Radix primitives, lucide-react,
  sonner, TanStack Query, react-hook-form + zod, and the usual class utilities.
  Route-level code splitting with a 350 KB gzip initial-JS budget; no remote
  assets, fonts, or telemetry.
- The suggestion popup stays native Win32 (`okkhor-win`). The WebView never
  renders per-keystroke suggestions.
- The UI is optional. Closing or crashing it must not affect typing.

### 2. Transport

- `crates/okkhor-ipc` (new crate, `#![forbid(unsafe_code)]` where possible;
  Windows handle work delegated through a small `unsafe` module with SAFETY
  comments if unavoidable).
- Windows named pipe, one pipe instance per connection, default instance
  bounded to 4 concurrent clients.
- JSON messages, 4-byte little-endian length prefix, UTF-8. Maximum frame
  1 MiB, maximum request timeout 5 s, server read timeout 10 s.
- Rationale for JSON over binary: only low-rate control traffic crosses the
  boundary; inspectability and versioned evolution outweigh bytes. The hot path
  never crosses IPC.
- Every connection begins with `Hello { protocol: 1, token }`. The token is 32
  random bytes from the OS CSPRNG, written to
  `%LOCALAPPDATA%\Okkhor\runtime\ipc.token`, rotated on every daemon start,
  compared in constant time. The pipe security descriptor is
  `D:(A;;GA;;;SY)(A;;GA;;;OW)` (System and Owner full access) using
  `ConvertStringSecurityDescriptorToSecurityDescriptorW`.
- No raw keystroke stream. `InsertText` is allowed only as an explicit UI
  action, capped at 512 UTF-16 code units, and it is the same injector the
  daemon already uses.
- Protocol version mismatch is a typed error; the UI shows "restart Okkhor".

### 3. Settings ownership

- The daemon is the single writer of
  `%LOCALAPPDATA%\Okkhor\settings.json`.
- The UI reads and writes through IPC (`settings.get`, `settings.update`). If
  the pipe is unavailable, the UI reads the file for display only and disables
  edits with an explicit banner. It never writes the file directly.
- Settings schema v1 is versioned, range-validated in Rust, and rejects
  unknown-but-unsafe values by clamping (not by panicking).
- Settings are normal-sensitivity: plaintext JSON, exportable. User words and
  learned data are sensitive: encrypted.

### 4. Encrypted user store

- `okkhor-win` gains `dpapi` (Windows `CryptProtectData`/`CryptUnprotectData`,
  current-user scope) with SAFETY comments and exhaustive error mapping.
- `okkhor-storage` gains `EncryptedJsonRepository` over a `Protector` trait so
  tests can use a deterministic fake and non-Windows builds can use a no-op
  that refuses to persist. Production storage requires DPAPI.
- Custom words, imported words, and AI cache entries that contain text are
  written only through the encrypted repository. Logs remain allowlisted and
  never receive text.

### 5. Workspace and CI

- `apps/okkhor-ui` is a JS app plus `apps/okkhor-ui/src-tauri` with its own
  empty `[workspace]` table, so the root workspace checks stay fast and do not
  require a Node toolchain.
- CI adds one `ui` job: `npm ci`, `npm run build`, `cargo test` in
  `src-tauri` (client framing tests only), and `cargo check` for the Tauri
  shell. Root `fmt`/`clippy`/`test` are unchanged.
- Dependencies: Tauri 2, React 19, Vite, TypeScript, Tailwind CSS v4,
  shadcn/ui + Radix, lucide-react, sonner, TanStack Query, react-hook-form,
  zod, Vitest, and React Testing Library. No second state library and no
  UI kit beyond shadcn primitives; the dependency list is reviewed for license
  and bundle cost.

## Consequences

Positive:

- the daemon stays the only authority over settings, tokens, and persistence;
- the hot path is untouched by UI work;
- IPC traffic is auditable and bounded;
- user text at rest is DPAPI-protected.

Negative:

- two rendering stacks and two toolchains (Rust + Node);
- WebView2 runtime is required for the settings window (Windows 11 ships it);
- the UI must degrade honestly when the daemon is absent.

## Alternatives considered

- **UI writes settings.json directly.** Simpler, but creates two writers and a
  reload race. Rejected.
- **Binary/postcard IPC.** Smaller frames, worse inspectability, no measured
  benefit for control traffic. Rejected.
- **All UI in the daemon process (egui/Slint).** Fewer processes, but a UI
  crash or WebView failure could reach the typing path, and Bengali shaping and
  accessibility are weaker. Rejected for 1.0.
- **TCP loopback.** Weaker local boundary than a named pipe with an owner-only
  DACL. Rejected.
