# Open decisions

What the owner still has to confirm or overrule, in one place. Each entry
points at the block in [spec.md](spec.md) (or a design note) where the
reasoning is. Implementation choices made without the owner are marked
`[assumed]` in the spec and listed here.

## Open

1. **P5 programs started by a profile** `[assumed]`: a switch to `neutral`
   does not stop programs that kaltstellen started (spec P5, last sentence).
   Not yet put to the owner.

## Deferred by the owner on 2026-11-23

Decided to be settled later, at the point named:

1. **L7 session end, wait time**: `WM_QUERYENDSESSION` waits at most 4 s for
   the resume; fixed once VM test T20 has measured a real shutdown.
2. **Elevated helper for services (A5, P8)**: the proposal in
   [design-elevated-helper.md](design-elevated-helper.md) stays a proposal,
   to be decided in detail later.
3. **Update signing (UP1)**: integrity by SHA-256 from `checksums.txt` only
   for now. Signing (Authenticode certificate or an own key pair) is decided
   with the first release.

## Assumptions confirmed by the owner on 2026-11-23

The author's defaults, confirmed as built:

- **UP1 retry waits**: a version that could not be installed is tried again
  1 day after the first failure, 2 after the second, 4 after the third, 7
  after every later one in a row (the rhythm was left to the author).
- **Automatic trigger details**
  ([design-automatic-triggers.md](design-automatic-triggers.md)): `priority`
  is an integer, default `0`, higher wins; a higher trigger takes over from
  a lower one and hands back to it when it ends; equal priority keeps the
  first; debounce about 5 s; the poll interval within 2 to 5 s is fixed
  when measured.
- **M6 lockdown details**: the Content-Security-Policy, no navigation away
  from the page, no new windows, permission requests denied, script
  dialogs and the status bar off (spec M6 "Hosting and lockdown").
- **Configuration file** as the only source of truth; the window reads and
  writes it.
- **W1**: a program that the target profile also freezes stays frozen
  through the switch.
- **W4**: choosing the active profile again applies it again.
- **L1**: one tray per session with a named mutex; the command line reaches
  it over a pipe only the user may open.
- **Main window**: a separate process, content and layout as built; a
  second start finds the open window by a window property.
- **N1 dismiss**: recorded in `seen.json`.

## Decided by the owner on 2026-11-23

- **L5 pipe**: as built, including no fallback when a running tray does not
  answer (exit code 5).
- **L6 interrupt**: as built: stop after the current program, resume
  everything, `neutral`, exit code 1.
- **L7 session end**: as built, except the 4 s wait (deferred item 1).
- **L8 start at sign-in**: never written without consent; the tray asks once
  on its first start (built after this decision). The question is a message
  box on its own thread; `KALTSTELLEN_AUTOSTART_PROMPT` (`yes`, `no`, `skip`)
  for tests and managed installations.
- **L9 error state**: no manual acknowledging; errors stay until the next
  switch or until their cause disappears by itself (built after this
  decision). Checked again when the menu opens and every 30 s while one
  could still resolve, with the rules per error code in spec L9.
- **L10 hotkeys**: as built: a taken hotkey is listed in the menu, the icon
  does not turn red, it is retried.
- **L11 callers**: a caller whose request equals the running switch is
  protected from that switch's next snapshot on, and what was frozen before
  stays frozen; a `--no-wait` caller is protected only while it still runs.
- **M6 toolkit**: WebView2 directly, plain HTML/CSS/JS without build step,
  native window removed, missing runtime gives a clear message and `KS702`.
- **M6 behaviour**: instant save without Save button and undo, as built; a
  second window brings the open one to the front (built after this
  decision).
- **M6 WebView details and bridge**: origin `https://ui.kaltstellen.invalid/`
  with a 403 for everything else; dev tools only in debug builds; user data
  folder `webview2` in the data directory; window 1040 x 760 at 100 %; the
  review options `--screenshot`, `--view`, `--theme`, `--drive` stay in the
  release executable, hidden from help. The message set of
  [gui-bridge.md](gui-bridge.md); every edit is one request and one save;
  the page gets the whole state with every reply.
- **M2 program icons**: shown at 20 CSS pixels in the three lists; read only
  from files on a fixed local disk (no icons for programs on network drives,
  shares or device paths), from a running process of that name first, else
  from an entry's configured path; at most 96 device pixels; kept while the
  window is open, at most 4 MiB; a grey square of the same size otherwise.
- **N2 unassigned programs**: as built: only running programs, compared by
  executable name.
- **Automatic triggers (U4)**: trigger programs per profile, opt-in, a manual
  choice always wins, built after version 1. All points of
  [design-automatic-triggers.md](design-automatic-triggers.md) decided: a
  notification with `Back to <previous>`; manual wins until the trigger
  situation changes; a 2 to 5 s snapshot poll if it stays within Q1; a
  `priority` per profile for two triggers at once; back to the previous
  profile when the trigger ends, after a debounce; `trigger = [...]` per
  profile, checked against its own profile, a section of its own in the
  window; a remembered tray item `Automatic switching`; after sign-in the
  profile from before the trigger is offered.
- **Updates (UP1)**: network access for the update accepted (non-goal
  reworded); only the tray checks, daily; release layout, development
  builds and package-manager behaviour as built; `kaltstellen.old.exe` kept
  until the next update; after an update the last profile is offered, not
  applied; installing automatically by default with a manual and an off
  mode, and updates shown in the main window (built after this decision).
- **UP1 automatic install timing**: retried when a switch ends with
  `neutral` active and after every later check; logged once per version
  while it waits; a saved `on` reads as `auto`; checked again right before
  the tray quits (`quit_if_idle`); no automatic install while the session
  ends; one install at a time. A version that could not be installed after
  it was verified is **tried again regularly**, not over and over (waits:
  1, 2, 4, then 7 days, confirmed below): the tray's timer tries it when the
  wait ends; on request it can be installed at any time.
- **UP1 in the main window**: a section `Updates` at the bottom of the
  sidebar; the window checks by running `kaltstellen update --check
  --quiet`; choosing a mode that checks starts a check at once; `Install`
  opens the same console window as the tray item.
