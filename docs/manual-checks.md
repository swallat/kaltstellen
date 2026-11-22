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
