<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="branding/wortmarke/logo-wortmarke-dunkel.svg">
    <img src="branding/wortmarke/logo-wortmarke-hell.svg" alt="kaltstellen" height="72">
  </picture>
</h1>

<p align="center">
  <em>kaltstellen</em> (German): to put something on ice.
</p>

<p align="center">
  <a href="https://github.com/swallat/kaltstellen/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/swallat/kaltstellen/ci.yml?branch=main&label=CI&labelColor=12324B" alt="CI status"></a>
  <img src="https://img.shields.io/badge/status-prototype-FF8A1F?labelColor=12324B" alt="Status: prototype">
  <img src="https://img.shields.io/badge/platform-Windows-1593AF?labelColor=12324B" alt="Platform: Windows">
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-1593AF?labelColor=12324B" alt="License: MIT or Apache-2.0"></a>
</p>

A small Windows tool that switches between app profiles to free up CPU and RAM.
Start the flight simulator and its companion tools come up while the music
software is frozen or closed; switch to the music profile and it is the other
way round.

**Status: early.** Profiles work from the command line:
`kaltstellen switch <profile>` starts, stops and freezes programs as the
configuration says and undoes it on the next switch. A first tray icon
(`kaltstellen tray`) switches profiles from its menu. Hotkeys and the
settings window are planned; see [docs/spec.md](docs/spec.md) and
[docs/plan.md](docs/plan.md).

## Using it

Copy [config.example.toml](config.example.toml) to
`%APPDATA%\kaltstellen\config.toml` and describe your profiles there.

```text
kaltstellen check-config [FILE]   validate the configuration
kaltstellen switch PROFILE        switch to a profile; "neutral" brings
          [--force] [--no-wait]   everything back
kaltstellen status [--unassigned] show the active profile and what is on ice;
                                  --unassigned also lists running programs
                                  without an entry in it, with what each is
kaltstellen recover [--no-wait]   resume everything frozen, e.g. after a crash
kaltstellen quit                  end the running tray as its menu's Quit does
kaltstellen tray                  notification-area icon with the profile menu
kaltstellen autostart [on|off]    start the tray at sign-in (per-user Run key)
kaltstellen automation [on|off]   show, resume or pause automatic switching by
                                  trigger programs (taken over by a running
                                  tray at once)
kaltstellen gui                   window to edit profiles and their programs
                                  (needs the WebView2 runtime of Windows 11)

kaltstellen list [FILTER]         list running processes; --info adds what
          [--info]                each program is (description, publisher)
kaltstellen suspend NAME [--trim] freeze a program by hand: all processes with
                                  that executable name and their children
kaltstellen resume NAME           continue them
```

- A switch runs in a fixed order: resume, stop, suspend, start. A step that
  fails does not stop the rest; errors are listed at the end (exit code 1).
- What was frozen or closed is written to
  `%LOCALAPPDATA%\kaltstellen\state.json` before it happens. If kaltstellen
  is killed, `kaltstellen recover` brings everything back.
- Remote-access tools, graphics driver components, licence services,
  security software and shells are only touched if their entry has
  `confirmed = true`, or with `--force` for one switch.
- With `kaltstellen tray` running, `switch`, `recover` and `status` are
  handed to the tray and wait for it (same output and exit codes);
  `--no-wait` returns once the tray has queued the request. A request that a
  later one replaces while it waits ends with exit code 3.
- Only processes of your own Windows session are touched.
- What a program is comes from its own file only, with no network access:
  the description from its version resource (which the file can claim
  freely) and the publisher from its signature, shown only when the
  signature is valid (`list --info`: `Notepad, signed by Microsoft Windows`;
  `Tool (Acme), unsigned`). The main window shows the same under each
  program's name.
- A suspended process uses no CPU and keeps its state.
- `--trim` also asks Windows to move the process's memory out of RAM. Because a
  frozen process never touches its pages, they stay out until it is resumed.
- Core system processes (`explorer.exe`, `dwm.exe`, `csrss.exe`, ...) are
  refused.

`suspend` and `resume` by hand keep no state: every `suspend` needs exactly
one `resume`. Profiles do keep state and freeze each process once.

Not every app survives being frozen. Programs with live network connections,
audio drivers or hardware access may lose their connection or hang after
resuming. Give such programs `action = "stop"` in their profile.

## Testing your own apps

`scripts/Test-SuspendCompat.ps1` (PowerShell 7) goes through the apps running
in your session one at a time: suspend, wait, resume, check that the app is
still alive and responding. An app that does not survive is closed completely
and started again.

```powershell
./scripts/Test-SuspendCompat.ps1        # list what would be tested
./scripts/Test-SuspendCompat.ps1 -Run   # run the test
```

Save your work first: an app that fails the test is killed and restarted.
Apps you do not want touched go into `local/compat-exclude.txt` or the
`-Exclude` parameter; `Get-Help ./scripts/Test-SuspendCompat.ps1 -Full` lists
all options.

Results land in `local/compat-results/`. The whole `local/` folder is ignored
by Git, because results, exclude lists and personal profiles describe one
particular machine.

## Build

Requires Windows, Git, rustup and the MSVC build tools with the Windows SDK
(WebView2 is part of Windows 11). rustup installs the Rust version fixed in
`rust-toolchain.toml` by itself on the first build:

```powershell
winget install Rustlang.Rustup
winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install Git.Git

git clone https://github.com/swallat/kaltstellen.git
cd kaltstellen
cargo build --release --locked
```

To try a development build without touching the real configuration,
`scripts\dev-run.ps1` pulls, builds and starts the tray with its
configuration, data and start-at-sign-in key in a sandbox (`%TEMP%\ks-dev`):

```powershell
.\scripts\dev-run.ps1            # pull, build, start the tray in the sandbox
.\scripts\dev-run.ps1 -Gui       # the main window instead
.\scripts\dev-run.ps1 -Recover   # resume everything it froze
.\scripts\dev-run.ps1 -Clean     # end the sandbox tray, remove the sandbox
```

A development build reports `0.0.0-dev+<commit>` and does not update itself.

The main window is a web page in `ui/` shown with WebView2 and embedded
into the executable; [ui/README.md](ui/README.md) explains how to work on its
design in a normal browser without building Rust.

The binary is `target\release\kaltstellen.exe`. Freezing processes that run
elevated or under another user requires an elevated terminal.

## The tray icon

`kaltstellen tray` puts an icon in the notification area. Its menu holds
actions only: the profiles of the configuration (the active one checked),
`Neutral`, lines that say what went wrong or what automation does, an
`Install update <version>…` offer when there is one, `Open kaltstellen…`
and `Quit`; the tooltip names the active profile. Settings (start at
sign-in, automatic switching, the update mode) are in the main window. It
runs once per session, and on start it resumes anything a crashed instance
left frozen. `Quit` resumes everything frozen and leaves closed programs
closed. When you sign out or shut down, it resumes everything first and
offers the last profile again after the next sign-in (`Apply last profile`
in the menu); it never applies it by itself. On its first start it asks
once whether it should start at sign-in; later, `Start at sign-in` in the
main window or `kaltstellen autostart on|off` changes the per-user Run key. Started from a terminal
it keeps that terminal until `Quit`; started from a shortcut it lets go of
its console window at once, which may still show for a moment. While it
runs, the command line goes through it. One tile, three states:

| | State | Meaning |
| :-: | --- | --- |
| <img src="branding/tray/png/kaltstellen-neutral-32.png" width="32" height="32" alt="Cold ice cube with two pause bars"> | Neutral | No profile active, nothing on ice |
| <img src="branding/tray/png/kaltstellen-aktiv-32.png" width="32" height="32" alt="Ice cube with a warm right side"> | Profile active | Apps outside the profile are frozen or closed |
| <img src="branding/tray/png/kaltstellen-fehler-32.png" width="32" height="32" alt="Red cube with an exclamation mark"> | Error | Something could not be frozen, resumed or started |

### Automatic triggers

A profile can name trigger programs: `trigger = ["simulator.exe"]` (names
or full paths) and optionally `priority = 10`. While the tray runs, it looks
for them every 5 s. When one starts, its profile is applied and a
notification says so; `Back to …` at the top of the menu (or a click on the
notification while it is on screen) goes back. About 5 to 10 s after the last
one has ended, the profile that was active before comes back, unless you
switched yourself in between. A switch of your own always wins: automation
waits until a trigger program starts or ends. When triggers of two profiles
run, the higher priority wins. `Automatic switching` in the main window (or
`kaltstellen automation off`) pauses all triggers at once, also in the
running tray, and is remembered across restarts. A trigger must not be frozen or
closed by its own profile; `check-config` refuses that.

Logo, wordmark, colours and icons live in [branding/](branding/), together
with the scripts that generate them. The notes there are in German.

## Updates

There are no releases yet; this describes how updates will work once there
are. kaltstellen updates itself from the
[GitHub releases](https://github.com/swallat/kaltstellen/releases) of this
repository:

```powershell
kaltstellen update --check   # is there a newer version?
kaltstellen update           # install it
```

Installing downloads the executable for your system and `checksums.txt` of
the same release, refuses the file if its SHA-256 differs, and tries the new
file (`--version`) before anything else changes. A running tray is quit
first, which resumes every frozen program; then the executable is replaced
and a new tray started. Its menu offers the profile that was active
(`Apply last profile`); nothing is frozen again by itself. If the new tray
does not start, the previous version is put back and started. The previous
executable stays next to the new one as `kaltstellen.old.exe` until the next
update, so you can go back by renaming it.

The tray looks for a new version once a day (first five minutes after it
starts). By default it installs a newer version by itself, but only while
`neutral` is active and no switch is running or waiting, so nothing you
froze is thawed; otherwise it waits for the next time you are back on
`neutral`. This check is the only network access kaltstellen makes: one
HTTPS request to the GitHub API, nothing about you or your configuration is
sent. Choose the mode with any of:

- the `Updates` section of the main window: `Install automatically`,
  `Check and ask` (the tray menu then shows `Install update <version>…` and
  installs only on click) or `Off`,
- `kaltstellen update --mode auto|manual|off`,
- the environment variable `KALTSTELLEN_UPDATE=auto|manual|off`, which wins
  over the others.

A running tray takes a mode chosen in the window or on the command line over
at once, and a mode that checks checks right away.

With the check off, nothing connects unless you run `kaltstellen update`.
A copy installed by scoop, winget or Chocolatey, or placed below Program
Files or in a folder you cannot write to, does not replace itself; it says
how to update instead. Builds of your own (`cargo build`) do not update
themselves either.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work, as defined in the Apache-2.0 license, shall be dual
licensed as above, without any additional terms or conditions.
