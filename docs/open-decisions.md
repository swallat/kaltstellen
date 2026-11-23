# Open decisions

What the owner still has to confirm or overrule, in one place. Each entry
points at the block in [spec.md](spec.md) (or a design note) where the
reasoning is. Implementation choices made without the owner are marked
`[assumed]` in the spec and listed here.

## Open

Details chosen while building M2 drag and drop and M5 failure markers,
which the owner asked for on 2026-11-23 (spec section 8):

1. **M5 where the markers come from**: the tray's error state (L9) over the
   pipe (`status` gets the field `last_switch`); no copy in `state.json`.
   Without a running tray, no markers.
2. **M5 when they refresh**: when the page asks for its state (start, the
   window getting the focus again, at most every 2 s) and when a switch
   from the window has ended; the tray re-checks L9 resolution before it
   answers. The window does not wait for a starting tray, waits at most
   0.5 s for a busy pipe and 1 s for the answer (about 1.5 s at worst).
   When the tray does not answer in time, the last known markers stay;
   only an answer replaces them. A switch from the tray menu or a hotkey
   while the window is open but not focused shows up when it gets the
   focus again (no timer).
3. **M5 which entries**: only the profile the last switch made active;
   matched by executable name, and by path where both the error and the
   entry have one (failed starts). Errors without a program, failed
   resumes (`KS106`, programs of the profile left behind) and errors of
   child processes under their own name get no marker.
4. **M5 look**: a red mark with the code and a few words per error
   (`KS110 start failed`), the full error as tooltip, a red edge on the row.
5. **M2 drop target**: anywhere in the window while a profile is shown,
   including the open `Add a program` dialog; nothing during other
   dialogs; at most 64 files per drop; shortcuts (`.lnk`) are refused like
   other non-`.exe` files instead of being resolved; only files on a fixed
   local disk (as for icons), checked before any file access.
6. **M2 navigation**: the page takes every drop, so the WebView never opens
   a dropped file; the existing navigation guard stays as the second line.
7. **M2 status of a drop**: the summary of a drop becomes the host's
   status line (`drop_done`), so it survives a refresh; refusals the page
   shows itself stay until the next action of the user.
8. **L9 path check of a failed start**: a failed start resolves only when a
   process of that name runs from the path it tried (not a same-named
   program from another folder); for that check the image path of processes
   of that name is read with query rights during the re-checks only. Adds a
   detail to the decided L9 rules (spec L9).
9. **M2 local disk**: "fixed local disk" is what Windows reports as fixed,
   so a USB hard disk counts as local; a symbolic link or junction on a
   local disk that points to a share is followed.

Choices made while building "what a program is" (M2, N1, command line),
which the owner asked for on 2026-11-23 with local sources only (spec M2):

1. **Where it is read**: in the window by one background thread, version
   resources of a batch first, then signatures one file after the other;
   the page gets `pending` and a `program_info` event when there is more,
   so the window never waits. On the command line on up to 8 threads.
2. **Size limit**: files over 1 GiB are not hashed for the signature
   (`not checked`); measured about 0.4 s for 238 MiB and 0.5 to 1.5 s for
   369 MiB on the owner's machine.
3. **Cache**: per window, by file, at most 1024 files, read again when the
   size or the modification time changes; the page keeps finished answers
   while it is open.
4. **Which string table**: the tables of `\VarFileInfo\Translation` in
   order, then `040904B0`, `040904E4`, `04090000`, `000004B0`; each field
   from the first table that has it.
5. **Text from files**: control characters and bidirectional overrides
   removed, whitespace collapsed, at most 200 characters (also protects
   the terminal from escape sequences).
6. **Signature check**: an own signature first, then a Windows catalog by
   SHA-256, then SHA-1; no revocation check, URL retrieval from the local
   cache only. A signature whose chain cannot be built from local data
   counts as not valid (with that reason). "No signature" only when
   `WinVerifyTrust` and the last error both say so (Microsoft's sample).
7. **What is shown**: the description (else the product) and the
   publisher only for a valid signature; the company the file names only
   where no valid signature names a publisher; `unsigned` in italics and
   muted, `⚠ signature not valid` in the warning colour; everything in a
   tooltip. Nothing while the file is unknown.
8. **Command-line format**: two spaces after what each line of `list` and
   `status --unassigned` had before, then `<description>, signed by
   <publisher>`, `<description> (<company>), unsigned` or `signature not
   valid: <reason>`; nothing when nothing is known. The header of `list`
   is unchanged. `list` takes longer than before (about 0.1 to 3.5 s
   instead of under 0.1 s on the owner's machine).

Choices made while building the automatic triggers (U4) on the owner's
request on 2026-11-23, which
[design-automatic-triggers.md](design-automatic-triggers.md) does not
cover. Each is marked `[assumed]` in spec L12 or C9:

1. **Poll interval 5 s** (L12): one poll measured about 3.5 ms of CPU time
   (not a few hundred microseconds); 3 s would be 0.12 % of one core, above
   Q1 if Q1 is read per core, 5 s is about 0.07 % (3.5 ms / 5 s). Start
   noticed within about 5 s, end 5 to 10 s after it.
2. **Trigger form** (C9): one string per program, a name or a full path
   (`trigger = ["sim.exe", 'D:\Games\sim.exe']`), instead of separate `exe`
   and `path` keys.
3. **One profile per trigger program** (C9): a program that triggers one
   profile cannot trigger another one; `check-config` refuses it.
   `priority` is a 32-bit integer and may stand without triggers.
4. **Any manual switch ends automation's ownership** (L12), also one to the
   trigger's own profile: nothing returns by itself when the trigger ends.
5. **Pause, tray start, cancelled end of session** (L12): pausing ends what
   automation owns; switching it on again, starting the tray and a
   cancelled shutdown treat trigger programs that run as starting, so their
   profile is applied.
   After a cancelled end of session the profile they return to is the one
   active then (usually `neutral`), not the one from before the trigger.
6. **Trigger of the profile that is already active** (L12): changes nothing
   and returns nothing.
7. **Return target gone** (L12): a profile from before the trigger that is
   no longer in the configuration becomes `neutral`.
   The same for `Back to` (menu item or notification click).
8. **Notification and undo** (L12): shown for every automatic switch, the
   return included; none when another request already waits; a failed
   switch says so. The one-click undo of decided detail 1 is kept by the
   menu item `Back to <previous>` at the top of the menu, valid until the
   next request, the next automatic switch or the end of the session. The
   notification click does the same only while it is the only
   notification of the tray on screen (chosen after review: the click does
   not say which notification was clicked, the order of the show, hide and
   timeout messages of stacked notifications is not documented, and with
   Do Not Disturb no notification shows); clicks in the notification
   centre do nothing. The menu item acts only if its way back is still the
   one it showed (a later automatic switch while the menu was open makes
   it do nothing). The count of notifications on screen starts afresh
   after the time Windows shows a notification plus 60 s without an event.
   Known limits: the count can still be off when the shell drops messages,
   and a user who closes the newer of two stacked notifications leaves the
   older one counted as the only one; a notification that stays on screen
   longer than that time (or across sleep) is no longer counted, so its
   click does nothing; the duration is read when the tray starts. The menu
   item is the reliable path.
9. **S5 offer** (L12, L7): an automatic switch does not use up the offer;
   when the profile from before the trigger was `neutral`, the earlier
   offer stays.
10. **Equal priority, same poll** (L12): the earlier profile in the file
    wins.
11. **Configuration re-read by the poll** (L12): a changed file is read at
    each poll; triggers added while no profile had any take effect when the
    tray next reads the file (menu opened, switch ended).
12. **Window layout** (M6 Content): the triggers as a text field, one per
    line, with the priority and `Set`, below the hotkey; a copied profile
    does not take them.
13. **What counts as manual** (L12): `recover` from the command line and
    `Back to` (menu item or notification click) hold automation like menu, hotkey and
    window switches.
14. **A waiting trigger ends a hold** (L12): the start or end of a trigger
    that only waits behind a higher one also counts as a change of the
    trigger situation and ends a manual hold.
15. **Hand-back to the user's own profile** (L12): when a higher trigger
    ends and the profile handed back to is the one from before the first
    trigger, automation no longer owns it; it is applied only if not
    active, and nothing returns from it later.
16. **Changed priorities** (L12): when a changed configuration lets another
    running trigger win, the next poll decides again; a manual hold stays.
17. **Unreadable path triggers** (L12): a process whose image path cannot be
    read is tried at three polls, then it does not count.
18. **C9 details**: hard-protected names are allowed as triggers (only
    looked for); a trigger with a path and an entry with another path of
    the same name do not clash.
19. **Status lines and pipe field** (L12, pipe protocol): wording of the
    `tray: …` lines and the `automation` field of the `status` reply.

Everything else the owner has not decided yet is deferred (below).

## Deferred by the owner on 2026-11-23

Decided to be settled later, at the point named:

1. **L7 session end, wait time**: `WM_QUERYENDSESSION` waits at most 4 s for
   the resume; fixed once VM test T20 has measured a real shutdown.
2. **Elevated helper for services (A5, P8)**: the proposal in
   [design-elevated-helper.md](design-elevated-helper.md) stays a proposal,
   to be decided in detail later.
3. **Update signing (UP1)**: integrity by SHA-256 from `checksums.txt` only
   for now. Signing (Authenticode certificate or an own key pair) is decided
   together with the release pipeline.

## Assumptions confirmed by the owner on 2026-11-23

The author's defaults, confirmed as built:

- **P5 programs started by a profile**: a switch to `neutral` does not stop
  programs that kaltstellen started (confirmed separately the same day).
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
  profile from before the trigger is offered. Planned after version 1;
  built on the owner's request on 2026-11-23, before the release (spec L12,
  C9; the choices made while building are under "Open" above).
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
  1, 2, 4, then 7 days, confirmed above): the tray's timer tries it when the
  wait ends; on request it can be installed at any time.
- **UP1 in the main window**: a section `Updates` at the bottom of the
  sidebar; the window checks by running `kaltstellen update --check
  --quiet`; choosing a mode that checks starts a check at once; `Install`
  opens the same console window as the tray item.
