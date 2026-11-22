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
5. Make a switch fail (a `start` entry with a missing `path`): the icon
   turns red with an exclamation mark; the menu shows the error line and
   `Clear error state`. Choose it: the icon is orange again.
6. Break the configuration file (an unknown key): the icon turns red and the
   menu shows `Configuration invalid: …`; fix the file and open the menu
   again: the error is gone.
7. Restart Explorer (Task Manager, Windows Explorer, Restart): the icon comes
   back by itself.
8. Start `kaltstellen tray` by double-clicking the executable in Explorer: a
   console window may flash and must close by itself; the icon appears.
9. `Quit`: the icon disappears, the process ends.

## Hotkeys (T17)

1. Give a test profile `hotkey = "Ctrl+Shift+Alt+F9"`, start the tray, press the keys: the profile is applied (icon orange), once per press even when held down.
2. Start a second program that takes the same hotkey first (for example a second test tray with another data folder and the same hotkey), then the first tray: the menu lists `Hotkey not available: …`; the profile still works from the menu.
3. With a German layout and `hotkey = "Ctrl+Alt+7"` in a profile, type AltGr+7 in an editor while the tray runs: `{` must still appear.

## Main window (WP10)

Start with the test configuration from above (copy `config.example.toml`).

1. `kaltstellen gui`: a window titled `kaltstellen` opens within a second,
   sharp at 100 %, 150 % and 200 % scaling; text is not cut off in buttons,
   labels and column headers. Move it to a monitor with another scaling:
   it stays sharp and laid out.
2. Tab moves between controls; Alt plus the underlined letter presses a
   button.
3. Select a profile: its entries appear with action and options. Type a
   name and press `New`, `Copy`, `Rename`: the list follows and the file
   keeps its comments (open it in an editor).
4. Select an entry, choose another action, `Set action`: the row changes.
   Choose `start` for an entry without a path: the status line refuses it.
5. `Add executable…`: the file dialog opens on `.exe` files; the chosen one
   appears with its path.
6. In the running programs, select `explorer.exe`, `Add to profile`: the
   status line refuses it. Select a remote-access tool if one runs (or add
   `anydesk.exe` by executable): the window asks; `No` adds nothing, `Yes`
   adds it with `confirmed`.
7. `Apply profile`: the status line reports the result; with the tray
   running its icon changes too.
8. Make the selected profile the active one, then `Delete`: after the
   question the window switches to neutral first and then deletes it.
9. Edit the file in an editor while the window is open, then make a change
   in the window: the status line says the file was changed and reloads it.
10. Tray menu `Open kaltstellen…` opens the window without a console
    window; closing the window ends its process.

## Unassigned programs (T22)

1. Apply a test profile, then start a program with a tray icon that has no
   entry in it (for example a chat or sync tool).
2. Open the tray menu: `Unassigned programs (1)` (or more) appears; it opens
   the main window.
3. In the main window, select the profile: the program is listed as `new`.
   `Add to profile` adds it and it leaves the list; for another program,
   `Dismiss` removes it from the list of this profile only.
