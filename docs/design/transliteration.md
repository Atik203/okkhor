# Transliteration: Avro phonetic pipeline

Owner: Task 0a (milestone 0.5). Status: implemented in
`crates/okkhor-transliterate` (`src/phonetic.rs`,
`assets/avro-v1.json`).

The rule table is hand-extracted data, not copied code: the 250-row
`keyMap` was read from the owner's licensed `Avro Phonetic (Custom)`
installation (modified Avro Phonetic by Mehdi Hasan, OmicronLab, adapted
by Nabil for Nms Kontho) and stored as an ordered JSON array. No legacy
code was executed or copied; no network, no I/O at runtime.

## Rule model

- One ordered list of `(pattern, replacement)` rows; file order is
  application order (`assets/avro-v1.json`, `rule_count: 250`).
- Each pattern is a small regex: escaped literals (`\.` → `।`,
  `\$` → `৳`), alternations (`m|M`), character classes of Bengali
  consonants, `^` anchors, a `;+` quantifier, and `\1` captures in
  replacements (converted to `$1` for the `regex` crate at load).
- Matching uses the `regex` crate (new dependency, justified: capture
  groups, alternation, anchors; compilation happens once at load, off the
  typing path). Conversion itself allocates only bounded `String`s.

## Segmentation and application

- Input is split into whitespace-delimited chunks; spacing is preserved
  exactly. Conversion runs left-to-right **per chunk**.
- At each position every rule is tried anchored there. The **longest
  match wins**; table order breaks ties. The matched input length is
  consumed, so every step advances and conversion always terminates.
- Context rules such as `([কখ...])a` inspect the **last already-built
  output character**, not the raw input: on a match the last output
  character is replaced by the expanded replacement (so `ক` + `a`
  becomes `কা`). `^` rules (`^o`, `^a|^A`, `^X`) only match at chunk
  start. `rr([..])` requires a literal `rr` in the input *and* a
  matching last output character.
- A first-byte dispatch index (input byte → candidate options, built in
  table order) keeps conversion to a handful of regex probes per
  character. Unmatched characters (emoji, Bengali input, uncovered
  punctuation) pass through unchanged.
- Bounds: input over 64 chars (or ≥256 bytes) yields no output; output is
  capped at 256 chars. Empty/whitespace input yields no output. The
  engine is infallible and deterministic.

## Table semantics that must not be "corrected"

- Lowercase `o` after a consonant is the inherent vowel (deleted):
  `bhalo` → ভাল, `kalo` → কাল, `acho` → আছ. Capital `O` is the ো-kar:
  `bhalO` → ভালো, `kalO` → কালো.
- `gy` → গ্য (জ্ঞ comes from `gg`/`jNG`). `ng` → ং at a syllable end,
  while `ngg`/`ngk`/`nga` win where they apply (longest match).
- Conjunct rows apply where they match: `kkh` → ক্ষ wins inside
  `okkhor` → অক্ষ্র; `tt` → ত্ত but `([..])r` reph can follow
  (`uttor` → উত্ত্র). `apni` → আপ্নি, `dhonnobad` → ধন্নবাদ.
- Digits map to Bangla digits (`123` → `১২৩`); `t``` → ৎ, `:` → ঃ,
  `,,` → hasanta+ZWNJ. `w` → ও, `z` → য, `x` → ক্স after a consonant
  (bare `x` → এক্স).

## Known gaps

- `^` anchors mean chunk (word) start, not sentence start handling
  beyond whitespace splitting.
- `A` after a consonant maps to bare `া`, dropping the consonant
  (`([..])A` → `া` has no `\1` in the legacy table); preserved as-is.
- Singleton retroflex vs dental (`T`/`t`, `D`/`d`, `N`/`n`, `S`/`s`,
  `Sh`/`sh`, `Y`/`y`) follows the table's case-sensitive rows; there is
  no case folding.
- Words needing dictionary correction (আপনি, ধন্যবাদ, মিষ্টি, বৃষ্টি,
  ডাল/ডিম, বাড়ি/গাড়ি, চাঁদ, সূর্য) are asserted as the table's literal
  output here; a dictionary/spell layer in a later task will correct
  them. Golden tests are split accordingly: a table-exact set plus
  comments where common expectation differs.

## Golden corpus

`crates/okkhor-transliterate/tests/fixtures/synthetic/phonetic_golden.json`
(schema 1, 82 cases: the 17 required verifications, vowel-sign vs
independent-vowel pairs, conjuncts, digits/punctuation, a sentence, and
the table-exact forms above). `tests/phonetic.rs` additionally covers:
invalid-pattern load failure, rule-count validation, bounds,
determinism, 200 synthetic strings, `into_candidate()`, and a p99
latency smoke (<1 ms for 32 chars on release).

## Reuse for Khipro/Phonetic

`PhoneticTransliterator::from_patterns` / `::from_asset_json` accept any
table in the same shape, so the Khipro (288 rules) and Phonetic
(256 rules) dumps can load through the identical engine later; only the
asset file changes.
