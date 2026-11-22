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
