# Open decisions

Everything the owner still has to confirm or overrule, in one place. Each
entry points at the `[assumed]` block in [spec.md](spec.md) (or a design
note) where the reasoning is. Nothing here is marked `[decided]` until the
owner says so.

## Tray and command line (M2)

1. **L5 pipe**: name with user SID and session; DACL for the user only; no
   fallback when a tray runs but its pipe does not answer (exit code 5);
   unknown profile and invalid configuration refused before queueing;
   `force` not compared for the W3 drop; `--no-wait` has no effect without a
   tray.
2. **L6 interrupt**: Ctrl+C, Ctrl+Break or closing the console during an
   in-process switch stops after the current program and resumes everything
   recorded (back to `neutral`), exit code 1. Alternative: finish the whole
   switch, or only undo what this run froze.
3. **L7 session end**: wait at most 4 s at `WM_QUERYENDSESSION`; stopped
   records cleared only at `WM_ENDSESSION` "ending"; the offer is used up by
   any switch; standby only logged.
4. **L8 start at sign-in**: off by default, value `kaltstellen` in the
   per-user Run key, `KALTSTELLEN_RUN_KEY` override for tests.
5. **L9 acknowledge and DPI**: `Clear error state` clears only the errors of
   the last switch, not an invalid configuration or a missing pipe; the
   executable carries a manifest with per-monitor DPI awareness v2 and
   common controls v6.
6. **L10 hotkeys**: hotkey IDs follow the profile order; a taken hotkey is listed in the menu but does not turn the icon red; it is retried at every sync.

## Main window (M3)

7. **M6 toolkit**: closed. The owner decided on 2026-11-23: WebView2
   directly, plain HTML/CSS/JS without build step, native window removed,
   runtime missing means a clear message and `KS702`.
8. **M6 behaviour**: every accepted edit is saved at once (no Save button,
   no undo); a file changed on disk wins and the edit is repeated; a second
   `kaltstellen gui` only reports instead of bringing the window to the
   front; deleting asks for confirmation; copying a profile drops its
   hotkey. The WebView window can be resized (the native one could not).
12. **M6 WebView details**: origin `https://ui.kaltstellen.invalid/` with a
    403 for everything else; dev tools only in debug builds; user data
    folder `webview2` in the data directory (cache of the page, a few MB);
    window 1040 x 760 at 100 %; review options `--screenshot`, `--view`,
    `--theme`, `--drive` stay in the release executable, hidden from help.
13. **M6 bridge**: the message set of `docs/gui-bridge.md`; every edit is
    one request and one save; the page gets the whole state with every
    reply (simple, a few KB).

## Unassigned programs (N1)

9. **N2 details**: only programs running at the snapshot are listed; an
   entry of any action removes a program from the list; programs are
   compared by executable name only (a path in an entry is not needed to
   count as a decision); "new" uses second resolution; plain
   `kaltstellen status` takes no snapshot, only `status --unassigned` does;
   the tray takes one after every applied profile and shows
   `Unassigned programs (N)`, which opens the main window.

## M4 (design notes only, nothing implemented)

10. **Elevated helper for services**: see [design-elevated-helper.md](design-elevated-helper.md) (service with trigger start, pipe with identity checks, admin-only allow list) and its open questions.
11. **Automatic triggers**: see [design-automatic-triggers.md](design-automatic-triggers.md); recommendation: no general automation, at most trigger programs per profile, opt-in, manual beats automatic.

