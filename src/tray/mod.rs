//! The tray app (`kaltstellen tray`, WP7): one instance per session with a
//! named pipe for the command line (L1), notification icon with three
//! states and a menu built from the configuration, switches on a worker
//! thread (W3), recovery at start (S3) and resume on Quit (L2).
//!
//! Everything here is safe code. The Win32 side lives in
//! [`crate::win32`]; this module decides what it shows.

pub mod client;
pub mod model;
pub mod protocol;
pub mod server;
pub mod source;
pub mod worker;

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use log::{info, warn};

use crate::config::{self, NEUTRAL};
use crate::error::{Code, Error};
use crate::exit;
use crate::paths;
use crate::settings::{self, Settings};
use crate::state::Store;
use crate::win32::{console, instance::Instance, pipe, tray as window};
use model::{Command as MenuCommand, Decision, Icon, Menu, Status};
use protocol::{Body, Command, Finished, TrayStatus};
use server::Incoming;
use source::Source;
use worker::{Controller, Done, EngineRunner, Job};

/// FNV-1a of the data directory: stable across builds, unlike the standard
/// hasher.
fn dir_hash(data_dir: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data_dir.to_string_lossy().to_lowercase().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Mutex of the tray instance in the session namespace (L1). With a
/// redirected data directory (tests, portable use) the name includes that
/// directory, so such an instance does not block the real one.
pub fn instance_name(data_dir: &Path, redirected: bool) -> String {
    const BASE: &str = "Local\\kaltstellen-tray";
    if redirected {
        format!("{BASE}-{}", dir_hash(data_dir))
    } else {
        BASE.to_owned()
    }
}

/// Pipe of the tray (L1). Pipe names are machine-wide, so the name carries
/// the user's SID and the session, plus the same data-directory suffix as
/// the mutex.
pub fn pipe_name(sid: &str, session: u32, data_dir: &Path, redirected: bool) -> String {
    let mut name = format!("\\\\.\\pipe\\kaltstellen-{sid}-{session}");
    if redirected {
        name.push('-');
        name.push_str(&dir_hash(data_dir));
    }
    name
}

/// Where the tray of this session and data directory can be found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub mutex: String,
    pub pipe: String,
}

impl Endpoints {
    pub fn current() -> Result<Endpoints, Error> {
        let data = paths::data_dir()
            .ok_or_else(|| Error::new(Code::StateIo, "no data directory (LOCALAPPDATA unset)"))?;
        let redirected = std::env::var_os("KALTSTELLEN_DATA_DIR").is_some_and(|v| !v.is_empty());
        let sid = pipe::current_user_sid()?;
        Ok(Endpoints {
            mutex: instance_name(&data, redirected),
            pipe: pipe_name(&sid, pipe::current_session(), &data, redirected),
        })
    }
}

fn lock(source: &Mutex<Source>) -> MutexGuard<'_, Source> {
    source.lock().unwrap_or_else(|e| e.into_inner())
}

/// `kaltstellen tray`. Returns the process exit code.
pub fn main() -> u8 {
    let endpoints = match Endpoints::current() {
        Ok(endpoints) => endpoints,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::ERRORS;
        }
    };
    let data = paths::data_dir().unwrap_or_default();
    let _instance = match Instance::acquire(&endpoints.mutex) {
        Ok(Some(instance)) => instance,
        Ok(None) => {
            info!("tray: another instance runs in this session; exiting");
            eprintln!("kaltstellen tray is already running in this session");
            return exit::OK;
        }
        Err(err) => {
            warn!("{err}");
            eprintln!("error: {err}");
            return exit::ERRORS;
        }
    };
    if console::detach_if_own() {
        info!("tray: started without a terminal; console released");
    } else {
        eprintln!("kaltstellen tray is running; use Quit in its menu to end it");
    }
    info!("tray: started (pid {})", std::process::id());

    let store = Store::new(&data);
    let config_file = paths::config_file();
    let config_dir = paths::config_dir();
    let source = Arc::new(Mutex::new(Source::new(config_file.clone())));
    let mut status = Status {
        // What a crashed instance left; recovery below makes it neutral.
        active: store
            .peek()
            .map_or_else(|| NEUTRAL.to_owned(), |s| s.active_profile),
        switching: Some(NEUTRAL.to_owned()),
        ..Status::default()
    };
    // S5: the profile of the last session is offered, not applied.
    if let Some(dir) = &config_dir {
        let (saved, problem) = settings::load(dir);
        if let Some(problem) = problem {
            warn!("tray: {problem}");
        }
        if let Some(last) = saved.last_profile {
            info!("tray: offering last profile \"{last}\"");
            status.offer = Some(last);
        }
    }
    let listener = match pipe::Listener::bind(&endpoints.pipe) {
        Ok(listener) => {
            info!("tray: pipe ready");
            Some(listener)
        }
        Err(err) => {
            // The menu still works; the command line reports KS501.
            warn!("tray: {err}");
            status.pipe_error = Some(err.to_string());
            None
        }
    };
    let names = {
        let mut source = lock(&source);
        source.refresh();
        status.config_error = source.problem().map(str::to_owned);
        source.profile_names()
    };
    let exit_code = Rc::new(Cell::new(exit::OK));

    let initial_icon = status.icon();
    info!("tray: icon {}", icon_name(initial_icon));
    let result = window::run(initial_icon, &status.tooltip(), |waker| {
        let (done_tx, done_rx) = mpsc::channel();
        let job_waker = waker.clone();
        let mut controller = Controller::spawn(
            EngineRunner::new(store.clone(), Arc::clone(&source)),
            move |done| {
                if done_tx.send(done).is_ok() {
                    job_waker.wake();
                }
            },
        );
        let (incoming_tx, incoming_rx) = mpsc::channel();
        if let Some(listener) = listener {
            let pipe_waker = waker.clone();
            server::spawn(listener, incoming_tx, move || pipe_waker.wake());
        }
        // S3: nothing stays frozen after a crash; then `neutral`.
        controller.request(Job::Recover);
        let menu = Menu::build(&names, &status);
        Box::new(App {
            controller,
            done: done_rx,
            incoming: incoming_rx,
            status,
            source: Arc::clone(&source),
            config_file: config_file.clone(),
            store: store.clone(),
            menu,
            exit_code: Rc::clone(&exit_code),
            shown: Some(initial_icon),
            running: Vec::new(),
            config_dir: config_dir.clone(),
            waiting: Vec::new(),
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
            exit::ERRORS
        }
    }
}

/// A command-line client waiting for the result of a job.
struct Waiter {
    reply: Sender<Body>,
    /// Lines shown before the result (configuration warnings).
    notes: Vec<String>,
}

impl Waiter {
    /// Sending fails when the client went away (`--no-wait`); that is fine.
    fn send(&self, body: Body) {
        let _ = self.reply.send(body);
    }
}

struct App {
    controller: Controller,
    done: Receiver<Done>,
    incoming: Receiver<Incoming>,
    status: Status,
    source: Arc<Mutex<Source>>,
    config_file: Option<PathBuf>,
    store: Store,
    /// The menu last shown; `WM_COMMAND` IDs refer to it.
    menu: Menu,
    exit_code: Rc<Cell<u8>>,
    /// The icon state last drawn.
    shown: Option<Icon>,
    /// Clients of the running job, and of the waiting one (W3).
    running: Vec<Waiter>,
    waiting: Vec<Waiter>,
    /// Where `settings.json` lives.
    config_dir: Option<PathBuf>,
}

/// How long `WM_QUERYENDSESSION` waits for everything to be resumed. Windows
/// treats a window that takes about 5 s as not responding.
const END_SESSION_WAIT: Duration = Duration::from_secs(4);

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

    /// The waiting request was replaced by `by` (W3): its clients end with
    /// exit code 3.
    fn supersede(&mut self, superseded: Option<Job>, by: &str) {
        if superseded.is_some() {
            for waiter in self.waiting.drain(..) {
                waiter.send(Body::Superseded { by: by.to_owned() });
            }
        }
    }

    /// A request from the menu (`waiter` is `None`) or the pipe.
    /// Writes the offered last profile to `settings.json` (S5).
    fn save_offer(&self) {
        let Some(dir) = &self.config_dir else {
            return;
        };
        let saved = Settings {
            last_profile: self.status.offer.clone(),
        };
        if let Err(err) = settings::save(dir, &saved) {
            warn!("tray: {err}");
        }
    }

    fn submit(&mut self, job: Job, waiter: Option<Waiter>) {
        // A deliberate switch replaces the offer of the last session.
        if matches!(job, Job::Switch { .. }) && self.status.offer.take().is_some() {
            self.save_offer();
        }
        let by = job.leads_to().to_owned();
        match self.controller.request(job) {
            Decision::Start(job) => {
                self.status.switching = Some(job.leads_to().to_owned());
                if let Some(waiter) = waiter {
                    waiter.send(Body::Accepted { waiting: false });
                    self.running.push(waiter);
                }
            }
            Decision::Queued { superseded } => {
                self.supersede(superseded, &by);
                if let Some(waiter) = waiter {
                    waiter.send(Body::Accepted { waiting: true });
                    self.waiting.push(waiter);
                }
            }
            Decision::Dropped { superseded } => {
                self.supersede(superseded, &by);
                // The running job leads there: its result is this one's too.
                if let Some(waiter) = waiter {
                    waiter.send(Body::Accepted { waiting: true });
                    self.running.push(waiter);
                }
            }
        }
        self.redraw();
    }

    fn quit(&mut self) {
        info!("tray: quit requested");
        let superseded = self.controller.quit();
        self.supersede(superseded, "quit");
        if self.controller.running() != Some(&Job::Quit) {
            info!("tray: quitting after the running switch");
        }
        self.status.switching = None;
        self.redraw();
    }

    fn profile_names(&self) -> Vec<String> {
        lock(&self.source).profile_names()
    }

    fn finished(&mut self, done: Done) {
        let errors: Vec<String> = done
            .outcome
            .errors
            .iter()
            .map(ToString::to_string)
            .collect();
        info!(
            "tray: {} finished, active profile \"{}\", {} error(s)",
            done.job,
            done.outcome.active,
            errors.len()
        );
        let mut result = Finished::new(
            done.outcome.ran,
            &done.outcome.active,
            &done.outcome.events,
            &done.outcome.errors,
        );
        if result.exit_code == exit::UNKNOWN_PROFILE {
            result.profiles = self.profile_names();
        }
        for waiter in self.running.drain(..) {
            let mut mine = result.clone();
            mine.notes = waiter.notes.clone();
            waiter.send(Body::Done(mine));
        }
        if done.job == Job::Quit {
            // L2 is done: suspended programs resumed, stopped ones stay.
            for error in &errors {
                warn!("tray: on quit: {error}");
            }
            self.exit_code.set(exit::of(true, &done.outcome.errors));
            window::exit();
            return;
        }
        self.status.finished(done.outcome.active, errors);
        self.status.config_error = lock(&self.source).problem().map(str::to_owned);
        if let Some(next) = self.controller.finished(&done.job) {
            self.status.switching = Some(next.leads_to().to_owned());
            self.running = std::mem::take(&mut self.waiting);
        }
    }

    /// A request from the command line.
    fn incoming(&mut self, incoming: Incoming) {
        let Incoming { command, reply } = incoming;
        let respond = |body| {
            let _ = reply.send(body);
        };
        if let Command::Status = command {
            respond(Body::Status(TrayStatus {
                state: self.store.peek().unwrap_or_default(),
                running: self.controller.running().map(ToString::to_string),
                waiting: self.controller.pending().map(ToString::to_string),
            }));
            return;
        }
        if self.controller.is_quitting() {
            respond(Body::Rejected {
                exit_code: exit::UNREACHABLE,
                lines: vec![format!(
                    "error: {}: the tray is quitting",
                    Code::TrayUnreachable
                )],
            });
            return;
        }
        let (job, notes) = match command {
            Command::Switch { profile, force, .. } => {
                let target = profile.trim().to_owned();
                // The same checks and messages as in-process (E2 codes 2, 4).
                let (config, notes) = match config::for_switch(self.config_file.as_deref(), &target)
                {
                    Ok(found) => found,
                    Err(lines) => {
                        respond(Body::Rejected {
                            exit_code: exit::CONFIG_INVALID,
                            lines,
                        });
                        return;
                    }
                };
                if !target.eq_ignore_ascii_case(NEUTRAL) && config.profile(&target).is_none() {
                    let error = Error::new(
                        Code::UnknownProfile,
                        format!("no profile named \"{target}\""),
                    );
                    let mut result = Finished::new(false, NEUTRAL, &[], &[error]);
                    result.notes = notes;
                    result.profiles = config.profiles.iter().map(|p| p.name.clone()).collect();
                    respond(Body::Done(result));
                    return;
                }
                (Job::Switch { target, force }, notes)
            }
            Command::Recover { .. } => (Job::Recover, Vec::new()),
            Command::Status => unreachable!("answered above"),
        };
        info!("tray: command line asks for {job}");
        self.submit(job, Some(Waiter { reply, notes }));
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
            Some(MenuCommand::Switch(target)) => {
                if self.controller.is_quitting() {
                    return;
                }
                info!("tray: menu asks for \"{target}\"");
                self.submit(Job::switch(&target), None);
            }
            Some(MenuCommand::Quit) => self.quit(),
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
        while let Ok(incoming) = self.incoming.try_recv() {
            self.incoming(incoming);
        }
        self.redraw();
    }

    fn close(&mut self) {
        info!("tray: WM_CLOSE received");
        self.quit();
    }

    fn query_end_session(&mut self) -> bool {
        info!("tray: session is ending; resuming everything");
        // The profile to offer next time: the active one, or the offer that
        // was not taken up yet.
        if !self.status.is_neutral() {
            self.status.offer = Some(self.status.active.clone());
        }
        if self.status.offer.is_some() {
            self.save_offer();
        }
        let superseded = self.controller.end_session();
        self.supersede(superseded, "end of session");
        // Answer only once frozen programs run again: Windows asks them next.
        let deadline = Instant::now() + END_SESSION_WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.done.recv_timeout(left) {
                Ok(done) => {
                    let last = matches!(done.job, Job::EndSession | Job::Quit);
                    self.finished(done);
                    if last {
                        break;
                    }
                }
                Err(_) => {
                    warn!("tray: resuming did not finish in time; letting the session end");
                    break;
                }
            }
        }
        self.redraw();
        true
    }

    fn end_session(&mut self, ending: bool) {
        if !ending {
            // The profile is off now; offer it again (S5).
            match &self.status.offer {
                Some(last) => info!("tray: end of session cancelled; offering \"{last}\" again"),
                None => info!("tray: end of session cancelled"),
            }
            self.redraw();
            return;
        }
        // Stopped records are cleared at sign-out and shutdown, kept on Quit
        // and after a crash (S5).
        let cleared = self.store.lock().and_then(|_lock| {
            let (mut state, _) = self.store.load();
            let count = state.stopped.len();
            state.stopped.clear();
            self.store.save(&state).map(|()| count)
        });
        match cleared {
            Ok(count) => info!("tray: session ends; {count} stopped record(s) cleared"),
            Err(err) => warn!("tray: session ends; {err}"),
        }
    }

    fn power(&mut self, event: u32) {
        // Standby is left alone (S5): frozen programs stay frozen.
        let name = match event {
            0x4 => "suspend",
            0x7 => "resume after suspend",
            0x12 => "automatic resume",
            0xA => "power status change",
            0x8013 => "power setting change",
            _ => "other",
        };
        info!("tray: power event {name} (0x{event:x})");
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

    #[test]
    fn l1_pipe_name_carries_user_session_and_data_dir() {
        let real = pipe_name("S-1-5-21-1", 3, Path::new("C:\\any"), false);
        assert_eq!(real, "\\\\.\\pipe\\kaltstellen-S-1-5-21-1-3");
        let test = pipe_name("S-1-5-21-1", 3, Path::new("C:\\tmp\\a"), true);
        let mutex = instance_name(Path::new("C:\\tmp\\a"), true);
        assert!(test.ends_with(mutex.rsplit('-').next().unwrap()));
        assert_ne!(
            pipe_name("S-1-5-21-2", 3, Path::new("C:\\any"), false),
            real
        );
        assert_ne!(
            pipe_name("S-1-5-21-1", 4, Path::new("C:\\any"), false),
            real
        );
    }
}
