# Main window bridge (version 1)

How the web page of `kaltstellen gui` talks to the Rust side (spec M6).
The source of truth is `src/gui/bridge.rs`; its unit tests pin the
behaviour. The page side is `ui/js/bridge.js`; `ui/js/mock.js` answers the
same messages in a normal browser.

## Transport

- WebView2 web messages: the page calls `chrome.webview.postMessage(object)`,
  the host answers with `PostWebMessageAsJson`, which the page receives as a
  `message` event.
- Only messages whose source is the embedded origin
  `https://ui.kaltstellen.invalid/` are accepted.
- Every message is a JSON object of at most 64 KiB with `"v": 1`.

## Requests (page to host)

Each request has `v`, a numeric `id` chosen by the page, and `type`. Unknown
fields are refused. Text fields are limited to 4096 characters without
control characters; paths must be absolute.

| `type` | Fields | Effect |
| --- | --- | --- |
| `get_state` | | Nothing; the reply carries the state. |
| `create_profile` | `name` | New empty profile. |
| `copy_profile` | `from`, `to` | Copy with all entries, without the hotkey. |
| `rename_profile` | `from`, `to` | Rename. |
| `delete_profile` | `name` | Delete; the active profile is first switched to `neutral` (M1), the deletion follows when that switch ended. |
| `set_hotkey` | `profile`, `hotkey` (string or `null`) | Set or remove; validated by the configuration rules (C5, C7). |
| `check_program` | `exe` | `data`: `{protection: "none" \| "warn" \| "hard", message?}`. |
| `add_entry` | `profile`, `exe`, `path?`, `confirmed?` | Add with the default action. Hard-protected programs are refused; warn-list programs need `confirmed: true`, which is then written. |
| `update_entry` | `profile`, `index`, `entry` | Replace the options of entry `index`: `entry` has `exe`, `path`, `action`, `args`, `cwd`, `on_timeout`, `restart_on_neutral`, `trim`, `confirmed`. Options the action does not take are dropped (C6). |
| `remove_entry` | `profile`, `index` | Remove the entry. |
| `pick_executable` | | Opens the file dialog; `data`: `{exe, path, protection}` or `null`. |
| `drop_files` | (files attached, see below) | M2: `.exe` files dropped from Explorer. `data`: `{files: [{exe, path, protection}], refused: [reason]}`. Nothing is added yet: the page adds each file with `add_entry` exactly as after `pick_executable` (warn list asks, hard-protected and duplicates are refused there). Refused here: more than 64 files, a name without a full path, a folder, a file that is not `.exe` (also shortcuts), a file that does not exist; a `drop_files` without files. |
| `running` | | `data`: running programs `{exe, private_bytes, protection}`, largest first. |
| `unassigned` | `profile` | `data`: `{exe, path, new}` (N1). |
| `dismiss` | `profile`, `exe` | Hide an unassigned program for this profile. |
| `apply` | `profile` | Start `kaltstellen switch PROFILE`; the result arrives as the `applied` event. One switch at a time. |
| `set_autostart` | `on` | Start at sign-in on or off (L8). |
| `set_update_mode` | `mode`: `"auto"`, `"manual"` or `"off"` | Saves the update mode in `settings.json` (UP1). Refused while `KALTSTELLEN_UPDATE` decides. A mode that checks starts a check at once, as `check_updates`. |
| `check_updates` | | Runs `kaltstellen update --check --quiet` in the background (the one network access, UP1); the end arrives as the `update_checked` event. One check at a time. |
| `install_update` | | Opens a console window with `kaltstellen update`, as the tray's `Install update…`. Refused when no newer version is known or this copy cannot update itself. |
| `icons` | `programs`: up to 256 `{exe, path?}`; `size`: 16 to 96 device pixels | `data`: one PNG `data:` URL or `null` per program, in the same order (M2). The file is the image of a running process of that name in this session (read with `PROCESS_QUERY_LIMITED_INFORMATION` only), else `path`, which is used only if an entry of the configuration has exactly that path: the page cannot make the host open a file of its choice. Only files on a fixed local disk are read (no shares, mapped network drives or device paths), so a slow server cannot hang the window and no credentials go out. The icon resource is read as data inside the window process (`SHDefExtractIconW`, which maps the file as a data file); no code of the file runs and no shell extension is asked. Kept per file and size while the window is open, at most 4 MiB. `null` (no file, no icon, not readable) is never an error; the page shows a placeholder of the same size. The page asks for the next common size at or above its icon size times `devicePixelRatio` and draws the list again when the scale changes, so icons stay sharp at every scaling; it splits requests to stay below the message size limit. The CSP needs nothing beyond `img-src data:`. |

Every change is validated with `config::parse` and saved atomically at once;
a refused change leaves the file as it was. A change that the configuration
rules would only warn about and then ignore (a hotkey that takes AltGr) is
refused too. A change that cannot be written (the file is read-only or held
open) is taken back, so `state` shows the file as it is. While a deleted
active profile waits for the switch to `neutral`, profile and entry edits
are refused; `delete_profile` is refused while a switch started by `apply`
runs. Edits keep the comments and the layout of the keys they do not
change, the line endings and a byte order mark of the file; entries written
inline (`app = [{ exe = "a.exe" }]`) are turned into `[[profile.app]]`
tables by the first edit of that profile.

## Replies (host to page)

```json
{"v": 1, "id": 7, "ok": true, "state": {}, "data": {}}
{"v": 1, "id": 8, "ok": false, "error": "KS205: …", "state": {}}
```

`state` always describes the file as it is now:

| Field | Meaning |
| --- | --- |
| `config_file` | Path of the configuration file. |
| `active` | Active profile (from `state.json`). |
| `default_action`, `default_trim` | From `[settings]`. |
| `profiles` | `{name, hotkey, active, entries}`; each entry `{exe, path, action, args, cwd, on_timeout, restart_on_neutral, trim, confirmed, protection, errors}`. `errors` (M5) lists the markers of the entry: `{code, label, text}`, for example `{"code": "KS110", "label": "start failed", "text": "KS110 [start] tool.exe: …"}`; empty when the entry is fine. See "Failure markers" below. |
| `autostart` | `true`, `false`, or `null` when unknown. |
| `applying` | A switch runs. |
| `status` | The status line. |
| `update` | `{version, mode, mode_locked, available, blocked, failed, retry_at, last_check, error, checking}`: the installed version; `auto`, `manual` or `off`; whether `KALTSTELLEN_UPDATE` decides; the newer version found by the last check or `null`; why this copy cannot install it (package manager, Program Files, development build) or `null`; whether that version could not be installed here last time (`install_update` still installs it); when the automatic install tries it again (seconds since 1970) or `null`; seconds since 1970 of the last check or `null`; why the last check failed or `null`; whether a check started from the window runs. |

## Dropped files (M2)

The page sees a dropped file only by name. It sends `drop_files` with
`chrome.webview.postMessageWithAdditionalObjects(message, files)`; the host
reads the path of each attached object
(`ICoreWebView2WebMessageReceivedEventArgs2::AdditionalObjects`,
`ICoreWebView2File::Path`) and hands the paths to the bridge beside the
message. The message itself cannot carry paths (unknown fields are
refused), and only `drop_files` may have attached files; any other request
with files is refused. The paths are untrusted like every other input:
text limits, a full path, an existing file, the `.exe` extension, at most
64 per drop, then the same checks as for a chosen file when each is added.
An attached object that is not a file, or has no path, counts as a file
without a full path. A runtime without `AdditionalObjects` hands over no
files, and the drop is refused with a message.

`[assumed]`: the page takes every drag and drop (`dragover` and `drop`
call `preventDefault`), so a drop anywhere in the window never makes the
WebView open the file; the host's navigation guard (M6) would cancel the
`file:` navigation as well. Files are added to the profile shown, whether
they are dropped on its program list or elsewhere in the window, also
onto the open `Add a program` dialog (which then closes); while another
dialog is open a drop does nothing. The program list gets a dashed outline
while files are dragged over the window.

## Failure markers (M5)

The errors of the last switch are the tray's error state (spec L9); the
window does not keep its own. When the page asks for `get_state` (at start,
and when the window gets the focus again) and when a switch started from
the window has ended, the host asks the running tray for its `status` over
the pipe ([pipe-protocol.md](pipe-protocol.md), field `last_switch`); every
other reply reuses that answer, so a reply never waits for the tray. The
host does not wait for a tray that is still starting and waits at most
0.5 s for a busy pipe; without an answer there are no markers.

`[assumed]` rules:

- Only entries of the profile that the last switch made active
  (`last_switch.profile`) get markers; other profiles never do.
- An error belongs to an entry with the same executable name (any case).
  Where both the error and the entry have a path (a failed start knows its
  path), the paths must be the same too, so of two entries of one name with
  different paths (C2) only the failed one is marked. Errors without a
  program (state file, snapshot) and errors that name a child process
  under its own name stay in the status line and the tray menu only.
- Markers go when the tray drops the error: when its cause is gone (L9:
  the program runs again or no longer runs; the tray checks this before it
  answers) or when the next switch replaces the list. Without a running
  tray (`kaltstellen gui` alone, the switch runs in-process) there are no
  markers; the status line reports the switch as before.

## Events and notices

- Host to page: `{"v": 1, "event": "applied", "state": {}}` when a switch
  started by `apply` (or by deleting the active profile) has ended;
  `{"v": 1, "event": "update_checked", "state": {}}` when a check started by
  `check_updates` or `set_update_mode` has ended.
- Page to host, without `id`: `{"v": 1, "type": "ready"}` once the first
  render is done; `{"v": 1, "type": "log", "message"}` for page errors (they
  go to the log file); `{"v": 1, "type": "drive_done", "results"}` at the end
  of a `--drive` run.

## Review and test options

`kaltstellen gui` has hidden options: `--screenshot FILE.png` captures the
page with WebView2's own capture once it is ready and exits; `--view
entry|add|confirm|new` opens that dialog first; `--theme light|dark` sets
the colour scheme; `--drive FILE.json` runs the JSON array of requests in
the file through the page's own bridge client (apply waits for its event;
a step `{"type": "drop", "names": [...]}` sends a drop event with files
made up in the page through the page's drop handler; WebView2 refuses to
attach such files, as they are not on disk, so a real drop stays a manual
check)
and `--drive-out FILE.json` receives the replies with the state after each. Review runs show the
window without taking the focus and end after at most 90 s.
