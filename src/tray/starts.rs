//! H9: programs that start while a profile is active. The helper's process
//! events (M4b) say which programs started; this module decides which of them
//! are worth a hint: one the profile has no entry for, that is not
//! hard-protected, not part of Windows, not kaltstellen itself, that was not
//! dismissed for the profile and that stays running for a few seconds
//! (short-lived processes are noise). The tray notes the programs in
//! `seen.json` (so the main window can point at them) and shows one
//! notification. Nothing here touches a process; without the helper there
//! are no events and nothing happens.
//!
//! The logic is plain data in and out so that it can be tested; the tray
//! does the pipe, the notification and the file.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::helper::events::Event;
use crate::kinds::{self, Kind};
use crate::protection;

/// A program that ended sooner than this after it started is not offered.
pub const MIN_LIFE: u64 = 10;
/// At most one notification per this many seconds; programs that start in
/// between are named together in the next one.
pub const BALLOON_GAP: u64 = 30;
/// The longest wait between two questions to a helper that does not answer.
const BACKOFF_MAX: u64 = 60;
/// More than this many unconfirmed starts are not tracked.
const PENDING_MAX: usize = 512;

/// What the helper answered to one `events` question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub events: Vec<Event>,
    /// Ask again with this.
    pub next: u64,
}

/// What decides whether a program is offered.
pub struct Context<'a> {
    /// Executables (lower case) the active profile has an entry for.
    pub decided: &'a HashSet<String>,
    /// Executables (lower case) dismissed for the active profile.
    pub dismissed: Option<&'a BTreeSet<String>>,
}

/// Whether a started program is worth offering to the active profile.
pub fn eligible(event: &Event, ctx: &Context) -> bool {
    let key = event.exe.to_lowercase();
    // Without a path the kernel gave no drive path: system plumbing mostly.
    if event.path.is_none()
        || key.starts_with("kaltstellen")
        || ctx.decided.contains(&key)
        || ctx.dismissed.is_some_and(|d| d.contains(&key))
        || protection::is_hard_protected(&event.exe)
    {
        return false;
    }
    kinds::classify(&event.exe, event.path.as_deref(), None).kind != Kind::System
}

#[derive(Debug, Default)]
pub struct StartWatch {
    enabled: bool,
    /// The profile being watched (lower case); empty for none.
    profile: String,
    /// Seconds since 1970 when it was applied; earlier starts are ignored.
    since: u64,
    after: Option<u64>,
    /// Starts not yet old enough, by process id.
    pending: HashMap<u32, Event>,
    /// Programs offered since the profile was applied (lower case).
    offered: HashSet<String>,
    /// Programs waiting for the next notification.
    waiting: Vec<String>,
    last_balloon: u64,
    wait: u64,
    next_try: u64,
}

impl StartWatch {
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, on: bool) {
        if self.enabled != on {
            self.enabled = on;
            self.pending.clear();
            self.waiting.clear();
            self.wait = 0;
            self.next_try = 0;
        }
    }

    /// Whether some start waits for its time.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Seconds since the profile was applied (0 if it was not; the largest
    /// value when the clock went back).
    pub fn age(&self, now: u64) -> u64 {
        if self.since == 0 {
            0
        } else {
            // A clock that went back counts as old: the window restarts.
            now.checked_sub(self.since).unwrap_or(u64::MAX)
        }
    }

    /// The profile watched now; empty or `neutral` watches nothing.
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// A profile was applied: what was tracked is dropped and only starts
    /// from now on count. `neutral` (or no name) watches nothing.
    pub fn activated(&mut self, profile: &str, now: u64) {
        let neutral = profile.trim().is_empty() || profile.trim().eq_ignore_ascii_case("neutral");
        self.profile = if neutral {
            String::new()
        } else {
            profile.trim().to_lowercase()
        };
        self.since = now;
        self.pending.clear();
        self.offered.clear();
        self.waiting.clear();
    }

    /// Whether the helper should be asked now (and with which `after`).
    pub fn due(&self, now: u64) -> Option<Option<u64>> {
        (self.enabled && !self.profile.is_empty() && now >= self.next_try).then_some(self.after)
    }

    /// Takes the helper's answer. No helper, a role that is off or an
    /// error all mean no events; the next question waits longer each time.
    pub fn feed(&mut self, now: u64, answer: Result<Option<Fetched>, String>) {
        let fetched = match answer {
            Ok(Some(fetched)) => fetched,
            Ok(None) | Err(_) => {
                self.wait = (self.wait * 2).clamp(4, BACKOFF_MAX);
                self.next_try = now + self.wait;
                return;
            }
        };
        self.wait = 0;
        self.next_try = 0;
        self.after = Some(fetched.next);
        for event in fetched.events {
            match event.kind.as_str() {
                "start" if event.time >= self.since => {
                    if self.pending.len() < PENDING_MAX {
                        self.pending.insert(event.pid, event);
                    }
                }
                "stop" => {
                    // Same process: the same pid and creation time. One that
                    // lived long enough stays: a late answer (the helper was
                    // not reachable for a while) brings start and stop
                    // together.
                    if self.pending.get(&event.pid).is_some_and(|p| {
                        p.created == event.created && event.time < p.time + MIN_LIFE
                    }) {
                        self.pending.remove(&event.pid);
                    }
                }
                _ => {}
            }
        }
    }

    /// The starts that stayed running long enough (the process did not end
    /// within [`MIN_LIFE`]), oldest first, whatever the program is. The
    /// other features that follow starts (a profile that reacts to them)
    /// take these; [`offers`](Self::offers) picks the ones to announce.
    pub fn settled(&mut self, now: u64) -> Vec<Event> {
        let old: Vec<u32> = self
            .pending
            .iter()
            .filter(|(_, e)| e.time + MIN_LIFE <= now)
            .map(|(pid, _)| *pid)
            .collect();
        let mut events: Vec<Event> = old
            .iter()
            .filter_map(|pid| self.pending.remove(pid))
            .collect();
        events.sort_by_key(|e| e.seq);
        events
    }

    /// Of settled starts, those worth offering, each program once per
    /// activation: `(exe, path)`. They are also queued for the next
    /// notification.
    pub fn offers(&mut self, settled: &[Event], ctx: &Context) -> Vec<(String, Option<String>)> {
        let mut found: Vec<(String, Option<String>)> = Vec::new();
        for event in settled {
            let key = event.exe.to_lowercase();
            if !eligible(event, ctx) || self.offered.contains(&key) {
                continue;
            }
            self.offered.insert(key);
            self.waiting.push(event.exe.clone());
            found.push((event.exe.clone(), event.path.clone()));
        }
        found
    }

    /// [`settled`](Self::settled) and [`offers`](Self::offers) in one.
    pub fn confirmed(&mut self, now: u64, ctx: &Context) -> Vec<(String, Option<String>)> {
        let settled = self.settled(now);
        self.offers(&settled, ctx)
    }

    /// The text of the next notification, when one is due. `profile` is
    /// the profile's name as the owner knows it. Programs that were noted
    /// as already offered elsewhere can be taken back with [`forget`].
    pub fn announcement(&mut self, now: u64, profile: &str) -> Option<(String, String)> {
        if self.waiting.is_empty() || now < self.last_balloon + BALLOON_GAP {
            return None;
        }
        self.last_balloon = now;
        let names = std::mem::take(&mut self.waiting);
        let shown: Vec<&str> = names.iter().take(3).map(String::as_str).collect();
        let more = names.len().saturating_sub(shown.len());
        let list = if more > 0 {
            format!("{} and {more} more", shown.join(", "))
        } else {
            shown.join(", ")
        };
        let title = if names.len() == 1 {
            "kaltstellen: new program".to_owned()
        } else {
            format!("kaltstellen: {} new programs", names.len())
        };
        let text = format!(
            "{list} started and {} no entry in {profile}. Open kaltstellen to add {}.",
            if names.len() == 1 { "has" } else { "have" },
            if names.len() == 1 { "it" } else { "them" },
        );
        Some((title, text))
    }

    /// Drops a program from the notification queue (it was noted before).
    pub fn forget(&mut self, exe: &str) {
        self.waiting.retain(|n| !n.eq_ignore_ascii_case(exe));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(seq: u64, kind: &str, pid: u32, exe: &str, time: u64) -> Event {
        Event {
            seq,
            kind: kind.to_owned(),
            pid,
            parent: 0,
            session: Some(1),
            created: time,
            time,
            exe: exe.to_owned(),
            path: Some(format!("C:\\Apps\\{exe}")),
        }
    }

    /// The end of the process that started at `created`.
    fn stop(seq: u64, pid: u32, exe: &str, created: u64, time: u64) -> Event {
        Event {
            created,
            ..event(seq, "stop", pid, exe, time)
        }
    }

    fn ctx<'a>(decided: &'a HashSet<String>) -> Context<'a> {
        Context {
            decided,
            dismissed: None,
        }
    }

    fn watch() -> StartWatch {
        let mut w = StartWatch::default();
        w.set_enabled(true);
        w.activated("Focus", 100);
        w
    }

    fn fetched(events: Vec<Event>, next: u64) -> Result<Option<Fetched>, String> {
        Ok(Some(Fetched { events, next }))
    }

    #[test]
    fn h9_a_program_that_stays_running_is_offered_once() {
        let mut w = watch();
        let none = HashSet::new();
        w.feed(
            110,
            fetched(
                vec![
                    event(1, "start", 10, "chat.exe", 105),
                    event(2, "start", 11, "chat.exe", 106),
                ],
                3,
            ),
        );
        assert!(w.confirmed(110, &ctx(&none)).is_empty(), "too young");
        let found = w.confirmed(116, &ctx(&none));
        assert_eq!(found.len(), 1, "two processes of one program count once");
        assert_eq!(found[0].0, "chat.exe");
        // Starts again later: not offered again.
        w.feed(
            130,
            fetched(vec![event(3, "start", 12, "chat.exe", 125)], 4),
        );
        assert!(w.confirmed(140, &ctx(&none)).is_empty());
    }

    #[test]
    fn h9_a_short_lived_process_is_not_offered() {
        let mut w = watch();
        let none = HashSet::new();
        w.feed(
            110,
            fetched(
                vec![
                    event(1, "start", 10, "tool.exe", 105),
                    stop(2, 10, "tool.exe", 105, 106),
                ],
                3,
            ),
        );
        assert!(w.confirmed(130, &ctx(&none)).is_empty());
        // A stop of another process with the same pid number does not
        // cancel a later start.
        w.feed(
            140,
            fetched(
                vec![
                    event(3, "start", 10, "tool.exe", 135),
                    event(4, "stop", 10, "other.exe", 120),
                ],
                5,
            ),
        );
        assert_eq!(w.confirmed(150, &ctx(&none)).len(), 1);
    }

    #[test]
    fn h9_a_late_answer_with_start_and_stop_keeps_a_long_lived_process() {
        let mut w = watch();
        let none = HashSet::new();
        w.feed(
            200,
            fetched(
                vec![
                    event(1, "start", 11, "tool.exe", 105),
                    stop(2, 11, "tool.exe", 105, 125),
                ],
                3,
            ),
        );
        assert_eq!(w.confirmed(200, &ctx(&none)).len(), 1);
    }

    #[test]
    fn h9_starts_before_the_activation_are_ignored() {
        let mut w = watch();
        let none = HashSet::new();
        w.feed(110, fetched(vec![event(1, "start", 10, "old.exe", 90)], 2));
        assert!(w.confirmed(200, &ctx(&none)).is_empty());
    }

    #[test]
    fn h9_entries_dismissed_protected_system_and_pathless_programs_are_left_out() {
        let none = HashSet::new();
        let decided: HashSet<String> = ["known.exe".to_owned()].into();
        let dismissed: BTreeSet<String> = ["quiet.exe".to_owned()].into();
        let with = Context {
            decided: &decided,
            dismissed: Some(&dismissed),
        };
        assert!(eligible(&event(1, "start", 1, "new.exe", 1), &ctx(&none)));
        assert!(!eligible(&event(1, "start", 1, "KNOWN.exe", 1), &with));
        assert!(!eligible(&event(1, "start", 1, "quiet.exe", 1), &with));
        assert!(!eligible(
            &event(1, "start", 1, "kaltstellen.exe", 1),
            &ctx(&none)
        ));
        let mut windows = event(1, "start", 1, "svchost.exe", 1);
        windows.path = Some("C:\\Windows\\System32\\svchost.exe".to_owned());
        assert!(!eligible(&windows, &ctx(&none)));
        let mut pathless = event(1, "start", 1, "new.exe", 1);
        pathless.path = None;
        assert!(!eligible(&pathless, &ctx(&none)));
        assert!(!eligible(
            &event(1, "start", 1, "lsass.exe", 1),
            &ctx(&none)
        ));
    }

    #[test]
    fn h9_one_notification_per_gap_names_the_programs_together() {
        let mut w = watch();
        let none = HashSet::new();
        w.feed(
            110,
            fetched(
                vec![
                    event(1, "start", 10, "a.exe", 101),
                    event(2, "start", 11, "b.exe", 101),
                    event(3, "start", 12, "c.exe", 101),
                    event(4, "start", 13, "d.exe", 101),
                ],
                5,
            ),
        );
        assert_eq!(w.confirmed(120, &ctx(&none)).len(), 4);
        let (title, text) = w.announcement(120, "Focus").unwrap();
        assert_eq!(title, "kaltstellen: 4 new programs");
        assert!(
            text.starts_with("a.exe, b.exe, c.exe and 1 more started and have no entry in Focus")
        );
        w.feed(125, fetched(vec![event(5, "start", 14, "e.exe", 121)], 6));
        assert_eq!(w.confirmed(135, &ctx(&none)).len(), 1);
        assert!(w.announcement(135, "Focus").is_none(), "gap not over");
        let (title, text) = w.announcement(151, "Focus").unwrap();
        assert_eq!(title, "kaltstellen: new program");
        assert!(text.starts_with("e.exe started and has no entry in Focus"));
        assert!(w.announcement(300, "Focus").is_none(), "nothing waiting");
    }

    #[test]
    fn h9_a_helper_that_does_not_answer_is_asked_less_and_less_often() {
        let mut w = watch();
        assert_eq!(w.due(100), Some(None));
        w.feed(100, Ok(None));
        assert_eq!(w.due(103), None);
        assert_eq!(w.due(104), Some(None));
        w.feed(104, Err("pipe".to_owned()));
        assert_eq!(w.due(111), None, "8 s");
        assert!(w.due(112).is_some());
        for step in 0..8 {
            w.feed(200 + step * 100, Ok(None));
        }
        assert_eq!(w.due(959), None, "at most a minute: 900 + 60");
        assert!(w.due(960).is_some());
        // An answer ends the backoff and remembers where to continue.
        w.feed(970, fetched(Vec::new(), 7));
        assert_eq!(w.due(971), Some(Some(7)));
    }

    #[test]
    fn h9_off_or_neutral_or_a_new_activation_watches_nothing_or_starts_over() {
        let mut w = watch();
        let none = HashSet::new();
        w.set_enabled(false);
        assert_eq!(w.due(1000), None);
        w.set_enabled(true);
        w.feed(110, fetched(vec![event(1, "start", 10, "a.exe", 105)], 2));
        w.activated("Focus", 111);
        assert!(w.confirmed(200, &ctx(&none)).is_empty(), "dropped");
        w.activated("neutral", 120);
        assert_eq!(w.due(1000), None);
        assert_eq!(w.profile(), "");
    }

    #[test]
    fn the_age_of_the_activation() {
        let mut w = StartWatch::default();
        assert_eq!(w.age(100), 0, "never applied");
        w.activated("focus", 90);
        assert_eq!(w.age(100), 10);
        assert_eq!(w.age(80), u64::MAX, "a clock that went back");
    }
}
