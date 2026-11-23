# Design note: automatic triggers (M4; U4)

Status: **built** on the owner's request on 2026-11-23, before the release,
so that it is tested together with version 1 (it was planned after version
1). **Decided** by the owner on 2026-11-23: the direction, and all open
points of this note (section "Decided details"). The details the author
proposed (priority form, takeover, equal priority keeping the first,
debounce, poll interval) were confirmed by the owner the same day. As
built: spec L12 (behaviour) and C9 (configuration); what had to be decided
while building is marked `[assumed]` there and listed in
[open-decisions.md](open-decisions.md). The poll interval is 5 s: one poll
measured about 3.5 ms of CPU time, more than the few hundred microseconds
estimated below (spec L12).

## Decided

- Automatic switching exists only as **trigger programs per profile**
  (the "Trigger model" below); no time of day, power source or full-screen
  detection.
- **Opt-in**: nothing happens unless a profile names trigger programs.
- **Manual always wins** (conflict rule 1 below).
- **After version 1**: a post-v1 item in `docs/plan.md`; on the owner's request built before the release instead (2026-11-23).

## Decided details (owner, 2026-11-23)

1. **Notification and undo**: an automatic switch shows a notification with
   a one-click `Back to <previous>`.
2. **How long manual wins**: until the trigger situation changes (the
   trigger program ends, or a trigger program starts).
3. **Detection**: a poll of the handle-free snapshot every 2 to 5 s, only
   while some profile has triggers, provided it measures below the Q1
   thresholds; not event-based. The interval within that range is fixed
   when measured.
4. **Two triggers at once**: a `priority` per profile decides (see conflict
   rule 2).
5. **Return when the trigger ends**: back to the profile that was active
   before the trigger, after the trigger stayed gone for a debounce time of
   about 5 s.
6. **Configuration form**: `trigger = ["simulator.exe"]` per profile, with
   optional paths as in A7; the configuration check refuses a trigger
   program that its own profile stops or freezes. In the main window the
   triggers are a section of their own in the profile view.
7. **Global off switch**: a tray item `Automatic switching` pauses all
   triggers; its state is remembered across restarts (`settings.json`).
8. **After sign-in (S5)**: the profile from before the trigger is offered,
   not the triggered one; if the trigger program starts again, its trigger
   applies as usual.

## Background: why only the narrow case

Manual switching already takes one click or one hotkey. Automatic switching
adds a background watcher (against the lazy, watcher-free design of W6 and
N1, and a cost for Q1), and it acts without the user looking: it may freeze
a program in the middle of a call or a download. The benefit is real for a
narrow case: "when the flight simulator starts, apply the flight profile;
when it ends, go back to neutral".

This is why only the narrow case below was chosen, opt-in per profile, with
a visible, reversible effect (notification plus one-click undo).

## Trigger model

- A profile may name trigger programs:
  `trigger = ["simulator.exe"]` (executable names, optional paths as in A7),
  and a `priority` (an integer, default `0`, higher wins).
- **Start of a trigger** (a matching process appears in the session):
  apply the profile, unless a manual switch holds automation (conflict
  rule 1) or automatic switching is paused.
- **End of the last trigger process** (gone for the debounce time): return
  to the profile that was active before the trigger fired (usually
  `neutral`), not blindly to `neutral`.
- Detection: a poll of the existing handle-free snapshot every 2 to 5 s,
  only while at least one profile has triggers and automatic switching is
  not paused (a few hundred microseconds per poll; measured against Q1
  before it is built).
- Triggers go through the same W3 queue as menu, hotkey and pipe requests,
  tagged as automatic.

## Conflict rules

1. **Manual beats automatic.** A manual switch (menu, hotkey, command line,
   window) suspends automation until the trigger situation changes (a
   trigger program ends, or a trigger program starts). Automatic switches
   never supersede a waiting manual request in W3; a manual request always
   supersedes a waiting automatic one.
2. **Two triggers at once** (programs of two profiles run): the profile
   with the higher `priority` wins. A trigger
   of higher priority that starts while a lower one is active switches to
   its profile; when it ends, the lower profile comes back if its trigger
   still runs, otherwise the profile from before the first trigger. A
   trigger of lower or equal priority that starts while another is active
   is logged and shown in the menu as "Trigger for X waits while Y is
   active"; at equal priority the one that came first stays.
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
| Trigger program crashes and restarts quickly | Flapping: switch out and in again | Debounce: act on "ended" only after it stayed gone for about 5 s. |
| Launcher process starts and hands over to the real program | Trigger fires and ends at once | Trigger on the real program; debounce; document. |
| Switch fails partly | W2 as usual: error icon, collected errors | Same as manual. |
| User is in a call when the trigger freezes the call program | Unwanted interruption | Notification with the switch and one-click undo (`Back to <previous>`); do not freeze programs with active audio (would need audio session checks, R3). |
| Poll misses a very short-lived trigger | No switch | Acceptable; triggers are long-running programs. |
| Sign-out while triggered | S5 resume as usual | The offer after sign-in is the profile from before the trigger (decided detail 8). |
