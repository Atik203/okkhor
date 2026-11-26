# Changelog

All notable changes to Okkhor are recorded here. The project follows Semantic
Versioning once it reaches 1.0.

## [0.9.11] - 2026-11-27

### Added

- **Exit in the quick bar.** A power button (two clicks) asks the daemon to
  shut down, waits for the pipe to close, force-stops an unresponsive engine,
  then exits the UI. The tray icon can no longer outlive an explicit exit.

### Fixed

- **Shortcuts stopped showing suggestions.** The worker re-read the modifier
  state when it drained the queue, so a fast Ctrl+Shift+S could be processed
  after Ctrl was released and start a composition. The hook now records
  `chord` on the key event; a chorded key cancels the composition and is never
  treated as a letter, digit, or commit.
- **The next-word ghost no longer lingers on mouse clicks.** It only shows in
  the window that received the commit and disappears after four seconds, so
  clicking the tray or the browser address bar cannot leave it behind.
- **Settings opens when Windows refuses focus.** The click used to fail
  silently because `set_focus` returned an error; showing, unminimizing and
  navigating now happen regardless.
- **Add-last-word works with learning off.** The last (roman, Bangla) pair is
  remembered for the explicit `+` action even when learning is disabled; it is
  still memory-only and never written or logged.

## [0.9.10] - 2026-11-26

### Fixed

- **Backspace deletes one step again.** While inline preedit was active the hook
  did not swallow Backspace, so the target application deleted a character
  natively *and* the daemon re-rendered the shorter word: one keypress removed
  two or three characters, and after two presses the whole word was gone while
  the suggestion list still showed a longer word. Backspace is now swallowed
  whenever the composition owns the on-screen word (inline preedit on, rendered
  text present, no chord) and passes through untouched when the document holds
  raw roman text or no composition is active.

## [0.9.9] - 2026-11-26

### Fixed

- **The quick bar can be resized again.** 0.9.7 fixed the bar at 340x52 and
  dropped the right-edge handle. The handle is back: a native window resize drag
  (no poll loop) between 300 and 560 px, height held at 52, and the width is
  remembered locally. It is a window behavior, not a control button, so the bar
  still carries only the five typing-surface targets.

## [0.9.8] - 2026-11-26

### Added

- **Measured Bangla accuracy harness.** `okkhor-bench --suggestions` now runs
  a 138-word everyday corpus and prints every top-1 miss with its roman input,
  so accuracy moves are visible instead of anecdotal. Baseline after the fixes:
  top-1 98.6 %, top-5 99.3 %, p50 1.5 ms / p99 3.3 ms.
- **Shipped spelling corrections.**
  `crates/okkhor-engine/assets/suggest/curated-overlay.tsv` holds words where
  the Avro rules and the licensed aliases both give a different form than the
  everyday spelling (for example `chokh` -> ছখ). Rows stage with the curated
  score, and a test asserts every overlay key wins top-1.

### Fixed

- **Capitals now type their own letters.** Avro gives `T`, `Th`, `D`, `Dh`,
  `R`, `Rh`, `N`, `S`, `Sh`, `I`, `U`, `O` and `Y` distinct Bangla forms, but the
  composition buffer folded every letter to lowercase, so ট, ঠ, ড, ঢ, ড়, ঢ়, ণ
  and ষ were unreachable from the keyboard. Case is now taken from
  `shift XOR caps`, preserved in the buffer, and honored by the engine; the
  non-semantic capitals (`Bhalo`, `Kemon`, `Ghor`) normalize to their lowercase
  key instead of leaking a literal Latin letter (ADR 0027).
- **The rule table is now Kontho's key map.** `avro-v1.json` was a derived set
  that disagreed with the owner-licensed `Avro Phonetic (Custom).nmsLayout`
  v1.29 in 247 of 250 rows. It is regenerated from that file with a data-only,
  no-unpickle reader (`scripts/import-avro-layout.py`), and the everyday corpus
  goes from 92.8 % to 100 % top-1 with p99 inside the 3 ms budget.
- **Capitalized input no longer resolves to the lowercase word.** Dictionary
  stages are skipped when the query contains a semantic capital, so `Taka`
  yields টাকা rather than তাকা.
- Corpus rows that were wrong in the harness itself (`bolo`, `ghumi`,
  `sontan`, `poRi`, `ki`) so a miss means an engine gap, not spelling noise.

## [0.9.7] - 2026-11-26

### Added

- **Mid-word editing shows where you are.** Left/Right inside a Bangla word
  moves the composition point, and the caret is projected into the rendered
  word instead of staying at its end. The caret is parked back before every
  rewrite, so erase counts stay measured from the word end; when a
  transliteration is not prefix-stable, the caret stays at the end rather than
  guessing an offset (ADR 0026, amending ADR 0022).

### Changed

- **The quick bar is a typing surface, not a control panel.** It shrinks from
  460×60 to a fixed 340×52 and keeps five targets: drag grip, mode, pause,
  add-last-word, settings. Encoding, the on-screen keyboard, hide-to-tray and
  quit moved to Settings or the tray, and the drag-to-resize handle is gone.
- **The toggle click is audible.** The embedded click peaked at 45 % of full
  scale (−7 dBFS), which was faint next to system sounds. It is normalized to
  92 % (+6.3 dB, RMS −12.9 dBFS) with the same 18 ms waveform;
  `scripts/make-sounds.ps1` regenerates it.

### Fixed

- **The suggestion card no longer covers the line you are typing on.** Rows are
  capped to the vertical room on the larger side of the caret before layout, so
  a list near the bottom of the screen flips above and shrinks instead of
  clamping over the text. Placement also takes the larger side when neither
  side holds the card.
- **Quick-bar notices are announced once.** The overlay pill and the old inline
  label both rendered the same message in a `role="status"` region, so screen
  readers read it twice. The stale inline copy is gone.

## [0.9.6] - 2026-11-25

### Fixed

- **Quick-bar feedback is always visible.** The inline notice lived in the
  shrinking middle of a full 460 px bar, so the **＋** action and the offline
  message could land in a zero-width area and show nothing. They now render as a
  centered overlay pill, and the snapshot query fails after two seconds instead
  of hanging, naming the offline state.

## [0.9.5] - 2026-11-25

### Fixed

- **`kor…` words keep their vowel.** The phonetic table turned any `consonant + r`
  into ra-phala, so `korte` rendered as `ক্রতে` and `kortechi` could never become
  `করতেছি`. The pipeline now tracks the last consumed input character and applies
  that rule only when no vowel precedes the `r`. Golden rows `উত্তর` and `অক্ষর`
  were the same bug and are corrected.
- **Backspace is one keystroke of undo.** The session snapshots buffer, candidates
  and selection before every letter, so Backspace restores the previous state
  instead of re-running the engine from a shorter buffer. A shortened buffer with
  no candidates can no longer replace or erase the whole rendered word.
- **No overlap on Space or danda.** An inline word and its delimiter are now
  written in one atomic `SendInput` batch; the danda no longer injects a second
  delimiter.
- **Digit selection in inline preedit** re-renders the chosen row and finalises
  without erasing the rendered word.

### Added

- **Quick-add from the quick bar.** The daemon remembers the `(roman, output)`
  pair of the last inline commit; `words.add_last` stores it as a user mapping, so
  a word missing from the dictionary ranks first next time. The pair never leaves
  the daemon and is persisted in the encrypted word store.
- **Five ghost candidates** (was three), so the reachable set matches the top-5
  metric.

## [0.10.0] - unreleased

### Added

- **Inline Bangla preedit (Kontho-style).** In Bangla phonetic mode the hook
  now swallows the letter and the document shows the evolving Bangla on every
  keystroke. The daemon keeps the roman buffer and the last rendered string and
  writes only the suffix after the common prefix, in one atomic `SendInput`
  batch. Backspace pops one roman character and walks the Bangla back. Up/Down
  change the highlighted row live; Tab/Space finalize; Esc leaves the text.
  Grapheme-safe: appending a vowel sign is a pure insert, changing a mark
  retypes only that cluster.
- **`Inline Bangla preedit` setting**, on by default; off restores the 0.9
  commit-on-Space behaviour exactly.
- **Real acceptance learning.** `SuggestionShown` is emitted once per candidate
  per composition, and `SuggestionAccepted` only when the committed word was
  actually shown. This makes Smriti's acceptance signal real instead of
  permanently zero.
- **Next-word ghost hint.** After a Bangla word is committed, the top local
  bigram follower appears as a grey, non-activating hint after the caret.
  Up/Down cycles up to five candidates; Tab inserts the selected word; typing
  dismisses it. It never edits the document by itself. English is wired after
  its learning events and model.
- **Full WTR1 index v2.** The importer now reads the documented dictionary
  format exactly, keeps every target, preserves preferred rank, applies nukta
  then NFC, and ships 135,409 aliases / 151,110 pairs (13,192 multi-target
  aliases). Golden top-1 92.9 % and top-5 100 % are unchanged. See ADR 0023.
- **Global next-word prior.** The ghost hint now merges the user's own Smriti
  pairs with an embedded `OKNW` bigram table: Bangla trained on 30.0M tokens
  (held-out top-5 31.1 %, coverage 96.0 %) and English on 65.6M tokens (top-5
  31.3 %, coverage 98.3 %). The table is a zero-copy lookup with no I/O on the
  query path; a missing or corrupt blob falls back to local pairs. See ADR 0024
  and `docs/learning.md`.

## [0.9.3] - 2026-11-24

### Fixed

- **Explicit user mappings rank first.** A custom pair such as
  `ammar -> আমার` now always outranks curated and dictionary rows. Previously
  the curated base score (1.30) could beat the user base (1.25), so a user's own
  word was not guaranteed to be row 1. Regression test added.

## [0.9.2] - 2026-11-24

### Fixed

- **Bangla Tab no longer leaves the first letter behind.** Tab is now part of
  the swallowed composition set. Before, Notepad inserted a tab before the
  commit erased the buffer, so one character survived.
- **Stuck key repeats.** Swallowed key-downs now use one flag per virtual key
  instead of a single "last key" slot, so holding or overlapping Down/Tab can
  no longer leave a key looking pressed.
- **Double Tab guard.** A swallowed Tab is re-emitted only when the hook
  actually took it, so Notepad no longer receives both a Tab and a re-emit.

### Added

- **English live preview.** Up/Down now replace the typed word in the document
  immediately, not just the highlight. Tab still accepts; Space finalizes.

## [0.9.1] - 2026-11-24

### Fixed

- **English completion navigation.** Up/Down move the highlight and Tab accepts
  the highlighted row. The hook swallows those arrows only while the English
  list is visible, so the caret no longer moves at the same time.
- **English Backspace and Tab safety.** The list keeps its rows after
  Backspace, and Tab refuses to commit a token that was typed in another
  field.
- **Bangla Tab commits.** Tab now accepts the highlighted Bangla candidate
  instead of cancelling the composition.
- **Partial commit recovery.** If Windows delivers only part of a
  `SendInput` batch (for example the backspaces but not the Bangla text), the
  unsent tail is retried once instead of leaving the word erased.
- **Tray icon and duplicate window.** The tray loads `okkhor.ico` from the
  portable folder one level above `bin\`, and tray Settings reuses the running
  Settings window instead of launching another one.

## [0.9.0] - 2026-11-24

### Added

- **Fixed-layout engine.** National/Jatiya, Probhat and Bijoy are now real
  physical-key layouts with Normal/Shift/AltGr/ShiftAltGr mappings and bounded
  conjunct sequences. Fixed mode swallows the raw letter and injects only the
  mapped character, so a Bengali IME or a browser child-window swap cannot
  corrupt the result. See ADR 0021.
- **Alternate phonetic tables.** Khipro and Phonetic/Banglish compile their
  stored rule tables through the existing regex transliterator and produce
  their own output. The selector offers one default "Avro (Phonetic)" choice;
  the other codes remain valid saved settings.
- **Sequence handling.** A key that starts a conjunct is held for at most
  30 ms; a matching second key replaces it, and a timeout flushes the single
  character. Focus changes, pause and mode changes clear the buffer.

### Fixed

- The first letter no longer stays behind in browser address bars: composition
  now tracks the owning process as well as the window handle.
- The English completion card no longer blanks on every keystroke while a
  caret sample is pending. Password samples and field changes still hide it.
- The card is placed just below and slightly left of the caret.
- The tray uses Okkhor's own `okkhor.ico` from the portable folder instead of
  the Windows default application icon.
- The IPC pipe now opens right after settings load, before learning, engine and
  caret setup, so the UI connects without polling for several seconds.

## [0.8.1] - 2026-11-24

### Fixed

- **Suggestions from the first character.** The English session no longer
  hides until three characters, and the engine no longer skips dictionary
  prefix stages below two characters. One-letter prefixes now offer real
  Bangla rows and English completions, with the existing scan/hit caps keeping
  the hot path bounded.
- **Backspace keeps the list alive.** Returning to a shorter prefix restores
  the cached rows in both Bangla and English instead of hiding the popup. The
  old tests that asserted a hidden list after Backspace were replaced with
  tests that assert visible completions.
- **Popup does no redundant work.** A visible card whose rows, selection and
  anchor are unchanged now skips the row clone, GDI text measurement, window
  move and repaint. Repeated row widths come from a font-scoped cache. Real
  content changes still repaint once, coalesced to 30 ms.
- **Blank replacement guard.** A commit whose encoded text is empty or made
  only of whitespace/zero-width characters is rejected before injection, so a
  word is never erased and replaced by a blank. A sequential-commit regression
  test covers four words in one session.

### Changed

- The layout selector now advertises only **Avro (Phonetic)**, the single
  table wired to the transliteration engine. Khipro, Phonetic, National,
  Probhat and Bijoy were advertised in 0.8.0 but all produced identical Avro
  output; Probhat and Bijoy-as-input-layout had no mapping data at all. The
  old codes remain valid stored values so existing settings still load, and
  Bijoy remains available as a separate *output encoding*. See
  `docs/parity/0.8-matrix.md`.

## [0.8.0] - 2026-11-24

### Fixed

- **Suggestion popup stability.** A visible card is updated in place instead of
  being re-shown on every key, only moves when the caret placement actually
  changes, keeps a stable width for the current list, and paints through a
  memory DC. Fast typing is coalesced to at most one visible update per 30 ms;
  the engine and commit state remain per-key. This removes the flicker and the
  one-character jump seen on the 0.7.0 beta.
- **Popup latching.** The caret worker now publishes a completed sample even
  when a newer request is queued, the daemon falls back to the last successful
  anchor, and a late caret sample re-shows the popup without waiting for the
  next keystroke. A transient UI Automation miss no longer hides the list for
  the rest of a word.
- **Stable ranking and selection.** Recency time is frozen for the duration of
  one composition, a bounded prefix cache returns the exact rows previously
  shown when the user backspaces and retypes, and an arrow-key selection now
  follows the same row across later keystrokes instead of snapping back to the
  top.

### Added

- **Raw Latin fallback.** Bangla mode keeps the typed Latin token as the last
  row, and adds a few English prefix completions when no dictionary-backed
  Bangla candidate exists. This keeps unknown phonetic words such as `also`
  selectable instead of forcing a rule-guessed Bangla word. Latin commits are
  not learned as Bangla pairs. See ADR 0020.
- **Legacy import report and parity matrix.** `scripts/import-legacy.ps1`
  inspects the owner-provided Kontho install read-only, records counts and
  SHA256s, and reports unsupported formats. `settings.pkl` and other pickle
  data are never deserialized. `WTR1`/`STRI` dictionaries and pickle-based
  layouts/encodings are recognized but not imported in 0.8.
- **National/Jatiya standard layout.** `crates/okkhor-lang/assets/layouts/national-v1.json`
  holds 77 mappings for the public BCC National keyboard. It is Okkhor's
  independent implementation, exposed as `Bangla (National)` in the layout
  selector; the layout asset is stored data, not a runtime dependency on any
  other keyboard product.
- **Professional layout naming.** The internal `banglish` code is now
  `phonetic`, with `banglish` accepted as a legacy alias and migrated in
  `settings.json`. The UI shows `Phonetic` / `Bangla (National)`.
- **MSVC release build and signing hook.** The portable build uses
  `x86_64-pc-windows-msvc`; `scripts/build-beta.ps1` can sign and timestamp the
  three executables when a trusted certificate is supplied. Unsigned builds
  still trigger SmartScreen; see `docs/release.md`.

## [0.7.0] - 2026-11-24

### Added

- **Opt-in AI rewrite.** Settings has an AI card with an explicit
  **Try a rewrite** action. The provider call runs in an isolated
  `okkhor-worker.exe` process with its own pipe/token; the daemon queues a job,
  returns an id immediately, and the UI polls/cancels. Results are shown for
  review and are never inserted automatically. Prompts and responses are
  memory-only and never logged.
- **OpenAI provider** behind the non-default `http` feature. Default model
  `gpt-6-luna` (roughly $0.0002 per short rewrite); `gpt-6.1-terra` is the
  quality option. The key comes from `OKKHOR_OPENAI_API_KEY` and is never
  persisted.
- **Shipped `okkhor-features` and `okkhor-model` crates.** One source of truth
  for the v1 feature vector, plus a checksum-verified embedded Tier 1 logistic
  reranker behind `--ranker hybrid`. The default remains `--ranker baseline`.
- **Dataset manifest v1 and deterministic trainer.** Fail-closed license,
  checksum, and row-count verification; the same seed and replay produce
  byte-identical model JSON.
- **Automated 0.7 QA gate and stress test.** `scripts/qa-0.7.ps1`,
  `docs/qa/0.7-matrix.md`, `docs/qa/0.7-checklist.md`, and a 10,000-event
  session stress test.

### Changed

- **Caret UI Automation runs on a dedicated latest-wins thread.** Typing no
  longer waits on UIA; a visible popup follows a newer caret without repainting.
- **English popup freshness.** The password flag is trusted only after a sample
  newer than the latest request, so a previous field cannot leak a completion
  into a password box.
- **Worker shutdown is detach-safe.** A hung UIA call no longer blocks daemon
  shutdown.

### Known limits

- The hybrid reranker is trained on the synthetic replay only and is default
  off; it is an exact baseline identity there and makes no real-world quality
  claim.
- AI needs a key in the environment or a local `.env`; it is not bundled.
- Installer, auto-update, and code signing remain 1.0 work.
- Real-key Windows QA (caret, IME/RDP/elevated/DPI) remains the owner's manual
  pass.

## [0.6.0] - 2026-11-24

### Added

- **The full local dictionary is now wired into typing.** The engine loads a
  committed `OKS1` index with 184,775 Bangla words, 114,193 roman→Bangla
  phonetic pairs, 1,161 autocorrect pairs, 722 suffix rules, and 58,928
  English words. Curated exact forms, the direct roman index, the Avro word
  list, and `wordfreq` popularity are merged and ranked, then `Smriti` adds
  personal frequency, recency, and previous-word context.
- **Eight suggestion rows by default**, up to ten. Digits `1`-`9` and `0`
  still select; the popup paints every row.
- **English prefix completions.** After three letters the caret popup offers
  completions. `Tab` accepts the highlighted row, a click accepts any row,
  `Esc` dismisses. Space, Enter, and punctuation never replace text. The
  feature is a Settings toggle and is suppressed in password fields.
- **Instant quick-bar state.** The daemon signals mode, pause, and encoding
  changes with a session-local event; the bar refetches immediately instead of
  waiting for the 15 s poll (now a 5 s fallback).
- **Learning persists.** Accepted and corrected words are saved to an
  encrypted `data/learning.bin` snapshot (DPAPI current-user), loaded at
  startup; a corrupt file starts empty.
- Settings: English suggestions toggle, a clearer row-count label with an
  "8 is the default" hint, and the inert AI/speech card removed.

### Changed

- The suggestion path is the staged `SuggestRequest` engine (curated → user →
  roman → autocorrect → rule → dictionary → suffix → Smriti) with a 64
  candidate bound and a 3 ms p99 budget. Measured on the development machine:
  0.37 ms p50 / 0.80 ms p99 over 2,000 prefix queries.
- The golden corpus reports 92.9% top-1 and 100% top-5 over 113 everyday
  cases (`cargo test -p okkhor-engine --test suggestions`).
- The popup theme is cached for one second instead of reading the registry on
  every show.

### Known limits

- The UI Automation caret resolver still runs on the worker for the first
  popup in a window; a dedicated latest-wins thread is a 0.6.1 polish item.
- Installer, auto-update, and code signing remain 1.0 work.
- The index is built from the owner-provided Avro/Nms installs and `wordfreq`;
  the manifest records source hashes and counts.

## [0.5.1] - 2026-11-24

### Fixed

- **Typing no longer leaves the roman text behind.** Commit keys (Space,
  Enter, digits `0`-`9`, arrows) are now swallowed while a composition is
  active and re-emitted where they belong, so `amar` + Space becomes `আমার `
  instead of `aআমার `. Letters and Backspace still pass through one-for-one
  with the buffer. Any other key ends the composition instead of shifting the
  erase count; a full buffer, an out-of-range digit, or a commit with no
  candidates re-emits the swallowed key so nothing is lost.
- **The suggestion popup follows the caret in browsers.** Chromium (Chrome,
  Edge, Electron) exposes no Win32 caret, so the popup used to pin itself to
  the top-left of the page. The daemon now reads the caret through UI
  Automation `TextPattern2::GetCaretRange` (with `TextPattern` selection as a
  fallback) and only falls back to the mouse when no caret exists.
- Single entry point for the portable beta: `Okkhor.exe` (the UI) now starts
  the resident engine in `bin\` when it is not already running, instead of
  showing a read-only "daemon is not running" screen on first launch. The
  engine stays a separate process (a UI crash must not stop typing) and keeps
  its own command line in `bin\okkhor-engine.exe`.
- The connection banner shows "Starting the Okkhor engine…" while the engine
  starts and offers an explicit **Start Okkhor** retry if startup fails; a
  missing `bin\` folder gets a precise explanation.
- The quick bar reconnects on its own while the engine starts instead of
  staying on "Daemon unavailable".
- Settings explains Bijoy output (ANSI bytes for SutonnyMJ documents; web
  pages show Latin letters and need Unicode) so the encoding choice is not a
  surprise.
- Bangla Unicode conventions: an unshifted `.` types the Bengali danda `।`
  (Shift+`.` still types `>`), and idle digits `0`-`9` type `০`-`৯`. Both stay
  ASCII in English and Bijoy.
- The quick bar now appears on first launch: its visibility was resolved
  before a cold engine start finished, so a default-on bar stayed hidden.
- Smoother typing: the worker poll quantum dropped from 10 ms to 5 ms, and the
  UIA focused element is cached briefly so caret tracking does not re-walk the
  browser's accessibility tree on every keystroke.
- The portable README documents the SmartScreen warning for the unsigned
  beta (`More info` → `Run anyway`) and how to verify the zip hash.

### Changed

- Portable folder layout: user-facing `Okkhor.exe` at the root, internals in
  `bin\` (`okkhor-engine.exe`, `okkhor-ui.exe`), fonts still at `fonts/`.
  The engine and tray resolve the UI at the root and fonts one level up.

## [0.5.0] - 2026-11-23

### Added

- Avro phonetic engine ported from the legacy 250-rule table, with Khipro and
  Banglish layout data.
- Unicode and SutonnyMJ/Bijoy ANSI output using the verified `ANSI 1` table;
  `okkhor daemon --encoding bijoy`.
- Bundled Kalpurush, Kalpurush ANSI, Noto Sans Bengali, and SutonnyMJ (staged
  from the owner's licensed installation); per-user install/status/uninstall.
- Tauri settings UI, on-screen keyboard, word manager, and a non-activating
  quick control bar with drag-to-move, horizontal resize (320-720 px),
  hide-to-tray/restore, and a two-click quit that shuts the daemon down too.
- Caret-anchored suggestion popup with up to ten numbered rows, light/dark
  theme, mouse and keyboard selection, and a curated starter list that adds
  exact corrections plus prefix alternatives to the Avro rule output.
- F9 Bangla/English toggle with Ctrl+Space fallback, tray left-click toggle,
  non-activating toast, and the existing pause hotkey; a short click plays on
  each mode change and can be disabled with `toggle_sound`.
- Daemon IPC v1: status, settings, words, text insertion, and capture control
  over an owner-only named pipe with a rotating token.
- Encrypted word store (DPAPI) and daemon-owned settings store.
- Offline replay/feature-schema foundation (`okkhor-train`, ADR 0015).

### Changed

- The typing path uses the real Avro table (replacing the 0.2 fixture rules).
- Per-user fonts are installed without admin; Bijoy prefers SutonnyMJ and
  falls back to Kalpurush ANSI.

### Known limitations

- Unsigned beta, no installer/updater.
- Alternatives come from a small curated starter list; the imported Avro
  dictionary and the learned reranker are later milestones.
- No speech/AI actions yet; no clipboard fallback or elevated-window injection.
- Manual real-key QA remains owner-gated; see `docs/qa/`.

## [0.9.11] - 2026-11-27

### Added

- **Exit in the quick bar.** A power button (two clicks) asks the daemon to
  shut down, waits for the pipe to close, force-stops an unresponsive engine,
  then exits the UI. The tray icon can no longer outlive an explicit exit.

### Fixed

- **Shortcuts stopped showing suggestions.** The worker re-read the modifier
  state when it drained the queue, so a fast Ctrl+Shift+S could be processed
  after Ctrl was released and start a composition. The hook now records
  `chord` on the key event; a chorded key cancels the composition and is never
  treated as a letter, digit, or commit.
- **The next-word ghost no longer lingers on mouse clicks.** It only shows in
  the window that received the commit and disappears after four seconds, so
  clicking the tray or the browser address bar cannot leave it behind.
- **Settings opens when Windows refuses focus.** The click used to fail
  silently because `set_focus` returned an error; showing, unminimizing and
  navigating now happen regardless.
- **Add-last-word works with learning off.** The last (roman, Bangla) pair is
  remembered for the explicit `+` action even when learning is disabled; it is
  still memory-only and never written or logged.

## [0.9.10] - 2026-11-26

### Fixed

- **Backspace deletes one step again.** While inline preedit was active the hook
  did not swallow Backspace, so the target application deleted a character
  natively *and* the daemon re-rendered the shorter word: one keypress removed
  two or three characters, and after two presses the whole word was gone while
  the suggestion list still showed a longer word. Backspace is now swallowed
  whenever the composition owns the on-screen word (inline preedit on, rendered
  text present, no chord) and passes through untouched when the document holds
  raw roman text or no composition is active.

## [0.9.9] - 2026-11-26

### Fixed

- **The quick bar can be resized again.** 0.9.7 fixed the bar at 340x52 and
  dropped the right-edge handle. The handle is back: a native window resize drag
  (no poll loop) between 300 and 560 px, height held at 52, and the width is
  remembered locally. It is a window behavior, not a control button, so the bar
  still carries only the five typing-surface targets.

## [0.9.8] - 2026-11-26

### Added

- 0.5 daemon IPC server (Task 3): `okkhor daemon` serves contract §4 on
  `\\.\pipe\okkhor-ipc-v1` (dedicated thread + runtime, max 4 clients, 10 s
  read timeout, 5 s deadline, owner-only DACL) unless `--no-ipc`. The token
  file is created/rotated at startup and removed on clean shutdown; setup
  failures degrade to typing-without-IPC. Handlers: `status` (incl. live
  `input_mode`/`output_encoding`), `settings.get/update` with live-apply
  (mode, encoding, pause/capture, popup enable, learning, autostart;
  hotkeys re-register where feasible else `Deferred`, fonts validated not
  installed), `layouts.list` (Avro/Khipro/Banglish/Bijoy), `words.*`
  (caps/dupes enforced), `text.insert` (512 UTF-16, 20/s, recorder-tested,
  injection-locked against commits), `capture.set`, `ai.complete` →
  `Unsupported`. Tray Settings launches `okkhor-ui.exe` (detached,
  `OKKHOR_UI_PATH` override, balloon + log code when missing); `daemon --ui`
  launches it once at startup. Unit + `ipc_roundtrip` pipe tests; no hook
  install, no SendInput, no text in logs/errors.
- Store review advisories: `Debug`/log output redacted (no user paths from
  either store), and atomic-replace failures surface loudly as a typed error
  plus one allowlisted log code instead of a silent remove-and-retry.
- 0.5 encrypted store (Task 2): `okkhor-win::dpapi` (current-user
  `CryptProtectData`/`CryptUnprotectData`, `CRYPTPROTECT_UI_FORBIDDEN`, typed
  errors, 64 MiB cap), `okkhor-storage::EncryptedJsonRepository` over a
  `Protector` trait (`OKE1` envelope, atomic replace, 8 MiB cap,
  `NotFound`/`Corrupt`/`Unsupported`, deterministic fake protector), and
  daemon `paths` (`settings.json`, `data/words.bin`, `runtime/ipc.token` with
  `OKKHOR_DATA_HOME` test override), `settings_store` (defaults, clamping,
  `update(patch)`), and `words_store` (ids, 20,000 cap, duplicates, search,
  CSV/JSON import/export with 5,000-row import cap) modules. The daemon loop
  and IPC handlers are Task 3 and are not wired yet.

### Fixed

- Associate the global low-level keyboard hook with the process module handle
  instead of passing a null module handle. Hook QA with real keys passes.

### Added

- New `okkhor-lang` crate (0.3 sprint, Task 1): layout/encoding/dictionary
  data types (`Layout`, `EncodingTable`, `Dictionary`) plus a synthetic-fixture
  importer (`import_fixture_dir`, `import_asset`) with `FixtureEnvelope`
  schema enforcement, deterministic `BTreeMap` ordering, and daemon-safe
  `LangError::kind()`/`code()` reporting. No transliteration, FFI, or Win32.
- New `okkhor-transliterate` crate (0.3 sprint): rule-based transliteration
  engine (`RuleTransliterator`, `RuleSet` parsed from synthetic JSON and
  validated/sorted at construction, literal prefix matching) with
  normalization helpers and daemon-safe `TransliterateError::kind()`/`code()`
  reporting. Full phonetic/conjunct behavior is a later milestone.
- New `okkhor-compat` crate (0.3 sprint): safe legacy-DLL boundary with a
  private `cfg(windows)` FFI module, DLL path validation against the
  daemon allowlist (`DllPathPolicy`, `ValidatedDllPath`), per-adapter Cargo
  feature flags, a fixed 5-slot `AdapterRegistry`, and
  `CompatError::kind()`/`code()` reporting with static `Display` strings.
  Tests use a stubbed loader seam and synthetic nonce files; no real DLL is
  ever opened.
- New `okkhor-engine` crate (0.4 sprint): candidate generation pipeline
  (`CandidateEngine::suggest`: normalize once, dictionary prefix lookup,
  transliteration application, merge/dedup, Smriti rank, top-N truncate) with
  per-language budgets, a synthetic golden corpus (`GoldenCorpus`,
  `check_parity` top-1/top-5), and `EngineError::kind()`/`code()` reporting.
  No `unsafe`, no I/O on the query path.
- `okkhor-bench --candidates` mode (0.4 sprint): runs the golden corpus
  through `CandidateEngine::suggest`, reports parity (`check_parity` with
  `limit = min(5, budget max_limit)`) plus rank latency p50/p95/p99/max, and
  writes JSON/CSV under `bench-out/`. First shared-desktop measurement is
  recorded in `docs/performance.md`.
- 0.3 core contracts (`docs/design/0.3-core-contracts.md`) with ADR 0010, and
  0.4 engine contracts (`docs/design/0.4-engine-contracts.md`) with ADR 0011.
- Lead merge gates G1–G4 closed: daemon logger scopes `lang`,
  `transliterate`, `compat` (G1); ADR 0010 index row (G2); layout stays
  metadata-only in the Smriti pair key (G3, `docs/learning.md`); compat
  naming clarified as `okkhor-compat` in ADR 0005 (G4).
- QA-only `okkhor inject <text>` command with a focus delay and forced
  `SendInput` option for manual live-app QA.

## [0.2.0] - 2026-11-23

### Added

- Win32 low-level keyboard hook (`WindowsKeyboardHook`) with a dedicated hook
  thread, message loop, bounded event queue, and pass-through behavior.
- Unicode `SendInput` text injector (`WindowsInjector`) with UTF-16 surrogate
  handling and 256-code-unit batching.
- `okkhor hook-demo` manual smoke test for the hook.
- Per-user single-instance guard (`SingleInstance`) with a named mutex.
- Capture pause state (`set_capture_enabled`, `toggle_capture`) that preserves
  pass-through and stops enqueueing without uninstalling the hook.
- Global `Ctrl+Alt+Shift+P` pause hotkey registered on a dedicated message-loop
  thread, with `hook-demo` wiring.
- Clipboard fallback with focus checks, text-only clipboard preservation,
  Ctrl+V injection, and previous-text restore.
- UIPI/`ERROR_ACCESS_DENIED` handling and explicit blocked/focus/clipboard
  errors.
- Caret anchoring via `GetGUIThreadInfo`/`ClientToScreen` with a mouse/screen
  fallback, DPI-scaled placement, and work-area clamping.
- A non-activating `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` popup scaffold and
  `okkhor popup-demo`.
- Hook cleanup on drop and a single-instance guard around `hook-demo`.
- Per-user autostart through `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
  with a quoted executable path and the `daemon` argument, plus
  `okkhor autostart enable|disable|status`.
- A minimal `Shell_NotifyIconW` tray (`TrayIcon`) on a hidden message-only
  window with Pause/Resume, a disabled Settings placeholder, Exit, and
  `okkhor tray-demo`.
- The resident `okkhor daemon` mode wiring the hook, pause hotkey, tray, and
  single-instance guard together.
- Redacted JSON-lines daemon logging under `%LOCALAPPDATA%\Okkhor\logs` and
  stderr with an allowlisted field set for startup, command, and error events.
- Strict `WindowsInjector::inject_unicode_only` entry point for diagnostics and
  benchmarks that must never use the clipboard fallback.
- Native-control `WM_CHAR` fast path (`Edit`, `RichEdit*`, `Scintilla`) with
  `SendInput` as the universal fallback and measured 1.1–1.3 ms inject p99.
- `okkhor-bench --sendinput` to measure the universal fallback separately.
- `apps/okkhor-bench` with deterministic synthetic English/Bangla ranking
  measurements, nearest-rank p50/p95/p99/max reporting, and git-ignored
  JSON/CSV output under `bench-out/`.
- Injection benchmark against a private non-elevated Win32 `EDIT` window through
  `WindowsInjector`, including Unicode/Bangla readback correctness and a
  deferred UTF-16 event-construction fallback when the session cannot focus a
  test window.
- CI-safe `--smoke` mode and unit-tested percentile/report helpers.
- ADR 0008 documenting the non-hook benchmark boundary.

### Not yet

- the Settings tray action, `TaskbarCreated` re-add after Explorer restart, and
  the settings UI;
- full suggestion-list rendering and interaction;
- hook-level keydown -> suggestion and keydown -> visible-text benchmarks;
- Windows integration matrix and signing.

## [0.1.0] — scaffold

### Added

- Rust workspace with `okkhor-core`, `okkhor-learn`, `okkhor-storage`,
  `okkhor-win`, and `okkhor-daemon`.
- Language, text normalization, candidate, and ranking primitives.
- Smriti learning engine with frequency, recency, acceptance, correction,
  transliteration-pair, and bigram signals.
- JSON snapshot persistence with schema validation.
- Windows input/injection boundary types.
- Daemon `self-check` covering English, Bangla, transliteration, and
  persistence.
- CI, formatting, clippy, tests, and developer check script.
- Architecture, roadmap, learning, performance, security, testing,
  compatibility, release, and glossary documentation.
- AGENTS.md engineering contract and ADRs.
- ADR 0007 and the long-term hybrid global/local model roadmap.

### Not included

- keyboard hook, text injection, tray, UI, speech, AI workers, installer.
