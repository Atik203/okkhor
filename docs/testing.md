# Testing

## Test pyramid

| Layer | Purpose | Examples |
| --- | --- | --- |
| Unit | pure logic | normalization, ranking, decay, caps, snapshot schema |
| Property | invariants | Unicode round-trips, score monotonicity, no panics |
| Golden | language correctness | word/layout/sentence corpus |
| Differential | parity with legacy | old vs new transliteration and suggestions |
| Integration | Windows behavior | hook, injection, focus, DPI, IME |
| Performance | latency/memory | keydown -> suggestion, keydown -> injection |
| Fuzz | parser safety | dictionary, layout, snapshot, IPC |

## Golden corpus

Build a corpus covering:

- Bengali conjuncts, virama, hasant, ZWJ/ZWNJ, combining marks;
- independent vowels and vowel signs;
- Bangla digits and number words;
- English words, punctuation, emoji;
- Avro, National, Probhat, Bijoy, Phonetic, Khipro, and custom layouts;
- transliteration variants and user-specific pairs;
- sentence starts, punctuation, and corrections;
- empty, whitespace, mixed-script, and very long inputs.

The corpus must be synthetic or licensed for test use. Do not commit real user
messages.

## Differential testing

While the legacy app is the oracle:

1. Prepare a corpus and a driver that sends the same input to both apps.
2. Capture the committed text, suggestion order, and timing.
3. Diff the outputs and classify each difference.
4. Accept intentional improvements explicitly; do not hide them in a passing
   test count.

Run this in a disposable VM or with explicit owner approval. Never run the
legacy binary in CI.

## Property tests

Use `proptest` for:

- `normalize_lookup` never panics and is idempotent;
- rank scores are finite for valid configs;
- snapshot round-trip preserves counts;
- prune never exceeds a cap;
- Unicode cursor movement never splits a grapheme cluster;
- serialization rejects unknown schema versions.

## Fuzzing

Use `cargo-fuzz` for:

- snapshot JSON;
- dictionary/layout importers;
- IPC message decoding;
- clipboard and injection length bounds.

## Windows integration matrix

| Dimension | Values |
| --- | --- |
| OS | Windows 11 24H2, 25H2 |
| DPI | 100%, 125%, 150%, 200% |
| Monitors | single, mixed-DPI dual, portrait |
| Target apps | Notepad, VS Code, browser, Office, terminal, Electron |
| Privilege | normal, elevated target, UAC prompt |
| Input | IME on/off, different layouts, RDP, touch keyboard |
| Power | balanced, battery saver |
| Session | local, RDP, fast user switching |

The table above is the coverage summary. The executable pass/fail matrix,
evidence rules, release gate checklist, and defect template live in
[qa/0.2-matrix.md](qa/0.2-matrix.md). Start with the
[manual QA guide](qa/README.md) before running a row.

## CI

CI runs:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
cargo run --package okkhor-daemon -- self-check
```

CI does not run global hooks or the legacy app. Those belong in a dedicated
Windows test VM with explicit approval.

## Tray and autostart checks

Manual smoke tests in a disposable session:

- `okkhor tray-demo`: right-click for Pause/Resume, disabled Settings, Exit;
  confirm the icon disappears on exit.
- `okkhor daemon`: confirm the tray Pause item and the pause hotkey both change
  capture state, then exit from the tray.
- `okkhor autostart status`: read-only check of the per-user `Run` value.
  `enable` and `disable` change `HKCU`; do not run them in automated tests.
- Inspect `%LOCALAPPDATA%\Okkhor\logs\okkhor.log` after a daemon start and
  confirm it contains only `startup`, `command`, and `error` records with
  allowlisted fields.

## Privacy tests

- Assert logs never contain fixtures marked as sensitive.
- Assert learning snapshots are encrypted at rest.
- Assert clear deletes memory and disk data.
- Assert password-field detection fails safe (pause, not learn).
- Assert crash reports contain no text fields.

## Definition of done

- New behavior has a test at the lowest useful layer.
- A bug fix has a regression test.
- A hot-path change has before/after numbers.
- A parser change has malformed-input coverage.
- A compatibility change has a differential result.
