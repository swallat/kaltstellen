//! Pure logic of the tray app: request queue (W3), status and icon state
//! (W2), and the menu built from the configuration. No Win32 here; the
//! window code in `win32::tray` only draws what this module decides.

use std::sync::{Arc, Mutex};

use crate::config::NEUTRAL;
use crate::error::{Code, Error, Step};
use crate::process::Guest;

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
    /// `guests`: the command lines that asked for it, protected like
    /// kaltstellen itself (T16).
    Switch {
        target: String,
        force: bool,
        guests: Guests,
    },
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
            guests: Guests::default(),
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

/// The callers a switch protects (T16), shared between the window thread,
/// which adds a caller that joins a running switch, and the worker, which
/// reads the list for every snapshot. Not part of a job's identity: two
/// jobs are equal whatever their guests are.
#[derive(Debug, Clone, Default)]
pub struct Guests(Arc<Mutex<Vec<Guest>>>);

impl Guests {
    pub fn of(guest: Option<Guest>) -> Self {
        let guests = Self::default();
        if let Some(guest) = guest {
            guests.add(guest);
        }
        guests
    }

    pub fn add(&self, guest: Guest) {
        let mut list = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !list.contains(&guest) {
            list.push(guest);
        }
    }

    pub fn list(&self) -> Vec<Guest> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl PartialEq for Guests {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Guests {}

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
    /// Quit or the end of the session runs or waits: nothing new is taken.
    Refused,
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
        // Quit and the end of the session resume everything; no later
        // request may replace them while they wait (as while quitting).
        if matches!(self.pending, Some(Job::Quit | Job::EndSession)) {
            return Decision::Refused;
        }
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
    /// They stay until the next switch or until their cause is gone (L9).
    pub switch_errors: Vec<SwitchError>,
    /// The configuration file is invalid (C3); the last valid one is used.
    pub config_error: Option<String>,
    /// The pipe for the command line could not be set up (L1).
    pub pipe_error: Option<String>,
    /// S5: the profile of the last session, offered for one click.
    pub offer: Option<String>,
    /// Start at sign-in: `None` if the Run key cannot be read.
    pub autostart: Option<bool>,
    /// Hotkeys another program holds (C5), as `hotkey (profile)`.
    pub hotkeys_unavailable: Vec<String>,
    /// N1: unassigned programs of the active profile at the last snapshot.
    pub unassigned: usize,
    /// UP1: the update mode.
    pub update_mode: crate::update::Mode,
    /// UP1: `KALTSTELLEN_UPDATE` decides; the menu cannot change it.
    pub update_locked: bool,
    /// UP1: a newer version found by the last check.
    pub update: Option<UpdateOffer>,
}

/// L9: when an error of the last switch counts as resolved without a switch,
/// judged from the handle-free snapshot (A7: no process is opened).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Resolved once a process of this program runs (a failed start).
    Runs(String),
    /// Resolved once no process of this program runs (a failed stop,
    /// close or freeze).
    Gone(String),
}

/// One error of the last switch, as shown and as checked again (L9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchError {
    pub text: String,
    /// `None`: nothing observable resolves it; it stays until the next
    /// switch.
    pub resolution: Option<Resolution>,
}

impl SwitchError {
    /// L9 rules per error code (`[assumed]` in the spec):
    /// - `KS110` start failed, `KS111` start needs elevation: resolved when
    ///   the program runs.
    /// - `KS108` close timed out (`on_timeout = "keep"`), `KS109` terminate
    ///   failed, `KS105` suspend failed: resolved when the program no
    ///   longer runs.
    /// - `KS102` needs administrator, `KS103` protected, in the stop or
    ///   suspend step: resolved when the program no longer runs.
    /// - Everything else (state or log I/O, warn list, resume, snapshot):
    ///   stays until the next switch.
    pub fn of(error: &Error) -> Self {
        let program = error.program.clone();
        let resolution = program.and_then(|exe| match (error.code, error.step) {
            (Code::StartFailed | Code::ElevationRequired, _) => Some(Resolution::Runs(exe)),
            (Code::CloseTimeout | Code::TerminateFailed | Code::SuspendFailed, _) => {
                Some(Resolution::Gone(exe))
            }
            (Code::NeedsAdmin | Code::Protected, Some(Step::Stop | Step::Suspend)) => {
                Some(Resolution::Gone(exe))
            }
            _ => None,
        });
        Self {
            text: error.to_string(),
            resolution,
        }
    }

    /// Whether `running(exe)` shows the cause is gone.
    pub fn resolved(&self, running: &dyn Fn(&str) -> bool) -> bool {
        match &self.resolution {
            Some(Resolution::Runs(exe)) => running(exe),
            Some(Resolution::Gone(exe)) => !running(exe),
            None => false,
        }
    }
}

impl From<&str> for SwitchError {
    /// An error without an observable resolution.
    fn from(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            resolution: None,
        }
    }
}

/// UP1: a newer version the menu offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateOffer {
    pub version: String,
    /// Why this copy cannot install it (package manager, folder,
    /// development build); `None` if it can.
    pub blocked: Option<String>,
    /// It could not be installed here before: offered, never installed by itself.
    pub failed: bool,
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
            autostart: None,
            hotkeys_unavailable: Vec::new(),
            unassigned: 0,
            update_mode: crate::update::Mode::Auto,
            update_locked: false,
            update: None,
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
    pub fn finished(&mut self, active: String, errors: Vec<SwitchError>) {
        self.active = active;
        self.switching = None;
        self.switch_errors = errors;
    }

    /// L9: drops the errors whose cause is gone and returns them. An
    /// invalid configuration or a missing pipe is not touched here.
    pub fn resolve(&mut self, running: &dyn Fn(&str) -> bool) -> Vec<SwitchError> {
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.switch_errors)
            .into_iter()
            .partition(|e| e.resolved(running));
        self.switch_errors = kept;
        gone
    }

    /// Whether a later check could still resolve an error (the re-check
    /// timer runs only then).
    pub fn resolvable(&self) -> bool {
        self.switch_errors.iter().any(|e| e.resolution.is_some())
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
        } else if let Some(first) = self.switch_errors.first().map(|e| &e.text) {
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
    ToggleAutostart,
    /// Starts the main window (`kaltstellen gui`).
    OpenWindow,
    /// UP1: switches the daily update check on or off.
    /// UP1: sets the update mode.
    UpdateMode(crate::update::Mode),
    /// UP1: opens a console running `kaltstellen update`.
    InstallUpdate,
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
/// Switches start at sign-in on or off.
pub const ID_AUTOSTART: u16 = 4;
pub const ID_OPEN: u16 = 6;
/// `Unassigned programs (N)`: opens the main window.
pub const ID_UNASSIGNED: u16 = 7;
/// UP1: install updates automatically.
pub const ID_UPDATE_AUTO: u16 = 8;
/// UP1: check and report only.
pub const ID_UPDATE_MANUAL: u16 = 10;
/// UP1: no checks.
pub const ID_UPDATE_OFF: u16 = 11;
/// UP1: `Install update <version>…`.
pub const ID_UPDATE_INSTALL: u16 = 9;
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
        if status.unassigned > 0 && !status.is_neutral() {
            menu.entry(
                ID_UNASSIGNED,
                format!("&Unassigned programs ({})", status.unassigned),
                false,
                Command::OpenWindow,
            );
        }
        for hotkey in &status.hotkeys_unavailable {
            menu.disabled(format!("Hotkey not available: {}", escape(hotkey)));
        }
        if let Some(on) = status.autostart {
            menu.entry(
                ID_AUTOSTART,
                "&Start at sign-in".to_owned(),
                on,
                Command::ToggleAutostart,
            );
        }
        match &status.update {
            Some(UpdateOffer {
                version,
                blocked: None,
                failed,
            }) => menu.entry(
                ID_UPDATE_INSTALL,
                if *failed {
                    format!("Install &update {}… (failed last time)", escape(version))
                } else {
                    format!("Install &update {}…", escape(version))
                },
                false,
                Command::InstallUpdate,
            ),
            Some(UpdateOffer {
                version,
                blocked: Some(how),
                ..
            }) => menu.disabled(truncate(
                &format!("Update {} available: {}", escape(version), escape(how)),
                100,
            )),
            None => {}
        }
        for (id, mode, label) in [
            (
                ID_UPDATE_AUTO,
                crate::update::Mode::Auto,
                "Updates: install &automatically",
            ),
            (
                ID_UPDATE_MANUAL,
                crate::update::Mode::Manual,
                "Updates: check and as&k",
            ),
            (ID_UPDATE_OFF, crate::update::Mode::Off, "Updates: o&ff"),
        ] {
            let checked = status.update_mode == mode;
            if status.update_locked {
                menu.items.push(Item::Entry {
                    id: 0,
                    label: format!("{label} (set by KALTSTELLEN_UPDATE)"),
                    checked,
                    enabled: false,
                });
            } else {
                menu.entry(id, label.to_owned(), checked, Command::UpdateMode(mode));
            }
        }
        menu.entry(
            ID_OPEN,
            "&Open kaltstellen…".to_owned(),
            false,
            Command::OpenWindow,
        );
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
    fn s5_a_waiting_end_of_session_or_quit_is_never_superseded() {
        for last in [Job::EndSession, Job::Quit] {
            let mut q = Queue::default();
            q.request(sw("slow"));
            assert_eq!(q.jump(last.clone()), Err(None));
            assert_eq!(q.request(sw("other")), Decision::Refused);
            assert_eq!(q.pending(), Some(&last));
            assert_eq!(q.finish(), Some(last));
        }
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
            guests: Guests::default(),
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
                "[x] Updates: install &automatically",
                "Updates: check and as&k",
                "Updates: o&ff",
                "&Open kaltstellen…",
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
    fn up1_menu_offers_an_update_and_the_three_modes() {
        let mut status = Status {
            update_mode: crate::update::Mode::Manual,
            update: Some(UpdateOffer {
                version: "1.2.0".into(),
                blocked: None,
                failed: false,
            }),
            ..Status::default()
        };
        let menu = Menu::build(&[], &status);
        let tail = &labels(&menu)[2..];
        assert_eq!(
            tail,
            [
                "Install &update 1.2.0…",
                "Updates: install &automatically",
                "[x] Updates: check and as&k",
                "Updates: o&ff",
                "&Open kaltstellen…",
                "&Quit"
            ]
        );
        assert_eq!(
            menu.command(ID_UPDATE_INSTALL),
            Some(&Command::InstallUpdate)
        );
        assert_eq!(
            menu.command(ID_UPDATE_AUTO),
            Some(&Command::UpdateMode(crate::update::Mode::Auto))
        );
        assert_eq!(
            menu.command(ID_UPDATE_OFF),
            Some(&Command::UpdateMode(crate::update::Mode::Off))
        );
        // It failed last time: still offered on request, and says so.
        status.update.as_mut().unwrap().failed = true;
        let menu = Menu::build(&[], &status);
        assert_eq!(
            labels(&menu)[2],
            "Install &update 1.2.0… (failed last time)"
        );
        assert_eq!(
            menu.command(ID_UPDATE_INSTALL),
            Some(&Command::InstallUpdate)
        );
        // Not installable here: a disabled line that says how instead; the
        // environment fixes the mode.
        status.update = Some(UpdateOffer {
            version: "1.2.0".into(),
            blocked: Some("installed by scoop: scoop update kaltstellen".into()),
            failed: false,
        });
        status.update_mode = crate::update::Mode::Off;
        status.update_locked = true;
        let menu = Menu::build(&[], &status);
        assert_eq!(
            &labels(&menu)[2..6],
            [
                "Update 1.2.0 available: installed by scoop: scoop update kaltstellen (disabled)",
                "Updates: install &automatically (set by KALTSTELLEN_UPDATE) (disabled)",
                "Updates: check and as&k (set by KALTSTELLEN_UPDATE) (disabled)",
                "[x] Updates: o&ff (set by KALTSTELLEN_UPDATE) (disabled)"
            ]
        );
        assert_eq!(menu.command(ID_UPDATE_INSTALL), None);
        assert_eq!(menu.command(ID_UPDATE_AUTO), None);
    }

    #[test]
    fn l9_errors_resolve_when_their_cause_is_gone() {
        let start = Error::new(Code::StartFailed, "missing")
            .program("tool.exe")
            .step(Step::Start);
        let keep = Error::new(Code::CloseTimeout, "left running")
            .program("chat.exe")
            .step(Step::Stop);
        let admin = Error::new(Code::NeedsAdmin, "denied")
            .program("svc.exe")
            .step(Step::Suspend);
        let io = Error::new(Code::StateIo, "disk full");
        let mut status = Status::default();
        status.finished(
            "focus".into(),
            [&start, &keep, &admin, &io]
                .iter()
                .map(|e| SwitchError::of(e))
                .collect(),
        );
        assert_eq!(status.icon(), Icon::Error);
        assert!(status.resolvable());
        // Nothing changed yet: tool.exe does not run, the others still do.
        let running = |exe: &str| exe != "tool.exe";
        assert!(status.resolve(&running).is_empty());
        // tool.exe started by hand, chat.exe and svc.exe closed by hand.
        let running = |exe: &str| exe == "tool.exe";
        let gone = status.resolve(&running);
        assert_eq!(gone.len(), 3);
        assert_eq!(status.switch_errors.len(), 1, "the state error stays");
        assert!(!status.resolvable(), "no timer needed for it");
        assert_eq!(status.icon(), Icon::Error);
        // The next switch replaces the list.
        status.finished("focus".into(), vec![]);
        assert_eq!(status.icon(), Icon::Active);
        // A broken configuration is not resolved this way.
        status.config_error = Some("KS202".into());
        assert!(status.resolve(&|_: &str| true).is_empty());
        assert_eq!(status.icon(), Icon::Error);
        assert!(
            Menu::build(&[], &status)
                .items
                .iter()
                .all(|i| !matches!(i, Item::Entry { label, .. } if label.contains("Clear")))
        );
    }

    #[test]
    fn n1_unassigned_line_only_with_programs_and_a_profile() {
        let mut status = Status {
            active: "focus".into(),
            unassigned: 2,
            ..Status::default()
        };
        let menu = Menu::build(&["focus".to_owned()], &status);
        assert!(labels(&menu).contains(&"&Unassigned programs (2)".to_owned()));
        assert_eq!(menu.command(ID_UNASSIGNED), Some(&Command::OpenWindow));
        status.unassigned = 0;
        assert_eq!(Menu::build(&[], &status).command(ID_UNASSIGNED), None);
    }

    #[test]
    fn start_at_sign_in_is_a_checked_item_when_known() {
        let on = Status {
            autostart: Some(true),
            ..Status::default()
        };
        let menu = Menu::build(&[], &on);
        assert!(labels(&menu).contains(&"[x] &Start at sign-in".to_owned()));
        assert_eq!(menu.command(ID_AUTOSTART), Some(&Command::ToggleAutostart));
        let unknown = Menu::build(&[], &Status::default());
        assert_eq!(unknown.command(ID_AUTOSTART), None);
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
                "[x] Updates: install &automatically",
                "Updates: check and as&k",
                "Updates: o&ff",
                "&Open kaltstellen…",
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
