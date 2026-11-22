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
