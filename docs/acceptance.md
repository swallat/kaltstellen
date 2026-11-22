# Acceptance checklist

Tests from [spec.md](spec.md) section 10. "VM" is the test VM, "desktop" the
real machine. Results: `pass`, `fail`, `open` (not run yet), `n/a`.

| ID | Check (short) | WP | VM | Desktop |
| --- | --- | --- | --- | --- |
| T1 | W1 order resume, stop, suspend, start | WP4 | open | pass |
| T2 | Shared freeze kept, no resume event | WP4 | open | pass |
| T3 | Failed stop: switch finishes, exit code 1 | WP4/5 | open | pass (CLI; tray error state by log, icon not looked at) |
| T4 | Two queued requests: only the last runs | WP4 | open | pass (tray menu commands and over the pipe: the replaced request ends with exit code 3) |
| T5 | Kill while frozen, restart: nothing frozen | WP6 | open | pass (CLI `recover`, tray restart, Ctrl+Break during an in-process switch) |
| T6 | Stale PID ignored, corrupt state renamed | WP6 | open | pass |
| T7 | Sign-out/shutdown resume, last profile offered | WP8 | open | open (logic passes with posted `WM_QUERYENDSESSION`/`WM_ENDSESSION`; no real sign-out run) |
| T8 | Invalid config keeps old one, file and line | WP3 | open | pass (CLI; tray reload by log, icon not looked at) |
| T9 | Hard-protected refused in config and CLI | WP2 | open | pass |
| T10 | `restart_on_neutral` only on neutral | WP4 | open | pass |
| T11 | `on_timeout = "keep"` with open dialog | WP2 | open | pass (engine) |
| T12 | Idle footprint Q1 over 60 s | WP7 | open | indicative pass (release build with pipe server, test configuration: private working set 1.4 MB, no measurable CPU over 63 s) |
| T13 | Long freeze of real programs | none | n/a | open |
| T14 | Shell responsive with windowed programs frozen | WP2/7 | open | open |
| T15 | Unmatched programs never opened | WP2 | open | open |
| T16 | Self and ancestor protection | WP2 | open | open (unit-tested) |
| T17 | Hotkey taken or `Ctrl+Alt+<printable>` | WP9 | open | open (logic passes: taken hotkey held by the test process, press posted as `WM_HOTKEY`; no real key press, see manual-checks.md) |
| T18 | Elevated target reported, switch continues | WP2 | open | open |
| T19 | Trim measured as available memory | WP2 | open | open |
| T20 | Shutdown not blocked by frozen programs | WP8 | open | open (needs a real shutdown on the VM) |
| T21 | Defender does not block or quarantine | all | open | open |
| T22 | N1 unassigned programs marked new | WP11 | open | open |

## Test fixtures

Built by `cargo build --workspace` into `target\debug`:

| Fixture | Purpose |
| --- | --- |
| `ks-heartbeat <file> [--alloc-mb n]` | Console app; heartbeat every 100 ms; optional memory for T19. |
| `ks-spawner <dir>` | Parent with a child and an orphan (PIDs in `<dir>\*.pid`). |
| `ks-window [--close-delay-ms n]` | Windowed app; closes on `WM_CLOSE`, optionally slowly (W3). |
| `ks-dialog` | Shows an unsaved-changes box on `WM_CLOSE` and stays open (T11). |

All fixtures accept `--heartbeat <file>`. A heartbeat that stops advancing
proves the process is frozen.

Window fixtures need an interactive desktop session (console or RDP); in a
non-interactive SSH session (session 0) their windows are not visible and
`WM_CLOSE` tests cannot run.
