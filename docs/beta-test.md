# Beta test on the owner's machine (1.0.0-beta.1)

What the owner checks on the real desktop with the first beta, in order of
importance. Each block says what to do, what to expect and what to note.
The details of each step are in [manual-checks.md](manual-checks.md); test
IDs (T…) refer to [spec.md](spec.md) section 10 and their results go into
[acceptance.md](acceptance.md), column "Desktop".

## Scripted run

`scripts/beta-test.ps1` walks through this plan step by step (Windows
PowerShell 5.1 or PowerShell 7, no modules). It installs the release,
backs up your configuration, builds a test configuration from a list of
programs you choose, prepares each step, checks what a script can check and
asks you about the rest. Run it from a clone of the repository at the tag
of the release (`git checkout v1.0.0-beta.1`); it is not part of the
release files.

```powershell
.\scripts\beta-test.ps1 setup            # download, verify, install, back up, test config
.\scripts\beta-test.ps1 run list         # the tests and how far you are
.\scripts\beta-test.ps1 run tray         # one test; "run all" runs all but t7 and t20
.\scripts\beta-test.ps1 run t7           # sign-out test; after signing in again:
.\scripts\beta-test.ps1 collect          # report zip on the Desktop
.\scripts\beta-test.ps1 teardown         # put everything back (-Uninstall removes the exe)
```

If PowerShell refuses to run scripts, start it with
`powershell -ExecutionPolicy Bypass -File .\scripts\beta-test.ps1 ...`.

**What setup does.**

- Downloads `kaltstellen-<version>-windows-<arch>.exe` for this machine and
  `checksums.txt` from the GitHub release (`-Version`, default
  `1.0.0-beta.1`), refuses the file if its SHA-256 differs, and installs it
  as `%LOCALAPPDATA%\Programs\kaltstellen\kaltstellen.exe` (`-InstallDir`).
  `-Exe <file>` installs a local build instead. This download is the
  script's only network access; the test tray itself may check GitHub for
  updates unless the update mode is off. With `-Sandbox`, an `-InstallDir`
  that holds a `kaltstellen.exe` (or the real install folder) is refused.
- Copies `%APPDATA%\kaltstellen`, `%LOCALAPPDATA%\kaltstellen` and the
  Run value `kaltstellen` to `%LOCALAPPDATA%\kaltstellen-beta-backup\<time>\`
  and checks the copies (SHA-256 of every file) before it changes anything,
  the executable included; if setup fails before it is ready, an executable
  it replaced is put back. A running kaltstellen is quit first, after
  asking, and setup says how to start it again afterwards. A second setup
  without teardown is refused. A `KALTSTELLEN_*` variable set for the user
  or the machine is reported, because it reaches every kaltstellen.
- Reads `%LOCALAPPDATA%\kaltstellen-beta-backup\beta-test.json`, the list of
  programs the test may touch: which may be frozen, which may be stopped,
  a large one for T19, optionally one you start as administrator (T18) and
  one with a tray icon (T22). The trigger is by default `"builtin"`: a
  renamed copy of `ping.exe` in the test folder that the script starts and
  ends itself, so the trigger test never touches a program of yours. On the
  first setup the list is copied from
  [beta-test.example.json](../scripts/beta-test.example.json) and opened in
  Notepad. Nothing outside this list is frozen or stopped; stop entries get
  `on_timeout = "keep"` unless the list says `kill`. The script closes or
  ends only processes it started itself (PID and start time recorded); for
  any other process of a listed name it asks you and never forces it.
- Writes the test configuration (profiles `beta-freeze`, `beta-stop`,
  `beta-failure`, `beta-dragdrop`, `beta-trigger`, `beta-trim-on`,
  `beta-trim-off`, `beta-elevated`, `beta-session`, `beta-altgr`) and sample
  files for drag and drop in `%TEMP%\kaltstellen-beta-<time>\test`, and
  starts the tray. Automatic switching starts paused, so the trigger
  program does nothing before test `triggers`. Updates are set to `check and
  ask` for the test, so nothing installs by itself; the product itself still
  checks GitHub once a day in this mode (only `off` stops that). Test
  `updates` sets `off` through the tray (`kaltstellen update --mode off`)
  for the rest of the test and installs only if you say so.
- `-Sandbox` leaves the real folders and the Run key alone: configuration,
  data, executable and test files live in `%TEMP%\kaltstellen-beta-<time>`,
  the Run value in `HKCU\Software\kaltstellen-beta-<time>\Run`, and the
  script sets `KALTSTELLEN_CONFIG_DIR`, `KALTSTELLEN_DATA_DIR` and
  `KALTSTELLEN_RUN_KEY` for everything it starts. Sign-in checks are `n/a`
  there.
- `-DryRun` (or `-WhatIf`) prints the plan and changes nothing; it works for
  teardown too.

**What a test step does.** It prepares the step through the installed
command line (switches a profile, starts listed programs that have a
`path`), prints what to do and waits for Enter, then checks what it can:
the lines of `kaltstellen.log` written since the step began,
`kaltstellen status`, whether every thread of a program is suspended,
whether windows answer, available memory (T19), taskbar and Explorer
response times (T14). What only eyes can judge it asks as y/n/s. Before a
step that freezes or closes programs it says so: save your work. At every
prompt `r` resumes everything (`recover`, then `neutral`) and `q` stops the
run after that. Results go to `results.jsonl` and `results.log` next to the
backup.

| Test | Automatic | Guided (you act, the script checks) | Only you |
| --- | --- | --- | --- |
| `tray` | config error by check-config | profile from the menu (log, frozen threads), Explorer restart, shortcut start, Quit | icon, tooltip, menu without settings |
| `signin` | Run value and `autostart on` (switched on with `kaltstellen autostart on` if you answered No) | the question at the first start | menu while asking, question text |
| `window` | start time, page ready, one window only, WebView2 processes end; header switches against `kaltstellen autostart` and `kaltstellen automation`, `Automatic switching` disabled without triggers | Apply profile, comments kept | look, scaling, keyboard, editing, header hint |
| `failure` | KS110, error resolves within 30 s | | red icon, marker, hover text |
| `dragdrop` | which entries the file gained | the drops | outline, status line |
| `descriptions` | `list --info`, `status --unassigned` recorded (`n/a` for a build without `list --info`) | | the window |
| `hotkeys` | taken hotkey warned (second test tray), Ctrl+Alt+7 refused | key press counted once per press | AltGr+7 |
| `unassigned` | listed as new | | menu item, Add, Dismiss |
| `stop` | closed, started again on neutral | | |
| `triggers` | check-config, apply within 5 s, back to neutral, manual switch wins, pause and resume with `kaltstellen automation off/on` (survives a restart), priority in the file | notification click, 4a, 4b, rename | notifications, header switch, `Automatic switching paused` in the menu |
| `t14` | taskbar and Explorer answer, no ghost windows | | Start menu |
| `t19` | available memory with trim on and off, resume time | | felt delay |
| `t18` | KS102, the rest of the switch | start as administrator | |
| `t13` | stays frozen for `-FreezeMinutes` (10) | | network, sync, sound, licence |
| `t15` | PIDs and times recorded | the trace | |
| `defender` | detections, if readable | | Protection history |
| `updates` | `update --mode off` taken over by the tray, version, `kaltstellen.old.exe`, one tray | install | offer in the menu, Mode in the main window, SmartScreen |
| `t7`, `t20` | resume time from the log, last profile offered not applied, tray at sign-in | | screens at sign-out or shutdown |

**T7 and T20** end the session and run on their own (`run t7`, never with
other tests). The script prepares them (programs frozen,
optionally the trigger active for "Automatic triggers" step 11), records a
pending marker and signs out (`shutdown /l`) or shuts down
(`shutdown /s /t 0`, `-Restart` for `/r`) only after you type `SIGNOUT` or
`SHUTDOWN`; after that it does nothing more. After signing in again, `collect` evaluates them: the time from
`tray: session is ending; resuming everything` to `tray: end of session
finished` (or `resuming did not finish in time`), whether the tray started
at sign-in and offers the last profile without applying it.

**collect** writes `kaltstellen-beta-report-<time>.zip` to the Desktop
(`-ReportDir`): the results, the `kaltstellen.log` lines of the test steps,
`kaltstellen --version`, the Windows version, `state.json` and
`settings.json`. For T7 and T20 only the log lines of the session end and
of the first tray start after it are taken, not the time between. The
results include the output of `kaltstellen list` and `status
--unassigned`, which names programs that ran; `collect -NoProgramLists`
leaves it out, and `state.json` too (it names what was frozen); the results
and log lines still name the programs of `beta-test.json`, which are the
test. Your profile folder (also in its short 8.3 form), user name, domain
and computer name appear as `%USERPROFILE%`, `%USERNAME%`, `%USERDOMAIN%`
and `%COMPUTERNAME%`, account SIDs as `S-1-5-21-<sid>`; nothing else is
collected. Look at it before you send it.

**teardown** is safe to repeat, also after an abort or a crash: `recover`,
`neutral`, quit the tray (and windows) it started, end the programs it
started (fixtures and copies in the test folder are killed, your programs
are asked to close), end the WebView2 processes of the test's data folder,
wait until no kaltstellen runs in the session (it asks you to quit any
other copy), check each backup against its manifest and only then put the folders and
the Run value back exactly (and remove them if they did not exist before),
remove the test folder (links in it are removed, never followed) and test
registry key. `-Uninstall` also removes the installed
executable, or puts back the one setup replaced after checking its SHA-256.
If setup replaced your own `kaltstellen.exe` and you leave `-Uninstall` out,
teardown restores everything else and then waits: `teardown -Uninstall`
puts yours back, `teardown -KeepBeta` keeps the beta. Results and backups
stay in `%LOCALAPPDATA%\kaltstellen-beta-backup`; delete that folder when
you no longer need them.

**What it does not do**: look at icons, menus, notifications or windows for
you, click notifications, drag files, judge whether a program survived a
long freeze, start a trace for T15, or sign out without your word.
Unattended runs (`-Unattended`, `-Answer`, `-AnswersFile`,
`-SimulateSessionEnd`) exist for testing the script itself; their answers
are recorded as presets, not as something seen.

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

- **Automatic** (manual-checks "Updates", step 1a): keep the mode `Install
  automatically` (main window, `Updates`). When `v1.0.0-beta.2` is out,
  apply a profile and wait for the daily check (or choose `Install
  automatically` again in the main window to check at once): nothing
  is installed while the profile is active; switch to `Neutral`: it
  installs without a window and the tray comes back with the new version.
- **On request with frozen programs** (step 4): before beta.2 is out,
  choose `Check and ask` in the main window. Then apply a profile that freezes
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
