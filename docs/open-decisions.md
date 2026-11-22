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

