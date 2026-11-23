# Beta test on the owner's machine (1.0.0-beta.1)

What the owner checks on the real desktop with the first beta, in order of
importance. Each block says what to do, what to expect and what to note.
The details of each step are in [manual-checks.md](manual-checks.md); test
IDs (T…) refer to [spec.md](spec.md) section 10 and their results go into
[acceptance.md](acceptance.md), column "Desktop".

## 0. Install

1. Download from the GitHub release `v1.0.0-beta.1` (a pre-release) the file
   for your system: `kaltstellen-1.0.0-beta.1-windows-x86_64.exe` (Intel or
   AMD) or `…-windows-aarch64.exe` (ARM), and `checksums.txt`.
2. Check the file:
   `(Get-FileHash .\kaltstellen-1.0.0-beta.1-windows-x86_64.exe).Hash.ToLower()`
   must equal the value in `checksums.txt`.
3. Put it in a folder of its own that you can write to, for example
   `%LOCALAPPDATA%\Programs\kaltstellen\`, and rename it to `kaltstellen.exe`.
   Not below `Program Files` (the updater refuses there) and not in a
   package manager's folder.
4. The executable is not signed: Windows SmartScreen may warn on the first
   start ("More info", "Run anyway"). Note what it says (T21 also asks
   whether Defender reacts at all).
5. `kaltstellen --version` prints `kaltstellen 1.0.0-beta.1`.
6. A configuration: `kaltstellen.exe` looks for `config.toml` in
   `%APPDATA%\kaltstellen\`. Start from `config.example.toml` of the
   repository and replace its program names with real ones. For the first
   steps, a separate test configuration is safer (manual-checks.md, top).

Logs: `%LOCALAPPDATA%\kaltstellen\kaltstellen.log`. Attach the relevant part
to every finding.

## 1. Daily use (most important)

Use the beta for a few days with real profiles. Note anything surprising.

1. **Tray** (manual-checks "Tray icon and menu"): icon, tooltip, menu,
   errors that resolve by themselves, Explorer restart.
2. **Start at sign-in** ("Start at sign-in question"): answer the question,
   sign out and in: the tray starts (the open autostart check).
3. **Main window** ("Main window", steps 1 to 15 including 13a to 13d):
   look, scaling, keyboard, editing, icons, failure markers, drag and drop
   from Explorer.
4. **Program descriptions**: the description, product and publisher shown
   for programs in the main window, `kaltstellen list` and
   `kaltstellen status --unassigned` (see the steps in manual-checks.md for
   this feature).
5. **Hotkeys** (T17, "Hotkeys"): a real key press, held keys, AltGr.
6. **Unassigned programs** (T22): a real program with a tray icon.

## 2. Automatic triggers

manual-checks "Automatic triggers", steps 1 to 11, with a real program as
trigger. Most important: the notification, its click while on screen, the
menu item `Back to …`, Do Not Disturb or a full-screen game (4b), two
notifications in quick succession (4a, note the order).

## 3. Real programs and the session

1. **T13 long freeze**: freeze each program you want to use for 10 minutes
   or more, resume, check network, sync, sound and licence by hand. Note
   which programs tolerate it and which do not.
2. **T14 shell stays responsive**: with windowed programs frozen, the
   taskbar, Start and Explorer keep working.
3. **T19 trim**: with `trim` on a large program, compare the available
   memory in Task Manager before and after the switch, and how long the
   program takes to react after `neutral`.
4. **T18 elevated program**: a program started as administrator in a
   `suspend` entry is reported as needing administrator, the switch goes on.
5. **T7 sign-out**: sign out with a profile active and programs frozen.
   Expect: sign-out is not held up, after sign-in the tray offers the last
   profile without applying it.
6. **T20 shutdown**: shut down with programs frozen. Expect no "this app is
   preventing shutdown" screen for them. Note how long it took; this sets
   the 4 s wait that is still open (open-decisions, deferred item 1).
7. **T15** (optional, needs an administrator session and a trace tool such
   as Process Monitor): programs without an entry are never opened by
   kaltstellen.

## 4. Updates (with beta.2)

When `v1.0.0-beta.2` is published, the installed beta.1 finds it at its
daily check (or `Updates: install automatically` in the menu checks at
once) and installs it while `neutral` is active (manual-checks "Updates",
steps 1a and 4). Note whether SmartScreen warns when the new tray starts.

## What to report

Per finding: the step, what happened, what was expected, the log lines
around it, the version (`kaltstellen --version`) and the Windows version
(`winver`). For T-tests: pass or fail and the numbers asked for.

## Uninstall

Quit the tray, `kaltstellen autostart off`, delete the folder with the
executable and, if wanted, `%APPDATA%\kaltstellen` (configuration) and
`%LOCALAPPDATA%\kaltstellen` (state, log, WebView2 data).
