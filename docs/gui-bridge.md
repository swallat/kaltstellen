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
| `copy_profile` | `from`, `to` | Copy with all entries, without the hotkey and without triggers. |
| `rename_profile` | `from`, `to` | Rename. |
| `delete_profile` | `name` | Delete; the active profile is first switched to `neutral` (M1), the deletion follows when that switch ended. |
| `set_hotkey` | `profile`, `hotkey` (string or `null`) | Set or remove; validated by the configuration rules (C5, C7). |
| `set_triggers` | `profile`, `triggers` (up to 64 strings: program names or full paths), `priority` (integer, default `0`) | U4: sets the trigger programs and the priority of the profile. An empty list removes `trigger`, priority `0` removes `priority`. Validated by the configuration rules: a trigger the profile itself stops or freezes, a program twice, a program that already triggers another profile and a priority outside the 32-bit range are refused. `copy_profile` does not copy triggers or priority. |
| `check_program` | `exe` | `data`: `{protection: "none" \| "warn" \| "hard", message?}`. |
| `add_entry` | `profile`, `exe`, `path?`, `confirmed?` | Add with the default action. Hard-protected programs are refused; warn-list programs need `confirmed: true`, which is then written. |
| `update_entry` | `profile`, `index`, `entry` | Replace the options of entry `index`: `entry` has `exe`, `path`, `action`, `args`, `cwd`, `on_timeout`, `restart_on_neutral`, `trim`, `confirmed`. Options the action does not take are dropped (C6). |
| `remove_entry` | `profile`, `index` | Remove the entry. |
| `pick_executable` | | Opens the file dialog; `data`: `{exe, path, protection}` or `null`. |
| `drop_files` | (files attached, see below) | M2: `.exe` files dropped from Explorer. `data`: `{files: [{exe, path, protection}], refused: [reason]}`. Nothing is added yet: the page adds each file with `add_entry` exactly as after `pick_executable` (warn list asks, hard-protected and duplicates are refused there). Refused here: more than 64 files, an item that is not a file, a name without a full path, a file not on a fixed local disk, a folder, a file that is not `.exe` (also shortcuts), a file that does not exist; a `drop_files` without files. |
| `drop_done` | `profile`, `added`, `not_added` (reasons) | M2: the summary of a drop becomes the status line. |
| `running` | | `data`: running programs `{exe, private_bytes, protection}`, largest first. |
| `compare` | `a`, `b` | H6: `data` is `{rows, meta}`: rows `{exe, path, a, b, difference}` with `difference` one of `only_a`, `only_b`, `action`, `options` or `null` (identical), in the order of A and then what only B has; `a` and `b` are the entries as text (`suspend · no trim`) or `null`; `meta` has `hotkey`, `triggers` and `priority` as `{a, b}` where they differ, else `null`. Reads the configuration only; the page copies or removes entries with `add_entry`, `update_entry` and `remove_entry`. Both profiles must exist. |
| `preview` | `profile` (or `neutral`) | H4: `data` is `{profile, items, problems, private_bytes}`: `items` in the order of W1, each `{step, program, outcome, text, processes, private_bytes}` with `step` one of `resume`, `stop`, `suspend`, `start` and `outcome` one of `will`, `no_change`, `problem`; `problems` are the errors the switch would report (P7, a missing file) as text; `private_bytes` is the private memory of what the stop and suspend steps would touch. Reads the processes and the saved state, opens each matching process with the rights the real step needs and closes it again; changes nothing. Runs on the window's thread like `running`. |
| `snapshot` | `mode` (`new` or `update`), `profile?` (with `update`) | H1: `data` is `{running, held, seen, existing}`; each row `{exe, path, private_bytes, protection, elevated, background, tray, held, first_seen, kind, suggestion, windows}` (`tray`: as for `unassigned`; H3: `kind` `{id, plural, label, known, source}` with `source` `name`, `description`, `folder`, `protection` or `none`; `suggestion` `{action, reason}` or `null`; `windows` `{count, dialog, unsaved, hung}` for running programs, read from window handles only, titles are looked at in the host and never sent). `running`: programs of the session, one row per name and image folder, largest first with those that have a window or notification-area icon before `background` ones; `held`: what the state file records as frozen (`held` is `suspend`) or stopped (`stop`); `seen`: programs of `seen.json` that do not run, newest first, at most 100. Hard-protected programs and kaltstellen itself are never listed; with `update`, programs the profile has an entry for are left out and counted in `existing`. `path` is read by opening each program once with query rights (closed at once); `elevated` is a program that could not be opened (P7), listed without path. Reads only: `seen.json` is not written. |
| `create_profile_from` | `name`, `entries`: up to 256 `{exe, path?, action, confirmed?, on_timeout?}` | H1: a new profile with these entries in one atomic save; refused as a whole when a name or entry breaks C1 to C8 or P6 (hard-protected, warn-list `suspend`/`stop` without `confirmed`). `confirmed` is written only for `suspend` and `stop`. |
| `add_entries` | `profile`, `entries` | H1 update mode: the same entries added to an existing profile in one atomic save; refused as a whole like `create_profile_from`. |
| `measure_start` | | H2: takes the first sample (process list with CPU time and I/O counters, TCP and UDP owner tables, the frozen records). Nothing is measured on a timer: the page asks when a dialog opens or the owner presses `Measure again`. |
| `measure` | `programs` (up to 256 executable names) | H2: takes the second sample and answers `{interval_ms, programs}` for the time since `measure_start` (refused when there was none, or less than 100 ms ago; the first sample is used up). Each program `{exe, frozen, private_bytes, cpu, io_per_s, tcp, udp, heavy}`: `cpu` a share of the whole CPU (0 to 1), `io_per_s` bytes per second over files, pipes and sockets together (never "network"), `tcp` established connections and `udp` endpoints owned, `heavy` the reason text (`1.3 GB memory, 12 % CPU`) or `null`; `cpu` and `io_per_s` are `null` for a frozen program. Programs are matched by executable name over the whole process list; a process in only one sample is left out of the deltas. No process is opened and no administrator right is needed. |
| `unassigned` | `profile` | `data`: `{exe, path, new, started, tray}` (N1; H9: `started` is a program that started while the profile was active and has no entry, noted by the tray in `seen.json`; such rows come first); `tray`: the program has an entry in the notification-area icon list of Windows 11 (registry `NotifyIconSettings`) and runs now; whether the icon shows at this moment is not known. |
| `dismiss` | `profile`, `exe` | Hide an unassigned program for this profile. |
| `suggestions` | `profile` | H5: the suggestions from use for the profile: `[{exe, path, action, reason, seen, of, entry}]` with `action` `stop`, `suspend` or `start` and `entry` the index of the entry it would change, else `null`, and `tray` as for `unassigned`. Reads `usage.json`, and for `tray` the process list and the icon list of the registry (no writes, no handles). Empty while `learn` is off. |
| `suggestion_done` | `profile`, `exe`, `never` | H5: after `Add entry` (`never` false) the counters of the program are reset; `Never for this program` (`never` true) is remembered in `usage.json`. `Not now` is the page's own: it hides the row until the window is opened again. |
| `set_learn` | `on` (bool) | H5: `Suggestions from use`; writes `learn` to `settings.json` (the tray and the command line read it at each switch). |
| `forget_learned` | | H5: deletes `usage.json`. |
| `apply` | `profile` | Start `kaltstellen switch PROFILE`; the result arrives as the `applied` event. One switch at a time. |
| `set_autostart` | `on` | Start at sign-in on or off (L8). |
| `set_automation` | `on` (bool) | U4: `Automatic switching` on or paused (moved from the tray menu, owner 2026-11-23). With a tray running, the host hands it over the pipe (`set_automation`), the tray takes it over at once and writes `triggers_paused` to `settings.json`; without one, or when the tray does not take it (an older tray, one that is quitting or does not answer), the host writes `settings.json` and the status line ends with `saved. The tray uses it when it starts.` or `saved. The running tray keeps its own setting until it restarts.` (an older tray, `KS505`) or `saved, but the running tray did not answer in time.`; after the older-tray case the window keeps showing the saved value with `automation.note`. A tray that took it but could not save it answers with a note, which ends the status line; the host writes nothing then. See "Settings and the running tray". |
| `set_update_mode` | `mode`: `"auto"`, `"manual"` or `"off"` | The update mode (UP1). Refused while `KALTSTELLEN_UPDATE` decides. Handed to the running tray like `set_automation` (pipe `set_update_mode`: the tray writes it and checks at once if the mode checks), else written to `settings.json` here, with the same endings of the status line. A mode that checks also starts the window's own check at once, as `check_updates` `[assumed]`, so the window can say what it found. |
| `check_updates` | | Runs `kaltstellen update --check --quiet` in the background (the one network access, UP1); the end arrives as the `update_checked` event. One check at a time. |
| `install_update` | | Opens a console window with `kaltstellen update`, as the tray's `Install update…`. Refused when no newer version is known or this copy cannot update itself. |
| `helper_status` | | Starts a background read of the optional background helper (M4; one at a time, it may take seconds when nothing answers). The result comes as the event `helper_status` with `data` `{state, answers, version, roles, notes}`: `not_installed`, `stopped`, `starting`, `running`, `stopping`, `other` or `unknown`; whether its pipe answered as the helper; its version; the roles switched on in its policy file; hints (service state unreadable, outdated helper, a pipe that is not the helper). Reads only. |
| `helper_install` | | Opens a console window with `kaltstellen helper install`, which asks Windows for administrator rights (UAC) and installs or updates the helper. The window itself stays unelevated; the request names no account (the helper serves the user who runs the window). |
| `helper_uninstall` | | The same with `kaltstellen helper uninstall`: stops and removes the helper. |
| `set_defaults` | `default_action` (`suspend`, `stop` or `ignore`), `trim`, `stop_timeout_s` (1 to 300), `minimize_before_suspend` | M6: writes the four keys of `[settings]`; unchanged keys keep how they are written, a missing key that would get its default stays out. Refused: `start`, an unknown action, a time outside 1 to 300. |
| `open_location` | `target`: `config_folder`, `log` or `log_folder` | M6: shows that place in Explorer. The host derives the path; the page cannot name one. A place that does not exist yet is an error. |
| `reload_config` | | M6: reads the configuration file again, for a change made by hand. A file that is not valid is refused and the window keeps what it had. |
| `forget_last_profile` | | M6: clears the profile of the last session in `settings.json` (the tray's `Apply last profile` offer, S5). |
| `icons` | `programs`: up to 256 `{exe, path?}`; `size`: 16 to 96 device pixels | `data`: one PNG `data:` URL or `null` per program, in the same order (M2). The file is the image of a running process of that name in this session (read with `PROCESS_QUERY_LIMITED_INFORMATION` only), else `path`, which is used only if an entry of the configuration has exactly that path: the page cannot make the host open a file of its choice. Only files on a fixed local disk are read (no shares, mapped network drives or device paths), so a slow server cannot hang the window and no credentials go out. The icon resource is read as data inside the window process (`SHDefExtractIconW`, which maps the file as a data file); no code of the file runs and no shell extension is asked. Kept per file and size while the window is open, at most 4 MiB. `null` (no file, no icon, not readable) is never an error; the page shows a placeholder of the same size. The page asks for the next common size at or above its icon size times `devicePixelRatio` and draws the list again when the scale changes, so icons stay sharp at every scaling; it splits requests to stay below the message size limit. The CSP needs nothing beyond `img-src data:`. |
| `program_info` | `programs`: up to 256 `{exe, path?}` | `data`: per program, in the same order, `null` (no file that may be read) or `{description, product, company, version, signature, publisher, catalog, problem, pending}` (M2, N1; owner request 2026-11-23, local sources only). The file is found exactly as for `icons` (a running process of that name in this session, else a configured `path`; fixed local disks only, checked before any file access). `description`, `product`, `company`, `version`: `FileDescription`, `ProductName`, `CompanyName`, `FileVersion` of the version resource, read as data (`GetFileVersionInfoW`), each from the first string table that has it: the tables of `\VarFileInfo\Translation` in order, then `040904B0`, `040904E4`, `04090000`, `000004B0`; control characters and bidirectional overrides removed, at most 200 characters; `null` when missing. They are what the file claims. `signature`: `valid`, `invalid` (a signature is there but does not verify; `problem` says why, for example `the file was changed after signing`), `unverified` (a signature is there, but whether it is valid cannot be decided from local data: an unknown root, a chain that needs certificates from the network, a revocation check that cannot run, a check the settings forbid; not a sign of tampering, shown neutrally), `unsigned`, `unknown` (not checked: `problem` says why, for example larger than 1024 MiB) or `pending`. `publisher`: the simple display name of the signer certificate, only for `valid`; `catalog`: the file is signed through a Windows catalog. The check is `WinVerifyTrust` (`WINTRUST_ACTION_GENERIC_VERIFY_V2`, no UI, no revocation check, URL retrieval from the local cache only, so no network access is requested `[assumed]`; Windows may still update its root and disallowed certificate lists on its own, which has no WinTrust flag, and manual check 13e verifies that no connection is made), first for a signature of the file's own, then for every system catalog that lists its SHA-256 hash, then (only if none does) its SHA-1 hash; the state is always closed. The host answers at once from a cache kept per file while the window is open (at most 1024 files, the finished ones looked up longest ago go first; a file whose size or time changed is read again); if the reader fails, what it had to read is answered as `unknown`; what is missing is read by one background thread, version resources first, then signatures one file after the other. `pending: true` means more is coming: the host sends the `program_info` event when it knows more (after the version resources of a batch, then at most every 0.3 s while it checks signatures, and when it is done), and the page asks again for what was pending. The page keeps finished answers until the state is refreshed, another profile is shown or `Add a program` opens (a running program is known to it by name only; the host finds the file again each time), and splits requests like `icons`. |

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

## Planned: profile assistance (H1 to H8, not built)

Requests the helpers of spec section 8 need. Names and fields are a proposal
**[assumed]** and may change when they are built.

| `type` | Fields | Effect |
| --- | --- | --- |
| `suggestions` | `profile` | H5: `data` the suggestions `{exe, path, action, reason}`. |
| `suggestion_answer` | `profile`, `exe`, `answer` (`add`, `later`, `never`), `action?` | H5: `add` writes an entry as `add_entry` does. |
| `templates` | | H7: `data` the shipped templates `{id, name, purpose, rules}`; `rules` is `[{kind, label, action}]` with `kind` the id of the kind in the snapshot rows (`kind.id`, H3), `action` `suspend`, `stop` or `ignore` (never `start`). The page applies the rules to the rows of a `snapshot` itself and counts the programs found; nothing is written by this request. |
| `set_learning` | `on` | H5: the `learn` key of `settings.json`. |
| `set_notify_new` | `on` | H9: the `notify_new` key of `settings.json` (`Tell me about new programs`); `state.notify_new` reports it. |
| `forget_learning` | | H5: deletes `usage.json`. |

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
| `default_action`, `default_trim`, `stop_timeout_s`, `minimize_before_suspend` | From `[settings]`. |
| `locations` | `{settings_file, data_dir, log_file, last_profile}` (M6): `settings.json`, the folder of the log, the log, and the profile that `settings.json` offers after sign-in (`null` when none). |
| `profiles` | `{name, hotkey, active, entries, triggers, priority}` (`triggers`: the trigger programs as written, U4); each entry `{exe, path, action, args, cwd, on_timeout, restart_on_neutral, trim, confirmed, protection, errors}`. `errors` (M5) lists the markers of the entry: `{code, label, text}`, for example `{"code": "KS110", "label": "start failed", "text": "KS110 [start] tool.exe: …"}`; empty when the entry is fine. See "Failure markers" below. |
| `autostart` | `true`, `false`, or `null` when unknown. |
| `learn` | H5: `Suggestions from use` is on (`settings.json`, default on). |
| `automation` | `{on, triggers, note}` (U4): `on` is false while automatic switching is paused, as the running tray reported it at the last refresh (see "Failure markers" for when that is), else as `settings.json` says (no tray, or a tray without triggers); `triggers`: some profile of the file names triggers. The page always shows the switch and disables it with a hint while `triggers` is false. `note`: `null`, or, after a change a running older tray did not take, `Saved; the running tray keeps its own setting until it restarts.`; `on` then shows the saved value until no tray runs or the tray reports it. |
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
text limits, a full path, on a fixed local disk, not a folder, the `.exe`
extension, an existing file, at most 64 per drop, then the same checks as
for a chosen file when each is added. The local-disk check comes first and
needs no file access (`[assumed]`, the rule of `icons`): a share, a mapped
or removable drive or a device path (`\\?\`, `\\.\`) is refused before
anything is read. An attached object that is not a file, or has no path,
is refused as "an item that is not a file". A runtime without
`AdditionalObjects` hands over no files, and the drop is refused with a
message.

The page then adds the files one after the other and sends
`drop_done` (`profile`, `added`, `not_added`: the reasons, at most 64) so
that the summary (`1 of 3 added to "focus". Not added: …`) becomes the
host's status line and survives a refresh of the state. If another profile
is selected while a drop is being added, the remaining files are not added
and are named in the summary. A refusal the page shows itself (a
protected program, a failed request) stays in the status line until the
next action of the user; a refresh on focus does not replace it.

`[assumed]`: the page takes every drag and drop (`dragover` and `drop`
call `preventDefault`), so a drop anywhere in the window never makes the
WebView open the file; the host's navigation guard (M6) would cancel the
`file:` navigation as well. Files are added to the profile shown, whether
they are dropped on its program list or elsewhere in the window, also
onto the open `Add a program` dialog (which then closes); while another
dialog is open a drop does nothing. The program list gets a dashed outline
while files are dragged over the window.

## Settings and the running tray

`Start at sign-in`, `Automatic switching` and the update mode are in the
window, not in the tray menu (owner, 2026-11-23). Start at sign-in is the
Run key itself, which the host writes and the tray reads when it needs it.
The other two are keys of `settings.json` that a running tray holds in
memory: the host sends them to the tray over the pipe (`set_automation`,
`set_update_mode`, [pipe-protocol.md](pipe-protocol.md)), and the tray
takes them over at once and writes the file, so it stays their one writer
while it runs. Without a tray the host writes the file. A tray that does
not take the setting (one from before these commands rejects them as
unknown) gets the file written by the host and keeps its own setting until
it restarts; the status line says so. A tray that quits or does not answer
in time also gets the file written by the host; the status line then says
that the value was saved but the tray did not answer in time. The host waits as for `status` below,
but up to 3 s for the answer to these two commands.
`[assumed]` (the mechanism; chosen over the tray re-reading the file,
because the effect is immediate and needs no watcher).

## Failure markers (M5)

The errors of the last switch are the tray's error state (spec L9); the
window does not keep its own. When the page asks for `get_state` (at start,
and when the window gets the focus again) and when a switch started from
the window has ended, the host asks the running tray for its `status` over
the pipe ([pipe-protocol.md](pipe-protocol.md), field `last_switch`); every
other reply reuses that answer, so a reply never waits for the tray. The
host does not wait for a tray that is still starting, waits at most 0.5 s
for a busy pipe and at most 1 s for the answer: at worst about 1.5 s on a
`get_state`, which the page sends at start, on focus at most every 2 s, and
after each switch from the window. `[assumed]`: only an answer replaces
the markers; when the tray does not answer in time the last known ones
stay. No tray running at all is an answer: no markers. The same answer
carries `automation.paused` for `Automatic switching`.

`[assumed]` rules:

- Only entries of the profile that the last switch made active
  (`last_switch.profile`) get markers; other profiles never do.
- An error belongs to an entry with the same executable name (any case).
  Where both the error and the entry have a path (a failed start knows its
  path), the paths must be the same too, so of two entries of one name with
  different paths (C2) only the failed one is marked. Errors without a
  program (state file, snapshot), failed resumes (`KS106`: programs of the
  profile left behind) and errors that name a child process under its own
  name stay in the status line and the tray menu only.
- Markers go when the tray drops the error: when its cause is gone (L9:
  the program runs again or no longer runs; the tray checks this before it
  answers) or when the next switch replaces the list. A failed start
  resolves only when the program runs from the path it was started from,
  so a program of the same name from another folder does not clear it.
- Without a running tray (`kaltstellen gui` alone, the switch runs
  in-process) there are no markers; the status line reports the switch as
  before.
- While the window is open but not focused, a switch from the tray menu or
  a hotkey does not reach it: markers and the active profile are brought
  up to date when the window gets the focus again (no timer).

## Events and notices

- Host to page: `{"v": 1, "event": "applied", "state": {}}` when a switch
  started by `apply` (or by deleting the active profile) has ended;
  `{"v": 1, "event": "update_checked", "state": {}}` when a check started by
  `check_updates` or `set_update_mode` has ended;
  `{"v": 1, "event": "program_info"}` (no state) when the host knows more
  about programs that a `program_info` reply gave as `pending` (M2).
- Page to host, without `id`: `{"v": 1, "type": "ready"}` once the first
  render is done; `{"v": 1, "type": "log", "message"}` for page errors (they
  go to the log file); `{"v": 1, "type": "drive_done", "results"}` at the end
  of a `--drive` run.

## Review and test options

`kaltstellen gui` has hidden options: `--screenshot FILE.png` captures the
page with WebView2's own capture once it is ready and exits; `--view
entry|add|confirm|new` opens that dialog first (`settings` opens the Settings view); `--theme light|dark` sets
the colour scheme; `--drive FILE.json` runs the JSON array of requests in
the file through the page's own bridge client (apply waits for its event;
a `program_info` step asks again until nothing is pending, at most 20 s,
and reports the `program_info` events seen as `events`; a step `{"type": "drop_real", "paths": [...]}` makes the host drop those
files from disk onto the page through the DevTools protocol
(`Input.dispatchDragEvent`: drag enter, over, drop), so the whole path of a
real drop runs; the paths are taken from the drive file read at start,
never from the page, which only names the step and the point, and only
while `--drive` runs. A step `{"type": "drop", "names": [...]}` sends a
drop event with files made up in the page, which WebView2 refuses to
attach as they are not on disk) and `--drive-out FILE.json` receives the
replies with the state after each. Review runs show the window without
taking the focus and end after at most 90 s.
