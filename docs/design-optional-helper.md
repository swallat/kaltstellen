# Design note: the optional background helper (M4; A5, P7, P8, W6, H2, H5)

Status: decided on 2026-11-24 (five decisions at the end); M4a built (see "M4a as built"), the rest not implemented yet. Requested by the owner on 2026-11-24 **[decided]**: an
optional background component with administrator rights that can be
installed later, while a portable version without administrator rights stays
possible. The owner chose the widest scope (watching, acting on elevated
programs, Windows services). The five decisions at the end are **[decided]**; everything else is **[assumed]** until the owner has reviewed it. Implementation follows the milestones below.

This note extends [design-elevated-helper.md](design-elevated-helper.md)
(services only, request driven). Its pipe trust model, allow list and
alternatives stay as written there; this note adds the roles *observe* and
*act*, the install rules and the behaviour without a helper. Where the two
differ, this note wins.

## Principles

1. **Optional.** The bare executable (portable use, the updater, a copy on a
   USB stick) works completely without the helper, as today. Every
   feature that needs it says so and degrades to what is possible without
   it (never an error at start).
2. **The tray never runs elevated** (P8). The helper is a separate Windows
   service. It is the eyes and the privileged hand; the tray keeps the
   policy: it decides what a profile does.
3. **Least capability.** The helper does only what an administrator allowed
   in a file that only administrators can write, and nothing for programs
   that are hard-protected (P6).
4. **The no-watcher rule is for the tray.** W6 and Q1 (no background
   watcher in the tray, idle footprint) stay true for the tray and for the
   portable use. With the helper installed and `observe` enabled, the
   *helper* watches; this is the owner's explicit exception of 2026-11-24 and
   changes nothing for people who do not install it.

## Components and install

- Same executable, hidden subcommand `kaltstellen helper` (Q2). It contains
  no window, tray or configuration code path.
- Windows service `kaltstellen-helper`, start type automatic (delayed), so
  the observation does not miss starts after sign-in. This replaces the
  "trigger start, stop when idle" of the older note for the roles *observe*
  and *act*; with only `services` enabled it may stay trigger-started.
- **The service runs a protected copy.** `kaltstellen helper install` (from
  an elevated prompt, the only time UAC appears) copies the running
  executable to `%ProgramFiles%\kaltstellen\` (writable by administrators
  only) and registers the service from that copy. Reason: a service that
  ran the portable copy from a user-writable folder would let any program of
  the user replace the file and run as `LocalSystem`. The same rule is why
  the helper never updates itself from the user-level updater; the owner
  updates it with `kaltstellen helper install` again (the tray tells when
  the helper is older than the tray, version check in the pipe hello).
- The setup offers it as an **optional component, unchecked by default**
  (a UAC prompt at that moment); the portable executable offers
  `kaltstellen helper install|uninstall|status` and the main window shows an
  entry `Background helper` in `Settings` with the state and the same
  actions (each starts an elevated process with a UAC prompt; the window
  itself stays unelevated).
- The folder `%ProgramData%\kaltstellen\` gets an explicit ACL at install
  (administrators and SYSTEM full control, the owner read only), so
  `helper-state.json` and `helper.log` cannot be changed by users.
- Policy file `%ProgramData%\kaltstellen\helper.toml` (ACL: administrators and
  SYSTEM write, the owner reads): the roles that are on, the allow list of
  services (older note) and the allow list of elevated programs (act).
  Changed only from an elevated prompt (`kaltstellen helper allow ...`).
- Account: `LocalSystem` with every privilege the service does not need
  removed (required-privileges list: the ones for kernel event sessions,
  opening processes of the owner's session, and `SeChangeNotify`). Decided: see the end of this note.

## Roles

### observe (eyes)

- Two real-time kernel event sessions owned by the service: process start
  and stop (`Microsoft-Windows-Kernel-Process`: PID, parent PID, image
  path, creation time, session, user SID) and network (`Microsoft-Windows-
  Kernel-Network`: bytes sent and received per PID). Both need the rights
  the service has and the tray lacks (H2).
- The service keeps only what the tray needs, in memory, bounded: a ring
  of the last 24 h of process start and stop events of the owner's
  session (PID, creation time, parent, image path, time) and per-PID byte
  counters per second for the last 10 minutes. No addresses, no ports, no
  packet data, no command lines, no window titles. Nothing is written to
  disk; a restart starts empty.
- Used by the tray through the pipe (pull and, while the tray runs, push of
  start events):
  - **H2**: per-program network volume (`Network ↓ ↑` next to `I/O` and
    `Connections`), measured over the same interval; the thing the portable
    use cannot show.
  - **H5**: real events with times instead of two lists (`ended`, `started`,
    `came back`, and by whom: parent process), which removes most of the
    noise listed in R12; the rules of H5 stay (counts, thresholds, nothing
    applied by itself).
  - **W6**: the tray learns of new processes at once. The profile is then
    applied to the program that appeared (below), instead of at the next
    switch.
- Cost, **[assumed]** targets to be measured on the VM and the desktop: under
  0.1 % CPU and 40 MB private memory at idle, event loss counted and logged,
  nothing when the pipe has no client for longer than 5 minutes except the
  rings.

### act (hands, only where the tray cannot)

- The tray remains the decision maker and does everything it can do
  unelevated, as now: graceful close, minimise, freeze, start. New
  programs reported by *observe* are matched against the active profile and
  handled by the tray in the same way as in a switch (W1 order for that
  program, W7), only when the profile has `watch = true` **[decided]** (a new optional
  boolean profile key, to be added to spec section 5 and C3 when this is
  accepted; default off). With `watch` on, a new matching program is handled
  without a click: that is the owner's exception to W6 and to the
  "nothing without a click" rule of H, limited to profiles that set the key
  and to machines with the helper. H5 (transitions only) is unchanged for
  everyone else; with the helper it may use the real events (above).
- The helper acts only for what the tray cannot (P7): elevated programs.
  Requests: `suspend`, `resume`, `terminate` for a PID with its creation
  time. The helper refuses unless the program is on the **allow list of
  elevated programs** in `helper.toml`, belongs to the owner's session and
  user (an elevated process of the same user, never another user's and
  never a service of the system), is not hard-protected and not on the
  warn list without the administrator's entry, and its creation time
  matches (S6).
- A graceful close and minimising cannot reach a window in the owner's
  session from a service (session 0 isolation; UIPI concerns the unelevated tray only, P7): for elevated programs
  `stop` means `terminate` after `stop_timeout_s`, with `on_timeout = keep`
  leaving it running. This is said in the entry dialog.
- Safety: everything the helper froze is recorded in
  `%ProgramData%\kaltstellen\helper-state.json` and resumed when the owner's
  tray asks, when the tray's pipe has been gone for 60 s, at service stop and
  at service start (after a crash), at sign-out and shutdown. A frozen
  elevated program never outlives the tray's session (S3, S10).

### services (older note)

- `stop-service` and `start-service` as in the older note, from the
  allow list, with a state record so `neutral` starts them again (P5). A
  new entry kind `service = "Name"` is handled in the stop and start steps.
  Without a helper such entries fail with `needs the helper` (W2).

## Trust between tray and helper

- As in the older note: pipe `\\.\pipe\kaltstellen-helper`, DACL for the
  owner's SID recorded at install (passed by the unelevated caller to the
  elevated installer, so an administrator's credentials entered at the UAC
  prompt do not record the wrong account), first instance, no remote clients, the
  tray verifies the server's PID against the service.
- **The path of the client is not a defence.** A process of the same user
  can run any file, and a portable copy must be able to use the helper. The
  protection is therefore the allow lists, the hard-protected list, the
  owner and session checks and the bounded verbs above, not who the client
  is. The helper does not rely on a signature (decision 4); once the project has a certificate it may additionally require the signature of the installed publisher.
- Versioned JSON lines, size-limited, `deny_unknown_fields`; one request at a
  time per client; every request and refusal logged to the event log and
  the log `kaltstellen.log` in `%ProgramData%\kaltstellen`.

## Without the helper (portable)

- Nothing is installed, no service, no elevation; the tray and the window
  behave as specified now. `Background helper` in `Settings` says `not
  installed` and what it would add (network volume, immediate handling of
  new programs, elevated programs, services).
- A portable copy on a machine where the helper is installed uses it (same
  user); a copy of another user does not (pipe DACL).
- A helper that stops or is removed: the tray falls back at once to the
  lazy behaviour (W6, N1, H5 with two lists) and shows an info line, the
  state of what the helper froze is resumed by the helper itself (above).

## Limits and risks (R13 to R15, to go into spec section 13)

- **R13** A `LocalSystem` service that can freeze and end processes is a
  target; its safety is the allow lists, the checks above and a small
  separately reviewed module, not secrecy. An unsigned executable cannot be verified by the helper.
- **R14** Kernel network events are high volume under load; the helper
  counts bytes per PID only and may lose events, which it logs. Values are
  per second counters, not a record of traffic.
- **R15** Antivirus and endpoint products watch services that suspend and end
  processes and open kernel event sessions; the helper is offered unsigned, so SmartScreen and antivirus products may warn (decision 4); the test with Defender (T21) still applies.

## Milestones (replaces M4 of section 11)

- M4a: `helper` subcommand, service install and uninstall, protected copy,
  pipe, trust checks, `status`, the `Background helper` entry in `Settings`.
- M4b: *observe* with the process events and the pull requests, then the
  network counters; H2 and H5 use them.
- M4c: watch mode (`watch = true`) in the tray (built: key and window, tray re-applies the active profile when a program with an entry started).
- M4d: *act* on elevated programs with the allow list and the safety
  record.
- M4e: services (A5).

Each step is useful on its own and ships behind the helper being installed.

## Decisions of the owner (2026-11-24)

1. **Account**: `LocalSystem` with privileges removed.
2. **Start**: automatic (delayed).
3. **`watch`**: opt-in key per profile.
4. **Code signing**: none for now, a certificate comes when the project is
   better known. The helper is built, tested and offered unsigned. The helper never relies on a signature, and the setup says that the file is unsigned.
5. **Elevated-program allow list**: changed only from an elevated prompt.

## M4a as built (2026-11-24) **[assumed]**

Built: the install and removal of the service, the protected copy, the
policy file, the pipe with `hello` and `status`, and the command line.
Nothing observes or acts yet (M4b to M4e); the roles in `helper.toml` can be
set, and `status` says that they are not all implemented.

- `kaltstellen helper install [--owner-sid SID]`: from an unelevated
  prompt it starts itself again with a UAC prompt (`runas`) and passes the
  caller's SID, so the account typed at the prompt does not change who the
  helper serves. Elevated, it creates `%ProgramFiles%\kaltstellen` and
  `%ProgramData%\kaltstellen`, gives the data folder an explicit access list
  (SYSTEM and administrators full, the owner read), writes `helper.toml`
  (a second install keeps roles and allow lists, only the owner is updated),
  stops a running service, copies the running executable through a
  temporary file and registers the service: `LocalSystem`, automatic
  (delayed), restart after two failures, required privileges reduced to
  `SeChangeNotifyPrivilege` (M4b adds what the event sessions need). The
  window of the elevated step stays open until Enter when it failed.
- `kaltstellen helper uninstall`: stops and deletes the service and
  removes the files the install made; folders only if empty.
- `kaltstellen helper status`: service state, protected copy, roles, and
  what the pipe answers, with a note when the helper is older than this
  executable. Exit code 0 also when no helper is installed.
- `kaltstellen helper run` (hidden): the service body. It reads
  `helper.toml`, creates `\\.\pipe\kaltstellen-helper` with an access list
  of the owner and SYSTEM, serves up to 8 clients at once with a 5 s limit
  per conversation, and refuses to start outside the service manager.
- Protocol: JSON lines of at most 4096 bytes, `{"v":1,"cmd":"hello"}` or
  `status`; unknown fields, verbs and versions are refused. The client
  checks that the pipe is served by SYSTEM (the helper's PID is checked
  against the service manager in a later step).
- Errors `KS1001` to `KS1005`.
- Tested: the protocol, the policy, the copy and `status` without a helper
  run in CI. Installing, starting and stopping the service has no automatic
  test (a test must never touch a real service); it is checked by hand on
  the VM (docs/manual-checks.md).

Hardening from the review of M4a (same day):

- The install uses a folder under `%ProgramFiles%` or `%ProgramData%` only if
  SYSTEM or the administrators own it and it is no link; any user can create
  a folder in ProgramData first and would keep the right to change its
  access list. Both folders come from the system (known folders), not from
  the environment, which an elevated process inherits from its caller; the
  `KALTSTELLEN_HELPER_*` redirections work in debug builds only (tests).
- The source of the copy is the running image as the kernel tracks it, opened
  once against writers, written to a temporary file and moved over the old
  copy in one step.
- A standard user cannot read the token of the SYSTEM service, so the client
  identifies the helper by process id: the pipe server must be the service's
  process (service state is read with query rights only). The owner's right
  on the pipe is read and write without creating further instances.
- The service reports `running` only once the policy is read and the pipe is
  bound, tries again for about a minute if the pipe name is taken, and is
  restarted also after an error exit.
- The owner SID must be one account (`S-1-5-21-…` with a RID of 1000 or
  more, or an Entra ID account), not a group or a built-in account.
- The data folder gets its access list before anything else is put in it,
  and files in it that SYSTEM or the administrators do not own are removed
  (a user owns what they created whatever its access list says). A
  `helper.toml` that exists but cannot be read stops the install instead of
  being replaced.
- Stopping waits for a starting service; deleting copes with a service that
  is already marked for deletion; `helper status` also works where the
  service state cannot be read, and says so.
- Known and accepted: the source of the copy is the running image and held
  open from the start of the elevated step, but a file swapped before the UAC
  prompt is answered cannot be told from the real one (the prompt shows the
  path only).

## Settings entry (built after M4a)

`Settings` > `Background helper` shows the service state and the pipe answer (`helper_status`, same facts as `kaltstellen helper status`) and starts `helper install` / `helper uninstall` in a console of its own (`--from-window` keeps that console readable). The window never elevates itself and sends no account name; the elevated step is the command line's `runas` relaunch (see above).

## M4b as built: process events (first part of *observe*)

- With the role `observe` on (the default after install) the service starts
  a real-time event session `kaltstellen-helper-process` for the provider
  `Microsoft-Windows-Kernel-Process` (keyword process start and stop only).
  A session of that name left behind by a crashed service is stopped and
  started again. If the session cannot start, the helper still answers and
  `helper status` says `observe (process events): not running` (also when
  the session ends later, for instance stopped from outside); the reason
  is in the log (`KS1006`).
- Event fields are read by name (TDH), not by byte offset, so event versions
  do not matter. Kept per event: number, `start`/`stop`, pid, parent pid,
  Windows session, creation time, time, file name, full path in drive form
  (`null` when the kernel path maps to no drive). No command lines, no
  window titles. A file name is cut at 255 characters, a path longer than
  520 is not given out (`path` is `null`). The ring holds at most 4096
  events and 24 hours (by the clock now, also when nothing happens), only in
  memory; a restart starts empty.
- Pipe verb `{"v":1,"cmd":"events","after":N}` (extra field `after`, only
  with `events`): the events from number `N` on that belong to **the
  caller's own Windows session** (looked up from the pipe's client process),
  at most 100 per answer, with `next` (ask again with this) and `lost`
  (events or buffers the kernel reported lost since the helper started;
  events that fell out of the ring show as a jump of `next` past `after`).
  An `after` beyond what the helper has numbered means it restarted since:
  the answer starts from the oldest event and `next` is smaller than
  `after`, which is the reader's sign of the restart. A caller whose session cannot be told gets nothing. A `stop`
  whose event does not name its session takes it from the matching `start`
  (same pid and creation time); without a match it is not given out.
- Replies may be up to 256 KiB; requests stay at 4096 bytes.
- `kaltstellen helper events [--after N]` prints them as JSON lines (for
  checking by hand; the tray will use the pipe).
- Not part of this step: network volume per program (H2) and the push of
  events to the tray (the tray polls with `after`); both stay in M4b/M4c.
- The tray's profile assistance (another thread) uses this verb for the
  event "process started": `exe`, `path` (may be `null`), `pid`, `time` as
  seconds since 1970.

## M4b as built, part 2: network counters

- A second event session `kaltstellen-helper-network` enables
  `Microsoft-Windows-Kernel-Network` (TCP and UDP, IPv4 and IPv6), with more
  buffers, so a burst of packet events cannot cost process events. Only the
  process id and the size in bytes of each event are read; no address, no
  port, no data. If it cannot be started, the process events still run and
  `helper status` says `network counters: not running`.
- The helper keeps per process per second sums of bytes sent and received
  for the last 10 minutes, in memory (at most 4096 processes). A process id
  can be reused: the counters of a process id are forgotten when a process
  with that number starts or ends, so a short-lived program has no counters
  after it ended and a new program never inherits old ones. At the limit,
  memory is bounded at a few tens of MB.
- Pipe verb `{"v":1,"cmd":"network","seconds":N}` (`seconds` 1 to 600,
  default 10, only with `network`): `traffic`, a list of `{pid, up, down}` for the
  processes of the **caller's own Windows session** (each process id is
  looked up when asked), processes without traffic left out.
- `kaltstellen helper traffic [--seconds N]` prints the same as JSON lines.
- H2: the window shows `Network ↓ ↑` next to `I/O` and `Connections` when
  the helper's counters run (built, see `measure` in gui-bridge.md); the
  tray menu itself shows no figures.
- Under heavy load the kernel may drop events; `lost` of the `events` verb
  counts buffers lost for the whole session. Values are counters, not a
  record of traffic (R14).

## M4d as built, part 1: the policy commands **[assumed]**

The first part of *act* is only the administrator's side: who may be acted
on, and the switch for the role. The helper does not suspend or end
anything yet.

- `kaltstellen helper allow list` (no elevation; the owner reads the
  file) shows the role and the allow list.
- `kaltstellen helper allow add <full path>` and `allow remove <full path>`,
  and `kaltstellen helper act on|off`, ask for administrator rights through
  the same UAC relaunch as `install` (decision 5: the allow list changes
  only from an elevated prompt). The elevated step edits `helper.toml` and
  restarts the service so that it reads the file again; nothing is created
  for a helper that is not installed.
- An entry is a full path on a drive to an `.exe`: no wildcards, quotes,
  relative parts or network paths, at most 520 characters, at most 256
  entries, and not a hard-protected program. It is checked before the UAC
  prompt (and the file must exist) and again by the elevated step. Entries
  compare case-insensitively with either slash. `remove` takes the text as
  given so that an entry that became invalid can still be removed.
- The window shows the same later (not part of this step).
- `helper.toml` is written through `helper.toml.tmp` and renamed, so a crash
  never leaves a half written policy. A service that was stopped on purpose
  stays stopped; if the restart fails the message says the policy is saved
  and how to start the service. Two simultaneous elevated edits can lose
  one update (no lock); the edits are rare and each needs a UAC prompt.
- Entries with a stream (`:` after the drive) or a short name (`~1`) are
  refused, so the protected-program check cannot be passed by spelling. The
  later enforcement compares with the process's final path.


## M4d as built, part 2: the helper's hands **[assumed]**

With the role *act* on, the helper answers `suspend`, `resume` and
`terminate` for one process (`pid` and its creation time `created`). The
tray's use of them comes in part 3; `kaltstellen helper suspend|resume|terminate
--pid N` exist for checking by hand.

- Every request is checked on one opened handle (so the process cannot
  change in between): the role is on; the pid is above 4 and not the
  helper's own or the caller's; the creation time matches (S6); the program
  runs as the owner recorded at install, in the Windows session of the
  caller; it is not hard-protected; its full image path is on the allow
  list. Anything else is refused with a reason.
- `suspend` is recorded in `%ProgramData%\kaltstellen\helper-state.json`
  before the process is touched (and removed again if the freeze fails or
  the process is gone). `resume` only thaws what this helper froze, so the
  helper cannot be used to resume somebody else's suspended program.
  `terminate` ends the process (for an elevated program `stop` is
  terminate, as the spec says).
- At most 64 programs are frozen at once.
- A frozen program is thawed when the process that asked is gone (checked
  every 5 s, by pid and creation time), when the service stops, and after a
  crash at the next start (from the record). A record that does not match a
  running process is dropped.
- The status shows `act` among the active roles only when it runs.
- What cannot be told or released stays on the list: a failed snapshot
  never makes the sweep forget a frozen program, and a resume that fails is
  tried again (at the next sweep, at stop, at the next start). At service
  stop new freezes are refused. A record left by a crash is thawed at start
  even when the role was switched off meanwhile.
- Known limit: the record is written before the checks, so a crash in that
  instant can make the next start resume a process that was not frozen by
  the helper (resuming a running process does nothing; a program somebody
  else suspended would be thawed).


## M4d as built, part 3: the tray uses the hands **[assumed]**

The engine gets an option `helper`, set only by the tray's worker (a
long-lived caller: the helper thaws what it froze when its asker is gone, so
a one-shot `kaltstellen switch` from a prompt does not use it).

- `suspend`: a process that cannot be opened for lack of administrator
  rights (`NeedsAdmin`, not a protected one) is recorded in the state like any
  frozen process and then handed to the helper. If the helper refuses (not
  installed, role off, not on the allow list, not the owner's), the record is
  removed and the original refusal is reported as before.
- `resume`: the same kind of refusal on a recorded process makes the engine
  ask the helper to resume it; if that fails the record stays for the next
  attempt.
- `stop`: elevated processes are ended by the helper at once (no graceful
  close, as the spec says `stop` is `terminate` for them); the others take
  the normal path. A refusal by the helper leaves the process to the normal
  step, which reports `NeedsAdmin`.
- The preview (dry run) still reports an elevated program as a problem even
  if the helper would take it; it does not ask the helper.
- If the tray ends (crash), the helper thaws its programs within seconds.
  The tray's own record then names processes that are no longer frozen;
  resuming those does nothing.
- An answer of the helper that says no is told from a lost connection: after
  a lost connection the record of a frozen program stays (the helper may have
  frozen it; a later resume asks again). An elevated program ended by the
  helper counts as stopped for the restart record even if another process of
  the program reports an error.


## M4e as built, part 1: the helper's side of *services* **[assumed]**

Decision of the owner (2026-11-25): a service gets its own entry kind
`service = "Name"` in a profile (part 2, not built yet).

- Policy: `kaltstellen helper services on|off` and `helper allow
  add-service|remove-service <key name>` (UAC, as for programs); `allow
  list` shows both lists. A service is named by its short key name
  (`Spooler`), at most 256 characters, no path, wildcard or control
  characters, at most 64 entries. A short list of services is never accepted
  and never touched even if a file lists them (the helper itself, RPC, DCOM,
  event log, security accounts, firewall, Defender, the task scheduler and a
  few more).
- Pipe verbs `stop_service` and `start_service` with the field `service`;
  the helper waits up to 1.8 s for the wanted state and answers `reached`
  (`false`: asked, not there yet; the caller asks again later). Refused with
  a reason: role off, not on the allow list, no such service, a refusal of
  Windows (for example a service with running dependents).
- The helper keeps no record. Starting what was stopped is the tray's job
  through its own record, as for a stopped program (part 2).
- `kaltstellen helper stop-service|start-service --name N` for checking
  by hand.
