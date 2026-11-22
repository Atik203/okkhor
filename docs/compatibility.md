# Legacy compatibility

Akkhor is a clean-room Rust implementation. The legacy application is a
behavioral oracle and an asset source, not a runtime dependency.

The legacy installation is owner-provided and read-only. Local harnesses may
read it through `AKKHOR_LEGACY_HOME`; never commit that path or its contents.

Never run the legacy executable or installer as part of normal development.

## What the legacy build contains

| Artifact | Observation |
| --- | --- |
| Main executable | 14 MB, x64 GUI, PyInstaller 2.1, Python 3.13 |
| Embedded archive | 1,700 inventory nodes, 1,693 `.pyc` modules |
| UI | PyQt5, 18 `.ui` screens, themes, animations, on-screen keyboard |
| Input | `keyboard` + `pynput` hooks, AHK `globalKeys.exe` |
| Text injection | Python `write()` loops, clipboard + `SendInput` fallback |
| Native DLLs | `word_trie.dll`, `_sentencetrie_native.dll`, `translator.dll`, `caret.dll`, `emoji_manager.dll` |
| Language data | Bangla/English dictionaries, layouts, encodings, fonts |
| Cloud | Google Speech (`recognize_google`), Gemini (`gemini-flash-latest`) |
| Licensing | encrypted serial, MAC address, Firebase date check |
| Updates | GitHub raw installer channel |

## Reuse matrix

| Asset | Reuse | Notes |
| --- | --- | --- |
| Bangla/English dictionaries (`*.bin`) | Yes, via importer | verify license/ownership |
| Layouts (`*.nmsLayout`) | Yes | parse and test all supported layouts |
| Encodings (`*.nmsEnco`) | Yes | convert to a documented format |
| JSON metadata | Yes | normalize and validate |
| Fonts/images/SFX | Yes, if licensed | keep out of git unless redistributable |
| Themes/`.ui` files | Design reference | do not ship PyQt code |
| `globalKeys.exe` / AHK | No | replace with native hotkeys |
| Python bytecode | No | use behavior as spec |
| `settings.pkl` | No direct load | one-time exporter only |
| `SentenceHistory.bin` | Optional | only after format analysis + tests |
| Native DLLs | Temporary FFI | replace one at a time |

## FFI plan

Create a `legacy-compat` crate only when phase 2 starts:

- load DLLs by absolute path, never by cwd;
- wrap every call in a safe Rust API;
- validate buffers and lengths;
- keep the DLL versions pinned and hashed;
- add a feature flag per adapter;
- differential-test against the legacy behavior;
- remove each adapter once the Rust replacement has parity.

Do not put FFI in `akkhor-core` or `akkhor-learn`.

## Settings migration

`settings.pkl` is a Python pickle. It is untrusted input and must never be
deserialized by the Rust app.

Options:

1. A one-time Python exporter that reads known keys and writes JSON.
2. A guided re-setup that imports only dictionaries, layouts, and custom words.
3. A migration report that lists what was imported and what was skipped.

Never execute pickle payloads. Never load a pickle from a network path or an
untrusted user.

## Sentence history

`Dictionaries\Langs\Bangla\SentenceHistory.bin` and the English equivalents may
contain valuable personal context. Treat them as sensitive.

- Analyze the format offline.
- Write a parser with bounds checks and tests.
- Import only counts/recency, or the minimum text needed for suggestions.
- Encrypt the imported data.
- Let the user skip the import and clear it later.

If the format is not understood, start fresh and say so.

## Asset rights

The legacy application and its assets may be proprietary. The repository must
not redistribute them. The importer operates on a user-selected local install
and writes to an ignored/user data directory. Record provenance and permission
before shipping any asset with Akkhor.

## Side-by-side testing

- Use different data directories and process names.
- Do not register both apps for the same global hotkeys during a test.
- Disable one hook before testing the other.
- Capture results, not user text.
- Restore the test machine after the run.

## Compatibility exit criteria

- top-5 parity at least 95% on the approved corpus, or documented intentional
  differences;
- all supported layouts import cleanly;
- custom words and shortcuts migrate;
- no legacy DLL is required by default at 1.0;
- no proprietary asset is committed without written permission.
