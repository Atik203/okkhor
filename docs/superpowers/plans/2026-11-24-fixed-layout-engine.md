# Fixed Layout Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship all six keyboard layouts as genuine, verified input engines in Okkhor 0.9.0 without destabilizing the existing Avro phonetic path.

**Architecture:** Keep the phonetic engine and add a second, independent fixed-key engine in `okkhor-lang`. A daemon dispatcher selects the engine from `settings.layout`, falling back to Avro. In fixed mode the hook swallows letters and the daemon injects the mapped Bangla character directly, with re-emit on failure.

**Tech Stack:** Rust 2024, `serde_json`, `okkhor-transliterate` regex pipeline, Win32 `SendInput` through `okkhor-win`, Vitest/Tauri UI.

**Spec:** `docs/superpowers/specs/2026-11-24-fixed-layout-engine-design.md`

## Global Constraints

- No runtime disk I/O, no pickle, no copied reference asset, no network, no AI on the hot path.
- Layout assets are embedded with `include_str!` and validated at startup; failure falls back to Avro with an allowlisted health event.
- `unsafe` only in `okkhor-win`, always with a `// SAFETY:` comment.
- `okkhor-core`, `okkhor-learn`, `okkhor-storage` remain `#![forbid(unsafe_code)]`.
- No new crate, process, IPC protocol, or dependency.
- Hook callback still only timestamps and enqueues; modifier state is read on the worker.
- Fixed layouts show no popup and do not feed learning or the OKS1 roman index.
- Every change ends with `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --locked`.

## Review Focus

| Input / condition | Expected behavior | Owning task |
| --- | --- | --- |
| AltGr held with a letter that has no AltGr entry | Falls back through Shift then Normal, never injects a control character | Task 3 |
| `g` followed by `s` and then another key in National | Longest-first sequence wins; buffer clears on the third key | Task 3 |
| Injection into an elevated or UIPI-blocked window | Original ASCII letter is re-emitted, no partial state | Task 6 |
| Windows input language set to Bengali with an IME active | Fixed layout still injects exactly one Bangla character because the raw letter is swallowed | Task 7 |
| Focus change or pause while a sequence is pending | Sequence buffer resets; no stale prefix affects the next character | Task 7 |
| Unknown or corrupt layout setting | Avro fallback plus health event; app still types | Task 5 |

---

### Task 1: Fixed layout asset schema and National/Jatiya asset

**Files:**
- Create: `crates/okkhor-lang/assets/layouts/fixed-national-v1.json`
- Create: `crates/okkhor-lang/tests/fixed_layout_assets.rs`

**Interfaces:**
- Consumes: public BCC National/Jatiya layout facts, verified against `E:\Nms_Kontho\avro_portable\Keyboard Layouts\National (Jatiya).avrolayout` (203 `KeyData` entries) and Kontho's National `keyMap` (`n → স`, `h → া`). A read-only extraction of those verified key facts is available for this implementation session at `C:\Users\atiku\AppData\Local\Temp\opencode\avro-national-map.json`; it is working data, not a shipped asset.
- Produces: an asset with `name`, `version`, `kind: "fixed"`, provenance fields, `rule_count`, `keys` object with `normal`/`shift`/`altgr`/`shift_altgr` maps, and `sequences` array. `Key_N_Normal` is `স`, `Key_H_Normal` is `া`, `Key_A_Normal` is `ৃ`, `Key_G_Normal` is `্`, `Key_1_Normal` and `Num1` are `১`, `Key_OEM3_Normal` is U+200C.

- [ ] **Step 1: Write the failing asset test**

```rust
#[test]
fn national_asset_has_valid_fixed_schema_and_golden_keys() {
    let raw = include_str!("../assets/layouts/fixed-national-v1.json");
    let value: serde_json::Value = serde_json::from_str(raw).expect("valid JSON");
    assert_eq!(value["kind"], "fixed");
    assert_eq!(value["rule_count"], 203);
    assert_eq!(value["keys"]["normal"]["Key_N_Normal"], "স");
    assert_eq!(value["keys"]["normal"]["Key_H_Normal"], "া");
    assert_eq!(value["keys"]["normal"]["Num1"], "১");
}
```

- [ ] **Step 2: Run the test to verify it fails** — `cargo test -p okkhor-lang --test fixed_layout_assets`; expect missing-file failure.
- [ ] **Step 3: Create the asset** from the extracted 203-entry National map, adding the documented sequence rules from the public standard (e.g. `g,s → উ`) and provenance fields.
- [ ] **Step 4: Run the test to verify it passes.**
- [ ] **Step 5: Commit** — `git commit -m "feat(0.9): add verified National fixed-layout asset"`

### Task 2: Probhat fixed asset and Bijoy mapping decision

**Files:**
- Create: `crates/okkhor-lang/assets/layouts/fixed-probhat-v1.json`
- Modify: `crates/okkhor-lang/tests/fixed_layout_assets.rs`

**Interfaces:**
- Consumes: Avro's `Probhat.avrolayout` (203 entries; `Key_N_Normal = ন`, `Key_H_Normal = হ`, `Key_A_Normal = া`). The read-only extraction is at `C:\Users\atiku\AppData\Local\Temp\opencode\avro-probhat-map.json` for this implementation session; it is working data, not a shipped asset.
- Produces: `bijoy` is documented as National keys + the existing Bijoy output encoding; no separate input asset.

- [ ] **Step 1: Write the failing test** asserting `kind == "fixed"`, `rule_count == 203`, and the three golden keys above.
- [ ] **Step 2: Run it to verify failure.**
- [ ] **Step 3: Create the asset** with provenance and NFC-normalized outputs.
- [ ] **Step 4: Run it to verify success.**
- [ ] **Step 5: Commit** — `git commit -m "feat(0.9): add verified Probhat fixed-layout asset"`

### Task 3: FixedLayout resolver, key identity, modifiers and sequences

**Files:**
- Create: `crates/okkhor-lang/src/fixed_layout.rs`
- Modify: `crates/okkhor-lang/src/lib.rs`
- Test: `crates/okkhor-lang/src/fixed_layout.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: Task 1/2 assets.
- Produces: `pub struct ModifierSet { pub shift: bool, pub altgr: bool }`; `pub fn from_asset_json(json: &str) -> Result<FixedLayout, FixedLayoutError>`; `pub fn vk_to_key_name(vk: u16) -> Option<&'static str>`; `FixedLayout::resolve(&self, key: &str, modifiers: ModifierSet) -> Option<&str>`; `FixedLayout::resolve_sequence(&self, keys: &[&str]) -> Option<&str>`.

- [ ] **Step 1: Write failing tests** for VK mapping (`0x41 → "Key_A"`, `0x31 → "Key_1"`, `0x60 → "Num0"`), modifier priority (ShiftAltGr > AltGr > Shift > Normal), unmapped-key `None`, longest-first sequence, and NFC output.
- [ ] **Step 2: Run tests to verify failure.**
- [ ] **Step 3: Implement the resolver** with a static VK→name table, validated parse, and bounded longest-first sequence lookup.
- [ ] **Step 4: Run tests to verify success.**
- [ ] **Step 5: Commit** — `git commit -m "feat(0.9): add fixed layout resolver with modifier and sequence support"`

### Task 4: Alternate phonetic table compilation

**Files:**
- Modify: `crates/okkhor-lang/src/lib.rs`
- Modify: `crates/okkhor-engine/src/suggest/mod.rs`
- Test: existing `crates/okkhor-engine/tests/suggestions.rs`

**Interfaces:**
- Consumes: `khipro-v1.json` and `phonetic-v1.json` (`{key, output}` rule shape).
- Produces: `pub fn layout_rules_to_asset_json(json: &str) -> Result<String, FixedLayoutError>`; `SuggestionEngine::from_embedded_table(table_json: &str) -> Result<Self, SuggestionError>`.

- [ ] **Step 1: Write a failing test** that compiles the Khipro table and transliterates `nftff → ণ্ঠ`.
- [ ] **Step 2: Run it to verify failure.**
- [ ] **Step 3: Implement the `{key, output}` → `{pattern, replacement}` adapter** and engine constructor; preserve embedded Avro as the default.
- [ ] **Step 4: Run the engine suite to verify success and unchanged Avro golden rows.**
- [ ] **Step 5: Commit** — `git commit -m "feat(0.9): compile alternate phonetic layout tables"`

### Task 5: LayoutEngine dispatcher and fallback

**Files:**
- Create: `apps/okkhor-daemon/src/layout_engine.rs`
- Modify: `apps/okkhor-daemon/src/main.rs`
- Test: `apps/okkhor-daemon/src/layout_engine.rs`

**Interfaces:**
- Consumes: Tasks 3/4.
- Produces: `pub enum LayoutEngine { Phonetic(Box<SuggestionEngine>), Fixed(Box<FixedLayout>) }`; `pub fn from_settings(code: &str, ranker: std::sync::Arc<dyn okkhor_core::Ranker + Send + Sync>) -> (Self, Option<&'static str>)`; `pub fn is_fixed(&self) -> bool`.

- [ ] **Step 1: Write failing tests** for every settings code (`avro`, `khipro`, `phonetic`, `banglish`, `national`, `probhat`, `bijoy`) and for an unknown code returning Avro plus a health code.
- [ ] **Step 2: Run tests to verify failure.**
- [ ] **Step 3: Implement the dispatcher** with compile-once tables and no hot-path I/O.
- [ ] **Step 4: Run tests to verify success.**
- [ ] **Step 5: Commit** — `git commit -m "feat(0.9): add layout engine dispatcher with Avro fallback"`

### Task 6: Hook letter swallowing, modifier query and injection fallback

**Files:**
- Modify: `crates/okkhor-win/src/win32.rs`
- Modify: `crates/okkhor-win/src/input.rs`
- Modify: `crates/okkhor-win/src/lib.rs`
- Test: `crates/okkhor-win/src/win32.rs` and `input.rs`

**Interfaces:**
- Consumes: none.
- Produces: `pub fn set_swallow_letters(enabled: bool)`; `pub fn modifier_state() -> ModifierFlags` with `shift` and `altgr` booleans; `pub fn inject_char(ch: char) -> Result<(), InjectError>`; `should_swallow_letters(...)` pure helper in `input.rs`.

- [ ] **Step 1: Write failing tests** for the swallow helper matrix and `inject_char` building one Unicode down/up pair with zero backspaces.
- [ ] **Step 2: Run tests to verify failure.**
- [ ] **Step 3: Implement** the state flag, key-up pairing, worker-side `GetAsyncKeyState` query, and character injection.
- [ ] **Step 4: Run `cargo test -p okkhor-win` to verify success.**
- [ ] **Step 5: Commit** — `git commit -m "feat(0.9): add fixed-layout swallowing and character injection"`

### Task 7: Daemon fixed-layout typing path

**Files:**
- Modify: `apps/okkhor-daemon/src/main.rs`
- Test: `apps/okkhor-daemon/src/main.rs` and `apps/okkhor-daemon/tests/`

**Interfaces:**
- Consumes: Tasks 5/6. Task 6's `ModifierFlags { shift, altgr }` is converted into Task 3's `ModifierSet { shift, altgr }` at the call site; both are plain boolean pairs.
- Produces: `FixedComposition { sequence: Vec<&'static str>, clear() }` and a pure `fixed_char_for(key, modifiers: ModifierSet) -> Option<String>` used by `handle_key`.

- [ ] **Step 1: Write failing tests** for sequence longest-first, buffer reset on focus change/pause/mode switch, unmapped-key re-emit, and no popup calls in fixed mode.
- [ ] **Step 2: Run tests to verify failure.**
- [ ] **Step 3: Implement** the fixed branch in the letter and digit arms, re-emit on injection failure, and suppress popup refresh while fixed.
- [ ] **Step 4: Run daemon tests and self-check to verify success.**
- [ ] **Step 5: Commit** — `git commit -m "feat(0.9): wire fixed-layout typing into the daemon"`

### Task 8: UI selector and settings compatibility

**Files:**
- Modify: `apps/okkhor-daemon/src/ipc.rs`
- Modify: `apps/okkhor-ui/src/lib/keyboard-data.ts`
- Modify: `apps/okkhor-ui/src/lib/settings-schema.ts`
- Modify: `apps/okkhor-daemon/tests/ipc_roundtrip.rs`

**Interfaces:**
- Consumes: Task 5 dispatcher.
- Produces: `layout_list()` advertising `avro`, `phonetic`, `national`, `probhat`, `bijoy`, `khipro`.

- [ ] **Step 1: Write failing tests** for the six advertised codes and unchanged canonicalization of legacy `banglish`.
- [ ] **Step 2: Run tests to verify failure.**
- [ ] **Step 3: Implement** the list and fallback UI data with accurate display names.
- [ ] **Step 4: Run Rust and UI tests to verify success.**
- [ ] **Step 5: Commit** — `git commit -m "feat(0.9): expose working layouts in the selector"`

### Task 9: ADR, full documentation pass, QA, 0.9.0 release

**Files:**
- Create: `docs/adr/0021-fixed-layout-engine.md`, `docs/releases/0.9.0.md`, `docs/qa/0.9-checklist.md`
- Modify: all stale `.md` files and `README-BETA.txt`; bump every version source to `0.9.0`.

**Interfaces:**
- Consumes: Tasks 1–8.
- Produces: a released, documented 0.9.0 beta.

- [ ] **Step 1: Write the ADR** recording the two-engine decision, swallowed-letter rationale, and Bijoy alias decision.
- [ ] **Step 2: Update every Markdown file and shipped text file** whose content contradicts the new behavior; leave historical release notes intact.
- [ ] **Step 3: Bump versions** in root `Cargo.toml`, Tauri `Cargo.toml`, `package.json`, `tauri.conf.json`; regenerate all three locks without touching third-party versions.
- [ ] **Step 4: Run all gates**: fmt, clippy, workspace tests, UI tests, daemon self-check, index checksum, `qa-0.9.ps1` equivalent, MSVC static-CRT build, fresh-extraction smoke.
- [ ] **Step 5: Commit, tag `v0.9.0`, push, publish the release** with SHA-256 and real-key QA rows marked as owner work.
- [ ] **Step 6: Commit** — `git commit -m "release: Okkhor 0.9.0"`
