# Acceptance checklist

Tests from [spec.md](spec.md) section 10. "VM" is the test VM, "desktop" the
real machine. Results: `pass`, `fail`, `open` (not run yet), `n/a`.

| ID | Check (short) | WP | VM | Desktop |
| --- | --- | --- | --- | --- |
| T1 | W1 order resume, stop, suspend, start | WP4 | pass (`cargo test`) | pass |
| T2 | Shared freeze kept, no resume event | WP4 | pass (`cargo test`) | pass |
| T3 | Failed stop: switch finishes, exit code 1 | WP4/5 | pass (`cargo test`) | pass (CLI; tray error state by log, icon not looked at) |
| T4 | Two queued requests: only the last runs | WP4 | pass (`cargo test`) | pass (tray menu commands and over the pipe: the replaced request ends with exit code 3) |
| T5 | Kill while frozen, restart: nothing frozen | WP6 | pass (`cargo test`; with the tray on `8e9e0c1`, debug build: two programs frozen through the tray, tray killed hard, both stay frozen; after the restarted tray's start-up recovery both run again and `status` reports `neutral`, nothing frozen or stopped) | pass (CLI `recover`, tray restart, Ctrl+Break during an in-process switch) |
| T6 | Stale PID ignored, corrupt state renamed | WP6 | pass (`cargo test`) | pass |
| T7 | Sign-out/shutdown resume, last profile offered | WP8 | open (logic passes in `cargo test`; real sign-out prepared, not run) | open (logic passes with posted `WM_QUERYENDSESSION`/`WM_ENDSESSION`; no real sign-out run) |
| T8 | Invalid config keeps old one, file and line | WP3 | pass (`cargo test`) | pass (CLI; tray reload by log, icon not looked at; window refuses invalid edits and invalid files, file unchanged) |
| T9 | Hard-protected refused in config and CLI | WP2 | pass (`cargo test`) | pass |
| T10 | `restart_on_neutral` only on neutral | WP4 | pass (`cargo test`) | pass |
| T11 | `on_timeout = "keep"` with open dialog | WP2 | pass (`cargo test`) | pass (engine) |
| T12 | Idle footprint Q1 over 60 s | WP7 | indicative pass (release tray, `config.example.toml`, 91 s: private working set at most 1.31 MB, private bytes 1.86 MB, CPU time below the 15.6 ms timer resolution) | indicative pass (release build with pipe server and hotkeys, test configuration: private working set 1.5 MB, no measurable CPU over 63 s) |
| T13 | Long freeze of real programs | none | n/a | open |
| T14 | Shell responsive with windowed programs frozen | WP2/7 | pass (4 `ks-window` frozen, with and without `minimize_before_suspend`: taskbar answers `WM_NULL` in 0 to 3 ms, a new Explorer window opens in 1.3 to 3.0 s, answers and closes; no ghost windows) | open |
| T15 | Unmatched programs never opened | WP2 | open (ETW trace prepared, needs an elevated session) | open |
| T16 | Self and ancestor protection | WP2 | pass with the tray running on `8e9e0c1`, debug build, three runs (a `cmd` copy runs a second copy that runs `kaltstellen switch`; the profile suspends that program name and another one: the switch returns in 0.6 to 2.1 s, neither copy in the caller's chain is reported frozen, the first copy's other child and the other program stop beating; both beat again after `neutral`). Earlier **fail** on `d20c727`, fixed in `4b8999d` | pass with the tray running (regression test with a program that runs `kaltstellen switch` itself; `cargo test`) |
| T17 | Hotkey taken or `Ctrl+Alt+<printable>` | WP9 | pass with key presses through `SendInput` (the keyboard's input queue, not a posted message) on `8e9e0c1`, debug build: `Ctrl+Shift+Alt+F9` switched the tray to its profile (`status` showed it within 92 ms) and froze its program; `Ctrl+Alt+Q` was warned about and not registered, and with the German layout active (checked) AltGr+Q typed exactly `@` into an empty text box. A key press by a person at the keyboard is still open (manual-checks.md) | open (logic passes: taken hotkey held by the test process, press posted as `WM_HOTKEY`; no real key press, see manual-checks.md) |
| T18 | Elevated target reported, switch continues | WP2 | open (prepared, needs administrator credentials) | open |
| T19 | Trim measured as available memory | WP2 | indicative pass (500 MB fixture: available memory +404 MB with trim, +11 MB without; working set 504 MB to 0; first heartbeat 74 ms after starting `resume`) | open |
| T20 | Shutdown not blocked by frozen programs | WP8 | open (prepared, needs a real shutdown) | open (needs a real shutdown on the VM) |
| T21 | Defender does not block or quarantine | all | pass (Defender real-time and behaviour monitoring on: build, three test runs, several tray runs; no detection, nothing quarantined; unsigned binary; cloud protection could not be reached). Again with the updater and the main window (signatures 1.463.424.0): over two days of builds, full test runs including the update tests against the loopback server, main window starts and the 30-minute run, Defender's detection list names no kaltstellen or test file; an explicit `MpCmdRun -Scan` of the release (`3b6f4ab`) and debug (`8e9e0c1`) executables found nothing | open |
| T22 | N1 unassigned programs marked new | WP11 | open (logic passes in `cargo test`; no real tray program could be started on the VM) | open (logic passes: a window fixture that starts later is listed as new by `status --unassigned`, an entry removes it, the tray counts it; the list in the window not looked at) |

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

VM run 2026-11-22 on `wp7-tray` (`d20c727`), Windows 11 26H2, standard user
in an RDP session, Defender on: format, unsafe check, clippy and
`cargo test --workspace` green; 147 tests passed three times in a row, none
skipped for a missing desktop, no fixture left running. Release binary
2.42 MiB.

VM run 2026-11-23, same VM, standard user in the console session,
Defender on. Code as merged in #3 (`8e9e0c1`): 207 tests green. T5, T16
and T17 above ran on a debug build of `8e9e0c1`; the main window and the
30-minute run on a release build of `3b6f4ab` (3.2 MiB), whose code is the
same (later commits only change a test). Sizes in MiB.

- Main window (WebView2 runtime 158.0.4370.33, `config.example.toml`, three
  starts): a main window handle after 1.1 to 1.2 s, page ready after 1.4 to
  1.8 s; window process 3.5 MiB private, 26.5 MiB working set; with the six
  processes below it (WebView2) 98 to 101 MiB private and 315 to 328 MiB
  working set. After the window process has ended on `WM_CLOSE`, the others
  end within 0.15 to 0.24 s. Without a runtime
  (`WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` pointing nowhere): `KS702` with the
  configuration file named, exit code 1 after 82 ms.
- 30-minute run with the example profiles through the tray (the example's
  program names are fixture copies, paths point at them): `focus`,
  `studio`, `neutral` every 20 s, 90 switches, none failed; median 94 ms,
  longest 1.98 s; the tray's private bytes 2.7 MiB on average over the
  first samples, 3.0 MiB over the last, 3.5 MiB at most; handles 171 to
  173 throughout. At the end `neutral`, nothing frozen or stopped, a
  process of every program running, and every program that was started
  with a heartbeat beating, except two as configured: `chat` is stopped and
  started again by `restart_on_neutral` with the example's arguments (no
  heartbeat), and the `helper` that `studio` stops stays stopped. No `WARN`
  or `ERROR` line in the log.
- Start at sign-in question (L8) without the test switch, answered through
  UI Automation: the first tray start shows the question after 0.9 s;
  "Yes" writes the Run value (a test key) with the quoted executable and
  `tray`, `kaltstellen autostart` reports `on`, `settings.json` records that
  the question was asked, and a second start does not ask again. Starting
  after a real sign-in is still open.
- T22: no real program that lives in the notification area could be started
  on this VM (OneDrive is blocked by policy, see the first run); still open.
- Not possible here: real sign-out and shutdown (T7, T20) would end this
  session; T15, T18 and the pipe test with a second account need
  administrator rights, which this user does not have.
