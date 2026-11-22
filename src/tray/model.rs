//! Pure logic of the tray app: request queue (W3), status and icon state
//! (W2), and the menu built from the configuration. No Win32 here; the
//! window code in `win32::tray` only draws what this module decides.

use crate::config::NEUTRAL;

/// Profile names compare like `Config::profile` does: trimmed, any case.
fn same_profile(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Work for the worker thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// S3 at start, or `kaltstellen recover`: resume what is recorded as
    /// frozen; `neutral`.
    Recover,
    /// `force` confirms warn-list programs for this request only (P6).
    Switch { target: String, force: bool },
    /// L2: resume everything suspended, keep stopped records, then exit.
    Quit,
    /// S5 at `WM_QUERYENDSESSION`: resume everything suspended, stay.
    EndSession,
}

impl Job {
    pub fn switch(target: &str) -> Self {
        Job::Switch {
            target: target.trim().to_owned(),
            force: false,
        }
    }

    /// The profile that is active once the job is done (W3 compares these).
    pub fn leads_to(&self) -> &str {
        match self {
            Job::Switch { target, .. } => target,
            Job::Recover | Job::Quit | Job::EndSession => NEUTRAL,
        }
    }
}

impl std::fmt::Display for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Job::Recover => f.write_str("recover"),
            Job::Switch { target, .. } => write!(f, "switch to \"{target}\""),
            Job::Quit => f.write_str("quit"),
            Job::EndSession => f.write_str("end of session"),
        }
    }
}

/// What the queue decided about a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Nothing runs: start this job now.
    Start(Job),
    /// A job runs; the request waits. A request it replaced is returned as
    /// superseded (W3, exit code 3 over the pipe).
    Queued { superseded: Option<Job> },
    /// The request leads to the profile the running job leads to, so it is
    /// dropped and shares the running job's result; a request it replaced
    /// is returned as superseded.
    Dropped { superseded: Option<Job> },
}

/// W3: while a job runs, at most one further request waits. The latest
/// wins; a waiting request equal to the profile the running job leads to is
/// dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Queue {
    running: Option<Job>,
    pending: Option<Job>,
}

impl Queue {
    pub fn request(&mut self, job: Job) -> Decision {
        let Some(running) = &self.running else {
            self.running = Some(job.clone());
            return Decision::Start(job);
        };
        let superseded = self.pending.take();
        if same_profile(running.leads_to(), job.leads_to()) {
            Decision::Dropped { superseded }
        } else {
            self.pending = Some(job);
            Decision::Queued { superseded }
        }
    }

    /// Starts `job` at once if nothing runs; otherwise it runs next and the
    /// waiting request, if any, is returned as superseded (Quit, end of
    /// session). `Err(superseded)` means it was queued.
    pub fn jump(&mut self, job: Job) -> Result<Job, Option<Job>> {
        if self.running.is_none() {
            self.running = Some(job.clone());
            return Ok(job);
        }
        Err(self.pending.replace(job))
    }

    /// The running job ended. Returns the waiting request, which is now the
    /// running one.
    pub fn finish(&mut self) -> Option<Job> {
        self.running = self.pending.take();
        self.running.clone()
    }

    pub fn running(&self) -> Option<&Job> {
        self.running.as_ref()
    }

    pub fn pending(&self) -> Option<&Job> {
        self.pending.as_ref()
    }
}

/// The three icon states of spec section 8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Neutral,
    Active,
    Error,
}

/// What the tray knows about the last switch and the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub active: String,
    /// Target of the switch that runs right now.
    pub switching: Option<String>,
    /// Errors of the last finished switch (W2); empty after a clean one.
    pub switch_errors: Vec<String>,
    /// The configuration file is invalid (C3); the last valid one is used.
    pub config_error: Option<String>,
    /// The pipe for the command line could not be set up (L1).
    pub pipe_error: Option<String>,
    /// S5: the profile of the last session, offered for one click.
    pub offer: Option<String>,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            active: NEUTRAL.to_owned(),
            switching: None,
            switch_errors: Vec::new(),
            config_error: None,
            pipe_error: None,
            offer: None,
        }
    }
}

/// `szTip` of `NOTIFYICONDATAW` holds 128 UTF-16 units including the NUL.
pub const TOOLTIP_MAX: usize = 127;

impl Status {
    pub fn is_neutral(&self) -> bool {
        same_profile(&self.active, NEUTRAL)
    }

    pub fn icon(&self) -> Icon {
        if !self.switch_errors.is_empty()
            || self.config_error.is_some()
            || self.pipe_error.is_some()
        {
            Icon::Error
        } else if self.is_neutral() {
            Icon::Neutral
        } else {
            Icon::Active
        }
    }

    /// One switch finished. A clean switch clears the error state (spec
    /// section 8); one with errors sets it.
    pub fn finished(&mut self, active: String, errors: Vec<String>) {
        self.active = active;
        self.switching = None;
        self.switch_errors = errors;
    }

    pub fn tooltip(&self) -> String {
        let mut tip = format!("kaltstellen: {}", self.active);
        if let Some(target) = &self.switching {
            tip.push_str(&format!(" (switching to {target})"));
        }
        match self.switch_errors.len() {
            0 => {}
            1 => tip.push_str(", 1 error"),
            n => tip.push_str(&format!(", {n} errors")),
        }
        if self.config_error.is_some() {
            tip.push_str(", configuration invalid");
        }
        truncate(&tip, TOOLTIP_MAX)
    }

    /// The disabled status line, if there is anything to say.
    pub fn status_line(&self) -> Option<String> {
        let line = if let Some(target) = &self.switching {
            format!("Switching to {target}…")
        } else if let Some(first) = self.switch_errors.first() {
            match self.switch_errors.len() {
                1 => format!("Last switch: {first}"),
                n => format!("Last switch: {n} errors, first: {first}"),
            }
        } else if let Some(problem) = &self.config_error {
            format!("Configuration invalid: {problem}")
        } else if let Some(problem) = &self.pipe_error {
            format!("Command line cannot reach the tray: {problem}")
        } else {
            return None;
        };
        Some(truncate(&line, 100))
    }
}

/// Cuts at a character boundary and marks the cut.
fn truncate(text: &str, max_units: usize) -> String {
    if text.encode_utf16().count() <= max_units {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut units = 0;
    for c in text.chars() {
        units += c.len_utf16();
        if units > max_units - 1 {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

/// What a menu item does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Switch(String),
    Quit,
}

/// One line of the tray menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Separator,
    Entry {
        /// Command ID passed back by `WM_COMMAND`; 0 for disabled lines.
        id: u16,
        label: String,
        checked: bool,
        enabled: bool,
    },
}

pub const ID_NEUTRAL: u16 = 1;
pub const ID_QUIT: u16 = 2;
/// Applies the profile of the last session (S5).
pub const ID_LAST: u16 = 3;
/// Profile `n` of the configuration has ID `ID_PROFILE_BASE + n`.
pub const ID_PROFILE_BASE: u16 = 100;

/// The menu for the given profiles and status, with the command of each ID.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Menu {
    pub items: Vec<Item>,
    commands: Vec<(u16, Command)>,
}

impl Menu {
    /// Spec section 8: profiles (active one checked), `Neutral`, a status
    /// line when there is something to report, `Quit`. `Open kaltstellen…`
    /// belongs to the GUI (M3) and is shown disabled.
    pub fn build(profiles: &[String], status: &Status) -> Self {
        let mut menu = Menu::default();
        // S5: offered after sign-in or a cancelled shutdown, never applied
        // by itself; only while nothing else is active.
        if let Some(last) = &status.offer
            && status.is_neutral()
            && let Some(name) = profiles.iter().find(|p| same_profile(p, last))
        {
            menu.entry(
                ID_LAST,
                format!("Apply last profile: {}", escape(name)),
                false,
                Command::Switch(name.clone()),
            );
            menu.items.push(Item::Separator);
        }
        let mut active_listed = status.is_neutral();
        for (n, name) in profiles.iter().enumerate() {
            let Some(id) = u16::try_from(n)
                .ok()
                .and_then(|n| n.checked_add(ID_PROFILE_BASE))
            else {
                break;
            };
            let checked = same_profile(name, &status.active);
            active_listed |= checked;
            menu.entry(id, escape(name), checked, Command::Switch(name.clone()));
        }
        if !active_listed {
            // C4: removed or renamed while active; it stays active under its
            // old name until the next switch.
            menu.items.push(Item::Entry {
                id: 0,
                label: format!(
                    "{} (no longer in the configuration)",
                    escape(&status.active)
                ),
                checked: true,
                enabled: false,
            });
        }
        menu.entry(
            ID_NEUTRAL,
            "&Neutral".to_owned(),
            status.is_neutral(),
            Command::Switch(NEUTRAL.to_owned()),
        );
        menu.items.push(Item::Separator);
        if let Some(line) = status.status_line() {
            menu.disabled(escape(&line));
            menu.items.push(Item::Separator);
        }
        menu.disabled("Open kaltstellen…".to_owned());
        menu.entry(ID_QUIT, "&Quit".to_owned(), false, Command::Quit);
        menu
    }

    fn entry(&mut self, id: u16, label: String, checked: bool, command: Command) {
        self.items.push(Item::Entry {
            id,
            label,
            checked,
            enabled: true,
        });
        self.commands.push((id, command));
    }

    fn disabled(&mut self, label: String) {
        self.items.push(Item::Entry {
            id: 0,
            label,
            checked: false,
            enabled: false,
        });
    }

    pub fn command(&self, id: u16) -> Option<&Command> {
        self.commands.iter().find(|(i, _)| *i == id).map(|(_, c)| c)
    }
}

/// A `&` in a menu label marks the access key; show a literal one.
fn escape(label: &str) -> String {
    label.replace('&', "&&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sw(target: &str) -> Job {
        Job::switch(target)
    }

    #[test]
    fn w3_idle_queue_starts_at_once_even_for_the_active_profile() {
        let mut q = Queue::default();
        assert_eq!(q.request(sw(" focus ")), Decision::Start(sw("focus")));
        assert_eq!(q.running(), Some(&sw("focus")));
        assert_eq!(q.finish(), None);
        // W4: re-applying when idle is a real switch.
        assert_eq!(q.request(sw("focus")), Decision::Start(sw("focus")));
    }

    #[test]
    fn w3_only_the_latest_request_waits() {
        let mut q = Queue::default();
        q.request(sw("a"));
        assert_eq!(q.request(sw("b")), Decision::Queued { superseded: None });
        assert_eq!(
            q.request(sw("c")),
            Decision::Queued {
                superseded: Some(sw("b"))
            }
        );
        assert_eq!(q.pending(), Some(&sw("c")));
        assert_eq!(q.finish(), Some(sw("c")));
        assert_eq!(q.running(), Some(&sw("c")));
        assert_eq!(q.finish(), None);
        assert_eq!(q.running(), None);
    }

    #[test]
    fn w3_request_equal_to_the_coming_profile_is_dropped() {
        let mut q = Queue::default();
        q.request(sw("a"));
        assert_eq!(q.request(sw("A")), Decision::Dropped { superseded: None });
        assert_eq!(q.pending(), None);
        // The latest wins, and the latest equals what will be active: the
        // earlier waiting request goes too.
        q.request(sw("b"));
        assert_eq!(
            q.request(sw("a")),
            Decision::Dropped {
                superseded: Some(sw("b"))
            }
        );
        assert_eq!(q.finish(), None);
    }

    #[test]
    fn w3_recovery_counts_as_a_switch_to_neutral() {
        let mut q = Queue::default();
        q.request(Job::Recover);
        assert_eq!(
            q.request(sw("Neutral")),
            Decision::Dropped { superseded: None }
        );
        assert_eq!(q.request(sw("a")), Decision::Queued { superseded: None });
        assert_eq!(
            q.request(Job::Recover),
            Decision::Dropped {
                superseded: Some(sw("a"))
            }
        );
    }

    #[test]
    fn force_travels_with_the_request() {
        let mut q = Queue::default();
        q.request(sw("a"));
        let forced = Job::Switch {
            target: "b".into(),
            force: true,
        };
        q.request(forced.clone());
        assert_eq!(q.finish(), Some(forced));
    }

    #[test]
    fn quit_jumps_the_queue_and_supersedes_the_waiting_request() {
        let mut q = Queue::default();
        assert_eq!(q.jump(Job::Quit), Ok(Job::Quit));
        let mut q = Queue::default();
        q.request(sw("a"));
        q.request(sw("b"));
        assert_eq!(q.jump(Job::Quit), Err(Some(sw("b"))));
        assert_eq!(q.finish(), Some(Job::Quit));
    }

    #[test]
    fn w2_icon_follows_profile_and_errors() {
        let mut s = Status::default();
        assert_eq!(s.icon(), Icon::Neutral);
        s.finished("focus".into(), vec![]);
        assert_eq!(s.icon(), Icon::Active);
        s.finished("focus".into(), vec!["KS105 boom".into()]);
        assert_eq!(s.icon(), Icon::Error);
        // Cleared by the next successful switch.
        s.finished("neutral".into(), vec![]);
        assert_eq!(s.icon(), Icon::Neutral);
        s.config_error = Some("KS202 line 3".into());
        assert_eq!(s.icon(), Icon::Error);
    }

    #[test]
    fn tooltip_names_the_active_profile_and_fits() {
        let mut s = Status::default();
        assert_eq!(s.tooltip(), "kaltstellen: neutral");
        s.finished("focus".into(), vec!["x".into(), "y".into()]);
        s.switching = Some("music".into());
        assert_eq!(
            s.tooltip(),
            "kaltstellen: focus (switching to music), 2 errors"
        );
        s.active = "ä".repeat(300);
        let tip = s.tooltip();
        assert!(tip.encode_utf16().count() <= TOOLTIP_MAX);
        assert!(tip.ends_with('…'));
    }

    fn labels(menu: &Menu) -> Vec<String> {
        menu.items
            .iter()
            .map(|item| match item {
                Item::Separator => "-".to_owned(),
                Item::Entry {
                    label,
                    checked,
                    enabled,
                    ..
                } => format!(
                    "{}{}{}",
                    if *checked { "[x] " } else { "" },
                    label,
                    if *enabled { "" } else { " (disabled)" }
                ),
            })
            .collect()
    }

    #[test]
    fn menu_lists_profiles_neutral_and_quit() {
        let profiles = vec!["focus".to_owned(), "R&D".to_owned()];
        let status = Status {
            active: "FOCUS".into(),
            ..Status::default()
        };
        let menu = Menu::build(&profiles, &status);
        assert_eq!(
            labels(&menu),
            vec![
                "[x] focus",
                "R&&D",
                "&Neutral",
                "-",
                "Open kaltstellen… (disabled)",
                "&Quit"
            ]
        );
        assert_eq!(
            menu.command(ID_PROFILE_BASE + 1),
            Some(&Command::Switch("R&D".into()))
        );
        assert_eq!(
            menu.command(ID_NEUTRAL),
            Some(&Command::Switch("neutral".into()))
        );
        assert_eq!(menu.command(ID_QUIT), Some(&Command::Quit));
        assert_eq!(menu.command(0), None);
    }

    #[test]
    fn s5_menu_offers_the_last_profile_while_neutral() {
        let profiles = vec!["focus".to_owned()];
        let status = Status {
            offer: Some("FOCUS".into()),
            ..Status::default()
        };
        let menu = Menu::build(&profiles, &status);
        assert_eq!(
            labels(&menu)[..2],
            ["Apply last profile: focus".to_owned(), "-".to_owned()]
        );
        assert_eq!(
            menu.command(ID_LAST),
            Some(&Command::Switch("focus".into()))
        );
        // Not while a profile is active, nor for a profile that is gone.
        let active = Status {
            active: "focus".into(),
            ..status.clone()
        };
        assert_eq!(Menu::build(&profiles, &active).command(ID_LAST), None);
        assert_eq!(Menu::build(&[], &status).command(ID_LAST), None);
    }

    #[test]
    fn menu_shows_errors_and_a_removed_active_profile() {
        let mut status = Status::default();
        status.finished("old".into(), vec!["KS110 [start] tool.exe: missing".into()]);
        let menu = Menu::build(&["focus".to_owned()], &status);
        assert_eq!(
            labels(&menu),
            vec![
                "focus",
                "[x] old (no longer in the configuration) (disabled)",
                "&Neutral",
                "-",
                "Last switch: KS110 [start] tool.exe: missing (disabled)",
                "-",
                "Open kaltstellen… (disabled)",
                "&Quit"
            ]
        );
        let neutral = Menu::build(&[], &Status::default());
        assert_eq!(
            labels(&neutral)[0],
            "[x] &Neutral",
            "neutral is checked when active"
        );
    }
}
