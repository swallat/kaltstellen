//! Pure logic of the tray app: request queue (W3), status and icon state
//! (W2), and the menu built from the configuration. No Win32 here; the
//! window code in `win32::tray` only draws what this module decides.

use std::sync::{Arc, Mutex};

use super::protocol::{ErrorLine, LastSwitch};
use crate::config::NEUTRAL;
use crate::error::{Code, Error, Step};
use crate::process::Guest;

/// M5: the encoded size `last_switch` may take in the status reply, in
/// bytes, so the reply stays far below the pipe's 64 KiB line whatever a
/// profile holds; errors beyond it are left out (the menu still has them).
const MAX_LAST_SWITCH: usize = 16 * 1024;
/// Longest program name, text and profile name of one error, in bytes.
const MAX_ERROR_FIELD: usize = 512;
/// Longest path sent with an error, in bytes.
const MAX_ERROR_PATH: usize = 1024;

/// `text` cut to at most `max` bytes, on a character boundary.
fn cut(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

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
        /// U4: requested by a trigger, not by the user.
        automatic: Option<Automatic>,
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
            automatic: None,
        }
    }

    /// U4: a switch a trigger asks for.
    pub fn automatic(target: &str, previous: &str, reason: &str) -> Self {
        Job::Switch {
            target: target.trim().to_owned(),
            force: false,
            guests: Guests::default(),
            automatic: Some(Automatic {
                previous: previous.to_owned(),
                reason: reason.to_owned(),
            }),
        }
    }

    pub fn is_automatic(&self) -> bool {
        matches!(
            self,
            Job::Switch {
                automatic: Some(_),
                ..
            }
        )
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
            Job::Switch {
                target,
                automatic: Some(_),
                ..
            } => write!(f, "automatic switch to \"{target}\""),
            Job::Switch { target, .. } => write!(f, "switch to \"{target}\""),
            Job::Quit => f.write_str("quit"),
            Job::EndSession => f.write_str("end of session"),
        }
    }
}

/// U4: why a trigger asked for a switch, for the notification and its
/// `Back to <previous>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Automatic {
    /// The profile active before the switch.
    pub previous: String,
    /// For example `simulator.exe started`.
    pub reason: String,
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
    /// U4: an automatic request never replaces a waiting manual one; it is
    /// not queued.
    Ignored,
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
        // U4, conflict rule 1: manual beats automatic. A manual request
        // replaces a waiting automatic one like any other (below).
        if job.is_automatic() && self.pending.as_ref().is_some_and(|p| !p.is_automatic()) {
            return Decision::Ignored;
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
    /// U4: automation as the menu shows it; `None` while no profile has
    /// triggers.
    pub automation: Option<super::triggers::View>,
    /// U4: `Back to <previous>` of the last automatic switch, while its
    /// way back is valid.
    pub back_to: Option<String>,
}

/// L9: when an error of the last switch counts as resolved without a switch,
/// judged from the handle-free snapshot (A7: no process is opened), except
/// for a failed start that names its path (see [`Running`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Resolved once a process of this program runs (a failed start); with
    /// a path, only one whose image is that path.
    Runs(String, Option<String>),
    /// Resolved once no process of this program runs (a failed stop,
    /// close or freeze).
    Gone(String),
}

/// L9: `running(exe, path)` tells whether a process of the program `exe`
/// runs in the session; with `path`, only one whose image is that path
/// counts (read for processes of that name only, A7's fallback).
pub type Running<'a> = dyn Fn(&str, Option<&str>) -> bool + 'a;

/// One error of the last switch, as shown and as checked again (L9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchError {
    pub text: String,
    /// `None`: nothing observable resolves it; it stays until the next
    /// switch.
    pub resolution: Option<Resolution>,
    /// What the main window needs to put a marker on the entry (M5).
    pub code: Option<Code>,
    pub program: Option<String>,
    pub path: Option<String>,
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
    ///
    /// A failed start that names its path resolves only when a process of
    /// that name runs from that path, so a program of the same name from
    /// another folder (C2) does not resolve it.
    pub fn of(error: &Error) -> Self {
        let resolution = error
            .program
            .clone()
            .and_then(|exe| match (error.code, error.step) {
                (Code::StartFailed | Code::ElevationRequired, _) => {
                    Some(Resolution::Runs(exe, error.path.clone()))
                }
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
            code: Some(error.code),
            program: error.program.clone(),
            path: error.path.clone(),
        }
    }

    /// Whether `running` shows the cause is gone.
    pub fn resolved(&self, running: &Running) -> bool {
        match &self.resolution {
            Some(Resolution::Runs(exe, path)) => running(exe, path.as_deref()),
            Some(Resolution::Gone(exe)) => !running(exe, None),
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
            code: None,
            program: None,
            path: None,
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
    /// It could not be installed here last time; still offered on request.
    pub failed: bool,
    /// After a failed install: when the automatic install may try it again
    /// (seconds since 1970); `None` if it may now.
    pub retry_at: Option<u64>,
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
            automation: None,
            back_to: None,
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
    pub fn resolve(&mut self, running: &Running) -> Vec<SwitchError> {
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.switch_errors)
            .into_iter()
            .partition(|e| e.resolved(running));
        self.switch_errors = kept;
        gone
    }

    /// M5: the errors of the last switch that name a program, for the
    /// markers of the main window; `None` when there are none. They belong
    /// to the profile that switch made active.
    pub fn last_switch(&self) -> Option<LastSwitch> {
        let profile = cut(&self.active, MAX_ERROR_FIELD);
        // Measured as sent (JSON escapes double every backslash).
        let encoded = |value: &str| serde_json::to_string(value).map_or(usize::MAX, |s| s.len());
        let mut used = encoded(&profile);
        let mut errors = Vec::new();
        for e in &self.switch_errors {
            let (Some(code), Some(program)) = (e.code, &e.program) else {
                continue;
            };
            let line = ErrorLine {
                code: code.to_string(),
                program: cut(program, MAX_ERROR_FIELD),
                // A cut path would match no entry; one too long is left out
                // and the marker goes by name only.
                path: e.path.clone().filter(|p| p.len() <= MAX_ERROR_PATH),
                text: cut(&e.text, MAX_ERROR_FIELD),
            };
            let size = serde_json::to_string(&line).map_or(usize::MAX, |s| s.len() + 1);
            if used.saturating_add(size) > MAX_LAST_SWITCH {
                break;
            }
            used += size;
            errors.push(line);
        }
        (!errors.is_empty()).then_some(LastSwitch { profile, errors })
    }

    /// Whether a later check could still resolve an error (the re-check
    /// timer runs only then).
    pub fn resolvable(&self) -> bool {
        self.switch_errors.iter().any(|e| e.resolution.is_some())
    }

    pub fn tooltip(&self) -> String {
        let mut tip = format!("kaltstellen: {}", self.active);
        if self
            .automation
            .as_ref()
            .is_some_and(|a| a.applied.is_some())
        {
            tip.push_str(" (by trigger)");
        }
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
    /// U4: pauses or resumes all triggers.
    ToggleAutomatic,
    /// U4: `Back to <previous>` after an automatic switch, with the
    /// target the menu showed.
    BackTo(String),
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
/// U4: `Automatic switching` (checked while not paused).
pub const ID_AUTOMATIC: u16 = 12;
/// U4: `Back to <previous>` after an automatic switch.
pub const ID_BACK: u16 = 13;
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
        // U4, decided detail 1: the one-click undo of an automatic switch,
        // also when its notification was not shown (Do Not Disturb).
        if let Some(previous) = &status.back_to {
            menu.entry(
                ID_BACK,
                truncate(&format!("&Back to {}", escape(previous)), 100),
                false,
                Command::BackTo(previous.clone()),
            );
            menu.items.push(Item::Separator);
        }
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
        if let Some(view) = &status.automation {
            // U4: what automation does, then the switch that pauses it.
            if let Some((_, program, base)) = &view.applied {
                menu.disabled(truncate(
                    &format!(
                        "Applied by trigger {}; back to {} when it ends",
                        escape(program),
                        escape(base)
                    ),
                    100,
                ));
            }
            for (waiting, winner) in &view.waiting {
                menu.disabled(truncate(
                    &format!(
                        "Trigger for {} waits while {} is active",
                        escape(waiting),
                        escape(winner)
                    ),
                    100,
                ));
            }
            if view.held && !view.paused {
                menu.disabled(
                    "Triggers wait after a manual switch until a trigger program starts or ends"
                        .to_owned(),
                );
            }
            menu.entry(
                ID_AUTOMATIC,
                "Au&tomatic switching".to_owned(),
                !view.paused,
                Command::ToggleAutomatic,
            );
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
                ..
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

/// U4: the profile `Back to` switches to. `current` is the way back of the
/// last automatic switch now; `shown` the target the menu item showed when
/// it was chosen (`None` for the notification click, which always means
/// the current one). A menu chosen while a later automatic switch changed
/// the way back does nothing, so the switch never differs from its label.
pub fn back_target(current: Option<&str>, shown: Option<&str>) -> Option<String> {
    let current = current?;
    match shown {
        Some(shown) if !same_profile(shown, current) => None,
        _ => Some(current.to_owned()),
    }
}

/// U4: how many notifications of the tray are on screen, from the shell's
/// `NIN_BALLOONSHOW` (+1) and `NIN_BALLOONTIMEOUT`, `NIN_BALLOONHIDE` or a
/// click (-1). The order of these messages for stacked notifications is
/// not documented, so the count heals itself: when the last of them is
/// older than `stale` (longer than any notification stays), a new one
/// starts from 0.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Balloons {
    count: u32,
    last: Option<std::time::Instant>,
}

impl Balloons {
    fn heal(&mut self, now: std::time::Instant, stale: std::time::Duration) {
        if self
            .last
            .is_some_and(|last| now.saturating_duration_since(last) > stale)
        {
            self.count = 0;
        }
        self.last = Some(now);
    }

    /// `NIN_BALLOONSHOW`.
    pub fn shown(&mut self, now: std::time::Instant, stale: std::time::Duration) {
        self.heal(now, stale);
        self.count = self.count.saturating_add(1);
    }

    /// `NIN_BALLOONTIMEOUT` or `NIN_BALLOONHIDE`. True when the last one
    /// left just now.
    pub fn gone(&mut self, now: std::time::Instant) -> bool {
        self.last = Some(now);
        let was = self.count;
        self.count = was.saturating_sub(1);
        was == 1
    }

    /// A click: whether exactly one notification was on screen (the click
    /// can only mean that one). The clicked notification is dismissed.
    pub fn clicked(&mut self, now: std::time::Instant, stale: std::time::Duration) -> bool {
        self.heal(now, stale);
        let alone = self.count == 1;
        self.count = self.count.saturating_sub(1);
        alone
    }

    /// Explorer restarted: none of the old notifications is on screen.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn count(&self) -> u32 {
        self.count
    }
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
    fn u4_manual_beats_automatic_in_the_queue() {
        let auto = |t: &str| Job::automatic(t, "neutral", "sim.exe started");
        // An automatic request does not replace a waiting manual one...
        let mut q = Queue::default();
        q.request(sw("a"));
        q.request(sw("b"));
        assert_eq!(q.request(auto("c")), Decision::Ignored);
        assert_eq!(q.pending(), Some(&sw("b")));
        // ...nor a waiting recovery from the command line.
        let mut q = Queue::default();
        q.request(sw("a"));
        q.request(Job::Recover);
        assert_eq!(q.request(auto("c")), Decision::Ignored);
        // A manual request replaces a waiting automatic one.
        let mut q = Queue::default();
        q.request(sw("a"));
        assert_eq!(q.request(auto("c")), Decision::Queued { superseded: None });
        assert_eq!(
            q.request(sw("b")),
            Decision::Queued {
                superseded: Some(auto("c"))
            }
        );
        // Automatic replaces automatic; one leading where the running
        // switch leads is dropped (conflict rule 3, W3); one runs at once
        // when nothing runs.
        let mut q = Queue::default();
        assert_eq!(q.request(auto("a")), Decision::Start(auto("a")));
        assert_eq!(q.request(auto("b")), Decision::Queued { superseded: None });
        assert_eq!(
            q.request(auto("c")),
            Decision::Queued {
                superseded: Some(auto("b"))
            }
        );
        assert_eq!(
            q.request(auto("A")),
            Decision::Dropped {
                superseded: Some(auto("c"))
            }
        );
        assert_eq!(auto("a").to_string(), "automatic switch to \"a\"");
        assert!(!sw("a").is_automatic());
    }

    #[test]
    fn force_travels_with_the_request() {
        let mut q = Queue::default();
        q.request(sw("a"));
        let forced = Job::Switch {
            target: "b".into(),
            force: true,
            guests: Guests::default(),
            automatic: None,
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
                retry_at: None,
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
            retry_at: None,
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
    fn m5_last_switch_lists_program_errors_of_the_active_profile() {
        let mut status = Status::default();
        assert_eq!(status.last_switch(), None);
        let start = Error::new(Code::StartFailed, "missing")
            .program("tool.exe")
            .path("C:\\t\\tool.exe")
            .step(Step::Start);
        let io = Error::new(Code::StateIo, "disk full");
        status.finished(
            "focus".into(),
            vec![
                SwitchError::of(&start),
                SwitchError::of(&io),
                "plain".into(),
            ],
        );
        let last = status.last_switch().unwrap();
        assert_eq!(last.profile, "focus");
        // Only errors that name a program can become markers.
        assert_eq!(
            last.errors,
            vec![ErrorLine {
                code: "KS110".into(),
                program: "tool.exe".into(),
                path: Some("C:\\t\\tool.exe".into()),
                text: "KS110 [start] tool.exe: missing".into(),
            }]
        );
        // Resolved (L9) or replaced by a clean switch: nothing to mark.
        let freeze = Error::new(Code::SuspendFailed, "denied")
            .program("chat.exe")
            .step(Step::Suspend);
        status.finished("focus".into(), vec![SwitchError::of(&freeze)]);
        assert!(status.last_switch().is_some());
        status.resolve(&|_: &str, _: Option<&str>| false);
        assert_eq!(status.last_switch(), None);
        status.finished("focus".into(), vec![SwitchError::of(&start)]);
        assert!(status.last_switch().is_some());
        status.finished("focus".into(), vec![]);
        assert_eq!(status.last_switch(), None);
    }

    #[test]
    fn m5_a_failed_start_with_a_path_resolves_only_from_that_path() {
        // tool.exe in C:\A is missing; a tool.exe from C:\B runs: that one
        // must not resolve A's error. The tray answers `running` from the
        // image paths of the processes of that name.
        let start = Error::new(Code::StartFailed, "missing")
            .program("tool.exe")
            .path("C:\\A\\tool.exe")
            .step(Step::Start);
        let mut status = Status::default();
        status.finished("focus".into(), vec![SwitchError::of(&start)]);
        assert!(status.resolvable(), "the re-check timer runs");
        let asked = std::cell::RefCell::new(Vec::new());
        let only_b = |exe: &str, path: Option<&str>| {
            asked
                .borrow_mut()
                .push((exe.to_owned(), path.map(str::to_owned)));
            exe == "tool.exe"
                && path.is_some_and(|p| {
                    crate::process::same_path(
                        std::path::Path::new(p),
                        std::path::Path::new("C:\\B\\tool.exe"),
                    )
                })
        };
        assert!(status.resolve(&only_b).is_empty());
        assert_eq!(
            asked.borrow().as_slice(),
            [("tool.exe".to_owned(), Some("C:\\A\\tool.exe".to_owned()))]
        );
        let last = status.last_switch().unwrap();
        assert_eq!(last.errors[0].path.as_deref(), Some("C:\\A\\tool.exe"));
        assert_eq!(status.icon(), Icon::Error);
        // Started from C:\A (any case): resolved without a switch.
        let from_a = |exe: &str, path: Option<&str>| {
            exe == "tool.exe"
                && path.is_some_and(|p| {
                    crate::process::same_path(
                        std::path::Path::new(p),
                        std::path::Path::new("c:\\a\\TOOL.exe"),
                    )
                })
        };
        assert_eq!(status.resolve(&from_a).len(), 1);
        assert_eq!(status.last_switch(), None);
    }

    #[test]
    fn m5_last_switch_stays_below_its_byte_budget_and_the_pipe_line() {
        use super::super::protocol::{Body, MAX_LINE, TrayStatus, encode, reply};
        let long = "ü€".repeat(400);
        let path = format!("C:{}", "\\a".repeat(400));
        let errors: Vec<SwitchError> = (0..200)
            .map(|i| {
                SwitchError::of(
                    &Error::new(Code::SuspendFailed, format!("{i} {long}"))
                        .program(format!("{long}{i}.exe"))
                        .path(&path)
                        .step(Step::Suspend),
                )
            })
            .collect();
        let mut status = Status::default();
        status.finished(long.clone(), errors);
        let last = status.last_switch().unwrap();
        assert!(!last.errors.is_empty() && last.errors.len() < 200);
        let encoded = serde_json::to_string(&last).unwrap();
        assert!(encoded.len() <= MAX_LAST_SWITCH + 64, "{}", encoded.len());
        for line in &last.errors {
            // Cut by bytes, on character boundaries (a `String` cannot be
            // anything else; `cut` would panic otherwise).
            assert!(line.text.len() <= MAX_ERROR_FIELD && line.program.len() <= MAX_ERROR_FIELD);
            assert!(line.path.is_some(), "a path of 802 bytes is sent");
        }
        let line = encode(&reply(Body::Status(TrayStatus {
            state: crate::state::State::default(),
            running: Some(long.clone()),
            waiting: None,
            // U4 at its worst: long names (the window allows 4096
            // characters) and many waiting profiles, cut to the budget.
            automation: Some(
                super::super::protocol::AutomationStatus {
                    paused: false,
                    held: false,
                    trigger: Some("ü".repeat(130)),
                    returns_to: Some("ü€".repeat(800)),
                    waiting: vec!["ü€".repeat(800); 64],
                    waiting_for: Some("ü€".repeat(800)),
                }
                .bounded(),
            ),
            last_switch: Some(last),
        })));
        assert!(line.len() < MAX_LINE, "{}", line.len());
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
        let running = |exe: &str, _: Option<&str>| exe != "tool.exe";
        assert!(status.resolve(&running).is_empty());
        // tool.exe started by hand, chat.exe and svc.exe closed by hand.
        let running = |exe: &str, _: Option<&str>| exe == "tool.exe";
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
        assert!(status.resolve(&|_: &str, _: Option<&str>| true).is_empty());
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
    fn u4_automatic_switching_item_only_with_triggers() {
        let plain = Menu::build(&[], &Status::default());
        assert_eq!(plain.command(ID_AUTOMATIC), None);
        let mut status = Status {
            active: "flight".into(),
            automation: Some(crate::tray::triggers::View {
                paused: false,
                held: false,
                applied: Some(("flight".into(), "sim.exe".into(), "neutral".into())),
                waiting: vec![("music".into(), "flight".into())],
            }),
            ..Status::default()
        };
        let menu = Menu::build(&["flight".to_owned(), "music".to_owned()], &status);
        let all = labels(&menu);
        let at = all
            .iter()
            .position(|l| l == "[x] Au&tomatic switching")
            .expect("item");
        assert_eq!(
            all[at - 2..at],
            [
                "Applied by trigger sim.exe; back to neutral when it ends (disabled)",
                "Trigger for music waits while flight is active (disabled)"
            ]
        );
        assert_eq!(menu.command(ID_AUTOMATIC), Some(&Command::ToggleAutomatic));
        assert_eq!(status.tooltip(), "kaltstellen: flight (by trigger)");
        assert_eq!(menu.command(ID_BACK), None);
        status.back_to = Some("R&D".into());
        let menu = Menu::build(&[], &status);
        assert_eq!(
            labels(&menu)[..2],
            ["&Back to R&&D".to_owned(), "-".to_owned()]
        );
        // The command carries the target as shown (not as escaped).
        assert_eq!(menu.command(ID_BACK), Some(&Command::BackTo("R&D".into())));
        status.back_to = None;
        let view = status.automation.as_mut().unwrap();
        view.paused = true;
        view.held = true;
        view.applied = None;
        view.waiting.clear();
        let all = labels(&Menu::build(&[], &status));
        assert!(all.contains(&"Au&tomatic switching".to_owned()), "{all:?}");
        assert!(
            !all.iter().any(|l| l.starts_with("Triggers wait")),
            "{all:?}"
        );
        status.automation.as_mut().unwrap().paused = false;
        let all = labels(&Menu::build(&[], &status));
        assert!(
            all.iter().any(|l| l.starts_with("Triggers wait")),
            "{all:?}"
        );
    }

    #[test]
    fn u4_back_to_acts_only_on_the_target_shown() {
        assert_eq!(
            back_target(Some("neutral"), Some("Neutral")),
            Some("neutral".into())
        );
        // The way back changed while the menu was open: nothing.
        assert_eq!(back_target(Some("flight"), Some("neutral")), None);
        assert_eq!(back_target(None, Some("neutral")), None);
        // The notification click means the current one.
        assert_eq!(back_target(Some("flight"), None), Some("flight".into()));
    }

    #[test]
    fn u4_balloons_are_counted_and_heal_themselves() {
        let t0 = std::time::Instant::now();
        let s = std::time::Duration::from_secs(1);
        let stale = s * 60;
        let mut b = Balloons::default();
        b.shown(t0, stale);
        assert!(b.clicked(t0 + s, stale), "the only one");
        assert_eq!(b.count(), 0);
        // Two stacked: a click cannot tell which.
        b.shown(t0 + s * 2, stale);
        b.shown(t0 + s * 3, stale);
        assert!(!b.clicked(t0 + s * 4, stale));
        assert!(b.gone(t0 + s * 5), "the other one left");
        assert!(!b.gone(t0 + s * 6), "never below 0");
        assert_eq!(b.count(), 0);
        // A SHOW without its TIMEOUT: counted on, until it is stale.
        b.shown(t0 + s * 10, stale);
        assert_eq!(b.count(), 1);
        b.shown(t0 + s * 200, stale);
        assert_eq!(b.count(), 1, "the old one is forgotten");
        assert!(!b.clicked(t0 + s * 400, stale), "stale: refused");
        b.shown(t0 + s * 401, stale);
        b.reset();
        assert_eq!(b, Balloons::default());
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
