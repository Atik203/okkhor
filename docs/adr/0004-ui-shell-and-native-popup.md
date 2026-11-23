# ADR 0004: UI shell and native suggestion popup

- Status: Accepted (amended by ADR 0012: IPC, settings ownership, store)
- Date: 2026-11-23

## Context

Okkhor needs a good-looking, responsive Windows UI with Bengali shaping,
themes, an on-screen keyboard, a word manager, and a caret-adjacent suggestion
popup. The UI must not steal focus or add latency to typing.

Tauri 2 gives a mature web UI and good complex-script rendering through
WebView2. Slint is a pure-Rust native option with a smaller runtime. A WebView
per suggestion popup would be too heavy and slow.

## Decision (proposed)

- Use Tauri 2 + Svelte/React for the main settings window, word manager, and
  on-screen keyboard.
- Use a pre-created native, non-activating popup for caret-adjacent suggestions
  (Direct2D/DirectWrite or `cosmic-text` + a lightweight window).
- Keep the UI in a separate process from the daemon.
- Revisit after a prototype measures show/hide latency and Bengali shaping.

## Consequences

Positive:

- fast UI development and good Bengali shaping for complex screens;
- native popup keeps the typing path light;
- UI crash does not stop typing.

Negative:

- two rendering stacks to maintain;
- WebView2 runtime dependency for the main UI;
- native popup needs DPI, focus, and accessibility work.

## Alternatives considered

- All-Slint: one stack, smaller runtime; validate Bengali shaping, IME, and
  accessibility first.
- All-Tauri including the popup: simpler stack, but WebView show/hide latency is
  a risk.
- Pure Win32/Direct2D: maximum control, highest UI cost.
- egui: fast to prototype, but weaker native text/IME/accessibility fit for a
  consumer keyboard app.
