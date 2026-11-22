# Design note: automatic triggers (M4; U4)

Status: **direction decided** by the owner on 2026-11-23: trigger programs
per profile, opt-in, a manual choice always wins, built **after version 1**.
Nothing of this is implemented; version 1 is manual only (spec section 1, U5).
The details below are a proposal; the points still open are listed at the
end.

## Decided

- Automatic switching exists only as **trigger programs per profile**
  (the "Trigger model" below); no time of day, power source or full-screen
  detection.
- **Opt-in**: nothing happens unless a profile names trigger programs.
- **Manual always wins** (conflict rule 1 below).
- **After version 1**: a post-v1 item in `docs/plan.md`.

## Background: why only the narrow case

Manual switching already takes one click or one hotkey. Automatic switching
adds a background watcher (against the lazy, watcher-free design of W6 and
N1, and a cost for Q1), and it acts without the user looking: it may freeze
a program in the middle of a call or a download. The benefit is real for a
narrow case: "when the flight simulator starts, apply the flight profile;
when it ends, go back to neutral".

This is why only the narrow case below was chosen, opt-in per profile, with
a visible, reversible effect (notification plus one-click undo proposed).

## Trigger model (if wanted)

- A profile may name trigger programs:
  `trigger = ["simulator.exe"]` (executable names, optional paths as in A7).
- **Start of a trigger** (a matching process appears in the session):
  apply the profile, unless the user switched manually within the last N
  minutes (see conflicts).
- **End of the last trigger process**: return to the profile that was active
  before the trigger fired (usually `neutral`), not blindly to `neutral`.
- Detection without polling: the tray already runs; it can subscribe to
  process start and stop events. Options: WMI `__InstanceCreationEvent`
  (convenient, but WMI costs memory and CPU and needs COM), ETW kernel
  process provider (needs admin for the kernel session; no), or polling the
  existing handle-free snapshot every few seconds (simple, a few hundred
  microseconds per poll; measure against Q1). Recommended: a 2 to 5 s poll
  of the snapshot, only while at least one profile has triggers.
- Triggers go through the same W3 queue as menu, hotkey and pipe requests,
  tagged as automatic.

## Conflict rules

1. **Manual beats automatic.** A manual switch (menu, hotkey, command line,
   window) suspends automation until the trigger situation changes (the
   trigger program ends, or a new trigger program starts). Automatic
   switches never supersede a waiting manual request in W3; a manual request
   always supersedes a waiting automatic one.
2. **Two triggers at once** (programs of two profiles run): the first one
   wins; the second is logged and shown in the menu as "Trigger for X
   ignored while Y is active". Alternative: a `priority` per profile. Keep
   first-wins until a real case needs priority.
3. **Trigger fires while a switch runs**: queued like any request; if the
   running switch leads to the trigger's profile, dropped (W3 rule).
4. **Trigger program is itself frozen or stopped by the target profile**:
   refused at configuration check (C-rule: a trigger program must not be in
   its own profile with `stop` or `suspend`).
5. **Return after the trigger ends** only if the active profile is still the
   one the trigger applied; if the user switched meanwhile, nothing happens.

## Failure modes and how they show

| Failure | Effect | Mitigation |
| --- | --- | --- |
| Trigger program crashes and restarts quickly | Flapping: switch out and in again | Debounce: act on "ended" only after it stayed gone for some seconds. |
| Launcher process starts and hands over to the real program | Trigger fires and ends at once | Trigger on the real program; debounce; document. |
| Switch fails partly | W2 as usual: error icon, collected errors | Same as manual. |
| User is in a call when the trigger freezes the call program | Unwanted interruption | Notification before or with the switch, one-click undo ("Back to neutral"); do not freeze programs with active audio (would need audio session checks, R3). |
| Poll misses a very short-lived trigger | No switch | Acceptable; triggers are long-running programs. |
| Sign-out while triggered | S5 resume as usual; the offer after sign-in is the triggered profile | Offer the profile that was active before the trigger instead. |

## Open points (to settle before it is built)

1. **Notification and undo**: should an automatic switch show a
   notification with a one-click `Back to <previous>`? (Proposed: yes.)
2. **How long manual wins**: until the trigger situation changes
   (proposed), a fixed time, or until automation is switched on again from
   the menu?
3. **Detection and Q1**: a 2 to 5 s poll of the handle-free snapshot, only
   while some profile has triggers, if measured below the Q1 thresholds; or
   must it be event-based?
4. **Two triggers at once**: first wins (proposed) or a `priority` per
   profile?
5. **Return when the trigger ends**: back to the profile active before the
   trigger (proposed), or always `neutral`; and the debounce time before
   "ended" counts (proposed: some seconds).
6. **Configuration form**: `trigger = ["simulator.exe"]` per profile with
   optional paths as in A7, and the check that a trigger program is not
   stopped or frozen by its own profile; where to edit it in the main window.
7. **A global off switch**: a tray item `Automatic switching` to pause all
   triggers, and whether its state is remembered.
8. **After sign-in (S5)**: offer the profile from before the trigger rather
   than the triggered one?
