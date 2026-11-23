# Tray pipe protocol (version 1)

How `kaltstellen switch`, `recover` and `status` talk to a running
`kaltstellen tray` (spec L1, L5). The source of truth is
`src/tray/protocol.rs`; its unit tests pin the wire form shown here.

## Transport

- Named pipe `\\.\pipe\kaltstellen-<user SID>-<session id>`. With
  `KALTSTELLEN_DATA_DIR` set, `-<hash of that folder>` is appended, the same
  suffix the single-instance mutex `Local\kaltstellen-tray-<hash>` gets.
- Byte mode. Each message is one JSON object on one line, UTF-8, ended by
  `\n`, at most 64 KiB.
- Access: a protected DACL with one entry, full access for the user who runs
  the tray; remote clients are refused; the tray creates the first instance
  with `FILE_FLAG_FIRST_PIPE_INSTANCE` and does not start its pipe if the
  name is taken. The client checks that the pipe's server process runs as the
  same user before it sends anything, and connects at identification level.
- One request per connection. The client sends one request; the tray sends
  zero or more `accepted` replies, then exactly one final reply, and closes.
- Time limits: the request must arrive within 10 s of connecting, and each
  reply (and the final wait until the client has read everything) must be
  taken within 10 s; otherwise the tray drops the connection. A client that
  waits for a long switch is not affected: the limit applies to reading and
  writing, not to the time between replies.

## Requests

Every request has `version` (currently `1`) and `command`.

| `command` | Fields | Meaning |
| --- | --- | --- |
| `switch` | `profile` (string), `force` (bool, default `false`), `wait` (bool, default `true`) | `kaltstellen switch PROFILE [--force] [--no-wait]` |
| `recover` | `wait` (bool, default `true`) | `kaltstellen recover [--no-wait]` |
| `status` | none | `kaltstellen status` |
| `quit` | none | Used by `kaltstellen update` (spec UP1): Quit as from the menu (L2, everything suspended is resumed); the final reply `done` comes when that finished, then the tray ends. Added within version 1; a tray that does not know it answers `rejected` with `KS505`. The tray may end before the reply reaches the client, so the client also accepts a closed connection followed by the tray's mutex disappearing. |
| `quit_if_idle` | none | Used by the tray's automatic install (`kaltstellen update --unattended`, spec UP1): `quit`, but only if, on the tray's window thread when the request is read, `neutral` is active, no switch runs or waits and the tray is not quitting. Otherwise the final reply is `rejected` (exit code 1, `KS808`) and the tray keeps running; the install then gives up without changing anything. Added within version 1; a tray that does not know it answers `rejected` with `KS505`, which the install treats the same way, so an older tray never quits for it. |

```json
{"version":1,"command":"switch","profile":"focus","force":true,"wait":false}
{"version":1,"command":"status"}
{"version":1,"command":"quit"}
{"version":1,"command":"quit_if_idle"}
```

`force` belongs to the request: it confirms warn-list programs (P6) for this
switch only and travels with it through the queue.

## Replies

Every reply has `version` and `reply`.

| `reply` | Final | Fields | Command-line result |
| --- | --- | --- | --- |
| `accepted` | no | `waiting` (bool) | Request runs now (`false`) or waits for the running switch (`true`). A client with `wait = false` stops here (exit code 0). |
| `done` | yes | `exit_code`, `active_profile` (absent if nothing could be attempted), `notes`, `events` (`step`, `program`, `pid`), `errors`, `profiles` (for exit code 4) | Printed exactly like an in-process run. |
| `superseded` | yes | `by` (profile of the later request) | Exit code 3, `KS402` (W3). |
| `rejected` | yes | `exit_code`, `lines` | Lines on stderr, then that exit code: 2 for an invalid configuration, 5 while the tray quits or for a protocol error. |
| `status` | yes | `state` (as in `state.json`), `running`, `waiting`, `last_switch` | Printed like an in-process `status`, plus `tray: running …` / `tray: waiting …` lines. `last_switch` (added within version 1, absent when empty) is for the main window's failure markers (M5): `{profile, errors}`, the profile the last switch made active and its errors that name a program and are not resolved yet (L9; the tray checks again before it answers), each `{code, program, path?, text}`, at most 32. The command line does not print it. |

```json
{"version":1,"reply":"accepted","waiting":true}
{"version":1,"reply":"superseded","by":"y"}
{"version":1,"reply":"done","exit_code":0,"active_profile":"play","notes":[],"events":[{"step":"suspend","program":"a.exe","pid":4242}],"errors":[],"profiles":[]}
```

## Errors and versions

- A message that is not JSON, has no `version`, has another `version` or an
  unknown `command` is answered with `rejected`, exit code 5, `KS505`.
- A client that gets a reply of another version stops with `KS505`, exit
  code 5.
- If the tray runs (its mutex exists) but its pipe cannot be opened within
  5 s, or the connection breaks before the final reply, the command line
  ends with `KS501`, exit code 5. If no tray runs, the command works
  in-process.

A later version adds fields with defaults where it can; anything else gets a
new `version` number.
