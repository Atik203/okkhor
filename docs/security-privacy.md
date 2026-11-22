# Security and privacy

Akkhor is a keyboard hook, a text injector, and a local learning store. That is
a high-trust combination. This document is the security contract for the
project.

## Data classification

| Data | Sensitivity | Default handling |
| --- | --- | --- |
| Keystrokes | critical | memory only; never persisted or logged |
| Committed words / context | sensitive | local learning store, encrypted |
| Clipboard | critical | transient only; no history |
| Window titles / app names | sensitive | not collected by default |
| Speech audio | critical | transient; cloud only on explicit action |
| AI prompts / responses | sensitive | background cache, encrypted, clearable |
| Settings | normal | local config, exportable |
| Crash data | sensitive | redacted, opt-in only |
| License identity | sensitive | OS-protected storage, minimal retention |

## Threat model

| Threat | Impact | Controls |
| --- | --- | --- |
| Keylogger perception / malware | loss of trust, data theft | local-only, open behavior, signed builds, no raw logs |
| IPC injection by another process | text injection / data leak | per-user named-pipe ACL, random session token, caller validation |
| Update tampering | code execution | signed manifests, signature verification, rollback |
| DLL search-order hijack | code execution | absolute paths, `SetDefaultDllDirectories`, no cwd loading |
| Clipboard snooping | text leak | minimal clipboard use, restore previous clipboard when possible |
| Learning-data theft | private text exposure | DPAPI encryption, file ACL, no cloud sync by default |
| Password-field capture | credential leak | UI Automation password detection where possible, pause hotkey |
| License bypass | revenue loss | signed license tokens, server verification, no custom crypto |
| AV false positive | blocked install / removal | code signing, stable publisher, Microsoft submission |

## Logging

Never log:

- keystrokes, words, sentences, or clipboard contents;
- window titles or full paths containing user names;
- speech audio or transcripts;
- AI prompts or responses;
- license keys or tokens.

Allowed:

- event counts;
- latency histograms;
- error codes;
- build/version/OS information;
- redacted provider status.

Structured logging must use a redaction layer. Tests must assert that sensitive
fields are absent.

## Storage

- Learning snapshots are encrypted at rest with Windows DPAPI or a key stored in
  the OS credential vault.
- Store under `%LOCALAPPDATA%\Akkhor`, not next to the executable.
- Use per-user ACLs; do not create world-readable files.
- Provide export and delete. Delete must remove both memory and disk copies.
- Backups must be encrypted and bounded.
- Never deserialize Python pickle or arbitrary object graphs.

## IPC

- Named pipe with a per-user DACL.
- Random session token written to a protected file or passed on the command line
  to the UI child only.
- Validate message size, schema, and sender.
- The UI cannot ask the daemon to inject arbitrary text without a local,
  explicit user action.
- No TCP listener on the public interface.

## Keyboard and injection

- Do not run elevated by default.
- Elevated target windows reject input from a non-elevated process; report the
  limitation instead of silently failing.
- Preserve the foreground window; abort injection if focus changes.
- Prefer Unicode `SendInput`; clipboard fallback must restore the previous
  clipboard when feasible.
- Provide a visible pause control and a panic hotkey.
- Do not capture or store text from password fields.

## Updates

- Sign manifests and binaries; verify before applying.
- Use an offline root or a pinned public key; do not trust a mutable URL alone.
- Update in a separate process, not inside the running daemon.
- Keep the previous version for rollback.
- Publish SHA-256 digests and an SBOM with each release.

## Licensing

- Use standard signatures (Ed25519) and a documented token format.
- Do not invent custom encryption for serials.
- Do not use MAC address as the sole identity; it is unstable and privacy
  sensitive.
- Keep license checks off the typing path.
- A license outage must not break local typing; use a bounded grace period.

## Antivirus and SmartScreen

Low-level hooks and `SendInput` are common malware techniques. Expect false
positives.

- Sign every binary with a stable publisher identity.
- Build a clean, reproducible release pipeline.
- Submit builds to Microsoft for review.
- Avoid packers and obfuscation.
- Keep a public privacy policy and a clear uninstall.
- Do not bundle unrelated executables.

## Incident response

1. Contain: disable the affected provider or update channel.
2. Preserve evidence without collecting user text.
3. Notify affected users if personal data may have been exposed.
4. Ship a signed fix and a rollback path.
5. Publish a post-mortem without sensitive details.

Report vulnerabilities through the private channel in `SECURITY.md`.
