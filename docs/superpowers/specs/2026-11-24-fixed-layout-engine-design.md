# Design: fixed-layout keyboard engine

Status: approved in conversation on 2026-11-24. Author: Okkhor engineering.
Target release: 0.9.0. This spec is the source of truth for the layout engine;
`docs/superpowers/plans/0.8.1-layout-engine.md` remains the evidence file that
motivated it.

## 1. Problem

Okkhor 0.8.1 has one real input engine: the 250-rule Avro phonetic table. The
layout selector offers only `avro` because every other advertised code in 0.8.0
ran the same table. The three reference keyboards all switch real data at
runtime:

- Avro ships five XML `.avrolayout` fixed-keyboard files keyed by physical key
  and modifier, plus a built-in phonetic engine.
- Kontho ships nine pickle `.nmsLayout` files with a `keyMap`, a `phonetic`
  boolean and a `dic` selector; National is non-phonetic and maps `n → স`,
  `h →া`.
- Bongo embeds JSON fixed layouts (`National.json`, `Borno.json`,
  `Probhat.json`) with `include_str!` and swaps `PhoneticMethod` for
  `FixedMethod` when the layout changes.

The owner wants all layouts to work in one release, to be more stable and
accurate than those implementations, and to handle edge cases.

## 2. Goals and non-goals

Goals:

- Ship working fixed layouts for National/Jatiya, Probhat and Bijoy, and real
  alternate phonetic tables for Khipro and Phonetic/Banglish, in one release.
- Never load a pickle, never copy a reference asset into git, and never read a
  layout file on the typing hot path.
- Make the fixed path safer than the references: the hook swallows the raw
  letter so a Bengali IME cannot compose it into a different character before
  Okkhor replaces it.
- Keep the Avro phonetic path and its suggestion behavior unchanged.

Non-goals:

- No next-word suggestions in fixed layouts in this release. The popup stays
  hidden there; a later release may add a next-word strip on committed words.
- No local learning from fixed-layout characters.
- No model training, no GPU work, no network calls.
- No new process, crate, or IPC protocol.

## 3. Decisions from owner review

| Question | Decision |
| --- | --- |
| First-release scope | All layouts in one release |
| Data provenance | Public standards, verified statically against the owner's reference installs; no reference file is copied or unpickled at runtime |
| Suggestions in fixed layouts | No word list; popup stays hidden, next-word strip is later work |
| Implementation approach | Two engine paths behind one layout-aware dispatcher (Approach A) |
| Documentation | Final step updates every `.md` file and shipped text file |

## 4. Architecture

```
settings.layout ─► LayoutEngine::from_settings()
                         │
          ┌──────────────┴───────────────┐
          ▼                              ▼
   Phonetic(SuggestionEngine)      Fixed(FixedLayout)
   Avro / Khipro / Phonetic        National / Probhat / Bijoy
   - popup with candidates         - no popup
   - composition buffer            - key + modifier -> Bangla
   - commit on Space               - direct injection
```

The dispatcher lives in `apps/okkhor-daemon/src/layout_engine.rs`. The existing
`SuggestionEngine` API is reused for all phonetic tables. The fixed layout is a
new, independent module in `crates/okkhor-lang/src/fixed_layout.rs`. Neither
depends on the other.

### 4.1 `okkhor-lang::fixed_layout`

- `FixedLayout` is built once from an embedded JSON string. Parsing validates
  `kind == "fixed"`, `rule_count`, key identifiers, modifier names, and that
  every output is non-empty NFC Bengali or punctuation.
- Key identity is a physical-key name (`Key_A`, `Key_N`, `Num1`, `Key_OEM3`,
  …), not a character. A pure function maps a Win32 virtual key to that name.
- `resolve(key, modifiers) -> Option<Cow<str>>` applies strict priority:
  ShiftAltGr, AltGr, Shift, Normal. An unmapped combination returns `None`.
- `sequences` are bounded ordered multi-key rules (at most three keys) matched
  longest-first before single-key resolution. A sequence buffer is owned by the
  daemon and cleared on non-letter key, focus change, mode switch, pause, and
  commit.
- All outputs are normalized to NFC at build time. Zero-width joiners and
  non-joiners are permitted only where a conjunct rule needs them.

### 4.2 Layout assets

New assets under `crates/okkhor-lang/assets/layouts/`:

| Asset | Kind | Source and verification |
| --- | --- | --- |
| `fixed-national-v1.json` | fixed, 203 key slots | Public BCC National/Jatiya standard; verified against Avro's `National (Jatiya).avrolayout` XML and Kontho's `National (Jatiya).nmsLayout` keyMap |
| `fixed-probhat-v1.json` | fixed, 203 key slots | Public Probhat layout; verified against Avro's `Probhat.avrolayout` and Bongo's `Probhat.json` |
| `bijoy` (setting) | fixed National key map + Bijoy output encoding | Matches Bongo's `bijoy` alias; no separate input table is invented |
| `khipro-v1.json` | phonetic, existing | Compile through the existing transliterator with a field adapter |
| `phonetic-v1.json` | phonetic, existing | Same adapter; renamed from the legacy `banglish` code |

Every asset carries `name`, `version`, `kind`, provenance fields, `rule_count`
and a human-readable description. No asset contains a reference install path.

### 4.3 Phonetic table selection

`PhoneticTransliterator` already parses `{pattern, replacement}` JSON. Add an
adapter that converts the stored `{key, output}` shape into that shape and
compiles it once at startup. A failed compile falls back to embedded Avro and
emits an allowlisted health event; it never fails startup.

### 4.4 Hook and injection changes

- `okkhor_win` publishes a `swallow_letters` flag. In fixed mode the hook drops
  letter keydowns and their matching keyups, using the existing swallowed-key
  mechanism so no phantom keyup remains.
- The daemon queries modifier state on the worker, not in the hook callback,
  so the hook still only timestamps and enqueues.
- A mapped character is delivered as one `SendInput` batch with zero
  backspaces. If injection fails, the daemon re-emits the original ASCII
  letter so no keystroke is lost, and logs an allowlisted code only.
- Space, Enter and Backspace are never swallowed in fixed mode and are not
  composition keys there.
- Digits use the layout's Num/top-row entries and produce Bangla numerals.

### 4.5 Runtime selection and reporting

`LayoutEngine::from_settings(layout, ranker, now_ms)` returns:

- `Phonetic` for `avro`, `khipro`, `phonetic`, and legacy `banglish`;
- `Fixed` for `national`, `probhat`, `bijoy`;
- `Phonetic(Avro)` plus a health event for any unknown code.

The `layout` field remains in `settings.json` and `status` with the same values.
A layout change clears the sequence buffer and hides the popup. The health
event reports only the resolved layout code, never typed text.

## 5. Error handling and privacy

- No layout data is read from disk at runtime; all tables are embedded.
- Parsing errors are typed and reported through the existing allowlisted
  logger; no user text or path is logged.
- Injection failure re-emits the raw ASCII key and leaves no half-state.
- Focus changes, pause, mode switch, and output-encoding change reset the
  sequence buffer.
- Fixed-layout characters are not learned as phonetic pairs and are not sent
  anywhere.
- Password fields behave like any other field for character insertion; no
  popup is shown and no learning occurs.

## 6. Testing strategy

- **Asset tests:** rule counts, key/mapping validity, NFC, provenance fields,
  and exact expected values for a golden set per layout.
- **Pure unit tests:** VK → key identity, modifier priority matrix, NumPad and
  top-row digits, longest-first sequence matching, unmapped-key fallback.
- **Daemon dispatcher tests:** every settings code resolves to the expected
  engine; unknown codes fall back with a health event; layout switch clears
  sequences; injection failure re-emits ASCII.
- **Reference differential:** static golden rows captured from the reference
  installs, used as compatibility evidence, not copied into shipped code.
- **Real-key QA:** new rows in `docs/qa/0.9-checklist.md` for each layout,
  modifier combinations, sequences, IME-on, elevated target, focus change,
  pause, and Bijoy output encoding.

## 7. Documentation and release

The final implementation step updates every Markdown file and shipped text
file whose content is now stale, including `CHANGELOG.md`, `README.md`,
`README-BETA.txt`, `docs/architecture.md`, `docs/roadmap.md`,
`docs/input-pipeline.md`, `docs/compatibility.md`, `docs/parity/0.8-matrix.md`,
`docs/qa/`, `docs/release.md`, ADR 0021, the implementation plan, and a new
`docs/releases/0.9.0.md`. Historical release notes remain historical.

Release gates: `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings`,
`cargo test --workspace --locked`, UI build/tests, daemon self-check, MSVC
static-CRT portable build, fresh-extraction artifact smoke, and real-key QA.
The release is unsigned unless a trusted Authenticode certificate is supplied.

## 8. Risks

- A wrong physical-key map would silently type the wrong character. Mitigated
  by golden tests and static differential verification before release.
- Swallowing letters changes established behavior. Mitigated by re-emit on
  failure and by keeping the flag strictly scoped to fixed layouts.
- Khipro/Phonetic rule tables are regex-shaped data of unknown quality.
  Mitigated by keeping Avro as the fallback and by golden tests per table.
