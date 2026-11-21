# Requirements

Status: **draft**. Nothing here is fixed yet; the open questions at the end
decide several of the points above them.

## Goal

One click (or no click at all) switches the PC between usage profiles such as
"flight sim", "music" and "everyday". Switching a profile starts the apps that
belong to it and puts the others on ice, so they stop costing CPU and RAM.

## Non-goals

- No general "game booster": no registry tweaks, no power-plan magic, no
  cleaning of temp files.
- No launcher or library for games.
- No account, no telemetry, no network access.
- Windows only.

## Functional requirements

### Profiles

- **P1** A profile has a name and a list of app entries.
- **P2** Each app entry names an action for when the profile becomes active:
  `start`, `stop`, `suspend`, or `ignore`.
- **P3** Profiles live in one human-readable config file that can be edited by
  hand and kept in version control.
- **P4** Exactly one profile is active at a time. A neutral profile restores
  everything.

### Actions

- **A1** `start`: launch an executable with arguments and working directory,
  unless it is already running.
- **A2** `stop`: ask the app to close (close its windows), wait for a
  configurable timeout, then terminate it.
- **A3** `suspend`: freeze all processes of the app, optionally trimming their
  memory out of RAM.
- **A4** An app is all processes with a given executable name, plus their
  child processes.
- **A5** Windows services can be stopped and started as part of a profile.
- **A6** Core system processes are never touched, whatever the config says.

### State and restore

- **S1** The tool records what it suspended and stopped, and undoes exactly
  that on the next profile switch.
- **S2** Each process is suspended at most once, so one resume always brings
  it back.
- **S3** If the tool exits or crashes, suspended processes are not left frozen:
  on exit it resumes them, and on the next start it recovers recorded state.
- **S4** A suspended app's windows are minimised first, so Windows does not
  show them as "Not responding".

### Interface

- **U1** Tray icon with a menu to pick the active profile and to quit.
- **U2** Command-line interface for the same operations, usable from scripts
  and Stream Deck buttons.
- **U3** Optional global hotkeys per profile.
- **U4** Optional automatic switching: a profile activates when a trigger
  process starts and deactivates when it exits.

## Quality requirements

- **Q1** Idle footprint of the tray app: a few MB of RAM, no measurable CPU.
- **Q2** Single executable, no installer and no runtime needed.
- **Q3** Runs without admin rights; features that need elevation (services,
  elevated processes) say so instead of failing silently.
- **Q4** All `unsafe` Win32 code is confined to one module.

## Open questions

1. Which apps belong to which profile? A concrete list for "flight sim" and
   "music" decides how much of A2, A4 and A5 is needed first.
2. Which of those apps tolerate being suspended? To be tested with the
   prototype; everything else has to be stopped and restarted.
3. Are services part of the first version, or only user processes?
4. How should elevation work: start the whole tray app elevated, or a small
   elevated helper only for the operations that need it?
5. Config format: TOML is the default choice. Any preference?
6. Is a graphical profile editor wanted, or is editing the config file enough?
