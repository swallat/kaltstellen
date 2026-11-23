# Design note: elevated helper for services (M4; A5, P8)

Status: proposal for the owner to decide. Nothing of this is implemented.

## Problem

A5 asks for stopping and starting Windows services as part of a profile.
Controlling most services needs administrator rights. P8 decided that the
tray never runs elevated and that a small elevated helper does this work.
P7 and P9 add two constraints: programs that kaltstellen starts must not
inherit elevation, and kaltstellen never triggers UAC prompts during a
switch.

## Proposal

### What the helper is

- The same executable with a hidden subcommand, `kaltstellen helper`, so Q2
  (one executable) holds. It contains no tray, window, engine or
  configuration code paths when started this way; it only parses its own
  narrow requests.
- It runs as a **Windows service** (`kaltstellen-helper`, `LocalSystem` or
  better a virtual service account with only `SERVICE_START`/`SERVICE_STOP`
  rights granted on the configured services), installed once by the owner
  from an elevated prompt with `kaltstellen helper install`. Installing is
  the only moment UAC appears; switches never prompt.
- Start type "manual, trigger start": started by the service control
  manager when the tray connects, ended after an idle timeout (for example
  60 s). No permanent process, so Q1 for the idle tray is unaffected.

### How tray and helper find and trust each other

- Named pipe `\\.\pipe\kaltstellen-helper`, created by the service with a
  DACL that grants connect to the **interactive user SID that installed it**
  (recorded at install time) and to nobody else; remote clients refused;
  `FILE_FLAG_FIRST_PIPE_INSTANCE`.
- The helper checks every client: `GetNamedPipeClientProcessId`, then the
  client's token user SID must equal the recorded owner SID, and the
  client's image path must be the installed `kaltstellen.exe` (same path as
  the service binary). Optionally also the Authenticode signer once the
  executable is signed (R7).
- The tray checks the server: `GetNamedPipeServerProcessId` must belong to
  the service process of `kaltstellen-helper` (query the service status for
  its PID) and run from the same executable path. This prevents a squatter
  pipe from collecting requests.
- No shared secret is needed; Windows identities do the authentication.

### What the helper accepts

- Exactly two request kinds: `stop-service NAME` and `start-service NAME`,
  plus `status NAME`. JSON lines, versioned like the tray pipe.
- `NAME` must be on an **allow list** stored at install time in a file only
  administrators can write (for example `%ProgramData%\kaltstellen\helper.toml`,
  ACL: Administrators and SYSTEM write, the owner read). The user
  configuration (`config.toml`, user-writable) only references services by
  name; a service not on the allow list is refused with a stable error code.
  Changing the allow list needs an elevated `kaltstellen helper allow NAME`.
- Hard-protected services (core Windows services, security software,
  drivers) are refused even when allowed, analogous to P6.
- No arbitrary command execution, no process start, no file access, no
  registry writes. The helper never starts programs, so P9 (no inherited
  elevation) holds by construction.

### Keeping the attack surface minimal

- Small, separately reviewed module tree (`src/helper/`), its own `unsafe`
  limited to service control calls in `src/win32/`.
- Requests are size-limited and parsed with `deny_unknown_fields`.
- One request at a time; every request and refusal is logged to the Windows
  event log and to `%ProgramData%\kaltstellen\helper.log`.
- Idle shutdown; no listening when no tray connects.
- Uninstall: `kaltstellen helper uninstall` removes the service, the pipe and
  the allow list.

### How it fits the switch

- Services become a fifth kind of entry (for example `service = "Spooler"`,
  action `stop` or `start`), handled in the stop and start steps of W1 by
  sending requests to the helper. Without a helper installed such entries
  fail with "needs the helper" (W2: collected, not fatal).
- State records stopped services so `neutral` can start them again, as for
  programs (P5).

## Alternatives considered

| Alternative | Why not (or not first) |
| --- | --- |
| Elevated tray | Rejected in P8: every started program would inherit elevation (P9), UIPI and integrity problems, bigger attack surface. |
| Scheduled task "run with highest privileges" started on demand | No UAC per call, but authenticating the caller is weak (anyone who can trigger the task can run it), passing parameters is awkward, and the task runs a full process per request. |
| UAC prompt per switch (`runas` of a helper process) | Violates "kaltstellen does not trigger UAC prompts" during a switch and is unusable with hotkeys. |
| Changing service ACLs once so the user may start/stop them | No helper needed at all, smallest code. But it permanently weakens the system per service, is hard to undo cleanly and invisible to the owner later. A candidate for very few services if the owner prefers it. |
| COM elevation moniker | Needs registration, per-call consent UI, more complex than a pipe. |

## Open questions for the owner

1. Is controlling services wanted in version 1 at all, and which services
   are the use case (this decides whether the ACL alternative is enough)?
2. Service account: `LocalSystem` (simple) or a virtual account with rights
   granted per allowed service (safer, more setup)?
3. Allow list maintained only from an elevated prompt, or also from the GUI
   with a UAC prompt at that moment?
4. Should a profile switch wait for a service to reach `STOPPED`/`RUNNING`
   (with a timeout like `stop_timeout_s`) or only send the request?
5. Code signing: is a certificate available? Signer checks make the client
   verification much stronger (R7).
