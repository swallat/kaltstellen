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
  <img src="https://img.shields.io/badge/platform-Windows-2A8FD6?labelColor=12324B" alt="Platform: Windows">
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-2A8FD6?labelColor=12324B" alt="License: MIT or Apache-2.0"></a>
</p>

A small Windows tool that switches between app profiles to free up CPU and RAM.
Start the flight simulator and its companion tools come up while the music
software is frozen or closed; switch to the music profile and it is the other
way round.

**Status: early prototype.** Today it is a command-line tool that can freeze,
resume and trim processes by name. Profiles, the tray icon and automatic
switching are planned; see [docs/requirements.md](docs/requirements.md) and
[docs/plan.md](docs/plan.md).

## What the prototype does

```text
kaltstellen list [FILTER]         list running processes
kaltstellen suspend NAME [--trim] freeze all processes with that executable name
kaltstellen resume NAME           continue them
```

- A suspended process uses no CPU and keeps its state.
- `--trim` also asks Windows to move the process's memory out of RAM. Because a
  frozen process never touches its pages, they stay out until it is resumed.
- Core system processes (`explorer.exe`, `dwm.exe`, `csrss.exe`, ...) are
  refused.

The prototype keeps no state: every `suspend` needs exactly one `resume`.
Suspending the same app twice means resuming it twice.

Not every app survives being frozen. Programs with live network connections,
audio drivers or hardware access may lose their connection or hang after
resuming. Use the prototype to find out which of your apps tolerate it.

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

Requires Windows and a stable Rust toolchain.

```powershell
cargo build --release
```

The binary is `target\release\kaltstellen.exe`. Freezing processes that run
elevated or under another user requires an elevated terminal.

## The tray icon (planned)

The tray app does not exist yet, but its icon does. One tile, three states:

| | State | Meaning |
| :-: | --- | --- |
| <img src="branding/tray/png/kaltstellen-neutral-32.png" width="32" height="32" alt="Blue tile with a white K"> | Neutral | No profile active, nothing on ice |
| <img src="branding/tray/png/kaltstellen-aktiv-32.png" width="32" height="32" alt="Orange tile with a dark K"> | Profile active | Apps outside the profile are frozen or closed |
| <img src="branding/tray/png/kaltstellen-fehler-32.png" width="32" height="32" alt="Red tile with a white exclamation mark"> | Error | Something could not be frozen, resumed or started |

Logo, wordmark, colours and icons live in [branding/](branding/), together
with the scripts that generate them. The notes there are in German.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work, as defined in the Apache-2.0 license, shall be dual
licensed as above, without any additional terms or conditions.
