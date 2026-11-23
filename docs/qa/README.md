# Manual QA

This directory contains the manual Windows QA process for Okkhor. Automated
testing stays in [testing.md](../testing.md); manual QA covers behavior that
unit tests cannot exercise: the global keyboard hook, focus, DPI, IME,
elevation, and session changes.

The 0.2 matrix is [0.2-matrix.md](0.2-matrix.md). It is the written pass/fail
record for the 0.2 input layer, not a demo script and not a substitute for
automated tests. The 0.7 hardening matrix is [0.7-matrix.md](0.7-matrix.md);
reuse this directory's safety, evidence, and privacy rules for it.

## Scope

0.2 covers:

- the `WH_KEYBOARD_LL` hook and pass-through;
- capture pause and the global pause hotkey;
- the per-user single-instance guard;
- the Unicode `SendInput` injector, guarded clipboard fallback, and native
  `WM_CHAR` fast path;
- the caret-aware, non-activating popup scaffold.

It does not cover, and a pass must not claim:

- tray, settings UI, suggestion rendering, or the on-screen keyboard;
- layout mapping, transliteration, or candidate generation (0.3);
- learning from live typing (0.4);
- AI, speech, installer, updater, or signing (later milestones).

If a row depends on something that is not implemented, mark it `BLOCKED` or
`N/A` with a reason. Code inspection is not a test result.

Injection rows must record which path ran:

- `native_fast_path` for `Edit`, `RichEdit*`, and `Scintilla`;
- `unicode_send_input` for the universal fallback;
- `clipboard_paste` only after a blocked `SendInput`.

The 2 ms injector-call target applies to the native fast path. The universal
`SendInput` path is currently measured at 5.8–7.5 ms p99 on the shared desktop
and must be recorded, not averaged into the fast-path result.

## Safety and isolation

Manual QA uses a global input hook and synthetic text. Treat the test machine as
untrusted.

- Run on a disposable VM or a dedicated test account, never a daily driver.
- `hook-demo` observes every key in the session. Do not run it while typing
  personal text, passwords, or credentials.
- Do not run the legacy application, its installer, or its DLLs. See
  [compatibility.md](../compatibility.md).
- For live-app injection rows, use the QA-only command:

  ```powershell
  okkhor inject "synthetic text" --delay-ms 3000
  # force the universal fallback:
  okkhor inject "synthetic text" --delay-ms 3000 --sendinput
  ```

  Focus the target window during the delay. Never use real user text with this
  command; the process command line is visible to other users.
- Close other keyboard utilities and remappers unless a row tests hook
  coexistence.
- Know how to stop: `hook-demo` exits after five seconds or on `Esc`; a hung
  process can be ended in Task Manager.
- UAC rows use the consent prompt; never type real credentials into a
  credential prompt.
- Use only the fixtures in [0.2-matrix.md](0.2-matrix.md), or an equally
  synthetic local fixture.

## Before a run

1. Read [testing.md](../testing.md), [security-privacy.md](../security-privacy.md),
   and the 0.2 matrix.
2. Build or obtain the MSVC release build for the exact commit under test.
   Record the commit, version, toolchain, and binary SHA-256. A local GNU build
   is exploratory only and must not be recorded as release evidence.
3. Run the self-check:

   ```powershell
   .\target\release\okkhor.exe self-check
   ```

   It must print `self-check: ok`.
4. Prepare the machine:
   - local test account with no personal files or cloud sync;
   - no notifications, OneDrive, or personal browser profile;
   - clipboard cleared; clipboard history and cloud sync off unless a row tests
     them;
   - target applications installed and versions recorded;
   - required layouts and IMEs installed and versions recorded;
   - display arrangement, resolution, and scaling recorded;
   - balanced power plan; no Windows Update or virus scan running.
5. Confirm no other low-level keyboard hook, macro tool, or remote-control agent
   is active unless the row requires it.
6. Fill in the run header before the first row.

### Run header

```text
Run ID:
Date:
Tester:
Commit / version:
Toolchain / build profile:
Binary SHA-256:
OS edition / build / UBR:
Device / CPU / RAM:
Session: local / RDP / fast user switching
Monitors: resolution, arrangement, scaling
Target apps and versions:
Layouts and IMEs:
Clipboard history / cloud sync: off / on
Other input software:
Evidence root:
```

## Available commands

| Command | What it exercises | Limits |
| --- | --- | --- |
| `okkhor.exe self-check` | learning, ranking, and persistence smoke test | no hook, UI, or injection |
| `okkhor.exe hook-demo` | hook install, pass-through, pause hotkey, demo single instance | five-second window; prints virtual-key and scan codes; no injection |
| `okkhor.exe popup-demo` | caret anchoring and non-activating popup | three-second window; fixed sample text; anchors to the foreground window at launch |
| Local QA harness | end-to-end injection, forced clipboard fallback, long-running hook | not in this repository; see the gap below |

Build the QA binary with the production toolchain:

```powershell
cargo build --workspace --release
.\target\release\okkhor.exe --version
.\target\release\okkhor.exe self-check
```

`hook-demo` prints one `Down` or `Up` line per key event and stops on `Esc` or
after five seconds. `popup-demo` shows a fixed non-activating popup near the
current caret for three seconds. A normal console launch makes the console the
foreground window; verify the foreground HWND before trusting popup placement
results.

## Run order

1. Capture the run header.
2. Run `self-check`.
3. Run the OS baseline on Windows 11 24H2, then on 25H2.
4. Run hook, pause hotkey, and single-instance rows.
5. Run Unicode injection and clipboard fallback rows with the harness.
6. Run caret popup and DPI rows.
7. Run target-application rows.
8. Run IME and layout rows.
9. Run privilege and UAC rows.
10. Run RDP, fast user switching, and touch keyboard rows.
11. Run privacy and observability rows.
12. Clean up the machine, archive sanitized evidence, and sign the release gate.

Stop immediately on a P0 result: data corruption, a privacy or security breach,
system-wide input failure, or injection into the wrong window. Report P0 and P1
results before continuing to the next section.

## Recording results

| Result | Meaning | Required action |
| --- | --- | --- |
| `PASS` | The expected result was observed and evidence is attached | None |
| `FAIL` | The expected result was not observed | File a defect with the template in the matrix |
| `BLOCKED` | The environment or harness cannot run the row | Record the reason, owner, and follow-up; do not count it as a pass |
| `N/A` | The capability is absent from this build or the OS configuration is invalid | Record the reason; do not use it for an untested row |
| `PENDING` | Not run yet | Complete before sign-off |

- Every result needs an evidence ID in the form `E-<row>-<sequence>`, for
  example `E-IN-02-01`.
- A flaky row is run three times; record the pass rate, for example `2/3`, and
  file a defect if it is not deterministic.
- A known limitation is not a pass unless the row's expected result explicitly
  names that limitation. Record it in the limitation register and gate it
  explicitly.
- Do not mark a row `PASS` from a unit test, a code review, or a screenshot of a
  different environment.

## Evidence rules

Evidence must let a reviewer confirm the result without exposing user data.

- Screenshots must show the row ID, the synthetic fixture, the target
  application, and enough environment context to identify the OS build and
  display configuration. Crop out unrelated desktop content.
- Video is for focus, timing, and popup transitions. Keep clips to 5–15
  seconds, with no audio and no personal content.
- Console logs may contain commands, event counts, result codes, and error
  codes. `hook-demo` key codes are still sensitive; treat a console capture as
  restricted evidence.
- Never include raw user text, clipboard contents, window titles, passwords,
  tokens, email addresses, or full user paths in evidence or logs.
- Store binary evidence outside the repository in the restricted QA share.
  Record its filename, date, and SHA-256 in the matrix or defect. Do not commit
  screenshots, videos, traces, or logs to git.
- ETW/WPR traces can contain window titles, paths, and process arguments.
  Privacy-review them before sharing; prefer a redacted summary.
- Retain sanitized evidence until the milestone closes, then delete raw
  captures. Keep only what a defect needs.
- Attach evidence to the milestone or issue only after the privacy review.

## Privacy constraints

- Use synthetic fixtures only. Never substitute a real message, name, email,
  URL with a token, password, or clipboard entry.
- Use a local test account. Do not sign in to personal Microsoft, browser,
  Office, Slack, Teams, or cloud-sync accounts.
- Clear the clipboard before and after each test. Use the clipboard sentinel
  fixture, not personal content.
- Disable Windows clipboard history and cloud clipboard unless a row explicitly
  tests them. Clear both after the test.
- RDP clipboard redirection can expose the local clipboard. Disable it unless
  the row tests it, and use only synthetic content when it is on.
- Never enter real credentials. Password-field rows use a synthetic local page
  and the pause hotkey.
- `hook-demo` records virtual-key and scan codes for the whole session. Run it
  only on the isolated test account and clear the console afterward.
- Do not export learning data during 0.2 QA; the input layer does not produce
  it.
- Clean up: sign out, delete test documents, clear clipboard and history, revert
  the VM snapshot, and remove any unsanitized evidence.

## 0.2 harness gap

The 0.2 daemon has no end-to-end injection command. `hook-demo` only prints
events, and `popup-demo` only shows a fixed popup. Rows tagged `[harness]`
require a local QA harness that can:

- inject a fixture into the foreground window and report the method or error;
- force the clipboard fallback path without bypassing its checks;
- keep the hook alive across RDP, fast user switching, and stress runs;
- delay injection so a focus change can be tested.

Until that harness exists, mark those rows `BLOCKED`. Unit tests are not
end-to-end evidence. The gap is recorded as `LIM-01` and `LIM-02` in the matrix.

## Exit criteria

A 0.2 QA run is complete only when:

- the matrix has been executed on Windows 11 24H2 and 25H2;
- no P0 or P1 failure is open, or the release is explicitly blocked;
- every failure has a reproducible defect and an accepted known limitation or
  a fix;
- every blocked row has an owner and a follow-up;
- evidence is sanitized, attached, and privacy-reviewed;
- the release gate in [0.2-matrix.md](0.2-matrix.md) is signed.
