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
  `start`, `stop`, `suspend`, or `ignore`. To put an app on ice, `suspend` is
  the default; `stop` is the exception for apps that do not tolerate being
  frozen (see [Findings](#findings)).
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

## Findings

### Short suspend test (prototype, `scripts/Test-SuspendCompat.ps1`)

One run on a single Windows 11 desktop with about 100 everyday apps running
(browsers, chat and music clients, launchers, cloud sync, vendor tray tools).
Each app was suspended with a memory trim for 15 seconds and then resumed.

- All but two apps were still alive and responsive afterwards; none had to be
  restarted. The two exceptions were never frozen: one ran elevated and could
  not be opened, one was already unresponsive before the test.
- While frozen, the apps' combined working set dropped from roughly 21 GB to
  almost nothing. Shortly after resuming it was back at only about 2 GB,
  because pages return on demand. The 21 GB overstate the gain somewhat:
  shared memory counts once per process.

What this does not show:

- The test only checks that processes are alive and their windows respond. It
  does not check that network connections, cloud sync, audio or licence checks
  still work.
- 15 seconds is short. Real use freezes apps for hours, which can outlast
  connection and session timeouts.

Consequences: `suspend` is the default action (P2), and the trim is worth
doing by default (A3). A per-app fallback to `stop` stays necessary, and S3
matters more than expected, since most apps will be frozen rather than closed.

## Open questions

1. Which apps belong to which profile? A concrete list for "flight sim" and
   "music" decides how much of A2, A4 and A5 is needed first.
2. Which apps misbehave after a long freeze (10 minutes and more), even though
   they survive a short one? To be tested with the apps that actually go into
   profiles, followed by a manual check that they still work.
3. Are services part of the first version, or only user processes?
4. How should elevation work: start the whole tray app elevated, or a small
   elevated helper only for the operations that need it?
5. Config format: TOML is the default choice. Any preference?
6. Is a graphical profile editor wanted, or is editing the config file enough?
