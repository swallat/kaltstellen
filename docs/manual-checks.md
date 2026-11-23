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
   `kaltstellen tray` from a terminal. Expect a blue tile with a white K in
   the notification area (possibly in the overflow area), sharp at 100 %,
   150 % and 200 % scaling and on a second monitor with another scaling.
2. Hover: the tooltip reads `kaltstellen: neutral`.
3. Right click and left click: the menu opens near the icon and lists the
   profiles, `Neutral` (checked), `Start at sign-in`, `Open kaltstellen…`,
   `Quit`. Clicking elsewhere closes it.
4. Choose a profile whose programs are not running: the icon turns orange,
   the tooltip names the profile, the profile is checked.
5. Make a switch fail (a `start` entry whose `path` does not exist yet):
   the icon turns red with an exclamation mark and the menu shows the error
   line; opening and closing the menu changes nothing. Now put the program
   at that path and start it by hand: within about 30 s (or when the menu
   opens) the error disappears and the icon is orange again, without a
   switch.
6. Break the configuration file (an unknown key): the icon turns red and the
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

1. Give a test profile `hotkey = "Ctrl+Shift+Alt+F9"`, start the tray, press the keys: the profile is applied (icon orange), once per press even when held down.
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
11. `Start at sign-in` in the header follows the tray's menu item (use the
    test Run key from above).
12. Edit the file in an editor while the window is open, then make a change
    in the window: the status line says the file was changed and reloads it.
13. Right click in the page: no browser context menu (release build). Ctrl+P,
    F5, Ctrl+F do nothing (release build).
13a. Updates (sidebar): the installed version is shown; `Check now` shows
    `Checking…`, then `Up to date` with the time or the newer version (with
    network off: `Last check failed` and the reason). Changing the mode
    writes `settings.json` like the tray items; with `KALTSTELLEN_UPDATE`
    set the selector is disabled. With a newer version known, `Install
    <version>` opens the console window of the update.
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
    gone, as in the tray's error list. A clean switch removes all
    markers. Without a tray (`kaltstellen gui` alone) there are no markers;
    the status line still reports the switch. Check in light and dark mode.
13d. Drag and drop (M2): drag one `.exe` from Explorer onto the open profile:
    while dragging, the program table gets a dashed outline; on drop the
    program is added with the default action, exactly as with `Choose
    executable…` (a warn-list program such as `anydesk.exe` asks first; a
    protected one such as `C:\Windows\explorer.exe` is refused with the
    reason in the status line; a program already in the profile is refused
    as a duplicate). Drop three `.exe` files at once: all three are added.
    Drop a `.txt` file, a shortcut (`.lnk`) and a folder together with one
    `.exe`: the `.exe` is added, the status line names each refused item
    and why. Drop onto the open `Add a program` dialog: the same, and the
    dialog closes. Drop anywhere else in the window (the sidebar, the
    header): no page change, nothing opens, the window stays on its page.
    Drop a link or selected text from a browser: nothing happens. Start
    the window elevated (`Run as administrator`) and drag from the normal
    Explorer: Windows refuses the drop (UIPI); that is expected.
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

## Updates (UP1)

Automated tests cover the check, the full update against a local server,
the rollback and the menu item that switches the check on. Only a person
can check what is on screen, and only a real release can show the real
path. Until the first release exists, steps 1 to 3 work with a copy:
copy `target\release\kaltstellen.exe` to a folder of its own and run it
from there with the test variables above.

1. Tray menu: `Updates: install automatically` is checked by default;
   choosing `check and ask` or `off` moves the check mark and
   `settings.json` gets `"update": "manual"` or `"off"`. With
   `KALTSTELLEN_UPDATE=off` set, the three lines read
   `(set by KALTSTELLEN_UPDATE)` and are disabled.
1a. Automatic install (after the first real release, with an older release
   in a normal folder): apply a profile and wait for the daily check (or
   choose `install automatically` again to check now): nothing is
   installed while the profile is active. Switch to `Neutral`: the update
   installs without a window, the tray icon disappears briefly and comes
   back with the new version.
2. With an `update.json` in the test data folder that names a higher
   version (`{ "latest": "9.9.9" }`), the menu shows a disabled line
   `Update 9.9.9 available: …` for a development build or a copy below
   `scoop\apps`, and `Install update 9.9.9…` for a release build.
2a. With `{ "latest": "9.9.9", "failed": "9.9.9", "failed_at": <now>, "failures": 1 }`
   in that file (`<now>` in seconds since 1970) and a
   release build (or `KALTSTELLEN_UPDATE_URL` set): the menu item reads
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
