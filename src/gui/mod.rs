//! The main window (`kaltstellen gui`, WP10; spec section 8). A process of
//! its own, started from the tray, ended when closed, so Q1 holds for the
//! tray whatever the window costs. It edits the configuration file through
//! [`document::Document`] and applies profiles by running
//! `kaltstellen switch`, which goes through the tray's pipe when a tray runs.

pub mod document;
pub mod model;
pub mod unassigned;

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use log::{info, warn};

use crate::config::{Action, NEUTRAL};
use crate::exit;
use crate::paths;
use crate::process::Snapshot;
use crate::state::Store;
use crate::win32::gui::{self as window, Event, Form, Ui};
use crate::win32::{console, instance::Instance};
use document::Document;
use model::{AddCheck, Candidate, ids};

/// Keeps a started `kaltstellen switch` from opening a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `kaltstellen gui`. Returns the exit code.
pub fn main() -> u8 {
    let Some(file) = paths::config_file() else {
        eprintln!("error: no configuration directory (APPDATA unset)");
        return exit::ERRORS;
    };
    let mutex = crate::tray::Endpoints::current()
        .map(|e| e.mutex.replace("kaltstellen-tray", "kaltstellen-gui"))
        .unwrap_or_else(|_| "Local\\kaltstellen-gui".to_owned());
    let _instance = match Instance::acquire(&mutex) {
        Ok(Some(instance)) => instance,
        Ok(None) => {
            eprintln!("the kaltstellen window is already open in this session");
            return exit::OK;
        }
        Err(err) => {
            eprintln!("error: {err}");
            return exit::ERRORS;
        }
    };
    let document = match Document::open(&file) {
        Ok(document) => document,
        Err(lines) => {
            // The window edits valid files only; check-config tells why.
            for line in &lines {
                eprintln!("{line}");
            }
            warn!("gui: configuration invalid; not opened");
            return exit::CONFIG_INVALID;
        }
    };
    console::detach_if_own();
    info!("gui: started (pid {})", std::process::id());
    let result = window::run("kaltstellen", model::SIZE, model::controls(), |ui| {
        let (results, inbox) = channel();
        let mut app = Main {
            document,
            selected: None,
            candidates: Vec::new(),
            results,
            inbox,
            pending_delete: None,
            busy: false,
            unassigned: Vec::new(),
        };
        if !app.document.config().profiles.is_empty() {
            app.selected = Some(0);
        }
        ui.set_choices(ids::ACTION, &model::ACTION_NAMES, None);
        app.show_profiles(ui);
        app.refresh_running(ui);
        app.refresh_unassigned(ui);
        ui.set_text(
            ids::STATUS,
            &format!("Editing {}", app.document.path().display()),
        );
        Box::new(app)
    });
    match result {
        Ok(()) => {
            info!("gui: ended");
            exit::OK
        }
        Err(err) => {
            warn!("{err}");
            eprintln!("error: {err}");
            exit::ERRORS
        }
    }
}

/// Result of a `kaltstellen switch` run in the background.
struct Applied {
    target: String,
    code: u8,
    stderr: String,
}

struct Main {
    document: Document,
    selected: Option<usize>,
    candidates: Vec<Candidate>,
    results: Sender<Applied>,
    inbox: Receiver<Applied>,
    /// M1: a profile to delete once the switch to `neutral` is done.
    pending_delete: Option<String>,
    busy: bool,
    unassigned: Vec<unassigned::Unassigned>,
}

fn active_profile() -> String {
    paths::data_dir()
        .and_then(|dir| Store::new(dir).peek())
        .map_or_else(|| NEUTRAL.to_owned(), |s| s.active_profile)
}

impl Main {
    fn profile_name(&self) -> Option<String> {
        let profiles = &self.document.config().profiles;
        self.selected
            .and_then(|n| profiles.get(n))
            .map(|p| p.name.clone())
    }

    fn show_profiles(&mut self, ui: &Ui) {
        let active = active_profile();
        let profiles = &self.document.config().profiles;
        if self.selected.is_some_and(|n| n >= profiles.len()) {
            self.selected = profiles.len().checked_sub(1);
        }
        let names: Vec<String> = profiles
            .iter()
            .map(|p| {
                if p.name.eq_ignore_ascii_case(&active) {
                    format!("{} (active)", p.name)
                } else {
                    p.name.clone()
                }
            })
            .collect();
        ui.set_list(ids::PROFILES, &names, self.selected);
        self.show_entries(ui);
    }

    fn show_entries(&self, ui: &Ui) {
        let rows = self
            .selected
            .and_then(|n| self.document.config().profiles.get(n))
            .map(model::rows)
            .unwrap_or_default();
        ui.set_rows(ids::ENTRIES, &rows);
        if let Some(name) = self.profile_name() {
            ui.set_text(ids::NAME, &name);
        }
    }

    fn refresh_running(&mut self, ui: &Ui) {
        match Snapshot::take() {
            Ok(snapshot) => {
                let session = snapshot.own_session().unwrap_or(0);
                self.candidates = model::candidates(snapshot.processes(), session);
                let rows: Vec<Vec<String>> = self.candidates.iter().map(Candidate::row).collect();
                ui.set_rows(ids::RUNNING, &rows);
            }
            Err(err) => ui.set_text(ids::STATUS, &err.to_string()),
        }
    }

    fn refresh_unassigned(&mut self, ui: &Ui) {
        self.unassigned = match self.profile_name() {
            Some(profile) => unassigned::current(self.document.config(), &profile),
            None => Vec::new(),
        };
        let rows: Vec<Vec<String>> = self
            .unassigned
            .iter()
            .map(unassigned::Unassigned::row)
            .collect();
        ui.set_rows(ids::UNASSIGNED, &rows);
    }

    /// Saves after an edit and reports the outcome in the status line.
    fn saved(&mut self, ui: &Ui, what: &str, result: crate::error::Result<()>) {
        let result = result.and_then(|()| self.document.save());
        match result {
            Ok(()) => {
                info!("gui: {what}");
                ui.set_text(ids::STATUS, &format!("{what}; saved."));
            }
            Err(err) => {
                warn!("gui: {err}");
                if err.reason.contains("changed by another program") {
                    // The file on disk wins: re-read it, the edit is lost.
                    if let Ok(fresh) = Document::open(self.document.path()) {
                        self.document = fresh;
                    }
                    ui.set_text(
                        ids::STATUS,
                        &format!("{err}. Reloaded; please repeat the change."),
                    );
                } else {
                    ui.set_text(ids::STATUS, &err.to_string());
                }
            }
        }
        self.show_profiles(ui);
        self.refresh_unassigned(ui);
    }

    /// Adds `exe` to the selected profile after the checks of M4.
    fn add(&mut self, ui: &Ui, exe: &str, path: Option<PathBuf>) {
        let Some(profile) = self.profile_name() else {
            ui.set_text(ids::STATUS, "Choose a profile first.");
            return;
        };
        let confirmed = match model::check_add(exe) {
            AddCheck::Refused(reason) => {
                ui.set_text(ids::STATUS, &reason);
                return;
            }
            AddCheck::Confirm(question) => {
                if !ui.ask("kaltstellen", &question) {
                    ui.set_text(ids::STATUS, &format!("{exe} not added."));
                    return;
                }
                true
            }
            AddCheck::Allowed => false,
        };
        let action = ui.choice(ids::ACTION).map(|n| model::ACTIONS[n]);
        let result = self
            .document
            .add_entry(&profile, exe, path.as_deref(), action, confirmed);
        self.saved(ui, &format!("{exe} added to \"{profile}\""), result);
    }

    /// Runs `kaltstellen switch TARGET` without a console window.
    fn apply(&mut self, ui: &Ui, target: &str) {
        if self.busy {
            ui.set_text(ids::STATUS, "A switch is still running.");
            return;
        }
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(err) => {
                ui.set_text(ids::STATUS, &format!("Cannot apply: {err}"));
                return;
            }
        };
        self.busy = true;
        ui.set_text(ids::STATUS, &format!("Applying \"{target}\"…"));
        let results = self.results.clone();
        let waker = ui.waker();
        let target = target.to_owned();
        thread::spawn(move || {
            use std::os::windows::process::CommandExt;
            let output = Command::new(exe)
                .args(["switch", &target])
                .stdin(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .output();
            let (code, stderr) = match output {
                Ok(o) => (
                    o.status.code().unwrap_or(1) as u8,
                    String::from_utf8_lossy(&o.stderr).into_owned(),
                ),
                Err(err) => (exit::UNREACHABLE, format!("error: {err}")),
            };
            let _ = results.send(Applied {
                target,
                code,
                stderr,
            });
            waker.wake();
        });
    }

    fn applied(&mut self, ui: &Ui, applied: Applied) {
        self.busy = false;
        let line = model::applied(&applied.target, applied.code, &applied.stderr);
        info!("gui: {line}");
        ui.set_text(ids::STATUS, &line);
        if let Some(name) = self.pending_delete.take() {
            if applied.target == NEUTRAL && applied.code <= exit::ERRORS {
                let result = self.document.delete_profile(&name);
                self.saved(ui, &format!("Profile \"{name}\" deleted"), result);
            } else {
                ui.set_text(
                    ids::STATUS,
                    &format!("{line} Profile \"{name}\" not deleted."),
                );
            }
        }
        self.show_profiles(ui);
        self.refresh_unassigned(ui);
    }
}

impl Form for Main {
    fn event(&mut self, ui: &Ui, event: Event) {
        let name = ui.text(ids::NAME).trim().to_owned();
        let profile = self.profile_name();
        let entry = ui.row_selection(ids::ENTRIES);
        match event {
            Event::Selected(ids::PROFILES) => {
                self.selected = ui.list_selection(ids::PROFILES);
                self.show_entries(ui);
                self.refresh_unassigned(ui);
            }
            Event::Clicked(ids::NEW) => {
                let result = self.document.add_profile(&name);
                if result.is_ok() {
                    self.selected = Some(self.document.config().profiles.len() - 1);
                }
                self.saved(ui, &format!("Profile \"{name}\" created"), result);
            }
            Event::Clicked(ids::COPY) => {
                let Some(from) = profile else { return };
                let to = if name.is_empty() || name.eq_ignore_ascii_case(&from) {
                    format!("{from} copy")
                } else {
                    name
                };
                let result = self.document.copy_profile(&from, &to);
                if result.is_ok() {
                    self.selected = Some(self.document.config().profiles.len() - 1);
                }
                self.saved(
                    ui,
                    &format!("Profile \"{from}\" copied to \"{to}\""),
                    result,
                );
            }
            Event::Clicked(ids::RENAME) => {
                let Some(from) = profile else { return };
                let result = self.document.rename_profile(&from, &name);
                self.saved(
                    ui,
                    &format!("Profile \"{from}\" renamed to \"{name}\""),
                    result,
                );
            }
            Event::Clicked(ids::DELETE) => {
                let Some(name) = profile else { return };
                if !ui.ask("kaltstellen", &format!("Delete the profile \"{name}\"?")) {
                    return;
                }
                if active_profile().eq_ignore_ascii_case(&name) {
                    // M1: deleting the active profile first switches to neutral.
                    self.pending_delete = Some(name);
                    self.apply(ui, NEUTRAL);
                } else {
                    let result = self.document.delete_profile(&name);
                    self.saved(ui, &format!("Profile \"{name}\" deleted"), result);
                }
            }
            Event::Clicked(ids::APPLY) => {
                if let Some(name) = profile {
                    self.apply(ui, &name);
                }
            }
            Event::Clicked(ids::SET_ACTION) => {
                let (Some(profile), Some(entry), Some(choice)) =
                    (profile, entry, ui.choice(ids::ACTION))
                else {
                    ui.set_text(ids::STATUS, "Choose an entry and an action first.");
                    return;
                };
                let action = model::ACTIONS[choice];
                let exe = self.document.config().profiles[self.selected.unwrap_or(0)].apps[entry]
                    .exe
                    .clone();
                let touches = matches!(action, Action::Stop | Action::Suspend);
                let mut result = self.document.set_action(&profile, entry, action);
                if result.is_ok()
                    && touches
                    && let AddCheck::Confirm(question) = model::check_add(&exe)
                    && ui.ask("kaltstellen", &question)
                {
                    result = self.document.confirm(&profile, entry);
                }
                self.saved(
                    ui,
                    &format!("{exe}: action {}", model::ACTION_NAMES[choice]),
                    result,
                );
            }
            Event::Clicked(ids::REMOVE) => {
                let (Some(profile), Some(entry)) = (profile, entry) else {
                    return;
                };
                let result = self.document.remove_entry(&profile, entry);
                self.saved(ui, "Entry removed", result);
            }
            Event::Clicked(ids::ADD_FILE) => {
                if let Some(path) = ui.pick_executable()
                    && let Some(exe) = path.file_name().map(|n| n.to_string_lossy().into_owned())
                {
                    self.add(ui, &exe, Some(path));
                }
            }
            Event::Clicked(ids::ADD_RUNNING) => {
                if let Some(candidate) = ui
                    .row_selection(ids::RUNNING)
                    .and_then(|n| self.candidates.get(n))
                    .cloned()
                {
                    self.add(ui, &candidate.exe, None);
                }
            }
            Event::Clicked(ids::REFRESH) => {
                self.refresh_running(ui);
                self.refresh_unassigned(ui);
            }
            Event::Clicked(ids::ADD_UNASSIGNED) => {
                if let Some(item) = ui
                    .row_selection(ids::UNASSIGNED)
                    .and_then(|n| self.unassigned.get(n))
                    .cloned()
                {
                    self.add(ui, &item.exe, None);
                }
            }
            Event::Clicked(ids::DISMISS) => {
                if let Some(item) = ui
                    .row_selection(ids::UNASSIGNED)
                    .and_then(|n| self.unassigned.get(n))
                    .cloned()
                {
                    if let Some(profile) = self.profile_name() {
                        unassigned::dismiss(&profile, &item);
                    }
                    self.refresh_unassigned(ui);
                }
            }
            Event::Woken => {
                while let Ok(applied) = self.inbox.try_recv() {
                    self.applied(ui, applied);
                }
            }
            Event::Closing => ui.close(),
            _ => {}
        }
    }
}
