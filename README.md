# kaltstellen

*kaltstellen* (German): to put something on ice.

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

## Build

Requires Windows and a stable Rust toolchain.

```powershell
cargo build --release
```

The binary is `target\release\kaltstellen.exe`. Freezing processes that run
elevated or under another user requires an elevated terminal.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work, as defined in the Apache-2.0 license, shall be dual
licensed as above, without any additional terms or conditions.
