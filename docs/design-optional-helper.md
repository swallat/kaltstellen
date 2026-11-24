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
- M4c: watch mode (`watch = true`) in the tray.
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
