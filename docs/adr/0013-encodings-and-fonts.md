# ADR 0013: Bangla output encodings and bundled fonts

- Status: Accepted
- Date: 2026-11-23
- Relates to: ADR 0005 (legacy reuse), ADR 0012 (settings/store), ADR 0014
  (input modes, fonts)

## Context

Okkhor must produce text that works in two worlds:

1. modern Unicode apps (browsers, editors, chat, Windows itself);
2. legacy **Bijoy ANSI** documents used in government and corporate forms,
   where the text is ASCII bytes rendered by a legacy-layout font such as
   SutonnyMJ.

Banglish input must feed both. Users should not have to convert text by hand
or choose fonts on first run.

The owner supplied the legacy installation and approved read-only use of its
layout/encoding data. The `.nmsLayout`/`.nmsEnco` files are Python pickles; they
were parsed with a restricted, data-only unpickler (no globals, no code
execution) and converted to JSON. The Avro, Khipro, Banglish, and ANSI
font-encoding rule tables are publicly mirrored by the original author.

## Decision

### 1. One text pipeline, two encodings

```text
keys -> layout engine -> Unicode Bangla
                          ├─ Unicode output (default)
                          └─ Bijoy encoder -> ASCII bytes + ANSI font
```

- The phonetic engine always produces **Unicode** internally.
- `OutputEncoding` is `Unicode` or `Bijoy`. Bijoy conversion happens once, at
  commit, off the hot path.
- Bijoy output is **SutonnyMJ ANSI bytes plus the SutonnyMJ font choice**;
  there is no private binary encoding. The document's font decides rendering,
  so the bytes remain portable as long as SutonnyMJ is the renderer.
- The conversion table is the ordered 256-rule **`ANSI 1` font-encoding**
  map ("for SutoniMJ font"), applied exactly in its documented order
  (pre-base `ে`/`ৈ`/`ি` reordering with the `©` reph marker, `ো`/`ৌ`
  decomposition, `(?<!্)`-guarded `ৃ`/`ু`/`ূ` and `্র…` families, conjunct
  ligatures, digits, taka, punctuation).
- A best-effort ordered inverse (`from_bijoy`, 676-rule `ANSI 1 to Unicode`
  table applied in the same sequential way) exists for paste/import.
- The `Bangla to Bijoy layout` 111-rule table is a **keyboard layout**
  (which key types which char), not a font encoding, and must never be used
  as the output encoder. Keyboard layouts drive input; font encodings drive
  output. The 0.5 code confused the two; `bijoy-keyboard-v1.json` is kept
  for reference only.

### 2. Layouts

- Default phonetic: **Avro** (250 ordered rules, from the legacy table).
- Second phonetic: **Khipro/Kontho** (288 rules), same engine.
- **Banglish** (256 rules) as a variant.
- **Bijoy** positional layout (124 rules) for users who type by key position.
- The rule engine is table-driven; adding a layout is data, not code.
- Provenance: Avro rules originate from OmicronLab's Avro Phonetic (Mehdi Hasan
  Khan), modified by the legacy app author; Khipro from the public
  `khipro-m17n` project. Attribution is recorded with the asset.

### 3. Fonts

| Role | Default | License | Bundled |
| --- | --- | --- | --- |
| Unicode UI/editor | Kalpurush | SIL OFL 1.0 | yes |
| Bijoy ANSI (default) | SutonnyMJ | proprietary, owner-approved for release builds | staged (never committed) |
| Bijoy ANSI (fallback) | Kalpurush ANSI | Avro Free Bangla Font Project | yes |
| Fallback | Noto Sans Bengali | SIL OFL | yes |
| Optional Unicode | Siyam Rupali | GPL-2+ | optional user install |

> **Amendment 2026-11-23 (owner decision, Task 0c fix round 1):** the
> SutonnyMJ row above supersedes the original "Optional Bijoy / detected,
> never committed" row and the "Bundle SutonnyMJ by default. Rejected"
> alternative below. SutonnyMJ is now the default Bijoy font (staged at
> build time from `OKKHOR_LEGACY_HOME`, never committed); Kalpurush ANSI
> is its fallback. Detection covers staged *and* installed fonts
> (`okkhor-win::fonts::resolve_bijoy_font_with_probe`).

- Fonts are fetched at build time with pinned URLs and SHA-256 (SutonnyMJ
  is staged from `OKKHOR_LEGACY_HOME`), not committed as binaries.
- First run installs Kalpurush and Kalpurush ANSI per-user (no admin). Noto is
  the fallback if registration fails.
- SutonnyMJ is the default Bijoy font when staged or already installed
  (detected); Kalpurush ANSI is the fallback. The default Bijoy path no
  longer excludes SutonnyMJ (see amendment above).
- A document that must be opened on a machine without the chosen font may
  render differently; the bytes are unchanged. The UI documents this.

### 4. Safety, performance, privacy

- Conversion and layout matching run on the daemon worker, never in the hook
  callback, and are bounded (input ≤ 4096 chars for conversion; composition
  buffer 32).
- No text is logged; conversion errors carry codes, not content.
- Tables are compile-time assets; no network access at runtime.
- Invalid or unparsable tables fail closed with a typed error; the Unicode path
  remains available.

### 5. Packaging

- Portable build: fonts in `fonts/`, loaded privately or installed by the user
  from the tray.
- Installer: per-user font registration with idempotent uninstall; no elevation
  required; machine-wide install is an explicit advanced option.
- `THIRD_PARTY_LICENSES.md` lists Kalpurush (OFL), Noto (OFL), SutonnyMJ
  and Kalpurush ANSI (owner-approved distribution), and the derived
  legacy tables with attribution.

## Consequences

Positive:

- Banglish → Bijoy works out of the box for official forms.
- Unicode remains the default for modern apps.
- No proprietary font is required by default.
- Layouts and encodings are data, making corrections and additions cheap.

Negative:

- Two font families to ship and keep in sync.
- Bijoy inverse conversion is exact for the render goldens but not
  guaranteed in general (line-start `†`/`ˆ` forms, legacy quirks).
- Users may still need SutonnyMJ for a specific document; it is detected and
  selectable but not bundled.
- Legacy table ordering is subtle and needs golden tests.
- The converter depends on `fancy-regex`: the `regex` crate cannot express
  the table's `(?<!্)` lookbehind or `\1` backreferences, and rewriting the
  guards away would change match boundaries for overlapping sequential
  rewrites. `fancy-regex` is MIT, pure Rust, and applies the table verbatim.

## Alternatives considered

- **Only Unicode.** Rejected: official Bijoy workflows remain common.
- **Only Bijoy.** Rejected: breaks modern apps and search.
- **Convert by mapping each Unicode codepoint independently.** Rejected:
  pre-base vowel signs and conjuncts need reordering rules.
- **Use the `regex` crate by translating the lookbehind guards away.**
  Rejected: capture-and-re-emit prefixes change match boundaries for
  overlapping sequential rewrites and risk diverging from the legacy engine;
  `fancy-regex` (MIT, pure Rust) applies the shipped table verbatim.
- **Bundle SutonnyMJ by default.** Superseded 2026-11-23 by owner decision
  (see amendment in §3): SutonnyMJ is now staged as the default Bijoy font;
  Kalpurush ANSI remains the fallback for rendering. Correction 2026-11-23
  (this change): Kalpurush ANSI is a *degraded* fallback — the converter's
  ANSI 1 byte output is verified for SutonnyMJ only, not byte-identical
  across Bijoy fonts.
- **Use a font's glyph IDs as the encoding.** Rejected: non-portable and
  unmaintainable.
