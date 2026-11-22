# Assets

This directory is for **import instructions and small, redistributable test
fixtures**, not the legacy application's proprietary payload.

Do not commit:

- the legacy `Kontho.exe`, installer, DLLs, Python bytecode, or extracted PYZ;
- dictionaries, fonts, images, sounds, or themes from the legacy app unless the
  owner has confirmed redistribution rights;
- user learning data, exports, or screenshots containing real text.

The planned importer will read a user-selected legacy installation and write a
separate `assets/private/` or user data directory that is git-ignored. The
repository should remain buildable without those files.

Fixture policy:

- prefer tiny synthetic words and sentences for unit tests;
- put licensed or user-provided corpora under `data/imported/` (ignored);
- record provenance and license in `docs/compatibility.md`.
