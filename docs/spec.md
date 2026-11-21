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

## 4. Session and lifecycle

- **L1 Single instance** **[assumed]**: one tray instance per user session, enforced with a named mutex. The command line talks to the running instance over a named pipe whose ACL allows only the current user **[r2]**. If none is running, `kaltstellen switch` runs in-process and takes an exclusive lock on the state file for the duration.
- **S5 Sign-out and shutdown** **[decided]**: all suspended programs are resumed first. On the next start the active profile is `neutral` and the tray offers the last active profile for one-click activation without applying it. The name of that last profile is stored in `settings.json`, not in `state.json`.
  - Mechanism **[decided]** (resume at `WM_QUERYENDSESSION`): the tray app owns a hidden top-level window and resumes everything when it receives `WM_QUERYENDSESSION`, not only on `WM_ENDSESSION`. Reason: a frozen program cannot answer `WM_QUERYENDSESSION`, so Windows would wait for it and show a blocking dialog. The tray sets a high shutdown priority (`SetProcessShutdownParameters`) so it is asked early. If the shutdown is then cancelled, the profile is gone and the tray offers it again. The command line uses `SetConsoleCtrlHandler` for the same purpose.
  - Records of stopped programs are cleared at sign-out and shutdown; they are kept across `Quit` and crashes.
- Standby and hibernate: untouched. Frozen processes stay frozen across standby **[decided]**. Wake events are only logged (`WM_POWERBROADCAST`). Risk: network connections, timers and sessions of frozen programs have expired on wake and are only processed after the next resume (R4).
- **L2 Quit** (tray menu): resumes all suspended programs, clears recorded suspended state, leaves stopped programs stopped (their records stay, so a later switch to `neutral` can restart them), then exits.
- **L3 Recover** **[r2]**: `kaltstellen recover` resumes everything recorded in state without needing the tray. Reason: a tray app killed by the user or a crash leaves programs frozen until kaltstellen runs again. The tray app should be started at sign-in so recovery happens automatically.
- **S3 Crash**: on the next start, recovery resumes every recorded suspended process that still matches (S6), clears state, and sets the active profile to `neutral`.

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
```

- **C1** Profile names are unique and non-empty; `neutral` is reserved.
- **C2** An `exe` appears at most once per profile; it may appear in several profiles; the active profile alone decides its action.
- **C3** An invalid file is rejected as a whole with file, line and reason. The previous valid configuration stays in effect and the tray shows the error state. Unknown keys are errors, so typos are caught.
- **C4** The file is re-read on change. A running switch finishes with the old configuration. If the active profile was removed or renamed, it stays active under its old name until the next switch.
- **C5** Hotkeys must be unique. A hotkey that cannot be registered (taken by another program) is reported once at start and ignored; the profile stays usable from tray and command line.
- **C6** `args` only for `start`; `restart_on_neutral` and `on_timeout` only for `stop`. Other combinations are errors. An entry with `action = "start"` requires `path` **[r2]** (a name alone cannot be launched).
- **C7 Hotkey syntax** **[r2]**: modifiers plus one key. `Ctrl+Alt` combined with a printable key is rejected with a warning, because on layouts with AltGr (such as German) `Ctrl+Alt+key` produces characters like `{`, `[`, `@`, `€` and registering it would steal them. Function keys with `Ctrl+Shift+Alt` are recommended. `RegisterHotKey` uses `MOD_NOREPEAT`. Hotkeys do not fire on the secure desktop (UAC, lock screen).

## 6. State file

`%LOCALAPPDATA%\kaltstellen\state.json`, written atomically (temp file + rename) after every step.

- Content: active profile, system boot time **[r2]**, and per affected process: PID, process creation time, full image path, action (`suspended` | `stopped`), and for stopped ones the start command and `restart_on_neutral`.
- **S10** **[r2]** Recovery first compares the boot time; after a reboot the suspended records are dropped without resuming anything. Before resuming a matching record it checks that the image path equals the recorded one. During a switch the open process handle is kept until the step finishes, which also rules out PID reuse within a step.
- **S6** A process matches a state entry only if PID and creation time both match; a reused PID is ignored.
- **S9** A corrupt file is renamed to `state.json.bad` and treated as empty.

## 7. Protection, errors, rights

- **P6 Two tiers** **[decided]**:
  - *Hard-protected*: core Windows processes, the shell, console hosts (`conhost`, `OpenConsole`), error reporting and the desktop window manager **[r2]**. Never touched, whatever the config says (A6). Not selectable in the GUI; rejected by the command line.
  - *Warn list*: remote-access tools, graphics drivers, licence services, security software, developer tools. Allowed only after explicit confirmation in the GUI (command line: `--force`). The list is data shipped with the tool. (User-extendable lists are post-v1 **[decided]**.)
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
- **Q4** All `unsafe` Win32 code lives in one module.
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

All eight points of the round 2 review were confirmed by the owner on 2026-11-21 and are marked **[decided]**. The remaining **[assumed]** items (not raised in round 2) still await confirmation.

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
