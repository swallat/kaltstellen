# kaltstellen implementation plan (draft)

Basis: [spec.md](spec.md) (v1.1). Milestones M1 to M3 are planned in detail, M4 roughly. Items marked **[assumed]** are the planner's defaults from when the plan was written; they are historical, and the current status of every decision is in [spec.md](spec.md) and [open-decisions.md](open-decisions.md).

Conventions: each work package (WP) lists the requirement IDs it covers, its dependencies, the acceptance tests (T-numbers from spec section 10), and concrete steps on the test VM. All `unsafe` code stays in `src/process.rs` and, since WP7, the Win32 wrapper tree `src/win32/` (Q4); other modules call their safe functions. English in code and docs, no machine-specific names in the repository.

## Test VM and what it cannot cover

Environment: a dedicated Windows 11 test VM, reached over SSH, standard user without administrator rights for normal runs, plus one elevated session for the elevation tests. **[assumed]** Test programs are small helper executables built in the repository (`tests/fixtures`): a windowed app, a console app, a program that spawns children, a program that spawns an orphaned child, a program with an unsaved-changes dialog on close, and a heartbeat program that writes a timestamp to a file every 100 ms (to prove it is frozen or running).

Cannot be covered on the VM (run on the desktop, manually): T13 (long freeze of real programs), audio devices and driver panels, exclusive-mode audio, GPU load, vendor tray tools, real network and licence checks, real anti-cheat. T12 and T19 give indicative numbers only; the final figures come from the desktop. Screens and shutdown dialogs (T20) need the VM console or an RDP session, not only SSH.

## Dependency overview

```
WP1 foundation ─┬─ WP2 process engine ─┬─ WP4 switch engine ─ WP5 CLI switch ─┐
                │                      │                                      ├─ M1 done
                └─ WP3 config ─────────┴──────────────────────────────────────┘
WP4 ─ WP6 state file & recovery ─ WP7 tray app ─ WP8 session events ─ M2 done
WP7 ─ WP9 hotkeys ─┐
WP7 ─ WP10 GUI process ─ WP11 unassigned programs ─ M3 done
```

## M1: profiles on the command line

### WP1 Foundation
- Covers: Q2, Q4, repository layout.
- Depends on: nothing.
- Steps: module layout (`process`, `config`, `engine`, `state`, `cli`); error type with stable codes (E1); logger writing to `%LOCALAPPDATA%\kaltstellen\kaltstellen.log`, rotating at 1 MB, three files; test fixtures crate; CI keeps format, clippy and tests green.
- Acceptance: CI green on the VM and on GitHub Actions; `unsafe` only in `process.rs` (check with a grep in CI).

### WP2 Process engine (`src/process.rs`)
- Covers: A3, A4, A6, A7 (matching, tree resolution, self-protection, least access), S2/S8, P7, P6 hard tier, W7 (minimise), trim.
- Depends on: WP1.
- Steps: snapshot of processes with parent PID, creation time and full image path; tree resolution with creation-time guard; path normalisation; session filter; hard-protected list; suspend/resume with `NtSuspendProcess`/`NtResumeProcess` holding the handle; trim with per-process error; graceful close by `WM_CLOSE`, then terminate or keep (`on_timeout`); minimise with wait until iconic or 1 s; error mapping to "needs administrator" and "protected".
- Acceptance: T9 (hard-protected refused), T15 (only matched processes opened), T16 (self and ancestor protection), T18 (elevated target reported), T11 (`keep`), T19 (trim measured as available memory).
- VM steps:
  1. Run the heartbeat fixture, suspend it, confirm the heartbeat file stops, resume, confirm it continues.
  2. Spawn a parent with a child and an orphan; suspend the parent; confirm the child is frozen and the orphan is not.
  3. Start the dialog fixture, run `stop` with `on_timeout = keep` and `kill`; confirm the two outcomes.
  4. Run a fixture elevated in an elevated session and target it from a normal session; confirm "needs administrator".
  5. Record available memory before and after trim of a fixture that allocates 500 MB.
  6. Run a trace (ETW or Process Monitor) of `switch` to confirm no handles are opened to unrelated processes.

### WP3 Configuration
- Covers: P1 to P4, C1 to C7, P5 fields, `config.example.toml`.
- Depends on: WP1.
- Steps: TOML schema with `deny_unknown_fields`; validation rules C1 to C7 with file and line in errors; hot reload with the rule that a running switch uses the old configuration (C4); settings defaults; `settings.json` for the last profile name.
- Acceptance: T8 (invalid config keeps the old one, names file and line), T17 (partly: hotkey syntax validation), unit tests for every rule.
- VM steps: feed broken, duplicated and valid files to `kaltstellen check-config` (new subcommand **[assumed]**), compare messages; edit the file while a switch runs and confirm the switch finishes with the old content.

### WP4 Switch engine
- Covers: W1 to W6, P5 behaviour, A1, A2, A5 not included, P6 enforcement in the engine.
- Depends on: WP2, WP3.
- Steps: planner that turns (state, target profile) into an ordered action list (resume except still-suspended, stop, suspend, start); executor with collected errors (W2); second enumeration after suspend (W6); queue of one request (W3); idempotence (W5).
- Acceptance: T1 (order), T2 (shared freeze kept), T3 (failure does not stop the switch), T4 (only the last request runs), T10 (`restart_on_neutral` only on neutral). Planner tests run without Windows (pure function), executor tests on the VM.
- VM steps: script two profiles with the fixtures; check the order in the log; make one step fail (protected exe, missing path) and confirm exit code 1 and that the rest ran; send two switches during a slow stop (delay fixture) and confirm only the second runs afterwards.

### WP5 CLI
- Covers: U2, U5 (command line part), E2, L1 (in-process path with state lock), L3 (`recover`), `status`.
- Depends on: WP4, WP6 (state file; implement the state writer first).
- Steps: `switch`, `status`, `list`, `suspend`, `resume`, `recover`, `check-config`; exit codes 0 to 5; `--no-wait`, `--force`.
- Acceptance: T3 (exit code), T9 (CLI refusal), T4 (exit code 3).

### WP6 State file (built together with WP4/WP5)
- Covers: S1, S3 (recovery logic), S6, S9, S10, atomic writes.
- Depends on: WP1.
- Acceptance: T5 (kill the tool while programs are frozen, run `recover`, nothing stays frozen), T6 (stale PID ignored, corrupt file renamed), plus boot-time mismatch test.
- VM steps: freeze fixtures, `taskkill` the tool, confirm frozen, run `recover`; edit state file PID to a live unrelated process and confirm it is not touched; corrupt the file; reboot the VM with a state file present and confirm nothing is resumed or touched afterwards (S10).

**M1 done when:** T1 to T6, T8 to T11, T15, T16, T18, T19 pass on the VM, and the example config switches a three-fixture scenario both ways.

## M2: tray app

### WP7 Tray app
- Covers: U1, L1 (mutex, named pipe with current-user ACL), L2, Q1, three icon states, error state clearing.
- Depends on: M1.
- Steps: hidden message window, notification icon with the three bundled icons, menu from the configuration, pipe server forwarding requests to the engine, single-instance mutex, start at sign-in (Run key, **[assumed]** opt-in). Idle trim of the tray itself was dropped: the measured private working set is about 1.4 MB, far below Q1, and trimming would only move shared pages.
- Acceptance: T12 (idle footprint, indicative on the VM), T3 (error state), T4 over the pipe.
- VM steps: start two instances, confirm the second exits; call the CLI with the tray running (pipe path) and not running (in-process path); measure working set and CPU for 60 s; connect to the pipe as another user and confirm refusal.

### WP8 Session and power events
- Covers: S5, S3 (runtime part), L2, standby behaviour, S4 via W7.
- Depends on: WP7.
- Steps: resume all on `WM_QUERYENDSESSION`; shutdown priority; stopped records cleared at sign-out, kept on Quit/crash; `settings.json` last profile offered after sign-in; console handler for CLI (done with M1, spec L6); `WM_POWERBROADCAST` logging only.
- Acceptance: T7, T20, T5 (tray crash variant).
- VM steps: freeze fixtures, sign out through RDP and sign in again; repeat with shutdown and with a cancelled shutdown; confirm no "app is preventing shutdown" dialog and that the tray offers the last profile; put the VM into standby if the hypervisor allows it (otherwise untested **[assumed]**).

**M2 done when:** T5, T7, T12, T20 pass, and a manual 30 minute run of the example profiles shows no stuck process.

## M3: GUI, hotkeys, unassigned programs

### WP9 Hotkeys
- Covers: U3, C5, C7.
- Depends on: WP7.
- Steps: `RegisterHotKey` with `MOD_NOREPEAT` on the tray window; rejection of `Ctrl+Alt+<printable>`; one-time report of failures; re-registration on config change.
- Acceptance: T17.
- VM steps: bind a hotkey taken by another fixture and confirm the report and that tray and CLI still work; bind `Ctrl+Alt+7` and confirm rejection; use a German keyboard layout in the VM to confirm AltGr characters still type.

### WP10 GUI process
- Covers: M1 to M4 (profile management, program assignment from the running list or by file), main window layout, `kaltstellen gui`, toolkit decision, error display.
- Depends on: WP7.
- Steps: toolkit spike first (native Win32 versus a small immediate-mode library) with a size and startup budget **[assumed]**: GUI process under 30 MB working set and window visible in under 1 s; config read/write preserving comments **[assumed: use a format-preserving TOML editor]**; apply profile over the pipe.
- Acceptance: manual walkthrough (create, copy, rename, delete profile; add program from running list and by file; change action), T8 for GUI writes, Q1 re-check with the GUI closed.
- VM steps: scripted UI walkthrough over RDP; confirm the config file stays valid and comments survive a GUI edit.

### WP11 Unassigned programs (N1)
- Covers: N1, `status --unassigned`, `seen.json`.
- Depends on: WP10, WP3.
- Steps: snapshot of programs with a notification icon (`HKCU\Control Panel\NotifyIconSettings`) plus visible-window fallback; diff against profile entries and `seen.json`; `new` flag by `first_seen`; dismiss; tray menu entry with count.
- Acceptance: T22.
- VM steps: fixtures with and without tray icon and window; start one after applying a profile; check the GUI and `status --unassigned`; add an entry and confirm the marker disappears; confirm hard-protected programs are never listed.
- Note: the registry source exists on Windows 11 only. Behaviour on Windows 10 is the window fallback only **[assumed]**; the VM is Windows 11.

**M3 done when:** T17 and T22 pass, the GUI walkthrough is clean, and a full run through all T1 to T22 is green on the VM, with T13 done on the desktop.

## M4 (rough)

- Services (A5): stop and start by name, via an elevated helper started on demand, never an elevated tray (decided). Needs a design note for the helper's pipe and its authentication before work starts.
- Automatic triggers (U4): trigger processes, conflict rules (two triggers at once, manual versus automatic), and the question of whether auto-activation is wanted at all. Needs a fresh decision round.
- Candidates after that: release builds, drag and drop, per-entry failure markers, user-extendable warn list (all post-v1, decided).

## Cross-cutting

- Every work package ends with CI green, updated `docs/spec.md` section and a short note in `docs/plan.md`.
- The test table T1 to T22 becomes a checklist in the repository (`docs/acceptance.md`) with columns VM, desktop and last result **[assumed]**.
- Risks to watch while building: R2 (frozen programs that others wait on) shows up first in T14, which still needs a VM script (explorer responsiveness with windowed fixtures frozen). It belongs to WP2 and WP7.
- T21 (Defender) runs once per milestone with Defender enabled on the VM.

## Test coverage matrix

| Test | WP | Where |
| --- | --- | --- |
| T1 to T4 | WP4 | VM |
| T5, T6 | WP6 | VM |
| T7, T20 | WP8 | VM (RDP/console) |
| T8 | WP3, WP10 | VM |
| T9, T15, T16, T18 | WP2 | VM (T18 needs elevated session) |
| T10 | WP4 | VM |
| T11 | WP2 | VM |
| T12 | WP7 | VM indicative, desktop final |
| T13 | none | desktop only |
| T14 | WP2, WP7 | VM |
| T17 | WP9 | VM |
| T19 | WP2 | VM indicative, desktop final |
| T21 | all | VM with Defender enabled |
| T22 | WP11 | VM |
