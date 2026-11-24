# Release process

## Versioning

Semantic versioning:

- `0.x.y` — pre-parity milestones; breaking changes allowed with a migration note;
- `1.0.0` — feature parity, signed installer, stable data schema;
- `1.x.y` — compatible improvements and fixes;
- `2.0.0` — a breaking storage, protocol, or behavior change.

Pre-release channels:

- `dev` — local/nightly, no promises;
- `beta` — opt-in, migration tested; signed once a trusted certificate is
  configured (the 0.8 betas are unsigned unless the owner signs them);
- `stable` — signed, supported, rollback tested.

## Artifacts

| Artifact | Naming |
| --- | --- |
| Installer | `Okkhor-Setup-<version>.exe` |
| Portable | `Okkhor-<version>-win64-portable.zip` |
| Symbol package | `Okkhor-<version>-symbols.zip` (private) |
| Checksums | `SHA256SUMS.txt` |
| SBOM | `Okkhor-<version>.sbom.json` |
| Update manifest | `latest.json` |

Every artifact must be reproducible from a tagged commit.

## Build toolchain (Windows)

The 0.8 portable beta is built with the production MSVC toolchain, not the GNU
dev fallback. `rust-toolchain.toml` selects the GNU toolchain in an interactive
shell, so `scripts/build-beta.ps1` overrides it for MSVC targets:

```powershell
$env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-msvc'
cargo build --release --locked --target x86_64-pc-windows-msvc ...
```

Build output lands in `target\x86_64-pc-windows-msvc\release\` and, for the
Tauri shell, `apps\okkhor-ui\src-tauri\target\x86_64-pc-windows-msvc\release\`.
MSVC matches CI, uses the production Windows ABI, and does not depend on the
MSYS2 runtime DLLs. The artifact must not contain those DLLs.

```powershell
.\scripts\build-beta.ps1 -Version 0.8.0
```

| Option | Meaning |
| --- | --- |
| `-Target <triple>` | Rust target; default `x86_64-pc-windows-msvc` |
| `-SkipUi` / `-SkipFonts` | Skip the Tauri shell / font staging |
| `-UiDynamicCrt` | Build only the UI with the dynamic CRT (fallback; see below) |
| `-SkipSign` | Stage an unsigned artifact even if a certificate is configured |
| `-PfxPath` + `-PfxPassword` | Sign with a PFX; the password also comes from `OKKHOR_SIGN_PFX_PASSWORD` |
| `-CertificateThumbprint` | Sign with a certificate in the current user's store |
| `-TimestampUrl` | RFC 3161 timestamp server; default `http://timestamp.digicert.com` |
| `-OutputDir` | Stage and zip somewhere other than `dist\` |

`BUILD-INFO.json` records the toolchain, target, and signature status. It never
records the PFX path, thumbprint, or password.

### Self-contained runtime (no VC++ Redistributable)

MSVC builds link the C runtime statically. `build-beta.ps1` sets:

```powershell
$env:RUSTFLAGS = '-C target-feature=+crt-static'   # MSVC targets only
```

and then reads the PE import table of every staged executable. A
`VCRUNTIME140*.dll`, `MSVCP140*.dll`, `CONCRT140*.dll`, or `VCOMP140*.dll`
import fails the build, because that would mean the portable zip needs the
Microsoft Visual C++ Redistributable. The UCRT `api-ms-win-crt-*.dll` imports
come from Windows 10/11 itself and are expected.

Verify by hand if needed (the SDK path varies):

```powershell
dumpbin /nologo /dependents dist\Okkhor-0.8.0-win64\Okkhor.exe
dumpbin /nologo /dependents dist\Okkhor-0.8.0-win64\bin\okkhor-engine.exe
dumpbin /nologo /dependents dist\Okkhor-0.8.0-win64\bin\okkhor-worker.exe
```

`BUILD-INFO.json` records the checked result under `runtime`
(`engine_crt`, `worker_crt`, `ui_crt`, `vc_redistributable_required`).
For 0.8.0 all three executables (`Okkhor.exe`, `bin\okkhor-engine.exe`,
`bin\okkhor-worker.exe`) were verified static-CRT, so the zip needs no separate
Visual C++ Redistributable. If a Tauri dependency ever refuses the static CRT,
use the fallback:

```powershell
.\scripts\build-beta.ps1 -Version 0.8.0 -UiDynamicCrt
```

That builds only `Okkhor.exe` with the dynamic CRT. The build prints a warning,
`BUILD-INFO.json` sets `ui_crt: "dynamic"` and
`vc_redistributable_required: true`, and the release notes must name the
Microsoft Visual C++ Redistributable (x64) as a requirement. The engine and
worker stay self-contained either way: typing must not depend on an installed
redistributable. Prefer the static UI.

## Signing

Status: the signing **hook** exists (`scripts/sign-artifact.ps1` and the
`build-beta.ps1` options above). No certificate is configured in this
repository, so a build made without one is unsigned and `BUILD-INFO.json`
reports `"signed": false`. That is not a 1.0 signed release.

- Sign `Okkhor.exe`, `bin\okkhor-engine.exe`, and `bin\okkhor-worker.exe`.
- Use a publicly trusted Authenticode certificate (OV or EV). Use a PFX or a
  hardware-backed key exposed through the Windows certificate store; a
  thumbprint keeps the private key out of a file on disk.
- Always timestamp (RFC 3161, `/tr` + `/td SHA256`) so signatures stay valid
  after the certificate expires.
- Verify every file with `signtool verify /pa /all` before publishing. The
  build script fails when verification fails.
- Signing runs after staging and before hashing, so `BUILD-INFO.json` and
  `SHA256SUMS.txt` describe the signed bytes that users download.
- Keep the signing key offline or in an HSM/token. Never commit certificates,
  private keys, PFX files, passwords, or tokens.
- Pin the public key in the updater and verify signatures before applying an
  update (1.0 work; there is no updater in 0.8).

```powershell
# PFX (password from a secret store, never a committed file)
$env:OKKHOR_SIGN_PFX_PASSWORD = '<secret>'
.\scripts\build-beta.ps1 -Version 0.8.0 -PfxPath C:\keys\okkhor.pfx

# Certificate already installed in CurrentUser\My
.\scripts\build-beta.ps1 -Version 0.8.0 -CertificateThumbprint <SHA-1 thumbprint>

# Sign an already staged folder
.\scripts\sign-artifact.ps1 -Path `
  dist\Okkhor-0.8.0-win64\Okkhor.exe, `
  dist\Okkhor-0.8.0-win64\bin\okkhor-engine.exe, `
  dist\Okkhor-0.8.0-win64\bin\okkhor-worker.exe `
  -CertificateThumbprint <thumbprint>
```

`sign-artifact.ps1` finds `signtool.exe` on `PATH` or in the newest installed
Windows 10/11 SDK and fails clearly when it is missing. A build that was asked
to sign never silently produces an unsigned artifact.

### Why SmartScreen and Defender warn on unsigned builds

- **SmartScreen** is reputation-based, not a malware verdict. A new, unsigned
  executable from a publisher with no download history triggers "Windows
  protected your PC". The warning is about identity and reputation.
- **Microsoft Defender** can flag unsigned, low-prevalence, freshly compiled
  binaries, especially from a zip archive, a self-extracting stub, or a packer.
  A keyboard hook plus an injected-input component also resembles behavior that
  malware uses.
- Neither is a Rust or MSVC bug. Both follow from the missing trusted signature
  and unknown publisher.
- Reputation accrues with consistent, signed releases from the same publisher
  identity. An EV certificate starts with more trust than an OV certificate.

### Owner actions: remove the SmartScreen warning

1. Obtain a trusted Authenticode code-signing certificate (OV or EV) in the
   publisher's legal name. A free or self-signed certificate is not enough.
   Okkhor is public and MIT-licensed, so the **SignPath Foundation's free OSS
   program** is the lowest-cost route: see
   <https://signpath.org/terms.html> and
   <https://about.signpath.io/product/open-source>. It signs through a managed
   pipeline with an OV certificate and requires a fully automated, verifiable
   build from source (this repo already has CI on `main`). An OV certificate
   still has to accumulate download reputation; EV reputation starts higher.
2. Install it on the release machine (token/HSM preferred, or an offline PFX)
   and keep the thumbprint or PFX path outside the repository.
3. Build with MSVC and the signing options:
   `.\scripts\build-beta.ps1 -Version <version> -PfxPath ...` (or
   `-CertificateThumbprint ...`).
4. Confirm the script signed all three executables, timestamped them, and
   passed `signtool verify /pa /all`.
5. Check one binary manually:
   `Get-AuthenticodeSignature .\dist\Okkhor-<version>-win64\Okkhor.exe`
   must report `Status: Valid`, the expected publisher, and a timestamp.
6. Publish the signed zip with `SHA256SUMS.txt` and put the zip hash in the
   release notes.
7. Keep the same publisher identity for every release so SmartScreen
   reputation accumulates. Switching certificates resets it.
8. If Defender still flags the build, submit it using the section below rather
   than assuming the signature fixes everything.

### Tester convenience: Mark-of-the-Web

A zip downloaded from GitHub carries Mark-of-the-Web, which is what triggers
the SmartScreen screen before the executable even starts. A tester who built or
downloaded the artifact from this project's official release can remove that
marker locally:

```powershell
Get-ChildItem -LiteralPath <unpacked-folder> -Recurse -File |
  Unblock-File
```

This is **not signing**: it removes the "came from the internet" marker only on
that machine, does not prove authenticity, and does not build publisher
reputation. Use it only for an artifact whose SHA-256 matches the release
notes. It is a convenience for known testers, not a fix to publish as a
workaround to users.

### Self-signed certificates

A self-signed certificate produces a valid signature only on machines that
already trust its root (or where the certificate was installed manually). It
does not remove SmartScreen on a user's machine, does not build reputation, and
is not a release fix. Use it only for a local smoke test of the signing hook;
`signtool verify /pa` is expected to fail with an untrusted-root error, and
`build-beta.ps1` treats that as a failure.

## Defender false positive

Use this when a signed build is still quarantined or flagged:

1. Keep the binary signed; do not ship an unsigned fallback.
2. Do not pack or compress the executables (no UPX, no self-extracting stub).
   Ship the plain zip with the documented folder layout.
3. Rebuild with MSVC and the release profile, not a debug or GNU build.
4. Submit the file and its SHA-256 to Microsoft at
   <https://www.microsoft.com/en-us/wdsi/filesubmission> as a software
   developer ("I believe this file is incorrectly detected"), naming the
   product, version, publisher, and download URL.
5. Publish the exact zip SHA-256 in the release notes so testers confirm they
   submitted the same bytes.
6. Resubmit only if the detection returns; each new binary hash is a new file
   to Defender.

## Update flow

1. Check a signed manifest.
2. Download to a staging directory.
3. Verify size, SHA-256, and signature.
4. Quit the daemon and UI cleanly.
5. Apply the update in a separate updater process.
6. Start the new version and run a health check.
7. On failure, roll back to the previous version and report.

Never update the running daemon in place.

## Data migrations

- Every storage schema has a version.
- Migrate a copy, then swap atomically.
- Keep the previous snapshot until the new version starts successfully.
- Refuse to write a newer schema than the app understands.
- Record the migration result in a user-visible report.

## Data locations (0.5)

All daemon state lives under `%LOCALAPPDATA%\Okkhor` (`OKKHOR_DATA_HOME`
overrides it for tests, dev, and portable use only):

| File | Content | Protection |
| --- | --- | --- |
| `settings.json` | Settings document, schema v1 | Plaintext JSON; the daemon is the only writer |
| `data\words.bin` | User/imported words, schema v1 | DPAPI current-user envelope (`OKE1` + version + ciphertext) |
| `runtime\ipc.token` | 32-byte session token for the settings UI | Owner-only ACL, rotated every daemon start, removed on clean shutdown |
| `logs\okkhor.log` | Redacted JSON-lines log | Event codes and numeric fields only; never user text |

Migration notes:

- `settings.json` moves freely between installs; unknown fields are ignored
  on read and dropped on write.
- `data\words.bin` is bound to the Windows user profile that sealed it
  (DPAPI current-user scope). Copying it to another machine or user does not
  decrypt — it fails closed. Migrate words with `words.export` (CSV/JSON)
  and `words.import` instead of copying the file.
- Never ask a user to send `words.bin`, the learning database, or typed text
  for diagnostics.

## Release checklist

- [ ] version bumped in `Cargo.toml` and changelog;
- [ ] `cargo fmt`, `clippy`, tests, self-check pass;
- [ ] built with the MSVC target (`BUILD-INFO.json` `target` is
      `x86_64-pc-windows-msvc`);
- [ ] performance gates met;
- [ ] security/privacy review for new data flows;
- [ ] docs updated;
- [ ] installer and portable build tested on a clean VM;
- [ ] upgrade and rollback tested from the previous stable;
- [ ] all three executables signed, timestamped, and `signtool verify /pa /all`
      passes, or the release is explicitly labelled an unsigned beta;
- [ ] SHA-256 published;
- [ ] beta feedback window completed before stable;
- [ ] no proprietary assets or user data in the artifact.

## Rollback

- Keep the previous signed installer and data snapshot.
- A failed update must leave the previous version runnable.
- A failed migration must not destroy the old snapshot.
- Document the manual recovery path in the release notes.

## Support

- Publish known issues and workarounds.
- Keep a privacy-safe diagnostic export.
- Never ask a user to send their learning database or typed text.
