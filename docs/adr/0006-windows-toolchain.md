# ADR 0006: Windows toolchain and build targets

- Status: Accepted
- Date: 2026-11-23

## Context

Okkhor is Windows-first. The current development host did not have MSVC Build
Tools or a Windows SDK linker, but it did have MSYS2 mingw64. Rust was
bootstrapped with the user-level GNU toolchain so the workspace could be built
and tested without an administrator prompt.

Production Windows code — hooks, injection, tray, installer, and WebView2 — must
be validated with MSVC, which is the standard Rust Windows target and the target
used by GitHub Actions.

## Decision

- CI and release builds use `x86_64-pc-windows-msvc` on Windows 11/Windows
  Server with the Visual Studio Build Tools and Windows SDK.
- The local GNU toolchain (`x86_64-pc-windows-gnu` + MSYS2 mingw64) is allowed
  for early, platform-independent development and tests.
- `okkhor-win` native code must not be considered shippable until it builds and
  passes integration tests with MSVC.
- Do not mix GNU-built and MSVC-built artifacts in one installer.
- Document the toolchain in `AGENTS.md` and keep the checks identical where
  possible.

## Consequences

Positive:

- developers can start without an elevated install;
- CI and release artifacts use the supported production toolchain;
- the pure Rust core stays toolchain-independent.

Negative:

- two local toolchains can cause confusion;
- GNU-only validation can miss calling-convention or import-library issues;
- the release machine needs Visual Studio Build Tools and the Windows SDK.

## Alternatives considered

- Require MSVC from day one: more correct, but blocked initial progress on a
  non-elevated host.
- Ship GNU builds: rejected for Tauri/WebView2 compatibility, signing, and
  long-term Windows support.
