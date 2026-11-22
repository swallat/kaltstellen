# kaltstellen specification (draft v1.1)

Revision: v1.1 incorporates the Windows-internals review. Changes from that review are marked **[r2]**; the first consolidation review is in [spec-review-notes.md](spec-review-notes.md).

Status: draft for `docs/spec.md` (English, no machine-specific names).
Legend: **[decided]** confirmed by the project owner; **[assumed]** author's reasoned default, to be confirmed. Anything unmarked is derived from `requirements.md` or follows directly from a marked decision.

## 1. Scope of version 1

- Manual activation only: tray menu, command line, optional global hotkey. **[decided]** (U5; automatic triggers U4 move to a later milestone.)
- Actions `start`, `stop`, `suspend`, `ignore`; user processes of the current Windows session only. Services (A5) and elevation are deferred to M4, to be done with a small elevated helper rather than an elevated tray. **[decided]**
- Configuration is one TOML file and stays authoritative; the GUI reads and writes it. **[assumed]**

## 2. Concepts

- **A7 Program** **[decided]**: identified by executable name; an entry may add a full path to tell programs with equal names apart. All child processes of a matched process belong to the program. Only processes in the current user's session are considered.
  - Tree resolution **[r2]**: children are found from a process snapshot via parent PID, accepted only if the child was created after its parent (guards against reused parent PIDs). Orphans whose parent already exited are not part of the tree; list such helper programs as separate entries. Windows has no enforced process tree.
  - Self-protection **[r2]**: kaltstellen's own processes, their ancestors, and the running tray instance are never touched, even when matched by an entry (a `switch` launched from a frozen program's child would otherwise freeze its own caller).
  - Least access **[r2]**: kaltstellen opens only processes that match an entry of the target profile or are recorded in state. It does not open other processes (relevant for anti-cheat and EDR software that watch handles).
  - A process that exits between enumeration and access is skipped silently; it is not an error.
  - Executable names compare case-insensitively; paths compare case-insensitively after normalisation to the full DOS path (`QueryFullProcessImageNameW`).
  - Packaged (UWP/MSIX) apps are not covered in version 1; behaviour is unspecified because Windows manages their lifecycle itself.
- **Profile**: a named list of program entries. `neutral` is built in, cannot be edited or deleted, and has no entries.
- **Active profile**: exactly one at a time (P4). Initially, and after every sign-in, it is `neutral`.
- **Switch**: moving from the active profile to a target profile.

## 3. Switch semantics

Let *prev* be the set of processes recorded in state (suspended or stopped by kaltstellen) and *target* the entries of the new profile.

- **W1 Order** **[decided]**: (1) resume, (2) stop, (3) suspend, (4) start.
  - *Resume* applies to every recorded suspended process **except** those that the target profile also suspends; those stay frozen untouched (no unfreeze/refreeze, no second trim). **[assumed]**
  - *Stop* ends processes whose target action is `stop`. A process is resumed before it is stopped, because a frozen process cannot close its windows.
  - *Suspend* freezes processes whose target action is `suspend` and that are not frozen already (W5). Windows are minimised first (W7).
  - *Start* launches entries whose action is `start` when no matching process is running. A matching process that is suspended counts as running and is resumed in step 1, not started again.
  - Entries with `ignore`, and programs not in the target profile, are left in whatever state they are in, except that previously recorded suspended ones are resumed (step 1).
- **W2 Failures** **[decided]**: a failed step does not stop the switch. Errors are collected, reported at the end, and the tray icon enters its error state. Nothing is rolled back. The state file reflects what really happened.
- **W3 Concurrency** **[decided]**: while a switch runs, a further request is queued. If several arrive, only the latest is kept. A queued request that equals the profile that will be active after the running switch is dropped.
- **W4 Same profile** **[assumed]**: switching to the already-active profile re-applies it. Because it is idempotent (W5), this resumes nothing that stays frozen, starts what is missing and freezes what is not frozen.
- **W5 Idempotence**: applying the same profile twice has the same effect as once. A process is suspended at most once (S2/S8).
- **W6 Snapshot**: process matching happens at the moment each step runs. After the suspend step, kaltstellen enumerates once more and freezes processes that appeared in the meantime (for example children spawned by a parent just before it froze). There is no background watcher in version 1; a program launched by the user while its profile is active is not frozen until the next switch. **[decided]**
- **W7 Minimise before freeze** **[r2]**: when `minimize_before_suspend` is true, visible top-level windows of the process are minimised (`ShowWindowAsync`), and kaltstellen waits until they report minimised or 1 s has passed before freezing. Windows that cannot be minimised from a lower integrity level are skipped. The previous window layout is not restored on resume. This reduces, but does not guarantee, "not responding" ghost windows (S4 is best effort).
- **P5 Neutral** **[decided]**: switching to `neutral` resumes everything kaltstellen suspended. Programs it stopped are restarted only if their entry set `restart_on_neutral = true` (default `false`); that setting is honoured only on a switch to `neutral`, never on a switch to another profile. Programs started by kaltstellen are not stopped again. **[assumed]**
- **A2 Stop**: send a close request to the program's windows, wait `stop_timeout_s`, then act according to the entry's `on_timeout`: `kill` (terminate, default) or `keep` (leave running, report an error). **[decided]** (A2 in `requirements.md` says "then terminate"; `keep` protects programs with unsaved-changes dialogs.)
- **A8 Details fixed while implementing** **[decided]** (WP2, WP4, WP6; confirmed by the owner 2026-11-22):
  - The process list comes from one system query (`NtQuerySystemInformation`) that returns PID, parent PID, session, creation time and name without opening any process (A7 least access). A process is opened only once it matched an entry; entries with a `path` open the processes of that name to read their image path.
  - A hard-protected process below a matched program (typically its console host) is left out together with everything below it.
  - Self-protection: an ancestor of kaltstellen that matches an entry is skipped itself, its other children are still part of the program. Processes started by kaltstellen's own process are never part of a program.
  - Stop: the close request (`WM_CLOSE`) goes to all top-level windows of the program, hidden ones included, so programs that live in the notification area can close too. A program without any window cannot be asked; `on_timeout` then applies at once instead of after `stop_timeout_s`.
  - Start: without `cwd` the program starts in the folder of its executable. It gets its own console and inherits no handles.
  - A step on a process that has exited meanwhile counts as done.
  - State (WP6): the boot time of S10 is the creation time of the `System` process. A suspended process is recorded and the state written before it is frozen, so a crash in between leaves a record to undo, never an unknown frozen process. The in-process lock of L1 is a separate file `state.lock` opened without sharing.
  - A program stopped by kaltstellen is restarted on `neutral` from its image path with the `args` and `cwd` of its stop entry (C6). The command line it originally ran with is not read from the process.
  - A suspended record whose resume fails is kept for the next switch or `recover`.
  - Without a tray instance there is nothing to queue: W3 and exit code 3 apply to requests sent to the tray; a second command-line switch during a running one ends with exit code 5.
  - `switch neutral` and `recover` work without a readable configuration.

## 4. Session and lifecycle

- **L1 Single instance** **[assumed]**: one tray instance per user session, enforced with a named mutex. The command line talks to the running instance over a named pipe whose ACL allows only the current user **[r2]**. If none is running, `kaltstellen switch` runs in-process and takes an exclusive lock on the state file for the duration.
- **S5 Sign-out and shutdown** **[decided]**: all suspended programs are resumed first. On the next start the active profile is `neutral` and the tray offers the last active profile for one-click activation without applying it. The name of that last profile is stored in `settings.json`, not in `state.json`.
  - Mechanism **[decided]** (resume at `WM_QUERYENDSESSION`): the tray app owns a hidden top-level window and resumes everything when it receives `WM_QUERYENDSESSION`, not only on `WM_ENDSESSION`. Reason: a frozen program cannot answer `WM_QUERYENDSESSION`, so Windows would wait for it and show a blocking dialog. The tray sets a high shutdown priority (`SetProcessShutdownParameters`) so it is asked early. If the shutdown is then cancelled, the profile is gone and the tray offers it again. The command line uses `SetConsoleCtrlHandler` for the same purpose.
  - Records of stopped programs are cleared at sign-out and shutdown; they are kept across `Quit` and crashes.
- Standby and hibernate: untouched. Frozen processes stay frozen across standby **[decided]**. Wake events are only logged (`WM_POWERBROADCAST`). Risk: network connections, timers and sessions of frozen programs have expired on wake and are only processed after the next resume (R4).
- **L2 Quit** (tray menu): resumes all suspended programs, clears recorded suspended state, leaves stopped programs stopped (their records stay, so a later switch to `neutral` can restart them), then exits.
- **L3 Recover** **[r2]**: `kaltstellen recover` resumes everything recorded in state without needing the tray. Reason: a tray app killed by the user or a crash leaves programs frozen until kaltstellen runs again. The tray app should be started at sign-in so recovery happens automatically.
- **S3 Crash**: on the next start, recovery resumes every recorded suspended process that still matches (S6), clears state, and sets the active profile to `neutral`.

- **L4 Tray details fixed while implementing** **[decided]** (WP7; confirmed by the owner 2026-11-22):
  - Single instance (L1): a mutex `Local\kaltstellen-tray` in the session namespace. With `KALTSTELLEN_DATA_DIR` set, the name gets a hash of that folder, so test or portable instances do not block the real one. A second start prints `kaltstellen tray is already running in this session` and exits with code 0; it forwards nothing yet (the pipe of L1 is still open).
  - Console: the executable stays a console program. Started without a terminal (Explorer, shortcut, Run key), Windows creates a console for it alone; the tray detects that (`GetConsoleProcessList` returns only itself) and releases it with `FreeConsole`, so the window closes again, possibly after showing for a moment. Started from a terminal it stays attached, prints one line and keeps the terminal until Quit. The other subcommands are unchanged.
  - State: the tray does not hold the state lock while idle. Each job (start-up recovery, switch, Quit) takes the lock, reads `state.json` afresh, runs the engine and writes it back. A command-line switch can therefore run while the tray is idle; one that overlaps a tray switch gets exit code 5 (and the tray's switch fails with `KS303` if the command line got the lock first). When the menu opens and no switch runs, the tray reads the active profile from the file, so a command-line switch shows up there.
  - Start-up recovery (S3) runs on the worker as the first job and counts as a switch to `neutral` for W3: a `Neutral` click during it is dropped, any other click waits.
  - Quit (L2) runs `recover` (L3): suspended programs resumed and their records cleared, stopped records kept, active profile `neutral`. If a switch runs, Quit waits for it and drops a waiting request. Exit code 0, or 1 if resuming reported errors (logged). `WM_CLOSE` to the tray window (for example `taskkill` without `/F`) is treated as Quit.
  - Menu: left and right click both open it (no main window yet). Profiles in file order, the active one checked; an active profile that is no longer in the configuration (C4) is shown checked and disabled with `(no longer in the configuration)`; `Neutral`; a disabled status line that shows, in this order, a running switch, the errors of the last switch (count and the first one), or why the configuration file is not in effect; `Open kaltstellen…` disabled until the GUI (M3); `Quit`.
  - Error state (W2): set when the last switch or recovery reported errors or the configuration file is invalid; cleared by the next switch without errors, or when the file is valid again. Acknowledging it without a switch is left to the GUI (M3). Icon changes are logged (`tray: icon neutral|active|error`).
  - Configuration (C3, C4): re-read when the menu opens and when a switch starts, never by a watcher (Q1); a file with unchanged time and size is not parsed again. A missing file means no profiles and is not an error. An invalid file keeps the last valid configuration in effect. A switch takes a copy of the configuration when it starts and keeps it to the end.
  - Warn-list programs (P6) are touched from the tray only if their entry has `confirmed = true`; otherwise the entry fails with `KS113` and the rest of the switch runs.
  - The icons are embedded as resources 101 (neutral), 102 (active), 103 (error) and loaded at the system's small-icon size; the process is not per-monitor DPI aware yet. After an Explorer restart (`TaskbarCreated`) the icon is added again.
  - The tray window is a hidden top-level window (class `kaltstellen-tray`, no taskbar button), not a message-only window, so it can receive `WM_QUERYENDSESSION` in WP8.

- **L5 Pipe details fixed while implementing** **[assumed]** (WP7, second increment). Message format: [pipe-protocol.md](pipe-protocol.md).
  - Name `\\.\pipe\kaltstellen-<user SID>-<session id>`, plus the data-directory hash of L4 when `KALTSTELLEN_DATA_DIR` is set. Pipe names are machine-wide, unlike the `Local\` mutex, hence user and session in the name.
  - Access (L1 "ACL allows only the current user"): protected DACL with a single entry for the user's SID (no SYSTEM, no Administrators), remote clients refused, first instance created with `FILE_FLAG_FIRST_PIPE_INSTANCE` so a name taken by someone else makes the tray run without a pipe (error state, status line) rather than share it. The client checks that the server process runs as the same user and connects at identification level only.
  - With a tray in the session, `switch`, `recover` and `status` go through the pipe; without one they run in-process as before. This replaces the State bullet of L4 for the command line: a command-line switch no longer competes with the tray for the state lock, it waits in the tray's queue. If the tray's mutex exists but its pipe does not answer within 5 s (tray starting, hung), the command ends with `KS501`, exit code 5, and does not fall back, so it cannot race the tray.
  - Output and exit codes through the tray are those of an in-process run (same configuration checks, same lines). Two additions: `waiting for the tray's running switch` on stderr when the request waits, and for `status` the lines `tray: running …` and `tray: waiting …` on stdout.
  - A request whose target equals the running job's target (W3 drop) is not an error: the client gets the running job's result. Recovery counts as a switch to `neutral` (L4), so `recover` and `switch neutral` share each other's result while the other runs. `force` is part of the request and travels with it; it is not compared when deciding about the drop.
  - The waiting request that a later one replaces ends with `KS402`, exit code 3; one replaced by Quit too (`by` is `quit`). Requests arriving while the tray quits are refused with exit code 5.
  - An unknown profile (exit code 4) and an invalid configuration (exit code 2) are refused at once, before queueing, so they never replace a waiting valid request. The tray's own menu keeps using the last valid configuration (C3).
  - `--no-wait` returns after the tray answered `accepted` and prints `started by the tray` or `queued by the tray`; without a tray it has no effect and the command runs in-process to the end.
  - The protocol is versioned (`version: 1`); a mismatch is `KS505`, exit code 5.

- **L6 Interrupting a command-line switch** **[assumed]** (S5 last sentence; M1):
  - Only an in-process switch (no tray running) installs the handler (`SetConsoleCtrlHandler`). Ctrl+C, Ctrl+Break and closing the console window no longer end the process at once. The switch checks for the interrupt before each step and before each program: the action on the current program finishes (a freeze, a close request with its `stop_timeout_s`, a start), later programs and steps are skipped and the error `KS403` is reported. Then kaltstellen runs `recover` (L3): everything recorded as suspended is resumed, also what earlier switches froze, stopped records are kept, the active profile becomes `neutral`. Exit code 1. Reason: a half-applied profile is neither the old nor the new one; going back to `neutral` is the state the user can reason about.
  - Closing the console gives the process about 5 s before Windows ends it; the handler holds it open up to 4.5 s while the switch winds down. A current step that takes longer (a slow close with a long `stop_timeout_s`) can therefore still be cut off by Windows; its suspend records stay in `state.json` for the next `recover` or tray start.
  - `CTRL_LOGOFF_EVENT` and `CTRL_SHUTDOWN_EVENT` are handled the same way, but Windows does not send them to processes that use `user32.dll`, which kaltstellen does; sign-out during a command-line switch is covered only as far as the console's close event arrives.
  - A command handed to the tray (L5) is not interrupted: Ctrl+C ends the waiting command line, the tray finishes the request.

- **L7 Session end details fixed while implementing** **[assumed]** (WP8):
  - At `WM_QUERYENDSESSION` the tray drops a waiting request (its command line gets exit code 3), queues a resume of everything suspended to run right after the running job (or at once), and answers only when it is done, at most after 4 s; then it allows the session to end. A switch still running after 4 s (long stop timeouts) is not waited for. The resume is the same as Quit (L2) but the tray keeps running. A second `WM_QUERYENDSESSION` while one is pending adds nothing.
  - The last profile is written to `settings.json` (configuration directory, key `last_profile`) at `WM_QUERYENDSESSION`: the active profile, or, while `neutral` is active, the profile still on offer. Stopped records are cleared only at `WM_ENDSESSION` with "ending", so a cancelled shutdown keeps them; `WM_ENDSESSION` "cancelled" keeps the profile off and the offer in the menu.
  - Offer: while `neutral` is active and the profile still exists, the menu starts with `Apply last profile: <name>`. Any switch from the menu or the command line uses the offer up and removes it from `settings.json`; start-up recovery and the session-end resume do not.
  - Shutdown priority `SetProcessShutdownParameters(0x3FF, 0)`, the highest level for applications, so the tray is asked before the programs it froze.
  - `WM_POWERBROADCAST`: suspend, resume and power events are logged (`tray: power event …`); nothing is resumed or frozen.
  - Tested by posting these messages to the tray window of a test instance; a real sign-out or shutdown (T7, T20) is still to be run on the VM.

- **L8 Start at sign-in** **[assumed]** (plan WP7, opt-in):
  - Off by default. Switched on with `kaltstellen autostart on` or the tray menu item `Start at sign-in` (checked when on), off with `kaltstellen autostart off` or the same item; `kaltstellen autostart` shows the state.
  - Value `kaltstellen` in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` with the data `"<full path of the running executable>" tray`. Switching on replaces a value that points elsewhere (another copy); the state then reads `on, but for another program` until switched on again from the copy in use. The menu item counts as checked only if the value points at the running executable.
  - Only `HKEY_CURRENT_USER` is written; no elevation. `KALTSTELLEN_RUN_KEY` replaces the key path below `HKEY_CURRENT_USER`; the tests use it with a key of their own and delete that key afterwards.
  - Errors are `KS601`, exit code 1.

- **L9 Acknowledging errors, DPI** **[assumed]** (M2 leftovers):
  - The menu shows `Clear error state` while the last switch reported errors. It clears those errors without a switch; the icon then shows the active profile again. An invalid configuration file or a missing pipe keeps the error state, because the cause is still there; they clear when it is gone.
  - The executable embeds a manifest (`kaltstellen.manifest`): per-monitor DPI awareness v2, common controls version 6 for the main window, `asInvoker`. The console subcommands print exactly the same output with the same exit codes as without it (compared for `--help`, `status`, `check-config`, `list`, `switch`, `autostart`). The tray logs its DPI awareness at start.

- **L10 Hotkeys** **[assumed]** (WP9; U3, C5, C7):
  - The tray registers the `hotkey` of each profile on its window with `MOD_NOREPEAT`. The `WM_HOTKEY` ID is the position of the profile in the file plus one. A pressed hotkey is queued like a menu click (W3).
  - `Ctrl+Alt+<letter or digit>` is never registered (C7); `check-config` warns about it.
  - The hotkeys are brought in line with the configuration at start, whenever the menu opens and after every finished job (each of which re-reads the file, C4). Only changed hotkeys are released and registered again.
  - A hotkey another program holds is logged once per hotkey and profile, listed in the menu as `Hotkey not available: <hotkey> (<profile>)`, and tried again at every later sync; it does not set the error state. The profile stays usable from the menu and the command line.
  - All hotkeys are released at Quit; Windows also releases them when the tray process ends.

- **M6 Main window details fixed while implementing** **[assumed]** (WP10; section 8 "Main window", "Toolkit"):
  - Toolkit: native Win32 controls (buttons, list box, list views, edit, combo box) through the existing `windows` crate, no new GUI dependency. Spike on this desktop, release builds, a window with a list, a table of 20 rows, a text field and buttons, measured from start until the window is visible and 3 s later:
    - native (this window): executable 2.4 MB in total (the whole tool), private working set 1.7 MB, working set 16 MB, visible after 0.42 to 0.50 s;
    - egui/eframe 0.37 with its default `wgpu` backend: executable 14.5 MB, private working set 155 MB, working set 229 MB, visible after 0.03 to 0.35 s;
    - egui/eframe 0.37 with the `glow` backend: executable 6.1 MB, private working set 36 MB, working set 67 MB, visible after 0.04 to 0.23 s.
    Only native meets the budget of 30 MB working set; egui would also add 4 to 12 MB to the single executable. Native also follows the system's look, scaling and keyboard navigation by itself. An embedded web view was not considered (spec: native preferred).
  - `kaltstellen gui` runs as its own process, one per session (mutex like the tray's, with `gui` in the name); a second start prints `the kaltstellen window is already open in this session` and exits with code 0 (it does not bring the open window to the front yet). The tray's `Open kaltstellen…` starts it without a console window.
  - Layout at 96 DPI, scaled to the monitor (per-monitor DPI, L9): profiles with a name field and `New`, `Copy`, `Rename`, `Delete`, `Apply profile` on the left; the entries of the selected profile (program, action, options) with an action list, `Set action`, `Remove`, `Add executable…` on the right; below them the running programs of the session (name, private memory, protection) with `Add to profile` and `Refresh`; the unassigned programs (N1) with `Add to profile` and `Dismiss`; the status line at the bottom. The window has a fixed size.
  - The configuration file stays the source of truth. The window opens only a valid file (an invalid one: the problems of `check-config` on stderr, exit code 2). Edits go through a format-preserving TOML editor (`toml_edit`), so comments and layout stay; every edit is validated with `config::parse` before it is accepted, an invalid result is refused with `KS205` and changes nothing. Each accepted edit is saved at once, atomically (temporary file, then rename). If the file changed on disk since it was read, the save is refused (`KS205`), the window re-reads the file and the edit has to be repeated.
  - Copying a profile copies its entries but not its hotkey (C5). New entries take the action selected in the action list, or `settings.default_action` when none is selected. A program added from the running list gets only `exe`; one added as an executable also gets `path`.
  - Changing an entry's action removes the options the new action does not take (C6); a `start` entry without `path` is refused.
  - Hard-protected programs cannot be added (refused in the status line). For a warn-list program the window asks; yes adds it with `confirmed = true`, no adds nothing. Setting a warn-list entry to `stop` or `suspend` asks the same.
  - Deleting a profile asks first. If it is the active profile, the window switches to `neutral` first and deletes only if that worked (M1).
  - `Apply profile` runs `kaltstellen switch <profile>` as a child process without a console window, so it goes through the tray's pipe when a tray runs and works in-process otherwise; the status line shows the result. Memory shown for running programs is the private working set from the same system query as the process snapshot, so no process is opened for it (A7).

- **N2 Unassigned programs, details** **[assumed]** (WP11; N1):
  - Sources, read only: the values `ExecutablePath` below `HKCU\Control Panel\NotifyIconSettings` (Windows 11), counted only while a process of that name runs in the session, plus processes of the session that own a visible top-level window. Only window handles are queried, no process is opened (A7). Hard-protected programs and kaltstellen itself are left out. A path is kept only when the registry gives an absolute one (entries starting with a known-folder GUID keep the name only).
  - Snapshots: when the main window opens, when its profile selection changes, on `Refresh`, after it saved an edit, after the tray applied a profile (not `neutral`), and on `kaltstellen status --unassigned`. Plain `status` takes none.
  - `seen.json` (data directory): each program with `first_seen`; per profile a fingerprint of its entries and when it last changed; per profile the dismissed programs. Written atomically; an unreadable file is reported (`KS304`) and treated as empty.
  - A program is unassigned in a profile when it runs now, has no entry with its executable name in that profile (any action counts, `ignore` too) and was not dismissed for that profile. It is `new` when it was first seen after the profile last changed (seconds). `neutral` lists nothing.
  - The tray menu shows `Unassigned programs (N)` for the active profile when N > 0; it opens the main window. The icon keeps its three states.

- **UP1 Updates** **[assumed]** (owner request 2026-11-22: an update function before the first release, modelled on the owner's project foresprint; its Go package `selfupdate` was the reference):
  - **Non-goal changed.** requirements.md said "no network access". Now: the only network access is the update check and download described here; it can be switched off and sends nothing about the user. A check is one HTTPS `GET` to the GitHub API with the user agent `kaltstellen/<version>`; GitHub sees the IP address and the time, nothing else is sent (no identifier, no configuration, no usage data). Telemetry stays a non-goal.
  - **Source.** GitHub Releases of the public repository `swallat/kaltstellen`: `GET https://api.github.com/repos/swallat/kaltstellen/releases/latest`, which leaves out drafts and pre-releases. A running pre-release (`1.0.0-beta.2`) instead reads `/releases?per_page=30` and takes the highest version that is not a draft, so a beta sees the next beta (as foresprint). The tag is `v<version>`. `KALTSTELLEN_UPDATE_URL` replaces `https://api.github.com/repos/swallat/kaltstellen` (tests, a mirror); plain `http://` is accepted only for loopback hosts.
  - **Release files** (what the client looks for, see [release-requirements.md](release-requirements.md)): `kaltstellen-<version>-windows-<arch>.exe` (`x86_64`, `aarch64`), the bare executable, and `checksums.txt` with lines `<sha256 hex>  <file name>` (`sha256sum` format, as foresprint). Other files are ignored, so a release can add a zip for people, signatures or notes without changing the client.
  - **Versions.** `MAJOR.MINOR.PATCH` with an optional pre-release after `-`, build metadata after `+` ignored, a leading `v` allowed. Numbers compare as numbers; a pre-release is lower than its release; pre-release parts compare dot by dot, numbers below words (as foresprint). A **development build** (compiled without `KALTSTELLEN_RELEASE=1`, the release workflow sets it) never checks by itself and refuses to install, unless `KALTSTELLEN_UPDATE_URL` names a source; `update --check` works. Reason: a local `cargo build` must not replace itself with a release.
  - **Opt-out, two modes**: `on` (default) checks once a day and reports; `off` never connects. Installing is always a deliberate command or click, never automatic, also in mode `on`, because installing quits the tray, which resumes every frozen program (as foresprint: its automatic check only reports). Stored in `settings.json` (configuration directory, key `update`); set with `kaltstellen update --auto on|off` or the tray item `Check for updates daily`. The environment variable `KALTSTELLEN_UPDATE` (`on`, `off`) wins over the file (as `FORESPRINT_AUTOUPDATE`), for example for a managed installation.
  - **Automatic trigger.** Only the tray checks automatically: 5 minutes after it starts if the last check is 24 hours or more ago, then 24 hours after each check. It sets one one-shot Win32 timer for the due time (no polling, no thread waiting, Q1). The check runs as a child process `kaltstellen update --check --quiet` without a console window: no network code runs inside the tray, so neither its memory nor a hang or crash in it reaches the process that holds the frozen programs. A failed check (offline) counts as a check and is tried again 24 hours later. Switching `Check for updates daily` on checks at once. Unlike foresprint, command-line commands never check by themselves: they are run by scripts and hotkeys, where a delay or a line on stderr does not belong.
  - **What is remembered.** `update.json` in the data directory: time of the last check, the newest version found and its release page, the error of a failed check. Written atomically by the checking process; the tray reads it when its menu opens and when the check child ends. A missing or damaged file means "never checked" (`KS807` in the log).
  - **Tray menu.** `Check for updates daily` (checked when on). When the last check found a newer version: `Install update <version>…`, which opens a console window running `kaltstellen update` so progress and errors are visible; it waits for Enter at the end if something failed. When this copy cannot update itself (see "Where"): a disabled line `Update <version> available: <how>` instead. Icon and tooltip do not change for an update.
  - **Command line.** `kaltstellen update --check`: checks now (also in mode `off`, it is explicit), prints the result, exit code 0, or 1 with `KS801`/`KS802` on failure. `kaltstellen update`: installs the newest version if it is newer (exit 0 also when there is nothing to do). `--force`: installs the newest version even if it is not newer (repair, tests). `--auto on|off`: sets the mode.
  - **Where.** The file replaced is the running executable (`current_exe`, links resolved). Installing is refused with `KS804`, before anything is downloaded, when the path lies below `\scoop\apps\`, `\WinGet\` or `\chocolatey\` (the message names `scoop update kaltstellen`, `winget upgrade` or `choco upgrade`), below `%ProgramFiles%` or `%ProgramFiles(x86)%` (needs administrator; kaltstellen does not elevate), or when a probe file cannot be created in the folder. The check still runs and reports there.
  - **Integrity and authenticity.** `checksums.txt` and the executable are downloaded over HTTPS (WinHTTP, Schannel, the system's certificate store and proxy settings, redirects followed except HTTPS to HTTP, at most 64 KiB and 64 MiB, 30 s timeouts per step). The SHA-256 of the executable (CNG `BCryptHash`) must equal its line in `checksums.txt`; a release without `checksums.txt` or without a line for the file is refused (`KS802`), a different sum is `KS803`; in both cases nothing is changed. This protects against a broken or truncated download and a replaced file on the way, not against someone who can publish releases in the repository (stolen account or token), because the sum comes from the same release. Signing would close that gap and needs the owner: either an Authenticode code-signing certificate (yearly cost; the client would check the signature and the publisher with `WinVerifyTrust`, and SmartScreen would stop warning), or a key pair of the owner's whose public half is compiled into kaltstellen and whose private half signs `checksums.txt` in the release workflow (an Actions secret; the client would check `checksums.txt.sig` with CNG, ECDSA P-256). Not implemented; a version that adds it will require the signature from then on. foresprint publishes a cosign bundle but its client does not check it either.
  - **Install steps.**
    1. Clean up what an earlier update left (`<name>.new.exe`, `<name>.failed.exe`), refuse per "Where", then download and verify as above.
    2. Write the executable as `<name>.new.exe` next to the running one and start it with `--version` (10 s). Its output must be `kaltstellen <version of the release>`; otherwise `KS806`, the staged file is removed and nothing else has changed.
    3. Nothing stays frozen. If a tray runs in this session, it is asked over the pipe to quit (new command `quit`, Quit semantics of L2: everything suspended is resumed and its records cleared, stopped records kept). Before that the active profile is written to `settings.json` as the S5 offer, so the new tray offers `Apply last profile: <name>` and applies nothing by itself. The update waits until the tray's mutex is gone (30 s). A tray that refuses (`quit` is unknown to it, it is quitting already, it does not end in time) stops the update with `KS808`; nothing has changed. Without a tray, `update` runs `recover` (L3) in-process when anything is recorded as frozen and prints how to apply the profile again.
    4. Swap: rename the running executable to `<name>.old.exe` (replacing an older one), then `<name>.new.exe` to the original name. Windows allows renaming a running executable, so tray and main-window processes may still run from it. If the second rename fails, the first is undone (`KS805`).
    5. If a tray was running, the new executable is started with `tray` (no console, same environment) and must answer `status` on its pipe within 15 s. If it ends or does not answer, it is ended, the new file is renamed to `<name>.failed.exe`, `<name>.old.exe` gets the original name back, the old tray is started again, and the update ends with `KS806`, exit code 1. Without a tray, the `--version` start of step 2 is the start check.
    6. `<name>.old.exe` is kept until the next update, as a manual way back (rename it); a main window still open runs from it until it is closed. Autostart (L8) keeps working because the original name stays.
  - **Pipe.** Protocol version 1 gains the request `quit` (reply `done` after Quit finished, then the tray ends); it is additive, so a tray that does not know it answers `KS505` (step 3 then stops with `KS808`).
  - **Errors**: `KS801` check failed (network, HTTP status), `KS802` release unusable (metadata, no file for this system, no checksum), `KS803` checksum mismatch, `KS804` this copy cannot update itself (package manager, folder, development build), `KS805` replacing the executable failed (undone), `KS806` the new version did not start (rolled back), `KS807` `update.json` unreadable or not writable, `KS808` the tray could not be stopped for the update. All exit code 1.
  - Not covered: the main window shows nothing about updates; a real update from a real GitHub release cannot be tried until there is one (see [manual-checks.md](manual-checks.md)).

## 5. Configuration (TOML)

Location `%APPDATA%\kaltstellen\config.toml`. The repository ships only `config.example.toml` with generic names.

```toml
[settings]
default_action = "suspend"
trim = true                   # EmptyWorkingSet after suspend
stop_timeout_s = 8
minimize_before_suspend = true

[[profile]]
name = "focus"
hotkey = "Ctrl+Shift+Alt+F1"

  [[profile.app]]
  exe = "browser.exe"
  path = ""                   # optional
  action = "suspend"          # start | stop | suspend | ignore

  [[profile.app]]
  exe = "tool.exe"
  action = "start"
  args = ["--minimized"]
  cwd = "C:\\tools"

  [[profile.app]]
  exe = "chat.exe"
  action = "stop"
  on_timeout = "kill"         # kill | keep
  restart_on_neutral = true
  args = ["--minimized"]      # optional, with cwd: used for the restart

  [[profile.app]]
  exe = "remote.exe"
  confirmed = true            # warn-list program, touched deliberately
```

- **C1** Profile names are unique and non-empty; `neutral` is reserved.
- **C2** An `exe` appears at most once per profile, unless every entry of that name has a `path` and the paths differ (A7) **[decided]**; it may appear in several profiles; the active profile alone decides its action.
- **C3** An invalid file is rejected as a whole with file, line and reason. The previous valid configuration stays in effect and the tray shows the error state. Unknown keys are errors, so typos are caught.
- **C4** The file is re-read on change. A running switch finishes with the old configuration. If the active profile was removed or renamed, it stays active under its old name until the next switch.
- **C5** Hotkeys must be unique. A hotkey that cannot be registered (taken by another program) is reported once at start and ignored; the profile stays usable from tray and command line.
- **C6** `args` and `cwd` for `start`, and for `stop` together with `restart_on_neutral = true`, where they are used for the restart on `neutral` **[decided]**; `restart_on_neutral` and `on_timeout` only for `stop`; `trim` only for `suspend`; `confirmed` only for `stop` and `suspend`. Other combinations are errors. An entry with `action = "start"` requires `path` **[r2]** (a name alone cannot be launched).
- **C7 Hotkey syntax** **[r2]**: modifiers plus one key. `Ctrl+Alt` combined with a printable key is rejected with a warning, because on layouts with AltGr (such as German) `Ctrl+Alt+key` produces characters like `{`, `[`, `@`, `€` and registering it would steal them. Function keys with `Ctrl+Shift+Alt` are recommended. `RegisterHotKey` uses `MOD_NOREPEAT`. Hotkeys do not fire on the secure desktop (UAC, lock screen).
- **C8 Details fixed while implementing** **[decided]** (WP3; confirmed by the owner 2026-11-22):
  - `action` may be omitted in an entry; it then takes `settings.default_action`, which cannot be `start`.
  - `exe` is a file name without folder; a name without extension gets `.exe`. If `path` is set it must be absolute and end in that file name. An empty `path` or `cwd` counts as not set.
  - C2 with A7: two entries of one profile may share an `exe` only if both have a `path` and the paths differ.
  - `cwd` is valid only for `start`, the per-entry `trim` (R5) only for `suspend`.
  - Profile names and hotkeys compare case-insensitively; names are trimmed.
  - Hotkey keys: `A` to `Z`, `0` to `9`, `F1` to `F24`, `Space`, `Tab`, `Enter`, `Esc`, `Insert`, `Delete`, `Home`, `End`, `PageUp`, `PageDown` and the arrow keys; modifiers `Ctrl`, `Alt`, `Shift`, `Win`. A function key may stand alone, every other key needs a modifier. Malformed hotkeys are errors (C3); only the AltGr case of C7 is a warning.
  - All rule violations of a file are reported together, each with its line.

## 6. State file

`%LOCALAPPDATA%\kaltstellen\state.json`, written atomically (temp file + rename) after every step.

- Content: active profile, system boot time **[r2]**, and per affected process: PID, process creation time, full image path, action (`suspended` | `stopped`), and for stopped ones the start command and `restart_on_neutral`.
- **S10** **[r2]** Recovery first compares the boot time; after a reboot the suspended records are dropped without resuming anything. Before resuming a matching record it checks that the image path equals the recorded one. During a switch the open process handle is kept until the step finishes, which also rules out PID reuse within a step.
- **S6** A process matches a state entry only if PID and creation time both match; a reused PID is ignored.
- **S9** A corrupt file is renamed to `state.json.bad` and treated as empty.

## 7. Protection, errors, rights

- **P6 Two tiers** **[decided]**:
  - *Hard-protected*: core Windows processes, the shell, console hosts (`conhost`, `OpenConsole`), error reporting and the desktop window manager **[r2]**. Never touched, whatever the config says (A6). Not selectable in the GUI; rejected by the command line.
  - *Warn list*: remote-access tools, graphics drivers, licence services, security software, developer tools. Allowed only after explicit confirmation: `confirmed = true` in the entry **[decided]**, which the GUI sets after asking, or `--force` on the command line for one switch. The list is data shipped with the tool. (User-extendable lists are post-v1 **[decided]**.)
- **P7** Processes of higher integrity (elevated) cannot be frozen or terminated from a non-elevated kaltstellen because of mandatory integrity control; `OpenProcess` for suspend/terminate fails with access denied **[r2]**. User Interface Privilege Isolation additionally blocks window messages, so the graceful close (`WM_CLOSE`) and the minimise step (W7) also fail for elevated windows. Protected processes (PPL) cannot be opened even by administrators. All of these are reported as "needs administrator" or "protected" and skipped. Not fatal (W2); never silent (Q3). `start` of a program that requires elevation fails with `ERROR_ELEVATION_REQUIRED` and is reported the same way; kaltstellen does not trigger UAC prompts.
- **P9** **[r2]** Processes started by kaltstellen are not placed in a job object and survive Quit of the tray. They do not inherit kaltstellen's handles. A future elevated tray would hand its elevation to every started program, which is one more reason for the helper design (P8).
- **P8** Services and an elevated helper are deferred. Decision: a small elevated helper, so the tray app never runs elevated. **[decided]**
- **E1 Errors**: each error has a stable code, program, step and reason. Errors are logged to `%LOCALAPPDATA%\kaltstellen\kaltstellen.log` (rotated at 1 MB, three files). The log stays local.
- **E2 Exit codes** **[decided]**: `0` success, `1` finished with collected errors, `2` configuration invalid, `3` request superseded by a later request (W3), `4` unknown profile, `5` tray not reachable and in-process lock held by another switch **[r2]**. The command line waits for completion by default; `--no-wait` returns after queuing.

## 8. Profile management and GUI

- **M1** Profiles are created, copied, renamed and deleted in the GUI or in the file. Deleting the active profile first switches to `neutral`.
- **M2** Add a program by choosing from running programs (name, memory, protection status) or by choosing an executable file. Drag and drop is post-v1 **[decided]** (blocked from non-elevated Explorer to elevated windows anyway).
- **M3** Each entry has a per-program action drop-down; `default_action` pre-fills new entries.
- **M4** Hard-protected programs cannot be added; warn-list programs ask for confirmation (P6).
- **M5** *(post-v1 [decided])* per-entry failure markers; version 1 shows errors only in the status line.
- **N1 Unassigned programs** **[decided]** (owner request, 2026-11-21): programs that appear while kaltstellen runs and have no entry in a profile are shown in that profile's settings as *unassigned*, so the owner can see which programs arrived since the profile was last changed and have not been considered deliberately.
  - An entry with any action, including `ignore`, counts as a deliberate decision; only programs without an entry are unassigned.
  - Detection is **lazy, not a watcher** (consistent with W6 and Q1): kaltstellen snapshots running programs when the main window opens, when a profile is applied, and on `kaltstellen status`. Each snapshot compares against the profile entries and the persisted `seen` list in `seen.json` (`exe`, optional path, `first_seen`).
  - A program first seen after the last change of that profile is flagged **new**; older unassigned programs are shown as plain *unassigned*. The marker disappears as soon as the program gets an entry. A per-program "dismiss" removes it from the list without adding an entry and records it in `seen.json` **[assumed]**.
  - Scope of "program" for N1 **[decided]**: programs with a notification-area icon, found through the per-user icon registry (`HKCU\Control Panel\NotifyIconSettings`, Windows 11) plus a fallback of processes with a visible top-level window. Hard-protected programs are never listed. Per-profile; `neutral` has no entries and shows nothing.
  - The tray menu shows `Unassigned programs (N)` only when N > 0. The tray icon keeps its three decided states; there is no extra icon state.
  - `kaltstellen status --unassigned` lists them for scripts.
- **Tray**: profile list (active checked), `Neutral`, `Open kaltstellen…`, `Quit`. Icon states: blue neutral, orange profile active, red with exclamation mark on error. The error state clears on the next successful switch or when acknowledged.
- **Main window** **[assumed]**: runs as a separate process (`kaltstellen gui`, same executable) started by the tray and ended when closed **[decided]**. Reason: Q1 stays true for the tray regardless of the GUI toolkit. It reads and writes the config file and asks the tray to apply a profile over the pipe. Left: profile list; right: entry table (program, action, options); bottom: `Add program…`, `Apply profile`; status line with last result and collected errors.
- **Toolkit** is not fixed here. Constraints: single executable, no runtime, small idle footprint. A native Win32 approach is preferred over an embedded web view. **[assumed]**

## 9. Quality targets (measurable)

- **Q1** Tray app idle: private working set below 15 MB and CPU below 0.1 % averaged over 60 s, GUI process not running. **[decided]** (thresholds)
- **Q2** One executable, no installer, no runtime.
- **Q3** Runs without administrator rights; operations needing them are reported (P7).
- **Q4** All `unsafe` Win32 code lives in one module. Widened in WP7 **[decided]** (2026-11-22): `unsafe` is allowed in `src/process.rs` (process APIs) and in the `src/win32/` tree (tray window, notification icon, menu, single-instance mutex, console), nowhere else. Each of these modules wraps one concern behind a safe interface; all decisions (queue, menu content, icon state, what a click does) are safe code outside them. Enforced by `#![deny(unsafe_code)]` with `#[allow]` on exactly these two modules, and by `scripts/check-unsafe.sh` in CI. Reason: the tray needs about a dozen more Win32 calls, and the safe tray crates bring a second windowing layer for what the existing `windows` dependency already covers (Q1, Q2).
- **Q5** A switch of 20 programs completes in under 10 s excluding stop timeouts and the minimise wait (W7). **[decided]** Treated as a target, not an acceptance test.

## 10. Acceptance tests (test host VM; real load stays on the desktop)

| ID | Check |
| --- | --- |
| T1 | Order W1 holds: log shows resume, stop, suspend, start, in that order. |
| T2 | A program suspended by both prev and target keeps its freeze, no resume event (W1). |
| T3 | Failure of one stop leaves the switch finishing, icon red, exit code 1 (W2, E2). |
| T4 | Two requests during a switch: only the last runs afterwards (W3). |
| T5 | Kill the tool while programs are frozen; restart; nothing stays frozen, active profile is `neutral` (S3). |
| T6 | Stale PID in state is ignored (S6); corrupt state is renamed (S9). |
| T7 | Sign-out and shutdown resume everything; next start offers the last profile without applying it (S5). |
| T8 | Invalid config keeps the old one active, names file and line (C3). |
| T9 | Hard-protected program is refused in config and in the command line (P6). |
| T10 | `restart_on_neutral` restarts only on `neutral` (P5). |
| T11 | `on_timeout = "keep"` leaves a program with an open dialog running and reports it (A2). |
| T12 | Idle footprint meets Q1 over 60 s. |
| T14 | **[r2]** With windowed programs frozen, the shell (taskbar, Explorer) stays responsive. |
| T15 | **[r2]** Programs not in an entry are never opened (verify with a handle/ETW trace). |
| T16 | **[r2]** A `switch` started from a child of a program marked `suspend` does not freeze itself or its ancestors. |
| T17 | **[r2]** Hotkey already taken, or `Ctrl+Alt+<printable>`: warned and ignored, tray and CLI still work (C5, C7). |
| T18 | **[r2]** Elevated target process: reported "needs administrator", switch continues (P7). |
| T19 | **[r2]** Trim effect measured as change in *available memory*, not working set; resume latency recorded. |
| T20 | **[r2]** Shutdown with frozen programs does not block on a "not responding" dialog (S5). |
| T21 | **[r2]** With Defender enabled, the tool is not quarantined and its calls are not blocked. |
| T22 | N1: start a new tray program after applying a profile; open the window; it is listed as new and unassigned; adding an entry removes the marker. |
| T13 | Long freeze: each target program frozen for 10 minutes or more, then checked by hand (network, sync, audio, licence). Not coverable in the VM; run on the desktop. |

## 11. Milestones (replaces plan.md M1 to M4)

- M1: config, `switch <profile>`, start/stop/suspend with process trees, state file (P1-P4, A1-A4, A6, A7, W1-W6, C1-C6, S6, S8, S9, E2).
- M2: tray app, single instance, resume on quit and sign-out, crash recovery, minimise before suspend (U1, L1, L2, S3, S5, S4).
- M3: main window with program assignment and per-program action; hotkeys (M1-M5, U3, E1).
- M4: automatic triggers with conflict rules (U4); services and elevation helper (A5, P8).

## 12. Confirmation status

All eight points of the round 2 review were confirmed by the owner on 2026-11-21 and are marked **[decided]**. The remaining **[assumed]** items (not raised in round 2) still await confirmation. On 2026-11-22 the owner confirmed the implementation details A8, C8 and L4, the widened Q4, the C2 exception for entries with paths, restart arguments on stop entries (C6) and `confirmed` for warn-list entries (P6).

The ones with most impact among the remaining **[assumed]** items: the resume-except-still-suspended optimisation (W1), re-applying the active profile (W4), the single-instance and pipe design (L1), the config file as source of truth, and the GUI toolkit and window layout.

## 13. Known limitations and risks (r2)

- **R1 Frozen programs still hold resources.** File locks, named mutexes, exclusive-mode audio devices and GPU resources stay held. A frozen audio program holding an exclusive device blocks the next profile's audio program. Such programs need `stop`.
- **R2 Programs that others wait on.** A frozen process that owns a window, a COM or RPC server, or the clipboard (delayed rendering) can hang other programs that call into it without a timeout. Test T14 covers the shell; other cases are documented, not prevented.
- **R3 Audio programs.** Freezing a program with a running audio stream can leave a buffer repeating or glitching. Stop the stream or use `stop`.
- **R4 Long freezes.** Network connections, tokens, licence checks and timers may expire while frozen (open question 2 stays; test T13).
- **R5 Trim.** `EmptyWorkingSet` moves pages to the standby or modified list; they are not freed. Modified pages are written to the pagefile, so a large trim causes a disk burst and a slow first resume. The measure of success is available memory (T19). Trim stays on by default **[decided]**; a per-entry `trim = false` override is allowed **[r2]**.
- **R6 Undocumented API.** `NtSuspendProcess`/`NtResumeProcess` are undocumented but long-stable (used by well-known system tools). Per-thread `SuspendThread` is documented but racy against thread creation. Suspend counts stack with other tools; kaltstellen only undoes its own.
- **R7 Security software.** Suspending and terminating other processes resembles behaviour that endpoint protection watches. Sign the executable and test with Defender enabled (T21).
- **R8 Trust in the config file.** `start` entries launch arbitrary executables; the config is a trusted local file, never fetched from the network.
