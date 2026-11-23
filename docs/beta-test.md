# Beta test on the owner's machine (1.0.0-beta.1)

What the owner checks on the real desktop with the first beta, in order of
importance. Each block says what to do, what to expect and what to note.
The details of each step are in [manual-checks.md](manual-checks.md); test
IDs (T…) refer to [spec.md](spec.md) section 10 and their results go into
[acceptance.md](acceptance.md), column "Desktop".

## 0. Install

1. Download from the GitHub release `v1.0.0-beta.1` (a pre-release) the file
   for your system, `kaltstellen-1.0.0-beta.1-windows-x86_64.exe` (Intel or
   AMD) or `kaltstellen-1.0.0-beta.1-windows-aarch64.exe` (ARM), and
   `checksums.txt`.
2. Check the file (with the name of the file you downloaded):
   `(Get-FileHash .\kaltstellen-1.0.0-beta.1-windows-x86_64.exe).Hash.ToLower()`
   must equal its value in `checksums.txt`.
3. Put it in a folder of its own that you can write to, for example
   `%LOCALAPPDATA%\Programs\kaltstellen\`, and rename it to `kaltstellen.exe`.
   Not below `Program Files` (the updater refuses there) and not in a
   package manager's folder.
4. The executable is not signed: Windows SmartScreen may warn on the first
   start ("More info", "Run anyway"). Note what it says.
5. `kaltstellen --version` prints `kaltstellen 1.0.0-beta.1`.

Logs: `%LOCALAPPDATA%\kaltstellen\kaltstellen.log`, timestamps in UTC.
Attach the relevant part to every finding.

## Before freezing real programs

- **The way out**: `kaltstellen recover` resumes everything kaltstellen
  froze (recorded in `%LOCALAPPDATA%\kaltstellen\state.json`), with or
  without a running tray (spec L3). Task Manager cannot resume a frozen
  program; `recover` can. If the tray crashes or is ended, its next start
  resumes everything and sets `neutral` (S3); with start at sign-in this
  happens at the next sign-in by itself.
- `Quit` resumes frozen programs but leaves stopped ones stopped (L2);
  switch to `Neutral` first if they should come back (`restart_on_neutral`).
- Save your work before a switch. Begin with `suspend` entries for programs
  that are not critical. A `stop` entry closes a program; with
  `on_timeout = "kill"` (the default) it ends it after `stop_timeout_s`,
  unsaved work included; `"keep"` leaves it running instead.
- Do not freeze programs that others call into or that hold the clipboard
  (spec R2), and run the long freeze (T13) on programs whose state you can
  afford to lose.

## Test configuration or real configuration

The first look at the menu and the window can use a test configuration
(manual-checks.md, top: `KALTSTELLEN_CONFIG_DIR`, `KALTSTELLEN_DATA_DIR`,
`KALTSTELLEN_RUN_KEY`). These variables only apply in the shell that sets
them, and the test Run key is never run by Windows. From section 1 on, close
that shell and use the real configuration: `%APPDATA%\kaltstellen\
config.toml`, for example `config.example.toml` from the repository at tag
`v1.0.0-beta.1` with real program names. Sign-in checks (1.2, 3.6) need the
real configuration with start at sign-in on.

## 1. Daily use (most important)

Use the beta for a few days with real profiles. Note anything surprising.

1. **Tray** (manual-checks "Tray icon and menu"): icon, tooltip, menu,
   errors that resolve by themselves, Explorer restart.
2. **Start at sign-in** ("Start at sign-in question"): answer Yes, sign out
   and in: the tray starts.
3. **Main window** ("Main window", steps 1 to 15 including 13a to 13d):
   look, scaling, keyboard, editing, icons, failure markers, drag and drop
   from Explorer.
4. **Program descriptions** (manual-checks, the section for this feature):
   the description, product and publisher shown for programs in the main
   window, `kaltstellen list --info` and `kaltstellen status --unassigned`
   (manual-checks 13e, including the check that no connection is made).
5. **Hotkeys** ("Hotkeys", steps 1 to 3; T17): a real key press, held keys,
   a taken hotkey, AltGr.
6. **Unassigned programs** (T22): a real program with a tray icon.
7. **Defender** (T21): after a few days, Windows Security, Protection
   history: nothing about kaltstellen.

## 2. Automatic triggers

manual-checks "Automatic triggers", steps 1 to 10, with a real program as
trigger. Most important: the notification, its click while on screen, the
menu item `Back to …`, Do Not Disturb or a full-screen game (4b), two
notifications in quick succession (4a, note the order).

## 3. Real programs and the session

1. **T13 long freeze**: freeze each program you want to use for 10 minutes
   or more, resume, check network, sync, sound and licence by hand. Note
   which programs tolerate it and which do not.
2. **T14 shell stays responsive**: with windowed programs frozen, the
   taskbar, Start and Explorer keep working.
3. **T19 trim**: with `trim = true` on a large program, note the available
   memory in Task Manager before and after the switch, and how long the
   program takes to react after `neutral`; then the same with
   `trim = false` for comparison.
4. **T18 elevated program**: a program started as administrator in a
   `suspend` entry is reported as needing administrator, the switch goes on.
5. **T15** (optional, needs an administrator session and a trace tool such
   as Process Monitor): programs without an entry are never opened by
   kaltstellen.
6. **T7 sign-out** (ends the session): sign out with a profile active and
   programs frozen. Expect: sign-out is not held up; the log has
   `tray: session is ending; resuming everything` and the resume finished
   before the session ended; after sign-in the tray offers the last profile
   without applying it. Also step 11 of "Automatic triggers" with a
   trigger active.
7. **T20 shutdown** (ends the session): shut down with programs frozen.
   Expect no "this app is preventing shutdown" screen for them. From the
   log after the restart, note the time between
   `tray: session is ending; resuming everything` and the end of the resume
   (or `resuming did not finish in time`): this sets the 4 s wait that is
   still open (open-decisions, deferred item 1).

## 4. Updates (with beta.2)

The default mode installs a newer version by itself while `neutral` is
active. Choose one of the two checks for beta.1 to beta.2; the other waits
for the next beta:

- **Automatic** (manual-checks "Updates", step 1a): keep `Updates: install
  automatically`. When `v1.0.0-beta.2` is out, apply a profile and wait for
  the daily check (or choose the menu item again to check at once): nothing
  is installed while the profile is active; switch to `Neutral`: it
  installs without a window and the tray comes back with the new version.
- **On request with frozen programs** (step 4): before beta.2 is out,
  choose `Updates: check and ask`. Then apply a profile that freezes
  something and run `kaltstellen update`: the frozen program runs again,
  one new tray icon appears, `kaltstellen --version` shows beta.2.

Note whether SmartScreen warns when the new tray starts.

## What to report

Per finding: the step, what happened, what was expected, the log lines
around it, the version (`kaltstellen --version`) and the Windows version
(`winver`). For T-tests: pass or fail and the numbers asked for.

## Uninstall

Switch to `Neutral` (stopped programs come back), then `Quit`,
`kaltstellen autostart off`, and delete the folder with the executable
(with `kaltstellen.old.exe` after an update). If wanted, also delete
`%APPDATA%\kaltstellen` (configuration) and `%LOCALAPPDATA%\kaltstellen`
(state, log, WebView2 data).
