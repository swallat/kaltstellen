//! The tray app (`kaltstellen tray`, WP7): one instance per session with a
//! named pipe for the command line (L1), notification icon with three
//! states and a menu built from the configuration, switches on a worker
//! thread (W3), recovery at start (S3) and resume on Quit (L2).
//!
//! Everything here is safe code. The Win32 side lives in
//! [`crate::win32`]; this module decides what it shows.

pub mod client;
pub mod hotkeys;
pub mod model;
pub mod protocol;
pub mod server;
pub mod source;
pub mod triggers;
pub mod update;
pub mod worker;

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use log::{info, warn};

use crate::autostart;
use crate::config::{self, NEUTRAL};
use crate::error::{Code, Error};
use crate::exit;
use crate::paths;
use crate::process::{Access, ProcessInfo, Snapshot, Target};
use crate::settings;
use crate::state::Store;
use crate::win32::{console, instance::Instance, pipe, tray as window};
use model::{Command as MenuCommand, Decision, Guests, Icon, Menu, Status, SwitchError};
use protocol::{AutomationStatus, Body, Command, Finished, TrayStatus};
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

/// Starts `kaltstellen gui` as a process of its own, without a console
/// window (spec section 8). It inherits the environment, so a test tray
/// opens a test window.
fn open_window() {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let started = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .arg("gui")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(DETACHED_PROCESS)
            .spawn()
    });
    match started {
        Ok(child) => info!("tray: main window started (pid {})", child.id()),
        Err(err) => warn!("tray: cannot start the main window: {err}"),
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
    info!(
        "tray: started (pid {}), DPI awareness {}",
        std::process::id(),
        window::dpi_awareness()
    );

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
    let mut triggers_paused = false;
    if let Some(dir) = &config_dir {
        let (saved, problem) = settings::load(dir);
        if let Some(problem) = problem {
            warn!("tray: {problem}");
        }
        if let Some(last) = saved.last_profile {
            info!("tray: offering last profile \"{last}\"");
            status.offer = Some(last);
        }
        // U4, decided detail 7: the pause survives a restart.
        triggers_paused = saved.triggers_paused;
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
        let mut app = App {
            controller,
            done: done_rx,
            incoming: incoming_rx,
            status,
            source: Arc::clone(&source),
            config_file: config_file.clone(),
            store: store.clone(),
            exit_code: Rc::clone(&exit_code),
            shown: Some(initial_icon),
            running: Vec::new(),
            config_dir: config_dir.clone(),
            waiting: Vec::new(),
            menu: Menu::default(),
            hotkeys: hotkeys::Registrar::default(),
            updates: update::Updates::new(waker.clone()),
            waker: waker.clone(),
            autostart_answer: None,
            update_started: None,
            update_waiting: None,
            update_held: None,
            update_deferred: false,
            shown_menu: None,
            session_ending: false,
            automation: triggers::Automation::new(trigger_debounce(), triggers_paused),
            polling: false,
            images: HashMap::new(),
            back_to: None,
        };
        app.refresh_autostart();
        app.ask_autostart_once();
        app.refresh_update();
        app.schedule_update();
        app.sync_hotkeys();
        if app.automation.paused() {
            info!("tray: automatic switching is paused");
        }
        app.arm_poll();
        app.menu = Menu::build(&names, &app.status);
        Box::new(app)
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
    hotkeys: hotkeys::Registrar,
    /// UP1: the daily check and its child process.
    updates: update::Updates,
    /// Wakes this window thread from other threads.
    waker: window::Waker,
    /// UP1: the version whose automatic install was started, and the one
    /// already reported as waiting (each is logged once).
    update_started: Option<String>,
    update_waiting: Option<String>,
    /// The failed version the automatic install leaves alone, and until
    /// when; logged once.
    update_held: Option<(String, u64)>,
    /// UP1: this tray refused `quit_if_idle` to the running automatic
    /// install, which therefore gives up; it is started again later.
    update_deferred: bool,
    /// The menu as it was shown last; a chosen ID is resolved against it.
    shown_menu: Option<Menu>,
    /// Windows asked to end the session and has not cancelled it: no
    /// automatic install starts and `quit_if_idle` is refused.
    session_ending: bool,
    /// L8: the answer to the start-at-sign-in question, while it is open.
    autostart_answer: Option<Receiver<bool>>,
    /// U4: what the trigger programs ask for.
    automation: triggers::Automation,
    /// U4: the poll timer runs.
    polling: bool,
    /// U4: image paths of processes named like a trigger with a path, by
    /// PID and creation time, so each is opened once (A7); `None` when it
    /// could not be read.
    images: HashMap<(u32, u64), Option<PathBuf>>,
    /// U4: the profile `Back to <previous>` of the last automatic switch's
    /// notification returns to; cleared by the next request.
    back_to: Option<String>,
}

/// L8: answers the start-at-sign-in question without a dialog (`yes`,
/// `no`) or skips it (`skip`); for tests and managed installations.
const AUTOSTART_PROMPT_ENV: &str = "KALTSTELLEN_AUTOSTART_PROMPT";

/// L9: how often errors of the last switch are checked again while one
/// could still resolve itself.
const RECHECK: Duration = Duration::from_secs(30);

/// How long `WM_QUERYENDSESSION` waits for everything to be resumed. Windows
/// treats a window that takes about 5 s as not responding.
const END_SESSION_WAIT: Duration = Duration::from_secs(4);

/// U4, decided detail 3: how often the trigger programs are looked for, in
/// the range of 2 to 5 s. Each poll costs about 3.5 ms of CPU time (the
/// kernel builds the process list); at 5 s that is 0.06 % of one core,
/// within Q1 however it is read, where 3 s would be 0.12 % (spec U4).
const TRIGGER_POLL: Duration = Duration::from_secs(5);

/// Test only: replaces [`TRIGGER_POLL`] and the debounce of
/// [`triggers::DEBOUNCE`], in milliseconds, so the tests need not wait.
const TRIGGER_POLL_ENV: &str = "KALTSTELLEN_TRIGGER_POLL_MS";
const TRIGGER_DEBOUNCE_ENV: &str = "KALTSTELLEN_TRIGGER_DEBOUNCE_MS";

fn millis_from_env(name: &str) -> Option<Duration> {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
}

fn trigger_poll() -> Duration {
    millis_from_env(TRIGGER_POLL_ENV).map_or(TRIGGER_POLL, |d| d.max(Duration::from_millis(100)))
}

fn trigger_debounce() -> Duration {
    millis_from_env(TRIGGER_DEBOUNCE_ENV).unwrap_or(triggers::DEBOUNCE)
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
    /// L9: runs the re-check timer while an error could still resolve.
    fn arm_recheck(&self) {
        window::set_recheck(self.status.resolvable().then_some(RECHECK));
    }

    /// L9: drops the errors of the last switch whose cause is gone, from
    /// the handle-free snapshot (no process is opened, A7).
    fn recheck_errors(&mut self) {
        if !self.status.resolvable() {
            window::set_recheck(None);
            return;
        }
        let Ok(snapshot) = Snapshot::take() else {
            return;
        };
        let session = snapshot.own_session();
        let running = |exe: &str| {
            let exe = exe.to_lowercase();
            let exe = if exe.ends_with(".exe") {
                exe
            } else {
                format!("{exe}.exe")
            };
            snapshot
                .processes()
                .iter()
                .any(|p| Some(p.session_id) == session && p.name.to_lowercase() == exe)
        };
        for gone in self.status.resolve(&running) {
            info!("tray: error resolved by itself: {}", gone.text);
        }
        self.arm_recheck();
        self.redraw();
    }

    /// WP9: registers what the configuration asks for now (C4 re-read),
    /// releases what it no longer does.
    fn sync_hotkeys(&mut self) {
        let wanted = hotkeys::wanted(&lock(&self.source).config().profiles);
        self.hotkeys.sync(
            &wanted,
            &mut |b| {
                window::register_hotkey(
                    b.id,
                    hotkeys::modifier_flags(&b.hotkey),
                    b.hotkey.virtual_key(),
                )
            },
            &mut window::unregister_hotkey,
        );
        self.status.hotkeys_unavailable = self.hotkeys.unavailable.clone();
    }

    /// N1: after a profile was applied, the snapshot of unassigned programs.
    fn refresh_unassigned(&mut self) {
        self.status.unassigned = match paths::data_dir() {
            Some(dir) if !self.status.is_neutral() => {
                let config = lock(&self.source).config().clone();
                let list = crate::unassigned::refresh(&dir, &config, &self.status.active);
                info!(
                    "tray: {} unassigned program(s) in \"{}\"",
                    list.len(),
                    self.status.active
                );
                list.len()
            }
            _ => 0,
        };
    }

    /// Reads start at sign-in for the menu.
    /// L8: asks once, on the very first start, whether the tray should
    /// start with Windows. The question runs on a thread of its own, so
    /// recovery, the pipe and the menu go on meanwhile; the answer comes
    /// back through the waker. `KALTSTELLEN_AUTOSTART_PROMPT` = `yes`, `no`
    /// answers without a dialog, `skip` does not ask (tests).
    fn ask_autostart_once(&mut self) {
        let Some(dir) = self.config_dir.clone() else {
            return;
        };
        if settings::load(&dir).0.autostart_asked {
            return;
        }
        if self.status.autostart == Some(true) {
            // Already on: nothing to ask.
            let _ = settings::edit(&dir, |s| s.autostart_asked = true);
            return;
        }
        let preset = std::env::var(AUTOSTART_PROMPT_ENV).ok();
        let (tx, rx) = mpsc::channel();
        self.autostart_answer = Some(rx);
        let waker = self.waker.clone();
        match preset.as_deref().map(str::trim) {
            Some("skip") => {
                self.autostart_answer = None;
            }
            Some("yes") | Some("no") => {
                let _ = tx.send(preset.as_deref().map(str::trim) == Some("yes"));
                waker.wake();
            }
            _ => {
                info!("tray: asking whether to start at sign-in");
                std::thread::spawn(move || {
                    let yes = crate::win32::dialog::ask_yes_no(
                        "kaltstellen",
                        "Start kaltstellen automatically when you sign in to Windows?\n\n\
                         You can change this later with \"Start at sign-in\" in the tray menu.",
                    );
                    let _ = tx.send(yes);
                    waker.wake();
                });
            }
        }
    }

    /// The answer to the start-at-sign-in question arrived.
    fn autostart_answered(&mut self) {
        let Some(answer) = self
            .autostart_answer
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        else {
            return;
        };
        self.autostart_answer = None;
        info!(
            "tray: start at sign-in question answered {}",
            if answer { "yes" } else { "no" }
        );
        if let Some(dir) = &self.config_dir
            && let Err(err) = settings::edit(dir, |s| s.autostart_asked = true)
        {
            warn!("tray: {err}");
        }
        if answer && self.status.autostart != Some(true) {
            self.toggle_autostart();
        }
    }

    fn refresh_autostart(&mut self) {
        let at = autostart::Location::current();
        self.status.autostart = match std::env::current_exe()
            .map_err(|e| Error::new(Code::AutostartIo, e.to_string()))
            .and_then(|exe| autostart::state(&at, &exe))
        {
            Ok(state) => Some(state == autostart::State::On),
            Err(err) => {
                warn!("tray: {err}");
                None
            }
        };
    }

    fn toggle_autostart(&mut self) {
        let at = autostart::Location::current();
        let result = std::env::current_exe()
            .map_err(|e| Error::new(Code::AutostartIo, e.to_string()))
            .and_then(|exe| match autostart::state(&at, &exe)? {
                autostart::State::On => autostart::disable(&at).map(|_| false),
                _ => autostart::enable(&at, &exe).map(|()| true),
            });
        match result {
            Ok(on) => {
                info!("tray: start at sign-in {}", if on { "on" } else { "off" });
                self.status.autostart = Some(on);
            }
            Err(err) => warn!("tray: {err}"),
        }
    }

    /// UP1: mode and the last known version for the menu.
    fn refresh_update(&mut self) {
        let (mode, locked, offer) = update::menu_state(self.config_dir.as_deref());
        self.status.update_mode = mode;
        self.status.update_locked = locked;
        self.status.update = offer;
    }

    /// UP1: sets the timer for the next automatic check, or for the moment
    /// a version that could not be installed is tried again, whichever
    /// comes first.
    fn schedule_update(&mut self) {
        let check = self.updates.next_delay(self.config_dir.as_deref());
        let retry = self.retry_delay();
        if let Some(delay) = check {
            info!("tray: next update check in {} s", delay.as_secs());
        }
        if let Some(delay) = retry {
            info!("tray: failed update tried again in {} s", delay.as_secs());
        }
        if let Some(delay) = [check, retry].into_iter().flatten().min() {
            window::set_timer(delay);
        }
    }

    /// UP1: how long until a held-back version may be installed
    /// automatically again; `None` when nothing is held back, the mode does
    /// not install by itself, or this copy cannot install it.
    fn retry_delay(&self) -> Option<Duration> {
        if self.status.update_mode != crate::update::Mode::Auto {
            return None;
        }
        let offer = self.status.update.as_ref()?;
        if offer.blocked.is_some() {
            return None;
        }
        let retry_at = offer.retry_at?;
        Some(Duration::from_secs(
            retry_at.saturating_sub(crate::update::record::now()),
        ))
    }

    /// UP1: sets the update mode from the menu.
    fn set_update_mode(&mut self, mode: crate::update::Mode) {
        let Some(dir) = &self.config_dir else {
            return;
        };
        if let Err(err) = settings::edit(dir, |s| s.update = Some(mode.name().to_owned())) {
            warn!("tray: {err}");
            return;
        }
        info!("tray: update mode {}", mode.name());
        self.refresh_update();
        if mode.checks() {
            // Look now; the timer follows the result.
            self.updates.start_check();
        }
    }

    /// UP1: `neutral` is active, no switch runs or waits, the tray is not
    /// quitting and the session is not ending. Asked on this window thread,
    /// so the answer holds for the request that is being handled.
    fn idle_in_neutral(&self) -> bool {
        !self.session_ending
            && self.status.is_neutral()
            && self.controller.running().is_none()
            && self.controller.pending().is_none()
            && !self.controller.is_quitting()
    }

    /// UP1: installs a found update by itself in mode `auto`, but only
    /// while `neutral` is active and no switch runs or waits, so nothing is
    /// thawed or disturbed. Otherwise it waits: it is tried again when the
    /// tray returns to `neutral` and after every later check. The install
    /// itself asks again right before the tray quits (`quit_if_idle`).
    /// A version that could not be installed here is left alone for a
    /// while (1, 2, 4, then 7 days); the timer tries it again when the wait
    /// ends (`timer`), or the next return to `neutral` or check does.
    fn maybe_install_update(&mut self) {
        let Some(offer) = self.status.update.clone() else {
            return;
        };
        if let Some(retry_at) = offer
            .retry_at
            .filter(|&at| crate::update::record::now() < at)
        {
            if self.update_held.as_ref() != Some(&(offer.version.clone(), retry_at)) {
                info!(
                    "tray: update {} could not be installed here recently; it is tried again automatically in {} h",
                    offer.version,
                    retry_at
                        .saturating_sub(crate::update::record::now())
                        .div_ceil(3600)
                );
                self.update_held = Some((offer.version.clone(), retry_at));
            }
            return;
        }
        let now = crate::update::may_install_now(
            self.status.update_mode,
            offer.blocked.is_none(),
            self.status.is_neutral(),
            self.idle_in_neutral(),
        );
        if now {
            // An install that runs already is not started or logged again.
            if !self.updates.installing()
                && self.update_started.as_deref() != Some(offer.version.as_str())
            {
                info!("tray: installing update {} automatically", offer.version);
                self.update_started = Some(offer.version.clone());
                self.updates.install_unattended();
            }
        } else if self.status.update_mode == crate::update::Mode::Auto
            && offer.blocked.is_none()
            && self.update_waiting.as_deref() != Some(offer.version.as_str())
        {
            info!(
                "tray: update {} waits until neutral is active and no switch runs",
                offer.version
            );
            self.update_waiting = Some(offer.version.clone());
        }
    }

    /// Carries out the menu command of item `id`.
    fn run(&mut self, id: u16, command: Option<MenuCommand>) {
        match command {
            Some(MenuCommand::Switch(target)) => {
                if self.controller.is_quitting() {
                    return;
                }
                info!("tray: menu asks for \"{target}\"");
                self.submit(Job::switch(&target), None);
            }
            Some(MenuCommand::ToggleAutostart) => self.toggle_autostart(),
            Some(MenuCommand::ToggleAutomatic) => self.toggle_automatic(),
            Some(MenuCommand::OpenWindow) => open_window(),
            Some(MenuCommand::UpdateMode(mode)) => self.set_update_mode(mode),
            Some(MenuCommand::InstallUpdate) => {
                if self.updates.installing() {
                    info!("tray: an update is being installed already");
                } else {
                    update::open_install();
                }
            }
            Some(MenuCommand::Quit) => self.quit(),
            None => info!("tray: command {id} has no meaning; ignored"),
        }
    }

    /// Writes the offered last profile to `settings.json` (S5).
    fn save_offer(&self) {
        let Some(dir) = &self.config_dir else {
            return;
        };
        // Only the offer: the update mode (UP1) in the same file stays.
        let offer = self.status.offer.clone();
        if let Err(err) = settings::edit(dir, |s| s.last_profile = offer) {
            warn!("tray: {err}");
        }
    }

    /// The profile that is active once the running and the waiting job
    /// are done (U4 compares triggers against it).
    fn will_be_active(&self) -> String {
        self.controller
            .pending()
            .or(self.controller.running())
            .map_or_else(|| self.status.active.clone(), |j| j.leads_to().to_owned())
    }

    /// U4: for `kaltstellen status`; `None` while no profile has triggers.
    fn automation_status(&self) -> Option<AutomationStatus> {
        if !self.triggers_configured() {
            return None;
        }
        let view = self.automation.view(&self.will_be_active());
        let (trigger, returns_to) = match view.applied {
            Some((_, program, base)) => (Some(program), Some(base)),
            None => (None, None),
        };
        Some(AutomationStatus {
            paused: view.paused,
            held: view.held,
            trigger,
            returns_to,
            waiting: view.waiting.into_iter().map(|(p, _)| p).collect(),
        })
    }

    /// U4: whether some profile of the configuration in effect has
    /// triggers.
    fn triggers_configured(&self) -> bool {
        triggers::configured(&lock(&self.source).config().profiles)
    }

    /// U4: starts the poll when some profile has triggers and automatic
    /// switching is on, stops it while paused, quitting or while the
    /// session ends. When the triggers are gone from the file, the next
    /// poll ends them first and then stops itself. Updates the menu's view.
    fn arm_poll(&mut self) {
        let configured = self.triggers_configured();
        let blocked =
            self.automation.paused() || self.controller.is_quitting() || self.session_ending;
        if blocked && self.polling {
            window::set_poll(None);
            self.polling = false;
            info!("tray: trigger poll stopped");
        } else if !blocked && configured && !self.polling {
            let interval = trigger_poll();
            window::set_poll(Some(interval));
            self.polling = true;
            info!("tray: trigger poll every {} ms", interval.as_millis());
        }
        self.status.automation = configured.then(|| self.automation.view(&self.will_be_active()));
    }

    /// U4: one poll of the handle-free snapshot. Processes are opened only
    /// for triggers with a path, and then only those of the trigger's name,
    /// each once (A7).
    fn poll_triggers(&mut self) {
        if self.automation.paused() || self.controller.is_quitting() || self.session_ending {
            self.arm_poll();
            return;
        }
        // The file may have changed; an unchanged one is not parsed again
        // (C4).
        let profiles = {
            let mut source = lock(&self.source);
            source.refresh();
            self.status.config_error = source.problem().map(str::to_owned);
            source.config().profiles.clone()
        };
        let Ok(snapshot) = Snapshot::take() else {
            return;
        };
        let session = snapshot.own_session();
        if !self.images.is_empty() {
            self.images
                .retain(|(pid, created), _| snapshot.find(*pid, *created).is_some());
        }
        let images = &mut self.images;
        let mut image = |info: &ProcessInfo| -> Option<PathBuf> {
            images
                .entry((info.pid, info.created))
                .or_insert_with(|| {
                    match Target::open(info, Access::Query)
                        .and_then(|t| t.map(|t| t.image_path()).transpose())
                    {
                        Ok(path) => path,
                        Err(err) => {
                            info!("tray: trigger check: {err}");
                            None
                        }
                    }
                })
                .clone()
        };
        let seen = triggers::scan(&profiles, snapshot.processes(), session, &mut image);
        let active = self.will_be_active();
        let (action, log) = self.automation.poll(Instant::now(), &seen, &active);
        for line in log {
            info!("tray: {line}");
        }
        if let Some(triggers::Action::Switch {
            profile,
            previous,
            reason,
        }) = action
        {
            // The profile to return to may have left the file meanwhile.
            let known = profile.eq_ignore_ascii_case(NEUTRAL)
                || profiles
                    .iter()
                    .any(|p| p.name.eq_ignore_ascii_case(&profile));
            let target = if known {
                profile
            } else {
                info!("tray: \"{profile}\" is no longer in the configuration; going to neutral");
                NEUTRAL.to_owned()
            };
            info!("tray: trigger asks for \"{target}\" ({reason})");
            self.submit(Job::automatic(&target, &previous, &reason), None);
        }
        if seen.is_empty() && self.polling {
            // No profile has triggers any more: what ran has ended above.
            window::set_poll(None);
            self.polling = false;
            info!("tray: trigger poll stopped, no profile has triggers");
        }
        self.arm_poll();
        self.redraw();
    }

    /// U4: the menu item `Automatic switching`; remembered in
    /// `settings.json` (decided detail 7).
    fn toggle_automatic(&mut self) {
        let paused = !self.automation.paused();
        if let Some(dir) = &self.config_dir
            && let Err(err) = settings::edit(dir, |s| s.triggers_paused = paused)
        {
            warn!("tray: {err}");
        }
        self.automation.set_paused(paused);
        info!(
            "tray: automatic switching {}",
            if paused { "paused" } else { "on" }
        );
        self.arm_poll();
        self.redraw();
    }

    /// U4, decided detail 1: an automatic switch ended; tell the user and
    /// offer the way back.
    fn notify_automatic(&mut self, target: &str, automatic: &model::Automatic, errors: usize) {
        let mut text = format!("{}: switched to {target}", automatic.reason);
        match errors {
            0 => text.push('.'),
            1 => text.push_str(", with 1 error."),
            n => text.push_str(&format!(", with {n} errors.")),
        }
        text.push_str(&format!(" Click here: Back to {}", automatic.previous));
        info!("tray: notification: {text}");
        if !window::balloon(&format!("kaltstellen: {target}"), &text) {
            warn!("tray: the notification could not be shown");
        }
        self.back_to = Some(automatic.previous.clone());
    }

    fn submit(&mut self, job: Job, waiter: Option<Waiter>) {
        let joining = match &job {
            Job::Switch { guests, .. } => guests.list(),
            _ => Vec::new(),
        };
        let automatic = job.is_automatic();
        // A deliberate switch replaces the offer of the last session; an
        // automatic one does not (U4, `[assumed]`).
        if matches!(job, Job::Switch { .. }) && !automatic && self.status.offer.take().is_some() {
            self.save_offer();
        }
        // U4: the last notification's `Back to` means nothing any more.
        self.back_to = None;
        // U4, conflict rule 1: everything but a trigger is manual here
        // (menu, hotkey, command line, window, `Back to`) and holds
        // automation until the trigger situation changes.
        if !automatic && self.automation.manual() {
            info!(
                "tray: automatic switching waits after a manual switch until a trigger program starts or ends"
            );
        }
        let by = job.leads_to().to_owned();
        match self.controller.request(job) {
            Decision::Ignored => {
                self.automation.refused();
            }
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
                // Its caller is protected by it from its next snapshot on
                // (T16); what the job froze before stays frozen.
                if let Some(Job::Switch { guests, .. }) = self.controller.running() {
                    for guest in &joining {
                        info!(
                            "tray: caller {} joins the running switch and is protected from its next step",
                            guest.pid
                        );
                        guests.add(*guest);
                    }
                }
                if let Some(waiter) = waiter {
                    waiter.send(Body::Accepted { waiting: true });
                    self.running.push(waiter);
                }
            }
            Decision::Refused => {
                if let Some(waiter) = waiter {
                    waiter.send(Body::Rejected {
                        exit_code: exit::UNREACHABLE,
                        lines: vec![format!(
                            "error: {}: the tray is quitting or the session is ending",
                            Code::TrayUnreachable
                        )],
                    });
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
            self.hotkeys.clear(&mut window::unregister_hotkey);
            window::exit();
            return;
        }
        self.status.finished(
            done.outcome.active,
            done.outcome.errors.iter().map(SwitchError::of).collect(),
        );
        self.arm_recheck();
        if done.outcome.ran && matches!(done.job, Job::Switch { .. }) {
            self.refresh_unassigned();
        }
        self.status.config_error = lock(&self.source).problem().map(str::to_owned);
        let next = self.controller.finished(&done.job);
        if let Some(next) = &next {
            self.status.switching = Some(next.leads_to().to_owned());
            self.running = std::mem::take(&mut self.waiting);
        }
        // U4: an automatic switch is announced with its way back, unless
        // another request runs already.
        if let Job::Switch {
            automatic: Some(automatic),
            ..
        } = &done.job
            && done.outcome.ran
            && next.is_none()
        {
            let active = self.status.active.clone();
            self.notify_automatic(&active, automatic, errors.len());
        }
        self.arm_poll();
        self.sync_hotkeys();
        if self.status.is_neutral() {
            // UP1: an update that waited for neutral may go now.
            self.maybe_install_update();
        }
        // Items that depend on the result (acknowledge, offer) get their IDs.
        // A menu that is open keeps its own commands (`chosen`).
        self.menu = Menu::build(&self.profile_names(), &self.status);
    }

    /// A request from the command line.
    fn incoming(&mut self, incoming: Incoming) {
        let Incoming {
            command,
            reply,
            caller,
        } = incoming;
        if command == Command::QuitIfIdle && !self.idle_in_neutral() {
            // UP1: a profile was switched on while the automatic install
            // downloaded. It gives up; the tray starts it again later.
            info!("tray: the automatic update asked to quit, but the tray is busy; it waits");
            self.update_deferred = true;
            let _ = reply.send(Body::Rejected {
                exit_code: exit::ERRORS,
                lines: vec![format!(
                    "error: {}: a profile is active or a switch runs; the update waits for neutral",
                    Code::UpdateTray
                )],
            });
            return;
        }
        if matches!(command, Command::Quit | Command::QuitIfIdle) {
            // UP1: the updater quits the tray like the menu does (L2) and
            // gets the result once everything is resumed.
            info!("tray: command line asks to quit");
            if !self.controller.is_quitting() {
                self.quit();
            }
            let waiter = Waiter {
                reply,
                notes: Vec::new(),
            };
            if self.controller.running() == Some(&Job::Quit) {
                self.running.push(waiter);
            } else {
                self.waiting.push(waiter);
            }
            return;
        }
        let respond = |body| {
            let _ = reply.send(body);
        };
        if let Command::Status = command {
            respond(Body::Status(TrayStatus {
                state: self.store.peek().unwrap_or_default(),
                running: self.controller.running().map(ToString::to_string),
                waiting: self.controller.pending().map(ToString::to_string),
                automation: self.automation_status(),
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
                (
                    Job::Switch {
                        target,
                        force,
                        guests: Guests::of(caller),
                        automatic: None,
                    },
                    notes,
                )
            }
            Command::Recover { .. } => (Job::Recover, Vec::new()),
            Command::Status | Command::Quit | Command::QuitIfIdle => {
                unreachable!("answered above")
            }
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
        self.refresh_autostart();
        self.refresh_update();
        self.sync_hotkeys();
        self.arm_poll();
        self.redraw();
        self.menu = Menu::build(&names, &self.status);
        self.shown_menu = Some(self.menu.clone());
        self.menu.items.clone()
    }

    fn chosen(&mut self, id: u16) {
        // The menu that was shown: `finished` may have rebuilt `self.menu`
        // while it was open (a profile added to the file, the offer taken).
        let shown = self.shown_menu.take();
        if id == 0 {
            return;
        }
        let command = shown.and_then(|menu| menu.command(id).cloned());
        self.run(id, command);
    }

    fn command(&mut self, id: u16) {
        let command = self.menu.command(id).cloned();
        self.run(id, command);
    }

    fn wake(&mut self) {
        self.autostart_answered();
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
        if self.updates.check_ended() {
            self.refresh_update();
            update::log_result(self.status.update.as_ref());
            // A version whose earlier attempt ended in an error before the
            // tray was touched (offline, checksum) is tried again after a
            // later check, at most once a day. Failures after it was
            // verified are recorded in `update.json` and tried again after
            // their wait (`retry_delay`).
            self.update_started = None;
            self.maybe_install_update();
            self.schedule_update();
        }
        if self.updates.install_ended() {
            // The automatic install ended and this tray still runs: it gave
            // up (busy, or an error), or the new version did not start and
            // `update.json` says so now.
            self.refresh_update();
            if std::mem::take(&mut self.update_deferred) {
                self.update_started = None;
                self.update_waiting = None;
                self.maybe_install_update();
            }
            // A failure recorded just now holds the version back: the timer
            // brings the next try.
            self.schedule_update();
        }
        self.redraw();
    }

    fn recheck(&mut self) {
        self.recheck_errors();
    }

    fn poll(&mut self) {
        self.poll_triggers();
    }

    fn balloon_clicked(&mut self) {
        let Some(previous) = self.back_to.take() else {
            info!("tray: notification clicked; nothing to go back to");
            return;
        };
        if self.controller.is_quitting() {
            return;
        }
        // A manual switch: it holds automation like any other (U4).
        info!("tray: notification clicked; back to \"{previous}\"");
        self.submit(Job::switch(&previous), None);
    }

    fn timer(&mut self) {
        if self
            .updates
            .next_delay(self.config_dir.as_deref())
            .is_some_and(|delay| delay.is_zero())
        {
            // The check's result decides on an install and sets the timer.
            if self.updates.start_check() {
                return;
            }
            // It could not be started: again a day later, and a held-back
            // version is still tried when its wait ends.
            let retry = self.retry_delay();
            let check = crate::update::record::INTERVAL;
            window::set_timer(retry.map_or(check, |r| r.min(check)));
            return;
        }
        // While a check runs, its end decides with fresh data (`wake`), and
        // the install must not write `update.json` alongside it.
        if !self.updates.checking() {
            // UP1: a version that could not be installed has waited long
            // enough. `update.json` is read again first; the wait may also
            // have ended unseen, when the menu read it meanwhile.
            let due = self.retry_delay().is_some_and(|delay| delay.is_zero());
            let held = self.update_held.clone();
            self.refresh_update();
            let released = held.is_some_and(|(version, _)| {
                self.status
                    .update
                    .as_ref()
                    .is_some_and(|o| o.version == version && o.retry_at.is_none())
            });
            if due || released {
                self.update_started = None;
                self.update_held = None;
                self.maybe_install_update();
            }
        }
        // Otherwise update.json changed meanwhile (a check from the command
        // line): the timer follows it.
        self.schedule_update();
    }

    fn close(&mut self) {
        info!("tray: WM_CLOSE received");
        self.quit();
    }

    fn query_end_session(&mut self) -> bool {
        info!("tray: session is ending; resuming everything");
        self.session_ending = true;
        // The profile to offer next time: the active one, or the offer that
        // was not taken up yet. A profile a trigger applied is not offered,
        // but the one from before the trigger (U4, decided detail 8).
        self.arm_poll();
        if !self.status.is_neutral() {
            match self.automation.base_of(&self.status.active) {
                Some(base) if base.eq_ignore_ascii_case(NEUTRAL) => {}
                Some(base) => self.status.offer = Some(base.to_owned()),
                None => self.status.offer = Some(self.status.active.clone()),
            }
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
        self.session_ending = ending;
        if !ending {
            // The profile is off now; offer it again (S5).
            match &self.status.offer {
                Some(last) => info!("tray: end of session cancelled; offering \"{last}\" again"),
                None => info!("tray: end of session cancelled"),
            }
            // U4: triggers start afresh; one that still runs applies again
            // (`[assumed]`).
            self.automation.restart();
            self.arm_poll();
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

    fn hotkey(&mut self, id: i32) {
        let Some(profile) = self.hotkeys.profile(id).map(str::to_owned) else {
            info!("tray: hotkey {id} has no profile; ignored");
            return;
        };
        if self.controller.is_quitting() {
            return;
        }
        // The same queue as a menu click (W3).
        info!("tray: hotkey asks for \"{profile}\"");
        self.submit(Job::switch(&profile), None);
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
