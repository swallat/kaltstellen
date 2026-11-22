//! The tray app (`kaltstellen tray`, WP7): one instance per session (L1),
//! notification icon with three states and a menu built from the
//! configuration, switches on a worker thread (W3), recovery at start (S3)
//! and resume on Quit (L2).
//!
//! Everything here is safe code. The Win32 side lives in
//! [`crate::win32`]; this module decides what it shows.

pub mod model;
pub mod source;
pub mod worker;

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard};

use log::{info, warn};

use crate::config::NEUTRAL;
use crate::paths;
use crate::state::Store;
use crate::win32::{console, instance::Instance, tray as window};
use model::{Command, Decision, Icon, Menu, Status};
use source::Source;
use worker::{Controller, Done, EngineRunner, Job};

/// E2 exit codes used by the tray.
const EXIT_OK: u8 = 0;
const EXIT_ERRORS: u8 = 1;

/// Mutex of the tray instance in the session namespace (L1). With a
/// redirected data directory (tests, portable use) the name includes that
/// directory, so such an instance does not block the real one.
pub fn instance_name(data_dir: &Path, redirected: bool) -> String {
    const BASE: &str = "Local\\kaltstellen-tray";
    if !redirected {
        return BASE.to_owned();
    }
    // FNV-1a: stable across builds, unlike the standard hasher.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data_dir.to_string_lossy().to_lowercase().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{BASE}-{hash:016x}")
}

fn lock(source: &Mutex<Source>) -> MutexGuard<'_, Source> {
    source.lock().unwrap_or_else(|e| e.into_inner())
}

/// `kaltstellen tray`. Returns the process exit code.
pub fn main() -> u8 {
    let Some(data) = paths::data_dir() else {
        eprintln!("error: no data directory (LOCALAPPDATA unset)");
        return EXIT_ERRORS;
    };
    let redirected = std::env::var_os("KALTSTELLEN_DATA_DIR").is_some_and(|v| !v.is_empty());
    let _instance = match Instance::acquire(&instance_name(&data, redirected)) {
        Ok(Some(instance)) => instance,
        Ok(None) => {
            info!("tray: another instance runs in this session; exiting");
            eprintln!("kaltstellen tray is already running in this session");
            return EXIT_OK;
        }
        Err(err) => {
            warn!("{err}");
            eprintln!("error: {err}");
            return EXIT_ERRORS;
        }
    };
    if console::detach_if_own() {
        info!("tray: started without a terminal; console released");
    } else {
        eprintln!("kaltstellen tray is running; use Quit in its menu to end it");
    }
    info!("tray: started (pid {})", std::process::id());

    let store = Store::new(&data);
    let source = Arc::new(Mutex::new(Source::new(paths::config_file())));
    let mut status = Status {
        // What a crashed instance left; recovery below makes it neutral.
        active: store
            .peek()
            .map_or_else(|| NEUTRAL.to_owned(), |s| s.active_profile),
        switching: Some(NEUTRAL.to_owned()),
        ..Status::default()
    };
    let names = {
        let mut source = lock(&source);
        source.refresh();
        status.config_error = source.problem().map(str::to_owned);
        source.profile_names()
    };
    let exit_code = Rc::new(Cell::new(EXIT_OK));

    let initial_icon = status.icon();
    info!("tray: icon {}", icon_name(initial_icon));
    let result = window::run(initial_icon, &status.tooltip(), |waker| {
        let (done_tx, done_rx) = mpsc::channel();
        let mut controller = Controller::spawn(
            EngineRunner::new(store.clone(), Arc::clone(&source)),
            move |done| {
                if done_tx.send(done).is_ok() {
                    waker.wake();
                }
            },
        );
        // S3: nothing stays frozen after a crash; then `neutral`.
        controller.recover();
        let menu = Menu::build(&names, &status);
        Box::new(App {
            controller,
            done: done_rx,
            status,
            source: Arc::clone(&source),
            store: store.clone(),
            menu,
            exit_code: Rc::clone(&exit_code),
            shown: Some(initial_icon),
        })
    });
    match result {
        Ok(()) => {
            info!("tray: ended");
            exit_code.get()
        }
        Err(err) => {
            warn!("{err}");
            eprintln!("error: {err}");
            EXIT_ERRORS
        }
    }
}

struct App {
    controller: Controller,
    done: Receiver<Done>,
    status: Status,
    source: Arc<Mutex<Source>>,
    store: Store,
    /// The menu last shown; `WM_COMMAND` IDs refer to it.
    menu: Menu,
    exit_code: Rc<Cell<u8>>,
    /// The icon state last drawn.
    shown: Option<Icon>,
}

fn icon_name(icon: Icon) -> &'static str {
    match icon {
        Icon::Neutral => "neutral",
        Icon::Active => "active",
        Icon::Error => "error",
    }
}

impl App {
    fn redraw(&mut self) {
        let icon = self.status.icon();
        if self.shown != Some(icon) {
            // Logged so the state is visible without looking at the screen.
            info!("tray: icon {}", icon_name(icon));
            self.shown = Some(icon);
        }
        window::show(icon, &self.status.tooltip());
    }

    fn quit(&mut self) {
        info!("tray: quit requested");
        if !self.controller.quit() {
            info!("tray: quitting after the running switch");
        }
        self.status.switching = None;
        self.redraw();
    }

    fn finished(&mut self, done: Done) {
        let errors: Vec<String> = done
            .outcome
            .errors
            .iter()
            .map(ToString::to_string)
            .collect();
        info!(
            "tray: {:?} finished, active profile \"{}\", {} error(s)",
            done.job,
            done.outcome.active,
            errors.len()
        );
        if done.job == Job::Quit {
            // L2 is done: suspended programs resumed, stopped ones stay.
            for error in &errors {
                warn!("tray: on quit: {error}");
            }
            self.exit_code.set(if errors.is_empty() {
                EXIT_OK
            } else {
                EXIT_ERRORS
            });
            window::exit();
            return;
        }
        self.status.finished(done.outcome.active, errors);
        self.status.config_error = lock(&self.source).problem().map(str::to_owned);
        if let Some(Job::Switch(next)) = self.controller.finished(&done.job) {
            self.status.switching = Some(next);
        }
    }
}

impl window::Handler for App {
    fn menu(&mut self) -> Vec<model::Item> {
        let names = {
            let mut source = lock(&self.source);
            source.refresh();
            self.status.config_error = source.problem().map(str::to_owned);
            source.profile_names()
        };
        // A command-line switch may have run meanwhile (until the pipe of L1
        // exists, it does not go through the tray).
        if self.controller.running().is_none()
            && !self.controller.is_quitting()
            && let Some(state) = self.store.peek()
        {
            self.status.active = state.active_profile;
        }
        self.redraw();
        self.menu = Menu::build(&names, &self.status);
        self.menu.items.clone()
    }

    fn command(&mut self, id: u16) {
        match self.menu.command(id).cloned() {
            Some(Command::Switch(target)) => {
                info!("tray: menu asks for \"{target}\"");
                if let Decision::Start(target) = self.controller.request(&target) {
                    self.status.switching = Some(target);
                }
                self.redraw();
            }
            Some(Command::Quit) => self.quit(),
            None => info!("tray: command {id} has no meaning; ignored"),
        }
    }

    fn wake(&mut self) {
        while let Ok(done) = self.done.try_recv() {
            let quit = done.job == Job::Quit;
            self.finished(done);
            if quit {
                return;
            }
        }
        self.redraw();
    }

    fn close(&mut self) {
        info!("tray: WM_CLOSE received");
        self.quit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l1_instance_name_is_per_session_and_per_redirected_data_dir() {
        let real = instance_name(Path::new("C:\\any"), false);
        assert_eq!(real, "Local\\kaltstellen-tray");
        let a = instance_name(Path::new("C:\\tmp\\a"), true);
        let a_upper = instance_name(Path::new("C:\\TMP\\A"), true);
        let b = instance_name(Path::new("C:\\tmp\\b"), true);
        assert!(a.starts_with("Local\\kaltstellen-tray-"));
        assert_eq!(a, a_upper);
        assert_ne!(a, b);
    }
}
