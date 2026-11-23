# Encrypted storage (0.5)

Binding contracts: `docs/design/0.5-ui-contracts.md` §2/§3, ADR 0012 §4,
`docs/security-privacy.md`.

## Pieces

| Piece | Lives in | Role |
| --- | --- | --- |
| `protect` / `unprotect` | `okkhor-win::dpapi` | DPAPI current-user seal; the only `unsafe` in this milestone's storage path |
| `Protector` trait | `okkhor-storage` | `protect`/`unprotect` over `StorageError`; keeps `unsafe` out of storage |
| `EncryptedJsonRepository<T, P>` | `okkhor-storage` | `OKE1` envelope, atomic replace, caps, `NotFound`/`Corrupt`/`Unsupported` |
| `FakeProtector` | `okkhor-storage` | Deterministic XOR mask for unit tests; never production |
| `paths` | daemon | `%LOCALAPPDATA%\Okkhor` layout + `OKKHOR_DATA_HOME` test override |
| `SettingsStore` | daemon | Plaintext `settings.json`: defaults, clamp, `update(patch)` |
| `WordsStore<P>` | daemon | Encrypted `data/words.bin`: ids, caps, dedupe, search, import/export |
| `DpapiProtector` | daemon | `Protector` adapter binding DPAPI to `WORDS_ENTROPY` |

## Envelope

```text
b"OKE1" | u32 LE version (= 1) | protector ciphertext
```

`load` checks, in order: file presence (`NotFound`), file size headroom
(`TooLarge`), magic + version (`Corrupt`), protector (`Unsupported` stays
`Unsupported`, everything else becomes `Corrupt`), plaintext size (`TooLarge`),
JSON shape (`Corrupt`). The original file is never modified or partially
loaded.

`save` serializes, size-checks (8 MiB plaintext cap), encrypts, then writes a
sibling temporary file (same directory, so the rename stays on one
filesystem) and renames over the target. The temporary file holds ciphertext
only. A failed save leaves any previous file byte-identical.

## Why this shape

- The repository is generic over the protector so every behavior except the
  cipher itself is tested without Windows: header tampering, truncation,
  future versions, atomicity under injected failure, caps, and missing files.
- One Windows-gated test proves the DPAPI-backed repository round trip,
  including cross-entropy rejection.
- `WordsStore` validates through `okkhor_ipc::words` (the same code the IPC
  layer uses), so validation cannot drift between the store and the protocol.
- `SettingsStore` validates through `okkhor_ipc::settings` the same way;
  unknown fields ride along on read and are dropped on the next write.

## Limits

- Total words: 20,000; import rows per request: 5,000; export payload: 1 MiB;
  `words.list` limit clamped to 200.
- DPAPI input cap (64 MiB) is a backstop; the repository enforces 8 MiB first.
- Zero-length DPAPI input is rejected with a typed error — the OS refuses it
  (`E_INVALIDARG`), so the wrapper does too instead of attempting the call.
- DPAPI binds to the user profile: `words.bin` does not move across machines
  or users. Migrate with export/import, never by copying the file.
