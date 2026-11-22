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
- Owner decisions (2026-11-22): spec blocks A8, C8, L4 and the widened Q4
  confirmed. Added on request: `args`/`cwd` on stop entries for the restart on
  `neutral`, and `confirmed = true` for warn-list entries (works from the tray).
- WP7 second increment and WP8 (2026-11-22): named pipe of the tray
  (`src/win32/pipe.rs`, `src/tray/{protocol,server,client}.rs`, format in
  `docs/pipe-protocol.md`, spec L5): `switch`, `recover` and `status` go
  through a running tray with the same output and exit codes, `--no-wait`,
  exit code 3 for a superseded request, in-process fallback without a tray.
  Console control handler for in-process switches (spec L6, `KS403`).
  Session end (spec L7): resume at `WM_QUERYENDSESSION`, shutdown priority,
  stopped records cleared at `WM_ENDSESSION`, `settings.json` with the last
  profile offered in the menu, power events logged. Start at sign-in via the
  per-user Run key (`kaltstellen autostart`, menu item; spec L8). Idle trim of
  the tray dropped (not needed for Q1). Tests at process level against the
  fixtures; session-end messages are posted to a test tray, never a real
  sign-out. Still open for M2: T7, T20 and T5 on the VM with a real sign-out
  and shutdown; connecting to the pipe as another user (only the DACL is
  checked); visual check of icon and menu; acknowledging the error state; a
  per-monitor DPI manifest; the manual 30-minute run of M2.
- M2 leftovers, WP9, WP10 (2026-11-22): `Clear error state` in the tray menu
  and a manifest with per-monitor DPI awareness (spec L9). Hotkeys registered
  on the tray window (spec L10). Main window `kaltstellen gui` (spec M6):
  toolkit spike measured native Win32 against egui, native chosen; edits
  through `toml_edit` with validation and atomic saves (`src/gui/document.rs`),
  view model in `src/gui/model.rs`, window in `src/win32/gui.rs`; tray item
  `Open kaltstellen…` starts it. Tested without eyes: controls exist, posted
  clicks change the file as expected, invalid edits change nothing, apply
  runs the switch, the process ends on close. Visual checks are listed in
  `docs/manual-checks.md`, decisions in `docs/open-decisions.md`.
- WP11 (2026-11-22): unassigned programs (spec N2): `src/unassigned.rs` with
  `seen.json`, notification-icon registry and visible-window sources (read
  only), `new` marker, dismiss; `kaltstellen status --unassigned`; tray menu
  line `Unassigned programs (N)`; list with `Add to profile` and `Dismiss` in
  the main window. Tests use injected observations; the live snapshot is
  only read.
- M4 design notes (2026-11-22): `docs/design-elevated-helper.md` (A5, P8) and
  `docs/design-automatic-triggers.md` (U4), for the owner to decide; no code.
- Main window on WebView2 (2026-11-23, owner decision; branch `gui-webview`):
  the native window is replaced by a WebView2 page (`ui/`, embedded; host in
  `src/win32/webview.rs`, bridge in `src/gui/bridge.rs`, documented in
  `docs/gui-bridge.md`). Document editing and validation stay in Rust.
  Tests drive the real page through its bridge (`--drive`) and capture it
  (`--screenshot`); screenshots of all views in light and dark were reviewed.
  Indicative: page ready after about 0.54 s, about 74 to 78 MB private
  working set for the window and its WebView2 processes, executable 2.97 MB
  (was 2.54 MB), tray idle unchanged at about 1.6 MB.
