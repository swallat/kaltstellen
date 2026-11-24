# Manual checks

Things only a person at the screen can check. Automated tests cover the
logic and the processes; they cannot see icons, menus or windows. Use a test
configuration, not the real one, unless a step says otherwise:

```powershell
$env:KALTSTELLEN_CONFIG_DIR = "$env:TEMP\ks-manual\config"
$env:KALTSTELLEN_DATA_DIR = "$env:TEMP\ks-manual\data"
$env:KALTSTELLEN_RUN_KEY = "Software\kaltstellen-manual\Run"
```

Afterwards remove `%TEMP%\ks-manual` and `HKCU\Software\kaltstellen-manual`.

## Tray icon and menu

1. Copy `config.example.toml` to the test configuration folder, start
   `kaltstellen tray` from a terminal. Expect a grey window with mint icicles in
   the notification area (possibly in the overflow area), sharp at 100 %,
   150 % and 200 % scaling and on a second monitor with another scaling.
2. Hover: the tooltip reads `kaltstellen: neutral`.
3. Right click and left click: the menu opens near the icon and lists the
   profiles, `Neutral` (checked), `Open kaltstellen…`, `Quit`, and no
   settings: no `Start at sign-in`, no `Automatic switching`, no `Updates:`
   lines (they are in the main window, Settings). Clicking elsewhere closes it.
4. Choose a profile whose programs are not running: the icon turns violet,
   the tooltip names the profile, the profile is checked.
5. Make a switch fail (a `start` entry whose `path` does not exist yet):
   the icon turns orange with a warning triangle and the menu shows the error
   line; opening and closing the menu changes nothing. Now put the program
   at that path and start it by hand: within about 30 s (or when the menu
   opens) the error disappears and the icon is violet again, without a
   switch.
6. Break the configuration file (an unknown key): the icon turns orange with a warning triangle and the
   menu shows `Configuration invalid: …`; fix the file and open the menu
   again: the error is gone.
7. Restart Explorer (Task Manager, Windows Explorer, Restart): the icon comes
   back by itself.
8. Start `kaltstellen tray` by double-clicking the executable in Explorer: a
   console window may flash and must close by itself; the icon appears.
9. `Quit`: the icon disappears, the process ends.

## Start at sign-in question (L8)

1. With a fresh test configuration folder, start `kaltstellen tray`: a
   message box asks whether kaltstellen should start with Windows; the tray
   icon is already there and its menu works while the box is open.
2. Answer Yes: `kaltstellen autostart` reports `on` (test Run key). Quit and
   start again: no question.
3. With another fresh folder answer No: nothing is written; no question at
   the next start either.

## Hotkeys (T17)

1. Give a test profile `hotkey = "Ctrl+Shift+Alt+F9"`, start the tray, press the keys: the profile is applied (icon violet), once per press even when held down.
2. Start a second program that takes the same hotkey first (for example a second test tray with another data folder and the same hotkey), then the first tray: the menu lists `Hotkey not available: …`; the profile still works from the menu.
3. With a German layout and `hotkey = "Ctrl+Alt+7"` in a profile, type AltGr+7 in an editor while the tray runs: `{` must still appear.

## Main window (WP10, WebView2)

Start with the test configuration from above (copy `config.example.toml`).
Screenshots from the automated review are in `local/screenshots/` when the
main session made them; they show the views below in light and dark.

1. `kaltstellen gui`: a window titled `kaltstellen` with the program icon
   opens within about a second. Check it at 100 %, 150 % and 200 % scaling
   and after moving it to a monitor with another scaling: sharp, nothing
   cut off, no horizontal scroll bar.
2. Switch Windows between light and dark mode (Settings, Personalisation,
   Colours): the window follows.
3. Keyboard only: Tab reaches every button, field and table command in a
   sensible order, the focus ring is visible everywhere, Enter and Space
   press buttons, Escape closes a dialog, and focus returns to the window
   afterwards.
4. Profiles: `New`, `Rename`, `Copy`, `Delete` change the list; the
   configuration file keeps its comments (open it in an editor). An invalid
   name shows the reason in the dialog.
5. Hotkey: type `Ctrl+Alt+Banana`, `Set`: the field is marked and the status
   line explains; a valid one is saved.
6. `Edit` on an entry: the options change with the action (start: arguments
   and folder; stop: timeout and restart, with arguments when restart is
   ticked; suspend: trim). `Start` without a path is refused with a message.
7. `Add program…`: running programs with memory and protection; protected
   ones cannot be added; for a warn-list program (for example `anydesk.exe`
   via `Choose executable…`) a question appears; `Add anyway` adds it with
   `confirmed`.
8. `Apply profile`: the button shows `Applying…`, then the status line the
   result; with the tray running its icon changes too.
9. Make the selected profile active, then `Delete`: the question says it
   switches to Neutral first; afterwards the profile is gone.
10. Unassigned programs: start a program with a tray icon that has no entry;
    `Refresh`: it is listed as `new`; `Dismiss` removes it.
11. `Start at sign-in` in Settings, General, follows `kaltstellen autostart
    on|off` (use the test Run key from above; focus the window again to
    refresh). `Automatic switching` in the header is disabled with a hint (hover) while
    no profile has triggers; see "Automatic triggers" step 7.
12. Edit the file in an editor while the window is open, then make a change
    in the window: the status line says the file was changed and reloads it.
13. Right click in the page: no browser context menu (release build). Ctrl+P,
    F5, Ctrl+F do nothing (release build).
13a. Settings (foot of the sidebar): General, Defaults, Profiles, Updates,
    Files and About; the list on the left scrolls to a section without
    the page jumping, and About (the last one) can be scrolled back up.
    Defaults: `Stop timeout` 12 and Save write `stop_timeout_s = 12` to
    `config.toml` with the comments kept; 0 or more than 300 is refused
    with a reason and writes nothing. Updates: the installed version is
    shown; `Check now` shows `Checking…`, then `Up to date` with the time
    or the newer version (with network off: `Last check failed` and the
    reason). Changing the mode writes `settings.json` like the tray items;
    with `KALTSTELLEN_UPDATE` set the selector is disabled. With a newer
    version known, `Install <version>` opens the console window of the
    update. Files: the three paths; `Open config folder`, `Open log file`
    and `Open log folder` open Explorer or the editor; `Reload` reads the
    file again; `Forget last profile` removes `last_profile` from
    `settings.json`.
13b. Program icons (M2): next to each program in a profile, in `Add
    program` and in the unassigned list the program's icon appears, sharp
    at 100 %, 150 % and 200 % scaling and after moving the window to a
    monitor with another scaling (the lists are drawn again). A program
    on a network drive, a program without
    a path that does not run, or a file without an icon, shows a grey
    square of the same size; rows do not move when the icons arrive. The
    log (`kaltstellen.log`) has one line per request: `gui: N icons at S px
    (… found, … read) in … ms; cache … icons, … KiB`.
13c. Failure markers (M5), with the tray running: give a profile a `start`
    entry whose `path` does not exist and a `suspend` entry for a running
    program, apply it from the window. The start entry shows a red mark
    with `KS110 start failed`; hovering it shows the whole error as in the
    tray menu; the row has a red edge. Other entries and other profiles
    show nothing. Put a program at that path and start it by hand, then
    click into another window and back into kaltstellen: the marker is
    gone, as in the tray's error list. A program of the same name started
    from another folder does not clear it. A clean switch removes all
    markers. With
    a `suspend` entry whose freeze fails (an elevated program): its
    marker goes once that program is closed and the window gets the focus
    again. Switch profiles from the tray menu while the window is open but
    in the background: the window shows the change when you click into it.
    Without a tray (`kaltstellen gui` alone) there are no markers; the
    status line still reports the switch. Check in light and dark mode.
13d. Drag and drop (M2): drag one `.exe` from Explorer onto the open profile:
    while dragging, the program table gets a dashed outline; on drop the
    program is added with the default action, exactly as with `Choose
    executable…` (a warn-list program such as `anydesk.exe` asks first; a
    protected one such as `C:\Windows\explorer.exe` is refused with the
    reason in the status line; a program already in the profile is refused
    as a duplicate). Drop three `.exe` files at once: all three are added.
    Drop a `.txt` file, a shortcut (`.lnk`) and a folder together with one
    `.exe`: the `.exe` is added, the status line says `1 of 4 added …
    Not added: …` and names each refused item and why; click into another
    window and back: the summary is still there. Drop an `.exe` from a
    network share or a USB stick (removable): refused as not on a local disk
    (a USB hard disk that Windows reports as fixed counts as local). Drop
    onto the open `Add a program` dialog: the same, and the dialog
    closes. Drop anywhere else in the window (the sidebar, the
    header): no page change, nothing opens, the window stays on its page.
    Drop a link or selected text from a browser: nothing happens. Start
    the window elevated (`Run as administrator`) and drag from the normal
    Explorer: Windows refuses the drop (UIPI); that is expected.
13e. What a program is (M2, N1, owner request 2026-11-23, local sources
    only): in a profile, in `Add a program` and in the unassigned list,
    each program gets a small muted line under its name: the file
    description (else the product name), then `·` and the verified
    publisher in the primary colour with a check mark (`✓ Microsoft
    Windows` for `notepad.exe`, which is catalog-signed); for an unsigned
    program the company the file names, if any, and an italic `unsigned`;
    for a program whose signature does not verify (copy a signed program
    such as `msedge.exe` to a local folder and change one byte in the
    middle with a hex editor, then add that copy by path) `⚠ signature not
    valid` in the warning colour. Hovering the line shows description,
    product, company, version and the signature (with the reason when it
    is not valid, `through a Windows catalog` for catalog-signed files)
    and says that only the publisher is verified. A program without a
    version resource shows only `unsigned` (the test fixtures); a program
    on a network drive or not running without a path shows no line at
    all. The descriptions appear almost at once, the publishers a moment
    later, starting with the first rows (a large program such as a 400 MB
    Electron app takes up to about 1 s); the window stays responsive
    while it checks, including scrolling and opening dialogs. The log has
    `gui: program info for N programs (… known, … pending) in … ms`,
    `gui: version resources of N files in … ms` and `gui: N signatures in
    … ms, … waiting`. A program signed with a certificate whose root this
    computer does not know (a self-signed test signature made with
    `New-SelfSignedCertificate` and `Set-AuthenticodeSignature` in
    PowerShell, without importing the certificate) shows an italic, muted `signature not verifiable here`,
    not the warning colour; its tooltip says this is no sign of a change.
    Check in light and dark mode.
    No network access (the check requests none; Windows' own update of its
    root certificate lists has no switch, so this is checked by hand):
    a. In an elevated console, empty the URL cache of the certificate
       engine (`certutil -urlcache * delete`) and turn on logging of
       outgoing connections (`auditpol /set /subcategory:"Filtering
       Platform Connection" /success:enable`).
    b. Note the time, open the window and `Add a program`, wait until all
       publishers are shown, and run `kaltstellen list --info` and
       `kaltstellen status --unassigned`.
    c. Event Viewer, Windows Logs, Security, events 5156 since that time:
       none with application `kaltstellen.exe`. Note any connection of
       `svchost.exe` (CryptSvc) at that time: that would be Windows
       updating its certificate lists on its own. Turn the logging off
       again (`/success:disable`). Also with Wi-Fi off or the cable
       pulled the lists fill as fast as before, with the same results or
       at most `signature not verifiable here` for chains Windows had
       not cached.
    In a console: plain `kaltstellen list` looks as before (no text after
    the names, as fast as before); `kaltstellen list --info note` shows
    `Notepad, signed by Microsoft Windows` after `Notepad.exe`;
    `kaltstellen status --unassigned` shows the same kind of text after
    each program.
14. With the window open and minimised, choose `Open kaltstellen…` again
    (or run `kaltstellen gui`): the same window comes back to the front, no
    second window opens.
15. Tray menu `Open kaltstellen…` opens the window without a console window;
    closing it ends its process and the `msedgewebview2.exe` processes it
    started (Task Manager, Details, column "Command line" shows the data
    folder `kaltstellen\webview2`).

## Unassigned programs (T22)

1. Apply a test profile, then start a program with a tray icon that has no
   entry in it (for example a chat or sync tool).
2. Open the tray menu: `Unassigned programs (1)` (or more) appears; it opens
   the main window.
3. In the main window, select the profile: the program is listed as `new`.
   `Add to profile` adds it and it leaves the list; for another program,
   `Dismiss` removes it from the list of this profile only.

## Automatic triggers (U4, spec L12)

Automated tests use copies of a test program as triggers and post the
notification click as a window message; they cannot see the notification
or use a real program. Use a real program you can start and end at will
(for example `notepad.exe` or a game) as the trigger and another one as the
program the profile freezes:

```toml
[[profile]]
name = "trigger test"
trigger = ["notepad.exe"]

  [[profile.app]]
  exe = "<a program that runs, not notepad>"
```

1. `kaltstellen check-config` lists `trigger notepad.exe (priority 0)`. Add
   the trigger program as an entry of the same profile: `check-config`
   refuses it (`… is a program that this profile freezes`); remove it again.
2. Start the tray and open the main window: `Automatic switching` in the
   header is enabled and checked. The tray menu has no such item.
3. Start Notepad. Within about 5 s the profile is applied (violet icon, the
   tooltip ends in `(by trigger)`), and a notification appears:
   `kaltstellen: trigger test`, `notepad.exe started: switched to trigger
   test. Click here: Back to neutral`. The menu shows `Applied by trigger
   notepad.exe; back to neutral when it ends`.
4. The menu starts with `Back to neutral`. Click the notification while it
   is on screen (the only one): `neutral` is applied. The menu shows
   `Triggers wait after a manual switch …` and no `Back to` any more;
   nothing switches back while Notepad keeps running. Clicking a
   notification in the notification centre after it left the screen does
   nothing (the log says `notification clicked; nothing to go back to`):
   the click does not say which notification it was. Repeat with the menu
   item `Back to neutral` instead of the click: the same.
4a. Two notifications in quick succession: start Notepad, close it, start
   it again within the debounce plus a few seconds so two automatic
   switches follow each other (or use two trigger programs with different
   priorities). While both are on screen a click does nothing; the menu
   item `Back to …` names the profile of the latest one and works. Note
   the order in which the toasts appear and leave, and whether a toast
   that a newer one replaces is reported as hidden (the log of the tray
   shows `notification gone` only when the count reaches 0; a click on the
   remaining one afterwards should say whether it was taken as the only
   one).
4b. Do Not Disturb (Focus) on, or a full-screen game running: start
   Notepad. The profile is applied, no notification appears, the menu
   item `Back to neutral` works.
5. Close Notepad, wait 10 s, start it again: the profile is applied again.
   Close it: 5 to 10 s later `neutral` comes back, with a notification.
6. Start Notepad, switch to another profile from the menu, close Notepad:
   the other profile stays.
7. Uncheck `Automatic switching` in the main window: the status line says
   `Automatic switching paused.` and the tray menu shows the disabled line
   `Automatic switching paused` at once. Start Notepad: nothing happens.
   Quit and start the tray again, focus the window: the switch is still
   unchecked. Check it: Notepad counts as starting and the profile is
   applied. With the tray quit, uncheck it in the window: the status line
   says it is saved and the tray uses it when it starts; start the tray:
   paused.
8. With Focus assist (Do not disturb) on, the notification goes to the
   notification centre only; the switch still happens and `Back to` is in
   the menu (see 4b).
9. In the main window, the profile shows the section `Triggers` with
   `notepad.exe` and priority `0`; change the priority to `5`, `Set`: the
   file has `priority = 5` and keeps its comments; enter the program the
   profile freezes as a trigger: refused with the reason. The typed text
   stays in the field and the field is marked; click into another window
   and back: the text and the mark are still there. Enter `x` as the
   priority, `Set`: the message stays after clicking away and back.
10. Rename the profile in the main window while Notepad has applied it.
    The old name stays active (C4) until the next poll sees the trigger
    under the new name and applies the renamed profile, with a
    notification; close Notepad: the profile from before comes back. Note
    what the menu and the notifications say.
11. Sign out while the profile is applied by Notepad (VM): after sign-in
    the menu offers the profile from before the trigger (or nothing, if
    that was `neutral`), not `trigger test`.

## Updates (UP1)

Automated tests cover the check, the full update against a local server,
the rollback and choosing a mode that checks (through the running tray). Only a person
can check what is on screen, and only a real release can show the real
path. Until the first release exists, steps 1 to 3 work with a copy:
copy `target\release\kaltstellen.exe` to a folder of its own and run it
from there with the test variables above.

1. Main window, Settings, `Updates` section, with the tray running: `Mode` reads
   `Install automatically` by default; choosing `Check and ask` or `Off`
   shows `Update mode manual.` (or `off`) in the status line,
   `settings.json` gets `"update": "manual"` or `"off"`, and the tray log
   says `tray: update mode …` (for `manual` also `tray: update check
   started`). The tray menu has no mode items. With `KALTSTELLEN_UPDATE=off`
   set for the window, `Mode` is disabled with `Set by KALTSTELLEN_UPDATE.`
1a. Automatic install (after the first real release, with an older release
   in a normal folder): apply a profile and wait for the daily check (or
   choose `Install automatically` again in the main window to check now):
   nothing is
   installed while the profile is active. Switch to `Neutral`: the update
   installs without a window, the tray icon disappears briefly and comes
   back with the new version.
2. With an `update.json` in the test data folder that names a higher
   version (`{ "latest": "9.9.9" }`), the tray menu shows a disabled line
   `Update 9.9.9 available: …` for a development build or a copy below
   `scoop\apps`, and `Install update 9.9.9…` for a release build.
2a. With `{ "latest": "9.9.9", "failed": "9.9.9", "failed_at": <now>, "failures": 1 }`
   in that file (`<now>` in seconds since 1970) and a
   release build (or `KALTSTELLEN_UPDATE_URL` set): the tray menu item reads
   `Install update 9.9.9… (failed last time)`, and the `Updates`
   section of the main window says that version 9.9.9 could not be
   installed here last time and is tried again automatically after a date
   one day later (in mode `auto`); its `Install` button is still there.
3. Clicking `Install update…` opens a console window that runs the update;
   on a failure it stays open until Enter.
4. After the first real release: put the previous release in a normal
   folder, start its tray, apply a profile that freezes something, run
   `kaltstellen update`. Expect: the frozen program runs again, the console
   says `updated to version …`, one new tray icon appears (not two), its
   menu offers `Apply last profile`, `kaltstellen --version` shows the new
   version, `kaltstellen.old.exe` lies next to it. Then sign out and in:
   autostart starts the new version.
5. SmartScreen: the executable is not signed. Note whether Windows warns
   when the new tray starts.
6. Behind a proxy or a TLS-inspecting firewall, if available: the check
   works with the system's proxy settings.

## Optional background helper (M4a, design-optional-helper.md)

`scripts/helper-vm-check.ps1 -Exe <copy of kaltstellen.exe> -OnThisMachine` runs the automatable parts of this section and of the next two in one go from a normal prompt (two UAC prompts, install and uninstall) and prints a PASS/FAIL table. What it leaves to a person is listed in its header: another user account, killing the service process, the declined prompt, the tray and window, and watch mode.

On the VM, from a copy of the executable that is **not** in `Program Files`.
Nothing here is covered by CI because it installs a real service.

1. `kaltstellen helper status` before anything: `service: not installed`,
   exit code 0.
2. `kaltstellen helper install` from a normal prompt: a UAC prompt appears,
   a window reports `the background helper is installed and running`.
   Declining the prompt gives `KS1005`, nothing installed.
3. `services.msc`: `kaltstellen background helper`, running, automatic
   (delayed), log on as Local System. `sc qprivs kaltstellen-helper` lists
   only `SeChangeNotifyPrivilege`. `sc qfailure kaltstellen-helper` shows the
   restart actions.
4. `%ProgramFiles%\kaltstellen\kaltstellen.exe` exists and a normal user
   cannot change or delete it. `%ProgramData%\kaltstellen`: `helper.toml`
   and `kaltstellen.log` exist, a normal user can read but not write them
   (`icacls`).
5. `kaltstellen helper status` (normal prompt): running, the roles, the pipe
   answers with the same version.
6. Another user (a second account, or `runas`): `helper status` does not get
   an answer from the pipe (access denied is shown, nothing hangs).
7. Kill the service process in Task Manager: it restarts after about a
   minute. `sc stop kaltstellen-helper` ends it within seconds; the log says
   `helper stopping`.
8. Install again with a newer build: the service is replaced, `helper.toml`
   keeps its roles. `kaltstellen helper uninstall`: service, protected copy,
   policy and log are gone, and both folders if nothing else is in them.
9. The tray, the window and every command work unchanged throughout, with
   the helper installed, stopped and removed.

Result of the first run on the VM (2026-11-24, build 0f05fd9, as a standard
user, UAC answered with an administrator's credentials): steps 1 to 5, 7 and
8 passed; a standard user reads the service state and gets the pipe's answer.
Not yet covered: step 6 (a second account), the declined prompt (`KS1005`),
step 9 for the tray and window, and an install over an older build. The first
uninstall left the protected copy behind (the service process still held it);
the file is now removed with retries.

## Background helper: process events (M4b)

With the helper installed and running (see above), signed in as the owner:

1. `kaltstellen helper status` shows `observe (process events): running`.
2. Run `kaltstellen helper events` once; note the `next:` number printed on
   stderr. Start Notepad, close it, then run `kaltstellen helper events
   --after <next>`. Expected: a `start` line for `notepad.exe` (with `path`
   `C:\Windows\...`, a `parent` and a `session`) and later a `stop` line
   for the same `pid`.
3. A second user account signed in at the same time (fast user switching):
   its programs do not appear in your output.
4. Stop the service (`sc stop kaltstellen-helper` from an elevated prompt),
   start it again: the numbers start from 0, nothing from before is shown.
5. Set `observe = false` in `%ProgramData%\kaltstellen\helper.toml` from an
   elevated prompt, restart the service: `helper status` says `not
   running` and `helper events` is refused.
6. `kaltstellen helper status` also says `network counters: running`. Start a large
   download in a browser, then run `kaltstellen helper traffic --seconds 10`:
   the browser's process id shows a `down` of a similar size as the
   download; a second account's programs are not listed.
