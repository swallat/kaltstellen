# Plan

> The detailed work breakdown, acceptance tests and VM test steps now live in [implementation-plan.md](implementation-plan.md); the behaviour is specified in [spec.md](spec.md). The milestones below remain the summary.

Milestones are ordered so that each one is usable on its own. Requirement IDs
refer to [requirements.md](requirements.md).

## M0: suspend prototype (done)

Command-line tool with `list`, `suspend [--trim]` and `resume` by executable
name. Purpose: find out which real apps tolerate being frozen.

## M1: profiles on the command line

- Config file with profiles and app entries (P1 to P4).
- `kaltstellen switch <profile>` applies a profile.
- Actions `start`, `stop` with graceful close and timeout, `suspend` with
  process trees (A1 to A4, A6).
- State file so a switch undoes the previous one (S1, S2).

## M2: tray app

- Tray icon with profile menu (U1); the CLI stays (U2).
- Resume on exit and recovery after a crash (S3).
- Minimise windows before suspending (S4).

## M3: automation

- Trigger processes switch profiles automatically (U4).
- Global hotkeys (U3).

## M4: services and elevation

- Stop and start services (A5).
- Elevation model as decided in open question 4 (Q3).

## Later, if wanted

- Graphical profile editor.
- Release builds attached to GitHub releases.

## Progress notes

- WP1 foundation (2026-11-22): module layout, error codes (`docs/errors.md`),
  rotating file logger, test fixtures (`tests/fixtures`), CI with fmt,
  unsafe check, clippy and tests. Acceptance checklist in `docs/acceptance.md`.
- WP3 configuration (2026-11-22): TOML schema and rules C1 to C7 in
  `src/config.rs`, hotkey syntax in `src/hotkey.rs`, `kaltstellen check-config`
  (exit code 2 when invalid), `config.example.toml`. Details decided on the
  way are in spec C8. Still open from WP3: hot reload (C4) and `settings.json`,
  both needed first by the tray app; T8 on the VM.
- WP2 process engine (2026-11-22): `src/process.rs` with handle-free
  snapshot, tree resolution with creation-time guard, self-protection,
  suspend/resume/trim, graceful stop with `on_timeout`, minimise, start;
  protection tiers in `src/protection.rs`; tests against the fixtures in
  `tests/fixtures/tests/engine.rs`. Details decided on the way are in spec
  A8. Still open from WP2: T15 (handle trace), T18 (elevated target), T19
  (trim measured) and T14, all on the VM; the warn list is not enforced yet
  (`--force`, WP5).
- WP4 switch engine, WP5 command line, WP6 state file (2026-11-22):
  `src/engine.rs` (W1 order, shared freeze kept, collected errors, second
  pass of W6, warn list with `--force`, recovery), `src/state.rs` (atomic
  `state.json`, lock), commands `switch`, `status`, `recover` with the exit
  codes of E2; tests in `tests/fixtures/tests/switch.rs`. Still open for M1:
  all VM runs of the acceptance list, T15, T18, T19; W3/T4 (request queue),
  `--no-wait` and the Ctrl+C handler come with the tray app (M2).
- WP7 tray app, first increment (2026-11-22): `kaltstellen tray` with
  single-instance mutex, notification icon with the three states and a
  tooltip naming the active profile, menu from the configuration, switches
  on a worker thread with the W3 queue, recovery at start (S3), Quit with
  resume (L2), lazy configuration reload keeping the last valid file
  (C3/C4). Pure logic in `src/tray/` (`model.rs`: queue, status, menu;
  `source.rs`: configuration; `worker.rs`: worker and engine runner); the
  Win32 side in `src/win32/` (window, icon, menu, mutex, console), to which
  Q4 was widened. Details decided on the way are in spec L4. Tests: unit
  tests of queue, menu, status and worker; `tests/tray.rs` (process level:
  single instance, `WM_CLOSE`); `tests/fixtures/tests/tray.rs` (menu
  commands against fixtures: switch, neutral, Quit, crash recovery, error
  state, invalid configuration, T4). Indicative Q1 on the desktop with a
  test configuration: private working set about 1.3 MB, no measurable CPU
  over 60 s idle. Still open from WP7: the named pipe and forwarding of
  command-line requests (L1 second half), `--no-wait` and exit code 3 over
  the pipe, start at sign-in (Run key), idle trim of the tray itself,
  acknowledging the error state, a per-monitor DPI manifest, and a visual
  check of icons and menu. Next: WP8 (`WM_QUERYENDSESSION`, shutdown
  priority, `settings.json` last profile, console handler), then hotkeys,
  GUI and unassigned programs (M3).
