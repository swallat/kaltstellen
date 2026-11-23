//! Automatic triggers (U4): which profile the trigger programs ask for, and
//! when. Pure logic, driven by the tray's poll of the handle-free process
//! snapshot; no Win32 here. Decided in `docs/design-automatic-triggers.md`:
//!
//! - A trigger program starting applies its profile; the last one ending
//!   (gone for the debounce time) returns to the profile from before the
//!   first trigger, but only if the trigger's profile is still active.
//! - Two profiles with running triggers: the higher `priority` wins, at
//!   equal priority the one that came first. A higher one takes over from a
//!   lower one and hands back to it when it ends, if it still runs.
//! - A manual switch holds automation until the trigger situation changes
//!   (a trigger program starts or ends).
//! - The tray menu item `Automatic switching` pauses everything.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::config::Profile;
use crate::process::ProcessInfo;

/// Profile names compare like `Config::profile` does: trimmed, any case.
fn same_profile(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// How long a trigger program must stay gone before it counts as ended
/// (decided detail 5: flapping of a program that restarts quickly).
pub const DEBOUNCE: Duration = Duration::from_secs(5);

/// One profile with triggers as one poll saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub profile: String,
    pub priority: i32,
    /// The trigger program found running, `None` if none runs.
    pub program: Option<String>,
}

/// What automation asks the tray to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Queue an automatic switch to `profile` (W3, tagged automatic).
    /// `previous` is the profile it leaves (for `Back to <previous>`),
    /// `reason` says why, for the log and the notification.
    Switch {
        profile: String,
        previous: String,
        reason: String,
    },
}

/// A profile whose trigger counts as running.
#[derive(Debug, Clone)]
struct Running {
    profile: String,
    priority: i32,
    /// Position in the file; breaks ties within one poll.
    order: usize,
    program: String,
    /// When it started to count as running: equal priority keeps the
    /// first (conflict rule 2).
    since: Instant,
    /// When a trigger program of it was last seen (debounce).
    last_seen: Instant,
}

/// Automation owns the active profile: it applied `applied`, which was
/// reached from `base`, the profile active before the first trigger.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Owner {
    base: String,
    applied: String,
    program: String,
}

/// What the menu and `kaltstellen status` show about automation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct View {
    pub paused: bool,
    /// A manual switch holds automation until the trigger situation
    /// changes.
    pub held: bool,
    /// The active profile was applied by a trigger: (profile, program,
    /// the profile it returns to).
    pub applied: Option<(String, String, String)>,
    /// Profiles whose trigger runs but which wait for another one:
    /// (waiting profile, the profile it waits for).
    pub waiting: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct Automation {
    debounce: Duration,
    running: Vec<Running>,
    owner: Option<Owner>,
    held: bool,
    paused: bool,
}

impl Automation {
    pub fn new(debounce: Duration, paused: bool) -> Self {
        Self {
            debounce,
            running: Vec::new(),
            owner: None,
            held: false,
            paused,
        }
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// The menu item `Automatic switching` (decided detail 7). Pausing ends
    /// what automation owns: nothing returns by itself afterwards. Going on
    /// again starts from scratch: trigger programs that run count as
    /// starting at the next poll. `[assumed]`
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        self.restart();
    }

    /// Forgets what was observed: the next poll sees every running trigger
    /// as starting (tray start, automation switched on again, a cancelled
    /// end of session). What automation owns is dropped.
    pub fn restart(&mut self) {
        self.running.clear();
        self.owner = None;
        self.held = false;
    }

    /// A manual switch (menu, hotkey, command line, window, `Back to`):
    /// automation holds until the trigger situation changes (conflict
    /// rule 1), and the target is the user's, so nothing returns from it
    /// when the trigger ends (conflict rule 5). Returns whether a hold
    /// began (for the log).
    pub fn manual(&mut self) -> bool {
        self.owner = None;
        let began = !self.held && !self.running.is_empty();
        // Only meaningful while a trigger runs: otherwise the next trigger
        // start is a change anyway.
        self.held = !self.running.is_empty();
        began
    }

    /// The automatic request was not queued (a waiting manual request
    /// wins, W3): automation does not own what will be active.
    pub fn refused(&mut self) {
        self.owner = None;
    }

    /// S5: the profile to offer after sign-in instead of `active`: the one
    /// from before the trigger, when the trigger's profile is active.
    pub fn base_of(&self, active: &str) -> Option<&str> {
        self.owner
            .as_ref()
            .filter(|o| same_profile(&o.applied, active))
            .map(|o| o.base.as_str())
    }

    /// The winning running trigger: highest priority, then the first that
    /// came, then file order.
    fn winner(&self) -> Option<&Running> {
        self.running.iter().min_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then(a.since.cmp(&b.since))
                .then(a.order.cmp(&b.order))
        })
    }

    /// One poll. `seen` lists every profile with triggers in file order;
    /// `active` is the profile that is active once the running and waiting
    /// switches are done. Returns what to queue, and log lines.
    pub fn poll(
        &mut self,
        now: Instant,
        seen: &[Seen],
        active: &str,
    ) -> (Option<Action>, Vec<String>) {
        let mut log = Vec::new();
        let mut changed = false;
        let mut ended_program = None;
        let winner_before = self.winner().map(|w| w.profile.to_lowercase());
        // Profiles that left the configuration end at once.
        self.running.retain(|r| {
            let kept = seen.iter().any(|s| same_profile(&s.profile, &r.profile));
            if !kept {
                log.push(format!(
                    "trigger of \"{}\" ended: the profile has no triggers any more",
                    r.profile
                ));
                changed = true;
            }
            kept
        });
        for (order, s) in seen.iter().enumerate() {
            let at = self
                .running
                .iter()
                .position(|r| same_profile(&r.profile, &s.profile));
            match (&s.program, at) {
                (Some(program), Some(at)) => {
                    let r = &mut self.running[at];
                    r.last_seen = now;
                    r.program = program.clone();
                    r.priority = s.priority;
                    r.order = order;
                }
                (Some(program), None) => {
                    log.push(format!(
                        "trigger {program} of \"{}\" started (priority {})",
                        s.profile, s.priority
                    ));
                    self.running.push(Running {
                        profile: s.profile.clone(),
                        priority: s.priority,
                        order,
                        program: program.clone(),
                        since: now,
                        last_seen: now,
                    });
                    changed = true;
                }
                (None, Some(at)) => {
                    let r = &mut self.running[at];
                    r.priority = s.priority;
                    r.order = order;
                    if now.saturating_duration_since(r.last_seen) >= self.debounce {
                        log.push(format!("trigger {} of \"{}\" ended", r.program, r.profile));
                        ended_program = Some(r.program.clone());
                        self.running.remove(at);
                        changed = true;
                    }
                }
                (None, None) => {}
            }
        }
        if self.paused {
            return (None, log);
        }
        let winner = self
            .winner()
            .map(|w| (w.profile.clone(), w.program.clone()));
        if !changed {
            // A changed configuration may make another trigger win: decided
            // again, but a manual hold stays, as no trigger program started
            // or ended.
            let reordered = winner.as_ref().map(|(p, _)| p.to_lowercase()) != winner_before;
            if !reordered || self.held {
                return (None, log);
            }
            log.push("the priorities changed; deciding again".to_owned());
        }
        if std::mem::take(&mut self.held) {
            log.push("the trigger situation changed; automatic switching goes on".to_owned());
        }
        for r in &self.running {
            if let Some((profile, _)) = &winner
                && !same_profile(&r.profile, profile)
            {
                log.push(format!(
                    "trigger for \"{}\" waits while \"{profile}\" is active",
                    r.profile
                ));
            }
        }
        let action = match (winner, self.owner.as_mut()) {
            (Some((profile, program)), None) => {
                if same_profile(active, &profile) {
                    // Already active by the user's choice: nothing to do,
                    // and nothing to return from. `[assumed]`
                    log.push(format!(
                        "\"{profile}\" is active already; the trigger changes nothing"
                    ));
                    None
                } else {
                    self.owner = Some(Owner {
                        base: active.to_owned(),
                        applied: profile.clone(),
                        program: program.clone(),
                    });
                    Some(Action::Switch {
                        profile,
                        previous: active.to_owned(),
                        reason: format!("{program} started"),
                    })
                }
            }
            (Some((profile, program)), Some(owner)) => {
                if same_profile(&owner.applied, &profile) {
                    owner.program = program;
                    None
                } else {
                    // Takeover by a higher trigger, or handing back to a
                    // lower one that still runs.
                    let reason = match &ended_program {
                        Some(ended) => format!("{ended} ended, {program} still runs"),
                        None => format!("{program} started"),
                    };
                    if same_profile(&owner.base, &profile) {
                        // The winner is the profile from before the first
                        // trigger, the user's own choice: automation does
                        // not own it, so nothing returns from it later.
                        self.owner = None;
                        if same_profile(active, &profile) {
                            return (None, log);
                        }
                    } else {
                        owner.applied = profile.clone();
                        owner.program = program;
                    }
                    Some(Action::Switch {
                        profile,
                        previous: active.to_owned(),
                        reason,
                    })
                }
            }
            (None, Some(_)) => {
                let owner = self.owner.take().expect("matched above");
                if same_profile(active, &owner.base) {
                    // Back already: nothing to apply again.
                    None
                } else if same_profile(active, &owner.applied) {
                    Some(Action::Switch {
                        profile: owner.base,
                        previous: active.to_owned(),
                        reason: format!("{} ended", ended_program.unwrap_or(owner.program)),
                    })
                } else {
                    // Conflict rule 5: the user switched meanwhile.
                    log.push(format!(
                        "\"{}\" is no longer active; not returning to \"{}\"",
                        owner.applied, owner.base
                    ));
                    None
                }
            }
            (None, None) => None,
        };
        (action, log)
    }

    /// The menu and status view. `active` as for [`Automation::poll`].
    pub fn view(&self, active: &str) -> View {
        let winner = self.winner().map(|w| w.profile.clone());
        View {
            paused: self.paused,
            held: self.held,
            applied: self
                .owner
                .as_ref()
                .filter(|o| same_profile(&o.applied, active))
                .map(|o| (o.applied.clone(), o.program.clone(), o.base.clone())),
            // Only while the winner is the active profile and no manual
            // switch holds automation does anything wait for it.
            waiting: match &winner {
                Some(winner) if !self.paused && !self.held && same_profile(winner, active) => self
                    .running
                    .iter()
                    .filter(|r| !same_profile(&r.profile, winner))
                    .map(|r| (r.profile.clone(), winner.clone()))
                    .collect(),
                _ => Vec::new(),
            },
        }
    }
}

/// Whether any profile has triggers: only then the tray polls.
pub fn configured(profiles: &[Profile]) -> bool {
    profiles.iter().any(|p| !p.triggers.is_empty())
}

/// Which trigger program of each profile runs in `session`, from a
/// snapshot of the processes. A trigger with a path needs the image path
/// of the processes of its name; `image` reads it (opening only those
/// processes, A7) and may cache it. Profiles without triggers are left out.
pub fn scan(
    profiles: &[Profile],
    processes: &[ProcessInfo],
    session: Option<u32>,
    image: &mut dyn FnMut(&ProcessInfo) -> Option<PathBuf>,
) -> Vec<Seen> {
    profiles
        .iter()
        .filter(|p| !p.triggers.is_empty())
        .map(|p| {
            let program = p.triggers.iter().find_map(|t| {
                processes
                    .iter()
                    .filter(|proc| {
                        Some(proc.session_id) == session && proc.name.eq_ignore_ascii_case(&t.exe)
                    })
                    .find(|proc| match &t.path {
                        None => true,
                        Some(path) => image(proc)
                            .is_some_and(|i| crate::process::same_path(&i, Path::new(path))),
                    })
                    .map(|_| t.exe.clone())
            });
            Seen {
                profile: p.name.clone(),
                priority: p.priority,
                program,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Trigger;

    const S: Duration = Duration::from_secs(1);

    fn seen(list: &[(&str, i32, Option<&str>)]) -> Vec<Seen> {
        list.iter()
            .map(|(profile, priority, program)| Seen {
                profile: (*profile).to_owned(),
                priority: *priority,
                program: program.map(str::to_owned),
            })
            .collect()
    }

    fn switch(profile: &str, previous: &str) -> Option<String> {
        Some(format!("{profile}<-{previous}"))
    }

    /// The action as `profile<-previous`.
    fn short(action: Option<Action>) -> Option<String> {
        action.map(
            |Action::Switch {
                 profile, previous, ..
             }| format!("{profile}<-{previous}"),
        )
    }

    struct Run {
        a: Automation,
        t0: Instant,
    }

    impl Run {
        fn new() -> Self {
            Run {
                a: Automation::new(DEBOUNCE, false),
                t0: Instant::now(),
            }
        }

        fn at(
            &mut self,
            secs: u64,
            list: &[(&str, i32, Option<&str>)],
            active: &str,
        ) -> Option<String> {
            short(
                self.a
                    .poll(self.t0 + S * secs as u32, &seen(list), active)
                    .0,
            )
        }
    }

    #[test]
    fn u4_start_applies_and_the_end_returns_after_the_debounce() {
        let mut r = Run::new();
        assert_eq!(r.at(0, &[("flight", 0, None)], "neutral"), None);
        assert_eq!(
            r.at(3, &[("flight", 0, Some("sim.exe"))], "neutral"),
            switch("flight", "neutral")
        );
        assert_eq!(r.at(6, &[("flight", 0, Some("sim.exe"))], "flight"), None);
        assert_eq!(
            r.a.view("flight").applied,
            Some(("flight".into(), "sim.exe".into(), "neutral".into()))
        );
        // Gone, but not for 5 s yet; back within the debounce: nothing.
        assert_eq!(r.at(9, &[("flight", 0, None)], "flight"), None);
        assert_eq!(r.at(12, &[("flight", 0, Some("sim.exe"))], "flight"), None);
        assert_eq!(r.at(15, &[("flight", 0, None)], "flight"), None);
        assert_eq!(
            r.at(16, &[("flight", 0, None)], "flight"),
            None,
            "4 s gone only"
        );
        assert_eq!(
            r.at(17, &[("flight", 0, None)], "flight"),
            switch("neutral", "flight")
        );
        assert_eq!(r.a.view("neutral"), View::default());
    }

    #[test]
    fn u4_return_goes_to_the_profile_before_the_trigger() {
        let mut r = Run::new();
        assert_eq!(
            r.at(0, &[("flight", 0, Some("sim.exe"))], "work"),
            switch("flight", "work")
        );
        assert_eq!(r.a.base_of("flight"), Some("work"), "S5 offer");
        assert_eq!(r.a.base_of("other"), None);
        assert_eq!(
            r.at(10, &[("flight", 0, None)], "flight"),
            switch("work", "flight")
        );
    }

    #[test]
    fn u4_rule5_no_return_when_the_user_switched_meanwhile() {
        let mut r = Run::new();
        r.at(0, &[("flight", 0, Some("sim.exe"))], "neutral");
        // A manual switch holds automation and ends its ownership.
        assert!(r.a.manual());
        assert!(r.a.view("work").held);
        assert_eq!(r.a.view("work").applied, None);
        assert_eq!(r.at(3, &[("flight", 0, Some("sim.exe"))], "work"), None);
        // The end of the trigger is a change: the hold ends, but nothing
        // returns because the user chose "work".
        assert_eq!(r.at(10, &[("flight", 0, None)], "work"), None);
        assert!(!r.a.view("work").held);
    }

    #[test]
    fn u4_rule1_manual_holds_until_a_trigger_starts() {
        let mut r = Run::new();
        let list = |b: Option<&'static str>| vec![("flight", 0, Some("sim.exe")), ("music", 0, b)];
        r.at(0, &list(None), "neutral");
        r.a.manual();
        assert_eq!(r.at(3, &list(None), "neutral"), None, "held");
        // Another trigger starts: the hold ends, and the winner (the first
        // at equal priority) is applied again.
        assert_eq!(
            r.at(6, &list(Some("player.exe")), "neutral"),
            switch("flight", "neutral")
        );
    }

    #[test]
    fn u4_manual_without_a_running_trigger_holds_nothing() {
        let mut r = Run::new();
        assert!(!r.a.manual());
        assert!(!r.a.view("x").held);
        assert_eq!(
            r.at(0, &[("flight", 0, Some("sim.exe"))], "work"),
            switch("flight", "work")
        );
    }

    #[test]
    fn u4_rule2_higher_priority_takes_over_and_hands_back() {
        let mut r = Run::new();
        let both = |low: Option<&'static str>, high: Option<&'static str>| {
            vec![("low", 1, low), ("high", 5, high)]
        };
        assert_eq!(
            r.at(0, &both(Some("a.exe"), None), "neutral"),
            switch("low", "neutral")
        );
        assert_eq!(
            r.at(3, &both(Some("a.exe"), Some("b.exe")), "low"),
            switch("high", "low")
        );
        assert_eq!(r.a.view("high").applied.unwrap().2, "neutral", "base kept");
        // The higher one ends while the lower still runs: back to it.
        assert_eq!(r.at(6, &both(Some("a.exe"), None), "high"), None);
        assert_eq!(
            r.at(11, &both(Some("a.exe"), None), "high"),
            switch("low", "high")
        );
        // Then the lower ends: back to the profile before the first.
        assert_eq!(r.at(14, &both(None, None), "low"), None);
        assert_eq!(r.at(20, &both(None, None), "low"), switch("neutral", "low"));
    }

    #[test]
    fn u4_rule2_higher_ends_after_the_lower_returns_to_the_base() {
        let mut r = Run::new();
        let both = |low: Option<&'static str>, high: Option<&'static str>| {
            vec![("low", 1, low), ("high", 5, high)]
        };
        r.at(0, &both(Some("a.exe"), None), "neutral");
        r.at(1, &both(Some("a.exe"), Some("b.exe")), "low");
        // The lower one ends first: nothing changes for the winner.
        assert_eq!(r.at(7, &both(None, Some("b.exe")), "high"), None);
        assert_eq!(r.at(10, &both(None, None), "high"), None);
        assert_eq!(
            r.at(12, &both(None, None), "high"),
            switch("neutral", "high")
        );
    }

    #[test]
    fn u4_hand_back_to_the_users_own_profile_ends_ownership() {
        let mut r = Run::new();
        let both = |low: Option<&'static str>, high: Option<&'static str>| {
            vec![("low", 1, low), ("high", 5, high)]
        };
        // The user chose "low" while its trigger runs: nothing to do.
        assert_eq!(r.at(0, &both(Some("a.exe"), None), "low"), None);
        assert_eq!(r.a.view("low").applied, None);
        // A higher one takes over from the user's profile...
        assert_eq!(
            r.at(1, &both(Some("a.exe"), Some("b.exe")), "low"),
            switch("high", "low")
        );
        // ...and hands back to it; that is the user's profile, not owned.
        assert_eq!(
            r.at(7, &both(Some("a.exe"), None), "high"),
            switch("low", "high")
        );
        assert_eq!(r.a.view("low").applied, None);
        assert_eq!(r.a.base_of("low"), None);
        // When "low"'s own trigger ends, nothing is applied again.
        assert_eq!(r.at(14, &both(None, None), "low"), None);
        // The hand-back when the user's profile is active already: nothing.
        let mut r = Run::new();
        r.at(0, &both(Some("a.exe"), None), "low");
        r.at(1, &both(Some("a.exe"), Some("b.exe")), "low");
        assert_eq!(r.at(7, &both(Some("a.exe"), None), "low"), None);
    }

    #[test]
    fn u4_return_skips_when_the_base_is_active_already() {
        let mut r = Run::new();
        r.at(0, &[("flight", 0, Some("sim.exe"))], "work");
        assert_eq!(r.at(10, &[("flight", 0, None)], "work"), None);
    }

    #[test]
    fn u4_changed_priorities_decide_again_unless_held() {
        let mut r = Run::new();
        let list = |a: i32, b: i32| vec![("a", a, Some("a.exe")), ("b", b, Some("b.exe"))];
        assert_eq!(r.at(0, &list(5, 1), "neutral"), switch("a", "neutral"));
        assert_eq!(r.at(1, &list(5, 1), "a"), None);
        assert_eq!(r.at(2, &list(1, 5), "a"), switch("b", "a"));
        // Held by a manual switch: a priority change alone does not act.
        r.a.manual();
        assert_eq!(r.at(3, &list(5, 1), "x"), None);
        assert!(r.a.view("x").held);
    }

    #[test]
    fn u4_waiting_is_shown_only_while_the_winner_is_active_and_not_held() {
        let mut r = Run::new();
        let list = vec![("first", 3, Some("a.exe")), ("second", 1, Some("b.exe"))];
        r.at(0, &list, "neutral");
        assert_eq!(r.a.view("neutral").waiting, vec![], "not active yet");
        assert_eq!(
            r.a.view("first").waiting,
            vec![("second".to_owned(), "first".to_owned())]
        );
        r.a.manual();
        assert_eq!(r.a.view("first").waiting, vec![], "held");
    }

    #[test]
    fn u4_rule2_lower_or_equal_waits_and_is_shown() {
        let mut r = Run::new();
        let list =
            |second: Option<&'static str>| vec![("first", 3, Some("a.exe")), ("second", 3, second)];
        assert_eq!(r.at(0, &list(None), "neutral"), switch("first", "neutral"));
        let (action, log) = r.a.poll(r.t0 + S, &seen(&list(Some("b.exe"))), "first");
        assert_eq!(action, None, "equal priority keeps the first");
        assert!(
            log.iter()
                .any(|l| l == "trigger for \"second\" waits while \"first\" is active"),
            "{log:?}"
        );
        assert_eq!(
            r.a.view("first").waiting,
            vec![("second".to_owned(), "first".to_owned())]
        );
        // Both start in one poll at equal priority: file order decides.
        let mut r = Run::new();
        assert_eq!(
            r.at(
                0,
                &[("x", 0, Some("x.exe")), ("y", 0, Some("y.exe"))],
                "neutral"
            ),
            switch("x", "neutral")
        );
    }

    #[test]
    fn u4_pause_stops_everything_and_going_on_starts_afresh() {
        let mut r = Run::new();
        r.at(0, &[("flight", 0, Some("sim.exe"))], "neutral");
        r.a.set_paused(true);
        assert!(r.a.view("flight").paused);
        assert_eq!(r.a.view("flight").applied, None, "pausing ends ownership");
        assert_eq!(r.at(3, &[("flight", 0, Some("sim.exe"))], "flight"), None);
        assert_eq!(
            r.at(10, &[("flight", 0, None)], "flight"),
            None,
            "no return"
        );
        r.a.set_paused(false);
        // A trigger that runs when automation goes on counts as starting.
        assert_eq!(
            r.at(13, &[("flight", 0, Some("sim.exe"))], "work"),
            switch("flight", "work")
        );
    }

    #[test]
    fn u4_trigger_of_the_profile_already_active_changes_nothing() {
        let mut r = Run::new();
        assert_eq!(r.at(0, &[("flight", 0, Some("sim.exe"))], "FLIGHT"), None);
        assert_eq!(r.a.view("flight").applied, None);
        assert_eq!(r.at(10, &[("flight", 0, None)], "flight"), None);
    }

    #[test]
    fn u4_a_refused_request_owns_nothing() {
        let mut r = Run::new();
        r.at(0, &[("flight", 0, Some("sim.exe"))], "neutral");
        r.a.refused();
        assert_eq!(r.a.base_of("flight"), None);
        assert_eq!(r.at(10, &[("flight", 0, None)], "flight"), None);
    }

    #[test]
    fn u4_a_profile_that_loses_its_triggers_ends_at_once() {
        let mut r = Run::new();
        r.at(0, &[("flight", 0, Some("sim.exe"))], "neutral");
        assert_eq!(r.at(1, &[], "flight"), switch("neutral", "flight"));
    }

    fn process(pid: u32, name: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: 0,
            name: name.to_owned(),
            session_id: 1,
            created: 1,
            private_bytes: 0,
        }
    }

    fn profile(name: &str, triggers: &[(&str, Option<&str>)]) -> Profile {
        Profile {
            name: name.to_owned(),
            hotkey: None,
            apps: Vec::new(),
            triggers: triggers
                .iter()
                .map(|(exe, path)| Trigger {
                    exe: (*exe).to_owned(),
                    path: path.map(PathBuf::from),
                })
                .collect(),
            priority: 2,
        }
    }

    #[test]
    fn u4_scan_matches_names_and_opens_only_processes_of_path_triggers() {
        let profiles = vec![
            profile("plain", &[]),
            profile("flight", &[("sim.exe", None)]),
            profile("pinned", &[("tool.exe", Some(r"C:\B\tool.exe"))]),
        ];
        let processes = vec![
            process(10, "SIM.EXE"),
            process(11, "tool.exe"),
            process(12, "other.exe"),
        ];
        let mut opened = Vec::new();
        let mut image = |p: &ProcessInfo| {
            opened.push(p.pid);
            Some(PathBuf::from(r"c:\a\TOOL.exe"))
        };
        let result = scan(&profiles, &processes, Some(1), &mut image);
        assert_eq!(
            result,
            vec![
                Seen {
                    profile: "flight".into(),
                    priority: 2,
                    program: Some("sim.exe".into())
                },
                Seen {
                    profile: "pinned".into(),
                    priority: 2,
                    program: None
                },
            ]
        );
        assert_eq!(
            opened,
            vec![11],
            "only the process of the path trigger's name"
        );
        let mut image = |_: &ProcessInfo| Some(PathBuf::from(r"c:/b/TOOL.exe"));
        assert_eq!(
            scan(&profiles, &processes, Some(1), &mut image)[1]
                .program
                .as_deref(),
            Some("tool.exe")
        );
        assert!(configured(&profiles));
        assert!(!configured(&profiles[..1]));
    }
}
