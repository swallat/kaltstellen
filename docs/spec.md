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

- **L5 Pipe details fixed while implementing** **[decided]** (owner, 2026-11-23) (WP7, second increment). Message format: [pipe-protocol.md](pipe-protocol.md).
  - Name `\\.\pipe\kaltstellen-<user SID>-<session id>`, plus the data-directory hash of L4 when `KALTSTELLEN_DATA_DIR` is set. Pipe names are machine-wide, unlike the `Local\` mutex, hence user and session in the name.
  - Access (L1 "ACL allows only the current user"): protected DACL with a single entry for the user's SID (no SYSTEM, no Administrators), remote clients refused, first instance created with `FILE_FLAG_FIRST_PIPE_INSTANCE` so a name taken by someone else makes the tray run without a pipe (error state, status line) rather than share it. The client checks that the server process runs as the same user and connects at identification level only.
  - With a tray in the session, `switch`, `recover` and `status` go through the pipe; without one they run in-process as before. This replaces the State bullet of L4 for the command line: a command-line switch no longer competes with the tray for the state lock, it waits in the tray's queue. If the tray's mutex exists but its pipe does not answer within 5 s (tray starting, hung), the command ends with `KS501`, exit code 5, and does not fall back, so it cannot race the tray.
  - Output and exit codes through the tray are those of an in-process run (same configuration checks, same lines). Two additions: `waiting for the tray's running switch` on stderr when the request waits, and for `status` the lines `tray: running …` and `tray: waiting …` on stdout.
  - A request whose target equals the running job's target (W3 drop) is not an error: the client gets the running job's result. Recovery counts as a switch to `neutral` (L4), so `recover` and `switch neutral` share each other's result while the other runs. `force` is part of the request and travels with it; it is not compared when deciding about the drop.
  - The waiting request that a later one replaces ends with `KS402`, exit code 3; one replaced by Quit too (`by` is `quit`). Requests arriving while the tray quits are refused with exit code 5.
  - An unknown profile (exit code 4) and an invalid configuration (exit code 2) are refused at once, before queueing, so they never replace a waiting valid request. The tray's own menu keeps using the last valid configuration (C3).
  - `--no-wait` returns after the tray answered `accepted` and prints `started by the tray` or `queued by the tray`; without a tray it has no effect and the command runs in-process to the end.
  - The protocol is versioned (`version: 1`); a mismatch is `KS505`, exit code 5.

- **L11 Self-protection over the pipe** (T16 fix after the first VM run; A7, A8):
  - A switch requested over the pipe protects the requesting command line and its ancestors exactly as an in-process switch protects kaltstellen and its ancestors, for that job only. The tray takes the client's PID from the pipe (`GetNamedPipeClientProcessId`), never from the message, and its creation time from the handle-free snapshot when the request arrives; a process with that PID but another creation time protects nothing (S6). Every snapshot of the job (each step and the second suspend pass) applies the protection; a caller that has exited by then protects nothing any more.
  - A waiting request that runs later protects its own caller. Menu and hotkey switches have no caller and are unchanged. `recover` and `neutral` freeze nothing and need no protection.
  - **[assumed]** A request that shares the result of an equal running switch (W3 drop) adds its caller to that running switch's protected set; it takes effect from the switch's next snapshot on. Processes the switch froze before that are not thawed. In practice a caller that can still send a request is not frozen itself, but its ancestors may already be if the running switch had passed its suspend step; the caller then waits for the result as usual.
  - **[assumed]** A `--no-wait` caller is protected the same way while it still runs; once it has exited (right after `accepted`), it and its former ancestors are ordinary processes for the rest of that switch.
- **L6 Interrupting a command-line switch** **[decided]** (owner, 2026-11-23) (S5 last sentence; M1):
  - Only an in-process switch (no tray running) installs the handler (`SetConsoleCtrlHandler`). Ctrl+C, Ctrl+Break and closing the console window no longer end the process at once. The switch checks for the interrupt before each step and before each program: the action on the current program finishes (a freeze, a close request with its `stop_timeout_s`, a start), later programs and steps are skipped and the error `KS403` is reported. Then kaltstellen runs `recover` (L3): everything recorded as suspended is resumed, also what earlier switches froze, stopped records are kept, the active profile becomes `neutral`. Exit code 1. Reason: a half-applied profile is neither the old nor the new one; going back to `neutral` is the state the user can reason about.
  - Closing the console gives the process about 5 s before Windows ends it; the handler holds it open up to 4.5 s while the switch winds down. A current step that takes longer (a slow close with a long `stop_timeout_s`) can therefore still be cut off by Windows; its suspend records stay in `state.json` for the next `recover` or tray start.
  - `CTRL_LOGOFF_EVENT` and `CTRL_SHUTDOWN_EVENT` are handled the same way, but Windows does not send them to processes that use `user32.dll`, which kaltstellen does; sign-out during a command-line switch is covered only as far as the console's close event arrives.
  - A command handed to the tray (L5) is not interrupted: Ctrl+C ends the waiting command line, the tray finishes the request.

- **L7 Session end details fixed while implementing** **[decided]** (owner, 2026-11-23) (WP8), except the 4 s wait, which stays **[assumed]** until VM test T20 has measured it:
  - At `WM_QUERYENDSESSION` the tray drops a waiting request (its command line gets exit code 3), queues a resume of everything suspended to run right after the running job (or at once), and answers only when it is done, at most after 4 s; then it allows the session to end. A switch still running after 4 s (long stop timeouts) is not waited for. The resume is the same as Quit (L2) but the tray keeps running. A second `WM_QUERYENDSESSION` while one is pending adds nothing.
  - The last profile is written to `settings.json` (configuration directory, key `last_profile`) at `WM_QUERYENDSESSION`: the active profile, or, while `neutral` is active, the profile still on offer. Stopped records are cleared only at `WM_ENDSESSION` with "ending", so a cancelled shutdown keeps them; `WM_ENDSESSION` "cancelled" keeps the profile off and the offer in the menu.
  - Offer: while `neutral` is active and the profile still exists, the menu starts with `Apply last profile: <name>`. Any switch from the menu or the command line uses the offer up and removes it from `settings.json`; start-up recovery and the session-end resume do not.
  - Shutdown priority `SetProcessShutdownParameters(0x3FF, 0)`, the highest level for applications, so the tray is asked before the programs it froze.
  - `WM_POWERBROADCAST`: suspend, resume and power events are logged (`tray: power event …`); nothing is resumed or frozen.
  - Tested by posting these messages to the tray window of a test instance; a real sign-out or shutdown (T7, T20) is still to be run on the VM.

- **L8 Start at sign-in** **[decided]** (owner, 2026-11-23; plan WP7):
  - Never written without consent. On its very first start the tray asks once whether it should start with Windows (Yes / No) and records in `settings.json` (`autostart_asked`) that it asked; it never asks again, also not after No. If start at sign-in is already on, it does not ask. `kaltstellen autostart on|off` and the menu item `Start at sign-in` stay. **[assumed]**: the question is a message box on a thread of its own, so start-up recovery, the pipe and the menu do not wait for it; `KALTSTELLEN_AUTOSTART_PROMPT` = `yes` or `no` answers it without a dialog and `skip` does not ask (tests, managed installations).
  - Value `kaltstellen` in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` with the data `"<full path of the running executable>" tray`. Switching on replaces a value that points elsewhere (another copy); the state then reads `on, but for another program` until switched on again from the copy in use. The menu item counts as checked only if the value points at the running executable.
  - Only `HKEY_CURRENT_USER` is written; no elevation. `KALTSTELLEN_RUN_KEY` replaces the key path below `HKEY_CURRENT_USER`; the tests use it with a key of their own and delete that key afterwards.
  - Errors are `KS601`, exit code 1.

- **L9 Error state, DPI** (M2):
  - Error state **[decided]** (owner, 2026-11-23): there is no manual acknowledging. The errors of the last switch stay in the menu and keep the icon red until the next switch, which replaces them with its own result (clean: neutral or active; errors: red with the new list). Opening or closing the menu clears nothing. An error whose cause the owner removed by hand resolves itself: it drops out of the list, and when none is left the icon shows the active profile again without a switch. An invalid configuration file or a missing pipe keeps the error state while the cause exists. All errors stay in the log.
  - How resolution is noticed **[assumed]**: no permanent watcher (Q1, W6). The tray checks again when its menu opens and with a repeating 30 s timer that runs only while at least one error could still resolve; the timer stops when none is left. The check uses the handle-free process snapshot and opens no process (A7).
  - Resolution rules per error **[assumed]**: `KS110` (start failed) and `KS111` (start needs elevation) resolve when a process of that program runs; `KS108` (did not close, `on_timeout = "keep"`), `KS109` (terminate failed) and `KS105` (suspend failed) resolve when no process of that program runs any more; `KS102` (needs administrator) and `KS103` (protected) in the stop or suspend step resolve when the program no longer runs. Everything else (state or log I/O, warn list, resume failures, snapshot) stays until the next switch. Programs are matched by executable name in the session.
  - The executable embeds a manifest (`kaltstellen.manifest`): per-monitor DPI awareness v2, common controls version 6 for the main window, `asInvoker`. The console subcommands print exactly the same output with the same exit codes as without it (compared for `--help`, `status`, `check-config`, `list`, `switch`, `autostart`). The tray logs its DPI awareness at start.

- **L10 Hotkeys** **[decided]** (owner, 2026-11-23) (WP9; U3, C5, C7):
  - The tray registers the `hotkey` of each profile on its window with `MOD_NOREPEAT`. The `WM_HOTKEY` ID is the position of the profile in the file plus one. A pressed hotkey is queued like a menu click (W3).
  - `Ctrl+Alt+<letter or digit>` is never registered (C7); `check-config` warns about it.
  - The hotkeys are brought in line with the configuration at start, whenever the menu opens and after every finished job (each of which re-reads the file, C4). Only changed hotkeys are released and registered again.
  - A hotkey another program holds is logged once per hotkey and profile, listed in the menu as `Hotkey not available: <hotkey> (<profile>)`, and tried again at every later sync; it does not set the error state. The profile stays usable from the menu and the command line.
  - All hotkeys are released at Quit; Windows also releases them when the tray process ends.

- **M6 Main window** (WP10; section 8 "Main window", "Toolkit"):
  - Toolkit **[decided]** (owner, 2026-11-23): Microsoft WebView2 directly, using the Evergreen runtime that is part of Windows 11; not Tauri, not Electron. The page is plain HTML, CSS and JavaScript (ES modules, no framework, no build step, no network or CDN resource) in `ui/`, embedded into the executable at compile time, so `cargo build` needs nothing but Rust. The native Win32 window of the first version is removed, not kept as a fallback. The owner accepts that the open window exceeds the 30 MB budget; the tray must stay as small as it is (Q1 is about the tray, the window is a separate process).
  - Measured **[assumed]** (indicative, release build, this desktop, test configuration): the page is ready about 0.54 s after start; with the window open, the window process and the six WebView2 processes it starts (counted as all processes below the window process) use about 74 to 78 MB private working set in total (working set about 345 to 360 MB, which counts shared pages more than once). All of them end when the window closes. The executable grows from 2.54 MB to 2.97 MB (WebView2 loader linked statically, bindings, page files; the native window code is gone). Spike figures of the first version for comparison: native Win32 16 MB working set, egui 67 to 229 MB.
  - Runtime **[decided]**: if no WebView2 runtime is found (`GetAvailableCoreWebView2BrowserVersionString`; WebView2's own `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` is honoured), `kaltstellen gui` prints why, points to the configuration file, `check-config`, `switch` and the tray menu, and exits with code 1 and `KS702`. The loader is linked statically (`WebView2LoaderStatic.lib`), so no `WebView2Loader.dll` ships beside the executable (Q2).
  - Hosting and lockdown **[assumed]**: the page is served from the never-resolving origin `https://ui.kaltstellen.invalid/` through a web-resource handler that answers every request: embedded files with a strict Content-Security-Policy (`default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`), everything else with 403, so the page has no network. Navigation away from the origin is cancelled, new windows are refused, permission requests are denied, default script dialogs and the status bar are off; in release builds dev tools, the context menu and browser accelerator keys are off as well. Messages are accepted only from the origin. The WebView2 user data folder is `webview2` in the data directory (in tests the test data directory), never next to the executable.
  - Bridge **[assumed]**: a versioned JSON message set, documented in [gui-bridge.md](gui-bridge.md). The page asks for state and sends intents; the Rust side (`src/gui/bridge.rs`) treats every message as untrusted (size limit, version, strict fields, text limits, absolute paths), repeats every check (hard-protected refused, warn list needs `confirmed`, C1 to C8 through `config::parse`) and answers with the complete state. The page never gets file or process access; the file dialog, the process list, the unassigned list, autostart and applying a profile are done by the Rust host.
  - Behaviour kept from the first version **[decided]** (owner, 2026-11-23), except the second window (see below): one window per session; started without a console by the tray's `Open kaltstellen…`; opens valid files only (exit code 2 otherwise); every accepted edit is saved at once through the format-preserving editor (`toml_edit`), validated and atomic; a file changed on disk meanwhile wins and the edit is refused (`KS205`); copying a profile drops its hotkey; deleting the active profile switches to `neutral` first; `Apply profile` runs `kaltstellen switch` as a child process without a console, so it goes through the tray's pipe when one runs.
  - Second window **[decided]** (owner, 2026-11-23): a second `kaltstellen gui`, or `Open kaltstellen…` while the window is open, brings the open window to the front, restored if it was minimised, and exits with code 0. **[assumed]**: the window carries a window property with a number derived from its instance (the per-session mutex name, which includes the data-directory hash in tests), so a test window and the real one are never confused; Windows may refuse the foreground to a process that has none, then the window only flashes in the taskbar.
  - Content **[assumed]**: profile list (active one marked) with `New`; per profile `Apply profile`, `Rename`, `Copy`, `Delete`, the hotkey with validation feedback, the entry table (program, path, action, options) with `Edit` and `Remove`; an edit dialog with the options of each action (`args`/`cwd` for start and for stop with `restart_on_neutral`, `on_timeout`, per-entry `trim`, `confirmed` for warn-list programs); `Add program…` with the running programs (name, private memory, protection) and `Choose executable…`; the unassigned programs (N1) with `Add` and `Dismiss`; `Start at sign-in` in the header; a status line. Light and dark follow `prefers-color-scheme`; colours come from the branding tokens.
  - Review options **[assumed]**: hidden `--screenshot`, `--view`, `--theme`, `--drive`, `--drive-out` (see [gui-bridge.md](gui-bridge.md)) let tests drive the real page through its bridge and capture it with WebView2's own capture API.
- **N2 Unassigned programs, details** **[decided]** (owner, 2026-11-23) (WP11; N1):
  - Sources, read only: the values `ExecutablePath` below `HKCU\Control Panel\NotifyIconSettings` (Windows 11), counted only while a process of that name runs in the session, plus processes of the session that own a visible top-level window. Only window handles are queried, no process is opened (A7). Hard-protected programs and kaltstellen itself are left out. A path is kept only when the registry gives an absolute one (entries starting with a known-folder GUID keep the name only).
  - Snapshots: when the main window opens, when its profile selection changes, on `Refresh`, after it saved an edit, after the tray applied a profile (not `neutral`), and on `kaltstellen status --unassigned`. Plain `status` takes none.
  - `seen.json` (data directory): each program with `first_seen`; per profile a fingerprint of its entries and when it last changed; per profile the dismissed programs. Written atomically; an unreadable file is reported (`KS304`) and treated as empty.
  - A program is unassigned in a profile when it runs now, has no entry with its executable name in that profile (any action counts, `ignore` too) and was not dismissed for that profile. It is `new` when it was first seen after the profile last changed (seconds). `neutral` lists nothing.
  - The tray menu shows `Unassigned programs (N)` for the active profile when N > 0; it opens the main window. The icon keeps its three states.

- **UP1 Updates** **[decided]** (owner, 2026-11-23), except signing (open, decided with the release pipeline) (owner request 2026-11-22: an update function before the first release, modelled on the owner's project foresprint; its Go package `selfupdate` was the reference):
  - **Non-goal changed.** requirements.md said "no network access". Now: the only network access is the update check and download described here; it can be switched off and sends nothing about the user. A check is one HTTPS `GET` to the GitHub API with the user agent `kaltstellen/<version>`; GitHub sees the IP address and the time, nothing else is sent (no identifier, no configuration, no usage data). Telemetry stays a non-goal.
  - **Source.** GitHub Releases of the public repository `swallat/kaltstellen`: `GET https://api.github.com/repos/swallat/kaltstellen/releases/latest`, which leaves out drafts and pre-releases. A running pre-release (`1.0.0-beta.2`) instead reads `/releases?per_page=30` and takes the highest version that is not a draft, so a beta sees the next beta (as foresprint). The tag is `v<version>`. `KALTSTELLEN_UPDATE_URL` replaces `https://api.github.com/repos/swallat/kaltstellen` (tests, a mirror); plain `http://` is accepted only for loopback hosts.
  - **Release files** (what the client looks for, see [release-requirements.md](release-requirements.md)): `kaltstellen-<version>-windows-<arch>.exe` (`x86_64`, `aarch64`), the bare executable, and `checksums.txt` with lines `<sha256 hex>  <file name>` (`sha256sum` format, as foresprint). Other files are ignored, so a release can add a zip for people, signatures or notes without changing the client.
  - **Versions.** `MAJOR.MINOR.PATCH` with an optional pre-release after `-`, build metadata after `+` ignored, a leading `v` allowed. Numbers compare as numbers; a pre-release is lower than its release; pre-release parts compare dot by dot, numbers below words (as foresprint). A **development build** (compiled without `KALTSTELLEN_RELEASE=1`, the release workflow sets it) never checks by itself and refuses to install, unless `KALTSTELLEN_UPDATE_URL` names a source; `update --check` works. Reason: a local `cargo build` must not replace itself with a release.
  - **Three modes** **[decided]** (owner, 2026-11-23): `auto` (default) checks once a day and installs a newer version by itself; `manual` checks once a day, reports, and installs only on request; `off` never connects. Stored in `settings.json` (configuration directory, key `update`); set with `kaltstellen update --mode auto|manual|off`, the tray items `Updates: install automatically` / `check and ask` / `off`, or the main window. The earlier `--auto on|off` still works (`on` now means `auto`), and a saved `on` reads as `auto` **[assumed]**. The environment variable `KALTSTELLEN_UPDATE` (`auto`, `manual`, `off`) wins over the file, for example for a managed installation. **Automatic install never thaws or disturbs work** **[decided]**: the tray installs by itself only while `neutral` is active and no switch runs or waits; otherwise the update waits. **[assumed]**: it is tried again whenever a switch ends with `neutral` active (from the menu, a hotkey or the command line) and after every later check; the tray logs once per version that it waits. The automatic install runs `kaltstellen update` without a console window (steps in the log); it quits the tray as below, which has nothing frozen in `neutral`, and starts the new tray. All safety below (checksum, staged start check, rollback, refusal under package managers and Program Files, development builds) applies unchanged; when installing is refused here, the tray only reports.
  - **Automatic trigger.** Only the tray checks automatically: 5 minutes after it starts if the last check is 24 hours or more ago, then 24 hours after each check. It sets one one-shot Win32 timer for the due time (no polling, no thread waiting, Q1). The check runs as a child process `kaltstellen update --check --quiet` without a console window: no network code runs inside the tray, so neither its memory nor a hang or crash in it reaches the process that holds the frozen programs. A failed check (offline) counts as a check and is tried again 24 hours later. Choosing a mode that checks, in the menu or the main window, checks at once. Unlike foresprint, command-line commands never check by themselves: they are run by scripts and hotkeys, where a delay or a line on stderr does not belong.
  - **What is remembered.** `update.json` in the data directory: time of the last check, the newest version found and its release page, the error of a failed check. Written atomically by the checking process; the tray reads it when its menu opens and when the check child ends. A missing or damaged file means "never checked" (`KS807` in the log).
  - **Tray menu.** The three mode items (the current one checked; disabled when `KALTSTELLEN_UPDATE` decides). When the last check found a newer version: `Install update <version>…`, which opens a console window running `kaltstellen update` so progress and errors are visible; it waits for Enter at the end if something failed. When this copy cannot update itself (see "Where"): a disabled line `Update <version> available: <how>` instead. Icon and tooltip do not change for an update.
  - **Command line.** `kaltstellen update --check`: checks now (also in mode `off`, it is explicit), prints the result, exit code 0, or 1 with `KS801`/`KS802` on failure. `kaltstellen update`: installs the newest version if it is newer (exit 0 also when there is nothing to do). `--force`: installs the newest version even if it is not newer (repair, tests). `--mode auto|manual|off`: sets the mode.
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
- **Toolkit**: WebView2 by owner decision on 2026-11-23 **[decided]**, see M6. (Earlier constraint: single executable, no runtime, small idle footprint, native preferred; overruled for the window knowingly, the tray keeps it.)

## 9. Quality targets (measurable)

- **Q1** Tray app idle: private working set below 15 MB and CPU below 0.1 % averaged over 60 s, GUI process not running. **[decided]** (thresholds)
- **Q2** One executable, no installer, no runtime. The main window needs the Microsoft Edge WebView2 Runtime, which Windows 11 includes; nothing else is shipped (the WebView2 loader is linked into the executable) **[decided]** (owner, 2026-11-23).
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
