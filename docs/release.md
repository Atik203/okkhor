# Release process

## Versioning

Semantic versioning:

- `0.x.y` — pre-parity milestones; breaking changes allowed with a migration note;
- `1.0.0` — feature parity, signed installer, stable data schema;
- `1.x.y` — compatible improvements and fixes;
- `2.0.0` — a breaking storage, protocol, or behavior change.

Pre-release channels:

- `dev` — local/nightly, no promises;
- `beta` — signed, opt-in, migration tested;
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

## Signing

- Sign the installer, executables, and update manifest.
- Pin the public key in the updater.
- Verify signatures before applying an update.
- Keep the signing key offline or in a hardware-backed store.
- Never commit certificates, private keys, or tokens.

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

## Release checklist

- [ ] version bumped in `Cargo.toml` and changelog;
- [ ] `cargo fmt`, `clippy`, tests, self-check pass;
- [ ] performance gates met;
- [ ] security/privacy review for new data flows;
- [ ] docs updated;
- [ ] installer and portable build tested on a clean VM;
- [ ] upgrade and rollback tested from the previous stable;
- [ ] signatures verified;
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
