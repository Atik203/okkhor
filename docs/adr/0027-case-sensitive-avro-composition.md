# 0027: Case-sensitive Avro composition

Status: Accepted (0.10). Date: 2026-11-26. Amends ADR 0022 for the roman
buffer's alphabet.

## Context

Avro is not a case-insensitive transliteration. Its key map gives a handful of
capitals their own Bangla forms:

- `T` → ট, `Th` → ঠ
- `D` → ড, `Dh` → ঢ, `DD` → ড্ড, `Dm` → ড্ম
- `R` → ড়, `Rh` → ঢ়
- `N` → ণ, `ND` → ণ্ড, `NG` → ঞ, `NN` → ণ্ণ, `NT` → ণ্ট, `NTh` → ণ্ঠ
- `S` → শ, `Sh` → ষ
- `I` → ঈ, `U` → ঊ, `O` → ও, `Y` → য়

Every other capital is the same key as its lowercase letter. Okkhor could not
type any of the first group: `InputSession::push_letter` folded every letter to
lowercase, so the engine never saw a capital.

Two deeper defects surfaced while fixing that:

1. The shipped `avro-v1.json` was a **derived** rule set, not Kontho's key map.
   Diffing the 250 rows showed only 3 in the same position, and the ordering,
   vowel rules, and capital digraphs differed.
2. The legacy key map expresses non-semantic capitals as `B -> b`, `K -> k`,
   ..., which emits a literal Latin letter when such a rule fires (`Bhalo`
   became `bহাল`).

## Decision

### Authentic table

`crates/okkhor-transliterate/assets/avro-v1.json` is generated from the
owner-licensed `Avro Phonetic (Custom).nmsLayout` v1.29 `keyMap` (250 rules, in
table order) and carries the extraction note in its `source` field.

The layout is a Python pickle. It is **never unpickled**: `scripts/import-avro-layout.py`
is a data-only reader that implements the subset of protocol-4 opcodes a data
container uses and refuses every opcode that can import or call code (`GLOBAL`,
`REDUCE`, `BUILD`, `OBJ`, `NEWOBJ`, `STACK_GLOBAL`, `EXT*`). Reading is
read-only and the file is not copied into the repository.

### Case reaches the buffer

- The hook still only reports a virtual key. The daemon derives the letter case
  as `shift XOR caps` (`okkhor_win::modifier_state().shift` and
  `caps_lock_on()`), which is what the target application would receive, and
  hands that letter to `push_letter`.
- The buffer stores case: it is one ASCII letter per element, so cursor and
  Backspace logic are unchanged.

### Non-semantic capitals normalize before matching

`normalize_case` lowercases capitals outside `TDNSRIUOY` before the rule
matcher sees them. That keeps `Bhalo`, `Kemon`, `Ghor` on the Bangla path
without touching the semantic set, and needs no table edits.

### Lookup gating

The suggestion index is keyed by lowercase roman. When the query contains a
semantic capital, the lowercase dictionary/typo/autocorrect stages are skipped
so `Taka` cannot be answered with the lowercase word তাকা; the rule path owns
that query. User words and the curated overlay still apply.

## Consequences

- ট, ঠ, ড, ঢ, ড়, ঢ়, ণ, ষ and the conjuncts built from them are now reachable
  from a normal keyboard.
- Sentence-initial capitals are Avro-semantic: `Dhonnobad` yields ঢন্নবাদ.
  That is the reference behavior; lowercase is how those words are typed.
- The measured corpus (`okkhor-bench --suggestions`, 138 everyday words) goes
  from 92.8 % to **100 % top-1** with the authentic table, case handling, and
  the shipped overlay; p99 stays inside the 3 ms suggestion budget.
- A future keyMap update is a data regeneration plus this import script, not a
  hand edit of the JSON.
