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

## Requests

Every request has `version` (currently `1`) and `command`.

| `command` | Fields | Meaning |
| --- | --- | --- |
| `switch` | `profile` (string), `force` (bool, default `false`), `wait` (bool, default `true`) | `kaltstellen switch PROFILE [--force] [--no-wait]` |
| `recover` | `wait` (bool, default `true`) | `kaltstellen recover [--no-wait]` |
| `status` | none | `kaltstellen status` |

```json
{"version":1,"command":"switch","profile":"focus","force":true,"wait":false}
{"version":1,"command":"status"}
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
| `status` | yes | `state` (as in `state.json`), `running`, `waiting` | Printed like an in-process `status`, plus `tray: running …` / `tray: waiting …` lines. |

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
