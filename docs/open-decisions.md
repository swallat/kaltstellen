# Open decisions

What the owner still has to confirm or overrule, in one place. Each entry
points at the block in [spec.md](spec.md) (or a design note) where the
reasoning is. Implementation choices made without the owner are marked
`[assumed]` in the spec and listed here.

## Open

1. **L7 session end, wait time**: `WM_QUERYENDSESSION` waits at most 4 s for
   the resume. Provisional until VM test T20 has measured a real shutdown.
2. **Elevated helper for services (A5, P8)**: deferred. The proposal in
   [design-elevated-helper.md](design-elevated-helper.md) stays a proposal,
   to be decided in detail later.
3. **Update signing (UP1)**: integrity by SHA-256 from `checksums.txt` only
   for now. Signing (Authenticode certificate or an own key pair) is decided
   together with the release pipeline.
4. **M6 WebView details** `[assumed]`: origin
   `https://ui.kaltstellen.invalid/` with a 403 for everything else; dev
   tools only in debug builds; user data folder `webview2` in the data
   directory; window 1040 x 760 at 100 %; the review options
   `--screenshot`, `--view`, `--theme`, `--drive` stay in the release
   executable, hidden from help.
5. **M6 bridge** `[assumed]`: the message set of
   [gui-bridge.md](gui-bridge.md); every edit is one request and one save;
   the page gets the whole state with every reply.

6. **L11 callers joining a running switch** `[assumed]`: a caller whose
   request equals the running switch is protected from that switch's next
   snapshot on; what was frozen before stays frozen.
7. **L11 `--no-wait` callers** `[assumed]`: protected only while they still
   run; after they exit, their ancestors are ordinary processes for the rest
   of that switch.

8. **L9 re-check** `[assumed]`: errors are checked again when the menu
   opens and every 30 s while one could still resolve; rules per error code
   in spec L9 (start errors resolve when the program runs, stop and freeze
   errors when it no longer runs, the rest stays until the next switch).

9. **L8 question** `[assumed]`: a message box on its own thread so start-up
   is not held up; `KALTSTELLEN_AUTOSTART_PROMPT` (`yes`, `no`, `skip`) for
   tests and managed installations.

10. **UP1 automatic install timing** `[assumed]`: retried when a switch ends
    with `neutral` active and after every later check; logged once per
    version while it waits; a saved `on` (earlier name) reads as `auto`.
    The condition is checked again right before the tray quits
    (`quit_if_idle`); a busy tray stays and the install starts again the
    next time the tray is idle in `neutral`. A version that could not be
    installed after it was verified (`KS806` did not start, `KS805` not
    swapped after the tray quit; not a start check that only timed out) is recorded in
    `update.json` and never installed automatically again; on request it
    still can be. Only the last such version is remembered. Failures before
    the tray is touched are retried after the next check (daily). No
    automatic install while the session ends; one install at a time.

11. **UP1 in the main window** `[assumed]`: a section `Updates` at the bottom
    of the sidebar; the window checks by running `kaltstellen update --check
    --quiet` (no network code in the window either); choosing a mode that
    checks starts a check at once; `Install` opens the same console window as
    the tray item rather than installing silently, so a window that the user
    is looking at shows the steps.

## Decided by the owner on 2026-11-23

- **L5 pipe**: as built, including no fallback when a running tray does not
  answer (exit code 5).
- **L6 interrupt**: as built: stop after the current program, resume
  everything, `neutral`, exit code 1.
- **L7 session end**: as built, except the 4 s wait (open item 1).
- **L8 start at sign-in**: never written without consent; the tray asks once
  on its first start (built after this decision).
- **L9 error state**: no manual acknowledging; errors stay until the next
  switch or until their cause disappears by itself (built after this
  decision).
- **L10 hotkeys**: as built: a taken hotkey is listed in the menu, the icon
  does not turn red, it is retried.
- **M6 toolkit**: WebView2 directly, plain HTML/CSS/JS without build step,
  native window removed, missing runtime gives a clear message and `KS702`.
- **M6 behaviour**: instant save without Save button and undo, as built; a
  second window brings the open one to the front (built after this
  decision).
- **N2 unassigned programs**: as built: only running programs, compared by
  executable name.
- **Automatic triggers (U4)**: trigger programs per profile, opt-in, a manual
  choice always wins, built after version 1; details in
  [design-automatic-triggers.md](design-automatic-triggers.md).
- **Updates (UP1)**: network access for the update accepted (non-goal
  reworded); only the tray checks, daily; release layout, development
  builds and package-manager behaviour as built; `kaltstellen.old.exe` kept
  until the next update; after an update the last profile is offered, not
  applied; installing automatically by default with a manual and an off
  mode, and updates shown in the main window (built after this decision).
